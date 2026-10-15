use super::*;

fn canned() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<lint>
<file name="a.css">
<issue line="1" char="5" severity="error" reason="Expected RBRACE &amp; more." evidence=".a { color: red; "/>
<issue line="2" char="1" severity="warning" reason="Rule is empty." evidence=""/>
</file>
</lint>
"#
}

#[test]
fn parses_issue_elements_with_severities() {
    let messages = parse_lint_xml_output("a.css", canned()).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (1, 5));
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].code, "CSSLint");
    assert_eq!(messages[0].name, "CSSLint");
    assert_eq!(messages[0].description, "Expected RBRACE & more.");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!((messages[1].line, messages[1].col), (2, 1));
    assert!(messages.iter().all(|m| m.path == "a.css"));
}

#[test]
fn empty_report_is_clean() {
    let clean = r#"<?xml version="1.0"?><lint><file name="a.css"></file></lint>"#;
    assert!(parse_lint_xml_output("a.css", clean).unwrap().is_empty());
}

#[test]
fn non_xml_output_is_an_error() {
    assert!(parse_lint_xml_output("a.css", "csslint exploded").is_err());
    assert!(parse_lint_xml_output("a.css", "").is_err());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = CsslintLinter::new();
    assert_eq!(linter.name(), "CSSLint");
    assert!(
        linter
            .lint_bytes("a.js", b".a { color: red; }\n")
            .is_empty()
    );
    assert!(
        linter
            .lint_bytes("noext", b".a { color: red; }\n")
            .is_empty()
    );
    assert!(linter.lint_bytes("a.css", b".a\0 {}\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-csslint-binary");
    let linter = CsslintLinter::with_binary(&missing);
    assert!(
        linter
            .lint_bytes("a.css", b".a { color: red; }\n")
            .is_empty()
    );
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn custom_severities_are_rejected_like_php() {
    let linter = CsslintLinter::new();
    assert_eq!(
        linter.validate_severity_code("CSSLint"),
        Err(
            "CSSLint does not currently support custom severity levels, because rules can't be \
             identified from messages in output."
                .to_string()
        )
    );
}

#[test]
fn original_text_is_evidence_sliced_at_char() {
    let messages = parse_lint_xml_output("a.css", canned()).unwrap();
    assert_eq!(messages[0].original.as_deref(), Some(" color: red; "));
    assert_eq!(messages[1].original.as_deref(), Some(""));
    let no_char = parse_lint_xml_output(
        "a.css",
        "<lint><file name=\"a.css\">\
         <issue line=\"1\" severity=\"error\" reason=\"r\" evidence=\"full\"/>\
         </file></lint>",
    )
    .unwrap();
    assert_eq!(no_char[0].original.as_deref(), Some("full"));
}

#[test]
fn php_int_cast_and_substr_match_edge_cases() {
    assert_eq!(php_int(""), 0);
    assert_eq!(php_int("abc"), 0);
    assert_eq!(php_int("  -12abc"), -12);
    assert_eq!(php_int("+7"), 7);
    assert_eq!(php_substr_from("hello", 1), "ello");
    assert_eq!(php_substr_from("hello", -2), "lo");
    assert_eq!(php_substr_from("hello", 9), "");
    assert_eq!(php_substr_from("hello", -9), "");
}
