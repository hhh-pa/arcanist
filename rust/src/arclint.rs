//! `.arclint` configuration: parse, linter selection, severities, path routing.
//!
//! PHP references: `src/lint/engine/ArcanistConfigurationDrivenLintEngine.php`
//! (config shape, `type` lookup, include/exclude/global-exclude routing),
//! `src/lint/linter/ArcanistLinter.php` (`severity`, `severity.rules`,
//! `standard` options), and `src/lint/ArcanistLintSeverity.php` (levels).
//!
//! [`ArclintConfig`] is a parsed `.arclint` file. [`plan_for_paths`] routes
//! caller paths to each configured linter; [`retain_selected`] filters a
//! caller-supplied linter vector down to the configured display names.
//!
//! PCRE patterns (`matchPaths()`,
//! `ArcanistConfigurationDrivenLintEngine.php:146-192`, and
//! `severity.rules`, `ArcanistLinter.php:547-569`) compile through
//! [`compile_pcre`]: delimiters plus the `i`, `m`, `s`, `x`, `u`, `D`,
//! `A` modifiers map onto the `regex` crate, with PCRE-only escapes
//! (`\Z`, `\h`, `\v`, `\R`, `\C`, `\Q...\E`, `(?<name>)`) emulated.
//! Constructs the `regex` crate cannot express (lookaround,
//! backreferences, conditionals, the `U` modifier) are invalid
//! patterns: configuration validation reports them, path matching
//! treats them as no match (PHP warns and returns false).
//!
//! Divergences (see T1128):
//! - PHP rejects unknown per-linter keys against each linter's
//!   `getLinterConfigurationOptions()`
//!   (`ArcanistConfigurationDrivenLintEngine.php:83-98`,
//!   `ArcanistLinter.php:490-513`); unknown keys are kept opaquely in
//!   [`LinterSpec::extra`] instead, since per-linter option tables live in
//!   the linter modules, not here.
//! - PHP `standard` values resolve through `ArcanistLinterStandard`
//!   (`ArcanistLinter.php:571-585`); here the raw value is kept in `extra`
//!   and no severity map is merged.
//! - PHP `buildLinters()` emits linters in config order
//!   (`ArcanistConfigurationDrivenLintEngine.php:51-136`);
//!   [`retain_selected`] preserves the caller vector's order.
//! - Linter sections and `severity.rules` entries iterate in sorted-key
//!   order: the pinned serde_json map does not preserve document order,
//!   while PHP iterates file order.
//! - PHP lists available types in class-map order in the invalid-type error
//!   (`ArcanistConfigurationDrivenLintEngine.php:55-61`); the list here is
//!   sorted for determinism.
//! - A `type` naming an exact display name (e.g. `"TXT"`) is accepted as an
//!   alias; PHP accepts only configuration names
//!   (`ArcanistConfigurationDrivenLintEngine.php:52-62,139-144`).

use std::collections::HashMap;
use std::path::Path;

/// The configuration file name, always at the working-copy root.
pub const CONFIG_FILE_NAME: &str = ".arclint";

/// Valid severity levels, in the PHP `$sev_map` order
/// (`src/lint/linter/ArcanistLinter.php`, `setLinterConfigurationValue`).
pub const SEVERITIES: &[&str] = &["error", "warning", "autofix", "advice", "disabled"];

/// True when `severity` is one of the five canonical levels.
#[must_use]
pub fn is_valid_severity(severity: &str) -> bool {
    SEVERITIES.contains(&severity)
}

