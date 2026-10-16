use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn parses_defaults() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&[]), true),
        Ok(Some(Options {
            all: false,
            json: false,
            conduit_uri: None,
        }))
    );
}

#[test]
fn parses_all_json_and_conduit_uri() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--all", "--json"]), true),
        Ok(Some(Options {
            all: true,
            json: true,
            conduit_uri: None,
        }))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri", "https://host/api/"]), true),
        Ok(Some(Options {
            all: false,
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri=https://host/api/"]), true),
        Ok(Some(Options {
            all: false,
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
    assert_eq!(
        parse_options_with_tty(&args(&["--all", "--help"]), true),
        Ok(None)
    );
}

#[test]
fn rejects_bad_arguments() {
    // T1153 LAND-42: toolset parse texts (`PhutilArgumentParser`).
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--bogus"]), true).unwrap_err(),
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["feature"]), true).unwrap_err(),
        "Unrecognized argument \"feature\"."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri"]), true).unwrap_err(),
        "Argument \"--conduit-uri\" requires a parameter."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--all=x"]), true).unwrap_err(),
        "Argument \"--all\" does not take a parameter."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--all", "--all"]), true).unwrap_err(),
        "Argument \"--all\" was provided twice."
    );
    // A lone `--` terminates flags and parses clean.
    assert!(
        parse_options_with_tty(&args(&["--"]), true)
            .unwrap()
            .is_some()
    );
    // Past `--`, flags are positionals.
    assert_eq!(
        parse_options_with_tty(&args(&["--", "--help"]), true).unwrap_err(),
        "Unrecognized argument \"--help\"."
    );
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
        }])
    );
}

#[test]
fn usage_covers_options() {
    // T1153 B21/B23: `--help` renders the generated workflow help
    // (ceb takes no arguments, so extensions stay undocumented).
    let text = usage_text(false);
    assert!(text.contains("BRANCHES WORKFLOW"), "{text}");
    assert!(text.contains("Show an enhanced view of branches"), "{text}");
    assert!(!text.contains("--all"), "{text}");
    assert!(!text.contains("--json"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn b21_branches_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv. Unknown flags still report first.
    assert_eq!(
        parse_options_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--all"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    let err = parse_options_with_tty(&args(&["--bogus"]), false).unwrap_err();
    assert!(err.contains("unrecognized"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(
        parse_options_with_tty(&args(&["--all", "--"]), false)
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
fn b23_branches_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help branches`.
    let text = usage_text(false);
    assert!(text.starts_with("BRANCHES WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Lists branches in the working copy"),
        "{text}"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `branches` runs in the toolset runtime: argument errors render
    // `USAGE EXCEPTION` (exit 1) with no usage dump.
    match execute(&std::env::temp_dir(), &args(&["--bogus"])) {
        Err(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected parse error"),
    }
    assert_eq!(
        run_in(&std::env::temp_dir(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn r4_rbd4_outside_repo_is_toolset_usage_not_fatal() {
    // Outside a repo the marker-type support gate throws
    // `PhutilArgumentUsageException` (exit 1) — T1153 RB-D4's "255" does
    // not reproduce at ceb.
    // T1153 B21/B23: piped runs carry `--` to reach the support gate.
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-branches-r4-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        run_in(&dir, &args(&["--"])),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}
