//! `arc anoid`: pilot a probe from the vessel "Arcanoid".
//!
//! PHP reference: `src/workflow/ArcanistAnoidWorkflow.php`: requires
//! `python3` on `$PATH`, then passes control to
//! `support/arcanoid/arcanoid.py`.
//!
//! Scope: the Rust port locates `support/arcanoid/arcanoid.py` by walking up
//! from the working copy (the checkout root carries `support/`) and runs
//! `python3 <script>`, inheriting stdio like `phutil_passthru` and mapping
//! its status onto the process exit code.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Subcommand name, matching `arc anoid`.
pub const NAME: &str = "anoid";

/// T1153 BROWSE-B23: `--help` renders the generated workflow help
/// (`ArcanistAnoidWorkflow.php:11-23`), not a short custom usage
/// string. T1153 RB-D5: the renderer is the shared `crate::help`
/// module; this file keeps only the data.
const HELP_EXAMPLES: &str = "**anoid**";
const HELP_SYNOPSIS: &str = "Pilot a probe from the vessel \"Arcanoid\".";
const HELP_TEXT: &str = "Take control of a probe launched from the science vessel \"Arcanoid\".";
/// Non-wildcard argument specs: (name, short, param, help). Empty:
/// `getWorkflowArguments()` returns no arguments (`:25-27`).
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
/// PHP's `parseFull()`, but before the empty-args success). A lone
/// `--` is the terminator (accepted); `--help` after `--` is a
/// positional (rejected, like PHP).
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

/// Locate `binary` on a `$PATH`-style search string.
#[must_use]
pub fn find_on_path(path_env: &std::ffi::OsStr, binary: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Candidate interpreter names: stock Windows installs call it `python`.
fn interpreter_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        &["python3", "python"]
    } else {
        &["python3"]
    }
}

/// Locate a Python 3 interpreter on the real `$PATH`.
#[must_use]
pub fn python3_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    interpreter_candidates()
        .iter()
        .find_map(|binary| find_on_path(&path, binary))
}

/// Locate `support/arcanoid/arcanoid.py` at or above `start`.
///
/// PHP execs its install-relative script directly via the shebang
/// (`ArcanistAnoidWorkflow.php:34-40` at ceb); a compiled binary has
/// no library root to resolve, so the port walks up instead and runs
/// the script through the gated `python3` — the script's own
/// `#!/usr/bin/env python3` shebang — which is equivalent on Unix
/// and the only viable spelling on Windows (T1153 CORE-58, kept).
/// A missing script is a clean error here instead of PHP's shell
/// failure.
#[must_use]
pub fn find_script(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join("support").join("arcanoid").join("arcanoid.py");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Run `program` with `args` in `dir`, inheriting stdio like
/// `phutil_passthru`, and return its status verbatim
/// (`PhutilExecPassthru::execute()` returns the subprocess error code,
/// `src/future/exec/PhutilExecPassthru.php:34-36,103` at ceb, T1153
/// T1153 CORE-57). A signaled child (no code, Unix-only) maps to 1.
pub fn passthru(program: &Path, args: &[&str], dir: &Path) -> Result<ExitCode, String> {
    let status = std::process::Command::new(program)
        .current_dir(dir)
        .args(args)
        .status()
        .map_err(|e| format!("failed to launch '{}': {e}", program.display()))?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}

/// Pilot the probe from `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
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
            // T1153 CORE-59: toolset argument errors render the red
            // `USAGE EXCEPTION` label with no usage dump (exit 1).
            return crate::conduit::report(crate::conduit::EnvelopeKind::ToolsetUsage, &message);
        }
        Ok(Some(())) => {}
    }
    let Some(python) = python3_on_path() else {
        // T1153 CORE-59: PHP's verbatim text
        // (`ArcanistAnoidWorkflow.php:28-32` at ceb), thrown as a
        // `PhutilArgumentUsageException` (the `USAGE EXCEPTION` arm).
        // The Windows `python` fallback stays additive and unnamed.
        return report(
            EnvelopeKind::ToolsetUsage,
            "The \"arc anoid\" workflow requires \"python3\" to be available in your $PATH.",
        );
    };
    let Some(script) = find_script(dir) else {
        eprintln!(
            "arc {NAME}: cannot find support/arcanoid/arcanoid.py above '{}'",
            dir.display()
        );
        return ExitCode::FAILURE;
    };
    let script_arg = script.to_string_lossy().to_string();
    match passthru(&python, &[script_arg.as_str()], dir) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "anoid_tests.rs"]
mod anoid_tests;
