//! PHPUnit wrapper engine and result parser.
//!
//! PHP references: `src/unit/engine/PhpunitTestEngine.php` (concrete,
//! `final class`) and `src/unit/parser/ArcanistPhpunitTestResultParser.php`
//! (concrete, `final class`). Ports `run` path selection (lines 13-90),
//! `parseTestResults` delegation (lines 102-111), `findTestFile` (lines
//! 124-156), `getSearchLocationsForTests` (lines 198-248), and
//! `prepareConfigFile` (lines 254-278); plus parser `parseTestResults` (parser
//! lines 19-103), `readCoverage` (parser lines 110-162), and `getJsonReport`
//! (parser lines 171-186).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - Test futures run sequentially instead of through `FutureIterator`
//!   limited to 4 (engine lines 76-77); observable results are identical.
//! - Search locations use `/` separators; PHP uses `DIRECTORY_SEPARATOR`
//!   (engine lines 221-244). Backslashes in inputs are normalized to `/`.
//! - Coverage line counts come from a caller-supplied callback instead of
//!   `count(file($class_path))` (parser line 129).
//! - The PHP runner silently skips missing test files after selection
//!   (engine lines 52-54); this port resolves through the same existence
//!   probe instead.

extern crate self as arc_rs;

use arc_rs::unit_engine::{
    CommandRunner, EngineContext, EngineError, JsonValue, RESULT_BROKEN, RESULT_FAIL, RESULT_PASS,
    RESULT_SKIP, TempFile, UnitResult, parse_json,
};
use std::collections::HashMap;

/// Default binary (`private $phpunitBinary = 'phpunit'`, engine line 9).
pub const PHPUNIT_BINARY: &str = "phpunit";
/// Config key for the phpunit XML file (engine line 257).
pub const CONFIG_KEY: &str = "phpunit_config";
/// Config key for the phpunit binary (engine line 270).
pub const BINARY_KEY: &str = "unit.phpunit.binary";
/// `display_errors` flag passed to every run (engine line 65).
pub const STDERR_FLAG: &str = "-d display_errors=stderr";

/// PHPUnit engine configuration: explicit binary/config paths plus run inputs.
#[derive(Debug, Clone)]
pub struct PhpunitEngine {
    /// phpunit binary path (default [`PHPUNIT_BINARY`]).
    pub binary: String,
    /// Resolved phpunit XML config, if any (`prepareConfigFile`).
    pub config_file: Option<String>,
}

impl PhpunitEngine {
    /// Engine with the default binary and no config file.
    #[must_use]
    pub fn new() -> Self {
        Self {
            binary: PHPUNIT_BINARY.to_string(),
            config_file: None,
        }
    }

    /// Engine with explicit binary and config paths.
    #[must_use]
    pub fn with_binary_and_config(binary: &str, config_file: Option<&str>) -> Self {
        Self {
            binary: binary.to_string(),
            config_file: config_file.map(str::to_string),
        }
    }

    /// `getEngineConfigurationName`: phpunit defines none, so `None`
    /// (inherits `ArcanistUnitTestEngine.php:17-19`).
    #[must_use]
    pub fn engine_configuration_name(&self) -> Option<&'static str> {
        None
    }

    /// `supportsRunAllTests`: phpunit does not override it, so false
    /// (inherits `ArcanistUnitTestEngine.php:38-40`).
    #[must_use]
    pub fn supports_run_all_tests(&self) -> bool {
        false
    }

    /// `run` (engine lines 13-90) with injected command runner and filesystem
    /// probes.
    pub fn run_with(
        &self,
        ctx: &EngineContext,
        runner: &dyn CommandRunner,
        fs: &dyn PhpunitFs,
    ) -> Result<Vec<UnitResult>, EngineError> {
        let affected = select_affected_tests(
            &ctx.paths,
            &ctx.project_root,
            &|path| fs.is_dir(path),
            &|path| fs.path_exists(path),
        )?;
        let mut merged = Vec::new();
        for (class_path, test_path) in &affected {
            let _ = class_path;
            if !fs.path_exists(test_path) {
                continue;
            }
            let json_tmp =
                TempFile::create("phpunit-json", ".json").map_err(EngineError::Failed)?;
            let json_path = json_tmp.path().to_string_lossy().to_string();
            let cover_tmp = if ctx.enable_coverage == Some(false) {
                None
            } else {
                Some(TempFile::create("phpunit-clover", ".xml").map_err(EngineError::Failed)?)
            };
            let cover_path = cover_tmp
                .as_ref()
                .map(|t| t.path().to_string_lossy().to_string());
            let argv = build_phpunit_argv(
                &self.binary,
                self.config_file.as_deref(),
                &json_path,
                cover_path.as_deref(),
                test_path,
            );
            let output = runner.run(&argv, &ctx.project_root);
            let json_text = fs
                .read_file(&json_path)
                .ok_or_else(|| EngineError::Failed(format!("cannot read '{json_path}'")))?;
            let clover_xml = cover_path.as_deref().and_then(|p| fs.read_file(p));
            let mut results = parse_phpunit_results(
                test_path,
                &json_text,
                clover_xml.as_deref(),
                &output.stderr,
                ctx.enable_coverage,
                &ctx.project_root,
                &affected,
                &|path| fs.line_count(path),
            )?;
            merged.append(&mut results);
        }
        Ok(merged)
    }
}

