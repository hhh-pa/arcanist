//! Cargo engine behavior plus W1 selection/config/render through the wired library.

use std::path::{Path, PathBuf};

use arc_rs::unit::{self, Options, find_crate_root_from};
use arc_rs::unit_engine::{CapturedOutput, EngineContext, UnitResult as EngineUnitResult};

/// Copy the `sample` fixture crate to a unique temp dir and return its root.
/// Each test gets its own copy so parallel runs never share cargo state.
fn fixture_copy(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dest = std::env::temp_dir().join(format!(
        "arc-rs-unit-it-{}-{nanos}-{tag}",
        std::process::id()
    ));
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("unit")
        .join("sample");
    copy_dir(&src, &dest);
    dest
}

fn copy_dir(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn failing_fixture_maps_to_fail() {
    let root = fixture_copy("fail");
    let target = root.join("target");
    let opts = Options::default();
    let report = unit::execute(&opts, &root, Some(&target)).unwrap();

    assert_eq!(report.engine, "cargo");
    assert!(!report.success);
    assert_eq!((report.passed, report.failed), (0, 1));
    assert!(
        report.tail.to_lowercase().contains("fail"),
        "tail should echo cargo failure output, got:\n{}",
        report.tail
    );
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].name, "cargo");
    assert_eq!(report.results[0].outcome, unit::Outcome::Fail);

    let json = report.to_json();
    assert_eq!(json["engine"], "cargo");
    assert_eq!(json["passed"], 0);
    assert_eq!(json["failed"], 1);
    assert!(json["duration_ms"].is_number());

    let summary = report.summary_line();
    assert!(summary.starts_with("FAIL"), "{summary}");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn passing_filter_from_nested_dir_maps_to_pass() {
    let root = fixture_copy("pass");
    let target = root.join("target");
    // Filter selects only a passing test; cwd is nested so the Cargo.toml
    // walk-up is exercised on the real fixture layout.
    let opts = unit::parse_args(&args(&["--", "adds_positive_numbers"])).unwrap();
    let nested = root.join("src");
    assert_eq!(find_crate_root_from(&nested), Some(root.clone()));
    let report = unit::execute(&opts, &nested, Some(&target)).unwrap();

    assert!(report.success);
    assert_eq!((report.passed, report.failed), (1, 0));
    assert_eq!(report.results[0].outcome, unit::Outcome::Pass);
    assert!(report.summary_line().starts_with("PASS"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn unknown_engine_lists_available_engines() {
    let opts = unit::parse_args(&args(&["--engine", "phpunit"])).unwrap();
    let err = unit::execute(&opts, Path::new("."), None).unwrap_err();
    assert!(err.contains("unknown engine 'phpunit'"), "{err}");
    assert!(err.contains("available engines: cargo"), "{err}");
}

#[test]
fn missing_cargo_toml_is_a_clear_error() {
    let dir = std::env::temp_dir().join(format!("arc-rs-unit-it-{}-nocrates", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let err = unit::execute(&Options::default(), &dir, None).unwrap_err();
    assert!(err.contains("no Cargo.toml"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_rejects_bad_flags_and_answers_help() {
    assert_eq!(
        unit::run(&args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        unit::run(&args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(unit::NAME, "unit");
}

// --- W1 wiring: selection, config, render with canned outputs ---

fn wire_unit_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("unit")
        .join("wire_unit")
        .join(name)
}

fn temp_project(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-unit-wire-{}-{nanos}-{tag}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

const CANNED_XUNIT: &str = "<testsuite name=\"t\" tests=\"3\">\
    <testcase classname=\"a\" name=\"ok\" time=\"0.125\"/>\
    <testcase classname=\"a\" name=\"bad\" time=\"0.5\">\
    <failure>oops</failure></testcase>\
    <testcase classname=\"a\" name=\"skipped\" time=\"0.0\">\
    <skipped>later</skipped></testcase></testsuite>";

#[test]
fn selection_prefers_explicit_unit_engine_arcunit_cargo() {
    let dir = temp_project("sel");
    assert_eq!(
        unit::discover_selection(&Options::default(), &dir).unwrap(),
        unit::Selection::Cargo
    );

    std::fs::copy(wire_unit_fixture("arcunit_nose.json"), dir.join(".arcunit")).unwrap();
    assert!(matches!(
        unit::discover_selection(&Options::default(), &dir).unwrap(),
        unit::Selection::Config { .. }
    ));

    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"unit.engine": "PytestTestEngine"}"#,
    )
    .unwrap();
    assert_eq!(
        unit::discover_selection(&Options::default(), &dir).unwrap(),
        unit::Selection::Single("PytestTestEngine".to_string())
    );

    let explicit = unit::parse_args(&args(&["--engine", "NoseTestEngine"])).unwrap();
    assert_eq!(
        unit::discover_selection(&explicit, &dir).unwrap(),
        unit::Selection::Single("NoseTestEngine".to_string())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_nose_runs_with_canned_output() {
    let dir = temp_project("nose");
    std::fs::copy(wire_unit_fixture("arcunit_nose.json"), dir.join(".arcunit")).unwrap();
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::create_dir_all(dir.join("tests").join("pkg")).unwrap();
    std::fs::write(dir.join("pkg").join("mod.py"), "x = 1\n").unwrap();
    std::fs::write(
        dir.join("tests").join("pkg").join("test_mod.py"),
        "def test_ok(): pass\n",
    )
    .unwrap();

    let selection = unit::discover_selection(&Options::default(), &dir).unwrap();
    let unit::Selection::Config { engines, root } = selection else {
        panic!("expected Config selection");
    };
    assert_eq!(root, dir);
    assert_eq!(engines.len(), 1);

    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), CANNED_XUNIT).unwrap();
        CapturedOutput::new(1, "", "")
    };
    let report = unit::run_config_with_runner(
        &engines,
        &forward_slashes(&dir),
        &["pkg/mod.py".to_string()],
        false,
        Some(false),
        &runner,
    )
    .unwrap();
    assert_eq!(report.engine, "ArcanistConfigurationDrivenUnitTestEngine");
    assert_eq!((report.passed, report.failed), (1, 1));
    assert!(!report.success);
    assert_eq!(report.results.len(), 3);

    let rendered: String = report
        .results
        .iter()
        .map(unit::render_unit_result)
        .collect();
    assert!(rendered.contains("  PASS 125ms a.ok\n"), "{rendered}");
    assert!(rendered.contains("  FAIL a.bad\n"), "{rendered}");
    assert!(rendered.contains("oops\n"), "{rendered}");
    assert!(rendered.contains("  SKIP a.skipped\n"), "{rendered}");

    let json = report.to_json();
    assert_eq!(json["engine"], "ArcanistConfigurationDrivenUnitTestEngine");
    assert_eq!(json["passed"], 1);
    assert_eq!(json["failed"], 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_mixed_fixture_selects_two_engines() {
    let dir = temp_project("mixed");
    std::fs::copy(
        wire_unit_fixture("arcunit_mixed.json"),
        dir.join(".arcunit"),
    )
    .unwrap();
    let selection = unit::discover_selection(&Options::default(), &dir).unwrap();
    let unit::Selection::Config { engines, .. } = selection else {
        panic!("expected Config selection");
    };
    assert_eq!(engines.len(), 2);
    assert_eq!(engines[0].engine_type, "PytestTestEngine");
    assert_eq!(engines[1].engine_type, "PhpunitTestEngine");
    assert_eq!(engines[1].exclude, vec!["vendor/".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_phutil_run_errors_clearly_and_badtype_rejected() {
    let dir = temp_project("phutil");
    std::fs::copy(
        wire_unit_fixture("arcunit_phutil.json"),
        dir.join(".arcunit"),
    )
    .unwrap();
    let selection = unit::discover_selection(&Options::default(), &dir).unwrap();
    let unit::Selection::Config { engines, .. } = selection else {
        panic!("expected Config selection");
    };
    assert_eq!(engines[0].engine_type, "phutil");
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let err = unit::run_config_with_runner(
        &engines,
        &forward_slashes(&dir),
        &[],
        false,
        Some(false),
        &runner,
    )
    .unwrap_err();
    assert!(err.contains("cannot execute phutil"), "{err}");

    std::fs::copy(
        wire_unit_fixture("arcunit_badtype.json"),
        dir.join(".arcunit"),
    )
    .unwrap();
    let err = unit::discover_selection(&Options::default(), &dir).unwrap_err();
    assert!(
        err.contains("specifies invalid type 'BogusEngine'"),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn everything_rejected_when_no_child_supports_it() {
    let dir = temp_project("everything");
    std::fs::write(
        dir.join(".arcunit"),
        r#"{"engines": {"p": {"type": "PytestTestEngine"}}}"#,
    )
    .unwrap();
    let selection = unit::discover_selection(&Options::default(), &dir).unwrap();
    let unit::Selection::Config { engines, .. } = selection else {
        panic!("expected Config selection");
    };
    let runner = |_: &[String], _: &str| CapturedOutput::new(0, "", "");
    let err = unit::run_config_with_runner(
        &engines,
        &forward_slashes(&dir),
        &[],
        true,
        Some(false),
        &runner,
    )
    .unwrap_err();
    assert!(err.contains("does not support --everything"), "{err}");

    let ctx = EngineContext {
        project_root: forward_slashes(&dir),
        paths: Vec::new(),
        enable_coverage: Some(false),
        run_all_tests: true,
    };
    let err = unit::run_single_with_runner("PytestTestEngine", &ctx, &runner).unwrap_err();
    assert!(
        err.message().contains("does not support --everything"),
        "{}",
        err.message()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn render_per_test_lines_through_wired_lib() {
    let mut pass = EngineUnitResult::new();
    pass.set_name("a.ok").unwrap();
    pass.set_result("pass");
    pass.set_duration(2.5);
    assert_eq!(unit::render_unit_result(&pass), "  PASS  2.5s a.ok\n");

    let mut fail = EngineUnitResult::new();
    fail.set_namespace("ns");
    fail.set_name("bad").unwrap();
    fail.set_result("fail");
    fail.set_user_data("traceback");
    assert_eq!(
        unit::render_unit_result(&fail),
        "  FAIL ns::bad\ntraceback\n"
    );

    assert_eq!(unit::format_time(61.0), "1m01s");
    assert!(unit::supports_run_all_tests("NoseTestEngine"));
    assert!(!unit::supports_run_all_tests("PhpunitTestEngine"));
}
