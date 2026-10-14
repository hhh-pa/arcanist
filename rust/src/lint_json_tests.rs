use super::*;

#[test]
fn linter_name_is_json() {
    assert_eq!(JsonLinter::new().name(), "JSON");
}

#[test]
fn path_filter_matches_dot_json_only() {
    assert!(is_json_path("a.json"));
    assert!(is_json_path("dir/b.JSON"));
    assert!(is_json_path(".json"));
    assert!(!is_json_path("a.json5"));
    assert!(!is_json_path("a.txt"));
    assert!(!is_json_path("json"));
    assert!(!is_json_path("a.json.bak"));
    assert!(!is_json_path(""));
}

#[test]
fn valid_documents_pass() {
    let linter = JsonLinter::new();
    for data in [
        b"{}".as_slice(),
        b"[]",
        b"{\"a\": [1, 2.5, -3e2, true, false, null]}",
        b"\"str\\u00e9\"",
        b"42",
        b"  { \"a\" : 1 }  \n",
    ] {
        assert!(linter.lint_bytes("f.json", data).is_empty(), "for {data:?}");
        assert!(parse_error(data).is_none(), "for {data:?}");
    }
}

#[test]
fn syntax_errors_report_positions() {
    // Trailing comma: serde_json points at the closing brace.
    let (line, col, message) = parse_error(b"{\"a\": 1,}").unwrap();
    assert_eq!((line, col), (1, 9));
    assert!(!message.contains(" at line "), "message: {message}");

    // Multi-line: error on line 3.
    let (line, _, _) = parse_error(b"{\n\"a\": 1,\n}").unwrap();
    assert_eq!(line, 3);

    // Single quotes are not valid JSON.
    assert!(parse_error(b"{'a': 1}").is_some());
    // Trailing garbage after the document.
    assert!(parse_error(b"{} {}").is_some());
    // Unclosed nesting.
    assert!(parse_error(b"{\"a\": [1, 2}").is_some());
}

#[test]
fn lint_message_shape() {
    let found = JsonLinter::new().lint_bytes("f.json", b"{\"a\": }");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "f.json");
    assert_eq!(found[0].code, "JSON1");
    assert_eq!(found[0].severity, "error");
    assert_eq!(found[0].name, "Parse Error");
    assert_eq!((found[0].line, found[0].col), (1, 7));
    assert!(!found[0].description.is_empty());
}

#[test]
fn empty_document_is_an_error_at_start() {
    // Mirrors PHP: parsing "" throws, so it is reported.
    let (line, col, _) = parse_error(b"").unwrap();
    assert_eq!((line, col), (1, 1));
    assert_eq!(JsonLinter::new().lint_bytes("f.json", b"").len(), 1);
}

#[test]
fn non_json_and_binary_inputs_are_silent() {
    let linter = JsonLinter::new();
    assert!(linter.lint_bytes("f.txt", b"{oops").is_empty());
    assert!(linter.lint_bytes("f.json5", b"{oops").is_empty());
    assert!(linter.lint_bytes("f.json", b"{\x00oops").is_empty());
}
