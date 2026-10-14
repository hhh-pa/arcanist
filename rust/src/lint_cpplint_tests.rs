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
