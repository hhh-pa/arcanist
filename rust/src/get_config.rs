//! `arc get-config` — read arc configuration settings.
//!
//! PHP reference: `src/workflow/ArcanistGetConfigWorkflow.php`, with setting
//! metadata from `src/configuration/ArcanistSettings.php` (no
//! `ArcanistSetting` subclasses exist in-tree, so the builtin table below is
//! complete) and source precedence from
//! `src/configuration/ArcanistConfigurationManager.php`:
//! local > project > user > system > default.
//!
//! `--verbose` prints every source's value, otherwise only the winning
//! value and its source are shown. `--show-source` is an arc-rs alias for
//! `--verbose` (PHP only accepts `--verbose`).
//!
//! Console output goes through the shared [`arc_rs::terminal`] API:
//! `writeOut()` sites (interpret pattern, substitute raw) use
//! `tsprintf_ansi` with the `%R` (raw string) conversion where the pattern
//! carries markup, `pht()` sites use `pht`, help wrapping uses
//! `console_wrap`, and markup-free `sprintf` sites use plain `format!`.
//!
//! Source precedence (`runtime --config` > local > project > user >
//! system > default) is shared by every command through [`MergedConfig`]:
//! `land` reads `arc.land.*`, `unit` reads `unit.engine`, and conduit-URI
//! discovery reads `phabricator.uri` (falling back to `default`) through
//! [`load_merged`] plus [`merged_value`] or [`checked_value`]. The merge
//! ports `ArcanistConfigurationManager::getConfigFromAllSources()` (the
//! flow every classic workflow runs at ceb) with the
//! `ArcanistConfigurationEngine` source list folded in: `--config-file`
//! shadows system+user like the engine, `--arcrc-file` redirects the user
//! file like the manager, and `--config` pairs apply classic
//! `applyRuntimeArcConfig()` semantics (last wins, unknown keys allowed).
//! Display (`render`, `all_keys`) covers the five static sources only:
//! stock `get-config` never shows runtime values.

// A crate cannot name itself in `use` (E0432), so alias the current crate
// to keep the shared-helper paths (`arc_rs::git`, `arc_rs::config`)
// working both once this module is wired into `lib.rs` and when it is
// included standalone by an integration test.
extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::terminal::{PhtArg, TVal};
use arc_rs::{config, set_config, terminal};

/// Subcommand name, matching `arc get-config`.
pub const NAME: &str = "get-config";

/// T1153 B21/B23: `--help` renders the generated workflow help
/// (`src/workflow/ArcanistGetConfigWorkflow.php`,
/// `getCommandSynopses()`/`getCommandHelp()`), not a short custom
/// usage string. T1153 RB-D5: the renderer is the shared `crate::help`
/// module; this file keeps only the data. The `--show-source` alias
/// and `--config`/`--config-file`/`--arcrc-file` extensions stay
/// functional but undocumented in `--help`, like PHP. Classic
/// workflows never require `--` (B21 exempt: PHP runs them via the
/// `scripts/arcanist.php` re-exec).
const HELP_EXAMPLES: &str = "**get-config**";
const HELP_SYNOPSIS: &str = "      **get-config** [__options__] -- [__name__ ...]";
const HELP_TEXT: &str = "          Supports: cli\n          Reads an arc configuration option. With no argument, reads all\n          options.\n\n          With __--verbose__, shows detailed information about one or more\n          options.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[(
    "verbose",
    None,
    None,
    "Show detailed information about options.",
)];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
        NAME,
        Some(HELP_EXAMPLES),
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        false,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(crate::terminal::ansi_enabled())
}

/// Source name for `--config` runtime values in merged reads.
pub const SOURCE_RUNTIME: &str = "runtime";

/// Source names for merged reads (T1153 Wave-2 carry-over: `which`
/// labels conduit sources from [`merged_value`]).
pub const SOURCE_LOCAL: &str = "local";
pub const SOURCE_PROJECT: &str = "project";
pub const SOURCE_USER: &str = "user";
pub const SOURCE_SYSTEM: &str = "system";
pub const SOURCE_DEFAULT: &str = "default";

const SOURCES: [&str; 5] = [
    SOURCE_LOCAL,
    SOURCE_PROJECT,
    SOURCE_USER,
    SOURCE_SYSTEM,
    SOURCE_DEFAULT,
];

