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

#[test]
fn trailing_dot_has_no_extension() {
    assert_eq!(extension_of("foo."), None);
    assert!(!is_supported("foo."));
    assert!(!is_supported("Makefile"));
}

#[test]
fn staged_name_falls_back_to_php() {
    assert_eq!(staged_name("Makefile"), "src.php");
    assert_eq!(staged_name("archive.verylongextension"), "src.php");
    assert_eq!(staged_name("odd.b-c"), "src.php");
    assert_eq!(staged_name("a.php"), "src.php");
    assert_eq!(staged_name("A.PHTML"), "src.PHTML");
}

#[test]
fn binary_lookup_skips_empty_dirs() {
    let dir = TempDir::new("phpcs-path-miss");
    assert!(binary_in(dir.path().as_os_str()).is_none());
}

#[test]
fn windows_lookup_finds_dot_exe() {
    let dir = TempDir::new("phpcs-path-exe");
    std::fs::write(dir.path().join("phpcs.exe"), "x").unwrap();
    let found = binary_in(dir.path().as_os_str());
    if cfg!(windows) {
        assert_eq!(found, Some(dir.path().join("phpcs.exe")));
    } else {
        assert!(found.is_none());
    }
}

#[test]
fn entities_and_char_refs_decode() {
    let xml = "<phpcs><file name=\"a.php\"><error line=\"1\" column=\"1\" source=\"S.Ent\">\
        &lt;&gt;&quot;&apos;&amp;&#65;&#x42;&#X43;&#ZZ;&#xZZ;&#xD800;!</error>\
        </file></phpcs>";
    let messages = parse_output("a.php", 1, xml, "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "<>\"'&ABC&#ZZ;&#xZZ;&#xD800;!");
}

#[test]
fn malformed_entities_stay_literal() {
    let text = "a & b &bogus; &abcdefghijklm; tail &";
    let xml = format!(
        "<phpcs><file name=\"a.php\"><error line=\"1\" column=\"1\" source=\"S.Ent\">{text}</error></file></phpcs>"
    );
    let messages = parse_output("a.php", 1, &xml, "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, text);
}

#[test]
fn finding_shapes_skip_self_close_pair_and_dangle() {
    let xml = "<phpcs><file name=\"a.php\">\
        <errorx line=\"9\" source=\"Junk.Skip\"/>\
        <error line=\"2\" column=\"1\" source=\"S.Self\"/>\
        <error line=\"3\" column=\"4\" source=\"S.Paired\">paired text</error>\
        <error line=\"4\" column=\"1\" source=\"S.Dangle\">dangling</file></phpcs>";
    let messages = parse_output("a.php", 1, xml, "");
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[0].name, "S.Self");
    assert_eq!(messages[0].description, "");
    assert_eq!((messages[1].line, messages[1].col), (3, 4));
    assert_eq!(messages[1].description, "paired text");
}

#[test]
fn unterminated_finding_tag_yields_no_messages() {
    let xml = "<phpcs><file name=\"a.php\"><error line=\"5\" source=\"S.Cut</file></phpcs>";
    assert!(parse_output("a.php", 1, xml, "").is_empty());
}

#[test]
fn file_element_shapes_skip_self_close_pair_and_dangle() {
    let xml = "<phpcs><filex name=\"j\"/><file name=\"b.php\"/>\
        <file name=\"a.php\">\
        <warning line=\"1\" column=\"2\" source=\"S.W\">wtext</warning>\
        </file><file name=\"cut\">text";
    let xml = format!("{xml}</phpcs>");
    let messages = parse_output("a.php", 1, &xml, "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHPCS.W.S.W");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].description, "wtext");
}

#[test]
fn attribute_edges_reject_fallback_and_default() {
    let xml = "<phpcs><file name=\"a.php\">\
        <error xline=\"9\" line=\"7\" column=\"1\" source=\"S.R\">t</error>\
        <error liner=\"1\" line=\"8\" column=\"1\" source=\"S.R2\">t</error>\
        <error line=7 column=\"1\" source=\"S.U\">t</error>\
        <error source=\"S.M\">t</error>\
        </file></phpcs>";
    let messages = parse_output("a.php", 1, xml, "");
    let lines: Vec<usize> = messages.iter().map(|m| m.line).collect();
    assert_eq!(lines, vec![7, 8, 1, 1]);
    assert_eq!(messages[3].col, 1);
}
