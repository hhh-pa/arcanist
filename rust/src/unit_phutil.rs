//! Phutil (libphutil test framework) unit test engine.
//!
//! PHP reference: `src/unit/engine/PhutilUnitTestEngine.php` (concrete,
//! `final class`). Ports `getEngineConfigurationName` (lines 8-10),
//! `supportsRunAllTests` (lines 12-14), `run` selection (lines 16-25),
//! the coverage gate (lines 27-43), `getAllTests` filtering (lines 80-112),
//! `getTestsForPaths` collection (lines 128-150), and `getTestPaths` (lines
//! 157-220).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - Test execution itself is in-process PHP (`newv($test_class)` plus
//!   `willRunTestCases`/`run`/`didRunTestCases` hooks, lines 45-77) with
//!   XDebug coverage. This port shells out to the user's `php` binary
//!   (like anoid's `python3`) running a staged driver script that mirrors
//!   selection plus execution; see [`phutil_driver_source`].
//! - `getAllTests`/`getTestsForPaths` use `PhutilSymbolLoader` (lines 83-87,
//!   136-142); the selection helpers below take caller-supplied symbols and
//!   apply the same `__tests__` and working-copy filters, while the staged
//!   driver uses the real symbol loader in-process.
//! - Library roots/names come from caller-supplied callbacks instead of
//!   `phutil_get_library_root_for_path` / `phutil_get_library_name_for_root`
//!   (lines 162-168).
//! - The driver cannot build arc's `WorkingCopy` object out-of-process, so
//!   per-test links and XDebug coverage maps (which need the project root
//!   via `getWorkingCopy()`) are unavailable unless the init script loads
//!   arc's PHP: forced `--coverage` without it is a clear error, and the
//!   default run proceeds uncovered.
//!
//! Out-of-process protocol: the driver reads a JSON input file
//! ([`PhutilRunInput`]), writes `{"results": [<toDictionary>...]}` or
//! `{"error": "..."}` to a JSON output file, and exits 0 whenever it wrote
//! output. The input's `initScript` (`.arcconfig`
//! [`INIT_SCRIPT_KEY`]) is a PHP file that loads the project's libraries;
//! without it the driver reports an error instead of running.

extern crate self as arc_rs;

use arc_rs::unit_engine::{
    EngineError, RESULT_BROKEN, RESULT_FAIL, RESULT_PASS, RESULT_SKIP, RESULT_UNSOUND, UnitResult,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// `getEngineConfigurationName` (lines 8-10) returns `'phutil'`.
pub const ENGINE_CONFIGURATION_NAME: &str = "phutil";

/// `supportsRunAllTests` (lines 12-14) returns true.
pub const SUPPORTS_RUN_ALL_TESTS: bool = true;

/// Directory marker for test locations (lines 93, 194, 206, 213).
pub const TESTS_DIR: &str = "__tests__";

/// One test search location: a library plus a library-relative test path
/// (lines 195-198, 204-207, 211-214).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPath {
    /// Library name.
    pub library: String,
    /// Library-relative path (`__tests__/`-suffixed, or a file under one).
    pub path: String,
}

/// Test-class selection for a run: either every test or the path-derived set
/// (lines 16-25). Empty selection throws no-effect (lines 23-25).
pub fn select_tests(
    run_all_tests: bool,
    all_tests: Vec<String>,
    tests_for_paths: Vec<String>,
) -> Result<Vec<String>, EngineError> {
    let run_tests = if run_all_tests {
        all_tests
    } else {
        tests_for_paths
    };
    if run_tests.is_empty() {
        return Err(EngineError::no_effect());
    }
    Ok(run_tests)
}

/// Coverage gate (lines 27-43): with coverage not disabled and XDebug present,
/// coverage turns on (line 41); with coverage forced on but XDebug missing,
/// throw `pht('You specified %s but %s is not available, so coverage can not
/// be enabled for %s.', '--coverage', 'XDebug', 'PhutilUnitTestEngine')`
/// (lines 32-38).
pub fn resolve_coverage(
    enable_coverage: Option<bool>,
    has_xdebug: bool,
) -> Result<Option<bool>, EngineError> {
    if enable_coverage == Some(false) {
        return Ok(enable_coverage);
    }
    if !has_xdebug {
        if enable_coverage == Some(true) {
            return Err(EngineError::Usage(
                "You specified --coverage but XDebug is not available, so coverage can not be \
                 enabled for PhutilUnitTestEngine."
                    .to_string(),
            ));
        }
        return Ok(enable_coverage);
    }
    Ok(Some(true))
}

