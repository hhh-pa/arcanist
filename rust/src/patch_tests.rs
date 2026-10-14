use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn args_of(words: &[&str]) -> Options {
    let ParseOutcome::Args(opts) = parse_args(&argv(words)) else {
        panic!("expected args for {words:?}");
    };
    opts
}

#[test]
fn parses_each_source() {
    assert_eq!(
        args_of(&["D123"]).source,
        Source::Revision(123),
        "positional D123"
    );
    assert_eq!(
        args_of(&["--revision", "D7"]).source,
        Source::Revision(7),
        "--revision"
    );
    assert_eq!(
        args_of(&["--revision=42"]).source,
        Source::Revision(42),
        "--revision="
    );
    assert_eq!(args_of(&["--diff", "9"]).source, Source::Diff(9), "--diff");
    assert_eq!(
        args_of(&["--patch", "fix.patch"]).source,
        Source::PatchFile("fix.patch".to_string()),
        "--patch"
    );
}

#[test]
fn parses_preview_force_and_conduit_uri() {
    let opts = args_of(&[
        "--diff",
        "3",
        "--preview",
        "--force",
        "--conduit-uri",
        "https://x/",
    ]);
    assert!(opts.preview);
    assert!(opts.force);
    assert_eq!(opts.conduit_uri, Some("https://x/".to_string()));
}

#[test]
fn compatibility_flags_are_accepted() {
    for flag in ["--nocommit", "--nobranch", "--skip-dependencies"] {
        let opts = args_of(&["D1", flag]);
        assert_eq!(opts.source, Source::Revision(1), "{flag}");
    }
}

#[test]
fn rejects_missing_and_multiple_sources() {
    let ParseOutcome::Error(message) = parse_args(&argv(&[])) else {
        panic!("expected error for no source");
    };
    assert!(message.contains("You must specify changes"), "{message}");

    let ParseOutcome::Error(message) = parse_args(&argv(&["D1", "--diff", "2"])) else {
        panic!("expected error for two sources");
    };
    assert!(message.contains("mutually exclusive"), "{message}");

    let ParseOutcome::Error(message) = parse_args(&argv(&["D1", "D2"])) else {
        panic!("expected error for two names");
    };
    assert!(message.contains("at most one"), "{message}");
}

#[test]
fn rejects_bad_ids_and_unknown_flags() {
    for words in [
        &["--revision", "abc"][..],
        &["D12x"][..],
        &["--diff", "4x"][..],
    ] {
        assert!(
            matches!(parse_args(&argv(words)), ParseOutcome::Error(_)),
            "{words:?}"
        );
    }
    let ParseOutcome::Error(message) = parse_args(&argv(&["D1", "--bogus"])) else {
        panic!("expected error for --bogus");
    };
    assert!(message.contains("unknown flag"), "{message}");
}

#[test]
fn defers_unported_flags() {
    for flag in ["--arcbundle", "--encoding", "--update"] {
        let argv = argv(&[flag]);
        let ParseOutcome::NotYetPorted(message) = parse_args(&argv) else {
            panic!("expected not-yet-ported for {flag}");
        };
        assert!(message.contains("not yet ported"), "{message}");
    }
}

#[test]
fn normalizes_revision_ids() {
    assert_eq!(normalize_revision_id("D123").unwrap(), 123);
    assert_eq!(normalize_revision_id("d123").unwrap(), 123);
    assert_eq!(normalize_revision_id("123").unwrap(), 123);
    assert!(normalize_revision_id("").is_err());
    assert!(normalize_revision_id("Dx").is_err());
}

#[test]
fn finds_first_diff_id() {
    let dict = serde_json::json!({"7": {"id": 7}, "8": {"id": "8"}});
    assert_eq!(first_diff_id(&dict).unwrap(), "7");
    let list = serde_json::json!([{"id": 42}]);
    assert_eq!(first_diff_id(&list).unwrap(), "42");
    assert!(first_diff_id(&serde_json::json!({})).is_err());
    assert!(first_diff_id(&serde_json::json!([])).is_err());
    assert!(first_diff_id(&serde_json::json!(null)).is_err());
}

#[test]
fn reads_raw_diff_shapes() {
    assert_eq!(
        raw_diff_text(&serde_json::json!("diff --git\n")),
        Some("diff --git\n".to_string())
    );
    assert_eq!(
        raw_diff_text(&serde_json::json!({"rawDiff": "x"})),
        Some("x".to_string())
    );
    assert_eq!(raw_diff_text(&serde_json::json!({"n": 1})), None);
}

#[test]
fn preview_returns_patch_unterminated() {
    assert_eq!(ensure_trailing_newline("a".to_string()), "a\n");
    assert_eq!(ensure_trailing_newline("a\n".to_string()), "a\n");
    assert_eq!(ensure_trailing_newline(String::new()), "");
}
