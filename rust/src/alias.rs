//! `arc-rs alias` — create and modify command aliases.
//!
//! Port of `src/toolset/workflow/ArcanistAliasWorkflow.php` (argument
//! dispatch, `--json`, creation), `src/toolset/ArcanistAlias.php` (old dict
//! and new list storage formats), `src/toolset/ArcanistAliasEngine.php`
//! (expansion, [`expand`]), and the `src/runtime/ArcanistRuntime.php`
//! dispatch (shell execution, hint application). The list and delete bodies
//! follow the pre-toolset `src/workflow/ArcanistAliasWorkflow.php`
//! (`70c6045c~1`), since the toolset versions on master are `TOOLSETS`
//! stubs returning 1.
//!
//! Aliases live under the top-level `aliases` key of `~/.arcrc`, read from
//! every file source (system, user, project, local, least to most specific,
//! mirroring `ArcanistConfigurationEngine`) but only ever written to the
//! user scope. `arc alias` manages the `arc` toolset only.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::terminal::PhtArg;
use crate::{
    amend, anoid, bookmarks, branches, close_revision, commit, config, cover, diff, download,
    export, get_config, inspect, install_certificate, land, liberate, lint, linters, list, look,
    markers, paste, patch, prompt, set_config, tasks, terminal, todo, unit, upgrade, upload, weld,
    which, work,
};

/// Subcommand name, matching `arc alias`.
pub const NAME: &str = "alias";

/// Toolset key of this binary (`arc-rs` is single-toolset).
pub const TOOLSET: &str = "arc";

/// Maximum alias-chain depth, mirroring `ArcanistAliasEngine`.
const STACK_LIMIT: usize = 16;

const USAGE: &str = "usage: arc-rs alias [--json]\n\
       arc-rs alias <name>\n\
       arc-rs alias <name> <target> -- [arguments]\n\
\n\
Create and modify command aliases.\n\
\n\
With no arguments, list aliases. With one argument, delete that alias\n\
(after confirmation). With two or more, define <name> as a shorthand\n\
for <target> with [arguments]; use \"--\" before any flags meant for\n\
the target. A target starting with \"!\" runs as a shell command.\n\
\n\
Options:\n\
  --json            Output aliases in JSON format (listing only).\n\
  -h, --help        Show this help.\n";

/// Every command `main` dispatches without aliases: the `COMMANDS` table
/// plus `call-conduit` (parsed by `classify`) and `help` (also handled by
/// `classify`, a builtin workflow in PHP). An alias can never shadow these.
const BUILTINS: &[&str] = &[
    NAME,
    amend::NAME,
    anoid::NAME,
    bookmarks::NAME,
    branches::NAME,
    close_revision::NAME,
    commit::NAME,
    cover::NAME,
    diff::NAME,
    download::NAME,
    export::NAME,
    get_config::NAME,
    inspect::NAME,
    install_certificate::NAME,
    land::NAME,
    liberate::NAME,
    lint::NAME,
    linters::NAME,
    list::NAME,
    look::NAME,
    markers::NAME,
    paste::NAME,
    patch::NAME,
    set_config::NAME,
    tasks::NAME,
    todo::NAME,
    unit::NAME,
    upgrade::NAME,
    upload::NAME,
    weld::NAME,
    which::NAME,
    work::NAME,
    "call-conduit",
    "help",
];

/// Every builtin command name, for registration sync checks.
#[must_use]
pub fn builtin_names() -> &'static [&'static str] {
    BUILTINS
}

/// True when `name` is a builtin command (PHP `isset($workflows[$name])`).
#[must_use]
pub fn is_builtin_command(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// One alias definition, mirroring `ArcanistAlias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub toolset: String,
    pub trigger: String,
    pub command: Vec<String>,
    /// Display name of the defining configuration source.
    pub source: String,
}

impl Alias {
    /// True when the alias invokes a shell command (`!`-prefixed head).
    #[must_use]
    pub fn is_shell(&self) -> bool {
        is_shell_command(&self.command)
    }

    /// New-format storage dictionary (`getStorageDictionary()`).
    #[must_use]
    pub fn storage_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "trigger": self.trigger,
            "toolset": self.toolset,
            "command": self.command,
        })
    }
}

/// True when `command` is a shell alias: a `!`-prefixed head
/// (`ArcanistAlias::isShellCommandAlias()`).
#[must_use]
pub fn is_shell_command(command: &[String]) -> bool {
    command.first().is_some_and(|head| head.starts_with('!'))
}