struct Setting {
    key: &'static str,
    type_name: &'static str,
    help: &'static str,
    example: &'static str,
    legacy: &'static str,
}

/// Builtin settings, mirroring `ArcanistSettings::getOptions()` verbatim
/// (with `pht()` substitutions applied).
const SETTINGS: [Setting; 15] = [
    Setting {
        key: "aliases",
        type_name: "aliases",
        help: "Configured command aliases. Use \"arc alias\" to define aliases.",
        example: "",
        legacy: "",
    },
    Setting {
        key: "arc.autostash",
        type_name: "bool",
        help: "Whether arc should permit the automatic stashing of changes in the working directory when requiring a clean working copy. This option should only be used when users understand how to restore their working directory from the local stash if an operation causes an unrecoverable error.",
        example: "false",
        legacy: "",
    },
    Setting {
        key: "arc.land.onto.default",
        type_name: "string",
        help: "The name of the default branch to land changes onto when `arc land` is run.",
        example: "\"develop\"",
        legacy: "",
    },
    Setting {
        key: "base",
        type_name: "string",
        help: "Base commit ruleset to invoke when determining the start of a commit range. See \"Arcanist User Guide: Commit Ranges\" for details.",
        example: "\"arc:amended, arc:prompt\"",
        legacy: "",
    },
    Setting {
        key: "browser",
        type_name: "string",
        help: "Command to use to invoke a web browser.",
        example: "\"gnome-www-browser\"",
        legacy: "",
    },
    Setting {
        key: "default",
        type_name: "string",
        help: "The URI of a server to connect to by default, if arc is run in a project without a configured URI or run outside of a project.",
        example: "\"http://devtools.example.com/\"",
        legacy: "",
    },
    Setting {
        key: "editor",
        type_name: "string",
        help: "Command to use to invoke an interactive editor, like `nano` or `vim`. This setting overrides the EDITOR environmental variable.",
        example: "\"nano\"",
        legacy: "",
    },
    Setting {
        key: "events.listeners",
        type_name: "list",
        help: "List of event listener classes to install at startup.",
        example: "[\"ExampleEventListener\"]",
        legacy: "",
    },
    Setting {
        key: "history.immutable",
        type_name: "bool",
        help: "If true, arc will never change repository history (e.g., through amending or rebasing). Defaults to true in Mercurial and false in Git. This setting has no effect in Subversion.",
        example: "false",
        legacy: "immutable_history",
    },
    Setting {
        key: "https.cabundle",
        type_name: "string",
        help: "Path to a custom CA bundle file to be used for cURL calls. This is used primarily when your conduit endpoint is behind HTTPS signed by your organization's internal CA.",
        example: "support/yourca.pem",
        legacy: "",
    },
    Setting {
        key: "lint.engine",
        type_name: "string",
        help: "The name of a default lint engine to use, if no lint engine is specified by the current project.",
        example: "\"ExampleLintEngine\"",
        legacy: "lint_engine",
    },
    Setting {
        key: "load",
        type_name: "list",
        help: "A list of paths to phutil libraries that should be loaded at startup. This can be used to make classes available, like lint or unit test engines.",
        example: "[\"/var/arc/customlib/src\"]",
        legacy: "phutil_libraries",
    },
    Setting {
        key: "phabricator.uri",
        type_name: "string",
        help: "Associates this working copy with a specific server.",
        example: "\"https://devtools.example.com/\"",
        legacy: "conduit_uri",
    },
    Setting {
        key: "repository.callsign",
        type_name: "string",
        help: "Associate the working copy with a specific repository. Normally, arc can figure this association out on its own, but if your setup is unusual you can use this option to tell it what the desired value is.",
        example: "\"X\"",
        legacy: "",
    },
    Setting {
        key: "unit.engine",
        type_name: "string",
        help: "The name of a default unit test engine to use, if no unit test engine is specified by the current project.",
        example: "\"ExampleUnitTestEngine\"",
        legacy: "unit_engine",
    },
];

fn setting(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Every known setting key, in sorted order.
#[must_use]
pub fn known_keys() -> Vec<&'static str> {
    SETTINGS.iter().map(|s| s.key).collect()
}

