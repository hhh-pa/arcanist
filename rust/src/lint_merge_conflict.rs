//! `merge-conflict` linter: flag unresolved merge conflict markers.
//!
//! PHP reference: `src/lint/linter/ArcanistMergeConflictLinter.php`
//! (`MERGECONFLICT`).
//!
//! Rule: any line that is exactly `<<<<<<<`, `=======`, or `>>>>>>>`
//! (seven identical characters, nothing else) indicates an unresolved
//! conflict. Applies to every file; binary (NUL) data is skipped, mirroring
//! the PHP engine's treatment of linters with
//! `shouldLintBinaryFiles() === false`.
//!
//! Divergence: a single trailing `\r` is stripped before comparing, so
//! CRLF files are handled the same as LF files.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};

const CODE_MERGECONFLICT: &str = "MERGECONFLICT1";
const NAME_MERGECONFLICT: &str = "Unresolved merge conflict";
const DESC_MERGECONFLICT: &str = "This syntax indicates there is an unresolved merge conflict.";

/// Linter flagging unresolved merge conflict markers (PHP `MERGECONFLICT`).
#[derive(Debug, Default)]
pub struct MergeConflictLinter;

impl MergeConflictLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// True when `line` (without its line ending) is exactly one of the three
/// seven-character conflict markers.
#[must_use]
pub fn is_marker(line: &[u8]) -> bool {
    line == b"<<<<<<<" || line == b"=======" || line == b">>>>>>>"
}

impl Linter for MergeConflictLinter {
    fn name(&self) -> &'static str {
        "MERGECONFLICT"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if data.contains(&0) || is_skippable_filesystem_object(path) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (index, line) in data.split(|byte| *byte == b'\n').enumerate() {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if is_marker(line) {
                out.push(LintMessage {
                    path: path.to_string(),
                    line: index + 1,
                    col: 1,
                    code: CODE_MERGECONFLICT.to_string(),
                    severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
                    name: NAME_MERGECONFLICT.to_string(),
                    description: DESC_MERGECONFLICT.to_string(),
                    original: None,
                    replacement: None,
                });
            }
        }
        out
    }
}

/// True only when `path` positively resolves to a directory or symlink (the
/// PHP linter never sees those). Missing paths return false so content
/// checks still run for display-only paths.
fn is_skippable_filesystem_object(path: &str) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.file_type().is_symlink() || meta.file_type().is_dir(),
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "lint_merge_conflict_tests.rs"]
mod lint_merge_conflict_tests;
