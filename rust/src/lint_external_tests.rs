use super::*;
use std::path::{Path, PathBuf};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-external-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn canned_parser(
    _path: &str,
    _exit_code: i32,
    stdout: &str,
    _stderr: &str,
) -> Option<Vec<LintMessage>> {
    if stdout == "unparseable" {
        return None;
    }
    if stdout.is_empty() {
        return Some(Vec::new());
    }
    Some(vec![LintMessage {
        path: "file.txt".to_string(),
        line: 1,
        col: 1,
        code: "EXT1".to_string(),
        severity: "error".to_string(),
        name: "External".to_string(),
        description: stdout.to_string(),
        original: None,
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    }])
}

#[test]
fn default_runner_stays_silent() {
    let linter = ExternalLinter::new();
    assert_eq!(linter.name(), "external");
    assert!(linter.lint_bytes("file.txt", b"anything\n").is_empty());
}

#[test]
fn missing_binary_stays_silent() {
    let config = ExternalToolConfig {
        binaries: vec!["definitely-missing-lint-binary-xyz".to_string()],
        install_instructions: "Install it.".to_string(),
        ..ExternalToolConfig::default()
    };
    let linter = ExternalLinter::with_config(config, canned_parser);
    assert_eq!(linter.config().binaries.len(), 1);
    assert!(linter.lint_bytes("file.txt", b"data\n").is_empty());
    assert!(linter.lint_bytes("file.txt", b"\0").is_empty());
}

#[test]
fn binary_resolution_errors_mirror_php() {
    let config = ExternalToolConfig {
        binaries: vec!["nope-1".to_string(), "nope-2".to_string()],
        install_instructions: "Run the installer.".to_string(),
        ..ExternalToolConfig::default()
    };
    let error = check_binary_configuration(&config).unwrap_err();
    assert!(error.contains("Unable to locate binary"));
    assert!(error.contains("nope-1, nope-2"));
    assert!(error.contains("TO INSTALL: Run the installer."));

    let config = ExternalToolConfig {
        binaries: vec!["whatever".to_string()],
        interpreters: vec!["missing-interpreter-xyz".to_string()],
        ..ExternalToolConfig::default()
    };
    let error = check_binary_configuration(&config).unwrap_err();
    assert!(error.contains("Unable to locate interpreter"));
}

#[test]
fn resolve_output_ports_future_semantics() {
    // Nonzero exit with messages is fine when errors are expected.
    let messages = resolve_output(canned_parser, "f.txt", 1, "oops\n", "", true).unwrap();
    assert_eq!(messages.len(), 1);

    // Nonzero exit without messages means parse failure.
    assert!(resolve_output(canned_parser, "f.txt", 1, "", "", true).is_err());

    // Parser failure on clean exit reports unparseable output.
    let error = resolve_output(canned_parser, "f.txt", 0, "unparseable", "", true).unwrap_err();
    assert!(error.contains("Linter failed to parse output!"));

    // Unexpected command errors fail fast.
    let error = resolve_output(canned_parser, "f.txt", 3, "out", "err", false).unwrap_err();
    assert!(error.contains("exit code 3"));
}

#[test]
fn version_compare_orders_like_php() {
    use std::cmp::Ordering::*;
    let cases = [
        ("1.0", "1.0", Equal),
        ("1.0", "1.1", Less),
        ("1.10", "1.9", Greater),
        ("1.0", "1.0.0", Less),
        ("2.0", "10.0", Less),
        ("1.0-dev", "1.0", Less),
        ("1.0-alpha", "1.0-beta", Less),
        ("1.0-beta", "1.0-RC", Less),
        ("1.0-RC", "1.0", Less),
        ("1.0", "1.0-pl", Less),
        ("1.0a", "1.0b", Less),
    ];
    for (left, right, expected) in cases {
        assert_eq!(version_compare(left, right), expected, "{left} vs {right}");
        assert_eq!(
            version_compare(right, left),
            expected.reverse(),
            "{right} vs {left}"
        );
    }
}

