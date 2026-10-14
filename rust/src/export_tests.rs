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
fn parses_formats_and_local_base() {
    let opts = args_of(&["--git"]);
    assert_eq!(opts.format, Format::Git);
    assert_eq!(opts.source, Source::Local { base: None });

    let opts = args_of(&["--unified", "HEAD~2"]);
    assert_eq!(opts.format, Format::Unified);
    assert_eq!(
        opts.source,
        Source::Local {
            base: Some("HEAD~2".to_string())
        }
    );
}

#[test]
fn parses_conduit_sources() {
    assert_eq!(
        args_of(&["--git", "--revision", "D5"]).source,
        Source::Revision(5)
    );
    assert_eq!(
        args_of(&["--unified", "--diff=11"]).source,
        Source::Diff(11)
    );
    let opts = args_of(&["--git", "--diff", "3", "--conduit-uri", "https://x/"]);
    assert_eq!(opts.conduit_uri, Some("https://x/".to_string()));
}

#[test]
fn requires_exactly_one_format() {
    let ParseOutcome::Error(message) = parse_args(&argv(&[])) else {
        panic!("expected error for no format");
    };
    assert!(message.contains("Specify one of"), "{message}");

    let ParseOutcome::Error(message) = parse_args(&argv(&["--git", "--unified"])) else {
        panic!("expected error for two formats");
    };
    assert!(message.contains("not compatible"), "{message}");
}

#[test]
fn rejects_mixed_sources_and_stray_positionals() {
    let ParseOutcome::Error(message) =
        parse_args(&argv(&["--git", "--revision", "1", "--diff", "2"]))
    else {
        panic!("expected error for mixed sources");
    };
    assert!(message.contains("not compatible"), "{message}");

    for words in [
        &["--git", "--revision", "1", "HEAD"][..],
        &["--git", "a", "b"][..],
    ] {
        assert!(
            matches!(parse_args(&argv(words)), ParseOutcome::Error(_)),
            "{words:?}"
        );
    }
}

#[test]
fn defers_unported_flags() {
    for flag in ["--arcbundle", "--encoding"] {
        let ParseOutcome::NotYetPorted(message) = parse_args(&argv(&[flag, "--git"])) else {
            panic!("expected not-yet-ported for {flag}");
        };
        assert!(message.contains("not yet ported"), "{message}");
    }
}

#[test]
fn normalizes_revision_ids() {
    assert_eq!(normalize_revision_id("D123").unwrap(), 123);
    assert_eq!(normalize_revision_id("9").unwrap(), 9);
    assert!(normalize_revision_id("D").is_err());
}

#[test]
fn finds_first_diff_id() {
    let dict = serde_json::json!({"3": {"id": 3}});
    assert_eq!(first_diff_id(&dict).unwrap(), "3");
    assert!(first_diff_id(&serde_json::json!({})).is_err());
}
