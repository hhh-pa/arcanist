use super::*;
use std::collections::HashSet;

/* G1 full-state tests: hg local commits and svn info. */

#[test]
fn g1_hg_author_and_commit_argv() {
    assert_eq!(
        parse_hg_author("plain name"),
        (Some("plain name".to_string()), None)
    );
    assert_eq!(
        parse_hg_author("A U Thor <a@example.test>"),
        (
            Some("A U Thor".to_string()),
            Some("a@example.test".to_string())
        )
    );
    assert_eq!(
        parse_hg_author("\"Quoted, Name\" <q@example.test>"),
        (
            Some("Quoted, Name".to_string()),
            Some("q@example.test".to_string())
        )
    );
    assert_eq!(
        parse_hg_author("bare@example.test"),
        (None, Some("bare@example.test".to_string()))
    );

    let argv = hg_local_commit_argv("abc", "default");
    assert_eq!(argv[0], "log");
    assert_eq!(argv[1], "--template");
    assert!(argv[2].contains("{node}\u{1}"), "{}", argv[2]);
    assert_eq!(argv[4], "(abc::. - abc)");
    assert_eq!(
        hg_parents_argv("node1"),
        vec![
            "parents".to_string(),
            "--template".to_string(),
            "{node}\n".to_string(),
            "--rev".to_string(),
            "node1".to_string(),
        ]
    );
}

#[test]
fn g1_parse_hg_local_commits() {
    let output = "node1\x011\x01A U Thor <a@t>\x01100 0\x01default\x01\x01\x01first desc\x02node2\x012\x01bare@t\x01200 0\x01default\x01\x01\x01second desc\nmore\x02garbage";
    let commits = parse_hg_local_commits(output);
    assert_eq!(commits.len(), 2);
    // Newest-first: node2 (no parents output) chains to node1.
    assert_eq!(commits[0].node, "node2");
    assert_eq!(commits[0].rev, "2");
    assert_eq!(commits[0].author, None);
    assert_eq!(commits[0].author_email.as_deref(), Some("bare@t"));
    assert_eq!(commits[0].time, 200);
    assert_eq!(commits[0].parents, Some(vec!["node1".to_string()]));
    assert_eq!(commits[0].summary, "second desc");
    assert_eq!(commits[1].node, "node1");
    assert_eq!(commits[1].author.as_deref(), Some("A U Thor"));
    // First record has no older node to chain to: lookup needed.
    assert_eq!(commits[1].parents, None);

    let json = commits[0].to_json();
    assert_eq!(json["commit"], serde_json::json!("node2"));
    assert_eq!(json["rev"], serde_json::json!("node2"));
    assert_eq!(json["local"], serde_json::json!("2"));
    assert_eq!(json["parents"], serde_json::json!(["node1"]));
    assert_eq!(json["author"], serde_json::Value::Null);

    // Non-empty {parents} always needs a lookup.
    let two_parent = "n1\x011\x01x\x010 0\x01b\x01\x01p1 p2\x01d\x02";
    assert_eq!(parse_hg_local_commits(two_parent)[0].parents, None);
    assert!(parse_hg_local_commits("").is_empty());
    assert!(parse_hg_local_commits("short\x01record\x02").is_empty());

    let bogus = std::path::Path::new("definitely-not-an-hg-repo-g1");
    assert!(get_hg_local_commits(bogus, "tip", "default").is_err());
}

#[test]
fn g1_svn_info_helpers() {
    let output = "Path: .\nWorking Copy Root Path: /repo\nURL: https://s.example/r/trunk\nRepository Root: https://s.example/r\nRevision: 42\nNode Kind: directory\n";
    let info = parse_svn_info(output);
    assert_eq!(
        info.get("URL").map(String::as_str),
        Some("https://s.example/r/trunk")
    );
    assert_eq!(
        info.get("Repository Root").map(String::as_str),
        Some("https://s.example/r")
    );
    assert_eq!(info.get("Revision").map(String::as_str), Some("42"));
    assert!(parse_svn_info("").is_empty());
    let crlf = parse_svn_info("URL: u\r\nRevision: 7\r\n");
    assert_eq!(crlf.get("Revision").map(String::as_str), Some("7"));

    let bogus = std::path::Path::new("definitely-not-an-svn-repo-g1");
    let err = get_svn_info(bogus, "/").unwrap_err();
    assert!(!err.is_empty(), "{err}");
}

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
fn gap2_run_lint_maps_codes_and_outcomes() {
    use std::cell::RefCell;
    let dir = std::env::temp_dir();
    let run = |outcome: ChildOutcome| move |_: &Path, _: &[String]| outcome.clone();
    // Skip short-circuits silently.
    let calls = RefCell::new(0);
    let counting = |_: &Path, _: &[String]| {
        *calls.borrow_mut() += 1;
        ChildOutcome::Code(0)
    };
    let mut out = Vec::new();
    assert_eq!(
        run_lint(&dir, Some("abc"), &[], true, &counting, &mut out).unwrap(),
        Some(LINT_RESULT_SKIP)
    );
    assert_eq!(*calls.borrow(), 0);
    assert!(out.is_empty());
    // Result codes map to banners.
    for (code, banner) in [
        (0, "** LINT OKAY ** No lint problems.\n"),
        (1, "** LINT MESSAGES ** Lint issued unresolved warnings.\n"),
        (2, "** LINT ERRORS ** Lint raised errors!\n"),
    ] {
        let mut out = Vec::new();
        assert_eq!(
            run_lint(
                &dir,
                None,
                &[],
                false,
                &run(ChildOutcome::Code(code)),
                &mut out
            )
            .unwrap(),
            Some(code)
        );
        assert_eq!(out, format!("Linting...\n{banner}").into_bytes());
    }
    // A SKIP code from the child prints no banner.
    let mut out = Vec::new();
    assert_eq!(
        run_lint(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(3)),
            &mut out
        )
        .unwrap(),
        Some(3)
    );
    assert_eq!(out, b"Linting...\n");
    // No engine / no effect print notices and yield None.
    let mut out = Vec::new();
    assert_eq!(
        run_lint(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::NoEngine),
            &mut out
        )
        .unwrap(),
        None
    );
    assert_eq!(
        out,
        b"Linting...\nNo lint engine configured for this project.\n"
    );
    let mut out = Vec::new();
    assert_eq!(
        run_lint(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::NoEffect("nothing to lint".to_string())),
            &mut out,
        )
        .unwrap(),
        None
    );
    assert_eq!(out, b"Linting...\nnothing to lint\n");
    // Child failures abort after the start notice.
    let mut out = Vec::new();
    assert_eq!(
        run_lint(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Failed("boom".to_string())),
            &mut out,
        )
        .unwrap_err(),
        "boom"
    );
    assert_eq!(out, b"Linting...\n");
    // Argv carries passthrough flags plus --rev.
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let capture = |_: &Path, argv: &[String]| {
        *seen.borrow_mut() = argv.to_vec();
        ChildOutcome::Code(0)
    };
    let flags = ["amend-all".to_string(), "apply-patches".to_string()];
    let mut out = Vec::new();
    run_lint(&dir, Some("abc123"), &flags, false, &capture, &mut out).unwrap();
    assert_eq!(
        *seen.borrow(),
        ["--apply-patches", "--amend-all", "--rev", "abc123"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    );
    let mut out = Vec::new();
    run_lint(&dir, None, &[], false, &capture, &mut out).unwrap();
    assert!(seen.borrow().is_empty());
}

