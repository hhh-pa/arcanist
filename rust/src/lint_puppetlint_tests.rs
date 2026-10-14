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
            "arc-rs-lint-puppetlint-tests-{tag}-{}-{n}",
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
    assert_eq!(PuppetlintLinter::new().name(), "PUPPETLINT");
    assert_eq!(binary_name(), "puppet-lint");
    assert_eq!(
        command_flags(),
        vec![
            "--error-level=all".to_string(),
            "--log-format=%{linenumber}|%{column}|%{kind}|%{check}|%{message}".to_string(),
        ]
    );
}

#[test]
fn supports_only_pp_files() {
    assert!(is_supported("site.pp"));
    assert!(is_supported("manifests/Init.PP"));
    assert!(is_supported("win\\a.pp"));
    for path in ["a.php", "a.p", "a.txt", "no-extension", ""] {
        assert!(!is_supported(path), "{path}");
    }
}

#[test]
fn skips_unsupported_and_binary_content_without_a_tool() {
    let linter = PuppetlintLinter::new();
    assert!(
        linter
            .lint_bytes("notes.txt", b"file { 'x': }\n")
            .is_empty()
    );
    assert!(linter.lint_bytes("site.pp", b"file \0 { }\n").is_empty());
}

#[test]
fn absent_or_broken_binary_reports_nothing() {
    assert!(lint_with_binary("site.pp", b"file { 'x': }\n", None).is_empty());
    let tmp = TempDir::new("missing");
    let missing = tmp.path().join("no-such-puppet-lint");
    assert!(lint_with_binary("site.pp", b"file { 'x': }\n", Some(&missing)).is_empty());
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
fn parses_findings_with_humanized_names_and_kinds() {
    let stdout = "3|13|warning|unquoted_file_mode|mode should be represented as a 4-digit octal value\n\
        1|1|error|hard_tabs|tab character found\n\
        2|5|autocorrect|arrow_alignment|indentation of => is not properly aligned\n\
        short|line\n\
        \n";
    let messages = parse_output("site.pp", 1, stdout, "");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].path, "site.pp");
    assert_eq!((messages[0].line, messages[0].col), (3, 13));
    assert_eq!(messages[0].code, "PUPPETLINT");
    assert_eq!(messages[0].name, "Unquoted File Mode");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(
        messages[0].description,
        "Mode should be represented as a 4-digit octal value"
    );
    assert_eq!(messages[1].severity, "error");
    assert_eq!(messages[1].name, "Hard Tabs");
    assert_eq!(messages[2].severity, "advice");
}

#[test]
fn messages_may_contain_pipes() {
    let stdout = "5|9|warning|double_quoted_strings|string contains only a | pipe\n";
    let messages = parse_output("site.pp", 0, stdout, "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "String contains only a | pipe");
}

#[test]
fn empty_output_yields_no_messages() {
    assert!(parse_output("site.pp", 0, "", "").is_empty());
}
