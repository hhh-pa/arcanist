//! `arc set-config` — write arc configuration settings.
//!
//! PHP reference: `src/workflow/ArcanistSetConfigWorkflow.php`, with value
//! coercion from `src/configuration/ArcanistSettings.php` (`willWriteValue`)
//! and storage paths from `src/configuration/ArcanistConfigurationManager.php`
//! (`~/.arcrc`) plus
//! `src/workingcopyidentity/ArcanistWorkingCopyIdentity.php`
//! (`arc/config` under `.git`, `.hg`, or `.svn`).
//!
//! User writes preserve the rest of `~/.arcrc` (hosts, aliases). An empty
//! value deletes the key. Local writes resolve the enclosing git, hg, or
//! svn working copy (T1153 CORE-32).
//!
//! Console output goes through the shared [`arc_rs::terminal`] API: the
//! unknown-key warning is a true `tsprintf()` site (escaped `%s`
//! conversions), `pht()` sites use `pht`, and markup-free `sprintf` sites
//! use plain `format!`.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::terminal::{PhtArg, TVal};
use arc_rs::{
    conduit::{self, EnvelopeKind},
    config, get_config, terminal,
};

/// Subcommand name, matching `arc set-config`.
pub const NAME: &str = "set-config";

const USAGE: &str = "usage: arc set-config [--local] [--config K=V] [--config-file FILE] [--arcrc-file FILE] [--] <name> <value>\n\
\n\
Write an arc configuration option. Writes user configuration (~/.arcrc)\n\
by default, or the working copy's local configuration (an arc directory\n\
under .git, .hg, or .svn, as appropriate) with --local. An empty value\n\
deletes the key.\n\
\n\
Options:\n\
  --local         Write local (working-copy) config instead of user config.\n\
  --config K=V    Accepted like other commands; writes ignore it.\n\
  --config-file F Accepted like other commands; writes ignore it.\n\
  --arcrc-file F  Write user config to F instead of ~/.arcrc.\n\
  -h, --help      Show this help.\n";

/// Setting type for coercion, mirroring the `ArcanistSettings` table
/// (`string`, `bool`, `list`, `wild`, `aliases`). Unknown keys are `wild`.
#[must_use]
pub fn setting_type(key: &str) -> &'static str {
    get_config::setting_type(key)
}

/// True when `key` is a setting arc recognizes.
#[must_use]
pub fn is_known(key: &str) -> bool {
    setting_type(key) != "wild"
}

/// Coerce a CLI string into a config value, mirroring
/// `ArcanistSettings::willWriteValue()`.
///
/// `list` accepts any JSON array or object: PHP decodes with `assoc=true`,
/// so objects arrive as (accepted) arrays. `bool` folds the usual
/// spellings; note PHP maps `''` to false too, though the CLI deletes on
/// empty values before coercion ever runs.
pub fn coerce_value(key: &str, raw: &str) -> Result<serde_json::Value, String> {
    match setting_type(key) {
        "bool" => {
            let lower = raw.to_lowercase();
            if ["false", "no", "off", "", "0"].contains(&lower.as_str()) {
                Ok(serde_json::Value::Bool(false))
            } else if ["true", "yes", "on", "1"].contains(&lower.as_str()) {
                Ok(serde_json::Value::Bool(true))
            } else {
                Err(terminal::pht(
                    "Type of setting '%s' must be boolean, like 'true' or 'false'.",
                    &[PhtArg::Text(key)],
                ))
            }
        }
        "list" => {
            let err = || {
                terminal::pht(
                    "Type of setting '%s' must be list. You can specify a list in JSON, like: %s",
                    &[
                        PhtArg::Text(key),
                        PhtArg::Text("[\"apple\", \"banana\", \"cherry\"]"),
                    ],
                )
            };
            let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|_| err())?;
            if parsed.is_array() || parsed.is_object() {
                Ok(parsed)
            } else {
                Err(err())
            }
        }
        "aliases" => Err(terminal::pht(
            "Use \"arc alias\" to configure aliases, not \"arc set-config\".",
            &[],
        )),
        _ => Ok(serde_json::Value::String(raw.to_string())),
    }
}

/// Format a value for display, mirroring
/// `ArcanistSettings::formatConfigValueForDisplay()`.
#[must_use]
pub fn format_value(value: &serde_json::Value) -> String {
    get_config::format_value("", value)
}

/// Parse `--config key=value` pairs into a runtime map, mirroring
/// `ArcanistConfigurationManager::applyRuntimeArcConfig()`: split on the
/// first `=`, coerce with `willWriteValue()`, and let later pairs win.
/// Unknown keys stay `wild`, like stock PHP.
pub fn parse_runtime_config(
    pairs: &[String],
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut map = serde_json::Map::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(terminal::pht(
                "Argument was '%s', but must be '%s'. For example, %s",
                &[
                    PhtArg::Text(pair),
                    PhtArg::Text("name=value"),
                    PhtArg::Text("history.immutable=true"),
                ],
            ));
        };
        map.insert(key.to_string(), coerce_value(key, value)?);
    }
    Ok(map)
}

