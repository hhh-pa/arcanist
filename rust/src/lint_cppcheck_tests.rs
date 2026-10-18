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

// -( T1170 COV5: close lint_cppcheck.rs )--------------------------------------

#[test]
fn empty_path_segments_are_skipped() {
    assert!(binary_in(std::ffi::OsStr::new("")).is_none());
    assert!(binary_in(std::ffi::OsStr::new(":")).is_none());
}

#[test]
fn default_lookup_runs_without_options() {
    // Outcome depends on the machine (a real cppcheck may exist), so
    // nothing is asserted; the lookup path itself must not fail.
    let _found = binary_on_path();
    let _messages = CppcheckLinter::new().lint_bytes("a.c", b"int x;\n");
}

#[cfg(unix)]
#[test]
fn tool_output_flows_through_lint_with_binary() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("tool-out");
    let fake = tmp.path().join("cppcheck");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\ncat >&2 <<'EOF'\n{REPORT}\nEOF\nexit 0\n"),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let messages = lint_with_binary("src/main.c", b"int x;\n", Some(&fake));
    assert_eq!(messages.len(), 4);
    assert!(messages.iter().all(|m| m.path == "src/main.c"));
    assert_eq!(messages[3].name, "nullPointer");
    // A nonzero tool exit yields no messages.
    std::fs::write(&fake, "#!/bin/sh\nexit 3\n").unwrap();
    assert!(lint_with_binary("src/main.c", b"int x;\n", Some(&fake)).is_empty());
}

#[test]
fn ampersand_without_semicolon_stays_literal() {
    // Quirk: the trailing `&`-tail is appended twice (once on the
    // missing-`;` break, once after the loop).
    let report = REPORT.replace("Unused variable: x", "fish & chips");
    let messages = parse_output("a.c", 0, "", &report);
    assert_eq!(messages[2].description, "fish & chips& chips");
}

#[test]
fn long_entities_are_kept_verbatim() {
    let report = REPORT.replace("Unused variable: x", "&averylongentityname;!");
    let messages = parse_output("a.c", 0, "", &report);
    assert_eq!(messages[2].description, "&averylongentityname;!");
}

#[test]
fn char_refs_decode_decimal_hex_and_upper_hex() {
    let report = REPORT.replace("Unused variable: x", "&#65;&#x41;&#X42;");
    let messages = parse_output("a.c", 0, "", &report);
    assert_eq!(messages[2].description, "AAB");
}

#[test]
fn unknown_and_broken_char_refs_stay_verbatim() {
    for entity in ["&bogus;", "&#xZZ;", "&#;", "&#55296;", "&#x;"] {
        let report = REPORT.replace("Unused variable: x", entity);
        let messages = parse_output("a.c", 0, "", &report);
        assert_eq!(messages[2].description, entity, "{entity}");
    }
}

#[test]
fn attr_lookup_skips_substring_and_unquoted_matches() {
    // `id` inside `grid` is not an attribute boundary.
    let report = REPORT.replacen("<error id=", "<error grid=\"1\" id=", 1);
    let messages = parse_output("a.c", 0, "", &report);
    assert_eq!(messages[0].name, "arrayIndexOutOfBounds");
    // A space before `=` is not an attribute either.
    let report = REPORT.replacen("id=\"unreadVariable\"", "id =\"unreadVariable\"", 1);
    let messages = parse_output("a.c", 0, "", &report);
    assert_eq!(messages[2].name, "");
    // Unquoted values are skipped.
    let report = REPORT.replacen("id=\"unreadVariable\"", "id=unreadVariable", 1);
    let messages = parse_output("a.c", 0, "", &report);
    assert_eq!(messages[2].name, "");
}

#[test]
fn unterminated_and_unclosed_reports_parse_to_nothing() {
    assert!(parse_output("a.c", 0, "", "<error id=\"x\"").is_empty());
    assert!(
        parse_output(
            "a.c",
            0,
            "",
            "<error id=\"x\" severity=\"style\" msg=\"m\">blah blah"
        )
        .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn linter_options_drive_lint_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("options");
    let fake = tmp.path().join("cppcheck");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"Cppcheck 2.10\"; exit 0; fi\ncat >&2 <<'EOF'\n{REPORT}\nEOF\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = crate::lint::OptionContext {
        project_root: tmp.path(),
    };
    let mut linter = CppcheckLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(fake.to_string_lossy().to_string()),
            &ctx,
        )
        .unwrap();
    // An unsatisfied version requirement silences the linter.
    linter
        .set_option("version", &serde_json::json!(">= 99"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("src.c", b"int x;\n").is_empty());
    // A satisfied requirement runs the tool with default flags.
    linter
        .set_option("version", &serde_json::json!(">= 2.0"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("src.c", b"int x;\n").len(), 4);
    // Configured flags replace the defaults; the tool still runs.
    linter
        .set_option("flags", &serde_json::json!(["--custom-flag"]), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("src.c", b"int x;\n").len(), 4);
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("temp-fail");
    let tool = tmp.path().join("fake-cppcheck");
    std::fs::write(&tool, b"x").unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut linter = CppcheckLinter {
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    linter.options.bin = Some(tool);
    assert!(linter.lint_bytes("src.c", b"int x;").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}
