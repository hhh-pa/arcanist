//! `arc-rs set-config` — write arc configuration settings.
//!
//! PHP reference: `src/workflow/ArcanistSetConfigWorkflow.php`, with value
//! coercion from `src/configuration/ArcanistSettings.php` (`willWriteValue`)
//! and storage paths from `src/configuration/ArcanistConfigurationManager.php`
//! (`~/.arcrc`) plus
//! `src/workingcopyidentity/ArcanistWorkingCopyIdentity.php`
//! (`.git/arc/config`).
//!
//! User writes preserve the rest of `~/.arcrc` (hosts, aliases). An empty
//! value deletes the key. Local writes only support git working copies
//! (see T1128).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{config, git};

/// Subcommand name, matching `arc set-config`.
pub const NAME: &str = "set-config";

const USAGE: &str = "usage: arc-rs set-config [--local] [--] <name> <value>\n\
\n\
Write an arc configuration option. Writes user configuration (~/.arcrc)\n\
by default, or the working copy's local configuration (.git/arc/config)\n\
with --local. An empty value deletes the key.\n\
\n\
Options:\n\
  --local         Write local (working-copy) config instead of user config.\n\
  -h, --help      Show this help.\n";

/// Setting type for coercion, mirroring the `ArcanistSettings` table
/// (`string`, `bool`, `list`, `wild`, `aliases`). Unknown keys are `wild`.
#[must_use]
pub fn setting_type(key: &str) -> &'static str {
    match key {
        "aliases" => "aliases",
        "arc.autostash" | "history.immutable" => "bool",
        "load" | "events.listeners" => "list",
        "arc.land.onto.default"
        | "base"
        | "browser"
        | "default"
        | "editor"
        | "https.cabundle"
        | "lint.engine"
        | "phabricator.uri"
        | "repository.callsign"
        | "unit.engine" => "string",
        _ => "wild",
    }
}

/// True when `key` is a setting arc recognizes.
#[must_use]
pub fn is_known(key: &str) -> bool {
    setting_type(key) != "wild"
}

/// Coerce a CLI string into a config value, mirroring
/// `ArcanistSettings::willWriteValue()`.
pub fn coerce_value(key: &str, raw: &str) -> Result<serde_json::Value, String> {
    match setting_type(key) {
        "bool" => {
            let lower = raw.to_lowercase();
            if ["false", "no", "off", "", "0"].contains(&lower.as_str()) {
                Ok(serde_json::Value::Bool(false))
            } else if ["true", "yes", "on", "1"].contains(&lower.as_str()) {
                Ok(serde_json::Value::Bool(true))
            } else {
                Err(format!(
                    "Type of setting '{key}' must be boolean, like 'true' or 'false'."
                ))
            }
        }
        "list" => {
            let err = || {
                format!(
                    "Type of setting '{key}' must be list. You can specify a list in JSON, like: {}",
                    "[\"apple\", \"banana\", \"cherry\"]"
                )
            };
            let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|_| err())?;
            if parsed.is_array() {
                Ok(parsed)
            } else {
                Err(err())
            }
        }
        "aliases" => {
            Err("Use \"arc alias\" to configure aliases, not \"arc set-config\".".to_string())
        }
        _ => Ok(serde_json::Value::String(raw.to_string())),
    }
}

/// Format a value for display, mirroring
/// `ArcanistSettings::formatConfigValueForDisplay()`.
#[must_use]
pub fn format_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(true) => "true".to_string(),
        serde_json::Value::Bool(false) => "false".to_string(),
        serde_json::Value::String(s) => format!("\"{s}\""),
        array @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
            serde_json::to_string(array).unwrap_or_else(|_| "null".to_string())
        }
        serde_json::Value::Number(n) => n.to_string(),
    }
}

/// Warning text for an unrecognized key, or `None` when known. Like PHP,
/// unknown keys still warn-but-proceed.
#[must_use]
pub fn warn_if_unknown(key: &str) -> Option<String> {
    if is_known(key) {
        None
    } else {
        Some(format!(
            "Warning: the configuration key \"{key}\" is not recognized by arc. It may be misspelled or out of date."
        ))
    }
}

/// Read a whole `~/.arcrc` document; a missing file yields `{}`.
pub fn read_user_doc(path: &Path) -> Result<serde_json::Value, String> {
    if !path.is_file() {
        return Ok(serde_json::json!({}));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("'{}' is not a valid JSON file: {e}", path.display()))?;
    if doc.is_object() {
        Ok(doc)
    } else {
        Err(format!("'{}' is not a JSON object", path.display()))
    }
}

