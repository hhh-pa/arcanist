//! `arc-rs` — Rust port of Arcanist.
//!
//! Tracks T1128. All arc subcommands are implemented and the ported depth
//! is wired into the run paths; remaining work is the coverage push past
//! 80% and removing the PHP tree.

use std::process::ExitCode;

use arc_rs::{
    alias, amend, anoid, bookmarks, branches, browse, close_revision, commit, config, cover, diff,
    download, export, get_config, inspect, install_certificate, land, liberate, lint, linters,
    list, look, markers, paste, patch, set_config, tasks, todo, unit, upgrade, upload, weld, which,
    work,
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
    (weld::NAME, weld::run),
    (which::NAME, which::run),
    (work::NAME, work::run),
];

fn usage() -> String {
    "usage: arc-rs [--version] [--help] <command>\n\ncommands (see <command> --help):\n  alias\n  amend\n  anoid\n  bookmarks\n  branches\n  browse\n  call-conduit\n  close-revision\n  commit\n  cover\n  diff\n  download\n  export\n  get-config\n  inspect\n  install-certificate\n  land\n  liberate\n  lint\n  linters\n  list\n  look\n  markers\n  paste\n  patch\n  set-config\n  tasks\n  todo\n  unit\n  upgrade\n  upload\n  weld\n  which\n  work\n"
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
    Unknown(String),
    Missing,
    Error(String),
}

fn parse_call_conduit(rest: &[String]) -> Result<CallArgs, String> {
    // PHP (`ArcanistCallConduitWorkflow.php:31-44`): `method` is a wildcard
    // argument and exactly one name is required; params come from stdin,
    // never from argv.
    let mut methods = Vec::new();
    let mut conduit_uri = None;
    let mut positional_only = false;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        if !positional_only && arg == "--" {
            positional_only = true;
        } else if !positional_only && arg == "--conduit-uri" {
            let Some(uri) = iter.next() else {
                return Err("--conduit-uri requires a value".to_string());
            };
            conduit_uri = Some(uri.clone());
        } else if !positional_only && arg.starts_with("--") {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            methods.push(arg.clone());
        }
    }
    let [method] = methods.as_slice() else {
        return Err("Provide exactly one Conduit method name to call.".to_string());
    };
    Ok(CallArgs {
        method: method.clone(),
        conduit_uri,
    })
}

fn classify(argv: &[String]) -> Action {
    let Some(cmd) = argv.first() else {
        return Action::Missing;
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => Action::Help,
        "-V" | "--version" => Action::Version,
        "call-conduit" => match parse_call_conduit(&argv[1..]) {
            Ok(args) => Action::CallConduit(args),
            Err(msg) => Action::Error(msg),
        },
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

fn run_call_conduit_with<R: std::io::Read, W: std::io::Write>(
    args: &CallArgs,
    stdin: R,
    stdout: &mut W,
) -> ExitCode {
    let params = match read_params_json(stdin) {
        Ok(value) => value,
        Err(msg) => {
            eprintln!("arc-rs: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let client = match config::client_for(args.conduit_uri.as_deref()) {
        Ok(client) => client,
        Err(msg) => {
            eprintln!("arc-rs: {msg}");
            return ExitCode::FAILURE;
        }
    };
    // API errors land in the envelope with exit 0, exactly like PHP
    // (`:57-74`); transport failures stay hard errors.
    let text = match client.call(&args.method, &params) {
        Ok(result) => render_envelope(None, result),
        Err(arc_rs::conduit::Error::Api { code, info }) => {
            render_envelope(Some((&code, &info)), serde_json::Value::Null)
        }
        Err(e) => {
            eprintln!("arc-rs: {e}");
            return ExitCode::FAILURE;
        }
    };
    // No trailing newline: PHP `echo`s the encoded blob as-is (`:67`).
    if stdout.write_all(text.as_bytes()).is_err() {
        eprintln!("arc-rs: failed to write output");
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

/// Expand `argv` through `aliases` before the `COMMANDS` lookup,
/// mirroring `ArcanistRuntime` running `ArcanistAliasEngine` over the
/// unconsumed argument vector. Leading flags are never commands (PHP
/// consumes toolset arguments before aliases run).
fn pre_dispatch(argv: Vec<String>, aliases: &[alias::Alias]) -> alias::Expansion {
    if argv.first().is_some_and(|first| first.starts_with('-')) {
        return alias::Expansion::Dispatch {
            argv,
            hints: Vec::new(),
        };
    }
    alias::expand(&argv, aliases, alias::is_builtin_command)
}

/// Execute a shell alias, mirroring `ArcanistRuntime::executeShellAlias()`.
fn execute_shell_alias(argv: &[String], hint: &str) -> ExitCode {
    eprintln!("SHELL ALIAS: {hint}");
    let Some((program, args)) = argv.split_first() else {
        eprintln!("arc-rs: empty shell alias");
        return ExitCode::FAILURE;
    };
    match std::process::Command::new(program).args(args).status() {
        Ok(status) => status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(e) => {
            eprintln!("arc-rs: cannot execute shell alias: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Expand `argv` through user aliases before dispatch. Returns the argv
/// to dispatch, or an exit code when expansion fully handles the
/// invocation (shell alias executed or broken chain). Unreadable
/// configuration is a pass-through: each command surfaces its own
/// config errors, so expansion must not break unrelated commands.
fn apply_alias_expansion(argv: Vec<String>) -> Result<Vec<String>, ExitCode> {
    if argv.first().is_none_or(|first| first.starts_with('-')) {
        return Ok(argv);
    }
    let (aliases, warnings) = alias::load_aliases().unwrap_or_default();
    for warning in &warnings {
        eprintln!("ALIAS: {warning}");
    }
    match pre_dispatch(argv, &aliases) {
        alias::Expansion::Dispatch { argv: next, hints } => {
            for hint in &hints {
                eprintln!("ALIAS: {hint}");
            }
            Ok(next)
        }
        alias::Expansion::Shell { argv: shell, hint } => Err(execute_shell_alias(&shell, &hint)),
        alias::Expansion::Failed { message, hints } => {
            for hint in &hints {
                eprintln!("ALIAS: {hint}");
            }
            eprintln!("arc-rs: {message}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn dispatch(argv: &[String]) -> ExitCode {
    if let Some(cmd) = argv.first()
        && let Some(run) = find_command(cmd)
    {
        return run(&argv[1..]);
    }
    match classify(argv) {
        Action::Help => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("arc-rs {VERSION}");
            ExitCode::SUCCESS
        }
        Action::CallConduit(args) => run_call_conduit(&args),
        Action::Missing => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
        Action::Unknown(c) => {
            eprintln!("arc-rs: unknown command '{c}'\n\n{}", usage());
            ExitCode::FAILURE
        }
        Action::Error(msg) => {
            eprintln!("arc-rs: {msg}\n\n{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let argv = match apply_alias_expansion(argv) {
        Ok(argv) => argv,
        Err(code) => return code,
    };
    dispatch(&argv)
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod main_tests;
