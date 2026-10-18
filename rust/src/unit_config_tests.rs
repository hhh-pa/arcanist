use super::*;

fn planned(name: &str) -> PlannedEngine {
    PlannedEngine {
        name: name.to_string(),
        engine_type: "phutil".to_string(),
        include: Vec::new(),
        exclude: Vec::new(),
    }
}

#[test]
fn parses_engines_map_with_rule_lists() {
    let data = r#"{"engines": {
      "a": {"type": "phutil"},
      "b": {"type": "nose", "include": "src/", "exclude": ["gen/", "tmp/"]}
    }}"#;
    let specs = parse_engine_specs(data, "/r/.arcunit", &["phutil", "nose"]).unwrap();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].engine_type, "phutil");
    assert!(specs[0].include.is_empty());
    assert_eq!(specs[1].include, vec!["src/".to_string()]);
    assert_eq!(
        specs[1].exclude,
        vec!["gen/".to_string(), "tmp/".to_string()]
    );
}

#[test]
fn invalid_type_names_available_engines() {
    let data = r#"{"engines": {"a": {"type": "bogus"}}}"#;
    let err = parse_engine_specs(data, "/r/.arcunit", &["phutil", "nose"]).unwrap_err();
    let message = err.message();
    assert!(
        message.contains("Test engine 'a' specifies invalid type 'bogus'."),
        "{message}"
    );
    assert!(err.message().contains("phutil, nose"), "{}", err.message());
    assert!(matches!(err, EngineError::Usage(_)));
}

#[test]
fn missing_type_and_bad_shapes_error() {
    // T1153 UNIT-U16: generic proxy wrappers plus the cause on the next
    // line, like PHP's `PhutilProxyException` rendering.
    let err =
        parse_engine_specs(r#"{"engines": {"a": {}}}"#, "/r/.arcunit", &["phutil"]).unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '.arcunit' file, for test engine 'a'.\nexpected a 'type' string"
    );
    let err = parse_engine_specs(
        r#"{"engines": {"a": {"type": "phutil", "include": 7}}}"#,
        "/r/.arcunit",
        &["phutil"],
    )
    .unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '.arcunit' file, for test engine 'a'.\n'include' must be a regex or a list of regexes"
    );
    let err = parse_engine_specs(
        r#"{"engines": {"a": {"type": "phutil", "bogus": 1}}}"#,
        "/r/.arcunit",
        &["phutil"],
    )
    .unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '.arcunit' file, for test engine 'a'.\nunknown key 'bogus'"
    );
    let err = parse_engine_specs("not json", "/r/.arcunit", &["phutil"]).unwrap_err();
    let message = err.message();
    assert!(
        message.starts_with(
            "Expected '.arcunit' file to be a valid JSON file, but failed to decode '/r/.arcunit'.\n"
        ),
        "{message}"
    );
    let err = parse_engine_specs(r#"{"nope": 1}"#, "/r/.arcunit", &["phutil"]).unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '/r/.arcunit' file.\nexpected an 'engines' map"
    );
    let err =
        parse_engine_specs(r#"{"engines": {"a": 7}}"#, "/r/.arcunit", &["phutil"]).unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '/r/.arcunit' file.\nengine 'a' must be a map"
    );
}

#[test]
fn match_paths_applies_include_then_exclude() {
    let paths = vec![
        "src/a.py".to_string(),
        "gen/b.py".to_string(),
        "src/c.txt".to_string(),
    ];
    let all = match_paths(&paths, &[], &[]);
    assert_eq!(all, paths);
    let inc = match_paths(&paths, &["/src/".to_string()], &[]);
    assert_eq!(inc, vec!["src/a.py".to_string(), "src/c.txt".to_string()]);
    let exc = match_paths(&paths, &[], &["/\\.txt$/".to_string()]);
    assert_eq!(exc, vec!["src/a.py".to_string(), "gen/b.py".to_string()]);
    let both = match_paths(&paths, &["/src/".to_string()], &["/\\.txt$/".to_string()]);
    assert_eq!(both, vec!["src/a.py".to_string()]);
}

