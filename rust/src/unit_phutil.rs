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
//!   XDebug coverage; no equivalent exists here, so this port selects test
//!   classes and resolves coverage only. The caller runs tests by other means.
//! - `getAllTests`/`getTestsForPaths` use `PhutilSymbolLoader` (lines 83-87,
//!   136-142); here the caller supplies symbol names and this port applies
//!   the same `__tests__` and working-copy filters.
//! - Library roots/names come from caller-supplied callbacks instead of
//!   `phutil_get_library_root_for_path` / `phutil_get_library_name_for_root`
//!   (lines 162-168).

extern crate self as arc_rs;

use arc_rs::unit_engine::EngineError;
use std::collections::HashMap;

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
            None => break,
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

#[cfg(test)]
#[path = "unit_phutil_tests.rs"]
mod unit_phutil_tests;
