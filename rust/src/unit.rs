//! `arc-rs unit` — unit-test engine selection plus a cargo adapter.
//!
//! PHP references: `src/workflow/ArcanistUnitWorkflow.php` (`getArguments`
//! lines 38-107, engine setup lines 151-168, per-result rendering lines
//! 184-199, overall mapping lines 243-253, output formats lines 333-348),
//! `src/workflow/ArcanistWorkflow.php` (`newUnitTestEngine` lines 2107-2147),
//! `src/unit/engine/ArcanistUnitTestEngine.php` (run-all gate lines 21-32),
//! `src/unit/renderer/ArcanistUnitConsoleRenderer.php` (`renderUnitResult`
//! lines 5-30, result labels lines 32-54, time format lines 83-99).
//!
//! Selection mirrors `newUnitTestEngine`: explicit `--engine` (workflow
//! line 151 passes `getArgument('engine)`), then the `unit.engine`
//! `.arcconfig` key (workflow lines 2111-2113), then `.arcunit` in the
//! project root (lines 2115-2119). When nothing is configured this port
//! runs the `cargo` engine; PHP throws `ArcanistNoEngineException`
//! (lines 2121-2129) instead.
//!
//! `--engine` accepts `cargo` plus the PHP engine class names verbatim.
//! `.arcunit` `type` values accept the same class names plus `phutil`
//! (`PhutilUnitTestEngine.php:8-10`); PHP resolves `.arcunit` types through
//! a class map of `getEngineConfigurationName` values
//! (`ArcanistConfigurationDrivenUnitTestEngine.php:176-181`) where only
//! `phutil` is non-null in-tree, so accepting class names there diverges
//! from PHP to make every ported engine selectable.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::unit_config::{self, PlannedEngine};
use arc_rs::unit_engine::{
    CommandRunner, EngineContext, EngineError, UnitResult as EngineUnitResult,
};
use arc_rs::unit_nose;
use arc_rs::unit_phpunit;
use arc_rs::unit_pytest;

pub const NAME: &str = "unit";

/// Engines `--engine` accepts: `cargo` plus the PHP engine class names
/// verbatim (`NoseTestEngine.php:8`, `PytestTestEngine.php:6`,
/// `PhpunitTestEngine.php:6`, `PhutilUnitTestEngine.php:6`,
/// `XUnitTestEngine.php:12`, `CSharpToolsTestEngine.php:10`,
/// `ArcanistConfigurationDrivenUnitTestEngine.php:3`).
pub const AVAILABLE_ENGINES: &[&str] = &[
    "cargo",
    "ArcanistConfigurationDrivenUnitTestEngine",
    "CSharpToolsTestEngine",
    "NoseTestEngine",
    "PhpunitTestEngine",
    "PhutilUnitTestEngine",
    "PytestTestEngine",
    "XUnitTestEngine",
];

/// `.arcunit` `type` values: the PHP engine class names plus `phutil`
/// (`PhutilUnitTestEngine.php:9` returns `'phutil'`).
pub const CONFIG_AVAILABLE: &[&str] = &[
    "CSharpToolsTestEngine",
    "NoseTestEngine",
    "PhpunitTestEngine",
    "PhutilUnitTestEngine",
    "PytestTestEngine",
    "XUnitTestEngine",
    "phutil",
];

pub const USAGE: &str =
    "usage: arc-rs unit [--json] [--engine ENGINE] [--everything] [-- <cargo-test-args>]\n";

/// Number of trailing output lines echoed after the one-line summary.
pub const TAIL_LINES: usize = 20;

/// Test outcome codes, mirroring `ArcanistUnitTestResult::RESULT_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
    Broken,
    Unsound,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Skip => "skip",
            Outcome::Broken => "broken",
            Outcome::Unsound => "unsound",
        }
    }

    pub fn from_pass(passed: bool) -> Self {
        if passed { Outcome::Pass } else { Outcome::Fail }
    }
}