/// `getAllTests` filter (lines 89-110): keep symbols declared under a
/// `__tests__/` path segment (line 93) whose library root sits in the working
/// copy (lines 99-104).
#[must_use]
pub fn symbol_should_run(symbol_where: &str, library_root: &str, project_root: &str) -> bool {
    if !is_in_tests_dir(symbol_where) {
        return false;
    }
    is_descendant(library_root, project_root)
}

/// `getTestsForPaths` collection (lines 128-150): union symbol names across
/// search locations, deduplicated (lines 144-146, 149).
#[must_use]
pub fn collect_test_names(symbols_per_path: &[Vec<String>]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for symbols in symbols_per_path {
        for symbol in symbols {
            if seen.insert(symbol.clone()) {
                names.push(symbol.clone());
            }
        }
    }
    names
}

/// `getTestPaths` (lines 157-220): for every affected path, resolve its
/// library (skipping paths in no library, lines 164-166; throwing the
/// verbatim error for unloaded libraries, lines 170-181), skip symlink-maze
/// paths outside their library (lines 186-192), map files already under
/// `__tests__/` directly (lines 194-200), and otherwise walk to the library
/// root adding `<subpath>/__tests__/` at each level (lines 202-216, with the
/// root contributing `__tests__/`).
pub fn get_test_paths(
    paths: &[String],
    project_root: &str,
    library_root_for_path: &dyn Fn(&str) -> Option<String>,
    library_name_for_root: &dyn Fn(&str) -> Option<String>,
    is_file: &dyn Fn(&str) -> bool,
) -> Result<Vec<TestPath>, EngineError> {
    let mut found: HashMap<String, TestPath> = HashMap::new();
    for path in paths {
        let Some(library_root) = library_root_for_path(path) else {
            continue;
        };
        let Some(library_name) = library_name_for_root(&library_root) else {
            return Err(EngineError::Failed(format!(
                "Attempting to run unit tests on a library which has not been loaded, at:\n\n    \
                 {library_root}\n\nMake sure this library is configured to load.\n\n(In rare cases, \
                 this may be because you are attempting to run one copy of this software against a \
                 different copy of this software. This operation is not supported.)"
            )));
        };
        let resolved = join_root(project_root, path);
        if !is_descendant(&resolved, &library_root) {
            continue;
        }
        if is_file(&resolved) && is_in_tests_dir(&resolved) {
            let library_path = readable_path(&resolved, &library_root);
            found.insert(
                format!("{library_name}:{library_path}"),
                TestPath {
                    library: library_name,
                    path: library_path,
                },
            );
            continue;
        }
        for subpath in walk_to_root(&resolved, &library_root) {
            if subpath == library_root {
                found.insert(
                    format!("{library_name}:."),
                    TestPath {
                        library: library_name.clone(),
                        path: format!("{TESTS_DIR}/"),
                    },
                );
            } else {
                let sub = readable_path(&subpath, &library_root);
                found.insert(
                    format!("{library_name}:{sub}"),
                    TestPath {
                        library: library_name.clone(),
                        path: format!("{sub}/{TESTS_DIR}/"),
                    },
                );
            }
        }
    }
    let mut out: Vec<TestPath> = found.into_values().collect();
    out.sort_by(|a, b| (&a.library, &a.path).cmp(&(&b.library, &b.path)));
    Ok(out)
}

/// Match `(?:^|/)__tests__/` (lines 93, 194) on normalized separators.
#[must_use]
pub fn is_in_tests_dir(path: &str) -> bool {
    let normal = path.replace('\\', "/");
    normal == TESTS_DIR
        || normal.starts_with(&format!("{TESTS_DIR}/"))
        || normal.contains(&format!("/{TESTS_DIR}/"))
        || normal.ends_with(&format!("/{TESTS_DIR}"))
}

/// Ancestors from `path` up to and including `stop` (lines 202-216 walk).
#[must_use]
pub fn walk_to_root(path: &str, stop: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = path.trim_end_matches('/').to_string();
    let stop = stop.trim_end_matches('/').to_string();
    loop {
        if !is_descendant(&current, &stop) && current != stop {
            break;
        }
        out.push(current.clone());
        if current == stop {
            break;
        }
        match current.rfind('/') {
            Some(i) => current = current[..i].to_string(),
            None => break, // LCOV_EXCL_LINE // Reason: dead: slashless current either equals stop or is not a descendant, both break above
        }
        if current.is_empty() {
            break;
        }
    }
    out
}

