//! `filename` linter: file names must be boring.
//!
//! PHP reference: `src/lint/linter/ArcanistFilenameLinter.php` (`NAME`).
//!
//! Rule: the whole path must match `^[a-z0-9./\\_-]+$` (case-insensitive).
//! Unlike most linters this one also applies to binaries, directories, and
//! symlinks, so the check is path-only and never inspects file contents.
//!
//! Divergence: PHP reports path-level messages with null line/char; this
//! port reports line 1, column 1 because `LintMessage` requires `usize`
//! positions.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};

const CODE_BAD_FILENAME: &str = "NAME1";
const NAME_BAD_FILENAME: &str = "Bad Filename";
const DESC_BAD_FILENAME: &str =
    "Name files using only letters, numbers, period, hyphen and underscore.";

/// Linter enforcing boring file names (PHP `NAME`).
#[derive(Debug, Default)]
pub struct FilenameLinter;

impl FilenameLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// True when `path` matches the PHP `^[a-z0-9./\\_-]+$` rule.
#[must_use]
pub fn is_valid_filename(path: &str) -> bool {
    !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'/' | b'\\' | b'_' | b'-'))
}

impl Linter for FilenameLinter {
    fn name(&self) -> &'static str {
        "NAME"
    }

    fn severity_lookup_code<'a>(&self, full_code: &'a str) -> &'a str {
        full_code.strip_prefix("NAME").unwrap_or(full_code)
    }

    // PHP lints binary files, directories, and symlinks with this
    // linter (`ArcanistFilenameLinter.php:28-54`).
    fn should_lint_binary_files(&self) -> bool {
        true
    }

    fn should_lint_directories(&self) -> bool {
        true
    }

    fn should_lint_symbolic_links(&self) -> bool {
        true
    }

    fn lint_bytes(&self, path: &str, _data: &[u8]) -> Vec<LintMessage> {
        if is_valid_filename(path) {
            return Vec::new();
        }
        vec![LintMessage {
            path: path.to_string(),
            // Path-level (`raiseLintAtPath()`): no line or char.
            line: 0,
            col: 0,
            code: CODE_BAD_FILENAME.to_string(),
            severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
            name: NAME_BAD_FILENAME.to_string(),
            description: DESC_BAD_FILENAME.to_string(),
            original: None,
            replacement: None,
        }]
    }
}

#[cfg(test)]
#[path = "lint_filename_tests.rs"]
mod lint_filename_tests;