/// (configuration name, display name) pairs, verbatim from PHP
/// `getLinterConfigurationName()` / `getLinterName()` in
/// `src/lint/linter/Arcanist*.php`.
const LINTER_TYPES: &[(&str, &str)] = &[
    ("text", "TXT"),
    ("filename", "NAME"),
    ("json", "JSON"),
    ("jsonlint", "JSON"),
    ("xml", "XML"),
    ("merge-conflict", "MERGECONFLICT"),
    ("generated", "GEN"),
    ("nolint", "NOLINT"),
    ("chmod", "CHMOD"),
    ("spelling", "SPELL"),
    ("script-and-regex", "S&RX"),
    ("composer", "COMPOSER"),
    ("phutil-library", "PHL"),
    ("jshint", "JSHint"),
    ("jscs", "JSCS"),
    ("csslint", "CSSLint"),
    ("lessc", "LESSC"),
    ("gjslint", "GJSLINT"),
    ("coffeelint", "COFFEE"),
    ("flake8", "flake8"),
    ("pyflakes", "PYFLAKES"),
    ("pylint", "PyLint"),
    ("pep8", "PEP8"),
    ("rubocop", "RuboCop"),
    ("ruby", "RUBY"),
    ("cppcheck", "cppcheck"),
    ("cpplint", "C++ Google's Styleguide"),
    ("golint", "GOLINT"),
    ("hlint", "HLINT"),
    ("csharp", "C#"),
    ("phpcs", "PHPCS"),
    ("php", "PHP"),
    ("puppet-lint", "PUPPETLINT"),
    ("xhpast", "XHP"),
];

/// Map a configuration name (`.arclint` `type`) to its display name.
#[must_use]
pub fn configuration_name_to_display(config_name: &str) -> Option<&'static str> {
    LINTER_TYPES
        .iter()
        .find(|(name, _)| *name == config_name)
        .map(|(_, display)| *display)
}

/// Canonical configuration name for a `.arclint` `type`: exact
/// configuration names map to themselves, display-name aliases map to
/// the first configuration name with that display (so `"JSON"` means
/// `"json"`, the first of the `json`/`jsonlint` pair).
#[must_use]
pub fn canonical_config_name(linter_type: &str) -> Option<&'static str> {
    if configuration_name_to_display(linter_type).is_some() {
        return LINTER_TYPES
            .iter()
            .find(|(name, _)| *name == linter_type)
            .map(|(name, _)| *name);
    }
    LINTER_TYPES
        .iter()
        .find(|(_, display)| *display == linter_type)
        .map(|(name, _)| *name)
}

/// Every selectable configuration name, sorted.
#[must_use]
pub fn available_types() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = LINTER_TYPES.iter().map(|(name, _)| *name).collect();
    out.sort_unstable();
    out
}

/// Every display name, deduplicated and sorted.
#[must_use]
pub fn display_names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = LINTER_TYPES.iter().map(|(_, display)| *display).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Resolve a `.arclint` `type` to its display name: exact configuration names
/// first (the PHP behavior), then exact display names as an alias.
fn resolve_type(linter_type: &str) -> Option<&'static str> {
    if let Some(display) = configuration_name_to_display(linter_type) {
        return Some(display);
    }
    LINTER_TYPES
        .iter()
        .find(|(_, display)| *display == linter_type)
        .map(|(_, display)| *display)
}

/* -(  PCRE patterns  )-------------------------------------------------------- */

/// Flags parsed from a PCRE pattern's trailing modifiers. Matching is
/// always Unicode-aware (subjects are `&str`): the `u` modifier is
/// accepted and ignored. PHP without `u` treats `\d`, `\s`, `\w`, and
/// case folding as ASCII-only, so non-ASCII subjects can differ.
#[derive(Debug, Clone, Copy, Default)]
struct PcreFlags {
    case_insensitive: bool,
    multi_line: bool,
    dot_all: bool,
    extended: bool,
    dollar_end_only: bool,
    anchored: bool,
}

/// The ending delimiter matching an opening `delim`.
fn pcre_end_delimiter(delim: u8) -> u8 {
    match delim {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        _ => delim,
    }
}

