use super::*;

#[test]
fn name_and_binary_match_php() {
    assert_eq!(PyFlakesLinter::new().name(), "PYFLAKES");
    assert_eq!(PyFlakesLinter::new().name(), "PYFLAKES");
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
    // T1153 LINT-33: ceb PHP has no column group, so the char is always
    // null and a `path:line:col` line reads the column digits as the
    // line number.
    let stdout = "target.py:1:1: undefined name 'foo'\n\
        target.py:2:5: 'os' imported but unused\n";
    let messages = parse_output("pkg/mod.py", stdout);
    assert_eq!(messages.len(), 2);

    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (1, 0));
    assert_eq!(messages[0].code, "PYFLAKES");
    assert_eq!(messages[0].name, "PYFLAKES");
    assert_eq!(messages[0].description, "undefined name 'foo'");
    assert_eq!(messages[0].severity, "error");

    assert_eq!((messages[1].line, messages[1].col), (5, 0));
    assert_eq!(messages[1].severity, "warning");
}

#[test]
fn error_triggers_mirror_php_regex() {
    // /(^undefined|^duplicate|before assignment$)/
    // T1153 LINT-33: canonical `path:line:col: desc` form (colon-less
    // lines match nothing in ceb PHP and are dropped).
    let stdout = "t.py:1:1: undefined name 'x'\n\
        t.py:2:1: duplicate argument 'a' in function definition\n\
        t.py:3:7: local variable 'x' referenced before assignment\n\
        t.py:4:1: redefinition of unused 'y'\n";
    let messages = parse_output("a.py", stdout);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[1].severity, "error");
    assert_eq!(messages[2].severity, "error");
    assert_eq!(messages[3].severity, "warning");
}

#[test]
fn missing_column_stays_null_char() {
    // T1153 LINT-33: no column group in ceb PHP, so the char is null (0).
    let messages = parse_output("a.py", "target.py:3: something odd\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 0));
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

#[test]
fn lint_bytes_stays_silent_without_the_tool() {
    // With no `bin` override the linter consults PATH; `pyflakes` is
    // absent from PATH here, so supported inputs stay silent without
    // spawning anything. (Clean input would lint empty even where the
    // tool is installed.)
    let linter = PyFlakesLinter::new();
    assert!(linter.lint_bytes("a.py", b"x = 1\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-pyflakes-lookup-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("arc-rs-pyflakes-pathext-{}", std::process::id()));
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
    assert!(parse_output("a.py", "f.py:3 \n").is_empty());
    assert!(parse_output("a.py", "f.py:: x\n").is_empty());
    assert!(parse_output("a.py", "\n").is_empty());
}

// -( T1153 Wave 2: lint retrocompat )--------------------------------------

#[test]
fn lint33_column_without_colon_matches_nothing() {
    // `path:line:col desc` (no colon after the column) matches nothing
    // in ceb PHP's `/^(.*?):(\d+): (.*)$/` and is dropped.
    assert!(parse_output("a.py", "f.py:3:5 something odd\n").is_empty());
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let base =
        std::env::temp_dir().join(format!("arc-rs-pyflakes-temp-fail-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let tool = base.join("fake-pyflakes");
    std::fs::write(&tool, b"x").unwrap();
    let locked = base.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut linter = PyFlakesLinter {
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    linter.options.bin = Some(tool);
    assert!(linter.lint_bytes("a.py", b"x = 1").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&base).ok();
}
