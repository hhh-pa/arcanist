#[path = "../src/unit.rs"]
mod unit;

use std::path::{Path, PathBuf};

use unit::{Options, find_crate_root_from};

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
