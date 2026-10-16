use super::*;

#[test]
fn parses_browser_config_values() {
    assert_eq!(
        configured_argv(&serde_json::json!("my-browser")),
        Some(vec!["my-browser".to_string()])
    );
    assert_eq!(
        configured_argv(&serde_json::json!(["my-browser", "--new"])),
        Some(vec!["my-browser".to_string(), "--new".to_string()])
    );
    // Empty and non-string values are falsy upstream: unconfigured.
    // ("0" is falsy in PHP too.)
    for value in [
        serde_json::json!(""),
        serde_json::json!("0"),
        serde_json::json!([]),
        serde_json::json!([42]),
        serde_json::json!(null),
        serde_json::json!(42),
    ] {
        assert_eq!(configured_argv(&value), None, "{value}");
    }
    // Non-string list members are dropped.
    assert_eq!(
        configured_argv(&serde_json::json!(["ok", 42])),
        Some(vec!["ok".to_string()])
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
    // CORE-33: the project file only counts at a working-copy root.
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
