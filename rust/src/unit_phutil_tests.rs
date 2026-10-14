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
