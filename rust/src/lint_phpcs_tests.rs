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
            "arc-rs-lint-phpcs-tests-{tag}-{}-{n}",
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
<phpcs version="3.7.1">
<file name="/tmp/staged/src.php">
 <error line="2" column="1" source="PSR1.Files.SideEffects">Found &amp; unexpected</error>
 <warning line="3" column="14" source="Squiz.WhiteSpace.SuperfluousWhitespace">Whitespace found at end of line</warning>
</file>
</phpcs>"#;

#[test]
fn linter_name_binary_and_flags_are_exact() {
    assert_eq!(PhpcsLinter::new().name(), "PHPCS");
    assert_eq!(binary_name(), "phpcs");
    assert_eq!(command_flags(), vec!["--report=xml".to_string()]);
}

#[test]
fn supports_php_extensions_case_insensitively() {
    assert!(is_supported("a.php"));
    assert!(is_supported("a.PHTML"));
    assert!(is_supported("a.Inc"));
    assert!(is_supported("win\\a.Php"));
    for path in ["a.php.bak", "a.js", "a.css", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = PhpcsLinter::new();
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
    let missing = tmp.path().join("no-such-phpcs");
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
fn parses_findings_in_document_order() {
    let messages = parse_output("src/a.php", 1, REPORT, "");
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.path == "src/a.php"));
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[0].code, "PHPCS.E.PSR1.Files.SideEffects");
    assert_eq!(messages[0].name, "PSR1.Files.SideEffects");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].description, "Found & unexpected");
    assert_eq!((messages[1].line, messages[1].col), (3, 14));
    assert_eq!(
        messages[1].code,
        "PHPCS.W.Squiz.WhiteSpace.SuperfluousWhitespace"
    );
    assert_eq!(messages[1].severity, "warning");
}

#[test]
fn clean_exit_and_invalid_xml_yield_no_messages() {
    assert!(parse_output("a.php", 0, REPORT, "").is_empty());
    assert!(parse_output("a.php", 1, "", "").is_empty());
    assert!(parse_output("a.php", 1, "not xml", "").is_empty());
    assert!(parse_output("a.php", 1, "<html></html>", "").is_empty());
}
