//! `nolint` linter: `@nolint` files opt out of all other linters.
//!
//! PHP reference: `src/lint/linter/ArcanistNoLintLinter.php` (`NOLINT`).
//!
//! PHP raises no messages; when file data contains `@nolint` it calls
//! `stopAllLinters()`. The arc-rs engine has no stop-all mechanism yet, so
//! this linter is silent and exposes [`is_nolint`] for the engine (or parent
//! wiring) to consult.
//!
//! Known limit: until the engine honors it, `@nolint` files are still
//! linted by every other linter.

extern crate self as arc_rs;

use arc_rs::lint::Linter;

/// Marker linter disabling lint per file (PHP `NOLINT`). Always silent; see
/// [`is_nolint`].
#[derive(Debug, Default)]
pub struct NoLintLinter;

impl NoLintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// True when `data` contains `@nolint` (byte substring, mirroring the PHP
/// `/@nolint/` match over the raw file data).
#[must_use]
pub fn is_nolint(data: &[u8]) -> bool {
    contains_bytes(data, b"@nolint")
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

impl Linter for NoLintLinter {
    fn name(&self) -> &'static str {
        "NOLINT"
    }

    fn can_customize_severities(&self) -> bool {
        false
    }

    fn priority(&self) -> f64 {
        0.25
    }

    fn lint_bytes(&self, _path: &str, _data: &[u8]) -> Vec<arc_rs::lint::LintMessage> {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "lint_nolint_tests.rs"]
mod lint_nolint_tests;
