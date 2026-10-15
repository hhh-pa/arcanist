use super::*;
use arc_rs::unit_engine::{
    CapturedOutput, EngineContext, RESULT_BROKEN, RESULT_FAIL, RESULT_PASS, RESULT_SKIP,
};
use std::collections::{HashMap, HashSet};

struct FakeFs {
    dirs: HashSet<String>,
    files: HashMap<String, String>,
}

impl PhpunitFs for FakeFs {
    fn is_dir(&self, path: &str) -> bool {
        self.dirs.contains(path)
    }

    fn path_exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
            || self.dirs.contains(path)
            || std::path::Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.files
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        self.files.get(path).map(|t| t.lines().count().max(1))
    }
}

#[test]
fn search_locations_cover_documented_layouts() {
    let dirs = get_search_locations_for_tests("/a/b/c/X.php");
    assert!(dirs.contains(&"/a/b/c/".to_string()), "{dirs:?}");
    assert!(dirs.contains(&"/a/b/c/tests/".to_string()), "{dirs:?}");
    assert!(dirs.contains(&"/a/b/tests/".to_string()), "{dirs:?}");
    assert!(dirs.contains(&"/a/tests/".to_string()), "{dirs:?}");
    assert!(dirs.contains(&"/tests/".to_string()), "{dirs:?}");
    // Replacement layout: /a/tests/c/.
    assert!(dirs.contains(&"/a/tests/c/".to_string()), "{dirs:?}");
    // Insertion layout: /tests/a/b/c/.
    assert!(dirs.contains(&"/tests/a/b/c/".to_string()), "{dirs:?}");
    // Deduplicated.
    let unique: HashSet<&String> = dirs.iter().collect();
    assert_eq!(unique.len(), dirs.len());
    // Capitalized variant present.
    assert!(dirs.contains(&"/a/b/c/Tests/".to_string()), "{dirs:?}");
}

#[test]
fn find_test_file_prefers_same_dir_and_skips_self() {
    let exists = |p: &str| p == "/a/b/c/XTest.php" || p == "/a/b/c/X.php";
    assert_eq!(
        find_test_file("/a/b/c/X.php", "/a", &exists),
        Some("/a/b/c/XTest.php".to_string())
    );
    // Only itself exists: no match.
    let only_self = |p: &str| p == "/a/b/c/X.php";
    assert_eq!(find_test_file("/a/b/c/X.php", "/a", &only_self), None);
    // Above the project root: skipped.
    let above = |p: &str| p == "/z/XTest.php";
    assert_eq!(find_test_file("/a/X.php", "/a", &above), None);
}

#[test]
fn selection_skips_dirs_and_non_php() {
    let is_dir = |p: &str| p == "/r/pkg";
    let exists = |p: &str| {
        matches!(
            p,
            "/r/pkg/Mod.php" | "/r/pkg/ModTest.php" | "/r/OtherTest.php"
        )
    };
    let affected = select_affected_tests(
        &[
            "pkg".to_string(),
            "pkg/Mod.php".to_string(),
            "note.txt".to_string(),
            "OtherTest.php".to_string(),
        ],
        "/r",
        &is_dir,
        &exists,
    )
    .unwrap();
    assert_eq!(
        affected.get("/r/pkg/Mod.php").unwrap(),
        "/r/pkg/ModTest.php"
    );
    assert_eq!(
        affected.get("/r/OtherTest.php").unwrap(),
        "/r/OtherTest.php"
    );
    assert_eq!(affected.len(), 2);

    let err = select_affected_tests(&["note.txt".to_string()], "/r", &is_dir, &exists).unwrap_err();
    assert!(err.is_no_effect());
}

#[test]
fn config_and_binary_resolution() {
    let exists = |p: &str| p == "/r/phpunit.xml";
    assert_eq!(
        resolve_config_file("/r", Some("phpunit.xml"), &exists).unwrap(),
        Some("/r/phpunit.xml".to_string())
    );
    assert_eq!(resolve_config_file("/r", None, &exists).unwrap(), None);
    let err = resolve_config_file("/r", Some("missing.xml"), &exists).unwrap_err();
    assert!(
        err.message()
            .contains("PHPUnit configuration file was not found in"),
        "{}",
        err.message()
    );

    let on_path = |b: &str| b == "phpunit";
    assert_eq!(resolve_phpunit_binary("/r", None, &on_path), "phpunit");
    assert_eq!(
        resolve_phpunit_binary("/r", Some("phpunit"), &on_path),
        "phpunit"
    );
    assert_eq!(
        resolve_phpunit_binary("/r", Some("tools/phpunit"), &on_path),
        "/r/tools/phpunit"
    );
}

