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
    // T1153 LAND-42: toolset parse texts (`PhutilArgumentParser`).
    assert_eq!(
        parse_options(&args(&["--bogus"])).unwrap_err(),
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    assert_eq!(
        parse_options(&args(&["feature"])).unwrap_err(),
        "Unrecognized argument \"feature\"."
    );
    assert_eq!(
        parse_options(&args(&["--type"])).unwrap_err(),
        "Argument \"--type\" requires a parameter."
    );
    assert_eq!(
        parse_options(&args(&["--conduit-uri"])).unwrap_err(),
        "Argument \"--conduit-uri\" requires a parameter."
    );
    assert_eq!(
        parse_options(&args(&["--json=x"])).unwrap_err(),
        "Argument \"--json\" does not take a parameter."
    );
    assert_eq!(
        parse_options(&args(&["--json", "--json"])).unwrap_err(),
        "Argument \"--json\" was provided twice."
    );
    // Parse errors precede workflow validation: the unknown flag
    // wins over the bad `--type`.
    assert_eq!(
        parse_options(&args(&["--type", "tag", "--bogus"])).unwrap_err(),
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    let err = parse_options(&args(&["--type", "tag"])).unwrap_err();
    assert!(
        err.contains("unknown marker type 'tag'"),
        "unexpected: {err}"
    );
    let err = parse_options(&args(&["--type=Tag"])).unwrap_err();
    assert!(err.contains("unknown marker type"), "unexpected: {err}");
    // A lone `--` terminates flags and parses clean.
    assert!(parse_options(&args(&["--"])).unwrap().is_some());
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
}

#[test]
fn land27_nameless_hg_markers_proceed() {
    // T1153 LAND-27 / T1153 RB-D34: PHP reads `name`/`node` unchecked,
    // so markers without them proceed with empty strings; without
    // `description` (T1153 RB-D39) no message attaches and the summary
    // stays empty.
    let markers = parse_hg_ls_markers(r#"[{"type": "branch"}]"#).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "");
    assert_eq!(markers[0].commit_hash, "");
    assert_eq!(markers[0].summary, "");
    assert_eq!(markers[0].message, None);
}

#[test]
fn land26_failed_hg_call_renders_command_exception() {
    // A nonzero exit throws like PHP's future: the `CommandException`
    // dump carries the command plus stdout/stderr sections, never
    // bare stderr. (`git` stands in as a portable failing `hg`.)
    let dir = std::env::temp_dir().join(format!("arc-rs-land26-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    let hg = crate::hg::HgCommand::custom("git", &["definitely-not-a-git-command"]);
    let err = query_hg_markers_with(&dir, &hg, "branch").unwrap_err();
    assert!(
        err.starts_with("Command failed with error #1!\nCOMMAND\n"),
        "unexpected error: {err}"
    );
    assert!(err.contains("\nSTDOUT\n(empty)\n\nSTDERR\n"), "{err}");
    assert!(err.contains("not a git command"), "{err}");
    assert!(err.contains("arc-ls-markers"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn land26_command_sections_truncate_like_php() {
    assert_eq!(summarize_command_section(""), "(empty)");
    assert_eq!(summarize_command_section("abc"), "abc");
    // A 1001-byte section keeps everything: the cut (1) is shorter
    // than the suffix, so PHP skips truncation.
    let near = "x".repeat(1001);
    assert_eq!(summarize_command_section(&near), near);
    // Past that, the head plus a grouped tail remain.
    let far = "y".repeat(2500);
    assert_eq!(
        summarize_command_section(&far),
        format!("{}... (1,500 more byte(s)) ...", "y".repeat(1000))
    );
}

#[test]
fn land25_bundled_extension_materializes() {
    // Outside any checkout the bundled `arc-hg.py` copy resolves,
    // with the compiled-in bytes on disk — and a deleted copy
    // reinstalls on the next call. One test (not two): the target
    // path is fixed, so a parallel remover flaked the reader
    // (macOS CI read back empty).
    let script = materialize_bundled_arc_hg().unwrap();
    assert_eq!(std::fs::read_to_string(&script).unwrap(), ARC_HG_BUNDLED);
    assert!(arc_hg_script(std::env::temp_dir().as_path()).is_some());
    let target = std::env::temp_dir().join(format!("arc-hg-{:016x}.py", arc_hg_content_tag()));
    std::fs::remove_file(&target).ok();
    let script = materialize_bundled_arc_hg().unwrap();
    assert_eq!(script, target);
    assert_eq!(std::fs::read_to_string(&script).unwrap(), ARC_HG_BUNDLED);
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
    // T1153 B21/B23 KEPT (Rule 2b): `markers` is abstract at ceb
    // (`ArcanistMarkersWorkflow` has no concrete workflow, so there
    // are no generated help bytes to match); the custom usage stays.
    let text = usage();
    assert!(text.contains("usage: arc markers"));
    assert!(text.contains("--type") && text.contains("--json"));
    assert!(text.contains("--conduit-uri"));
    assert!(!text.contains("MARKERS WORKFLOW"), "{text}");
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

fn repository_hit(name: &str, uri: &str, rules: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "phid": format!("PHID-REPO-{name}"),
        "fields": {
            "name": name,
            "refRules": {"permanentRefRules": rules},
        },
        "attachments": {
            "uris": {
                "uris": [
                    {"fields": {"uri": {"effective": uri}}},
                ],
            },
        },
    })
}

fn remote_with_push(name: &str, push: &str) -> RemoteRef {
    RemoteRef {
        name: name.to_string(),
        fetch_uri: None,
        push_uri: Some(push.to_string()),
    }
}

#[test]
fn rbd30_repo_collision_last_wins() {
    // Both repos claim the push URI; PHP's `$uri_map` overwrite keeps
    // the LAST one, so `master` is permanent only via the second hit.
    let repositories = vec![
        revision::RepositoryRef::from_search_hit(&repository_hit(
            "first",
            "git@h.test:repo.git",
            &[],
        )),
        revision::RepositoryRef::from_search_hit(&repository_hit(
            "second",
            "git@h.test:repo.git",
            &["master"],
        )),
    ];
    let remote = remote_with_push("origin", "git@h.test:repo.git");
    assert!(is_permanent_git_ref(&repositories, &remote, "master"));
    assert!(!is_permanent_git_ref(&repositories, &remote, "feature"));
    // Reversed, the empty rule set (match-all) wins instead.
    let reversed = vec![repositories[1].clone(), repositories[0].clone()];
    assert!(is_permanent_git_ref(&reversed, &remote, "feature"));
}

#[test]
fn land23_loads_fail_without_conduit() {
    // Without a client the hardpoint loads fail like PHP instead
    // of yielding empty annotation.
    let states = vec![StateCommit {
        hash: "abc".to_string(),
        message: String::new(),
    }];
    let err = load_revision_refs(None, &states).unwrap_err();
    assert!(
        err.contains("This command needs to communicate with Phabricator"),
        "unexpected: {err}"
    );
    let err = load_revision_refs_for_vcs(None, &states, "hg").unwrap_err();
    assert!(
        err.contains("This command needs to communicate with Phabricator"),
        "unexpected: {err}"
    );
}

#[test]
fn land24_hash_pairs_use_vcs_kind() {
    let states = vec![StateCommit {
        hash: "abc".to_string(),
        message: String::new(),
    }];
    assert_eq!(hash_pair_kind("git"), "gtcm");
    assert_eq!(hash_pair_kind("hg"), "hgcm");
    assert_eq!(
        state_hash_pairs(&states, hash_pair_kind("git")),
        vec![("gtcm".to_string(), "abc".to_string())]
    );
    assert_eq!(
        state_hash_pairs(&states, hash_pair_kind("hg")),
        vec![("hgcm".to_string(), "abc".to_string())]
    );
    assert_eq!(strip_pair_prefix("hgcm:abc", "hgcm"), Some("abc"));
    assert_eq!(strip_pair_prefix("gtcm:abc", "hgcm"), None);
    assert_eq!(strip_pair_prefix("hgcmX:abc", "hgcm"), None);
}

#[test]
fn rbd32_revision_ref_lists_key_by_phid() {
    // `mergeHardpointValues` keys by PHID with first-wins: duplicate
    // PHIDs collapse, and PHID-less refs collapse onto the `""` key
    // instead of accumulating.
    let mut merged = std::collections::HashMap::from([("abc".to_string(), Vec::new())]);
    let first =
        revision::RevisionRef::from_query_row(&serde_json::json!({"id": 1, "phid": "PHID-DREV-1"}));
    let dupe =
        revision::RevisionRef::from_query_row(&serde_json::json!({"id": 1, "phid": "PHID-DREV-1"}));
    let other =
        revision::RevisionRef::from_query_row(&serde_json::json!({"id": 2, "phid": "PHID-DREV-2"}));
    let bare_a = revision::RevisionRef::from_query_row(&serde_json::json!({"id": 3}));
    let bare_b = revision::RevisionRef::from_query_row(&serde_json::json!({"id": 4}));
    for reference in [&first, &dupe, &other, &bare_a, &bare_b] {
        push_revision_ref_once(&mut merged, "abc", reference);
    }
    let ids: Vec<u32> = merged["abc"].iter().map(|r| r.id()).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    // Unknown hashes are ignored, like PHP's `$map` miss.
    push_revision_ref_once(&mut merged, "zzz", &other);
    assert!(!merged.contains_key("zzz"));
}

#[test]
fn b21_markers_terminator_gate_stays_global() {
    // T1153 B21/B23 KEPT (Rule 2b): `markers` is abstract at ceb
    // (Rust-only extra, no PHP workflow), so there is no per-command
    // terminator behavior to match. The library parse takes no TTY
    // input and succeeds without `--`; the binary gate still covers
    // `arc markers` when piped (`main.rs needs_terminator_error`,
    // `TOOLSET_EXTRA_COMMANDS`).
    assert!(parse_options(&args(&[])).unwrap().is_some());
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    assert!(
        parse_options(&args(&["--type", "branch"]))
            .unwrap()
            .is_some()
    );
}

#[test]
fn b23_markers_help_stays_custom() {
    // T1153 B21/B23 KEPT (Rule 2b): abstract at ceb, so the custom
    // usage stays instead of generated workflow help.
    let text = usage();
    assert!(text.starts_with("usage: arc markers"), "{text}");
    assert!(!text.contains("WORKFLOW"), "{text}");
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-markers-{tag}-{}-{n}", std::process::id()));
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init", "-q", "-b", "master"]);
    git(tmp.path(), &["config", "user.email", "t@t"]);
    git(tmp.path(), &["config", "user.name", "t"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-qm", "add f"]);
    tmp
}

/// A fake `hg` whose stdout is `body` (mirrors `HgCommand::custom`;
/// the appended hg argv are ignored by `sh -c`).
fn fake_hg_stdout(body: &str) -> crate::hg::HgCommand {
    crate::hg::HgCommand {
        program: "sh".to_string(),
        prefix: vec!["-c".to_string(), format!("printf '%s' '{body}'")],
    }
}

/// Serve canned Conduit envelopes in order; returns the base URI and
/// the observed (request line, body) pairs once all are served.
fn serve_seq(canned: Vec<String>) -> (String, std::sync::mpsc::Receiver<Vec<(String, String)>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for body in canned {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut raw = vec![0u8; len];
            stream.read_exact(&mut raw).unwrap();
            let line = head.lines().next().unwrap_or("").to_string();
            seen.push((line, String::from_utf8(raw).unwrap()));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        tx.send(seen).unwrap();
    });
    (base, rx)
}

fn set_view(id: u64, parent: Option<u64>, children: Vec<u64>, group: u64) -> SetView {
    SetView {
        set_id: id,
        parent,
        children,
        group,
        members: Vec::new(),
        depth: 0,
    }
}

#[test]
fn t1170_resolve_type_covers_bookmark_branch_and_unknown() {
    let empty = TempDir::new("resolve-empty");
    let err = resolve_type(empty.path(), Some(MARKER_TYPE_BOOKMARK)).unwrap_err();
    assert!(err.contains("no Mercurial '.hg' directory found"), "{err}");
    let err = resolve_type(empty.path(), Some(MARKER_TYPE_BRANCH)).unwrap_err();
    assert!(err.contains("no git repository or Mercurial"), "{err}");
    let err = resolve_type(empty.path(), Some("tag")).unwrap_err();
    assert_eq!(err, "unknown marker type 'tag'");
    let hg = TempDir::new("resolve-hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(
        resolve_type(hg.path(), Some(MARKER_TYPE_BOOKMARK)).unwrap(),
        MARKER_TYPE_BOOKMARK
    );
}

#[test]
fn t1170_php_int_prefix_parses_signs_like_php_cast() {
    assert_eq!(php_int_prefix("-12abc"), -12);
    assert_eq!(php_int_prefix("+7"), 7);
    assert_eq!(php_int_prefix("1729000000 +0200"), 1_729_000_000);
    assert_eq!(php_int_prefix("abc"), 0);
    assert_eq!(php_int_prefix(""), 0);
}

#[test]
fn t1170_query_git_markers_fails_outside_repos() {
    let empty = TempDir::new("gitmarkers-empty");
    let err = query_git_markers(empty.path(), MARKER_TYPE_BRANCH, false).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn t1170_query_git_markers_filters_type_and_marks_current() {
    let repo = init_repo("gitmarkers");
    let branches = query_git_markers(repo.path(), MARKER_TYPE_BRANCH, false).unwrap();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].name, "master");
    assert!(branches[0].active);
    assert!(!branches[0].summary.is_empty());
    // No bookmark flavor under git: every record filters out.
    let bookmarks = query_git_markers(repo.path(), MARKER_TYPE_BOOKMARK, false).unwrap();
    assert!(bookmarks.is_empty());
}

#[test]
fn t1170_arc_hg_script_prefers_checkout_copy() {
    let root = TempDir::new("archg");
    let nested = root.path().join("a").join("b");
    let script = root.path().join("support").join("hg").join("arc-hg.py");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "# fake arc-hg").unwrap();
    assert_eq!(arc_hg_script(&nested), Some(script));
}

#[test]
fn t1170_hg_flags_treat_arrays_and_objects_like_php_empty() {
    let markers = parse_hg_ls_markers(
        r#"[
            {"type": "bookmark", "name": "a", "node": "aaa", "isActive": []},
            {"type": "bookmark", "name": "b", "node": "bbb", "isActive": [1]},
            {"type": "bookmark", "name": "c", "node": "ccc", "isActive": {}},
            {"type": "bookmark", "name": "d", "node": "ddd", "isActive": {"x": 1}}
        ]"#,
    )
    .unwrap();
    assert!(!markers[0].active);
    assert!(markers[1].active);
    assert!(!markers[2].active);
    assert!(markers[3].active);
}

#[test]
fn t1170_query_hg_markers_needs_mercurial_checkout() {
    let empty = TempDir::new("hgmarkers-empty");
    let hg = crate::hg::HgCommand::custom("true", &[]);
    assert_eq!(
        query_hg_markers_with(empty.path(), &hg, MARKER_TYPE_BRANCH).unwrap_err(),
        "not inside a Mercurial working copy"
    );
    // hg failures surface as errors. Uses a fake nonzero backend, never
    // the system hg: hg's presence varies by machine (ubuntu CI runners
    // ship it, macOS runners do not), so system hg is unassertable.
    let hgdir = TempDir::new("hgmarkers-fake");
    std::fs::create_dir(hgdir.path().join(".hg")).unwrap();
    let failing = crate::hg::HgCommand::custom("false", &[]);
    assert!(query_hg_markers_with(hgdir.path(), &failing, MARKER_TYPE_BRANCH).is_err());
}

#[test]
fn t1170_query_hg_markers_filters_by_type() {
    let dir = TempDir::new("hgmarkers-fake");
    std::fs::create_dir(dir.path().join(".hg")).unwrap();
    let body = r#"[
        {"type": "branch", "name": "main", "node": "aaa", "isActive": true, "description": "first"},
        {"type": "bookmark", "name": "mark", "node": "bbb", "isActive": false, "description": "second"}
    ]"#;
    let markers =
        query_hg_markers_with(dir.path(), &fake_hg_stdout(body), MARKER_TYPE_BOOKMARK).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "mark");
    assert_eq!(markers[0].marker_type, MARKER_TYPE_BOOKMARK);
    assert_eq!(markers[0].summary, "second");
}

#[test]
fn t1170_published_git_hashes_need_conduit() {
    let repo = init_repo("pubhashes");
    let sha = git(repo.path(), &["rev-parse", "HEAD"]);
    git(
        repo.path(),
        &["remote", "add", "origin", "git@h.test:repo.git"],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", &sha],
    );
    assert_eq!(
        published_git_hashes(repo.path(), None).unwrap_err(),
        crate::conduit::no_uri_block()
    );
}

#[test]
fn t1170_published_git_hashes_keep_permanent_refs() {
    let repo = init_repo("pubhashes-mock");
    let sha = git(repo.path(), &["rev-parse", "HEAD"]);
    git(
        repo.path(),
        &["remote", "add", "origin", "git@h.test:repo.git"],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", &sha],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/feature", &sha],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/stale/other", &sha],
    );
    let hit = repository_hit("repo", "git@h.test:repo.git", &["main"]);
    let (base, rx) = serve_seq(vec![format!(
        r#"{{"result": {{"data": [{hit}]}}, "error_code": null}}"#
    )]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    // `main` is permanent; `feature` is not; `stale` names no remote.
    let hashes = published_git_hashes(repo.path(), Some(&client)).unwrap();
    assert_eq!(hashes, vec![sha]);
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 1);
}

#[test]
fn t1170_permanent_refs_need_push_uri_and_rule_match() {
    // No push URI: never permanent.
    let remote = RemoteRef {
        name: "origin".to_string(),
        fetch_uri: None,
        push_uri: None,
    };
    assert!(!is_permanent_git_ref(&[], &remote, "main"));
    // A repository whose URIs never match leaves no winner.
    let other = revision::RepositoryRef::from_search_hit(&repository_hit(
        "r",
        "git@h.test:other.git",
        &["main"],
    ));
    let remote = remote_with_push("origin", "git@h.test:repo.git");
    assert!(!is_permanent_git_ref(
        std::slice::from_ref(&other),
        &remote,
        "main"
    ));
}

#[test]
fn t1170_published_hg_hashes_list_nodes() {
    let dir = TempDir::new("pubhg");
    let hg = fake_hg_stdout("aaa111\n\nbbb222\n");
    assert_eq!(
        published_hg_hashes_with(dir.path(), &hg).unwrap(),
        vec!["aaa111".to_string(), "bbb222".to_string()]
    );
    assert!(published_hg_hashes(dir.path()).is_err());
}

#[test]
fn t1170_revision_id_index_skips_bad_fields_and_groups_dupes() {
    let states = vec![
        StateCommit {
            hash: "bad".to_string(),
            message: "x\n\nDifferential Revision: xyz".to_string(),
        },
        StateCommit {
            hash: "h1".to_string(),
            message: "one\n\nDifferential Revision: D5".to_string(),
        },
        StateCommit {
            hash: "h2".to_string(),
            message: "two\n\nDifferential Revision: D5".to_string(),
        },
        StateCommit {
            hash: "plain".to_string(),
            message: "no field".to_string(),
        },
    ];
    let (map, order) = revision_id_index(&states);
    assert_eq!(order, vec![5]);
    assert_eq!(
        map.get(&5).unwrap(),
        &vec!["h1".to_string(), "h2".to_string()]
    );
}

#[test]
fn t1170_load_revision_refs_merges_message_query_hits() {
    let states = vec![StateCommit {
        hash: "abc".to_string(),
        message: "change\n\nDifferential Revision: D7".to_string(),
    }];
    let query_rows = r#"[{"id": 7, "phid": "PHID-DREV-7", "hashes": [["gtcm", "abc"]]}]"#;
    let hit = serde_json::json!({
        "id": 7,
        "phid": "PHID-DREV-7",
        "fields": {
            "title": "change",
            "status": {"value": "needs-review", "name": "Needs Review"},
        },
    });
    let (base, rx) = serve_seq(vec![
        format!(r#"{{"result": {query_rows}, "error_code": null}}"#),
        format!(r#"{{"result": {{"data": [{hit}]}}, "error_code": null}}"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let merged = load_revision_refs_for_vcs(Some(&client), &states, "git").unwrap();
    // The hash query and the id query agree on PHID-DREV-7: one ref.
    let refs = merged.get("abc").unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].id(), 7);
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
}

#[test]
fn t1170_hash_active_and_parent_lookups_miss_cleanly() {
    let groups: HashMap<String, Vec<MarkerRef>> = HashMap::new();
    assert!(!hash_has_active_marker(&groups, "zzz"));
    let sets: Vec<commitgraph::CommitSet> = Vec::new();
    assert!(parent_set_ids(&sets, 99).is_empty());
}

#[test]
fn t1170_marker_part_vectors_handle_empty_names() {
    let mut names = Vec::new();
    assert_eq!(set_marker_parts(&[], &mut names), (0, String::new()));
    assert!(names.is_empty());
    let empty = partition_sort_vector(false, 0, &[]);
    let marked = partition_sort_vector(false, 0, &["main".to_string()]);
    assert_ne!(empty.sort_string(), marked.sort_string());
    let active = partition_sort_vector(true, 0, &[]);
    assert_ne!(empty.sort_string(), active.sort_string());
}

#[test]
fn t1170_partition_order_compares_bytes_then_position() {
    use std::cmp::Ordering;
    assert_eq!(
        partition_order_cmp(&("a".to_string(), 0), &("b".to_string(), 0)),
        Ordering::Less
    );
    assert_eq!(
        partition_order_cmp(&("b".to_string(), 0), &("a".to_string(), 0)),
        Ordering::Greater
    );
    assert_eq!(
        partition_order_cmp(&("a".to_string(), 0), &("a".to_string(), 1)),
        Ordering::Less
    );
    assert_eq!(
        partition_order_cmp(&("a".to_string(), 1), &("a".to_string(), 1)),
        Ordering::Equal
    );
}

#[test]
#[should_panic(expected = "vector keys cover every set")]
fn t1170_find_set_position_panics_off_map() {
    let sets: Vec<commitgraph::CommitSet> = Vec::new();
    find_set_position(&sets, 7);
}

#[test]
#[should_panic(expected = "view ids resolve")]
fn t1170_view_index_panics_off_map() {
    view_index(&[], 7);
}

#[test]
fn t1170_build_set_views_tolerates_unknown_sets() {
    let sets: HashMap<u64, &commitgraph::CommitSet> = HashMap::new();
    let views = build_set_views(&sets, 99);
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].set_id, 99);
    assert!(views[0].parent.is_none());
    assert!(views[0].children.is_empty());
}

#[test]
fn t1170_group_for_view_covers_shapes() {
    // Childless views group alone.
    let views = vec![set_view(1, None, vec![], 1)];
    assert_eq!(
        group_for_view(&views, 0, &HashMap::new(), &HashMap::new()),
        1
    );
    // Revised views group alone even with children.
    let views = vec![set_view(1, None, vec![2], 1)];
    let revisions = HashMap::from([(1u64, true)]);
    assert_eq!(group_for_view(&views, 0, &revisions, &HashMap::new()), 1);
    // A root with children groups alone.
    let views = vec![set_view(1, None, vec![2], 1)];
    assert_eq!(
        group_for_view(&views, 0, &HashMap::new(), &HashMap::new()),
        1
    );
    // An only child joins its parent's group.
    let views = vec![
        set_view(1, None, vec![2], 7),
        set_view(2, Some(1), vec![3], 2),
    ];
    assert_eq!(
        group_for_view(&views, 1, &HashMap::new(), &HashMap::new()),
        7
    );
    // A non-only child groups alone.
    let views = vec![
        set_view(1, None, vec![2, 3], 7),
        set_view(2, Some(1), vec![4], 2),
    ];
    assert_eq!(
        group_for_view(&views, 1, &HashMap::new(), &HashMap::new()),
        2
    );
}

#[test]
fn t1170_small_groups_split_into_singletons() {
    let mut holder = set_view(1, None, vec![2], 1);
    holder.members = vec![1, 2];
    let mut views = vec![holder, set_view(2, Some(1), vec![], 1)];
    split_small_groups(&mut views);
    assert_eq!(views[0].group, 1);
    assert_eq!(views[0].members, vec![1]);
    assert_eq!(views[1].group, 2);
    assert_eq!(views[1].members, vec![2]);
}

#[test]
fn t1170_view_depth_steps_two_past_groups() {
    let mut holder = set_view(1, None, vec![2], 1);
    holder.members = vec![1, 2];
    holder.depth = 5;
    let views = vec![holder, set_view(2, Some(1), vec![], 2)];
    assert_eq!(view_depth(&views, 1), 7);
    assert_eq!(view_depth(&views, 0), 0);
}

#[test]
fn t1170_commit_merge_map_skips_phid_less_revisions() {
    let commits = vec![("h".to_string(), "msg\n".to_string())];
    let bare = revision::RevisionRef::from_query_row(&serde_json::json!({"id": 5}));
    let merge_strings = revision_merge_strings(std::slice::from_ref(&bare));
    let map = commit_merge_map(&commits, std::slice::from_ref(&bare), &merge_strings);
    assert!(map.is_empty());
}

#[test]
fn t1170_find_revision_by_phid_reports_both_ways() {
    let refs = vec![revision(1, "PHID-DREV-1", "one", false)];
    assert_eq!(find_revision_by_phid(&refs, "PHID-DREV-1"), Some(0));
    assert_eq!(find_revision_by_phid(&refs, "PHID-DREV-9"), None);
}

#[test]
fn t1170_paired_indexes_skip_unknown_phids() {
    let commits = vec![("h".to_string(), "m\n".to_string())];
    let merge_map = HashMap::from([("PHID-DREV-9".to_string(), vec![0usize])]);
    let (result, merged) = paired_revision_indexes(&commits, &[], &merge_map);
    assert!(result.is_empty());
    assert!(merged.is_empty());
}

#[test]
fn t1170_commit_marker_list_ignores_empty_groups() {
    let commits = vec![("h".to_string(), "m\n".to_string())];
    let item = Item {
        commit: Some(0),
        ..Item::default()
    };
    let markers = HashMap::from([("h".to_string(), Vec::new())]);
    assert!(commit_marker_list(&item, &commits, &markers).is_none());
}

#[test]
fn t1170_commit_range_label_handles_empty_views() {
    assert_eq!(commit_range_label(&[]), "");
    assert_eq!(
        commit_range_label(&[("abc123456789".to_string(), "m".to_string())]),
        "abc1234"
    );
}

#[test]
fn t1170_marker_cell_geometry_caps_depth_by_width() {
    assert_eq!(marker_cell_geometry(99, Some(80)), (8, 16));
    assert_eq!(marker_cell_geometry(1, Some(80)), (2, 22));
}

#[test]
fn t1170_first_row_path_marks_open_parents() {
    assert_eq!(first_row_path(2, false, false), "┗━┯ ");
    assert_eq!(first_row_path(0, false, true), "• ");
}

#[test]
fn t1170_marker_item_path_indents_nested_rows() {
    let item = Item::default();
    assert_eq!(marker_item_path(&item, false, true, 2, false), "  └ ");
    assert_eq!(marker_item_path(&item, false, true, 0, false), "└ ");
}

#[test]
fn t1170_draw_view_row_indents_deep_views() {
    let commits = vec![("aaa1111".to_string(), "first\n".to_string())];
    let items = vec![Item {
        commit: Some(0),
        ..Item::default()
    }];
    let cells = draw_view_row(&commits, &[], items, 1, 2, false, Some(80), false);
    let marker = cells[0].display_lines().join("\n");
    assert!(marker.starts_with("  "), "got: {marker:?}");
}

#[test]
fn t1170_collapse_placeholders_render_in_commits_and_messages() {
    let commits = vec![("aaa1111".to_string(), "first\n".to_string())];
    let items = vec![Item {
        commit: Some(0),
        collapse: Some(12),
        ..Item::default()
    }];
    let cells = draw_view_row(&commits, &[], items, 1, 0, false, Some(80), false);
    let commits_cell = cells[1].display_lines().join("\n");
    assert!(commits_cell.contains(':'), "got: {commits_cell:?}");
    let messages = cells[4].display_lines().join("\n");
    assert!(
        messages.contains("<... 12 more commits ...>"),
        "got: {messages}"
    );
}

#[test]
fn t1170_render_partition_requires_state_for_every_hash() {
    let mut graph = commitgraph::CommitGraph::new();
    commitgraph::graph_from_corpus(&mut graph, "A>B B>C").unwrap();
    let heads = vec!["C".to_string()];
    let partitions = commitgraph::partition_query(&graph, &heads, None).unwrap();
    let mut counter = 1;
    let sets =
        commitgraph::set_query(&graph, &partitions[0], &HashMap::new(), &mut counter).unwrap();
    let err = render_partition(
        &sets,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        Some(80),
        false,
    )
    .unwrap_err();
    assert!(err.contains("Found no state ref"), "{err}");
}

#[test]
fn t1170_render_partition_skips_grouped_non_holders() {
    let mut graph = commitgraph::CommitGraph::new();
    commitgraph::graph_from_corpus(&mut graph, "A>B B>C C>D D>E E>F").unwrap();
    let heads = vec!["F".to_string()];
    let partitions = commitgraph::partition_query(&graph, &heads, None).unwrap();
    let waypoints = HashMap::from([
        ("B".to_string(), vec![2u32]),
        ("C".to_string(), vec![3]),
        ("D".to_string(), vec![4]),
        ("E".to_string(), vec![5]),
        ("F".to_string(), vec![6]),
    ]);
    let mut counter = 1;
    let sets = commitgraph::set_query(&graph, &partitions[0], &waypoints, &mut counter).unwrap();
    assert!(sets.len() >= 4, "got {} sets", sets.len());
    let states = HashMap::from([
        ("A".to_string(), "a\n".to_string()),
        ("B".to_string(), "b\n".to_string()),
        ("C".to_string(), "c\n".to_string()),
        ("D".to_string(), "d\n".to_string()),
        ("E".to_string(), "e\n".to_string()),
        ("F".to_string(), "f\n".to_string()),
    ]);
    let rows = render_partition(
        &sets,
        &states,
        &HashMap::new(),
        &HashMap::new(),
        Some(80),
        false,
    )
    .unwrap();
    assert!(!rows.is_empty());
}

#[test]
fn t1170_mark_set_ancestors_skips_visited_parents() {
    let mut graph = commitgraph::CommitGraph::new();
    commitgraph::graph_from_corpus(&mut graph, "A>B B>C").unwrap();
    let heads = vec!["C".to_string()];
    let partitions = commitgraph::partition_query(&graph, &heads, None).unwrap();
    let waypoints = HashMap::from([("C".to_string(), vec![7u32])]);
    let mut counter = 1;
    let sets = commitgraph::set_query(&graph, &partitions[0], &waypoints, &mut counter).unwrap();
    assert_eq!(sets.len(), 2);
    let mut active = HashMap::from([(sets[0].set_id(), ()), (sets[1].set_id(), ())]);
    mark_set_ancestors(&sets, &mut active);
    assert_eq!(active.len(), 2);
}

#[test]
fn t1170_sort_sets_treats_empty_marker_groups_as_absent() {
    let mut graph = commitgraph::CommitGraph::new();
    commitgraph::graph_from_corpus(&mut graph, "A>B B>C").unwrap();
    let heads = vec!["C".to_string()];
    let partitions = commitgraph::partition_query(&graph, &heads, None).unwrap();
    let mut counter = 1;
    let base =
        commitgraph::set_query(&graph, &partitions[0], &HashMap::new(), &mut counter).unwrap();
    let groups = HashMap::from([("A".to_string(), Vec::new())]);
    let mut sets = base.clone();
    let with_empty = sort_sets(&graph, &mut sets, &groups);
    let mut sets = base.clone();
    let without = sort_sets(&graph, &mut sets, &HashMap::new());
    assert_eq!(with_empty.sort_string(), without.sort_string());
}

#[test]
fn t1170_workflow_queries_reject_unknown_vcs_and_cover_hg_tails() {
    let dir = TempDir::new("workflow-q");
    let hg = crate::hg::HgCommand::custom("true", &[]);
    let err =
        query_workflow_markers(dir.path(), &hg, "svn", MARKER_TYPE_BRANCH, false).unwrap_err();
    assert!(err.contains("does not support branches"), "{err}");
    let hg = fake_hg_stdout("node1\nnode2\n");
    assert_eq!(
        query_workflow_tails(dir.path(), &hg, "hg", None).unwrap(),
        vec!["node1".to_string(), "node2".to_string()]
    );
}

#[test]
fn t1170_run_head_query_reads_hg_graphs() {
    let dir = TempDir::new("headquery");
    let mut graph = commitgraph::CommitGraph::new();
    let hg = crate::hg::HgCommand::custom("true", &[]);
    let heads = vec!["abc".to_string()];
    let seen = run_head_query(&mut graph, dir.path(), &hg, "hg", &heads, &[]).unwrap();
    assert!(seen.is_empty());
}

#[test]
fn t1170_load_disjoint_heads_skips_known_and_loads_missing() {
    let dir = TempDir::new("disjoint-empty");
    let mut graph = commitgraph::CommitGraph::new();
    let hg = crate::hg::HgCommand::custom("true", &[]);
    let mut nodes = vec!["a".to_string()];
    load_disjoint_heads(
        &mut graph,
        dir.path(),
        &hg,
        "git",
        &["a".to_string()],
        &mut nodes,
    )
    .unwrap();
    assert_eq!(nodes, vec!["a".to_string()]);
    // Unknown heads load exactly, without duplicating known nodes.
    let repo = init_repo("disjoint-git");
    let sha = git(repo.path(), &["rev-parse", "HEAD"]);
    let mut graph = commitgraph::CommitGraph::new();
    let mut nodes = Vec::new();
    load_disjoint_heads(
        &mut graph,
        repo.path(),
        &hg,
        "git",
        std::slice::from_ref(&sha),
        &mut nodes,
    )
    .unwrap();
    assert_eq!(nodes, vec![sha]);
    // The hg arm resolves errors to empty, keeping known nodes.
    let mut graph = commitgraph::CommitGraph::new();
    let mut nodes = vec!["keep".to_string()];
    load_disjoint_heads(
        &mut graph,
        dir.path(),
        &hg,
        "hg",
        &["missing".to_string()],
        &mut nodes,
    )
    .unwrap();
    assert_eq!(nodes, vec!["keep".to_string()]);
}

#[test]
fn t1170_list_git_branches_needs_a_repo() {
    let empty = TempDir::new("listbranches-empty");
    assert_eq!(
        list_git_branches(empty.path()).unwrap_err(),
        "not inside a git repository"
    );
    let repo = init_repo("listbranches");
    let markers = list_git_branches(repo.path()).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "master");
}

