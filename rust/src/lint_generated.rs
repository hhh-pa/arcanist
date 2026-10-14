//! `generated` linter: `@generated` files opt out of all other linters.
//!
//! PHP reference: `src/lint/linter/ArcanistGeneratedLinter.php` (`GEN`).
//!
//! PHP raises no messages; when file data contains `@generated` it calls
//! `stopAllLinters()`. The arc-rs engine has no stop-all mechanism yet, so
//! this linter is silent and exposes [`is_generated`] for the engine (or
//! parent wiring) to consult.
//!
//! Known limit: until the engine honors it, `@generated` files are still
//! linted by every other linter.

extern crate self as arc_rs;

use arc_rs::lint::Linter;

/// Marker linter for generated code (PHP `GEN`). Always silent; see
/// [`is_generated`].
#[derive(Debug, Default)]
pub struct GeneratedLinter;

impl GeneratedLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// True when `data` contains `@generated` (byte substring, mirroring the
/// PHP `/@generated/` match over the raw file data).
#[must_use]
pub fn is_generated(data: &[u8]) -> bool {
    contains_bytes(data, b"@generated")
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

impl Linter for GeneratedLinter {
    fn name(&self) -> &'static str {
        "GEN"
    }

    fn lint_bytes(&self, _path: &str, _data: &[u8]) -> Vec<arc_rs::lint::LintMessage> {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "lint_generated_tests.rs"]
mod lint_generated_tests;
