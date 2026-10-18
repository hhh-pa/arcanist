use super::*;

fn canned() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<lint>
<file name="a.css">
<issue line="1" char="5" severity="error" reason="Expected RBRACE &amp; more." evidence=".a { color: red; "/>
<issue line="2" char="1" severity="warning" reason="Rule is empty." evidence=""/>
</file>
</lint>
"#
}

#[test]
fn parses_issue_elements_with_severities() {
    let messages = parse_lint_xml_output("a.css", canned()).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (1, 5));
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].code, "CSSLint");
    assert_eq!(messages[0].name, "CSSLint");
    assert_eq!(messages[0].description, "Expected RBRACE & more.");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!((messages[1].line, messages[1].col), (2, 1));
    assert!(messages.iter().all(|m| m.path == "a.css"));
}

#[test]
fn empty_report_is_clean() {
    let clean = r#"<?xml version="1.0"?><lint><file name="a.css"></file></lint>"#;
    assert!(parse_lint_xml_output("a.css", clean).unwrap().is_empty());
}

#[test]
fn non_xml_output_is_an_error() {
    assert!(parse_lint_xml_output("a.css", "csslint exploded").is_err());
    assert!(parse_lint_xml_output("a.css", "").is_err());
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = CsslintLinter::new();
    assert_eq!(linter.name(), "CSSLint");
    assert!(
        linter
            .lint_bytes("a.js", b".a { color: red; }\n")
            .is_empty()
    );
    assert!(
        linter
            .lint_bytes("noext", b".a { color: red; }\n")
            .is_empty()
    );
    assert!(linter.lint_bytes("a.css", b".a\0 {}\n").is_empty());
}

#[test]
fn missing_binary_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-csslint-binary");
    let linter = CsslintLinter::with_binary(&missing);
    assert!(
        linter
            .lint_bytes("a.css", b".a { color: red; }\n")
            .is_empty()
    );
}

#[test]
fn lookup_miss_reports_no_tool() {
    assert!(find_on_path("arc-rs-definitely-missing-tool").is_none());
}

#[test]
fn custom_severities_are_rejected_like_php() {
    let linter = CsslintLinter::new();
    assert_eq!(
        linter.validate_severity_code("CSSLint"),
        Err(
            "CSSLint does not currently support custom severity levels, because rules can't be \
             identified from messages in output."
                .to_string()
        )
    );
}

#[test]
fn original_text_is_evidence_sliced_at_char() {
    let messages = parse_lint_xml_output("a.css", canned()).unwrap();
    assert_eq!(messages[0].original.as_deref(), Some(" color: red; "));
    assert_eq!(messages[1].original.as_deref(), Some(""));
    let no_char = parse_lint_xml_output(
        "a.css",
        "<lint><file name=\"a.css\">\
         <issue line=\"1\" severity=\"error\" reason=\"r\" evidence=\"full\"/>\
         </file></lint>",
    )
    .unwrap();
    assert_eq!(no_char[0].original.as_deref(), Some("full"));
}

