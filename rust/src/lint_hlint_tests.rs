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