/// True when the `]` at `index` closes the class opened at
/// `class_open` (a `]` first in a class, or right after `[^`, is a
/// literal, like PCRE).
fn closes_class(bytes: &[u8], class_open: usize, index: usize) -> bool {
    if index == class_open + 1 {
        return false;
    }
    if index == class_open + 2 && bytes[class_open + 1] == b'^' {
        return false;
    }
    true
}

/// Split a PCRE pattern into its body and modifiers, mirroring how
/// `preg_match` locates the ending delimiter: the first unescaped
/// delimiter byte outside a character class (with bracket nesting for
/// `()`, `[]`, `{}`, `<>`), then modifier letters.
fn split_pcre(pattern: &str) -> Result<(String, PcreFlags), String> {
    let bytes = pattern.as_bytes();
    let Some(&delim) = bytes.first() else {
        return Err("empty regular expression".to_string());
    };
    if delim.is_ascii_alphanumeric() || delim == b'\\' || delim.is_ascii_whitespace() {
        return Err(format!("invalid delimiter '{}'", delim as char));
    }
    let end = pcre_end_delimiter(delim);
    let body_end = scan_pcre_body(bytes, delim, end)?;
    let body = &pattern[1..body_end];
    let flags = parse_pcre_modifiers(&pattern[body_end + 1..])?;
    Ok((body.to_string(), flags))
}

/// Scan state for [`scan_pcre_body`]: the byte cursor, the bracket
/// nesting depth, and the opening `[` of the current character
/// class, if any.
struct PcreScan {
    index: usize,
    depth: usize,
    class_open: Option<usize>,
}

/// Find the closing delimiter of a PCRE pattern, honoring escapes,
/// character classes, and bracket-delimiter nesting.
fn scan_pcre_body(bytes: &[u8], delim: u8, end: u8) -> Result<usize, String> {
    let bracketed = end != delim;
    let mut scan = PcreScan {
        index: 1,
        depth: 0,
        class_open: None,
    };
    while scan.index < bytes.len() {
        if let Some(body_end) = step_pcre_scan(bytes, &mut scan, delim, end, bracketed) {
            if body_end == 1 {
                return Err("empty regular expression".to_string());
            }
            return Ok(body_end);
        }
    }
    Err("missing ending delimiter".to_string())
}

/// Advance one scan step; reports the body end when the closing
/// delimiter is reached outside classes at depth zero.
fn step_pcre_scan(
    bytes: &[u8],
    scan: &mut PcreScan,
    delim: u8,
    end: u8,
    bracketed: bool,
) -> Option<usize> {
    let byte = bytes[scan.index];
    if byte == b'\\' {
        scan.index += 2;
        return None;
    }
    if byte == b'[' && scan.class_open.is_none() {
        scan.class_open = Some(scan.index);
    } else if closes_pcre_class(bytes, scan, byte) {
        scan.class_open = None;
    } else if byte == end && scan.class_open.is_none() && scan.depth == 0 {
        return Some(scan.index);
    } else if bracketed && scan.class_open.is_none() && (byte == delim || byte == end) {
        step_pcre_depth(scan, byte, delim);
    }
    scan.index += 1;
    None
}

/// True when `byte` closes the currently open character class.
fn closes_pcre_class(bytes: &[u8], scan: &PcreScan, byte: u8) -> bool {
    byte == b']'
        && scan
            .class_open
            .is_some_and(|open| closes_class(bytes, open, scan.index))
}

/// Track one bracket-delimiter byte: the opener nests deeper, the
/// closer returns (only reached when the two differ).
fn step_pcre_depth(scan: &mut PcreScan, byte: u8, delim: u8) {
    if byte == delim {
        scan.depth += 1;
    } else {
        scan.depth -= 1;
    }
}

/// Parse the PCRE modifier tail into flags.
fn parse_pcre_modifiers(tail: &str) -> Result<PcreFlags, String> {
    let mut flags = PcreFlags::default();
    for modifier in tail.bytes() {
        match modifier {
            b'i' => flags.case_insensitive = true,
            b'm' => flags.multi_line = true,
            b's' => flags.dot_all = true,
            b'x' => flags.extended = true,
            b'D' => flags.dollar_end_only = true,
            b'A' => flags.anchored = true,
            b'u' | b'S' | b'X' | b'J' => {}
            _ => return Err(format!("unknown modifier '{}'", modifier as char)),
        }
    }
    Ok(flags)
}

