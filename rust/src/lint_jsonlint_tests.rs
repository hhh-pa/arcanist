use super::*;

#[test]
fn parses_compact_lines_with_and_without_path_prefix() {
    let stderr = "a.json: line 1, col 15, Parse error on line 1\nline 2, col 3, trailing comma\nnot a finding\n";
    let messages = parse_compact_output("a.json", stderr);
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (1, 15));
    assert_eq!(messages[0].description, "Parse error on line 1");
    assert_eq!(messages[0].code, "JSON");
    assert_eq!(messages[0].name, "JSON");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[1].line, messages[1].col), (2, 3));
    assert_eq!(messages[1].description, "Trailing comma");
    assert!(messages.iter().all(|m| m.path == "a.json"));
}

#[test]
fn greedy_prefix_split_matches_php() {
    let messages = parse_compact_output("a.json", "x: y: line 4, col 2, bad value\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (4, 2));
    assert_eq!(messages[0].description, "Bad value");
}

#[test]
fn empty_output_is_clean() {
    assert!(parse_compact_output("a.json", "").is_empty());
}

#[test]
fn native_check_validates_json_without_the_tool() {
    let valid = native_json_check("a.json", b"{\"a\": [1, 2]}\n");
    assert!(valid.is_empty());
    let invalid = native_json_check("a.json", b"{\"a\": [1, 2,}\n");
    assert_eq!(invalid.len(), 1);
    assert_eq!(invalid[0].code, "JSON1");
    assert_eq!(invalid[0].name, "Parse Error");
    assert_eq!(invalid[0].severity, "error");
    assert_eq!((invalid[0].line, invalid[0].col), (1, 13));
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = JsonlintLinter::new();
    assert_eq!(linter.name(), "JSON");
    assert!(linter.lint_bytes("a.js", b"not json").is_empty());
    assert!(linter.lint_bytes("noext", b"not json").is_empty());
    assert!(linter.lint_bytes("a.json", b"{\0}").is_empty());
}

#[test]
fn missing_binary_falls_back_to_native_check() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-jsonlint-binary");
    let linter = JsonlintLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.json", b"{\"ok\": true}\n").is_empty());
    let found = linter.lint_bytes("a.json", b"{\"ok\": tru}\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "JSON1");
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn ucfirst_is_byte_based_like_php() {
    assert_eq!(ucfirst("parse error"), "Parse error");
    assert_eq!(ucfirst("école"), "école");
    assert_eq!(ucfirst(""), "");
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
            "arc-rs-lint-jsonlint-tests-{tag}-{}-{n}",
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
    let tool = tmp.path().join("fake-jsonlint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = JsonlintLinter::new();
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
    assert_eq!(JsonlintLinter::new().resolve_binary(), find_on_path(BINARY));
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
    assert_eq!(temp_extension("a.json", "json"), "json");
    assert_eq!(temp_extension("noext", "json"), "json");
    assert_eq!(temp_extension("trailing.", "json"), "json");
}

#[test]
fn colon_at_line_start_matches_nothing() {
    // `rfind` at index 0 breaks out of the split loop like the PHP,
    // leaving no candidate behind.
    assert!(parse_compact_output("a.json", ": line 1, col 2, x\n").is_empty());
}

#[test]
fn failed_splits_fall_through_to_earlier_ones() {
    // The rightmost `: line ` split fails to parse, so the scan
    // retries the earlier split, rightmost-first like the PHP.
    let messages = parse_compact_output("a.json", "x: line 4, col 2, ok: line garbage\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (4, 2));
    assert_eq!(messages[0].description, "Ok: line garbage");
}

#[test]
fn nonnumeric_positions_match_nothing() {
    assert!(parse_compact_output("a.json", "line abc, col 2, x\n").is_empty());
    assert!(parse_compact_output("a.json", "line 1, col xyz, y\n").is_empty());
    assert!(parse_compact_output("a.json", "line , col 2, x\n").is_empty());
}

#[test]
fn short_message_keeps_text_without_positions() {
    assert_eq!(short_message("boom"), "boom");
    assert_eq!(
        short_message("expected value at line 1 column 3"),
        "expected value"
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
    let messages = finish("a.json", &failed, Vec::new());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "jsonlint failed (exit 3): boom");
    let clean = ToolOutput {
        success: true,
        code: 0,
        stderr: String::new(),
    };
    assert!(finish("a.json", &clean, Vec::new()).is_empty());
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_tool_and_parses_findings() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-jsonlint",
        "#!/bin/sh\necho 'a.json: line 1, col 5, oops' >&2\n",
    );
    let linter = JsonlintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.json", b"{\"a\": 1}\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (1, 5));
    assert_eq!(messages[0].description, "Oops");
    assert_eq!(messages[0].code, "JSON");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_tool_failure_on_nonzero_exit() {
    let tmp = TempDir::new("run-fail");
    let tool = write_script(
        tmp.path(),
        "fake-jsonlint",
        "#!/bin/sh\necho exploded >&2\nexit 3\n",
    );
    let linter = JsonlintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.json", b"{\"a\": 1}\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(
        messages[0].description,
        "jsonlint failed (exit 3): exploded"
    );
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    // A tool reporting no parseable version stays inactive.
    let tool = write_script(tmp.path(), "fake-jsonlint", "#!/bin/sh\necho junk\n");
    let mut linter = JsonlintLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.json", b"{\"a\": 1}\n").is_empty());
    // A tool answering `--version` with a triple proceeds to lint.
    let tool = write_script(
        tmp.path(),
        "fake-jsonlint-new",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 1.9.0; else echo 'a.json: line 2, col 3, bad value' >&2; fi\n",
    );
    let mut linter = JsonlintLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    let messages = linter.lint_bytes("a.json", b"{\"a\": 1}\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 3));
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_route_through_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-jsonlint.cmd", "#!/bin/sh\nexit 0\n");
    let linter = JsonlintLinter::with_binary(&tool);
    assert!(linter.lint_bytes("a.json", b"{\"a\": 1}\n").is_empty());
}
