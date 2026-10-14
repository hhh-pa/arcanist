//! `spelling` linter: common misspellings (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistSpellingLinter.php` (rules,
//! messages, dictionary shape) and `resources/spelling/english.json` (the
//! default dictionary: 587 exact rules plus 5 partial rules).
//!
//! Scope: exact (whole-word, case-insensitive) and partial (substring,
//! case-insensitive, overlapping) rules from JSON dictionaries shaped like
//! the PHP one (`{"rules": {"exact": {...}, "partial": {...}}}`). The
//! default linter bundles a small curated subset of the PHP dictionary as a
//! fixture (40 exact plus all 5 partial rules); full parity with the 587-word
//! PHP dictionary needs a custom dictionary via [`SpellingLinter::new`] plus
//! [`SpellingLinter::load_dictionary_json`].
//!
//! Out of scope: autofix replacements (PHP computes `original`/`replacement`
//! via `fixLetterCase`, but [`LintMessage`] has no such fields, so they are
//! dropped); project-root-relative dictionary paths (callers pass text).

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};

/// Full code for exact-word findings (`SPELL` + `LINT_SPELLING_EXACT`).
pub const CODE_EXACT: &str = "SPELL1";
/// Full code for partial-word findings (`SPELL` + `LINT_SPELLING_PARTIAL`).
pub const CODE_PARTIAL: &str = "SPELL2";
/// Display name for both spelling codes.
pub const NAME_SPELLING: &str = "Possible Spelling Mistake";
/// Both spelling codes map to `warning` in the PHP severity map.
pub const SEVERITY_WARNING: &str = "warning";

/// Small default wordlist (curated subset of the PHP dictionary).
const DEFAULT_DICTIONARY_JSON: &str = include_str!("../tests/fixtures/lintB/spelling_default.json");

/// One rule: lowercase match key, rule word as written, correction.
#[derive(Debug, Clone)]
struct Rule {
    lower: Vec<u8>,
    word: String,
    correction: String,
}

/// Wordlist-based spelling linter.
///
/// The default instance carries the bundled fixture wordlist; use
/// [`SpellingLinter::empty`] plus [`SpellingLinter::load_dictionary_json`]
/// or [`SpellingLinter::add_exact_rule`] for custom dictionaries.
#[derive(Debug, Clone)]
pub struct SpellingLinter {
    exact: Vec<Rule>,
    partial: Vec<Rule>,
}

impl SpellingLinter {
    /// Linter with the bundled default wordlist.
    #[must_use]
    pub fn new() -> Self {
        let mut linter = Self::empty();
        // The bundled fixture is valid by construction; a broken fixture
        // must degrade to an empty wordlist rather than panic.
        linter.load_dictionary_json(DEFAULT_DICTIONARY_JSON).ok();
        linter
    }