/// Translate a PCRE body into Rust `regex` syntax: unescape the
/// delimiter, expand `\Q...\E` quoting, map the `(?<name>)` /
/// `(?'name')` group spellings, and emulate the escapes the `regex`
/// crate lacks (`\Z`, `\h`, `\H`, `\v`, `\V`, `\R`, `\C`, `\0`). A
/// `$` outside classes becomes end-or-before-final-newline when
/// neither `m` nor `D` is set, matching PCRE `$` semantics. The scan
/// is byte-based so multibyte literals pass through untouched.
fn translate_pcre_body(body: &str, delim: u8, end: u8, flags: PcreFlags) -> Result<String, String> {
    let bytes = body.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(body.len() + 16);
    let mut index = 0;
    let mut class_open = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\\' {
            let Some(next) = bytes.get(index + 1) else {
                return Err("trailing backslash".to_string());
            };
            index = translate_backslash(body, index, *next, delim, end, &mut out);
            continue;
        }
        if byte == b'[' && class_open.is_none() {
            class_open = Some(index);
            out.push(b'[');
        } else if byte == b']'
            && let Some(open) = class_open
            && closes_class(bytes, open, index)
        {
            class_open = None;
            out.push(b']');
        } else if byte == b'$'
            && class_open.is_none()
            && !flags.multi_line
            && !flags.dollar_end_only
        {
            out.extend_from_slice(b"(?:\\n?\\z)");
        } else if byte == b'(' && class_open.is_none() {
            out.extend_from_slice(translate_group_open(body, index).as_bytes());
            index = skip_group_open(body, index);
            continue;
        } else {
            out.push(byte);
        }
        index += 1;
    }
    String::from_utf8(out).map_err(|_| "invalid pattern".to_string())
}

/// Translate the `\X` escape at `index`, appending to `out` and
/// returning the next scan index.
fn translate_backslash(
    body: &str,
    index: usize,
    next: u8,
    delim: u8,
    end: u8,
    out: &mut Vec<u8>,
) -> usize {
    let bytes = body.as_bytes();
    let in_class = class_is_open(bytes, index);
    if next == b'Q' {
        let (quoted, rest) = take_quoted(body, index + 2);
        out.extend_from_slice(regex::escape(quoted).as_bytes());
        return rest;
    }
    if let Some(expansion) = translate_escape(next, in_class) {
        out.extend_from_slice(expansion.as_bytes());
        return index + 2;
    }
    if next == delim || next == end {
        out.push(next);
        return index + 2;
    }
    if next == b'0' && !bytes.get(index + 2).is_some_and(u8::is_ascii_digit) {
        out.extend_from_slice(b"\\x00");
        return index + 2;
    }
    out.push(b'\\');
    out.push(next);
    index + 2
}

/// True when `index` sits inside a `[...]` class, rescanning from the
/// body start (patterns are short; clarity beats a threaded flag).
fn class_is_open(bytes: &[u8], index: usize) -> bool {
    let mut open = None;
    let mut cursor = 0;
    while cursor < index {
        if bytes[cursor] == b'\\' {
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b'[' && open.is_none() {
            open = Some(cursor);
        } else if bytes[cursor] == b']'
            && let Some(start) = open
            && closes_class(bytes, start, cursor)
        {
            open = None;
        }
        cursor += 1;
    }
    open.is_some()
}

/// Expand one PCRE escape into `regex` syntax, or `None` to keep the
/// original escape. Class-only translations apply inside `[...]`.
/// Expansions use escapes (never literal control bytes) so the `x`
/// flag cannot swallow them.
fn translate_escape(next: u8, in_class: bool) -> Option<&'static str> {
    if in_class {
        return match next {
            b'h' => Some(" \\t"),
            b'v' => Some("\\n\\x0B\\x0C\\r"),
            _ => None,
        };
    }
    match next {
        b'Z' => Some("(?:\\n?\\z)"),
        b'h' => Some("[ \\t]"),
        b'H' => Some("[^ \\t]"),
        b'v' => Some("[\\n\\x0B\\x0C\\r]"),
        b'V' => Some("[^\\n\\x0B\\x0C\\r]"),
        b'R' => Some("(?:\\r\\n|[\\n\\r])"),
        b'C' => Some("(?s:.)"),
        _ => None,
    }
}

