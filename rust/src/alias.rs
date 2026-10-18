//! `arc-rs alias` — create and modify command aliases.
//!
//! Port of `src/toolset/workflow/ArcanistAliasWorkflow.php` (argument
//! dispatch, `--json`, creation), `src/toolset/ArcanistAlias.php` (old dict
//! and new list storage formats), `src/toolset/ArcanistAliasEngine.php`
//! (expansion, [`expand`]), and the `src/runtime/ArcanistRuntime.php`
//! dispatch (shell execution, hint application). The list and delete bodies
//! follow the pre-toolset `src/workflow/ArcanistAliasWorkflow.php`
//! (`70c6045c~1`), since the toolset versions at ceb are `TOOLSETS`
//! stubs returning 1 silently (T1153 CORE-26, kept: PHP's silent
//! stubs; Rust implements both).
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
    amend, anoid, bookmarks, branches, browse, close_revision, commit, conduit, config, cover,
    diff, download, export, get_config, inspect, install_certificate, land, liberate, lint,
    linters, list, look, markers, paste, patch, prompt, set_config, tasks, terminal, todo, unit,
    upgrade, upload, version, weld, which, work,
};

/// Subcommand name, matching `arc alias`.
pub const NAME: &str = "alias";

/// Toolset key of this binary (`arc` is single-toolset).
pub const TOOLSET: &str = "arc";

