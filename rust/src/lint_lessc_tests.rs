use super::*;

#[test]
fn parses_lint_lines_with_mapped_names() {
    let stderr = "NameError: variable @color is undefined in a.less on line 2, column 12:\n\
        SyntaxError: unrecognised input in a.less on line 5, column 3:\n\
        5 .a { color: @color; }\n";
    let messages = parse_lint_output("a.less", stderr);
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (2, 12));
    assert_eq!(messages[0].code, "LESSCNameError");
    assert_eq!(messages[0].name, "Unknown lint message!");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "Variable @color is undefined");
    assert_eq!(messages[1].code, "LESSCSyntaxError");
    assert_eq!(messages[1].name, "Unknown lint message!");
    assert!(messages.iter().all(|m| m.path == "a.less"));
}

#[test]
fn unknown_error_names_become_clear_errors() {
    let messages = parse_lint_output(
        "a.less",
        "FutureError: something new in a.less on line 1, column 1:\n",
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert!(messages[0].description.contains("FutureError"));
    // An unrecognized name aborts the parse like the PHP throw: valid
    // findings before it are discarded too.
    let mixed = parse_lint_output(
        "a.less",
        "NameError: bad thing in a.less on line 1, column 1:\n\
         FutureError: something new in a.less on line 2, column 1:\n",
    );
    assert_eq!(mixed.len(), 1);
    assert!(mixed[0].description.contains("FutureError"));
}

#[test]
fn empty_output_is_clean() {
    assert!(parse_lint_output("a.less", "").is_empty());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = LesscLinter::new();
    assert_eq!(linter.name(), "LESSC");
    assert!(linter.lint_bytes("a.css", b"@x: 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"@x: 1;\n").is_empty());
    assert!(linter.lint_bytes("a.less", b"@x\0: 1;\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-lessc-binary");
    let linter = LesscLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.less", b"@x: 1;\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn ucfirst_is_byte_based_like_php() {
    assert_eq!(ucfirst("variable is bad"), "Variable is bad");
    assert_eq!(ucfirst("école"), "école");
}

#[test]
fn severity_lookup_strips_lessc_to_the_error_name() {
    let linter = LesscLinter::new();
    assert_eq!(
        linter.severity_lookup_code("LESSCRuntimeError"),
        "RuntimeError"
    );
    assert_eq!(linter.severity_lookup_code("LESSC"), "");
    assert_eq!(linter.severity_lookup_code("OTHER"), "OTHER");
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
            "arc-rs-lint-lessc-tests-{tag}-{}-{n}",
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
fn strict_modes_toggle_tool_flags() {
    let tmp = TempDir::new("strict");
    let ctx = option_ctx(tmp.path());
    let mut linter = LesscLinter::new();
    assert!(
        linter
            .tool_flags()
            .contains(&"--strict-math=off".to_string())
    );
    assert!(
        linter
            .tool_flags()
            .contains(&"--strict-units=off".to_string())
    );
    linter
        .set_option("lessc.strict-math", &serde_json::json!(true), &ctx)
        .unwrap();
    assert!(
        linter
            .tool_flags()
            .contains(&"--strict-math=on".to_string())
    );
    linter
        .set_option("lessc.strict-units", &serde_json::json!(true), &ctx)
        .unwrap();
    assert!(
        linter
            .tool_flags()
            .contains(&"--strict-units=on".to_string())
    );
    let err = linter
        .set_option("lessc.strict-math", &serde_json::json!("yes"), &ctx)
        .unwrap_err();
    assert!(err.contains("must be a boolean"), "{err}");
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-lessc");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = LesscLinter::new();
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
    assert_eq!(LesscLinter::new().resolve_binary(), find_on_path(BINARY));
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
    assert_eq!(temp_extension("a.less", "less"), "less");
    assert_eq!(temp_extension("noext", "less"), "less");
    assert_eq!(temp_extension("trailing.", "less"), "less");
}

#[test]
fn malformed_lines_match_nothing() {
    // Error names must be bare words.
    assert!(
        parse_lint_output("a.less", "Bad Name: desc in a.less on line 1, column 1:\n").is_empty()
    );
    assert!(parse_lint_output("a.less", ": desc in a.less on line 1, column 1:\n").is_empty());
    // The description must be nonempty.
    assert!(parse_lint_output("a.less", "NameError:  in a.less on line 1, column 1:\n").is_empty());
    // Positions must be numeric.
    assert!(
        parse_lint_output("a.less", "NameError: x in a.less on line X, column 1:\n").is_empty()
    );
    assert!(
        parse_lint_output("a.less", "NameError: x in a.less on line 1, column Y:\n").is_empty()
    );
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
        stderr: "boom\n".to_string(),
    };
    let messages = finish("a.less", &failed, Vec::new());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "lessc failed (exit 3): boom");
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    let clean = ToolOutput {
        success: true,
        code: 0,
        stderr: String::new(),
    };
    assert!(finish("a.less", &clean, Vec::new()).is_empty());
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_tool_and_parses_findings() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-lessc",
        "#!/bin/sh\necho 'NameError: variable @x is undefined in a.less on line 2, column 12:' >&2\n",
    );
    let linter = LesscLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.less", b"@x: 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 12));
    assert_eq!(messages[0].code, "LESSCNameError");
    assert_eq!(messages[0].description, "Variable @x is undefined");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_tool_failure_on_nonzero_exit() {
    let tmp = TempDir::new("run-fail");
    let tool = write_script(
        tmp.path(),
        "fake-lessc",
        "#!/bin/sh\necho exploded >&2\nexit 3\n",
    );
    let linter = LesscLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.less", b"@x: 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "lessc failed (exit 3): exploded");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    // `lessc` probes `--version` but never reports one, so any
    // nonempty requirement stays inactive.
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(
        tmp.path(),
        "fake-lessc",
        "#!/bin/sh\necho 'NameError: variable @x is undefined in a.less on line 2, column 12:' >&2\n",
    );
    let mut linter = LesscLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.less", b"@x: 1;\n").is_empty());
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_route_through_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-lessc.cmd", "#!/bin/sh\nexit 0\n");
    let linter = LesscLinter::with_binary(&tool);
    assert!(linter.lint_bytes("a.less", b"@x: 1;\n").is_empty());
}
