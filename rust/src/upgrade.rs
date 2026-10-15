//! `arc-rs upgrade` — report the running version and update status.
//!
//! PHP reference: `src/workflow/ArcanistUpgradeWorkflow.php`, which pulls
//! the tool's own checkout. Without flags this port is report-only: it
//! prints the running version, the enclosing checkout's branch/HEAD,
//! cleanliness, and ahead/behind counts versus the upstream, plus how to
//! update manually. `--apply` performs PHP's upgrade (`git pull --rebase`)
//! on the enclosing checkout of the current directory.
//!
//! Status lines render through the log engine (`writeStatus` blue,
//! `writeSuccess` green); the uncommitted-changes block wraps its
//! paragraphs like `PhutilConsoleBlock` and escapes listed paths.
//!
//! Deviations: PHP upgrades the interpreter's library directory while
//! `--apply` upgrades the enclosing checkout of the current directory
//! (a compiled binary has no library directory); a rebuild reminder
//! follows a successful apply (PHP needs none).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::git;
use arc_rs::terminal::{self, PhtArg, TVal};

/// Subcommand name, matching `arc upgrade`.
pub const NAME: &str = "upgrade";

const USAGE: &str = "usage: arc-rs upgrade [--check] [--json] [--apply] [--yes]\n\
\n\
Report the running arc-rs version and whether the enclosing git checkout\n\
is behind its upstream. With --apply, upgrade the enclosing checkout\n\
with `git pull --rebase` instead of reporting (report flags are ignored).\n\
\n\
Options:\n\
  --check         Exit non-zero when the checkout is behind its upstream.\n\
  --json          Emit the report as JSON.\n\
  --apply         Upgrade the enclosing checkout (must be clean, on\n\
                  master or stable).\n\
  --yes           Accepted for scripting; upgrade never prompts.\n\
  -h, --help      Show this help.\n";

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

/// Library name `--apply` upgrades, mirroring PHP's `$library`.
pub const APPLY_LIBRARY: &str = "arc-rs";

/// Branches eligible for automatic upgrades.
pub const APPLY_BRANCHES: &[&str] = &["master", "stable"];

/// Target of an `--apply` upgrade: the enclosing checkout and its branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyTarget {
    pub root: PathBuf,
    pub branch: String,
}

/// One log line on stderr, through the log engine (`writeStatus`
/// blue, `writeSuccess` green).
fn push_status(color: &str, label: &str, message: &str) {
    eprint!(
        "{}",
        terminal::format_log(color, label, message, terminal::ansi_enabled())
    );
}

/// Collect paths from one `status --porcelain=2 -z` entry. Rename entries
/// (`2`) contribute the new path here; the old path arrives as the next
/// NUL-separated field and is collected by [`uncommitted_paths`].
fn porcelain_entry_paths(entry: &str) -> Option<&str> {
    let kind = entry.split(' ').next()?;
    match kind {
        // Limited splits keep paths with spaces intact, like PHP's
        // `explode(' ', $entry, N)`.
        "1" => entry.splitn(9, ' ').nth(8),
        "2" => entry.splitn(10, ' ').nth(9),
        "u" => entry.splitn(11, ' ').nth(10),
        "?" => entry.split_once(' ').map(|(_, rest)| rest),
        _ => None,
    }
}

