//! `arc-rs` — Rust port of Arcanist.
//!
//! Tracks T1128. All arc subcommands are implemented and the ported depth
//! is wired into the run paths; remaining work is the coverage push past
//! 80% and removing the PHP tree.

use std::process::ExitCode;

use arc_rs::{
    amend, anoid, bookmarks, branches, close_revision, commit, config, cover, diff, download,
    export, get_config, inspect, install_certificate, land, liberate, lint, linters, list, look,
    markers, paste, patch, set_config, tasks, todo, unit, upgrade, upload, weld, which, work,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const PLANNED: &[&str] = &[];

fn usage() -> String {
    let mut text = "usage: arc-rs [--version] [--help] <command>\n\ncommands (see <command> --help):\n  amend\n  anoid\n  bookmarks\n  branches\n  call-conduit\n  close-revision\n  commit\n  cover\n  diff\n  download\n  export\n  get-config\n  inspect\n  install-certificate\n  land\n  liberate\n  lint\n  linters\n  list\n  look\n  markers\n  paste\n  patch\n  set-config\n  tasks\n  todo\n  unit\n  upgrade\n  upload\n  weld\n  which\n  work\n"
        .to_string();
    if !PLANNED.is_empty() {
        let planned = PLANNED
            .iter()
            .map(|c| format!("  {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        text.push_str(&format!(
            "\nplanned commands (not yet ported):\n{planned}\n"
        ));
    }
    text
}

#[derive(Debug, PartialEq, Eq)]
struct CallArgs {
    method: String,
    params_json: String,
    conduit_uri: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
    CallConduit(CallArgs),
    Planned(String),
    Unknown(String),
    Missing,
    Error(String),
}

fn parse_call_conduit(rest: &[String]) -> Result<CallArgs, String> {
    let mut iter = rest.iter();
    let Some(method) = iter.next() else {
        return Err("call-conduit requires a method name".to_string());
    };
    let mut params_json = "{}".to_string();
    let mut conduit_uri = None;
    let mut positional = false;
    while let Some(arg) = iter.next() {
        if arg == "--conduit-uri" {
            let Some(uri) = iter.next() else {
                return Err("--conduit-uri requires a value".to_string());
            };
            conduit_uri = Some(uri.clone());
        } else if arg.starts_with("--") {
            return Err(format!("unknown flag '{arg}'"));
        } else if positional {
            return Err("call-conduit takes at most one params-json argument".to_string());
        } else {
            params_json = arg.clone();
            positional = true;
        }
    }
    Ok(CallArgs {
        method: method.clone(),
        params_json,
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
        c if PLANNED.contains(&c) => Action::Planned(c.to_string()),
        c => Action::Unknown(c.to_string()),
    }
}

fn run_call_conduit(args: &CallArgs) -> ExitCode {
    let params: serde_json::Value = match serde_json::from_str(&args.params_json) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("arc-rs: invalid params JSON: {e}");
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
    match client.call(&args.method, &params) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "null".to_string())
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("arc-rs: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = argv.first() {
        type CommandFn = fn(&[String]) -> ExitCode;
        const COMMANDS: &[(&str, CommandFn)] = &[
            (amend::NAME, amend::run),
            (anoid::NAME, anoid::run),
            (bookmarks::NAME, bookmarks::run),
            (branches::NAME, branches::run),
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
        if let Some((_, run)) = COMMANDS.iter().find(|(name, _)| *name == cmd.as_str()) {
            return run(&argv[1..]);
        }
    }
    match classify(&argv) {
        Action::Help => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("arc-rs {VERSION}");
            ExitCode::SUCCESS
        }
        Action::CallConduit(args) => run_call_conduit(&args),
        Action::Planned(c) => {
            eprintln!("arc-rs {c}: not yet ported (see T1128)");
            ExitCode::FAILURE
        }
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

#[cfg(test)]
#[path = "main_tests.rs"]
mod main_tests;
