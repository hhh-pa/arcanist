use super::*;
use crate::unit_engine::UnitResult as EngineUnitResult;
use crate::unit_engine::{CapturedOutput, EngineContext};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-unit-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "arc-rs-unit-{}-{}-{tag}",
        std::process::id(),
        nanos
    ))
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_args(&args(&[])).unwrap(), Options::default());
}

#[test]
fn parses_json_and_engine_flags() {
    let opts = parse_args(&args(&["--json", "--engine", "cargo"])).unwrap();
    assert!(opts.json);
    assert_eq!(opts.engine, "cargo");
    assert!(opts.cargo_args.is_empty());

    let opts = parse_args(&args(&["--engine=cargo", "--", "--nocapture"])).unwrap();
    assert_eq!(opts.engine, "cargo");
    // UNIT-U6: post-`--` tokens collect into `paths` like PHP.
    assert_eq!(opts.paths, args(&["--nocapture"]));
    assert!(opts.cargo_args.is_empty());
}

#[test]
fn forwards_everything_after_dashdash() {
    let opts = parse_args(&args(&["--", "--json", "--engine", "bogus"])).unwrap();
    assert!(!opts.json);
    assert_eq!(opts.engine, "cargo");
    // UNIT-U6: post-`--` tokens collect into `paths` like PHP.
    assert_eq!(opts.paths, args(&["--json", "--engine", "bogus"]));
    assert!(opts.cargo_args.is_empty());
}

#[test]
fn parses_help_flags() {
    assert!(parse_args(&args(&["--help"])).unwrap().help);
    assert!(parse_args(&args(&["-h"])).unwrap().help);
}