/// Help text for a known key, or `None` when the key is not recognized.
#[must_use]
pub fn help_for(key: &str) -> Option<&'static str> {
    setting(key).map(|s| s.help)
}

/// Example value for a known key, or `None` when the key has none.
#[must_use]
pub fn example_for(key: &str) -> Option<&'static str> {
    match setting(key) {
        Some(s) if !s.example.is_empty() => Some(s.example),
        _ => None,
    }
}

/// Setting type (`string`, `bool`, `list`, `wild`, `aliases`).
#[must_use]
pub fn setting_type(key: &str) -> &'static str {
    setting(key).map_or("wild", |s| s.type_name)
}

/// Legacy project-config name for a key, if one exists.
#[must_use]
pub fn legacy_name(key: &str) -> Option<&'static str> {
    match setting(key) {
        Some(s) if !s.legacy.is_empty() => Some(s.legacy),
        _ => None,
    }
}

/// Default configuration, mirroring `ArcanistSettings::getDefaultSettings()`.
#[must_use]
pub fn default_config() -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    map.insert("load".to_string(), serde_json::json!([]));
    map.insert("events.listeners".to_string(), serde_json::json!([]));
    map.insert("arc.autostash".to_string(), serde_json::json!(false));
    map
}

/// Format a value for display, mirroring
/// `ArcanistSettings::formatConfigValueForDisplay()`. `serde_json` never
/// escapes `/`, which matches PHP's display-time `\/` unescaping.
/// Divergences: non-ASCII strings (PHP's `json_encode` emits `\uXXXX`)
/// and whole floats (PHP prints `1.0` as `1`).
#[must_use]
pub fn format_value(key: &str, value: &serde_json::Value) -> String {
    let _ = key;
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

/// Read a JSON object file, reporting `parse_message` when the content is
/// not JSON. Each config source has its own PHP error text, supplied by
/// the caller.
fn read_config_file(
    path: &Path,
    parse_message: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| parse_message.to_string())?;
    doc.as_object()
        .cloned()
        .ok_or_else(|| format!("'{}' is not a JSON object", path.display()))
}

/// Working-copy flavor, mirroring `ArcanistWorkingCopyIdentity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Hg,
    Svn,
}

/// Locate the working-copy root enclosing `dir`, mirroring
/// `ArcanistWorkingCopyIdentity::newFromPathWithConfig()`: the innermost
/// directory holding `.git`, `.hg`, or `.svn` wins (`.git` first per
/// directory, like PHP's `$try` order). Pure filesystem checks, so a bare
/// marker directory counts exactly like PHP's `pathExists()`.
#[must_use]
pub fn find_vcs_root(dir: &Path) -> Option<(VcsKind, PathBuf)> {
    for ancestor in dir.ancestors() {
        if ancestor.join(".git").exists() {
            return Some((VcsKind::Git, ancestor.to_path_buf()));
        }
        if ancestor.join(".hg").exists() {
            return Some((VcsKind::Hg, ancestor.to_path_buf()));
        }
        if ancestor.join(".svn").exists() {
            return Some((VcsKind::Svn, ancestor.to_path_buf()));
        }
    }
    None
}

/// Read the project `.arcconfig` for `dir`, mirroring
/// `ArcanistWorkingCopyIdentity::newFromPathWithConfig()`: git and
/// Mercurial read `.arcconfig` ONLY at the working-copy root;
/// Subversion takes the nearest `.arcconfig` from `dir` up to the root;
/// outside any working copy there is no project config at all, so a stray
/// `.arcconfig` in a plain directory is ignored like PHP ignores it.
/// The parse error mirrors `parseRawConfigFile()`; the non-object
/// rejection is arc-rs strictness (PHP tolerates it).
pub fn read_project_config(
    dir: &Path,
) -> Result<(Option<PathBuf>, serde_json::Map<String, serde_json::Value>), String> {
    let Some((kind, root)) = find_vcs_root(dir) else {
        return Ok((None, serde_json::Map::new()));
    };
    if kind == VcsKind::Svn {
        for ancestor in dir.ancestors() {
            let file = ancestor.join(".arcconfig");
            if file.is_file() {
                return read_project_file(&file);
            }
            if ancestor == root.as_path() {
                break;
            }
        }
        return Ok((None, serde_json::Map::new()));
    }
    let file = root.join(".arcconfig");
    if !file.is_file() {
        return Ok((None, serde_json::Map::new()));
    }
    read_project_file(&file)
}

