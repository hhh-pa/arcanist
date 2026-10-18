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
            "arc-rs-lint-cpplint-tests-{tag}-{}-{n}",
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
    assert_eq!(CpplintLinter::new().name(), "C++ Google's Styleguide");
    assert_eq!(binary_name(), "cpplint.py");
    assert!(command_flags().is_empty());
}

#[test]
fn supports_cpp_extensions_case_insensitively() {
    for path in ["a.c", "a.h", "src/a.cpp", "a.HPP", "a.cc", "win\\a.C"] {
        assert!(is_supported(path), "{path}");
    }
    for path in ["a.go", "a.py", "a.txt", "Makefile", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = CpplintLinter::new();
    assert!(
        linter
            .lint_bytes("notes.txt", b"int main() {}\n")
            .is_empty()
    );
    assert!(linter.lint_bytes("src.cpp", b"int \0 main\n").is_empty());
}

#[test]
fn absent_or_broken_binary_reports_nothing() {
    assert!(lint_with_binary("src.cpp", b"int x;\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-cpplint");
    assert!(lint_with_binary("src.cpp", b"int x;\n", Some(&missing)).is_empty());
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
fn parses_findings_and_maps_line_zero_to_line_one() {
    let stderr = "src/foo.cc:0:  No #ifndef header guard found  [build/header_guard] [5]\n\
        src/foo.cc:2:  Include the directory when naming .h files  [build/include_subdir] [4]\n\
        Done processing src/foo.cc\n";
    let messages = parse_output("foo.cc", 1, "", stderr);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].path, "foo.cc");
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert_eq!(messages[0].code, "build/header_guard");
    assert_eq!(messages[0].name, "build/header_guard");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].description, "No #ifndef header guard found");
    assert_eq!(messages[1].line, 2);
    assert_eq!(messages[1].code, "build/include_subdir");
}

#[test]
fn skips_malformed_lines_and_empty_output() {
    assert!(parse_output("a.cc", 0, "", "").is_empty());
    let stderr = "a.cc: no brackets here\n\
        a.cc:3:  message [category] [NaN]\n\
        a.cc:3:  message without category\n";
    assert!(parse_output("a.cc", 0, "", stderr).is_empty());
}

#[test]
fn matches_php_on_unanchored_and_empty_cases() {
    // The PHP regex is unanchored: digits in the path do not confuse the
    // line split, and empty messages are accepted like PHP.
    let stderr = "v2/foo2.cc:7:  spaced message  [whitespace/x] [1]\n\
        a.cc:3:   [category] [4]\n";
    let messages = parse_output("a.cc", 0, "", stderr);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].line, 7);
    assert_eq!(messages[0].description, "spaced message");
    assert_eq!(messages[1].line, 3);
    assert_eq!(messages[1].description, "");
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

fn set_bin(linter: &mut CpplintLinter, ctx: &OptionContext, tool: &Path) {
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
fn staged_name_falls_back_to_cpp() {
    assert_eq!(staged_name("Makefile"), "src.cpp");
    assert_eq!(staged_name("archive.verylongextension"), "src.cpp");
    assert_eq!(staged_name("odd.b-c"), "src.cpp");
    assert_eq!(staged_name("a.cpp"), "src.cpp");
    assert_eq!(staged_name("A.HPP"), "src.HPP");
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
fn lines_without_a_number_are_skipped() {
    let stderr = "nodigits: message  [category] [1]\n";
    assert!(parse_output("a.cc", 0, "", stderr).is_empty());
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-cpplint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = CpplintLinter::new();
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
fn lint_bytes_runs_configured_tool_and_parses_stderr() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-cpplint",
        "#!/bin/sh\necho 'src/foo.cc:2:  Include the directory  [build/include_subdir] [4]' >&2\nexit 1\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = CpplintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    let messages = linter.lint_bytes("foo.cc", b"int x;\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "build/include_subdir");
    assert_eq!(messages[0].severity, "warning");
}

#[test]
#[cfg(unix)]
fn version_requirement_keeps_the_linter_inactive() {
    // Cpplint defines no `getVersion()`: any requirement gates the run.
    let tmp = TempDir::new("version");
    let tool = write_script(
        tmp.path(),
        "fake-cpplint",
        "#!/bin/sh\necho 'src/foo.cc:2:  m  [build/x] [4]' >&2\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = CpplintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("foo.cc", b"int x;\n").is_empty());
}

#[test]
fn severity_codes_match_the_php_category_shape() {
    let linter = CpplintLinter::new();
    for code in [
        "build/include_order",
        "whitespace/braces",
        "runtime/casting",
        "a/b2_c+d",
    ] {
        assert_eq!(linter.validate_severity_code(code), Ok(code.to_string()));
    }
    for code in [
        "",
        "build",
        "/braces",
        "build/",
        "Build/braces",
        "build/Braces",
        "a/b/c",
        "build/in clude",
    ] {
        assert_eq!(
            linter.validate_severity_code(code),
            Err(format!(
                "Unrecognized lint message code \"{code}\". Expected a valid cpplint lint code \
                 like \"build/include_order\" or \"whitespace/braces\"."
            )),
            "{code:?}"
        );
    }
}
