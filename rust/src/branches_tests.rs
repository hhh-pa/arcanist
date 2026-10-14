use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn options(all: bool, json: bool, conduit_uri: Option<&str>) -> Options {
    Options {
        all,
        json,
        conduit_uri: conduit_uri.map(str::to_string),
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(
        parse_options(&args(&[])),
        Ok(Some(options(false, false, None)))
    );
}

#[test]
fn parses_all_and_json() {
    assert_eq!(
        parse_options(&args(&["--all", "--json"])),
        Ok(Some(options(true, true, None)))
    );
}

#[test]
fn parses_conduit_uri() {
    assert_eq!(
        parse_options(&args(&["--conduit-uri", "http://x/"])),
        Ok(Some(options(false, false, Some("http://x/"))))
    );
    assert_eq!(
        parse_options(&args(&["--conduit-uri=http://x/"])),
        Ok(Some(options(false, false, Some("http://x/"))))
    );
    assert!(parse_options(&args(&["--conduit-uri"])).is_err());
    assert!(parse_options(&args(&["--conduit-uri="])).is_err());
}

#[test]
fn help_returns_none() {
    assert_eq!(parse_options(&args(&["--help"])), Ok(None));
    assert_eq!(parse_options(&args(&["-h"])), Ok(None));
    assert_eq!(parse_options(&args(&["--all", "--help"])), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options(&args(&["feature"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
}

#[test]
fn parses_ref_line_with_upstream() {
    let branch = parse_ref_line(
        "feature\x1fabc1234\x1fadd thing\x1forigin/feature\x1f[ahead 2]",
        Some("feature"),
        false,
    )
    .unwrap();
    assert_eq!(
        branch,
        Branch {
            name: "feature".to_string(),
            current: true,
            hash: "abc1234".to_string(),
            subject: "add thing".to_string(),
            upstream: Some("origin/feature".to_string()),
            track: "[ahead 2]".to_string(),
            remote: false,
            revisions: Vec::new(),
        }
    );
}

#[test]
fn parses_ref_line_without_upstream() {
    let branch = parse_ref_line(
        "main\x1fdef5678\x1ffirst commit\x1f\x1f",
        Some("main"),
        false,
    )
    .unwrap();
    assert_eq!(branch.upstream, None);
    assert_eq!(branch.track, "");
    assert!(branch.current);
    assert!(!branch.remote);
}

#[test]
fn remote_refs_are_never_current() {
    let branch = parse_ref_line(
        "origin/main\x1fdef5678\x1ffirst commit\x1f\x1f",
        Some("origin/main"),
        true,
    )
    .unwrap();
    assert!(!branch.current);
    assert!(branch.remote);
}

#[test]
fn rejects_malformed_ref_lines() {
    assert!(parse_ref_line("only-name", None, false).is_err());
    assert!(parse_ref_line("a\x1fb\x1fc\x1fd", None, false).is_err());
    assert!(parse_ref_line("a\x1fb\x1fc\x1fd\x1fe\x1fextra", None, false).is_err());
}

fn plain(name: &str) -> Branch {
    Branch {
        name: name.to_string(),
        current: false,
        hash: "abc1234".to_string(),
        subject: "subject".to_string(),
        upstream: None,
        track: String::new(),
        remote: false,
        revisions: Vec::new(),
    }
}

fn annotated(name: &str) -> Branch {
    let mut branch = plain(name);
    branch.revisions = vec![amend::AnnotatedRevision {
        monogram: "D123".to_string(),
        status: "Accepted".to_string(),
        title: "Title".to_string(),
    }];
    branch
}

#[test]
fn formats_text_with_marker_and_tracking() {
    let branches = vec![
        Branch {
            name: "main".to_string(),
            current: true,
            hash: "abc1234".to_string(),
            subject: "first".to_string(),
            upstream: None,
            track: String::new(),
            remote: false,
            revisions: Vec::new(),
        },
        Branch {
            name: "feature".to_string(),
            current: false,
            hash: "def5678".to_string(),
            subject: "add thing".to_string(),
            upstream: Some("origin/feature".to_string()),
            track: "[ahead 2]".to_string(),
            remote: false,
            revisions: Vec::new(),
        },
        Branch {
            name: "origin/main".to_string(),
            current: false,
            hash: "abc1234".to_string(),
            subject: "first".to_string(),
            upstream: None,
            track: String::new(),
            remote: true,
            revisions: Vec::new(),
        },
    ];
    let text = format_text(&branches);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("* main abc1234 first"), "got: {text}");
    assert!(
        lines[1].starts_with("  feature def5678 add thing -> origin/feature [ahead 2]"),
        "got: {text}"
    );
    assert!(lines[2].ends_with("(remote)"), "got: {text}");
}

#[test]
fn formats_text_with_annotations() {
    let text = format_text(&[annotated("feature")]);
    assert!(
        text.contains("feature abc1234 subject [D123 Accepted]"),
        "got: {text}"
    );
}

#[test]
fn formats_json_objects() {
    let branches = vec![Branch {
        name: "main".to_string(),
        current: true,
        hash: "abc1234".to_string(),
        subject: "first".to_string(),
        upstream: None,
        track: String::new(),
        remote: false,
        revisions: Vec::new(),
    }];
    let value = format_json(&branches);
    assert_eq!(
        value,
        serde_json::json!([{
            "name": "main",
            "current": true,
            "hash": "abc1234",
            "subject": "first",
            "upstream": null,
            "track": "",
            "remote": false,
            "revisions": [],
        }])
    );
}

#[test]
fn formats_json_with_annotations() {
    let value = format_json(&[annotated("feature")]);
    assert_eq!(
        value[0]["revisions"],
        serde_json::json!([{
            "monogram": "D123",
            "status": "Accepted",
            "title": "Title",
        }])
    );
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("usage: arc-rs branches"));
    assert!(text.contains("--all") && text.contains("--json"));
    assert!(text.contains("review-status"));
}