/// Parse one alias entry, mirroring `ArcanistAlias::newFromConfig()`:
/// a string list is the old `{"draft": ["diff", "--draft"]}` format
/// (toolset implied `arc`); a `trigger`/`toolset`/`command` dictionary is
/// the modern format. Anything else is invalid.
fn parse_entry(key: &str, value: &serde_json::Value) -> Result<Alias, String> {
    if let Some(items) = value.as_array() {
        let mut command = Vec::with_capacity(items.len());
        for item in items {
            let Some(text) = item.as_str() else {
                return Err(terminal::pht(
                    "Found invalid alias definition (with key \"%s\").",
                    &[PhtArg::Text(key)],
                ));
            };
            command.push(text.to_string());
        }
        return Ok(Alias {
            toolset: TOOLSET.to_string(),
            trigger: key.to_string(),
            command,
            source: String::new(),
        });
    }
    let invalid = || {
        terminal::pht(
            "Found invalid alias definition (with key \"%s\").",
            &[PhtArg::Text(key)],
        )
    };
    let map = value.as_object().ok_or_else(|| {
        terminal::pht(
            "Expected alias definition (with key \"%s\") to be a dictionary.",
            &[PhtArg::Text(key)],
        )
    })?;
    let trigger = map.get("trigger").and_then(|v| v.as_str());
    let toolset = map.get("toolset").and_then(|v| v.as_str());
    let command = map
        .get("command")
        .and_then(|v| v.as_array())
        .and_then(|items| {
            items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
        });
    match (trigger, toolset, command) {
        (Some(trigger), Some(toolset), Some(command)) => Ok(Alias {
            toolset: toolset.to_string(),
            trigger: trigger.to_string(),
            command,
            source: String::new(),
        }),
        _ => Err(invalid()),
    }
}

/// Warning for an invalid alias entry, mirroring the engine's
/// `EFFECT_CONFIGURATION` message.
fn invalid_warning(source: &str, detail: &str) -> String {
    terminal::pht(
        "Configuration source (\"%s\") defines an invalid alias, which will be ignored: %s",
        &[PhtArg::Text(source), PhtArg::Text(detail)],
    )
}

/// Parse one source's `aliases` storage value (old dict or new list),
/// collecting invalid-entry warnings. A non-array value warns like an
/// invalid entry (PHP throws `Expected a list or dictionary!` here).
pub fn aliases_from_storage(value: &serde_json::Value, source: &str) -> (Vec<Alias>, Vec<String>) {
    let entries: Vec<(String, &serde_json::Value)> = match value {
        serde_json::Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item))
            .collect(),
        serde_json::Value::Object(map) => {
            map.iter().map(|(key, item)| (key.clone(), item)).collect()
        }
        _ => {
            let detail = terminal::pht("Expected a list or dictionary!", &[]);
            return (Vec::new(), vec![invalid_warning(source, &detail)]);
        }
    };
    let mut aliases = Vec::new();
    let mut warnings = Vec::new();
    for (key, item) in entries {
        match parse_entry(&key, item) {
            Ok(mut alias) => {
                alias.source = source.to_string();
                aliases.push(alias);
            }
            Err(detail) => warnings.push(invalid_warning(source, &detail)),
        }
    }
    (aliases, warnings)
}

fn take_source(
    map: &serde_json::Map<String, serde_json::Value>,
    source: &str,
    aliases: &mut Vec<Alias>,
    warnings: &mut Vec<String>,
) {
    if let Some(value) = map.get("aliases") {
        let (mut found, mut found_warnings) = aliases_from_storage(value, source);
        aliases.append(&mut found);
        warnings.append(&mut found_warnings);
    }
}

/// Load aliases from every file source, least to most specific (system,
/// user, project, local), mirroring the PHP source order. Later entries
/// win during resolution.
pub fn load_aliases_in(
    dir: &Path,
    user_file: Option<&Path>,
) -> Result<(Vec<Alias>, Vec<String>), String> {
    let mut aliases = Vec::new();
    let mut warnings = Vec::new();
    let system_path = get_config::system_config_path();
    let system = get_config::read_system_config()?;
    take_source(
        &system,
        &format!("system ({})", system_path.display()),
        &mut aliases,
        &mut warnings,
    );
    if let Some(path) = user_file {
        let user = get_config::read_user_file(path)?;
        take_source(
            &user,
            &format!("user ({})", path.display()),
            &mut aliases,
            &mut warnings,
        );
    }
    let (project_file, project) = get_config::read_project_config(dir)?;
    let project_source = project_file.map_or_else(
        || "project".to_string(),
        |file| format!("project ({})", file.display()),
    );
    take_source(&project, &project_source, &mut aliases, &mut warnings);
    let local = get_config::read_local_config(dir)?;
    take_source(&local, "local", &mut aliases, &mut warnings);
    Ok((aliases, warnings))
}

