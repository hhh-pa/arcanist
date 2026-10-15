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
            "arc-rs-lint-php-tests-{tag}-{}-{n}",
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
    assert_eq!(PhpLinter::new().name(), "PHP");
    assert_eq!(binary_name(), "php");
    assert_eq!(command_flags(), vec!["-l".to_string()]);
}

#[test]
fn supports_php_extensions_case_insensitively() {
    assert!(is_supported("a.php"));
    assert!(is_supported("a.phtml"));
    assert!(is_supported("a.inc"));
    assert!(is_supported("win\\a.PHP"));
    for path in ["a.php.bak", "a.js", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = PhpLinter::new();
    assert!(
        linter
            .lint_bytes("notes.txt", b"<?php echo 1;\n")
            .is_empty()
    );
    assert!(linter.lint_bytes("a.php", b"<?php \0 echo;\n").is_empty());
}

#[test]
fn absent_or_broken_binary_reports_nothing() {
    assert!(lint_with_binary("a.php", b"<?php echo 1;\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-php");
    assert!(lint_with_binary("a.php", b"<?php echo 1;\n", Some(&missing)).is_empty());
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
fn parses_parse_errors_from_either_stream() {
    let stderr = "PHP Parse error:  syntax error, unexpected '}' in /tmp/src.php on line 4\n";
    let messages = parse_output("src/a.php", 255, "", stderr);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "src/a.php");
    assert_eq!((messages[0].line, messages[0].col), (4, 1));
    assert_eq!(messages[0].code, "PHP1");
    assert_eq!(messages[0].name, "Parse Error");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "syntax error, unexpected '}'");

    // Older PHP prints the diagnostic to stdout instead.
    let messages = parse_output("src/a.php", 255, stderr, "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHP1");
}

#[test]
fn parses_fatal_errors_without_a_php_prefix() {
    let stderr = "Fatal error: Cannot redeclare f() in /tmp/x.php on line 7\n";
    let messages = parse_output("x.php", 255, "", stderr);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHP2");
    assert_eq!(messages[0].name, "Fatal Error");
    assert_eq!(messages[0].line, 7);
    assert_eq!(messages[0].description, "Cannot redeclare f()");
}

#[test]
fn splits_greedily_and_reports_only_the_first_error() {
    let stderr = "PHP Parse error:  foo in bar in /tmp/x.php on line 3\n\
        PHP Fatal error:  second in /tmp/x.php on line 9\n";
    let messages = parse_output("x.php", 255, "", stderr);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "foo in bar");
    assert_eq!(messages[0].line, 3);
}

#[test]
fn clean_output_and_unknown_types_yield_no_messages() {
    assert!(parse_output("a.php", 0, "No syntax errors detected in /tmp/a.php\n", "").is_empty());
    assert!(parse_output("a.php", 0, "", "").is_empty());
    let stderr = "PHP Warning error:  something odd in /tmp/a.php on line 1\n";
    assert!(parse_output("a.php", 1, "", stderr).is_empty());
}

#[test]
fn tolerates_crlf_line_endings() {
    let stderr = "PHP Parse error:  oops in /tmp/a.php on line 2\r\n";
    let messages = parse_output("a.php", 255, "", stderr);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 2);
}
