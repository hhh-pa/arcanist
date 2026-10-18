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
#[cfg(windows)]
fn windows_lookup_finds_dot_exe() {
    let dir = TempDir::new("phpcs-path-exe");
    std::fs::write(dir.path().join("phpcs.exe"), "x").unwrap();
    let found = binary_in(dir.path().as_os_str());
    assert_eq!(found, Some(dir.path().join("phpcs.exe")));
}

#[test]
#[cfg(not(windows))]
fn windows_lookup_ignores_dot_exe() {
    let dir = TempDir::new("phpcs-path-exe");
    std::fs::write(dir.path().join("phpcs.exe"), "x").unwrap();
    assert!(binary_in(dir.path().as_os_str()).is_none());
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

fn set_bin(linter: &mut PhpcsLinter, ctx: &OptionContext, tool: &Path) {
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            ctx,
        )
        .unwrap();
}

#[test]
fn standard_feeds_the_standard_flag() {
    assert!(mandatory_flags(Some("PSR2")).contains(&"--standard=PSR2".to_string()));
    assert_eq!(mandatory_flags(None), vec!["--report=xml".to_string()]);
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
fn unterminated_file_tag_yields_no_messages() {
    assert!(parse_output("a.php", 1, "<phpcs><file name=\"a", "").is_empty());
    assert!(parse_output("a.php", 1, "<phpcs><file name=\"a</phpcs>", "").is_empty());
}

#[test]
fn set_standard_and_delegated_options() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-phpcs");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = PhpcsLinter::new();
    linter
        .set_option("phpcs.standard", &serde_json::json!("PSR2"), &ctx)
        .unwrap();
    assert_eq!(
        linter.set_option("phpcs.standard", &serde_json::json!(42), &ctx),
        Err("'phpcs.standard' must be a string".to_string())
    );
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
fn lint_bytes_runs_configured_tool_and_parses_report() {
    let tmp = TempDir::new("run");
    let one_line = REPORT.replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-phpcs",
        &format!("#!/bin/sh\nprintf '%s\\n' '{one_line}'\nexit 1\n"),
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = PhpcsLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    let messages = linter.lint_bytes("a.php", b"<?php echo 1;\n");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "PHPCS.E.PSR1.Files.SideEffects");
    assert_eq!(messages[1].severity, "warning");
}

#[test]
#[cfg(unix)]
fn lint_bytes_passes_the_configured_standard() {
    let tmp = TempDir::new("run-standard");
    let one_line = REPORT.replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-phpcs",
        &format!(
            "#!/bin/sh\nif printf '%s' \"$*\" | grep -q -- '--standard=PSR2'; then printf '%s\\n' '{one_line}'; exit 1; else exit 0; fi\n"
        ),
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = PhpcsLinter::new();
    linter
        .set_option("phpcs.standard", &serde_json::json!("PSR2"), &ctx)
        .unwrap();
    set_bin(&mut linter, &ctx, &tool);
    assert_eq!(linter.lint_bytes("a.php", b"<?php echo 1;\n").len(), 2);
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(tmp.path(), "fake-phpcs", "#!/bin/sh\necho junk\n");
    let mut linter = PhpcsLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.php", b"<?php echo 1;\n").is_empty());
    let one_line = REPORT.replace('\n', "");
    let tool = write_script(
        tmp.path(),
        "fake-phpcs-new",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'PHP_CodeSniffer version 3.7.1'; else printf '%s\\n' '{one_line}'; exit 1; fi\n"
        ),
    );
    let mut linter = PhpcsLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.php", b"<?php echo 1;\n").len(), 2);
}

#[test]
fn severity_codes_need_the_phpcs_kind_prefix() {
    let linter = PhpcsLinter::new();
    for code in ["PHPCS.E.Whitespace", "PHPCS.W.Generic"] {
        assert_eq!(linter.validate_severity_code(code), Ok(code.to_string()));
    }
    for code in ["", "PHPCS", "PHPCS.X.A", "PHPCS.E", "E.Whitespace"] {
        assert_eq!(
            linter.validate_severity_code(code),
            Err(format!(
                "Invalid severity code '{code}', should begin with 'PHPCS.'."
            )),
            "{code:?}"
        );
    }
}