#[test]
fn php_int_cast_and_substr_match_edge_cases() {
    assert_eq!(php_int(""), 0);
    assert_eq!(php_int("abc"), 0);
    assert_eq!(php_int("  -12abc"), -12);
    assert_eq!(php_int("+7"), 7);
    assert_eq!(php_substr_from("hello", 1), "ello");
    assert_eq!(php_substr_from("hello", -2), "lo");
    assert_eq!(php_substr_from("hello", 9), "");
    assert_eq!(php_substr_from("hello", -9), "");
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
            "arc-rs-lint-csslint-tests-{tag}-{}-{n}",
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
    let tool = tmp.path().join("fake-csslint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = CsslintLinter::new();
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
    assert_eq!(CsslintLinter::new().resolve_binary(), find_on_path(BINARY));
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
    assert_eq!(temp_extension("a.css", "css"), "css");
    assert_eq!(temp_extension("noext", "css"), "css");
    assert_eq!(temp_extension("trailing.", "css"), "css");
}

#[test]
fn unknown_severities_become_errors() {
    let xml = "<lint><file name=\"a.css\">\
         <issue line=\"1\" char=\"1\" severity=\"info\" reason=\"r\" evidence=\"\"/>\
         </file></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn element_tags_skip_name_prefix_collisions() {
    let xml = "<lint><issues><issue line=\"2\" char=\"1\" severity=\"error\" \
         reason=\"r\" evidence=\"\"/></issues></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
}

#[test]
fn element_tags_stop_at_unclosed_tag() {
    let xml = "<lint><issue line=\"2\" severity=\"error\" reason=\"oops\"";
    assert!(parse_lint_xml_output("a.css", xml).unwrap().is_empty());
}

#[test]
fn parse_attrs_skips_bare_and_odd_names() {
    // `checked` has no `=`, `?` is not a name byte, and padding
    // around `=` is skipped; the real attributes still parse.
    let xml = "<lint><issue ?line = \"2\" checked severity = \"error\" \
         reason=\"r\" evidence=\"\"/></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 2);
    assert_eq!(messages[0].severity, "error");
    // A multibyte non-name byte skips one scalar, never a bare byte.
    let xml = "<lint><issue \u{e9}line=\"3\" severity=\"error\" \
         reason=\"r\" evidence=\"\"/></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 3);
}

#[test]
fn parse_attrs_halts_on_dangling_equals() {
    let xml = "<lint><issue foo= ></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "");
}

#[test]
fn parse_attrs_skips_unquoted_values() {
    let xml = "<lint><issue foo=bar severity=\"error\" reason=\"r\" \
         evidence=\"\"/></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "r");
}

#[test]
fn unescape_handles_entities_and_bare_ampersands() {
    let xml = "<lint><issue line=\"1\" char=\"1\" severity=\"error\" \
         reason=\"a &amp; b &bogus; c &#65; d &#x42; e &#X43; f &amp\" \
         evidence=\"\"/></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages[0].description, "a & b &bogus; c A d B e C f &amp");
    // Invalid numerics stay literal.
    let xml = "<lint><issue line=\"1\" char=\"1\" severity=\"error\" \
         reason=\"&#xZZ; &#99999999;\" evidence=\"\"/></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages[0].description, "&#xZZ; &#99999999;");
}

#[test]
fn original_text_starts_at_char_boundaries() {
    // `char - 1` landing mid-scalar advances to the next boundary.
    let xml = "<lint><file name=\"a.css\">\
         <issue line=\"1\" char=\"2\" severity=\"error\" reason=\"r\" evidence=\"\u{e9}x\"/>\
         </file></lint>";
    let messages = parse_lint_xml_output("a.css", xml).unwrap();
    assert_eq!(messages[0].original.as_deref(), Some("x"));
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
    let messages = finish("a.css", &failed, Vec::new());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "csslint failed (exit 3): boom");
    let clean = ToolOutput {
        success: true,
        code: 0,
        stdout: String::new(),
        stderr: String::new(),
    };
    assert!(finish("a.css", &clean, Vec::new()).is_empty());
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_tool_and_parses_report() {
    let tmp = TempDir::new("run");
    let xml = canned().replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-csslint",
        &format!("#!/bin/sh\nprintf '%s\\n' '{xml}'\n"),
    );
    let linter = CsslintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.css", b".a { color: red; }\n");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].description, "Expected RBRACE & more.");
    assert_eq!(messages[1].severity, "warning");
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_unparseable_success() {
    let tmp = TempDir::new("run-junk");
    let tool = write_script(tmp.path(), "fake-csslint", "#!/bin/sh\necho junk\n");
    let linter = CsslintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.css", b".a { color: red; }\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].description,
        "CSSLint returned unparseable output."
    );
}

#[test]
#[cfg(unix)]
fn lint_bytes_reports_tool_failure_on_nonzero_exit() {
    let tmp = TempDir::new("run-fail");
    let tool = write_script(
        tmp.path(),
        "fake-csslint",
        "#!/bin/sh\necho exploded >&2\nexit 3\n",
    );
    let linter = CsslintLinter::with_binary(&tool);
    let messages = linter.lint_bytes("a.css", b".a { color: red; }\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "csslint failed (exit 3): exploded");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    // A tool reporting no parseable version stays inactive.
    let tool = write_script(tmp.path(), "fake-csslint", "#!/bin/sh\necho junk\n");
    let mut linter = CsslintLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .lint_bytes("a.css", b".a { color: red; }\n")
            .is_empty()
    );
    // A tool answering `--version` with a `v`-triple proceeds to lint.
    let xml = canned().replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-csslint-new",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo v1.2.0; else printf '%s\\n' '{xml}'; fi\n"
        ),
    );
    let mut linter = CsslintLinter::with_binary(&tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.css", b".a { color: red; }\n").len(), 2);
}

#[test]
#[cfg(unix)]
fn cmd_shim_binaries_route_through_cmd() {
    // A `.cmd` tool runs via `cmd /C`, which does not exist on unix,
    // so the lint stays silent after exercising the scripted arm.
    let tmp = TempDir::new("cmdshim");
    let tool = write_script(tmp.path(), "fake-csslint.cmd", "#!/bin/sh\nexit 0\n");
    let linter = CsslintLinter::with_binary(&tool);
    assert!(
        linter
            .lint_bytes("a.css", b".a { color: red; }\n")
            .is_empty()
    );
}