#[test]
fn version_requirements_check_operators() {
    let ok = |version: Option<&str>, requirement: &str| {
        check_version_requirement("L", "tool", version, requirement, None).is_ok()
    };
    assert!(ok(Some("1.5"), ">= 1.2"));
    assert!(ok(Some("1.2"), ">=1.2"));
    assert!(!ok(Some("1.1"), ">= 1.2"));
    assert!(ok(Some("1.0"), "= 1.0"));
    assert!(ok(Some("1.0"), "1.0"));
    assert!(!ok(Some("1.1"), "1.0"));
    assert!(ok(Some("2.0"), "> 1.9"));
    assert!(!ok(Some("1.9"), "> 1.9"));
    assert!(ok(Some("0.9"), "< 1.0"));
    assert!(!ok(Some("1.0"), "< 1.0"));
    assert!(ok(Some("1.0"), "<= 1.0"));
    assert!(ok(None, ""));
    assert!(!ok(None, ">= 1.0"));

    let error = check_version_requirement("L", "tool", Some("1.0"), ">= 2.0", Some("Upgrade."))
        .unwrap_err();
    assert!(error.contains("requires tool version >= 2.0"));
    assert!(error.contains("You have version 1.0"));
    assert!(error.contains("TO UPGRADE: Upgrade."));
}

#[test]
fn temp_file_resolves_as_path_candidate() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-lint-external-{}-resolve",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let tool = dir.join("fake-tool");
    std::fs::write(&tool, b"#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool, permissions).unwrap();
    }
    let candidate = tool.to_string_lossy().to_string();
    assert!(binary_exists(&candidate));
    let config = ExternalToolConfig {
        binaries: vec![candidate],
        ..ExternalToolConfig::default()
    };
    let resolved = check_binary_configuration(&config).unwrap();
    assert!(resolved.interpreter.is_none());
    assert!(!binary_exists("definitely-missing-lint-binary-xyz"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tool_options_reject_mistyped_flags_and_version() {
    let mut options = ToolOptions::default();
    assert_eq!(
        apply_tool_option(
            &mut options,
            "flags",
            &serde_json::json!(["--ok", 42]),
            Path::new(".")
        )
        .unwrap_err(),
        "'flags' must be a list of strings"
    );
    assert_eq!(
        apply_tool_option(
            &mut options,
            "version",
            &serde_json::json!(42),
            Path::new(".")
        )
        .unwrap_err(),
        "'version' must be a string"
    );
    assert_eq!(
        apply_tool_option(
            &mut options,
            "flags",
            &serde_json::json!(["--ok"]),
            Path::new(".")
        ),
        Ok(())
    );
}

#[test]
fn configured_bin_accepts_lists_and_rejects_shapes() {
    let root = TempDir::new("bin");
    assert_eq!(
        resolve_configured_bin(
            &serde_json::json!(["nope-a-xyz", "nope-b-xyz"]),
            root.path()
        )
        .unwrap_err(),
        "None of the configured binaries can be located."
    );
    assert_eq!(
        resolve_configured_bin(&serde_json::json!(["ok", 42]), root.path()).unwrap_err(),
        "'bin' must be a string or a list of strings"
    );
    assert_eq!(
        resolve_configured_bin(&serde_json::json!(42), root.path()).unwrap_err(),
        "'bin' must be a string or a list of strings"
    );
}

#[test]
fn configured_bin_falls_back_to_the_project_root() {
    let root = TempDir::new("binroot");
    let tool = root.path().join("cov5e-tool");
    std::fs::write(&tool, b"#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool, permissions).unwrap();
    }
    // A bare name missing from PATH resolves against the root.
    assert_eq!(
        resolve_configured_bin(&serde_json::json!("cov5e-tool"), root.path()).unwrap(),
        tool
    );
}

#[test]
fn unspawnable_binary_stays_silent() {
    // An executable script naming a missing interpreter passes
    // resolution but fails exec (ENOENT, with no shell fallback),
    // which stays silent like a missing binary.
    let root = TempDir::new("badinterp");
    let tool = root.path().join("fake-tool");
    std::fs::write(&tool, b"#!/definitely/missing/interp-xyz\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool, permissions).unwrap();
    }
    let config = ExternalToolConfig {
        binaries: vec![tool.to_string_lossy().to_string()],
        ..ExternalToolConfig::default()
    };
    let linter = ExternalLinter::with_config(config, canned_parser);
    assert!(linter.lint_bytes("file.txt", b"data\n").is_empty());
}
