//! `arc-rs upgrade` — report the running version and update status.
//!
//! PHP reference: `src/workflow/ArcanistUpgradeWorkflow.php`, which pulls
//! the tool's own checkout. This port is deliberately report-only: it
//! prints the running version, the enclosing checkout's branch/HEAD,
//! cleanliness, and ahead/behind counts versus the upstream, plus how to
//! update manually. It never fetches, pulls, or replaces the binary
//! (see T1128).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::git;

/// Subcommand name, matching `arc upgrade`.
pub const NAME: &str = "upgrade";

const USAGE: &str = "usage: arc-rs upgrade [--check] [--json]\n\
\n\
Report the running arc-rs version and whether the enclosing git checkout\n\
is behind its upstream. arc-rs never updates itself; use the printed\n\
instructions to update manually.\n\
\n\
Options:\n\
  --check         Exit non-zero when the checkout is behind its upstream.\n\
  --json          Emit the report as JSON.\n\
  -h, --help      Show this help.\n\
\n\
Not yet ported: automatic self-update (--apply); run the printed commands.\n";

/// The running version, from `Cargo.toml`.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Update status of one directory.
pub struct Status {
    /// Running `arc-rs` version.
    pub version: String,
    /// Whether `dir` is inside a git working tree.
    pub in_repo: bool,
    /// Repository top level, when in a repo.
    pub root: Option<String>,
    /// Current branch short name, or `None` when detached.
    pub branch: Option<String>,
    /// HEAD short SHA, or `None` when there are no commits.
    pub head: Option<String>,
    /// Whether the working copy has no changes.
    pub clean: Option<bool>,
    /// Number of `git status --porcelain` lines.
    pub changed_files: Option<usize>,
    /// Upstream short name (e.g. `origin/main`), if configured.
    pub upstream: Option<String>,
    /// Commits ahead of the upstream.
    pub ahead: Option<u64>,
    /// Commits behind the upstream.
    pub behind: Option<u64>,
}

/// Inspect `dir` using only local git state (no fetch, no network).
pub fn inspect(dir: &Path) -> Result<Status, String> {
    let version = version().to_string();
    if !git::is_inside_work_tree(dir) {
        return Ok(Status {
            version,
            in_repo: false,
            root: None,
            branch: None,
            head: None,
            clean: None,
            changed_files: None,
            upstream: None,
            ahead: None,
            behind: None,
        });
    }
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])?;
    let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
    let head = git::stdout_text(dir, &["rev-parse", "--short", "HEAD"]).ok();
    let porcelain = git::stdout_text(dir, &["status", "--porcelain"])?;
    let changed_files = if porcelain.is_empty() {
        0
    } else {
        porcelain.lines().count()
    };
    let clean = changed_files == 0;
    let (upstream, ahead, behind) = if git::ok(dir, &["rev-parse", "--verify", "@{upstream}"]) {
        let name = git::stdout_text(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok();
        let (ahead, behind) = git::stdout_text(
            dir,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .ok()
        .and_then(|text| parse_ahead_behind(&text))
        .unzip();
        (name, ahead, behind)
    } else {
        (None, None, None)
    };
    Ok(Status {
        version,
        in_repo: true,
        root: Some(root),
        branch,
        head,
        clean: Some(clean),
        changed_files: Some(changed_files),
        upstream,
        ahead,
        behind,
    })
}

fn parse_ahead_behind(text: &str) -> Option<(u64, u64)> {
    let mut parts = text.split_whitespace();
    let ahead = parts.next()?.parse().ok()?;
    let behind = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

/// Whether an update is available: behind > 0. `None` when unknown (not
/// in a repo, or no upstream).
#[must_use]
pub fn update_available(status: &Status) -> Option<bool> {
    status.behind.map(|behind| behind > 0)
}

/// How to update arc-rs manually.
#[must_use]
pub fn how_to_update() -> &'static str {
    "To update: run `git pull --rebase` in the arc-rs checkout, then `cargo build` (or `cargo install --path rust`). arc-rs never updates itself."
}

/// Render the human-readable report.
#[must_use]
pub fn render(status: &Status) -> String {
    let mut out = format!("arc-rs {}\n\n", status.version);
    if !status.in_repo {
        out.push_str("Not inside a git checkout; newest available is unknown.\n\n");
        out.push_str(how_to_update());
        out.push('\n');
        return out;
    }
    if let Some(root) = &status.root {
        out.push_str(&format!("Repository: {root}\n"));
    }
    out.push_str(&format!(
        "Branch: {}\n",
        status.branch.as_deref().unwrap_or("HEAD (detached)")
    ));
    out.push_str(&format!(
        "HEAD: {}\n",
        status.head.as_deref().unwrap_or("none (no commits)")
    ));
    match (status.clean, status.changed_files) {
        (Some(true), _) => out.push_str("Working copy: clean\n"),
        (Some(false), Some(n)) => {
            out.push_str(&format!("Working copy: dirty ({n} changed files)\n"))
        }
        _ => out.push_str("Working copy: unknown\n"),
    }
    match (&status.upstream, status.ahead, status.behind) {
        (Some(name), Some(ahead), Some(behind)) => {
            out.push_str(&format!(
                "Upstream: {name} (ahead {ahead}, behind {behind})\n"
            ));
        }
        (Some(name), _, _) => out.push_str(&format!("Upstream: {name} (counts unknown)\n")),
        (None, _, _) => out.push_str("Upstream: none configured\n"),
    }
    match update_available(status) {
        Some(true) => out.push_str("Status: update available\n"),
        Some(false) => out.push_str("Status: up to date\n"),
        None => out.push_str("Status: newest available unknown\n"),
    }
    out.push('\n');
    out.push_str(how_to_update());
    out.push('\n');
    out
}

/// Render the machine-readable report.
#[must_use]
pub fn report_json(status: &Status) -> serde_json::Value {
    serde_json::json!({
        "version": status.version,
        "in_repo": status.in_repo,
        "root": status.root,
        "branch": status.branch,
        "head": status.head,
        "clean": status.clean,
        "changed_files": status.changed_files,
        "upstream": status.upstream,
        "ahead": status.ahead,
        "behind": status.behind,
        "update_available": update_available(status),
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    check: bool,
    json: bool,
}

enum Parsed {
    Help,
    Options(Options),
    NotYetPorted(String),
    Error(String),
}

fn parse_args(argv: &[String]) -> Parsed {
    let mut check = false;
    let mut json = false;
    for arg in argv {
        if arg == "--check" {
            check = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--apply" || arg == "--yes" {
            return Parsed::NotYetPorted(format!(
                "arc-rs {NAME}: {arg} is not yet ported (automatic self-update; see T1128)"
            ));
        } else if arg == "-h" || arg == "--help" {
            return Parsed::Help;
        } else if arg.starts_with('-') {
            return Parsed::Error(format!("unknown flag '{arg}'"));
        } else {
            return Parsed::Error(format!("unexpected argument '{arg}'"));
        }
    }
    Parsed::Options(Options { check, json })
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Parsed::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::NotYetPorted(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
        Parsed::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Parsed::Options(options) => options,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let status = match inspect(&cwd) {
        Ok(status) => status,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&status))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        print!("{}", render(&status));
    }
    if options.check && update_available(&status) == Some(true) {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod upgrade_tests;
