use super::*;
use arc_rs::unit_engine::EngineError;

#[test]
fn selection_prefers_run_all_and_rejects_empty() {
    let all = select_tests(true, vec!["A".to_string()], vec!["B".to_string()]).unwrap();
    assert_eq!(all, vec!["A".to_string()]);
    let some = select_tests(false, vec!["A".to_string()], vec!["B".to_string()]).unwrap();
    assert_eq!(some, vec!["B".to_string()]);
    let err = select_tests(false, Vec::new(), Vec::new()).unwrap_err();
    assert!(err.is_no_effect());
    assert_eq!(ENGINE_CONFIGURATION_NAME, "phutil");
}

#[test]
fn coverage_gate_matches_xdebug_branches() {
    assert_eq!(resolve_coverage(Some(false), true).unwrap(), Some(false));
    assert_eq!(resolve_coverage(Some(false), false).unwrap(), Some(false));
    assert_eq!(resolve_coverage(None, true).unwrap(), Some(true));
    assert_eq!(resolve_coverage(None, false).unwrap(), None);
    assert_eq!(resolve_coverage(Some(true), true).unwrap(), Some(true));
    let err = resolve_coverage(Some(true), false).unwrap_err();
    assert!(matches!(err, EngineError::Usage(_)));
    assert!(
        err.message()
            .contains("You specified --coverage but XDebug is not available"),
        "{}",
        err.message()
    );
    assert!(
        err.message().contains("PhutilUnitTestEngine"),
        "{}",
        err.message()
    );
}

#[test]
fn symbol_filter_requires_tests_dir_and_working_copy() {
    assert!(symbol_should_run("/lib/__tests__/A.php", "/lib", "/"));
    assert!(symbol_should_run("__tests__/A.php", "/lib", "/"));
    assert!(!symbol_should_run("/lib/src/A.php", "/lib", "/"));
    assert!(!symbol_should_run("/lib/__tests__/A.php", "/lib", "/other"));
}

#[test]
fn collect_names_dedupes_in_order() {
    let names = collect_test_names(&[
        vec!["B".to_string(), "A".to_string()],
        vec!["A".to_string(), "C".to_string()],
    ]);
    assert_eq!(
        names,
        vec!["B".to_string(), "A".to_string(), "C".to_string()]
    );
}

#[test]
fn test_paths_walk_to_library_root() {
    let root_for = |_: &str| Some("/lib".to_string());
    let name_for = |_: &str| Some("lib".to_string());
    let is_file = |_: &str| true;
    let paths = get_test_paths(
        &["/lib/src/sub/Thing.php".to_string()],
        "/",
        &root_for,
        &name_for,
        &is_file,
    )
    .unwrap();
    let rendered: Vec<String> = paths
        .iter()
        .map(|p| format!("{}:{}", p.library, p.path))
        .collect();
    assert!(
        rendered.contains(&"lib:src/sub/__tests__/".to_string()),
        "{rendered:?}"
    );
    assert!(
        rendered.contains(&"lib:src/__tests__/".to_string()),
        "{rendered:?}"
    );
    assert!(
        rendered.contains(&"lib:__tests__/".to_string()),
        "{rendered:?}"
    );

    // Files already under __tests__/ map directly.
    let direct = get_test_paths(
        &["/lib/__tests__/A.php".to_string()],
        "/",
        &root_for,
        &name_for,
        &is_file,
    )
    .unwrap();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].path, "__tests__/A.php");
}

