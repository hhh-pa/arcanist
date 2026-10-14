//! Public-behavior tests for bundle C JavaScript/CSS linters through the
//! wired library. The engine surface beyond the trait and message types is
//! exercised by `lint_text.rs`.

use std::path::{Path, PathBuf};

use arc_rs::{
    lint::Linter, lint_closure, lint_coffeelint, lint_csslint, lint_jscs, lint_jshint,
    lint_jsonlint, lint_lessc,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("arc-rs-lintC-{tag}-{pid}-{n}"));
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

fn read_fixture(rel: &str) -> Vec<u8> {
    std::fs::read(Path::new("tests/fixtures/lintC").join(rel)).unwrap()
}

fn read_fixture_text(rel: &str) -> String {
    String::from_utf8(read_fixture(rel)).unwrap()
}

/// A fake external tool: a script that records its argv, replays canned
/// stdout/stderr, and exits with a fixed status.
struct FakeTool {
    dir: TempDir,
    script: PathBuf,
}

impl FakeTool {
    fn script(&self) -> &Path {
        &self.script
    }

    fn args_text(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("args.txt")).unwrap_or_default()
    }
}

fn fake_tool(tag: &str, stdout_text: &str, stderr_text: &str, exit_code: i32) -> FakeTool {
    let dir = TempDir::new(tag);
    std::fs::write(dir.path().join("out.txt"), stdout_text).unwrap();
    std::fs::write(dir.path().join("err.txt"), stderr_text).unwrap();
    let script = write_fake_script(
        &dir,
        !stdout_text.is_empty(),
        !stderr_text.is_empty(),
        exit_code,
    );
    FakeTool { dir, script }
}

#[cfg(windows)]
fn write_fake_script(
    dir: &TempDir,
    emit_stdout: bool,
    emit_stderr: bool,
    exit_code: i32,
) -> PathBuf {
    let path = dir.path().join("tool.bat");
    let out = dir.path().join("out.txt").display().to_string();
    let err = dir.path().join("err.txt").display().to_string();
    let args = dir.path().join("args.txt").display().to_string();
    let mut body = format!("@echo off\r\necho %* > \"{args}\"\r\n");
    if emit_stdout {
        body.push_str(&format!("type \"{out}\"\r\n"));
    }
    if emit_stderr {
        body.push_str(&format!("type \"{err}\" 1>&2\r\n"));
    }
    body.push_str(&format!("exit /b {exit_code}\r\n"));
    std::fs::write(&path, body).unwrap();
    path
}

