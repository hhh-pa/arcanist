//! Child-process constructor that propagates the classic `ARCANIST` export.
//!
//! PHP exports the command process-wide via `putenv` (the raw lowercased
//! argv word, `scripts/arcanist.php:229` at ceb), so every child process —
//! git hooks, linters, browsers — inherits `ARCANIST`. The port records
//! the name in a [`crate::globals`] cell instead (`std::env::set_var` is
//! `unsafe` in edition 2024 while shipped code keeps zero `unsafe`), so
//! every spawn site builds its [`std::process::Command`] here to re-attach
//! the variable explicitly. Only `ARCANIST` rides along: PHP children
//! never saw the `ARC_*` globals either, and the parent environment is
//! otherwise inherited unchanged.
//!
//! Spawn sites also run their commands here: [`spawn_output`] retries
//! briefly on ETXTBSY, so backends share one transient-spawn policy
//! instead of each duplicating it.

use std::ffi::OsStr;
use std::process::Command;

/// Build a child-process command for `program`, propagating the classic
/// `ARCANIST` export when the command cell is set (T1153 CORE-19).
///
/// Mirrors [`Command::new`]: the argument type, the inherited parent
/// environment, and the returned builder are identical — the only
/// addition is `.env("ARCANIST", <cell>)` when
/// [`crate::globals::command_name`] is `Some`.
#[must_use]
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    if let Some(name) = crate::globals::command_name() {
        cmd.env("ARCANIST", name);
    }
    cmd
}

/// Run `cmd` to completion, retrying briefly on ETXTBSY ("Text file
/// busy"). A freshly written executable can still be settling when it
/// is spawned (parallel tests, an editor saving a hook); the error is
/// transient by nature, so retry bounded, then return the last error
/// unchanged.
pub(crate) fn spawn_output(cmd: &mut Command) -> Result<std::process::Output, std::io::Error> {
    spawn_output_with_limit(cmd, 50)
}

/// [`spawn_output`] with an injectable retry bound, so tests can exhaust
/// the retries quickly instead of sleeping through all fifty.
pub(crate) fn spawn_output_with_limit(
    cmd: &mut Command,
    max_attempts: u32,
) -> Result<std::process::Output, std::io::Error> {
    spawn_output_with(cmd, &is_text_file_busy, max_attempts)
}

/// Retry loop with an injectable busy predicate, so tests can force the
/// ETXTBSY path where the platform never reports it (macOS, Windows).
/// Test-only: production goes through [`spawn_output`].
#[cfg(test)]
pub(crate) fn spawn_output_retry(
    cmd: &mut Command,
    is_busy: &dyn Fn(&std::io::Error) -> bool,
) -> Result<std::process::Output, std::io::Error> {
    spawn_output_with(cmd, is_busy, 50)
}

fn spawn_output_with(
    cmd: &mut Command,
    is_busy: &dyn Fn(&std::io::Error) -> bool,
    max_attempts: u32,
) -> Result<std::process::Output, std::io::Error> {
    let mut attempt = 0;
    loop {
        match cmd.output() {
            Err(error) if is_busy(&error) && attempt < max_attempts => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

/// True for ETXTBSY (errno 26 on Linux and the BSDs, including macOS).
/// Windows has no such spawn error.
#[cfg(unix)]
pub(crate) fn is_text_file_busy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

/// True for ETXTBSY (errno 26 on Linux and the BSDs, including macOS).
/// Windows has no such spawn error.
#[cfg(not(unix))]
pub(crate) fn is_text_file_busy(_error: &std::io::Error) -> bool {
    false
}

#[cfg(test)]
#[path = "proc_env_tests.rs"]
mod proc_env_tests;