/// Maximum alias-chain depth, mirroring `ArcanistAliasEngine`.
const STACK_LIMIT: usize = 16;

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/toolset/workflow/ArcanistAliasWorkflow.php:17-71`).
const HELP_EXAMPLES: &str = "**alias**\n**alias** __command__ __target__ -- [__arguments__]";
const HELP_SYNOPSIS: &str = "Create and modify command aliases.";
const HELP_TEXT: &str = "Create an alias from __command__ to __target__ (optionally, with __options__).\n\
\n\
Aliases allow you to create shorthands for commands and sets of flags you\n\
commonly use, like defining \"arc draft\" as a shorthand for \"arc diff --draft\".\n\
\n\
**Creating Aliases**\n\
\n\
You can define \"arc draft\" as a shorthand for \"arc diff --draft\" like this:\n\
\n\
\x20 $ arc alias draft diff -- --draft\n\
\n\
Now, when you run \"arc draft\", the command will function like\n\
\"arc diff --draft\".\n\
\n\
<bg:yellow> NOTE: </bg> Make sure you use \"--\" before specifying any flags you\n\
want to pass to the command! Otherwise, the flags will be interpreted as flags\n\
to \"arc alias\".\n\
\n\
**Listing Aliases**\n\
\n\
Without any arguments, \"arc alias\" will list aliases.\n\
\n\
**Removing Aliases**\n\
\n\
To remove an alias, run:\n\
\n\
\x20 $ arc alias <alias-name>\n\
\n\
You will be prompted to remove the alias.\n\
\n\
**Shell Commands**\n\
\n\
If you begin an alias with \"!\", the remainder of the alias will be invoked as\n\
a shell command. For example, if you want to implement \"arc ls\", you can do so\n\
like this:\n\
\n\
\x20 $ arc alias ls '!ls'\n\
\n\
When run, \"arc ls\" will now behave like \"ls\".\n\
\n\
**Multiple Toolsets**\n\
\n\
This workflow supports any toolset, even though the examples in this help text\n\
use \"arc\". If you are working with another toolset, use the binary for that\n\
toolset define aliases for it:\n\
\n\
\x20 $ phage alias ...\n\
\n\
Aliases are bound to the toolset which was used to define them. If you define\n\
an \"arc draft\" alias, that does not also define a \"phage draft\" alias.\n\
\n\
**Builtins**\n\
\n\
You can not overwrite the behavior of builtin workflows, including \"alias\"\n\
itself, and if you install a new workflow it will take precedence over any\n\
existing aliases with the same name.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] =
    &[("json", None, None, "Output aliases in JSON format.")];

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
        true,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(crate::terminal::ansi_enabled())
}

/// Every command `main` dispatches without aliases: the `COMMANDS` table
/// plus `call-conduit` (parsed by `classify`) and `help` (also handled by
/// `classify`, a builtin workflow in PHP). An alias can never shadow these.
/// `call-conduit` is unshadowable: PHP registers `ArcanistCallConduitWorkflow`
/// (`extends ArcanistArcWorkflow`, name `call-conduit`) in `$workflows`, so the
/// engine's `isset($workflows[$command])` check always wins over aliases.
const BUILTINS: &[&str] = &[
    NAME,
    amend::NAME,
    anoid::NAME,
    bookmarks::NAME,
    branches::NAME,
    browse::NAME,
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
    version::NAME,
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

/// The 21 workflows the toolset runtime serves in-process at ceb
/// (`newWorkflows()`: `ArcanistArcWorkflow` subclasses plus the five
/// `src/toolset/workflow/` commands), alphabetical. The classic
/// subprocess serves the other 16; `markers` is abstract in PHP
/// (`browse` is concrete, T1153 CORE-66) and `arc` itself is not a
/// command.
#[must_use]
pub fn toolset_names() -> &'static [&'static str] {
    &[
        "alias",
        "amend",
        "anoid",
        "bookmarks",
        "branches",
        "browse",
        "call-conduit",
        "download",
        "help",
        "inspect",
        "land",
        "liberate",
        "look",
        "paste",
        "prompts",
        "shell-complete",
        "upload",
        "upgrade",
        "version",
        "weld",
        "work",
    ]
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
/// `EFFECT_CONFIGURATION` message (T1153 CORE-24, kept: PHP
/// references the undefined constant and fatals; Rust warns and
/// ignores).
fn invalid_warning(source: &str, detail: &str) -> String {
    terminal::pht(
        "Configuration source (\"%s\") defines an invalid alias, which will be ignored: %s",
        &[PhtArg::Text(source), PhtArg::Text(detail)],
    )
}

/// Parse one source's `aliases` storage value (old dict or new list),
/// collecting invalid-entry warnings. A non-array whole value is a hard
/// error, mirroring PHP: `ArcanistAliasesConfigOption` throws `Expected a
/// list or dictionary!`, which `validateConfiguration()` wraps as
/// `Configuration value ("aliases") ... is not valid` before any dispatch,
/// and `ArcanistRuntime::execute()` does not catch it (PHP-fatal).
pub fn aliases_from_storage(
    value: &serde_json::Value,
    source: &str,
) -> Result<(Vec<Alias>, Vec<String>), String> {
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
            return Err(terminal::pht(
                "Configuration value (\"%s\") defined in source \"%s\" is not valid: %s",
                &[
                    PhtArg::Text("aliases"),
                    PhtArg::Text(source),
                    PhtArg::Text(&detail),
                ],
            ));
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
    Ok((aliases, warnings))
}

fn take_source(
    map: &serde_json::Map<String, serde_json::Value>,
    source: &str,
    aliases: &mut Vec<Alias>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if let Some(value) = map.get("aliases") {
        let (mut found, mut found_warnings) = aliases_from_storage(value, source)?;
        aliases.append(&mut found);
        warnings.append(&mut found_warnings);
    }
    Ok(())
}

/// Parse aliases from an already-read system config map into the
/// accumulators. Split from [`load_aliases_in`]'s `/etc` read so hermetic
/// tests can pin the bad-value error path with a fixture map (writing
/// `/etc/arcconfig` needs privileges).
fn take_system_source(
    system: &serde_json::Map<String, serde_json::Value>,
    system_path: &Path,
    aliases: &mut Vec<Alias>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    take_source(
        system,
        &format!("system ({})", system_path.display()),
        aliases,
        warnings,
    )
}