#[test]
fn gap2_run_unit_confirms_unsound_and_maps_codes() {
    use std::cell::RefCell;
    use std::io::Cursor;
    let dir = std::env::temp_dir();
    let run = |outcome: ChildOutcome| move |_: &Path, _: &[String]| outcome.clone();
    let mut out = Vec::new();
    let mut input = Cursor::new(b"".to_vec());
    assert_eq!(
        run_unit(
            &dir,
            None,
            &[],
            true,
            &run(ChildOutcome::Code(0)),
            &mut input,
            &mut out
        )
        .unwrap(),
        Some(UNIT_RESULT_SKIP)
    );
    assert!(out.is_empty());
    for (code, banner) in [
        (0, "** UNIT OKAY ** No unit test failures.\n"),
        (2, "** UNIT ERRORS ** Unit testing raised errors!\n"),
    ] {
        let mut out = Vec::new();
        let mut input = Cursor::new(b"".to_vec());
        assert_eq!(
            run_unit(
                &dir,
                None,
                &[],
                false,
                &run(ChildOutcome::Code(code)),
                &mut input,
                &mut out,
            )
            .unwrap(),
            Some(code)
        );
        assert_eq!(out, format!("Running unit tests...\n{banner}").into_bytes());
    }
    // Unsound prompts (default yes); declining aborts.
    for answer in ["y\n", "\n"] {
        let mut out = Vec::new();
        let mut input = Cursor::new(answer.as_bytes().to_vec());
        assert_eq!(
            run_unit(
                &dir,
                None,
                &[],
                false,
                &run(ChildOutcome::Code(UNIT_RESULT_UNSOUND)),
                &mut input,
                &mut out,
            )
            .unwrap(),
            Some(UNIT_RESULT_UNSOUND)
        );
        let text = String::from_utf8(out).unwrap();
        // The prompt renderer wraps long prompts; both halves show.
        assert!(
            text.contains("Unit test results included failures"),
            "{text}"
        );
        assert!(text.contains("Ignore unsound test failures?"), "{text}");
        assert!(text.contains("** UNIT UNSOUND **"), "{text}");
    }
    let mut out = Vec::new();
    let mut input = Cursor::new(b"n\n".to_vec());
    assert_eq!(
        run_unit(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(UNIT_RESULT_UNSOUND)),
            &mut input,
            &mut out,
        )
        .unwrap_err(),
        USER_ABORT
    );
    assert!(
        !String::from_utf8(out)
            .unwrap()
            .contains("** UNIT UNSOUND **")
    );
    // No engine yields None.
    let mut out = Vec::new();
    let mut input = Cursor::new(b"".to_vec());
    assert_eq!(
        run_unit(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::NoEngine),
            &mut input,
            &mut out
        )
        .unwrap(),
        None
    );
    assert_eq!(
        out,
        b"Running unit tests...\nNo unit test engine is configured for this project.\n"
    );
    // Unit argv carries coverage flags plus --rev.
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let capture = |_: &Path, argv: &[String]| {
        *seen.borrow_mut() = argv.to_vec();
        ChildOutcome::Code(0)
    };
    let mut out = Vec::new();
    let mut input = Cursor::new(b"".to_vec());
    run_unit(
        &dir,
        Some("def456"),
        &["coverage".to_string()],
        false,
        &capture,
        &mut input,
        &mut out,
    )
    .unwrap();
    assert_eq!(
        *seen.borrow(),
        ["--coverage", "--rev", "def456"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    );
}

#[test]
fn gap2_run_lint_unit_combines_reports() {
    use std::cell::RefCell;
    use std::io::Cursor;
    let dir = std::env::temp_dir();
    let lint = |_: &Path, _: &[String]| ChildOutcome::Code(0);
    let unit_calls = RefCell::new(0);
    let unit = |_: &Path, _: &[String]| {
        *unit_calls.borrow_mut() += 1;
        ChildOutcome::Code(2)
    };
    let mut out = Vec::new();
    let mut input = Cursor::new(b"".to_vec());
    let report = run_lint_unit(
        &dir,
        None,
        &[],
        &[],
        false,
        false,
        &lint,
        &unit,
        &mut input,
        &mut out,
    )
    .unwrap();
    assert_eq!(report.lint_result, Some(0));
    assert_eq!(report.unit_result, Some(2));
    assert!(report.unresolved_lint.is_empty());
    assert!(report.test_results.is_empty());
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("Linting...\n** LINT OKAY **"), "{text}");
    assert!(
        text.contains("Running unit tests...\n** UNIT ERRORS **"),
        "{text}"
    );
    assert_eq!(*unit_calls.borrow(), 1);
    // A lint failure aborts before unit runs.
    let failing = |_: &Path, _: &[String]| ChildOutcome::Failed("lint blew up".to_string());
    let mut out = Vec::new();
    let mut input = Cursor::new(b"".to_vec());
    assert_eq!(
        run_lint_unit(
            &dir,
            None,
            &[],
            &[],
            false,
            false,
            &failing,
            &unit,
            &mut input,
            &mut out
        )
        .unwrap_err(),
        "lint blew up"
    );
    assert_eq!(*unit_calls.borrow(), 1);
}

#[test]
fn gap2_in_process_probes_detect_missing_engines() {
    // Marker-only repo (no shell): discovery succeeds, selection finds
    // no engine.
    let dir = std::env::temp_dir().join(format!("arc-rs-diff-d2-lint-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    assert_eq!(
        run_lint_in_process(&dir, &[]),
        ChildOutcome::NoEngine,
        "lint without .arclint"
    );
    assert_eq!(
        run_unit_in_process(&dir, &[]),
        ChildOutcome::NoEngine,
        "unit without configs"
    );
    let _ = std::fs::remove_dir_all(&dir);
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

#[test]
fn unified_diff_parses_git_change() {
    let text = "diff --git a/f.txt b/f.txt\nindex 111..222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n a\n+b\n c\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(change.old_path, "f.txt");
    assert_eq!(change.current_path, "f.txt");
    assert_eq!(change.change_type, TYPE_CHANGE);
    assert_eq!(change.file_type, FILE_TEXT);
    assert!(!change.is_message());
    assert_eq!(change.hunks.len(), 1);
    let hunk = &change.hunks[0];
    assert_eq!(
        (hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len),
        (1, 2, 1, 3)
    );
    assert!(
        hunk.corpus.starts_with("@@ -1,2 +1,3 @@\n"),
        "{}",
        hunk.corpus
    );
    assert!(hunk.corpus.contains("+b\n"), "{}", hunk.corpus);
    assert_eq!(change.corpus_size(), hunk.corpus.len());
    assert_eq!(change.commit_hash, None);
}

#[test]
fn unified_diff_handles_add_delete_rename_binary() {
    let add = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n";
    let changes = parse_unified_diff(add).unwrap();
    assert_eq!(changes[0].change_type, TYPE_ADD);
    assert_eq!(changes[0].current_path, "new.txt");

    let delete = "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
    let changes = parse_unified_diff(delete).unwrap();
    assert_eq!(changes[0].change_type, TYPE_DELETE);
    assert_eq!(changes[0].current_path, "old.txt");

    let rename = "diff --git a/a.txt b/b.txt\nsimilarity index 90%\nrename from a.txt\nrename to b.txt\n--- a/a.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-x\n+y\n";
    let changes = parse_unified_diff(rename).unwrap();
    assert_eq!(changes[0].change_type, TYPE_MOVE_HERE);
    assert_eq!(changes[0].old_path, "a.txt");
    assert_eq!(changes[0].current_path, "b.txt");

    let copy = "diff --git a/a.txt b/c.txt\ncopy from a.txt\ncopy to c.txt\n--- a/a.txt\n+++ b/c.txt\n@@ -1 +1 @@\n x\n";
    let changes = parse_unified_diff(copy).unwrap();
    assert_eq!(changes[0].change_type, TYPE_COPY_HERE);

    let binary = "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";
    let changes = parse_unified_diff(binary).unwrap();
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());

    let git_binary =
        "diff --git a/bin.dat b/bin.dat\nGIT binary patch\nliteral 3\nabc\n\nliteral 3\ndef\n\n";
    let changes = parse_unified_diff(git_binary).unwrap();
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());
}

