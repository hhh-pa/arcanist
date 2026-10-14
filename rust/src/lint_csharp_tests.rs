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
            "arc-rs-lint-csharp-tests-{tag}-{}-{n}",
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

const RESULTS: &str = r#"[
  {"FileName":"C:\\proj\\Foo.cs","Issues":[
    {"LineNumber":10,"Column":5,"OriginalText":"x","ReplacementText":"y","Index":{"Code":"SA1600","Name":"ElementsMustBeDocumented","Message":"Missing %s for %s.","Severity":2},"Parameters":["XML comment","Foo"]},
    {"LineNumber":3,"Column":1,"Index":{"Code":"CS0001","Name":"Syntax","Message":"Syntax error","Severity":3},"Parameters":[]},
    {"LineNumber":1,"Column":1,"Index":{"Code":"XX0","Name":"N0","Message":"m","Severity":0},"Parameters":[]},
    {"LineNumber":2,"Column":2,"Index":{"Code":"XX1","Name":"N1","Message":"m","Severity":1},"Parameters":[]},
    {"LineNumber":5,"Column":5,"Index":{"Code":"XX4","Name":"N4","Message":"m","Severity":4},"Parameters":[]},
    {"LineNumber":6,"Column":6,"Index":{"Code":"XX9","Name":"N9","Message":"m","Severity":9},"Parameters":[]}
  ]},
  {"FileName":"other.cs","Issues":null},
  {"FileName":"skipped.cs","Issues":[
    {"LineNumber":1,"Column":1,"Index":{"Code":"YY","Name":"Y","Message":"m","Severity":3},"Parameters":[]}
  ]}
]"#;

#[test]
fn linter_name_binary_and_flags_are_exact() {
    assert_eq!(CsharpLinter::new().name(), "C#");
    assert_eq!(binary_name(), "cslint.exe");
    assert_eq!(runtime_name(), "mono");
    assert_eq!(default_settings_base64(), "bnVsbA==");
    assert_eq!(
        command_flags(),
        vec!["--settings-base64=bnVsbA==".to_string(), "-r=.".to_string(),]
    );
}

#[test]
fn supports_only_cs_files() {
    assert!(is_supported("Foo.cs"));
    assert!(is_supported("src/Bar.CS"));
    assert!(is_supported("win\\A.cs"));
    for path in ["a.c", "a.css", "a.txt", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = CsharpLinter::new();
    assert!(linter.lint_bytes("notes.txt", b"class A {}\n").is_empty());
    assert!(linter.lint_bytes("Foo.cs", b"class \0 A\n").is_empty());
}

#[test]
fn absent_or_broken_command_reports_nothing() {
    assert!(lint_with_command("Foo.cs", b"class A {}\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-cslint");
    assert!(lint_with_command("Foo.cs", b"class A {}\n", Some((&missing, &[]))).is_empty());
}

#[test]
fn command_lookup_needs_analyzer_and_runtime() {
    let tmp = TempDir::new("lookup");
    assert!(resolve_command_in(tmp.path().as_os_str()).is_none());
    let cslint = tmp.path().join(binary_name());
    std::fs::write(&cslint, b"x").unwrap();
    if cfg!(windows) {
        let (program, prefix) = resolve_command_in(tmp.path().as_os_str()).unwrap();
        assert_eq!(program, cslint);
        assert_eq!(prefix, command_flags());
    } else {
        assert!(resolve_command_in(tmp.path().as_os_str()).is_none());
        let mono = tmp.path().join(runtime_name());
        std::fs::write(&mono, b"x").unwrap();
        let (program, prefix) = resolve_command_in(tmp.path().as_os_str()).unwrap();
        assert_eq!(program, mono);
        assert_eq!(prefix[0], cslint.to_string_lossy());
        assert_eq!(&prefix[1..], command_flags().as_slice());
    }
}

#[test]
fn parses_issues_with_rendered_descriptions_and_severities() {
    let messages = parse_output("Foo.cs", 0, RESULTS, "");
    // The null-`Issues` result aborts parsing; the trailing result is skipped.
    assert_eq!(messages.len(), 6);
    assert!(messages.iter().all(|m| m.path == "Foo.cs"));
    assert_eq!((messages[0].line, messages[0].col), (10, 5));
    assert_eq!(messages[0].code, "SA1600");
    assert_eq!(messages[0].name, "ElementsMustBeDocumented");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].description, "Missing XML comment for Foo.");
    assert_eq!(messages[1].severity, "error");
    assert_eq!(messages[2].severity, "advice");
    assert_eq!(messages[3].severity, "autofix");
    assert_eq!(messages[4].severity, "disabled");
    assert_eq!(messages[5].severity, "advice");
}

#[test]
fn tool_failure_and_invalid_json_yield_no_messages() {
    assert!(parse_output("Foo.cs", 1, RESULTS, "").is_empty());
    assert!(parse_output("Foo.cs", 0, "", "").is_empty());
    assert!(parse_output("Foo.cs", 0, "not json", "").is_empty());
    assert!(parse_output("Foo.cs", 0, "{\"a\":1}", "").is_empty());
}

#[test]
fn vsprintf_renders_common_conversions() {
    let params = ["a".to_string(), "b".to_string(), "42".to_string()];
    assert_eq!(
        vsprintf("Missing %s for %s.", &params[..2]).as_deref(),
        Some("Missing a for b.")
    );
    assert_eq!(vsprintf("%s=%d!", &params[1..]).as_deref(), Some("b=42!"));
    assert_eq!(vsprintf("100%%", &[]).as_deref(), Some("100%"));
    assert_eq!(
        vsprintf("%2$s then %1$s", &params).as_deref(),
        Some("b then a")
    );
    assert_eq!(
        vsprintf("%.2f", &["3.14159".to_string()]).as_deref(),
        Some("3.14")
    );
    assert_eq!(vsprintf("%x", &["255".to_string()]).as_deref(), Some("ff"));
    assert_eq!(vsprintf("trailing%", &[]).as_deref(), Some("trailing%"));
}

#[test]
fn vsprintf_declines_what_it_cannot_render() {
    assert_eq!(vsprintf("%s %s", &["only".to_string()]), None);
    assert_eq!(vsprintf("%q", &["x".to_string()]), None);
    assert_eq!(vsprintf("%5d", &["3".to_string()]), None);
    assert_eq!(vsprintf("%.2s", &["abc".to_string()]), None);
    assert_eq!(vsprintf("%*s", &["1".to_string(), "x".to_string()]), None);
}

#[test]
fn unrenderable_formats_fall_back_to_the_raw_message() {
    let stdout = r#"[{"FileName":"F.cs","Issues":[
      {"LineNumber":1,"Column":1,"Index":{"Code":"C","Name":"N","Message":"needs %s and %s","Severity":2},"Parameters":["one"]}
    ]}]"#;
    let messages = parse_output("F.cs", 0, stdout, "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "needs %s and %s");
}
