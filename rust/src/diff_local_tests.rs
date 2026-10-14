use super::*;
use std::collections::HashSet;

fn fields(pairs: &[(&str, FieldValue)]) -> CommitFields {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn text(value: &str) -> FieldValue {
    FieldValue::Text(value.to_string())
}

fn list(values: &[&str]) -> FieldValue {
    FieldValue::List(values.iter().map(|value| (*value).to_string()).collect())
}

fn log(hash: &str, message: &str) -> LogMessage {
    LogMessage {
        commit_hash: hash.to_string(),
        message: message.to_string(),
    }
}

fn entry(change_type: u32, file_type: u32, path: &str) -> ChangeEntry {
    ChangeEntry {
        change_type,
        file_type,
        old_path: None,
        away_paths: Vec::new(),
        current: diff::FileDiff {
            path: path.to_string(),
            added: 0,
            deleted: 0,
        },
    }
}

#[test]
fn revision_id_absent_without_tag() {
    assert_eq!(
        parse_revision_id_from_raw_corpus("Fix things\n\nNo tags here.\n"),
        Ok(None)
    );
    assert_eq!(parse_revision_id_from_raw_corpus(""), Ok(None));
}

#[test]
fn revision_id_accepts_bare_and_uri_forms() {
    assert_eq!(
        parse_revision_id_from_raw_corpus("Title\n\nDifferential Revision: D123\n"),
        Ok(Some(123))
    );
    assert_eq!(
        parse_revision_id_from_raw_corpus("differential revision: d45"),
        Ok(Some(45))
    );
    assert_eq!(
        parse_revision_id_from_raw_corpus(
            "Differential Revision: https://devtools.example.com/D123"
        ),
        Ok(Some(123))
    );
    assert_eq!(
        parse_revision_id_from_raw_corpus("Differential Revision: https://x.test/D7?foo=1#frag"),
        Ok(Some(7))
    );
    assert_eq!(
        parse_revision_id_from_raw_corpus("Differential Revision:    D11  \n"),
        Ok(Some(11))
    );
    assert_eq!(
        parse_revision_id_from_raw_corpus("Differential Revision:D13"),
        Ok(Some(13))
    );
    assert_eq!(
        parse_revision_id_from_raw_corpus("Differential Revision: /D9"),
        Ok(Some(9))
    );
}

#[test]
fn revision_id_first_match_wins() {
    assert_eq!(
        parse_revision_id_from_raw_corpus("Differential Revision: D1\nDifferential Revision: D2\n"),
        Ok(Some(1))
    );
}

#[test]
fn revision_id_rejects_invalid_values() {
    for bad in [
        "D0",
        "D",
        "Dabc",
        "D12x",
        "D 12",
        "https://x.test/D1/D2",
        "006",
    ] {
        let err = parse_revision_id_from_raw_corpus(&format!("T\nDifferential Revision: {bad}\n"))
            .unwrap_err();
        assert!(
            err.contains("Invalid \"Differential Revision\" field")
                && err.contains(bad)
                && err.contains("D123"),
            "{bad}: {err}"
        );
    }
    assert_eq!(
        parse_revision_id_from_raw_corpus("T\nDifferential Revision: D0\n").unwrap_err(),
        "Invalid \"Differential Revision\" field in commit message. This field \
         should have a revision identifier like \"D123\" or a server URI \
         like \"https://devtools.example.com/D123\", but has \"D0\"."
    );
}

#[test]
fn commit_message_parses_git_svn_id() {
    let message = CommitMessage::new_from_raw_corpus(
        "Subject\n\ngit-svn-id: https://svn.example.com/repo/trunk@1234 \
         01234567-89ab-cdef-0123-456789abcdef\n",
    )
    .unwrap();
    assert_eq!(
        message.git_svn_base_revision.as_deref(),
        Some("https://svn.example.com/repo/trunk@1234")
    );
    assert_eq!(
        message.git_svn_base_path.as_deref(),
        Some("https://svn.example.com/repo/trunk")
    );
    assert_eq!(
        message.git_svn_uuid.as_deref(),
        Some("01234567-89ab-cdef-0123-456789abcdef")
    );
    assert_eq!(message.revision_id, None);
    assert!(message.raw_corpus.starts_with("Subject"));
}

#[test]
fn commit_message_git_svn_edge_cases() {
    let plain = CommitMessage::new_from_raw_corpus("Just words\n").unwrap();
    assert_eq!(plain.git_svn_base_revision, None);
    assert_eq!(plain.git_svn_base_path, None);
    assert_eq!(plain.git_svn_uuid, None);

    // Case-sensitive tag.
    let upper = CommitMessage::new_from_raw_corpus("Git-SVN-Id: u@1 w\n").unwrap();
    assert_eq!(upper.git_svn_base_revision, None);

    // Malformed lines are ignored.
    for bad in [
        "git-svn-id: no-at-sign",
        "git-svn-id: @1 u",
        "git-svn-id: u@x w",
    ] {
        let parsed = CommitMessage::new_from_raw_corpus(bad).unwrap();
        assert_eq!(parsed.git_svn_base_revision, None, "{bad}");
    }

    // First match wins; no space after the colon is fine.
    let multi =
        CommitMessage::new_from_raw_corpus("git-svn-id:a@1 u1\ngit-svn-id: b@2 u2\n").unwrap();
    assert_eq!(multi.git_svn_base_revision.as_deref(), Some("a@1"));

    assert!(CommitMessage::new_from_raw_corpus("T\nDifferential Revision: bogus\n").is_err());
    assert_eq!(revision_monogram(123), "D123");
}

#[test]
fn remove_comments_strips_trailing_block_only() {
    assert_eq!(remove_comments("Hello\n# comment\n# more\n"), "Hello\n");
    assert_eq!(remove_comments("Hello\n\nText\n# c\n"), "Hello\n\nText\n");
    assert_eq!(remove_comments("# lead\nbody\n"), "# lead\nbody\n");
    assert_eq!(
        remove_comments("body\nmiddle # not comment\n# tail\n"),
        "body\nmiddle # not comment\n"
    );
    assert_eq!(remove_comments(""), "\n");
    assert_eq!(remove_comments("# only\n# comments\n"), "\n");
    assert_eq!(remove_comments("a\n\n# c"), "a\n");
    assert_eq!(remove_comments("no newline"), "no newline\n");
}

#[test]
fn field_tags_parse_title_and_known_tags() {
    let parsed = parse_field_tags(
        "Add retry helper\n\
         \n\
         Some description here.\n\
         \n\
         Test Plan: cargo test uploader\n  second line of plan\n\
         Reviewers: alice, bob\n\
         CC: carol\n\
         Tasks: T1, T2\n\
         Summary: Explicit summary.\n\
         Differential Revision: D5\n",
    );
    assert_eq!(parsed.get(FIELD_TITLE), Some(&text("Add retry helper")));
    assert_eq!(
        parsed.get(FIELD_TEST_PLAN),
        Some(&text("cargo test uploader\n  second line of plan"))
    );
    assert_eq!(parsed.get(FIELD_REVIEWERS), Some(&list(&["alice", "bob"])));
    assert_eq!(parsed.get(FIELD_CCS), Some(&list(&["carol"])));
    assert_eq!(parsed.get(FIELD_TASKS), Some(&list(&["T1", "T2"])));
    assert_eq!(
        parsed.get(FIELD_SUMMARY),
        Some(&text("Some description here.\n\nExplicit summary."))
    );
    assert!(!parsed.contains_key(FIELD_REVISION_ID));
}

#[test]
fn field_tags_handle_edges() {
    assert!(parse_field_tags("").is_empty());

    // A tag-looking first line is still the title.
    let first = parse_field_tags("Reviewers: bob\n");
    assert_eq!(first.get(FIELD_TITLE), Some(&text("Reviewers: bob")));
    assert!(!first.contains_key(FIELD_REVIEWERS));

    // Unknown tags are preserved as body text.
    let unknown = parse_field_tags("Title\n\nFoo: bar\n");
    assert_eq!(unknown.get(FIELD_SUMMARY), Some(&text("Foo: bar")));

    // The Differential Revision line is dropped, not bodied.
    let revision = parse_field_tags("T\nDifferential Revision: D1\n");
    assert_eq!(revision.len(), 1);

    // Repeated tags accumulate.
    let repeated = parse_field_tags("T\nReviewers: a\n\nReviewers: b\n");
    assert_eq!(repeated.get(FIELD_REVIEWERS), Some(&list(&["a", "b"])));
    let summaries = parse_field_tags("T\nSummary: one\n\nSummary: two\n");
    assert_eq!(summaries.get(FIELD_SUMMARY), Some(&text("one\n\ntwo")));

    // Empty values are dropped.
    let empty = parse_field_tags("T\nReviewers:\nTest Plan:\nTasks: , ,\n");
    assert_eq!(empty.len(), 1);

    // Blank first line means no title.
    let blank = parse_field_tags("\nBody text\n");
    assert!(!blank.contains_key(FIELD_TITLE));
    assert_eq!(blank.get(FIELD_SUMMARY), Some(&text("Body text")));
}

#[test]
fn faux_flag_message_builds_placeholder_block() {
    assert_eq!(
        faux_flag_message(Some("a, b"), None).as_deref(),
        Some("<placeholder>\n\nReviewers: a, b")
    );
    assert_eq!(
        faux_flag_message(None, Some("c")).as_deref(),
        Some("<placeholder>\n\nCC: c")
    );
    assert_eq!(
        faux_flag_message(Some("a"), Some("c")).as_deref(),
        Some("<placeholder>\n\nReviewers: a\n\nCC: c")
    );
    assert_eq!(faux_flag_message(None, None), None);
    assert_eq!(faux_flag_message(Some(""), Some("")), None);
}

#[test]
fn faux_flag_constants_match_php() {
    assert_eq!(PLACEHOLDER_TITLE, "<placeholder>");
    assert_eq!(FLAGS_COMMIT_HASH, "(Flags)     ");
    assert_eq!(FLAGS_COMMIT_HASH.len(), 12);
    assert_eq!(FLAGS_SUMMARY, "Command-Line Flags");
    assert_eq!(
        included_commit_line("abcdef1234567890", "Do thing"),
        "abcdef123456 Do thing"
    );
    assert_eq!(included_commit_line("abc", "x"), "abc x");
}

#[test]
fn merge_fields_first_title_wins_and_demotes() {
    let old = fields(&[
        ("title", text("Old title")),
        ("summary", text("Old summary")),
        (FIELD_REVIEWERS, list(&["a"])),
    ]);
    let new = fields(&[
        ("title", text("New title")),
        ("summary", text("New summary")),
        (FIELD_REVIEWERS, list(&["b"])),
    ]);
    let merged = merge_fields_oldest_first(&[old, new]);
    assert_eq!(merged.get(FIELD_TITLE), Some(&text("Old title")));
    assert_eq!(
        merged.get(FIELD_SUMMARY),
        Some(&text("Old summary\n\nNew title\n\nNew summary"))
    );
    assert_eq!(merged.get(FIELD_REVIEWERS), Some(&list(&["a", "b"])));
}

#[test]
fn merge_fields_skips_placeholders_and_blanks() {
    let flags = fields(&[
        ("title", text("<placeholder>")),
        (FIELD_REVIEWERS, list(&["x"])),
    ]);
    let real = fields(&[
        ("title", text("Real")),
        ("summary", text("   ")),
        (FIELD_TEST_PLAN, text("plan")),
    ]);
    let merged = merge_fields_oldest_first(&[flags, real]);
    assert_eq!(merged.get(FIELD_TITLE), Some(&text("Real")));
    assert!(!merged.contains_key(FIELD_SUMMARY));
    assert_eq!(merged.get(FIELD_TEST_PLAN), Some(&text("plan")));
    assert_eq!(merged.get(FIELD_REVIEWERS), Some(&list(&["x"])));

    assert!(merge_fields_oldest_first(&[]).is_empty());

    // Shape mixes resolve to the newer value instead of fataling.
    let mixed =
        merge_fields_oldest_first(&[fields(&[("k", text("a"))]), fields(&[("k", list(&["b"]))])]);
    assert_eq!(mixed.get("k"), Some(&list(&["b"])));
}

#[test]
fn usable_message_selection_stops_at_known_commits() {
    let empty = HashSet::new();
    assert_eq!(
        select_usable_messages(&[log("h1", "only")], &empty).unwrap(),
        Vec::<String>::new()
    );
    assert!(select_usable_messages(&[], &empty).unwrap().is_empty());

    let messages = vec![log("h1", "new work"), log("h2", "older work")];
    assert_eq!(
        select_usable_messages(&messages, &empty).unwrap(),
        vec!["new work".to_string(), "older work".to_string()]
    );

    let revised = vec![
        log("h1", "new work"),
        log("h2", "old\nDifferential Revision: D3\n"),
    ];
    assert_eq!(
        select_usable_messages(&revised, &empty).unwrap(),
        vec!["new work".to_string()]
    );

    let active: HashSet<String> = ["h1".to_string()].into_iter().collect();
    assert!(
        select_usable_messages(&messages, &active)
            .unwrap()
            .is_empty()
    );

    let invalid = vec![log("h1", "ok"), log("h2", "Differential Revision: nope")];
    assert!(select_usable_messages(&invalid, &empty).is_err());
}

#[test]
fn usable_logs_format_oldest_first_with_bullets() {
    assert_eq!(
        format_usable_logs(&["new msg\nsecond line".to_string(), "old msg".to_string()]),
        "  - old msg\n  - new msg\n"
    );
    assert_eq!(format_usable_logs(&[]), "");
    assert_eq!(format_usable_logs(&["  \n ".to_string()]), "  - \n");
}

#[test]
fn update_template_matches_php_sprintf() {
    assert_eq!(
        default_update_message_template("  - did thing\n", 7, "Title"),
        "  - did thing\n\
         \n\
         # Updating D7: Title\n\
         #\n\
         # Enter a brief description of the changes included in this update.\n\
         # The first line is used as subject, next lines as comment.\n\
         #\n\
         # If you intended to create a new revision, use:\n\
         #  $ arc diff --create\n\n"
    );
}

#[test]
fn raw_update_message_guard_reports_exact_error() {
    assert_eq!(
        RAW_UPDATE_MESSAGE_ERROR,
        "When using '--raw' to update a revision, specify an update message with \
         '--message'. (Normally, we'd launch an editor to ask you for a message, but can \
         not do that because stdin is the diff source.)"
    );
    assert_eq!(
        require_non_raw_update_message(true).unwrap_err(),
        RAW_UPDATE_MESSAGE_ERROR
    );
    assert!(require_non_raw_update_message(false).is_ok());
}

#[test]
fn base_commit_argument_allows_exactly_one() {
    let none: Vec<String> = vec![];
    assert_eq!(parse_base_commit_argument(true, &none).unwrap(), None);
    assert_eq!(
        parse_base_commit_argument(true, &["abc".to_string()]).unwrap(),
        Some("abc".to_string())
    );
    assert_eq!(
        parse_base_commit_argument(true, &["a".to_string(), "b".to_string()]).unwrap_err(),
        "Specify exactly one base commit. The end of the commit range is always the working \
         copy state."
    );
    // A VCS without commit ranges rejects even one argument, but no
    // argument stays a no-op.
    assert_eq!(parse_base_commit_argument(false, &none).unwrap(), None);
    assert_eq!(
        parse_base_commit_argument(false, &["abc".to_string()]).unwrap_err(),
        "This version control system does not support commit ranges."
    );
}

#[test]
fn git_constants_match_php() {
    assert_eq!(
        GIT_MAGIC_ROOT_COMMIT,
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    );
    assert_eq!(SEARCH_LENGTH_FOR_PARENT_REVISIONS, 16);
}

#[test]
fn revision_id_normalization_strips_one_d() {
    assert_eq!(normalize_revision_id("D123"), "123");
    assert_eq!(normalize_revision_id("d1"), "1");
    assert_eq!(normalize_revision_id("123"), "123");
    assert_eq!(normalize_revision_id("DD1"), "D1");
    assert_eq!(normalize_revision_id(""), "");
}

#[test]
fn update_revision_id_requires_digits() {
    assert_eq!(parse_update_revision_id("D123").unwrap(), 123);
    assert_eq!(parse_update_revision_id("123").unwrap(), 123);
    assert_eq!(parse_update_revision_id("D5 ").unwrap(), 5);
    assert_eq!(parse_update_revision_id(" 5").unwrap(), 5);
    assert_eq!(parse_update_revision_id("0").unwrap(), 0);
    for bad in [
        "",
        "D",
        " D5",
        "1e3",
        "12.3",
        "+123",
        "D12x",
        "99999999999999999999999",
    ] {
        assert_eq!(
            parse_update_revision_id(bad).unwrap_err(),
            "Parameter to --update must be a Differential Revision number.",
            "{bad}"
        );
    }
}

#[test]
fn summary_characters_cover_every_change_type() {
    let cases = [
        (TYPE_ADD, 'A'),
        (TYPE_CHANGE, 'M'),
        (TYPE_DELETE, 'D'),
        (TYPE_MOVE_AWAY, 'V'),
        (TYPE_COPY_AWAY, 'P'),
        (TYPE_MOVE_HERE, 'V'),
        (TYPE_COPY_HERE, 'P'),
        (TYPE_MULTICOPY, 'P'),
        (TYPE_MESSAGE, 'Q'),
        (TYPE_CHILD, '@'),
    ];
    assert_eq!(
        (
            TYPE_ADD,
            TYPE_CHANGE,
            TYPE_DELETE,
            TYPE_MOVE_AWAY,
            TYPE_COPY_AWAY
        ),
        (1, 2, 3, 4, 5)
    );
    assert_eq!(
        (
            TYPE_MOVE_HERE,
            TYPE_COPY_HERE,
            TYPE_MULTICOPY,
            TYPE_MESSAGE,
            TYPE_CHILD
        ),
        (6, 7, 8, 9, 10)
    );
    for (change_type, symbol) in cases {
        assert_eq!(summary_character_for_change_type(Some(change_type)), symbol);
    }
    assert_eq!(summary_character_for_change_type(Some(99)), '~');
    assert_eq!(summary_character_for_change_type(None), '~');
}

#[test]
fn short_names_cover_every_file_type() {
    assert_eq!(
        (
            FILE_TEXT,
            FILE_IMAGE,
            FILE_BINARY,
            FILE_DIRECTORY,
            FILE_SYMLINK,
            FILE_DELETED,
            FILE_NORMAL
        ),
        (1, 2, 3, 4, 5, 6, 7)
    );
    assert_eq!(short_name_for_file_type(Some(FILE_TEXT)), None);
    assert_eq!(short_name_for_file_type(Some(FILE_DIRECTORY)), Some("dir"));
    assert_eq!(short_name_for_file_type(Some(FILE_IMAGE)), Some("img"));
    assert_eq!(short_name_for_file_type(Some(FILE_BINARY)), Some("bin"));
    assert_eq!(short_name_for_file_type(Some(FILE_SYMLINK)), Some("sym"));
    assert_eq!(short_name_for_file_type(Some(FILE_DELETED)), Some("???"));
    assert_eq!(short_name_for_file_type(Some(FILE_NORMAL)), Some("???"));
    assert_eq!(short_name_for_file_type(Some(99)), Some("???"));
    assert_eq!(short_name_for_file_type(None), Some("???"));
}

#[test]
fn location_predicates_match_php_sets() {
    for change_type in [TYPE_MOVE_AWAY, TYPE_COPY_AWAY, TYPE_MULTICOPY] {
        assert!(is_old_location_change_type(change_type));
        assert!(!is_new_location_change_type(change_type));
    }
    for change_type in [TYPE_MOVE_HERE, TYPE_COPY_HERE] {
        assert!(is_new_location_change_type(change_type));
        assert!(!is_old_location_change_type(change_type));
    }
    for change_type in [
        TYPE_ADD,
        TYPE_CHANGE,
        TYPE_DELETE,
        TYPE_MESSAGE,
        TYPE_CHILD,
        99,
    ] {
        assert!(!is_old_location_change_type(change_type));
        assert!(!is_new_location_change_type(change_type));
    }
}

#[test]
fn text_summaries_follow_sprintf_layout() {
    assert_eq!(LOCATION_INDENT.len(), 13);
    assert_eq!(
        render_text_summary(&entry(TYPE_CHANGE, FILE_TEXT, "f.txt")),
        "M       f.txt"
    );
    assert_eq!(
        render_text_summary(&entry(TYPE_ADD, FILE_BINARY, "logo.png")),
        "A (bin) logo.png"
    );
    assert_eq!(
        render_text_summary(&entry(TYPE_CHANGE, FILE_SYMLINK, "link")),
        "M (sym) link"
    );
    assert_eq!(
        render_text_summary(&entry(TYPE_DELETE, FILE_TEXT, "gone.txt")),
        "D       gone.txt"
    );
    assert_eq!(
        render_text_summary(&entry(TYPE_CHANGE, 99, "f.txt")),
        "M (???) f.txt"
    );
    assert_eq!(
        render_text_summary(&entry(99, FILE_TEXT, "f.txt")),
        "~       f.txt"
    );

    let mut renamed = entry(TYPE_MOVE_HERE, FILE_TEXT, "new.txt");
    renamed.old_path = Some("old.txt".to_string());
    assert_eq!(
        render_text_summary(&renamed),
        "V       new.txt\n             from: old.txt"
    );

    let mut moved = entry(TYPE_MOVE_AWAY, FILE_TEXT, "old.txt");
    moved.away_paths = vec!["n1".to_string(), "n2".to_string()];
    assert_eq!(
        render_text_summary(&moved),
        "V       old.txt\n             to: n1\n             to: n2"
    );
}

#[test]
fn included_changes_block_matches_run_output() {
    let block = render_included_changes(&[
        entry(TYPE_ADD, FILE_TEXT, "a.txt"),
        entry(TYPE_DELETE, FILE_BINARY, "b.bin"),
    ]);
    assert_eq!(
        block,
        "Included changes:\n  A       a.txt\n  D (bin) b.bin\n"
    );
    assert_eq!(render_included_changes(&[]), "Included changes:\n");
}

#[test]
fn binary_heuristic_is_nul_presence() {
    assert!(!is_heuristic_binary_file(b"plain text\n"));
    assert!(!is_heuristic_binary_file(b""));
    assert!(is_heuristic_binary_file(b"a\0b"));
    assert!(is_heuristic_binary_file(b"\0"));
}

#[test]
fn gigantic_diff_limits_use_strict_comparison() {
    assert_eq!(GIGANTIC_DIFF_LIMIT_BYTES, 4 * 1024 * 1024);
    assert_eq!(
        gigantic_diff_paths(&[
            ("small", GIGANTIC_DIFF_LIMIT_BYTES),
            ("big", GIGANTIC_DIFF_LIMIT_BYTES + 1),
        ]),
        vec!["big".to_string()]
    );
    assert!(gigantic_diff_paths(&[]).is_empty());
    assert_eq!(
        gigantic_diff_warning("huge.dat", 5_000_000),
        "Diff for 'huge.dat' with context is 5000000 bytes in length. Generally, source \
         changes should not be this large."
    );
    assert_eq!(
        ABORTED_GIGANTIC_DIFF,
        "Aborted generation of gigantic diff."
    );
    assert_eq!(
        NO_CHANGES_FOUND,
        "No changes found. (Did you specify the wrong commit range?)"
    );
}

#[test]
fn full_diff_options_follow_php_order() {
    assert_eq!(
        DIFF_BASE_OPTIONS,
        &["--no-ext-diff", "--no-textconv", "--submodule=short"]
    );
    assert_eq!(DEFAULT_DIFF_CONTEXT_LINES, 0x7FFF);
    assert_eq!(
        DIFF_CONFIG_OPTIONS,
        &["-c", "diff.suppressBlankEmpty=false"]
    );
    assert_eq!(
        full_diff_options(3),
        [
            "--no-ext-diff",
            "--no-textconv",
            "--submodule=short",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U3",
            "-M",
            "-C",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
    );
}

#[test]
fn name_status_parses_plain_and_rename_records() {
    assert!(parse_name_status_z(b"").unwrap().is_empty());
    assert_eq!(
        parse_name_status_z(b"M\0f.txt\0A\0new.txt\0").unwrap(),
        vec![
            NameStatusEntry {
                status: 'M',
                old_path: None,
                path: "f.txt".to_string(),
            },
            NameStatusEntry {
                status: 'A',
                old_path: None,
                path: "new.txt".to_string(),
            },
        ]
    );
    assert_eq!(
        parse_name_status_z(b"R100\0old.txt\0new.txt\0C075\0a\0b\0").unwrap(),
        vec![
            NameStatusEntry {
                status: 'R',
                old_path: Some("old.txt".to_string()),
                path: "new.txt".to_string(),
            },
            NameStatusEntry {
                status: 'C',
                old_path: Some("a".to_string()),
                path: "b".to_string(),
            },
        ]
    );
    assert!(parse_name_status_z(b"M\0").is_err());
    assert_eq!(
        parse_name_status_z(b"R100\told\0").unwrap_err(),
        "truncated rename record 1"
    );
}

#[test]
fn local_commit_information_parses_framed_records() {
    assert!(parse_local_commit_information("").unwrap().is_empty());
    let record = [
        "c1",
        "t1",
        "p1 p2",
        "1700000000",
        "Author",
        "a@x.test",
        "Subject",
        "Subject\n\nBody\n  ",
    ]
    .join("\x01");
    let parsed = parse_local_commit_information(&format!("{record}\x02")).unwrap();
    assert_eq!(
        parsed,
        vec![LocalCommitInfo {
            commit: "c1".to_string(),
            tree: "t1".to_string(),
            parents: vec!["p1".to_string(), "p2".to_string()],
            time: "1700000000".to_string(),
            author: "Author".to_string(),
            summary: "Subject".to_string(),
            message: "Subject\n\nBody".to_string(),
            author_email: "a@x.test".to_string(),
        }]
    );
    assert_eq!(
        parsed[0].to_json(),
        serde_json::json!({
            "commit": "c1",
            "tree": "t1",
            "parents": ["p1", "p2"],
            "time": "1700000000",
            "author": "Author",
            "summary": "Subject",
            "message": "Subject\n\nBody",
            "authorEmail": "a@x.test",
        })
    );
    assert!(parse_local_commit_information("a\x01b\x02").is_err());
}

#[test]
fn hash_message_log_parses_pairs() {
    assert!(parse_hash_message_log("").unwrap().is_empty());
    assert_eq!(
        parse_hash_message_log("h1\x01msg one\x02h2\x01msg two\x02").unwrap(),
        vec![log("h1", "msg one"), log("h2", "msg two")]
    );
    assert!(parse_hash_message_log("lonely").is_err());
}

#[test]
fn parent_log_info_folds_history_newest_first() {
    let empty = parent_log_info(&[]);
    assert_eq!(
        empty,
        ParentLogInfo {
            parent: None,
            base_revision: None,
            base_path: None,
            uuid: None,
        }
    );
    assert_eq!(
        empty.to_json(),
        serde_json::json!({
            "parent": null,
            "base_revision": null,
            "base_path": null,
            "uuid": null,
        })
    );

    let info = parent_log_info(&[
        "Bump\nDifferential Revision: D4\n".to_string(),
        "Old\ngit-svn-id: http://s/r/trunk@77 uuid-1\n".to_string(),
        "Older\ngit-svn-id: http://s/r/trunk@78 uuid-2\n".to_string(),
    ]);
    assert_eq!(info.parent, Some(4));
    assert_eq!(info.base_revision.as_deref(), Some("http://s/r/trunk@77"));
    assert_eq!(info.base_path.as_deref(), Some("http://s/r/trunk"));
    // Scanning stopped once parent and base revision were known, so the
    // third message's UUID is never seen.
    assert_eq!(info.uuid.as_deref(), Some("uuid-1"));

    // Without a parent, scanning runs on: first git-svn data wins the base
    // while every later UUID overwrites.
    let svn_only = parent_log_info(&[
        "Old\ngit-svn-id: http://s/r/trunk@77 uuid-1\n".to_string(),
        "Older\ngit-svn-id: http://s/r/trunk@78 uuid-2\n".to_string(),
    ]);
    assert_eq!(svn_only.parent, None);
    assert_eq!(
        svn_only.base_revision.as_deref(),
        Some("http://s/r/trunk@77")
    );
    assert_eq!(svn_only.uuid.as_deref(), Some("uuid-2"));

    // Invalid revision fields are skipped, not fatal.
    let skipped = parent_log_info(&[
        "T\nDifferential Revision: nope\n".to_string(),
        "T2\nDifferential Revision: D9\n".to_string(),
    ]);
    assert_eq!(skipped.parent, Some(9));

    // Scanning stops once parent and base revision are both known.
    let early = parent_log_info(&[
        "A\nDifferential Revision: D1\ngit-svn-id: u@2 z1\n".to_string(),
        "B\ngit-svn-id: u@3 z2\n".to_string(),
    ]);
    assert_eq!(early.uuid.as_deref(), Some("z1"));
}

#[test]
fn branch_name_from_ref_requires_heads_prefix() {
    assert_eq!(
        branch_name_from_ref("refs/heads/main").as_deref(),
        Some("main")
    );
    assert_eq!(branch_name_from_ref("refs/heads/"), None);
    assert_eq!(branch_name_from_ref("main"), None);
    assert_eq!(branch_name_from_ref("refs/tags/v1"), None);
}

#[test]
fn lint_unit_status_words_match_php_maps() {
    assert_eq!(
        (
            LINT_RESULT_OKAY,
            LINT_RESULT_WARNINGS,
            LINT_RESULT_ERRORS,
            LINT_RESULT_SKIP
        ),
        (0, 1, 2, 3)
    );
    assert_eq!(
        (
            UNIT_RESULT_OKAY,
            UNIT_RESULT_UNSOUND,
            UNIT_RESULT_FAIL,
            UNIT_RESULT_SKIP
        ),
        (0, 1, 2, 3)
    );
    assert_eq!(lint_status(Some(0)), "okay");
    assert_eq!(lint_status(Some(2)), "fail");
    assert_eq!(lint_status(Some(1)), "warn");
    assert_eq!(lint_status(Some(3)), "skip");
    assert_eq!(lint_status(None), "none");
    assert_eq!(lint_status(Some(99)), "none");
    assert_eq!(unit_status(Some(0)), "okay");
    assert_eq!(unit_status(Some(2)), "fail");
    assert_eq!(unit_status(Some(1)), "warn");
    assert_eq!(unit_status(Some(3)), "skip");
    assert_eq!(unit_status(None), "none");
    assert_eq!(unit_status(Some(99)), "none");
    assert_eq!(harbormaster_type_from_result(0), "pass");
    assert_eq!(harbormaster_type_from_result(3), "pass");
    assert_eq!(harbormaster_type_from_result(1), "fail");
    assert_eq!(harbormaster_type_from_result(2), "fail");
    assert_eq!(harbormaster_type_from_result(99), "fail");
}

#[test]
fn skip_predicates_cover_nolint_nounit_raw_head() {
    assert!(should_skip_lint(true, false, None));
    assert!(should_skip_lint(false, true, None));
    assert!(should_skip_lint(false, false, Some("abc")));
    assert!(!should_skip_lint(false, false, None));
    assert!(!should_skip_lint(false, false, Some("")));
    assert!(should_skip_unit(true, false, None));
    assert!(should_skip_unit(false, true, None));
    assert!(should_skip_unit(false, false, Some("abc")));
    assert!(!should_skip_unit(false, false, None));
    assert!(!should_skip_unit(false, false, Some("")));
}

#[test]
fn passthru_argv_uses_spec_order() {
    let flags = ["amend-all".to_string(), "apply-patches".to_string()];
    assert_eq!(
        passthru_argv("lint", &flags),
        vec!["--apply-patches".to_string(), "--amend-all".to_string()]
    );
    assert_eq!(
        passthru_argv(
            "lint",
            &["never-apply-patches".to_string(), "bogus".to_string()]
        ),
        vec!["--never-apply-patches".to_string()]
    );
    assert_eq!(
        passthru_argv("unit", &["no-coverage".to_string()]),
        vec!["--no-coverage".to_string()]
    );
    assert!(passthru_argv("bogus", &flags).is_empty());
    assert!(passthru_argv("lint", &[]).is_empty());

    let base = vec!["--coverage".to_string()];
    assert_eq!(
        lint_unit_argv(&base, Some("abc")),
        vec![
            "--coverage".to_string(),
            "--rev".to_string(),
            "abc".to_string()
        ]
    );
    assert_eq!(lint_unit_argv(&[], None), Vec::<String>::new());
}

#[test]
fn lint_unit_report_uses_verbatim_keys() {
    let report = LintUnitReport {
        lint_result: Some(0),
        unresolved_lint: vec![serde_json::json!({"path": "a.php"})],
        unit_result: None,
        test_results: vec![],
    };
    assert_eq!(
        report.to_json(),
        serde_json::json!({
            "lintResult": 0,
            "unresolvedLint": [{"path": "a.php"}],
            "unitResult": null,
            "testResults": [],
        })
    );
    assert_eq!(DIFF_PROPERTY_LINT, "arc:lint");
    assert_eq!(DIFF_PROPERTY_UNIT, "arc:unit");
    assert_eq!(DIFF_PROPERTY_LOCAL_COMMITS, "local:commits");
    assert_eq!(DIFF_PROPERTY_ONTO, "arc:onto");
    assert_eq!(DIFF_PROPERTY_STAGING, "arc.staging");
    assert!(should_update_lint_property(false, true));
    assert!(!should_update_lint_property(true, true));
    assert!(!should_update_lint_property(false, false));
    assert!(should_update_unit_property(false, true));
    assert!(!should_update_unit_property(true, true));
    assert!(!should_update_unit_property(false, false));
    assert!(should_update_local_property(false, true));
    assert!(!should_update_local_property(true, true));
    assert!(!should_update_local_property(false, false));
    assert!(should_update_onto_property(true));
    assert!(!should_update_onto_property(false));
}

#[test]
fn diff_spec_renders_verbatim_keys() {
    let spec = DiffSpec {
        source_machine: "box".to_string(),
        source_path: Some("/repo".to_string()),
        branch: Some("main".to_string()),
        bookmark: None,
        source_control_system: Some(SOURCE_CONTROL_SYSTEM_GIT.to_string()),
        source_control_path: None,
        source_control_base_revision: Some("abc".to_string()),
        creation_method: CREATION_METHOD.to_string(),
        repository_phid: Some("PHID-REPO-1".to_string()),
    };
    assert_eq!(CREATION_METHOD, "arc");
    assert_eq!(SOURCE_CONTROL_SYSTEM_GIT, "git");
    let json = spec.to_json();
    assert_eq!(json["sourceMachine"], serde_json::json!("box"));
    assert_eq!(json["sourcePath"], serde_json::json!("/repo"));
    assert_eq!(json["branch"], serde_json::json!("main"));
    assert_eq!(json["bookmark"], serde_json::Value::Null);
    assert_eq!(json["sourceControlSystem"], serde_json::json!("git"));
    assert_eq!(json["sourceControlPath"], serde_json::Value::Null);
    assert_eq!(json["sourceControlBaseRevision"], serde_json::json!("abc"));
    assert_eq!(json["creationMethod"], serde_json::json!("arc"));
    assert_eq!(json["repositoryPHID"], serde_json::json!("PHID-REPO-1"));

    let bare = DiffSpec {
        source_machine: "box".to_string(),
        source_path: None,
        branch: None,
        bookmark: None,
        source_control_system: None,
        source_control_path: None,
        source_control_base_revision: None,
        creation_method: CREATION_METHOD.to_string(),
        repository_phid: None,
    };
    assert!(bare.to_json().get("repositoryPHID").is_none());
    assert!(!default_source_machine_name().is_empty());
}

#[test]
fn onto_targets_prefer_upstream_over_config() {
    assert_eq!(ONTO_KIND_UPSTREAM, "upstream");
    assert_eq!(DIFF_ONTO_CONFIG_KEY, "arc.land.onto.default");
    let upstream = diff_onto_targets(Some("origin/main"), Some("stable"));
    assert_eq!(upstream.len(), 1);
    assert_eq!(
        upstream[0].to_json(),
        serde_json::json!({"type": "branch", "name": "origin/main", "kind": "upstream"})
    );
    let configured = diff_onto_targets(None, Some("stable"));
    assert_eq!(
        configured[0].to_json(),
        serde_json::json!({
            "type": "branch",
            "name": "stable",
            "kind": "arc.land.onto.default",
        })
    );
    assert!(diff_onto_targets(None, None).is_empty());
    assert!(diff_onto_targets(Some(""), Some("")).is_empty());
}

#[test]
fn workflow_predicates_follow_php_branches() {
    assert!(is_raw_diff_source(true, None));
    assert!(is_raw_diff_source(false, Some("git diff")));
    assert!(!is_raw_diff_source(false, None));

    assert!(!should_only_create_diff(true, None, false, true));
    assert!(!should_only_create_diff(false, Some("123"), false, true));
    assert!(should_only_create_diff(false, None, true, false));
    assert!(should_only_create_diff(false, None, false, true));
    assert!(!should_only_create_diff(false, None, false, false));

    assert!(!should_amend(true, false, None, false));
    assert!(!should_amend(false, true, None, false));
    assert!(!should_amend(false, false, Some("abc"), false));
    assert!(!should_amend(false, false, None, true));
    assert!(should_amend(false, false, None, false));
}