#[test]
fn unified_diff_handles_hg_and_svn_headers() {
    let hg = "diff -r abc123 f.txt\n--- f.txt\tThu Jan 01 00:00:00 2026\n+++ f.txt\tThu Jan 01 00:00:00 2026\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(hg).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "f.txt");
    assert_eq!(changes[0].hunks.len(), 1);

    let svn =
        "Index: f.txt\n--- f.txt\t(revision 5)\n+++ f.txt\t(working copy)\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(svn).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "f.txt");
    assert_eq!(changes[0].old_path, "f.txt");
}

#[test]
fn unified_diff_message_changes_filter() {
    let text = "commit abc123def\nAuthor: A U Thor\n\n    add feature\n\ndiff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes[0].is_message());
    assert_eq!(changes[0].commit_hash.as_deref(), Some("abc123def"));
    assert!(
        changes[0]
            .message
            .as_deref()
            .unwrap()
            .contains("add feature")
    );
    let files = without_message_changes(&changes);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].current_path, "f.txt");
    assert!(without_message_changes(&[]).is_empty());
    assert!(parse_unified_diff("").unwrap().is_empty());
}

#[test]
fn gap2_combined_hunk_header_fails_like_php() {
    let text = "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@@ -1,2 -1,2 +1,3 @@@\n  a\n- b\n+B\n";
    let err = parse_unified_diff(text).unwrap_err();
    // PHP `didFailParse()`: message, blank line, then the ±5-line context
    // with `%8.8s %6.6s   %s` rows (`:1198-1226`).
    let expected = "Diff Parse Exception: Expected hunk header '@@ -NN,NN +NN,NN @@'.\n\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}1   diff --git a/f.txt b/f.txt\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}2   --- a/f.txt\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}3   +++ b/f.txt\n\
        \u{20}\u{20}\u{20}>>>        4   @@@ -1,2 -1,2 +1,3 @@@\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}5     a\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}6   - b\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}7   +B\n";
    assert_eq!(err, expected);
}

#[test]
fn gap2_combined_hunk_after_first_hunk_fails_like_php() {
    let text = "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n a\n@@@ -1 -1 +1 @@@\n b\n";
    let err = parse_unified_diff(text).unwrap_err();
    assert!(
        err.starts_with("Diff Parse Exception: Expected hunk header '@@ -NN,NN +NN,NN @@'.\n\n"),
        "{err:?}"
    );
    assert!(err.contains(">>>"), "{err:?}");
    assert!(err.contains("@@@ -1 -1 +1 @@@"), "{err:?}");
}

#[test]
fn gap2_diff_cc_header_fails_like_php() {
    let text = "diff --cc f.txt\nindex 111,222..333\n--- a/f.txt\n+++ b/f.txt\n@@@ -1,2 -1,2 +1,3 @@@\n  a\n";
    let err = parse_unified_diff(text).unwrap_err();
    let expected = "Diff Parse Exception: Expected a hunk header, like 'Index: /path/to/file.ext' (svn), 'Property changes on: /path/to/file.ext' (svn properties), 'commit 59bcc3ad6775562f845953cf01624225' (git show), 'diff --git' (git diff), '--- filename' (unified diff), or 'diff -r' (hg diff or patch).\n\n\
        \u{20}\u{20}\u{20}>>>        1   diff --cc f.txt\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}2   index 111,222..333\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}3   --- a/f.txt\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}4   +++ b/f.txt\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}5   @@@ -1,2 -1,2 +1,3 @@@\n\
        \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}6     a\n";
    assert_eq!(err, expected);
}

#[test]
fn gap2_diff_cc_mid_diff_fails_without_recovery() {
    let text = "diff --git a/g.txt b/g.txt\n--- a/g.txt\n+++ b/g.txt\n@@ -1 +1 @@\n-a\n+b\n\ndiff --cc f.txt\n";
    let err = parse_unified_diff(text).unwrap_err();
    assert!(err.contains(EXPECTED_DIFF_START), "{err:?}");
    // Failing line is the 1-based `diff --cc` line (8).
    assert!(err.contains(">        8   diff --cc f.txt"), "{err:?}");
}

#[test]
fn gap2_leading_diff_cc_recovers_to_later_git_block() {
    let text = "diff --cc f.txt\n# stray comment\n\ndiff --git a/g.txt b/g.txt\n--- a/g.txt\n+++ b/g.txt\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "g.txt");
    assert_eq!(changes[0].hunks.len(), 1);
}

struct FailingReader;

impl std::io::Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("boom"))
    }
}

#[test]
fn raw_stdin_and_command_helpers() {
    assert_eq!(RAW_STDIN_NOTICE, "Reading diff from stdin...\n");
    assert_eq!(UNKNOWN_RAW_DIFF_SOURCE, "Unknown raw diff source.");
    let text = read_raw_stdin_from(std::io::Cursor::new("diff text")).unwrap();
    assert_eq!(text, "diff text");
    let err = read_raw_stdin_from(FailingReader).unwrap_err();
    assert!(err.contains("failed to read diff from stdin"), "{err}");

    let out = shell_command_output("echo hello").unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
    assert!(out.status.success());
    assert!(shell_command_status("exit 0").unwrap().success());
    assert!(!shell_command_status("exit 3").unwrap().success());
    // Quoted arguments survive the shell round-trip on every platform.
    let quoted = shell_command_output("echo \"a b\"").unwrap();
    assert!(String::from_utf8_lossy(&quoted.stdout).contains("a b"));

    let out = run_raw_command_text("echo hello").unwrap();
    assert!(out.contains("hello"), "{out}");
    assert!(run_raw_command_text("exit 1").is_err());
}

