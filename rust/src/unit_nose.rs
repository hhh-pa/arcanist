//! Nose unit test engine wrapper.
//!
//! PHP reference: `src/unit/engine/NoseTestEngine.php` (concrete,
//! `final class`). Ports `supportsRunAllTests` (lines 12-14), `run`
//! (lines 16-51), `runTests` (lines 53-97), `buildTestFuture` (lines 99-111),
//! `parseTestResults` (lines 113-126), and `readCoverage` (lines 128-180).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - Test futures run sequentially instead of through `FutureIterator`
//!   limited to 4 (lines 74-75); observable results are identical.
//! - The run-all `glob("$root/tests/**/test_*.py")` (line 19) is a recursive
//!   directory walk; `**` matches any depth in both.
//! - Coverage line counts come from a caller-supplied callback instead of
//!   reading each covered file (line 148); files with unknown counts are
//!   skipped rather than failing the run.

extern crate self as arc_rs;

use arc_rs::unit_engine::{
    CapturedOutput, CommandRunner, EngineContext, EngineError, TempFile, UnitResult,
    coverage_string_from_lines, find_elements, parse_xunit_testcases,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Test binary (hardcoded `nosetests`, lines 100-102).
pub const NOSE_BINARY: &str = "nosetests";

/// `supportsRunAllTests` (lines 12-14) returns true.
pub const SUPPORTS_RUN_ALL_TESTS: bool = true;

/// Nose engine configuration: explicit binary path plus run inputs.
#[derive(Debug, Clone)]
pub struct NoseEngine {
    /// `nosetests` binary path (default [`NOSE_BINARY`]).
    pub binary: String,
}

impl NoseEngine {
    /// Engine with the default binary.
    #[must_use]
    pub fn new() -> Self {
        Self {
            binary: NOSE_BINARY.to_string(),
        }
    }

    /// Engine with an explicit binary path (output-capture tests point this
    /// at a fake binary).
    #[must_use]
    pub fn with_binary(binary: &str) -> Self {
        Self {
            binary: binary.to_string(),
        }
    }

    /// `getEngineConfigurationName`: nose defines none, so `None`
    /// (inherits `ArcanistUnitTestEngine.php:17-19`).
    #[must_use]
    pub fn engine_configuration_name(&self) -> Option<&'static str> {
        None
    }

    /// `supportsRunAllTests` (lines 12-14).
    #[must_use]
    pub fn supports_run_all_tests(&self) -> bool {
        SUPPORTS_RUN_ALL_TESTS
    }

    /// `run` (lines 16-51) with injected filesystem probes and command runner.
    /// `is_dir`/`is_readable` see absolute paths, mirroring `resolvePath`.
    pub fn run_with(
        &self,
        ctx: &EngineContext,
        runner: &dyn CommandRunner,
        fs: &dyn NoseFs,
    ) -> Result<Vec<UnitResult>, EngineError> {
        if ctx.run_all_tests {
            let tests = expand_run_all_tests(&ctx.project_root);
            return self.run_tests(&tests, &ctx.project_root, ctx, runner, fs);
        }
        let tests = select_tests(&ctx.paths, &ctx.project_root, fs);
        self.run_tests(&tests, "./", ctx, runner, fs)
    }

    /// `runTests` (lines 53-97): empty input yields no results (line 54-56);
    /// each test runs nose with xunit plus optional coverage temp files.
    pub fn run_tests(
        &self,
        test_paths: &[String],
        source_path: &str,
        ctx: &EngineContext,
        runner: &dyn CommandRunner,
        fs: &dyn NoseFs,
    ) -> Result<Vec<UnitResult>, EngineError> {
        if test_paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut merged = Vec::new();
        for test_path in test_paths {
            let xunit_tmp = TempFile::create("nose-xunit", ".xml").map_err(EngineError::Failed)?;
            let cover_tmp = TempFile::create("nose-cover", ".xml").map_err(EngineError::Failed)?;
            let cover_arg = if ctx.enable_coverage == Some(false) {
                None
            } else {
                Some(cover_tmp.path().to_string_lossy().to_string())
            };
            let argv = build_nose_argv(
                &self.binary,
                &xunit_tmp.path().to_string_lossy(),
                cover_arg.as_deref(),
                test_path,
            );
            let output = runner.run(&argv, &ctx.project_root);
            check_nose_exit(&argv, &output)?;
            let xunit_xml = std::fs::read_to_string(xunit_tmp.path())
                .map_err(|e| EngineError::Failed(e.to_string()))?;
            let cover_xml = if cover_arg.is_some() {
                Some(
                    std::fs::read_to_string(cover_tmp.path())
                        .map_err(|e| EngineError::Failed(e.to_string()))?,
                )
            } else {
                None
            };
            let mut results = parse_test_results(
                source_path,
                &xunit_xml,
                cover_xml.as_deref(),
                ctx.enable_coverage,
                ctx,
                &|path| fs.line_count(path),
            )?;
            merged.append(&mut results);
        }
        Ok(merged)
    }
}

impl Default for NoseEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Filesystem seam for the nose engine (path probes plus coverage line counts).
pub trait NoseFs {
    /// `is_dir($absolute_path)` (line 29).
    fn is_dir(&self, path: &str) -> bool;
    /// `is_readable(...)` (lines 31, 36, 44).
    fn is_readable(&self, path: &str) -> bool;
    /// Line count of a covered file (line 148); `None` skips the file.
    fn line_count(&self, path: &str) -> Option<usize>;
}

