use super::*;

#[test]
fn name_and_binary_match_php() {
    assert_eq!(PyFlakesLinter::new().name(), "PYFLAKES");
    assert_eq!(PyFlakesLinter.name(), "PYFLAKES");
    assert_eq!(BINARY, "pyflakes");
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
fn parses_canonical_output() {
    let stdout = "target.py:1:1 undefined name 'foo'\n\
        target.py:2:5 'os' imported but unused\n";
    let messages = parse_output("pkg/mod.py", stdout);
    assert_eq!(messages.len(), 2);

    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert_eq!(messages[0].code, "PYFLAKES");
    assert_eq!(messages[0].name, "PYFLAKES");
    assert_eq!(messages[0].description, "undefined name 'foo'");
    assert_eq!(messages[0].severity, "error");

    assert_eq!((messages[1].line, messages[1].col), (2, 5));
    assert_eq!(messages[1].severity, "warning");
}

#[test]
fn error_triggers_mirror_php_regex() {
    // /(^undefined|^duplicate|before assignment$)/
    let stdout = "t.py:1:1 undefined name 'x'\n\
        t.py:2:1 duplicate argument 'a' in function definition\n\
        t.py:3:7 local variable 'x' referenced before assignment\n\
        t.py:4:1 redefinition of unused 'y'\n";
    let messages = parse_output("a.py", stdout);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[1].severity, "error");
    assert_eq!(messages[2].severity, "error");
    assert_eq!(messages[3].severity, "warning");
}

#[test]
fn missing_column_defaults_to_one() {
    let messages = parse_output("a.py", "target.py:3: something odd\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert_eq!(messages[0].severity, "warning");
}

#[test]
fn skips_garbage_lines() {
    let messages = parse_output("a.py", "garbage\n\ntarget.py:xx:yy nope\n");
    assert!(messages.is_empty());
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = PyFlakesLinter::new();
    assert!(linter.lint_bytes("a.txt", b"x\n").is_empty());
    assert!(linter.lint_bytes("a.py", b"a\0b\n").is_empty());
}