impl Default for PhpunitEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Filesystem seam for the phpunit engine.
pub trait PhpunitFs {
    /// `is_dir($path)` (engine line 22).
    fn is_dir(&self, path: &str) -> bool;
    /// `Filesystem::pathExists(...)` (engine lines 52, 139, 260).
    fn path_exists(&self, path: &str) -> bool;
    /// Read a report or temp file.
    fn read_file(&self, path: &str) -> Option<String>;
    /// `count(file($class_path))` (parser line 129).
    fn line_count(&self, path: &str) -> Option<usize>;
}

/// `run` path selection (engine lines 13-46): resolve each path, skip
/// directories (lines 22-24) and non-`.php` files (lines 28-30), keep
/// `*Test.php` files directly (lines 32-36), otherwise locate the test with
/// [`find_test_file`] (lines 38-40). Empty selection throws no-effect
/// (lines 44-46).
pub fn select_affected_tests(
    paths: &[String],
    project_root: &str,
    is_dir: &dyn Fn(&str) -> bool,
    path_exists: &dyn Fn(&str) -> bool,
) -> Result<HashMap<String, String>, EngineError> {
    let mut affected = HashMap::new();
    for path in paths {
        let resolved = join_root(project_root, path);
        if is_dir(&resolved) {
            continue;
        }
        if !resolved.ends_with(".php") {
            continue;
        }
        if resolved.ends_with("Test.php") {
            affected.insert(resolved.clone(), resolved);
            continue;
        }
        if let Some(test) = find_test_file(&resolved, project_root, path_exists) {
            affected.insert(resolved, test);
        }
    }
    if affected.is_empty() {
        return Err(EngineError::no_effect());
    }
    Ok(affected)
}

/// `findTestFile` (engine lines 124-156): probe `<file>` and `<base>Test.php`
/// in every search location, skipping missing files (lines 139-142), files
/// above the project root (lines 143-146), and the input file itself
/// (case-insensitive, lines 147-150).
#[must_use]
pub fn find_test_file(
    path: &str,
    project_root: &str,
    path_exists: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let root = normalize(path);
    let project_root = normalize(project_root);
    let file = file_name(&root);
    let stem = file.strip_suffix(".php").unwrap_or(&file);
    let candidates = [file.clone(), format!("{stem}Test.php")];
    for dir in get_search_locations_for_tests(&root) {
        for candidate in &candidates {
            let full = format!("{dir}{candidate}");
            if !path_exists(&full) {
                continue;
            }
            if !is_descendant(&full, &project_root) {
                continue;
            }
            if full.to_lowercase() == root.to_lowercase() {
                continue;
            }
            return Some(full);
        }
    }
    None
}