#[test]
fn hg_argv_and_parsers() {
    assert_eq!(
        hg_diff_options(7),
        vec!["--git".to_string(), "-U7".to_string()]
    );
    assert_eq!(
        hg_full_diff_argv("abc", 32767),
        vec![
            "diff".to_string(),
            "--git".to_string(),
            "-U32767".to_string(),
            "--rev".to_string(),
            "abc".to_string(),
            "--".to_string(),
            String::new(),
        ]
    );
    assert_eq!(hg_branch_argv(), vec!["branch".to_string()]);
    assert_eq!(
        hg_status_argv("abc"),
        vec!["status".to_string(), "--rev".to_string(), "abc".to_string()]
    );
    let log_argv = hg_commit_log_argv("abc", "default");
    assert_eq!(log_argv[0], "log");
    assert!(
        log_argv.contains(&"(abc::. - abc)".to_string()),
        "{log_argv:?}"
    );
    assert_eq!(
        hg_amend_argv("msg.txt"),
        vec![
            "commit".to_string(),
            "--amend".to_string(),
            "--logfile".to_string(),
            "msg.txt".to_string(),
            "--".to_string(),
        ]
    );

    let entries =
        parse_hg_status_details("M f.txt\nA g.txt\nR h.txt\n? u.txt\n! m.txt\nC c.txt\nI i.txt\n")
            .unwrap();
    let masks: Vec<u32> = entries.iter().map(|(_, flags)| *flags).collect();
    assert_eq!(
        masks,
        vec![
            FLAG_MODIFIED,
            FLAG_ADDED,
            FLAG_DELETED,
            FLAG_UNTRACKED,
            FLAG_MISSING,
            0,
            0,
        ]
    );
    let copied = parse_hg_status_details("A new.txt\n  old.txt\n").unwrap();
    assert_eq!(copied[0].0.from.as_deref(), Some("old.txt"));
    assert_eq!(
        parse_hg_status_details("X").unwrap_err(),
        "Unparsable Mercurial status line 'X'."
    );
    assert_eq!(
        parse_hg_status_details("Z f.txt").unwrap_err(),
        "Unknown Mercurial status 'Z'."
    );
    // Like PHP, whole-output trim runs first, so a leading copy-source
    // line loses its spaces and reports as unparsable.
    assert_eq!(
        parse_hg_status_details("  orphan.txt").unwrap_err(),
        "Unparsable Mercurial status line 'orphan.txt'."
    );
    assert!(parse_hg_status_details("  \n").unwrap().is_empty());

    let map = parse_hg_status("M f.txt\nA g.txt\n").unwrap();
    assert_eq!(
        map,
        vec![
            ("f.txt".to_string(), FLAG_MODIFIED),
            ("g.txt".to_string(), FLAG_ADDED)
        ]
    );

    let log = parse_hg_commit_log("node1\x01first desc\x02node2\x01second desc\x02");
    assert_eq!(
        log,
        vec![
            ("node1".to_string(), "first desc".to_string()),
            ("node2".to_string(), "second desc".to_string()),
        ]
    );
    assert!(parse_hg_commit_log("").is_empty());
    assert!(parse_hg_commit_log("garbage-without-separator\x02").is_empty());

    assert!(hg_supports_amend_from_help("commit --amend ..."));
    assert!(!hg_supports_amend_from_help("commit ..."));

    let bogus = std::path::Path::new("definitely-not-an-hg-repo-g1");
    assert!(get_full_hg_diff(bogus, "tip", 3).is_err());
    assert!(get_hg_branch_name(bogus).is_err());
}

#[test]
fn svn_helpers() {
    assert_eq!(
        svn_status_argv(&[]),
        vec!["--xml".to_string(), "status".to_string()]
    );
    assert_eq!(
        svn_status_argv(&["a.txt".to_string()]),
        vec![
            "--xml".to_string(),
            "status".to_string(),
            "a.txt".to_string()
        ]
    );
    assert_eq!(svn_props_mask("none").unwrap(), 0);
    assert_eq!(svn_props_mask("normal").unwrap(), 0);
    assert_eq!(svn_props_mask("modified").unwrap(), FLAG_MODIFIED);
    assert_eq!(
        svn_props_mask("bogus").unwrap_err(),
        "Unrecognized property status 'bogus'."
    );

    for (item, flags) in [
        ("none", 0),
        ("normal", 0),
        ("external", FLAG_EXTERNALS),
        ("unversioned", FLAG_UNTRACKED),
        ("obstructed", FLAG_OBSTRUCTED),
        ("missing", FLAG_MISSING),
        ("added", FLAG_ADDED),
        ("replaced", FLAG_MODIFIED),
        ("modified", FLAG_MODIFIED),
        ("deleted", FLAG_DELETED),
        ("conflicted", FLAG_CONFLICT),
        ("incomplete", FLAG_INCOMPLETE),
    ] {
        assert_eq!(parse_svn_item_status(item).unwrap(), flags, "{item}");
    }
    assert_eq!(
        parse_svn_item_status("bogus").unwrap_err(),
        "Unrecognized item status 'bogus'."
    );

    assert_eq!(
        svn_info_argv("/"),
        vec!["info".to_string(), ".".to_string()]
    );
    assert_eq!(
        svn_info_argv("f.txt"),
        vec!["info".to_string(), "f.txt@".to_string()]
    );
    assert_eq!(
        svn_diff_argv("f.txt", None, 3),
        vec![
            "diff".to_string(),
            "--depth".to_string(),
            "empty".to_string(),
            "f.txt".to_string(),
        ]
    );
    assert_eq!(
        svn_diff_argv("f.txt", Some("/bin/sh-diff"), 5),
        vec![
            "diff".to_string(),
            "--depth".to_string(),
            "empty".to_string(),
            "--diff-cmd".to_string(),
            "/bin/sh-diff".to_string(),
            "-x".to_string(),
            "-U5".to_string(),
            "f.txt".to_string(),
        ]
    );

    assert_eq!(svn_base_rev_line(7, "a/b"), "    Revision 7, a/b");
    assert_eq!(check_svn_base_consistency(&[]).unwrap(), 0);
    assert_eq!(check_svn_base_consistency(&[("n", 0)]).unwrap(), 0);
    assert_eq!(
        check_svn_base_consistency(&[("a", 9), ("b", 9)]).unwrap(),
        9
    );
    let err = check_svn_base_consistency(&[("a", 9), ("b", 10)]).unwrap_err();
    assert!(
        err.contains("Base revisions of changed paths are mismatched."),
        "{err}"
    );
    assert!(err.contains("    Revision 9, a"), "{err}");
    assert!(err.contains("    Revision 10, b"), "{err}");

    let warning = svn_externals_warning();
    assert!(
        warning.contains("These changes will not be included in the diff"),
        "{warning}"
    );
    assert!(
        warning.ends_with("Modified 'svn:externals' files:"),
        "{warning}"
    );
    assert_eq!(
        PROMPT_SVN_EXTERNALS_ANYWAY,
        "Generate a diff (with just local changes) anyway?"
    );

    assert_eq!(
        svn_branch_name("https://s.example/r/trunk", "https://s.example/r"),
        "/trunk"
    );
    assert_eq!(
        svn_branch_name("https://other/x", "https://s.example/r"),
        "svn"
    );
}

