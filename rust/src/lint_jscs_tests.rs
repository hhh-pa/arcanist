use super::*;

fn canned() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<checkstyle version="1.0">
<file name="a.js">
<error line="2" column="1" severity="error" message="Missing semicolon" source="requireSemicolons"/>
<error line="4" column="12" severity="warning" message="Line too long" source="maximumLineLength"/>
<error line="6" column="3" severity="info" message="Stray &quot;quote&quot; &amp; ampersand" source="misc"/>
</file>
</checkstyle>
"#
}

#[test]
fn parses_error_elements_with_severities() {
    let messages = parse_checkstyle_output("a.js", canned()).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].code, "JSCS");
    assert_eq!(messages[0].name, "JSCS");
    assert_eq!(messages[0].description, "Missing semicolon");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!((messages[1].line, messages[1].col), (4, 12));
    assert_eq!(messages[2].severity, "advice");
    assert_eq!(messages[2].description, "Stray \"quote\" & ampersand");
    assert!(messages.iter().all(|m| m.path == "a.js"));
}

#[test]
fn empty_report_is_clean() {
    let clean =
        r#"<?xml version="1.0"?><checkstyle version="1.0"><file name="a.js"></file></checkstyle>"#;
    assert!(parse_checkstyle_output("a.js", clean).unwrap().is_empty());
}

#[test]
fn non_xml_output_is_an_error() {
    assert!(parse_checkstyle_output("a.js", "jscs exploded").is_err());
    assert!(parse_checkstyle_output("a.js", "").is_err());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = JscsLinter::new();
    assert_eq!(linter.name(), "JSCS");
    assert!(linter.lint_bytes("a.json", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("a.js", b"var x\0;\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-jscs-binary");
    let linter = JscsLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}