/// `getSearchLocationsForTests` (engine lines 198-248): the file's own
/// directory, then `tests/`/`Tests/` in every ancestor (lines 210-218), then
/// each path component replaced by `tests/`/`Tests/` (lines 221-230), then
/// `tests/`/`Tests/` inserted at each level (lines 233-240); deduplicated
/// (lines 242-247).
#[must_use]
pub fn get_search_locations_for_tests(path: &str) -> Vec<String> {
    let normal = normalize(path);
    let dir = dir_name(&normal);
    let test_dirs = ["tests", "Tests"];
    let mut tries: Vec<Vec<String>> = Vec::new();

    tries.push(vec![dir.clone()]);

    for parent in walk_to_root(&dir) {
        let base = if parent == "/" { String::new() } else { parent };
        for test_dir in &test_dirs {
            tries.push(vec![base.clone(), (*test_dir).to_string()]);
        }
    }

    let parts: Vec<String> = dir
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    for key in (0..parts.len()).rev() {
        for test_dir in &test_dirs {
            let mut attempt = parts.clone();
            attempt[key] = (*test_dir).to_string();
            let mut full = vec![String::new()];
            full.extend(attempt);
            tries.push(full);
        }
    }
    for key in (0..parts.len()).rev() {
        for test_dir in &test_dirs {
            let mut attempt = parts.clone();
            attempt[key] = format!("{test_dir}/{}", attempt[key]);
            let mut full = vec![String::new()];
            full.extend(attempt);
            tries.push(full);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();
    for parts in tries {
        let joined = format!("{}/", parts.join("/"));
        if seen.insert(joined.clone()) {
            results.push(joined);
        }
    }
    results
}

/// Ancestor chain from `dir` up to (and including) `/`.
fn walk_to_root(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = dir.to_string();
    loop {
        out.push(current.clone());
        if current == "/" || current.is_empty() {
            break;
        }
        match current.rfind('/') {
            Some(0) => current = "/".to_string(),
            Some(i) => current = current[..i].to_string(),
            None => {
                current = "/".to_string();
            }
        }
    }
    out
}

/// `prepareConfigFile` config half (engine lines 254-268): resolve the
/// `phpunit_config` value against the project root; a configured-but-missing
/// file throws `pht('PHPUnit configuration file was not found in %s', ...)`
/// (engine lines 263-266).
pub fn resolve_config_file(
    project_root: &str,
    config_value: Option<&str>,
    path_exists: &dyn Fn(&str) -> bool,
) -> Result<Option<String>, EngineError> {
    match config_value {
        None => Ok(None),
        Some(config) => {
            let full = format!("{project_root}/{config}");
            if path_exists(&full) {
                Ok(Some(full))
            } else {
                Err(EngineError::Failed(format!(
                    "PHPUnit configuration file was not found in {full}"
                )))
            }
        }
    }
}

/// `prepareConfigFile` binary half (engine lines 269-277): a configured
/// `unit.phpunit.binary` that exists on PATH is used as-is, otherwise it is
/// resolved against the project root.
#[must_use]
pub fn resolve_phpunit_binary(
    project_root: &str,
    configured: Option<&str>,
    binary_exists: &dyn Fn(&str) -> bool,
) -> String {
    match configured {
        None => PHPUNIT_BINARY.to_string(),
        Some(bin) if binary_exists(bin) => bin.to_string(),
        Some(bin) => join_root(project_root, bin),
    }
}

/// Run argv (engine lines 63-68): `<php> [-c <config>] -d
/// display_errors=stderr --log-json <json> [--coverage-clover <clover>]
/// <test>`.
#[must_use]
pub fn build_phpunit_argv(
    binary: &str,
    config_file: Option<&str>,
    json_tmp: &str,
    clover_tmp: Option<&str>,
    test_path: &str,
) -> Vec<String> {
    let mut argv = vec![binary.to_string()];
    if let Some(config) = config_file {
        argv.push("-c".to_string());
        argv.push(config.to_string());
    }
    argv.push(STDERR_FLAG.to_string());
    argv.push("--log-json".to_string());
    argv.push(json_tmp.to_string());
    if let Some(clover) = clover_tmp {
        argv.push("--coverage-clover".to_string());
        argv.push(clover.to_string());
    }
    argv.push(test_path.to_string());
    argv
}

/// Parser `parseTestResults` (parser lines 19-103): parse the phpunit JSON
/// report for one test path. Empty reports yield one broken result named for
/// the path with stderr details (parser lines 20-26); a trailing `testStart`
/// without completion yields a broken result named for the last event with
/// stderr details (parser lines 96-101).
// Eight inputs mirror the PHP parser state (`path`, report, clover, stderr,
// coverage flag, project root, affected tests, line counts); bundling them
// would obscure the one-to-one port.
#[allow(clippy::too_many_arguments)]
pub fn parse_phpunit_results(
    path: &str,
    test_results: &str,
    clover_xml: Option<&str>,
    stderr: &str,
    enable_coverage: Option<bool>,
    project_root: &str,
    affected_tests: &HashMap<String, String>,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<Vec<UnitResult>, EngineError> {
    if test_results.is_empty() {
        let mut result = UnitResult::new();
        result.set_name(path).map_err(EngineError::Failed)?;
        result.set_user_data(stderr);
        result.set_result(RESULT_BROKEN);
        return Ok(vec![result]);
    }
    let report = get_json_report(test_results)?;
    let coverage = if enable_coverage == Some(false) {
        Vec::new()
    } else {
        read_clover_coverage_ordered(
            clover_xml.unwrap_or(""),
            project_root,
            affected_tests,
            line_count_of,
        )
    };

    let mut results = Vec::new();
    let mut last_test_finished = true;
    let mut last_event: Option<&JsonValue> = None;
    let empty: &[JsonValue] = &[];
    let events = match &report {
        JsonValue::Arr(items) => items.as_slice(),
        _ => empty,
    };
    for event in events {
        last_event = Some(event);
        match event.get("event").and_then(JsonValue::as_str) {
            Some("test") => {}
            Some("testStart") => {
                last_test_finished = false;
                continue;
            }
            _ => continue,
        }
        results.push(build_phpunit_event_result(event, &coverage)?);
        last_test_finished = true;
    }

    if !last_test_finished && let Some(event) = last_event {
        let name = event.get("test").and_then(JsonValue::as_str).unwrap_or("");
        let mut result = UnitResult::new();
        result.set_name(name).map_err(EngineError::Failed)?;
        result.set_user_data(stderr);
        result.set_result(RESULT_BROKEN);
        results.push(result);
    }
    Ok(results)
}

/// Build one result from a `test` event: the stripped test name, status
/// and user data from [`phpunit_event_outcome`], duration, and the shared
/// clover coverage map.
fn build_phpunit_event_result(
    event: &JsonValue,
    coverage: &[(String, String)],
) -> Result<UnitResult, EngineError> {
    let (status, user_data) = phpunit_event_outcome(event);
    let raw_name = event.get("test").and_then(JsonValue::as_str).unwrap_or("");
    let name = strip_paren_suffix(raw_name);
    let mut result = UnitResult::new();
    result.set_name(&name).map_err(EngineError::Failed)?;
    result.set_result(status);
    result.set_duration(event.get("time").and_then(JsonValue::as_num).unwrap_or(0.0));
    result.set_coverage_ordered(coverage.to_vec());
    result.set_user_data(&user_data);
    Ok(result)
}

/// Resolve a `test` event's status and user data: `fail` gains the message
/// plus trace, `error` with a skip/incomplete message becomes a skip,
/// otherwise an `error` is broken with the message plus trace.
fn phpunit_event_outcome(event: &JsonValue) -> (&'static str, String) {
    let mut status = RESULT_PASS;
    let mut user_data = String::new();
    match event.get("status").and_then(JsonValue::as_str) {
        Some("fail") => {
            status = RESULT_FAIL;
            user_data.push_str(
                event
                    .get("message")
                    .and_then(JsonValue::as_str)
                    .unwrap_or(""),
            );
            user_data.push('\n');
            user_data.push_str(&format_phpunit_trace(event));
        }
        Some("error") => {
            let message = event
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            if message.contains("Skipped Test") || message.contains("Incomplete Test") {
                status = RESULT_SKIP;
                user_data.push_str(message);
            } else {
                status = RESULT_BROKEN;
                user_data.push_str(message);
                user_data.push_str(&format_phpunit_trace(event));
            }
        }
        _ => {}
    }
    (status, user_data)
}

/// Format a `test` event's trace frames as `\nfile:line` lines.
fn format_phpunit_trace(event: &JsonValue) -> String {
    let mut out = String::new();
    for trace in event
        .get("trace")
        .and_then(JsonValue::as_arr)
        .unwrap_or(&[])
    {
        let file = trace.get("file").and_then(JsonValue::as_str).unwrap_or("");
        let line = trace_number(trace.get("line"));
        out.push_str(&format!("\n{file}:{line}"));
    }
    out
}

fn trace_number(value: Option<&JsonValue>) -> String {
    match value {
        Some(JsonValue::Num(n)) => format!("{}", *n as i64),
        Some(JsonValue::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Strip the ` (...)` suffix from phpunit test names
/// (`preg_replace('/ \(.*\)/s', '', ...)`, parser line 83).
fn strip_paren_suffix(name: &str) -> String {
    match name.find(" (") {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

/// `getJsonReport` (parser lines 171-186): phpunit emits concatenated JSON
/// objects, so `}{` boundaries (with optional whitespace and quote, line 183)
/// are repaired with a comma and the whole is wrapped in `[...]`. Empty input
/// throws the PHP guidance error (parser lines 174-180).
pub fn get_json_report(json: &str) -> Result<JsonValue, EngineError> {
    if json.is_empty() {
        return Err(EngineError::Failed(
            "JSON report file is empty, it probably means that phpunit failed to run tests. Try \
             running arc unit with --trace option and then run generated phpunit command yourself, \
             you might get the answer."
                .to_string(),
        ));
    }
    let repaired = repair_concatenated_json(json);
    let wrapped = format!("[{repaired}]");
    parse_json(&wrapped).map_err(EngineError::Failed)
}

/// Insert commas at `}<ws>"` object boundaries (parser line 183). As a
/// documented extension, `}<ws>{` boundaries are split too: phpunit emits one
/// JSON object per event, so consecutive `}{` pairs are the norm and PHP's
/// `}"`-only repair would leave them unparseable.
fn repair_concatenated_json(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let mut out = String::with_capacity(json.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '}' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '"' || chars[j] == '{') {
                out.push('}');
                out.push(',');
                out.extend(chars[i + 1..j].iter());
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Parser `readCoverage` (parser lines 110-162): clover `<file name>` tags
/// filtered to selected tests (parser lines 124-126); only `stmt` lines count
/// (parser lines 138-140); the report starts as all-`N` (parser line 136) with
/// covered/uncovered marks overlaid (parser lines 141-149); files with no
/// covered line are dropped as false positives (parser lines 154-158) and
/// names are relativized against the project root (parser lines 155-157).
/// Empty input yields no reports (parser lines 112-114).
/// Legacy unordered form; the parse path uses
/// [`read_clover_coverage_ordered`].
#[must_use]
pub fn read_clover_coverage(
    clover_xml: &str,
    project_root: &str,
    affected_tests: &HashMap<String, String>,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> HashMap<String, String> {
    read_clover_coverage_ordered(clover_xml, project_root, affected_tests, line_count_of)
        .into_iter()
        .collect()
}

/// [`read_clover_coverage`] in document order (UNIT-U20): reports follow
/// the `<file>` elements like PHP's insertion-ordered array.
#[must_use]
pub fn read_clover_coverage_ordered(
    clover_xml: &str,
    project_root: &str,
    affected_tests: &HashMap<String, String>,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> Vec<(String, String)> {
    let mut reports = Vec::new();
    if clover_xml.is_empty() {
        return reports;
    }
    for file in arc_rs::unit_engine::find_elements(clover_xml, "file") {
        let Some(class_path) = file.attr("name") else {
            continue;
        };
        if !affected_tests.contains_key(class_path) {
            continue;
        }
        let Some(line_count) = line_count_of(class_path) else {
            continue;
        };
        if let Some(marks) = clover_file_marks(&file.inner, line_count) {
            let prefix = format!("{}/", normalize(project_root));
            let relative = normalize(class_path)
                .strip_prefix(&prefix)
                .unwrap_or(class_path)
                .to_string();
            reports.push((relative, String::from_utf8_lossy(&marks).to_string()));
        }
    }
    reports
}

/// Per-line coverage marks for one clover `<file>`: the report starts as
/// all-`N` with covered (`C`) and uncovered (`U`) `stmt` lines overlaid.
/// `None` when no line is covered (a false positive, dropped like PHP).
fn clover_file_marks(inner: &str, line_count: usize) -> Option<Vec<u8>> {
    let mut coverage = vec![b'N'; line_count];
    let mut any_covered = false;
    for line in arc_rs::unit_engine::find_elements(inner, "line") {
        if line.attr("type") != Some("stmt") {
            continue;
        }
        let count = line
            .attr("count")
            .and_then(|c| c.parse::<i64>().ok())
            .unwrap_or(0);
        let num = line
            .attr("num")
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        if num == 0 || num > line_count {
            continue;
        }
        if count > 0 {
            coverage[num - 1] = b'C';
            any_covered = true;
        } else {
            coverage[num - 1] = b'U';
        }
    }
    if any_covered { Some(coverage) } else { None }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn dir_name(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn join_root(root: &str, path: &str) -> String {
    if path.starts_with('/') || path.contains(':') {
        return normalize(path);
    }
    format!(
        "{}/{}",
        normalize(root).trim_end_matches('/'),
        normalize(path)
    )
}

fn is_descendant(path: &str, root: &str) -> bool {
    let base = root.trim_end_matches('/');
    path == base || path.starts_with(&format!("{base}/"))
}

#[cfg(test)]
#[path = "unit_phpunit_tests.rs"]
mod unit_phpunit_tests;
