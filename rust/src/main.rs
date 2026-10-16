//! `arc-rs` — Rust port of Arcanist.
//!
//! Tracks T1128. All arc subcommands are implemented and the ported depth
//! is wired into the run paths; remaining work is the coverage push past
//! 80% and removing the PHP tree.

use std::process::ExitCode;

use arc_rs::{
    alias, amend, anoid, argparse, bookmarks, branches, browse, close_revision, commit, conduit,
    config, cover, diff, download, export, get_config, help, inspect, install_certificate, land,
    liberate, lint, linters, list, look, markers, paste, patch, prompts, set_config,
    shell_complete, tasks, terminal, todo, unit, upgrade, upload, version, weld, which, work,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

type CommandFn = fn(&[String]) -> ExitCode;

const COMMANDS: &[(&str, CommandFn)] = &[
    (alias::NAME, alias::run),
    (amend::NAME, amend::run),
    (anoid::NAME, anoid::run),
    (bookmarks::NAME, bookmarks::run),
    (branches::NAME, branches::run),
    (browse::NAME, browse::run),
    (close_revision::NAME, close_revision::run),
    (commit::NAME, commit::run),
    (cover::NAME, cover::run),
    (diff::NAME, diff::run),
    (download::NAME, download::run),
    (export::NAME, export::run),
    (get_config::NAME, get_config::run),
    (inspect::NAME, inspect::run),
    (install_certificate::NAME, install_certificate::run),
    (land::NAME, land::run),
    (liberate::NAME, liberate::run),
    (lint::NAME, lint::run),
    (linters::NAME, linters::run),
    (list::NAME, list::run),
    (look::NAME, look::run),
    (markers::NAME, markers::run),
    (paste::NAME, paste::run),
    (patch::NAME, patch::run),
    (set_config::NAME, set_config::run),
    (tasks::NAME, tasks::run),
    (todo::NAME, todo::run),
    (unit::NAME, unit::run),
    (upgrade::NAME, upgrade::run),
    (upload::NAME, upload::run),
    (version::NAME, version::run),
    (weld::NAME, weld::run),
    (which::NAME, which::run),
    (work::NAME, work::run),
];

/// Bare command list. Test-locked contract text; the actual `arc help`
/// output renders the full workflow reference (`render_help_bare`).
#[allow(dead_code)]
fn usage() -> String {
    "usage: arc [--version] [--help] <command>\n\ncommands (see <command> --help):\n  alias\n  amend\n  anoid\n  bookmarks\n  branches\n  browse\n  call-conduit\n  close-revision\n  commit\n  cover\n  diff\n  download\n  export\n  get-config\n  inspect\n  install-certificate\n  land\n  liberate\n  lint\n  linters\n  list\n  look\n  markers\n  paste\n  patch\n  set-config\n  tasks\n  todo\n  unit\n  upgrade\n  upload\n  version\n  weld\n  which\n  work\n"
        .to_string()
}

#[derive(Debug, PartialEq, Eq)]
struct CallArgs {
    method: String,
    conduit_uri: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
    CallConduit(CallArgs),
    CallConduitHelp,
    Unknown(String),
    Missing,
    Error(String),
}

// T1153 R10: the old `CALL_CONDUIT_USAGE` duplicate is gone;
// `arc call-conduit --help` renders from the workflow reference
// table (`HELP_ENTRIES`), like every other command.

/// Parse `call-conduit` arguments (PHP
/// `ArcanistCallConduitWorkflow.php:31-44`): `--conduit-uri` plus a
/// wildcard `method`, exactly one required. Returns the spelling
/// notices alongside the outcome: PHP logs them during the scan, so
/// they print even when parsing later fails.
fn parse_call_conduit_with(
    rest: &[String],
    autocorrect: bool,
) -> (Result<CallArgs, String>, Vec<String>) {
    let parsed = match argparse::parse_toolset(
        rest,
        &[argparse::FlagSpec::valued("conduit-uri", "uri")],
        true,
        autocorrect,
    ) {
        Ok(parsed) => parsed,
        Err(message) => return (Err(message), Vec::new()),
    };
    let notices = parsed.notices.clone();
    let [method] = parsed.positionals.as_slice() else {
        return (
            Err("Provide exactly one Conduit method name to call.".to_string()),
            notices,
        );
    };
    (
        Ok(CallArgs {
            method: method.clone(),
            conduit_uri: parsed.value("conduit-uri").map(str::to_string),
        }),
        notices,
    )
}

fn parse_call_conduit(rest: &[String]) -> Result<CallArgs, String> {
    use std::io::IsTerminal;
    let (result, notices) = parse_call_conduit_with(rest, std::io::stdin().is_terminal());
    let ansi = terminal::ansi_enabled();
    for notice in &notices {
        eprint!(
            "{}",
            terminal::tsprintf_ansi("%s\n", &[terminal::TVal::Text(notice)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default()
        );
    }
    result
}

fn classify(argv: &[String]) -> Action {
    let Some(cmd) = argv.first() else {
        return Action::Missing;
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => Action::Help,
        "-V" | "--version" => Action::Version,
        "call-conduit" => {
            if call_conduit_wants_help(&argv[1..]) {
                Action::CallConduitHelp
            } else {
                match parse_call_conduit(&argv[1..]) {
                    Ok(args) => Action::CallConduit(args),
                    Err(msg) => Action::Error(msg),
                }
            }
        }
        c => Action::Unknown(c.to_string()),
    }
}

/// Read the params JSON blob from stdin (PHP `readStdin` + decode,
/// `ArcanistCallConduitWorkflow.php:46-52`).
fn read_params_json<R: std::io::Read>(mut reader: R) -> Result<serde_json::Value, String> {
    let mut text = String::new();
    reader
        .read_to_string(&mut text)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    serde_json::from_str(&text)
        .map_err(|_| "Provide method parameters on stdin as a JSON blob.".to_string())
}

/// Render the `{error, errorMessage, response}` envelope (PHP `:67-72`).
/// Key order matches PHP: `serde_json` maps sort keys the same way.
fn render_envelope(error: Option<(&str, &str)>, response: serde_json::Value) -> String {
    let (code, message) = match error {
        Some((code, info)) => (
            serde_json::Value::String(code.to_string()),
            serde_json::Value::String(info.to_string()),
        ),
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };
    serde_json::to_string_pretty(&serde_json::json!({
        "error": code,
        "errorMessage": message,
        "response": response,
    }))
    .unwrap_or_else(|_| "null".to_string())
}

/// Render an API failure's envelope: the message is the
/// `ConduitClientException` text `"{code}: {info}"` (T1153 CORE-1),
/// with no `<method>` infix at ceb.
fn render_api_envelope(code: &str, info: &str) -> String {
    let message = format!("{code}: {info}");
    render_envelope(Some((code, &message)), serde_json::Value::Null)
}

fn run_call_conduit_with<R: std::io::Read, W: std::io::Write>(
    args: &CallArgs,
    stdin: R,
    stdout: &mut W,
) -> ExitCode {
    // T1153 R4: `call-conduit` is a toolset workflow
    // (`ArcanistCallConduitWorkflow extends ArcanistArcWorkflow`), so
    // every error below escapes `ArcanistRuntime::execute()` into a PHP
    // fatal (exit 255): the stdin error is an `ArcanistUsageException`
    // (`:51-55`), and transport/protocol failures are uncaught
    // `Exception`s. Only `ConduitClientException` is caught, into the
    // JSON envelope with exit 0 (`:57-74`).
    let params = match read_params_json(stdin) {
        Ok(value) => value,
        Err(msg) => return conduit::report(conduit::EnvelopeKind::Fatal, &msg),
    };
    // T1153 CORE-4: `call-conduit` requires no conduit channel or auth,
    // so the engine runs tokenless like PHP's (`newConduitEngine()`
    // with a null token).
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let user_file = config::arcrc_path();
    let client = match config::client_for_tokenless_with(
        args.conduit_uri.as_deref(),
        &cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    ) {
        Ok(client) => client,
        Err(msg) => return conduit::report(conduit::EnvelopeKind::Fatal, &msg),
    };
    // API errors land in the envelope with exit 0, exactly like PHP
    // (`:57-74`); transport failures stay hard errors. T1153 CORE-5:
    // the engine future maps auth failures to login guidance, which
    // is not a `ConduitClientException` — it escapes to the toolset
    // runtime's auth arm (title + body, exit 1), never the envelope.
    // T1153 CORE-1: the envelope message is `"{code}: {info}"`.
    let text = match client.call(&args.method, &params) {
        Ok(result) => render_envelope(None, result),
        Err(error) => {
            if let Some(guidance) = client.auth_guidance_for(&error) {
                return conduit::report_auth(&guidance.title, &guidance.body);
            }
            match error {
                arc_rs::conduit::Error::Api { code, info } => render_api_envelope(&code, &info),
                other => {
                    return conduit::report(conduit::EnvelopeKind::Fatal, &other.to_string());
                }
            }
        }
    };
    // No trailing newline: PHP `echo`s the encoded blob as-is (`:67`).
    if stdout.write_all(text.as_bytes()).is_err() {
        eprintln!("arc: failed to write output");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_call_conduit(args: &CallArgs) -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_call_conduit_with(args, stdin.lock(), &mut stdout.lock())
}

fn find_command(name: &str) -> Option<CommandFn> {
    COMMANDS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, run)| *run)
}

/// Expand `argv` through user aliases before dispatch. Returns the argv
/// to dispatch, or an exit code when expansion fully handles the
/// invocation (shell alias executed or broken chain). Unreadable
/// configuration is a pass-through: each command surfaces its own
/// config errors, so expansion must not break unrelated commands.
fn apply_alias_expansion(argv: Vec<String>) -> Result<Vec<String>, ExitCode> {
    let (outcome, notices) = alias::apply_expansion_with(
        argv,
        || alias::load_aliases().ok(),
        |shell_argv| {
            // Shell words are never empty here (creation requires a
            // command); an empty `sh -c ''` would exit 0 like PHP's
            // `passthru('')` anyway.
            let (program, args) = shell_argv
                .split_first()
                .map(|(program, args)| (program.as_str(), args.to_vec()))
                .unwrap_or(("sh", vec!["-c".to_string(), String::new()]));
            std::process::Command::new(program)
                .args(&args)
                .status()
                .map(|status| alias::shell_status_from_exit_status(&status))
                .map_err(|e| e.to_string())
        },
    );
    for notice in &notices {
        eprintln!("{notice}");
    }
    outcome
}

/// `-h`/`--help` before any `--` separator requests `call-conduit` help
/// (PHP's global argument parser also accepts help flags on any
/// workflow). A `--help` after `--` stays a positional method name, as
/// does one consumed as a `--conduit-uri` value, mirroring the parser.
fn call_conduit_wants_help(rest: &[String]) -> bool {
    let mut skip_next = false;
    for arg in rest.iter().take_while(|arg| arg.as_str() != "--") {
        if skip_next {
            skip_next = false;
        } else if arg == "--conduit-uri" {
            skip_next = true;
        } else if arg == "-h" || arg == "--help" {
            return true;
        }
    }
    false
}

#[allow(dead_code)]
fn find_command_ci(name: &str) -> Option<CommandFn> {
    COMMANDS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, run)| *run)
}

