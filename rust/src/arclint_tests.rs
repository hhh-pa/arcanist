use super::*;

fn contains_matcher(pattern: &str, path: &str) -> bool {
    // Test-only stand-in for PCRE: treat the pattern as a substring probe.
    path.contains(pattern)
}

#[test]
fn parses_sample_config() {
    let text = include_str!("../tests/fixtures/lintcfg/arclint.sample.json");
    let config = parse_arclint(text).unwrap();
    assert_eq!(config.global_exclude, vec!["externals/".to_string()]);
    assert_eq!(config.linters.len(), 3);
    // Sorted-key order: json-files, spelling-strict, text.
    assert_eq!(config.linters[0].name, "json-files");
    assert_eq!(config.linters[0].linter_type, "json");
    assert_eq!(config.linters[0].display, "JSON");
    assert_eq!(
        config.linters[1]
            .severity
            .map
            .get("SPELL1")
            .map(String::as_str),
        Some("error")
    );
    assert_eq!(config.linters[2].display, "TXT");
    assert_eq!(
        selected_display_names(&config),
        vec!["JSON", "SPELL", "TXT"]
    );
}

#[test]
fn resolves_all_configuration_names_verbatim() {
    let cases = [
        ("text", "TXT"),
        ("filename", "NAME"),
        ("json", "JSON"),
        ("jsonlint", "JSON"),
        ("xml", "XML"),
        ("merge-conflict", "MERGECONFLICT"),
        ("generated", "GEN"),
        ("nolint", "NOLINT"),
        ("chmod", "CHMOD"),
        ("spelling", "SPELL"),
        ("script-and-regex", "S&RX"),
        ("composer", "COMPOSER"),
        ("phutil-library", "PHL"),
        ("jshint", "JSHint"),
        ("jscs", "JSCS"),
        ("csslint", "CSSLint"),
        ("lessc", "LESSC"),
        ("gjslint", "GJSLINT"),
        ("coffeelint", "COFFEE"),
        ("flake8", "flake8"),
        ("pyflakes", "PYFLAKES"),
        ("pylint", "PyLint"),
        ("pep8", "PEP8"),
        ("rubocop", "RuboCop"),
        ("ruby", "RUBY"),
        ("cppcheck", "cppcheck"),
        ("cpplint", "C++ Google's Styleguide"),
        ("golint", "GOLINT"),
        ("hlint", "HLINT"),
        ("csharp", "C#"),
        ("phpcs", "PHPCS"),
        ("php", "PHP"),
        ("puppet-lint", "PUPPETLINT"),
        ("xhpast", "XHP"),
    ];
    assert_eq!(cases.len(), 34);
    for (config_name, display) in cases {
        assert_eq!(
            configuration_name_to_display(config_name),
            Some(display),
            "for {config_name}"
        );
        assert_eq!(resolve_type(config_name), Some(display));
        // Exact display names are accepted as aliases.
        assert_eq!(resolve_type(display), Some(display));
    }
    assert_eq!(configuration_name_to_display("TXT"), None);
    assert_eq!(configuration_name_to_display("nope"), None);
    assert_eq!(display_names().len(), 33);
    assert!(display_names().contains(&"C++ Google's Styleguide"));
}

