use super::*;

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