/// One test result: name, outcome, and duration in seconds.
///
/// Mirrors `ArcanistUnitTestResult` (`name`, `result`, `duration`). Until
/// per-test parsing is ported, each engine run yields a single aggregate
/// result named after the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitResult {
    pub name: String,
    pub outcome: Outcome,
    pub duration_secs: f64,
}

impl UnitResult {
    pub fn new(name: &str, outcome: Outcome, duration_secs: f64) -> Self {
        Self {
            name: name.to_string(),
            outcome,
            duration_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub engine: String,
    pub json: bool,
    pub help: bool,
    pub cargo_args: Vec<String>,
    /// `--everything` (`ArcanistUnitWorkflow.php:93-100`): run all tests.
    pub everything: bool,
    /// True when `--engine` was given explicitly (override like PHP
    /// `newUnitTestEngine($engine_class)`).
    pub engine_explicit: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            engine: "cargo".to_string(),
            json: false,
            help: false,
            cargo_args: Vec::new(),
            everything: false,
            engine_explicit: false,
        }
    }
}

/// Parse `argv` (excluding the `unit` subcommand name).
///
/// Everything after the first `--` is forwarded verbatim to `cargo test`;
/// only tokens before it are interpreted as `unit` flags.
pub fn parse_args(argv: &[String]) -> Result<Options, String> {
    let mut opts = Options::default();
    let mut rest = argv.iter();
    let mut forwarding = false;

    while let Some(arg) = rest.next() {
        if forwarding {
            opts.cargo_args.push(arg.clone());
            continue;
        }
        if arg == "--" {
            forwarding = true;
        } else if arg == "--json" {
            opts.json = true;
        } else if arg == "--everything" {
            opts.everything = true;
        } else if arg == "--help" || arg == "-h" {
            opts.help = true;
        } else if arg == "--engine" {
            let Some(value) = rest.next() else {
                return Err("--engine requires a value".to_string());
            };
            opts.engine = value.clone();
            opts.engine_explicit = true;
        } else if let Some(value) = arg.strip_prefix("--engine=") {
            if value.is_empty() {
                return Err("--engine requires a value".to_string());
            }
            opts.engine = value.to_string();
            opts.engine_explicit = true;
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!(
                "unexpected argument '{arg}' (pass cargo test args after '--')"
            ));
        }
    }
    Ok(opts)
}

