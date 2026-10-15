//! C# tools (cscover over xUnit) unit test engine.
//!
//! PHP reference: `src/unit/engine/CSharpToolsTestEngine.php` (concrete,
//! `final class`, extends `XUnitTestEngine`). Ports `loadEnvironment` (lines
//! 22-55), `assemblyShouldBeInstrumented` (lines 64-78), `buildTestFuture`
//! (lines 88-143), `parseCoverageResult` (lines 152-157),
//! `getCachedResultsIfPossible`/`addCachedResults` (lines 167-188),
//! `processTags` (lines 198-208), and `readCoverage` (lines 216-285).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - The `unit.csharp.coverage.match` regex uses the same ordered-substring
//!   matcher as the configuration-driven engine instead of
//!   `preg_match($this->matchRegex, $file)` (line 71); see there for limits.
//! - Coverage line counts come from a caller-supplied callback instead of
//!   `count(file($absolute_file))` (line 249).
//! - xUnit assembly-result parsing is duplicated here in miniature (parent
//!   `XUnitTestEngine` behavior) so this module depends only on the shared
//!   `unit_engine` base, never on a sibling engine file.
//! - The xUnit XML filename embeds caller-supplied randomness instead of
//!   `Filesystem::readRandomCharacters(10)` (line 96).

extern crate self as arc_rs;

use arc_rs::unit_engine::{
    EngineError, RESULT_FAIL, RESULT_PASS, RESULT_SKIP, RESULT_UNSOUND, UnitResult, find_elements,
};
use std::collections::HashMap;

/// cscover binary-hint config key (line 24).
pub const CSCOVER_BINARY_KEY: &str = "unit.csharp.cscover.binary";
/// Coverage match-regex config key (line 26).
pub const COVERAGE_MATCH_KEY: &str = "unit.csharp.coverage.match";
/// Coverage excluded-files config key (line 28).
pub const COVERAGE_EXCLUDED_KEY: &str = "unit.csharp.coverage.excluded";
/// `loadEnvironment` cscover half (lines 33-54): with coverage explicitly
/// disabled there is no cover engine (lines 33-35); a missing hint throws the
/// verbatim PHP error naming `unit.csharp.coverage.binary` (lines 39-44; note
/// the PHP quirk that the engine reads `unit.csharp.cscover.binary`); a
/// hint that resolves to nothing throws `pht('Unable to locate %s coverage
/// runner (have you built yet?)', 'cscover')` (lines 50-53). Returns the
/// resolved cover engine path, if any.
pub fn load_cover_engine(
    project_root: &str,
    cscover_hint_path: Option<&str>,
    enable_coverage: Option<bool>,
    file_exists: &dyn Fn(&str) -> bool,
) -> Result<Option<String>, EngineError> {
    if enable_coverage == Some(false) {
        return Ok(None);
    }
    let Some(hint) = cscover_hint_path else {
        return Err(EngineError::Failed(
            "Unable to locate cscover. Configure it with the 'unit.csharp.coverage.binary' \
             option in .arcconfig."
                .to_string(),
        ));
    };
    let candidate = format!("{project_root}/{hint}");
    if file_exists(&candidate) {
        Ok(Some(candidate))
    } else {
        Err(EngineError::Failed(
            "Unable to locate cscover coverage runner (have you built yet?)".to_string(),
        ))
    }
}

/// Ordered-substring matcher standing in for the coverage-match
/// `preg_match` (line 71). Tiny duplicate of the configuration-driven
/// engine's matcher (kept local so this module never depends on a sibling).
#[must_use]
pub fn match_rule(rule: &str, file: &str) -> bool {
    let mut body = rule;
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
            body = &body[1..last];
        }
    }
    let literal = body.replace("\\/", "/");
    let literal = literal.strip_prefix('^').unwrap_or(&literal);
    let literal = literal.strip_suffix('$').unwrap_or(literal);
    if literal.is_empty() {
        return true;
    }
    file.contains(literal)
}

/// `assemblyShouldBeInstrumented` (lines 64-78): excluded files are skipped
/// (lines 65-69); with a match regex only matching assemblies qualify
/// (lines 70-76); otherwise everything qualifies (line 77).
#[must_use]
pub fn assembly_should_be_instrumented(
    file: &str,
    excluded_files: Option<&HashMap<String, String>>,
    match_regex: Option<&str>,
) -> bool {
    if let Some(excluded) = excluded_files
        && excluded.contains_key(file)
    {
        return false;
    }
    if let Some(regex) = match_regex {
        return match_rule(regex, file);
    }
    true
}

/// Assembly filename check (line 120): names ending in `.dll` or `.exe`.
#[must_use]
pub fn is_assembly_file(name: &str) -> bool {
    name.ends_with(".dll") || name.ends_with(".exe")
}