#[test]
fn argv_shape_matches_exec_future() {
    assert_eq!(
        build_phpunit_argv("phpunit", Some("p.xml"), "j.json", Some("c.xml"), "T.php"),
        vec![
            "phpunit",
            "-c",
            "p.xml",
            "-d display_errors=stderr",
            "--log-json",
            "j.json",
            "--coverage-clover",
            "c.xml",
            "T.php"
        ]
    );
    assert_eq!(
        build_phpunit_argv("phpunit", None, "j.json", None, "T.php"),
        vec![
            "phpunit",
            "-d display_errors=stderr",
            "--log-json",
            "j.json",
            "T.php"
        ]
    );
}

#[test]
fn json_report_repair_and_empty_guidance() {
    let report = get_json_report("{\"event\":\"test\"} {\"event\":\"testStart\"}").unwrap();
    assert_eq!(report.as_arr().unwrap().len(), 2);
    let err = get_json_report("").unwrap_err();
    assert!(
        err.message().contains("JSON report file is empty"),
        "{}",
        err.message()
    );
}

#[test]
fn parser_maps_events_statuses_and_trailing_start() {
    let json = concat!(
        "{\"event\":\"testStart\",\"test\":\"A::t0\"}",
        "{\"event\":\"test\",\"test\":\"A::t1 (with data)\",\"status\":\"pass\",\"time\":0.5}",
        "{\"event\":\"test\",\"test\":\"A::t2\",\"status\":\"fail\",\"message\":\"nope\",\"time\":1,",
        "\"trace\":[{\"file\":\"F.php\",\"line\":3}]}",
        "{\"event\":\"test\",\"test\":\"A::t3\",\"status\":\"error\",\"message\":\"Skipped Test: x\",\"time\":0}",
        "{\"event\":\"test\",\"test\":\"A::t4\",\"status\":\"error\",\"message\":\"boom\",\"time\":0,\"trace\":[]}",
        "{\"event\":\"testStart\",\"test\":\"A::t5\"}",
    );
    let affected = HashMap::new();
    let results = parse_phpunit_results(
        "T.php",
        json,
        None,
        "STDERR",
        Some(false),
        "/r",
        &affected,
        &|_| None,
    )
    .unwrap();
    assert_eq!(results.len(), 5);
    assert_eq!(results[0].get_name(), "A::t1");
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert_eq!(results[0].get_duration(), Some(0.5));
    assert_eq!(results[1].get_result(), RESULT_FAIL);
    assert!(
        results[1].get_user_data().unwrap().contains("F.php:3"),
        "{:?}",
        results[1].get_user_data()
    );
    assert_eq!(results[2].get_result(), RESULT_SKIP);
    assert_eq!(results[3].get_result(), RESULT_BROKEN);
    assert_eq!(results[4].get_name(), "A::t5");
    assert_eq!(results[4].get_result(), RESULT_BROKEN);
    assert_eq!(results[4].get_user_data(), Some("STDERR"));

    let results = parse_phpunit_results(
        "T.php",
        "",
        None,
        "STDERR",
        Some(false),
        "/r",
        &affected,
        &|_| None,
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_name(), "T.php");
    assert_eq!(results[0].get_result(), RESULT_BROKEN);
}

#[test]
fn clover_filters_uncovered_files_and_relativizes() {
    let clover = r#"<coverage><project>
      <file name="/r/src/A.php"><line num="1" type="stmt" count="1"/><line num="2" type="stmt" count="0"/><line num="3" type="method" count="0"/></file>
      <file name="/r/src/B.php"><line num="1" type="stmt" count="0"/></file>
      <file name="/r/src/C.php"><line num="1" type="stmt" count="1"/></file>
    </project></coverage>"#;
    let mut affected = HashMap::new();
    affected.insert("/r/src/A.php".to_string(), "t".to_string());
    affected.insert("/r/src/B.php".to_string(), "t".to_string());
    let counts = |_: &str| Some(3);
    let reports = read_clover_coverage(clover, "/r", &affected, &counts);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports.get("src/A.php").unwrap(), "CUN");
    assert!(read_clover_coverage("", "/r", &affected, &counts).is_empty());
}

#[test]
fn run_with_uses_canned_reports() {
    let mut files = HashMap::new();
    files.insert("/r/TTest.php".to_string(), "<?php\n".to_string());
    let fs = FakeFs {
        dirs: HashSet::new(),
        files,
    };
    let engine = PhpunitEngine::with_binary_and_config("fake-phpunit", None);
    let ctx = EngineContext {
        paths: vec!["TTest.php".to_string()],
        enable_coverage: Some(false),
        ..EngineContext::new("/r")
    };
    let runner = |argv: &[String], _cwd: &str| {
        assert_eq!(argv[0], "fake-phpunit");
        let json = argv
            .iter()
            .skip_while(|a| a.as_str() != "--log-json")
            .nth(1)
            .unwrap();
        std::fs::write(
            json,
            "{\"event\":\"test\",\"test\":\"T::t\",\"status\":\"pass\",\"time\":0.1}",
        )
        .unwrap();
        CapturedOutput::new(0, "", "")
    };
    let results = engine.run_with(&ctx, &runner, &fs).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert!(!engine.supports_run_all_tests());
    assert_eq!(engine.engine_configuration_name(), None);
}
