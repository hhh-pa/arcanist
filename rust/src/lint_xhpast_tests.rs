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
            "arc-rs-lint-xhpast-tests-{tag}-{}-{n}",
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
    assert_eq!(XhpastLinter::new().name(), "XHP");
    assert_eq!(binary_name(), "xhpast");
    assert!(command_flags().is_empty());
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
    let linter = XhpastLinter::new();
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
    let missing = tmp.path().join("no-such-xhpast");
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
fn clean_parse_yields_no_messages() {
    assert!(parse_output("a.php", 0, "", "").is_empty());
}

#[test]
fn failed_parse_yields_one_unable_to_parse_warning() {
    for exit_code in [1, 127] {
        let messages = parse_output("src/a.php", exit_code, "", "boom");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].path, "src/a.php");
        assert_eq!((messages[0].line, messages[0].col), (0, 0));
        assert_eq!(messages[0].code, "XHP2");
        assert_eq!(messages[0].name, "Unable to Parse");
        assert_eq!(messages[0].severity, "warning");
        assert!(
            messages[0]
                .description
                .contains(&format!("exit status {exit_code}")),
            "{}",
            messages[0].description
        );
    }
}