#[test]
fn change_review_texts() {
    let err = gigantic_svn_error("big.txt", 5_000_000);
    assert!(
        err.contains("Diff for 'big.txt' with context is 5000000 bytes in length."),
        "{err}"
    );
    assert!(
        err.contains("svn propset svn:mime-type application/octet-stream <filename>"),
        "{err}"
    );

    let confirm = gigantic_confirm("big.txt", 5_000_000);
    assert!(
        confirm.contains("Generally, source changes should not be this large."),
        "{confirm}"
    );
    assert!(
        confirm.contains("Mark this file as 'binary' and continue?"),
        "{confirm}"
    );

    assert_eq!(UTF8_HEADER, "Invalid Content Encoding (Non-UTF8)");
    let body = utf8_warning_body(2);
    assert!(
        body.contains("This diff includes 2 file(s) which are not valid UTF-8"),
        "{body}"
    );
    assert!(
        body.contains("these files will be marked as binary."),
        "{body}"
    );
    assert!(
        body.contains("User Guide: UTF-8 and Character Encoding"),
        "{body}"
    );
    assert!(body.contains("    2 AFFECTED FILE(S)\n"), "{body}");
    assert_eq!(
        utf8_continue_prompt(3),
        "Do you want to mark these 3 file(s) as binary and continue?"
    );
    assert_eq!(ABORTED_UTF8, "Aborted workflow to fix UTF-8.");
    assert_eq!(
        converted_hunk_notice("f.txt", "latin1"),
        "Converted a 'f.txt' hunk from 'latin1' to UTF-8.\n"
    );
    assert_eq!(
        encoding_lookup_failed("nope"),
        "Lookup of encoding in project failed: nope"
    );

    assert_eq!(LINTING, "Linting...");
    assert_eq!(RUNNING_UNIT_TESTS, "Running unit tests...");
    assert_eq!(
        lint_result_notice(LINT_RESULT_OKAY),
        Some(("LINT OKAY", "No lint problems."))
    );
    assert_eq!(
        lint_result_notice(LINT_RESULT_WARNINGS),
        Some(("LINT MESSAGES", "Lint issued unresolved warnings."))
    );
    assert_eq!(
        lint_result_notice(LINT_RESULT_ERRORS),
        Some(("LINT ERRORS", "Lint raised errors!"))
    );
    assert_eq!(lint_result_notice(LINT_RESULT_SKIP), None);
    assert_eq!(lint_result_notice(99), None);
    assert_eq!(
        unit_result_notice(UNIT_RESULT_OKAY),
        Some(("UNIT OKAY", "No unit test failures."))
    );
    assert_eq!(
        unit_result_notice(UNIT_RESULT_UNSOUND),
        Some((
            "UNIT UNSOUND",
            "Unit testing raised errors, but all failing tests are unsound."
        ))
    );
    assert_eq!(
        unit_result_notice(UNIT_RESULT_FAIL),
        Some(("UNIT ERRORS", "Unit testing raised errors!"))
    );
    assert_eq!(unit_result_notice(UNIT_RESULT_SKIP), None);
    assert!(UNSOUND_PROMPT.contains("Ignore unsound test failures?"));
    assert_eq!(
        NO_LINT_ENGINE,
        "No lint engine configured for this project."
    );
    assert_eq!(
        NO_UNIT_ENGINE,
        "No unit test engine is configured for this project."
    );

    assert_eq!(
        git_amend_argv(None),
        vec![
            "commit".to_string(),
            "--amend".to_string(),
            "--allow-empty".to_string(),
            "-C".to_string(),
            "HEAD".to_string(),
        ]
    );
    assert_eq!(
        git_amend_argv(Some("m.txt")),
        vec![
            "commit".to_string(),
            "--amend".to_string(),
            "--allow-empty".to_string(),
            "-F".to_string(),
            "m.txt".to_string(),
        ]
    );
}

#[test]
fn message_texts() {
    assert_eq!(PROMPT_USE_SAVED_MESSAGE, "Do you want to use this message?");
    assert_eq!(
        PROMPT_EDIT_MESSAGE_AGAIN,
        "Do you want to edit the message?"
    );
    assert_eq!(
        TASK_NEW_REVISION,
        "Provide the details for a new revision, then save and exit."
    );
    assert_eq!(
        TASK_UPDATE_COMMENTS,
        "Update the revision comments, then save and exit."
    );
    assert_eq!(
        TASK_EDIT_REVISION_INFO,
        "Update the details for a revision, then save and exit."
    );
    assert_eq!(TEMPLATE_NOT_EDITED, "Template not edited.");
    assert_eq!(COMMIT_MESSAGE_HAS_ERRORS, "Commit message has errors:");
    assert_eq!(RESOLVE_THESE_ERRORS, "Resolve these errors:");
    assert_eq!(
        MUST_RESOLVE_ERRORS,
        "You must resolve these errors to continue."
    );
    assert_eq!(
        MESSAGE_HAS_UNRESOLVED_ERRORS,
        "Message has unresolved errors."
    );

    assert_eq!(truncate_preview_64("  Title line\nsecond"), "Title line");
    assert_eq!(truncate_preview_64(""), "");
    let long = "x".repeat(100);
    assert_eq!(truncate_preview_64(&long).len(), 64);

    let notice = saved_message_notice("/tmp/create-message", "Fix pager");
    assert!(
        notice.contains("You have a saved revision message in '/tmp/create-message'."),
        "{notice}"
    );
    assert!(
        notice.contains("Message begins:\n\n       Fix pager\n\n"),
        "{notice}"
    );
    assert!(
        notice.ends_with("You can use this message, or discard it."),
        "{notice}"
    );
    let bare = saved_message_notice("/tmp/create-message", "");
    assert!(!bare.contains("Message begins:"), "{bare}");

    let block = included_commits_block(Some("main"), &["abc title".to_string()]);
    assert_eq!(
        block,
        vec![
            String::new(),
            "Included commits in branch main:".to_string(),
            String::new(),
            "        abc title".to_string(),
        ]
    );
    let raw_block = included_commits_block(None, &[]);
    assert_eq!(raw_block[1], "Included commits:");

    let issues = new_revision_issues(&["  included".to_string()], &["note".to_string()]);
    assert_eq!(issues[0], "NEW DIFFERENTIAL REVISION");
    assert_eq!(issues[1], "Describe the changes in this new revision.");
    assert!(issues.contains(&"  $ arc diff --update <revision>".to_string()));
    assert!(issues.contains(&"note".to_string()));
    let plain = new_revision_issues(&[], &[]);
    assert!(!plain.iter().any(|line| line == "note"));

    assert_eq!(
        commit_parse_note("abc123"),
        "NOTE: commit abc123 could not be completely parsed:"
    );
    assert_eq!(WHERE_COMMIT_MESSAGE, "commit message");
    assert_eq!(copy_saved_to("X"), "A copy was saved to X.");
    assert_eq!(message_saved_to("X"), "(Message saved to X.)");
}

