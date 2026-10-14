//! `arc-rs anoid`: pilot a probe from the vessel "Arcanoid".
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

fn usage() -> String {
    "usage: arc-rs anoid\n\
     \n\
     Pilot a probe from the vessel \"Arcanoid\".\n\
     \n\
     Take control of a probe launched from the science vessel \"Arcanoid\".\n\
     Requires \"python3\" to be available in your $PATH and\n\
     support/arcanoid/arcanoid.py to exist at or above the working copy.\n\
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
        return Err("unexpected argument '--': anoid takes no arguments".to_string());
    }
    if first.starts_with('-') {
        return Err(format!("unknown flag '{first}'"));
    }
    Err(format!("unexpected argument '{first}'"))
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
/// `phutil_passthru`, and map its status onto an [`ExitCode`].
pub fn passthru(program: &Path, args: &[&str], dir: &Path) -> Result<ExitCode, String> {
    let status = std::process::Command::new(program)
        .current_dir(dir)
        .args(args)
        .status()
        .map_err(|e| format!("failed to launch '{}': {e}", program.display()))?;
    if status.success() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// Pilot the probe from `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match parse_args(argv) {
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        Ok(Some(())) => {}
    }
    let Some(python) = python3_on_path() else {
        eprintln!(
            "arc-rs {NAME}: The \"arc-rs anoid\" workflow requires Python 3 on your $PATH (python3, or python on Windows)."
        );
        return ExitCode::FAILURE;
    };
    let Some(script) = find_script(dir) else {
        eprintln!(
            "arc-rs {NAME}: cannot find support/arcanoid/arcanoid.py above '{}'",
            dir.display()
        );
        return ExitCode::FAILURE;
    };
    let script_arg = script.to_string_lossy().to_string();
    match passthru(&python, &[script_arg.as_str()], dir) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
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