#[test]
fn rejects_bad_invocations() {
    // D1 strings verbatim from `ArcanistWorkflow::parseArguments`.
    let err = parse_args(&args(&["--bogus"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
    let err = parse_args(&args(&["--bogus=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
    let err = parse_args(&args(&["-x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'x'. Try 'arc help'.");
    let err = parse_args(&args(&["-"])).unwrap_err();
    assert_eq!(err, "Unknown argument ''. Try 'arc help'.");
    for flag in ["--rev", "--engine", "--output", "--target", "--conduit-uri"] {
        let err = parse_args(&args(&[flag])).unwrap_err();
        assert_eq!(err, format!("Option '{flag}' requires a parameter."));
    }
    // Empty `--engine=` parses (D1 stores empty); discovery treats it
    // as unset and falls through to config (UNIT-U14).
    let opts = parse_args(&args(&["--engine="])).unwrap();
    assert_eq!(opts.engine, "");
    assert!(opts.engine_explicit);
    // Positionals collect into `paths` (the `*` wildcard), never error.
    let opts = parse_args(&args(&["some-path"])).unwrap();
    assert_eq!(opts.paths, args(&["some-path"]));
    assert!(opts.cargo_args.is_empty());
}

#[test]
fn outcome_codes_match_php_result_constants() {
    assert_eq!(Outcome::Pass.as_str(), "pass");
    assert_eq!(Outcome::Fail.as_str(), "fail");
    assert_eq!(Outcome::Skip.as_str(), "skip");
    assert_eq!(Outcome::Broken.as_str(), "broken");
    assert_eq!(Outcome::Unsound.as_str(), "unsound");
    assert_eq!(Outcome::from_pass(true), Outcome::Pass);
    assert_eq!(Outcome::from_pass(false), Outcome::Fail);
}

#[test]
fn json_shape_has_required_keys() {
    let report = EngineReport {
        engine: "cargo".to_string(),
        passed: 1,
        failed: 0,
        duration_ms: 12,
        success: true,
        tail: String::new(),
        results: vec![UnitResult::new("cargo", Outcome::Pass, 0.012)],
    };
    let value = report.to_json();
    assert_eq!(value["engine"], "cargo");
    assert_eq!(value["passed"], 1);
    assert_eq!(value["failed"], 0);
    assert_eq!(value["duration_ms"], 12);
    assert_eq!(value.as_object().unwrap().len(), 4);
}

#[test]
fn summary_line_marks_pass_and_fail() {
    let mut report = EngineReport {
        engine: "cargo".to_string(),
        passed: 1,
        failed: 0,
        duration_ms: 7,
        success: true,
        tail: String::new(),
        results: Vec::new(),
    };
    let line = report.summary_line();
    assert!(line.starts_with("PASS"), "{line}");
    assert!(line.contains("cargo"), "{line}");
    report.success = false;
    report.passed = 0;
    report.failed = 1;
    let line = report.summary_line();
    assert!(line.starts_with("FAIL"), "{line}");
}

#[test]
fn tail_keeps_last_n_lines() {
    assert_eq!(tail("a\nb\nc", 2), "b\nc");
    assert_eq!(tail("a\nb\n", 5), "a\nb");
    assert_eq!(tail("", 20), "");
    assert_eq!(tail("only", 0), "");
}

#[test]
fn finds_crate_root_upwards() {
    let root = unique_dir("find");
    let child = root.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(root.join("a").join("Cargo.toml"), "[package]\n").unwrap();
    assert_eq!(
        find_crate_root_from(&child),
        Some(root.join("a")),
        "should find ancestor Cargo.toml"
    );
    assert_eq!(find_crate_root_from(&root), None);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_unknown_engine_without_running_cargo() {
    let opts = Options {
        engine: "phpunit".to_string(),
        ..Options::default()
    };
    let err = execute(&opts, Path::new("."), None).unwrap_err();
    // UNIT-U8: PHP's subclass message, not an allowlist dump.
    assert!(err.contains("is not a subclass of"), "{err}");
    assert!(err.contains("\"phpunit\""), "{err}");
}

#[test]
fn errors_clearly_without_cargo_toml() {
    let dir = unique_dir("nocrate");
    std::fs::create_dir_all(&dir).unwrap();
    let err = execute(&Options::default(), &dir, None).unwrap_err();
    assert!(err.contains("no Cargo.toml"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

// --- W1 wiring: selection, config, run, render ---

#[test]
fn parses_everything_and_tracks_explicit_engine() {
    let defaults = parse_args(&args(&[])).unwrap();
    assert!(!defaults.everything);
    assert!(!defaults.engine_explicit);

    let opts = parse_args(&args(&["--everything"])).unwrap();
    assert!(opts.everything);
    assert!(!opts.engine_explicit);

    let opts = parse_args(&args(&["--engine", "cargo"])).unwrap();
    assert!(opts.engine_explicit);
    let opts = parse_args(&args(&["--engine=cargo"])).unwrap();
    assert!(opts.engine_explicit);

    let opts = parse_args(&args(&["--", "--everything"])).unwrap();
    assert!(!opts.everything);
    // UNIT-U6: post-`--` tokens collect into `paths` like PHP.
    assert_eq!(opts.paths, args(&["--everything"]));
    assert!(opts.cargo_args.is_empty());
}

#[test]
fn finds_arcunit_and_unit_engine_keys() {
    // T1153 CORE-33/UNIT-U13: `unit.engine` reads through the source merge, so
    // the project file only counts at a working-copy root. Shadow options
    // keep the ambient `~/.arcrc` out of the assertions.
    let root = unique_dir("arcunit-find");
    let child = root.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(root.join(".arcunit"), "{}").unwrap();
    assert_eq!(find_arcunit_from(&child), Some(root.clone()));
    assert_eq!(find_arcunit_from(&root), Some(root.clone()));

    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"unit.engine": "NoseTestEngine"}"#,
    )
    .unwrap();
    let shadow = shadow_opts(&root);
    assert_eq!(
        find_unit_engine_with(&child, None, &shadow).as_deref(),
        Some("NoseTestEngine")
    );
    assert_eq!(
        find_unit_engine_with(&root, None, &shadow).as_deref(),
        Some("NoseTestEngine")
    );
    // A subdir project file stays invisible under git.
    std::fs::write(
        child.join(".arcconfig"),
        r#"{"unit.engine": "PytestTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        find_unit_engine_with(&child, None, &shadow).as_deref(),
        Some("NoseTestEngine")
    );
    std::fs::remove_dir_all(&root).ok();

    let bare = unique_dir("arcunit-bare");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::create_dir_all(bare.join(".git")).unwrap();
    let shadow = shadow_opts(&bare);
    assert_eq!(find_arcunit_from(&bare), None);
    assert_eq!(find_unit_engine_with(&bare, None, &shadow), None);
    std::fs::write(bare.join(".arcconfig"), "not json").unwrap();
    assert_eq!(find_unit_engine_with(&bare, None, &shadow), None);
    std::fs::write(bare.join(".arcconfig"), r#"{"unit.engine": ""}"#).unwrap();
    assert_eq!(find_unit_engine_with(&bare, None, &shadow), None);
    std::fs::write(bare.join(".arcconfig"), "{}").unwrap();
    assert_eq!(find_unit_engine_with(&bare, None, &shadow), None);
    std::fs::write(bare.join(".arcconfig"), r#"{"unit.engine": 42}"#).unwrap();
    assert_eq!(find_unit_engine_with(&bare, None, &shadow), None);
    std::fs::remove_dir_all(&bare).ok();
}

/// Hermetic [`get_config::LoadOptions`]: `--config-file` shadows the
/// ambient user/system sources.
fn shadow_opts(dir: &std::path::Path) -> arc_rs::get_config::LoadOptions {
    let empty = dir.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    arc_rs::get_config::LoadOptions {
        config_files: vec![empty],
        ..Default::default()
    }
}

#[test]
fn discover_selection_prefers_explicit_then_config_then_cargo() {
    let dir = unique_dir("sel-order");
    std::fs::create_dir_all(&dir).unwrap();
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    // UNIT-U7: nothing configured is `ArcanistNoEngineException`, not a
    // cargo default.
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    assert!(err.contains("No unit test engine is configured"), "{err}");

    // UNIT-U15: only the `phutil` class-map name validates in `.arcunit`.
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"n": {"type": "NoseTestEngine"}}}"#,
    )
    .unwrap();
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    assert!(
        err.contains("specifies invalid type 'NoseTestEngine'"),
        "{err}"
    );
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"p": {"type": "phutil"}}}"#,
    )
    .unwrap();
    assert_eq!(
        discover_selection(&Options::default(), &dir).unwrap(),
        Selection::Config {
            engines: vec![arc_rs::unit_config::PlannedEngine {
                name: "p".to_string(),
                engine_type: "phutil".to_string(),
                include: Vec::new(),
                exclude: Vec::new(),
            }],
            root: dir.clone(),
        }
    );

    let explicit_cargo = parse_args(&args(&["--engine", "cargo"])).unwrap();
    assert_eq!(
        discover_selection(&explicit_cargo, &dir).unwrap(),
        Selection::Cargo
    );
    let nose = parse_args(&args(&["--engine", "NoseTestEngine"])).unwrap();
    assert_eq!(
        discover_selection(&nose, &dir).unwrap(),
        Selection::Single("NoseTestEngine".to_string())
    );
    let config_class = parse_args(&args(&[
        "--engine",
        "ArcanistConfigurationDrivenUnitTestEngine",
    ]))
    .unwrap();
    assert!(matches!(
        discover_selection(&config_class, &dir).unwrap(),
        Selection::Config { .. }
    ));

    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit.engine": "PytestTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_selection(&Options::default(), &dir).unwrap(),
        Selection::Single("PytestTestEngine".to_string())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn discover_selection_rejects_bad_config() {
    let dir = unique_dir("sel-bad");
    std::fs::create_dir_all(&dir).unwrap();
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();

    let bogus = parse_args(&args(&["--engine", "BogusEngine"])).unwrap();
    let err = discover_selection(&bogus, &dir).unwrap_err();
    // UNIT-U8: PHP's subclass message.
    assert!(err.contains("is not a subclass of"), "{err}");
    assert!(err.contains("\"BogusEngine\""), "{err}");

    let config_class = parse_args(&args(&[
        "--engine",
        "ArcanistConfigurationDrivenUnitTestEngine",
    ]))
    .unwrap();
    let err = discover_selection(&config_class, &dir).unwrap_err();
    assert!(err.contains("Unable to find '.arcunit'"), "{err}");

    std::fs::write(dir.join(".arcconfig"), r#"{"unit.engine": "BogusEngine"}"#).unwrap();
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    // UNIT-U8: PHP's subclass message.
    assert!(err.contains("is not a subclass of"), "{err}");

    std::fs::write(dir.join(".arcconfig"), "{}").unwrap();
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"bad": {"type": "BogusEngine"}}}"#,
    )
    .unwrap();
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    assert!(
        err.contains("specifies invalid type 'BogusEngine'"),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn supports_run_all_tests_matches_php() {
    assert!(supports_run_all_tests("NoseTestEngine"));
    assert!(!supports_run_all_tests("PytestTestEngine"));
    assert!(!supports_run_all_tests("PhpunitTestEngine"));
    assert!(supports_run_all_tests("XUnitTestEngine"));
    assert!(supports_run_all_tests("CSharpToolsTestEngine"));
    assert!(supports_run_all_tests("PhutilUnitTestEngine"));
    assert!(supports_run_all_tests("phutil"));
    assert!(!supports_run_all_tests("Bogus"));
}

#[test]
fn execute_rejects_known_non_cargo() {
    let opts = parse_args(&args(&["--engine", "NoseTestEngine"])).unwrap();
    let err = execute(&opts, Path::new("."), None).unwrap_err();
    assert!(err.contains("not the cargo engine"), "{err}");
}

const CANNED_XUNIT: &str = "<testsuite name=\"t\" tests=\"2\">\
    <testcase classname=\"a\" name=\"ok\" time=\"0.125\"/>\
    <testcase classname=\"a\" name=\"bad\" time=\"0.5\">\
    <failure>oops</failure></testcase></testsuite>";

fn forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn run_single_nose_with_canned_xunit() {
    let root = unique_dir("nose-run");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::create_dir_all(root.join("tests").join("pkg")).unwrap();
    std::fs::write(root.join("pkg").join("mod.py"), "x = 1\n").unwrap();
    std::fs::write(
        root.join("tests").join("pkg").join("test_mod.py"),
        "def test_ok(): pass\n",
    )
    .unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(&root),
        paths: vec!["pkg/mod.py".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), CANNED_XUNIT).unwrap();
        CapturedOutput::new(1, "", "")
    };
    let results = run_single_with_runner("NoseTestEngine", &ctx, &runner).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(results[0].get_name(), "a.ok");
    assert_eq!(results[1].get_result(), "fail");
    assert!(results[1].get_user_data().unwrap().contains("oops"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_single_pytest_with_canned_junit() {
    let root = unique_dir("pytest-run");
    std::fs::create_dir_all(&root).unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(&root),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let runner = |argv: &[String], _cwd: &str| {
        if argv[0] == "py.test" {
            let junit = argv.iter().find(|a| a.starts_with("--junit-xml=")).unwrap();
            std::fs::write(junit.trim_start_matches("--junit-xml="), CANNED_XUNIT).unwrap();
        }
        CapturedOutput::new(0, "", "")
    };
    let results = run_single_with_runner("PytestTestEngine", &ctx, &runner).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(results[1].get_result(), "fail");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_single_phpunit_with_canned_json() {
    let root = unique_dir("phpunit-run");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("Calc.php"), "<?php\n").unwrap();
    std::fs::write(root.join("src").join("CalcTest.php"), "<?php\n").unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(&root),
        paths: vec!["src/Calc.php".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let events = "{\"event\":\"testStart\",\"test\":\"CalcTest::testAdd\"}\n\
        {\"event\":\"test\",\"test\":\"CalcTest::testAdd\",\"status\":\"pass\",\"time\":0.02}\n";
    let runner = |argv: &[String], _cwd: &str| {
        let at = argv.iter().position(|a| a == "--log-json").unwrap();
        std::fs::write(&argv[at + 1], events).unwrap();
        CapturedOutput::new(0, "", "")
    };
    let results = run_single_with_runner("PhpunitTestEngine", &ctx, &runner).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "pass");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_single_rejects_everything_when_unsupported() {
    let ctx = EngineContext {
        project_root: forward_slashes(&std::env::temp_dir()),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: true,
    };
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let err = run_single_with_runner("PytestTestEngine", &ctx, &runner).unwrap_err();
    let message = err.message();
    assert!(
        message.contains("does not support --everything"),
        "{message}"
    );

    let root = unique_dir("nose-everything");
    std::fs::create_dir_all(&root).unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(&root),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: true,
    };
    let results = run_single_with_runner("NoseTestEngine", &ctx, &runner).unwrap();
    assert!(results.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_single_unrunnable_engines_error_clearly() {
    let ctx = EngineContext::new(&forward_slashes(&std::env::temp_dir()));
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    // Phutil without a PHP interpreter fails with the clear php-absent
    // error (forced absent, so no PATH assumption); xUnit/C# need .NET
    // toolchains plus `.arcconfig` discovery, so bare temp dirs fail with
    // environment errors.
    for engine in ["PhutilUnitTestEngine", "phutil"] {
        let err = run_single_with_php(engine, &ctx, &runner, &|| None).unwrap_err();
        assert_eq!(
            err.message(),
            &format!("Engine '{engine}' requires PHP on your $PATH (php) to execute phutil tests."),
            "{engine}"
        );
    }
    for engine in ["XUnitTestEngine", "CSharpToolsTestEngine"] {
        let err = run_single_with_runner(engine, &ctx, &runner).unwrap_err();
        let msg = err.message();
        let markers = [
            "Unable to find",
            "discovery rules",
            "Unable to locate",
            "cscover",
        ];
        assert!(
            markers.iter().any(|marker| msg.contains(marker)),
            "{engine}: {msg}"
        );
    }
    let err = run_single_with_runner("Bogus", &ctx, &runner).unwrap_err();
    // UNIT-U8: PHP's subclass message.
    let message = err.message();
    assert!(message.contains("is not a subclass of"), "{message}");
}

#[test]
fn run_config_merges_canned_results_and_counts() {
    let root = unique_dir("config-run");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::create_dir_all(root.join("tests").join("pkg")).unwrap();
    std::fs::write(root.join("pkg").join("mod.py"), "x = 1\n").unwrap();
    std::fs::write(
        root.join("tests").join("pkg").join("test_mod.py"),
        "def test_ok(): pass\n",
    )
    .unwrap();
    // UNIT-U15: class names no longer validate via `.arcunit`
    // (`CONFIG_AVAILABLE` is the `phutil`-only class map), so the merge
    // test plans against an explicit available list.
    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"n": {"type": "NoseTestEngine"}}}"#,
        "/r/.arcunit",
        &["NoseTestEngine"],
    )
    .unwrap();
    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), CANNED_XUNIT).unwrap();
        CapturedOutput::new(1, "", "")
    };
    let report = run_config_with_runner(
        &planned,
        &forward_slashes(&root),
        &["pkg/mod.py".to_string()],
        false,
        Some(false),
        &runner,
    )
    .unwrap();
    assert_eq!(report.engine, "ArcanistConfigurationDrivenUnitTestEngine");
    assert_eq!((report.passed, report.failed), (1, 1));
    assert!(!report.success);
    assert_eq!(report.results.len(), 2);
    let json = report.to_json();
    assert_eq!(json["passed"], 1);
    assert_eq!(json["failed"], 1);
    assert_eq!(json.as_object().unwrap().len(), 4);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_config_validates_everything_and_no_effect() {
    // UNIT-U15: explicit available list (see above).
    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"p": {"type": "PytestTestEngine"}}}"#,
        "/r/.arcunit",
        &["PytestTestEngine"],
    )
    .unwrap();
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let err =
        run_config_with_runner(&planned, "/tmp", &[], true, Some(false), &runner).unwrap_err();
    assert!(err.contains("does not support --everything"), "{err}");

    // UNIT-U15: explicit available list (see above).
    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"p": {"type": "PhpunitTestEngine"}}}"#,
        "/r/.arcunit",
        &["PhpunitTestEngine"],
    )
    .unwrap();
    let root = unique_dir("config-noeffect");
    std::fs::create_dir_all(&root).unwrap();
    let err = run_config_with_runner(
        &planned,
        &forward_slashes(&root),
        &[],
        false,
        Some(false),
        &runner,
    )
    .unwrap_err();
    assert!(err.contains("No tests to run."), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

fn engine_result(name: &str, code: &str) -> EngineUnitResult {
    let mut result = EngineUnitResult::new();
    result.set_name(name).unwrap();
    result.set_result(code);
    result
}

#[test]
fn render_unit_result_cases() {
    // UNIT-U10: the plain rendering strips the badge/duration markup like
    // PHP, keeping the star, spaces, and padding.
    let mut pass = engine_result("t", "pass");
    pass.set_namespace("ns");
    pass.set_duration(0.125);
    assert_eq!(render_unit_result(&pass), "   PASS  125ms   ns::t\n");

    let bare_pass = engine_result("t", "pass");
    assert_eq!(
        render_unit_result(&bare_pass),
        format!("   PASS   <1ms{}  t\n", expected_star())
    );

    let mut fail = engine_result("t", "fail");
    fail.set_user_data("oops");
    assert_eq!(render_unit_result(&fail), "   FAIL  t\noops\n");

    let fail_bare = engine_result("t", "fail");
    assert_eq!(render_unit_result(&fail_bare), "   FAIL  t\n");

    assert_eq!(
        render_unit_result(&engine_result("t", "skip")),
        "   SKIP  t\n"
    );
    assert_eq!(
        render_unit_result(&engine_result("t", "broken")),
        "   BROKEN  t\n"
    );
    assert_eq!(
        render_unit_result(&engine_result("t", "unsound")),
        "   UNSOUND  t\n"
    );
    assert_eq!(render_unit_result(&engine_result("t", "bogus")), "   t\n");

    let mut trailing = engine_result("t", "fail");
    trailing.set_user_data("x\n");
    assert_eq!(render_unit_result(&trailing), "   FAIL  t\nx\n\n");
}

#[test]
fn format_time_branches() {
    assert_eq!(format_time(61.0), "1m01s");
    assert_eq!(format_time(125.0), "2m05s");
    assert_eq!(format_time(1.0), " 1.0s");
    assert_eq!(format_time(12.34), "12.3s");
    assert_eq!(format_time(0.125), "125ms");
    assert_eq!(format_time(0.005), "  5ms");
    assert_eq!(format_time(0.0), " <1ms");
    assert_eq!(format_time(0.0005), " <1ms");
}

#[test]
fn config_report_json_shape() {
    let report = ConfigReport {
        engine: "NoseTestEngine".to_string(),
        passed: 2,
        failed: 1,
        duration_ms: 9,
        success: false,
        results: Vec::new(),
    };
    let value = report.to_json();
    assert_eq!(value["engine"], "NoseTestEngine");
    assert_eq!(value["passed"], 2);
    assert_eq!(value["failed"], 1);
    assert_eq!(value["duration_ms"], 9);
    assert_eq!(value.as_object().unwrap().len(), 4);
}

// --- Full-state: D1 parsing, conflicts, supports ---

fn defaults() -> Options {
    Options::default()
}

#[test]
fn parses_all_spec_flags() {
    let opts = parse_options(&args(&[
        "--rev",
        "HEAD~1",
        "--engine",
        "NoseTestEngine",
        "--coverage",
        "--detailed-coverage",
        "--json",
        "--output",
        "json",
        "--target",
        "PHID-1",
        "--everything",
        "--ugly",
        "--conduit-uri",
        "https://phab.test/",
        "src/a.php",
    ]));
    // `--coverage` + `--output json` + `--json` + `--everything` + `--rev`
    // conflicts, so parse the legal subset instead.
    assert!(opts.is_err());

    let opts = parse_options(&args(&[
        "--rev",
        "HEAD~1",
        "--engine",
        "NoseTestEngine",
        "--conduit-uri=https://phab.test/",
        "src/a.php",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(opts.rev.as_deref(), Some("HEAD~1"));
    assert_eq!(opts.engine, "NoseTestEngine");
    assert!(opts.engine_explicit);
    assert_eq!(opts.conduit_uri.as_deref(), Some("https://phab.test/"));
    assert_eq!(opts.paths, args(&["src/a.php"]));
    assert_eq!(opts.provided, vec!["rev", "engine", "conduit-uri"]);

    let opts = parse_options(&args(&[
        "--coverage",
        "--detailed-coverage",
        "--target",
        "PHID-1",
        "--output=json",
    ]))
    .unwrap()
    .unwrap();
    assert!(opts.coverage);
    assert!(opts.detailed_coverage);
    assert_eq!(opts.target.as_deref(), Some("PHID-1"));
    assert_eq!(opts.output.as_deref(), Some("json"));

    // Re-providing overwrites without error.
    let opts = parse_options(&args(&["--engine", "a", "--engine", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(opts.engine, "b");
    assert_eq!(opts.provided, vec!["engine"]);
    assert_eq!(defaults().provided.len(), 0);
}

#[test]
fn parses_help_and_dashdash() {
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_options(&args(&["-h"])).unwrap(), None);
    // UNIT-U6: past `--`, help collects into `paths` like PHP.
    let opts = parse_options(&args(&["--", "--help"])).unwrap().unwrap();
    assert_eq!(opts.paths, args(&["--help"]));
    assert!(opts.cargo_args.is_empty());
    // Unknown `--help=x` errors like D1.
    let err = parse_options(&args(&["--help=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'help'. Try 'arc help'.");
    // `--bool=value` splices the value as the next token (a path here).
    let opts = parse_options(&args(&["--json=x"])).unwrap().unwrap();
    assert!(opts.json);
    assert_eq!(opts.paths, vec!["x".to_string()]);
    // UNIT-U6: a second `--` collects verbatim into `paths` like PHP.
    let opts = parse_options(&args(&["--", "a", "--", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(opts.paths, args(&["a", "--", "b"]));
}

#[test]
fn rejects_every_declared_conflict() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["--coverage", "--no-coverage"],
            "Arguments '--coverage' and '--no-coverage' are mutually exclusive.",
        ),
        (
            &["--no-coverage", "--coverage"],
            "Arguments '--coverage' and '--no-coverage' are mutually exclusive.",
        ),
        (
            &["--output", "json", "--json"],
            "Arguments '--output' and '--json' are mutually exclusive: Only one output format allowed",
        ),
        (
            &["--json", "--output", "full"],
            "Arguments '--output' and '--json' are mutually exclusive: Only one output format allowed",
        ),
        (
            &["--output", "full", "--ugly"],
            "Arguments '--output' and '--ugly' are mutually exclusive: Only one output format allowed",
        ),
        (
            &["--everything", "--rev", "HEAD"],
            "Arguments '--everything' and '--rev' are mutually exclusive: --everything runs all tests.",
        ),
        (
            &["--rev", "HEAD", "--everything"],
            "Arguments '--everything' and '--rev' are mutually exclusive: --everything runs all tests.",
        ),
    ];
    for (argv, expected) in cases {
        let err = parse_options(&args(argv)).unwrap_err();
        assert_eq!(&err, expected, "{argv:?}");
    }
    // `--json` + `--ugly` stays legal: ugly wins in `output_format`.
    let opts = parse_options(&args(&["--json", "--ugly"]))
        .unwrap()
        .unwrap();
    assert!(opts.json && opts.ugly);
    assert_eq!(output_format(&opts), "ugly");
}

#[test]
fn supports_check_rejects_rev_under_svn() {
    // Direct `check_supports` (private, via super) in provision order.
    assert!(super::check_supports(super::Vcs::Git, &["rev"]).is_ok());
    assert!(super::check_supports(super::Vcs::Hg, &["rev"]).is_ok());
    let err = super::check_supports(super::Vcs::Svn, &["rev"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--rev' is not supported under svn. Arc unit does not currently support --rev in SVN."
    );
    // Unrelated flags pass on every VCS.
    for vcs in [super::Vcs::Git, super::Vcs::Hg, super::Vcs::Svn] {
        assert!(super::check_supports(vcs, &["engine", "json"]).is_ok());
        assert!(super::check_supports(vcs, &[]).is_ok());
    }
}

#[test]
fn output_format_and_coverage_flags() {
    let mut opts = Options::default();
    assert_eq!(output_format(&opts), "full");
    assert_eq!(enable_coverage(&opts), None);

    opts.ugly = true;
    opts.json = true;
    opts.output = Some("json".to_string());
    assert_eq!(output_format(&opts), "ugly");
    opts.ugly = false;
    assert_eq!(output_format(&opts), "json");
    opts.json = false;
    assert_eq!(output_format(&opts), "json");
    opts.output = Some("ugly".to_string());
    assert_eq!(output_format(&opts), "ugly");
    opts.output = Some("none".to_string());
    assert_eq!(output_format(&opts), "none");
    opts.output = Some("full".to_string());
    assert_eq!(output_format(&opts), "full");
    opts.output = Some("bogus".to_string());
    assert_eq!(output_format(&opts), "full");
    opts.output = None;
    assert_eq!(output_format(&opts), "full");

    opts.coverage = true;
    assert_eq!(enable_coverage(&opts), Some(true));
    opts.coverage = false;
    opts.detailed_coverage = true;
    opts.no_coverage = true;
    // Detailed implies coverage even alongside `--no-coverage` (PHP order).
    assert_eq!(enable_coverage(&opts), Some(true));
    opts.detailed_coverage = false;
    assert_eq!(enable_coverage(&opts), Some(false));
    opts.no_coverage = false;
    assert_eq!(enable_coverage(&opts), None);
}

#[test]
fn harbormaster_and_exit_codes() {
    use crate::unit_engine::{
        WORKFLOW_RESULT_FAIL, WORKFLOW_RESULT_OKAY, WORKFLOW_RESULT_SKIP, WORKFLOW_RESULT_UNSOUND,
    };
    assert_eq!(harbormaster_type(WORKFLOW_RESULT_OKAY), "pass");
    assert_eq!(harbormaster_type(WORKFLOW_RESULT_SKIP), "pass");
    assert_eq!(harbormaster_type(WORKFLOW_RESULT_UNSOUND), "fail");
    assert_eq!(harbormaster_type(WORKFLOW_RESULT_FAIL), "fail");
    assert_eq!(harbormaster_type(99), "fail");

    assert_eq!(
        exit_code_for_overall(WORKFLOW_RESULT_OKAY),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        exit_code_for_overall(WORKFLOW_RESULT_UNSOUND),
        std::process::ExitCode::from(1)
    );
    assert_eq!(
        exit_code_for_overall(WORKFLOW_RESULT_FAIL),
        std::process::ExitCode::from(2)
    );

    assert!(check_everything_paths(false, &[]).is_ok());
    assert!(check_everything_paths(false, &args(&["a"])).is_ok());
    assert!(check_everything_paths(true, &[]).is_ok());
    let err = check_everything_paths(true, &args(&["a"])).unwrap_err();
    assert_eq!(
        err,
        "You can not specify paths with --everything. The --everything flag runs every test \
         associated with a tracked file in the working copy."
    );
}

#[test]
fn git_detection_and_status_errors_outside_repo() {
    let bare = unique_dir("git-bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(!is_inside_git_repo(&bare));
    // Outside a repo every status helper errors (covers `git_output`).
    assert!(uncommitted_paths(&bare).is_err());
    assert!(commit_range_paths(&bare, "HEAD").is_err());
    assert!(working_copy_paths(&bare, None).is_err());
    assert!(all_tracked_files(&bare).is_err());
    // Manifest dir is inside the fanout worktree repo.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(is_inside_git_repo(&manifest));
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn git_helpers_list_status_range_and_tracked() {
    let repo = unique_dir("git-repo");
    std::fs::create_dir_all(&repo).unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{args:?}: {stderr}");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@t.test"]);
    run(&["config", "user.name", "t"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-qm", "one"]);
    // Tracked, clean, no range.
    assert!(is_inside_git_repo(&repo));
    assert_eq!(all_tracked_files(&repo).unwrap(), vec!["a.txt".to_string()]);
    assert!(uncommitted_paths(&repo).unwrap().is_empty());
    // Uncommitted change plus untracked (excluded) plus rename handling.
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    assert_eq!(uncommitted_paths(&repo).unwrap(), vec!["a.txt".to_string()]);
    run(&["add", "a.txt"]);
    run(&["commit", "-qm", "two"]);
    assert!(uncommitted_paths(&repo).unwrap().is_empty());
    // Range HEAD~1...HEAD touches a.txt; bogus rev errors with the PHP
    // usage message; the magic empty tree lists every tracked file.
    assert_eq!(
        commit_range_paths(&repo, "HEAD~1").unwrap(),
        vec!["a.txt".to_string()]
    );
    assert_eq!(
        commit_range_paths(&repo, "no-such-rev-xyz").unwrap_err(),
        "Unable to find any git commit named 'no-such-rev-xyz' in this repository."
    );
    assert_eq!(
        commit_range_paths(&repo, crate::git::GIT_MAGIC_ROOT_COMMIT).unwrap(),
        vec!["a.txt".to_string()]
    );
    assert_eq!(
        working_copy_paths(&repo, Some("HEAD~1")).unwrap(),
        vec!["a.txt".to_string()]
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn validate_and_select_paths() {
    let root = unique_dir("paths");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("f.php"), "<?php\n").unwrap();
    std::fs::write(root.join(".arcconfig"), "{}").unwrap();

    // Explicit paths validate and relativize against the project root.
    let out = validate_paths(&["sub/f.php".to_string()], &root, &root).unwrap();
    assert_eq!(out, vec!["sub/f.php".to_string()]);
    let abs = root.join("sub").join("f.php").to_string_lossy().to_string();
    let out = validate_paths(&[abs], &root, &root).unwrap();
    assert_eq!(out, vec!["sub/f.php".to_string()]);
    let err = validate_paths(&["missing.php".to_string()], &root, &root).unwrap_err();
    assert_eq!(err, "Path 'missing.php' does not exist!");

    // `select_paths`: everything+paths errors; explicit wins; empty without
    // VCS passes through empty (engine reports no tests).
    let mut opts = Options {
        everything: true,
        paths: vec!["a".to_string()],
        ..Options::default()
    };
    assert!(select_paths(&opts, &root, &root).is_err());
    opts.paths.clear();
    // Temp dir has no VCS marker and is not under the worktree repo? It is
    // under /tmp, so no repo: empty selection.
    assert!(super::detect_vcs(&root).is_none());
    assert!(select_paths(&opts, &root, &root).unwrap().is_empty());
    opts.everything = false;
    opts.paths = vec!["sub/f.php".to_string()];
    assert_eq!(
        select_paths(&opts, &root, &root).unwrap(),
        vec!["sub/f.php".to_string()]
    );
    opts.paths.clear();
    assert!(super::detect_vcs(&root).is_none());
    assert!(select_paths(&opts, &root, &root).unwrap().is_empty());
    // Private project-root helper prefers the `.arcconfig` ancestor.
    assert_eq!(super::project_root_path(&root.join("sub")), root);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn coverage_collect_merge_and_render() {
    use std::collections::HashMap;
    let mut a = engine_result("a", "pass");
    a.set_coverage(HashMap::from([("f.php".to_string(), "CU".to_string())]));
    let mut b = engine_result("b", "fail");
    b.set_coverage(HashMap::from([("f.php".to_string(), "CC".to_string())]));
    let collected = collect_coverage(&[a, b]);
    assert_eq!(collected["f.php"], vec!["CU".to_string(), "CC".to_string()]);

    let (sorted, reports) = merge_file_coverage(&["asked.php".to_string()], &collected);
    // `asked.php` seeds at 0; `f.php` merges CU+CC=CC (2/2 covered).
    assert_eq!(sorted.len(), 2);
    assert_eq!(sorted[0].0, "asked.php");
    assert_eq!(sorted[0].1, 0.0);
    assert_eq!(sorted[1].0, "f.php");
    assert_eq!(sorted[1].1, 1.0);
    assert_eq!(reports["f.php"], "CC");

    let root = unique_dir("cov-render");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f.php"), "one\ntwo\n").unwrap();
    let text = render_coverage_report(&sorted, &reports, false, &root);
    assert!(text.starts_with("\nCOVERAGE REPORT\n"), "{text}");
    assert!(text.contains("    100%     f.php\n"), "{text}");
    assert!(text.contains("      0%     asked.php\n"), "{text}");
    let detailed = render_coverage_report(&sorted, &reports, true, &root);
    // UNIT-U11: plain detailed lines carry no mark letter.
    assert!(!detailed.contains("[C]"), "{detailed}");
    assert!(detailed.contains("    1   one"), "{detailed}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detailed_coverage_marks_lines() {
    // UNIT-U11: plain detailed lines carry no mark letter, just the
    // padded source line.
    let out = render_detailed_coverage_report("a\nbb\nccc", "CUXN-extra");
    assert!(out.contains("    1   a "), "{out}");
    assert!(out.contains("    2   bb "), "{out}");
    assert!(out.contains("    3   ccc "), "{out}");
    assert!(!out.contains('['), "{out}");
    let out = render_detailed_coverage_report("only", "");
    assert!(out.contains("    1   only "), "{out}");
}

#[test]
fn json_and_modern_dicts() {
    let mut r = engine_result("t", "pass");
    r.set_namespace("ns");
    r.set_duration(0.5);
    r.set_user_data("trace");
    let value = results_to_json(std::slice::from_ref(&r));
    let arr = value.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["namespace"], "ns");
    assert_eq!(arr[0]["name"], "t");
    assert_eq!(arr[0]["result"], "pass");
    assert_eq!(arr[0]["userData"], "trace");
    assert!(arr[0]["link"].is_null());

    let modern = modern_unit_value(&arr[0]);
    assert!(modern.get("userData").is_none());
    assert_eq!(modern["details"], "trace");
    assert!(modern.get("link").is_none());
    assert_eq!(modern["name"], "t");

    // Empty userData drops without `details`; non-objects pass through.
    let mut bare = engine_result("b", "fail");
    bare.set_user_data("");
    let arr = results_to_json(std::slice::from_ref(&bare));
    let modern = modern_unit_value(&arr[0]);
    assert!(modern.get("details").is_none());
    let num = serde_json::json!(3);
    assert_eq!(modern_unit_value(&num), num);

    // Workflow output formats.
    let root = unique_dir("wf-out");
    std::fs::create_dir_all(&root).unwrap();
    let full = render_workflow_output(&[r.clone()], &[], "full", false, &root);
    assert!(full.contains("  PASS"), "{full}");
    let json = render_workflow_output(&[r.clone()], &[], "json", false, &root);
    assert!(json.contains("\"name\": \"t\""), "{json}");
    assert!(json.ends_with('\n'));
    let ugly = render_workflow_output(&[r], &[], "ugly", false, &root);
    assert!(ugly.contains("\"name\":\"t\""), "{ugly}");
    // Like PHP `json_encode`, ugly carries no trailing newline.
    assert!(!ugly.ends_with('\n'), "{ugly}");
    assert_eq!(render_workflow_output(&[], &[], "none", false, &root), "");
    assert_eq!(render_workflow_output(&[], &[], "bogus", false, &root), "");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn u21_sendmessage_params_keep_php_key_order() {
    // T1153 UNIT-U21: PHP sends `harbormaster.sendmessage` params in
    // insertion order (`ArcanistUnitWorkflow.php:424-426` at ceb), with
    // `toDictionary` order inside each unit dict
    // (`ArcanistUnitTestResult.php:164-175`) and `details` trailing
    // after the modern rename (`ArcanistWorkflow.php:2243-2253`).
    let mut r = engine_result("t", "pass");
    r.set_namespace("ns");
    r.set_link("l");
    r.set_duration_int(3);
    r.set_extra_data_ordered(vec![
        ("zeta".to_string(), "1".to_string()),
        ("alpha".to_string(), "2".to_string()),
    ]);
    r.set_user_data("trace");
    r.set_coverage_ordered(vec![("b.rs".to_string(), "C".to_string())]);
    let params = sendmessage_params("PHID-TARGET", 0, std::slice::from_ref(&r));
    let top: Vec<&str> = params
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(top, vec!["buildTargetPHID", "unit", "type"]);
    let unit = &params["unit"][0];
    let keys: Vec<&str> = unit
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "namespace",
            "name",
            "link",
            "result",
            "duration",
            "extra",
            "coverage",
            "details",
        ]
    );
    // Nested dicts keep producer order too (not sorted).
    let extra: Vec<&str> = unit["extra"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(extra, vec!["zeta", "alpha"]);
}

#[test]
fn upload_requires_conduit() {
    let r = engine_result("t", "pass");
    // Invalid URI plus no token: deterministic conduit error, no network.
    let err = upload_results(
        "PHID-1",
        0,
        std::slice::from_ref(&r),
        Some("https://invalid.invalid/"),
    )
    .unwrap_err();
    assert!(
        err.contains("no token") || err.contains("transport") || err.contains("conduit"),
        "{err}"
    );
}

#[test]
fn binaries_arcconfig_discovery_and_files() {
    assert!(binary_exists("cargo"));
    assert!(!binary_exists("definitely-not-a-binary-xyz-123"));
    assert!(resolve_binary_path("cargo").is_some());
    assert!(resolve_binary_path("definitely-not-a-binary-xyz-123").is_none());

    let root = unique_dir("arcconfig");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("f.txt"), "x").unwrap();
    assert!(read_arcconfig(&forward_slashes(&root)).is_none());
    assert!(arcconfig_string(&forward_slashes(&root), "unit.engine").is_none());

    std::fs::write(
        root.join(".arcconfig"),
        r#"{"unit.engine": "NoseTestEngine", "unit.csharp.discovery": {"src/(.*)/": [["$1.Tests.csproj", "$1.Tests.dll"]]}}"#,
    )
    .unwrap();
    let root_s = forward_slashes(&root);
    assert!(read_arcconfig(&root_s).is_some());
    assert_eq!(
        arcconfig_string(&root_s, "unit.engine").as_deref(),
        Some("NoseTestEngine")
    );
    let rules = parse_discovery_rules(
        read_arcconfig(&root_s)
            .unwrap()
            .get("unit.csharp.discovery")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].regex, "src/(.*)/");
    assert_eq!(rules[0].targets[0].project, "$1.Tests.csproj");
    assert!(parse_discovery_rules(&serde_json::json!({"r": "not-array"})).is_none());
    assert!(parse_discovery_rules(&serde_json::json!([])).is_none());

    let files = all_files_under(&root_s);
    assert!(files.contains(&".arcconfig".to_string()), "{files:?}");
    assert!(files.contains(&"sub/f.txt".to_string()), "{files:?}");

    // Phpunit engine honors merged binary/config keys (UNIT-U13
    // follow-through): the project file counts at a working-copy root,
    // and shadow options keep ambient config out.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let shadow = shadow_opts(&root);
    let merged_for =
        |root: &std::path::Path| arc_rs::get_config::load_merged(root, None, &shadow).unwrap();
    let merged = merged_for(&root);
    let engine = phpunit_engine_from_merged(&root_s, Some(&merged));
    assert_eq!(engine.binary, "phpunit");
    assert!(engine.config_file.is_none());
    std::fs::write(root.join("phpunit.xml"), "<x/>").unwrap();
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"phpunit_config": "phpunit.xml", "unit.phpunit.binary": "my-phpunit"}"#,
    )
    .unwrap();
    let merged = merged_for(&root);
    let engine = phpunit_engine_from_merged(&root_s, Some(&merged));
    assert!(engine.config_file.is_some());
    assert!(engine.binary.ends_with("my-phpunit"), "{}", engine.binary);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn xunit_and_csharp_runs_cover_stages() {
    // Bare temp project: missing toolchain/discovery errors (no PATH hacks).
    let root = unique_dir("xunit-run");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    assert!(run_xunit_with_runner(&ctx, &runner).is_err());
    assert!(run_csharp_with_runner(&ctx, &runner).is_err());

    // Direct stage helpers with a fake env (covers generate/build/test).
    let env = crate::unit_xunit::XunitEnvironment {
        build_engine: "fake-msbuild".to_string(),
        runtime_engine: String::new(),
        test_engine: "fake-xunit".to_string(),
        project_root: root_s.clone(),
        discovery_rules: Vec::new(),
    };
    // No Build/Protobuild: generate is empty.
    assert!(super::run_xunit_generate(&env, &runner).unwrap().is_empty());
    // Build stage maps exit 0/1; hard errors propagate.
    let projects = vec![crate::unit_xunit::TestProject {
        project: format!("{root_s}/A.Tests.csproj"),
        assembly: format!("{root_s}/A.Tests.dll"),
    }];
    let built = super::run_xunit_build(&env, &projects, &runner).unwrap();
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].get_result(), "pass");
    let failing = |_: &[String], _: &str| CapturedOutput::new(1, "build log", "");
    let built = super::run_xunit_build(&env, &projects, &failing).unwrap();
    assert_eq!(built[0].get_result(), "fail");
    let hard = |_: &[String], _: &str| CapturedOutput::new(3, "", "boom");
    assert!(super::run_xunit_build(&env, &projects, &hard).is_err());
    // Test stage tolerates missing XML reports.
    let tested = super::run_xunit_test(&env, &projects, &runner, false).unwrap();
    assert!(tested.is_empty());
    // Random stems are 10 chars.
    assert_eq!(super::random_xml_stem().len(), 10);

    // Configured-but-missing phpunit config surfaces verbatim. The
    // project file counts at a working-copy root (UNIT-U13
    // follow-through reads through the merge).
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"phpunit_config": "missing.xml"}"#,
    )
    .unwrap();
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: vec!["src/Calc.php".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let err = run_single_with_runner("PhpunitTestEngine", &ctx, &runner).unwrap_err();
    let message = err.message();
    assert!(
        message.contains("PHPUnit configuration file was not found"),
        "{message}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// C# project fixture: a working-copy root (bare `.git`, so the
/// project `.arcconfig` counts) with discovery rules, an xunit hint,
/// and the hinted files on disk.
fn xunit_fixture(tag: &str) -> TempDir {
    let dir = TempDir::new(tag);
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join("Tests.csproj"), "<x/>\n").unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::write(dir.path().join("tools/xunit.exe"), "fake\n").unwrap();
    std::fs::write(
        dir.path().join(".arcconfig"),
        r#"{"unit.csharp.discovery": {"src/": [["Tests.csproj", "Tests.dll"]]}, "unit.csharp.xunit.binary": "tools/xunit.exe"}"#,
    )
    .unwrap();
    dir
}

#[test]
fn xunit_plan_builds_past_injected_toolchain() {
    // The toolchain probe is injected true: the resolve/file closures
    // run, and discovery maps the run path to the staged project.
    let dir = xunit_fixture("xunit-plan");
    let root_s = forward_slashes(dir.path());
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: vec!["src/foo.cs".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let plan = prepare_xunit_run_with_merged_and_exists(&ctx, None, &|_| true).unwrap();
    assert_eq!(plan.env.build_engine, "msbuild");
    assert_eq!(plan.projects.len(), 1);
    assert_eq!(plan.projects[0].project, format!("{root_s}/Tests.csproj"));
}

#[test]
fn xunit_run_executes_stages_past_injected_toolchain() {
    // Past the injected toolchain the plan runs generate/build/test
    // through the canned runner; no mapped projects returns empty.
    let dir = xunit_fixture("xunit-run-ok");
    let root_s = forward_slashes(dir.path());
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: vec!["src/foo.cs".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let results = run_xunit_with_runner_and_exists(&ctx, &runner, &|_| true).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "pass");
    let empty = EngineContext {
        project_root: root_s.clone(),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    assert!(
        run_xunit_with_runner_and_exists(&empty, &runner, &|_| true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn csharp_run_executes_stages_past_injected_toolchain() {
    // Same staging for the C# cover path (coverage disabled, so the
    // plain xUnit future runs); no mapped projects returns empty.
    let dir = xunit_fixture("csharp-run-ok");
    let root_s = forward_slashes(dir.path());
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: vec!["src/foo.cs".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let results = run_csharp_with_runner_and_exists(&ctx, &runner, &|_| true).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "pass");
    let empty = EngineContext {
        project_root: root_s.clone(),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    assert!(
        run_csharp_with_runner_and_exists(&empty, &runner, &|_| true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn csharp_run_generate_failure_short_circuits_later_stages() {
    // Both Protobuild markers (`Build/` plus root `Protobuild.exe`)
    // stage the generate stage; a nonzero Protobuild exit
    // short-circuits build+test (early-Ok), so the runner sees exactly
    // one call and the single "fail" carries the regenerate name.
    let dir = xunit_fixture("csharp-gen-fail");
    std::fs::create_dir_all(dir.path().join("Build")).unwrap();
    std::fs::write(dir.path().join("Protobuild.exe"), "fake\n").unwrap();
    let calls = AtomicUsize::new(0);
    let runner = |_: &[String], _: &str| {
        calls.fetch_add(1, Ordering::SeqCst);
        CapturedOutput::new(1, "", "")
    };
    let ctx = EngineContext {
        project_root: forward_slashes(dir.path()),
        paths: vec!["src/foo.cs".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let results = run_csharp_with_runner_and_exists(&ctx, &runner, &|_| true).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "fail");
    assert!(results[0].get_name().contains("regenerate"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn csharp_run_build_failure_skips_test_stage() {
    // No Protobuild markers skips generate; a nonzero msbuild exit
    // then short-circuits the test stage: one call, one "fail".
    let dir = xunit_fixture("csharp-build-fail");
    let calls = AtomicUsize::new(0);
    let runner = |_: &[String], _: &str| {
        calls.fetch_add(1, Ordering::SeqCst);
        CapturedOutput::new(1, "", "")
    };
    let ctx = EngineContext {
        project_root: forward_slashes(dir.path()),
        paths: vec!["src/foo.cs".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let results = run_csharp_with_runner_and_exists(&ctx, &runner, &|_| true).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "fail");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn xunit_prepare_run_all_walks_project_files() {
    // run_all_tests walks the whole tree instead of ctx paths; the
    // walked src/ file maps to the csproj, staging the walker arm.
    let dir = xunit_fixture("xunit-run-all");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/foo.cs"), "class X {}\n").unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(dir.path()),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: true,
    };
    let plan = prepare_xunit_run_with_merged_and_exists(&ctx, None, &|_| true).unwrap();
    assert_eq!(plan.projects.len(), 1);
    assert!(format!("{:?}", plan.projects[0]).contains("Tests.csproj"));
}

#[test]
fn run_opts_in_config_selection_with_arcunit_fixture() {
    // A `.arcunit` fixture stages the Config dispatch: zero engines
    // means nothing spawns; the empty merge reports NoEffect (exit 0).
    let dir = TempDir::new("arcunit-run");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{args:?}: {stderr}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t.test"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "one"]);
    std::fs::write(dir.path().join(".arcunit"), "{\"engines\": {}}").unwrap();
    assert_eq!(
        run_opts_in(&Options::default(), dir.path()),
        ExitCode::SUCCESS
    );
}

// --- Gap2: hg/svn selection (U1/U2) ---

#[test]
fn hg_symbol_escape_quotes_non_symbols() {
    assert_eq!(hg_symbol_escape("abc123"), "abc123");
    assert_eq!(hg_symbol_escape("tip"), "tip");
    assert_eq!(hg_symbol_escape("my-branch"), "'my-branch'");
    assert_eq!(hg_symbol_escape("a'b\\c"), "'a\\'b\\\\c'");
    assert_eq!(hg_symbol_escape(""), "''");
}

#[test]
fn hg_base_revsets_quote_then_raw() {
    assert_eq!(
        hg_base_revsets("tip"),
        ["ancestor(tip,.)".to_string(), "ancestor(tip,.)".to_string()]
    );
    assert_eq!(
        hg_base_revsets("my-branch"),
        [
            "ancestor('my-branch',.)".to_string(),
            "ancestor(my-branch,.)".to_string()
        ]
    );
}

#[test]
fn hg_status_paths_omits_untracked() {
    let stdout = "M mod.py\nA add.py\nR del.py\n! gone.py\n? new.py\nC clean.py\nI ign.py\nA copy.py\n  orig.py\n";
    assert_eq!(
        super::hg_status_paths(stdout).unwrap(),
        args(&[
            "add.py", "clean.py", "copy.py", "del.py", "gone.py", "ign.py", "mod.py",
        ])
    );
    assert!(super::hg_status_paths("").unwrap().is_empty());
    assert!(super::hg_status_paths("bogus line").is_err());
}

#[test]
fn svn_rev_is_a_usage_error() {
    let dir = unique_dir("svn-rev");
    std::fs::create_dir_all(&dir).unwrap();
    // The guard fires before any binary runs: system backend, no PATH
    // assumption.
    let err = svn_working_copy_paths_with(&dir, &crate::svn::SvnCommand::system(), Some("123"))
        .unwrap_err();
    assert_eq!(
        err,
        "This version control system does not support commit ranges."
    );
    std::fs::remove_dir_all(&dir).ok();
}

// --- Gap2: cargo extension behavior (U3) ---

#[test]
fn cargo_argv_appends_paths_as_filters() {
    assert_eq!(cargo_test_argv(&Options::default()), args(&["test"]));
    let opts = parse_args(&args(&["--", "--nocapture"])).unwrap();
    assert_eq!(cargo_test_argv(&opts), args(&["test", "--nocapture"]));
    let opts = Options {
        paths: args(&["adds_", "other"]),
        ..Options::default()
    };
    assert_eq!(cargo_test_argv(&opts), args(&["test", "adds_", "other"]));
    // UNIT-U6: post-`--` tokens are paths, so argv keeps CLI order.
    let opts = parse_args(&args(&["flt", "--", "--", "--nocapture"])).unwrap();
    assert_eq!(
        cargo_test_argv(&opts),
        args(&["test", "flt", "--", "--nocapture"])
    );
    // Hand-built `cargo_args` still lead (Rust extension).
    let opts = Options {
        cargo_args: args(&["--nocapture"]),
        paths: args(&["flt"]),
        ..Options::default()
    };
    assert_eq!(
        cargo_test_argv(&opts),
        args(&["test", "--nocapture", "flt"])
    );
}

#[test]
fn cargo_coverage_warning_names_the_flag() {
    assert_eq!(cargo_coverage_warning(&Options::default()), None);
    let mut opts = Options {
        no_coverage: true,
        ..Options::default()
    };
    assert_eq!(cargo_coverage_warning(&opts), None);
    opts.coverage = true;
    assert_eq!(
        cargo_coverage_warning(&opts).as_deref(),
        Some("arc unit: warning: the cargo engine does not support coverage; ignoring --coverage")
    );
    opts.detailed_coverage = true;
    assert_eq!(
        cargo_coverage_warning(&opts).as_deref(),
        Some(
            "arc unit: warning: the cargo engine does not support coverage; ignoring --detailed-coverage"
        )
    );
}

// --- Gap2: documented extensions stay (U7/U8) ---

#[test]
fn unknown_engine_message_lists_every_engine() {
    let opts = Options {
        engine: "Nope".to_string(),
        ..Options::default()
    };
    // UNIT-U8: PHP's subclass message.
    assert_eq!(
        execute(&opts, Path::new("."), None).unwrap_err(),
        "Configured unit test engine \"Nope\" is not a subclass of \
         \"ArcanistUnitTestEngine\", but must be."
    );
    // UNIT-U7: nothing configured is `ArcanistNoEngineException`, not a
    // cargo default; cargo still selects when named.
    let dir = unique_dir("cargo-default");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    assert!(err.contains("No unit test engine is configured"), "{err}");
    let explicit = parse_args(&args(&["--engine", "cargo"])).unwrap();
    assert_eq!(
        discover_selection(&explicit, &dir).unwrap(),
        Selection::Cargo
    );
    std::fs::write(dir.join(".arcconfig"), r#"{"unit.engine": "cargo"}"#).unwrap();
    assert_eq!(
        discover_selection(&Options::default(), &dir).unwrap(),
        Selection::Cargo
    );
    std::fs::remove_dir_all(&dir).ok();
}

// --- Gap2: PHP byte-compatible JSON (U9) ---

#[test]
fn json_text_escapes_slashes_only_when_ugly() {
    let result = engine_result("a/b", "pass");
    let ugly = results_to_json_text(std::slice::from_ref(&result), false);
    assert_eq!(
        ugly,
        "[{\"namespace\":null,\"name\":\"a\\/b\",\"link\":null,\"result\":\"pass\",\
         \"duration\":null,\"extra\":null,\"userData\":null,\"coverage\":null}]"
    );
    let pretty = results_to_json_text(std::slice::from_ref(&result), true);
    assert_eq!(
        pretty,
        "{\n  \"0\": {\n    \"namespace\": null,\n    \"name\": \"a/b\",\n    \
         \"link\": null,\n    \"result\": \"pass\",\n    \"duration\": null,\n    \
         \"extra\": null,\n    \"userData\": null,\n    \"coverage\": null\n  }\n}\n"
    );
}

#[test]
fn json_text_escapes_controls_and_unicode() {
    let result = engine_result("q\"\\\n\r\t\u{8}\u{c}\u{1}é😀", "fail");
    let ugly = results_to_json_text(std::slice::from_ref(&result), false);
    assert!(
        ugly.contains("\"name\":\"q\\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u00e9\\ud83d\\ude00\""),
        "{ugly}"
    );
    // Pretty escapes identically apart from slashes.
    let pretty = results_to_json_text(std::slice::from_ref(&result), true);
    assert!(
        pretty.contains("\"name\": \"q\\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u00e9\\ud83d\\ude00\""),
        "{pretty}"
    );
}

#[test]
fn json_text_renders_nested_maps_and_lists() {
    use std::collections::HashMap;
    let mut one = engine_result("one", "pass");
    one.set_coverage(HashMap::from([(
        "src/App.cs".to_string(),
        "CC".to_string(),
    )]));
    let two = engine_result("two", "fail");
    let ugly = results_to_json_text(&[one.clone(), two.clone()], false);
    assert!(ugly.starts_with("[{"), "{ugly}");
    assert!(ugly.contains("},{\""), "{ugly}");
    assert!(ugly.ends_with("}]"), "{ugly}");

    let pretty = results_to_json_text(&[one, two], true);
    assert_eq!(
        pretty,
        "{\n  \"0\": {\n    \"namespace\": null,\n    \"name\": \"one\",\n    \
         \"link\": null,\n    \"result\": \"pass\",\n    \"duration\": null,\n    \
         \"extra\": null,\n    \"userData\": null,\n    \"coverage\": {\n      \
         \"src/App.cs\": \"CC\"\n    }\n  },\n  \"1\": {\n    \
         \"namespace\": null,\n    \"name\": \"two\",\n    \"link\": null,\n    \
         \"result\": \"fail\",\n    \"duration\": null,\n    \"extra\": null,\n    \
         \"userData\": null,\n    \"coverage\": null\n  }\n}\n"
    );
}

#[test]
fn json_text_orders_keys_and_renders_empty_maps() {
    use std::collections::HashMap;
    let mut r = engine_result("t", "pass");
    r.set_duration(2.0);
    r.set_extra_data(HashMap::new());
    r.set_coverage(HashMap::from([
        ("b.php".to_string(), "U".to_string()),
        ("a.php".to_string(), "C".to_string()),
    ]));
    let ugly = results_to_json_text(std::slice::from_ref(&r), false);
    assert!(ugly.contains("\"duration\":2.0"), "{ugly}");
    assert!(ugly.contains("\"extra\":[]"), "{ugly}");
    assert!(
        ugly.contains("\"coverage\":{\"a.php\":\"C\",\"b.php\":\"U\"}"),
        "{ugly}"
    );
    let mut last = 0;
    for key in [
        "namespace",
        "name",
        "link",
        "result",
        "duration",
        "extra",
        "userData",
        "coverage",
    ] {
        let at = ugly.find(&format!("\"{key}\"")).unwrap();
        assert!(at > last, "{ugly}");
        last = at;
    }
    let mut short = engine_result("t", "pass");
    short.set_duration(0.125);
    let ugly = results_to_json_text(std::slice::from_ref(&short), false);
    assert!(ugly.contains("\"duration\":0.125"), "{ugly}");

    assert_eq!(results_to_json_text(&[], false), "[]");
    assert_eq!(results_to_json_text(&[], true), "{}\n");
}

// --- Gap2: cscover flow (U4) plus full Ok-path runs (U10) ---

const ASSEMBLY_XML: &str = "<assembly>\
    <test name=\"A.T1\" time=\"0.5\" result=\"Pass\"/>\
    <test name=\"A.T2\" time=\"0.25\" result=\"Fail\"/>\
    </assembly>";

fn csharp_cover_xml(root_s: &str) -> String {
    format!(
        "<coverage>\
        <instrumented file=\"{root_s}/src/App.cs\" start=\"1\" end=\"4\"/>\
        <executed file=\"{root_s}/src/App.cs\" start=\"1\" end=\"2\"/>\
        </coverage>"
    )
}

fn fake_xunit_plan(root_s: &str, assembly: &str) -> XunitPlan {
    XunitPlan {
        env: crate::unit_xunit::XunitEnvironment {
            build_engine: "fake-msbuild".to_string(),
            runtime_engine: String::new(),
            test_engine: "fake-xunit".to_string(),
            project_root: root_s.to_string(),
            discovery_rules: Vec::new(),
        },
        projects: vec![crate::unit_xunit::TestProject {
            project: format!("{root_s}/A.Tests.csproj"),
            assembly: assembly.to_string(),
        }],
        is_windows: false,
    }
}

fn staged_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn csharp_cover_run_wraps_xunit_and_reads_coverage() {
    use std::cell::RefCell;
    use std::collections::HashMap;

    let root = unique_dir("csharp-cover");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("App.Tests.dll"), "dll").unwrap();
    std::fs::write(bin.join("Dep.dll"), "dll").unwrap();
    std::fs::write(bin.join("Skip.dll"), "dll").unwrap();
    std::fs::write(bin.join("readme.txt"), "txt").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("App.cs"), "one\ntwo\nthree\nfour\n").unwrap();
    let root_s = forward_slashes(&root);
    let bindir = format!("{root_s}/bin");
    let assembly = format!("{bindir}/App.Tests.dll");
    let plan = fake_xunit_plan(&root_s, &assembly);
    let cover = super::CsharpCover {
        cover_engine: "fake-cscover".to_string(),
        match_regex: None,
        excluded: Some(HashMap::from([("Skip.dll".to_string(), "1".to_string())])),
    };

    let cover_xml = csharp_cover_xml(&root_s);
    let seen = RefCell::new(Vec::new());
    let runner = |argv: &[String], _cwd: &str| {
        seen.borrow_mut().push(argv.to_vec());
        let o = argv.iter().position(|a| a == "-o").unwrap();
        std::fs::write(&argv[o + 1], &cover_xml).unwrap();
        let xml = argv
            .iter()
            .position(|a| a == "/xml")
            .map(|i| &argv[i + 1])
            .unwrap();
        let w = argv.iter().position(|a| a == "-w").unwrap();
        std::fs::write(format!("{}/{xml}", argv[w + 1]), ASSEMBLY_XML).unwrap();
        CapturedOutput::new(0, "", "")
    };
    let results = super::run_csharp_test(&plan, Some(&cover), None, &runner).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].get_name(), "A.T1");
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(results[1].get_result(), "fail");
    assert_eq!(
        results[0]
            .get_coverage()
            .unwrap()
            .get("src/App.cs")
            .unwrap(),
        "CCUU"
    );

    // Full cscover argv: cover first (empty runtime omitted), then
    // -o/-c/-a/-w plus the instrumented assemblies (Skip.dll excluded,
    // readme.txt not an assembly).
    let calls = seen.borrow();
    assert_eq!(calls.len(), 1);
    let argv = &calls[0];
    assert_eq!(argv.len(), 13);
    assert_eq!(argv[0], "fake-cscover");
    assert_eq!(argv[1], "-o");
    let out_path = &argv[2];
    assert!(
        out_path.starts_with(&root_s) && out_path.ends_with(".xml"),
        "{out_path}"
    );
    assert_eq!(argv[3], "-c");
    assert_eq!(argv[4], "fake-xunit");
    assert_eq!(argv[5], "-a");
    assert_eq!(argv[6], assembly);
    assert_eq!(argv[7], "/xml");
    assert!(argv[8].ends_with(".results.xml"), "{}", argv[8]);
    assert_eq!(argv[9], "-w");
    assert_eq!(argv[10], bindir);
    assert_eq!(argv[11], format!("{bindir}/App.Tests.dll"));
    assert_eq!(argv[12], format!("{bindir}/Dep.dll"));

    // Staged reports are removed after parsing.
    assert!(
        staged_leftovers(&root)
            .iter()
            .all(|n| !n.starts_with(".cscover-"))
    );
    assert!(
        staged_leftovers(&bin)
            .iter()
            .all(|n| !n.ends_with(".results.xml"))
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn csharp_plain_fallback_skips_cscover() {
    let root = unique_dir("csharp-plain");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let assembly = format!("{root_s}/bin/App.Tests.dll");
    let plan = fake_xunit_plan(&root_s, &assembly);
    let runner = |argv: &[String], _cwd: &str| {
        assert!(!argv.contains(&"-o".to_string()), "{argv:?}");
        let at = argv.iter().position(|a| a == "/xml").unwrap();
        std::fs::write(format!("{root_s}/{}", argv[at + 1]), ASSEMBLY_XML).unwrap();
        CapturedOutput::new(0, "", "")
    };
    // Coverage disabled: no cover engine, plain xUnit reports under the
    // project root, no coverage attached.
    let results = super::run_csharp_test(&plan, None, Some(false), &runner).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].get_coverage().is_none());
    assert_eq!(staged_leftovers(&root).len(), 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn csharp_missing_reports_contribute_nothing() {
    use std::collections::HashMap;

    let root = unique_dir("csharp-missing");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("App.Tests.dll"), "dll").unwrap();
    let root_s = forward_slashes(&root);
    let assembly = format!("{root_s}/bin/App.Tests.dll");
    let plan = fake_xunit_plan(&root_s, &assembly);
    let cover = super::CsharpCover {
        cover_engine: "fake-cscover".to_string(),
        match_regex: None,
        excluded: Some(HashMap::new()),
    };
    // The runner writes nothing (Mono segfault tolerance): no results,
    // and the staged cover file is still removed.
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let results = super::run_csharp_test(&plan, Some(&cover), None, &runner).unwrap();
    assert!(results.is_empty());
    assert!(
        staged_leftovers(&root)
            .iter()
            .all(|n| !n.starts_with(".cscover-"))
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn csharp_cover_config_loads_and_errors() {
    let root = unique_dir("csharp-cfg");
    std::fs::create_dir_all(root.join("tools")).unwrap();
    std::fs::write(root.join("tools").join("cscover.exe"), "x").unwrap();
    let root_s = forward_slashes(&root);
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: Vec::new(),
        enable_coverage: None,
        run_all_tests: false,
    };
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"unit.csharp.cscover.binary": "tools/cscover.exe",
            "unit.csharp.coverage.match": "App",
            "unit.csharp.coverage.excluded": {"Skip.dll": "1"}}"#,
    )
    .unwrap();
    let cover = super::load_csharp_cover(&ctx, None).unwrap().unwrap();
    assert!(
        cover.cover_engine.ends_with("tools/cscover.exe"),
        "{}",
        cover.cover_engine
    );
    assert_eq!(cover.match_regex.as_deref(), Some("App"));
    assert!(cover.excluded.unwrap().contains_key("Skip.dll"));

    let off = EngineContext {
        enable_coverage: Some(false),
        ..ctx.clone()
    };
    assert!(super::load_csharp_cover(&off, None).unwrap().is_none());

    std::fs::write(root.join(".arcconfig"), "{}").unwrap();
    let err = super::load_csharp_cover(&ctx, None).unwrap_err();
    let message = err.message();
    assert!(message.contains("unit.csharp.coverage.binary"), "{message}");

    std::fs::write(
        root.join(".arcconfig"),
        r#"{"unit.csharp.cscover.binary": "nope.exe"}"#,
    )
    .unwrap();
    let err = super::load_csharp_cover(&ctx, None).unwrap_err();
    let message = err.message();
    assert!(message.contains("have you built yet"), "{message}");

    assert!(super::parse_excluded_files(&serde_json::json!(["a"])).is_none());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn xunit_plan_runs_stages_with_fakes() {
    let root = unique_dir("xunit-plan");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let assembly = format!("{root_s}/bin/A.Tests.dll");
    let plan = fake_xunit_plan(&root_s, &assembly);
    let runner = |argv: &[String], _cwd: &str| {
        if argv[0] == "fake-xunit" {
            let at = argv.iter().position(|a| a == "/xml").unwrap();
            std::fs::write(
                format!("{root_s}/{}", argv[at + 1]),
                "<assembly><test name=\"A.T1\" time=\"0.5\" result=\"Pass\"/>\
                 <test name=\"A.T2\" time=\"0\" result=\"Skip\"/></assembly>",
            )
            .unwrap();
        }
        CapturedOutput::new(0, "", "")
    };
    // No Build dir: empty generate, passing build, two parsed tests.
    let results = super::run_xunit_plan(&plan, &runner).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].get_name(),
        format!("(build) {root_s}/A.Tests.csproj")
    );
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(results[1].get_name(), "A.T1");
    assert_eq!(results[2].get_result(), "skip");
    assert_eq!(staged_leftovers(&root).len(), 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn phutil_run_parses_canned_output_and_cleans_up() {
    use std::cell::RefCell;

    let root = unique_dir("phutil-run");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: vec!["a.php".to_string()],
        enable_coverage: None,
        run_all_tests: false,
    };
    let seen = RefCell::new(Vec::new());
    let runner = |argv: &[String], cwd: &str| {
        seen.borrow_mut().push((argv.to_vec(), cwd.to_string()));
        assert_eq!(argv[0], "fake-php");
        assert!(argv[1].ends_with(".php") && Path::new(&argv[1]).exists());
        let input_text = std::fs::read_to_string(&argv[2]).unwrap();
        let input: serde_json::Value = serde_json::from_str(&input_text).unwrap();
        assert_eq!(
            input["projectRoot"],
            serde_json::Value::String(root_s.clone())
        );
        assert_eq!(input["paths"], serde_json::json!(["a.php"]));
        assert!(input["enableCoverage"].is_null());
        std::fs::write(
            &argv[3],
            r#"{"results": [{"name": "t", "result": "pass", "duration": 1.5}]}"#,
        )
        .unwrap();
        CapturedOutput::new(0, "", "")
    };
    let results = run_single_with_php("phutil", &ctx, &runner, &|| {
        Some(vec!["fake-php".to_string()])
    })
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(results[0].get_duration(), Some(1.5));
    let calls = seen.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.len(), 4);
    assert_eq!(calls[0].1, root_s);
    let argv = &calls[0].0;
    assert!(argv[2].contains(".arc-unit-phutil-input-"), "{argv:?}");
    assert_eq!(staged_leftovers(&root).len(), 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn phutil_run_maps_driver_failures() {
    let root = unique_dir("phutil-fail");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let ctx = EngineContext {
        project_root: root_s,
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: true,
    };
    let php = || Some(vec!["fake-php".to_string()]);
    macro_rules! run {
        ($runner:expr) => {
            run_single_with_php("PhutilUnitTestEngine", &ctx, &$runner, &php).unwrap_err()
        };
    }
    let err = run!(|_: &[String], _: &str| CapturedOutput::new(3, "out", "bad thing"));
    assert_eq!(err.message(), "phutil driver failed: bad thing");
    let err = run!(|_: &[String], _: &str| CapturedOutput::new(3, "out only", ""));
    assert_eq!(err.message(), "phutil driver failed: out only");
    let err = run!(|_: &[String], _: &str| CapturedOutput::new(3, "", ""));
    assert_eq!(err.message(), "phutil driver failed with exit code 3");
    let err = run!(|_: &[String], _: &str| CapturedOutput::new(0, "", ""));
    assert!(err.message().contains("did not write"), "{}", err.message());
    let err = run!(|argv: &[String], _: &str| {
        std::fs::write(&argv[3], r#"{"error": "no init"}"#).unwrap();
        CapturedOutput::new(0, "", "")
    });
    assert!(matches!(err, crate::unit_engine::EngineError::Usage(_)));
    assert_eq!(err.message(), "no init");
    let err = run!(|argv: &[String], _: &str| {
        std::fs::write(&argv[3], r#"{"results": []}"#).unwrap();
        CapturedOutput::new(0, "", "")
    });
    assert!(err.is_no_effect());
    assert_eq!(staged_leftovers(&root).len(), 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn assembly_parent_dir_falls_back_to_root() {
    assert_eq!(super::assembly_parent_dir("/r/bin/App.dll", "/r"), "/r/bin");
    assert_eq!(super::assembly_parent_dir("App.dll", "/r"), "/r");
}

#[test]
fn default_php_prefix_matches_path_probe() {
    assert_eq!(
        super::resolve_php_prefix(),
        crate::unit_phutil::php_argv_prefix()
    );
}

#[test]
fn single_php_seam_rejects_unknown_engines() {
    let ctx = EngineContext::new(&forward_slashes(&std::env::temp_dir()));
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let err = run_single_with_php("Bogus", &ctx, &runner, &|| None).unwrap_err();
    // UNIT-U8: PHP's subclass message.
    let message = err.message();
    assert!(message.contains("is not a subclass of"), "{message}");
}

#[test]
fn single_wrapper_matches_php_seam_for_non_phutil() {
    let root = unique_dir("wrap-parity");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::create_dir_all(root.join("tests").join("pkg")).unwrap();
    std::fs::write(root.join("pkg").join("mod.py"), "x\n").unwrap();
    std::fs::write(root.join("tests").join("pkg").join("test_mod.py"), "t\n").unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(&root),
        paths: vec!["pkg/mod.py".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), CANNED_XUNIT).unwrap();
        CapturedOutput::new(1, "", "")
    };
    // The php resolver is unused outside the phutil arm.
    let plain = run_single_with_runner("NoseTestEngine", &ctx, &runner).unwrap();
    let seamed = run_single_with_php("NoseTestEngine", &ctx, &runner, &|| None).unwrap();
    assert_eq!(plain, seamed);
    std::fs::remove_dir_all(&root).ok();
}

fn expected_star() -> &'static str {
    if cfg!(windows) { "*" } else { "\u{2605}" }
}

#[test]
fn result_label_badges_match_php() {
    assert_eq!(
        format_result_label("pass", true),
        "\x1b[42m\x1b[1m PASS \x1b[m\x1b[49m"
    );
    assert_eq!(
        format_result_label("fail", true),
        "\x1b[41m\x1b[1m FAIL \x1b[m\x1b[49m"
    );
    assert_eq!(
        format_result_label("skip", true),
        "\x1b[43m\x1b[1m SKIP \x1b[m\x1b[49m"
    );
    assert_eq!(
        format_result_label("broken", true),
        "\x1b[41m\x1b[1m BROKEN \x1b[m\x1b[49m"
    );
    assert_eq!(
        format_result_label("unsound", true),
        "\x1b[43m\x1b[1m UNSOUND \x1b[m\x1b[49m"
    );
    assert_eq!(format_result_label("bogus", true), "");
    // Like PHP, the stripped badge keeps its padding, and the plain
    // workflow rendering uses it (UNIT-U10).
    assert_eq!(format_result_label("pass", false), " PASS ");
}

#[test]
fn test_duration_buckets_match_php() {
    let star = expected_star();
    assert_eq!(
        format_test_duration(0.049, true),
        format!("\x1b[32m 49ms\x1b[39m\x1b[33m{star}\x1b[39m ")
    );
    assert_eq!(format_test_duration(0.051, true), "\x1b[32m 51ms\x1b[39m  ");
    assert_eq!(format_test_duration(0.3, true), "\x1b[33m300ms\x1b[39m  ");
    assert_eq!(format_test_duration(1.0, true), "\x1b[31m 1.0s\x1b[39m  ");
    assert_eq!(format_test_duration(0.3, false), "300ms  ");
}

#[test]
fn render_unit_result_with_ansi_styles_pass_line() {
    let mut pass = engine_result("t", "pass");
    pass.set_namespace("ns");
    pass.set_duration(0.125);
    assert_eq!(
        render_unit_result_with_ansi(&pass, true),
        "  \x1b[42m\x1b[1m PASS \x1b[m\x1b[49m \x1b[32m125ms\x1b[39m   ns::t\n"
    );
    // UNIT-U10: stripped markup keeps star/spaces/padding.
    assert_eq!(
        render_unit_result_with_ansi(&pass, false),
        "   PASS  125ms   ns::t\n"
    );
}

#[test]
fn render_unit_result_with_ansi_stars_fast_pass() {
    let star = expected_star();
    let mut pass = engine_result("t", "pass");
    pass.set_duration(0.005);
    assert_eq!(
        render_unit_result_with_ansi(&pass, true),
        format!(
            "  \x1b[42m\x1b[1m PASS \x1b[m\x1b[49m \x1b[32m  5ms\x1b[39m\x1b[33m{star}\x1b[39m  t\n"
        )
    );
    // UNIT-U10: the fast-run star survives stripping.
    assert_eq!(
        render_unit_result_with_ansi(&pass, false),
        format!("   PASS    5ms{star}  t\n")
    );
}

#[test]
fn render_unit_result_with_ansi_appends_user_data_raw() {
    let mut fail = engine_result("t", "fail");
    fail.set_user_data("oops");
    assert_eq!(
        render_unit_result_with_ansi(&fail, true),
        "  \x1b[41m\x1b[1m FAIL \x1b[m\x1b[49m t\noops\n"
    );
    // UNIT-U10: stripped badge keeps its padding.
    assert_eq!(
        render_unit_result_with_ansi(&fail, false),
        "   FAIL  t\noops\n"
    );
    assert_eq!(
        render_unit_result_with_ansi(&engine_result("t", "bogus"), true),
        "   t\n"
    );
}

#[test]
fn coverage_file_line_bolds_percent_on_tty() {
    assert_eq!(
        coverage_file_line(100, "f.php", true),
        "    \x1b[1m100%\x1b[m     f.php\n"
    );
    assert_eq!(
        coverage_file_line(100, "f.php", false),
        "    100%     f.php\n"
    );
    assert_eq!(
        coverage_file_line(5, "g.php", true),
        "    \x1b[1m  5%\x1b[m     g.php\n"
    );
}

#[test]
fn render_coverage_report_with_ansi_underlines_header() {
    let root = unique_dir("cov-ansi");
    let sorted = vec![("f.php".to_string(), 1.0)];
    let reports = HashMap::from([("f.php".to_string(), "C".to_string())]);
    assert_eq!(
        render_coverage_report_with_ansi(&sorted, &reports, false, &root, true),
        "\n\x1b[4mCOVERAGE REPORT\x1b[m\n    \x1b[1m100%\x1b[m     f.php\n"
    );
    assert_eq!(
        render_coverage_report_with_ansi(&sorted, &reports, false, &root, false),
        "\nCOVERAGE REPORT\n    100%     f.php\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn render_detailed_with_ansi_paints_marks() {
    let covered = format!("{:<80}", "a");
    assert_eq!(
        render_detailed_coverage_report_with_ansi("a\nbb", "CU", true),
        format!(
            "    1  \x1b[42m {covered} \x1b[49m\n    2  \x1b[41m {:<80} \x1b[49m\n",
            "bb"
        )
    );
    let magenta = format!("{:<80}", "ccc");
    assert!(
        render_detailed_coverage_report_with_ansi("ccc", "X", true)
            .contains(&format!("\x1b[45m {magenta} \x1b[49m"))
    );
    // `N`, missing bytes, and `"0"` stay plain like PHP's default arm.
    for report in ["N", "", "0"] {
        let out = render_detailed_coverage_report_with_ansi("only", report, true);
        assert_eq!(out, format!("    1   {:<80} \n", "only"));
    }
    // UNIT-U11: plain strips the backgrounds with no mark letter.
    let plain = render_detailed_coverage_report_with_ansi("a\nbb", "CU", false);
    assert!(!plain.contains("[C]"), "{plain}");
    assert!(!plain.contains("[U]"), "{plain}");
    assert!(!plain.contains('\x1b'), "{plain}");
    assert!(plain.starts_with("    1   a"), "{plain}");
}

#[test]
fn workflow_output_with_ansi_threads_flag() {
    let root = unique_dir("wf-ansi");
    std::fs::create_dir_all(&root).unwrap();
    let mut pass = engine_result("t", "pass");
    pass.set_duration(0.125);
    let full = render_workflow_output_with_ansi(&[pass.clone()], &[], "full", false, &root, true);
    assert!(
        full.contains("\x1b[42m\x1b[1m PASS \x1b[m\x1b[49m"),
        "{full}"
    );
    let plain = render_workflow_output_with_ansi(&[pass.clone()], &[], "full", false, &root, false);
    // UNIT-U10: stripped markup keeps star/spaces/padding.
    assert_eq!(plain, "   PASS  125ms   t\n");
    // Machine formats carry no markup in either mode.
    let json = render_workflow_output_with_ansi(&[pass.clone()], &[], "json", false, &root, true);
    assert!(!json.contains('\x1b'), "{json}");
    assert_eq!(
        json,
        render_workflow_output_with_ansi(&[pass.clone()], &[], "json", false, &root, false)
    );
    let ugly = render_workflow_output_with_ansi(&[pass], &[], "ugly", false, &root, true);
    assert!(!ugly.contains('\x1b'), "{ugly}");
    assert_eq!(
        render_workflow_output_with_ansi(&[], &[], "none", false, &root, true),
        ""
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn r6_unitu13_engine_reads_any_source() {
    // UNIT-U13: `unit.engine` resolves from local/user/`--config`, not
    // just `.arcconfig`. Shadow files keep ambient config out.
    let dir = unique_dir("u13-any");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let user = dir.join("arcrc.json");
    std::fs::write(&user, r#"{"config": {"unit.engine": "PytestTestEngine"}}"#).unwrap();
    assert_eq!(
        find_unit_engine_with(
            &dir,
            Some(&user),
            &arc_rs::get_config::LoadOptions::default()
        )
        .as_deref(),
        Some("PytestTestEngine")
    );
    // Project beats user; `--config` beats project.
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit.engine": "NoseTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        find_unit_engine_with(
            &dir,
            Some(&user),
            &arc_rs::get_config::LoadOptions::default()
        )
        .as_deref(),
        Some("NoseTestEngine")
    );
    let mut runtime = serde_json::Map::new();
    runtime.insert(
        "unit.engine".to_string(),
        serde_json::json!("XUnitTestEngine"),
    );
    let opts = arc_rs::get_config::LoadOptions {
        runtime,
        ..Default::default()
    };
    assert_eq!(
        find_unit_engine_with(&dir, Some(&user), &opts).as_deref(),
        Some("XUnitTestEngine")
    );
    // The project legacy `unit_engine` name resolves too.
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit_engine": "PhpunitTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        find_unit_engine_with(&dir, None, &shadow_opts(&dir)).as_deref(),
        Some("PhpunitTestEngine")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r4_unitu3_no_effect_preserved() {
    use arc_rs::conduit::EnvelopeKind;
    // `NoEffect` ("No tests to run.") routes to stdout + exit 0.
    let error = classify_engine_error(arc_rs::unit_engine::EngineError::no_effect());
    assert_eq!(error.kind, EnvelopeKind::NoEffect);
    assert_eq!(error.message, "No tests to run.");
    assert_eq!(error.kind.exit_code(), std::process::ExitCode::SUCCESS);
    // End to end through the classified config runner: no engines
    // planned means no tests to run.
    let dir = unique_dir("r4-unitu3");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.to_string_lossy().to_string();
    let err =
        run_config_with_runner_classified(&[], &root, &[], false, None, &RealRunner).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::NoEffect);
    assert_eq!(err.message, "No tests to run.");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r4_unitu4_usage_kinds() {
    use arc_rs::conduit::EnvelopeKind;
    // Engine usage errors render `Usage Exception:`.
    let error = classify_engine_error(arc_rs::unit_engine::EngineError::Usage("bad".to_string()));
    assert_eq!(error.kind, EnvelopeKind::Usage);
    // Unknown `--engine` names are usage errors.
    let opts = Options {
        engine: "NopeEngine".to_string(),
        engine_explicit: true,
        ..Options::default()
    };
    let dir = unique_dir("r4-unitu4");
    std::fs::create_dir_all(&dir).unwrap();
    let err = discover_selection_classified(&opts, &dir).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    // `--everything` with paths and missing paths are usage errors;
    // VCS failures are generic.
    assert_eq!(
        classify_select_paths_error(
            "You can not specify paths with --everything. The --everything flag runs every test associated with a tracked file in the working copy."
                .to_string()
        )
        .kind,
        EnvelopeKind::Usage
    );
    assert_eq!(
        classify_select_paths_error("Path 'x' does not exist!".to_string()).kind,
        EnvelopeKind::Usage
    );
    assert_eq!(
        classify_select_paths_error("failed to run git: boom".to_string()).kind,
        EnvelopeKind::Exception
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r6_unitu13_selection_honors_config_flags() {
    let dir = unique_dir("u13-sel");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    // `--config unit.engine=...` selects without any project file.
    let opts = parse_args(&args(&["--config", "unit.engine=PytestTestEngine"])).unwrap();
    assert_eq!(opts.config, ["unit.engine=PytestTestEngine"]);
    assert_eq!(
        discover_selection(&opts, &dir).unwrap(),
        Selection::Single("PytestTestEngine".to_string())
    );
    // `--config-file` shadows a user engine.
    let user = dir.join("arcrc.json");
    std::fs::write(&user, r#"{"config": {"unit.engine": "NoseTestEngine"}}"#).unwrap();
    let arcrc_opts = Options {
        arcrc_file: Some(user.to_string_lossy().to_string()),
        ..Options::default()
    };
    assert_eq!(
        discover_selection(&arcrc_opts, &dir).unwrap(),
        Selection::Single("NoseTestEngine".to_string())
    );
    let empty = dir.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    let shadowed = Options {
        arcrc_file: Some(user.to_string_lossy().to_string()),
        config_file: vec![empty.to_string_lossy().to_string()],
        ..Options::default()
    };
    // UNIT-U7: shadowed-out engine is `ArcanistNoEngineException`.
    let err = discover_selection(&shadowed, &dir).unwrap_err();
    assert!(err.contains("No unit test engine is configured"), "{err}");
    // A mistyped `unit.engine` errors instead of falling through.
    std::fs::write(dir.join(".arcconfig"), r#"{"unit.engine": 42}"#).unwrap();
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    assert_eq!(err, "Type of setting 'unit.engine' must be string.");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r6_core34_unit_config_flags_parse() {
    let opts = parse_args(&args(&[
        "--config",
        "a=1",
        "--config=b=2",
        "--config-file",
        "f",
        "--arcrc-file=g",
    ]))
    .unwrap();
    assert_eq!(opts.config, ["a=1", "b=2"]);
    assert_eq!(opts.config_file, ["f"]);
    assert_eq!(opts.arcrc_file.as_deref(), Some("g"));
}

#[test]
fn r4_unitu5_failed_is_generic() {
    use arc_rs::conduit::EnvelopeKind;
    let error = classify_engine_error(arc_rs::unit_engine::EngineError::Failed("boom".to_string()));
    assert_eq!(error.kind, EnvelopeKind::Exception);
    assert_eq!(error.kind.exit_code(), std::process::ExitCode::FAILURE);
}

// --- T1153 Wave 2: UNIT findings ---

#[test]
fn unit_u1_config_runs_echo_nothing_when_children_echo() {
    // Only the configuration-driven engine suppresses the workflow echo.
    assert!(!should_echo_test_results(
        "ArcanistConfigurationDrivenUnitTestEngine"
    ));
    for engine in [
        "NoseTestEngine",
        "PytestTestEngine",
        "PhpunitTestEngine",
        "PhutilUnitTestEngine",
        "phutil",
        "XUnitTestEngine",
        "CSharpToolsTestEngine",
        "cargo",
        "Bogus",
    ] {
        assert!(should_echo_test_results(engine), "{engine}");
    }

    // Canned config run: every ported child echoes, so the mask is all
    // false and the `full` rendering prints zero per-result lines.
    let root = unique_dir("u1-echo");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::create_dir_all(root.join("tests").join("pkg")).unwrap();
    std::fs::write(root.join("pkg").join("mod.py"), "x = 1\n").unwrap();
    std::fs::write(
        root.join("tests").join("pkg").join("test_mod.py"),
        "def test_ok(): pass\n",
    )
    .unwrap();
    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"n": {"type": "NoseTestEngine"}}}"#,
        "/r/.arcunit",
        &["NoseTestEngine"],
    )
    .unwrap();
    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), CANNED_XUNIT).unwrap();
        CapturedOutput::new(1, "", "")
    };
    let (report, echo) = run_config_with_echo_classified(
        &planned,
        &forward_slashes(&root),
        &["pkg/mod.py".to_string()],
        false,
        Some(false),
        &runner,
        &|| None,
    )
    .unwrap();
    assert!(!report.results.is_empty());
    assert_eq!(echo, vec![false; report.results.len()]);
    let full = render_workflow_output_with_echo(
        &report.results,
        &[],
        "full",
        false,
        &root,
        false,
        Some(&echo),
    );
    assert_eq!(full, "");

    // Coverage still renders and machine formats still carry results.
    use std::collections::HashMap;
    let mut covered = engine_result("c", "pass");
    covered.set_coverage(HashMap::from([("f.php".to_string(), "C".to_string())]));
    let full = render_workflow_output_with_echo(
        std::slice::from_ref(&covered),
        &[],
        "full",
        false,
        &root,
        false,
        Some(&[false]),
    );
    assert!(full.contains("COVERAGE REPORT"), "{full}");
    assert!(!full.contains("PASS"), "{full}");
    let ugly = render_workflow_output_with_echo(
        std::slice::from_ref(&covered),
        &[],
        "ugly",
        false,
        &root,
        false,
        Some(&[false]),
    );
    assert!(ugly.contains("\"name\":\"c\""), "{ugly}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn unit_u2_phutil_echoes_at_workflow_level() {
    // Workflow-level echo matches PHP: `shouldEchoTestResults` is true
    // for every ported engine including phutil, so single runs print
    // per-result lines. (Rule 3: PHP's in-process during-run progress
    // is libphutil-runtime-bound and unverifiable — no libphutil source
    // is in scope to pin its bytes — so the capturing runner plus the
    // single workflow-level echo is the kept equivalent.)
    assert!(should_echo_test_results("PhutilUnitTestEngine"));
    assert!(should_echo_test_results("phutil"));
    let root = unique_dir("u2-echo");
    std::fs::create_dir_all(&root).unwrap();
    let out = render_workflow_output(
        std::slice::from_ref(&engine_result("t", "pass")),
        &[],
        "full",
        false,
        &root,
    );
    assert!(out.contains("PASS"), "{out}");
    assert!(out.contains(" t\n"), "{out}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn unit_u6_dashdash_collects_into_paths() {
    let opts = parse_args(&args(&["--engine", "cargo", "--", "--json", "p.php"])).unwrap();
    assert!(!opts.json);
    assert_eq!(opts.paths, args(&["--json", "p.php"]));
    assert!(opts.cargo_args.is_empty());
    // Cargo still forwards them as `cargo test` filters.
    assert_eq!(cargo_test_argv(&opts), args(&["test", "--json", "p.php"]));
}

#[test]
fn unit_u7_no_engine_is_usage_error() {
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("u7-kind");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let err = discover_selection_classified(&Options::default(), &dir).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert!(
        err.message.contains("No unit test engine is configured"),
        "{}",
        err.message
    );
    assert!(err.message.contains("'.arcunit'"), "{}", err.message);
    assert!(err.message.contains("'unit.engine'"), "{}", err.message);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unit_u8_bad_engine_subclass_message() {
    let dir = unique_dir("u8-msg");
    std::fs::create_dir_all(&dir).unwrap();
    let opts = parse_args(&args(&["--engine", "BogusEngine"])).unwrap();
    assert_eq!(
        discover_selection(&opts, &dir).unwrap_err(),
        "Configured unit test engine \"BogusEngine\" is not a subclass of \
         \"ArcanistUnitTestEngine\", but must be."
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unit_u9_everything_checked_before_discovery() {
    // `.hg` marks a working copy without needing a git binary.
    let dir = unique_dir("u9-order");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    // `--everything` plus paths wins even when discovery would fail.
    let opts = parse_args(&args(&["--everything", "p", "--engine", "Bogus"])).unwrap();
    let err = plan_run(&opts, &dir).unwrap_err();
    assert!(
        err.message
            .starts_with("You can not specify paths with --everything."),
        "{}",
        err.message
    );
    // Control: without paths, discovery runs and rejects the engine.
    let opts = parse_args(&args(&["--everything", "--engine", "Bogus"])).unwrap();
    let err = plan_run(&opts, &dir).unwrap_err();
    assert!(
        err.message.contains("is not a subclass of"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unit_u10_plain_keeps_star_and_padding() {
    let star = expected_star();
    let mut pass = engine_result("t", "pass");
    pass.set_duration(0.005);
    let plain = render_unit_result_with_ansi(&pass, false);
    assert!(plain.contains(star), "{plain}");
    assert!(plain.starts_with("   PASS "), "{plain}");
    assert!(plain.ends_with(&format!("{star}  t\n")), "{plain}");
}

#[test]
fn unit_u11_plain_detailed_has_no_mark_letters() {
    let out = render_detailed_coverage_report_with_ansi("a\nb", "CU", false);
    assert!(!out.contains('[') && !out.contains(']'), "{out}");
    assert_eq!(out.lines().count(), 2);
    // ...while TTY keeps painted backgrounds.
    let tty = render_detailed_coverage_report_with_ansi("a\nb", "CU", true);
    assert!(tty.contains("\x1b[42m"), "{tty}");
    assert!(tty.contains("\x1b[41m"), "{tty}");
}

#[test]
fn unit_u12_coverage_ties_keep_insertion_order() {
    use std::collections::HashMap;
    let mut first = engine_result("a", "pass");
    first.set_coverage(HashMap::from([("b.php".to_string(), "CU".to_string())]));
    let mut second = engine_result("b", "pass");
    second.set_coverage(HashMap::from([("a.php".to_string(), "CU".to_string())]));
    let collected = collect_coverage_ordered(&[first, second]);
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0].0, "b.php");
    assert_eq!(collected[1].0, "a.php");
    // Tied fractions keep insertion order (no filename tiebreak).
    let (sorted, _) = merge_file_coverage_ordered(&["z.php".to_string()], &collected);
    let names: Vec<&str> = sorted.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["z.php", "b.php", "a.php"]);
}

#[test]
fn unit_u20_collect_keeps_per_result_document_order() {
    // Files from one result keep document order (not sorted, not hashed).
    let mut one = engine_result("a", "pass");
    one.set_coverage_ordered(vec![
        ("z.php".to_string(), "CU".to_string()),
        ("a.php".to_string(), "CU".to_string()),
    ]);
    let collected = collect_coverage_ordered(&[one]);
    let names: Vec<&str> = collected.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["z.php", "a.php"]);
}

#[test]
fn unit_u20_json_keeps_nested_document_order() {
    let mut result = engine_result("a", "pass");
    result.set_extra_data_ordered(vec![
        ("zebra".to_string(), "1".to_string()),
        ("apple".to_string(), "2".to_string()),
    ]);
    result.set_coverage_ordered(vec![
        ("z.php".to_string(), "CU".to_string()),
        ("a.php".to_string(), "CU".to_string()),
    ]);
    let ugly = results_to_json_text(std::slice::from_ref(&result), false);
    assert!(
        ugly.contains("\"extra\":{\"zebra\":\"1\",\"apple\":\"2\"}"),
        "{ugly}"
    );
    assert!(
        ugly.contains("\"coverage\":{\"z.php\":\"CU\",\"a.php\":\"CU\"}"),
        "{ugly}"
    );
    let pretty = results_to_json_text(std::slice::from_ref(&result), true);
    let zebra = pretty.find("\"zebra\"").unwrap();
    let apple = pretty.find("\"apple\"").unwrap();
    assert!(zebra < apple, "{pretty}");
}

#[test]
fn unit_u13follow_engine_params_read_merged() {
    let dir = unique_dir("u13f");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let root_s = forward_slashes(&dir);
    let user = dir.join("arcrc.json");
    std::fs::write(
        &user,
        r#"{"config": {"unit.phpunit.binary": "user-phpunit",
            "unit.phutil.init-script": "user-init.php"}}"#,
    )
    .unwrap();
    let default = arc_rs::get_config::LoadOptions::default();
    let merged = arc_rs::get_config::load_merged(&dir, Some(&user), &default).unwrap();
    // User source wins over defaults.
    let engine = phpunit_engine_from_merged(&root_s, Some(&merged));
    assert!(engine.binary.ends_with("user-phpunit"), "{}", engine.binary);
    assert_eq!(
        engine_param_string_from(Some(&merged), &root_s, arc_rs::unit_phutil::INIT_SCRIPT_KEY)
            .as_deref(),
        Some("user-init.php")
    );
    // Project beats user.
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit.phpunit.binary": "proj-phpunit"}"#,
    )
    .unwrap();
    let merged = arc_rs::get_config::load_merged(&dir, Some(&user), &default).unwrap();
    let engine = phpunit_engine_from_merged(&root_s, Some(&merged));
    assert!(engine.binary.ends_with("proj-phpunit"), "{}", engine.binary);
    // Runtime `--config` beats project.
    let mut runtime = serde_json::Map::new();
    runtime.insert(
        "unit.phpunit.binary".to_string(),
        serde_json::json!("rt-phpunit"),
    );
    let opts = arc_rs::get_config::LoadOptions {
        runtime,
        ..Default::default()
    };
    let merged = arc_rs::get_config::load_merged(&dir, Some(&user), &opts).unwrap();
    let engine = phpunit_engine_from_merged(&root_s, Some(&merged));
    assert!(engine.binary.ends_with("rt-phpunit"), "{}", engine.binary);
    // `None` falls back to the project-root read.
    let engine = phpunit_engine_from_merged(&root_s, None);
    assert!(engine.binary.ends_with("proj-phpunit"), "{}", engine.binary);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unit_u14_empty_engine_falls_through_to_config() {
    let dir = unique_dir("u14-empty");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let empty = parse_args(&args(&["--engine="])).unwrap();
    // Bare: falls through to the NoEngine error.
    let err = discover_selection(&empty, &dir).unwrap_err();
    assert!(err.contains("No unit test engine is configured"), "{err}");
    // Config wins when set...
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit.engine": "PytestTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_selection(&empty, &dir).unwrap(),
        Selection::Single("PytestTestEngine".to_string())
    );
    // ...and `.arcunit` when config is unset.
    std::fs::write(dir.join(".arcconfig"), "{}").unwrap();
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"p": {"type": "phutil"}}}"#,
    )
    .unwrap();
    assert!(matches!(
        discover_selection(&empty, &dir).unwrap(),
        Selection::Config { .. }
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unit_u15_arcunit_types_are_class_map_only() {
    assert_eq!(CONFIG_AVAILABLE, &["phutil"]);
    let err = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"n": {"type": "NoseTestEngine"}}}"#,
        "/r/.arcunit",
        CONFIG_AVAILABLE,
    )
    .unwrap_err();
    let message = err.message();
    assert!(
        message.contains("Available test engines are: phutil."),
        "{message}"
    );
    // ...while single runs still take class names plus `phutil`.
    assert!(SINGLE_AVAILABLE.contains(&"NoseTestEngine"));
    assert!(SINGLE_AVAILABLE.contains(&"phutil"));
}

#[test]
fn unit_u18_int_durations_render_bare() {
    let mut r = engine_result("t", "pass");
    r.set_duration_int(3);
    assert!(r.duration_is_int());
    let ugly = results_to_json_text(std::slice::from_ref(&r), false);
    assert!(ugly.contains("\"duration\":3,"), "{ugly}");
    assert!(!ugly.contains("\"duration\":3.0"), "{ugly}");
    let pretty = results_to_json_text(std::slice::from_ref(&r), true);
    assert!(pretty.contains("\"duration\": 3,"), "{pretty}");
    let value = results_to_json(std::slice::from_ref(&r));
    assert_eq!(value[0]["duration"], serde_json::json!(3));
    assert!(value[0]["duration"].is_i64());
    let json = r.to_dictionary().to_json_string();
    assert!(json.contains("\"duration\":3,"), "{json}");
    // Floats keep the `.0` (PHP `json_encode(3.0)`).
    r.set_duration(3.0);
    assert!(!r.duration_is_int());
    let ugly = results_to_json_text(std::slice::from_ref(&r), false);
    assert!(ugly.contains("\"duration\":3.0,"), "{ugly}");
}

#[test]
fn unit_u19_unset_name_result_are_null() {
    let bare = EngineUnitResult::new();
    assert_eq!(bare.get_name(), "");
    assert_eq!(bare.get_result(), "");
    let ugly = results_to_json_text(std::slice::from_ref(&bare), false);
    assert!(ugly.contains("\"name\":null"), "{ugly}");
    assert!(ugly.contains("\"result\":null"), "{ugly}");
    let pretty = results_to_json_text(std::slice::from_ref(&bare), true);
    assert!(pretty.contains("\"name\": null"), "{pretty}");
    assert!(pretty.contains("\"result\": null"), "{pretty}");
    let value = results_to_json(std::slice::from_ref(&bare));
    assert!(value[0]["name"].is_null());
    assert!(value[0]["result"].is_null());
    let dict = bare.to_dictionary().to_json_string();
    assert!(dict.contains("\"name\":null"), "{dict}");
    assert!(dict.contains("\"result\":null"), "{dict}");
    // Explicitly set values (even empty) stay strings.
    let mut empty = EngineUnitResult::new();
    empty.set_name("").unwrap();
    empty.set_result("");
    let ugly = results_to_json_text(std::slice::from_ref(&empty), false);
    assert!(ugly.contains("\"name\":\"\""), "{ugly}");
    assert!(ugly.contains("\"result\":\"\""), "{ugly}");
}

#[test]
fn unit_u23_working_copy_required() {
    use arc_rs::conduit::EnvelopeKind;
    let bare = unique_dir("u23-bare");
    std::fs::create_dir_all(&bare).unwrap();
    let err = plan_run(&Options::default(), &bare).unwrap_err();
    assert_eq!(
        err.message,
        "This command must be run in a Git, Mercurial or Subversion working copy."
    );
    assert_eq!(err.kind, EnvelopeKind::Usage);
    // With a VCS marker the gate passes (discovery runs next).
    let repo = unique_dir("u23-repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(repo.join(".hg")).unwrap();
    let err = plan_run(&Options::default(), &repo).unwrap_err();
    assert!(
        err.message.contains("No unit test engine is configured"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&bare).ok();
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn unit_u24_detailed_pad_counts_bytes() {
    // `str_pad` pads to 80 BYTES: `é` (2 bytes) takes 78 spaces.
    let padded = pad_detailed_line("é");
    assert_eq!(padded.len(), 80);
    assert_eq!(padded.chars().count(), 79);
    assert_eq!(pad_detailed_line("a").len(), 80);
    // Never truncates.
    assert_eq!(pad_detailed_line(&"x".repeat(90)).len(), 90);
}

#[test]
fn unit_u26_empty_rev_takes_plain_status_path() {
    let dir = unique_dir("u26-rev");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    // Fake `hg`: serves `status`, fails `log` (range resolution).
    // `HgCommand` prepends global `--encoding` args, so the scripts scan
    // every argument for the subcommand.
    #[cfg(windows)]
    let script_name = "fakehg.bat";
    #[cfg(not(windows))]
    let script_name = "fakehg.sh";
    #[cfg(windows)]
    let script_text = "@echo off\r\nfor %%a in (%*) do if \"%%a\"==\"log\" exit /b 1\r\necho M mod.py\r\necho ? new.py\r\n";
    #[cfg(not(windows))]
    let script_text = "#!/bin/sh\nfor a in \"$@\"; do if [ \"$a\" = \"log\" ]; then exit 1; fi; done\nprintf 'M mod.py\\n? new.py\\n'\n";
    let script = dir.join(script_name);
    std::fs::write(&script, script_text).unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let script_s = forward_slashes(&script);
    #[cfg(windows)]
    let hg = arc_rs::hg::HgCommand::custom("cmd", &["/C", script_s.as_str()]);
    #[cfg(not(windows))]
    let hg = arc_rs::hg::HgCommand::custom(script_s.as_str(), &[]);
    let svn = arc_rs::svn::SvnCommand::system();
    // Empty `--rev` matches no rev (plain status, no `log` call).
    let via_none = select_paths_with(&Options::default(), &dir, &dir, &hg, &svn).unwrap();
    let empty = Options {
        rev: Some(String::new()),
        ..Options::default()
    };
    let via_empty = select_paths_with(&empty, &dir, &dir, &hg, &svn).unwrap();
    assert_eq!(via_empty, via_none);
    assert_eq!(via_empty, args(&["mod.py"]));
    // A real rev still attempts range resolution (and fails here).
    let bogus = Options {
        rev: Some("bogus".to_string()),
        ..Options::default()
    };
    assert!(select_paths_with(&bogus, &dir, &dir, &hg, &svn).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn d22_cargo_aggregate_results_convert_to_engine_dicts() {
    // T1153 DIFF-D22: the cargo aggregate becomes one pass/fail engine
    // result named after the engine, serializable via `results_to_json`.
    let report = EngineReport {
        engine: "cargo".to_string(),
        passed: 3,
        failed: 0,
        duration_ms: 1500,
        success: true,
        tail: String::new(),
        results: vec![UnitResult::new("cargo", Outcome::Pass, 1.5)],
    };
    let converted = cargo_aggregate_results(&report).unwrap();
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].get_name(), "cargo");
    assert_eq!(converted[0].get_result(), "pass");
    let json = results_to_json(&converted);
    assert_eq!(json[0]["name"], serde_json::json!("cargo"));
    assert_eq!(json[0]["result"], serde_json::json!("pass"));
}
#[test]
fn r10_unit_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "UNIT WORKFLOW\n\
         \n\
         unit\n\
         \x20     unit [options] [paths]\n\
         \x20     unit [options] --rev [rev]\n\
         \n\
         \x20         Supports: git, svn, hg\n\
         \x20         Run unit tests that cover specified paths. If no paths are\n\
         specified,\n\
         \x20         unit tests covering all modified files will be run.\n\
         \n\
         \x20     --coverage\n\
         \x20         Always enable coverage information.\n\
         \n\
         \x20     --detailed-coverage\n\
         \x20         Show a detailed coverage report on the CLI. Implies --coverage.\n\
         \n\
         \x20     --engine classname\n\
         \x20         Override configured unit engine for this project.\n\
         \n\
         \x20     --everything\n\
         \x20         Run every test associated with a tracked file in the working copy.\n\
         \n\
         \x20     --json\n\
         \x20         Report results in JSON format.\n\
         \n\
         \x20     --no-coverage\n\
         \x20         Always disable coverage information.\n\
         \n\
         \x20     --output format\n\
         \x20         With 'full', show full pretty report (Default). With 'json', report\n\
         \x20         results in JSON format. With 'ugly', use uglier (but more\n\
         \x20         efficient) JSON formatting. With 'none', don't print results.\n\
         \n\
         \x20     --rev revision\n\
         \x20         Run unit tests covering changes since a specific revision.\n\
         \n\
         \x20     --target phid\n\
         \x20         (PROTOTYPE) Record a copy of the test results on the specified\n\
         \x20         Harbormaster build target.\n\
         \n\
         \x20     --ugly\n\
         \x20         With --json, use uglier (but more efficient) formatting.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("UNIT WORKFLOW"), "{}", usage());
}

#[test]
fn b21_unit_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `unit`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_options(&args(&["--coverage"])).unwrap().is_some());
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    let text = usage_text(false);
    assert!(text.contains("UNIT WORKFLOW"), "{text}");
}

#[test]
fn b23_unit_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help unit`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("UNIT WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Run unit tests that cover specified paths."),
        "{text}"
    );
    assert!(text.contains("--detailed-coverage"), "{text}");
    assert!(!text.contains("--conduit-uri"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// --- T1170 coverage gap: `unit.rs` missed lines ---
//
// Each test below names the manifest lines it covers. Skipped clusters
// (unreachable arms, platform gates, toolchain/ambient requirements) are
// listed in the task report, not here.

#[test]
fn git_repo_probe_reports_false_for_missing_dir() {
    // unit.rs:800 — `git rev-parse` cannot spawn when the cwd is gone.
    let missing = unique_dir("git-gone").join("no-such-dir");
    assert!(!is_inside_git_repo(&missing));
}

#[cfg(not(windows))]
#[test]
fn uncommitted_paths_strips_legacy_arrow_and_quotes() {
    // unit.rs:853,857 — `git status -z` never emits `->` renames (order
    // is reversed, arrow omitted) or quoting, so these strips only fire
    // on literal `->`/`"` bytes inside file names. Both bytes are illegal
    // in Windows names, hence the gate.
    let repo = unique_dir("git-odd-names");
    std::fs::create_dir_all(&repo).unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{args:?}: {stderr}");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@t.test"]);
    run(&["config", "user.name", "t"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("a -> b.txt"), "one\n").unwrap();
    std::fs::write(repo.join("\"q\""), "one\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "one"]);
    std::fs::write(repo.join("a -> b.txt"), "two\n").unwrap();
    std::fs::write(repo.join("\"q\""), "two\n").unwrap();
    assert_eq!(uncommitted_paths(&repo).unwrap(), args(&["b.txt", "q"]));
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn commit_range_reports_diff_failures() {
    // unit.rs:886-889 — fresh repo, no commits: the empty-tree base
    // resolves but `git diff` against the unborn HEAD fails.
    let repo = unique_dir("git-nodiff");
    std::fs::create_dir_all(&repo).unwrap();
    let out = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let err = commit_range_paths(&repo, crate::git::GIT_MAGIC_ROOT_COMMIT).unwrap_err();
    assert!(err.starts_with("git diff failed: "), "{err}");
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn select_paths_everything_lists_git_tracked_files() {
    // unit.rs:1158 — `--everything` inside a git repo lists tracked files.
    let repo = unique_dir("sel-everything");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("sub").join("b.txt"), "b\n").unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@t.test"]);
    run(&["config", "user.name", "t"]);
    run(&["config", "commit.gpgsign", "false"]);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "one"]);
    let opts = Options {
        everything: true,
        ..Options::default()
    };
    assert_eq!(
        select_paths(&opts, &repo, &repo).unwrap(),
        args(&["a.txt", "sub/b.txt"])
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn select_paths_status_lists_git_uncommitted_files() {
    // unit.rs:1173 — default selection unions the range with uncommitted
    // paths; untracked files stay excluded.
    let repo = unique_dir("sel-status");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@t.test"]);
    run(&["config", "user.name", "t"]);
    run(&["config", "commit.gpgsign", "false"]);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "one"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    assert_eq!(
        select_paths(&Options::default(), &repo, &repo).unwrap(),
        args(&["a.txt"])
    );
    let ranged = Options {
        rev: Some("HEAD".to_string()),
        ..Options::default()
    };
    assert_eq!(
        select_paths(&ranged, &repo, &repo).unwrap(),
        args(&["a.txt"])
    );
    std::fs::remove_dir_all(&repo).ok();
}

fn stage_passing_crate(tag: &str) -> PathBuf {
    let dir = unique_dir(tag);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"t1170fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("lib.rs"),
        "#[test]\nfn passes() {\n    assert_eq!(1 + 1, 2);\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn cargo_selection_reports_passing_run() {
    // unit.rs:1427,3698-3702,3751-3753,3766,3774 — a real `cargo test`
    // success through the collecting selection: coverage warning,
    // full-format summary plus tail, exit 0.
    let dir = stage_passing_crate("cargo-pass");
    let opts = Options {
        coverage: true,
        ..Options::default()
    };
    assert_eq!(
        super::run_cargo_selection(&opts, &dir),
        std::process::ExitCode::SUCCESS
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cargo_collecting_renders_json_and_ugly() {
    // unit.rs:3690-3696 — the machine-format print arms plus the
    // converted aggregate results.
    let dir = stage_passing_crate("cargo-fmt");
    for output in ["json", "ugly"] {
        let opts = Options {
            output: Some(output.to_string()),
            ..Options::default()
        };
        let (code, results) = super::run_cargo_collecting(&opts, &dir).unwrap();
        assert_eq!(code, std::process::ExitCode::SUCCESS, "{output}");
        assert_eq!(results.len(), 1, "{output}");
        assert_eq!(results[0].get_name(), "cargo", "{output}");
        assert_eq!(results[0].get_result(), "pass", "{output}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_config_selection_shim_roundtrips() {
    // unit.rs:1498-1500 — the string-error shim over the classified load.
    let dir = unique_dir("cfg-shim");
    std::fs::create_dir_all(&dir).unwrap();
    let err = super::load_config_selection(&dir).unwrap_err();
    assert!(err.contains("Unable to find '.arcunit'"), "{err}");
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"p": {"type": "phutil"}}}"#,
    )
    .unwrap();
    assert_eq!(
        super::load_config_selection(&dir).unwrap(),
        Selection::Config {
            engines: vec![arc_rs::unit_config::PlannedEngine {
                name: "p".to_string(),
                engine_type: "phutil".to_string(),
                include: Vec::new(),
                exclude: Vec::new(),
            }],
            root: dir.clone(),
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_selection_reports_unreadable_arcunit() {
    // unit.rs:1516 — invalid UTF-8 fails `read_to_string` portably.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("cfg-unread");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".arcunit"), b"\xff\xfe{\xff").unwrap();
    let err = super::load_config_selection_classified(&dir).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Exception);
    assert!(err.message.contains("cannot read"), "{}", err.message);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn discover_selection_config_class_routes_to_arcunit() {
    // unit.rs:1564 — `unit.engine` naming the configuration-driven
    // class loads `.arcunit` like the explicit `--engine` path.
    let dir = unique_dir("cfg-class");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit.engine": "ArcanistConfigurationDrivenUnitTestEngine"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"p": {"type": "phutil"}}}"#,
    )
    .unwrap();
    assert_eq!(
        discover_selection_classified(&Options::default(), &dir).unwrap(),
        Selection::Config {
            engines: vec![arc_rs::unit_config::PlannedEngine {
                name: "p".to_string(),
                engine_type: "phutil".to_string(),
                include: Vec::new(),
                exclude: Vec::new(),
            }],
            root: dir.clone(),
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn real_runner_maps_empty_missing_and_ok_commands() {
    // unit.rs:1607-1627 — the production command runner: empty argv,
    // spawn failure, and a real exit-0 command.
    let dir = unique_dir("real-run");
    std::fs::create_dir_all(&dir).unwrap();
    let cwd = forward_slashes(&dir);

    let out = RealRunner.run(&[], &cwd);
    assert_eq!(out.exit_code, 1);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "empty command");

    let out = RealRunner.run(&args(&["definitely-not-a-binary-xyz-123"]), &cwd);
    assert_eq!(out.exit_code, 1);
    assert_eq!(out.stdout, "");
    assert!(
        out.stderr
            .starts_with("failed to run 'definitely-not-a-binary-xyz-123': "),
        "{}",
        out.stderr
    );

    let out = RealRunner.run(&args(&["git", "--version"]), &cwd);
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("git version"), "{}", out.stdout);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn real_pytest_fs_probes() {
    // unit.rs:1642-1653 — the pytest filesystem probes.
    use crate::unit_pytest::PytestFs;
    let dir = unique_dir("pytest-fs");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let file = dir.join("three.txt");
    std::fs::write(&file, "a\nb\nc").unwrap();
    let empty = dir.join("empty.txt");
    std::fs::write(&empty, "").unwrap();
    let missing = forward_slashes(&dir.join("missing"));
    let fs = super::RealPytestFs;
    let (d, f, e) = (
        forward_slashes(&dir),
        forward_slashes(&file),
        forward_slashes(&empty),
    );
    assert!(fs.path_exists(&f));
    assert!(!fs.path_exists(&missing));
    assert_eq!(fs.read_file(&f).as_deref(), Some("a\nb\nc"));
    assert!(fs.read_file(&missing).is_none());
    assert!(fs.is_dir(&d));
    assert!(!fs.is_dir(&f));
    assert!(fs.is_file(&f));
    assert!(!fs.is_file(&d));
    assert_eq!(fs.line_count(&f), Some(3));
    assert_eq!(fs.line_count(&e), Some(1));
    assert_eq!(fs.line_count(&missing), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn real_phpunit_fs_counts_lines() {
    // unit.rs:1673-1676 — the phpunit line-count probe.
    use crate::unit_phpunit::PhpunitFs;
    let dir = unique_dir("phpunit-fs");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("two.txt");
    std::fs::write(&file, "a\nb\n").unwrap();
    let fs = super::RealPhpunitFs;
    assert_eq!(fs.line_count(&forward_slashes(&file)), Some(2));
    assert_eq!(fs.line_count(&forward_slashes(&dir.join("missing"))), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn discovery_rules_reject_short_pairs() {
    // unit.rs:1778 — a discovery target with fewer than two entries.
    assert!(parse_discovery_rules(&serde_json::json!({"src/(.*)/": [["only-one"]]})).is_none());
    assert!(parse_discovery_rules(&serde_json::json!({"src/(.*)/": [[42, "b"]]})).is_none());
}

#[test]
fn all_files_under_missing_root_is_empty() {
    // unit.rs:1805 — an unreadable root yields no files.
    let missing = unique_dir("files-gone").join("no-such-dir");
    assert!(all_files_under(&forward_slashes(&missing)).is_empty());
}

#[test]
fn xunit_generate_maps_pass_fail_and_hard_errors() {
    // unit.rs:1932-1960 — the regenerate stage with `Build/` plus
    // `Protobuild.exe` present: pass, fail, and hard-error exits.
    let root = unique_dir("xunit-gen");
    std::fs::create_dir_all(root.join("Build")).unwrap();
    std::fs::write(root.join("Protobuild.exe"), "x").unwrap();
    let env = crate::unit_xunit::XunitEnvironment {
        build_engine: "fake-msbuild".to_string(),
        runtime_engine: String::new(),
        test_engine: "fake-xunit".to_string(),
        project_root: forward_slashes(&root),
        discovery_rules: Vec::new(),
    };
    let regen = |argv: &[String], _cwd: &str| {
        assert!(argv.contains(&"Protobuild.exe".to_string()), "{argv:?}");
        CapturedOutput::new(0, "regen ok", "")
    };
    let pass = super::run_xunit_generate(&env, &regen).unwrap();
    assert_eq!(pass.len(), 1);
    assert_eq!(pass[0].get_result(), "pass");
    let name = pass[0].get_name();
    assert!(name.starts_with("(regenerate projects for "), "{name}");

    let failing = |_: &[String], _: &str| CapturedOutput::new(1, "regen log", "");
    let fail = super::run_xunit_generate(&env, &failing).unwrap();
    assert_eq!(fail.len(), 1);
    assert_eq!(fail[0].get_result(), "fail");

    let hard = |_: &[String], _: &str| CapturedOutput::new(3, "", "boom");
    let err = super::run_xunit_generate(&env, &hard).unwrap_err();
    let message = err.message();
    assert!(message.contains("failed with exit code 3"), "{message}");
    assert!(err.message().contains("boom"), "{}", err.message());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn xunit_plan_stops_after_failing_generate() {
    // unit.rs:1898 — a failing regenerate stage ends the plan before
    // the build stage runs.
    let root = unique_dir("xunit-stopgen");
    std::fs::create_dir_all(root.join("Build")).unwrap();
    std::fs::write(root.join("Protobuild.exe"), "x").unwrap();
    let root_s = forward_slashes(&root);
    let plan = fake_xunit_plan(&root_s, &format!("{root_s}/A.Tests.dll"));
    let runner = |argv: &[String], _cwd: &str| {
        assert!(
            argv.contains(&"Protobuild.exe".to_string()),
            "must stop before build: {argv:?}"
        );
        CapturedOutput::new(1, "gen log", "")
    };
    let results = super::run_xunit_plan(&plan, &runner).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "fail");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn xunit_plan_stops_after_failing_build() {
    // unit.rs:1903 — a failing build stage ends the plan before the
    // test stage runs (no `/xml` invocation).
    let root = unique_dir("xunit-stopbuild");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let plan = fake_xunit_plan(&root_s, &format!("{root_s}/A.Tests.dll"));
    let runner = |argv: &[String], _cwd: &str| {
        assert_eq!(argv[0], "fake-msbuild", "must stop before test: {argv:?}");
        CapturedOutput::new(1, "build log", "")
    };
    let results = super::run_xunit_plan(&plan, &runner).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "fail");
    let name = results[0].get_name();
    assert!(name.starts_with("(build) "), "{name}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn csharp_test_propagates_cover_stage_failures() {
    // unit.rs:2270 — an unwritable cover stage errors the plan before
    // any command runs (the project root here is a file).
    use std::collections::HashMap;
    let root = unique_dir("csharp-nocover");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("App.Tests.dll"), "dll").unwrap();
    let not_a_dir = root.join("not-a-dir");
    std::fs::write(&not_a_dir, "x").unwrap();
    let root_s = forward_slashes(&not_a_dir);
    let assembly = format!("{}/App.Tests.dll", forward_slashes(&bin));
    let plan = fake_xunit_plan(&root_s, &assembly);
    let cover = super::CsharpCover {
        cover_engine: "fake-cscover".to_string(),
        match_regex: None,
        excluded: Some(HashMap::new()),
    };
    let runner = |_: &[String], _: &str| panic!("no command may run");
    let err = super::run_csharp_test(&plan, Some(&cover), None, &runner).unwrap_err();
    let message = err.message();
    assert!(message.contains("cannot stage cover file"), "{message}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_phutil_with_runner_reads_merged_params() {
    // unit.rs:2408-2416 — the merged-config entry point stages, runs,
    // parses, and cleans up like the explicit-merge twin.
    let root = unique_dir("phutil-merge");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = forward_slashes(&root);
    let ctx = EngineContext {
        project_root: root_s.clone(),
        paths: vec!["a.php".to_string()],
        enable_coverage: None,
        run_all_tests: false,
    };
    let runner = |argv: &[String], cwd: &str| {
        assert_eq!(argv[0], "fake-php");
        assert_eq!(cwd, root_s);
        std::fs::write(
            &argv[3],
            r#"{"results": [{"name": "t", "result": "pass"}]}"#,
        )
        .unwrap();
        CapturedOutput::new(0, "", "")
    };
    let results = run_phutil_with_runner(&ctx, &runner, "phutil", &|| {
        Some(vec!["fake-php".to_string()])
    })
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(staged_leftovers(&root).len(), 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn phpunit_run_with_existing_config_file_proceeds() {
    // unit.rs:2510 — a configured `phpunit_config` that EXISTS skips
    // the missing-file error and runs the engine.
    let root = unique_dir("phpunit-cfg");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("src").join("Calc.php"), "<?php\n").unwrap();
    std::fs::write(root.join("src").join("CalcTest.php"), "<?php\n").unwrap();
    std::fs::write(root.join("phpunit.xml"), "<x/>").unwrap();
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"phpunit_config": "phpunit.xml"}"#,
    )
    .unwrap();
    let ctx = EngineContext {
        project_root: forward_slashes(&root),
        paths: vec!["src/Calc.php".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let events = "{\"event\":\"testStart\",\"test\":\"CalcTest::testAdd\"}\n\
        {\"event\":\"test\",\"test\":\"CalcTest::testAdd\",\"status\":\"pass\",\"time\":0.02}\n";
    let runner = |argv: &[String], _cwd: &str| {
        let at = argv.iter().position(|a| a == "--log-json").unwrap();
        std::fs::write(&argv[at + 1], events).unwrap();
        CapturedOutput::new(0, "", "")
    };
    let results = run_single_with_runner("PhpunitTestEngine", &ctx, &runner).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), "pass");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn single_report_aggregates_counts_and_success() {
    // unit.rs:2539-2589 — the single-engine report aggregates counts,
    // success, and per-test results; unknown engines stay usage errors.
    let root = unique_dir("single-report");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::create_dir_all(root.join("tests").join("pkg")).unwrap();
    std::fs::write(root.join("pkg").join("mod.py"), "x = 1\n").unwrap();
    std::fs::write(
        root.join("tests").join("pkg").join("test_mod.py"),
        "def test_ok(): pass\n",
    )
    .unwrap();
    let root_s = forward_slashes(&root);
    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), CANNED_XUNIT).unwrap();
        CapturedOutput::new(1, "", "")
    };
    let report = run_single_report(
        "NoseTestEngine",
        &root_s,
        &["pkg/mod.py".to_string()],
        false,
        Some(false),
        &runner,
    )
    .unwrap();
    assert_eq!(report.engine, "NoseTestEngine");
    assert_eq!((report.passed, report.failed), (1, 1));
    assert!(!report.success);
    assert_eq!(report.results.len(), 2);

    let err = run_single_report("Bogus", &root_s, &[], false, Some(false), &runner).unwrap_err();
    assert!(err.contains("is not a subclass of"), "{err}");
    let err = run_single_report_classified("Bogus", &root_s, &[], false, Some(false), &runner)
        .unwrap_err();
    assert_eq!(err.kind, arc_rs::conduit::EnvelopeKind::Usage);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn merge_ordered_updates_seeded_paths_in_place() {
    // unit.rs:2976 — a requested path that also has coverage keeps its
    // seed slot and takes the merged fraction.
    let (sorted, reports) = merge_file_coverage_ordered(
        &["f.php".to_string()],
        &[("f.php".to_string(), vec!["CC".to_string()])],
    );
    assert_eq!(sorted, vec![("f.php".to_string(), 1.0)]);
    assert_eq!(reports["f.php"], "CC");
}

#[test]
fn cargo_outcome_codes_convert_all_outcomes() {
    // unit.rs:3707-3727 — every cargo outcome converts to its engine
    // result code with name and duration intact.
    assert_eq!(super::cargo_outcome_code(Outcome::Pass), "pass");
    assert_eq!(super::cargo_outcome_code(Outcome::Fail), "fail");
    assert_eq!(super::cargo_outcome_code(Outcome::Skip), "skip");
    assert_eq!(super::cargo_outcome_code(Outcome::Broken), "broken");
    assert_eq!(super::cargo_outcome_code(Outcome::Unsound), "unsound");

    let converted = super::convert_cargo_result(&UnitResult::new("n", Outcome::Skip, 0.5));
    assert_eq!(converted.get_name(), "n");
    assert_eq!(converted.get_result(), "skip");
    assert_eq!(converted.get_duration(), Some(0.5));

    let report = EngineReport {
        engine: "cargo".to_string(),
        passed: 1,
        failed: 1,
        duration_ms: 9,
        success: false,
        tail: String::new(),
        results: vec![
            UnitResult::new("a", Outcome::Pass, 0.1),
            UnitResult::new("b", Outcome::Broken, 0.2),
        ],
    };
    let converted = super::converted_cargo_results(&report);
    assert_eq!(converted.len(), 2);
    assert_eq!(converted[0].get_result(), "pass");
    assert_eq!(converted[1].get_result(), "broken");
}

#[test]
fn upload_cargo_target_maps_success_to_overall() {
    // unit.rs:3733-3739 — both overall arms reach the conduit upload,
    // which fails deterministically against an invalid URI with no token.
    let opts = Options {
        target: Some("PHID-1".to_string()),
        conduit_uri: Some("https://invalid.invalid/".to_string()),
        ..Options::default()
    };
    for success in [true, false] {
        let report = EngineReport {
            engine: "cargo".to_string(),
            passed: u32::from(success),
            failed: u32::from(!success),
            duration_ms: 1,
            success,
            tail: String::new(),
            results: Vec::new(),
        };
        let err = super::upload_cargo_target(&opts, &report).unwrap_err();
        assert!(
            err.contains("no token") || err.contains("transport") || err.contains("conduit"),
            "{success}: {err}"
        );
    }
}

#[test]
fn empty_json_object_renders_braces() {
    // unit.rs:3742-3744 — the pretty-print fallback.
    let err = serde_json::from_str::<serde_json::Value>("{bad").unwrap_err();
    assert_eq!(super::empty_json_object(err), "{}");
}

#[test]
fn cargo_selection_reports_missing_crate() {
    // unit.rs:3754 — a cargo run outside any crate reports a generic
    // failure.
    let dir = unique_dir("cargo-nocrate");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        super::run_cargo_selection(&Options::default(), &dir),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn single_collecting_rejects_cargo_args() {
    // unit.rs:3820-3826 — `cargo test` args need the cargo engine.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("single-cargoargs");
    std::fs::create_dir_all(&dir).unwrap();
    let opts = Options {
        cargo_args: args(&["--nocapture"]),
        ..Options::default()
    };
    let err = super::run_single_collecting(&opts, &dir, "NoseTestEngine", &[], false).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert_eq!(
        err.message,
        "cargo test args after '--' require the cargo engine"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn single_collecting_reports_unknown_engine() {
    // unit.rs:3827-3840,3842 — an unknown engine class fails before any
    // command runs.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("single-bogus");
    std::fs::create_dir_all(&dir).unwrap();
    let err =
        super::run_single_collecting(&Options::default(), &dir, "Bogus", &[], false).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert!(
        err.message.contains("is not a subclass of"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_collecting_rejects_cargo_args() {
    // unit.rs:3869-3874 — `cargo test` args need the cargo engine.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("config-cargoargs");
    std::fs::create_dir_all(&dir).unwrap();
    let opts = Options {
        cargo_args: args(&["--nocapture"]),
        ..Options::default()
    };
    let err = super::run_config_collecting(&opts, &[], &dir, &[], false).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert_eq!(
        err.message,
        "cargo test args after '--' require the cargo engine"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_collecting_reports_bad_child() {
    // unit.rs:3875-3888,3892 — an unknown child engine class fails the
    // config run as a usage error.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("config-badchild");
    std::fs::create_dir_all(&dir).unwrap();
    let planned = vec![crate::unit_config::PlannedEngine {
        name: "bad".to_string(),
        engine_type: "Bogus".to_string(),
        include: Vec::new(),
        exclude: Vec::new(),
    }];
    let err =
        super::run_config_collecting(&Options::default(), &planned, &dir, &[], false).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert!(
        err.message.contains("is not a subclass of"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finish_php_report_collecting_maps_exit_codes() {
    // unit.rs:3902-3921,3929 — print per the output format, skip the
    // upload without a target, map the overall result to the exit code,
    // and pass the results through.
    let dir = unique_dir("finish-map");
    std::fs::create_dir_all(&dir).unwrap();
    let report = ConfigReport {
        engine: "NoseTestEngine".to_string(),
        passed: 1,
        failed: 1,
        duration_ms: 3,
        success: false,
        results: vec![engine_result("a", "pass"), engine_result("b", "fail")],
    };
    let opts = Options {
        output: Some("none".to_string()),
        ..Options::default()
    };
    let (code, results) =
        super::finish_php_report_collecting(&opts, &dir, &[], &report, false, None).unwrap();
    assert_eq!(code, std::process::ExitCode::from(2));
    assert_eq!(results.len(), 2);

    let passing = ConfigReport {
        passed: 1,
        failed: 0,
        success: true,
        results: vec![engine_result("a", "pass")],
        ..report
    };
    let (code, results) =
        super::finish_php_report_collecting(&Options::default(), &dir, &[], &passing, false, None)
            .unwrap();
    assert_eq!(code, std::process::ExitCode::SUCCESS);
    assert_eq!(results.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finish_php_report_collecting_upload_failure_is_generic() {
    // unit.rs:3922-3928,3933-3955 — an upload failure becomes a generic
    // error through the options-threaded upload.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("finish-upload");
    std::fs::create_dir_all(&dir).unwrap();
    let report = ConfigReport {
        engine: "NoseTestEngine".to_string(),
        passed: 1,
        failed: 0,
        duration_ms: 1,
        success: true,
        results: vec![engine_result("a", "pass")],
    };
    let opts = Options {
        output: Some("none".to_string()),
        target: Some("PHID-1".to_string()),
        conduit_uri: Some("https://invalid.invalid/".to_string()),
        ..Options::default()
    };
    let err =
        super::finish_php_report_collecting(&opts, &dir, &[], &report, false, None).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Exception);
    let message = &err.message;
    let markers = ["no token", "transport", "conduit"];
    assert!(
        markers.iter().any(|marker| message.contains(marker)),
        "{message}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_unit_options_ok_path() {
    // unit.rs:3959 — valid argv parses through.
    let opts = super::parse_unit_options(&args(&[])).unwrap();
    assert_eq!(opts.engine, "cargo");
    assert!(!opts.help);
}

#[test]
fn unit_working_dir_returns_cwd() {
    // unit.rs:3971-3973 — the happy path returns the process cwd.
    assert_eq!(
        super::unit_working_dir().unwrap(),
        std::env::current_dir().unwrap()
    );
}

#[test]
fn plan_run_single_selects_paths() {
    // unit.rs:4040-4043 — an explicit engine plus explicit paths plans a
    // single run (`.hg` marks the working copy without needing git).
    let dir = unique_dir("plan-single");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    std::fs::write(dir.join("f.php"), "<?php\n").unwrap();
    let opts = parse_args(&args(&["--engine", "NoseTestEngine", "f.php"])).unwrap();
    let run = plan_run(&opts, &dir).unwrap();
    assert_eq!(
        format!("{run:?}"),
        "Single { class: \"NoseTestEngine\", paths: [\"f.php\"] }"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn plan_run_config_selects_paths() {
    // unit.rs:4045-4051 — the configuration-driven class plus explicit
    // paths plans a config run over `.arcunit`.
    let dir = unique_dir("plan-config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    std::fs::write(dir.join("f.php"), "<?php\n").unwrap();
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"p": {"type": "phutil"}}}"#,
    )
    .unwrap();
    let opts = parse_args(&args(&[
        "--engine",
        "ArcanistConfigurationDrivenUnitTestEngine",
        "f.php",
    ]))
    .unwrap();
    let run = plan_run(&opts, &dir).unwrap();
    assert_eq!(
        format!("{run:?}"),
        format!(
            "Config {{ engines: [PlannedEngine {{ name: \"p\", engine_type: \"phutil\", \
             include: [], exclude: [] }}], root: {dir:?}, paths: [\"f.php\"] }}"
        )
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn single_selection_reports_errors() {
    // unit.rs:3798-3805,3807-3809 — the single-run wrapper maps a
    // collecting failure to its reported exit code.
    let dir = unique_dir("single-sel");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        super::run_single_selection(&Options::default(), &dir, "Bogus", &[], false),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_selection_reports_errors() {
    // unit.rs:3847-3854,3856-3858 — the config-run wrapper maps a
    // collecting failure to its reported exit code.
    let dir = unique_dir("config-sel");
    std::fs::create_dir_all(&dir).unwrap();
    let planned = vec![crate::unit_config::PlannedEngine {
        name: "bad".to_string(),
        engine_type: "Bogus".to_string(),
        include: Vec::new(),
        exclude: Vec::new(),
    }];
    assert_eq!(
        super::run_config_selection(&Options::default(), &planned, &dir, &[], false),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn child_collecting_help_and_usage_errors() {
    // unit.rs:4075-4082 — `diff`'s child entry rejects `--help` and bad
    // flags as unreported usage errors.
    use arc_rs::conduit::EnvelopeKind;
    let dir = unique_dir("child-parse");
    std::fs::create_dir_all(&dir).unwrap();
    let err = run_in_collecting(&dir, &args(&["--help"])).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert_eq!(err.message, "unexpected --help in child unit argv");
    let err = run_in_collecting(&dir, &args(&["--bogus"])).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert_eq!(err.message, "Unknown argument 'bogus'. Try 'arc help'.");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn child_collecting_runs_each_selection() {
    // unit.rs:4086-4094 — the child entry dispatches cargo, single, and
    // config selections; each sub-case fails deterministically without
    // external tools (missing crate, missing phpunit config, empty
    // engine map).
    let dir = unique_dir("child-cargo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    let err = run_in_collecting(&dir, &args(&["--engine", "cargo"])).unwrap_err();
    assert!(err.message.contains("no Cargo.toml"), "{}", err.message);
    std::fs::remove_dir_all(&dir).ok();

    let dir = unique_dir("child-single");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    std::fs::write(dir.join("f.php"), "<?php\n").unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phpunit_config": "missing.xml"}"#,
    )
    .unwrap();
    let err =
        run_in_collecting(&dir, &args(&["--engine", "PhpunitTestEngine", "f.php"])).unwrap_err();
    assert!(
        err.message
            .contains("PHPUnit configuration file was not found"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&dir).ok();

    let dir = unique_dir("child-config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    std::fs::write(dir.join("f.php"), "<?php\n").unwrap();
    std::fs::write(dir.join(".arcunit"), r#"{"engines": {}}"#).unwrap();
    let err = run_in_collecting(
        &dir,
        &args(&[
            "--engine",
            "ArcanistConfigurationDrivenUnitTestEngine",
            "f.php",
        ]),
    )
    .unwrap_err();
    assert_eq!(err.message, "No tests to run.");
    std::fs::remove_dir_all(&dir).ok();
}

// -( T1170 COV: close unit.rs )--------------------------------------------------

#[test]
fn phpunit_engine_from_config_reads_project_files() {
    let root = unique_dir("phpunit-cfg");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let root_s = forward_slashes(&root);
    std::fs::write(root.join("phpunit.xml"), "<x/>").unwrap();
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"phpunit_config": "phpunit.xml", "unit.phpunit.binary": "my-phpunit"}"#,
    )
    .unwrap();
    let engine = phpunit_engine_from_config(&root_s);
    assert!(engine.config_file.is_some());
    assert!(engine.binary.ends_with("my-phpunit"), "{}", engine.binary);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn single_nose_selection_with_no_paths_succeeds_empty() {
    let root = unique_dir("single-empty");
    std::fs::create_dir_all(&root).unwrap();
    let opts = Options::default();
    // Nose with no paths runs no commands, so the real runner is safe.
    let (code, results) = run_single_collecting(&opts, &root, "NoseTestEngine", &[], false)
        .expect("empty nose selection succeeds");
    assert!(results.is_empty());
    assert_eq!(
        run_single_selection(&opts, &root, "NoseTestEngine", &[], false),
        code
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn config_selection_with_idle_nose_succeeds_empty() {
    let root = unique_dir("config-empty");
    std::fs::create_dir_all(&root).unwrap();
    let opts = Options::default();
    let engines = vec![PlannedEngine {
        name: "nose".to_string(),
        engine_type: "NoseTestEngine".to_string(),
        include: Vec::new(),
        exclude: Vec::new(),
    }];
    let (code, results) = run_config_collecting(&opts, &engines, &root, &[], false)
        .expect("idle config selection succeeds");
    assert!(results.is_empty());
    assert_eq!(
        run_config_selection(&opts, &engines, &root, &[], false),
        code
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn upload_results_for_options_rejects_bad_cli_config() {
    let root = unique_dir("upload-badcfg");
    std::fs::create_dir_all(&root).unwrap();
    let opts = Options {
        config: vec!["bogus".to_string()],
        ..Options::default()
    };
    let err = upload_results_for_options(&opts, &root, "PHID-X", 0, &[]).unwrap_err();
    assert!(err.contains("name=value"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn run_rejects_everything_with_paths() {
    assert_eq!(
        run(&args(&["--everything", "x"])),
        std::process::ExitCode::FAILURE
    );
}

// -( T1170 endgame band I )----------------------------------------------------

fn stage_custom_harness_crate(tag: &str) -> PathBuf {
    // A fixture crate whose test binary is a custom harness
    // (`harness = false`) printing one byte with no trailing newline.
    let dir = unique_dir(tag);
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"t1170fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [[test]]\nname = \"t\"\npath = \"tests/t.rs\"\nharness = false\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("tests").join("t.rs"),
        "fn main() {\n    print!(\"x\");\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn cargo_run_merges_unterminated_stdout_with_stderr() {
    // `cargo test` stdout without a trailing newline (custom harness)
    // gains the separator before stderr is appended.
    let dir = stage_custom_harness_crate("cargo-nonl");
    let report = super::execute(&Options::default(), &dir, None).unwrap();
    assert!(report.success);
    assert!(
        report.tail.starts_with("x\n"),
        "unexpected tail: {:?}",
        report.tail
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cargo_quiet_run_reports_without_stderr() {
    // `cargo test --quiet` writes no status to stderr; the run still
    // reports its pass with the harness tail.
    let dir = stage_passing_crate("cargo-quiet");
    let opts = Options {
        cargo_args: vec!["--quiet".to_string()],
        ..Options::default()
    };
    let report = super::execute(&opts, &dir, None).unwrap();
    assert!(report.success);
    assert!(report.tail.contains("test result"), "{}", report.tail);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_single_selection_rejects_everything_for_pytest() {
    // `--engine PytestTestEngine --everything` plans a single-engine
    // run, then fails the run-all gate before any binary is spawned
    // (no ambient-tool dependence, deterministic everywhere).
    assert_eq!(
        run(&args(&["--engine", "PytestTestEngine", "--everything"])),
        std::process::ExitCode::FAILURE
    );
    // The plan really is single-engine (not the usage-error arm):
    // `run` uses the process checkout, which is always a git tree.
    let cwd = std::env::current_dir().unwrap();
    let opts = parse_options(&args(&["--engine", "PytestTestEngine", "--everything"]))
        .unwrap()
        .unwrap();
    assert!(matches!(
        plan_run(&opts, &cwd),
        Ok(PlannedRun::Single { .. })
    ));
}
