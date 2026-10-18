//! `arc-rs version` — display the current version of Arcanist.
//!
//! Port of `src/toolset/workflow/ArcanistVersionWorkflow.php` (ceb):
//! no workflow arguments (`getWorkflowArguments()` returns empty), the
//! `git` binary precondition (`:39-44`), the single `arcanist` library
//! root resolved to its git working copy (`:46-63`), and the
//! `"%s %s (%s)\n"` render of the HEAD commit hash and date (`:65-76`).
//!
//! Divergence: PHP resolves the library root from the running source
//! tree (`phutil_get_library_root('arcanist')`, `:47`); the compiled
//! binary has no source tree, so the checkout is resolved from the
//! running executable's directory instead (`run_in` takes it
//! explicitly, so tests use fixture checkouts), and installed
//! binaries outside any checkout report the not-a-git-copy error
//! (T1153 CORE-60, kept: no library root exists to resolve). The
//! git-missing and not-a-git-working-copy errors keep the PHP text.
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

/// T1153 BROWSE-B23: `--help` renders the generated workflow help
/// (`ArcanistVersionWorkflow.php:16-29`), not a short custom usage
/// string. T1153 RB-D5: the renderer is the shared `crate::help`
/// module; this file keeps only the data.
const HELP_EXAMPLES: &str = "**version**";
const HELP_SYNOPSIS: &str = "Show toolset version information.";
const HELP_TEXT: &str = "Shows the current version.";
/// Non-wildcard argument specs: (name, short, param, help). Empty:
/// `getWorkflowArguments()` returns no arguments (`:30-32`).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[];

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