#[test]
fn test_paths_skip_outside_and_unloaded() {
    // No library: skipped.
    let none = |_: &str| None;
    let name = |_: &str| Some("lib".to_string());
    let is_file = |_: &str| false;
    let paths = get_test_paths(&["x.php".to_string()], "/", &none, &name, &is_file).unwrap();
    assert!(paths.is_empty());

    // Unloaded library: verbatim error.
    let root_for = |_: &str| Some("/lib".to_string());
    let no_name = |_: &str| None;
    let err =
        get_test_paths(&["x.php".to_string()], "/", &root_for, &no_name, &is_file).unwrap_err();
    assert!(
        err.message().contains("not been loaded"),
        "{}",
        err.message()
    );

    // Symlink maze: path outside its library is skipped.
    let root_other = |_: &str| Some("/other".to_string());
    let paths = get_test_paths(
        &["/lib/A.php".to_string()],
        "/",
        &root_other,
        &name,
        &is_file,
    )
    .unwrap();
    assert!(paths.is_empty());
}

#[test]
fn walk_stops_at_library_root() {
    assert_eq!(
        walk_to_root("/lib/src/sub", "/lib"),
        vec![
            "/lib/src/sub".to_string(),
            "/lib/src".to_string(),
            "/lib".to_string()
        ]
    );
    assert_eq!(walk_to_root("/lib", "/lib"), vec!["/lib".to_string()]);
}

// --- Out-of-process execution (U5) ---

#[test]
fn php_discovery_probes_path() {
    assert_eq!(php_candidates(), &["php"]);
    assert_eq!(PHP_BINARY, "php");
    assert_eq!(INIT_SCRIPT_KEY, "unit.phutil.init-script");
    // Self-consistent without assuming php is present or absent.
    match php_on_path() {
        Some(path) => assert_eq!(
            php_argv_prefix(),
            Some(vec![path.to_string_lossy().into_owned()])
        ),
        None => assert_eq!(php_argv_prefix(), None),
    }
    assert_eq!(
        missing_php_message("phutil"),
        "Engine 'phutil' requires PHP on your $PATH (php) to execute phutil tests."
    );
}

#[test]
fn input_json_round_trips() {
    let input = PhutilRunInput {
        project_root: "/r".to_string(),
        run_all_tests: true,
        paths: vec!["a.php".to_string()],
        enable_coverage: None,
        init_script: Some("init.php".to_string()),
    };
    let text = phutil_input_json(&input);
    let back: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back["projectRoot"], "/r");
    assert_eq!(back["runAllTests"], true);
    assert_eq!(back["paths"], serde_json::json!(["a.php"]));
    assert!(back["enableCoverage"].is_null());
    assert_eq!(back["initScript"], "init.php");

    let covered = PhutilRunInput {
        enable_coverage: Some(true),
        init_script: None,
        ..input.clone()
    };
    let back: serde_json::Value = serde_json::from_str(&phutil_input_json(&covered)).unwrap();
    assert_eq!(back["enableCoverage"], true);
    assert!(back["initScript"].is_null());
}

#[test]
fn php_argv_shape_is_pinned() {
    let pair = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        build_php_argv(&["php".to_string()], "d.php", "i.json", "o.json"),
        pair(&["php", "d.php", "i.json", "o.json"])
    );
    // Prefix argv (Windows `cmd /C fake.bat`) stays in front.
    assert_eq!(
        build_php_argv(
            &["cmd".to_string(), "/C".to_string(), "f.bat".to_string()],
            "d.php",
            "i.json",
            "o.json",
        ),
        pair(&["cmd", "/C", "f.bat", "d.php", "i.json", "o.json"])
    );
}

#[test]
fn known_result_codes_cover_all_five() {
    for code in ["pass", "fail", "skip", "broken", "unsound"] {
        assert!(is_known_result_code(code), "{code}");
    }
    assert!(!is_known_result_code("bogus"));
    assert!(!is_known_result_code(""));
}