/// Read one project file, with the `parseRawConfigFile()` error text.
fn read_project_file(
    file: &Path,
) -> Result<(Option<PathBuf>, serde_json::Map<String, serde_json::Value>), String> {
    let from_where = file.display().to_string();
    let message = terminal::pht(
        "Unable to parse '%s' file '%s'.",
        &[PhtArg::Text(".arcconfig"), PhtArg::Text(&from_where)],
    );
    let map = read_config_file(file, &message)?;
    Ok((Some(file.to_path_buf()), map))
}

/// Read the working copy's local config (`arc/config` under the VCS
/// metadata directory: `.git`, `.hg`, or `.svn` at the working-copy
/// root, mirroring `ArcanistWorkingCopyIdentity::$localMetaDir`); empty
/// outside a working copy. T1153 CORE-32: Mercurial and Subversion read
/// like git. The parse error mirrors `readLocalArcConfig()`:
/// `Failed to parse '%s' as JSON.`
pub fn read_local_config(dir: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let Some((kind, root)) = find_vcs_root(dir) else {
        return Ok(serde_json::Map::new());
    };
    let meta = match kind {
        VcsKind::Git => ".git",
        VcsKind::Hg => ".hg",
        VcsKind::Svn => ".svn",
    };
    let file = root.join(meta).join("arc").join("config");
    if !file.is_file() {
        return Ok(serde_json::Map::new());
    }
    let path = file.display().to_string();
    let message = terminal::pht("Failed to parse '%s' as JSON.", &[PhtArg::Text(&path)]);
    read_config_file(&file, &message)
}

/// Read the user config from a `~/.arcrc` file, mirroring PHP's
/// `readUserArcConfig()`: ONLY the `config` object when present (T1153
/// CORE-30: top-level siblings like `aliases` are dropped on this path),
/// else the whole file -- including `hosts`, exactly like stock PHP,
/// which never redacts credential-adjacent keys on this path. A missing
/// file yields an empty config. The parse error mirrors
/// `readUserConfigurationFile()`, including its literal `~/.arcrc`.
pub fn read_user_file(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let obj = read_user_doc(path)?;
    // Stock PHP returns the whole file when `isset($config['config'])` is
    // false (missing or null); only a present, non-null section narrows
    // the user config.
    match obj.get("config") {
        None | Some(serde_json::Value::Null) => Ok(obj),
        Some(serde_json::Value::Object(config)) => Ok(config.clone()),
        Some(_) => Err(format!(
            "'{}' has a non-object \"config\" section",
            path.display()
        )),
    }
}

/// Read the user config through the toolset transform, mirroring
/// `ArcanistUserConfigurationSource::getAllValues()`: the `config`
/// section plus every top-level key except `hosts` and `config` itself,
/// with the section winning collisions (T1153 CORE-28: `$result +=
/// $config` keeps the section's value). Toolset workflows (`alias`,
/// `prompts`) read user config on this path; classic workflows use
/// [`read_user_file`]. Load and shape errors match [`read_user_file`].
pub fn read_user_file_toolset(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let obj = read_user_doc(path)?;
    let mut map = match obj.get("config") {
        None | Some(serde_json::Value::Null) => serde_json::Map::new(),
        Some(serde_json::Value::Object(config)) => config.clone(),
        Some(_) => {
            return Err(format!(
                "'{}' has a non-object \"config\" section",
                path.display()
            ));
        }
    };
    for (key, value) in &obj {
        if key != "hosts" && key != "config" && !map.contains_key(key) {
            map.insert(key.clone(), value.clone());
        }
    }
    Ok(map)
}

/// Load a `~/.arcrc` file as a JSON object: missing files read empty,
/// with the `readUserConfigurationFile()` parse error.
fn read_user_doc(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if !path.is_file() {
        return Ok(serde_json::Map::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        terminal::pht(
            "Your '%s' file is not a valid JSON file.",
            &[PhtArg::Text("~/.arcrc")],
        )
    })?;
    doc.as_object()
        .cloned()
        .ok_or_else(|| format!("'{}' is not a JSON object", path.display()))
}

