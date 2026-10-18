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
            "arc-rs-lint-golint-tests-{tag}-{}-{n}",
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

#[test]
fn linter_name_binary_and_flags_are_exact() {
    assert_eq!(GolintLinter::new().name(), "GOLINT");
    assert_eq!(binary_name(), "golint");
    assert!(command_flags().is_empty());
}

#[test]
fn supports_only_go_files() {
    assert!(is_supported("main.go"));
    assert!(is_supported("src/pkg/Foo.GO"));
    assert!(is_supported("win\\a.go"));
    for path in ["a.g old", "a.g", "a.php", "go", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = GolintLinter::new();
    assert!(linter.lint_bytes("notes.txt", b"package main\n").is_empty());
    assert!(
        linter
            .lint_bytes("main.go", b"package \0 main\n")
            .is_empty()
    );
}

#[test]
fn absent_or_broken_binary_reports_nothing() {
    assert!(lint_with_binary("main.go", b"package main\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-golint");
    assert!(lint_with_binary("main.go", b"package main\n", Some(&missing)).is_empty());
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
fn parses_findings_with_advice_severity() {
    let stdout = "/tmp/src.go:7:6: exported type Exported should have comment\n\
        /tmp/src.go:12:12: if block ends with a return statement, so drop this else\n\
        not a finding\n\
        \n";
    let messages = parse_output("main.go", 0, stdout, "");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].path, "main.go");
    assert_eq!((messages[0].line, messages[0].col), (7, 6));
    assert_eq!(messages[0].code, "GOLINT");
    assert_eq!(messages[0].name, "GOLINT");
    assert_eq!(messages[0].severity, "advice");
    assert_eq!(
        messages[0].description,
        "Exported type Exported should have comment"
    );
    assert_eq!((messages[1].line, messages[1].col), (12, 12));
}

#[test]
fn tool_failure_and_empty_output_yield_no_messages() {
    let stdout = "/tmp/src.go:7:6: exported type Exported should have comment\n";
    assert!(parse_output("main.go", 1, stdout, "").is_empty());
    assert!(parse_output("main.go", 0, "", "").is_empty());
    assert!(parse_output("main.go", 0, "short:line\n", "").is_empty());
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

fn set_bin(linter: &mut GolintLinter, ctx: &OptionContext, tool: &Path) {
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            ctx,
        )
        .unwrap();
}

#[test]
fn trailing_dot_has_no_extension() {
    assert_eq!(extension_of("foo."), None);
    assert!(!is_supported("foo."));
}

#[test]
fn staged_name_falls_back_to_go() {
    assert_eq!(staged_name("Makefile"), "src.go");
    assert_eq!(staged_name("archive.verylongextension"), "src.go");
    assert_eq!(staged_name("odd.b-c"), "src.go");
    assert_eq!(staged_name("a.go"), "src.go");
    assert_eq!(staged_name("A.GO"), "src.GO");
}

#[test]
fn binary_lookup_skips_empty_path_entries() {
    let tmp = TempDir::new("lookup-empty");
    let marker = tmp.path().join(binary_name());
    std::fs::write(&marker, b"x").unwrap();
    let both = std::env::join_paths([std::path::Path::new(""), tmp.path()]).unwrap();
    assert_eq!(binary_in(&both), Some(marker));
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-golint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = GolintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("flags", &serde_json::json!(["--extra"]), &ctx)
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(1), &ctx)
            .is_err()
    );
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_configured_tool_and_parses_stdout() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-golint",
        "#!/bin/sh\necho '/tmp/src.go:7:6: exported type Exported should have comment'\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = GolintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    let messages = linter.lint_bytes("main.go", b"package main\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (7, 6));
    assert_eq!(messages[0].severity, "advice");
}

#[test]
#[cfg(unix)]
fn version_requirement_keeps_the_linter_inactive() {
    // GoLint defines no `getVersion()`: any requirement gates the run.
    let tmp = TempDir::new("version");
    let tool = write_script(
        tmp.path(),
        "fake-golint",
        "#!/bin/sh\necho '/tmp/src.go:7:6: something'\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = GolintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("main.go", b"package main\n").is_empty());
}

#[test]
fn ucfirst_uppercases_only_the_first_character() {
    assert_eq!(ucfirst(""), "");
    assert_eq!(ucfirst("aBC"), "ABC");
    assert_eq!(ucfirst(" already"), " already");
    // PHP `ucfirst()` is byte-based: non-ASCII initials are untouched.
    assert_eq!(ucfirst("école"), "école");
    assert_eq!(ucfirst("ßeta"), "ßeta");
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("temp-fail");
    let tool = tmp.path().join("fake-golint");
    std::fs::write(&tool, b"x").unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut linter = GolintLinter {
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    linter.options.bin = Some(tool);
    assert!(linter.lint_bytes("main.go", b"package main").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}