/// Real-filesystem probes.
#[derive(Debug, Default)]
pub struct RealNoseFs;

impl NoseFs for RealNoseFs {
    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn is_readable(&self, path: &str) -> bool {
        std::fs::File::open(path).is_ok()
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.lines().count().max(1))
    }
}

/// Map one source file to its nose test path (line 41):
/// `tests/<directory>/test_<filename>`.
#[must_use]
pub fn test_for_source(path: &str) -> String {
    let (directory, filename) = match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (".", path),
    };
    if directory == "." || directory.is_empty() {
        format!("tests/test_{filename}")
    } else {
        format!("tests/{directory}/test_{filename}")
    }
}

/// `run` path mapping (lines 23-50): directories map to `tests/<path>` when
/// readable (lines 29-34); readable files map via [`test_for_source`] when the
/// target is readable (lines 36-47). Paths are joined onto the project root
/// like `Filesystem::resolvePath`, and returned absolute.
#[must_use]
pub fn select_tests(paths: &[String], project_root: &str, fs: &dyn NoseFs) -> Vec<String> {
    let mut affected = Vec::new();
    for path in paths {
        let absolute = join_root(project_root, path);
        if fs.is_dir(&absolute) {
            let candidate = join_root(project_root, &format!("tests/{path}"));
            if fs.is_readable(&candidate) {
                affected.push(candidate);
            }
        }
        if fs.is_readable(&absolute) {
            let candidate = join_root(project_root, &test_for_source(path));
            if fs.is_readable(&candidate) {
                affected.push(candidate);
            }
        }
    }
    affected
}

/// Run-all test discovery (line 19): every `test_*.py` under `<root>/tests`.
#[must_use]
pub fn expand_run_all_tests(project_root: &str) -> Vec<String> {
    let mut out = Vec::new();
    let base = PathBuf::from(project_root).join("tests");
    collect_nose_tests(&base, &mut out);
    out.sort();
    out
}

fn collect_nose_tests(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.sort();
    for path in names {
        if path.is_dir() {
            collect_nose_tests(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("test_") && n.ends_with(".py"))
        {
            out.push(path.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// `buildTestFuture` argv (lines 99-111):
/// `nosetests --with-xunit --xunit-file=X [--with-coverage --cover-xml
/// --cover-xml-file=C] <path>`. Coverage flags are present unless coverage is
/// explicitly disabled (line 104).
#[must_use]
pub fn build_nose_argv(
    nose_binary: &str,
    xunit_file: &str,
    cover_file: Option<&str>,
    test_path: &str,
) -> Vec<String> {
    let mut argv = vec![
        nose_binary.to_string(),
        "--with-xunit".to_string(),
        format!("--xunit-file={xunit_file}"),
    ];
    if let Some(cover) = cover_file {
        argv.push("--with-coverage".to_string());
        argv.push("--cover-xml".to_string());
        argv.push(format!("--cover-xml-file={cover}"));
    }
    argv.push(test_path.to_string());
    argv
}

/// Exit handling (lines 78-84): nose returns 1 for failing/broken tests, which
/// still yields parsed results; anything above 1 is rethrown.
pub fn check_nose_exit(argv: &[String], output: &CapturedOutput) -> Result<(), EngineError> {
    if output.exit_code > 1 {
        return Err(EngineError::Failed(format!(
            "command '{}' failed with exit code {}: {}",
            argv.join(" "),
            output.exit_code,
            output.stderr.trim()
        )));
    }
    Ok(())
}

/// `parseTestResults` (lines 113-126): parse the xunit report, attaching the
/// coverage report to every result unless coverage is disabled.
pub fn parse_test_results(
    _source_path: &str,
    xunit_xml: &str,
    cover_xml: Option<&str>,
    enable_coverage: Option<bool>,
    ctx: &EngineContext,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<Vec<UnitResult>, EngineError> {
    let mut results = parse_xunit_testcases(xunit_xml).map_err(EngineError::Failed)?;
    if enable_coverage != Some(false) {
        let empty = String::new();
        let coverage = read_coverage(
            cover_xml.unwrap_or(&empty),
            &ctx.project_root,
            line_count_of,
        );
        for result in &mut results {
            result.set_coverage(coverage.clone());
        }
    }
    Ok(results)
}

/// `readCoverage` (lines 128-180): coverage.py XML with `<class filename>`
/// plus `<line number hits>` entries. Empty input yields no reports
/// (lines 130-132); files outside the project root are skipped (lines 143-145).
#[must_use]
pub fn read_coverage(
    coverage_xml: &str,
    project_root: &str,
    line_count_of: &dyn Fn(&str) -> Option<usize>,
) -> HashMap<String, String> {
    let mut reports = HashMap::new();
    if coverage_xml.is_empty() {
        return reports;
    }
    for class in find_elements(coverage_xml, "class") {
        let Some(filename) = class.attr("filename") else {
            continue;
        };
        if !is_descendant(filename, project_root) {
            continue;
        }
        let Some(line_count) = line_count_of(filename) else {
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
        reports.insert(
            filename.to_string(),
            coverage_string_from_lines(line_count, &lines),
        );
    }
    reports
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

/// `Filesystem::isDescendant($path, $root)`: path equals or sits under root.
fn is_descendant(path: &str, root: &str) -> bool {
    let normal = path.replace('\\', "/");
    let base = root.replace('\\', "/");
    let base = base.trim_end_matches('/');
    normal == *base || normal.starts_with(&format!("{base}/"))
}

#[cfg(test)]
#[path = "unit_nose_tests.rs"]
mod unit_nose_tests;