#[test]
fn staging_and_upload_helpers() {
    assert_eq!(
        staging_skip_body(STAGING_USER_SKIP),
        "Flag --skip-staging was specified."
    );
    assert_eq!(
        staging_skip_body(STAGING_DIFF_RAW),
        "Raw changes can not be pushed to a staging area."
    );
    assert_eq!(
        staging_skip_body(STAGING_REPOSITORY_UNKNOWN),
        "Unable to determine repository for this change."
    );
    assert_eq!(
        staging_skip_body(STAGING_REPOSITORY_UNAVAILABLE),
        "The server does not support staging areas."
    );
    assert_eq!(
        staging_skip_body(STAGING_REPOSITORY_UNSUPPORTED),
        "The server does not support staging areas for this repository."
    );
    assert_eq!(
        staging_skip_body(STAGING_REPOSITORY_UNCONFIGURED),
        "No staging area is configured for this repository."
    );
    assert_eq!(
        staging_skip_body(STAGING_CLIENT_UNSUPPORTED),
        "This client version does not support staging this repository."
    );
    assert_eq!(SKIP_STAGING_HEADER, "SKIP STAGING");

    let config = StagingConfig::new(true, Some("ssh://x"), true);
    assert_eq!(config.prefix, "phabricator");
    assert_eq!(
        staging_skip_reason(true, false, Some("PHID"), Some(&config)),
        Some(STAGING_USER_SKIP)
    );
    assert_eq!(
        staging_skip_reason(false, true, Some("PHID"), Some(&config)),
        Some(STAGING_DIFF_RAW)
    );
    assert_eq!(
        staging_skip_reason(false, false, None, Some(&config)),
        Some(STAGING_REPOSITORY_UNKNOWN)
    );
    assert_eq!(
        staging_skip_reason(false, false, Some("PHID"), None),
        Some(STAGING_REPOSITORY_UNAVAILABLE)
    );
    let unsupported = StagingConfig::new(false, Some("ssh://x"), true);
    assert_eq!(
        staging_skip_reason(false, false, Some("PHID"), Some(&unsupported)),
        Some(STAGING_REPOSITORY_UNSUPPORTED)
    );
    let unconfigured = StagingConfig::new(true, None, true);
    assert_eq!(
        staging_skip_reason(false, false, Some("PHID"), Some(&unconfigured)),
        Some(STAGING_REPOSITORY_UNCONFIGURED)
    );
    let non_git = StagingConfig::new(true, Some("ssh://x"), false);
    assert_eq!(
        staging_skip_reason(false, false, Some("PHID"), Some(&non_git)),
        Some(STAGING_CLIENT_UNSUPPORTED)
    );
    assert_eq!(
        staging_skip_reason(false, false, Some("PHID"), Some(&config)),
        None
    );

    assert_eq!(PUSH_STAGING_HEADER, "PUSH STAGING");
    assert_eq!(PUSH_STAGING_BODY, "Pushing changes to staging area...");
    assert_eq!(
        staging_tag("phabricator", "base", 7),
        "refs/tags/phabricator/base/7"
    );
    assert_eq!(
        staging_refspecs("base1", "head1", "phabricator", 7, false),
        vec![
            "base1:refs/tags/phabricator/base/7".to_string(),
            "head1:refs/tags/phabricator/diff/7".to_string(),
        ]
    );
    assert_eq!(
        staging_refspecs(GIT_MAGIC_ROOT_COMMIT, "head1", "phabricator", 7, false),
        vec!["head1:refs/tags/phabricator/diff/7".to_string()]
    );
    assert_eq!(
        staging_refspecs("base1", "head1", "phabricator", 7, true),
        vec![
            "base1:refs/tags/phabricator/base/7".to_string(),
            "base1:refs/tags/phabricator/diff/7".to_string(),
        ]
    );
    assert_eq!(
        staging_lfs_refspec("head1", "phabricator", 7),
        "+head1:refs/tags/phabricator/diff/7"
    );
    assert_eq!(
        staging_push_argv("ssh://x", &["a:b".to_string()], true),
        vec![
            "push".to_string(),
            "--no-verify".to_string(),
            "--".to_string(),
            "ssh://x".to_string(),
            "a:b".to_string(),
        ]
    );
    assert_eq!(
        staging_push_argv("ssh://x", &[], false),
        vec!["push".to_string(), "--".to_string(), "ssh://x".to_string()]
    );
    assert!(!git_supports_push_no_verify("1.8.1"));
    assert!(git_supports_push_no_verify("1.8.2"));
    assert!(git_supports_push_no_verify("2.40.0"));
    assert_eq!(STAGING_FAILED_HEADER, "STAGING FAILED");
    assert!(STAGING_PUSH_FAILED.contains("--skip-staging"));
    assert!(STAGING_LFS_PUSH_FAILED.contains("lfs changes"));

    assert_eq!(UPLOAD_COMPLETE, "Upload complete.");
    assert_eq!(
        uploaded_binary_notice("a.png"),
        "Uploaded binary data for \"a.png\"."
    );
    assert_eq!(
        failed_binary_upload("a.png"),
        "Failed to upload binary \"a.png\"."
    );
    assert_eq!(PROMPT_CONTINUE, "Continue?");
    assert!(ABORTED_UPLOAD_FAILURE.contains("--skip-binaries"));
    assert!(needs_binary_upload(FILE_BINARY, false));
    assert!(!needs_binary_upload(FILE_BINARY, true));
    assert!(!needs_binary_upload(FILE_TEXT, false));
    assert!(is_image_mime_type("image/png"));
    assert!(!is_image_mime_type("application/octet-stream"));

    assert_eq!(UPDATING_COMMIT_MESSAGE, "Updating commit message...");
    assert!(AMEND_UNSUPPORTED.contains("only supported in git and hg"));
    assert!(should_open_in_browser(true));
    assert!(!should_open_in_browser(false));
}

/* Gap2 D5/D11: svn affected paths + per-path diff fan-out. */

fn gap2_svn_scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-diff-d5-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn gap2_status_map(entries: &[(&str, u32)]) -> svn::StatusMap {
    entries
        .iter()
        .map(|(path, mask)| ((*path).to_string(), *mask))
        .collect()
}

#[test]
fn gap2_split_svn_externals_keeps_and_warns() {
    let status = gap2_status_map(&[
        ("plain.txt", svn::FLAG_MODIFIED),
        ("added.txt", svn::FLAG_ADDED),
        ("ext-mod.txt", svn::FLAG_EXTERNALS | svn::FLAG_MODIFIED),
        ("ext-add.txt", svn::FLAG_EXTERNALS | svn::FLAG_ADDED),
        ("ext-del.txt", svn::FLAG_EXTERNALS | svn::FLAG_DELETED),
        ("ext-clean.txt", svn::FLAG_EXTERNALS),
        (
            "ext-untracked.txt",
            svn::FLAG_EXTERNALS | svn::FLAG_UNTRACKED,
        ),
    ]);
    let (kept, warn) = split_svn_externals(&status);
    assert_eq!(
        kept.keys().cloned().collect::<Vec<_>>(),
        vec!["added.txt".to_string(), "plain.txt".to_string()]
    );
    assert_eq!(
        warn,
        vec![
            "ext-add.txt".to_string(),
            "ext-del.txt".to_string(),
            "ext-mod.txt".to_string(),
        ]
    );
}