/// Load aliases for the current directory and `~/.arcrc`.
pub fn load_aliases() -> Result<(Vec<Alias>, Vec<String>), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_aliases_in(&cwd, config::arcrc_path().as_deref())
}

/// Outcome of alias expansion, mirroring `ArcanistAliasEffect` handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expansion {
    /// Dispatch `argv` (rewritten, or unchanged when nothing matched);
    /// `hints` describe each hop.
    Dispatch {
        argv: Vec<String>,
        hints: Vec<String>,
    },
    /// Execute `argv` as a shell command instead of dispatching.
    Shell { argv: Vec<String>, hint: String },
    /// The chain is broken (dangling, cyclic, or too deep): fail.
    Failed { message: String, hints: Vec<String> },
}

fn dispatch_of(command: &str, rest: &[String], hints: Vec<String>) -> Expansion {
    let mut argv = vec![command.to_string()];
    argv.extend(rest.iter().cloned());
    Expansion::Dispatch { argv, hints }
}

/// Expand `argv` through `aliases`, mirroring
/// `ArcanistAliasEngine::resolveAliasesForCommand()`: builtins always win,
/// the last same-toolset definition wins, alias arguments come first with
/// the user's arguments appended, and chains resolve recursively.
pub fn expand(argv: &[String], aliases: &[Alias], is_builtin: impl Fn(&str) -> bool) -> Expansion {
    let Some(first) = argv.first() else {
        return Expansion::Dispatch {
            argv: Vec::new(),
            hints: Vec::new(),
        };
    };
    let mut hints = Vec::new();
    let mut command = first.clone();
    let mut rest = argv[1..].to_vec();
    let mut stack: Vec<String> = Vec::new();
    loop {
        if is_builtin(&command) {
            return dispatch_of(&command, &rest, hints);
        }
        let toolset_matches: Vec<&Alias> = aliases
            .iter()
            .filter(|alias| alias.trigger == command && alias.toolset == TOOLSET)
            .collect();
        if toolset_matches.is_empty() {
            return expand_no_match(aliases, &command, &rest, &mut hints, &stack);
        }
        let alias = toolset_matches[toolset_matches.len() - 1];
        push_shadowed_hints(&command, &toolset_matches, alias, &mut hints);
        let Some(head) = alias.command.first() else {
            return Expansion::Failed {
                message: not_found_message(""),
                hints,
            };
        };
        if alias.is_shell() {
            return expand_shell(alias, &command, &rest);
        }
        if stack.contains(head) {
            let mut cycle = stack.clone();
            cycle.push(head.clone());
            return Expansion::Failed {
                message: terminal::pht(
                    "Alias definitions form a cycle which can not be resolved: %s.",
                    &[PhtArg::Text(&cycle.join(" -> "))],
                ),
                hints,
            };
        }
        stack.push(head.clone());
        if stack.len() >= STACK_LIMIT {
            return Expansion::Failed {
                message: terminal::pht(
                    "Alias definitions form an unreasonably deep stack. A chain of aliases may not resolve more than %s times.",
                    &[PhtArg::Int(STACK_LIMIT as i64)],
                ),
                hints,
            };
        }
        hints.push(terminal::pht(
            "%s %s -> %s %s %s",
            &[
                PhtArg::Text(TOOLSET),
                PhtArg::Text(&command),
                PhtArg::Text(TOOLSET),
                PhtArg::Text(head),
                PhtArg::Text(&alias.command[1..].join(" ")),
            ],
        ));
        let mut next = alias.command[1..].to_vec();
        next.extend(rest);
        rest = next;
        command = head.clone();
    }
}

/// Resolve a command with no same-toolset alias: suggest cross-toolset
/// matches, then pass through when the user typed it directly (so the
/// parser can spellcheck it) or fail when an alias chain led here.
fn expand_no_match(
    aliases: &[Alias],
    command: &str,
    rest: &[String],
    hints: &mut Vec<String>,
    stack: &[String],
) -> Expansion {
    for alias in aliases.iter().filter(|alias| alias.trigger == command) {
        hints.push(terminal::pht(
            "No \"%s %s\" alias is defined, did you mean \"%s %s\"?",
            &[
                PhtArg::Text(TOOLSET),
                PhtArg::Text(command),
                PhtArg::Text(&alias.toolset),
                PhtArg::Text(command),
            ],
        ));
    }
    if stack.is_empty() {
        return dispatch_of(command, rest, std::mem::take(hints));
    }
    Expansion::Failed {
        message: not_found_message(command),
        hints: std::mem::take(hints),
    }
}