/* -(  Global arguments  )---------------------------------------------- */
// T1153 RB-D5: `workflow_header`/`no_workflow_line` live in the shared
// `help` renderer (`arc_rs::help`); call sites below use it directly.

/// Classic (old-style) workflow names at ceb: everything concrete
/// which is not in the toolset set. In-process these run the same
/// `COMMANDS` runners, but with the classic subprocess conventions
/// (`ARCANIST` set, usage envelopes, no `--` requirement).
const CLASSIC_COMMANDS: &[&str] = &[
    "close-revision",
    "commit",
    "cover",
    "diff",
    "export",
    "get-config",
    "install-certificate",
    "lint",
    "linters",
    "list",
    "patch",
    "set-config",
    "tasks",
    "todo",
    "unit",
    "which",
];

/// Rust-only commands (abstract in PHP at ceb): they resolve and run
/// in-process like toolset commands, but are excluded from spelling
/// options so typo behavior stays PHP-faithful. Only `markers`
/// qualifies (`ArcanistMarkersWorkflow` is abstract); `browse` is a
/// concrete PHP workflow (T1153 CORE-66) and lives in
/// [`ALL_WORKFLOWS`].
const TOOLSET_EXTRA_COMMANDS: &[&str] = &["markers"];

/// Parsed global (pre-command) arguments.
#[derive(Debug, Default, PartialEq, Eq)]
struct Globals {
    trace: bool,
    /// `Some(false)` for `--no-ansi`, `Some(true)` for `--ansi`.
    ansi: Option<bool>,
    show_help: bool,
    show_standard_options: bool,
    conduit_uri: Option<String>,
    conduit_token: Option<String>,
    config: Vec<String>,
    config_files: Vec<String>,
    arcrc_file: Option<String>,
    anonymous: bool,
}

/// A global-argument parse error with its envelope arm: standard-spec
/// errors render bold (`printUsageException`), config/toolset-spec
/// errors render the red toolset label (uncaught into `execute()`),
/// and classic-only-spec errors render the red subprocess banner
/// (`PhutilArgumentUsageException` is not an `ArcanistUsageException`).
#[derive(Debug)]
struct GlobalError {
    message: String,
    kind: conduit::EnvelopeKind,
}

impl GlobalError {
    fn bold(message: String) -> Self {
        Self {
            message,
            kind: conduit::EnvelopeKind::Usage,
        }
    }

    fn red(message: String) -> Self {
        Self {
            message,
            kind: conduit::EnvelopeKind::ToolsetUsage,
        }
    }

    fn classic(message: String) -> Self {
        Self {
            message,
            kind: conduit::EnvelopeKind::Exception,
        }
    }
}

/// Split `--name=value` into parts (no `=` means a bare flag).
fn split_flag_value(text: &str) -> (&str, Option<&str>) {
    match text.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (text, None),
    }
}

/// Split one argv token into its dash prefix and flag body (`None`
/// for positionals, which the caller copies to `rest`). Any single
/// dash prefixes `-h` (PHP's global parser accepts help flags on any
/// workflow).
fn standard_token_kind(arg: &str) -> Option<(&str, &str)> {
    if let Some(body) = arg.strip_prefix("--") {
        Some(("--", body))
    } else if arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--") {
        Some(("-", &arg[1..]))
    } else {
        None
    }
}

/// True for the standard-argument names (boolean or valued).
fn is_standard_flag(canonical: &str) -> bool {
    matches!(
        canonical,
        "trace" | "no-ansi" | "ansi" | "help" | "show-standard-options" | "recon" | "xprofile"
    )
}

/// Take a valued standard parameter (inline or the next token),
/// returning the value and the argv slots consumed.
fn standard_valued_param(
    typed: &str,
    inline: Option<&str>,
    next: Option<&str>,
) -> Result<(String, usize), String> {
    match inline {
        Some(value) => Ok((value.to_owned(), 1)),
        None => {
            let Some(next) = next else {
                return Err(format!("Argument \"{typed}\" requires a parameter."));
            };
            Ok((next.to_owned(), 2))
        }
    }
}

/// Record one known standard argument into `globals` (`typed` is the
/// `--name`/`-x` spelling for error texts). Returns the argv slots
/// consumed (2 when a valued parameter comes from the next token).
fn standard_consume_flag<'a>(
    globals: &mut Globals,
    seen: &mut std::collections::HashSet<&'a str>,
    canonical: &'a str,
    typed: &str,
    inline: Option<&str>,
    known_valued: bool,
    next: Option<&str>,
) -> Result<usize, String> {
    if inline.is_some() && !known_valued {
        return Err(format!("Argument \"{typed}\" does not take a parameter."));
    }
    if !seen.insert(canonical) {
        return Err(format!("Argument \"{typed}\" was provided twice."));
    }
    if known_valued {
        let (value, consumed) = standard_valued_param(typed, inline, next)?;
        // `--xprofile` needs XHProf (PHP-only): parsed, ignored.
        let _ = value;
        return Ok(consumed);
    }
    if canonical == "no-ansi" && seen.contains("ansi") {
        return Err(format!(
            "Argument \"{typed}\" conflicts with argument \"--ansi\"."
        ));
    }
    standard_apply_flag(globals, canonical);
    Ok(1)
}

/// Apply one boolean standard argument to `globals`.
fn standard_apply_flag(globals: &mut Globals, canonical: &str) {
    match canonical {
        "trace" => globals.trace = true,
        "no-ansi" => globals.ansi = Some(false),
        "ansi" => globals.ansi = Some(true),
        "help" => globals.show_help = true,
        "show-standard-options" => globals.show_standard_options = true,
        // `--recon` needs the PHP remote console: parsed, ignored.
        _ => {}
    }
}

/// Phase 1: standard arguments (`parseStandardArguments()`), bold
/// errors. Consumed anywhere; `--` stops the scan.
fn strip_standard(argv: &[String], globals: &mut Globals) -> Result<Vec<String>, GlobalError> {
    let mut rest = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            rest.extend(argv[index..].iter().cloned());
            break;
        }
        let Some((pre, body)) = standard_token_kind(arg) else {
            rest.push(arg.clone());
            index += 1;
            continue;
        };
        let (name, inline) = split_flag_value(body);
        let canonical = if pre == "-" { "help" } else { name };
        if !is_standard_flag(canonical) {
            rest.push(arg.clone());
            index += 1;
            continue;
        }
        let known_valued = canonical == "xprofile";
        let typed = format!("{pre}{name}");
        let next = argv.get(index + 1).map(String::as_str);
        match standard_consume_flag(
            globals,
            &mut seen,
            canonical,
            &typed,
            inline,
            known_valued,
            next,
        ) {
            Ok(consumed) => index += consumed,
            Err(message) => return Err(GlobalError::bold(message)),
        }
    }
    Ok(rest)
}

/// Phase 2: toolset config arguments (`parsePartial($config_args,
/// initial_only: true)`), red errors. Valued and repeatable; flags
/// must precede the first non-flag argument (T1153 CORE-34:
/// `--config`/`--config-file`/`--arcrc-file` reach every command
/// through [`get_config::LoadOptions`]).
fn strip_config(argv: &[String], globals: &mut Globals) -> Result<Vec<String>, GlobalError> {
    let mut rest = Vec::new();
    let mut is_initial = true;
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            rest.extend(argv[index..].iter().cloned());
            break;
        }
        let Some(body) = arg.strip_prefix("--") else {
            if arg != "-" {
                is_initial = false;
            }
            rest.push(arg.clone());
            index += 1;
            continue;
        };
        let (name, inline) = split_flag_value(body);
        if !matches!(name, "library" | "config" | "config-file") {
            rest.push(arg.clone());
            index += 1;
            continue;
        }
        if !is_initial {
            return Err(GlobalError::red(format!(
                "Argument \"--{name}\" appears after the first non-flag argument. This special argument must appear before other arguments."
            )));
        }
        let value = match inline {
            Some(value) => value.to_string(),
            None => {
                let Some(next) = argv.get(index + 1) else {
                    return Err(GlobalError::red(format!(
                        "Argument \"--{name}\" requires a parameter."
                    )));
                };
                index += 1;
                next.clone()
            }
        };
        match name {
            // `--library` names PHP libraries (nothing to load in a
            // static binary): parsed, ignored.
            "library" => {}
            "config" => globals.config.push(value),
            "config-file" => globals.config_files.push(value),
            _ => {}
        }
        index += 1;
    }
    Ok(rest)
}

/// Phase 3: toolset arguments (`ArcanistArcToolset`), red errors.
fn strip_toolset(argv: &[String], globals: &mut Globals) -> Result<Vec<String>, GlobalError> {
    let mut rest = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            rest.extend(argv[index..].iter().cloned());
            break;
        }
        let Some(body) = arg.strip_prefix("--") else {
            rest.push(arg.clone());
            index += 1;
            continue;
        };
        let (name, inline) = split_flag_value(body);
        if !matches!(name, "conduit-uri" | "conduit-token") {
            rest.push(arg.clone());
            index += 1;
            continue;
        }
        let value = match inline {
            Some(value) => value.to_string(),
            None => {
                let Some(next) = argv.get(index + 1) else {
                    return Err(GlobalError::red(format!(
                        "Argument \"--{name}\" requires a parameter."
                    )));
                };
                index += 1;
                next.clone()
            }
        };
        if !seen.insert(name) {
            return Err(GlobalError::red(format!(
                "Argument \"--{name}\" was provided twice."
            )));
        }
        match name {
            "conduit-uri" => globals.conduit_uri = Some(value),
            "conduit-token" => globals.conduit_token = Some(value),
            _ => {}
        }
        index += 1;
    }
    Ok(rest)
}

/// Consume `--arcrc-file[=value]` (`next` is the following token
/// for separate parameters). Returns the argv slots consumed (2 when
/// the parameter comes from the next token).
fn classic_arcrc_file(
    globals: &mut Globals,
    seen: &mut std::collections::HashSet<&str>,
    inline: Option<&str>,
    next: Option<&str>,
) -> Result<usize, String> {
    let (value, consumed) = match inline {
        Some(value) => (value.to_owned(), 1),
        None => {
            let Some(next) = next else {
                return Err("Argument \"--arcrc-file\" requires a parameter.".to_string());
            };
            (next.to_owned(), 2)
        }
    };
    if !seen.insert("arcrc-file") {
        return Err("Argument \"--arcrc-file\" was provided twice.".to_string());
    }
    globals.arcrc_file = Some(value);
    Ok(consumed)
}

/// Consume the boolean `--anonymous` flag.
fn classic_anonymous(
    globals: &mut Globals,
    seen: &mut std::collections::HashSet<&str>,
    inline: Option<&str>,
) -> Result<(), String> {
    if inline.is_some() {
        return Err("Argument \"--anonymous\" does not take a parameter.".to_string());
    }
    if !seen.insert("anonymous") {
        return Err("Argument \"--anonymous\" was provided twice.".to_string());
    }
    globals.anonymous = true;
    Ok(())
}

/// Consume `--load-phutil-library` (parsed, ignored: it names PHP
/// libraries and there is nothing to load in a static binary).
/// `has_next` is whether a following token exists for a separate
/// parameter. Returns the argv slots consumed.
fn classic_phutil_library(inline: Option<&str>, has_next: bool) -> Result<usize, String> {
    match inline {
        Some(_) => Ok(1),
        None => {
            if !has_next {
                return Err("Argument \"--load-phutil-library\" requires a parameter.".to_string());
            }
            Ok(2)
        }
    }
}