/// Assembly scan (lines 117-125): `.dll`/`.exe` entries of the assembly
/// directory that should be instrumented, as `<dir>/<file>` paths.
#[must_use]
pub fn collect_assemblies(
    assembly_dir: &str,
    dir_files: &[&str],
    excluded_files: Option<&HashMap<String, String>>,
    match_regex: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    for file in dir_files {
        if (is_assembly_file(file))
            && assembly_should_be_instrumented(file, excluded_files, match_regex)
        {
            out.push(format!("{assembly_dir}/{file}"));
        }
    }
    out
}

/// xUnit command selection (lines 102-116): with an empty runtime engine the
/// test engine is the command with `<assembly> /xml <xml>` args; otherwise
/// the runtime is the command with `<engine> <assembly> /xml <xml>` args.
#[must_use]
pub fn xunit_cmd_and_args(
    runtime_engine: &str,
    test_engine: &str,
    assembly: &str,
    xml_name: &str,
) -> (String, Vec<String>) {
    if runtime_engine.is_empty() {
        (
            test_engine.to_string(),
            vec![
                assembly.to_string(),
                "/xml".to_string(),
                xml_name.to_string(),
            ],
        )
    } else {
        (
            runtime_engine.to_string(),
            vec![
                test_engine.to_string(),
                assembly.to_string(),
                "/xml".to_string(),
                xml_name.to_string(),
            ],
        )
    }
}

/// cscover argv (lines 129-136): `<runtime> <cover> -o <out> -c <cmd> -a
/// <args...> -w <dir> <assemblies...>`; an empty runtime is omitted.
#[must_use]
pub fn build_cscover_argv(
    runtime_engine: &str,
    cover_engine: &str,
    cover_out: &str,
    xunit_cmd: &str,
    xunit_args: &[String],
    assembly_dir: &str,
    assemblies: &[String],
) -> Vec<String> {
    let mut argv = Vec::new();
    if !runtime_engine.is_empty() {
        argv.push(runtime_engine.to_string());
    }
    argv.push(cover_engine.to_string());
    argv.push("-o".to_string());
    argv.push(cover_out.to_string());
    argv.push("-c".to_string());
    argv.push(xunit_cmd.to_string());
    argv.push("-a".to_string());
    argv.extend(xunit_args.iter().cloned());
    argv.push("-w".to_string());
    argv.push(assembly_dir.to_string());
    argv.extend(assemblies.iter().cloned());
    argv
}

/// Whether the cscover path applies (lines 89-91, 126-128): coverage must be
/// enabled and at least one assembly must qualify; otherwise the plain xUnit
/// path runs.
#[must_use]
pub fn use_cover_engine(enable_coverage: Option<bool>, assemblies: &[String]) -> bool {
    enable_coverage != Some(false) && !assemblies.is_empty()
}

/// One coverage span: file plus 1-based start/end lines (`processTags`,
/// lines 198-208).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageSpan {
    /// Absolute file path (`file` attribute).
    pub file: String,
    /// Start line (`start` attribute).
    pub start: u32,
    /// End line (`end` attribute).
    pub end: u32,
}

/// `processTags` (lines 198-208): read `file`/`start`/`end` attributes.
/// Spans missing any attribute are skipped.
#[must_use]
pub fn process_tags(xml: &str, tag: &str) -> Vec<CoverageSpan> {
    let mut out = Vec::new();
    for element in find_elements(xml, tag) {
        let file = element.attr("file");
        let start = element.attr("start").and_then(|s| s.parse::<u32>().ok());
        let end = element.attr("end").and_then(|s| s.parse::<u32>().ok());
        if let (Some(file), Some(start), Some(end)) = (file, start, end) {
            out.push(CoverageSpan {
                file: file.to_string(),
                start,
                end,
            });
        }
    }
    out
}