fn not_found_message(command: &str) -> String {
    terminal::pht(
        "Alias resolved to \"%s\", but this is not a valid workflow or alias name. This alias or workflow might have previously existed and been removed.",
        &[PhtArg::Text(command)],
    )
}

/// Hints for same-trigger definitions shadowed by the winning (last)
/// definition, mirroring the engine's `EFFECT_IGNORED` messages.
fn push_shadowed_hints(
    command: &str,
    toolset_matches: &[&Alias],
    winner: &Alias,
    hints: &mut Vec<String>,
) {
    if toolset_matches.len() <= 1 {
        return;
    }
    hints.push(terminal::pht(
        "Multiple configuration sources define an alias for \"%s %s\". The last definition in the most specific source (\"%s\") will be used.",
        &[
            PhtArg::Text(TOOLSET),
            PhtArg::Text(command),
            PhtArg::Text(&winner.source),
        ],
    ));
    for ignored in &toolset_matches[..toolset_matches.len() - 1] {
        hints.push(terminal::pht(
            "A definition of \"%s %s\" in \"%s\" will be ignored.",
            &[
                PhtArg::Text(TOOLSET),
                PhtArg::Text(command),
                PhtArg::Text(&ignored.source),
            ],
        ));
    }
}

/// Resolve a shell alias: the `!`-stripped head plus alias and user
/// arguments, mirroring the engine's `EFFECT_SHELL`.
fn expand_shell(alias: &Alias, command: &str, rest: &[String]) -> Expansion {
    let head = &alias.command[0];
    let mut shell = vec![head[1..].to_string()];
    shell.extend(alias.command[1..].iter().cloned());
    shell.extend(rest.iter().cloned());
    let hint = terminal::pht(
        "%s %s -> $ %s",
        &[
            PhtArg::Text(TOOLSET),
            PhtArg::Text(command),
            PhtArg::Text(&shell_display(&shell)),
        ],
    );
    Expansion::Shell { argv: shell, hint }
}

/// Quote one shell-display word, `escapeshellarg()`-like: bare when it
/// needs no quoting, single-quoted otherwise. Display-only.
fn shell_quote(text: &str) -> String {
    let bare = !text.is_empty()
        && text.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b'=' | b','
                )
        });
    if bare {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Render a shell argv for hints, mirroring `csprintf('%Ls', ...)`.
fn shell_display(argv: &[String]) -> String {
    argv.iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<String>>()
        .join(" ")
}

/// Last-wins effective `arc` aliases, sorted by trigger.
#[must_use]
pub fn effective_aliases(aliases: &[Alias]) -> Vec<&Alias> {
    let mut effective: Vec<&Alias> = Vec::new();
    for alias in aliases.iter().filter(|alias| alias.toolset == TOOLSET) {
        if let Some(known) = effective
            .iter_mut()
            .find(|known| known.trigger == alias.trigger)
        {
            *known = alias;
        } else {
            effective.push(alias);
        }
    }
    effective.sort_by(|left, right| left.trigger.cmp(&right.trigger));
    effective
}

/// Render listed aliases as a console table, mirroring the old workflow's
/// `printAliases()`: `arc <trigger>`, the command, and the type.
#[must_use]
pub fn render_list(aliases: &[&Alias]) -> String {
    if aliases.is_empty() {
        return format!(
            "{}\n",
            terminal::pht("You have not defined any aliases yet.", &[])
        );
    }
    let alias_title = terminal::pht("Alias", &[]);
    let command_title = terminal::pht("Command", &[]);
    let type_title = terminal::pht("Type", &[]);
    let shell_kind = terminal::pht("Shell Command", &[]);
    let arc_kind = terminal::pht("Arcanist Command", &[]);
    let mut rows = vec![vec![
        terminal::TableCell::Text(alias_title.clone()),
        terminal::TableCell::Text(command_title.clone()),
        terminal::TableCell::Text(type_title.clone()),
    ]];
    for alias in aliases {
        let joined = alias.command.join(" ");
        let (command, kind) = if alias.is_shell() {
            (joined[1..].to_string(), shell_kind.clone())
        } else {
            (format!("arc {joined}"), arc_kind.clone())
        };
        rows.push(vec![
            terminal::TableCell::Text(format!("arc {}", alias.trigger)),
            terminal::TableCell::Text(command),
            terminal::TableCell::Text(kind),
        ]);
    }
    terminal::console_table(&[&alias_title, &command_title, &type_title], &rows)
}

/// Build the `--json` document: new-format storage dictionaries.
#[must_use]
pub fn aliases_json(aliases: &[&Alias]) -> serde_json::Value {
    serde_json::Value::Array(aliases.iter().map(|alias| alias.storage_dict()).collect())
}

/// Parsed `alias` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Emit JSON instead of the table (listing only).
    pub json: bool,
    /// Wildcard positional arguments.
    pub positional: Vec<String>,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut json = false;
    let mut positional = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            positional.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "-h" || arg == "--help" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
    }
    Ok(Some(Options { json, positional }))
}

