use super::*;

fn canned() -> &'static str {
    r#"[{"file":"a.js","line":3,"col":9,"reason":"Missing semicolon.","code":"W032"},{"file":"a.js","line":7,"col":1,"reason":"Too many errors.","code":"E043"},{"file":"a.js","line":10,"col":5,"reason":"Expected an assignment.","code":"E030"}]"#
}

#[test]
fn parses_report_items_with_severities() {
    let messages = parse_reporter_output("a.js", canned()).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].code, "W032");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].name, "JSHintW032");
    assert_eq!((messages[0].line, messages[0].col), (3, 9));
    assert_eq!(messages[0].description, "Missing semicolon.");
    assert_eq!(messages[1].severity, "disabled");
    assert_eq!(messages[1].name, "JSHintE043");
    assert_eq!(messages[2].severity, "error");
    assert!(messages.iter().all(|m| m.path == "a.js"));
}

#[test]
fn empty_report_is_clean() {
    assert!(parse_reporter_output("a.js", "[]").unwrap().is_empty());
}

#[test]
fn malformed_report_is_an_error() {
    assert!(parse_reporter_output("a.js", "not json").is_err());
    assert!(parse_reporter_output("a.js", r#"{"oops":true}"#).is_err());
    assert!(parse_reporter_output("a.js", "").is_err());
}

#[test]
fn missing_fields_fall_back_to_line_one() {
    let messages = parse_reporter_output("a.js", r#"[{}]"#).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = JshintLinter::new();
    assert_eq!(linter.name(), "JSHint");
    assert!(linter.lint_bytes("a.css", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("a.js", b"var x = 1;\0\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-jshint-binary");
    let linter = JshintLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn severity_codes_are_raw_like_php() {
    let linter = JshintLinter::new();
    for code in ["E033", "W093"] {
        assert_eq!(linter.validate_severity_code(code), Ok(code.to_string()));
    }
    for code in ["", "E", "C901", "[E033]", "e033"] {
        assert_eq!(
            linter.validate_severity_code(code),
            Err(format!(
                "Unrecognized lint message code \"{code}\". Expected a valid JSHint lint code \
                 like \"E033\" or \"W093\"."
            )),
            "{code:?}"
        );
    }
}