fn join_root(root: &str, path: &str) -> String {
    let normal = path.replace('\\', "/");
    if normal.starts_with('/') || normal.contains(':') {
        return normal;
    }
    format!(
        "{}/{}",
        root.replace('\\', "/").trim_end_matches('/'),
        normal
    )
}

fn readable_path(path: &str, root: &str) -> String {
    let base = format!("{}/", root.trim_end_matches('/'));
    path.strip_prefix(&base).unwrap_or(path).to_string()
}

fn is_descendant(path: &str, root: &str) -> bool {
    let base = root.trim_end_matches('/');
    path == base || path.starts_with(&format!("{base}/"))
}

/// PHP interpreter binary name, resolved on the user's `PATH`.
pub const PHP_BINARY: &str = "php";

/// `.arcconfig` key for the PHP bootstrap file (project-relative or
/// absolute) that loads the project's libraries before tests run.
/// (Rust extension: PHP runs in-process, already bootstrapped.)
pub const INIT_SCRIPT_KEY: &str = "unit.phutil.init-script";

/// Candidate interpreter names (mirrors anoid: stock installs call it
/// `php` on every platform; the `.exe` probe handles Windows suffixes).
fn php_candidates() -> &'static [&'static str] {
    &[PHP_BINARY]
}

/// Locate a PHP interpreter on the real `$PATH` (anoid's
/// `python3_on_path` pattern: `find_on_path` per candidate).
#[must_use]
pub fn php_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    php_on_path_in(&path)
}

/// [`php_on_path`] against an explicit search path (test seam).
fn php_on_path_in(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    php_candidates()
        .iter()
        .find_map(|binary| arc_rs::anoid::find_on_path(path_env, binary))
}

/// Default `php` invocation prefix: the resolved interpreter path, if any.
#[must_use]
pub fn php_argv_prefix() -> Option<Vec<String>> {
    let path = std::env::var_os("PATH")?;
    php_argv_prefix_in(&path)
}

/// [`php_argv_prefix`] against an explicit search path (test seam).
fn php_argv_prefix_in(path_env: &std::ffi::OsStr) -> Option<Vec<String>> {
    php_on_path_in(path_env).map(|path| vec![path.to_string_lossy().into_owned()])
}

/// Clear error when no `php` is available (the php-absent path is correct:
/// phutil tests cannot run without an interpreter).
#[must_use]
pub fn missing_php_message(engine: &str) -> String {
    format!("Engine '{engine}' requires PHP on your $PATH (php) to execute phutil tests.")
}

/// Input for one phutil driver run, serialized to the staged input file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhutilRunInput {
    /// Project root (selection scope and relative-path base).
    pub project_root: String,
    /// Enumerate every test (`getAllTests`) instead of path-derived ones.
    pub run_all_tests: bool,
    /// Workflow-selected paths (empty under `--everything`).
    pub paths: Vec<String>,
    /// Coverage flag (`None` is the default, like PHP's `null`).
    pub enable_coverage: Option<bool>,
    /// Init script from [INIT_SCRIPT_KEY], if configured.
    pub init_script: Option<String>,
}

/// Serialize one driver input to JSON.
#[must_use]
pub fn phutil_input_json(input: &PhutilRunInput) -> String {
    serde_json::json!({
        "projectRoot": input.project_root,
        "runAllTests": input.run_all_tests,
        "paths": input.paths,
        "enableCoverage": input.enable_coverage,
        "initScript": input.init_script,
    })
    .to_string()
}

/// Driver argv: `php <driver> <input-json> <output-json>`.
#[must_use]
pub fn build_php_argv(
    php_prefix: &[String],
    driver: &str,
    input: &str,
    output: &str,
) -> Vec<String> {
    let mut argv = php_prefix.to_vec();
    argv.push(driver.to_string());
    argv.push(input.to_string());
    argv.push(output.to_string());
    argv
}

/// True for the five known result codes (anything else is driver-output
/// corruption, since the driver only emits `toDictionary` codes).
#[must_use]
pub fn is_known_result_code(code: &str) -> bool {
    matches!(
        code,
        RESULT_PASS | RESULT_FAIL | RESULT_SKIP | RESULT_BROKEN | RESULT_UNSOUND
    )
}