/// Take a `\Q...\E` quoted span starting after the `\Q`; an unterminated
/// span quotes to the end of the pattern, like PCRE.
fn take_quoted(body: &str, start: usize) -> (&str, usize) {
    let bytes = body.as_bytes();
    let mut index = start;
    while index < bytes.len() {
        if bytes[index] == b'\\' && bytes.get(index + 1) == Some(&b'E') {
            return (&body[start..index], index + 2);
        }
        index += 1;
    }
    (&body[start..], bytes.len())
}

/// Rewrite a group opening at `index`: the `(?<name>` and `(?'name'`
/// spellings become `(?P<name>`, everything else is kept verbatim.
fn translate_group_open(body: &str, index: usize) -> String {
    let rest = &body[index..];
    if let Some(name) = group_name_after(rest, "(?<", ">") {
        return format!("(?P<{name}>");
    }
    if let Some(name) = group_name_after(rest, "(?'", "'") {
        return format!("(?P<{name}>");
    }
    "(".to_string()
}

/// Length of the group opening at `index` (matching
/// [`translate_group_open`]) so the scanner can skip past it.
fn skip_group_open(body: &str, index: usize) -> usize {
    let rest = &body[index..];
    if let Some(name) = group_name_after(rest, "(?<", ">") {
        return index + "(?<".len() + name.len() + ">".len();
    }
    if let Some(name) = group_name_after(rest, "(?'", "'") {
        return index + "(?'".len() + name.len() + "'".len();
    }
    index + 1
}

/// The group name when `rest` starts with `prefix + name + suffix` and
/// the name is a valid group identifier (PCRE `(?<name>` rejects
/// `(?<=` / `(?<!` lookbehinds, which keep their meaning).
fn group_name_after<'a>(rest: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let after = rest.strip_prefix(prefix)?;
    if prefix == "(?<" && after.starts_with(['=', '!']) {
        return None;
    }
    let end = after.find(suffix)?;
    let name = &after[..end];
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    Some(name)
}

/// Compile a PHP PCRE pattern (delimiters plus modifiers, as passed to
/// `preg_match`) into a [`regex::Regex`]. Matching is unanchored, like
/// `preg_match`. An `Err` mirrors `preg_match($rule, '') === false`
/// (invalid pattern): callers validating configuration report it,
/// callers matching paths treat it as no match.
pub fn compile_pcre(pattern: &str) -> Result<regex::Regex, String> {
    let (body, flags) =
        split_pcre(pattern).map_err(|detail| format!("invalid pattern: {detail}"))?;
    let delim = pattern.as_bytes()[0];
    let end = pcre_end_delimiter(delim);
    let mut translated = translate_pcre_body(&body, delim, end, flags)
        .map_err(|detail| format!("invalid pattern: {detail}"))?;
    if flags.anchored {
        translated = format!("\\A(?:{translated})");
    }
    let mut builder = regex::RegexBuilder::new(&translated);
    builder
        .case_insensitive(flags.case_insensitive)
        .multi_line(flags.multi_line)
        .dot_matches_new_line(flags.dot_all)
        .ignore_whitespace(flags.extended);
    builder.build().map_err(|_| "invalid pattern".to_string())
}