/// Stdout text plus invalid-alias warnings (which go to stderr).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Reject alias triggers that collide with builtins, mirroring
/// `validateAliasTrigger()`.
pub fn validate_trigger(trigger: &str) -> Result<(), String> {
    if is_builtin_command(trigger) {
        return Err(terminal::pht(
            "You can not define an alias for \"%s\" because it is a builtin workflow for the current toolset (\"%s\"). The \"alias\" workflow can only define new commands as aliases; it can not redefine existing commands to mean something else.",
            &[PhtArg::Text(trigger), PhtArg::Text(TOOLSET)],
        ));
    }
    Ok(())
}

/// Normalize the user's stored aliases to new-format entries, converting
/// old-dict entries (PHP modernizes them on write) and preserving invalid
/// entries verbatim so managing aliases never deletes unknown data.
fn user_entries(
    doc: &serde_json::Value,
    source: &str,
) -> Result<(Vec<serde_json::Value>, Vec<String>), String> {
    let Some(stored) = doc.get("aliases") else {
        return Ok((Vec::new(), Vec::new()));
    };
    if let Some(items) = stored.as_array() {
        let (_, warnings) = aliases_from_storage(stored, source);
        return Ok((items.clone(), warnings));
    }
    let Some(map) = stored.as_object() else {
        return Err(terminal::pht("Expected a list or dictionary!", &[]));
    };
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    for (key, item) in map {
        match parse_entry(key, item) {
            Ok(alias) => entries.push(alias.storage_dict()),
            Err(detail) => {
                entries.push(item.clone());
                warnings.push(invalid_warning(source, &detail));
            }
        }
    }
    Ok((entries, warnings))
}

/// True when a normalized entry defines `trigger` for this toolset.
fn matches_target(entry: &serde_json::Value, trigger: &str) -> bool {
    entry.get("trigger").and_then(|v| v.as_str()) == Some(trigger)
        && entry.get("toolset").and_then(|v| v.as_str()) == Some(TOOLSET)
}

/// String command of a normalized new-format entry, if well-formed.
fn entry_command(entry: &serde_json::Value) -> Option<Vec<String>> {
    entry
        .get("command")?
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_string))
        .collect::<Option<Vec<String>>>()
}

/// List effective aliases for `dir` and `user_file`.
pub fn execute_list(dir: &Path, user_file: Option<&Path>, json: bool) -> Result<Report, String> {
    let (aliases, warnings) = load_aliases_in(dir, user_file)?;
    let effective = effective_aliases(&aliases);
    let text = if json {
        let mut doc = serde_json::to_string_pretty(&aliases_json(&effective))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        doc.push('\n');
        doc
    } else {
        render_list(&effective)
    };
    Ok(Report { text, warnings })
}

/// Define `trigger` as `command` in `user_file`, replacing any previous
/// same-toolset definition (which the engine would shadow anyway).
pub fn execute_create(
    user_file: &Path,
    trigger: &str,
    command: &[String],
) -> Result<Report, String> {
    validate_trigger(trigger)?;
    let mut doc = set_config::read_user_doc(user_file)?;
    let source = format!("user ({})", user_file.display());
    let (mut entries, warnings) = user_entries(&doc, &source)?;
    entries.retain(|entry| !matches_target(entry, trigger));
    entries.push(
        Alias {
            toolset: TOOLSET.to_string(),
            trigger: trigger.to_string(),
            command: command.to_vec(),
            source: source.clone(),
        }
        .storage_dict(),
    );
    doc.as_object_mut()
        .expect("read_user_doc object")
        .insert("aliases".to_string(), serde_json::Value::Array(entries));
    set_config::write_user_doc(user_file, &doc)?;
    let joined = command.join(" ");
    let message = if is_shell_command(command) {
        terminal::pht(
            "Aliased \"%s\" to shell command \"%s\".",
            &[
                PhtArg::Text(&format!("arc {trigger}")),
                PhtArg::Text(&joined[1..]),
            ],
        )
    } else {
        terminal::pht(
            "Aliased \"%s\" to \"%s\".",
            &[
                PhtArg::Text(&format!("arc {trigger}")),
                PhtArg::Text(&format!("arc {joined}")),
            ],
        )
    };
    Ok(Report {
        text: format!("{message}\n"),
        warnings,
    })
}

