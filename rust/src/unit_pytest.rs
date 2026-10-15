//! Pytest (`py.test`) unit test engine wrapper.
//!
//! PHP reference: `src/unit/engine/PytestTestEngine.php` (concrete,
//! `final class`). Ports `run` (lines 10-34), `buildTestFuture` (lines 36-49),
//! `parseTestResults` (lines 51-64), and `readCoverage` (lines 66-143).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - Coverage line counts and module-path probes come from caller-supplied
//!   callbacks instead of `count(file(...))` / `is_dir` / `is_file`
//!   (lines 89-111); modules that resolve to nothing are skipped, mirroring
//!   the `file_exists` skip (lines 102-104).
//! - `buildTestFuture` fetches `$this->getPaths()` (line 37) but never uses
//!   it; this port takes no paths there either, faithfully.

extern crate self as arc_rs;

use arc_rs::unit_engine::{
    CapturedOutput, CommandRunner, EngineContext, EngineError, TempFile, UnitResult,
    coverage_string_from_lines, find_elements, parse_xunit_testcases,
};
use std::collections::HashMap;

/// Test binary (`py.test`, line 39).
pub const PYTEST_BINARY: &str = "py.test";
/// Coverage binary (`coverage`, lines 29 and 44).
pub const COVERAGE_BINARY: &str = "coverage";

/// Pytest engine configuration: explicit binary paths plus run inputs.
#[derive(Debug, Clone)]
pub struct PytestEngine {
    /// `py.test` binary path (default [`PYTEST_BINARY`]).
    pub binary: String,
    /// `coverage` binary path (default [`COVERAGE_BINARY`]).
    pub coverage_binary: String,
}

impl PytestEngine {
    /// Engine with default binaries.
    #[must_use]
    pub fn new() -> Self {
        Self {
            binary: PYTEST_BINARY.to_string(),
            coverage_binary: COVERAGE_BINARY.to_string(),
        }
    }

    /// Engine with explicit binary paths.
    #[must_use]
    pub fn with_binaries(binary: &str, coverage_binary: &str) -> Self {
        Self {
            binary: binary.to_string(),
            coverage_binary: coverage_binary.to_string(),
        }
    }

    /// `getEngineConfigurationName`: pytest defines none, so `None`
    /// (inherits `ArcanistUnitTestEngine.php:17-19`).
    #[must_use]
    pub fn engine_configuration_name(&self) -> Option<&'static str> {
        None
    }

    /// `supportsRunAllTests`: pytest does not override it, so false
    /// (inherits `ArcanistUnitTestEngine.php:38-40`).
    #[must_use]
    pub fn supports_run_all_tests(&self) -> bool {
        false
    }

    /// `run` (lines 10-34) with injected command runner and filesystem probes.
    pub fn run_with(
        &self,
        ctx: &EngineContext,
        runner: &dyn CommandRunner,
        fs: &dyn PytestFs,
    ) -> Result<Vec<UnitResult>, EngineError> {
        let junit_tmp = TempFile::create("pytest-junit", ".xml").map_err(EngineError::Failed)?;
        let cover_tmp = TempFile::create("pytest-cover", ".xml").map_err(EngineError::Failed)?;
        let junit_path = junit_tmp.path().to_string_lossy().to_string();
        let cover_path = cover_tmp.path().to_string_lossy().to_string();
        // The runner must produce the junit report (line 20 checks this); drop
        // the pre-created scratch so a missing report is observable.
        std::fs::remove_file(&junit_path).ok();

        let argv = self.build_test_argv(ctx, &junit_path);
        let output = runner.run(&argv, &ctx.project_root);
        require_junit_output(
            fs.path_exists(&junit_path),
            output.exit_code,
            &argv,
            &output.stdout,
            &output.stderr,
        )?;
        let junit_xml = fs
            .read_file(&junit_path)
            .ok_or_else(|| EngineError::Failed(format!("cannot read '{junit_path}'")))?;

        let cover_argv = build_coverage_xml_argv(&self.coverage_binary, &cover_path);
        runner.run(&cover_argv, &ctx.project_root);

        let cover_xml = if ctx.enable_coverage == Some(false) {
            None
        } else {
            Some(fs.read_file(&cover_path).unwrap_or_default())
        };
        parse_test_results(
            &junit_xml,
            cover_xml.as_deref(),
            ctx.enable_coverage,
            &ctx.project_root,
            &ctx.paths,
            &|path| fs.line_count(path),
            &|path| fs.is_dir(path),
            &|path| fs.is_file(path),
        )
    }

    /// `buildTestFuture` (lines 36-49) as argv: `py.test --junit-xml=J`,
    /// wrapped as `coverage run --source <root> -m <inner...>` unless coverage
    /// is disabled (line 41). Note the fetched-but-unused `$paths` (line 37).
    #[must_use]
    pub fn build_test_argv(&self, ctx: &EngineContext, junit_file: &str) -> Vec<String> {
        let inner = vec![self.binary.clone(), format!("--junit-xml={junit_file}")];
        if ctx.enable_coverage == Some(false) {
            inner
        } else {
            let mut argv = vec![
                self.coverage_binary.clone(),
                "run".to_string(),
                "--source".to_string(),
                ctx.project_root.clone(),
                "-m".to_string(),
            ];
            argv.extend(inner);
            argv
        }
    }
}