#[test]
fn rule_matcher_uses_preg_semantics() {
    // T1153 UNIT-U17: full PCRE via the shared engine (classes,
    // alternation, modifiers), unanchored like `preg_match`; invalid
    // patterns never match (PHP warns and returns false).
    assert!(rule_matches("@src/@", "src/a.py"));
    assert!(!rule_matches("@src/@", "lib/a.py"));
    assert!(rule_matches("/^src\\//", "src/a.py"));
    assert!(!rule_matches("/^src\\//", "lib/src/a.py"));
    assert!(rule_matches("/\\.(py|txt)$/", "src/a.py"));
    assert!(rule_matches("/[a-z]+\\.py$/", "src/a.py"));
    assert!(rule_matches("/SRC/i", "src/a.py"));
    assert!(!rule_matches("/SRC/", "src/a.py"));
    assert!(rule_matches("/(.*)Tests\\/(.*)/", "a/Tests/b"));
    assert!(!rule_matches("/(.*)Tests\\/(.*)/", "a/b"));
    assert!(!rule_matches("src/", "src/a.py"));
    assert!(!rule_matches("(.*)Tests/(.*)", "a/Tests/b"));
    assert!(!rule_matches("/[/", "src/a.py"));
    assert!(!rule_matches("", "src/a.py"));
}

#[test]
fn merge_runs_concatenates_and_preserves_no_effect() {
    let mut result = UnitResult::new();
    result.set_name("t").unwrap();
    let merged = merge_engine_runs(vec![
        Ok(vec![result.clone()]),
        Err(EngineError::no_effect()),
        Ok(vec![result.clone()]),
    ])
    .unwrap();
    assert_eq!(merged.len(), 2);
    let err = merge_engine_runs(vec![
        Err(EngineError::no_effect()),
        Err(EngineError::no_effect()),
    ])
    .unwrap_err();
    assert!(err.is_no_effect());
    assert_eq!(err.message(), "No tests to run.");
    let hard = merge_engine_runs(vec![
        Err(EngineError::no_effect()),
        Err(EngineError::Failed("boom".to_string())),
    ])
    .unwrap_err();
    assert_eq!(hard.message(), "boom");
}

#[test]
fn child_context_assigns_paths_and_run_all() {
    let ctx = EngineContext {
        project_root: "/r".to_string(),
        paths: vec!["src/a.py".to_string(), "gen/b.py".to_string()],
        enable_coverage: Some(true),
        run_all_tests: true,
    };
    let engine = PlannedEngine {
        include: vec!["/src/".to_string()],
        ..planned("a")
    };
    let child = child_context(&ctx, &engine, true);
    assert_eq!(child.paths, vec!["src/a.py".to_string()]);
    assert!(child.run_all_tests);
    let child = child_context(&ctx, &engine, false);
    assert!(!child.run_all_tests);
    let empty = EngineContext::new("/r");
    assert!(paths_for_engine(&empty.paths, &[], &[]).is_none());
}

#[test]
fn supports_run_all_is_any() {
    assert!(supports_run_all_tests(&[false, true]));
    assert!(!supports_run_all_tests(&[false, false]));
    assert!(!supports_run_all_tests(&[]));
    assert_eq!(available_index(&["a", "b"], "b"), Some(1));
    assert_eq!(available_index(&["a"], "z"), None);
    assert_eq!(available_list(&["a", "b"]), "a, b");
}

#[test]
fn non_string_rule_entries_reject_the_list() {
    let err = parse_engine_specs(
        r#"{"engines": {"a": {"type": "phutil", "include": ["src/", 7]}}}"#,
        "/r/.arcunit",
        &["phutil"],
    )
    .unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '.arcunit' file, for test engine 'a'.\n'include' must be a regex or a list of regexes"
    );
}

#[test]
fn bad_exclude_shape_errors() {
    let err = parse_engine_specs(
        r#"{"engines": {"a": {"type": "phutil", "exclude": 7}}}"#,
        "/r/.arcunit",
        &["phutil"],
    )
    .unwrap_err();
    assert_eq!(
        err.message(),
        "Error in parsing '.arcunit' file, for test engine 'a'.\n'exclude' must be a regex or a list of regexes"
    );
}

#[test]
fn counts_results_by_code() {
    use arc_rs::unit_engine::{RESULT_FAIL, RESULT_PASS};
    let mut pass = UnitResult::new();
    pass.set_result(RESULT_PASS);
    let mut fail = UnitResult::new();
    fail.set_result(RESULT_FAIL);
    let counts = count_by_result(&[pass.clone(), fail, pass]);
    assert_eq!(counts.get(RESULT_PASS), Some(&2));
    assert_eq!(counts.get(RESULT_FAIL), Some(&1));
    assert!(count_by_result(&[]).is_empty());
}