/// Sorted uncommitted paths, mirroring `getUncommittedStatus()` keys
/// (`status --porcelain=2 -z`, renames contribute both sides).
pub fn uncommitted_paths(dir: &Path) -> Result<Vec<String>, String> {
    let out = git::output(dir, &["status", "--porcelain=2", "-z"])
        .map_err(|e| format!("failed to read working copy status: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.split('\0');
    let mut paths: Vec<String> = Vec::new();
    while let Some(entry) = fields.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(path) = porcelain_entry_paths(entry) {
            paths.push(path.to_string());
        }
        if entry.starts_with("2 ")
            && let Some(orig) = fields.next()
            && !orig.is_empty()
        {
            paths.push(orig.to_string());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// One block paragraph: `tsprintf('%s', $item)->applyWrap()`
/// (escape, then wrap at indent 0).
fn wrap_paragraph(message: &str, ansi: bool) -> String {
    let text = terminal::tsprintf_ansi("%s", &[TVal::Text(message)], ansi)
        .expect("fixed tsprintf pattern");
    terminal::console_wrap(text.as_str(), 0, true)
}

/// Render the uncommitted-changes console block (stdout text, mirroring
/// `PhutilConsoleBlock` with an unwrapped `PhutilConsoleList`): wrapped
/// `pht` paragraphs around `      - ` items, escaped at draw time.
#[must_use]
pub fn render_uncommitted_block(root: &Path, paths: &[String], ansi: bool) -> String {
    let root_text = root.display().to_string();
    let first = terminal::pht(
        "You have uncommitted changes in the working copy (\"%s\") for this library (\"%s\"):",
        &[PhtArg::Text(&root_text), PhtArg::Text(APPLY_LIBRARY)],
    );
    let mut out = format!("{}\n\n", wrap_paragraph(&first, ansi));
    for path in paths {
        out.push_str(&format!("      - {}\n", terminal::escape_value(path, true)));
    }
    let last = terminal::pht("Discard these changes before running \"arc upgrade\".", &[]);
    out.push_str(&format!("\n{}\n\n", wrap_paragraph(&last, ansi)));
    out
}

/// Resolve the `--apply` target: `dir` must be a git working copy on
/// `master` or `stable`.
pub fn resolve_apply_target(dir: &Path) -> Result<ApplyTarget, String> {
    if !git::is_inside_work_tree(dir) {
        return Err(format!(
            "The \"upgrade\" workflow uses \"git pull\" to upgrade, but the software directory \
             (in \"{}\") is not a Git working copy. You must leave this directory as a Git \
             working copy to use \"arc upgrade\".",
            dir.display()
        ));
    }
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("failed to find the checkout root: {e}"))?;
    let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
    match branch {
        Some(name) if APPLY_BRANCHES.contains(&name.as_str()) => Ok(ApplyTarget {
            root: PathBuf::from(root),
            branch: name,
        }),
        _ => Err(format!(
            "Library \"{APPLY_LIBRARY}\" (in \"{root}\") is on branch \"{}\", but this branch is \
             not supported for automatic upgrades. Supported branches are: {}.",
            branch.as_deref().unwrap_or("HEAD (detached)"),
            APPLY_BRANCHES.join(", ")
        )),
    }
}

/// Reminder printed after a successful `--apply`: the running binary
/// predates the upgraded checkout, so it must be rebuilt.
#[must_use]
pub fn rebuild_note() -> &'static str {
    "Rebuild arc-rs from the upgraded checkout before using it (e.g. `cargo build` in the checkout root)."
}

/// Run `git pull --rebase origin -- <branch>` in `root`, returning stdout
/// on success and `(stdout, stderr)` on failure.
fn pull_rebase(root: &Path, branch: &str) -> Result<String, (String, String)> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["pull", "--rebase", "origin", "--", branch])
        .output()
        .map_err(|e| (String::new(), format!("failed to run git pull: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err((stdout, stderr))
    }
}

/// Best-effort `git rebase --abort` after a failed pull, mirroring PHP.
fn abort_rebase(root: &Path) {
    std::process::Command::new("git")
        .current_dir(root)
        .args(["rebase", "--abort"])
        .output()
        .ok();
}

/// Perform the `--apply` upgrade: cleanliness gate, pull, abort on
/// failure. Status lines go to stderr; pull output passes through stdout.
pub fn run_apply(target: &ApplyTarget) -> Result<(), String> {
    let dirty = uncommitted_paths(&target.root)?;
    if !dirty.is_empty() {
        print!(
            "{}",
            render_uncommitted_block(&target.root, &dirty, terminal::ansi_enabled())
        );
        return Err("\"arc upgrade\" can only upgrade clean working copies.".to_string());
    }
    push_status(
        "blue",
        &terminal::pht("UPGRADING", &[]),
        &terminal::pht(
            "Upgrading \"%s\" (on branch \"%s\").",
            &[PhtArg::Text(APPLY_LIBRARY), PhtArg::Text(&target.branch)],
        ),
    );
    match pull_rebase(&target.root, &target.branch) {
        Ok(stdout) => {
            print!("{stdout}");
            push_status(
                "green",
                &terminal::pht("UPGRADED", &[]),
                &terminal::pht("This software is now up to date.", &[]),
            );
            eprintln!("{}", rebuild_note());
            Ok(())
        }
        Err((stdout, stderr)) => {
            print!("{stdout}");
            eprint!("{stderr}");
            abort_rebase(&target.root);
            let detail = stderr.trim();
            if detail.is_empty() {
                Err("git pull --rebase failed.".to_string())
            } else {
                Err(detail.to_string())
            }
        }
    }
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
    apply: bool,
}

enum Parsed {
    Help,
    Options(Options),
    Error(String),
}

fn parse_args(argv: &[String]) -> Parsed {
    let mut check = false;
    let mut json = false;
    let mut apply = false;
    for arg in argv {
        if arg == "--check" {
            check = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--apply" {
            apply = true;
        } else if arg == "--yes" {
            // Accepted for scripting; upgrade never prompts.
        } else if arg == "-h" || arg == "--help" {
            return Parsed::Help;
        } else if arg.starts_with('-') {
            return Parsed::Error(format!("unknown flag '{arg}'"));
        } else {
            return Parsed::Error(format!("unexpected argument '{arg}'"));
        }
    }
    Parsed::Options(Options { check, json, apply })
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Parsed::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Parsed::Options(options) => options,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if options.apply {
        push_status(
            "blue",
            &terminal::pht("PREPARING", &[]),
            &terminal::pht(
                "Preparing to upgrade \"%s\"...",
                &[PhtArg::Text(APPLY_LIBRARY)],
            ),
        );
        let target = match resolve_apply_target(&cwd) {
            Ok(target) => target,
            Err(message) => {
                eprintln!("arc-rs {NAME}: {message}");
                return ExitCode::FAILURE;
            }
        };
        return match run_apply(&target) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("arc-rs {NAME}: {message}");
                ExitCode::FAILURE
            }
        };
    }
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