#[cfg(not(windows))]
fn write_fake_script(
    dir: &TempDir,
    emit_stdout: bool,
    emit_stderr: bool,
    exit_code: i32,
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path().join("tool.sh");
    let out = dir.path().join("out.txt").display().to_string();
    let err = dir.path().join("err.txt").display().to_string();
    let args = dir.path().join("args.txt").display().to_string();
    let mut body = format!("#!/bin/sh\necho \"$@\" > \"{args}\"\n");
    if emit_stdout {
        body.push_str(&format!("cat \"{out}\"\n"));
    }
    if emit_stderr {
        body.push_str(&format!("cat \"{err}\" >&2\n"));
    }
    body.push_str(&format!("exit {exit_code}\n"));
    std::fs::write(&path, &body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn registry_names_are_exact() {
    assert_eq!(lint_jshint::JshintLinter::new().name(), "JSHint");
    assert_eq!(lint_jscs::JscsLinter::new().name(), "JSCS");
    assert_eq!(lint_jsonlint::JsonlintLinter::new().name(), "JSON");
    assert_eq!(lint_csslint::CsslintLinter::new().name(), "CSSLint");
    assert_eq!(lint_lessc::LesscLinter::new().name(), "LESSC");
    assert_eq!(lint_closure::ClosureLinter::new().name(), "GJSLINT");
    assert_eq!(lint_coffeelint::CoffeelintLinter::new().name(), "COFFEE");
}

#[test]
fn linters_skip_non_matching_extensions() {
    assert!(
        lint_jshint::JshintLinter::new()
            .lint_bytes("a.txt", b"var x = 1;\n")
            .is_empty()
    );
    assert!(
        lint_jscs::JscsLinter::new()
            .lint_bytes("a.txt", b"var x = 1;\n")
            .is_empty()
    );
    assert!(
        lint_jsonlint::JsonlintLinter::new()
            .lint_bytes("a.txt", b"not json")
            .is_empty()
    );
    assert!(
        lint_csslint::CsslintLinter::new()
            .lint_bytes("a.txt", b".a { color: red; }\n")
            .is_empty()
    );
    assert!(
        lint_lessc::LesscLinter::new()
            .lint_bytes("a.css", b"@x: 1;\n")
            .is_empty()
    );
    assert!(
        lint_closure::ClosureLinter::new()
            .lint_bytes("a.txt", b"var x = 1;\n")
            .is_empty()
    );
    assert!(
        lint_coffeelint::CoffeelintLinter::new()
            .lint_bytes("a.js", b"x = 1\n")
            .is_empty()
    );
}

#[test]
fn linters_skip_binary_data_without_running_the_tool() {
    // Each fake would emit a finding if spawned; NUL bytes must win.
    let data = b"var x = 1;\0\n";
    let jshint = fake_tool(
        "bin-jshint",
        r#"[{"line":1,"col":1,"reason":"x","code":"E001"}]"#,
        "",
        0,
    );
    assert!(
        lint_jshint::JshintLinter::with_binary(jshint.script())
            .lint_bytes("a.js", data)
            .is_empty()
    );
    let jscs = fake_tool("bin-jscs", "<checkstyle></checkstyle>", "", 0);
    assert!(
        lint_jscs::JscsLinter::with_binary(jscs.script())
            .lint_bytes("a.js", data)
            .is_empty()
    );
    let jsonlint = fake_tool("bin-jsonlint", "", "line 1, col 1, boom", 0);
    assert!(
        lint_jsonlint::JsonlintLinter::with_binary(jsonlint.script())
            .lint_bytes("a.json", data)
            .is_empty()
    );
    let csslint = fake_tool("bin-csslint", "<lint></lint>", "", 0);
    assert!(
        lint_csslint::CsslintLinter::with_binary(csslint.script())
            .lint_bytes("a.css", data)
            .is_empty()
    );
    let lessc = fake_tool("bin-lessc", "", "NameError: x in f on line 1, column 1:", 0);
    assert!(
        lint_lessc::LesscLinter::with_binary(lessc.script())
            .lint_bytes("a.less", data)
            .is_empty()
    );
    let closure = fake_tool("bin-closure", "Line 1, E:0001: x", "", 0);
    assert!(
        lint_closure::ClosureLinter::with_binary(closure.script())
            .lint_bytes("a.js", data)
            .is_empty()
    );
    let coffee = fake_tool("bin-coffee", r#"{"a.coffee": []}"#, "", 0);
    assert!(
        lint_coffeelint::CoffeelintLinter::with_binary(coffee.script())
            .lint_bytes("a.coffee", data)
            .is_empty()
    );
}

#[test]
fn jshint_runs_tool_and_parses_report() {
    let canned = read_fixture_text("canned/jshint.json");
    let tool = fake_tool("run-jshint", &canned, "", 2);
    let input = read_fixture("src/sample.js");
    let messages =
        lint_jshint::JshintLinter::with_binary(tool.script()).lint_bytes("sample.js", &input);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].code, "W032");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!((messages[0].line, messages[0].col), (3, 9));
    assert_eq!(messages[1].severity, "disabled");
    assert_eq!(messages[2].severity, "error");
    let args = tool.args_text();
    assert!(args.contains("--reporter="), "args: {args}");
    assert!(args.contains(".js"), "args: {args}");
}

#[test]
fn jscs_runs_tool_and_parses_checkstyle() {
    let canned = read_fixture_text("canned/jscs.xml");
    let tool = fake_tool("run-jscs", &canned, "", 1);
    let input = read_fixture("src/sample.js");
    let messages =
        lint_jscs::JscsLinter::with_binary(tool.script()).lint_bytes("sample.js", &input);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "JSCS");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[1].severity, "warning");
    let args = tool.args_text();
    assert!(args.contains("--reporter=checkstyle"), "args: {args}");
    assert!(args.contains("--no-colors"), "args: {args}");
    assert!(args.contains(".js"), "args: {args}");
}

#[test]
fn jsonlint_runs_tool_and_parses_compact_stderr() {
    let canned = read_fixture_text("canned/jsonlint.txt");
    let tool = fake_tool("run-jsonlint", "", &canned, 1);
    let input = read_fixture("src/invalid.json");
    let messages =
        lint_jsonlint::JsonlintLinter::with_binary(tool.script()).lint_bytes("sample.json", &input);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "JSON");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (1, 15));
    let args = tool.args_text();
    assert!(args.contains("--compact"), "args: {args}");
    assert!(args.contains(".json"), "args: {args}");
}

#[test]
fn csslint_runs_tool_and_parses_lint_xml() {
    let canned = read_fixture_text("canned/csslint.xml");
    let tool = fake_tool("run-csslint", &canned, "", 0);
    let input = read_fixture("src/sample.css");
    let messages =
        lint_csslint::CsslintLinter::with_binary(tool.script()).lint_bytes("sample.css", &input);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "CSSLint");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (1, 5));
    assert_eq!(messages[1].severity, "warning");
    let args = tool.args_text();
    assert!(args.contains("--format=lint-xml"), "args: {args}");
    assert!(args.contains(".css"), "args: {args}");
}