/// Fetch an optional string field from a driver result object.
fn result_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// Fetch an optional string-map field from a driver result object, in
/// document order (serde keeps `preserve_order`, UNIT-U20). Non-string
/// values become empty (only keys matter downstream).
fn result_map_field(value: &serde_json::Value, key: &str) -> Option<Vec<(String, String)>> {
    let obj = value.get(key)?.as_object()?;
    Some(
        obj.iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect(),
    )
}

/// Parse one driver result object (`toDictionary` shape) into a result.
fn parse_phutil_result(value: &serde_json::Value) -> Result<UnitResult, EngineError> {
    let name = result_string_field(value, "name").ok_or_else(|| {
        EngineError::Failed("phutil driver result is missing its 'name'.".to_string())
    })?;
    let code = result_string_field(value, "result").ok_or_else(|| {
        EngineError::Failed("phutil driver result is missing its 'result'.".to_string())
    })?;
    if !is_known_result_code(&code) {
        return Err(EngineError::Failed(format!(
            "phutil driver reported unknown result code '{code}'."
        )));
    }
    let mut result = UnitResult::new();
    result.set_name(&name).map_err(EngineError::Failed)?;
    result.set_result(&code);
    if let Some(namespace) = result_string_field(value, "namespace") {
        result.set_namespace(&namespace);
    }
    if let Some(link) = result_string_field(value, "link") {
        result.set_link(&link);
    }
    if let Some(duration) = value.get("duration").and_then(|v| v.as_f64()) {
        result.set_duration(duration);
    }
    if let Some(user_data) = result_string_field(value, "userData") {
        result.set_user_data(&user_data);
    }
    if let Some(coverage) = result_map_field(value, "coverage") {
        result.set_coverage_ordered(coverage);
    }
    Ok(result)
}

/// Parse driver output JSON into results: `{"results": [...]}` yields
/// results (empty yields `No tests to run.`, like an empty class
/// selection), `{"error": "..."}` yields the driver's usage error.
pub fn parse_phutil_output(text: &str, output_file: &str) -> Result<Vec<UnitResult>, EngineError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        EngineError::Failed(format!("cannot parse phutil output '{output_file}': {e}"))
    })?;
    if let Some(error) = result_string_field(&value, "error") {
        return Err(EngineError::Usage(error));
    }
    let Some(items) = value.get("results").and_then(|v| v.as_array()) else {
        return Err(EngineError::Failed(format!(
            "phutil output '{output_file}' has neither 'results' nor 'error'."
        )));
    };
    if items.is_empty() {
        return Err(EngineError::no_effect());
    }
    items.iter().map(parse_phutil_result).collect()
}

/// The staged driver script: enumerate and run libphutil test cases
/// out-of-process, reporting JSON. Usage: `php <driver> <input.json>
/// <output.json>`. Mirrors `PhutilUnitTestEngine::run` (selection lines
/// 16-25, coverage gate lines 27-43, execution lines 45-77) with
/// libphutil primitives; the init script must load libphutil first.
#[must_use]
pub fn phutil_driver_source() -> &'static str {
    r#"<?php
// arc-rs phutil unit driver: enumerate and run libphutil test cases
// out-of-process, reporting JSON results.
// Usage: php <driver> <input.json> <output.json>
// Input keys: projectRoot, runAllTests, paths, enableCoverage
// (bool|null), initScript (string|null).
$input_file = $argv[1];
$output_file = $argv[2];
$input = json_decode(@file_get_contents($input_file), true);
if (!is_array($input)) {
  fwrite(STDERR, "phutil driver: cannot read input '{$input_file}'.\n");
  exit(1);
}
$emit = function ($data) use ($output_file) {
  file_put_contents($output_file, json_encode($data));
};
$root = $input['projectRoot'];
$init = isset($input['initScript']) ? $input['initScript'] : null;
if ($init === null || !strlen($init)) {
  $emit(array(
    'error' => "Cannot run phutil tests: no init script configured (set ".
      "'unit.phutil.init-script' in .arcconfig to a PHP file that loads ".
      "your project's libraries).",
  ));
  exit(0);
}
if ($init[0] !== '/' && !preg_match('/^[A-Za-z]:[\\\\\\/]/', $init)) {
  $init = $root.'/'.$init;
}
if (!file_exists($init)) {
  $emit(array('error' => "Phutil init script was not found in {$init}"));
  exit(0);
}
require_once $init;
$coverage = array_key_exists('enableCoverage', $input)
  ? $input['enableCoverage']
  : null;