/// True when PHP's `preg_match($rule, $subject)` would report a match.
/// Invalid patterns never match (PHP warns and returns false).
#[must_use]
pub fn pcre_matches(pattern: &str, subject: &str) -> bool {
    compile_pcre(pattern).is_ok_and(|re| re.is_match(subject))
}

/// Per-linter severity overrides: an exact code map plus ordered regex rules,
/// mirroring `customSeverityMap` / `customSeverityRules` in
/// `src/lint/linter/ArcanistLinter.php`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeverityOverrides {
    /// Exact code -> severity (`severity` key).
    pub map: HashMap<String, String>,
    /// (pattern, severity) in config order (`severity.rules` key).
    pub rules: Vec<(String, String)>,
}

impl SeverityOverrides {
    /// Resolve the severity for `code`, mirroring `getLintMessageSeverity()`
    /// (`src/lint/linter/ArcanistLinter.php:350-360`): exact map first,
    /// then the first matching `severity.rules` pattern (PCRE,
    /// unanchored), else `default`. Callers pass the code in the form
    /// PHP looks up (short codes for `raiseLintAt*` linters, full
    /// codes for direct-message linters).
    #[must_use]
    pub fn severity_for(&self, code: &str, default: &str) -> String {
        if let Some(severity) = self.map.get(code) {
            return severity.clone();
        }
        for (pattern, severity) in &self.rules {
            if pcre_matches(pattern, code) {
                return severity.clone();
            }
        }
        default.to_string()
    }
}

/// One named entry under the top-level `linters` map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinterSpec {
    /// The config key (e.g. `"text-without-length"`).
    pub name: String,
    /// The raw `type` value (e.g. `"text"`).
    pub linter_type: String,
    /// Display name the type resolves to (e.g. `"TXT"`).
    pub display: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub severity: SeverityOverrides,
    /// Linter-specific keys (including `standard`), kept opaquely.
    pub extra: HashMap<String, serde_json::Value>,
}

/// A parsed `.arclint` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArclintConfig {
    pub global_exclude: Vec<String>,
    /// In sorted-key order (see the divergences note).
    pub linters: Vec<LinterSpec>,
}

fn shape_error(context: &str, detail: &str) -> String {
    format!("Error in parsing {context}: {detail}")
}

fn unknown_severity_error(severity: &str) -> String {
    format!(
        "Unknown lint severity \"{severity}\". Valid severities are: {}.",
        SEVERITIES.join(", ")
    )
}

/// Parse a `regex | list<regex>` value into patterns.
fn parse_pattern_list(value: &serde_json::Value, what: &str) -> Result<Vec<String>, String> {
    match value {
        serde_json::Value::String(text) => Ok(vec![text.clone()]),
        serde_json::Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                match item {
                    serde_json::Value::String(text) => out.push(text.clone()),
                    _ => return Err(format!("{what} must be a string or a list of strings")),
                }
            }
            Ok(out)
        }
        _ => Err(format!("{what} must be a string or a list of strings")),
    }
}

/// Parse a `severity` / `severity.rules` map, validating severity values.
/// `severity.rules` patterns must also be valid PCRE
/// (`ArcanistLinter.php:552-558`), checked before the severity like PHP.
fn parse_severity_map(
    value: &serde_json::Value,
    into_map: &mut HashMap<String, String>,
    into_rules: &mut Vec<(String, String)>,
    is_rules: bool,
) -> Result<(), String> {
    let serde_json::Value::Object(entries) = value else {
        return Err("severity maps must be objects".to_string());
    };
    for (code, severity) in entries {
        if is_rules && compile_pcre(code).is_err() {
            return Err(format!(
                "Severity rule \"{code}\" is not a valid regular expression."
            ));
        }
        let serde_json::Value::String(severity) = severity else {
            return Err(format!("severity for code \"{code}\" must be a string"));
        };
        if !is_valid_severity(severity) {
            return Err(unknown_severity_error(severity));
        }
        if is_rules {
            into_rules.push((code.clone(), severity.clone()));
        } else {
            into_map.insert(code.clone(), severity.clone());
        }
    }
    Ok(())
}