/// Walk up from `dir` looking for `Cargo.toml`; returns the directory that
/// contains it, if any.
pub fn find_crate_root_from(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find_map(|dir| {
        if dir.join("Cargo.toml").is_file() {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

/// Walk up from `dir` looking for `.arcunit`; returns the directory that
/// contains it, if any (`ArcanistWorkflow.php:2116` checks
/// `getProjectPath('.arcunit')`).
#[must_use]
pub fn find_arcunit_from(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find_map(|dir| {
        if dir.join(".arcunit").is_file() {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

/// Read the `unit.engine` key (`ArcanistWorkflow.php:2112`
/// `getConfigFromAnySource('unit.engine')`) from the nearest `.arcconfig`
/// walking up from `dir`.
#[must_use]
pub fn find_unit_engine_from(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors() {
        let Ok(text) = std::fs::read_to_string(ancestor.join(".arcconfig")) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(name) = value.get("unit.engine").and_then(|v| v.as_str())
            && !name.is_empty()
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Aggregate report for one engine run.
#[derive(Debug, Clone)]
pub struct EngineReport {
    pub engine: String,
    pub passed: u32,
    pub failed: u32,
    pub duration_ms: u64,
    pub success: bool,
    pub tail: String,
    pub results: Vec<UnitResult>,
}

impl EngineReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": self.engine,
            "passed": self.passed,
            "failed": self.failed,
            "duration_ms": self.duration_ms,
        })
    }

    pub fn summary_line(&self) -> String {
        let status = if self.success { "PASS" } else { "FAIL" };
        format!(
            "{} {} ({} passed, {} failed, {} ms)",
            status, self.engine, self.passed, self.failed, self.duration_ms
        )
    }
}

/// Aggregate report for a configuration-driven or single-engine run over
/// per-test results.
#[derive(Debug, Clone)]
pub struct ConfigReport {
    pub engine: String,
    pub passed: u32,
    pub failed: u32,
    pub duration_ms: u64,
    pub success: bool,
    pub results: Vec<EngineUnitResult>,
}

impl ConfigReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": self.engine,
            "passed": self.passed,
            "failed": self.failed,
            "duration_ms": self.duration_ms,
        })
    }
}

/// Last `n` lines of `text` (without a trailing partial-line artifact).
pub fn tail(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Run the configured engine in the crate found by walking up from `cwd`.
///
/// `target_dir` overrides `CARGO_TARGET_DIR` for the child process only;
/// pass `None` for normal runs (cargo default) and `Some` in tests to keep
/// build output out of the repo. The child always runs with
/// `CARGO_NET_OFFLINE=true`.
///
/// Per-test parsing is out of scope: `passed`/`failed` are 1/0 or 0/1 based
/// on the `cargo test` exit status.
pub fn execute(
    opts: &Options,
    cwd: &Path,
    target_dir: Option<&Path>,
) -> Result<EngineReport, String> {
    if !AVAILABLE_ENGINES.contains(&opts.engine.as_str()) {
        return Err(format!(
            "unknown engine '{}' (available engines: {})",
            opts.engine,
            AVAILABLE_ENGINES.join(", ")
        ));
    }
    if opts.engine != "cargo" {
        return Err(format!(
            "engine '{}' is not the cargo engine (execute() runs cargo only)",
            opts.engine
        ));
    }
    let Some(root) = find_crate_root_from(cwd) else {
        return Err(format!(
            "no Cargo.toml found walking up from '{}'",
            cwd.display()
        ));
    };

    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("test")
        .args(&opts.cargo_args)
        .current_dir(&root)
        .env("CARGO_NET_OFFLINE", "true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = target_dir {
        cmd.env("CARGO_TARGET_DIR", dir);
    }
    let started = std::time::Instant::now();
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run 'cargo test': {e}"))?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

    let success = output.status.success();
    let (passed, failed) = if success { (1, 0) } else { (0, 1) };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let results = vec![UnitResult::new(
        &opts.engine,
        Outcome::from_pass(success),
        duration_ms as f64 / 1000.0,
    )];
    Ok(EngineReport {
        engine: opts.engine.clone(),
        passed,
        failed,
        duration_ms,
        success,
        tail: tail(&combined, TAIL_LINES),
        results,
    })
}

/// Engine selection mirroring `newUnitTestEngine`
/// (`ArcanistWorkflow.php:2107-2147`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Default `cargo` run (PHP throws `ArcanistNoEngineException` here).
    Cargo,
    /// One engine named by `--engine` or `unit.engine`.
    Single(String),
    /// Configuration-driven run over `.arcunit` engines.
    Config {
        engines: Vec<PlannedEngine>,
        root: PathBuf,
    },
}

fn validate_engine(engine: &str) -> Result<(), String> {
    if !AVAILABLE_ENGINES.contains(&engine) {
        return Err(format!(
            "unknown engine '{engine}' (available engines: {})",
            AVAILABLE_ENGINES.join(", ")
        ));
    }
    Ok(())
}

fn load_config_selection(cwd: &Path) -> Result<Selection, String> {
    let Some(dir) = find_arcunit_from(cwd) else {
        return Err(
            "Unable to find '.arcunit' file to configure test engines. Create an \
            '.arcunit' file in the root directory of the working copy."
                .to_string(),
        );
    };
    let path = dir.join(".arcunit");
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let display = path.to_string_lossy().to_string();
    let engines = unit_config::parse_engine_specs(&data, &display, CONFIG_AVAILABLE)
        .map_err(|e| e.message().to_string())?;
    Ok(Selection::Config { engines, root: dir })
}

/// Select engines: explicit `--engine`, then `unit.engine` from `.arcconfig`,
/// then `.arcunit`, else the `cargo` default.
pub fn discover_selection(opts: &Options, cwd: &Path) -> Result<Selection, String> {
    if opts.engine_explicit {
        validate_engine(&opts.engine)?;
        if opts.engine == "cargo" {
            return Ok(Selection::Cargo);
        }
        if opts.engine == "ArcanistConfigurationDrivenUnitTestEngine" {
            return load_config_selection(cwd);
        }
        return Ok(Selection::Single(opts.engine.clone()));
    }
    if let Some(class) = find_unit_engine_from(cwd) {
        validate_engine(&class)?;
        if class == "cargo" {
            return Ok(Selection::Cargo);
        }
        if class == "ArcanistConfigurationDrivenUnitTestEngine" {
            return load_config_selection(cwd);
        }
        return Ok(Selection::Single(class));
    }
    if find_arcunit_from(cwd).is_some() {
        return load_config_selection(cwd);
    }
    Ok(Selection::Cargo)
}

/// Whether an engine supports `--everything`: nose
/// (`NoseTestEngine.php:12-14`), xunit (`XUnitTestEngine.php:24-26`),
/// phutil (`PhutilUnitTestEngine.php:12-14`) and csharp (inherits xunit)
/// return true; pytest and phpunit inherit the false default
/// (`ArcanistUnitTestEngine.php:38-40`).
#[must_use]
pub fn supports_run_all_tests(engine_type: &str) -> bool {
    matches!(
        engine_type,
        "NoseTestEngine"
            | "XUnitTestEngine"
            | "CSharpToolsTestEngine"
            | "PhutilUnitTestEngine"
            | "phutil"
    )
}

/// Real command runner: spawn `argv` with `cwd`, capturing output.
struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, argv: &[String], cwd: &str) -> arc_rs::unit_engine::CapturedOutput {
        if argv.is_empty() {
            return arc_rs::unit_engine::CapturedOutput::new(1, "", "empty command");
        }
        match std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .output()
        {
            Ok(output) => arc_rs::unit_engine::CapturedOutput::new(
                output.status.code().unwrap_or(1),
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            ),
            Err(e) => arc_rs::unit_engine::CapturedOutput::new(
                1,
                "",
                &format!("failed to run '{}': {e}", argv.join(" ")),
            ),
        }
    }
}

