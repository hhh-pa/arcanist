//! `arc-rs version` — display the current version of Arcanist.
//!
//! Port of `src/toolset/workflow/ArcanistVersionWorkflow.php` (master):
//! no workflow arguments (`getWorkflowArguments()` returns empty), the
//! `git` binary precondition (`:34-41`), the single `arcanist` library
//! root resolved to its git working copy (`:43-57`), and the
//! `"%s %s (%s)\n"` render of the HEAD commit hash and date (`:59-70`).
//!
//! Divergence: PHP resolves the library root from the running source
//! tree (`phutil_get_library_root('arcanist')`); the compiled binary has
//! no source tree, so the checkout is resolved from the running
//! executable's directory instead (`run_in` takes it explicitly, so
//! tests use fixture checkouts). The git-missing and
//! not-a-git-working-copy errors keep the PHP text.
//!
//! The date renders in the process timezone like PHP's `date()`, via
//! `look`'s std-only zone lookup (explicit `TZ`, `/etc/localtime`,
//! `/etc/timezone`, else UTC).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::terminal::PhtArg;
use crate::{anoid, git, look, terminal};

/// Subcommand name, matching `arc version`.
pub const NAME: &str = "version";

/// Library label in the version line (PHP's `$roots` key, `:44`).
pub const LIBRARY: &str = "arcanist";

fn usage() -> String {
    "usage: arc version\n\
     \n\
     Show toolset version information.\n\
     \n\
     Shows the current version.\n\
     \n\
     Options:\n\
     \x20 -h, --help    Show this help.\n"
        .to_string()
}

/// Parse `argv` (excluding the subcommand name); `Ok(None)` requests help.
pub fn parse_args(argv: &[String]) -> Result<Option<()>, String> {
    let Some(first) = argv.first() else {
        return Ok(Some(()));
    };
    if first == "-h" || first == "--help" {
        return Ok(None);
    }
    if first == "--" {
        return Err("unexpected argument '--': version takes no arguments".to_string());
    }
    if first.starts_with('-') {
        return Err(format!("unknown flag '{first}'"));
    }
    Err(format!("unexpected argument '{first}'"))
}

/// True when a `git` binary is on the real `$PATH`
/// (PHP `Filesystem::binaryExists('git')`, `:34`).
#[must_use]
pub fn git_binary_available() -> bool {
    std::env::var_os("PATH").is_some_and(|path| anoid::find_on_path(&path, "git").is_some())
}

/// PHP `:36-40`: `Cannot display current version without "git" installed.`
#[must_use]
pub fn missing_git_message() -> String {
    terminal::pht(
        "Cannot display current version without \"%s\" installed.",
        &[PhtArg::Text("git")],
    )
}

/// PHP `:52-56`: the not-a-git-working-copy error for `tool_dir`.
#[must_use]
pub fn not_git_message(tool_dir: &Path) -> String {
    let dir = tool_dir.display().to_string();
    terminal::pht(
        "Library \"%s\" (at \"%s\") is not a Git working copy, so no version information can be provided.",
        &[PhtArg::Text(LIBRARY), PhtArg::Text(&dir)],
    )
}

/// Resolve the `arcanist` library checkout containing `tool_dir` (PHP's
/// `$roots` entry, `:43-49`): the enclosing git working copy, or the PHP
/// not-a-git-working-copy error.
pub fn library_root(tool_dir: &Path) -> Result<PathBuf, String> {
    if !git::is_inside_work_tree(tool_dir) {
        return Err(not_git_message(tool_dir));
    }
    let root = git::stdout_text(tool_dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(root))
}

/// Split PHP's `log -1 --format=%ct%x01%H` output into
/// (timestamp, commit) (PHP `:59-63`: trim, then `explode("\1")`).
pub fn split_log_line(line: &str) -> Result<(i64, String), String> {
    let trimmed = line.trim();
    let Some((timestamp, commit)) = trimmed.split_once('\x01') else {
        return Err(format!("unexpected git log output: {trimmed:?}"));
    };
    Ok((timestamp.parse::<i64>().unwrap_or(0), commit.to_string()))
}

/// HEAD commit of the checkout at `root` (PHP `:59-60`).
pub fn head_commit(root: &Path) -> Result<(i64, String), String> {
    let line = git::stdout_text(root, &["log", "-1", "--format=%ct%x01%H"])?;
    split_log_line(&line)
}

/// Abbreviated English month names for PHP `date('M')`.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm;
/// same epoch math as `commitgraph`'s private helper).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097) as u32;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Live process-timezone offset for `timestamp` (UTC fallback).
fn live_offset(timestamp: i64) -> i64 {
    look::local_utc_offset(u64::try_from(timestamp).unwrap_or(0))
}

/// Format `timestamp` like PHP `date('j M Y')` (day without leading
/// zeros) at a fixed seconds-east-of-UTC `offset`; pure for tests.
#[must_use]
pub fn format_version_date_in(timestamp: i64, offset: i64) -> String {
    let shifted = timestamp + offset;
    let (year, month, day) = civil_from_days(shifted.div_euclid(86_400));
    format!("{day} {} {year:04}", MONTHS[(month - 1) as usize])
}

/// Format `timestamp` like PHP `date('j M Y')` in the process timezone.
#[must_use]
pub fn format_version_date(timestamp: i64) -> String {
    format_version_date_in(timestamp, live_offset(timestamp))
}

/// Render the version line (PHP `:65-70`): `arcanist <hash> (<date>)`,
/// at a fixed offset; pure for tests.
#[must_use]
pub fn render_version_line_in(commit: &str, timestamp: i64, offset: i64) -> String {
    format!(
        "{LIBRARY} {commit} ({})\n",
        format_version_date_in(timestamp, offset)
    )
}

/// Render the version line in the process timezone.
#[must_use]
pub fn render_version_line(commit: &str, timestamp: i64) -> String {
    render_version_line_in(commit, timestamp, live_offset(timestamp))
}

/// `(timestamp, commit)` for the checkout containing `tool_dir`.
fn head_of(tool_dir: &Path) -> Result<(i64, String), String> {
    if !git_binary_available() {
        return Err(missing_git_message());
    }
    let root = library_root(tool_dir)?;
    head_commit(&root)
}

/// Full `version` output for the checkout containing `tool_dir`, at a
/// fixed offset; pure for tests.
pub fn version_text_in(tool_dir: &Path, offset: i64) -> Result<String, String> {
    let (timestamp, commit) = head_of(tool_dir)?;
    Ok(render_version_line_in(&commit, timestamp, offset))
}

/// Full `version` output for the checkout containing `tool_dir`, in the
/// process timezone.
pub fn version_text(tool_dir: &Path) -> Result<String, String> {
    let (timestamp, commit) = head_of(tool_dir)?;
    Ok(render_version_line_in(
        &commit,
        timestamp,
        live_offset(timestamp),
    ))
}

/// Directory the library checkout is resolved from: the running
/// executable's directory (the compiled analogue of PHP's library
/// root), falling back to the working directory.
fn tool_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Display the version per `argv`, resolving the checkout from `tool_dir`.
pub fn run_in(tool_dir: &Path, argv: &[String]) -> ExitCode {
    match parse_args(argv) {
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        Ok(Some(())) => {}
    }
    match version_text(tool_dir) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    run_in(&tool_dir(), argv)
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod version_tests;