#[test]
fn gap2_svn_file_list_contains() {
    let root = gap2_svn_scratch("filelist");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("a.txt"), "a").unwrap();
    std::fs::write(root.join("sub2.txt"), "b").unwrap();
    std::fs::write(root.join("top.txt"), "c").unwrap();

    let empty = SvnFileList::new(&root, &[]);
    assert!(empty.contains(&root, "sub/a.txt"));
    assert!(empty.contains(&root, "anything/at/all.txt"));

    let file = SvnFileList::new(&root, &["top.txt".to_string()]);
    assert!(file.contains(&root, "top.txt"));
    assert!(!file.contains(&root, "sub/a.txt"));

    let dir = SvnFileList::new(&root, &["sub".to_string()]);
    assert!(dir.contains(&root, "sub/a.txt"));
    assert!(!dir.contains(&root, "top.txt"));
    // A sibling prefix is not a parent: `sub/` never matches `sub2.txt`.
    assert!(!dir.contains(&root, "sub2.txt"));

    // Dot segments collapse lexically.
    let dotted = SvnFileList::new(&root, &["./sub/../sub".to_string()]);
    assert!(dotted.contains(&root, "sub/a.txt"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_check_svn_base_revision_strings_exact() {
    assert_eq!(check_svn_base_revision_strings(&[]).unwrap(), None);
    assert_eq!(check_svn_base_revision_strings(&[("n", "")]).unwrap(), None);
    assert_eq!(
        check_svn_base_revision_strings(&[("n", "0")]).unwrap(),
        None
    );
    assert_eq!(
        check_svn_base_revision_strings(&[("a", "9"), ("b", "9")]).unwrap(),
        Some("9".to_string())
    );
    // Strict string compare: zero-padded revisions mismatch.
    let err = check_svn_base_revision_strings(&[("a", "09"), ("b", "9")]).unwrap_err();
    assert_eq!(
        err,
        "Base revisions of changed paths are mismatched. Update all paths to the same base \
         revision before creating a diff: \n\n    Revision 09, a\n    Revision 9, b"
    );
    // Non-numeric revisions stay (PHP 8 lexicographic `<=`) and mismatch.
    let err = check_svn_base_revision_strings(&[("a", "xx"), ("b", "9")]).unwrap_err();
    assert!(err.contains("    Revision xx, a"), "{err}");
    assert!(err.contains("    Revision 9, b"), "{err}");
}

#[test]
fn gap2_confirm_svn_externals_bytes() {
    use std::io::Cursor;

    // No externals, or an already-warned workflow: silent success.
    let mut warned = false;
    let mut out = Vec::new();
    confirm_svn_externals(&mut Cursor::new(b"".to_vec()), &mut out, &mut warned, &[]).unwrap();
    assert!(out.is_empty());
    assert!(!warned);
    let mut warned = true;
    let mut out = Vec::new();
    confirm_svn_externals(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &mut warned,
        &["ext.txt".to_string()],
    )
    .unwrap();
    assert!(out.is_empty());

    // Acceptance writes the warning plus prompt and flips the flag. The
    // wrapped file list is dropped like PHP's `sprintf` (never printed).
    let mut warned = false;
    let mut out = Vec::new();
    confirm_svn_externals(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &mut warned,
        &["ext.txt".to_string()],
    )
    .unwrap();
    assert!(warned);
    let expected = format!(
        "{}\n\n{}\n",
        svn_externals_warning(),
        diff::format_confirm_prompt(PROMPT_SVN_EXTERNALS_ANYWAY, true)
    );
    assert_eq!(String::from_utf8(out).unwrap(), expected);

    // Refusal aborts; EOF answers the default (no) and aborts too.
    for answer in ["n\n", ""] {
        let mut warned = false;
        let mut out = Vec::new();
        let err = confirm_svn_externals(
            &mut Cursor::new(answer.as_bytes().to_vec()),
            &mut out,
            &mut warned,
            &["ext.txt".to_string()],
        )
        .unwrap_err();
        assert_eq!(err, USER_ABORT);
        assert!(!warned);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .starts_with(&svn_externals_warning())
        );
    }
}

#[test]
fn gap2_svn_skeleton_change_types() {
    let root = gap2_svn_scratch("skeleton");
    std::fs::create_dir_all(root.join("adir")).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();

    let added = svn_skeleton_change(&root, "a.txt", svn::FLAG_ADDED);
    assert_eq!(added.change_type, TYPE_ADD);
    assert_eq!(added.file_type, FILE_TEXT);
    assert_eq!(added.current_path, "a.txt");
    assert!(added.old_path.is_empty());
    assert!(added.hunks.is_empty() && added.away_paths.is_empty());

    let deleted = svn_skeleton_change(&root, "a.txt", svn::FLAG_DELETED);
    assert_eq!(deleted.change_type, TYPE_DELETE);
    let modified = svn_skeleton_change(&root, "a.txt", svn::FLAG_MODIFIED);
    assert_eq!(modified.change_type, TYPE_CHANGE);

    let dir = svn_skeleton_change(&root, "adir", svn::FLAG_MODIFIED);
    assert_eq!(dir.change_type, TYPE_CHANGE);
    assert_eq!(dir.file_type, FILE_DIRECTORY);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("a.txt"), root.join("link.txt")).unwrap();
        let link = svn_skeleton_change(&root, "link.txt", svn::FLAG_MODIFIED);
        assert_eq!(link.file_type, FILE_SYMLINK);
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_svn_copy_old_path_shapes() {
    let root = "https://s.example/r/trunk";
    // Same revision: plain relative path.
    assert_eq!(
        svn_copy_old_path("https://s.example/r/trunk/old.txt", "9", "9", root),
        Some("old.txt".to_string())
    );
    // Differing revision: `@rev` suffix (loose compare).
    assert_eq!(
        svn_copy_old_path("https://s.example/r/trunk/old.txt", "7", "9", root),
        Some("old.txt@7".to_string())
    );
    // Loose numeric compare: zero-padded revisions match.
    assert_eq!(
        svn_copy_old_path("https://s.example/r/trunk/old.txt", "09", "9", root),
        Some("old.txt".to_string())
    );
    // Missing copy revision: no suffix.
    assert_eq!(
        svn_copy_old_path("https://s.example/r/trunk/old.txt", "", "9", root),
        Some("old.txt".to_string())
    );
    // Outside the root, or empty: no old path.
    assert_eq!(
        svn_copy_old_path("https://s.example/r/branches/x/old.txt", "7", "9", root),
        None
    );
    assert_eq!(svn_copy_old_path("", "7", "9", root), None);
}

fn gap2_path_data(
    path: &str,
    flags: u32,
    info: &[(&str, &str)],
    diff: Option<&str>,
) -> SvnPathData {
    use std::collections::BTreeMap;
    SvnPathData {
        path: path.to_string(),
        flags,
        info: info
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>(),
        diff: diff.map(|text| text.as_bytes().to_vec()),
    }
}

#[test]
fn gap2_apply_svn_copy_info_flows() {
    // Move: the deleted origin becomes MOVE_AWAY, the new path MOVE_HERE.
    let mut changes = vec![
        svn_skeleton_change(Path::new("."), "old.txt", svn::FLAG_DELETED),
        svn_skeleton_change(Path::new("."), "new.txt", svn::FLAG_ADDED),
    ];
    let moved = vec![gap2_path_data(
        "new.txt",
        svn::FLAG_ADDED,
        &[
            ("Copied From URL", "https://s.example/r/trunk/old.txt"),
            ("Copied From Rev", "9"),
            ("Revision", "9"),
        ],
        None,
    )];
    apply_svn_copy_info(&mut changes, &moved, Some("https://s.example/r/trunk")).unwrap();
    assert_eq!(changes[0].change_type, TYPE_MOVE_AWAY);
    assert_eq!(changes[0].away_paths, vec!["new.txt".to_string()]);
    assert_eq!(changes[1].change_type, TYPE_MOVE_HERE);
    assert_eq!(changes[1].old_path, "old.txt");

    // A `@rev` origin never matches a working-copy skeleton: the origin is
    // fresh, so the change copies here instead of moving.
    let mut changes = vec![
        svn_skeleton_change(Path::new("."), "old.txt", svn::FLAG_DELETED),
        svn_skeleton_change(Path::new("."), "new.txt", svn::FLAG_ADDED),
    ];
    let at_rev = vec![gap2_path_data(
        "new.txt",
        svn::FLAG_ADDED,
        &[
            ("Copied From URL", "https://s.example/r/trunk/old.txt"),
            ("Copied From Rev", "7"),
            ("Revision", "9"),
        ],
        None,
    )];
    apply_svn_copy_info(&mut changes, &at_rev, Some("https://s.example/r/trunk")).unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[1].change_type, TYPE_COPY_HERE);
    assert_eq!(changes[1].old_path, "old.txt@7");
    assert_eq!(changes[2].current_path, "old.txt@7");
    assert_eq!(changes[2].change_type, TYPE_COPY_AWAY);

    // Copy of an unmodified path: fresh origin (CHANGE) becomes COPY_AWAY.
    let mut changes = vec![svn_skeleton_change(
        Path::new("."),
        "copy.txt",
        svn::FLAG_ADDED,
    )];
    let data = vec![gap2_path_data(
        "copy.txt",
        svn::FLAG_ADDED,
        &[
            ("Copied From URL", "https://s.example/r/trunk/orig.txt"),
            ("Copied From Rev", "9"),
            ("Revision", "9"),
        ],
        None,
    )];
    apply_svn_copy_info(&mut changes, &data, Some("https://s.example/r/trunk")).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].change_type, TYPE_COPY_HERE);
    assert_eq!(changes[0].old_path, "orig.txt");
    assert_eq!(changes[1].current_path, "orig.txt");
    assert_eq!(changes[1].change_type, TYPE_COPY_AWAY);
    assert_eq!(changes[1].away_paths, vec!["copy.txt".to_string()]);

    // A copy-here change whose origin is absent stays untouched.
    let mut changes = vec![ParsedChange {
        old_path: "orig.txt".to_string(),
        current_path: "copy.txt".to_string(),
        change_type: TYPE_COPY_HERE,
        file_type: FILE_TEXT,
        hunks: Vec::new(),
        away_paths: Vec::new(),
        commit_hash: None,
        message: None,
    }];
    apply_svn_copy_info(&mut changes, &data, Some("https://s.example/r/trunk")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_COPY_HERE);

    // Bad origin states fail like PHP.
    let mut changes = vec![
        ParsedChange {
            old_path: String::new(),
            current_path: "orig.txt".to_string(),
            change_type: TYPE_MESSAGE,
            file_type: FILE_TEXT,
            hunks: Vec::new(),
            away_paths: Vec::new(),
            commit_hash: None,
            message: None,
        },
        svn_skeleton_change(Path::new("."), "copy.txt", svn::FLAG_ADDED),
    ];
    let err =
        apply_svn_copy_info(&mut changes, &data, Some("https://s.example/r/trunk")).unwrap_err();
    assert_eq!(err, "Bad origin state 9.");
}

