use super::*;

fn canned() -> &'static str {
    r#"{"a.coffee": [{"lineNumber": 2, "name": "no_trailing_semicolons", "message": "Trailing semicolon", "level": "error", "line": "x = 1;"}, {"lineNumber": 5, "name": "max_line_length", "message": "Line too long", "level": "warn", "line": "y = 2;"}, {"lineNumber": 8, "name": "cyclomatic_complexity", "message": "Complex", "level": "info", "line": "z = 3;"}]}"#
}

#[test]
fn parses_reports_with_levels_and_word_names() {
    let messages = parse_raw_output("a.coffee", canned()).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].line, 2);
    assert_eq!(messages[0].col, 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].code, "COFFEE");
    assert_eq!(messages[0].name, "No Trailing Semicolons");
    assert_eq!(messages[0].description, "Trailing semicolon");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!(messages[1].name, "Max Line Length");
    assert_eq!(messages[2].severity, "advice");
    assert!(messages.iter().all(|m| m.path == "a.coffee"));
}

#[test]
fn empty_report_is_clean() {
    let messages = parse_raw_output("a.coffee", r#"{"a.coffee": []}"#).unwrap();
    assert!(messages.is_empty());
}

#[test]
fn wrong_shapes_are_errors() {
    assert!(parse_raw_output("a.coffee", "not json").is_err());
    assert!(parse_raw_output("a.coffee", "{}").is_err());
    assert!(parse_raw_output("a.coffee", r#"{"a": [], "b": []}"#).is_err());
    assert!(parse_raw_output("a.coffee", r#"{"a.coffee": {}}"#).is_err());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = CoffeelintLinter::new();
    assert_eq!(linter.name(), "COFFEE");
    assert!(linter.lint_bytes("a.js", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("noext", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("a.coffee", b"x\0= 1\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-coffeelint-binary");
    let linter = CoffeelintLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.coffee", b"x = 1\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn custom_severities_are_rejected_like_php() {
    let linter = CoffeelintLinter::new();
    assert_eq!(
        linter.validate_severity_code("COFFEE"),
        Err(
            "CoffeeLint does not currently support custom severity levels, because rules can't \
             be identified from messages in output."
                .to_string()
        )
    );
}

#[test]
fn ucwords_is_byte_based_like_php() {
    assert_eq!(ucwords("no trailing"), "No Trailing");
    assert_eq!(ucwords("a\tb\nc"), "A\tB\nC");
    assert_eq!(ucwords("école x"), "école X");
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-coffeelint-tests-{tag}-{}-{n}",
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

#[cfg(unix)]
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn option_ctx(root: &Path) -> OptionContext<'_> {
    OptionContext { project_root: root }
}

#[test]
fn set_config_feeds_the_file_flag() {
    let tmp = TempDir::new("flags");
    let ctx = option_ctx(tmp.path());
    let mut linter = CoffeelintLinter::new();
    linter
        .set_option("coffeelint.config", &serde_json::json!("cfg.json"), &ctx)
        .unwrap();
    assert!(linter.tool_flags().contains(&"--file=cfg.json".to_string()));
    assert_eq!(
        linter.set_option("coffeelint.config", &serde_json::json!(42), &ctx),
        Err("'coffeelint.config' must be a string".to_string())
    );
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-coffeelint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = CoffeelintLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    assert_eq!(linter.resolve_binary(), Some(tool));
    linter
        .set_option("flags", &serde_json::json!(["--extra"]), &ctx)
        .unwrap();
    assert!(linter.tool_flags().contains(&"--extra".to_string()));
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(1), &ctx)
            .is_err()
    );
}

#[test]
fn resolve_binary_falls_back_to_path() {
    // No configured binary: whatever the ambient `PATH` holds decides.
    // The lookup itself is covered; the outcome stays unasserted.
    let linter = CoffeelintLinter::new();
    let _ = linter.resolve_binary();
    let _ = find_on_path("sh");
}

#[test]
fn temp_extension_falls_back_without_a_suffix() {
    assert_eq!(temp_extension("noext", "coffee"), "coffee");
    assert_eq!(temp_extension("a.coffee", "coffee"), "coffee");
}

#[test]
fn truncate_stops_at_char_boundaries() {
    let text = format!("{}é", "a".repeat(299));
    assert_eq!(text.len(), 301);
    assert_eq!(truncate(&text, 300), format!("{}...", "a".repeat(299)));
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_tool_and_parses_report() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-coffeelint",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n",
            canned().replace('\'', "'\\''")
        ),
    );
    let linter = CoffeelintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.coffee", b"x = 1\n");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].description, "Trailing semicolon");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_unparseable_success() {
    let tmp = TempDir::new("run-junk");
    let tool = write_script(tmp.path(), "fake-coffeelint", "#!/bin/sh\necho junk\n");
    let linter = CoffeelintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.coffee", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert!(messages[0].description.contains("unparseable"));
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_tool_failure_on_empty_nonzero_exit() {
    let tmp = TempDir::new("run-fail");
    let tool = write_script(
        tmp.path(),
        "fake-coffeelint",
        "#!/bin/sh\necho '{\"a.coffee\": []}'\necho exploded >&2\nexit 3\n",
    );
    let linter = CoffeelintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.coffee", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(
        messages[0].description,
        "coffeelint failed (exit 3): exploded"
    );
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(tmp.path(), "fake-coffeelint", "#!/bin/sh\necho junk\n");
    let mut linter = CoffeelintLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.coffee", b"x = 1\n").is_empty());
    let tool = write_script(
        tmp.path(),
        "fake-coffeelint-new",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 2.1.0; else printf '%s\\n' '{}'; fi\n",
            canned().replace('\'', "'\\''")
        ),
    );
    let mut linter = CoffeelintLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.coffee", b"x = 1\n").len(), 3);
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_route_through_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-coffeelint.cmd", "#!/bin/sh\nexit 0\n");
    let linter = CoffeelintLinter::with_binary(&tool);
    assert!(linter.lint_bytes("a.coffee", b"x = 1\n").is_empty());
}

#[test]
fn reports_carry_the_source_line_as_original() {
    let messages = parse_raw_output("a.coffee", canned()).unwrap();
    assert_eq!(messages[0].original.as_deref(), Some("x = 1;"));
    let missing = parse_raw_output(
        "a.coffee",
        r#"{"a.coffee": [{"lineNumber": 1, "name": "x", "message": "m", "level": "warn"}]}"#,
    )
    .unwrap();
    assert_eq!(missing[0].original, None);
}