    /// Linter with no rules.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            exact: Vec::new(),
            partial: Vec::new(),
        }
    }

    /// Linter holding exactly the rules in one dictionary document.
    ///
    /// Mirrors `loadDictionary`: `text` must decode to
    /// `{"rules": {"exact": {misspelling: correction}, "partial": {...}}}`;
    /// both sections are optional.
    pub fn from_dictionary_json(text: &str) -> Result<Self, String> {
        let mut linter = Self::empty();
        linter.load_dictionary_json(text)?;
        Ok(linter)
    }

    /// Merge one dictionary document into this linter. Later rules replace
    /// earlier rules with the identical word, mirroring PHP `array_merge`.
    pub fn load_dictionary_json(&mut self, text: &str) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| format!("invalid spelling dictionary: {error}"))?;
        let rules = value
            .get("rules")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| "invalid spelling dictionary: missing 'rules' object".to_string())?;
        for (section, target) in [("exact", &mut self.exact), ("partial", &mut self.partial)] {
            let Some(entries) = rules.get(section) else {
                continue;
            };
            let entries = entries.as_object().ok_or_else(|| {
                format!("invalid spelling dictionary: '{section}' must be an object")
            })?;
            for (word, correction) in entries {
                let Some(correction) = correction.as_str() else {
                    return Err(format!(
                        "invalid spelling dictionary: '{section}.{word}' \
                         correction must be a string"
                    ));
                };
                if word.is_empty() {
                    continue;
                }
                target.retain(|rule| rule.word != *word);
                target.push(Rule {
                    lower: word.as_bytes().to_ascii_lowercase(),
                    word: word.clone(),
                    correction: correction.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Read a dictionary file and merge it, mirroring `loadDictionary`.
    pub fn load_dictionary_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read dictionary '{}': {error}", path.display()))?;
        self.load_dictionary_json(&text)
    }

    /// Add one exact (whole-word) rule, mirroring `addExactWordRule`.
    pub fn add_exact_rule(&mut self, misspelling: &str, correction: &str) -> &mut Self {
        if !misspelling.is_empty() {
            self.exact.retain(|rule| rule.word != misspelling);
            self.exact.push(Rule {
                lower: misspelling.as_bytes().to_ascii_lowercase(),
                word: misspelling.to_string(),
                correction: correction.to_string(),
            });
        }
        self
    }

    /// Add one partial (substring) rule, mirroring `addPartialWordRule`.
    pub fn add_partial_rule(&mut self, misspelling: &str, correction: &str) -> &mut Self {
        if !misspelling.is_empty() {
            self.partial.retain(|rule| rule.word != misspelling);
            self.partial.push(Rule {
                lower: misspelling.as_bytes().to_ascii_lowercase(),
                word: misspelling.to_string(),
                correction: correction.to_string(),
            });
        }
        self
    }

    /// Number of `(exact, partial)` rules held.
    #[must_use]
    pub fn rule_counts(&self) -> (usize, usize) {
        (self.exact.len(), self.partial.len())
    }
}

impl Default for SpellingLinter {
    fn default() -> Self {
        Self::new()
    }
}

impl Linter for SpellingLinter {
    fn name(&self) -> &'static str {
        "SPELL"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if data.is_empty() || data.contains(&0) {
            return Vec::new();
        }
        if self.exact.is_empty() && self.partial.is_empty() {
            return Vec::new();
        }
        // Case-insensitive matching over bytes, like PCRE `#...#i` without
        // the `u` modifier (ASCII case folding only).
        let lower = data.to_ascii_lowercase();
        let mut out = Vec::new();
        for rule in &self.exact {
            for offset in find_word_matches(&lower, &rule.lower) {
                let (line, col) = line_col_for_offset(data, offset);
                out.push(spelling_message(
                    path,
                    line,
                    col,
                    CODE_EXACT,
                    &rule.word,
                    &rule.correction,
                ));
            }
        }
        for rule in &self.partial {
            for offset in find_substring_matches(&lower, &rule.lower) {
                let (line, col) = line_col_for_offset(data, offset);
                out.push(spelling_message(
                    path,
                    line,
                    col,
                    CODE_PARTIAL,
                    &rule.word,
                    &rule.correction,
                ));
            }
        }
        out.sort_by(|a, b| (a.line, a.col, &a.code).cmp(&(b.line, b.col, &b.code)));
        out
    }
}

fn spelling_message(
    path: &str,
    line: usize,
    col: usize,
    code: &str,
    word: &str,
    correction: &str,
) -> LintMessage {
    LintMessage {
        path: path.to_string(),
        line,
        col,
        code: code.to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: NAME_SPELLING.to_string(),
        description: format!(
            "Possible spelling error. You wrote '{word}', but did you mean '{correction}'?"
        ),
        original: None,
        replacement: None,
    }
}

/// PCRE `\b` word byte (ASCII only, matching non-`u` mode).
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Non-overlapping whole-word matches (mirrors `preg_match_all`).
fn find_word_matches(lower_data: &[u8], word: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if word.is_empty() || word.len() > lower_data.len() {
        return out;
    }
    let mut start = 0;
    while start + word.len() <= lower_data.len() {
        let Some(relative) = lower_data[start..]
            .windows(word.len())
            .position(|window| window == word)
        else {
            break;
        };
        let at = start + relative;
        let left_ok = at == 0 || !is_word_byte(lower_data[at - 1]);
        let end = at + word.len();
        let right_ok = end == lower_data.len() || !is_word_byte(lower_data[end]);
        if left_ok && right_ok {
            out.push(at);
        }
        start = at + word.len();
    }
    out
}

/// Overlapping substring matches (mirrors the `stripos` loop with
/// `$pos = $next + 1`).
fn find_substring_matches(lower_data: &[u8], word: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if word.is_empty() || word.len() > lower_data.len() {
        return out;
    }
    let mut start = 0;
    while start + word.len() <= lower_data.len() {
        let Some(relative) = lower_data[start..]
            .windows(word.len())
            .position(|window| window == word)
        else {
            break;
        };
        let at = start + relative;
        out.push(at);
        start = at + 1;
    }
    out
}

/// 1-based (line, col) for a byte offset, counting bytes like the PHP engine.
fn line_col_for_offset(data: &[u8], offset: usize) -> (usize, usize) {
    let end = offset.min(data.len());
    let mut line = 1;
    let mut col = 1;
    for &byte in &data[..end] {
        if byte == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
#[path = "lint_spelling_tests.rs"]
mod lint_spelling_tests;
