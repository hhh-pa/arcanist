use super::*;

#[test]
fn parses_lint_lines_with_mapped_names() {
    let stderr = "NameError: variable @color is undefined in a.less on line 2, column 12:\n\
        SyntaxError: unrecognised input in a.less on line 5, column 3:\n\
        5 .a { color: @color; }\n";
    let messages = parse_lint_output("a.less", stderr);
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (2, 12));
    assert_eq!(messages[0].code, "LESSCNameError");
    assert_eq!(messages[0].name, "Name Error");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "Variable @color is undefined");
    assert_eq!(messages[1].code, "LESSCSyntaxError");
    assert_eq!(messages[1].name, "Syntax Error");
    assert!(messages.iter().all(|m| m.path == "a.less"));
}

#[test]
fn unknown_error_names_become_clear_errors() {
    let messages = parse_lint_output(
        "a.less",
        "FutureError: something new in a.less on line 1, column 1:\n",
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert!(messages[0].description.contains("FutureError"));
}

#[test]
fn empty_output_is_clean() {
    assert!(parse_lint_output("a.less", "").is_empty());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = LesscLinter::new();
    assert_eq!(linter.name(), "LESSC");
    assert!(linter.lint_bytes("a.css", b"@x: 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"@x: 1;\n").is_empty());
    assert!(linter.lint_bytes("a.less", b"@x\0: 1;\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-lessc-binary");
    let linter = LesscLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.less", b"@x: 1;\n").is_empty());
}
