use super::*;

#[test]
fn name_binary_and_flags_match_php() {
    assert_eq!(PyLintLinter::new().name(), "PyLint");
    assert_eq!(PyLintLinter.name(), "PyLint");
    assert_eq!(BINARY, "pylint");
    assert_eq!(
        mandatory_flags(),
        vec![
            "--reports=no",
            "--msg-template={line}|{column}|{msg_id}|{symbol}|{msg}",
        ]
    );
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
fn parses_template_output_with_severities() {
    let stdout = "1|0|C0114|missing-module-docstring|Missing module docstring\n\
        2|4|E0602|undefined-variable|Undefined variable 'x'\n\
        3|0|W0611|unused-import|Unused import os\n\
        4|8|R0903|too-few-public-methods|Too few public methods (0/2)\n\
        5|2|F0001|fatal|Some fatal thing\n";
    let messages = parse_output("pkg/mod.py", 1, stdout);
    assert_eq!(messages.len(), 5);

    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (1, 0));
    assert_eq!(messages[0].code, "C0114");
    assert_eq!(messages[0].name, "Missing Module Docstring");
    assert_eq!(messages[0].severity, "advice");

    assert_eq!(messages[1].code, "E0602");
    assert_eq!(messages[1].name, "Undefined Variable");
    assert_eq!(messages[1].severity, "error");

    assert_eq!(messages[2].severity, "warning");
    assert_eq!(messages[3].severity, "advice");
    assert_eq!(messages[4].severity, "error");
}

#[test]
fn negative_column_clamps_to_zero() {
    // T9257: pylint sometimes reports -1 as the column.
    let messages = parse_output("a.py", 1, "3|-1|W0611|unused-import|Unused import os\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 0));
}

#[test]
fn disabled_severity_messages_are_dropped() {
    // Codes outside R/C/W/E/F are SEVERITY_DISABLED in PHP, which the
    // engine filters out.
    let stdout = "1|0|I0020|suppressed-message|Suppressed\n\
        2|0|E0602|undefined-variable|Undefined variable 'x'\n";
    let messages = parse_output("a.py", 1, stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "E0602");
}

#[test]
fn usage_error_exit_yields_no_messages() {
    let messages = parse_output("a.py", 32, "1|0|E0602|undefined-variable|msg\n");
    assert!(messages.is_empty());
}

#[test]
fn skips_short_and_malformed_lines() {
    let messages = parse_output("a.py", 1, "only|two\n\nxx|0|E0602|s|m\n");
    assert!(messages.is_empty());
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = PyLintLinter::new();
    assert!(linter.lint_bytes("a.txt", b"x\n").is_empty());
    assert!(linter.lint_bytes("a.py", b"a\0b\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-pylint-lookup-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("arc-rs-pylint-pathext-{}", std::process::id()));
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
fn skips_lines_with_broken_fields() {
    // PHP `(int)`-casts junk columns to 0 instead of skipping the line.
    let messages = parse_output("a.py", 1, "3|xx|E0602|s|m\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 0));
    assert!(parse_output("a.py", 1, "xx|0|E0602|s|m\n").is_empty());
    assert!(parse_output("a.py", 1, "nocolumns\n").is_empty());
    assert!(parse_output("a.py", 1, "\n").is_empty());
}