fn parse_linter_spec(name: &str, value: &serde_json::Value) -> Result<LinterSpec, String> {
    let context = format!("'.arclint' file, for linter '{name}'");
    let fail = |detail: String| shape_error(&context, &detail);
    let serde_json::Value::Object(spec) = value else {
        return Err(fail("linter specs must be objects".to_string()));
    };
    let linter_type = match spec.get("type") {
        Some(serde_json::Value::String(linter_type)) => linter_type.clone(),
        _ => return Err(fail("'type' must be a string".to_string())),
    };
    let Some(display) = resolve_type(&linter_type) else {
        return Err(format!(
            "Linter '{name}' specifies invalid type '{linter_type}'. Available linters are: {}.",
            available_types().join(", ")
        ));
    };
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    let mut severity = SeverityOverrides::default();
    let mut extra = HashMap::new();
    for (key, val) in spec {
        match key.as_str() {
            "type" => {}
            "include" => {
                include = parse_pattern_list(val, "'include'").map_err(fail)?;
            }
            "exclude" => {
                exclude = parse_pattern_list(val, "'exclude'").map_err(fail)?;
            }
            "severity" => {
                parse_severity_map(val, &mut severity.map, &mut severity.rules, false)
                    .map_err(fail)?;
            }
            "severity.rules" => {
                parse_severity_map(val, &mut severity.map, &mut severity.rules, true)
                    .map_err(fail)?;
            }
            _ => {
                extra.insert(key.clone(), val.clone());
            }
        }
    }
    Ok(LinterSpec {
        name: name.to_string(),
        linter_type,
        display: display.to_string(),
        include,
        exclude,
        severity,
        extra,
    })
}

/// Parse `.arclint` JSON text.
pub fn parse_arclint(text: &str) -> Result<ArclintConfig, String> {
    let config: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("Expected '.arclint' file to be a valid JSON file: {e}"))?;
    let serde_json::Value::Object(root) = &config else {
        return Err(shape_error(
            "'.arclint' file",
            "top level must be an object",
        ));
    };
    let mut global_exclude = Vec::new();
    if let Some(value) = root.get("exclude") {
        global_exclude = parse_pattern_list(value, "'exclude'")
            .map_err(|detail| shape_error("'.arclint' file", &detail))?;
    }
    let Some(linters) = root.get("linters") else {
        return Err(shape_error("'.arclint' file", "'linters' is required"));
    };
    let serde_json::Value::Object(linters) = linters else {
        return Err(shape_error("'.arclint' file", "'linters' must be a map"));
    };
    let mut specs = Vec::new();
    for (name, spec) in linters {
        specs.push(parse_linter_spec(name, spec)?);
    }
    Ok(ArclintConfig {
        global_exclude,
        linters: specs,
    })
}

