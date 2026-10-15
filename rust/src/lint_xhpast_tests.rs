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

fn canned(source: &str) -> String {
    xhp_ast::tokens_to_canned(source, &xhp_ast::tokenize(source))
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

#[test]
fn rule_table_covers_all_ported_ids() {
    assert_eq!(RULE_TABLE.len(), 108);
    let mut ids: Vec<u32> = RULE_TABLE.iter().map(|(id, _, _)| *id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 108);
    for missing in [11, 14, 29, 31, 33, 35, 89, 93, 94] {
        assert!(!ids.contains(&missing), "{missing}");
    }
    assert_eq!(rule_meta(42), Some(("`elseif` Usage", "advice")));
    assert_eq!(rule_meta(1), Some(("PHP Syntax Error!", "error")));
    assert_eq!(rule_meta(2), Some(("Unable to Parse", "warning")));
    assert_eq!(rule_meta(16), Some(("TODO Comment", "disabled")));
    assert_eq!(rule_meta(999), None);
    for (id, _, _) in RULE_TABLE {
        assert!((1..=134).contains(id), "{id}");
    }
}

#[test]
fn lint_canned_runs_rule_tree_with_exact_messages() {
    let options = xhp_ast::XhpOptions::default();
    let messages = lint_canned(
        "src/a.php",
        &canned("<?php if ($a) {\n} elseif ($b) {\n}\n"),
        &options,
    );
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.path, "src/a.php");
    assert_eq!((message.line, message.col), (2, 3));
    assert_eq!(message.code, "XHP42");
    assert_eq!(message.severity, "advice");
    assert_eq!(message.name, "`elseif` Usage");
    assert_eq!(
        message.description,
        "Usage of `else if` is preferred over `elseif`."
    );
    assert_eq!(message.original, None);
    assert_eq!(message.replacement, None);
}

#[test]
fn lint_canned_rejects_garbage_with_unable_to_parse() {
    let options = xhp_ast::XhpOptions::default();
    let messages = lint_canned("a.php", "not json", &options);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "XHP2");
    assert_eq!((messages[0].line, messages[0].col), (0, 0));
}

#[test]
fn parse_output_zero_exit_runs_canned_rules() {
    let messages = parse_output("a.php", 0, &canned("<?php elseif ($x) {}\n"), "");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "XHP42");
}

#[test]
fn parse_output_syntax_stderr_yields_xhp1_at_line() {
    let messages = parse_output(
        "a.php",
        1,
        "",
        "XHPAST Parse Error: syntax error, unexpected T_STRING on line 9",
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "XHP1");
    assert_eq!(messages[0].name, "PHP Syntax Error!");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (9, 1));
    assert!(
        messages[0]
            .description
            .starts_with("This file contains a syntax error: "),
        "{}",
        messages[0].description
    );
}

#[test]
fn options_map_fans_out_to_rule_tree() {
    let mut map = std::collections::HashMap::new();
    map.insert("xhpast.php-version".to_string(), serde_json::json!("5.3.0"));
    map.insert(
        "xhpast.blacklisted.function".to_string(),
        serde_json::json!({"foo": "do not use foo"}),
    );
    map.insert(
        "xhpast.printf-functions".to_string(),
        serde_json::json!({"myprintf": 1, "other": "2"}),
    );
    map.insert(
        "xhpast.dynamic-string.functions".to_string(),
        serde_json::json!({"q": 0}),
    );
    let options = xhp_ast::XhpOptions::from_map(&map);
    assert_eq!(options.php_version.as_deref(), Some("5.3.0"));
    assert_eq!(
        options.blacklisted,
        vec![("foo".to_string(), "do not use foo".to_string())]
    );
    assert!(options.printf.contains(&("myprintf".to_string(), 1)));
    assert!(options.printf.contains(&("other".to_string(), 2)));
    assert_eq!(options.dynamic_functions.len(), 1);
    assert!(options.php_version_windows.is_none());
}

#[test]
fn rule_options_are_accepted_and_short_codes_look_up() {
    let ctx = OptionContext {
        project_root: Path::new("."),
    };
    let mut linter = XhpastLinter::new();
    for (key, value) in [
        ("xhpast.php-version", serde_json::json!("5.3.0")),
        ("xhpast.php-version.windows", serde_json::json!("5.4.0")),
        ("xhpast.blacklisted.function", serde_json::json!({})),
        ("xhpast.deprecated.functions", serde_json::json!({})),
        ("xhpast.printf-functions", serde_json::json!({})),
        ("xhpast.switchhook", serde_json::json!("hook")),
        ("xhpast.naminghook", serde_json::json!("hook")),
        ("xhpast.dynamic-string.classes", serde_json::json!({})),
        ("xhpast.dynamic-string.functions", serde_json::json!({})),
    ] {
        linter.set_option(key, &value, &ctx).unwrap();
    }
    assert!(
        linter
            .set_option("bin", &serde_json::json!("x"), &ctx)
            .is_err()
    );
    assert_eq!(linter.severity_lookup_code("XHP2"), "2");
    assert_eq!(linter.severity_lookup_code("XHP"), "");
    assert_eq!(linter.severity_lookup_code("OTHER"), "OTHER");
}