#[test]
fn rejects_malformed_and_misshapen_configs() {
    let err = parse_arclint("{oops").unwrap_err();
    assert!(err.contains("valid JSON"), "unexpected: {err}");

    let err = parse_arclint("[]").unwrap_err();
    assert!(
        err.contains("top level must be an object"),
        "unexpected: {err}"
    );

    let err = parse_arclint("{}").unwrap_err();
    assert!(err.contains("'linters' is required"), "unexpected: {err}");

    let err = parse_arclint(r#"{"linters": []}"#).unwrap_err();
    assert!(err.contains("'linters' must be a map"), "unexpected: {err}");

    let err = parse_arclint(r#"{"linters": {"a": {"type": 7}}}"#).unwrap_err();
    assert!(err.contains("for linter 'a'"), "unexpected: {err}");

    let err =
        parse_arclint(r#"{"linters": {"a": {"type": "text", "include": ["x", 7]}}}"#).unwrap_err();
    assert!(err.contains("list of strings"), "unexpected: {err}");

    let err = parse_arclint(r#"{"exclude": 7, "linters": {}}"#).unwrap_err();
    assert!(err.contains("'exclude'"), "unexpected: {err}");
}

#[test]
fn rejects_unknown_types_with_available_list() {
    let err = parse_arclint(r#"{"linters": {"mine": {"type": "nope"}}}"#).unwrap_err();
    assert!(
        err.contains("Linter 'mine' specifies invalid type 'nope'. Available linters are: "),
        "unexpected: {err}"
    );
    assert!(err.contains("cpplint"), "unexpected: {err}");
}

#[test]
fn validates_severity_values() {
    let err = parse_arclint(r#"{"linters": {"a": {"type": "text", "severity": {"1": "bogus"}}}}"#)
        .unwrap_err();
    assert_eq!(
        err,
        "Error in parsing '.arclint' file, for linter 'a': Unknown lint severity \"bogus\". Valid severities are: error, warning, autofix, advice, disabled."
    );
    let err =
        parse_arclint(r#"{"linters": {"a": {"type": "text", "severity.rules": {"x": "bogus"}}}}"#)
            .unwrap_err();
    assert!(err.contains("Unknown lint severity"), "unexpected: {err}");

    for level in SEVERITIES {
        let text = format!(
            "{{\"linters\": {{\"a\": {{\"type\": \"text\", \"severity\": {{\"1\": \"{level}\"}}}}}}}}"
        );
        assert!(parse_arclint(&text).is_ok(), "for {level}");
    }
    assert!(!is_valid_severity("Bogus"));
}

#[test]
fn keeps_linter_specific_keys_opaquely() {
    let config = parse_arclint(
        r#"{"linters": {"x": {"type": "xhpast", "include": "(\\.php$)", "standard": "phutil.xhpast"}}}"#,
    )
    .unwrap();
    assert_eq!(config.linters[0].include, vec!["(\\.php$)".to_string()]);
    assert_eq!(
        config.linters[0].extra.get("standard"),
        Some(&serde_json::json!("phutil.xhpast"))
    );
}

#[test]
fn severity_resolution_prefers_map_then_rules_then_default() {
    let config = parse_arclint(
        r#"{"linters": {"a": {"type": "text",
          "severity": {"TXT2": "disabled"},
          "severity.rules": {"TXT": "warning"}}}}"#,
    )
    .unwrap();
    let severity = &config.linters[0].severity;
    assert_eq!(
        severity.severity_for("TXT2", "error", contains_matcher),
        "disabled"
    );
    assert_eq!(
        severity.severity_for("TXT6", "error", contains_matcher),
        "warning"
    );
    assert_eq!(
        severity.severity_for("OTHER", "error", contains_matcher),
        "error"
    );
}

#[test]
fn routes_paths_through_include_exclude_and_global() {
    let paths = ["a.php", "b.js", "externals/c.php", "docs/d.php"]
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    // No includes: everything kept except excludes.
    let routed = route_paths(&paths, &[], &[], &[], contains_matcher);
    assert_eq!(routed, paths);
    // Include narrows; exclude and global-exclude remove.
    let routed = route_paths(
        &paths,
        &[".php".to_string()],
        &["docs/".to_string()],
        &["externals/".to_string()],
        contains_matcher,
    );
    assert_eq!(routed, vec!["a.php".to_string()]);
}

#[test]
fn plans_paths_per_linter_in_spec_order() {
    let config = parse_arclint(
        r#"{"exclude": "generated/",
          "linters": {
            "all-text": {"type": "text"},
            "just-php": {"type": "php", "include": ".php"}}}"#,
    )
    .unwrap();
    let paths = ["a.php", "generated/b.php", "c.txt"]
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    let plans = plan_for_paths(&config, &paths, contains_matcher);
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].display, "TXT");
    assert_eq!(
        plans[0].paths,
        vec!["a.php".to_string(), "c.txt".to_string()]
    );
    assert_eq!(plans[1].display, "PHP");
    assert_eq!(plans[1].paths, vec!["a.php".to_string()]);
}

#[test]
fn retain_selected_matches_display_names_exactly() {
    struct Fake {
        name: &'static str,
    }
    let config = parse_arclint(
        r#"{"linters": {"a": {"type": "text"}, "b": {"type": "text"}, "c": {"type": "json"}}}"#,
    )
    .unwrap();
    assert_eq!(selected_display_names(&config), vec!["TXT", "JSON"]);
    let mut linters = vec![
        Fake { name: "TXT" },
        Fake { name: "txt" },
        Fake { name: "JSON" },
        Fake { name: "XML" },
    ];
    retain_selected(&mut linters, |linter| linter.name, &config);
    let names: Vec<&str> = linters.iter().map(|linter| linter.name).collect();
    assert_eq!(names, vec!["TXT", "JSON"]);
}