#[test]
fn gap2_merge_svn_path_diffs_shapes() {
    let root = gap2_svn_scratch("merge");
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    std::fs::write(root.join("b.bin"), b"\x00\x01").unwrap();

    // Hunkless skeleton for paths without diff text.
    let data = vec![gap2_path_data("a.txt", svn::FLAG_MODIFIED, &[], None)];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_CHANGE);
    assert!(changes[0].hunks.is_empty());

    // Parsed hunks merge into the skeleton; the `---` target sets old path;
    // the current path stays pinned to the `Index:` header.
    let text = "Index: a.txt\n===================================================================\n--- a.txt\t(revision 9)\n+++ a.txt\t(working copy)\n@@ -1 +1 @@\n-a\n+b\n";
    let data = vec![gap2_path_data("a.txt", svn::FLAG_MODIFIED, &[], Some(text))];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "a.txt");
    assert_eq!(changes[0].old_path, "a.txt");
    assert_eq!(changes[0].hunks.len(), 1);

    // Skeleton types survive the parse (an added file has no git markers).
    let added_text = "Index: n.txt\n===================================================================\n--- n.txt\t(revision 0)\n+++ n.txt\t(working copy)\n@@ -0,0 +1 @@\n+x\n";
    let data = vec![gap2_path_data(
        "n.txt",
        svn::FLAG_ADDED,
        &[],
        Some(added_text),
    )];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes[0].change_type, TYPE_ADD);

    // The synthetic-unchanged suffix strips; current path pins to `Index:`.
    let synth = "Index: a.txt\n===================================================================\n--- a.txt (synthetic)\n+++ a.txt (synthetic)\n@@ -1,1 +1,1 @@\n a\n";
    let data = vec![gap2_path_data(
        "a.txt",
        svn::FLAG_MODIFIED,
        &[],
        Some(synth),
    )];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes[0].current_path, "a.txt");
    assert_eq!(changes[0].old_path, "a.txt");
    assert_eq!(changes[0].hunks.len(), 1);

    // The svn binary marker sets `FILE_BINARY` with no hunks.
    let binary = "Index: b.bin\n===================================================================\nCannot display: file marked as a binary type.\nsvn:mime-type = application/octet-stream\n";
    let data = vec![gap2_path_data(
        "b.bin",
        svn::FLAG_MODIFIED,
        &[],
        Some(binary),
    )];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());

    // T5555: a repeat path drops earlier hunks; the later block wins.
    let dir_text = "Index: a.txt\n===================================================================\n--- a.txt\t(revision 9)\n+++ a.txt\t(working copy)\n@@ -1 +1 @@\n-dir\n+dir2\n";
    let file_text = "Index: a.txt\n===================================================================\n--- a.txt\t(revision 9)\n+++ a.txt\t(working copy)\n@@ -1 +1 @@\n-file\n+file2\n";
    let data = vec![
        gap2_path_data("sub", svn::FLAG_ADDED, &[], Some(dir_text)),
        gap2_path_data("a.txt", svn::FLAG_MODIFIED, &[], Some(file_text)),
    ];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes.len(), 2);
    let file = changes.iter().find(|c| c.current_path == "a.txt").unwrap();
    assert_eq!(file.hunks.len(), 1);
    assert!(
        file.hunks[0].corpus.contains("-file\n"),
        "{}",
        file.hunks[0].corpus
    );

    // Blank-only diff text fails like PHP's `parseDiff()` guard.
    let data = vec![gap2_path_data(
        "a.txt",
        svn::FLAG_MODIFIED,
        &[],
        Some("  \n"),
    )];
    assert_eq!(
        merge_svn_path_diffs(&root, &data, None).unwrap_err(),
        EMPTY_DIFF
    );

    // Deleted directories report `FILE_DIRECTORY` from `Node Kind`.
    let data = vec![gap2_path_data(
        "gone",
        svn::FLAG_DELETED,
        &[("Node Kind", "directory")],
        None,
    )];
    let changes = merge_svn_path_diffs(&root, &data, None).unwrap();
    assert_eq!(changes[0].file_type, FILE_DIRECTORY);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_svn_binary_marker_matches_php() {
    assert!(is_svn_binary_marker(
        "Cannot display: file marked as a binary type."
    ));
    assert!(is_svn_binary_marker(
        "Cannot display: file marked as a binary type.  "
    ));
    assert!(!is_svn_binary_marker(
        " Cannot display: file marked as a binary type."
    ));
    assert!(!is_svn_binary_marker("Binary files a and b differ"));
}

#[test]
fn gap2_default_svn_diff_cmd_shape() {
    #[cfg(windows)]
    assert_eq!(default_svn_diff_cmd(), None);
    #[cfg(not(windows))]
    {
        let script = default_svn_diff_cmd().unwrap();
        assert!(
            script.ends_with("scripts/repository/binary_safe_diff.sh"),
            "{script:?}"
        );
    }
}