/// Delete `trigger` from `user_file` after confirmation, mirroring the
/// old workflow's `removeAlias()`. Only user-scope aliases are managed.
pub fn execute_delete(
    user_file: &Path,
    trigger: &str,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<Report, String> {
    let mut doc = set_config::read_user_doc(user_file)?;
    let source = format!("user ({})", user_file.display());
    let (mut entries, warnings) = user_entries(&doc, &source)?;
    let current = entries
        .iter()
        .rev()
        .find(|entry| matches_target(entry, trigger))
        .and_then(entry_command);
    let Some(command) = current else {
        let message = terminal::pht("No alias \"%s\" to remove.", &[PhtArg::Text(trigger)]);
        return Ok(Report {
            text: format!("{message}\n"),
            warnings,
        });
    };
    let joined = command.join(" ");
    let shown = if is_shell_command(&command) {
        terminal::pht(
            "\"%s\" is currently aliased to shell command \"%s\".",
            &[
                PhtArg::Text(&format!("arc {trigger}")),
                PhtArg::Text(&joined[1..]),
            ],
        )
    } else {
        terminal::pht(
            "\"%s\" is currently aliased to \"%s\".",
            &[
                PhtArg::Text(&format!("arc {trigger}")),
                PhtArg::Text(&format!("arc {joined}")),
            ],
        )
    };
    let query = terminal::pht("Delete this alias?", &[]);
    let confirmed = prompt::console_confirm_with_tty(input, &query, "[y/N]", true, stdin_tty)?;
    if !confirmed {
        return Err(prompt::ABORT.to_string());
    }
    entries.retain(|entry| !matches_target(entry, trigger));
    doc.as_object_mut()
        .expect("read_user_doc object")
        .insert("aliases".to_string(), serde_json::Value::Array(entries));
    set_config::write_user_doc(user_file, &doc)?;
    let removed = terminal::pht(
        "Removed alias \"%s\".",
        &[PhtArg::Text(&format!("arc {trigger}"))],
    );
    Ok(Report {
        text: format!("{shown}\n{removed}\n"),
        warnings,
    })
}

fn user_file_or_error() -> Result<PathBuf, String> {
    config::arcrc_path()
        .ok_or_else(|| "cannot locate ~/.arcrc (HOME/USERPROFILE is not set)".to_string())
}

fn print_report(report: &Report) {
    for warning in &report.warnings {
        eprintln!("ALIAS: {warning}");
    }
    print!("{}", report.text);
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
    if options.json && !options.positional.is_empty() {
        eprintln!(
            "arc-rs {NAME}: {}",
            terminal::pht(
                "The \"--json\" argument may only be used when listing aliases.",
                &[]
            )
        );
        return ExitCode::FAILURE;
    }
    if options.positional.is_empty() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        return match execute_list(&cwd, config::arcrc_path().as_deref(), options.json) {
            Ok(report) => {
                print_report(&report);
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("arc-rs {NAME}: {message}");
                ExitCode::FAILURE
            }
        };
    }
    if options.positional.len() == 1 {
        let user_file = match user_file_or_error() {
            Ok(path) => path,
            Err(message) => {
                eprintln!("arc-rs {NAME}: {message}");
                return ExitCode::FAILURE;
            }
        };
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        return match execute_delete(
            &user_file,
            &options.positional[0],
            &mut locked,
            prompt::stdin_is_terminal(),
        ) {
            Ok(report) => {
                print_report(&report);
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("arc-rs {NAME}: {message}");
                ExitCode::FAILURE
            }
        };
    }
    let user_file = match user_file_or_error() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    match execute_create(&user_file, &options.positional[0], &options.positional[1..]) {
        Ok(report) => {
            print_report(&report);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "alias_tests.rs"]
mod alias_tests;