/// Location of the system config, mirroring
/// `ArcanistConfigurationManager::getSystemArcConfigLocation()`.
#[must_use]
pub fn system_config_path() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
        base.join("Phabricator").join("Arcanist").join("config")
    } else {
        PathBuf::from("/etc/arcconfig")
    }
}

/// Read a system config file; a missing file yields an empty config. The
/// parse error mirrors `readSystemArcConfig()`.
pub fn read_system_config_from(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if !path.is_file() {
        return Ok(serde_json::Map::new());
    }
    let name = path.display().to_string();
    let message = terminal::pht(
        "Your '%s' file is not a valid JSON file.",
        &[PhtArg::Text(&name)],
    );
    read_config_file(path, &message)
}

/// Read the system config from its standard location.
pub fn read_system_config() -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let path = system_config_path();
    read_system_config_from(&path)
}

/// Configuration from every source for one directory.
#[derive(Debug, Clone)]
pub struct Sources {
    pub local: serde_json::Map<String, serde_json::Value>,
    pub project: serde_json::Map<String, serde_json::Value>,
    pub user: serde_json::Map<String, serde_json::Value>,
    pub system: serde_json::Map<String, serde_json::Value>,
    pub default: serde_json::Map<String, serde_json::Value>,
    /// The `.arcconfig` file the project map came from, if any.
    pub project_file: Option<PathBuf>,
}

/// Load every source for `dir`. `user_file` is the `~/.arcrc` path (from
/// [`config::arcrc_path`]); `None` skips the user source.
pub fn load_sources(dir: &Path, user_file: Option<&Path>) -> Result<Sources, String> {
    load_merged(dir, user_file, &LoadOptions::default()).map(|merged| merged.sources)
}

/// Overrides for a merged config load, mirroring the global flags PHP
/// parses before dispatch (`scripts/arcanist.php` base args plus the
/// `ArcanistRuntime` `--config-file`).
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Parsed `--config key=value` pairs (coerced storage values), the
    /// highest-precedence source.
    pub runtime: serde_json::Map<String, serde_json::Value>,
    /// `--config-file` paths: when non-empty they REPLACE the system and
    /// user sources (`ArcanistConfigurationEngine.php:43-47`), merged in
    /// order so later files win. A missing file reads empty, like PHP's
    /// `pathExists()` guard.
    pub config_files: Vec<PathBuf>,
    /// `--arcrc-file` path: replaces `~/.arcrc` as the user file
    /// (`ArcanistConfigurationManager::setUserConfigurationFileLocation()`).
    /// A missing file is an error, like PHP.
    pub arcrc_file: Option<PathBuf>,
}

impl LoadOptions {
    /// Overlay the dispatcher's global `--config`/`--config-file`/
    /// `--arcrc-file` (T1153 CORE-10/T1153 CORE-11 family): explicit per-command
    /// values win (runtime per key, `arcrc_file` when set),
    /// config-file lists concatenate (global first, later files win).
    /// [`load_merged`] applies this, so globals reach every command.
    #[must_use]
    pub fn with_global(&self) -> LoadOptions {
        let global = crate::config::global_load_options();
        let mut runtime = global.runtime;
        runtime.extend(self.runtime.clone());
        let mut config_files = global.config_files;
        config_files.extend(self.config_files.iter().cloned());
        LoadOptions {
            runtime,
            config_files,
            arcrc_file: self.arcrc_file.clone().or(global.arcrc_file),
        }
    }
}

/// Configuration from every source, plus the `--config` runtime map.
///
/// Display (`render`, `all_keys`) covers the five static [`Sources`]
/// only: stock `get-config` never shows runtime values
/// (`ArcanistGetConfigWorkflow.php` reads local/project/user/system/
/// default). Programmatic reads ([`merged_value`], [`checked_value`])
/// consult the runtime map first.
#[derive(Debug, Clone)]
pub struct MergedConfig {
    pub sources: Sources,
    pub runtime: serde_json::Map<String, serde_json::Value>,
}

