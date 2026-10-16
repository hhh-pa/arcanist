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
    // Empty `--engine=` parses (D1 stores empty); discovery rejects it.
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
    // CORE-33/UNIT-U13: `unit.engine` reads through the source merge, so
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
    // CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
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
    // CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();

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
        assert!(
            msg.contains("Unable to find")
                || msg.contains("discovery rules")
                || msg.contains("Unable to locate")
                || msg.contains("cscover"),
            "{engine}: {msg}"
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
    // Past `--`, help forwards to cargo.
    let opts = parse_options(&args(&["--", "--help"])).unwrap().unwrap();
    assert_eq!(opts.cargo_args, args(&["--help"]));
    assert!(opts.paths.is_empty());
    // Unknown `--help=x` errors like D1.
    let err = parse_options(&args(&["--help=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'help'. Try 'arc help'.");
    // `--bool=value` splices the value as the next token (a path here).
    let opts = parse_options(&args(&["--json=x"])).unwrap().unwrap();
    assert!(opts.json);
    assert_eq!(opts.paths, vec!["x".to_string()]);
    // Second `--` forwards verbatim to cargo.
    let opts = parse_options(&args(&["--", "a", "--", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(opts.cargo_args, args(&["a", "--", "b"]));
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
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
    if super::detect_vcs(&root).is_none() {
        assert!(select_paths(&opts, &root, &root).unwrap().is_empty());
    }
    opts.everything = false;
    opts.paths = vec!["sub/f.php".to_string()];
    assert_eq!(
        select_paths(&opts, &root, &root).unwrap(),
        vec!["sub/f.php".to_string()]
    );
    opts.paths.clear();
    if super::detect_vcs(&root).is_none() {
        assert!(select_paths(&opts, &root, &root).unwrap().is_empty());
    }
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
    assert!(detailed.contains("[C]"), "{detailed}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detailed_coverage_marks_lines() {
    let out = render_detailed_coverage_report("a\nbb\nccc", "CUXN-extra");
    assert!(out.contains("    1  [C] a"), "{out}");
    assert!(out.contains("    2  [U] bb"), "{out}");
    assert!(out.contains("    3  [X] ccc"), "{out}");
    let out = render_detailed_coverage_report("only", "");
    assert!(out.contains("[N]"), "{out}");
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

    // Phpunit engine honors `.arcconfig` binary/config keys.
    let engine = phpunit_engine_from_config(&root_s);
    assert_eq!(engine.binary, "phpunit");
    assert!(engine.config_file.is_none());
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

    // Configured-but-missing phpunit config surfaces verbatim.
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
    assert!(
        err.message()
            .contains("PHPUnit configuration file was not found"),
        "{}",
        err.message()
    );
    std::fs::remove_dir_all(&root).ok();
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
    let opts = parse_args(&args(&["flt", "--", "--", "--nocapture"])).unwrap();
    assert_eq!(
        cargo_test_argv(&opts),
        args(&["test", "--", "--nocapture", "flt"])
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
    assert_eq!(
        execute(&opts, Path::new("."), None).unwrap_err(),
        "unknown engine 'Nope' (available engines: cargo, \
         ArcanistConfigurationDrivenUnitTestEngine, CSharpToolsTestEngine, NoseTestEngine, \
         PhpunitTestEngine, PhutilUnitTestEngine, PytestTestEngine, XUnitTestEngine)"
    );
    // Cargo stays the default selection (documented extension, not PHP's
    // ArcanistNoEngineException).
    let dir = unique_dir("cargo-default");
    std::fs::create_dir_all(&dir).unwrap();
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
    assert!(
        argv[2].starts_with(&root_s) && argv[2].ends_with(".xml"),
        "{}",
        argv[2]
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
    let cover = super::load_csharp_cover(&ctx).unwrap().unwrap();
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
    assert!(super::load_csharp_cover(&off).unwrap().is_none());

    std::fs::write(root.join(".arcconfig"), "{}").unwrap();
    let err = super::load_csharp_cover(&ctx).unwrap_err();
    assert!(
        err.message().contains("unit.csharp.coverage.binary"),
        "{}",
        err.message()
    );

    std::fs::write(
        root.join(".arcconfig"),
        r#"{"unit.csharp.cscover.binary": "nope.exe"}"#,
    )
    .unwrap();
    let err = super::load_csharp_cover(&ctx).unwrap_err();
    assert!(
        err.message().contains("have you built yet"),
        "{}",
        err.message()
    );

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
    assert!(
        calls[0].0[2].contains(".arc-unit-phutil-input-"),
        "{:?}",
        calls[0].0
    );
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
    assert!(
        err.message().contains("unknown engine 'Bogus'"),
        "{}",
        err.message()
    );
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
    // Like PHP, the stripped badge keeps its padding; the plain
    // workflow rendering uses bare labels instead.
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
    assert_eq!(
        render_unit_result_with_ansi(&pass, false),
        "  PASS 125ms ns::t\n"
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
    assert_eq!(
        render_unit_result_with_ansi(&pass, false),
        "  PASS   5ms t\n"
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
    assert_eq!(
        render_unit_result_with_ansi(&fail, false),
        "  FAIL t\noops\n"
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
    // Plain keeps the bracketed marks.
    let plain = render_detailed_coverage_report_with_ansi("a\nbb", "CU", false);
    assert!(plain.contains("[C]"), "{plain}");
    assert!(plain.contains("[U]"), "{plain}");
    assert!(!plain.contains('\x1b'), "{plain}");
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
    assert_eq!(plain, "  PASS 125ms t\n");
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
    assert_eq!(
        discover_selection(&shadowed, &dir).unwrap(),
        Selection::Cargo
    );
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