/// `readCoverage` (lines 216-285): `instrumented` spans mark lines `U`
/// (lines 256-266), `executed` spans mark lines `C` (lines 268-278), over an
/// all-`N` report sized by the file's line count (lines 248-254); files are
/// keyed relative to the project root (line 238). Note the fetched-but-unused
/// `$modified` paths (line 225). Spans outside the file are clipped.
pub fn read_cscover_coverage(
    cover_xml: &str,
    project_root: &str,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<HashMap<String, String>, EngineError> {
    let instrumented = process_tags(cover_xml, "instrumented");
    let executed = process_tags(cover_xml, "executed");

    let mut files: Vec<String> = Vec::new();
    for span in &instrumented {
        let relative = relative_to_root(&span.file, project_root);
        if !files.contains(&relative) {
            files.push(relative);
        }
    }

    let mut reports = HashMap::new();
    for file in &files {
        let absolute = format!("{project_root}/{file}");
        let Some(line_count) = line_count_of(&absolute) else {
            continue;
        };
        let mut coverage = vec![b'N'; line_count];
        for span in &instrumented {
            if relative_to_root(&span.file, project_root) != *file {
                continue;
            }
            paint_span(&mut coverage, span.start, span.end, b'U');
        }
        for span in &executed {
            if relative_to_root(&span.file, project_root) != *file {
                continue;
            }
            paint_span(&mut coverage, span.start, span.end, b'C');
        }
        reports.insert(file.clone(), String::from_utf8_lossy(&coverage).to_string());
    }
    Ok(reports)
}

fn paint_span(coverage: &mut [u8], start: u32, end: u32, mark: u8) {
    if start == 0 {
        return;
    }
    let from = (start - 1) as usize;
    let to = (end as usize).min(coverage.len());
    if from >= coverage.len() || to <= from {
        return;
    }
    for slot in &mut coverage[from..to] {
        *slot = mark;
    }
}

/// Relativize an absolute coverage path (line 238):
/// `substr($absolute_file, strlen($this->projectRoot) + 1)`.
#[must_use]
pub fn relative_to_root(absolute: &str, project_root: &str) -> String {
    let prefix = format!("{project_root}/");
    absolute
        .strip_prefix(&prefix)
        .unwrap_or(absolute)
        .replace('\\', "/")
}

/// `parseCoverageResult` (lines 152-157): with coverage disabled there is no
/// report (parent returns null); otherwise read the cscover file.
pub fn parse_coverage_result(
    cover_xml: Option<&str>,
    enable_coverage: Option<bool>,
    project_root: &str,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<Option<HashMap<String, String>>, EngineError> {
    if enable_coverage == Some(false) {
        return Ok(None);
    }
    match cover_xml {
        Some(xml) => read_cscover_coverage(xml, project_root, line_count_of).map(Some),
        None => Ok(None),
    }
}

/// Coverage cache mirroring `getCachedResultsIfPossible` (lines 167-175) and
/// `addCachedResults` (lines 183-188): reports keyed by coverage-file name.
#[derive(Debug, Default)]
pub struct CoverageCache {
    entries: HashMap<String, HashMap<String, String>>,
}

impl CoverageCache {
    /// Empty cache (`$this->cachedResults` starts null, line 168).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `getCachedResultsIfPossible`: cached reports for a coverage file.
    #[must_use]
    pub fn get_cached(&self, cover_file: &str) -> Option<&HashMap<String, String>> {
        self.entries.get(cover_file)
    }

    /// `addCachedResults`: store reports for a coverage file.
    pub fn add_cached(&mut self, cover_file: &str, reports: HashMap<String, String>) {
        self.entries.insert(cover_file.to_string(), reports);
    }

    /// Cached read: serve from the cache when possible, else parse, store,
    /// and return (lines 217-220, 283-284).
    pub fn read_cached(
        &mut self,
        cover_file: &str,
        cover_xml: &str,
        project_root: &str,
        line_count_of: &dyn Fn(&str) -> Option<usize>,
    ) -> Result<HashMap<String, String>, EngineError> {
        if let Some(cached) = self.get_cached(cover_file) {
            return Ok(cached.clone());
        }
        let reports = read_cscover_coverage(cover_xml, project_root, line_count_of)?;
        self.add_cached(cover_file, reports.clone());
        Ok(reports)
    }
}

/// Parent xUnit assembly-result parsing, duplicated in miniature from the
/// xUnit engine (the PHP class inherits it): `<test name time result>` with
/// `Pass`/`Fail`/`Skip` mapping, defaulting to unsound.
pub fn parse_assembly_results(
    xunit_xml: &str,
    coverage: Option<&HashMap<String, String>>,
) -> Result<Vec<UnitResult>, EngineError> {
    let mut results = Vec::new();
    for test in find_elements(xunit_xml, "test") {
        let status = match test.attr("result") {
            Some("Pass") => RESULT_PASS,
            Some("Fail") => RESULT_FAIL,
            Some("Skip") => RESULT_SKIP,
            _ => RESULT_UNSOUND,
        };
        let mut result = UnitResult::new();
        result
            .set_name(test.attr("name").unwrap_or(""))
            .map_err(EngineError::Failed)?;
        result.set_result(status);
        result.set_duration(
            test.attr("time")
                .and_then(|t| t.parse::<f64>().ok())
                .unwrap_or(0.0),
        );
        if let Some(coverage) = coverage {
            result.set_coverage(coverage.clone());
        }
        results.push(result);
    }
    Ok(results)
}

#[cfg(test)]
#[path = "unit_csharp_tests.rs"]
mod unit_csharp_tests;
