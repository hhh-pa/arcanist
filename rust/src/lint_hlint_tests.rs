use super::*;
use std::path::PathBuf;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-hlint-tests-{tag}-{}-{n}",
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

const IDEAS: &str = r#"[
  {"module":"Main","decl":"f","severity":"Warning","hint":"Use null","startLine":1,"startColumn":8,"from":"length xs == 0","to":"null xs","note":[]},
  {"module":"Main","decl":"test","severity":"Error","hint":"Eta reduce","startLine":3,"startColumn":1,"from":"f x","to":"f","note":["May change semantics","\"quoted\""]},
  {"module":"M","decl":"g","severity":"Suggestion","hint":"Use map","startLine":5,"startColumn":3,"from":"x","to":"y","note":[]}
]"#;

const NULL_IDEA: &str = r#"[
  {"module":"Main","decl":"f","severity":"Warning","hint":"Use null","startLine":1,"startColumn":8,"from":"a","to":"b","note":[]},
  null
]"#;

#[test]
fn linter_name_binary_and_flags_are_exact() {
    assert_eq!(HlintLinter::new().name(), "HLINT");
    assert_eq!(binary_name(), "hlint");
    assert_eq!(command_flags(), vec!["--json".to_string()]);
}

#[test]
fn supports_haskell_extensions_case_insensitively() {
    assert!(is_supported("Main.hs"));
    assert!(is_supported("Old.LHS"));
    assert!(is_supported("win\\A.Hs"));
    for path in ["a.h", "a.hs.bak", "a.txt", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = HlintLinter::new();
    assert!(linter.lint_bytes("notes.txt", b"f xs = x\n").is_empty());
    assert!(linter.lint_bytes("Main.hs", b"f \0 x\n").is_empty());
}

#[test]
fn absent_or_broken_binary_reports_nothing() {
    assert!(lint_with_binary("Main.hs", b"f x = x\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-hlint");
    assert!(lint_with_binary("Main.hs", b"f x = x\n", Some(&missing)).is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let tmp = TempDir::new("lookup");
    assert!(binary_in(tmp.path().as_os_str()).is_none());
    let marker = tmp.path().join(binary_name());
    std::fs::write(&marker, b"x").unwrap();
    assert_eq!(binary_in(tmp.path().as_os_str()), Some(marker));
}

#[test]
fn parses_ideas_with_mapped_severities_and_notes() {
    let messages = parse_output("Main.hs", 1, IDEAS, "");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].path, "Main.hs");
    assert_eq!((messages[0].line, messages[0].col), (1, 8));
    assert_eq!(messages[0].code, "HLINT");
    assert_eq!(messages[0].name, "Use null");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(
        messages[0].description,
        "In module `Main`, declaration `f`."
    );
    assert_eq!(messages[1].severity, "error");
    assert_eq!(
        messages[1].description,
        "In module `Main`, declaration `test`. **NOTE**: May change semantics. **NOTE**: quoted."
    );
    assert_eq!(messages[2].severity, "advice");
    assert_eq!(messages[0].original.as_deref(), Some("length xs == 0"));
    assert_eq!(messages[0].replacement.as_deref(), Some("null xs"));
}

#[test]
fn null_element_discards_everything_like_php() {
    // PHP `return`s null on the first null element, so no message is
    // added at all, not even the valid ones before it.
    assert!(parse_output("Main.hs", 0, NULL_IDEA, "").is_empty());
    assert!(parse_output("Main.hs", 0, r#"[null]"#, "").is_empty());
    assert!(parse_output("Main.hs", 0, r#"["oops"]"#, "").is_empty());
}

#[test]
fn invalid_json_yields_no_messages() {
    assert!(parse_output("Main.hs", 0, "", "").is_empty());
    assert!(parse_output("Main.hs", 0, "not json", "").is_empty());
    assert!(parse_output("Main.hs", 0, "{\"a\":1}", "").is_empty());
    assert!(parse_output("Main.hs", 0, "[]", "").is_empty());
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
fn trailing_dot_has_no_extension() {
    assert!(!is_supported("Main."));
    assert_eq!(extension_of("Main."), None);
    assert_eq!(staged_name("Makefile"), "src.hs");
    assert_eq!(staged_name("a.verylongextension"), "src.hs");
    assert_eq!(staged_name("Main.hs"), "src.hs");
}

#[test]
fn empty_path_segments_are_skipped() {
    assert!(binary_in(std::ffi::OsStr::new("")).is_none());
}

#[test]
fn missing_or_unshaped_notes_add_no_suffix() {
    for idea in [
        r#"[{"module":"M","decl":"d","severity":"Warning","hint":"h","startLine":1,"startColumn":1,"from":"a","to":"b"}]"#,
        r#"[{"module":"M","decl":"d","severity":"Warning","hint":"h","startLine":1,"startColumn":1,"from":"a","to":"b","note":"x"}]"#,
        r#"[{"module":"M","decl":"d","severity":"Warning","hint":"h","startLine":1,"startColumn":1,"from":"a","to":"b","note":[42,{"k":"v"}]}]"#,
    ] {
        let messages = parse_output("Main.hs", 0, idea, "");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].description, "In module `M`, declaration `d`.");
    }
}

#[test]
fn positions_tolerate_numbers_strings_and_missing_values() {
    let line_of = |idea: &str| parse_output("Main.hs", 0, idea, "")[0].line;
    let col_of = |idea: &str| parse_output("Main.hs", 0, idea, "")[0].col;
    let idea = |line: &str, col: &str| {
        format!(
            r#"[{{"module":"M","decl":"d","severity":"Warning","hint":"h","startLine":{line},"startColumn":{col},"from":"a","to":"b","note":[]}}]"#
        )
    };
    assert_eq!(line_of(&idea("-3", "1")), 1);
    assert_eq!(line_of(&idea("2.5", "1")), 1);
    assert_eq!(
        (
            line_of(&idea("\"7\"", "\"3\"")),
            col_of(&idea("\"7\"", "\"3\""))
        ),
        (7, 3)
    );
    assert_eq!(line_of(&idea("\"abc\"", "1")), 1);
    let messages = parse_output("Main.hs", 0, "[{}]", "");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-hlint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = HlintLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("flags", &serde_json::json!(["--json"]), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("flags", &serde_json::json!("--json"), &ctx)
            .is_err()
    );
    linter
        .set_option("version", &serde_json::json!(">= 3.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(true), &ctx)
            .is_err()
    );
}

#[test]
#[cfg(unix)]
fn lint_with_binary_runs_the_tool() {
    let tmp = TempDir::new("run");
    let flat = IDEAS.replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-hlint",
        &format!("#!/bin/sh\nprintf '%s\\n' '{flat}'\n"),
    );
    let messages = lint_with_binary("Main.hs", b"f x = x\n", Some(&tool));
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].name, "Use null");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    // A tool reporting no parseable version stays inactive.
    let tool = write_script(tmp.path(), "fake-hlint", "#!/bin/sh\necho junk\n");
    let mut linter = HlintLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 3.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("Main.hs", b"f x = x\n").is_empty());
    // A tool answering `--version` proceeds to lint.
    let flat = IDEAS.replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-hlint-new",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'HLint v3.8, Copyright'; else printf '%s\\n' '{flat}'; fi\n"
        ),
    );
    let mut linter = HlintLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 3.8"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("Main.hs", b"f x = x\n").len(), 3);
    // An unsatisfied requirement stays inactive even with a version.
    linter
        .set_option("version", &serde_json::json!(">= 9.9"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("Main.hs", b"f x = x\n").is_empty());
}

#[test]
fn t1170_parse_idea_rejects_non_objects() {
    // T1170 band J: only objects parse into ideas; anything else
    // yields no message.
    assert!(parse_idea("a.hs", &serde_json::json!(5)).is_none());
    assert!(parse_idea("a.hs", &serde_json::json!(null)).is_none());
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("temp-fail");
    let tool = tmp.path().join("fake-hlint");
    std::fs::write(&tool, b"x").unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut linter = HlintLinter {
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    linter.options.bin = Some(tool);
    assert!(linter.lint_bytes("Main.hs", b"f x = x").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}
