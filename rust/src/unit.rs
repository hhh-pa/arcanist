//! `arc-rs unit` — unit-test engine framework plus a cargo adapter.
//!
//! Mirrors the result model of `src/unit/ArcanistUnitTestResult.php`
//! (`name`, `outcome`, `duration`) at the engine granularity. Only the
//! `cargo` engine is supported; per-test parsing of `cargo test` output is
//! explicitly out of scope (see T1128), so each run produces one aggregate
//! result mapped from the `cargo test` exit status.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const NAME: &str = "unit";

/// Engines this port implements. `--engine` accepts only these values.
pub const AVAILABLE_ENGINES: &[&str] = &["cargo"];

pub const USAGE: &str = "usage: arc-rs unit [--json] [--engine ENGINE] [-- <cargo-test-args>]\n";

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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            engine: "cargo".to_string(),
            json: false,
            help: false,
            cargo_args: Vec::new(),
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
        } else if arg == "--help" || arg == "-h" {
            opts.help = true;
        } else if arg == "--engine" {
            let Some(value) = rest.next() else {
                return Err("--engine requires a value".to_string());
            };
            opts.engine = value.clone();
        } else if let Some(value) = arg.strip_prefix("--engine=") {
            if value.is_empty() {
                return Err("--engine requires a value".to_string());
            }
            opts.engine = value.to_string();
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
    match execute(&opts, &cwd, None) {
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

#[cfg(test)]
#[path = "unit_tests.rs"]
mod unit_tests;