/// Parse `argv` (excluding the subcommand name); `Ok(None)` requests help.
pub fn parse_args(argv: &[String]) -> Result<Option<()>, String> {
    parse_args_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_args`] with an injected TTY flag (deterministic tests).
/// T1153 BROWSE-B21: piped runs without `--` fail with
/// `MISSING_TERMINATOR` (checked after unknown-flag scanning, like
/// PHP's `parseFull()`). A lone `--` is the terminator (accepted).
fn parse_args_with_tty(argv: &[String], stdin_tty: bool) -> Result<Option<()>, String> {
    let missing = crate::help::terminator_missing(argv, stdin_tty);
    let mut terminated = false;
    for arg in argv {
        if !terminated && arg == "--" {
            terminated = true;
            continue;
        }
        if !terminated && (arg == "-h" || arg == "--help") {
            if missing {
                return Err(crate::help::MISSING_TERMINATOR.to_string());
            }
            return Ok(None);
        }
        if !terminated && arg.starts_with('-') && arg.len() > 1 {
            return Err(format!("unknown flag '{arg}'"));
        }
        return Err(format!("unexpected argument '{arg}'"));
    }
    if missing {
        return Err(crate::help::MISSING_TERMINATOR.to_string());
    }
    Ok(Some(()))
}

/// True when a `git` binary is on the real `$PATH`
/// (PHP `Filesystem::binaryExists('git')`, `:34`).
#[must_use]
pub fn git_binary_available() -> bool {
    std::env::var_os("PATH").is_some_and(|path| anoid::find_on_path(&path, "git").is_some())
}

/// PHP `:39-44` at ceb: `Cannot display current version without "git" installed.`
#[must_use]
pub fn missing_git_message() -> String {
    terminal::pht(
        "Cannot display current version without \"%s\" installed.",
        &[PhtArg::Text("git")],
    )
}

/// `Filesystem::readablePath()` (`src/filesystem/Filesystem.php:1019-1034`
/// at ceb): strip the CWD prefix (then the resolved-CWD prefix) when
/// `path` sits under it, else return `path` unchanged (T1153 CORE-61).
fn readable_path(cwd: &Path, path: &Path) -> String {
    let text = path.display().to_string();
    let mut parents = vec![cwd.to_path_buf()];
    if let Ok(resolved) = cwd.canonicalize()
        && resolved != cwd
    {
        parents.push(resolved);
    }
    for parent in &parents {
        let base = parent.display().to_string();
        // Try both separators: paths may use `/` where Windows
        // reports `\` (T1153 CORE-61).
        for sep in [std::path::MAIN_SEPARATOR, '/'] {
            let trimmed = base.trim_end_matches(sep);
            let prefix = format!("{trimmed}{sep}");
            if let Some(rest) = text.strip_prefix(&prefix) {
                return rest.to_string();
            }
        }
    }
    text
}

/// PHP `:56-62` at ceb: the not-a-git-working-copy error for `tool_dir`,
/// with the path relativized like `Filesystem::readablePath()`
/// (T1153 CORE-61).
#[must_use]
pub fn not_git_message(cwd: &Path, tool_dir: &Path) -> String {
    let dir = readable_path(cwd, tool_dir);
    terminal::pht(
        "Library \"%s\" (at \"%s\") is not a Git working copy, so no version information can be provided.",
        &[PhtArg::Text(LIBRARY), PhtArg::Text(&dir)],
    )
}

/// Resolve the `arcanist` library checkout containing `tool_dir` (PHP's
/// `$roots` entry, `:46-48` at ceb): the enclosing git working copy, or the PHP
/// not-a-git-working-copy error.
pub fn library_root(cwd: &Path, tool_dir: &Path) -> Result<PathBuf, String> {
    if !git::is_inside_work_tree(tool_dir) {
        return Err(not_git_message(cwd, tool_dir));
    }
    let root = git::stdout_text(tool_dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(root))
}

/// Split PHP's `log -1 --format=%ct%x01%H` output into
/// (timestamp, commit) (PHP `:65-70` at ceb: trim, then `explode("\1")`).
pub fn split_log_line(line: &str) -> Result<(i64, String), String> {
    let trimmed = line.trim();
    let Some((timestamp, commit)) = trimmed.split_once('\x01') else {
        return Err(format!("unexpected git log output: {trimmed:?}"));
    };
    Ok((timestamp.parse::<i64>().unwrap_or(0), commit.to_string()))
}

/// HEAD commit of the checkout at `root` (PHP `:65-68` at ceb).
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

/// Render the version line (PHP `:72-76` at ceb): `arcanist <hash> (<date>)`,
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
/// `cwd` relativizes the not-a-git-copy error like
/// `Filesystem::readablePath()` (T1153 CORE-61).
fn head_of(cwd: &Path, tool_dir: &Path) -> Result<(i64, String), String> {
    if !git_binary_available() {
        return Err(missing_git_message()); // LCOV_EXCL_LINE // Reason: git is installed on ubuntu CI; the missing-git arm never runs
    }
    let root = library_root(cwd, tool_dir)?;
    head_commit(&root)
}

/// Full `version` output for the checkout containing `tool_dir`, at a
/// fixed offset; pure for tests.
pub fn version_text_in(cwd: &Path, tool_dir: &Path, offset: i64) -> Result<String, String> {
    let (timestamp, commit) = head_of(cwd, tool_dir)?;
    Ok(render_version_line_in(&commit, timestamp, offset))
}

/// Full `version` output for the checkout containing `tool_dir`, in the
/// process timezone.
pub fn version_text(cwd: &Path, tool_dir: &Path) -> Result<String, String> {
    let (timestamp, commit) = head_of(cwd, tool_dir)?;
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
    use crate::conduit::{EnvelopeKind, report};
    match parse_args(argv) {
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            // T1153 BROWSE-B21: the terminator failure prints the three
            // cyan hints first (like `ArcanistRuntime.php:172-188`).
            if message == crate::help::MISSING_TERMINATOR {
                crate::help::print_terminator_hints(crate::terminal::ansi_enabled());
            }
            // T1153 CORE-49: toolset argument errors render the red
            // `USAGE EXCEPTION` label with no usage dump (exit 1).
            return crate::conduit::report(crate::conduit::EnvelopeKind::ToolsetUsage, &message);
        }
        Ok(Some(())) => {}
    }
    if !git_binary_available() {
        // T1153 CORE-49: the missing-`git` throw is an
        // `ArcanistUsageException` (`:39-44` at ceb), which escapes
        // the toolset runtime into a PHP fatal (255).
        return report(EnvelopeKind::Fatal, &missing_git_message()); // LCOV_EXCL_LINE // Reason: git is installed on ubuntu CI; the missing-git arm never runs
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match version_text(&cwd, tool_dir) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        // T1153 CORE-49: the not-a-git-copy throw is a
        // `PhutilArgumentUsageException` (`:56-62` at ceb), caught as
        // `USAGE EXCEPTION` (exit 1).
        Err(message) => report(EnvelopeKind::ToolsetUsage, &message),
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    run_in(&tool_dir(), argv)
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod version_tests;