#[test]
fn lessc_runs_tool_and_parses_lint_stderr() {
    let canned = read_fixture_text("canned/lessc.txt");
    let tool = fake_tool("run-lessc", "", &canned, 1);
    let input = read_fixture("src/sample.less");
    let messages =
        lint_lessc::LesscLinter::with_binary(tool.script()).lint_bytes("sample.less", &input);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "LESSCSyntaxError");
    assert_eq!(messages[0].name, "Syntax Error");
    assert_eq!((messages[0].line, messages[0].col), (5, 3));
    assert_eq!(messages[1].code, "LESSCNameError");
    let args = tool.args_text();
    for flag in [
        "--lint",
        "--no-color",
        "--strict-math=off",
        "--strict-units=off",
    ] {
        assert!(args.contains(flag), "args: {args}");
    }
    assert!(args.contains(".less"), "args: {args}");
}

#[test]
fn closure_runs_tool_with_bare_path_and_parses_stdout() {
    let canned = read_fixture_text("canned/gjslint.txt");
    let tool = fake_tool("run-closure", &canned, "", 2);
    let input = read_fixture("src/sample.js");
    let messages =
        lint_closure::ClosureLinter::with_binary(tool.script()).lint_bytes("sample.js", &input);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "GJSLINT0001");
    assert_eq!(messages[0].line, 3);
    assert_eq!(messages[1].code, "GJSLINT0213");
    let args = tool.args_text();
    assert!(!args.contains("--"), "args: {args}");
    assert!(args.contains(".js"), "args: {args}");
}

#[test]
fn coffeelint_runs_tool_and_parses_raw_json() {
    let canned = read_fixture_text("canned/coffeelint.json");
    let tool = fake_tool("run-coffee", &canned, "", 1);
    let input = read_fixture("src/sample.coffee");
    let messages = lint_coffeelint::CoffeelintLinter::with_binary(tool.script())
        .lint_bytes("sample.coffee", &input);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "COFFEE");
    assert_eq!(messages[0].name, "No Trailing Semicolons");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].line, 2);
    assert_eq!(messages[1].severity, "warning");
    let args = tool.args_text();
    assert!(args.contains("--reporter=raw"), "args: {args}");
    assert!(args.contains(".coffee"), "args: {args}");
}

#[test]
fn absent_tools_are_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lintC-no-such-tool");
    assert!(
        lint_jshint::JshintLinter::with_binary(&missing)
            .lint_bytes("a.js", b"var x = 1;\n")
            .is_empty()
    );
    assert!(
        lint_jscs::JscsLinter::with_binary(&missing)
            .lint_bytes("a.js", b"var x = 1;\n")
            .is_empty()
    );
    assert!(
        lint_csslint::CsslintLinter::with_binary(&missing)
            .lint_bytes("a.css", b".a {}\n")
            .is_empty()
    );
    assert!(
        lint_lessc::LesscLinter::with_binary(&missing)
            .lint_bytes("a.less", b"@x: 1;\n")
            .is_empty()
    );
    assert!(
        lint_closure::ClosureLinter::with_binary(&missing)
            .lint_bytes("a.js", b"var x = 1;\n")
            .is_empty()
    );
    assert!(
        lint_coffeelint::CoffeelintLinter::with_binary(&missing)
            .lint_bytes("a.coffee", b"x = 1\n")
            .is_empty()
    );
    // JSONLint instead falls back to a native check: valid JSON stays silent.
    assert!(
        lint_jsonlint::JsonlintLinter::with_binary(&missing)
            .lint_bytes("a.json", b"{\"ok\": true}\n")
            .is_empty()
    );
}

#[test]
fn jsonlint_falls_back_to_native_check_without_the_tool() {
    let missing = std::env::temp_dir().join("arc-rs-lintC-no-such-jsonlint");
    let linter = lint_jsonlint::JsonlintLinter::with_binary(&missing);
    let invalid = read_fixture("src/invalid.json");
    let found = linter.lint_bytes("invalid.json", &invalid);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "JSON1");
    assert_eq!(found[0].name, "Parse Error");
    assert_eq!(found[0].severity, "error");
    assert_eq!(found[0].line, 1);
}

#[test]
fn tool_failure_is_a_clear_error() {
    let tool = fake_tool("fail-jshint", "", "jshint: command exploded", 3);
    let messages =
        lint_jshint::JshintLinter::with_binary(tool.script()).lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert!(
        messages[0].description.contains("exit 3"),
        "{}",
        messages[0].description
    );
    assert!(
        messages[0].description.contains("command exploded"),
        "{}",
        messages[0].description
    );
}

#[test]
fn unparseable_output_is_a_clear_error() {
    let tool = fake_tool("garbage-jscs", "this is not xml", "", 0);
    let messages =
        lint_jscs::JscsLinter::with_binary(tool.script()).lint_bytes("a.js", b"var x = 1;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert!(
        messages[0].description.contains("unparseable"),
        "{}",
        messages[0].description
    );
}
