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
    assert!(
        err.message()
            .contains("Test engine 'a' specifies invalid type 'bogus'."),
        "{}",
        err.message()
    );
    assert!(err.message().contains("phutil, nose"), "{}", err.message());
    assert!(matches!(err, EngineError::Usage(_)));
}

#[test]
fn missing_type_and_bad_shapes_error() {
    let err =
        parse_engine_specs(r#"{"engines": {"a": {}}}"#, "/r/.arcunit", &["phutil"]).unwrap_err();
    assert!(
        err.message().contains("for test engine 'a'"),
        "{}",
        err.message()
    );
    let err = parse_engine_specs(
        r#"{"engines": {"a": {"type": "phutil", "include": 7}}}"#,
        "/r/.arcunit",
        &["phutil"],
    )
    .unwrap_err();
    assert!(err.message().contains("'include'"), "{}", err.message());
    let err = parse_engine_specs("not json", "/r/.arcunit", &["phutil"]).unwrap_err();
    assert!(
        err.message().contains("valid JSON file"),
        "{}",
        err.message()
    );
    assert!(err.message().contains("/r/.arcunit"), "{}", err.message());
    let err = parse_engine_specs(r#"{"nope": 1}"#, "/r/.arcunit", &["phutil"]).unwrap_err();
    assert!(err.message().contains("'engines'"), "{}", err.message());
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
    let inc = match_paths(&paths, &["src/".to_string()], &[]);
    assert_eq!(inc, vec!["src/a.py".to_string(), "src/c.txt".to_string()]);
    let exc = match_paths(&paths, &[], &[".txt".to_string()]);
    assert_eq!(exc, vec!["src/a.py".to_string(), "gen/b.py".to_string()]);
    let both = match_paths(&paths, &["src/".to_string()], &[".txt".to_string()]);
    assert_eq!(both, vec!["src/a.py".to_string()]);
}

#[test]
fn rule_matcher_strips_delimiters_and_orders_captures() {
    assert!(rule_matches("@src/@", "src/a.py"));
    assert!(!rule_matches("@src/@", "lib/a.py"));
    assert!(rule_matches("/^src\\//", "src/a.py"));
    assert!(rule_matches("(.*)Tests/(.*)", "a/Tests/b"));
    assert!(!rule_matches("(.*)Tests/(.*)", "a/b"));
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
        include: vec!["src/".to_string()],
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
