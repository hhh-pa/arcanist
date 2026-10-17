use super::*;

use std::path::{Path, PathBuf};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-spelling-{tag}-{}-{n}", std::process::id()));
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

fn lint(linter: &SpellingLinter, data: &str) -> Vec<LintMessage> {
    linter.lint_bytes("file.txt", data.as_bytes())
}

#[test]
fn default_wordlist_loads_fixture_subset() {
    let linter = SpellingLinter::new();
    assert_eq!(linter.rule_counts(), (40, 5));
}

#[test]
fn reports_exact_misspelling_with_shape() {
    let linter = SpellingLinter::new();
    let messages = lint(&linter, "fix the begining now\n");
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.path, "file.txt");
    assert_eq!((message.line, message.col), (1, 9));
    assert_eq!(message.code, "SPELL1");
    assert_eq!(message.severity, "warning");
    assert_eq!(message.name, "Possible Spelling Mistake");
    assert_eq!(
        message.description,
        "Possible spelling error. You wrote 'begining', but did you mean 'beginning'?"
    );
}

#[test]
fn exact_matches_are_case_insensitive_whole_words() {
    let linter = SpellingLinter::new();
    // "Teh" matches; the correct "the" and the substrings "xteh"/"tehx"
    // must not match the "teh" rule.
    let messages = lint(&linter, "Teh the xteh tehx\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert!(lint(&linter, "beginings\n").is_empty());
}

#[test]
fn partial_rules_match_overlapping_substrings() {
    let mut linter = SpellingLinter::empty();
    linter.add_partial_rule("aa", "a");
    let messages = lint(&linter, "aaa\n");
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.code == "SPELL2"));
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert_eq!((messages[1].line, messages[1].col), (1, 2));
}

#[test]
fn partial_rules_match_inside_larger_words() {
    let linter = SpellingLinter::new();
    let messages = lint(&linter, "misrecieved\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "SPELL2");
    assert_eq!((messages[0].line, messages[0].col), (1, 4));
}

#[test]
fn findings_sort_by_line_col_code() {
    let mut linter = SpellingLinter::empty();
    linter
        .add_exact_rule("zzz", "Z")
        .add_partial_rule("aaa", "A");
    let messages = lint(&linter, "zzz aaa\nzzz\n");
    assert_eq!(messages.len(), 3);
    let positions: Vec<(usize, usize, &str)> = messages
        .iter()
        .map(|m| (m.line, m.col, m.code.as_str()))
        .collect();
    assert_eq!(
        positions,
        vec![(1, 1, "SPELL1"), (1, 5, "SPELL2"), (2, 1, "SPELL1")]
    );
}

#[test]
fn custom_dictionary_replaces_identical_words() {
    let mut linter = SpellingLinter::empty();
    linter.add_exact_rule("teh", "THE");
    linter
        .load_dictionary_json(r#"{"rules": {"exact": {"teh": "the"}}}"#)
        .unwrap();
    assert_eq!(linter.rule_counts(), (1, 0));
    let messages = lint(&linter, "teh\n");
    assert_eq!(messages.len(), 1);
    assert!(messages[0].description.contains("'the'"));
}

#[test]
fn from_dictionary_json_accepts_partial_only() {
    let linter =
        SpellingLinter::from_dictionary_json(r#"{"rules": {"partial": {"xx": "yy"}}}"#).unwrap();
    assert_eq!(linter.rule_counts(), (0, 1));
}

#[test]
fn bad_dictionaries_error_clearly() {
    assert!(SpellingLinter::from_dictionary_json("{nope").is_err());
    assert!(SpellingLinter::from_dictionary_json("{}").is_err());
    assert!(SpellingLinter::from_dictionary_json(r#"{"rules": []}"#).is_err());
    assert!(SpellingLinter::from_dictionary_json(r#"{"rules": {"exact": {"w": 7}}}"#).is_err());
}

#[test]
fn non_object_sections_error_and_empty_words_skip() {
    let err = SpellingLinter::from_dictionary_json(r#"{"rules": {"exact": []}}"#).unwrap_err();
    assert_eq!(
        err,
        "invalid spelling dictionary: 'exact' must be an object"
    );
    let linter =
        SpellingLinter::from_dictionary_json(r#"{"rules": {"exact": {"": "x"}}}"#).unwrap();
    assert_eq!(linter.rule_counts(), (0, 0));
}

#[test]
fn empty_rules_skip_and_default_matches_new() {
    let mut linter = SpellingLinter::empty();
    linter.add_exact_rule("", "x");
    linter.add_partial_rule("", "x");
    assert_eq!(linter.rule_counts(), (0, 0));
    assert_eq!(
        SpellingLinter::default().rule_counts(),
        SpellingLinter::new().rule_counts()
    );
}

#[test]
fn skips_empty_binary_and_ruleless_inputs() {
    let linter = SpellingLinter::new();
    assert!(lint(&linter, "").is_empty());
    assert!(linter.lint_bytes("file.txt", b"teh\0\n").is_empty());
    let empty = SpellingLinter::empty();
    assert!(lint(&empty, "teh begining\n").is_empty());
}

#[test]
fn loads_dictionary_file_from_disk() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-lint-spelling-{}-{}",
        std::process::id(),
        "load-dict"
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("custom.json");
    std::fs::write(&path, r#"{"rules": {"exact": {"wrod": "word"}}}"#).unwrap();
    let mut linter = SpellingLinter::empty();
    linter.load_dictionary_file(&path).unwrap();
    assert_eq!(linter.rule_counts(), (1, 0));
    assert_eq!(lint(&linter, "a wrod here\n").len(), 1);
    assert!(
        linter
            .load_dictionary_file(&dir.join("missing.json"))
            .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dictionaries_option_loads_files_from_project_root() {
    let dir = std::env::temp_dir().join(format!("arc-rs-spelling-opt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("custom.json"),
        r#"{"rules": {"exact": {"teh": "the"}}}"#,
    )
    .unwrap();
    let ctx = OptionContext { project_root: &dir };
    let mut linter = SpellingLinter::empty();
    linter
        .set_option(
            "spelling.dictionaries",
            &serde_json::json!(["custom.json"]),
            &ctx,
        )
        .unwrap();
    assert_eq!(linter.rule_counts(), (1, 0));
    let messages = lint(&linter, "teh cat\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "SPELL1");
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(1), &ctx)
            .is_err()
    );
    assert!(
        linter
            .set_option("spelling.dictionaries", &serde_json::json!("x"), &ctx)
            .is_err()
    );
    assert!(
        linter
            .set_option(
                "spelling.dictionaries",
                &serde_json::json!(["nope.json"]),
                &ctx
            )
            .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dictionaries_option_rejects_non_string_entries() {
    let tmp = TempDir::new("badopt");
    let ctx = OptionContext {
        project_root: tmp.path(),
    };
    let mut linter = SpellingLinter::empty();
    let err = linter
        .set_option("spelling.dictionaries", &serde_json::json!([42]), &ctx)
        .unwrap_err();
    assert_eq!(err, "'spelling.dictionaries' must be a list of strings");
}

#[test]
fn severity_lookup_strips_the_spell_prefix() {
    let linter = SpellingLinter::new();
    assert_eq!(linter.severity_lookup_code("SPELL1"), "1");
    assert_eq!(linter.severity_lookup_code("SPELL"), "");
    assert_eq!(linter.severity_lookup_code("OTHER"), "OTHER");
}