/// Load and parse `<dir>/.arclint`, with PHP-shaped file errors.
pub fn load_arclint(dir: &Path) -> Result<ArclintConfig, String> {
    let path = dir.join(CONFIG_FILE_NAME);
    if !path.exists() {
        return Err(
            "Unable to find '.arclint' file to configure linters. Create an '.arclint' file in the root directory of the working copy."
                .to_string(),
        );
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    parse_arclint(&text).map_err(|e| e.replace("'.arclint' file", &format!("'{}'", path.display())))
}

/// Route `paths` through include/exclude/global-exclude lists, mirroring
/// `matchPaths()` in `ArcanistConfigurationDrivenLintEngine.php:146-192`:
/// keep when no includes are set or any include matches, then drop on
/// any exclude or global-exclude match. Patterns are PCRE matched
/// unanchored (`preg_match`); invalid patterns never match.
pub fn route_paths(
    paths: &[String],
    include: &[String],
    exclude: &[String],
    global_exclude: &[String],
) -> Vec<String> {
    let include = compile_all(include);
    let exclude = compile_all(exclude);
    let global_exclude = compile_all(global_exclude);
    let mut out = Vec::new();
    for path in paths {
        let keep = if include.is_empty() {
            true
        } else {
            include.iter().any(|rule| rule.matches(path))
        };
        if !keep {
            continue;
        }
        if exclude.iter().any(|rule| rule.matches(path)) {
            continue;
        }
        if global_exclude.iter().any(|rule| rule.matches(path)) {
            continue;
        }
        out.push(path.clone());
    }
    out
}

/// One routing pattern: a compiled PCRE regex, or an invalid pattern
/// that never matches.
#[derive(Debug, Clone)]
struct RoutePattern {
    regex: Option<regex::Regex>,
}

impl RoutePattern {
    fn matches(&self, path: &str) -> bool {
        self.regex.as_ref().is_some_and(|re| re.is_match(path))
    }
}

fn compile_all(patterns: &[String]) -> Vec<RoutePattern> {
    patterns
        .iter()
        .map(|pattern| RoutePattern {
            regex: compile_pcre(pattern).ok(),
        })
        .collect()
}

/// Precompiled include/exclude/global-exclude lists for one configured
/// section, so the engine can test paths without recompiling.
#[derive(Debug, Clone, Default)]
pub struct SectionRouting {
    include: Vec<RoutePattern>,
    exclude: Vec<RoutePattern>,
    global_exclude: Vec<RoutePattern>,
}

impl SectionRouting {
    /// Compile routing lists once. Empty lists match everything.
    #[must_use]
    pub fn new(include: &[String], exclude: &[String], global_exclude: &[String]) -> Self {
        Self {
            include: compile_all(include),
            exclude: compile_all(exclude),
            global_exclude: compile_all(global_exclude),
        }
    }

    /// True when `path` survives this section's routing, mirroring
    /// `matchPaths()`.
    #[must_use]
    pub fn allows(&self, path: &str) -> bool {
        if !self.include.is_empty() && !self.include.iter().any(|rule| rule.matches(path)) {
            return false;
        }
        if self.exclude.iter().any(|rule| rule.matches(path)) {
            return false;
        }
        if self.global_exclude.iter().any(|rule| rule.matches(path)) {
            return false;
        }
        true
    }
}

/// One configured linter with its routed paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinterPlan {
    /// The config key.
    pub name: String,
    /// Resolved display name.
    pub display: String,
    pub paths: Vec<String>,
    pub severity: SeverityOverrides,
}

/// Build per-linter path plans for `paths`, in sorted-key order.
pub fn plan_for_paths(config: &ArclintConfig, paths: &[String]) -> Vec<LinterPlan> {
    config
        .linters
        .iter()
        .map(|spec| LinterPlan {
            name: spec.name.clone(),
            display: spec.display.clone(),
            paths: route_paths(paths, &spec.include, &spec.exclude, &config.global_exclude),
            severity: spec.severity.clone(),
        })
        .collect()
}

/// Display names selected by `config`, deduplicated, in sorted-key order.
#[must_use]
pub fn selected_display_names(config: &ArclintConfig) -> Vec<String> {
    let mut out = Vec::new();
    for spec in &config.linters {
        if !out.contains(&spec.display) {
            out.push(spec.display.clone());
        }
    }
    out
}

/// Keep only the linters whose display name (`name_of`) the config selects.
/// Matching is exact and case-sensitive against the verbatim PHP names.
pub fn retain_selected<T>(
    linters: &mut Vec<T>,
    name_of: impl Fn(&T) -> &str,
    config: &ArclintConfig,
) {
    let selected = selected_display_names(config);
    linters.retain(|linter| selected.iter().any(|name| name == name_of(linter)));
}

#[cfg(test)]
#[path = "arclint_tests.rs"]
mod arclint_tests;
