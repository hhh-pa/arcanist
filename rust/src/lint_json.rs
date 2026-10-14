//! `json` linter: JSON files must parse.
//!
//! PHP reference: `src/lint/linter/ArcanistJSONLinter.php` (`JSON`).
//!
//! Self-filter: only `*.json` paths (case-insensitive) are parsed; anything
//! else passes silently. Binary (NUL) data is skipped, mirroring the PHP
//! engine's treatment of linters with `shouldLintBinaryFiles() === false`.
//!
//! Divergence: PHP parses with `PhutilJSONParser`; this port uses
//! `serde_json`. Positions are 1-based (line, column); `serde_json` reports
//! column 0 for empty input, which is clamped to 1.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};

const CODE_PARSE_ERROR: &str = "JSON1";
const NAME_PARSE_ERROR: &str = "Parse Error";

/// Linter reporting JSON syntax errors (PHP `JSON`).
#[derive(Debug, Default)]
pub struct JsonLinter;

impl JsonLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// True when `path` names a `.json` file (ASCII case-insensitive).
#[must_use]
pub fn is_json_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 5 && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b".json")
}

/// Parse `data` as JSON, returning the 1-based line, column, and message of
/// the first syntax error, or `None` when the document is valid.
#[must_use]
pub fn parse_error(data: &[u8]) -> Option<(usize, usize, String)> {
    match serde_json::from_slice::<serde_json::Value>(data) {
        Ok(_) => None,
        Err(error) => {
            let line = error.line().max(1);
            let col = error.column().max(1);
            Some((line, col, short_message(&error.to_string())))
        }
    }
}

/// Strip `serde_json`'s trailing " at line L column C": positions travel in
/// dedicated fields, mirroring the PHP exception's line/char API.
fn short_message(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(index) => message[..index].to_string(),
        None => message.to_string(),
    }
}

impl Linter for JsonLinter {
    fn name(&self) -> &'static str {
        "JSON"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_json_path(path) || data.contains(&0) || is_skippable_filesystem_object(path) {
            return Vec::new();
        }
        match parse_error(data) {
            None => Vec::new(),
            Some((line, col, message)) => vec![LintMessage {
                path: path.to_string(),
                line,
                col,
                code: CODE_PARSE_ERROR.to_string(),
                severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
                name: NAME_PARSE_ERROR.to_string(),
                description: message,
            }],
        }
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
#[path = "lint_json_tests.rs"]
mod lint_json_tests;
