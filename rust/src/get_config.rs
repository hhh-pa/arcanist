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
//! Deviation: project-config lookup walks up from the working directory
//! instead of reading only the VCS root, so `get-config` keeps working
//! outside repositories.

// A crate cannot name itself in `use` (E0432), so alias the current crate
// to keep the shared-helper paths (`arc_rs::git`, `arc_rs::config`)
// working both once this module is wired into `lib.rs` and when it is
// included standalone by an integration test.
extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::terminal::{PhtArg, TVal};
use arc_rs::{config, git, terminal};

/// Subcommand name, matching `arc get-config`.
pub const NAME: &str = "get-config";

const USAGE: &str = "usage: arc get-config [--verbose] [--] [NAME...]\n\
\n\
Read arc configuration. With no NAME, show all known and configured keys.\n\
\n\
Precedence (highest wins): local (.git/arc/config), project (.arcconfig),\n\
user (~/.arcrc), system (/etc/arcconfig), default.\n\
\n\
Options:\n\
  --verbose       Show detailed information about each option.\n\
  --show-source   Alias for --verbose.\n\
  -h, --help      Show this help.\n";

const SOURCE_LOCAL: &str = "local";
const SOURCE_PROJECT: &str = "project";
const SOURCE_USER: &str = "user";
const SOURCE_SYSTEM: &str = "system";
const SOURCE_DEFAULT: &str = "default";

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

/// Read the nearest enclosing `.arcconfig` above `dir`. The parse error
/// mirrors `ArcanistWorkingCopyIdentity::parseRawConfigFile()`; the
/// non-object rejection is arc-rs strictness (PHP tolerates it).
pub fn read_project_config(
    dir: &Path,
) -> Result<(Option<PathBuf>, serde_json::Map<String, serde_json::Value>), String> {
    for ancestor in dir.ancestors() {
        let file = ancestor.join(".arcconfig");
        if !file.is_file() {
            continue;
        }
        let from_where = file.display().to_string();
        let message = terminal::pht(
            "Unable to parse '%s' file '%s'.",
            &[PhtArg::Text(".arcconfig"), PhtArg::Text(&from_where)],
        );
        let map = read_config_file(&file, &message)?;
        return Ok((Some(file), map));
    }
    Ok((None, serde_json::Map::new()))
}

/// Read the working copy's local config (`.git/arc/config`); empty outside
/// a git repository. Only git is supported (see T1128). The parse error
/// mirrors `readLocalArcConfig()`: `Failed to parse '%s' as JSON.`
pub fn read_local_config(dir: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if !git::is_inside_work_tree(dir) {
        return Ok(serde_json::Map::new());
    }
    let top = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])?;
    let file = PathBuf::from(top).join(".git").join("arc").join("config");
    if !file.is_file() {
        return Ok(serde_json::Map::new());
    }
    let path = file.display().to_string();
    let message = terminal::pht("Failed to parse '%s' as JSON.", &[PhtArg::Text(&path)]);
    read_config_file(&file, &message)
}

/// Read the user config from a `~/.arcrc` file, mirroring PHP's
/// `readUserArcConfig()`: the `config` object when present (plus the
/// top-level `aliases` key), else the whole file -- including `hosts`,
/// exactly like stock PHP, which never redacts credential-adjacent keys
/// on this path. A missing file yields an empty config. The parse error
/// mirrors `readUserConfigurationFile()`, including its literal `~/.arcrc`.
pub fn read_user_file(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
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
    let obj = doc
        .as_object()
        .ok_or_else(|| format!("'{}' is not a JSON object", path.display()))?;
    // Stock PHP returns the whole file when `isset($config['config'])` is
    // false (missing or null); only a present, non-null section narrows
    // the user config.
    let mut map = match obj.get("config") {
        None | Some(serde_json::Value::Null) => obj.clone(),
        Some(serde_json::Value::Object(config)) => config.clone(),
        Some(_) => {
            return Err(format!(
                "'{}' has a non-object \"config\" section",
                path.display()
            ));
        }
    };
    if let Some(aliases) = obj.get("aliases") {
        map.insert("aliases".to_string(), aliases.clone());
    }
    Ok(map)
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
    let (project_file, project) = read_project_config(dir)?;
    Ok(Sources {
        local: read_local_config(dir)?,
        project,
        user: match user_file {
            Some(path) => read_user_file(path)?,
            None => serde_json::Map::new(),
        },
        system: read_system_config()?,
        default: default_config(),
        project_file,
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
/// key's legacy name, mirroring the PHP working copy identity.
#[must_use]
pub fn value_at(key: &str, source: &str, sources: &Sources) -> Option<serde_json::Value> {
    if let Some(value) = map_for(source, sources).get(key) {
        return Some(value.clone());
    }
    if source == SOURCE_PROJECT
        && let Some(legacy) = legacy_name(key)
    {
        return map_for(source, sources).get(legacy).cloned();
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
}

fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut show_source = false;
    let mut keys = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            keys.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--show-source" || arg == "--verbose" {
            show_source = true;
        } else if arg == "-h" || arg == "--help" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            keys.push(arg.clone());
        }
    }
    Ok(Some(Options { show_source, keys }))
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
            eprintln!("arc {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user_file = config::arcrc_path();
    let sources = match load_sources(&cwd, user_file.as_deref()) {
        Ok(sources) => sources,
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