/// The `(was ...)` operand: PHP formats the old value (possibly null)
/// before comparing, so a missing key renders as `null`.
fn format_old(old: &Option<serde_json::Value>) -> String {
    match old {
        Some(previous) => format_value(previous),
        None => "null".to_string(),
    }
}

/// Warning text for an unrecognized key, or `None` when known, with the
/// process ANSI setting. Like PHP, unknown keys warn-but-proceed.
#[must_use]
pub fn warn_if_unknown(key: &str) -> Option<String> {
    warn_if_unknown_ansi(key, terminal::ansi_enabled())
}

/// Warning text with an explicit ANSI setting:
/// `writeErr('%s', tsprintf("**%s:** %s\\n", pht('Warning'),
/// pht('The configuration key "%s" is not recognized by arc. It may be
/// misspelled or out of date.', $key)))`. This is a true `tsprintf()`
/// site, so both conversions are escaping `%s`.
#[must_use]
pub fn warn_if_unknown_ansi(key: &str, ansi: bool) -> Option<String> {
    if is_known(key) {
        return None;
    }
    let warning = terminal::pht("Warning", &[]);
    let detail = terminal::pht(
        "The configuration key \"%s\" is not recognized by arc. It may be misspelled or out of date.",
        &[PhtArg::Text(key)],
    );
    let text = terminal::tsprintf_ansi(
        "**%s:** %s\n",
        &[TVal::Text(&warning), TVal::Text(&detail)],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    Some(text.trim_end_matches('\n').to_string())
}

/// Read a whole `~/.arcrc` document; a missing file yields `{}`. The parse
/// error mirrors `readUserConfigurationFile()`, including its literal
/// `~/.arcrc`.
pub fn read_user_doc(path: &Path) -> Result<serde_json::Value, String> {
    if !path.is_file() {
        return Ok(serde_json::json!({}));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        terminal::pht(
            "Your '%s' file is not a valid JSON file.",
            &[PhtArg::Text("~/.arcrc")],
        )
    })?;
    if doc.is_object() {
        Ok(doc)
    } else {
        Err(format!("'{}' is not a JSON object", path.display()))
    }
}

fn write_json_file(path: &Path, doc: &serde_json::Value, chmod_600: bool) -> Result<(), String> {
    let mut text =
        serde_json::to_string_pretty(doc).map_err(|e| format!("cannot encode JSON: {e}"))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("cannot write '{}': {e}", path.display()))?;
    // The user file may hold tokens; PHP hardens it to 0600 on unix (the
    // local file gets no chmod). Best effort: a chmod failure must not
    // fail the write itself.
    #[cfg(unix)]
    if chmod_600 && let Ok(metadata) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions).ok();
    }
    #[cfg(not(unix))]
    let _ = chmod_600;
    Ok(())
}

/// Write a whole `~/.arcrc` document.
pub fn write_user_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    write_json_file(path, doc, true)
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

/// Local config file for a repository root of flavor `kind`:
/// `<root>/.(git|hg|svn)/arc/config`, mirroring
/// `ArcanistWorkingCopyIdentity::$localMetaDir` (T1153 CORE-32).
#[must_use]
pub fn local_config_path_for(kind: get_config::VcsKind, repo_root: &Path) -> PathBuf {
    let meta = match kind {
        get_config::VcsKind::Git => ".git",
        get_config::VcsKind::Hg => ".hg",
        get_config::VcsKind::Svn => ".svn",
    };
    repo_root.join(meta).join("arc").join("config")
}

/// Local config file for a git repository root: `<root>/.git/arc/config`.
#[must_use]
pub fn local_config_path(repo_root: &Path) -> PathBuf {
    local_config_path_for(get_config::VcsKind::Git, repo_root)
}

/// Read a local config file; a missing file yields an empty config. The
/// parse error mirrors `readLocalArcConfig()`.
pub fn read_local_file(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if !path.is_file() {
        return Ok(serde_json::Map::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        let name = path.display().to_string();
        terminal::pht("Failed to parse '%s' as JSON.", &[PhtArg::Text(&name)])
    })?;
    doc.as_object()
        .cloned()
        .ok_or_else(|| format!("'{}' is not a JSON object", path.display()))
}

/// Write a local config file, creating the `arc/` dir as needed.
pub fn write_local_file(
    path: &Path,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create '{}': {e}", parent.display()))?;
    }
    write_json_file(path, &serde_json::Value::Object(map.clone()), false)
}

/// Set or delete `key` in the local config of the `kind` repository
/// rooted at `repo_root`. Returns the message to print.
pub fn apply_local_at_kind(
    kind: get_config::VcsKind,
    repo_root: &Path,
    key: &str,
    raw: &str,
) -> Result<String, String> {
    let path = local_config_path_for(kind, repo_root);
    let mut map = read_local_file(&path)?;
    let message = apply_to_map(&mut map, key, raw, "local")?;
    write_local_file(&path, &map)?;
    Ok(message)
}