/// Phase 4: classic-only arguments (`scripts/arcanist.php` base args).
/// PHP consumes these in the classic subprocess (after the toolset
/// runtime declines the command); the port consumes them in-process
/// so classic commands observe them, with the subprocess red-banner
/// arm on errors.
fn strip_classic(argv: &[String], globals: &mut Globals) -> Result<Vec<String>, GlobalError> {
    let mut rest = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            rest.extend(argv[index..].iter().cloned());
            break;
        }
        let Some(body) = arg.strip_prefix("--") else {
            rest.push(arg.clone());
            index += 1;
            continue;
        };
        let (name, inline) = split_flag_value(body);
        let next = argv.get(index + 1).map(String::as_str);
        let consumed = match name {
            "arcrc-file" => classic_arcrc_file(globals, &mut seen, inline, next)
                .map_err(GlobalError::classic)?,
            "anonymous" => {
                classic_anonymous(globals, &mut seen, inline).map_err(GlobalError::classic)?;
                1
            }
            "load-phutil-library" => {
                classic_phutil_library(inline, next.is_some()).map_err(GlobalError::classic)?
            }
            _ => {
                rest.push(arg.clone());
                1
            }
        };
        index += consumed;
    }
    Ok(rest)
}

/// Strip global arguments in PHP's phase order (standard, config,
/// toolset, classic), returning the command argv plus the globals.
fn strip_globals(argv: &[String]) -> Result<(Vec<String>, Globals), GlobalError> {
    let mut globals = Globals::default();
    let rest = strip_standard(argv, &mut globals)?;
    let rest = strip_config(&rest, &mut globals)?;
    let rest = strip_toolset(&rest, &mut globals)?;
    let rest = strip_classic(&rest, &mut globals)?;
    Ok((rest, globals))
}

/// Config keys `--config` accepts at ceb: the toolset options map
/// (`ArcanistArcConfigurationEngineExtension`), aliases included.
const RUNTIME_CONFIG_KEYS: &[&str] = &[
    "aliases",
    "arc.land.onto",
    "arc.land.onto-remote",
    "arc.land.strategy",
    "base",
    "conduit_uri",
    "default",
    "pager",
    "phabricator.uri",
    "prompts",
    "repository",
    "repository.callsign",
];

/// Validate `--config` pairs with the toolset texts
/// (`ArcanistRuntimeConfigurationSource`), then coerce values for the
/// load-options channel. Errors render the red toolset label: the
/// toolset runtime validates before dispatch, so these preempt the
/// classic subprocess texts.
fn validate_runtime_config(
    pairs: &[String],
) -> Result<serde_json::Map<String, serde_json::Value>, GlobalError> {
    let mut seen = std::collections::HashSet::new();
    for pair in pairs {
        let Some((key, _)) = pair.split_once('=') else {
            return Err(GlobalError::red(terminal::pht(
                "Configuration option \"%s\" is not valid. Configuration options passed with command line flags must be in the form \"name=value\".",
                &[terminal::PhtArg::Text(pair)],
            )));
        };
        if !seen.insert(key) {
            return Err(GlobalError::red(terminal::pht(
                "Configuration option \"%s\" was provided multiple times with \"--config\" flags. Specify each option no more than once.",
                &[terminal::PhtArg::Text(key)],
            )));
        }
        if !RUNTIME_CONFIG_KEYS.contains(&key) {
            return Err(GlobalError::red(terminal::pht(
                "Configuration option (\"%s\") specified with \"--config\" flag is not a recognized option.",
                &[terminal::PhtArg::Text(key)],
            )));
        }
    }
    set_config::parse_runtime_config(pairs).map_err(GlobalError::red)
}

/// Apply globals to the [`globals`] cells the commands read. Only
/// `main` calls this; `dispatch` stays free of process-global writes
/// so unit tests stay hermetic.
fn apply_globals(globals: &Globals) -> Result<(), GlobalError> {
    if globals.trace {
        arc_rs::globals::set_trace(true);
    }
    if let Some(ansi) = globals.ansi {
        arc_rs::globals::set_ansi(ansi);
    }
    if let Some(uri) = &globals.conduit_uri {
        arc_rs::globals::set_conduit_uri(uri.clone());
    }
    if let Some(token) = &globals.conduit_token {
        arc_rs::globals::set_conduit_token(token.clone());
    }
    if globals.anonymous {
        arc_rs::globals::set_anonymous(true);
    }
    if !globals.config.is_empty()
        || !globals.config_files.is_empty()
        || globals.arcrc_file.is_some()
    {
        let runtime = validate_runtime_config(&globals.config)?;
        let doc = serde_json::json!({
            "runtime": runtime,
            "config_files": globals.config_files,
            "arcrc_file": globals.arcrc_file,
        });
        arc_rs::globals::set_load_options(doc.to_string());
    }
    Ok(())
}

/// Print the magenta `ARGV` trace line (`writeTrace('ARGV', ...)`).
fn trace_argv(full_argv: &[String]) {
    eprint!(
        "{}",
        terminal::format_log(
            "magenta",
            "ARGV",
            &terminal::shell_join(full_argv),
            terminal::ansi_enabled()
        )
    );
}

/* -(  Workflow reference  )-------------------------------------------- */

/// One displayed argument spec (wildcards excluded, like
/// `renderArgumentSpecs()`).
struct HelpArg {
    name: &'static str,
    short: Option<&'static str>,
    param: Option<&'static str>,
    help: Option<&'static str>,
}

/// One `help` entry: toolset workflows contribute
/// `getWorkflowInformation()` + `getWorkflowArguments()`, legacy
/// workflows contribute `getCommandSynopses()` + `getCommandHelp()` +
/// `getArguments()` (source-indented, kept verbatim). `None` examples
/// render the `**name**` default; toolset help bodies unwrap single
/// newlines (`newPhutilWorkflow()`).
struct HelpEntry {
    name: &'static str,
    examples: Option<&'static str>,
    synopsis: Option<&'static str>,
    help: Option<&'static str>,
    unwrap_help: bool,
    args: &'static [HelpArg],
}