/// Resolve the user-config file: `--arcrc-file` wins over the default
/// `~/.arcrc`; a missing custom file errors, like PHP's
/// `setUserConfigurationFileLocation()`.
pub fn resolve_user_file(
    default: Option<PathBuf>,
    arcrc_file: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    match arcrc_file {
        Some(path) if !path.is_file() => Err(terminal::pht(
            "Custom %s file was specified, but it was not found!",
            &[PhtArg::Text("arcrc")],
        )),
        Some(path) => Ok(Some(path.to_path_buf())),
        None => Ok(default),
    }
}

/// Build [`LoadOptions`] from raw CLI values: parse and coerce `--config`
/// pairs (classic `applyRuntimeArcConfig()` semantics via
/// [`set_config::parse_runtime_config`]), collect `--config-file`s, and
/// keep `--arcrc-file` for [`load_merged`] to resolve.
pub fn load_options_from_cli(
    config: &[String],
    config_files: &[String],
    arcrc_file: Option<&str>,
) -> Result<LoadOptions, String> {
    Ok(LoadOptions {
        runtime: set_config::parse_runtime_config(config)?,
        config_files: config_files.iter().map(PathBuf::from).collect(),
        arcrc_file: arcrc_file.map(PathBuf::from),
    })
}

/// Load every source plus overrides for `dir`. `user_file` is the default
/// `~/.arcrc` path; `opts.arcrc_file` overrides it, and non-empty
/// `opts.config_files` shadow both the user and system sources.
pub fn load_merged(
    dir: &Path,
    user_file: Option<&Path>,
    opts: &LoadOptions,
) -> Result<MergedConfig, String> {
    let opts = opts.with_global();
    let opts = &opts;
    let (project_file, project) = read_project_config(dir)?;
    let local = read_local_config(dir)?;
    let (user, system) = if opts.config_files.is_empty() {
        let resolved =
            resolve_user_file(user_file.map(Path::to_path_buf), opts.arcrc_file.as_deref())?;
        let user = match resolved.as_deref() {
            Some(path) => read_user_file(path)?,
            None => serde_json::Map::new(),
        };
        (user, read_system_config()?)
    } else {
        let mut merged = serde_json::Map::new();
        for path in &opts.config_files {
            if !path.is_file() {
                continue;
            }
            let name = path.display().to_string();
            let message = terminal::pht(
                "Your '%s' file is not a valid JSON file.",
                &[PhtArg::Text(&name)],
            );
            for (key, value) in read_config_file(path, &message)? {
                merged.insert(key, value);
            }
        }
        (serde_json::Map::new(), merged)
    };
    Ok(MergedConfig {
        sources: Sources {
            local,
            project,
            user,
            system,
            default: default_config(),
            project_file,
        },
        runtime: opts.runtime.clone(),
    })
}

fn map_for<'a>(
    source: &str,
    sources: &'a Sources,
) -> &'a serde_json::Map<String, serde_json::Value> {
    match source {
        SOURCE_LOCAL => &sources.local,
        SOURCE_PROJECT => &sources.project,
        SOURCE_USER => &sources.user,
        SOURCE_SYSTEM => &sources.system,
        _ => &sources.default,
    }
}

/// The value of `key` at one source. The project source also accepts the
/// key's legacy name, mirroring the PHP working copy identity -- including
/// its `null`-key quirk (T1153 CORE-31): `getLegacyName()` returns `null`
/// when a key has no legacy name, and `array_key_exists(null, ...)`
/// coerces to `''`, so a project `{"": v}` entry answers every key
/// without a legacy name.
#[must_use]
pub fn value_at(key: &str, source: &str, sources: &Sources) -> Option<serde_json::Value> {
    if let Some(value) = map_for(source, sources).get(key) {
        return Some(value.clone());
    }
    if source == SOURCE_PROJECT {
        let map = map_for(source, sources);
        if let Some(legacy) = legacy_name(key) {
            return map.get(legacy).cloned();
        }
        return map.get("").cloned();
    }
    None
}

/// The winning value of `key` with its source, or `None` when unset.
#[must_use]
pub fn current_value(key: &str, sources: &Sources) -> Option<(&'static str, serde_json::Value)> {
    for source in SOURCES {
        if let Some(value) = value_at(key, source, sources) {
            return Some((source, value));
        }
    }
    None
}

