//! Configuration-driven unit test engine.
//!
//! PHP reference: `src/unit/engine/ArcanistConfigurationDrivenUnitTestEngine.php`
//! (concrete, `final class`). Ports `supportsRunAllTests` (lines 6-16),
//! `buildTestEngines` (lines 18-115), `run` (lines 117-170),
//! `shouldEchoTestResults` (lines 172-174), and `matchPaths` (lines 186-218).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - `include`/`exclude` rules are matched with an ordered-substring matcher
//!   instead of `preg_match($rule, $path)` (line 195): this port has no regex
//!   engine (dependency-free crate). Rules with `(.*)` match their literal
//!   parts in order; other rules match as literal substrings after stripping
//!   delimiters and `^`/`$` anchors. Regex character classes, quantifiers, and
//!   alternations are not supported.
//! - `loadAvailableTestEngines` (lines 176-181) uses a class-map query; here
//!   the caller supplies the available engine names explicitly.
//! - `run` renders proxied results through `ArcanistUnitRenderer`
//!   (lines 151-157); rendering is out of scope, so this port only merges.

extern crate self as arc_rs;

use arc_rs::unit_engine::{EngineContext, EngineError, JsonValue, UnitResult, parse_json};
use std::collections::HashMap;

/// Config file name (`.arcunit`), per `buildTestEngines` (line 20).
pub const ARCUNIT_FILE: &str = ".arcunit";

/// `shouldEchoTestResults` (lines 172-174) returns false: the proxy engine
/// renders while its children run.
pub const SHOULD_ECHO_TEST_RESULTS: bool = false;

/// One planned child engine from the `.arcunit` file: its config key plus the
/// validated `type`/`include`/`exclude` spec (lines 62-112).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEngine {
    /// Config key under `engines` (the `$name` loop variable, line 62).
    pub name: String,
    /// Validated `type` value (lines 63-81).
    pub engine_type: String,
    /// `include` rules (line 101; empty when absent).
    pub include: Vec<String>,
    /// `exclude` rules (line 102; empty when absent).
    pub exclude: Vec<String>,
}

/// Parse one spec's `include`/`exclude` value: a single regex string or a list
/// of them (`'include' => 'optional regex | list<regex>'`, lines 88-89).
fn parse_rule_list(value: &JsonValue) -> Option<Vec<String>> {
    match value {
        JsonValue::Str(rule) => Some(vec![rule.clone()]),
        JsonValue::Arr(items) => {
            let mut rules = Vec::new();
            for item in items {
                match item {
                    JsonValue::Str(rule) => rules.push(rule.clone()),
                    _ => return None,
                }
            }
            Some(rules)
        }
        _ => None,
    }
}

/// `buildTestEngines` spec parsing (lines 31-115): decode the `.arcunit` JSON
/// and validate each engine spec against the available engine names, in PHP
/// order (invalid-type check first for non-null types, then the shape check).
pub fn parse_engine_specs(
    data: &str,
    config_path: &str,
    available: &[&str],
) -> Result<Vec<PlannedEngine>, EngineError> {
    let config = parse_json(data).map_err(|e| {
        EngineError::Failed(format!(
            "Expected '.arcunit' file to be a valid JSON file, but failed to decode \
             '{config_path}': {e}"
        ))
    })?;
    let Some(entries) = config.get("engines").and_then(JsonValue::as_obj) else {
        return Err(EngineError::Failed(format!(
            "Error in parsing '{config_path}' file: expected an 'engines' map."
        )));
    };
    check_spec_shapes(entries, config_path)?;

    let mut planned = Vec::new();
    for (name, spec) in entries {
        planned.push(parse_single_spec(name, spec, available)?);
    }
    Ok(planned)
}

/// Reject non-map engine specs before any per-engine validation, so shape
/// errors take precedence over type errors regardless of entry order.
fn check_spec_shapes(
    entries: &[(String, JsonValue)],
    config_path: &str,
) -> Result<(), EngineError> {
    for (name, spec) in entries {
        let Some(spec_obj) = spec.as_obj() else {
            return Err(EngineError::Failed(format!(
                "Error in parsing '{config_path}' file: engine '{name}' must be a map."
            )));
        };
        let _ = spec_obj;
    }
    Ok(())
}