if ($coverage !== false) {
  if (!function_exists('xdebug_start_code_coverage')) {
    if ($coverage === true) {
      $emit(array('error' => 'You specified --coverage but XDebug is not '.
        'available, so coverage can not be enabled for '.
        'PhutilUnitTestEngine.'));
      exit(0);
    }
  } else if (!class_exists('ArcanistWorkingCopyIdentity')) {
    // Out-of-process runs cannot build arc's WorkingCopy object, which
    // coverage maps need for the project root: refuse forced coverage,
    // and run uncovered by default.
    if ($coverage === true) {
      $emit(array('error' => 'Coverage for PhutilUnitTestEngine requires '.
        'an Arcanist working copy, which is unavailable when arc runs '.
        'phutil tests out-of-process; run without --coverage.'));
      exit(0);
    }
    $coverage = false;
  } else {
    $coverage = true;
  }
}
$classes = array();
if (!empty($input['runAllTests'])) {
  $symbols = id(new PhutilSymbolLoader())
    ->setType('class')
    ->setAncestorClass('PhutilTestCase')
    ->setConcreteOnly(true)
    ->selectSymbolsWithoutLoading();
  foreach ($symbols as $symbol) {
    if (!preg_match('@(?:^|/)__tests__/@', $symbol['where'])) {
      continue;
    }
    $lib_root = phutil_get_library_root($symbol['library']);
    if (!Filesystem::isDescendant($lib_root, $root)) {
      continue;
    }
    $classes[$symbol['name']] = true;
  }
} else {
  $look = array();
  foreach ($input['paths'] as $path) {
    $lib_root = phutil_get_library_root_for_path($path);
    if (!$lib_root) {
      continue;
    }
    $lib_name = phutil_get_library_name_for_root($lib_root);
    if (!$lib_name) {
      $emit(array('error' => "Attempting to run unit tests on a library ".
        "which has not been loaded, at:\n\n    {$lib_root}\n\nMake sure ".
        "this library is configured to load.\n\n(In rare cases, this may ".
        "be because you are attempting to run one copy of this software ".
        "against a different copy of this software. This operation is not ".
        "supported.)"));
      exit(0);
    }
    $resolved = Filesystem::resolvePath($path, $root);
    if (!Filesystem::isDescendant($resolved, $lib_root)) {
      continue;
    }
    $lib_path = Filesystem::readablePath($resolved, $lib_root);
    if (is_file($resolved) &&
        preg_match('@(?:^|/)__tests__/@', $resolved)) {
      $look[$lib_name.':'.$lib_path] = array(
        'library' => $lib_name,
        'path' => $lib_path,
      );
      continue;
    }
    foreach (Filesystem::walkToRoot($resolved, $lib_root) as $sub) {
      if ($sub == $lib_root) {
        $look[$lib_name.':.'] = array(
          'library' => $lib_name,
          'path' => '__tests__/',
        );
      } else {
        $sub_path = Filesystem::readablePath($sub, $lib_root);
        $look[$lib_name.':'.$sub_path] = array(
          'library' => $lib_name,
          'path' => $sub_path.'/__tests__/',
        );
      }
    }
  }
  foreach ($look as $info) {
    $symbols = id(new PhutilSymbolLoader())
      ->setType('class')
      ->setLibrary($info['library'])
      ->setPathPrefix($info['path'])
      ->setAncestorClass('PhutilTestCase')
      ->setConcreteOnly(true)
      ->selectAndLoadSymbols();
    foreach ($symbols as $symbol) {
      $classes[$symbol['name']] = true;
    }
  }
}
$cases = array();
foreach (array_keys($classes) as $class) {
  $case = newv($class, array());
  $case->setEnableCoverage($coverage);
  if (!empty($input['paths'])) {
    $case->setPaths($input['paths']);
  }
  $cases[] = $case;
}
foreach ($cases as $case) {
  $case->willRunTestCases($cases);
}
$results = array();
foreach ($cases as $case) {
  $results[] = $case->run();
}
$results = array_mergev($results);
foreach ($cases as $case) {
  $case->didRunTestCases($cases);
}
$dicts = array();
foreach ($results as $result) {
  $dicts[] = $result->toDictionary();
}
$emit(array('results' => array_values($dicts)));
"#
}

#[cfg(test)]
#[path = "unit_phutil_tests.rs"]
mod unit_phutil_tests;
