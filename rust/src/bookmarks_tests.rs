use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-bookmarks-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn parses_defaults() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&[]), true),
        Ok(Some(Options {
            json: false,
            conduit_uri: None,
        }))
    );
}

#[test]
fn parses_json_and_conduit_uri() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--json"]), true),
        Ok(Some(Options {
            json: true,
            conduit_uri: None,
        }))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri=https://host/api/"]), true),
        Ok(Some(Options {
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
}

#[test]
fn help_returns_none() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(parse_options_with_tty(&args(&["--help"]), true), Ok(None));
    assert_eq!(parse_options_with_tty(&args(&["-h"]), true), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    // T1153 B21/B23: TTY runs need no `--`; a lone `--` is the
    // terminator (accepted), and `--help` after `--` is positional.
    let err = parse_options_with_tty(&args(&["--bogus"]), true).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options_with_tty(&args(&["mark"]), true).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), true)
            .unwrap()
            .is_some()
    );
    let err = parse_options_with_tty(&args(&["--", "--help"]), true).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options_with_tty(&args(&["--conduit-uri"]), true).unwrap_err();
    assert!(err.contains("requires a value"), "unexpected: {err}");
}

#[test]
fn finds_hg_root_upward() {
    let dir = scratch("root");
    let nested = dir.join("a").join("b");
    std::fs::create_dir_all(nested.join(".hg")).unwrap();
    assert_eq!(find_hg_root(&nested), Some(nested.clone()));
    assert_eq!(find_hg_root(&dir.join("sibling-without-hg")), None);
    assert_eq!(find_hg_root(&dir), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_like_arc_outside_mercurial() {
    let dir = scratch("nowhere");
    let err = list(&dir).unwrap_err();
    assert!(
        err.contains("not inside a working copy"),
        "unexpected: {err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn formats_json_objects() {
    let marks = vec![Bookmark {
        name: "active".to_string(),
        active: true,
        rev: String::new(),
        node: "abc123".to_string(),
    }];
    assert_eq!(
        format_json(&marks),
        serde_json::json!([{
            "name": "active",
            "active": true,
            "rev": "",
            "node": "abc123",
        }])
    );
}

#[test]
fn usage_covers_options() {
    // T1153 B21/B23: `--help` renders the generated workflow help
    // (ceb takes no arguments, so extensions stay undocumented).
    let text = usage_text(false);
    assert!(text.contains("BOOKMARKS WORKFLOW"), "{text}");
    assert!(
        text.contains("Show an enhanced view of bookmarks"),
        "{text}"
    );
    assert!(!text.contains("--json"), "{text}");
    assert!(!text.contains("--conduit-uri"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn b21_bookmarks_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv (PHP's `parseFull` runs before
    // workflow validation). Unknown flags still report first.
    assert_eq!(
        parse_options_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--json"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    let err = parse_options_with_tty(&args(&["--bogus"]), false).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(
        parse_options_with_tty(&args(&["--json", "--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(parse_options_with_tty(&args(&[]), true).unwrap().is_some());
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn b23_bookmarks_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help bookmarks`.
    // No specs and no `__param__` markers, so no underline codes.
    let text = usage_text(false);
    assert!(text.starts_with("BOOKMARKS WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Lists bookmarks in the working copy"),
        "{text}"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
