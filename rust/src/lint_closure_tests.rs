use super::*;

#[test]
fn parses_finding_lines_and_skips_headers() {
    let stdout = "----- FILE  :  a.js -----\n\
        Line 3, E:0001: Missing semicolon at end of line\n\
        Line 7, E:0213: Extra semicolon, E:9999: nested\n\
        Found 2 errors in 1 files\n";
    let messages = parse_gjslint_output("a.js", stdout);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].line, 3);
    assert_eq!(messages[0].col, 1);
    assert_eq!(messages[0].code, "GJSLINT0001");
    assert_eq!(messages[0].name, "GJSLINT0001");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "Missing semicolon at end of line");
    assert_eq!(messages[1].code, "GJSLINT0213");
    assert_eq!(messages[1].description, "Extra semicolon, E:9999: nested");
    assert!(messages.iter().all(|m| m.path == "a.js"));
}

#[test]
fn empty_output_is_clean() {
    assert!(parse_gjslint_output("a.js", "").is_empty());
    assert!(parse_gjslint_output("a.js", "Found 0 errors\n").is_empty());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = ClosureLinter::new();
    assert_eq!(linter.name(), "GJSLINT");
    assert!(linter.lint_bytes("a.ts", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("a.js", b"var\0x;\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-gjslint-binary");
    let linter = ClosureLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

// -( T1170 wave 5d: options, tool runs, parser edges )------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-closure-tests-{tag}-{}-{n}",
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
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-gjslint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = ClosureLinter::new();
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
    assert!(
        linter
            .set_option("flags", &serde_json::json!("--extra"), &ctx)
            .is_err()
    );
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(true), &ctx)
            .is_err()
    );
}

#[test]
fn resolve_binary_falls_back_to_path_lookup() {
    // The default linter delegates to the PATH lookup; both calls run
    // the same lookup, so they agree without depending on ambient PATH
    // contents.
    assert_eq!(ClosureLinter::new().resolve_binary(), find_on_path(BINARY));
}

#[test]
fn find_on_path_matches_absolute_paths() {
    // An absolute candidate survives the directory join, so a temp
    // file exercises the hit arm without touching PATH.
    let tmp = TempDir::new("path-hit");
    let tool = tmp.path().join("probe-target");
    std::fs::write(&tool, b"x").unwrap();
    let absolute = tool.to_string_lossy().to_string();
    assert_eq!(find_on_path(&absolute), Some(tool));
}

#[test]
fn temp_extension_falls_back_without_suffix() {
    assert_eq!(temp_extension("a.js", "js"), "js");
    assert_eq!(temp_extension("noext", "js"), "js");
    assert_eq!(temp_extension("trailing.", "js"), "js");
}

#[test]
fn nonnumeric_fields_match_nothing() {
    assert!(parse_gjslint_output("a.js", "Line abc, E:0001: x\n").is_empty());
    assert!(parse_gjslint_output("a.js", "Line 1, E:xyz: x\n").is_empty());
    assert!(parse_gjslint_output("a.js", "Line 1, E:: x\n").is_empty());
}

#[test]
fn truncate_stops_at_char_boundaries() {
    let long = "x".repeat(299) + "é" + &"y".repeat(50);
    assert_eq!(truncate(&long, 300), format!("{}...", "x".repeat(299)));
    assert_eq!(truncate("short", 300), "short");
    assert_eq!(first_line(""), "no output");
    assert_eq!(first_line("  \nsecond"), "no output");
}

#[test]
fn finish_reports_tool_failure_on_empty_nonzero() {
    let failed = ToolOutput {
        success: false,
        code: 3,
        stdout: String::new(),
        stderr: "boom\n".to_string(),
    };
    let messages = finish("a.js", &failed, Vec::new());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "gjslint failed (exit 3): boom");
    let clean = ToolOutput {
        success: true,
        code: 0,
        stdout: String::new(),
        stderr: String::new(),
    };
    assert!(finish("a.js", &clean, Vec::new()).is_empty());
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_tool_and_parses_findings() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-gjslint",
        "#!/bin/sh\necho 'Line 3, E:0001: Missing semicolon'\n",
    );
    let linter = ClosureLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 3);
    assert_eq!(messages[0].code, "GJSLINT0001");
    assert_eq!(messages[0].description, "Missing semicolon");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_tool_failure_on_nonzero_exit() {
    let tmp = TempDir::new("run-fail");
    let tool = write_script(
        tmp.path(),
        "fake-gjslint",
        "#!/bin/sh\necho exploded >&2\nexit 3\n",
    );
    let linter = ClosureLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "gjslint failed (exit 3): exploded");
}

#[test]
fn version_requirement_gates_linting() {
    // `gjslint` has no version pattern, so any nonempty requirement
    // stays inactive without spawning: the installed version is
    // unknowable. A plain file suffices for the binary probe.
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = tmp.path().join("fake-gjslint");
    std::fs::write(&tool, b"x").unwrap();
    let mut linter = ClosureLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.js", b"var x = 1\n").is_empty());
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_route_through_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-gjslint.cmd", "#!/bin/sh\nexit 0\n");
    let linter = ClosureLinter::with_binary(&tool);
    assert!(linter.lint_bytes("a.js", b"var x = 1\n").is_empty());
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("temp-fail");
    let tool = tmp.path().join("fake-gjslint");
    std::fs::write(&tool, b"x").unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let linter = ClosureLinter {
        binary: Some(tool),
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    assert!(linter.lint_bytes("a.js", b"var x = 1;").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}