/// Validate one engine spec: a known `type` plus optional `include`/
/// `exclude` rule lists; unknown keys are errors.
fn parse_single_spec(
    name: &str,
    spec: &JsonValue,
    available: &[&str],
) -> Result<PlannedEngine, EngineError> {
    let spec_obj = spec.as_obj().unwrap_or(&[]);
    let engine_type = spec_obj
        .iter()
        .find(|(k, _)| k == "type")
        .and_then(|(_, v)| v.as_str());

    if let Some(engine_type) = engine_type
        && !available.contains(&engine_type)
    {
        return Err(EngineError::Usage(format!(
            "Test engine '{name}' specifies invalid type '{engine_type}'. Available test \
             engines are: {}.",
            available.join(", ")
        )));
    }

    let Some(engine_type) = engine_type else {
        return Err(EngineError::Failed(format!(
            "Error in parsing '.arcunit' file, for test engine '{name}': expected a \
             'type' string."
        )));
    };
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    for (key, value) in spec_obj {
        match key.as_str() {
            "type" => {}
            "include" => {
                include = parse_rule_list(value).ok_or_else(|| {
                    EngineError::Failed(format!(
                        "Error in parsing '.arcunit' file, for test engine '{name}': \
                         'include' must be a regex or a list of regexes."
                    ))
                })?;
            }
            "exclude" => {
                exclude = parse_rule_list(value).ok_or_else(|| {
                    EngineError::Failed(format!(
                        "Error in parsing '.arcunit' file, for test engine '{name}': \
                         'exclude' must be a regex or a list of regexes."
                    ))
                })?;
            }
            _ => {
                return Err(EngineError::Failed(format!(
                    "Error in parsing '.arcunit' file, for test engine '{name}': unknown \
                     key '{key}'."
                )));
            }
        }
    }
    Ok(PlannedEngine {
        name: name.to_string(),
        engine_type: engine_type.to_string(),
        include,
        exclude,
    })
}

/// Read `.arcunit` from the project root and parse it (lines 19-43). A missing
/// file mirrors the PHP usage error (lines 23-29).
pub fn load_engine_specs(
    project_root: &str,
    available: &[&str],
) -> Result<Vec<PlannedEngine>, EngineError> {
    let config_path = format!("{project_root}/{ARCUNIT_FILE}");
    let data = std::fs::read_to_string(&config_path).map_err(|_| {
        EngineError::Usage(
            "Unable to find '.arcunit' file to configure test engines. Create an '.arcunit' \
             file in the root directory of the working copy."
                .to_string(),
        )
    })?;
    parse_engine_specs(&data, &config_path, available)
}

/// Ordered-substring rule matcher standing in for `preg_match($rule, $path)`
/// (line 195). See the module docs for the Divergence.
#[must_use]
pub fn rule_matches(rule: &str, path: &str) -> bool {
    let body = strip_rule_delimiters(rule);
    if body.contains("(.*)") {
        return match_capture_rule(body, path);
    }
    match_literal_rule(body, path)
}

/// Strip `preg_match` delimiters with trailing flags, e.g. `@...@i`.
/// Bracket openers are never treated as delimiters: a rule starting with a
/// group (e.g. `(.*)Tests/`) is matched as a body instead.
fn strip_rule_delimiters(rule: &str) -> &str {
    let body = rule;
    if body.len() >= 2 {
        let first = body.chars().next().unwrap_or('\0');
        let is_bracket = matches!(first, '(' | '[' | '{' | '<');
        if !first.is_alphanumeric()
            && first != '\\'
            && first != '\0'
            && !is_bracket
            && let Some(last) = body.rfind(first)
            && last > 0
        {
            return &body[1..last];
        }
    }
    body
}

