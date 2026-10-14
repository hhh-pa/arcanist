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

#[test]
fn severity_defaults_to_advice() {
    let ParseOutcome::Args(args) = parse_args(&[]) else {
        panic!("expected args");
    };
    assert_eq!(args.severity, "advice");
    assert_eq!(DEFAULT_SEVERITY, "advice");
}

#[test]
fn parses_severity_forms() {
    let argv = vec!["--severity".to_string(), "error".to_string()];
    let ParseOutcome::Args(args) = parse_args(&argv) else {
        panic!("expected args");
    };
    assert_eq!(args.severity, "error");

    let argv = vec!["--severity=warning".to_string()];
    let ParseOutcome::Args(args) = parse_args(&argv) else {
        panic!("expected args");
    };
    assert_eq!(args.severity, "warning");
}

#[test]
fn rejects_missing_and_unknown_severity() {
    let argv = vec!["--severity".to_string()];
    let ParseOutcome::Error(message) = parse_args(&argv) else {
        panic!("expected error");
    };
    assert!(message.contains("--severity requires a value"), "{message}");

    for argv in [
        vec!["--severity".to_string(), "bogus".to_string()],
        vec!["--severity=bogus".to_string()],
        vec!["--severity=".to_string()],
    ] {
        let ParseOutcome::Error(message) = parse_args(&argv) else {
            panic!("expected error for {argv:?}");
        };
        assert!(message.contains("unknown severity '"), "{message}");
    }
}

#[test]
fn severity_filter_uses_php_ranks() {
    // disabled 10 < advice 20 < autofix 25 < warning 30 < error 40.
    assert!(severity_allows("error", "advice"));
    assert!(severity_allows("error", "error"));
    assert!(severity_allows("warning", "warning"));
    assert!(severity_allows("autofix", "advice"));
    assert!(severity_allows("error", "disabled"));
    assert!(!severity_allows("autofix", "error"));
    assert!(!severity_allows("warning", "error"));
    assert!(!severity_allows("advice", "warning"));
    assert!(!severity_allows("disabled", "advice"));
    // PHP passes unknown message severities and unknown minimums.
    assert!(severity_allows("bogus", "error"));
    assert!(severity_allows("error", "bogus"));
}

#[test]
fn filter_by_severity_drops_below_minimum() {
    let messages = TextLinter::new().lint_bytes("f.txt", b"a\tb\nbad   \n");
    assert_eq!(messages.len(), 2);
    let kept = filter_by_severity(messages.clone(), "error");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].code, "TXT2");
    let kept = filter_by_severity(messages, DEFAULT_SEVERITY);
    assert_eq!(kept.len(), 2);
}