fn write_json_file(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    let mut text =
        serde_json::to_string_pretty(doc).map_err(|e| format!("cannot encode JSON: {e}"))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("cannot write '{}': {e}", path.display()))?;
    // The user file may hold tokens; keep PHP's 0600 hardening on unix.
    // Best effort: a chmod failure must not fail the write itself.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(path, permissions).ok();
        }
    }
    Ok(())
}

/// Write a whole `~/.arcrc` document.
pub fn write_user_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    write_json_file(path, doc)
}

/// Set or delete `key` in the user config, preserving the rest of the
/// file. Returns the message to print. An empty `raw` deletes the key.
pub fn apply_user(user_file: &Path, key: &str, raw: &str) -> Result<String, String> {
    let mut doc = read_user_doc(user_file)?;
    let message = {
        let obj = doc.as_object_mut().expect("read_user_doc object");
        let config_value = obj
            .entry("config".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let config = config_value.as_object_mut().ok_or_else(|| {
            format!(
                "'{}' has a non-object \"config\" section",
                user_file.display()
            )
        })?;
        apply_to_map(config, key, raw, "user")?
    };
    write_user_doc(user_file, &doc)?;
    Ok(message)
}

/// Local config file for a repository root: `<root>/.git/arc/config`.
#[must_use]
pub fn local_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".git").join("arc").join("config")
}

/// Read a local config file; a missing file yields an empty config.
pub fn read_local_file(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if !path.is_file() {
        return Ok(serde_json::Map::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse '{}' as JSON: {e}", path.display()))?;
    doc.as_object()
        .cloned()
        .ok_or_else(|| format!("'{}' is not a JSON object", path.display()))
}

/// Write a local config file, creating `.git/arc/` as needed.
pub fn write_local_file(
    path: &Path,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create '{}': {e}", parent.display()))?;
    }
    write_json_file(path, &serde_json::Value::Object(map.clone()))
}

/// Set or delete `key` in the local config of the repository rooted at
/// `repo_root`. Returns the message to print.
pub fn apply_local_at(repo_root: &Path, key: &str, raw: &str) -> Result<String, String> {
    let path = local_config_path(repo_root);
    let mut map = read_local_file(&path)?;
    let message = apply_to_map(&mut map, key, raw, "local")?;
    write_local_file(&path, &map)?;
    Ok(message)
}

/// Set or delete `key` in the local config of the repository containing
/// `dir`. Fails outside a git working copy.
pub fn apply_local(dir: &Path, key: &str, raw: &str) -> Result<String, String> {
    if !git::is_inside_work_tree(dir) {
        return Err("--local requires a git working copy".to_string());
    }
    let top = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])?;
    apply_local_at(Path::new(&top), key, raw)
}

fn apply_to_map(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    raw: &str,
    which: &str,
) -> Result<String, String> {
    let old = map.get(key).cloned();
    if raw.is_empty() {
        map.remove(key);
        match old {
            Some(previous) => Ok(format!(
                "Deleted key \"{key}\" from {which} config (was {}).",
                format_value(&previous)
            )),
            None => Ok(format!("Deleted key \"{key}\" from {which} config.")),
        }
    } else {
        let value = coerce_value(key, raw)?;
        map.insert(key.to_string(), value.clone());
        let display = format_value(&value);
        match old {
            Some(previous) => Ok(format!(
                "Set key \"{key}\" = {display} in {which} config (was {}).",
                format_value(&previous)
            )),
            None => Ok(format!("Set key \"{key}\" = {display} in {which} config.")),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    local: bool,
    key: String,
    value: String,
}

fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut local = false;
    let mut positional = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            positional.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--local" {
            local = true;
        } else if arg == "-h" || arg == "--help" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
    }
    if positional.len() != 2 {
        return Err("Specify a key and a value.".to_string());
    }
    Ok(Some(Options {
        local,
        key: positional[0].clone(),
        value: positional[1].clone(),
    }))
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(warning) = warn_if_unknown(&options.key) {
        eprintln!("{warning}");
    }
    let result = if options.local {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        apply_local(&cwd, &options.key, &options.value)
    } else {
        match config::arcrc_path() {
            Some(path) => apply_user(&path, &options.key, &options.value),
            None => Err("cannot locate ~/.arcrc (HOME/USERPROFILE is not set)".to_string()),
        }
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "set_config_tests.rs"]
mod set_config_tests;