impl Default for PytestEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Filesystem seam for the pytest engine.
pub trait PytestFs {
    /// `Filesystem::pathExists($junit_tmp)` (line 20).
    fn path_exists(&self, path: &str) -> bool;
    /// Read a temp output file.
    fn read_file(&self, path: &str) -> Option<String>;
    /// `is_dir($absolute_path)` (line 89).
    fn is_dir(&self, path: &str) -> bool;
    /// `is_file($absolute_path.'.py')` (line 97).
    fn is_file(&self, path: &str) -> bool;
    /// `count(file($absolute_path))` (line 111); `None` skips the file.
    fn line_count(&self, path: &str) -> Option<usize>;
}

/// `coverage xml -o <cover>` (line 29), run with cwd set to the project root.
#[must_use]
pub fn build_coverage_xml_argv(coverage_binary: &str, cover_file: &str) -> Vec<String> {
    vec![
        coverage_binary.to_string(),
        "xml".to_string(),
        "-o".to_string(),
        cover_file.to_string(),
    ]
}

/// Missing-junit guard (lines 20-27): without the report file the run fails
/// with `pht('Command failed with error #%s!', $err)` (line 22).
pub fn require_junit_output(
    junit_exists: bool,
    err: i32,
    argv: &[String],
    stdout: &str,
    stderr: &str,
) -> Result<(), EngineError> {
    if !junit_exists {
        return Err(EngineError::Failed(format!(
            "Command failed with error #{err}! command '{}' stdout: {stdout} stderr: {stderr}",
            argv.join(" "),
        )));
    }
    Ok(())
}

/// `parseTestResults` (lines 51-64): parse the junit report, attaching the
/// coverage report to every result unless coverage is disabled.
// Eight inputs mirror the PHP run state (reports, coverage flag, root, paths,
// filesystem probes); bundling them would obscure the one-to-one port.
#[allow(clippy::too_many_arguments)]
pub fn parse_test_results(
    junit_xml: &str,
    cover_xml: Option<&str>,
    enable_coverage: Option<bool>,
    project_root: &str,
    paths: &[String],
    line_count_of: &dyn Fn(&str) -> Option<usize>,
    is_dir: &dyn Fn(&str) -> bool,
    is_file: &dyn Fn(&str) -> bool,
) -> Result<Vec<UnitResult>, EngineError> {
    let mut results = parse_xunit_testcases(junit_xml).map_err(EngineError::Failed)?;
    if enable_coverage != Some(false) {
        let empty = String::new();
        let coverage = read_coverage(
            cover_xml.unwrap_or(&empty),
            project_root,
            paths,
            line_count_of,
            is_dir,
            is_file,
        );
        for result in &mut results {
            result.set_coverage(coverage.clone());
        }
    }
    Ok(results)
}

/// Dotted module filename to a relative path stem (lines 82-84): split on
/// `.`, drop the last segment, rejoin with `/`. For example,
/// `tornado.web.py` becomes `tornado/web`.
#[must_use]
pub fn module_to_relative(dotted: &str) -> String {
    let mut parts: Vec<&str> = dotted.split('.').collect();
    parts.pop();
    parts.join("/")
}

/// Resolve a module stem to a relative source path (lines 88-104): a stem
/// that is a directory gains `/__init__.py` (lines 89-92); a stem with a
/// `.py` file gains `.py` (lines 97-100); anything else resolves to nothing
/// (lines 102-104). Probes see root-joined absolute paths.
#[must_use]
pub fn resolve_module_path(
    project_root: &str,
    relative: &str,
    is_dir: &dyn Fn(&str) -> bool,
    is_file: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let absolute = format!("{}/{}", project_root.trim_end_matches('/'), relative);
    if is_dir(&absolute) {
        return Some(format!("{relative}/__init__.py"));
    }
    if is_file(&format!("{absolute}.py")) {
        return Some(format!("{relative}.py"));
    }
    None
}

/// `readCoverage` (lines 66-143): coverage.py XML keyed by dotted module
/// name, resolved to repo-relative paths and filtered to the run paths with
/// `in_array($relative_path, $paths)` (line 106). Empty input yields no
/// reports (lines 68-70). Probes see root-joined absolute paths, mirroring
/// `Filesystem::resolvePath` against the project root.
#[must_use]
pub fn read_coverage(
    coverage_xml: &str,
    project_root: &str,
    paths: &[String],
    line_count_of: &dyn Fn(&str) -> Option<usize>,
    is_dir: &dyn Fn(&str) -> bool,
    is_file: &dyn Fn(&str) -> bool,
) -> HashMap<String, String> {
    let mut reports = HashMap::new();
    if coverage_xml.is_empty() {
        return reports;
    }
    for class in find_elements(coverage_xml, "class") {
        let Some(filename) = class.attr("filename") else {
            continue;
        };
        let stem = module_to_relative(filename);
        let Some(relative) = resolve_module_path(project_root, &stem, is_dir, is_file) else {
            continue;
        };
        if !paths.contains(&relative) {
            continue;
        }
        let absolute = format!("{}/{}", project_root.trim_end_matches('/'), relative);
        let Some(line_count) = line_count_of(&absolute) else {
            continue;
        };
        let mut lines = Vec::new();
        for line in find_elements(&class.inner, "line") {
            let number = line.attr("number").and_then(|n| n.parse::<u32>().ok());
            let hits = line.attr("hits").and_then(|h| h.parse::<i64>().ok());
            if let (Some(number), Some(hits)) = (number, hits) {
                lines.push((number, hits));
            }
        }
        reports.insert(relative, coverage_string_from_lines(line_count, &lines));
    }
    reports
}

/// Format a captured pytest run for diagnostics (exit plus stderr tail).
#[must_use]
pub fn describe_output(output: &CapturedOutput) -> String {
    format!("exit {}: {}", output.exit_code, output.stderr.trim())
}

#[cfg(test)]
#[path = "unit_pytest_tests.rs"]
mod unit_pytest_tests;
