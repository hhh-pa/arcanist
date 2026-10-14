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
//! Divergences (see T1128):
//! - Include/exclude/`severity.rules` patterns are PCRE in PHP
//!   (`ArcanistConfigurationDrivenLintEngine.php:146-192` `matchPaths()`;
//!   `ArcanistLinter.php:547-569`). The dependency set has no regex engine,
//!   so callers supply the matcher (same approach as `lint_script_and_regex`);
//!   `severity.rules` patterns are stored unchecked.
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
    /// (`src/lint/linter/ArcanistLinter.php`): exact map first, then the
    /// first matching rule, else `default`. Pattern matching is caller
    /// supplied (`matches(pattern, code)`), since there is no regex engine.
    #[must_use]
    pub fn severity_for(
        &self,
        code: &str,
        default: &str,
        matches: impl Fn(&str, &str) -> bool,
    ) -> String {
        if let Some(severity) = self.map.get(code) {
            return severity.clone();
        }
        for (pattern, severity) in &self.rules {
            if matches(pattern, code) {
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
/// `matchPaths()` in `ArcanistConfigurationDrivenLintEngine.php`: keep when
/// no includes are set or any include matches, then drop on any exclude or
/// global-exclude match. `matches(pattern, path)` is caller supplied.
pub fn route_paths(
    paths: &[String],
    include: &[String],
    exclude: &[String],
    global_exclude: &[String],
    matches: impl Fn(&str, &str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        let keep = if include.is_empty() {
            true
        } else {
            include.iter().any(|rule| matches(rule, path))
        };
        if !keep {
            continue;
        }
        if exclude.iter().any(|rule| matches(rule, path)) {
            continue;
        }
        if global_exclude.iter().any(|rule| matches(rule, path)) {
            continue;
        }
        out.push(path.clone());
    }
    out
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
pub fn plan_for_paths(
    config: &ArclintConfig,
    paths: &[String],
    matches: impl Fn(&str, &str) -> bool,
) -> Vec<LinterPlan> {
    config
        .linters
        .iter()
        .map(|spec| LinterPlan {
            name: spec.name.clone(),
            display: spec.display.clone(),
            paths: route_paths(
                paths,
                &spec.include,
                &spec.exclude,
                &config.global_exclude,
                &matches,
            ),
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