/// Set or delete `key` in the local config of the git repository rooted
/// at `repo_root`. Returns the message to print.
pub fn apply_local_at(repo_root: &Path, key: &str, raw: &str) -> Result<String, String> {
    apply_local_at_kind(get_config::VcsKind::Git, repo_root, key, raw)
}

/// Set or delete `key` in the local config of the working copy
/// containing `dir`, for git, hg, or svn alike (T1153 CORE-32: the VCS
/// root comes from [`get_config::find_vcs_root`], pure filesystem like
/// PHP's `$try` walk — no binaries needed). Fails outside any working
/// copy with the stock `scripts/arcanist.php` usage error (`--local`
/// requires a repository).
pub fn apply_local(dir: &Path, key: &str, raw: &str) -> Result<String, String> {
    let Some((kind, root)) = get_config::find_vcs_root(dir) else {
        return Err(terminal::pht(
            "This command must be run in a Git, Mercurial or Subversion working copy.",
            &[],
        ));
    };
    apply_local_at_kind(kind, &root, key, raw)
}

fn apply_to_map(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    raw: &str,
    which: &str,
) -> Result<String, String> {
    let old = map.get(key).cloned();
    // `pht('Deleted key "%s" from %s config.', ...)` vs
    // `pht('Deleted key "%s" from %s config (was %s).', ...)`: PHP formats
    // `$old` before the null check, so the `(was ...)` form always wins --
    // even for keys that never existed (`(was null)`). Same quirk for the
    // `pht('Set key ...')` pair. Both print through markup-free
    // `tsprintf("%s\\n", $message)`, so plain `pht()` is exact.
    if raw.is_empty() {
        map.remove(key);
        let was = format_old(&old);
        Ok(terminal::pht(
            "Deleted key \"%s\" from %s config (was %s).",
            &[PhtArg::Text(key), PhtArg::Text(which), PhtArg::Text(&was)],
        ))
    } else {
        let value = coerce_value(key, raw)?;
        map.insert(key.to_string(), value.clone());
        let display = format_value(&value);
        let was = format_old(&old);
        Ok(terminal::pht(
            "Set key \"%s\" = %s in %s config (was %s).",
            &[
                PhtArg::Text(key),
                PhtArg::Text(&display),
                PhtArg::Text(which),
                PhtArg::Text(&was),
            ],
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    local: bool,
    key: String,
    value: String,
    config: Vec<String>,
    config_files: Vec<String>,
    arcrc_file: Option<String>,
}

fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        local: false,
        key: String::new(),
        value: String::new(),
        config: Vec::new(),
        config_files: Vec::new(),
        arcrc_file: None,
    };
    let mut positional = Vec::new();
    let mut end_of_flags = false;
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        index += 1;
        if end_of_flags {
            positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            end_of_flags = true;
            continue;
        }
        if arg == "--local" {
            options.local = true;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            return Ok(None);
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg.as_str(), None),
        };
        if crate::get_config::take_runtime_flag(
            &mut options.config,
            &mut options.config_files,
            &mut options.arcrc_file,
            name,
            inline,
            argv,
            &mut index,
        )? {
            continue;
        }
        if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        }
        positional.push(arg.clone());
    }
    if positional.len() != 2 {
        return Err(terminal::pht("Specify a key and a value.", &[]));
    }
    options.key = positional[0].clone();
    options.value = positional[1].clone();
    Ok(Some(options))
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        // T1153 R4 (CORE-49): `set-config` is a subprocess
        // workflow: usage errors render `Usage Exception:` with no
        // usage dump (`scripts/arcanist.php:432-444`).
        Err(message) => return conduit::report(EnvelopeKind::Usage, &message),
    };
    // `--config` validates like everywhere else, then stays inert: stock
    // PHP consumes it before dispatch and the write ignores runtime
    // sources. `--config-file` is likewise accepted and ignored.
    if let Err(message) = parse_runtime_config(&options.config) {
        eprintln!("arc {NAME}: {message}");
        return ExitCode::FAILURE;
    }
    if let Some(warning) = warn_if_unknown(&options.key) {
        eprintln!("{warning}");
    }
    let result = if options.local {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        apply_local(&cwd, &options.key, &options.value)
    } else {
        match get_config::resolve_user_file(
            config::arcrc_path(),
            options.arcrc_file.as_deref().map(Path::new),
        ) {
            Ok(Some(path)) => apply_user(&path, &options.key, &options.value),
            Ok(None) => Err("cannot locate ~/.arcrc (HOME/USERPROFILE is not set)".to_string()),
            Err(message) => Err(message),
        }
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        // T1153 R4 (CORE-49): write failures are generic exceptions
        // under the red banner (`:449-468`).
        Err(message) => conduit::report(EnvelopeKind::Exception, &message),
    }
}

#[cfg(test)]
#[path = "set_config_tests.rs"]
mod set_config_tests;