const HELP_ENTRIES: &[HelpEntry] = &[
    HelpEntry {
        name: "alias",
        examples: Some("**alias**\n**alias** __command__ __target__ -- [__arguments__]"),
        synopsis: Some("Create and modify command aliases."),
        help: Some(
            "Create an alias from __command__ to __target__ (optionally, with __options__).\n\
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
            existing aliases with the same name.",
        ),
        unwrap_help: true,
        args: &[HelpArg {
            name: "json",
            short: None,
            param: None,
            help: Some("Output aliases in JSON format."),
        }],
    },
    HelpEntry {
        name: "amend",
        examples: Some("**amend** [options] -- "),
        synopsis: Some("Amend the working copy, synchronizing the local commit message."),
        help: Some(
            "Amend the working copy, synchronizing the local commit message from\n\
            Differential.\n\
            \n\
            Supported in Mercurial 2.2 and newer.",
        ),
        unwrap_help: true,
        args: &[
            HelpArg {
                name: "show",
                short: None,
                param: None,
                help: Some("Show the amended commit message, without modifying the working copy."),
            },
            HelpArg {
                name: "revision",
                short: None,
                param: Some("id"),
                help: Some(
                    "Use the message from a specific revision. If you do not specify a revision, arc will guess which revision is in the working copy.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "anoid",
        examples: Some("**anoid**"),
        synopsis: Some("Pilot a probe from the vessel \"Arcanoid\"."),
        help: Some("Take control of a probe launched from the science vessel \"Arcanoid\"."),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "bookmarks",
        examples: Some("**bookmarks**"),
        synopsis: Some("Show an enhanced view of bookmarks in the working copy."),
        help: Some(
            "Lists bookmarks in the working copy, annotated with additional information\nabout review status.",
        ),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "branches",
        examples: Some("**branches**"),
        synopsis: Some("Show an enhanced view of branches in the working copy."),
        help: Some(
            "Lists branches in the working copy, annotated with additional information\nabout review status.",
        ),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        // T1153 R10: fixed to the ceb bytes
        // (`src/browse/workflow/ArcanistBrowseWorkflow.php:13-60`); the
        // previous text described the Rust-extended resolver (commit /
        // object / path / revision types), not the ceb workflow.
        name: "browse",
        examples: Some(
            "**browse** [options] -- __target__ ...\n**browse** -- __file-name__\n**browse** -- __object-name__",
        ),
        synopsis: Some("Open a file or object in a local web browser."),
        help: Some(
            "Open a file or object (like a task or revision) in a local web browser.\n\
            \n\
            \x20 $ arc browse README   # Open a file in Diffusion.\n\
            \x20 $ arc browse T123     # View a task.\n\
            \x20 $ arc browse HEAD     # View a symbolic commit.\n\
            \n\
            To choose a browser binary to invoke, use:\n\
            \n\
            \x20 $ arc set-config browser __browser-binary__\n\
            \n\
            If no browser is set, the command will try to guess which browser to use.",
        ),
        unwrap_help: true,
        args: &[
            HelpArg {
                name: "branch",
                short: None,
                param: Some("branch-name"),
                help: Some("Default branch name to view on server. Defaults to \"master\"."),
            },
            HelpArg {
                name: "types",
                short: None,
                param: Some("type-list"),
                help: Some(
                    "Force targets to be interpreted as naming particular types of resources.",
                ),
            },
            HelpArg {
                name: "force",
                short: None,
                param: None,
                help: Some("(DEPRECATED) Obsolete, use \"--types path\" instead."),
            },
        ],
    },
    HelpEntry {
        name: "call-conduit",
        examples: Some("**call-conduit** -- __method__"),
        synopsis: Some("Call Conduit API methods."),
        help: Some(
            "Allows you to make a raw Conduit method call:\n\
            \n\
            \x20 - Run this command from a working directory.\n\
            \x20 - Call parameters are required, and read as a JSON blob from stdin.\n\
            \x20 - Results are written to stdout as a JSON blob.\n\
            \n\
            This workflow is primarily useful for writing scripts which integrate\n\
            with Phabricator. Examples:\n\
            \n\
            \x20 $ echo '{}' | arc call-conduit -- conduit.ping\n\
            \x20 $ echo '{\"phid\":\"PHID-FILE-xxxx\"}' | arc call-conduit -- file.download",
        ),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "close-revision",
        examples: None,
        synopsis: Some("      **close-revision** [__options__] __revision__"),
        help: Some(
            "          Supports: git, hg, svn\n          Close a revision which has been committed (svn) or pushed (git, hg).\n          You should not normally need to do this: arc commit (svn), arc amend\n          (git, hg), arc land (git, hg), or repository tracking on the master\n          remote repository should do it for you. However, if these mechanisms\n          have failed for some reason you can use this command to manually\n          change a revision status from \"Accepted\" to \"Closed\".",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "finalize",
                short: None,
                param: None,
                help: Some(
                    "Close only if the repository is untracked and the revision is accepted. Continue even if the close can't happen. This is a soft version of '' used by other workflows.",
                ),
            },
            HelpArg {
                name: "quiet",
                short: None,
                param: None,
                help: Some("Do not print a success message."),
            },
        ],
    },
    HelpEntry {
        name: "commit",
        examples: None,
        synopsis: Some("      **commit** [--revision __revision_id__] [--show]"),
        help: Some(
            "          Supports: svn\n          Commit a revision which has been accepted by a reviewer.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "show",
                short: None,
                param: None,
                help: Some(
                    "Show the command which would be issued, but do not actually commit anything.",
                ),
            },
            HelpArg {
                name: "revision",
                short: None,
                param: Some("revision_id"),
                help: Some(
                    "Commit a specific revision. If you do not specify a revision, arc will look for committable revisions.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "cover",
        examples: None,
        synopsis: Some("      **cover** [--rev __revision__] [__path__ ...]"),
        help: Some(
            "          Supports: svn, git, hg\n          Cover your... professional reputation. Show blame for the lines you\n          changed in your working copy (svn) or since some commit (hg, git).\n          This will take a minute because blame takes a minute, especially under\n          SVN.",
        ),
        unwrap_help: false,
        args: &[HelpArg {
            name: "rev",
            short: None,
            param: Some("revision"),
            help: Some("Cover changes since a specific revision."),
        }],
    },
    HelpEntry {
        name: "diff",
        examples: None,
        synopsis: Some("      **diff** [__paths__] (svn)\n      **diff** [__commit__] (git, hg)"),
        help: Some(
            "          Supports: git, svn, hg\n          Generate a Differential diff or revision from local changes.\n\n          Under git and mercurial, you can specify a commit (like __HEAD^^^__\n          or __master__) and Differential will generate a diff against the\n          merge base of that commit and your current working directory parent.\n\n          Under svn, you can choose to include only some of the modified files\n          in the working copy in the diff by specifying their paths. If you\n          omit paths, all changes are included in the diff.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "message",
                short: Some("m"),
                param: Some("message"),
                help: Some(
                    "When updating a revision, use the specified message instead of prompting.",
                ),
            },
            HelpArg {
                name: "message-file",
                short: Some("F"),
                param: Some("file"),
                help: Some("When creating a revision, read revision information from this file."),
            },
            HelpArg {
                name: "edit",
                short: None,
                param: None,
                help: Some(
                    "When updating a revision under git, edit revision information before updating.",
                ),
            },
            HelpArg {
                name: "raw",
                short: None,
                param: None,
                help: Some(
                    "Read diff from stdin, not from the working copy. This disables many Arcanist/Phabricator features which depend on having access to the working copy.",
                ),
            },
            HelpArg {
                name: "raw-command",
                short: None,
                param: Some("command"),
                help: Some(
                    "Generate diff by executing a specified command, not from the working copy. This disables many Arcanist/Phabricator features which depend on having access to the working copy.",
                ),
            },
            HelpArg {
                name: "create",
                short: None,
                param: None,
                help: Some("Always create a new revision."),
            },
            HelpArg {
                name: "update",
                short: None,
                param: Some("revision_id"),
                help: Some("Always update a specific revision."),
            },
            HelpArg {
                name: "draft",
                short: None,
                param: None,
                help: Some(
                    "Create a draft revision so you can look over your changes before involving anyone else. Other users will not be notified about the revision until you later use \"Request Review\" to publish it. You can still share the draft by giving someone the link.",
                ),
            },
            HelpArg {
                name: "nounit",
                short: None,
                param: None,
                help: Some("Do not run unit tests."),
            },
            HelpArg {
                name: "nolint",
                short: None,
                param: None,
                help: Some("Do not run lint."),
            },
            HelpArg {
                name: "only",
                short: None,
                param: None,
                help: Some(
                    "Instead of creating or updating a revision, only create a diff, which you may later attach to a revision.",
                ),
            },
            HelpArg {
                name: "allow-untracked",
                short: None,
                param: None,
                help: Some("Skip checks for untracked files in the working copy."),
            },
            HelpArg {
                name: "apply-patches",
                short: None,
                param: None,
                help: Some(
                    "Apply patches suggested by lint to the working copy without prompting.",
                ),
            },
            HelpArg {
                name: "never-apply-patches",
                short: None,
                param: None,
                help: Some("Never apply patches suggested by lint."),
            },
            HelpArg {
                name: "amend-all",
                short: None,
                param: None,
                help: Some(
                    "When linting git repositories, amend HEAD with all patches suggested by lint without prompting.",
                ),
            },
            HelpArg {
                name: "amend-autofixes",
                short: None,
                param: None,
                help: Some(
                    "When linting git repositories, amend HEAD with autofix patches suggested by lint without prompting.",
                ),
            },
            HelpArg {
                name: "add-all",
                short: Some("a"),
                param: None,
                help: Some("Automatically add all unstaged and uncommitted files to the commit."),
            },
            HelpArg {
                name: "json",
                short: None,
                param: None,
                help: Some("Emit machine-readable JSON. EXPERIMENTAL! Probably does not work!"),
            },
            HelpArg {
                name: "no-amend",
                short: None,
                param: None,
                help: Some("Never amend commits in the working copy with lint patches."),
            },
            HelpArg {
                name: "uncommitted",
                short: None,
                param: None,
                help: Some("Suppress warning about uncommitted changes."),
            },
            HelpArg {
                name: "verbatim",
                short: None,
                param: None,
                help: Some(
                    "When creating a revision, try to use the working copy commit message verbatim, without prompting to edit it. When updating a revision, update some fields from the local commit message.",
                ),
            },
            HelpArg {
                name: "reviewers",
                short: None,
                param: Some("usernames"),
                help: Some("When creating a revision, add reviewers."),
            },
            HelpArg {
                name: "cc",
                short: None,
                param: Some("usernames"),
                help: Some("When creating a revision, add CCs."),
            },
            HelpArg {
                name: "skip-binaries",
                short: None,
                param: None,
                help: Some("Do not upload binaries (like images)."),
            },
            HelpArg {
                name: "skip-staging",
                short: None,
                param: None,
                help: Some("Do not copy changes to the staging area."),
            },
            HelpArg {
                name: "base",
                short: None,
                param: Some("rules"),
                help: Some("Additional rules for determining base revision."),
            },
            HelpArg {
                name: "coverage",
                short: None,
                param: None,
                help: Some("Always enable coverage information."),
            },
            HelpArg {
                name: "no-coverage",
                short: None,
                param: None,
                help: Some("Always disable coverage information."),
            },
            HelpArg {
                name: "browse",
                short: None,
                param: None,
                help: Some("After creating a diff or revision, open it in a web browser."),
            },
            HelpArg {
                name: "head",
                short: None,
                param: Some("commit"),
                help: Some(
                    "Specify the end of the commit range. This disables many Arcanist/Phabricator features which depend on having access to the working copy.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "download",
        examples: Some("**download** [__options__] -- __file__"),
        synopsis: Some("Download a file to local disk."),
        help: Some("Download a file to local disk."),
        unwrap_help: true,
        args: &[HelpArg {
            name: "as",
            short: None,
            param: Some("path"),
            help: Some("Save the file to a specific location."),
        }],
    },
    HelpEntry {
        name: "export",
        examples: None,
        synopsis: Some(
            "      **export** [__paths__] __format__ (svn)\n      **export** [__commit_range__] __format__ (git, hg)\n      **export** __--revision__ __revision_id__ __format__\n      **export** __--diff__ __diff_id__ __format__",
        ),
        help: Some(
            "          Supports: svn, git, hg\n          Export the local changeset (or a Differential changeset) to a file,\n          in some __format__: git diff (__--git__), unified diff\n          (__--unified__), or arc bundle (__--arcbundle__ __path__) format.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "git",
                short: None,
                param: None,
                help: Some(
                    "Export change as a git patch. This format is more complete than unified, but less complete than arc bundles. These patches can be applied with 'git apply' or 'arc patch'.",
                ),
            },
            HelpArg {
                name: "unified",
                short: None,
                param: None,
                help: Some(
                    "Export change as a unified patch. This format is less complete than git patches or arc bundles. These patches can be applied with 'patch' or 'arc patch'.",
                ),
            },
            HelpArg {
                name: "arcbundle",
                short: None,
                param: Some("file"),
                help: Some(
                    "Export change as an arc bundle. This format can represent all changes. These bundles can be applied with 'arc patch'.",
                ),
            },
            HelpArg {
                name: "encoding",
                short: None,
                param: Some("encoding"),
                help: Some("Attempt to convert non UTF-8 patch into specified encoding."),
            },
            HelpArg {
                name: "revision",
                short: None,
                param: Some("revision_id"),
                help: Some(
                    "Instead of exporting changes from the working copy, export them from a Differential revision.",
                ),
            },
            HelpArg {
                name: "diff",
                short: None,
                param: Some("diff_id"),
                help: Some(
                    "Instead of exporting changes from the working copy, export them from a Differential diff.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "get-config",
        examples: None,
        synopsis: Some("      **get-config** [__options__] -- [__name__ ...]"),
        help: Some(
            "          Supports: cli\n          Reads an arc configuration option. With no argument, reads all\n          options.\n\n          With __--verbose__, shows detailed information about one or more\n          options.",
        ),
        unwrap_help: false,
        args: &[HelpArg {
            name: "verbose",
            short: None,
            param: None,
            help: Some("Show detailed information about options."),
        }],
    },
    HelpEntry {
        name: "help",
        examples: Some("**help** [__command__]"),
        synopsis: Some("Show this help, or workflow help for __command__."),
        help: None,
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "inspect",
        examples: Some("**inspect** [__options__] -- __object__"),
        synopsis: Some("Show internal object information."),
        help: Some("Inspect internal object properties."),
        unwrap_help: true,
        args: &[HelpArg {
            name: "explore",
            short: None,
            param: None,
            help: Some("Load all object hardpoints."),
        }],
    },
    HelpEntry {
        name: "install-certificate",
        examples: None,
        synopsis: Some("      **install-certificate** [uri]"),
        help: Some(
            "          Supports: http, https\n          Installs Conduit credentials into your ~/.arcrc for the given install\n          of Phabricator. You need to do this before you can use 'arc', as it\n          enables 'arc' to link your command-line activity with your account on\n          the web. Run this command from within a project directory to install\n          that project's certificate, or specify an explicit URI (like\n          \"https://phabricator.example.com/\").",
        ),
        unwrap_help: false,
        args: &[],
    },
    HelpEntry {
        name: "land",
        examples: Some("**land** [__options__] -- [__ref__ ...]"),
        synopsis: Some("Publish reviewed changes."),
        help: Some(
            "Supports: git, git/p4, git/svn, hg\n\
            \n\
            Publish accepted revisions after review. This command is the last step in the\n\
            standard Differential code review workflow.\n\
            \n\
            To publish changes in local branch or bookmark \"feature1\", you will usually\n\
            run this command:\n\
            \n\
            \x20 **$ arc land feature1**\n\
            \n\
            This workflow merges and pushes changes associated with revisions that are\n\
            ancestors of __ref__. Without __ref__, the current working copy state will be\n\
            used. You can specify multiple __ref__ arguments to publish multiple changes at\n\
            once.\n\
            \n\
            A __ref__ can be any symbol which identifies a commit: a branch name, a tag\n\
            name, a bookmark name, a topic name, a raw commit hash, a symbolic reference,\n\
            etc.\n\
            \n\
            When you provide a __ref__, all unpublished changes which are present in\n\
            ancestors of that __ref__ will be selected for publishing. (With the\n\
            **--pick** flag, only the unpublished changes you directly reference will be\n\
            selected.)\n\
            \n\
            For example, if you provide local branch \"feature3\" as a __ref__ argument, that\n\
            may also select the changes in \"feature1\" and \"feature2\" (if they are ancestors\n\
            of \"feature3\"). If you stack changes in a single local branch, all commits in\n\
            the stack may be selected.\n\
            \n\
            The workflow merges unpublished changes reachable from __ref__ \"into\" some\n\
            intermediate branch, then pushes the combined state \"onto\" some destination\n\
            branch (or list of branches).\n\
            \n\
            (In Mercurial, the \"into\" and \"onto\" branches may be bookmarks instead.)\n\
            \n\
            In the most common case, there is only one \"onto\" branch (often \"master\" or\n\
            \"default\" or some similar branch) and the \"into\" branch is the same branch. For\n\
            example, it is common to merge local feature branch \"feature1\" into\n\
            \"origin/master\", then push it onto \"origin/master\".\n\
            \n\
            The list of \"onto\" branches is selected by examining these sources in order:\n\
            \n\
            \x20 - the **--onto** flags;\n\
            \x20 - the __arc.land.onto__ configuration setting;\n\
            \x20 - (in Git) the upstream of the branch targeted by the land operation,\n\
            \x20 \x20 recursively;\n\
            \x20 - or by falling back to a standard default:\n\
            \x20 \x20 - (in Git) \"master\";\n\
            \x20 \x20 - (in Mercurial) \"default\".\n\
            \n\
            The remote to push \"onto\" is selected by examining these sources in order:\n\
            \n\
            \x20 - the **--onto-remote** flag;\n\
            \x20 - the __arc.land.onto-remote__ configuration setting;\n\
            \x20 - (in Git) the upstream of the current branch, recursively;\n\
            \x20 - (in Git) the special \"p4\" remote which indicates a repository has\n\
            \x20 \x20 been synchronized with Perforce;\n\
            \x20 - or by falling back to a standard default:\n\
            \x20 \x20 - (in Git) \"origin\";\n\
            \x20 \x20 - (in Mercurial) \"default\".\n\
            \n\
            The branch to merge \"into\" is selected by examining these sources in order:\n\
            \n\
            \x20 - the **--into** flag;\n\
            \x20 - the **--into-empty** flag;\n\
            \x20 - or by falling back to the first \"onto\" branch.\n\
            \n\
            The remote to merge \"into\" is selected by examining these sources in order:\n\
            \n\
            \x20 - the **--into-remote** flag;\n\
            \x20 - the **--into-local** flag (which disables fetching before merging);\n\
            \x20 - or by falling back to the \"onto\" remote.\n\
            \n\
            After selecting remotes and branches, the commits which will land are printed.\n\
            \n\
            With **--preview**, execution stops here, before the change is merged.\n\
            \n\
            The \"into\" branch is fetched from the \"into\" remote (unless **--into-local** or\n\
            **--into-empty** are specified) and the changes are merged into the state in\n\
            the \"into\" branch according to the selected merge strategy.\n\
            \n\
            The default merge strategy is \"squash\", which produces a single commit from\n\
            all local commits for each change. A different strategy can be selected with\n\
            the **--strategy** flag.\n\
            \n\
            The resulting merged change will be given an up-to-date commit message\n\
            describing the final state of the revision in Differential.\n\
            \n\
            With **--hold**, execution stops here, before the change is pushed.\n\
            \n\
            The change is pushed onto all of the \"onto\" branches in the \"onto\" remote.\n\
            \n\
            If you are landing multiple changes, they are normally all merged locally and\n\
            then all pushed in a single operation. Instead, you can merge and push them one\n\
            at a time with **--incremental**.\n\
            \n\
            Under merge strategies which mutate history (including the default \"squash\"\n\
            strategy), local refs which descend from commits that were published are\n\
            now updated. For example, if you land \"feature4\", local branches \"feature5\" and\n\
            \"feature6\" may now be rebased on the published version of the change.\n\
            \n\
            Once everything has been pushed, cleanup occurs. Consulting mystical sources of\n\
            power, the workflow makes a guess about what state you wanted to end up in\n\
            after the process finishes. The working copy is put into that state.\n\
            \n\
            Any obsolete refs that point at commits which were published are deleted,\n\
            unless the **--keep-branches** flag is passed.",
        ),
        unwrap_help: true,
        args: &[
            HelpArg {
                name: "hold",
                short: None,
                param: None,
                help: Some("Prepare the changes to be pushed, but do not actually push them."),
            },
            HelpArg {
                name: "keep-branches",
                short: None,
                param: None,
                help: Some(
                    "Keep local branches around after changes are pushed. By default, local branches are deleted after the changes they contain are published.",
                ),
            },
            HelpArg {
                name: "onto-remote",
                short: None,
                param: Some("remote-name"),
                help: Some(
                    "Push to a remote other than the default.\n\nRelated configuration: **arc.land.onto-remote**",
                ),
            },
            HelpArg {
                name: "onto",
                short: None,
                param: Some("branch-name"),
                help: Some(
                    "After merging, push changes onto a specified branch.\n\nSpecifying this flag multiple times will push to multiple branches.\n\nRelated configuration: **arc.land.onto**",
                ),
            },
            HelpArg {
                name: "strategy",
                short: None,
                param: Some("strategy-name"),
                help: Some(
                    "Merge using a particular strategy. Supported strategies are \"squash\" and \"merge\".\n\nThe \"squash\" strategy collapses multiple local commits into a single commit when publishing. It produces a linear published history (but discards local checkpoint commits). This is the default strategy.\n\nThe \"merge\" strategy generates a merge commit when publishing that retains local checkpoint commits (but produces a nonlinear published history). Select this strategy if you do not want \"arc land\" to discard checkpoint commits.\n\nRelated configuration: **arc.land.strategy**",
                ),
            },
            HelpArg {
                name: "revision",
                short: None,
                param: Some("revision-identifier"),
                help: Some(
                    "Land a specific revision, rather than determining revisions automatically from the commits that are landing.",
                ),
            },
            HelpArg {
                name: "preview",
                short: None,
                param: None,
                help: Some(
                    "Show the changes that will land. Does not modify the working copy or the remote.",
                ),
            },
            HelpArg {
                name: "into",
                short: None,
                param: Some("commit-ref"),
                help: Some(
                    "Specify the state to merge into. By default, this is the same as the \"onto\" ref.",
                ),
            },
            HelpArg {
                name: "into-remote",
                short: None,
                param: Some("remote-name"),
                help: Some(
                    "Specifies the remote to fetch the \"into\" ref from. By default, this is the same as the \"onto\" remote.",
                ),
            },
            HelpArg {
                name: "into-local",
                short: None,
                param: None,
                help: Some(
                    "Use the local \"into\" ref state instead of fetching it from a remote.",
                ),
            },
            HelpArg {
                name: "into-empty",
                short: None,
                param: None,
                help: Some(
                    "Merge into the empty state instead of an existing state. This mode is primarily useful when creating a new repository, and selected automatically if the \"onto\" ref does not exist and the \"into\" state is not specified.",
                ),
            },
            HelpArg {
                name: "incremental",
                short: None,
                param: None,
                help: Some(
                    "When landing multiple revisions at once, push and rebase after each merge completes instead of waiting until all merges are completed to push.\n\nThis is slower than the default behavior and not atomic, but may make it easier to resolve conflicts and land complicated changes by allowing you to make progress one step at a time.",
                ),
            },
            HelpArg {
                name: "pick",
                short: None,
                param: None,
                help: Some(
                    "Land only the changes directly named by arguments, instead of all reachable ancestors.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "liberate",
        examples: Some("**liberate**\n**liberate** [__path__]"),
        synopsis: Some("Create or update an Arcanist library."),
        help: Some("Create or update an Arcanist library."),
        unwrap_help: true,
        args: &[HelpArg {
            name: "clean",
            short: None,
            param: None,
            help: Some("Perform a clean rebuild, ignoring caches. Thorough, but slow."),
        }],
    },
    HelpEntry {
        name: "lint",
        examples: None,
        synopsis: Some(
            "      **lint** [__options__] [__paths__]\n      **lint** [__options__] --rev [__rev__]",
        ),
        help: Some(
            "          Supports: git, svn, hg\n          Run static analysis on changes to check for mistakes. If no files\n          are specified, lint will be run on all files which have been modified.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "lintall",
                short: None,
                param: None,
                help: Some(
                    "Show all lint warnings, not just those on changed lines. When paths are specified, this is the default behavior.",
                ),
            },
            HelpArg {
                name: "rev",
                short: None,
                param: Some("revision"),
                help: Some("Lint changes since a specific revision."),
            },
            HelpArg {
                name: "output",
                short: None,
                param: Some("format"),
                help: Some("Select an output format."),
            },
            HelpArg {
                name: "outfile",
                short: None,
                param: Some("path"),
                help: Some("Output the linter results to a file. Defaults to stdout."),
            },
            HelpArg {
                name: "engine",
                short: None,
                param: Some("classname"),
                help: Some("Override configured lint engine for this project."),
            },
            HelpArg {
                name: "apply-patches",
                short: None,
                param: None,
                help: Some(
                    "Apply patches suggested by lint to the working copy without prompting.",
                ),
            },
            HelpArg {
                name: "never-apply-patches",
                short: None,
                param: None,
                help: Some("Never apply patches suggested by lint."),
            },
            HelpArg {
                name: "amend-all",
                short: None,
                param: None,
                help: Some(
                    "When linting git repositories, amend HEAD with all patches suggested by lint without prompting.",
                ),
            },
            HelpArg {
                name: "amend-autofixes",
                short: None,
                param: None,
                help: Some(
                    "When linting git repositories, amend HEAD with autofix patches suggested by lint without prompting.",
                ),
            },
            HelpArg {
                name: "everything",
                short: None,
                param: None,
                help: Some(
                    "Lint all tracked files in the working copy. Ignored files and untracked files will not be linted.",
                ),
            },
            HelpArg {
                name: "severity",
                short: None,
                param: Some("string"),
                help: Some(
                    "Set minimum message severity. One of: 'advice', 'autofix', 'warning', 'error', 'disabled'. Defaults to 'advice'.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "linters",
        examples: None,
        synopsis: Some("      **linters** [__options__] [__name__]"),
        help: Some(
            "          Supports: cli\n          List the available and configured linters, with information about\n          what they do and which versions are installed.\n\n          if __name__ is provided, the linter with that name will be displayed.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "verbose",
                short: None,
                param: None,
                help: Some("Show detailed information, including options."),
            },
            HelpArg {
                name: "search",
                short: None,
                param: Some("search"),
                help: Some(
                    "Search for linters. Search is case-insensitive, and is performed against name and description of each linter.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "list",
        examples: None,
        synopsis: Some("      **list**"),
        help: Some(
            "          Supports: git, svn, hg\n          List your open Differential revisions.",
        ),
        unwrap_help: false,
        args: &[],
    },
    HelpEntry {
        name: "look",
        examples: Some("**look**\n**look** [options] -- __thing__"),
        synopsis: Some("You stand in the middle of a small clearing."),
        help: Some("Look around, or look at a specific __thing__."),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "markers",
        examples: Some("**markers**"),
        synopsis: Some("Show an enhanced view of markers in the working copy."),
        help: Some(
            "Show an enhanced view of markers in the working copy.\nWithout --type, git checkouts list branches and Mercurial\ncheckouts list bookmarks.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "type",
                short: None,
                param: Some("TYPE"),
                help: Some("list 'branch' or 'bookmark' markers"),
            },
            HelpArg {
                name: "json",
                short: None,
                param: None,
                help: Some("emit a JSON array of marker objects"),
            },
        ],
    },
    HelpEntry {
        name: "paste",
        examples: Some("**paste** [__options__] --\n**paste** [__options__] -- __object__"),
        synopsis: None,
        help: Some(
            "Share and grab text using the Paste application. To create a paste, use the\n\"--input\" flag or provide the text on stdin:\n\
            \n\
            \x20 $ cat list_of_ducks.txt | arc paste\n\
            \x20 $ arc paste --input list_of_ducks.txt\n\
            \n\
            To retrieve a paste, specify the paste ID:\n\
            \n\
            \x20 $ arc paste P123",
        ),
        unwrap_help: true,
        args: &[
            HelpArg {
                name: "title",
                short: None,
                param: Some("title"),
                help: Some("Title for the paste."),
            },
            HelpArg {
                name: "lang",
                short: None,
                param: Some("language"),
                help: Some("Language for the paste."),
            },
            HelpArg {
                name: "input",
                short: None,
                param: Some("path"),
                help: Some("Create a paste using the content in a file."),
            },
            HelpArg {
                name: "browse",
                short: None,
                param: None,
                help: Some("After creating a paste, open it in a web browser."),
            },
        ],
    },
    HelpEntry {
        name: "patch",
        examples: None,
        synopsis: Some(
            "      **patch** __D12345__\n      **patch** __--revision__ __revision_id__\n      **patch** __--diff__ __diff_id__\n      **patch** __--patch__ __file__\n      **patch** __--arcbundle__ __bundlefile__",
        ),
        help: Some(
            "          Supports: git, svn, hg\n          Apply the changes in a Differential revision, patchfile, or arc\n          bundle to the working copy.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "revision",
                short: None,
                param: Some("revision_id"),
                help: Some(
                    "Apply changes from a Differential revision, using the most recent diff that has been attached to it. You can run 'arc patch D12345' as a shorthand.",
                ),
            },
            HelpArg {
                name: "diff",
                short: None,
                param: Some("diff_id"),
                help: Some(
                    "Apply changes from a Differential diff. Normally you want to use --revision to get the most recent changes, but you can specifically apply an out-of-date diff or a diff which was never attached to a revision by using this flag.",
                ),
            },
            HelpArg {
                name: "arcbundle",
                short: None,
                param: Some("bundlefile"),
                help: Some("Apply changes from an arc bundle generated with 'arc export'."),
            },
            HelpArg {
                name: "patch",
                short: None,
                param: Some("patchfile"),
                help: Some("Apply changes from a git patchfile or unified patchfile."),
            },
            HelpArg {
                name: "encoding",
                short: None,
                param: Some("encoding"),
                help: Some("Attempt to convert non UTF-8 patch into specified encoding."),
            },
            HelpArg {
                name: "update",
                short: None,
                param: None,
                help: Some("Update the local working copy before applying the patch."),
            },
            HelpArg {
                name: "nocommit",
                short: None,
                param: None,
                help: Some(
                    "Normally under git/hg, if the patch is successful, the changes are committed to the working copy. This flag prevents the commit.",
                ),
            },
            HelpArg {
                name: "skip-dependencies",
                short: None,
                param: None,
                help: Some(
                    "Normally, if a patch has dependencies that are not present in the working copy, arc tries to apply them as well. This flag prevents such work.",
                ),
            },
            HelpArg {
                name: "nobranch",
                short: None,
                param: None,
                help: Some(
                    "Normally, a new branch (git) or bookmark (hg) is created and then the patch is applied and committed in the new branch/bookmark. This flag cherry-picks the resultant commit onto the original branch and deletes the temporary branch.",
                ),
            },
            HelpArg {
                name: "force",
                short: None,
                param: None,
                help: Some("Do not run any sanity checks."),
            },
        ],
    },
    HelpEntry {
        name: "prompts",
        examples: Some("**prompts** __workflow__"),
        synopsis: None,
        help: Some(
            "Show information about prompts a workflow may execute, and review saved\n\
            responses.\n\
            \n\
            **Show Prompts**\n\
            \n\
            To show possible prompts a workflow may execute, run:\n\
            \n\
            \x20 $ arc prompts __workflow__\n\
            \n\
            **Saving Responses**\n\
            \n\
            If you always want to answer a particular prompt in a certain way, you can\n\
            save your response to the prompt. When you encounter the prompt again, your\n\
            saved response will be used automatically.\n\
            \n\
            To save a response, add \"*\" or \"!\" to the end of the response you want to save\n\
            when you answer the prompt:\n\
            \n\
            \x20 - Using \"*\" will save the response in user configuration. In the future,\n\
            \x20 \x20 the saved answer will be used any time you encounter the prompt (in any\n\
            \x20 \x20 project).\n\
            \x20 - Using \"!\" will save the response in working copy configuration. In the\n\
            \x20 \x20 future, the saved answer will be used when you encounter the prompt in\n\
            \x20 \x20 the current working copy.\n\
            \n\
            For example, if you would like to always answer \"y\" to a particular prompt,\n\
            respond with \"y*\" or \"y!\" to save your response.\n",
        ),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "set-config",
        examples: None,
        synopsis: Some("      **set-config** [__options__] -- __name__ __value__"),
        help: Some(
            "          Supports: cli\n          Sets an arc configuration option.\n\n          Options are either user (apply to all arc commands you invoke\n          from the current user) or local (apply only to the current working\n          copy). By default, user configuration is written. Use __--local__\n          to write local configuration.\n\n          User values are written to '~/.arcrc' on Linux and Mac OS X, and an\n          undisclosed location on Windows. Local values are written to an arc\n          directory under either .git, .hg, or .svn as appropriate.",
        ),
        unwrap_help: false,
        args: &[HelpArg {
            name: "local",
            short: None,
            param: None,
            help: Some("Set a local config value instead of a user one."),
        }],
    },
    HelpEntry {
        name: "shell-complete",
        examples: None,
        synopsis: Some("Install shell completion."),
        help: Some(
            "Install shell completion so you can use the \"tab\" key to autocomplete\n\
            commands and flags in your shell for Arcanist toolsets and workflows.\n\
            \n\
            The **bash** shell is supported.\n\
            \n\
            **Installing Completion**\n\
            \n\
            To install shell completion, run the command:\n\
            \n\
            \x20 $ arc shell-complete\n\
            \n\
            This will install shell completion into your current shell. After installing,\n\
            you may need to start a new shell (or open a new terminal window) to pick up\n\
            the updated configuration.\n\
            \n\
            Once installed, completion should work across all Arcanist toolsets.\n\
            \n\
            **Using Completion**\n\
            \n\
            After completion is installed, use the \"tab\" key to automatically complete\n\
            workflows and flags. For example, if you type:\n\
            \n\
            \x20 $ arc diff --draf<tab>\n\
            \n\
            ...your shell should automatically expand the flag to:\n\
            \n\
            \x20 $ arc diff --draft\n\
            \n\
            **Updating Completion**\n\
            \n\
            To update shell completion, run the same command:\n\
            \n\
            \x20 $ arc shell-complete\n\
            \n\
            You can update shell completion without reinstalling it by running:\n\
            \n\
            \x20 $ arc shell-complete --generate\n\
            \n\
            You may need to update shell completion if:\n\
            \n\
            \x20 - you install new Arcanist toolsets; or\n\
            \x20 - you move the Arcanist directory; or\n\
            \x20 - you upgrade Arcanist and the new version fixes shell completion bugs.",
        ),
        unwrap_help: true,
        args: &[
            HelpArg {
                name: "current",
                short: None,
                param: Some("cursor-position"),
                help: Some("Internal. Current term in the argument list being completed."),
            },
            HelpArg {
                name: "generate",
                short: None,
                param: None,
                help: Some(
                    "Regenerate shell completion rules, without installing any configuration.",
                ),
            },
            HelpArg {
                name: "shell",
                short: None,
                param: Some("shell-name"),
                help: Some("Install completion support for a particular shell."),
            },
        ],
    },
    HelpEntry {
        name: "tasks",
        examples: None,
        synopsis: Some("      **tasks** [__options__]"),
        help: Some("        View all assigned tasks."),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "status",
                short: None,
                param: Some("task_status"),
                help: Some("Show tasks that are open or closed, default is open."),
            },
            HelpArg {
                name: "owner",
                short: None,
                param: Some("username"),
                help: Some(
                    "Only show tasks assigned to the given username, also accepts @all to show all, default is you.",
                ),
            },
            HelpArg {
                name: "order",
                short: None,
                param: Some("task_order"),
                help: Some(
                    "Arrange tasks based on priority, created, or modified, default is priority.",
                ),
            },
            HelpArg {
                name: "limit",
                short: None,
                param: Some("n"),
                help: Some("Limit the amount of tasks outputted, default is all."),
            },
            HelpArg {
                name: "unassigned",
                short: None,
                param: None,
                help: Some("Only show tasks that are not assigned (upforgrabs)."),
            },
        ],
    },
    HelpEntry {
        name: "todo",
        examples: None,
        synopsis: Some("      **todo** __summary__ [__options__]"),
        help: Some("        Quickly create a task for yourself."),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "cc",
                short: Some("C"),
                param: Some("cc"),
                help: Some("Other users to CC on the new task."),
            },
            HelpArg {
                name: "project",
                short: None,
                param: Some("project"),
                help: Some("Projects to assign to the task."),
            },
            HelpArg {
                name: "browse",
                short: None,
                param: None,
                help: Some("After creating the task, open it in a web browser."),
            },
        ],
    },
    HelpEntry {
        name: "unit",
        examples: None,
        synopsis: Some(
            "      **unit** [__options__] [__paths__]\n      **unit** [__options__] --rev [__rev__]",
        ),
        help: Some(
            "          Supports: git, svn, hg\n          Run unit tests that cover specified paths. If no paths are specified,\n          unit tests covering all modified files will be run.",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "rev",
                short: None,
                param: Some("revision"),
                help: Some("Run unit tests covering changes since a specific revision."),
            },
            HelpArg {
                name: "engine",
                short: None,
                param: Some("classname"),
                help: Some("Override configured unit engine for this project."),
            },
            HelpArg {
                name: "coverage",
                short: None,
                param: None,
                help: Some("Always enable coverage information."),
            },
            HelpArg {
                name: "no-coverage",
                short: None,
                param: None,
                help: Some("Always disable coverage information."),
            },
            HelpArg {
                name: "detailed-coverage",
                short: None,
                param: None,
                help: Some("Show a detailed coverage report on the CLI. Implies --coverage."),
            },
            HelpArg {
                name: "json",
                short: None,
                param: None,
                help: Some("Report results in JSON format."),
            },
            HelpArg {
                name: "output",
                short: None,
                param: Some("format"),
                help: Some(
                    "With 'full', show full pretty report (Default). With 'json', report results in JSON format. With 'ugly', use uglier (but more efficient) JSON formatting. With 'none', don't print results.",
                ),
            },
            HelpArg {
                name: "target",
                short: None,
                param: Some("phid"),
                help: Some(
                    "(PROTOTYPE) Record a copy of the test results on the specified Harbormaster build target.",
                ),
            },
            HelpArg {
                name: "everything",
                short: None,
                param: None,
                help: Some("Run every test associated with a tracked file in the working copy."),
            },
            HelpArg {
                name: "ugly",
                short: None,
                param: None,
                help: Some("With --json, use uglier (but more efficient) formatting."),
            },
        ],
    },
    HelpEntry {
        name: "upgrade",
        examples: Some("**upgrade**"),
        synopsis: Some("Upgrade Arcanist to the latest version."),
        help: Some("Upgrade Arcanist to the latest version."),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "upload",
        examples: Some("**upload** [__options__] -- __file__ [__file__ ...]"),
        synopsis: Some("Upload files."),
        help: Some("Upload one or more files from local disk to Phabricator."),
        unwrap_help: true,
        args: &[
            HelpArg {
                name: "json",
                short: None,
                param: None,
                help: Some("Output upload information in JSON format."),
            },
            HelpArg {
                name: "browse",
                short: None,
                param: None,
                help: Some("After the upload completes, open the files in a web browser."),
            },
            HelpArg {
                name: "temporary",
                short: None,
                param: None,
                help: Some(
                    "Mark the file as temporary. Temporary files will be deleted after 24 hours.",
                ),
            },
        ],
    },
    HelpEntry {
        name: "version",
        examples: Some("**version**"),
        synopsis: Some("Show toolset version information."),
        help: Some("Shows the current version."),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "weld",
        examples: Some("**weld** [options] -- __file__ __file__ ..."),
        synopsis: Some("Robustly fuse files together."),
        help: Some(
            "Robustly fuse two or more files together. The resulting joint is much stronger\nthan the one created by tools like __cat__.",
        ),
        unwrap_help: true,
        args: &[],
    },
    HelpEntry {
        name: "which",
        examples: None,
        synopsis: Some(
            "      **which** [options] (svn)\n      **which** [options] [__commit__] (hg, git)",
        ),
        help: Some(
            "          Supports: svn, git, hg\n          Shows which repository the current working copy corresponds to,\n          which commits 'arc diff' will select, and which revision is in\n          the working copy (or which revisions, if more than one matches).",
        ),
        unwrap_help: false,
        args: &[
            HelpArg {
                name: "any-status",
                short: None,
                param: None,
                help: Some("Show committed and abandoned revisions."),
            },
            HelpArg {
                name: "base",
                short: None,
                param: Some("rules"),
                help: Some("Additional rules for determining base revision."),
            },
            HelpArg {
                name: "show-base",
                short: None,
                param: None,
                help: Some("Print base commit only and exit."),
            },
            HelpArg {
                name: "head",
                short: None,
                param: Some("commit"),
                help: Some("Specify the end of the commit range to select."),
            },
        ],
    },
    HelpEntry {
        name: "work",
        examples: Some("**work** [--start __start__] __symbol__"),
        synopsis: Some("Begin or resume work."),
        help: Some(
            "Begin or resume work on a branch, bookmark, task, or revision.\n\
            \n\
            The __symbol__ may be a branch or bookmark name, a revision name (like \"D123\"),\n\
            a task name (like \"T123\"), or a new symbol.\n\
            \n\
            If you provide a symbol which currently does not identify any ongoing work,\n\
            Arcanist will create a new branch or bookmark with the name you provide.\n\
            \n\
            If you provide the name of an existing branch or bookmark, Arcanist will switch\n\
            to that branch or bookmark.\n\
            \n\
            If you provide the name of a revision or task, Arcanist will look for a related\n\
            branch or bookmark that exists in the working copy. If it finds one, it will\n\
            switch to it. If it does not find one, it will attempt to create a new branch\n\
            or bookmark.\n\
            \n\
            When \"arc work\" creates a branch or bookmark, it will use **--start** as the\n\
            branchpoint if it is provided. Otherwise, the current working copy state will\n\
            serve as the starting point.",
        ),
        unwrap_help: true,
        args: &[HelpArg {
            name: "start",
            short: None,
            param: Some("symbol"),
            help: Some("When creating a new branch or bookmark, use this as the branch point."),
        }],
    },
];

/// All 37 concrete workflows at ceb (toolset 21 + legacy 16,
/// counted from `getWorkflowName` definitions, T1153 CORE-66): the
/// classic resolution set and the `help` listing without the one
/// Rust-only extra (`markers`, abstract in PHP).
const ALL_WORKFLOWS: &[&str] = &[
    "alias",
    "amend",
    "anoid",
    "bookmarks",
    "branches",
    "browse",
    "call-conduit",
    "close-revision",
    "commit",
    "cover",
    "diff",
    "download",
    "export",
    "get-config",
    "help",
    "inspect",
    "install-certificate",
    "land",
    "liberate",
    "lint",
    "linters",
    "list",
    "look",
    "paste",
    "patch",
    "prompts",
    "set-config",
    "shell-complete",
    "tasks",
    "todo",
    "unit",
    "upgrade",
    "upload",
    "version",
    "weld",
    "which",
    "work",
];

/// Parser specs known when bare help renders: standard, config, and
// T1153 RB-D5: `indent_text`/`unwrap_help`/`render_argument_specs` live
// in the shared `help` renderer; the functions below call `help::`.
/// toolset arguments (`renderHelp()` shows the non-standard ones, plus
/// the standard ones under `--show-standard-options`).
/// A global spec: name, short flag, parameter, help, standard.
type GlobalSpec = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    bool,
);

const GLOBAL_SPECS: &[GlobalSpec] = &[
    (
        "trace",
        None,
        None,
        "Trace command execution and show service calls.",
        true,
    ),
    (
        "no-ansi",
        None,
        None,
        "Disable ANSI terminal codes, printing plain text with no color or style.",
        true,
    ),
    (
        "ansi",
        None,
        None,
        "Use formatting even in environments which probably don't support it.",
        true,
    ),
    (
        "xprofile",
        None,
        Some("profile"),
        "Profile script execution and write results to a file.",
        true,
    ),
    ("help", Some("h"), None, "Show this help.", true),
    (
        "show-standard-options",
        None,
        None,
        "Show every option, including standard options like this one.",
        true,
    ),
    ("recon", None, None, "Start in remote console mode.", true),
    ("library", None, Some("path"), "Load a library.", false),
    (
        "config",
        None,
        Some("key=value"),
        "Specify a runtime configuration value.",
        false,
    ),
    (
        "config-file",
        None,
        Some("path"),
        "Load one or more configuration files. If this flag is provided, the system and user configuration files are ignored.",
        false,
    ),
    (
        "conduit-uri",
        None,
        Some("uri"),
        "Connect to Phabricator install specified by __uri__.",
        false,
    ),
    (
        "conduit-token",
        None,
        Some("token"),
        "Use a specific authentication token.",
        false,
    ),
];

/// `renderWorkflowHelp()`: examples + synopsis, plus the full help
/// and argument specs when `details` is set. Unknown names render the
/// no-workflow message. Always ends with a newline.
fn render_workflow_help(name: &str, details: bool, ansi: bool) -> String {
    let indent = if details { 0 } else { 6 };
    let Some(entry) = HELP_ENTRIES
        .iter()
        .find(|entry| entry.name == name.to_lowercase())
    else {
        return format!(
            "{}\n",
            terminal::console_wrap(&help::no_workflow_line(name, ansi), indent, true)
        );
    };
    let examples = entry
        .examples
        .map(str::to_string)
        .unwrap_or_else(|| format!("**{}**", entry.name));
    let mut parts = vec![
        help::indent_text(indent, &examples, ansi),
        help::indent_text(indent, entry.synopsis.unwrap_or(""), ansi),
    ];
    if details {
        if let Some(help) = entry.help {
            let body = if entry.unwrap_help {
                help::unwrap_help(help)
            } else {
                help.to_string()
            };
            parts.push(String::new());
            parts.push(help::indent_text(indent, &body, ansi));
        }
        if !entry.args.is_empty() {
            let specs: Vec<help::SpecView<'_>> = entry
                .args
                .iter()
                .map(|arg| (arg.name, arg.short, arg.param, arg.help))
                .collect();
            parts.push(String::new());
            parts.push(help::render_argument_specs(&specs, ansi));
        }
    }
    parts.push(String::new());
    parts.join("\n")
}

/// `renderHelp()`: the `WORKFLOWS` reference (ksorted, `help` included
/// like PHP), the `OPTION REFERENCE`, and the hints. No trailing
/// newline beyond the joined lines.
fn render_help_bare(show_standard_options: bool, ansi: bool) -> String {
    let mut entries: Vec<&HelpEntry> = HELP_ENTRIES.iter().collect();
    entries.sort_by(|a, b| a.name.cmp(b.name));
    let mut parts = vec![terminal::interpret("**WORKFLOWS**", ansi), String::new()];
    for entry in entries {
        parts.push(render_workflow_help(entry.name, false, ansi));
    }
    parts.push(terminal::interpret("**OPTION REFERENCE**", ansi));
    parts.push(String::new());
    let specs: Vec<help::SpecView<'_>> = GLOBAL_SPECS
        .iter()
        .filter(|spec| show_standard_options || !spec.4)
        .map(|spec| (spec.0, spec.1, spec.2, Some(spec.3)))
        .collect();
    parts.push(help::render_argument_specs(&specs, ansi));
    parts.push(String::new());
    parts.push(help::indent_text(
        0,
        "Use **help** __command__ for a detailed command reference.",
        ansi,
    ));
    if !show_standard_options {
        parts.push(help::indent_text(
            0,
            "Use __--show-standard-options__ to show additional options.",
            ansi,
        ));
    }
    parts.push(String::new());
    parts.join("\n")
}

/// `help [name ...]`, mirroring `PhutilHelpArgumentWorkflow::execute`:
/// bare `help` prints the command reference; otherwise each name
/// prints a `**NAME WORKFLOW**` header plus detailed help, or a
/// no-workflow message when nothing matches. Always exits 0; PHP's
/// pager is intentionally not ported.
fn run_help(names: &[String], show_standard_options: bool) -> ExitCode {
    if names.is_empty() {
        print!(
            "{}",
            render_help_bare(show_standard_options, terminal::ansi_enabled())
        );
        return ExitCode::SUCCESS;
    }
    let ansi = terminal::ansi_enabled();
    for name in names {
        println!("{}\n", help::workflow_header(name, ansi));
        print!("{}", render_workflow_help(name, true, ansi));
        println!();
    }
    ExitCode::SUCCESS
}

/* -(  Command resolution  )-------------------------------------------- */

/// Resolution of the command word: toolset first (exact, then
/// spelling-corrected when interactive), then the classic stages
/// (exact, unique prefix, spelling-corrected), else unknown.
enum Resolution {
    /// Run `name` (`notice`, if any, goes to stderr first).
    Run {
        name: &'static str,
        notice: Option<String>,
    },
    /// Report the unknown-command message through the usage arm.
    Unknown { message: String },
}

/// True for the 16 legacy workflows (the classic subprocess sets
/// `ARCANIST` for exactly these; toolset commands never see it).
fn is_classic_command(name: &str) -> bool {
    CLASSIC_COMMANDS.contains(&name)
}

/// `raiseUnknownCommand()`: `Unknown command '%s'. Try 'arc help'.`
/// plus the sorted, 4-space `Did you mean:` list.
fn format_unknown_command(command: &str, maybe: &[String]) -> String {
    let mut message = format!("Unknown command '{command}'. Try 'arc help'.");
    if !maybe.is_empty() {
        let mut sorted = maybe.to_vec();
        sorted.sort();
        message.push_str("\n\nDid you mean:\n");
        for other in sorted {
            message.push_str(&format!("    {other}\n"));
        }
        message.pop();
    }
    message
}

fn resolve_command(first: &str, interactive: bool) -> Resolution {
    if alias::toolset_names().contains(&first) || TOOLSET_EXTRA_COMMANDS.contains(&first) {
        return Resolution::Run {
            name: find_static_name(first).unwrap_or("help"),
            notice: None,
        };
    }
    let corrected = argparse::correct_commands(first, alias::toolset_names());
    if corrected.len() == 1 && interactive {
        let name = find_static_name(&corrected[0]).unwrap_or("help");
        return Resolution::Run {
            name,
            notice: Some(format!(
                "(Assuming \"{first}\" is the British spelling of \"{name}\".)"
            )),
        };
    }
    let command = first.to_lowercase();
    if ALL_WORKFLOWS.contains(&command.as_str()) {
        return Resolution::Run {
            name: find_static_name(&command).unwrap_or("help"),
            notice: None,
        };
    }
    let prefixes: Vec<String> = ALL_WORKFLOWS
        .iter()
        .filter(|name| name.starts_with(command.as_str()))
        .map(|name| name.to_string())
        .collect();
    if prefixes.len() == 1 {
        return Resolution::Run {
            name: find_static_name(&prefixes[0]).unwrap_or("help"),
            notice: None,
        };
    }
    if prefixes.len() > 1 {
        return Resolution::Unknown {
            message: format_unknown_command(&command, &prefixes),
        };
    }
    let corrected = argparse::correct_commands(&command, ALL_WORKFLOWS);
    if corrected.len() == 1 {
        let name = find_static_name(&corrected[0]).unwrap_or("help");
        return Resolution::Run {
            name,
            notice: Some(format!(
                "(Assuming '{command}' is the British spelling of '{name}'.)"
            )),
        };
    }
    if corrected.len() > 1 {
        return Resolution::Unknown {
            message: format_unknown_command(&command, &corrected),
        };
    }
    Resolution::Unknown {
        message: format_unknown_command(&command, &[]),
    }
}

/// Map a resolved name back to its static table entry.
fn find_static_name(name: &str) -> Option<&'static str> {
    ALL_WORKFLOWS
        .iter()
        .chain(TOOLSET_EXTRA_COMMANDS.iter())
        .find(|known| ***known == *name)
        .copied()
}

/// True when the invocation needs the non-interactive `--` error: a
/// toolset command (exact match, since autocorrect is off when piped)
/// whose flow arguments contain no `--` terminator
/// (`ArcanistRuntime.php:84-86,170-195` at ceb, T1153 CORE-17 —
/// including `call-conduit`).
fn needs_terminator_error(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let toolset = alias::toolset_names().contains(&first.as_str())
        || TOOLSET_EXTRA_COMMANDS.contains(&first.as_str());
    toolset && !argv[1..].iter().any(|arg| arg == "--")
}

/// Test entry point: strip globals, validate `--config`, then
/// dispatch. (`main` uses [`dispatch_inner`] directly so the consumed
/// help flags survive alias expansion.)
#[allow(dead_code)]
fn dispatch(argv: &[String]) -> ExitCode {
    let (argv, globals) = match strip_globals(argv) {
        Ok(pair) => pair,
        Err(error) => return conduit::report(error.kind, &error.message),
    };
    if let Err(error) = validate_runtime_config(&globals.config) {
        return conduit::report(error.kind, &error.message);
    }
    dispatch_inner(&argv, &globals)
}

// T1153 RB-D5: the `usage:` lead lives in the shared `help` renderer
// (the release smoke test greps it); both bare dispatches use it.

/// Dispatch a bare `arc` (no command word).
fn dispatch_empty(globals: &Globals) -> ExitCode {
    if globals.show_help {
        print!(
            "{}{}",
            help::BARE_HELP_LEAD,
            render_help_bare(globals.show_standard_options, terminal::ansi_enabled())
        );
        return ExitCode::SUCCESS;
    }
    conduit::report(conduit::EnvelopeKind::ToolsetUsage, "Choose a workflow!")
}

/// Dispatch with `--help` anywhere (`cmd FLOW --help` runs `cmd help
/// FLOW`): resolve first so typos still report the unknown-command
/// error. `argv` is non-empty (checked by the caller).
fn dispatch_help_flag(argv: &[String], globals: &Globals) -> ExitCode {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal();
    let first = argv[0].clone();
    if first == "call-conduit" {
        return run_help(&["call-conduit".to_string()], globals.show_standard_options);
    }
    match resolve_command(&first, interactive) {
        Resolution::Run { name, .. } => {
            run_help(&[name.to_string()], globals.show_standard_options)
        }
        Resolution::Unknown { message } => conduit::report(conduit::EnvelopeKind::Usage, &message),
    }
}

/// Report a `call-conduit` argument error: the method-count error is a
/// `runWorkflow()` usage error (red label); flag errors are parser
/// errors (bold label). Exits match at 1 either way.
fn report_call_conduit_error(msg: &str) -> ExitCode {
    let kind = if msg == "Provide exactly one Conduit method name to call." {
        conduit::EnvelopeKind::ToolsetUsage
    } else {
        conduit::EnvelopeKind::ToolsetParseError
    };
    conduit::report(kind, msg)
}

/// Map a resolved command name to its runner (`None` for `help`, which
/// renders the bare help instead of running a workflow).
fn resolve_runner(name: &str) -> Option<CommandFn> {
    match name {
        "help" => None,
        "prompts" => Some(prompts::run),
        "shell-complete" => Some(shell_complete::run),
        _ => find_command(name),
    }
}

/// Run a resolved command: print its spelling notice, export the
/// classic name for hooks, then run.
fn dispatch_run(
    name: &'static str,
    notice: Option<&str>,
    first: &str,
    argv: &[String],
    globals: &Globals,
) -> ExitCode {
    if let Some(notice) = notice {
        let ansi = terminal::ansi_enabled();
        eprint!(
            "{}",
            terminal::tsprintf_ansi("%s\n", &[terminal::TVal::Text(notice)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default()
        );
    }
    if is_classic_command(name) {
        // The classic subprocess exports the resolved command for
        // hooks (`scripts/arcanist.php:229`).
        arc_rs::globals::set_command_name(name.to_string());
    }
    match resolve_runner(name) {
        Some(run) => run(&argv[1..]),
        None if name == "help" => run_help(&[], globals.show_standard_options),
        None => conduit::report(
            conduit::EnvelopeKind::Usage,
            &format_unknown_command(&first.to_lowercase(), &[]),
        ),
    }
}

/// Dispatch an unclassified command word through resolution.
fn dispatch_unknown(first: &str, argv: &[String], globals: &Globals) -> ExitCode {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal();
    match resolve_command(first, interactive) {
        Resolution::Run { name, notice } => {
            dispatch_run(name, notice.as_deref(), first, argv, globals)
        }
        Resolution::Unknown { message } => conduit::report(conduit::EnvelopeKind::Usage, &message),
    }
}

/// Dispatch one classified top-level action.
fn dispatch_action(action: Action, argv: &[String], globals: &Globals) -> ExitCode {
    match action {
        Action::Help => {
            print!(
                "{}{}",
                help::BARE_HELP_LEAD,
                render_help_bare(globals.show_standard_options, terminal::ansi_enabled())
            );
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("arc {VERSION}");
            ExitCode::SUCCESS
        }
        Action::CallConduit(args) => run_call_conduit(&args),
        Action::CallConduitHelp => {
            run_help(&["call-conduit".to_string()], globals.show_standard_options)
        }
        Action::Missing => {
            conduit::report(conduit::EnvelopeKind::ToolsetUsage, "Choose a workflow!")
        }
        Action::Unknown(first) => dispatch_unknown(&first, argv, globals),
        Action::Error(msg) => report_call_conduit_error(&msg),
    }
}

/// Dispatch already-stripped `argv` with its globals. `main` calls
/// this after alias expansion (which needs the stripped argv); the
/// flags must flow through instead of being re-stripped, since
/// re-stripping would lose `--help`/`--show-standard-options`.
fn dispatch_inner(argv: &[String], globals: &Globals) -> ExitCode {
    if argv.is_empty() {
        return dispatch_empty(globals);
    }
    if argv.first().is_some_and(|first| first == "help") {
        let mut names = argv[1..].to_vec();
        if names.first().is_some_and(|name| name == "--") {
            names.remove(0);
        }
        return run_help(&names, globals.show_standard_options);
    }
    if globals.show_help {
        return dispatch_help_flag(argv, globals);
    }
    dispatch_action(classify(argv), argv, globals)
}

/// Binary entry: strip globals, trace, expand aliases, gate the
/// non-interactive `--` rule, dispatch. T1153 CORE-18 stays unported
/// (Rule 3): PHP's SIGINT double-press routing needs `pcntl_signal`,
/// which has no `std` equivalent (`libc` is transitive-only and
/// `Cargo.toml` is frozen), so interrupts keep the default behavior.
fn main() -> ExitCode {
    let full: Vec<String> = std::env::args().collect();
    let argv: Vec<String> = full.iter().skip(1).cloned().collect();
    let (stripped, globals) = match strip_globals(&argv) {
        Ok(pair) => pair,
        Err(error) => return conduit::report(error.kind, &error.message),
    };
    if let Err(error) = apply_globals(&globals) {
        return conduit::report(error.kind, &error.message);
    }
    if terminal::trace_enabled() {
        trace_argv(&full);
    }
    let argv = match apply_alias_expansion(stripped) {
        Ok(argv) => argv,
        Err(code) => return code,
    };
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) && needs_terminator_error(&argv) {
        let ansi = terminal::ansi_enabled();
        eprint!(
            "{}",
            terminal::format_log(
                "cyan",
                "USAGE",
                "\"arc\" is being run noninteractively, but the argument list is missing \"--\" to indicate end of flags.",
                ansi
            )
        );
        eprint!(
            "{}",
            terminal::format_log(
                "cyan",
                "USAGE",
                "When running noninteractively, you MUST provide \"--\" to all commands (even if they take no arguments).",
                ansi
            )
        );
        let learn_more = terminal::tsprintf_ansi(
            "%s <__%s__>",
            &[
                terminal::TVal::Text("Learn More:"),
                terminal::TVal::Text("https://phurl.io/u/noninteractive"),
            ],
            ansi,
        )
        .map(|t| t.to_string())
        .unwrap_or_default();
        let body = terminal::TerminalString::new(learn_more);
        eprint!(
            "{}",
            terminal::tsprintf_ansi(
                "**<bg:cyan> %s </bg>** %s\n",
                &[
                    terminal::TVal::Text("USAGE"),
                    terminal::TVal::Terminal(&body),
                ],
                ansi
            )
            .map(|t| t.to_string())
            .unwrap_or_default()
        );
        return conduit::report(
            conduit::EnvelopeKind::ToolsetUsage,
            "Missing required \"--\" in argument list.",
        );
    }
    dispatch_inner(&argv, &globals)
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod main_tests;
