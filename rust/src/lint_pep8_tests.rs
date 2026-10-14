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

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-pep8-lookup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(binary_in(dir.as_os_str()).is_none());
    let marker = dir.join(BINARY);
    std::fs::write(&marker, b"x").unwrap();
    assert_eq!(binary_in(dir.as_os_str()), Some(marker.clone()));
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let both = std::env::join_paths([&empty, &dir]).unwrap();
    assert_eq!(binary_in(&both), Some(marker.clone()));
    assert!(binary_on_path(dir.join("nope").to_str().unwrap()).is_none());
    assert!(binary_on_path(marker.to_str().unwrap()).is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(windows)]
fn binary_lookup_probes_pathext_on_windows() {
    let dir = std::env::temp_dir().join(format!("arc-rs-pep8-pathext-{}", std::process::id()));
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
    assert!(parse_output("a.py", "f.py:3:5 E225 x\n").is_empty());
    assert!(parse_output("a.py", "f.py:1:1: \n").is_empty());
    assert!(parse_output("a.py", "\n").is_empty());
}
