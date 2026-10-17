use super::*;

fn canned() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<checkstyle version="1.0">
<file name="a.js">
<error line="2" column="1" severity="error" message="Missing semicolon" source="requireSemicolons"/>
<error line="4" column="12" severity="warning" message="Line too long" source="maximumLineLength"/>
<error line="6" column="3" severity="info" message="Stray &quot;quote&quot; &amp; ampersand" source="misc"/>
</file>
</checkstyle>
"#
}

#[test]
fn parses_error_elements_with_severities() {
    let messages = parse_checkstyle_output("a.js", canned()).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].code, "JSCS");
    assert_eq!(messages[0].name, "JSCS");
    assert_eq!(messages[0].description, "Missing semicolon");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!((messages[1].line, messages[1].col), (4, 12));
    assert_eq!(messages[2].severity, "advice");
    assert_eq!(messages[2].description, "Stray \"quote\" & ampersand");
    assert!(messages.iter().all(|m| m.path == "a.js"));
}

#[test]
fn empty_report_is_clean() {
    let clean =
        r#"<?xml version="1.0"?><checkstyle version="1.0"><file name="a.js"></file></checkstyle>"#;
    assert!(parse_checkstyle_output("a.js", clean).unwrap().is_empty());
}

#[test]
fn non_xml_output_is_an_error() {
    assert!(parse_checkstyle_output("a.js", "jscs exploded").is_err());
    assert!(parse_checkstyle_output("a.js", "").is_err());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = JscsLinter::new();
    assert_eq!(linter.name(), "JSCS");
    assert!(linter.lint_bytes("a.json", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("noext", b"var x = 1;\n").is_empty());
    assert!(linter.lint_bytes("a.js", b"var x\0;\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-jscs-binary");
    let linter = JscsLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn custom_severities_are_rejected_like_php() {
    let linter = JscsLinter::new();
    assert_eq!(
        linter.validate_severity_code("JSCS"),
        Err(
            "JSCS does not currently support custom severity levels, because rules can't be \
             identified from messages in output."
                .to_string()
        )
    );
}

// -( Coverage: options, tool runs, parser edges )------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-jscs-tests-{tag}-{}-{n}",
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
fn set_config_and_preset_feed_tool_flags() {
    let tmp = TempDir::new("flags");
    let ctx = option_ctx(tmp.path());
    let mut linter = JscsLinter::new();
    linter
        .set_option("jscs.config", &serde_json::json!("/tmp/jscs.json"), &ctx)
        .unwrap();
    linter
        .set_option("jscs.preset", &serde_json::json!("crockford"), &ctx)
        .unwrap();
    let flags = linter.tool_flags();
    assert!(flags.contains(&"--reporter=checkstyle".to_string()));
    assert!(flags.contains(&"--no-colors".to_string()));
    assert!(flags.contains(&"--config=/tmp/jscs.json".to_string()));
    assert!(flags.contains(&"--preset=crockford".to_string()));
}

#[test]
fn set_config_and_preset_reject_non_strings() {
    let tmp = TempDir::new("opt-err");
    let ctx = option_ctx(tmp.path());
    let mut linter = JscsLinter::new();
    assert_eq!(
        linter.set_option("jscs.config", &serde_json::json!(42), &ctx),
        Err("'jscs.config' must be a string".to_string())
    );
    assert_eq!(
        linter.set_option("jscs.preset", &serde_json::json!(true), &ctx),
        Err("'jscs.preset' must be a string".to_string())
    );
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-jscs");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = JscsLinter::new();
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
    assert_eq!(JscsLinter::new().resolve_binary(), find_on_path(BINARY));
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
fn element_tags_skip_name_prefix_collisions() {
    let xml = "<checkstyle><errors><error line=\"2\" column=\"1\" severity=\"error\" \
         message=\"m\"/></errors></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
}

#[test]
fn element_tags_stop_at_unclosed_tag() {
    let xml = "<checkstyle><error line=\"2\" severity=\"error\" message=\"oops\"";
    assert!(parse_checkstyle_output("a.js", xml).unwrap().is_empty());
}

#[test]
fn parse_attrs_skips_bare_and_odd_names() {
    // `checked` has no `=`, `?` is not a name byte, and padding
    // around `=` is skipped; the real attributes still parse.
    let xml = "<checkstyle><error ?line = \"2\" checked severity = \"error\" \
         message=\"m\"/></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 2);
    assert_eq!(messages[0].severity, "error");
    // A multibyte non-name byte skips one scalar, never a bare byte.
    let xml = "<checkstyle><error \u{e9}line=\"3\" severity=\"error\" \
         message=\"m\"/></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 3);
}

#[test]
fn parse_attrs_halts_on_dangling_equals() {
    let xml = "<checkstyle><error foo= ></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "");
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
}

#[test]
fn parse_attrs_skips_unquoted_values() {
    let xml = "<checkstyle><error foo=bar severity=\"error\" \
         message=\"m\"/></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "m");
}

#[test]
fn unescape_handles_entities_and_bare_ampersands() {
    let xml = "<checkstyle><error line=\"1\" column=\"1\" severity=\"error\" \
         message=\"a &amp; b &bogus; c &#65; d &#x42; e &#X43; f &amp\"/></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages[0].description, "a & b &bogus; c A d B e C f &amp");
    // Invalid numerics stay literal.
    let xml = "<checkstyle><error line=\"1\" column=\"1\" severity=\"error\" \
         message=\"&#xZZ; &#99999999;\"/></checkstyle>";
    let messages = parse_checkstyle_output("a.js", xml).unwrap();
    assert_eq!(messages[0].description, "&#xZZ; &#99999999;");
}

#[test]
fn truncate_stops_at_char_boundaries() {
    let long = "x".repeat(299) + "\u{e9}" + &"y".repeat(50);
    assert_eq!(truncate(&long, 300), "x".repeat(299) + "...");
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
    assert_eq!(messages[0].description, "jscs failed (exit 3): boom");
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
fn lint_bytes_runs_tool_and_parses_report() {
    let tmp = TempDir::new("run");
    let xml = canned().replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-jscs",
        &format!("#!/bin/sh\nprintf '%s\\n' '{xml}'\n"),
    );
    let linter = JscsLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].description, "Missing semicolon");
    assert_eq!(messages[2].severity, "advice");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_unparseable_success() {
    let tmp = TempDir::new("run-junk");
    let tool = write_script(tmp.path(), "fake-jscs", "#!/bin/sh\necho junk\n");
    let linter = JscsLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "JSCS returned unparseable output.");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_tool_failure_on_nonzero_exit() {
    let tmp = TempDir::new("run-fail");
    let tool = write_script(
        tmp.path(),
        "fake-jscs",
        "#!/bin/sh\necho exploded >&2\nexit 3\n",
    );
    let linter = JscsLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "jscs failed (exit 3): exploded");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    // A tool reporting no parseable version stays inactive.
    let tool = write_script(tmp.path(), "fake-jscs", "#!/bin/sh\necho junk\n");
    let mut linter = JscsLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
    // A tool answering `--version` with a triple proceeds to lint.
    let xml = canned().replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-jscs-new",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 2.1.0; else printf '%s\\n' '{xml}'; fi\n"
        ),
    );
    let mut linter = JscsLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.js", b"var x = 1;\n").len(), 3);
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_route_through_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-jscs.cmd", "#!/bin/sh\nexit 0\n");
    let linter = JscsLinter::with_binary(&tool);
    assert!(linter.lint_bytes("a.js", b"var x = 1;\n").is_empty());
}