/// Load aliases from every file source, least to most specific (system,
/// user, project, local), mirroring the PHP source order. Later entries
/// win during resolution. A non-array `aliases` value in any source is a
/// hard error (PHP-fatal at startup validation); invalid single entries
/// warn and are ignored.
pub fn load_aliases_in(
    dir: &Path,
    user_file: Option<&Path>,
) -> Result<(Vec<Alias>, Vec<String>), String> {
    let mut aliases = Vec::new();
    let mut warnings = Vec::new();
    let system_path = get_config::system_config_path();
    let system = get_config::read_system_config()?;
    take_system_source(&system, &system_path, &mut aliases, &mut warnings)?;
    if let Some(path) = user_file {
        // T1153 CORE-28: toolset workflows read the user file through the
        // `ArcanistUserConfigurationSource` transform (section wins).
        let user = get_config::read_user_file_toolset(path)?;
        take_source(
            &user,
            &format!("user ({})", path.display()),
            &mut aliases,
            &mut warnings,
        )?;
    }
    let (project_file, project) = get_config::read_project_config(dir)?;
    let project_source = project_file.map_or_else(
        || "project".to_string(),
        |file| format!("project ({})", file.display()),
    );
    take_source(&project, &project_source, &mut aliases, &mut warnings)?;
    let local = get_config::read_local_config(dir)?;
    take_source(&local, "local", &mut aliases, &mut warnings)?;
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
        // PHP renders the tail with `(string)csprintf('%LR', $alias_argv)`
        // (`EFFECT_ALIAS`): minimal `%R` quoting per word.
        let display_argv = effect_display(&alias.command[1..]);
        hints.push(terminal::pht(
            "%s %s -> %s %s %s",
            &[
                PhtArg::Text(TOOLSET),
                PhtArg::Text(&command),
                PhtArg::Text(TOOLSET),
                PhtArg::Text(head),
                PhtArg::Text(&display_argv),
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

/// Quote one display word with PHP `%R` ("readable") semantics
/// (`xsprintf_command`): bare when it matches `^[a-zA-Z0-9:/@._+-]+$`,
/// `escapeshellarg()`-quoted otherwise. Display-only.
fn shell_quote(text: &str) -> String {
    let bare = !text.is_empty()
        && text.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b':' | b'/' | b'@' | b'.' | b'_' | b'+' | b'-')
        });
    if bare {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Render an argv the way PHP `%LR` does: minimal quoting, one word
/// per argument. Used for `EFFECT_ALIAS` tails.
fn effect_display(argv: &[String]) -> String {
    argv.iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<String>>()
        .join(" ")
}

/// Render an argv for shell-alias hints, one `%Ls`-quoted word per
/// argument: PHP `escapeshellarg()`s every word even when bare-safe.
/// Display uses Unix quoting on all platforms. Display-only; execution
/// uses the raw argv.
fn shell_display(argv: &[String]) -> String {
    argv.iter()
        .map(|word| format!("'{}'", word.replace('\'', "'\\''")))
        .collect::<Vec<String>>()
        .join(" ")
}

/// How a shell-alias child terminated: a normal exit code, or death by
/// signal. Mirrors the `proc_close()` result `phutil_passthru()` returns
/// from `PhutilExecPassthru::executeCommand()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellStatus {
    Exited(i32),
    Signaled(i32),
}

/// Build a [`ShellStatus`] from a finished child. Unix only in practice
/// (only Unix reports fatal signals); elsewhere a missing code maps to
/// `Signaled(-1)`.
#[cfg(unix)]
#[must_use]
pub fn shell_status_from_exit_status(status: &std::process::ExitStatus) -> ShellStatus {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        ShellStatus::Exited(code)
    } else {
        ShellStatus::Signaled(status.signal().unwrap_or(-1))
    }
}

/// Build a [`ShellStatus`] from a finished child (non-Unix fallback).
#[cfg(not(unix))]
#[must_use]
pub fn shell_status_from_exit_status(status: &std::process::ExitStatus) -> ShellStatus {
    status
        .code()
        .map_or(ShellStatus::Signaled(-1), ShellStatus::Exited)
}

/// Map a shell-alias termination to an exit code, mirroring
/// `phutil_passthru()` returning `proc_close()` verbatim (`exit($err)`):
/// normal exits pass through, unrepresentable codes fail. Signal deaths
/// map to `FAILURE`: PHP returns the raw wait status there (e.g. the bare
/// signal number), `-1` under `--enable-sigchild` (where the manual calls
/// the return "undefined"), or `128+N` when `sh` interposes between
/// `proc_open()` and the real child — there is no single PHP value to
/// match, so Rust uses a fixed failure.
#[must_use]
pub fn shell_status_exit_code(status: ShellStatus) -> ExitCode {
    match status {
        ShellStatus::Exited(code) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        ShellStatus::Signaled(_) => ExitCode::FAILURE,
    }
}

/// One stderr hint line, mirroring `ArcanistLogEngine::writeHint()`
/// (cyan label, message, no trailing newline — the caller prints lines).
fn hint_notice(label: &str, message: &str) -> String {
    terminal::format_log("cyan", label, message, terminal::ansi_enabled())
        .trim_end()
        .to_string()
}

/// Build the spawner argv for a resolved shell alias: on Unix the words
/// run through `sh -c` (PHP `proc_open()` mediates the shell, so
/// metacharacters expand and unknown commands exit 127); elsewhere the
/// words spawn directly (PHP `bypass_shell` on Windows).
fn shell_spawn_argv(shell_argv: &[String]) -> (String, Vec<String>) {
    #[cfg(unix)]
    let spawned = (
        "sh".to_string(),
        vec!["-c".to_string(), terminal::shell_join(shell_argv)],
    );
    #[cfg(not(unix))]
    let spawned = match shell_argv.split_first() {
        Some((program, args)) => (program.clone(), args.to_vec()),
        None => (String::new(), Vec::new()),
    };
    spawned
}

/// Run a resolved shell alias through an injected spawner, mirroring
/// `ArcanistRuntime::executeShellAlias()` (hint first, then execute).
/// Returns the exit code plus stderr lines in order; the caller prints
/// each line. A spawn failure reports `cannot execute shell alias` and
/// fails (T1153 CORE-29, kept: PHP throws an uncaught `Failed to
/// passthru proc_open()` here, which Rust softens to a reported
/// failure).
pub fn run_shell_alias_with(
    shell_argv: &[String],
    hint: &str,
    spawn: impl FnOnce(&str, &[String]) -> Result<ShellStatus, String>,
) -> (ExitCode, Vec<String>) {
    let mut notices = vec![hint_notice("SHELL ALIAS", hint)];
    let (program, args) = shell_spawn_argv(shell_argv);
    match spawn(&program, &args) {
        Ok(status) => (shell_status_exit_code(status), notices),
        Err(error) => {
            notices.push(format!("cannot execute shell alias: {error}"));
            (ExitCode::FAILURE, notices)
        }
    }
}

/// Injectable equivalent of `main`'s alias-expansion step: expand `argv`
/// through loaded aliases, running shell aliases via `spawn`. `load`
/// supplies aliases plus configuration warnings (`None` means unreadable
/// configuration, which passes through untouched so each command can
/// surface its own config errors). Returns the argv to dispatch (or the
/// handling exit code) plus stderr lines in order; the caller prints each
/// line. Leading-dash invocations never expand (PHP consumes toolset
/// arguments before aliases run).
pub fn apply_expansion_with(
    argv: Vec<String>,
    load: impl FnOnce() -> Option<(Vec<Alias>, Vec<String>)>,
    spawn: impl FnOnce(&[String]) -> Result<ShellStatus, String>,
) -> (Result<Vec<String>, ExitCode>, Vec<String>) {
    if argv.first().is_none_or(|first| first.starts_with('-')) {
        return (Ok(argv), Vec::new());
    }
    let Some((aliases, warnings)) = load() else {
        return (Ok(argv), Vec::new());
    };
    // T1153 CORE-23: every effect message surfaces as a cyan `ALIAS`
    // hint on stderr (`applyAliasEffects()` + `writeHint()`).
    let mut notices: Vec<String> = warnings.iter().map(|w| hint_notice("ALIAS", w)).collect();
    match expand(&argv, &aliases, is_builtin_command) {
        Expansion::Dispatch { argv: next, hints } => {
            notices.extend(hints.iter().map(|h| hint_notice("ALIAS", h)));
            (Ok(next), notices)
        }
        Expansion::Shell { argv: shell, hint } => {
            let (code, mut shell_notices) = run_shell_alias_with(&shell, &hint, |program, args| {
                let mut full = vec![program.to_string()];
                full.extend(args.iter().cloned());
                spawn(&full)
            });
            notices.append(&mut shell_notices);
            (Err(code), notices)
        }
        // T1153 CORE-27 (kept): a broken chain (dangling, cyclic,
        // too deep) fails fast with the cyan hints, instead of PHP's
        // legacy fallthrough to the unknown-command error.
        Expansion::Failed { message, hints } => {
            notices.extend(hints.iter().map(|h| hint_notice("ALIAS", h)));
            notices.push(hint_notice("ALIAS", &message));
            (Err(ExitCode::FAILURE), notices)
        }
    }
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
/// Returns `Ok(None)` when `--help` was requested. A lone `-` is
/// positional (T1153 RB-D6); `--json` rejects repeats and `=value`
/// with the toolset texts (T1153 RB-D2/T1153 RB-D25). The unknown-flag text
/// stays the legacy `unknown flag` wording (locked by contract tests;
/// PHP reports `Argument "--x" is unrecognized. ...`).
pub fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    parse_args_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_args`] with an injected TTY flag (deterministic tests).
/// T1153 B21/B23: piped runs without `--` fail with
/// `MISSING_TERMINATOR` (checked after unknown-flag scanning, like
/// PHP's `parseFull()`, but before success). `--help` without `--`
/// when piped reports the terminator. Vs ceb
/// `ArcanistRuntime.php:80-86,172-188` and
/// `PhutilArgumentParser.php:331-336`.
fn parse_args_with_tty(argv: &[String], stdin_tty: bool) -> Result<Option<Options>, String> {
    let missing = crate::help::terminator_missing(argv, stdin_tty);
    let mut json = false;
    let mut positional = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            positional.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--json" {
            if json {
                return Err("Argument \"--json\" was provided twice.".to_string());
            }
            json = true;
        } else if arg == "-h" || arg == "--help" {
            if missing {
                return Err(crate::help::MISSING_TERMINATOR.to_string());
            }
            return Ok(None);
        } else if arg == "-" {
            positional.push(arg.clone());
        } else if arg.starts_with('-') {
            if arg.starts_with("--json=") {
                return Err("Argument \"--json\" does not take a parameter.".to_string());
            }
            return Err(format!("unknown flag '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
    }
    if missing {
        return Err(crate::help::MISSING_TERMINATOR.to_string());
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
        let (_, warnings) = aliases_from_storage(stored, source)?;
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

/// Define `trigger` as `command` in `user_file`, appending silently
/// like `runCreateAlias()` (T1153 CORE-25): duplicates accumulate (the
/// engine last-wins with `IGNORED` hints at expansion), shell aliases
/// included, and nothing confirms on stdout.
pub fn execute_create(
    user_file: &Path,
    trigger: &str,
    command: &[String],
) -> Result<Report, String> {
    validate_trigger(trigger)?;
    let mut doc = set_config::read_user_doc(user_file)?;
    let source = format!("user ({})", user_file.display());
    let (mut entries, warnings) = user_entries(&doc, &source)?;
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
    Ok(Report {
        text: String::new(),
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

fn print_report(report: &Report) {
    for warning in &report.warnings {
        eprintln!("ALIAS: {warning}");
    }
    print!("{}", report.text);
}

/// Entry point: `argv` excludes the subcommand name.
///
/// `alias` is a toolset workflow, so errors use the toolset arms:
/// parse errors are bold usage errors (the `parse()` catch), while
/// `runWorkflow()` usage errors (bad `--json`, builtin triggers)
/// carry the red label. Config I/O failures escape like uncaught
/// exceptions (fatal); a declined delete aborts like
/// `ArcanistUserAbortException`.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            // T1153 B21/B23: the terminator failure prints the three
            // cyan hints first (like `ArcanistRuntime.php:172-188`)
            // and reports through the toolset usage arm (exit 1);
            // other parse errors keep the bold parse-error arm.
            if message == crate::help::MISSING_TERMINATOR {
                crate::help::print_terminator_hints(crate::terminal::ansi_enabled());
                return conduit::report(conduit::EnvelopeKind::ToolsetUsage, &message);
            }
            return conduit::report(conduit::EnvelopeKind::ToolsetParseError, &message);
        }
    };
    if options.json && !options.positional.is_empty() {
        return conduit::report(
            conduit::EnvelopeKind::ToolsetUsage,
            &terminal::pht(
                "The \"--json\" argument can only be used to list aliases.",
                &[],
            ),
        );
    }
    if options.positional.is_empty() {
        return run_list(&options);
    }
    if options.positional.len() == 1 {
        return run_delete(&options);
    }
    run_create_with(config::arcrc_path().as_deref(), &options)
}

fn run_list(options: &Options) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_list_with(&cwd, config::arcrc_path().as_deref(), options)
}

/// [`run_list`] with an explicit working copy and credential file
/// (test seam: hermetic tests stage a fake arcrc, a hostile one, or
/// `None` instead of touching HOME).
fn run_list_with(dir: &Path, user_file: Option<&Path>, options: &Options) -> ExitCode {
    match execute_list(dir, user_file, options.json) {
        Ok(report) => {
            print_report(&report);
            ExitCode::SUCCESS
        }
        Err(message) => conduit::report(conduit::EnvelopeKind::Fatal, &message),
    }
}

fn run_delete(options: &Options) -> ExitCode {
    let user_file = config::arcrc_path();
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    run_delete_with(
        user_file.as_deref(),
        &options.positional[0],
        &mut locked,
        prompt::stdin_is_terminal(),
    )
}

/// [`run_delete`] with explicit inputs (test seam: `None` stages the
/// unset-HOME arm, hostile paths stage arcrc failures, and canned
/// stdin plus `stdin_tty` stage declined prompts).
fn run_delete_with(
    user_file: Option<&Path>,
    trigger: &str,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> ExitCode {
    let Some(user_file) = user_file else {
        return conduit::report(
            conduit::EnvelopeKind::Fatal,
            "cannot locate ~/.arcrc (HOME/USERPROFILE is not set)",
        );
    };
    match execute_delete(user_file, trigger, input, stdin_tty) {
        Ok(report) => {
            print_report(&report);
            ExitCode::SUCCESS
        }
        Err(message) => {
            if message == prompt::ABORT {
                conduit::report(conduit::EnvelopeKind::UserAbort, &message)
            } else {
                conduit::report(conduit::EnvelopeKind::Fatal, &message)
            }
        }
    }
}

/// Create runner with an explicit credential file (test seam: like
/// [`run_delete_with`]; `run` passes the ambient `~/.arcrc`, so a
/// builtin trigger still fails before any filesystem read).
fn run_create_with(user_file: Option<&Path>, options: &Options) -> ExitCode {
    if let Err(message) = validate_trigger(&options.positional[0]) {
        return conduit::report(conduit::EnvelopeKind::ToolsetUsage, &message);
    }
    let Some(user_file) = user_file else {
        return conduit::report(
            conduit::EnvelopeKind::Fatal,
            "cannot locate ~/.arcrc (HOME/USERPROFILE is not set)",
        );
    };
    match execute_create(user_file, &options.positional[0], &options.positional[1..]) {
        Ok(report) => {
            print_report(&report);
            ExitCode::SUCCESS
        }
        Err(message) => conduit::report(conduit::EnvelopeKind::Fatal, &message),
    }
}

#[cfg(test)]
#[path = "alias_tests.rs"]
mod alias_tests;