/// The winning value of `key` across runtime plus static sources, or
/// `None` when unset: runtime > local > project > user > system >
/// default, mirroring `getConfigFromAnySource()` precedence without type
/// validation (like `get-config` display, which shows raw values).
#[must_use]
pub fn merged_value(key: &str, merged: &MergedConfig) -> Option<(&'static str, serde_json::Value)> {
    if let Some(value) = merged.runtime.get(key) {
        return Some((SOURCE_RUNTIME, value.clone()));
    }
    current_value(key, &merged.sources)
}

/// Validate a stored value against its setting type, mirroring
/// `ArcanistSettings::willReadValue()`. `list` accepts JSON objects too:
/// PHP decodes to associative arrays, which pass its `is_array()` check.
pub fn check_value(key: &str, value: &serde_json::Value) -> Result<serde_json::Value, String> {
    match setting_type(key) {
        "string" if !value.is_string() => Err(terminal::pht(
            "Type of setting '%s' must be string.",
            &[PhtArg::Text(key)],
        )),
        "bool" if !value.is_boolean() => Err(terminal::pht(
            "Type of setting '%s' must be boolean.",
            &[PhtArg::Text(key)],
        )),
        "list" if !(value.is_array() || value.is_object()) => Err(terminal::pht(
            "Type of setting '%s' must be list.",
            &[PhtArg::Text(key)],
        )),
        _ => Ok(value.clone()),
    }
}

/// The winning value with `willReadValue()` validation, mirroring
/// `ArcanistConfigurationManager::getConfigFromAllSources()`: every
/// present source value is checked in precedence order (even below a
/// valid winner, like PHP), and the builtin defaults read unchecked.
pub fn checked_value(
    key: &str,
    merged: &MergedConfig,
) -> Result<Option<(&'static str, serde_json::Value)>, String> {
    if let Some(value) = merged.runtime.get(key) {
        check_value(key, value)?;
    }
    for source in SOURCES {
        if source == SOURCE_DEFAULT {
            continue;
        }
        if let Some(value) = value_at(key, source, &merged.sources) {
            check_value(key, &value)?;
        }
    }
    Ok(merged_value(key, merged))
}

/// Every key to show with no arguments: known keys plus any configured
/// key, sorted and deduplicated.
#[must_use]
pub fn all_keys(sources: &Sources) -> Vec<String> {
    let mut keys: Vec<String> = known_keys().iter().map(ToString::to_string).collect();
    for source in SOURCES {
        keys.extend(map_for(source, sources).keys().cloned());
    }
    keys.sort();
    keys.dedup();
    keys
}

fn row(label: &str, value: &str, out: &mut String) {
    out.push_str(&format!("{label:>20}: {value}\n"));
}

/// Render one key's block with the process ANSI setting.
#[must_use]
pub fn render(key: &str, sources: &Sources, detailed: bool) -> String {
    render_ansi(key, sources, detailed, terminal::ansi_enabled())
}

/// Render one key's block, briefly or with per-source detail, through the
/// workflow's `writeOut()` calls: the header is
/// `writeOut("**%s**\\n\\n", $key)` (interpreted pattern, raw
/// substitution, hence `%R`), help goes through
/// `phutil_console_wrap($help, 4)`, labels are `pht()` texts
/// (`pht('Example Value')`, `pht('Current Value')`,
/// `pht('Current Source')`, `pht('%s Value', $source)` with lowercase
/// source names), and a missing example prints empty.
#[must_use]
pub fn render_ansi(key: &str, sources: &Sources, detailed: bool, ansi: bool) -> String {
    let head = terminal::tsprintf_ansi("**%R**\n\n", &[TVal::Text(key)], ansi)
        .map(|t| t.to_string())
        .unwrap_or_default();
    let mut out = head;
    if detailed {
        let help = match help_for(key) {
            Some(help) => help.to_string(),
            None => terminal::pht(
                "(This configuration value is not recognized by arc. It may be misspelled or out of date.)",
                &[],
            ),
        };
        out.push_str(&terminal::console_wrap(&help, 4, true));
        out.push_str("\n\n");
        let example = terminal::pht("Example Value", &[]);
        row(&example, example_for(key).unwrap_or(""), &mut out);
        out.push('\n');
    }
    let value_label = terminal::pht("Current Value", &[]);
    let source_label = terminal::pht("Current Source", &[]);
    match current_value(key, sources) {
        Some((source, value)) => {
            row(&value_label, &format_value(key, &value), &mut out);
            row(&source_label, source, &mut out);
        }
        None => {
            row(&value_label, "-", &mut out);
            row(&source_label, "-", &mut out);
        }
    }
    if detailed {
        out.push('\n');
        for source in SOURCES {
            let label = terminal::pht("%s Value", &[PhtArg::Text(source)]);
            match value_at(key, source, sources) {
                Some(value) => row(&label, &format_value(key, &value), &mut out),
                None => row(&label, "-", &mut out),
            }
        }
    }
    out
}

