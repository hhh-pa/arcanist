use super::*;
use crate::unit_engine::UnitResult as EngineUnitResult;
use crate::unit_engine::{CapturedOutput, EngineContext};

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
    assert_eq!(opts.cargo_args, args(&["--nocapture"]));
}

#[test]
fn forwards_everything_after_dashdash() {
    let opts = parse_args(&args(&["--", "--json", "--engine", "bogus"])).unwrap();
    assert!(!opts.json);
    assert_eq!(opts.engine, "cargo");
    assert_eq!(opts.cargo_args, args(&["--json", "--engine", "bogus"]));
}

#[test]
fn parses_help_flags() {
    assert!(parse_args(&args(&["--help"])).unwrap().help);
    assert!(parse_args(&args(&["-h"])).unwrap().help);
}

#[test]
fn rejects_bad_invocations() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["-x"])).is_err());
    assert!(parse_args(&args(&["--engine"])).is_err());
    assert!(parse_args(&args(&["--engine="])).is_err());
    let err = parse_args(&args(&["some-path"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "{err}");
    assert!(err.contains("'--'"), "{err}");
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
    assert!(err.contains("unknown engine 'phpunit'"), "{err}");
    assert!(err.contains("available engines: cargo"), "{err}");
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
    assert_eq!(opts.cargo_args, args(&["--everything"]));
}

#[test]
fn finds_arcunit_and_unit_engine_keys() {
    let root = unique_dir("arcunit-find");
    let child = root.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(root.join(".arcunit"), "{}").unwrap();
    assert_eq!(find_arcunit_from(&child), Some(root.clone()));
    assert_eq!(find_arcunit_from(&root), Some(root.clone()));

    std::fs::write(
        root.join(".arcconfig"),
        r#"{"unit.engine": "NoseTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        find_unit_engine_from(&child).as_deref(),
        Some("NoseTestEngine")
    );
    assert_eq!(
        find_unit_engine_from(&root).as_deref(),
        Some("NoseTestEngine")
    );
    std::fs::remove_dir_all(&root).ok();

    let bare = unique_dir("arcunit-bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(find_arcunit_from(&bare), None);
    assert_eq!(find_unit_engine_from(&bare), None);
    std::fs::write(bare.join(".arcconfig"), "not json").unwrap();
    assert_eq!(find_unit_engine_from(&bare), None);
    std::fs::write(bare.join(".arcconfig"), r#"{"unit.engine": ""}"#).unwrap();
    assert_eq!(find_unit_engine_from(&bare), None);
    std::fs::write(bare.join(".arcconfig"), "{}").unwrap();
    assert_eq!(find_unit_engine_from(&bare), None);
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn discover_selection_prefers_explicit_then_config_then_cargo() {
    let dir = unique_dir("sel-order");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        discover_selection(&Options::default(), &dir).unwrap(),
        Selection::Cargo
    );

    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"n": {"type": "NoseTestEngine"}}}"#,
    )
    .unwrap();
    match discover_selection(&Options::default(), &dir).unwrap() {
        Selection::Config { engines, root } => {
            assert_eq!(engines.len(), 1);
            assert_eq!(engines[0].engine_type, "NoseTestEngine");
            assert_eq!(root, dir);
        }
        other => panic!("expected Config, got {other:?}"),
    }

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

    let bogus = parse_args(&args(&["--engine", "BogusEngine"])).unwrap();
    let err = discover_selection(&bogus, &dir).unwrap_err();
    assert!(err.contains("unknown engine 'BogusEngine'"), "{err}");

    let config_class = parse_args(&args(&[
        "--engine",
        "ArcanistConfigurationDrivenUnitTestEngine",
    ]))
    .unwrap();
    let err = discover_selection(&config_class, &dir).unwrap_err();
    assert!(err.contains("Unable to find '.arcunit'"), "{err}");

    std::fs::write(dir.join(".arcconfig"), r#"{"unit.engine": "BogusEngine"}"#).unwrap();
    let err = discover_selection(&Options::default(), &dir).unwrap_err();
    assert!(err.contains("unknown engine 'BogusEngine'"), "{err}");

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
    assert!(
        err.message().contains("does not support --everything"),
        "{}",
        err.message()
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
    for (engine, needle) in [
        ("PhutilUnitTestEngine", "cannot execute phutil"),
        ("phutil", "cannot execute phutil"),
        ("XUnitTestEngine", "not yet ported"),
        ("CSharpToolsTestEngine", "not yet ported"),
    ] {
        let err = run_single_with_runner(engine, &ctx, &runner).unwrap_err();
        assert!(
            err.message().contains(needle),
            "{engine}: {}",
            err.message()
        );
    }
    let err = run_single_with_runner("Bogus", &ctx, &runner).unwrap_err();
    assert!(
        err.message().contains("unknown engine"),
        "{}",
        err.message()
    );
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
    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"n": {"type": "NoseTestEngine"}}}"#,
        "/r/.arcunit",
        CONFIG_AVAILABLE,
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
    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"p": {"type": "PytestTestEngine"}}}"#,
        "/r/.arcunit",
        CONFIG_AVAILABLE,
    )
    .unwrap();
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let err =
        run_config_with_runner(&planned, "/tmp", &[], true, Some(false), &runner).unwrap_err();
    assert!(err.contains("does not support --everything"), "{err}");

    let planned = crate::unit_config::parse_engine_specs(
        r#"{"engines": {"p": {"type": "PhpunitTestEngine"}}}"#,
        "/r/.arcunit",
        CONFIG_AVAILABLE,
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
    let mut pass = engine_result("t", "pass");
    pass.set_namespace("ns");
    pass.set_duration(0.125);
    assert_eq!(render_unit_result(&pass), "  PASS 125ms ns::t\n");

    let bare_pass = engine_result("t", "pass");
    assert_eq!(render_unit_result(&bare_pass), "  PASS  <1ms t\n");

    let mut fail = engine_result("t", "fail");
    fail.set_user_data("oops");
    assert_eq!(render_unit_result(&fail), "  FAIL t\noops\n");

    let fail_bare = engine_result("t", "fail");
    assert_eq!(render_unit_result(&fail_bare), "  FAIL t\n");

    assert_eq!(
        render_unit_result(&engine_result("t", "skip")),
        "  SKIP t\n"
    );
    assert_eq!(
        render_unit_result(&engine_result("t", "broken")),
        "  BROKEN t\n"
    );
    assert_eq!(
        render_unit_result(&engine_result("t", "unsound")),
        "  UNSOUND t\n"
    );
    assert_eq!(render_unit_result(&engine_result("t", "bogus")), "   t\n");

    let mut trailing = engine_result("t", "fail");
    trailing.set_user_data("x\n");
    assert_eq!(render_unit_result(&trailing), "  FAIL t\nx\n\n");
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
