//! `arc-rs work`: begin or resume work on a branch.
//!
//! PHP references: `src/workflow/ArcanistWorkWorkflow.php` (exactly one
//! symbol, `--start`) and `src/work/ArcanistGitWorkEngine.php` (check out an
//! existing branch, else `checkout -b` from the startpoint, tracking it when
//! the startpoint is itself a branch).
//!
//! Scope: local git branches only. Revision (`D123`) and task (`T123`)
//! symbols need Conduit lookups and Mercurial bookmarks need `hg`, so both
//! fail with a not-yet-ported message.

use std::path::Path;
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::git;

/// Subcommand name, matching `arc work`.
pub const NAME: &str = "work";

fn usage() -> String {
    "usage: arc-rs work [--start START] SYMBOL\n\
     \n\
     Begin or resume work on a branch.\n\
     \n\
     If SYMBOL names an existing local branch, check it out. Otherwise create\n\
     a new branch named SYMBOL from --start (or the current branch or HEAD).\n\
     \n\
     Options:\n\
     \x20 --start START  Branch point for a new branch.\n\
     \x20 -h, --help     Show this help.\n\
     \n\
     Not yet ported (see T1128): revision (D123) and task (T123) symbols\n\
     via Conduit, and Mercurial bookmarks.\n"
        .to_string()
}

/// Parsed `work` options.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub start: Option<String>,
    pub symbol: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// Parse `argv` (excluding the subcommand name).
pub fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut start = None;
    let mut symbols = Vec::new();
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            symbols.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--start" {
            let Some(value) = iter.next() else {
                return ParseOutcome::Error("--start requires a value".to_string());
            };
            start = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--start=") {
            start = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            symbols.push(arg.clone());
        }
    }
    if symbols.is_empty() {
        return ParseOutcome::Error(
            "Provide a branch, bookmark, task, or revision name to begin or resume work on."
                .to_string(),
        );
    }
    if symbols.len() > 1 {
        return ParseOutcome::Error(
            "Too many arguments: provide exactly one argument.".to_string(),
        );
    }
    let symbol = symbols.pop().unwrap_or_default();
    if symbol.is_empty() {
        return ParseOutcome::Error(
            "Provide a nonempty symbol to begin or resume work on.".to_string(),
        );
    }
    ParseOutcome::Args(Options { start, symbol })
}

fn looks_like_monogram(symbol: &str, letter: char) -> bool {
    let mut chars = symbol.chars();
    matches!(chars.next(), Some(first) if first.eq_ignore_ascii_case(&letter))
        && !chars.as_str().is_empty()
        && chars.as_str().bytes().all(|b| b.is_ascii_digit())
}

/// True when `symbol` looks like a revision monogram (`D123`).
#[must_use]
pub fn looks_like_revision(symbol: &str) -> bool {
    looks_like_monogram(symbol, 'D')
}

/// True when `symbol` looks like a task monogram (`T123`).
#[must_use]
pub fn looks_like_task(symbol: &str) -> bool {
    looks_like_monogram(symbol, 'T')
}

/// True when `symbol` names an existing local branch.
#[must_use]
pub fn branch_exists(dir: &Path, symbol: &str) -> bool {
    let target = format!("refs/heads/{symbol}");
    git::ok(dir, &["show-ref", "--verify", "--quiet", &target])
}

/// Resolve `rev` to a commit SHA, or `None` when it does not resolve.
#[must_use]
pub fn resolve_commit(dir: &Path, rev: &str) -> Option<String> {
    let arg = format!("{rev}^{{commit}}");
    git::stdout_text(dir, &["rev-parse", "--verify", &arg])
        .ok()
        .filter(|sha| !sha.is_empty())
}

/// Default branch point: the current branch name, else the `HEAD` SHA.
#[must_use]
pub fn default_start(dir: &Path) -> Option<String> {
    if let Ok(name) = git::stdout_text(dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        && !name.is_empty()
    {
        return Some(name);
    }
    resolve_commit(dir, "HEAD")
}

/// True when `start` resolves to a local or remote-tracking branch, in which
/// case the new branch is created with `--track` like the PHP engine.
/// (`--track` is rejected by git for SHA or tag startpoints, so those
/// create a plain branch.)
fn start_is_branch(dir: &Path, start: &str) -> bool {
    let Ok(full) = git::stdout_text(
        dir,
        &["rev-parse", "--verify", "--symbolic-full-name", start],
    ) else {
        return false;
    };
    full.starts_with("refs/heads/") || full.starts_with("refs/remotes/")
}

fn work_outside_repo(dir: &Path) -> ExitCode {
    if dir.join(".hg").is_dir() {
        eprintln!("arc-rs {NAME}: Mercurial working copies are not yet ported (see T1128)");
    } else {
        eprintln!("arc-rs {NAME}: \"arc-rs work\" must be run in a Git or Mercurial working copy.");
    }
    ExitCode::FAILURE
}

fn checkout_existing(dir: &Path, symbol: &str) -> ExitCode {
    println!("BRANCH Checking out branch \"{symbol}\".");
    match git::stdout_text(dir, &["checkout", symbol, "--"]) {
        Ok(_) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("arc-rs {NAME}: git checkout failed: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Reject revision/task monograms, which need Conduit lookups.
fn reject_monogram(symbol: &str) -> Option<ExitCode> {
    if looks_like_revision(symbol) {
        eprintln!(
            "arc-rs {NAME}: resolving revision \"{symbol}\" via Conduit is not yet ported (see T1128)"
        );
        return Some(ExitCode::FAILURE);
    }
    if looks_like_task(symbol) {
        eprintln!(
            "arc-rs {NAME}: resolving task \"{symbol}\" via Conduit is not yet ported (see T1128)"
        );
        return Some(ExitCode::FAILURE);
    }
    None
}

fn resolve_start(dir: &Path, start: Option<String>) -> Result<String, ExitCode> {
    match start {
        Some(start) => {
            if resolve_commit(dir, &start).is_none() {
                eprintln!("arc-rs {NAME}: Unable to resolve startpoint \"{start}\".");
                return Err(ExitCode::FAILURE);
            }
            Ok(start)
        }
        None => match default_start(dir) {
            Some(start) if resolve_commit(dir, &start).is_some() => Ok(start),
            Some(start) => {
                eprintln!("arc-rs {NAME}: Unable to resolve startpoint \"{start}\".");
                Err(ExitCode::FAILURE)
            }
            None => {
                eprintln!(
                    "arc-rs {NAME}: Unable to determine a startpoint; pass --start <commit>."
                );
                Err(ExitCode::FAILURE)
            }
        },
    }
}

fn create_branch(dir: &Path, symbol: &str, start: &str) -> ExitCode {
    println!("NEW BRANCH Creating new branch \"{symbol}\" from \"{start}\".");
    let mut args: Vec<&str> = vec!["checkout"];
    if start_is_branch(dir, start) {
        args.push("--track");
    }
    args.extend(["-b", symbol, start, "--"]);
    match git::stdout_text(dir, &args) {
        Ok(_) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("arc-rs {NAME}: git checkout failed: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Begin or resume work in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(options) => options,
    };
    if !git::is_inside_work_tree(dir) {
        return work_outside_repo(dir);
    }
    let Options { start, symbol } = options;
    if branch_exists(dir, &symbol) {
        return checkout_existing(dir, &symbol);
    }
    if let Some(code) = reject_monogram(&symbol) {
        return code;
    }
    let start = match resolve_start(dir, start) {
        Ok(start) => start,
        Err(code) => return code,
    };
    create_branch(dir, &symbol, &start)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "work_tests.rs"]
mod work_tests;