fn unit_temp_dir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let root = Path::new("target/lint-unit-tmp");
    std::fs::create_dir_all(root).unwrap();
    let path = root.join(format!("lint-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Manual upward scan, an independent oracle for [`discover_arclint`].
fn manual_scan(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        let candidate = dir.join(".arclint");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

#[test]
fn discovery_prefers_nearest_arclint() {
    let root = unit_temp_dir("discover");
    let child = root.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(root.join(".arclint"), "{}\n").unwrap();
    std::fs::write(child.join(".arclint"), "{}\n").unwrap();

    assert_eq!(
        discover_arclint(&child),
        Some(child.join(".arclint")),
        "nearest config wins"
    );
    assert_eq!(
        discover_arclint(&root.join("a")),
        Some(root.join(".arclint")),
        "parent config found from a child without one"
    );
    // A file start resolves from its parent directory.
    let file = child.join("f.txt");
    std::fs::write(&file, b"ok\n").unwrap();
    assert_eq!(discover_arclint(&file), Some(child.join(".arclint")));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn discovery_matches_manual_scan() {
    // No assumption about the checkout above the temp dir: whatever the
    // manual walk finds, discovery must find.
    let root = unit_temp_dir("scan");
    let child = root.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    assert_eq!(discover_arclint(&child), manual_scan(&child));
    std::fs::remove_dir_all(&root).ok();
}

fn text_only_config() -> crate::arclint::ArclintConfig {
    crate::arclint::parse_arclint(r#"{"linters": {"text": {"type": "text"}}}"#).unwrap()
}

#[test]
fn selection_runs_only_configured_linters() {
    let selected = Engine::from_config(&text_only_config());
    // Invalid JSON with clean text: the JSON linter would flag it, but the
    // text-only engine stays silent.
    assert!(selected.lint_bytes("bad.json", b"not json\n").is_empty());
    assert!(
        Engine::new()
            .lint_bytes("bad.json", b"not json\n")
            .iter()
            .any(|m| m.code == "JSON1")
    );
    // The selected linter still runs.
    assert!(
        selected
            .lint_bytes("f.txt", b"bad   \n")
            .iter()
            .any(|m| m.code == "TXT6")
    );
}

#[test]
fn selection_requires_exact_php_type_names() {
    let error = crate::arclint::parse_arclint(r#"{"linters": {"t": {"type": "TEXT"}}}"#)
        .expect_err("wrong-case type must be rejected");
    assert!(error.contains("invalid type 'TEXT'"), "{error}");
}

#[test]
fn stop_markers_need_their_linter_selected() {
    let data = b"@nolint\nbad   \n";
    assert!(Engine::new().lint_bytes("f.txt", data).is_empty());
    // Without NOLINT selected, the marker does not stop other linters.
    assert!(
        Engine::from_config(&text_only_config())
            .lint_bytes("f.txt", data)
            .iter()
            .any(|m| m.code == "TXT6")
    );
    let data = b"@generated\nbad   \n";
    assert!(Engine::new().lint_bytes("f.txt", data).is_empty());
    assert!(
        Engine::from_config(&text_only_config())
            .lint_bytes("f.txt", data)
            .iter()
            .any(|m| m.code == "TXT6")
    );
}

#[test]
fn engine_for_dir_reports_config_errors() {
    let root = unit_temp_dir("config-errors");
    std::fs::write(root.join(".arclint"), "not json\n").unwrap();
    let error = engine_for_dir(&root).err().expect("invalid JSON must fail");
    assert!(error.contains("valid JSON"), "{error}");

    std::fs::write(
        root.join(".arclint"),
        r#"{"linters": {"bogus": {"type": "nope"}}}"#,
    )
    .unwrap();
    let error = engine_for_dir(&root).err().expect("unknown type must fail");
    assert!(error.contains("invalid type 'nope'"), "{error}");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn engine_for_dir_falls_back_without_config() {
    // A fresh dir under the system temp area has no config above it in
    // practice; the manual scan keeps the test honest either way.
    let dir = std::env::temp_dir().join(format!("arc-rs-lint-nocfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    if manual_scan(&dir).is_none() {
        let (engine, config) = engine_for_dir(&dir).unwrap();
        assert_eq!(config, None);
        assert!(
            engine
                .lint_bytes("bad.json", b"not json\n")
                .iter()
                .any(|m| m.code == "JSON1"),
            "no config means every ported linter runs"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// ETXTBSY retry: hold a script open for writing, release it mid-retry,
/// and the spawn must succeed. Unix-only (Windows has no ETXTBSY).
#[cfg(unix)]
#[test]
fn spawn_retries_while_script_is_busy() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-lint-busy-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let sh = dir.join("busy.sh");
    std::fs::write(&sh, "#!/bin/sh\necho hi\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&sh).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sh, perms).unwrap();
    }
    let held = std::fs::OpenOptions::new().write(true).open(&sh).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(held);
    });
    let mut command = std::process::Command::new(&sh);
    let out = spawn_with_retry(&mut command).unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    releaser.join().unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/// A permanently busy script still fails (after the bound).
#[cfg(unix)]
#[test]
fn spawn_gives_up_on_permanently_busy_script() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-lint-stuck-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let sh = dir.join("stuck.sh");
    std::fs::write(&sh, "#!/bin/sh\necho hi\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&sh).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sh, perms).unwrap();
    }
    let _held = std::fs::OpenOptions::new().write(true).open(&sh).unwrap();
    let mut command = std::process::Command::new(&sh);
    assert!(spawn_with_retry(&mut command).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