/// Real-filesystem probes for the pytest engine.
struct RealPytestFs;

impl unit_pytest::PytestFs for RealPytestFs {
    fn path_exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn is_file(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.lines().count().max(1))
    }
}

/// Real-filesystem probes for the phpunit engine.
struct RealPhpunitFs;

impl unit_phpunit::PhpunitFs for RealPhpunitFs {
    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn path_exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.lines().count().max(1))
    }
}

/// Run one engine by `.arcunit` type or `--engine` class name.
///
/// Every command goes through `runner` so tests inject canned outputs.
pub fn run_single_with_runner(
    engine_type: &str,
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    if !CONFIG_AVAILABLE.contains(&engine_type) {
        return Err(EngineError::Usage(format!(
            "unknown engine '{engine_type}' (available engines: {})",
            AVAILABLE_ENGINES.join(", ")
        )));
    }
    EngineContext::check_set_run_all_tests(
        supports_run_all_tests(engine_type),
        engine_type,
        ctx.run_all_tests,
    )
    .map_err(EngineError::Usage)?;
    match engine_type {
        "NoseTestEngine" => {
            unit_nose::NoseEngine::new().run_with(ctx, runner, &unit_nose::RealNoseFs)
        }
        "PytestTestEngine" => unit_pytest::PytestEngine::new().run_with(ctx, runner, &RealPytestFs),
        "PhpunitTestEngine" => {
            unit_phpunit::PhpunitEngine::new().run_with(ctx, runner, &RealPhpunitFs)
        }
        "PhutilUnitTestEngine" | "phutil" => Err(EngineError::Usage(
            "Engine 'PhutilUnitTestEngine' executes PHP test cases in-process and requires \
             a PHP runtime with loaded libraries; arc-rs cannot execute phutil tests."
                .to_string(),
        )),
        "XUnitTestEngine" => Err(EngineError::Usage(
            "Engine 'XUnitTestEngine' requires .NET build orchestration, which is not yet \
             ported (see T1128)."
                .to_string(),
        )),
        "CSharpToolsTestEngine" => Err(EngineError::Usage(
            "Engine 'CSharpToolsTestEngine' requires .NET build orchestration, which is not \
             yet ported (see T1128)."
                .to_string(),
        )),
        _ => Err(EngineError::Usage(format!(
            "unknown engine '{engine_type}' (available engines: {})",
            AVAILABLE_ENGINES.join(", ")
        ))),
    }
}