/// Render every requested key with the process ANSI setting.
#[must_use]
pub fn render_all(keys: &[String], sources: &Sources, detailed: bool) -> String {
    render_all_ansi(keys, sources, detailed, terminal::ansi_enabled())
}

/// Render every requested key, with the `--verbose` hint when brief:
/// `writeOut("(%s)\\n", pht('Run with %s for more details.', '--verbose'))`.
#[must_use]
pub fn render_all_ansi(keys: &[String], sources: &Sources, detailed: bool, ansi: bool) -> String {
    let mut out = String::new();
    let multi = keys.len() > 1;
    for key in keys {
        out.push_str(&render_ansi(key, sources, detailed, ansi));
        if multi {
            out.push('\n');
        }
    }
    if !detailed {
        let hint = terminal::pht(
            "Run with %s for more details.",
            &[PhtArg::Text("--verbose")],
        );
        out.push_str(&format!("({hint})\n"));
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    show_source: bool,
    keys: Vec<String>,
    config: Vec<String>,
    config_files: Vec<String>,
    arcrc_file: Option<String>,
}

/// Split a `--flag` or `--flag=value` token into its name and inline
/// value.
fn split_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (arg, None),
    }
}

/// Consume a `--config`/`--config-file`/`--arcrc-file` flag shared by the
/// config commands: when `name` is one of those, take its value (inline
/// `--flag=value`, else the token at `*index`, advancing past it) into
/// the matching sink and return true; otherwise return false without
/// consuming anything.
pub fn take_runtime_flag(
    config: &mut Vec<String>,
    config_files: &mut Vec<String>,
    arcrc_file: &mut Option<String>,
    name: &str,
    inline: Option<&str>,
    argv: &[String],
    index: &mut usize,
) -> Result<bool, String> {
    if !matches!(name, "--config" | "--config-file" | "--arcrc-file") {
        return Ok(false);
    }
    let value = match inline {
        Some(value) => value.to_string(),
        None => {
            if *index >= argv.len() {
                return Err(format!("{name} requires a value"));
            }
            let value = argv[*index].clone();
            *index += 1;
            value
        }
    };
    match name {
        "--config" => config.push(value),
        "--config-file" => config_files.push(value),
        _ => *arcrc_file = Some(value),
    }
    Ok(true)
}

fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        show_source: false,
        keys: Vec::new(),
        config: Vec::new(),
        config_files: Vec::new(),
        arcrc_file: None,
    };
    let mut end_of_flags = false;
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        index += 1;
        if end_of_flags {
            options.keys.push(arg.clone());
            continue;
        }
        if arg == "--" {
            end_of_flags = true;
            continue;
        }
        if arg == "--show-source" || arg == "--verbose" {
            options.show_source = true;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            return Ok(None);
        }
        let (name, inline) = split_flag(arg);
        if take_runtime_flag(
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
        options.keys.push(arg.clone());
    }
    Ok(Some(options))
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };
    let cli = match load_options_from_cli(
        &options.config,
        &options.config_files,
        options.arcrc_file.as_deref(),
    ) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user_file = config::arcrc_path();
    // Display covers the static sources only: stock `get-config` never
    // shows `--config` runtime values, though they still validate.
    let sources = match load_merged(&cwd, user_file.as_deref(), &cli) {
        Ok(merged) => merged.sources,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let keys = if options.keys.is_empty() {
        all_keys(&sources)
    } else {
        options.keys
    };
    print!("{}", render_all(&keys, &sources, options.show_source));
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "get_config_tests.rs"]
mod get_config_tests;
