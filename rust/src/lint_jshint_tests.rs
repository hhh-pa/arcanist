use super::*;

fn canned() -> &'static str {
    r#"[{"file":"a.js","line":3,"col":9,"reason":"Missing semicolon.","code":"W032"},{"file":"a.js","line":7,"col":1,"reason":"Too many errors.","code":"E043"},{"file":"a.js","line":10,"col":5,"reason":"Expected an assignment.","code":"E030"}]"#
}

#[test]
fn parses_report_items_with_severities() {
    let messages = parse_reporter_output("a.js", canned()).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].code, "W032");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].name, "JSHintW032");
    assert_eq!((messages[0].line, messages[0].col), (3, 9));
    assert_eq!(messages[0].description, "Missing semicolon.");
    assert_eq!(messages[1].severity, "disabled");
    assert_eq!(messages[1].name, "JSHintE043");
    assert_eq!(messages[2].severity, "error");
    assert!(messages.iter().all(|m| m.path == "a.js"));
}

#[test]
fn empty_report_is_clean() {
    assert!(parse_reporter_output("a.js", "[]").unwrap().is_empty());
}

#[test]
fn malformed_report_is_an_error() {
    assert!(parse_reporter_output("a.js", "not json").is_err());
    assert!(parse_reporter_output("a.js", r#"{"oops":true}"#).is_err());
    assert!(parse_reporter_output("a.js", "").is_err());
}

#[test]
fn missing_fields_fall_back_to_line_one() {
    let messages = parse_reporter_output("a.js", r#"[{}]"#).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = JshintLinter::new();
    assert_eq!(linter.name(), "JSHint");
    assert!(linter.lint_bytes("a.css", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("a.js", b"var x = 1;\0\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-jshint-binary");
    let linter = JshintLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[cfg(unix)]
struct TempDir {
    path: PathBuf,
}

#[cfg(unix)]
impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-jshint-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
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

fn option_ctx(root: &Path) -> OptionContext<'_> {
    OptionContext { project_root: root }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[test]
fn set_option_handles_configs_and_tool_options() {
    let dir = std::env::temp_dir().join(format!("arc-rs-jshint-opts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tool = dir.join("fake-jshint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(&dir);
    let mut linter = JshintLinter::new();
    linter
        .set_option("jshint.jshintrc", &serde_json::json!("/tmp/rc"), &ctx)
        .unwrap();
    linter
        .set_option("jshint.jshintignore", &serde_json::json!("/tmp/ig"), &ctx)
        .unwrap();
    assert_eq!(
        linter.set_option("jshint.jshintrc", &serde_json::json!(42), &ctx),
        Err("'jshint.jshintrc' must be a string".to_string())
    );
    assert_eq!(
        linter.set_option("jshint.jshintignore", &serde_json::json!(true), &ctx),
        Err("'jshint.jshintignore' must be a string".to_string())
    );
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("flags", &serde_json::json!(["--extra"]), &ctx)
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 2.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(true), &ctx)
            .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn configs_feed_tool_flags() {
    let dir = std::env::temp_dir().join(format!("arc-rs-jshint-flags-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = option_ctx(&dir);
    let mut linter = JshintLinter::new();
    linter
        .set_option("jshint.jshintrc", &serde_json::json!("/tmp/rc"), &ctx)
        .unwrap();
    linter
        .set_option("jshint.jshintignore", &serde_json::json!("/tmp/ig"), &ctx)
        .unwrap();
    let flags = linter.tool_flags("rep.js");
    assert!(flags.contains(&"--reporter=rep.js".to_string()));
    assert!(flags.contains(&"--config=/tmp/rc".to_string()));
    assert!(flags.contains(&"--exclude-path=/tmp/ig".to_string()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn temp_extension_falls_back_without_a_suffix() {
    assert_eq!(temp_extension("Makefile", "js"), "js");
    assert_eq!(temp_extension("a.txt", "js"), "txt");
}

#[test]
#[cfg(unix)]
fn configured_bin_resolves_and_runs() {
    let tmp = TempDir::new("bin");
    let flat = canned().replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-jshint",
        &format!("#!/bin/sh\nprintf '%s\\n' '{flat}'\n"),
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = JshintLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    assert_eq!(linter.lint_bytes("a.js", b"var x = 1;\n").len(), 3);
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(tmp.path(), "fake-jshint", "#!/bin/sh\necho junk\n");
    let mut linter = JshintLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 2.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
#[cfg(unix)]
fn clean_exit_with_junk_output_reports_parse_failure() {
    let tmp = TempDir::new("junk");
    let tool = write_script(tmp.path(), "fake-jshint", "#!/bin/sh\necho junk\n");
    let linter = JshintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "JSHint");
    assert!(messages[0].description.contains("unparseable"));
}

#[test]
#[cfg(unix)]
fn failing_exit_with_empty_report_reports_tool_failure() {
    let tmp = TempDir::new("fail");
    let tool = write_script(
        tmp.path(),
        "fake-jshint",
        "#!/bin/sh\necho boom >&2\necho '[]'\nexit 3\n",
    );
    let linter = JshintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "jshint failed (exit 3): boom");
}

#[test]
#[cfg(unix)]
fn long_stderr_truncates_on_char_boundaries() {
    let tmp = TempDir::new("trunc");
    // Byte 300 lands inside the two-byte `é`, forcing the boundary walk.
    let loud = format!("{}{}{}", "a".repeat(299), "é", "b".repeat(50));
    let tool = write_script(
        tmp.path(),
        "fake-jshint",
        &format!("#!/bin/sh\necho '{loud}' >&2\necho junk\nexit 1\n"),
    );
    let linter = JshintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].description.ends_with("..."),
        "{}",
        messages[0].description
    );
    assert!(messages[0].description.len() < loud.len());
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_are_silent_without_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-jshint.cmd", "#!/bin/sh\nexit 0\n");
    let linter = JshintLinter::with_binary(&tool);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
fn severity_codes_are_raw_like_php() {
    let linter = JshintLinter::new();
    for code in ["E033", "W093"] {
        assert_eq!(linter.validate_severity_code(code), Ok(code.to_string()));
    }
    for code in ["", "E", "C901", "[E033]", "e033"] {
        assert_eq!(
            linter.validate_severity_code(code),
            Err(format!(
                "Unrecognized lint message code \"{code}\". Expected a valid JSHint lint code \
                 like \"E033\" or \"W093\"."
            )),
            "{code:?}"
        );
    }
}
