use super::*;

#[test]
fn name_and_binary_match_php() {
    assert_eq!(Pep8Linter::new().name(), "PEP8");
    assert_eq!(Pep8Linter.name(), "PEP8");
    assert_eq!(BINARY, "pep8");
    assert!(mandatory_flags().is_empty());
}

#[test]
fn supported_paths_are_python_files() {
    for path in ["a.py", "dir/b.pyw", "C:\\proj\\c.PY"] {
        assert!(is_supported_path(path), "for {path}");
    }
    for path in ["a.pyx", "a.pyc", "a.txt", "a.rb", "py", ""] {
        assert!(!is_supported_path(path), "for {path}");
    }
}

#[test]
fn parses_canonical_output_with_severities() {
    let stdout = "target.py:2:5: E225 missing whitespace around operator\n\
        target.py:4:80: W291 trailing whitespace\n";
    let messages = parse_output("pkg/mod.py", stdout);
    assert_eq!(messages.len(), 2);

    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (2, 5));
    assert_eq!(messages[0].code, "E225");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "PEP8 E225");
    assert_eq!(
        messages[0].description,
        "missing whitespace around operator"
    );

    assert_eq!((messages[1].line, messages[1].col), (4, 80));
    assert_eq!(messages[1].code, "W291");
    assert_eq!(messages[1].severity, "warning");
}

#[test]
fn skips_garbage_and_trims_fields() {
    let stdout = "bogus\n\ntarget.py:1:1: E000   padded   \n";
    let messages = parse_output("a.py", stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "padded");
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = Pep8Linter::new();
    assert!(linter.lint_bytes("a.txt", b"x\n").is_empty());
    assert!(linter.lint_bytes("a.py", b"a\0b\n").is_empty());
}
