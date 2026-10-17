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

#[cfg(test)]
#[path = "proc_env_tests.rs"]
mod proc_env_tests;
