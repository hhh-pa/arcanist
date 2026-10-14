use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn marker(name: &str, active: bool, hash: &str, epoch: u64, subject: &str) -> Marker {
    Marker {
        name: name.to_string(),
        marker_type: MARKER_TYPE_BRANCH.to_string(),
        active,
        hash: hash.to_string(),
        epoch,
        subject: subject.to_string(),
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(
        parse_options(&args(&[])),
        Ok(Some(Options {
            marker_type: None,
            json: false,
        }))
    );
}

#[test]
fn parses_type_and_json() {
    assert_eq!(
        parse_options(&args(&["--type", "bookmark", "--json"])),
        Ok(Some(Options {
            marker_type: Some("bookmark".to_string()),
            json: true,
        }))
    );
    assert_eq!(
        parse_options(&args(&["--type=branch"])),
        Ok(Some(Options {
            marker_type: Some("branch".to_string()),
            json: false,
        }))
    );
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
    let err = parse_options(&args(&["feature"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--type"])).unwrap_err();
    assert!(err.contains("requires a value"), "unexpected: {err}");
    let err = parse_options(&args(&["--type", "tag"])).unwrap_err();
    assert!(
        err.contains("unknown marker type 'tag'"),
        "unexpected: {err}"
    );
    let err = parse_options(&args(&["--type=Tag"])).unwrap_err();
    assert!(err.contains("unknown marker type"), "unexpected: {err}");
}

#[test]
fn short_hash_takes_seven() {
    assert_eq!(short_hash("abc123456789"), "abc1234");
    assert_eq!(short_hash("tiny"), "tiny");
}

#[test]
fn limit_guards_over_a_thousand() {
    assert!(check_limit(1000).is_ok());
    assert_eq!(
        check_limit(1001).unwrap_err(),
        "Found more than 1000 unpublished commits which are ancestors of heads."
    );
}

#[test]
fn sorts_active_epoch_then_name() {
    let mut markers = vec![
        marker("zeta", false, "a", 300, "z"),
        marker("Alpha", false, "b", 100, "a"),
        marker("mid", true, "c", 50, "m"),
        marker("beta", false, "d", 100, "b"),
    ];
    sort_markers(&mut markers);
    let names: Vec<&str> = markers.iter().map(|marker| marker.name.as_str()).collect();
    assert_eq!(names, vec!["mid", "zeta", "Alpha", "beta"]);
}

#[test]
fn parses_git_ref_lines() {
    let marker = parse_git_ref_line(
        "feature\x1fabc1234567890\x1f1729000000\x1fadd thing",
        Some("feature"),
    )
    .unwrap();
    assert_eq!(marker.name, "feature");
    assert!(marker.active);
    assert_eq!(marker.marker_type, "branch");
    assert_eq!(marker.epoch, 1_729_000_000);
    assert_eq!(marker.subject, "add thing");

    let marker = parse_git_ref_line("main\x1fdef0\x1f10\x1ffirst", Some("feature")).unwrap();
    assert!(!marker.active);

    assert!(parse_git_ref_line("only-name", None).is_err());
    assert!(parse_git_ref_line("a\x1fb\x1fc", None).is_err());
    assert!(parse_git_ref_line("a\x1fb\x1fNaN\x1fc", None).is_err());
    assert!(parse_git_ref_line("a\x1fb\x1f1\x1fc\x1fextra", None).is_err());
}

#[test]
fn parses_hg_ls_markers_fixture() {
    let text = include_str!("../tests/fixtures/lintcfg/hglsmarkers.json");
    let items = parse_hg_ls_markers(text, "bookmark").unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "review");
    assert!(items[0].active);
    assert_eq!(items[0].subject, "first line of description");
    assert_eq!(items[1].name, "wip");
    assert!(!items[1].active);

    // Closed heads are skipped; the other type is filtered out.
    let items = parse_hg_ls_markers(text, "branch").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "default");
}

#[test]
fn rejects_unknown_hg_ls_marker_types() {
    let err = parse_hg_ls_markers(
        r#"[{"type": "tag", "name": "v1", "node": "abc"}]"#,
        "branch",
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Call to \"hg arc-ls-markers\" returned marker of unknown type \"tag\"."
    );
    assert!(parse_hg_ls_markers("oops", "branch").is_err());
    assert!(parse_hg_ls_markers(r#"[{"type": "branch"}]"#, "branch").is_err());
}

#[test]
fn parses_hg_name_lines() {
    let text = include_str!("../tests/fixtures/lintcfg/hgbookmarks.txt");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        parse_hg_name_line(lines[0]).unwrap(),
        ("review".to_string(), true, "abc123456789".to_string())
    );
    assert_eq!(
        parse_hg_name_line(lines[1]).unwrap(),
        ("wip".to_string(), false, "def012345678".to_string())
    );
    // `hg branches` lines end with an inactive note; only the head matters.
    assert_eq!(
        parse_hg_name_line("stable  9:0123456789ab (inactive)").unwrap(),
        ("stable".to_string(), false, "0123456789ab".to_string())
    );
    assert!(parse_hg_name_line("").is_err());
    assert!(parse_hg_name_line("lonely").is_err());
}

#[test]
fn parses_hg_log_lines() {
    let text = include_str!("../tests/fixtures/lintcfg/hglog.txt");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        parse_hg_log_line(lines[0]).unwrap(),
        (
            "abc123456789".to_string(),
            1_729_000_000,
            "add thing".to_string()
        )
    );
    assert!(parse_hg_log_line("a\x1fb").is_err());
    assert!(parse_hg_log_line("a\x1fnot-a-date\x1fc").is_err());
}

#[test]
fn renders_grid_with_php_columns_and_bullets() {
    let markers = vec![
        marker("main", true, "abc1234567890", 200, "first"),
        marker("feature", false, "def5678901234", 100, "add thing"),
    ];
    let grid = render_grid(&markers);
    let expected = "• * main   abc1234        first    \n\
                    • feature  def5678        add thing\n";
    assert_eq!(grid, expected);
}

#[test]
fn renders_empty_grid_as_blank_line() {
    assert_eq!(render_grid(&[]), "\n");
}

#[test]
fn formats_json_objects() {
    let markers = vec![Marker {
        name: "review".to_string(),
        marker_type: "bookmark".to_string(),
        active: true,
        hash: "abc1234".to_string(),
        epoch: 42,
        subject: "look here".to_string(),
    }];
    assert_eq!(
        format_json(&markers),
        serde_json::json!([{
            "name": "review",
            "type": "bookmark",
            "active": true,
            "hash": "abc1234",
            "epoch": 42,
            "subject": "look here",
        }])
    );
}

#[test]
fn resolves_types_against_detected_vcs() {
    let dir = std::env::temp_dir().join(format!("arc-rs-markers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let err = resolve_type(&dir, None).unwrap_err();
    assert!(
        err.contains("not inside a working copy"),
        "unexpected: {err}"
    );

    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    assert_eq!(resolve_type(&dir, None).unwrap(), MARKER_TYPE_BOOKMARK);
    assert_eq!(
        resolve_type(&dir, Some(MARKER_TYPE_BRANCH)).unwrap(),
        MARKER_TYPE_BRANCH
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("usage: arc-rs markers"));
    assert!(text.contains("--type") && text.contains("--json"));
    assert!(text.contains("review-status"));
}
