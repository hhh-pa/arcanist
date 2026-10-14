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
            "arc-rs-lint-cppcheck-tests-{tag}-{}-{n}",
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

const REPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<results version="2">
  <cppcheck version="2.10"/>
  <errors>
    <error id="arrayIndexOutOfBounds" severity="error" msg="Array &apos;a[10]&apos; accessed at index 10, which is out of bounds." verbose="long">
      <location file="/tmp/staged/src.c" line="4"/>
      <location file="/tmp/staged/src.c" line="3"/>
    </error>
    <error id="unreadVariable" severity="style" msg="Unused variable: x" verbose="long">
      <location file="/tmp/staged/src.c" line="2"/>
    </error>
    <error id="nullPointer" severity="warning" inconclusive="true" msg="Possible null pointer dereference: p" verbose="long">
      <location file="/tmp/staged/src.c" line="9"/>
    </error>
  </errors>
</results>"#;

#[test]
fn linter_name_binary_and_flags_are_exact() {
    assert_eq!(CppcheckLinter::new().name(), "cppcheck");
    assert_eq!(binary_name(), "cppcheck");
    assert_eq!(
        command_flags(),
        vec![
            "--quiet",
            "--inline-suppr",
            "--xml",
            "--xml-version=2",
            "-j2",
            "--enable=performance,style,portability,information",
        ]
    );
}

#[test]
fn supports_c_and_cpp_extensions_case_insensitively() {
    for path in [
        "a.c",
        "a.h",
        "src/a.cpp",
        "src/a.hpp",
        "a.cc",
        "a.hh",
        "a.cxx",
        "a.hxx",
        "a.C++",
        "a.H++",
        "a.tpp",
        "a.ipp",
        "win\\a.CPP",
    ] {
        assert!(is_supported(path), "{path}");
    }
    for path in [
        "a.go",
        "a.php",
        "a.txt",
        "a.cpp.bak",
        "Makefile",
        ".h",
        "no-extension",
        "",
    ] {
        if path == ".h" {
            // A bare ".h" has extension "h"; every other entry must skip.
            assert!(is_supported(path), "{path}");
        } else {
            assert!(!is_supported(path), "{path}");
        }
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = CppcheckLinter::new();
    assert!(
        linter
            .lint_bytes("notes.txt", b"int main() {}\n")
            .is_empty()
    );
    assert!(linter.lint_bytes("src.c", b"int \0 main\n").is_empty());
}

#[test]
fn absent_or_broken_binary_reports_nothing() {
    assert!(lint_with_binary("src.c", b"int x;\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-cppcheck");
    assert!(lint_with_binary("src.c", b"int x;\n", Some(&missing)).is_empty());
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
fn parses_each_location_with_mapped_severities() {
    let messages = parse_output("src/main.c", 0, "", REPORT);
    assert_eq!(messages.len(), 4);
    assert!(messages.iter().all(|m| m.path == "src/main.c"));
    assert_eq!(messages[0].code, "Cppcheck");
    assert_eq!(messages[0].name, "arrayIndexOutOfBounds");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (4, 1));
    assert_eq!(
        messages[0].description,
        "Array 'a[10]' accessed at index 10, which is out of bounds."
    );
    assert_eq!((messages[1].line, messages[1].col), (3, 1));
    assert_eq!(messages[2].severity, "warning");
    assert_eq!(messages[2].name, "unreadVariable");
    assert_eq!(messages[3].severity, "advice");
    assert_eq!(messages[3].name, "nullPointer");
}

#[test]
fn empty_error_list_parses_to_nothing() {
    let empty = r#"<?xml version="1.0"?><results version="2"><errors></errors></results>"#;
    assert!(parse_output("a.c", 0, "", empty).is_empty());
}

#[test]
fn tool_failure_and_garbage_yield_no_messages() {
    assert!(parse_output("a.c", 1, "", REPORT).is_empty());
    assert!(parse_output("a.c", 0, "", "not xml at all").is_empty());
    assert!(parse_output("a.c", 0, "", "").is_empty());
}

#[test]
fn staged_names_keep_safe_extensions() {
    assert_eq!(staged_name("foo/bar.cc"), "src.cc");
    assert_eq!(staged_name("weird."), "src.cpp");
    assert_eq!(staged_name("a.verylongextensionname"), "src.cpp");
}