#[test]
fn t1170_list_with_reads_hg_bookmarks_and_branches() {
    let dir = TempDir::new("listhg");
    std::fs::create_dir(dir.path().join(".hg")).unwrap();
    let body = r#"[
        {"type": "bookmark", "name": "wip", "node": "bbb", "isActive": true, "description": "work"},
        {"type": "branch", "name": "main", "node": "aaa", "isActive": false, "description": "line"}
    ]"#;
    let markers = list_with(dir.path(), MARKER_TYPE_BOOKMARK, &fake_hg_stdout(body)).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "wip");
    assert!(markers[0].active);
    let markers = list_with(dir.path(), MARKER_TYPE_BRANCH, &fake_hg_stdout(body)).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "main");
}

#[test]
fn t1170_detect_vcs_covers_hg_svn_and_bare_dirs() {
    let hg = TempDir::new("vcs-hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(detect_vcs(hg.path()), Some("hg"));
    let svn = TempDir::new("vcs-svn");
    std::fs::create_dir(svn.path().join(".svn")).unwrap();
    assert_eq!(detect_vcs(svn.path()), Some("svn"));
    let bare = TempDir::new("vcs-bare");
    assert_eq!(detect_vcs(bare.path()), None);
    let repo = init_repo("vcs-git");
    assert_eq!(detect_vcs(repo.path()), Some("git"));
}

#[test]
fn t1170_check_vcs_support_rejects_bare_and_svn() {
    let bare = TempDir::new("support-bare");
    let err = check_vcs_support(bare.path(), MARKER_TYPE_BRANCH).unwrap_err();
    assert!(err.contains("not inside a working copy"), "{err}");
    let svn = TempDir::new("support-svn");
    std::fs::create_dir(svn.path().join(".svn")).unwrap();
    let err = check_vcs_support(svn.path(), MARKER_TYPE_BRANCH).unwrap_err();
    assert!(err.contains("does not support branches"), "{err}");
    let hg = TempDir::new("support-hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(
        check_vcs_support(hg.path(), MARKER_TYPE_BOOKMARK).unwrap(),
        "hg"
    );
    assert_eq!(
        check_markers_support(hg.path(), None).unwrap(),
        MARKER_TYPE_BOOKMARK
    );
    assert!(check_markers_support(bare.path(), None).is_err());
}

#[test]
fn t1170_execute_with_client_routes_help_and_parse_errors() {
    let dir = TempDir::new("execwc");
    let help = execute_with_client(dir.path(), &args(&["--help"]), Some(80), None).unwrap();
    assert!(help.contains("usage: arc markers"), "{help}");
    let err = execute_with_client(dir.path(), &args(&["--bogus"]), Some(80), None).unwrap_err();
    assert!(err.contains("--bogus"), "{err}");
}

// -( T1170 wave 5d )-----------------------------------------------------------

#[test]
fn t1170_query_markers_system_needs_checkout() {
    // Outside a checkout the system-hg entry fails before spawning,
    // so no assertion touches the ambient `hg` backend.
    let empty = TempDir::new("syshg-empty");
    assert_eq!(
        query_hg_markers(empty.path(), MARKER_TYPE_BRANCH).unwrap_err(),
        "not inside a Mercurial working copy"
    );
}

#[test]
fn t1170_merge_id_query_refs_skips_unknown_ids() {
    // The id query may return rows for revisions no state mentions
    // (stale server data); they are skipped, not merged.
    let states = vec![StateCommit {
        hash: "abc".to_string(),
        message: "change\n\nDifferential Revision: D7".to_string(),
    }];
    let query_rows = r#"[{"id": 7, "phid": "PHID-DREV-7", "hashes": [["gtcm", "abc"]]}]"#;
    let hit = serde_json::json!({
        "id": 7,
        "phid": "PHID-DREV-7",
        "fields": {
            "title": "change",
            "status": {"value": "needs-review", "name": "Needs Review"},
        },
    });
    let stranger = serde_json::json!({
        "id": 999,
        "phid": "PHID-DREV-999",
        "fields": {
            "title": "stranger",
            "status": {"value": "needs-review", "name": "Needs Review"},
        },
    });
    let (base, rx) = serve_seq(vec![
        format!(r#"{{"result": {query_rows}, "error_code": null}}"#),
        format!(r#"{{"result": {{"data": [{hit}, {stranger}]}}, "error_code": null}}"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let merged = load_revision_refs_for_vcs(Some(&client), &states, "git").unwrap();
    let refs = merged.get("abc").unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].id(), 7);
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
}

#[test]
fn t1170_run_head_query_propagates_hg_failures() {
    // Spawn failures resolve to empty output; unparseable log output
    // is what fails the query.
    let dir = TempDir::new("headquery-fail");
    let mut graph = commitgraph::CommitGraph::new();
    let hg = fake_hg_stdout("bogus\n");
    let heads = vec!["abc".to_string()];
    let err = run_head_query(&mut graph, dir.path(), &hg, "hg", &heads, &[]).unwrap_err();
    assert!(!err.is_empty(), "{err}");
}

#[test]
fn t1170_load_disjoint_heads_propagates_hg_failures() {
    let dir = TempDir::new("disjoint-fail");
    let mut graph = commitgraph::CommitGraph::new();
    let hg = fake_hg_stdout("bogus\n");
    let mut nodes = vec!["keep".to_string()];
    let err = load_disjoint_heads(
        &mut graph,
        dir.path(),
        &hg,
        "hg",
        &["missing".to_string()],
        &mut nodes,
    )
    .unwrap_err();
    assert!(!err.is_empty(), "{err}");
    assert_eq!(nodes, vec!["keep".to_string()]);
}

#[test]
fn t1170_execute_builds_tokenless_client_then_queries() {
    // With an explicit URI the tokenless setup succeeds offline and
    // the workflow proceeds to its conduit queries, which fail
    // against the dead port (no proxy: localhost is exempt).
    let repo = init_repo("exec-query");
    let sha = git(repo.path(), &["rev-parse", "HEAD"]);
    git(
        repo.path(),
        &["remote", "add", "origin", "https://h.test/repo.git"],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", &sha],
    );
    let err = execute(
        repo.path(),
        &args(&["--conduit-uri", "http://127.0.0.1:9/"]),
    )
    .unwrap_err();
    assert!(!err.is_empty(), "{err}");
}

// -( T1170 wave 6g batch D )--------------------------------------------------

#[test]
fn t1170_query_git_remotes_rejects_remote_less_checkouts() {
    // T1153 RB-D9: empty `git remote -v` output throws (`split_lines`
    // yields one empty line, which misses the remote pattern), so a
    // remote-less checkout fails instead of listing nothing. Pins the
    // invariant behind `published_git_hashes`' empty-remotes arm.
    let repo = init_repo("noremotes");
    let err = query_git_remotes(repo.path()).unwrap_err();
    assert!(err.contains("Failed to match remote pattern"), "{err}");
}

#[test]
fn t1170_remote_cache_markers_always_carry_remote_names() {
    // `query_git_markers(..., remote_cache = true)` keeps only
    // `refs/remotes/<remote>/...` records, each parsed with
    // `remote_name = Some(..)` (a bare `refs/remotes/<name>` record is
    // skipped, never emitted nameless). Pins the invariant behind
    // `published_git_hashes`' nameless-marker arm.
    let repo = init_repo("remotecache");
    let head = git(repo.path(), &["rev-parse", "HEAD"]);
    git(
        repo.path(),
        &["remote", "add", "origin", "https://example.test/repo.git"],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", &head],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/feature", &head],
    );
    let markers = query_git_markers(repo.path(), MARKER_TYPE_BRANCH, true).unwrap();
    assert_eq!(markers.len(), 2);
    for marker in &markers {
        assert_eq!(marker.remote_name.as_deref(), Some("origin"), "{marker:?}");
    }
}
// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
fn revision_helper_builds_published_refs() {
    // The `published` arm of the `revision` helper builds a
    // published-status ref (every other caller passes `false`).
    let published = revision(3, "PHID-DREV-3", "shipped", true);
    assert!(published.is_status_published());
    assert_eq!(published.status(), Some("published"));
    let review = revision(4, "PHID-DREV-4", "wip", false);
    assert!(!review.is_status_published());
}

#[test]
#[should_panic(expected = "failed:")]
fn git_helper_reports_failing_commands() {
    // A failing git invocation panics with the command and stderr.
    let tmp = TempDir::new("w6h-git-fail");
    git(tmp.path(), &["--bogus-flag-xyz"]);
}
