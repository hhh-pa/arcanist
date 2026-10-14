use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn empty_options() -> Options {
    Options {
        messages: Vec::new(),
        show: false,
        revision: None,
        conduit_uri: None,
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_options(&args(&[])), Ok(Some(empty_options())));
}

#[test]
fn parses_message_forms() {
    let mut expected = empty_options();
    expected.messages = vec!["one".to_string()];
    assert_eq!(
        parse_options(&args(&["-m", "one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options(&args(&["--message", "one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options(&args(&["--message=one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(parse_options(&args(&["-mone"])), Ok(Some(expected)));
}

#[test]
fn messages_are_repeatable() {
    let options = parse_options(&args(&["-m", "one", "-m", "two"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.messages, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn parses_revision_and_conduit_uri() {
    let options = parse_options(&args(&["--revision", "D123"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.revision, Some("D123".to_string()));
    let options = parse_options(&args(&["--revision=123"])).unwrap().unwrap();
    assert_eq!(options.revision, Some("123".to_string()));
    let options = parse_options(&args(&["--conduit-uri", "http://x/"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
}

#[test]
fn help_returns_none() {
    assert_eq!(parse_options(&args(&["--help"])), Ok(None));
    assert_eq!(parse_options(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    // Amend rewrites the message only; staging flags are not accepted.
    let err = parse_options(&args(&["--all"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options(&args(&["file.txt"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(parse_options(&args(&["-m"])).is_err());
    assert!(parse_options(&args(&["--revision"])).is_err());
    assert!(parse_options(&args(&["--conduit-uri"])).is_err());
}

#[test]
fn parses_revision_ids() {
    assert_eq!(parse_revision_id("123"), Ok(123));
    assert_eq!(parse_revision_id("D123"), Ok(123));
    assert_eq!(parse_revision_id("d7"), Ok(7));
    for bad in ["", "D", "D0", "0", "abc", "D12x", "-3"] {
        assert!(parse_revision_id(bad).is_err(), "accepted: {bad:?}");
    }
}

#[test]
fn normalizes_messages() {
    assert_eq!(normalize_message("hello"), Ok("hello\n".to_string()));
    assert_eq!(
        normalize_message("  hello  \n\n"),
        Ok("hello\n".to_string())
    );
    assert!(normalize_message("").is_err());
    assert!(normalize_message("   \n").is_err());
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("usage: arc-rs amend"));
    assert!(text.contains("--revision") && text.contains("--show"));
    assert!(text.contains("history.immutable"));
}