/// Ordered literal match around `(.*)` captures: the first part anchors at
/// the path start, later parts match in order.
fn match_capture_rule(body: &str, path: &str) -> bool {
    let mut rest = path;
    for (i, part) in body.split("(.*)").enumerate() {
        if part.is_empty() {
            continue;
        }
        let part = part.replace("\\/", "/");
        if i == 0 {
            if let Some(stripped) = rest.strip_prefix(part.as_str()) {
                rest = stripped;
            } else {
                return false;
            }
        } else if let Some(at) = rest.find(part.as_str()) {
            rest = &rest[at + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// Literal substring match after stripping `^`/`$` anchors.
fn match_literal_rule(body: &str, path: &str) -> bool {
    let mut literal = body.to_string();
    if literal.starts_with('^') {
        literal.remove(0);
    }
    if literal.ends_with('$') {
        literal.pop();
    }
    literal = literal.replace("\\/", "/");
    if literal.is_empty() {
        return true;
    }
    path.contains(&literal)
}

/// `matchPaths` (lines 186-218): keep paths matching any include rule (or all
/// paths when no includes), then drop paths matching any exclude rule.
#[must_use]
pub fn match_paths(paths: &[String], include: &[String], exclude: &[String]) -> Vec<String> {
    let mut matched = Vec::new();
    for path in paths {
        let mut keep = false;
        if include.is_empty() {
            keep = true;
        } else {
            for rule in include {
                if rule_matches(rule, path) {
                    keep = true;
                    break;
                }
            }
        }
        if !keep {
            continue;
        }
        let mut dropped = false;
        for rule in exclude {
            if rule_matches(rule, path) {
                dropped = true;
                break;
            }
        }
        if !dropped {
            matched.push(path.clone());
        }
    }
    matched
}

/// `supportsRunAllTests` (lines 6-16): true when any child engine supports it.
#[must_use]
pub fn supports_run_all_tests(children_support: &[bool]) -> bool {
    children_support.iter().any(|s| *s)
}

/// Per-engine path assignment (lines 100-109): only when the proxy itself has
/// paths, each child gets its include/exclude-filtered subset.
#[must_use]
pub fn paths_for_engine(
    all_paths: &[String],
    include: &[String],
    exclude: &[String],
) -> Option<Vec<String>> {
    if all_paths.is_empty() {
        return None;
    }
    Some(match_paths(all_paths, include, exclude))
}

/// `run` result merging (lines 128-169): concatenate child results; swallow
/// per-engine `NoEffect` (lines 158-160); when every child reports no effect,
/// preserve that behavior with `No tests to run.` (lines 163-167). Any other
/// error propagates, mirroring the uncaught PHP exception.
pub fn merge_engine_runs(
    runs: Vec<Result<Vec<UnitResult>, EngineError>>,
) -> Result<Vec<UnitResult>, EngineError> {
    let mut all_results: Vec<Vec<UnitResult>> = Vec::new();
    for run in runs {
        match run {
            Ok(results) => all_results.push(results),
            Err(e) if e.is_no_effect() => {}
            Err(e) => return Err(e),
        }
    }
    if all_results.is_empty() {
        return Err(EngineError::no_effect());
    }
    Ok(all_results.concat())
}

/// Build child contexts from the proxy context (lines 122-144): paths default
/// to empty under `--everything`, and run-all propagates only to children
/// that support it.
#[must_use]
pub fn child_context(
    ctx: &EngineContext,
    engine: &PlannedEngine,
    supports_run_all: bool,
) -> EngineContext {
    let mut child = EngineContext {
        project_root: ctx.project_root.clone(),
        paths: Vec::new(),
        enable_coverage: ctx.enable_coverage,
        run_all_tests: false,
    };
    if let Some(paths) = paths_for_engine(&ctx.paths, &engine.include, &engine.exclude) {
        child.paths = paths;
    }
    if supports_run_all {
        child.run_all_tests = ctx.run_all_tests;
    }
    child
}

/// Coverage-type name lookup shared by engine wiring: map a config `type` to
/// its index in the available list (lines 65-76 use a class-map query instead).
#[must_use]
pub fn available_index(available: &[&str], engine_type: &str) -> Option<usize> {
    available.iter().position(|name| *name == engine_type)
}

/// Placeholder engine-name registry helper kept honest: with no class map,
/// the caller owns the registry, so this only formats the "available" list
/// tail used by the invalid-type error.
#[must_use]
pub fn available_list(available: &[&str]) -> String {
    available.join(", ")
}

/// Count results by code, for harness reporting.
#[must_use]
pub fn count_by_result(results: &[UnitResult]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for result in results {
        *counts.entry(result.get_result().to_string()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
#[path = "unit_config_tests.rs"]
mod unit_config_tests;
