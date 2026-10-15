//! XUnit (.NET/Mono) unit test engine.
//!
//! PHP reference: `src/unit/engine/XUnitTestEngine.php` (concrete and
//! extensible, `@concrete-extensible` line 10, `class` line 12). Ports
//! `supportsRunAllTests` (lines 24-26), `loadEnvironment` (lines 36-95),
//! `run` path selection (lines 103-113), `mapPathsToResults` (lines 121-159),
//! `runAllTests` (lines 167-184), `resultsContainFailures` (lines 194-202),
//! `generateProjects` (lines 215-264), `buildProjects` (lines 277-312),
//! `buildTestFuture` (lines 322-342), `testAssemblies` (lines 351-391),
//! `parseCoverageResult` (lines 402-404), and `parseTestResult` (lines
//! 415-463).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - Discovery-rule matching replaces `preg_match('/'.$regex.'/', $path)`
//!   and `preg_replace` (lines 124-132) with a small matcher supporting
//!   literal text plus `(.*)` captures with `$1`/`${1}` substitution; other
//!   regex syntax is matched literally. This port has no regex engine
//!   (dependency-free crate).
//! - Test assemblies build sequentially instead of through `FutureIterator`
//!   limits of 1 and 8 (lines 291, 367-368); observable results match.
//! - `setDuration($time)` (line 454) passes the raw `time` attribute string;
//!   this port parses it as a float (defaulting to 0.0), since the PHP
//!   `setDuration` int|float check would reject a non-numeric string.
//! - The xUnit XML filename embeds caller-supplied randomness instead of
//!   `Filesystem::readRandomCharacters(10)` (line 326).

extern crate self as arc_rs;

use arc_rs::unit_engine::{
    CommandRunner, EngineContext, EngineError, RESULT_FAIL, RESULT_PASS, RESULT_SKIP,
    RESULT_UNSOUND, UnitResult, find_elements, text_content,
};
use std::collections::HashMap;

/// Build engine candidates (lines 40-51).
pub const BUILD_MSBUILD: &str = "msbuild";
/// Build engine candidates (lines 40-51).
pub const BUILD_XBUILD: &str = "xbuild";
/// Mono runtime binary (line 56).
pub const MONO_BINARY: &str = "mono";
/// xUnit console runner on PATH (line 85).
pub const XUNIT_CONSOLE_BINARY: &str = "xunit.console.clr4.exe";
/// Discovery-rules config key (lines 64-66).
pub const DISCOVERY_KEY: &str = "unit.csharp.discovery";
/// xUnit binary-hint config key (lines 79-80).
pub const XUNIT_BINARY_KEY: &str = "unit.csharp.xunit.binary";
/// MSBuild flag skipping test execution during the build (lines 283-286).
pub const SKIP_TESTS_FLAG: &str = "/p:SkipTestsOnBuild=True";

/// `supportsRunAllTests` (lines 24-26) returns true.
pub const SUPPORTS_RUN_ALL_TESTS: bool = true;

/// One discovery target: a project template plus an assembly template, each
/// with `$1`-style back-references (lines 128-133).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryTarget {
    /// `.csproj` template (index 0, line 130).
    pub project: String,
    /// `.dll` template (index 1, line 131).
    pub assembly: String,
}

/// One discovery rule: a regex plus its target templates (line 123).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRule {
    /// Regex matched against each path (line 126).
    pub regex: String,
    /// Target templates per match (line 127).
    pub targets: Vec<DiscoveryTarget>,
}

/// Resolved test project: `.csproj` plus output assembly (lines 148-151).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestProject {
    /// Resolved `.csproj` path.
    pub project: String,
    /// Resolved assembly path.
    pub assembly: String,
}

/// Loaded environment: build/runtime/test engines, project root, and discovery
/// rules (lines 36-95).
#[derive(Debug, Clone)]
pub struct XunitEnvironment {
    /// `msbuild` or `xbuild` (lines 40-51).
    pub build_engine: String,
    /// `''` on Windows else the mono path (lines 54-61).
    pub runtime_engine: String,
    /// xUnit console runner path (lines 82-94).
    pub test_engine: String,
    /// Project root (line 37).
    pub project_root: String,
    /// Discovery rules (lines 64-74).
    pub discovery_rules: Vec<DiscoveryRule>,
}