#[test]
fn parses_driver_results() {
    let text = r#"{"results": [
        {"namespace": "C", "name": "testIt", "link": "http://x/", "result": "pass",
         "duration": 0.25, "userData": "3 assertions",
         "coverage": {"a.php": "CC"}},
        {"name": "testSkip", "result": "skip"}
    ]}"#;
    let results = parse_phutil_output(text, "o.json").unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].get_namespace(), Some("C"));
    assert_eq!(results[0].get_name(), "testIt");
    assert_eq!(results[0].get_link(), Some("http://x/"));
    assert_eq!(results[0].get_result(), "pass");
    assert_eq!(results[0].get_duration(), Some(0.25));
    assert_eq!(results[0].get_user_data(), Some("3 assertions"));
    assert_eq!(
        results[0].get_coverage().unwrap().get("a.php").unwrap(),
        "CC"
    );
    assert_eq!(results[1].get_result(), "skip");
    assert_eq!(results[1].get_duration(), None);
}

#[test]
fn parses_driver_errors_and_empty_selection() {
    let err = parse_phutil_output(r#"{"error": "no init"}"#, "o.json").unwrap_err();
    assert!(matches!(err, EngineError::Usage(_)));
    assert_eq!(err.message(), "no init");

    let err = parse_phutil_output(r#"{"results": []}"#, "o.json").unwrap_err();
    assert!(err.is_no_effect());
    assert_eq!(err.message(), "No tests to run.");
}

#[test]
fn rejects_malformed_driver_output() {
    let err = parse_phutil_output("not json", "o.json").unwrap_err();
    assert!(
        err.message()
            .contains("cannot parse phutil output 'o.json'"),
        "{}",
        err.message()
    );

    let err = parse_phutil_output(r#"{"results": [{"result": "pass"}]}"#, "o.json").unwrap_err();
    assert!(
        err.message().contains("missing its 'name'"),
        "{}",
        err.message()
    );

    let err = parse_phutil_output(r#"{"results": [{"name": "t"}]}"#, "o.json").unwrap_err();
    assert!(
        err.message().contains("missing its 'result'"),
        "{}",
        err.message()
    );

    let err = parse_phutil_output(
        r#"{"results": [{"name": "t", "result": "bogus"}]}"#,
        "o.json",
    )
    .unwrap_err();
    assert!(
        err.message().contains("unknown result code 'bogus'"),
        "{}",
        err.message()
    );

    let err = parse_phutil_output(r#"{"other": 1}"#, "o.json").unwrap_err();
    assert!(
        err.message().contains("neither 'results' nor 'error'"),
        "{}",
        err.message()
    );

    let long = "t".repeat(256);
    let err = parse_phutil_output(
        &format!(r#"{{"results": [{{"name": "{long}", "result": "pass"}}]}}"#),
        "o.json",
    )
    .unwrap_err();
    assert!(
        err.message().contains("maximum length"),
        "{}",
        err.message()
    );
}

#[test]
fn driver_source_covers_selection_execution_and_gates() {
    let driver = phutil_driver_source();
    // Protocol: input/output argv, JSON in/out.
    assert!(driver.contains("$argv[1]"), "{driver}");
    assert!(driver.contains("$argv[2]"), "{driver}");
    // Init-script gate with the config key.
    assert!(driver.contains("unit.phutil.init-script"), "{driver}");
    // PHP-exact coverage gate plus the out-of-process limitation.
    assert!(driver.contains("xdebug_start_code_coverage"), "{driver}");
    assert!(
        driver.contains("You specified --coverage but XDebug"),
        "{driver}"
    );
    assert!(driver.contains("out-of-process"), "{driver}");
    // Selection mirrors getAllTests/getTestsForPaths.
    assert!(driver.contains("selectSymbolsWithoutLoading"), "{driver}");
    assert!(driver.contains("selectAndLoadSymbols"), "{driver}");
    assert!(driver.contains("__tests__"), "{driver}");
    // Execution mirrors run(): hooks, merge, toDictionary emit.
    assert!(driver.contains("willRunTestCases"), "{driver}");
    assert!(driver.contains("didRunTestCases"), "{driver}");
    assert!(driver.contains("toDictionary"), "{driver}");
}
