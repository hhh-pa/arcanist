use super::*;

#[test]
fn name_and_binary_match_php() {
    assert_eq!(Flake8Linter::new().name(), "flake8");
    assert_eq!(Flake8Linter.name(), "flake8");
    assert_eq!(BINARY, "flake8");
    assert!(mandatory_flags().is_empty());
}

#[test]
fn supported_paths_are_python_files() {
    for path in ["a.py", "dir/b.pyw", "C:\\proj\\c.PY", "src/d.PyW", ".py"] {
        assert!(is_supported_path(path), "for {path}");
    }
    for path in [
        "a.pyx",
        "a.pyc",
        "a.pyi",
        "a.txt",
        "a.rb",
        "py",
        "",
        "a.py/b.txt",
    ] {
        assert!(!is_supported_path(path), "for {path}");
    }
}

#[test]
fn parses_canonical_output_with_severities() {
    let stdout = "target.py:3:1: E302 expected 2 blank lines, found 1\n\
        target.py:5:12: W291 trailing whitespace\n\
        target.py:9: C901 'f' is too complex (12)\n";
    let messages = parse_output("src/main.py", stdout);
    assert_eq!(messages.len(), 3);

    assert_eq!(messages[0].path, "src/main.py");
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert_eq!(messages[0].code, "E302");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "flake8 E302");
    assert_eq!(messages[0].description, "expected 2 blank lines, found 1");

    assert_eq!((messages[1].line, messages[1].col), (5, 12));
    assert_eq!(messages[1].code, "W291");
    assert_eq!(messages[1].severity, "warning");

    // No column in the tool output: PHP leaves char unset, we use 1.
    assert_eq!((messages[2].line, messages[2].col), (9, 1));
    assert_eq!(messages[2].code, "C901");
    assert_eq!(messages[2].severity, "advice");
}

#[test]
fn extension_codes_are_errors() {
    let messages = parse_output("a.py", "target.py:1:1: F821 undefined name 'x'\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "flake8 F821");
}

#[test]
fn skips_garbage_and_trims_fields() {
    let stdout = "not a finding\n\n   \ntarget.py:1:1: E000    spaced out   \nno-colon line\n";
    let messages = parse_output("a.py", stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "spaced out");
}

#[test]
fn tolerates_colons_in_path() {
    // The PHP path group is lazy: the first boundary giving a full
    // match wins, so drive-letter-style prefixes still parse.
    let messages = parse_output("a.py", "C:\\t.py:2:4: E225 missing whitespace\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 4));
    assert_eq!(messages[0].code, "E225");
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = Flake8Linter::new();
    assert!(linter.lint_bytes("a.txt", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("a.rb", b"puts 1\n").is_empty());
    assert!(linter.lint_bytes("a.py", b"a\0b\n").is_empty());
    assert!(linter.lint_bytes("Makefile", b"all:\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-flake8-lookup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(binary_in(dir.as_os_str()).is_none());
    let marker = dir.join(BINARY);
    std::fs::write(&marker, b"x").unwrap();
    assert_eq!(binary_in(dir.as_os_str()), Some(marker.clone()));
    // Second-directory hit after an empty first segment.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let both = std::env::join_paths([&empty, &dir]).unwrap();
    assert_eq!(binary_in(&both), Some(marker.clone()));
    // Explicit paths bypass the search.
    assert!(binary_on_path(dir.join("nope").to_str().unwrap()).is_none());
    assert!(binary_on_path(marker.to_str().unwrap()).is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(windows)]
fn binary_lookup_probes_pathext_on_windows() {
    let dir = std::env::temp_dir().join(format!("arc-rs-flake8-pathext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exts = std::env::var_os("PATHEXT")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".BAT".to_string());
    let ext = exts
        .split(';')
        .map(str::trim)
        .find(|e| !e.is_empty())
        .unwrap_or(".BAT");
    std::fs::write(dir.join(format!("{BINARY}{ext}")), b"x").unwrap();
    let found = binary_in(dir.as_os_str()).unwrap();
    assert!(
        found
            .to_string_lossy()
            .to_lowercase()
            .ends_with(&ext.to_lowercase())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn skips_lines_with_broken_columns() {
    assert!(parse_output("a.py", "f.py:3:5 E000 x\n").is_empty());
    assert!(parse_output("a.py", "f.py:1:1: \n").is_empty());
    assert!(parse_output("a.py", "\n").is_empty());
}