/// `loadEnvironment` (lines 36-95) with injected probes. Errors mirror the PHP
/// messages verbatim.
pub fn load_environment(
    project_root: &str,
    is_windows: bool,
    binary_exists: &dyn Fn(&str) -> bool,
    resolve_binary: &dyn Fn(&str) -> String,
    discovery_rules: Option<Vec<DiscoveryRule>>,
    xunit_hint_path: Option<&str>,
    file_exists: &dyn Fn(&str) -> bool,
) -> Result<XunitEnvironment, EngineError> {
    let build_engine = if binary_exists(BUILD_MSBUILD) {
        BUILD_MSBUILD.to_string()
    } else if binary_exists(BUILD_XBUILD) {
        BUILD_XBUILD.to_string()
    } else {
        return Err(EngineError::Failed(format!(
            "Unable to find {} or {} in {}!",
            BUILD_MSBUILD, BUILD_XBUILD, "PATH"
        )));
    };

    let runtime_engine = if is_windows {
        String::new()
    } else if binary_exists(MONO_BINARY) {
        resolve_binary(MONO_BINARY)
    } else {
        return Err(EngineError::Failed(
            "Unable to find Mono and you are not on Windows!".to_string(),
        ));
    };

    let discovery_rules = discovery_rules.ok_or_else(|| {
        EngineError::Failed(format!(
            "You must configure discovery rules to map C# files back to test projects (`{DISCOVERY_KEY}` in .arcconfig)."
        ))
    })?;

    let mut test_engine = None;
    if let Some(hint) = xunit_hint_path {
        let candidate = format!("{project_root}/{hint}");
        if file_exists(&candidate) {
            test_engine = Some(candidate);
        }
    }
    if test_engine.is_none() && binary_exists(XUNIT_CONSOLE_BINARY) {
        test_engine = Some(XUNIT_CONSOLE_BINARY.to_string());
    }
    let test_engine = test_engine.ok_or_else(|| {
        EngineError::Failed(format!(
            "Unable to locate xUnit console runner. Configure it with the \
             `{XUNIT_BINARY_KEY}' option in .arcconfig."
        ))
    })?;

    Ok(XunitEnvironment {
        build_engine,
        runtime_engine,
        test_engine,
        project_root: project_root.to_string(),
        discovery_rules,
    })
}

/// Match a discovery regex against a path, returning `(.*)` captures in order.
/// Literal text (after un-escaping `\/`) must appear in order; a pattern
/// without captures matches by substring. Unanchored, like `preg_match`.
#[must_use]
pub fn discovery_match(regex: &str, path: &str) -> Option<Vec<String>> {
    let mut captures = Vec::new();
    let parts: Vec<&str> = regex.split("(.*)").collect();
    if parts.len() == 1 {
        let literal = regex.replace("\\/", "/");
        return if path.contains(&literal) {
            Some(captures)
        } else {
            None
        };
    }
    let mut rest = path;
    for (i, part) in parts.iter().enumerate() {
        let part = part.replace("\\/", "/");
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if let Some(stripped) = rest.strip_prefix(part.as_str()) {
                rest = stripped;
            } else {
                // Unanchored: find the first literal anywhere.
                let at = rest.find(part.as_str())?;
                captures.push(rest[..at].to_string());
                rest = &rest[at + part.len()..];
            }
            continue;
        }
        let at = rest.find(part.as_str())?;
        captures.push(rest[..at].to_string());
        rest = &rest[at + part.len()..];
    }
    // Trailing capture after the last literal.
    if regex.ends_with("(.*)") {
        captures.push(rest.to_string());
    }
    Some(captures)
}

