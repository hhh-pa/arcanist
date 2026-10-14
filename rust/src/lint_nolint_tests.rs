use super::*;

#[test]
fn linter_name_is_nolint() {
    assert_eq!(NoLintLinter::new().name(), "NOLINT");
}

#[test]
fn detects_nolint_marker() {
    assert!(is_nolint(b"// @nolint\n"));
    assert!(is_nolint(b"/* @nolint reason */"));
    assert!(is_nolint(b"@nolint"));
    // Case-sensitive, mirroring the PHP regex; must not confuse the
    // generated marker.
    assert!(!is_nolint(b"@Nolint"));
    assert!(!is_nolint(b"@generated"));
    assert!(!is_nolint(b"nolint without the at-sign"));
    assert!(!is_nolint(b""));
}

#[test]
fn linter_is_always_silent() {
    let linter = NoLintLinter::new();
    assert!(linter.lint_bytes("f.txt", b"plain").is_empty());
    assert!(linter.lint_bytes("f.txt", b"// @nolint\n").is_empty());
}
