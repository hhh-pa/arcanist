use super::*;

#[test]
fn parses_browser_config_values() {
    // T1153 CORE-63: PHP wraps any truthy non-array `browser` value
    // and runs it (`:2148-2154` at ceb), string-casting scalars.
    assert_eq!(
        configured_argv(&serde_json::json!("my-browser")),
        Some(vec!["my-browser".to_string()])
    );
    assert_eq!(
        configured_argv(&serde_json::json!(["my-browser", "--new"])),
        Some(vec!["my-browser".to_string(), "--new".to_string()])
    );
    // Truthy scalars wrap and cast.
    assert_eq!(
        configured_argv(&serde_json::json!(42)),
        Some(vec!["42".to_string()])
    );
    assert_eq!(
        configured_argv(&serde_json::json!(4.5)),
        Some(vec!["4.5".to_string()])
    );
    assert_eq!(
        configured_argv(&serde_json::json!(true)),
        Some(vec!["1".to_string()])
    );
    // List members cast too.
    assert_eq!(
        configured_argv(&serde_json::json!(["ok", 42])),
        Some(vec!["ok".to_string(), "42".to_string()])
    );
    assert_eq!(
        configured_argv(&serde_json::json!([42])),
        Some(vec!["42".to_string()])
    );
    // Falsy values are unconfigured ("0" is falsy in PHP too).
    for value in [
        serde_json::json!(""),
        serde_json::json!("0"),
        serde_json::json!(0),
        serde_json::json!(0.0),
        serde_json::json!(false),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!(null),
    ] {
        assert_eq!(configured_argv(&value), None, "{value}");
    }
    // Nested structures have no sane spelling and are dropped.
    assert_eq!(
        configured_argv(&serde_json::json!(["ok", ["nested"]])),
        Some(vec!["ok".to_string()])
    );
    assert_eq!(configured_argv(&serde_json::json!([[1]])), None);
    // Falsy members of a truthy list still cast (`false` and `null`
    // spell the empty string, like PHP).
    assert_eq!(
        configured_argv(&serde_json::json!([false])),
        Some(vec![String::new()])
    );
    assert_eq!(
        configured_argv(&serde_json::json!([null])),
        Some(vec![String::new()])
    );
    // A truthy object maps its values in order.
    assert_eq!(
        configured_argv(&serde_json::json!({"a": "x"})),
        Some(vec!["x".to_string()])
    );
}

#[test]
fn reads_browser_from_project_config() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-browser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"browser": ["my-browser", "--new"]}"#,
    )
    .unwrap();
    // Project config wins over any user/system config.
    assert_eq!(
        configured_browser(&dir),
        Some(vec!["my-browser".to_string(), "--new".to_string()])
    );
    std::fs::remove_dir_all(&dir).ok();

    let bare = std::env::temp_dir().join(format!(
        "arc-rs-browser-bare-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bare).unwrap();
    // No config anywhere beneath: only the real user/system config could
    // answer, so just exercise the lookup without asserting its value.
    let _ = configured_browser(&bare);
    std::fs::remove_dir_all(&bare).ok();
}

/// A fake browser command which exits zero without doing anything.
#[cfg(windows)]
fn null_browser() -> Vec<String> {
    vec!["cmd".to_string(), "/c".to_string(), "rem".to_string()]
}

/// A fake browser command which exits zero without doing anything.
#[cfg(not(windows))]
fn null_browser() -> Vec<String> {
    vec!["true".to_string()]
}

#[test]
fn opens_uris_with_a_configured_browser() {
    let browser = null_browser();
    open_uris_in_browser(&["http://example.test/P1".to_string()], Some(&browser)).unwrap();
}

#[test]
#[cfg(unix)]
fn path_scan_finds_the_posix_shell() {
    // `sh` ships on every unix test machine; the scan finds it
    // executable without spawning anything.
    assert!(binary_exists("sh"));
}

#[test]
#[cfg(unix)]
fn reports_browsers_that_exit_nonzero() {
    // `false` spawns fine but exits 1, so the open fails with PHP's
    // error (no network: the URI is just an ignored argument).
    let err = open_uris_in_browser(
        &["http://example.test/P1".to_string()],
        Some(&["false".to_string()]),
    )
    .unwrap_err();
    assert!(
        err.contains("Failed to open URI \"http://example.test/P1\" in browser"),
        "{err}"
    );
    assert!(err.contains("false"), "{err}");
    assert!(
        err.contains("Check your \"browser\" config option."),
        "{err}"
    );
}

#[test]
fn reports_failing_browsers_like_php() {
    let err = open_uris_in_browser(
        &["http://example.test/P1".to_string()],
        Some(&["definitely-not-a-browser-xyz".to_string()]),
    )
    .unwrap_err();
    assert!(
        err.contains("Failed to open URI \"http://example.test/P1\" in browser"),
        "{err}"
    );
    assert!(err.contains("definitely-not-a-browser-xyz"), "{err}");
    assert!(
        err.contains("Check your \"browser\" config option."),
        "{err}"
    );
}

#[test]
#[cfg(not(windows))]
fn missing_binaries_fall_back_to_the_scan_error() {
    // Without configuration and without any candidate binary, the scan
    // fails with the usage error (never reaches process spawn).
    let err = diff::browser_open_argv("http://example.test/", None, &|_| false).unwrap_err();
    assert_eq!(err, diff::NO_BROWSER_MESSAGE);
    assert!(!binary_exists("definitely-not-a-binary-xyz"));
}

#[test]
#[cfg(unix)]
fn browse_b19_unmarked_files_are_not_binaries() {
    // PHP resolves via `which %s`, which only finds executables;
    // `is_file()` alone also matches unmarked files.
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-browser-exec-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let prog = dir.join("prog");
    std::fs::write(&prog, "#!/bin/sh\ntrue\n").unwrap();
    let name = prog.to_str().unwrap().to_string();
    assert!(!binary_exists(&name));
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&prog).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&prog, perms).unwrap();
    }
    assert!(binary_exists(&name));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(not(windows))]
fn r9_browse_b24_no_browser_scan_error_matches_ceb() {
    // `open_uris_in_browser` propagates the `browser_open_argv` scan error
    // unchanged, so this locks the exact bytes the browse (T1153 BROWSE-B24),
    // upload --browse (T1153 UPLOAD-U13), and todo --browse (T1153 RB-D29)
    // paths surface when no browser resolves.
    let err = diff::browser_open_argv("http://example.test/", None, &|_| false).unwrap_err();
    assert_eq!(
        err,
        "Unable to find a browser command to run. Set 'browser' in your Arcanist config to specify a command to use."
    );
}