/// Apply `$1`/`${1}` substitution of captures into a template, mirroring
/// `preg_replace('/'.$regex.'/', $target, $path)` (lines 130-132).
#[must_use]
pub fn discovery_replace(template: &str, captures: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some((text, next)) = replace_capture(&chars, i, captures) {
            out.push_str(&text);
            i = next;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Expand a `$1`/`${1}` reference at `i`: the replacement plus the resume
/// offset. Anything else (including malformed or out-of-range references,
/// which `preg_replace` emits literally) is `None`.
fn replace_capture(chars: &[char], i: usize, captures: &[String]) -> Option<(String, usize)> {
    if chars[i] != '$' || i + 1 >= chars.len() {
        return None;
    }
    if chars[i + 1] == '{' {
        return replace_braced_capture(chars, i, captures);
    }
    if chars[i + 1].is_ascii_digit() {
        return replace_plain_capture(chars, i, captures);
    }
    None
}

/// Expand a `${1}` reference at `i` (`chars[i]` is `$`, `chars[i + 1]` is
/// `{`).
fn replace_braced_capture(
    chars: &[char],
    i: usize,
    captures: &[String],
) -> Option<(String, usize)> {
    let end = chars[i + 2..].iter().position(|c| *c == '}')?;
    let num: String = chars[i + 2..i + 2 + end].iter().collect();
    let n = num.parse::<usize>().ok()?;
    if n >= 1 && n <= captures.len() {
        Some((captures[n - 1].clone(), i + 3 + end))
    } else {
        None
    }
}

/// Expand a `$1` reference at `i` (`chars[i]` is `$`, `chars[i + 1]` is a
/// digit), consuming the longest digit run.
fn replace_plain_capture(chars: &[char], i: usize, captures: &[String]) -> Option<(String, usize)> {
    let mut j = i + 1;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    let num: String = chars[i + 1..j].iter().collect();
    let n = num.parse::<usize>().ok()?;
    if n >= 1 && n <= captures.len() {
        Some((captures[n - 1].clone(), j))
    } else {
        None
    }
}

/// `mapPathsToResults` (lines 121-159): apply every discovery rule to every
/// path (rules outer, paths inner), expand project/assembly templates,
/// prefix the project root, keep existing projects (line 134), and dedupe by
/// assembly (lines 139-146).
#[must_use]
pub fn map_paths_to_results(
    paths: &[String],
    env: &XunitEnvironment,
    file_exists: &dyn Fn(&str) -> bool,
) -> Vec<TestProject> {
    let mut results = Vec::new();
    for rule in &env.discovery_rules {
        for path in paths {
            let Some(captures) = discovery_match(&rule.regex, path) else {
                continue;
            };
            for target in &rule.targets {
                let project = format!(
                    "{}/{}",
                    env.project_root,
                    discovery_replace(&target.project, &captures)
                );
                let assembly = format!(
                    "{}/{}",
                    env.project_root,
                    discovery_replace(&target.assembly, &captures)
                );
                if file_exists(&project)
                    && !results.iter().any(|r: &TestProject| r.assembly == assembly)
                {
                    results.push(TestProject { project, assembly });
                }
            }
        }
    }
    results
}

/// `resultsContainFailures` (lines 194-202): any result that is not a pass.
#[must_use]
pub fn results_contain_failures(results: &[UnitResult]) -> bool {
    results.iter().any(|r| r.get_result() != RESULT_PASS)
}

/// `runAllTests` (lines 167-184): empty input yields nothing (lines 168-170);
/// generate, build, then test, stopping after the first failing stage
/// (lines 174-180).
pub fn run_all_tests(
    test_projects: &[TestProject],
    generate: &dyn Fn() -> Vec<UnitResult>,
    build: &dyn Fn(&[TestProject]) -> Vec<UnitResult>,
    test: &dyn Fn(&[TestProject]) -> Vec<UnitResult>,
) -> Vec<UnitResult> {
    if test_projects.is_empty() {
        return Vec::new();
    }
    let mut results = generate();
    if results_contain_failures(&results) {
        return results;
    }
    let built = build(test_projects);
    results.extend(built);
    if results_contain_failures(&results) {
        return results;
    }
    results.extend(test(test_projects));
    results
}

/// Whether project regeneration applies (lines 217-226): both the `Build`
/// directory and `Protobuild.exe` must exist.
#[must_use]
pub fn should_generate(build_dir_exists: bool, protobuild_exists: bool) -> bool {
    build_dir_exists && protobuild_exists
}

/// Platform detection (lines 229-237): `Windows`/`Linux` by host, overridden
/// by the second-to-last dotted component of the first `*.sln` file found.
#[must_use]
pub fn detect_platform(is_windows: bool, dir_files: &[&str]) -> String {
    let mut platform = if is_windows { "Windows" } else { "Linux" }.to_string();
    for file in dir_files {
        if file.len() >= 4 && file[file.len() - 4..].eq_ignore_ascii_case(".sln") {
            let parts: Vec<&str> = file.split('.').collect();
            if parts.len() >= 2 {
                platform = parts[parts.len() - 2].to_string();
            }
            break;
        }
    }
    platform
}

/// Regenerate argv (lines 240-243): `<runtime> Protobuild.exe --resync
/// <platform>`; an empty runtime is omitted.
#[must_use]
pub fn build_regenerate_argv(runtime_engine: &str, platform: &str) -> Vec<String> {
    let mut argv = Vec::new();
    if !runtime_engine.is_empty() {
        argv.push(runtime_engine.to_string());
    }
    argv.push("Protobuild.exe".to_string());
    argv.push("--resync".to_string());
    argv.push(platform.to_string());
    argv
}

/// Regenerate result name (line 248): `(regenerate projects for <platform>)`.
#[must_use]
pub fn regenerate_result_name(platform: &str) -> String {
    format!("(regenerate projects for {platform})")
}

/// Build argv (lines 283-286): `<build> /p:SkipTestsOnBuild=True`.
#[must_use]
pub fn build_build_argv(build_engine: &str) -> Vec<String> {
    vec![build_engine.to_string(), SKIP_TESTS_FLAG.to_string()]
}

/// Build result name (line 294): `(build) <project>`.
#[must_use]
pub fn build_result_name(project: &str) -> String {
    format!("(build) {project}")
}

/// Stage exit mapping shared by generate/build (lines 250-259, 296-305):
/// exit 0 passes; exit 1 fails with stdout details; anything higher is a hard
/// error (rethrown `CommandException`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOutcome {
    /// Exit 0.
    Pass,
    /// Exit 1.
    Fail,
    /// Exit above 1.
    HardError,
}

/// Map a stage exit code per the generate/build handlers.
#[must_use]
pub fn stage_outcome(exit_code: i32) -> StageOutcome {
    if exit_code == 0 {
        StageOutcome::Pass
    } else if exit_code == 1 {
        StageOutcome::Fail
    } else {
        StageOutcome::HardError
    }
}

/// Build one stage result (generate lines 247-263, build lines 293-310).
pub fn stage_result(
    name: &str,
    exit_code: i32,
    stdout: &str,
    duration_secs: f64,
) -> Result<UnitResult, String> {
    let mut result = UnitResult::new();
    result.set_name(name)?;
    result.set_duration(duration_secs);
    match stage_outcome(exit_code) {
        StageOutcome::Pass => {
            result.set_result(RESULT_PASS);
            Ok(result)
        }
        StageOutcome::Fail => {
            result.set_result(RESULT_FAIL);
            result.set_user_data(stdout);
            Ok(result)
        }
        StageOutcome::HardError => Err(format!("command failed with exit code {exit_code}")),
    }
}

/// Test-run argv (lines 330-334): `<runtime> <engine> <assembly> /xml <xml>`;
/// an empty runtime is omitted.
#[must_use]
pub fn build_test_argv(
    runtime_engine: &str,
    test_engine: &str,
    assembly: &str,
    xml_name: &str,
) -> Vec<String> {
    let mut argv = Vec::new();
    if !runtime_engine.is_empty() {
        argv.push(runtime_engine.to_string());
    }
    argv.push(test_engine.to_string());
    argv.push(assembly.to_string());
    argv.push("/xml".to_string());
    argv.push(xml_name.to_string());
    argv
}

/// xUnit report filename from caller randomness (line 326):
/// `<random10>.results.xml`.
#[must_use]
pub fn xml_report_name(random: &str) -> String {
    format!("{random}.results.xml")
}

/// Output path for the xUnit report (lines 335-341): `<root>/<xml>`, with a
/// backslash separator on Windows (lines 338-340).
#[must_use]
pub fn xunit_output_path(project_root: &str, xml_name: &str, is_windows: bool) -> String {
    if is_windows {
        format!("{project_root}\\{xml_name}")
    } else {
        format!("{project_root}/{xml_name}")
    }
}

/// `parseCoverageResult` (lines 402-404) returns null: xUnit has no coverage
/// of its own (coverage comes from subclasses like the cscover engine).
#[must_use]
pub fn parse_coverage_result() -> Option<HashMap<String, String>> {
    None
}

/// `parseTestResult` (lines 415-463): `<test name time result>` entries with
/// `Pass`/`Fail`/`Skip` mapping (lines 425-435, defaulting to unsound, line
/// 424); details come from the first `<reason>` or `<failure>` child's
/// `<message>` plus `<stack-trace>` (lines 437-449); coverage attaches only
/// when present (lines 456-458).
pub fn parse_assembly_results(
    xunit_xml: &str,
    coverage: Option<&HashMap<String, String>>,
) -> Result<Vec<UnitResult>, String> {
    let mut results = Vec::new();
    for test in find_elements(xunit_xml, "test") {
        let name = test.attr("name").unwrap_or("");
        let time = test.attr("time").unwrap_or("");
        let status = match test.attr("result") {
            Some("Pass") => RESULT_PASS,
            Some("Fail") => RESULT_FAIL,
            Some("Skip") => RESULT_SKIP,
            _ => RESULT_UNSOUND,
        };
        let mut userdata = String::new();
        let reasons = find_elements(&test.inner, "reason");
        let failures = find_elements(&test.inner, "failure");
        let node = if !reasons.is_empty() {
            reasons.first()
        } else {
            failures.first()
        };
        if let Some(node) = node {
            if let Some(message) = find_elements(&node.inner, "message").first() {
                userdata.push_str(&text_content(&message.inner));
            }
            if let Some(trace) = find_elements(&node.inner, "stack-trace").first() {
                userdata.push('\n');
                userdata.push_str(&text_content(&trace.inner));
            }
        }
        let mut result = UnitResult::new();
        result.set_name(name)?;
        result.set_result(status);
        result.set_duration(time.parse::<f64>().unwrap_or(0.0));
        result.set_user_data(&userdata);
        if let Some(coverage) = coverage {
            result.set_coverage(coverage.clone());
        }
        results.push(result);
    }
    Ok(results)
}

/// `run` path selection (lines 106-110): run-all lists every file under the
/// project root, otherwise the context paths are used. The caller supplies
/// the file listing (PHP uses `FileFinder`).
#[must_use]
pub fn select_run_paths(
    run_all_tests: bool,
    ctx_paths: &[String],
    all_files: &[String],
) -> Vec<String> {
    if run_all_tests {
        all_files.to_vec()
    } else {
        ctx_paths.to_vec()
    }
}

/// Missing-report tolerance (lines 372-387): assemblies whose XML report never
/// appears (Mono segfault bug) contribute no results instead of failing.
#[must_use]
pub fn should_parse_output(report_exists: bool) -> bool {
    report_exists
}

/// XUnit engine handle: environment plus host/cwd behavior for runs.
#[derive(Debug, Clone)]
pub struct XunitEngine {
    /// Loaded environment.
    pub env: XunitEnvironment,
    /// Host is Windows (output path separators).
    pub is_windows: bool,
}

impl XunitEngine {
    /// Handle from a loaded environment.
    #[must_use]
    pub fn new(env: XunitEnvironment, is_windows: bool) -> Self {
        Self { env, is_windows }
    }

    /// `getEngineConfigurationName`: xunit defines none, so `None`
    /// (inherits `ArcanistUnitTestEngine.php:17-19`).
    #[must_use]
    pub fn engine_configuration_name(&self) -> Option<&'static str> {
        None
    }

    /// `supportsRunAllTests` (lines 24-26).
    #[must_use]
    pub fn supports_run_all_tests(&self) -> bool {
        SUPPORTS_RUN_ALL_TESTS
    }

    /// Build/test one assembly set through the runner with canned-friendly
    /// seams: `random_names` supplies the per-assembly XML names, `read_file`
    /// reads reports back, `remove_file` deletes them (line 377).
    pub fn test_assemblies_with(
        &self,
        test_projects: &[TestProject],
        runner: &dyn CommandRunner,
        random_names: &dyn Fn() -> String,
        read_file: &dyn Fn(&str) -> Option<String>,
        report_exists: &dyn Fn(&str) -> bool,
        remove_file: &dyn Fn(&str),
    ) -> Result<Vec<UnitResult>, EngineError> {
        let mut merged = Vec::new();
        for project in test_projects {
            let xml_name = xml_report_name(&random_names());
            let argv = build_test_argv(
                &self.env.runtime_engine,
                &self.env.test_engine,
                &project.assembly,
                &xml_name,
            );
            runner.run(&argv, &self.env.project_root);
            let output = xunit_output_path(&self.env.project_root, &xml_name, self.is_windows);
            if should_parse_output(report_exists(&output)) {
                let xml = read_file(&output)
                    .ok_or_else(|| EngineError::Failed(format!("cannot read '{output}'")))?;
                let mut results =
                    parse_assembly_results(&xml, None).map_err(EngineError::Failed)?;
                merged.append(&mut results);
                remove_file(&output);
            }
        }
        Ok(merged)
    }
}

/// Report the engine run inputs for diagnostics.
#[must_use]
pub fn describe_run(ctx: &EngineContext, projects: &[TestProject]) -> String {
    format!(
        "root {} paths {} projects {}",
        ctx.project_root,
        ctx.paths.len(),
        projects.len()
    )
}

#[cfg(test)]
#[path = "unit_xunit_tests.rs"]
mod unit_xunit_tests;
