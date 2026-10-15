use super::*;

fn canned() -> &'static str {
    r#"{"a.coffee": [{"lineNumber": 2, "name": "no_trailing_semicolons", "message": "Trailing semicolon", "level": "error", "line": "x = 1;"}, {"lineNumber": 5, "name": "max_line_length", "message": "Line too long", "level": "warn", "line": "y = 2;"}, {"lineNumber": 8, "name": "cyclomatic_complexity", "message": "Complex", "level": "info", "line": "z = 3;"}]}"#
}

#[test]
fn parses_reports_with_levels_and_word_names() {
    let messages = parse_raw_output("a.coffee", canned()).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].line, 2);
    assert_eq!(messages[0].col, 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].code, "COFFEE");
    assert_eq!(messages[0].name, "No Trailing Semicolons");
    assert_eq!(messages[0].description, "Trailing semicolon");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!(messages[1].name, "Max Line Length");
    assert_eq!(messages[2].severity, "advice");
    assert!(messages.iter().all(|m| m.path == "a.coffee"));
}

#[test]
fn empty_report_is_clean() {
    let messages = parse_raw_output("a.coffee", r#"{"a.coffee": []}"#).unwrap();
    assert!(messages.is_empty());
}

#[test]
fn wrong_shapes_are_errors() {
    assert!(parse_raw_output("a.coffee", "not json").is_err());
    assert!(parse_raw_output("a.coffee", "{}").is_err());
    assert!(parse_raw_output("a.coffee", r#"{"a": [], "b": []}"#).is_err());
    assert!(parse_raw_output("a.coffee", r#"{"a.coffee": {}}"#).is_err());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = CoffeelintLinter::new();
    assert_eq!(linter.name(), "COFFEE");
    assert!(linter.lint_bytes("a.js", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("noext", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("a.coffee", b"x\0= 1\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-coffeelint-binary");
    let linter = CoffeelintLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.coffee", b"x = 1\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn custom_severities_are_rejected_like_php() {
    let linter = CoffeelintLinter::new();
    assert_eq!(
        linter.validate_severity_code("COFFEE"),
        Err(
            "CoffeeLint does not currently support custom severity levels, because rules can't \
             be identified from messages in output."
                .to_string()
        )
    );
}

#[test]
fn ucwords_is_byte_based_like_php() {
    assert_eq!(ucwords("no trailing"), "No Trailing");
    assert_eq!(ucwords("a\tb\nc"), "A\tB\nC");
    assert_eq!(ucwords("école x"), "école X");
}

#[test]
fn reports_carry_the_source_line_as_original() {
    let messages = parse_raw_output("a.coffee", canned()).unwrap();
    assert_eq!(messages[0].original.as_deref(), Some("x = 1;"));
    let missing = parse_raw_output(
        "a.coffee",
        r#"{"a.coffee": [{"lineNumber": 1, "name": "x", "message": "m", "level": "warn"}]}"#,
    )
    .unwrap();
    assert_eq!(missing[0].original, None);
}
