use super::*;

#[test]
fn parses_finding_lines_and_skips_headers() {
    let stdout = "----- FILE  :  a.js -----\n\
        Line 3, E:0001: Missing semicolon at end of line\n\
        Line 7, E:0213: Extra semicolon, E:9999: nested\n\
        Found 2 errors in 1 files\n";
    let messages = parse_gjslint_output("a.js", stdout);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].line, 3);
    assert_eq!(messages[0].col, 1);
    assert_eq!(messages[0].code, "GJSLINT0001");
    assert_eq!(messages[0].name, "GJSLINT0001");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "Missing semicolon at end of line");
    assert_eq!(messages[1].code, "GJSLINT0213");
    assert_eq!(messages[1].description, "Extra semicolon, E:9999: nested");
    assert!(messages.iter().all(|m| m.path == "a.js"));
}

#[test]
fn empty_output_is_clean() {
    assert!(parse_gjslint_output("a.js", "").is_empty());
    assert!(parse_gjslint_output("a.js", "Found 0 errors\n").is_empty());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = ClosureLinter::new();
    assert_eq!(linter.name(), "GJSLINT");
    assert!(linter.lint_bytes("a.ts", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("a.js", b"var\0x;\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-gjslint-binary");
    let linter = ClosureLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}