/// Count per-test results: `pass` counts as passed, `skip` counts as
/// neither, every other code counts as failed (fail-closed).
fn count_results(results: &[EngineUnitResult]) -> (u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    for result in results {
        match result.get_result() {
            arc_rs::unit_engine::RESULT_PASS => passed += 1,
            arc_rs::unit_engine::RESULT_SKIP => {}
            _ => failed += 1,
        }
    }
    (passed, failed)
}

/// Run one engine and aggregate its per-test results.
pub fn run_single_report(
    engine_type: &str,
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<ConfigReport, String> {
    let ctx = EngineContext {
        project_root: project_root.to_string(),
        paths: paths.to_vec(),
        enable_coverage,
        run_all_tests,
    };
    let started = std::time::Instant::now();
    let results =
        run_single_with_runner(engine_type, &ctx, runner).map_err(|e| e.message().to_string())?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let (passed, failed) = count_results(&results);
    let success =
        arc_rs::unit_engine::overall_result(&results) == arc_rs::unit_engine::WORKFLOW_RESULT_OKAY;
    Ok(ConfigReport {
        engine: engine_type.to_string(),
        passed,
        failed,
        duration_ms,
        success,
        results,
    })
}

/// Run `.arcunit` engines and merge their results, mirroring
/// `ArcanistConfigurationDrivenUnitTestEngine::run`
/// (`ArcanistConfigurationDrivenUnitTestEngine.php:117-170`).
pub fn run_config_with_runner(
    planned: &[PlannedEngine],
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<ConfigReport, String> {
    let child_supports: Vec<bool> = planned
        .iter()
        .map(|engine| supports_run_all_tests(&engine.engine_type))
        .collect();
    if run_all_tests && !unit_config::supports_run_all_tests(&child_supports) {
        return Err(
            "Engine 'ArcanistConfigurationDrivenUnitTestEngine' does not support --everything."
                .to_string(),
        );
    }
    let top = EngineContext {
        project_root: project_root.to_string(),
        paths: paths.to_vec(),
        enable_coverage,
        run_all_tests,
    };
    let started = std::time::Instant::now();
    let mut runs = Vec::new();
    for engine in planned {
        let child =
            unit_config::child_context(&top, engine, supports_run_all_tests(&engine.engine_type));
        runs.push(run_single_with_runner(&engine.engine_type, &child, runner));
    }
    let results = unit_config::merge_engine_runs(runs).map_err(|e| e.message().to_string())?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let (passed, failed) = count_results(&results);
    let success =
        arc_rs::unit_engine::overall_result(&results) == arc_rs::unit_engine::WORKFLOW_RESULT_OKAY;
    Ok(ConfigReport {
        engine: "ArcanistConfigurationDrivenUnitTestEngine".to_string(),
        passed,
        failed,
        duration_ms,
        success,
        results,
    })
}

/// Format a duration in seconds, mirroring `formatTime`
/// (`ArcanistUnitConsoleRenderer.php:83-99`).
#[must_use]
pub fn format_time(seconds: f64) -> String {
    if seconds >= 60.0 {
        let minutes = (seconds / 60.0).floor() as i64;
        let rest = seconds as i64 % 60;
        return format!("{minutes}m{rest:02}s");
    }
    if seconds >= 1.0 {
        return format!("{seconds:4.1}s");
    }
    let milliseconds = seconds * 1000.0;
    if milliseconds >= 1.0 {
        return format!("{:3}ms", milliseconds.round() as i64);
    }
    " <1ms".to_string()
}

/// Render one result, mirroring `renderUnitResult`
/// (`ArcanistUnitConsoleRenderer.php:5-30`) in plain text: `  <LABEL>[ time]
/// name`, plus `userData` for non-passing results. Labels match
/// `getFormattedResult` (lines 32-54) without console colors.
#[must_use]
pub fn render_unit_result(result: &EngineUnitResult) -> String {
    let code = result.get_result();
    let label = match code {
        arc_rs::unit_engine::RESULT_PASS => "PASS",
        arc_rs::unit_engine::RESULT_FAIL => "FAIL",
        arc_rs::unit_engine::RESULT_SKIP => "SKIP",
        arc_rs::unit_engine::RESULT_BROKEN => "BROKEN",
        arc_rs::unit_engine::RESULT_UNSOUND => "UNSOUND",
        _ => "",
    };
    let duration = if code == arc_rs::unit_engine::RESULT_PASS {
        format!(" {}", format_time(result.get_duration().unwrap_or(0.0)))
    } else {
        String::new()
    };
    let name = match result.get_namespace() {
        Some(namespace) if !namespace.is_empty() => {
            format!("{}::{}", namespace, result.get_name())
        }
        _ => result.get_name().to_string(),
    };
    let mut out = format!("  {label}{duration} {name}\n");
    if code != arc_rs::unit_engine::RESULT_PASS
        && let Some(user_data) = result.get_user_data()
        && !user_data.is_empty()
    {
        out.push_str(user_data);
        out.push('\n');
    }
    out
}

fn project_root_for_single(cwd: &Path) -> String {
    let dir = cwd
        .ancestors()
        .find(|dir| dir.join(".arcunit").is_file() || dir.join(".arcconfig").is_file())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.to_path_buf());
    dir.to_string_lossy().replace('\\', "/")
}

fn run_config_report(report: &ConfigReport, json: bool) -> ExitCode {
    if json {
        println!("{}", report.to_json());
    } else {
        for result in &report.results {
            print!("{}", render_unit_result(result));
        }
    }
    if report.success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Run the default `cargo` selection and report its outcome.
fn run_cargo_selection(opts: &Options, cwd: &Path) -> ExitCode {
    match execute(opts, cwd, None) {
        Ok(report) => {
            if opts.json {
                println!("{}", report.to_json());
            } else {
                println!("{}", report.summary_line());
                if !report.tail.is_empty() {
                    println!("{}", report.tail);
                }
            }
            if report.success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Run one engine named by `--engine` or `unit.engine`.
fn run_single_selection(opts: &Options, cwd: &Path, class: &str) -> ExitCode {
    if !opts.cargo_args.is_empty() {
        eprintln!("arc-rs unit: cargo test args after '--' require the cargo engine");
        return ExitCode::FAILURE;
    }
    let root = project_root_for_single(cwd);
    let runner = RealRunner;
    match run_single_report(class, &root, &[], opts.everything, None, &runner) {
        Ok(report) => run_config_report(&report, opts.json),
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Run a configuration-driven (`.arcunit`) selection.
fn run_config_selection(opts: &Options, engines: &[PlannedEngine], root: &Path) -> ExitCode {
    if !opts.cargo_args.is_empty() {
        eprintln!("arc-rs unit: cargo test args after '--' require the cargo engine");
        return ExitCode::FAILURE;
    }
    let root_string = root.to_string_lossy().replace('\\', "/");
    let runner = RealRunner;
    match run_config_with_runner(engines, &root_string, &[], opts.everything, None, &runner) {
        Ok(report) => run_config_report(&report, opts.json),
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let opts = match parse_args(argv) {
        Ok(opts) => opts,
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if opts.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("arc-rs unit: cannot read current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let selection = match discover_selection(&opts, &cwd) {
        Ok(selection) => selection,
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            return ExitCode::FAILURE;
        }
    };
    match selection {
        Selection::Cargo => run_cargo_selection(&opts, &cwd),
        Selection::Single(class) => run_single_selection(&opts, &cwd, &class),
        Selection::Config { engines, root } => run_config_selection(&opts, &engines, &root),
    }
}

#[cfg(test)]
#[path = "unit_tests.rs"]
mod unit_tests;
