//! Configuration-driven unit test engine.
//!
//! PHP reference: `src/unit/engine/ArcanistConfigurationDrivenUnitTestEngine.php`
//! (concrete, `final class`). Ports `supportsRunAllTests` (lines 6-16),
//! `buildTestEngines` (lines 18-115), `run` (lines 117-170),
//! `shouldEchoTestResults` (lines 172-174), and `matchPaths` (lines 186-218).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - `loadAvailableTestEngines` (lines 176-181) uses a class-map query; here
//!   the caller supplies the available engine names explicitly.
//! - `run` renders proxied results through `ArcanistUnitRenderer`
//!   (lines 151-157); rendering is out of scope, so this port only merges.
//!
//! `include`/`exclude` rules match with the shared PCRE engine
//! (`arclint::pcre_matches`, T1153 UNIT-U17), mirroring
//! `preg_match($rule, $path)` (line 195): invalid patterns never match,
//! like PHP's warn-and-`false`.

extern crate self as arc_rs;

use arc_rs::arclint;
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

/// A `PhutilProxyException` rendering (T1153 UNIT-U16): PHP prints the
/// wrapper plus each chain message on its own line
/// (`scripts/arcanist.php:449-468`), so the wrapper ends with a period
/// and the cause follows on the next line (same shape as the LINT-40
/// port; inner `PhutilTypeSpec` wordings live in unpinned libphutil, so
/// only the wrapper shape is exact).
fn proxy_failed(wrapper: String, detail: String) -> EngineError {
    EngineError::Failed(format!("{wrapper}.\n{detail}"))
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
        proxy_failed(
            format!(
                "Expected '.arcunit' file to be a valid JSON file, but failed to decode \
                 '{config_path}'"
            ),
            e.to_string(),
        )
    })?;
    let Some(entries) = config.get("engines").and_then(JsonValue::as_obj) else {
        return Err(proxy_failed(
            format!("Error in parsing '{config_path}' file"),
            "expected an 'engines' map".to_string(),
        ));
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
        if spec.as_obj().is_none() {
            return Err(proxy_failed(
                format!("Error in parsing '{config_path}' file"),
                format!("engine '{name}' must be a map"),
            ));
        }
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

    let context = format!("Error in parsing '.arcunit' file, for test engine '{name}'");
    let Some(engine_type) = engine_type else {
        return Err(proxy_failed(
            context,
            "expected a 'type' string".to_string(),
        ));
    };
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    for (key, value) in spec_obj {
        match key.as_str() {
            "type" => {}
            "include" => {
                include = parse_rule_list(value).ok_or_else(|| {
                    proxy_failed(
                        context.clone(),
                        "'include' must be a regex or a list of regexes".to_string(),
                    )
                })?;
            }
            "exclude" => {
                exclude = parse_rule_list(value).ok_or_else(|| {
                    proxy_failed(
                        context.clone(),
                        "'exclude' must be a regex or a list of regexes".to_string(),
                    )
                })?;
            }
            _ => {
                return Err(proxy_failed(
                    context.clone(),
                    format!("unknown key '{key}'"),
                ));
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

/// `preg_match($rule, $path)` (line 195) via the shared PCRE engine:
/// unanchored match; invalid patterns never match (PHP warns and returns
/// `false`). Patterns are validated at match time only, like PHP (the
/// LINT twin `arclint::route_paths` ports the same `matchPaths` copy).
#[must_use]
pub fn rule_matches(rule: &str, path: &str) -> bool {
    arclint::pcre_matches(rule, path)
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
