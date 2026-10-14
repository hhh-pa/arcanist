use super::*;

#[test]
fn linter_name_is_merge_conflict() {
    assert_eq!(MergeConflictLinter::new().name(), "MERGECONFLICT");
}

#[test]
fn markers_are_flagged_per_line() {
    let linter = MergeConflictLinter::new();
    let data = b"ok\n<<<<<<<\nours\n=======\ntheirs\n>>>>>>>\nok\n";
    let found = linter.lint_bytes("f.txt", data);
    assert_eq!(found.len(), 3);
    for (message, line) in found.iter().zip([2, 4, 6]) {
        assert_eq!(message.path, "f.txt");
        assert_eq!((message.line, message.col), (line, 1));
        assert_eq!(message.code, "MERGECONFLICT1");
        assert_eq!(message.severity, "error");
        assert_eq!(message.name, "Unresolved merge conflict");
        assert_eq!(
            message.description,
            "This syntax indicates there is an unresolved merge conflict."
        );
    }
}

#[test]
fn near_markers_do_not_match() {
    let linter = MergeConflictLinter::new();
    // Six or eight repeats, trailing/leading text, indentation, and interior
    // whitespace all fail the exact `/^(>{7}|<{7}|={7})$/` rule.
    let data = b"<<<<<<\n>>>>>>>>\n========\n<<<<<<< x\nx =======\n  =======\n== == ==\n";
    assert!(linter.lint_bytes("f.txt", data).is_empty());
    assert!(is_marker(b"======="));
    assert!(!is_marker(b"======"));
    assert!(!is_marker(b"========"));
    assert!(!is_marker(b" ======="));
}

#[test]
fn crlf_markers_match() {
    let linter = MergeConflictLinter::new();
    let found = linter.lint_bytes("f.txt", b"ok\r\n=======\r\nok\r\n");
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].line, found[0].col), (2, 1));
}

#[test]
fn marker_on_last_line_without_newline_matches() {
    let linter = MergeConflictLinter::new();
    let found = linter.lint_bytes("f.txt", b"ok\n>>>>>>>");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 2);
}

#[test]
fn binary_data_is_silent() {
    let linter = MergeConflictLinter::new();
    assert!(linter.lint_bytes("f.txt", b"=======\x00").is_empty());
}

#[test]
fn applies_to_every_extension() {
    let linter = MergeConflictLinter::new();
    for path in ["a.rs", "b.json", "c.xml", "Makefile", "no-ext"] {
        assert_eq!(linter.lint_bytes(path, b"<<<<<<<\n").len(), 1, "for {path}");
    }
}
