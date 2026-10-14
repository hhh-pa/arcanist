use super::*;

fn lint_text(data: &str) -> Vec<LintMessage> {
    TextLinter::new().lint_bytes("file.txt", data.as_bytes())
}

#[test]
fn reports_trailing_whitespace_per_line() {
    let messages = lint_text("The quick brown fox \njumps over the lazy dog. \n");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "TXT6");
    assert_eq!(messages[0].severity, "autofix");
    assert_eq!((messages[0].line, messages[0].col), (1, 20));
    assert_eq!((messages[1].line, messages[1].col), (2, 25));
}

#[test]
fn reports_every_tab() {
    let messages = lint_text("a\tb\nc\td\n");
    let tabs: Vec<&LintMessage> = messages.iter().filter(|m| m.code == "TXT2").collect();
    assert_eq!(tabs.len(), 2);
    assert_eq!((tabs[0].line, tabs[0].col), (1, 2));
    assert_eq!((tabs[1].line, tabs[1].col), (2, 2));
    assert!(tabs.iter().all(|m| m.severity == "error"));
}

#[test]
fn reports_missing_eof_newline() {
    let messages = lint_text("hello\nworld");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "TXT4");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (2, 6));
}

#[test]
fn accepts_clean_and_empty_files() {
    assert!(lint_text("hello\nworld\n").is_empty());
    assert!(lint_text("").is_empty());
}

#[test]
fn maps_offsets_to_lines() {
    let data = b"ab\ncde\n";
    assert_eq!(line_col_for_offset(data, 0), (1, 1));
    assert_eq!(line_col_for_offset(data, 3), (2, 1));
    assert_eq!(line_col_for_offset(data, 7), (3, 1));
}

#[test]
fn formats_text_and_json() {
    let message = LintMessage {
        path: "a.txt".to_string(),
        line: 3,
        col: 5,
        code: "TXT2".to_string(),
        severity: "error".to_string(),
        name: "Tab Literal".to_string(),
        description: DESC_TAB.to_string(),
    };
    assert_eq!(
        message.to_line(),
        "a.txt:3:5: error: Configure your editor to use spaces for indentation."
    );
    let json = message.to_json();
    assert_eq!(json["path"], "a.txt");
    assert_eq!(json["line"], 3);
    assert_eq!(json["col"], 5);
    assert_eq!(json["char"], 5);
    assert_eq!(json["code"], "TXT2");
    assert_eq!(json["severity"], "error");
}

#[test]
fn parses_paths_and_json_flag() {
    let argv = vec![
        "a.txt".to_string(),
        "--json".to_string(),
        "b.txt".to_string(),
    ];
    let ParseOutcome::Args(args) = parse_args(&argv) else {
        panic!("expected args");
    };
    assert!(args.json);
    assert_eq!(args.paths, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

#[test]
fn rejects_unknown_and_defers_unported_flags() {
    let argv = vec!["--bogus".to_string()];
    assert!(matches!(parse_args(&argv), ParseOutcome::Error(_)));
    for flag in ["--rev", "--apply-patches", "--engine", "--everything"] {
        let argv = vec![flag.to_string()];
        let ParseOutcome::NotYetPorted(message) = parse_args(&argv) else {
            panic!("expected not-yet-ported for {flag}");
        };
        assert!(message.contains("not yet ported"), "{message}");
    }
}
