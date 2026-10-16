use super::*;
use std::collections::HashMap;

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

fn marker_ref(name: &str, active: bool, hash: &str) -> MarkerRef {
    MarkerRef {
        name: name.to_string(),
        marker_type: MARKER_TYPE_BRANCH.to_string(),
        epoch: 0,
        marker_hash: None,
        commit_hash: hash.to_string(),
        summary: String::new(),
        message: None,
        active,
        remote_name: None,
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(
        parse_options(&args(&[])),
        Ok(Some(Options {
            marker_type: None,
            json: false,
            conduit_uri: None,
        }))
    );
}

#[test]
fn parses_type_json_and_conduit_uri() {
    assert_eq!(
        parse_options(&args(&["--type", "bookmark", "--json"])),
        Ok(Some(Options {
            marker_type: Some("bookmark".to_string()),
            json: true,
            conduit_uri: None,
        }))
    );
    assert_eq!(
        parse_options(&args(&[
            "--type=branch",
            "--conduit-uri",
            "https://host/api/"
        ])),
        Ok(Some(Options {
            marker_type: Some("branch".to_string()),
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
    assert_eq!(
        parse_options(&args(&["--conduit-uri=https://host/api/"])),
        Ok(Some(Options {
            marker_type: None,
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
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
    let err = parse_options(&args(&["--conduit-uri"])).unwrap_err();
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
    assert_eq!(display_hash("abc1234567890"), "abc123456789");
}

#[test]
fn limit_guards_over_a_thousand() {
    assert!(check_limit(1000).is_ok());
    assert_eq!(
        check_limit(1001).unwrap_err(),
        "Found more than 1,000 unpublished commits which are ancestors of heads."
    );
}

#[test]
fn sorts_active_epoch_then_natural_name() {
    let mut markers = vec![
        marker("zeta", false, "a", 300, "z"),
        marker("Alpha", false, "b", 100, "a"),
        marker("mid", true, "c", 50, "m"),
        marker("beta", false, "d", 100, "b"),
    ];
    sort_markers(&mut markers);
    let names: Vec<&str> = markers.iter().map(|marker| marker.name.as_str()).collect();
    assert_eq!(names, vec!["mid", "zeta", "Alpha", "beta"]);

    // Natural order: b2 sorts before b10.
    let mut markers = vec![
        marker("b10", false, "a", 1, ""),
        marker("b2", false, "b", 1, ""),
    ];
    sort_markers(&mut markers);
    assert_eq!(markers[0].name, "b2");
}

#[test]
fn parses_git_branch_records() {
    let record = "refs/heads/feature\x01abc1234\x011729000000 +0200\x01tree1\x01\x01add thing\x01add thing\n\nbody\x01";
    let marker = parse_git_ref_record(record).unwrap().unwrap();
    assert_eq!(marker.name, "feature");
    assert_eq!(marker.marker_type, "branch");
    assert_eq!(marker.commit_hash, "abc1234");
    assert_eq!(marker.epoch, 1_729_000_000);
    assert_eq!(marker.summary, "add thing");
    assert_eq!(marker.message, Some("add thing\n\nbody".to_string()));
    assert!(!marker.active);
    assert!(!marker.is_remote_cache());
}

#[test]
fn parses_git_remote_records_with_tag_targets() {
    let record = "refs/remotes/origin/feature\x01aaa\x011\x01t\x01dst1\x01s\x01s\x01";
    let marker = parse_git_ref_record(record).unwrap().unwrap();
    assert_eq!(marker.name, "feature");
    assert_eq!(marker.remote_name, Some("origin".to_string()));
    assert!(marker.is_remote_cache());
    // Annotated tags resolve through the `*objectname` field.
    assert_eq!(marker.commit_hash, "dst1");
}

#[test]
fn discards_other_refs_and_blank_records() {
    assert!(parse_git_ref_record("").unwrap().is_none());
    assert!(parse_git_ref_record("\n").unwrap().is_none());
    let record = "refs/tags/v1\x01aaa\x011\x01t\x01\x01s\x01s\x01";
    assert!(parse_git_ref_record(record).unwrap().is_none());
    // A remote ref without a path separator never occurs; skip it.
    let record = "refs/remotes/origin\x01aaa\x011\x01t\x01\x01s\x01s\x01";
    assert!(parse_git_ref_record(record).unwrap().is_none());
}

#[test]
fn rejects_malformed_git_records() {
    let err = parse_git_ref_record("only-name").unwrap_err();
    assert!(err.contains("Unexpected field count"), "unexpected: {err}");
    assert!(parse_git_ref_record("a\x01b\x01c").is_err());
}

#[test]
fn parses_remote_lines() {
    assert_eq!(
        parse_remote_line("origin\thttps://host/repo.git (fetch)").unwrap(),
        (
            "origin".to_string(),
            "https://host/repo.git".to_string(),
            "fetch".to_string()
        )
    );
    assert_eq!(
        parse_remote_line("origin\thttps://host/repo.git (push)").unwrap(),
        (
            "origin".to_string(),
            "https://host/repo.git".to_string(),
            "push".to_string()
        )
    );
    assert!(parse_remote_line("").is_err());
    assert!(parse_remote_line("origin https://host/repo.git (fetch)").is_err());
    assert!(parse_remote_line("origin\thttps://host/repo.git (prune)").is_err());
    let err = parse_remote_line("bogus").unwrap_err();
    assert_eq!(
        err,
        "Failed to match remote pattern against line \"bogus\"."
    );
}

#[test]
fn parses_hg_ls_markers_fixture() {
    let text = include_str!("../tests/fixtures/lintcfg/hglsmarkers.json");
    let markers = parse_hg_ls_markers(text).unwrap();
    // Closed heads are skipped; every other type is kept.
    assert_eq!(markers.len(), 3);
    assert_eq!(markers[0].name, "review");
    assert!(markers[0].active);
    assert_eq!(markers[0].summary, "first line of description");
    assert_eq!(
        markers[0].message,
        Some("first line of description\nsecond line".to_string())
    );
    assert_eq!(markers[1].name, "wip");
    assert!(!markers[1].active);
    assert_eq!(markers[2].marker_type, "branch");
}

#[test]
fn hg_flags_use_php_truthiness() {
    let markers = parse_hg_ls_markers(
        r#"[
            {"type": "bookmark", "name": "a", "node": "aaa", "isActive": "false"},
            {"type": "bookmark", "name": "b", "node": "bbb", "isActive": "0"},
            {"type": "bookmark", "name": "c", "node": "ccc", "isActive": 1}
        ]"#,
    )
    .unwrap();
    // PHP `empty()` treats "false" as set and "0" as empty.
    assert!(markers[0].active);
    assert!(!markers[1].active);
    assert!(markers[2].active);
}

#[test]
fn rejects_unknown_hg_ls_marker_types() {
    let err = parse_hg_ls_markers(r#"[{"type": "tag", "name": "v1", "node": "abc"}]"#).unwrap_err();
    assert_eq!(
        err,
        "Call to \"hg arc-ls-markers\" returned marker of unknown type \"tag\"."
    );
    assert!(parse_hg_ls_markers("oops").is_err());
    assert!(parse_hg_ls_markers(r#"[{"type": "branch"}]"#).is_err());
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
fn checks_marker_type_support() {
    assert!(supports_marker_type("git", "branch"));
    assert!(!supports_marker_type("git", "bookmark"));
    assert!(supports_marker_type("hg", "branch"));
    assert!(supports_marker_type("hg", "bookmark"));
    assert!(!supports_marker_type("svn", "branch"));
    assert!(!supports_marker_type("svn", "bookmark"));
    assert_eq!(
        support_error("svn", "branch"),
        "The version control system (\"svn\") in the current working copy does not support branches."
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
fn usage_covers_options() {
    let text = usage();
    assert!(text.contains("usage: arc markers"));
    assert!(text.contains("--type") && text.contains("--json"));
    assert!(text.contains("--conduit-uri"));
}

fn revision(id: u32, phid: &str, title: &str, published: bool) -> revision::RevisionRef {
    let (value, name) = if published {
        ("published", "Published")
    } else {
        ("needs-review", "Needs Review")
    };
    revision::RevisionRef::from_search_hit(&serde_json::json!({
        "id": id,
        "phid": phid,
        "fields": {
            "title": title,
            "status": {"value": value, "name": name},
        },
    }))
}

#[test]
fn merges_single_revisions_into_commits() {
    let commits = vec![("aaa".to_string(), "add thing\n".to_string())];
    let revisions = vec![revision(1, "PHID-DREV-1", "unrelated title", false)];
    let items = build_items(&commits, &revisions, &HashMap::new());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].commit, Some(0));
    assert_eq!(items[0].revision, Some(0));
}

#[test]
fn merges_multiple_revisions_by_summary_key() {
    let commits = vec![
        ("aaa".to_string(), "first change\n".to_string()),
        ("bbb".to_string(), "second change\n".to_string()),
    ];
    let revisions = vec![
        revision(1, "PHID-DREV-1", "first change", false),
        revision(2, "PHID-DREV-2", "other work", false),
    ];
    let items = build_items(&commits, &revisions, &HashMap::new());
    // The unmatched revision renders first, on its own item.
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].revision, Some(1));
    assert!(items[0].commit.is_none());
    assert_eq!(items[1].commit, Some(0));
    assert_eq!(items[1].revision, Some(0));
    assert_eq!(items[2].commit, Some(1));
    assert_eq!(items[2].revision, None);
}

#[test]
fn attaches_head_markers_and_lists_extras() {
    let commits = vec![("aaa".to_string(), "subject\n".to_string())];
    let mut markers = HashMap::new();
    markers.insert(
        "aaa".to_string(),
        vec![
            marker_ref("one", false, "aaa"),
            marker_ref("two", true, "aaa"),
        ],
    );
    let items = build_items(&commits, &[], &markers);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].commit, Some(0));
    assert_eq!(items[0].marker.as_ref().unwrap().name, "one");
    assert!(items[1].commit.is_none());
    assert_eq!(items[1].marker.as_ref().unwrap().name, "two");
}

#[test]
fn collapses_long_leading_commit_runs() {
    let commits: Vec<(String, String)> = (0..12)
        .map(|index| (format!("h{index:02}"), format!("subject {index}\n")))
        .collect();
    let items = build_items(&commits, &[], &HashMap::new());
    // 3 of context on each side plus one placeholder carrying the count.
    assert_eq!(items.len(), 7);
    assert_eq!(items[3].collapse, Some(12));
    assert_eq!(items[3].commit, Some(3));
}

#[test]
fn leaves_short_and_nonleading_runs_uncollapsed() {
    let commits: Vec<(String, String)> = (0..8)
        .map(|index| (format!("h{index:02}"), format!("subject {index}\n")))
        .collect();
    let items = build_items(&commits, &[], &HashMap::new());
    assert_eq!(items.len(), 8);

    // A leading revision breaks the leading run, so nothing collapses.
    let revisions = vec![revision(9, "PHID-DREV-9", "detached", false)];
    let commits: Vec<(String, String)> = (0..12)
        .map(|index| (format!("h{index:02}"), format!("other {index}\n")))
        .collect();
    let items = build_items(&commits, &revisions, &HashMap::new());
    assert_eq!(items.len(), 13);
    assert!(items.iter().all(|item| item.collapse.is_none()));
}

#[test]
fn draws_tree_cells_for_a_view() {
    let commits = vec![
        ("aaa1111".to_string(), "first\n".to_string()),
        ("bbb2222".to_string(), "second\n".to_string()),
    ];
    let revisions = vec![revision(1, "PHID-DREV-1", "first", false)];
    let mut markers = HashMap::new();
    markers.insert(
        "aaa1111".to_string(),
        vec![marker_ref("main", true, "aaa1111")],
    );
    let items = build_items(&commits, &revisions, &markers);
    let cells = draw_view_row(&commits, &revisions, items, 1, 0, false, Some(120), false);
    assert_eq!(cells.len(), 5);
    let marker = cells[0].display_lines().join("\n");
    assert!(marker.contains("•"), "got: {marker}");
    assert!(marker.contains(" *  main"), "got: {marker}");
    let revisions_cell = cells[3].display_lines().join("\n");
    assert!(revisions_cell.contains("D1"), "got: {revisions_cell}");
    let status = cells[2].display_lines().join("\n");
    assert!(status.contains("Needs Review"), "got: {status}");
}

#[test]
fn draws_group_trailers_for_collapsed_members() {
    let commits = vec![("aaa1111".to_string(), "first\n".to_string())];
    let cells = draw_view_row(
        &commits,
        &[],
        vec![Item {
            commit: Some(0),
            ..Item::default()
        }],
        4,
        1,
        true,
        Some(80),
        false,
    );
    let marker = cells[0].display_lines().join("\n");
    assert!(
        marker.contains("(... 3 more revisions ...)"),
        "got: {marker}"
    );
}

#[test]
fn merge_keys_use_byte_prefixes() {
    assert_eq!(merge_key("ABC"), "abc");
    assert_eq!(merge_key(&"x".repeat(40)), "x".repeat(32));
}

fn colored_revision(color: &str) -> revision::RevisionRef {
    revision::RevisionRef::from_search_hit(&serde_json::json!({
        "id": 7,
        "phid": "PHID-DREV-7",
        "fields": {
            "title": "colored",
            "status": {
                "value": "needs-review",
                "name": "Needs Review",
                "color.ansi": color,
            },
        },
    }))
}

#[test]
fn marker_labels_style_active_and_inactive() {
    assert_eq!(active_marker_label("main", false), " *  main");
    assert_eq!(inactive_marker_label("dev", false), "dev");
    assert_eq!(
        active_marker_label("main", true),
        "\x1b[42m\x1b[1m * \x1b[m\x1b[49m \x1b[1mmain\x1b[m"
    );
    assert_eq!(inactive_marker_label("dev", true), "\x1b[1mdev\x1b[m");
}

#[test]
fn status_labels_colorize_on_tty() {
    let reference = colored_revision("red");
    // Narrow terminals show the short name, wide ones the full name.
    assert_eq!(revision_status_label(&reference, true, false), "Review");
    assert_eq!(
        revision_status_label(&reference, false, false),
        "Needs Review"
    );
    assert_eq!(
        revision_status_label(&reference, true, true),
        "\x1b[31mReview\x1b[39m"
    );
    assert_eq!(
        revision_status_label(&reference, false, true),
        "\x1b[31mNeeds Review\x1b[39m"
    );
    // Falsy server colors ("", "0") and missing colors stay plain.
    for color in ["", "0"] {
        let plain = colored_revision(color);
        assert_eq!(revision_status_label(&plain, false, true), "Needs Review");
    }
    let uncolored = revision(1, "PHID-DREV-1", "plain", false);
    assert_eq!(
        revision_status_label(&uncolored, false, true),
        "Needs Review"
    );
}

#[test]
fn styled_rows_carry_escapes_only_on_tty() {
    let commits = vec![("aaa1111".to_string(), "first\n".to_string())];
    let revisions = vec![colored_revision("green")];
    let mut markers = HashMap::new();
    markers.insert(
        "aaa1111".to_string(),
        vec![marker_ref("main", true, "aaa1111")],
    );
    let items = build_items(&commits, &revisions, &markers);
    let plain = draw_view_row(
        &commits,
        &revisions,
        items.clone(),
        1,
        0,
        false,
        Some(120),
        false,
    );
    let marker = plain[0].display_lines().join("\n");
    assert!(marker.contains(" *  main"), "got: {marker}");
    assert!(!marker.contains('\x1b'), "got: {marker}");
    let status = plain[2].display_lines().join("\n");
    assert!(status.contains("Needs Review"), "got: {status}");
    assert!(!status.contains('\x1b'), "got: {status}");

    let styled = draw_view_row(&commits, &revisions, items, 1, 0, false, Some(120), true);
    let marker = styled[0].display_lines().join("\n");
    assert!(
        marker.contains("\x1b[42m\x1b[1m * \x1b[m\x1b[49m \x1b[1mmain\x1b[m"),
        "got: {marker}"
    );
    let status = styled[2].display_lines().join("\n");
    assert!(
        status.contains("\x1b[32mNeeds Review\x1b[39m"),
        "got: {status}"
    );
}

#[test]
fn r4_rbd4_toolset_usage_matcher() {
    // Support-gate, outside-repo, limit, and bad-type errors are
    // toolset usage (exit 1); query failures escape into a fatal.
    assert!(is_toolset_usage_message(
        "not inside a working copy (no git repository or Mercurial '.hg' directory found)"
    ));
    assert!(is_toolset_usage_message("not inside a git repository"));
    assert!(is_toolset_usage_message(
        "The version control system (\"svn\") in the current working copy does not support branches."
    ));
    assert!(is_toolset_usage_message(
        "Found more than 1,000 unpublished commits which are ancestors of heads."
    ));
    assert!(is_toolset_usage_message("unknown marker type 'zzz'"));
    assert!(!is_toolset_usage_message("failed to run git: boom"));
    assert!(!is_toolset_usage_message("boom"));
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    match execute(&std::env::temp_dir(), &args(&["--bogus"])) {
        Err(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected parse error"),
    }
    assert_eq!(
        run_in(&std::env::temp_dir(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}
