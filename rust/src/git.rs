//! Shared git subprocess helpers for workflow modules.
//!
//! New code must use these instead of spawning `git` directly, so flags,
//! error shapes, and directory handling stay uniform.

use std::path::Path;

/// Run git in `dir`, returning the raw output. Spawn failures become
/// errors; non-zero exit statuses are left for the caller to interpret.
pub fn output(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// Run git in `dir`, returning trimmed stdout, or the combined trimmed
/// output when git exits non-zero.
pub fn stdout_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = output(dir, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            Err(format!("git {} failed", args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

/// True when git exits zero in `dir` for `args`.
#[must_use]
pub fn ok(dir: &Path, args: &[&str]) -> bool {
    output(dir, args).is_ok_and(|out| out.status.success())
}

/// True when `dir` is inside a git working tree.
#[must_use]
pub fn is_inside_work_tree(dir: &Path) -> bool {
    let Ok(out) = output(dir, &["rev-parse", "--is-inside-work-tree"]) else {
        return false;
    };
    out.status.success() && out.stdout.trim_ascii() == b"true"
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod git_tests;
