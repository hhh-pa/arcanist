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
    // T1153 RB-D37: PHP's exact error text (`phabricator.example.com`).
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
         should have a revision identifier like \"D123\" or a Phabricator URI \
         like \"https://phabricator.example.com/D123\", but has \"D0\"."
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
    // DIFF-D21: `is_numeric()` forms are accepted and truncated.
    assert_eq!(parse_update_revision_id("1e3").unwrap(), 1000);
    assert_eq!(parse_update_revision_id("12.3").unwrap(), 12);
    assert_eq!(parse_update_revision_id("+123").unwrap(), 123);
    assert_eq!(parse_update_revision_id(" 1e2 ").unwrap(), 100);
    for bad in [
        "",
        "D",
        " D5",
        "D12x",
        "-5",
        "0x10",
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
        ChildOutcome::Code(0, Vec::new())
    };
    let mut out = Vec::new();
    assert_eq!(
        run_lint(&dir, Some("abc"), &[], true, &counting, &mut out).unwrap(),
        (Some(LINT_RESULT_SKIP), Vec::new())
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
                &run(ChildOutcome::Code(code, Vec::new())),
                &mut out
            )
            .unwrap(),
            (Some(code), Vec::new())
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
            &run(ChildOutcome::Code(3, Vec::new())),
            &mut out
        )
        .unwrap(),
        (Some(3), Vec::new())
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
        (None, Vec::new())
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
        (None, Vec::new())
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
        ChildOutcome::Code(0, Vec::new())
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
            &run(ChildOutcome::Code(0, Vec::new())),
            &mut input,
            &mut out
        )
        .unwrap(),
        (Some(UNIT_RESULT_SKIP), Vec::new())
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
                &run(ChildOutcome::Code(code, Vec::new())),
                &mut input,
                &mut out,
            )
            .unwrap(),
            (Some(code), Vec::new())
        );
        assert_eq!(out, format!("Running unit tests...\n{banner}").into_bytes());
    }
    // Unsound prompts (DIFF-D16: default no, like PHP); only `y` continues.
    {
        let mut out = Vec::new();
        let mut input = Cursor::new(b"y\n".to_vec());
        assert_eq!(
            run_unit(
                &dir,
                None,
                &[],
                false,
                &run(ChildOutcome::Code(UNIT_RESULT_UNSOUND, Vec::new())),
                &mut input,
                &mut out,
            )
            .unwrap(),
            (Some(UNIT_RESULT_UNSOUND), Vec::new())
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
    // DIFF-D16: `n` and empty (Enter/EOF take the no default) abort.
    for answer in [b"n\n".to_vec(), b"\n".to_vec(), b"".to_vec()] {
        let mut out = Vec::new();
        let mut input = Cursor::new(answer);
        assert_eq!(
            run_unit(
                &dir,
                None,
                &[],
                false,
                &run(ChildOutcome::Code(UNIT_RESULT_UNSOUND, Vec::new())),
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
    }
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
        (None, Vec::new())
    );
    assert_eq!(
        out,
        b"Running unit tests...\nNo unit test engine is configured for this project.\n"
    );
    // Unit argv carries coverage flags plus --rev.
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let capture = |_: &Path, argv: &[String]| {
        *seen.borrow_mut() = argv.to_vec();
        ChildOutcome::Code(0, Vec::new())
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
    let lint = |_: &Path, _: &[String]| ChildOutcome::Code(0, Vec::new());
    let unit_calls = RefCell::new(0);
    let unit = |_: &Path, _: &[String]| {
        *unit_calls.borrow_mut() += 1;
        ChildOutcome::Code(2, Vec::new())
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
    // T1153 DIFF-D22: child dicts flow into the report (they feed the
    // `arc:lint`/`arc:unit` diff properties).
    {
        let lint_dicts =
            |_: &Path, _: &[String]| ChildOutcome::Code(1, vec![serde_json::json!({"code": "X"})]);
        let unit_dicts =
            |_: &Path, _: &[String]| ChildOutcome::Code(0, vec![serde_json::json!({"name": "t"})]);
        let mut out = Vec::new();
        let mut input = Cursor::new(b"".to_vec());
        let report = run_lint_unit(
            &dir,
            None,
            &[],
            &[],
            false,
            false,
            &lint_dicts,
            &unit_dicts,
            &mut input,
            &mut out,
        )
        .unwrap();
        assert_eq!(
            report.unresolved_lint,
            vec![serde_json::json!({"code": "X"})]
        );
        assert_eq!(report.test_results, vec![serde_json::json!({"name": "t"})]);
    }
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
    assert_eq!(hunk.corpus, " a\n+b\n c\n");
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
fn old_path_add_reports_dev_null_like_php() {
    // T1153 OLD-PATH: `ArcanistDiffParser.php:801-808` unconditionally
    // `setOldPath($old_file)` from the `---` line, so git/hg ADD
    // carries `/dev/null` (not empty or the current path).
    let add = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n";
    let changes = parse_unified_diff(add).unwrap();
    assert_eq!(changes[0].change_type, TYPE_ADD);
    assert_eq!(changes[0].old_path, "/dev/null");
    assert_eq!(changes[0].current_path, "new.txt");
    // Hg `--git` ADD uses the same `/dev/null` shape.
    let hg_add = "diff -r abc123 new.txt\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n";
    let changes = parse_unified_diff(hg_add).unwrap();
    assert_eq!(changes[0].change_type, TYPE_ADD);
    assert_eq!(changes[0].old_path, "/dev/null");
    // DELETE keeps the path on both sides (PHP discards the `+++`
    // `/dev/null`, `:804-808` only sets the old path from `---`).
    let delete = "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
    let changes = parse_unified_diff(delete).unwrap();
    assert_eq!(changes[0].change_type, TYPE_DELETE);
    assert_eq!(changes[0].old_path, "old.txt");
    assert_eq!(changes[0].current_path, "old.txt");
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

/* T1153 wave 3 parser halves: EXPORT-E11..E24/E37 + E20. */

fn e_headers(body: &str) -> String {
    format!("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n{body}")
}

#[test]
fn e11_hunk_length_explicitness_bits() {
    // Explicit `,length` on both sides.
    let changes = parse_unified_diff(&e_headers("@@ -1,2 +1,3 @@\n a\n+b\n c\n")).unwrap();
    assert_eq!(changes[0].hunks.len(), 1);
    assert_eq!(
        changes[0].hunk_len_explicit,
        vec![HunkLenExplicit {
            old_explicit: true,
            new_explicit: true,
        }]
    );
    // Bare offsets imply length 1 and encode as numbers.
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n")).unwrap();
    assert_eq!(
        changes[0].hunk_len_explicit,
        vec![HunkLenExplicit {
            old_explicit: false,
            new_explicit: false,
        }]
    );
    // Spelled `,1` differs from omitted (string "1" vs number 1).
    let changes = parse_unified_diff(&e_headers("@@ -1,1 +1,1 @@\n-a\n+b\n")).unwrap();
    assert_eq!(
        (changes[0].hunks[0].old_len, changes[0].hunks[0].new_len),
        (1, 1)
    );
    assert_eq!(
        changes[0].hunk_len_explicit,
        vec![HunkLenExplicit {
            old_explicit: true,
            new_explicit: true,
        }]
    );
    // Mixed: explicit old, bare new.
    let changes = parse_unified_diff(&e_headers("@@ -0,0 +1 @@\n+hi\n")).unwrap();
    assert_eq!(
        changes[0].hunk_len_explicit,
        vec![HunkLenExplicit {
            old_explicit: true,
            new_explicit: false,
        }]
    );
}

#[test]
fn e12_corpus_retains_original_line_endings() {
    // CRLF bodies keep `\r\n`.
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\r\n+b\r\n")).unwrap();
    assert_eq!(changes[0].hunks[0].corpus, "-a\r\n+b\r\n");
    // An unterminated final line stays unterminated.
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b")).unwrap();
    assert_eq!(changes[0].hunks[0].corpus, "-a\n+b");
    // LF bodies are byte-identical to before.
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n")).unwrap();
    assert_eq!(changes[0].hunks[0].corpus, "-a\n+b\n");
}

#[test]
fn e13_filemodes_retained_through_parser() {
    let text = "diff --git a/f.txt b/f.txt\nold mode 100644\nnew mode 100755\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_CHANGE);
    assert_eq!(changes[0].old_mode.as_deref(), Some("100644"));
    assert_eq!(changes[0].new_mode.as_deref(), Some("100755"));

    let add = "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hi\n";
    let changes = parse_unified_diff(add).unwrap();
    assert_eq!(changes[0].change_type, TYPE_ADD);
    assert_eq!(changes[0].old_mode, None);
    assert_eq!(changes[0].new_mode.as_deref(), Some("100644"));

    let delete = "diff --git a/o.txt b/o.txt\ndeleted file mode 100644\n--- a/o.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
    let changes = parse_unified_diff(delete).unwrap();
    assert_eq!(changes[0].change_type, TYPE_DELETE);
    assert_eq!(changes[0].old_mode.as_deref(), Some("100644"));
    assert_eq!(changes[0].new_mode, None);

    // Plain changes carry no modes.
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n")).unwrap();
    assert_eq!(changes[0].old_mode, None);
    assert_eq!(changes[0].new_mode, None);
}

#[test]
fn e15_hunk_body_validates_line_counts() {
    // Too few lines.
    let err = parse_unified_diff(&e_headers("@@ -1,2 +1,2 @@\n a\n+b\n")).unwrap_err();
    assert!(err.contains(WRONG_HUNK_LINE_COUNT), "{err:?}");
    // Too many added lines (counter goes negative).
    let err = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n+c\n")).unwrap_err();
    assert!(err.contains(WRONG_HUNK_LINE_COUNT), "{err:?}");
    // Too many removed lines.
    let err = parse_unified_diff(&e_headers("@@ -1,2 +1,1 @@\n-a\n-b\n-c\n+d\n")).unwrap_err();
    assert!(err.contains(WRONG_HUNK_LINE_COUNT), "{err:?}");
    // A new header with unexhausted counts fails at that line.
    let err =
        parse_unified_diff(&e_headers("@@ -1,2 +1,2 @@\n a\n@@ -9 +9 @@\n-b\n+c\n")).unwrap_err();
    assert!(err.contains(WRONG_HUNK_LINE_COUNT), "{err:?}");
    assert!(err.contains("@@ -9 +9 @@"), "{err:?}");
    // The next block with unexhausted counts fails too.
    let err = parse_unified_diff(&format!(
        "{}diff --git a/g.txt b/g.txt\n--- a/g.txt\n+++ b/g.txt\n@@ -1 +1 @@\n-x\n+y\n",
        e_headers("@@ -1,2 +1,2 @@\n a\n")
    ))
    .unwrap_err();
    assert!(err.contains(WRONG_HUNK_LINE_COUNT), "{err:?}");
}

#[test]
fn e15_hunk_boundaries_mirror_php() {
    // A context line past exhausted counts ends the hunk (consumed).
    let changes =
        parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n c\n@@ -2 +2 @@\n-d\n+e\n")).unwrap();
    assert_eq!(changes[0].hunks.len(), 2);
    assert_eq!(changes[0].hunks[0].corpus, "-a\n+b\n");
    assert_eq!(changes[0].hunks[1].corpus, "-d\n+e\n");
    // A `-` line with the old side exhausted holds for a following
    // file (here skipped as between-block noise).
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n--- stray\n")).unwrap();
    assert_eq!(changes[0].hunks.len(), 1);
    assert_eq!(changes[0].hunks[0].corpus, "-a\n+b\n");
    // ...but with the new side unexhausted it is a miscount.
    let err = parse_unified_diff(&e_headers("@@ -1,1 +1,2 @@\n-a\n+b\n-c\n+d\n")).unwrap_err();
    assert!(err.contains(WRONG_HUNK_LINE_COUNT), "{err:?}");
    // Empty lines count as context (`diff.suppressBlankEmpty`).
    let changes = parse_unified_diff(&e_headers("@@ -1,2 +1,2 @@\n a\n\n b\n")).unwrap();
    assert_eq!(changes[0].hunks[0].corpus, " a\n\n");
    // A zero-length hunk holds no body.
    let changes = parse_unified_diff(&e_headers("@@ -0,0 +0,0 @@\n")).unwrap();
    assert_eq!(changes[0].hunks.len(), 1);
    assert!(changes[0].hunks[0].corpus.is_empty());
}

#[test]
fn e15_no_newline_markers_validate_and_persist() {
    // Malformed `\` lines fail.
    let err = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n\\ bogus\n+b\n")).unwrap_err();
    assert!(err.contains(EXPECTED_NO_NEWLINE_MARKER), "{err:?}");
    // Well-formed markers persist in the corpus; a middle marker
    // (new side unexhausted) continues the hunk.
    let changes = parse_unified_diff(&e_headers(
        "@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n",
    ))
    .unwrap();
    assert_eq!(
        changes[0].hunks[0].corpus,
        "-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n"
    );
}

#[test]
fn e15_malformed_hunk_headers_fail() {
    // Missing closing `@@`.
    let err = parse_unified_diff(&e_headers("@@ -1 +1\n-a\n+b\n")).unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_HEADER), "{err:?}");
    // Combined-style third range.
    let err = parse_unified_diff(&e_headers("@@ -1,2 +1,2 +1,2 @@\n-a\n+b\n")).unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_HEADER), "{err:?}");
    // Non-numeric range.
    let err = parse_unified_diff(&e_headers("@@ -a +1 @@\n-a\n+b\n")).unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_HEADER), "{err:?}");
    // Without `---`/`+++` the failure names the hunk target instead.
    let err = parse_unified_diff("diff --git a/f.txt b/f.txt\n@@ -1 +1\n").unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_TARGET), "{err:?}");
}

#[test]
fn e16_c_quoted_paths_decode_like_php() {
    // `---`/`+++` paths stay literal, quotes included.
    let changes = parse_unified_diff(
        "diff --git a/f.txt b/f.txt\n--- \"a/old name\"\n+++ \"b/new name\"\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    assert_eq!(changes[0].old_path, "\"a/old name\"");
    assert_eq!(changes[0].current_path, "\"b/new name\"");
    // The `diff --git` pair decodes (quoted, with spaces).
    let changes = parse_unified_diff(
        "diff --git \"a/old name\" \"b/old name\"\n--- \"a/old name\"\n+++ \"b/old name\"\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    // Non-rename `+++` still overwrites from the literal line (the
    // `+++`-discard applies to resolved renames/copies only).
    assert_eq!(changes[0].old_path, "\"a/old name\"");
    assert_eq!(changes[0].current_path, "\"b/old name\"");
    // Quoted renames decode the `rename to` side and keep the
    // literal `---` old path (PHP overwrites the decoded source).
    let changes = parse_unified_diff(
        "diff --git \"a/o n\" \"b/n n\"\nrename from \"o n\"\nrename to \"n n\"\n--- \"a/o n\"\n+++ \"b/n n\"\n@@ -1 +1 @@\n-x\n+y\n",
    )
    .unwrap();
    assert_eq!(changes[0].change_type, TYPE_MOVE_HERE);
    assert_eq!(changes[0].old_path, "\"a/o n\"");
    assert_eq!(changes[0].current_path, "n n");
    // Octal escapes decode to bytes.
    let changes = parse_unified_diff(
        "diff --git \"a/f\\303\\244\" \"b/f\\303\\244\"\n--- \"a/f\\303\\244\"\n+++ \"b/f\\303\\244\"\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    assert_eq!(changes[0].old_path, "\"a/f\\303\\244\"");
    // Unquoted paths are untouched.
    let changes = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n")).unwrap();
    assert_eq!(changes[0].old_path, "f.txt");
    assert_eq!(changes[0].current_path, "f.txt");
}

#[test]
fn e16_git_common_filename_and_unescape_units() {
    assert_eq!(
        extract_git_common_filename("a/f.txt b/f.txt", false).as_deref(),
        Some("f.txt")
    );
    assert_eq!(
        extract_git_common_filename("\"a/old name\" \"b/old name\"", false).as_deref(),
        Some("old name")
    );
    assert_eq!(extract_git_common_filename("a/o b/n", false), None);
    assert_eq!(
        extract_git_common_filename("a/f.txt b/f.txt junk", false),
        None
    );
    assert_eq!(
        extract_git_common_filename("src/f dst/f", false).as_deref(),
        Some("f")
    );
    assert_eq!(
        extract_git_common_filename("f f", false).as_deref(),
        Some("f")
    );
    assert_eq!(
        extract_git_common_filename("a/ a/", false).as_deref(),
        Some("a/")
    );
    assert_eq!(
        extract_git_common_filename("/f /f", false).as_deref(),
        Some("/f")
    );
    assert_eq!(unescape_git_filename("\"a\\tb\"", false), "a\tb");
    assert_eq!(unescape_git_filename("\"a\\303\\244\"", false), "aä");
    assert_eq!(unescape_git_filename("plain", false), "plain");
    assert_eq!(unescape_git_filename("\"\"", false), "\"\"");
    assert_eq!(stripcslashes("a\\nb\\ x", false), "a\nb x");
    assert_eq!(stripcslashes("\\101", false), "A");
    // Degenerate `a/`-only targets strip like PHP.
    assert_eq!(diff_path_value("--- a/", false), "");
    assert_eq!(strip_ab_prefix("a/"), "");
}

#[test]
fn e21_typechange_blocks_merge_to_change() {
    // Real git form (probed): a delete block plus an add block.
    let text = "diff --git a/link b/link\n\
        deleted file mode 120000\n\
        index ac558a3..0000000\n\
        --- a/link\n\
        +++ /dev/null\n\
        @@ -1 +0,0 @@\n\
        -real\n\
        \\ No newline at end of file\n\
        diff --git a/link b/link\n\
        new file mode 100644\n\
        index 0000000..d95f3ad\n\
        --- /dev/null\n\
        +++ b/link\n\
        @@ -0,0 +1 @@\n\
        +content\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(change.change_type, TYPE_CHANGE);
    assert_eq!(change.old_path, "/dev/null");
    assert_eq!(change.current_path, "link");
    assert_eq!(change.old_mode.as_deref(), Some("120000"));
    assert_eq!(change.new_mode.as_deref(), Some("100644"));
    assert_eq!(change.hunks.len(), 1);
    assert_eq!(change.hunks[0].corpus, "+content\n");
    // Reversed order (add then delete) deletes unconditionally.
    let text = "diff --git a/f b/f\n\
        new file mode 100644\n\
        --- /dev/null\n\
        +++ b/f\n\
        @@ -0,0 +1 @@\n\
        +hi\n\
        diff --git a/f b/f\n\
        deleted file mode 100644\n\
        --- a/f\n\
        +++ /dev/null\n\
        @@ -1 +0,0 @@\n\
        -hi\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_DELETE);
    // Both flags in one block read as delete-then-new.
    let text = "diff --git a/f b/f\n\
        deleted file mode 120000\n\
        new file mode 100644\n\
        --- a/f\n\
        +++ b/f\n\
        @@ -1 +1 @@\n\
        -a\n\
        +b\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes[0].change_type, TYPE_CHANGE);
}

#[test]
fn e22_files_differ_wording_is_binary() {
    let text = "diff --git a/f b/f\nFiles a/f and b/f differ\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());
    assert_eq!(changes[0].hunk_len_explicit.len(), 0);
}

#[test]
fn e23_malformed_binary_lines_fail() {
    // Before `---`/`+++`: the hunk-target text.
    let err = parse_unified_diff("diff --git a/f b/f\nBinary files x\n").unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_TARGET), "{err:?}");
    // Past the paths: the hunk-header text.
    let err = parse_unified_diff(&e_headers("Binary files x\n")).unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_HEADER), "{err:?}");
    // The `Files` wording without a tail fails the same way.
    let err = parse_unified_diff("diff --git a/f b/f\nFiles x\n").unwrap_err();
    assert!(err.contains(EXPECTED_HUNK_TARGET), "{err:?}");
}

#[test]
fn e24_duplicate_blocks_merge_later_wins() {
    // Later hunks win; the position stays stable.
    let text = "Index: b.txt\n\
        --- b.txt\t(revision 1)\n\
        +++ b.txt\t(working copy)\n\
        @@ -1 +1 @@\n\
        -b\n\
        +B\n\
        Index: a.txt\n\
        --- a.txt\t(revision 1)\n\
        +++ a.txt\t(working copy)\n\
        @@ -1 +1 @@\n\
        -a\n\
        +A\n\
        Index: b.txt\n\
        --- b.txt\t(revision 2)\n\
        +++ b.txt\t(working copy)\n\
        @@ -1 +1 @@\n\
        -b\n\
        +B2\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].current_path, "b.txt");
    assert_eq!(changes[0].hunks[0].corpus, "-b\n+B2\n");
    assert_eq!(changes[1].current_path, "a.txt");
    // An armless incoming block keeps the existing type (PHP
    // mutates the reused change through arms only).
    let text = "diff --git a/f b/f\n\
        deleted file mode 100644\n\
        --- a/f\n\
        +++ /dev/null\n\
        @@ -1 +0,0 @@\n\
        -x\n\
        diff --git a/f b/f\n\
        --- a/f\n\
        +++ b/f\n\
        @@ -1 +1 @@\n\
        -x\n\
        +y\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_DELETE);
    assert_eq!(changes[0].hunks[0].corpus, "-x\n+y\n");
    // Binary is sticky across the merge.
    let text = "diff --git a/f b/f\n\
        Binary files a/f and b/f differ\n\
        diff --git a/f b/f\n\
        --- a/f\n\
        +++ b/f\n\
        @@ -1 +1 @@\n\
        -a\n\
        +b\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert_eq!(changes[0].hunks[0].corpus, "-a\n+b\n");
    // Filemodes union across the merge.
    let text = "diff --git a/f b/f\n\
        old mode 100644\n\
        diff --git a/f b/f\n\
        new mode 100755\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].old_mode.as_deref(), Some("100644"));
    assert_eq!(changes[0].new_mode.as_deref(), Some("100755"));
}

#[test]
fn e24_rename_blocks_never_merge() {
    // Unkeyed rename/copy blocks stay separate (like PHP's
    // `buildChange(null)`). Companion folding is EXPORT-E14
    // carry-over: the export side fabricates one companion per
    // here-change today.
    let text = "diff --git a/src b/one\n\
        copy from src\n\
        copy to one\n\
        --- a/src\n\
        +++ b/one\n\
        @@ -1 +1 @@\n\
        -a\n\
        +b\n\
        diff --git a/src b/two\n\
        copy from src\n\
        copy to two\n\
        --- a/src\n\
        +++ b/two\n\
        @@ -1 +1 @@\n\
        -a\n\
        +b\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].change_type, TYPE_COPY_HERE);
    assert_eq!(changes[1].change_type, TYPE_COPY_HERE);
}

#[test]
fn e37_hg_binary_changed_marker_is_binary() {
    let text = "diff -r abc123 f.bin\nBinary file f.bin has changed\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "f.bin");
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());
}

#[test]
fn e20_synthetic_predicate_flags_pure_moves() {
    let pure =
        "diff --git a/o.txt b/n.txt\nsimilarity index 100%\nrename from o.txt\nrename to n.txt\n";
    let changes = parse_unified_diff(pure).unwrap();
    assert_eq!(changes[0].change_type, TYPE_MOVE_HERE);
    assert!(changes[0].hunks.is_empty());
    assert!(needs_synthetic_git_hunks(&changes[0]));

    let moved = "diff --git a/o.txt b/n.txt\nrename from o.txt\nrename to n.txt\n--- a/o.txt\n+++ b/n.txt\n@@ -1 +1 @@\n-x\n+y\n";
    let changes = parse_unified_diff(moved).unwrap();
    assert!(!needs_synthetic_git_hunks(&changes[0]));

    let plain = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n")).unwrap();
    assert!(!needs_synthetic_git_hunks(&plain[0]));
}

#[test]
fn e14_single_move_gains_away_companion() {
    // T1153 EXPORT-E14: one rename yields the here-change plus a
    // `MOVE_AWAY` companion (null old path, away path set) right after
    // it, like PHP's `buildChange($oldPath)` (`:668-687`).
    let text = "diff --git a/g.txt b/h.txt\nsimilarity index 90%\nrename from g.txt\nrename to h.txt\n--- a/g.txt\n+++ b/h.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let full = with_companions(&parse_unified_diff(text).unwrap());
    assert_eq!(full.len(), 2);
    assert_eq!(full[0].change_type, TYPE_MOVE_HERE);
    assert_eq!(full[1].change_type, TYPE_MOVE_AWAY);
    assert_eq!(full[1].current_path, "g.txt");
    assert!(full[1].old_path.is_empty());
    assert_eq!(full[1].away_paths, vec!["h.txt".to_string()]);
    assert!(full[1].hunks.is_empty());
    assert_eq!(full[1].file_type, FILE_TEXT);
}

#[test]
fn e14_repeats_from_one_source_fold_to_one_companion() {
    // T1153 EXPORT-E14: two copies from one source fold to one
    // `COPY_AWAY` with both away paths (`:654-666`); a move folded
    // onto it retypes to `MULTICOPY` (`:677-678`).
    let text = "diff --git a/src b/one\ncopy from src\ncopy to one\n--- a/src\n+++ b/one\n@@ -1 +1 @@\n-a\n+b\n\
        diff --git a/src b/two\ncopy from src\ncopy to two\n--- a/src\n+++ b/two\n@@ -1 +1 @@\n-a\n+b\n";
    let full = with_companions(&parse_unified_diff(text).unwrap());
    assert_eq!(full.len(), 3);
    assert_eq!(full[1].change_type, TYPE_COPY_AWAY);
    assert_eq!(full[1].current_path, "src");
    assert_eq!(
        full[1].away_paths,
        vec!["one".to_string(), "two".to_string()]
    );
    // A move onto the copy-away retypes to multicopy.
    let mut fold = CompanionFold::new();
    let (at, is_new) = fold.fold(TYPE_COPY_HERE, "src", "one");
    assert!(is_new);
    let (again, is_new) = fold.fold(TYPE_MOVE_HERE, "src", "two");
    assert!(!is_new);
    assert_eq!(again, at);
    assert_eq!(fold.companion(at).change_type, TYPE_MULTICOPY);
    assert_eq!(
        fold.companion(at).away_paths,
        vec!["one".to_string(), "two".to_string()]
    );
}

#[test]
fn e14_with_companions_skips_paired_and_repeats() {
    // T1153 EXPORT-E14: here-changes the Subversion collector already
    // paired keep their companion (no double), and re-running the
    // fold is a no-op.
    let text =
        "diff --git a/g.txt b/h.txt\nsimilarity index 100%\nrename from g.txt\nrename to h.txt\n";
    let once = with_companions(&parse_unified_diff(text).unwrap());
    assert_eq!(once.len(), 2);
    let twice = with_companions(&once);
    assert_eq!(twice, once);
    // Non-moves pass through untouched.
    let plain = parse_unified_diff(&e_headers("@@ -1 +1 @@\n-a\n+b\n")).unwrap();
    assert_eq!(with_companions(&plain), plain);
}

#[test]
fn e20_attach_fabricates_here_and_companion_hunks() {
    // T1153 EXPORT-E20: `attach_synthetic_git_hunks` runs the
    // `loadSyntheticData()` fabrication over folded companions too:
    // the here-change gains context hunks (counts kept from the
    // add-side reparse), the move-away companion delete-side hunks.
    let text =
        "diff --git a/o.txt b/n.txt\nsimilarity index 100%\nrename from o.txt\nrename to n.txt\n";
    let mut full = with_companions(&parse_unified_diff(text).unwrap());
    assert_eq!(full.len(), 2);
    let raw_for = |path: &str| -> Result<String, String> {
        if path == "n.txt" {
            Ok("diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n".to_string())
        } else {
            Ok("diff --git a/o.txt b/o.txt\ndeleted file mode 100644\n--- a/o.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n".to_string())
        }
    };
    attach_synthetic_git_hunks(&mut full, &raw_for).unwrap();
    assert_eq!(full[0].hunks.len(), 1);
    assert_eq!(full[0].hunks[0].corpus, " one\n two\n");
    assert_eq!(hunk_counts(&full[0].hunks[0]), (2, 0));
    assert_eq!(full[1].hunks.len(), 1);
    assert_eq!(full[1].hunks[0].corpus, "-one\n-two\n");
    assert_eq!(hunk_counts(&full[1].hunks[0]), (0, 2));
    // Paths with no reparse match keep no hunks, like PHP.
    let mut lonely = with_companions(&parse_unified_diff(text).unwrap());
    let empty = |_: &str| -> Result<String, String> {
        Ok("diff --git a/q b/q\n--- a/q\n+++ b/q\n@@ -1 +1 @@\n-a\n+b\n".to_string())
    };
    attach_synthetic_git_hunks(&mut lonely, &empty).unwrap();
    assert!(lonely[0].hunks.is_empty());
    assert!(lonely[1].hunks.is_empty());
}

#[test]
fn e20_synthetic_hunks_rewrite_adds_to_context() {
    // A pure-move reparse reads as an add; `+` rewrites to ` `.
    let raw = "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n";
    let made = synthetic_hunks_for_path(raw, "n.txt").unwrap().unwrap();
    assert_eq!(made.file_type, FILE_TEXT);
    assert_eq!(made.hunks.len(), 1);
    assert_eq!(
        (
            made.hunks[0].old_start,
            made.hunks[0].old_len,
            made.hunks[0].new_start,
            made.hunks[0].new_len,
        ),
        (0, 0, 1, 2)
    );
    assert_eq!(made.hunks[0].corpus, " one\n two\n");
    // The move-source reparse reads as a delete; `-` lines persist.
    let raw = "diff --git a/o.txt b/o.txt\ndeleted file mode 100644\n--- a/o.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n";
    let made = synthetic_hunks_for_path(raw, "o.txt").unwrap().unwrap();
    assert_eq!(made.hunks[0].corpus, "-one\n-two\n");
    // No change targets the path: nothing attaches.
    assert!(
        synthetic_hunks_for_path(raw, "elsewhere")
            .unwrap()
            .is_none()
    );
    // Binary reparses propagate the file type with no hunks.
    let raw = "diff --git a/i.png b/i.png\nBinary files a/i.png and b/i.png differ\n";
    let made = synthetic_hunks_for_path(raw, "i.png").unwrap().unwrap();
    assert_eq!(made.file_type, FILE_BINARY);
    assert!(made.hunks.is_empty());
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
        hunk_len_explicit: Vec::new(),
        old_mode: None,
        new_mode: None,
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
            hunk_len_explicit: Vec::new(),
            old_mode: None,
            new_mode: None,
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

/* Gap2 D10: pullDataFromConduit. */

#[test]
fn gap2_pull_data_from_conduit_assigns_fields() {
    let seen = std::cell::RefCell::new((String::new(), serde_json::json!(null)));
    let call = |method: &str, params: &serde_json::Value| {
        *seen.borrow_mut() = (method.to_string(), params.clone());
        Ok(serde_json::json!({
            "fields": {
                "title": "Hello",
                "reviewerPHIDs": ["PHID-USER-1", "PHID-USER-2"],
                "count": 7,
            },
            "transactions": [{"type": "title"}],
            "errors": [],
        }))
    };
    let mut message = CommitMessage::new_from_raw_corpus("Hello\n").unwrap();
    assert!(message.get_fields().is_empty());
    assert_eq!(message.get_transactions(), None);
    message.pull_data_from_conduit(&call, true).unwrap();

    let (method, params) = seen.borrow().clone();
    assert_eq!(method, "differential.parsecommitmessage");
    assert_eq!(params["corpus"], serde_json::json!("Hello\n"));
    assert_eq!(params["partial"], serde_json::json!(true));

    assert_eq!(
        message.get_field_value("title"),
        Some(&FieldValue::Text("Hello".to_string()))
    );
    assert_eq!(
        message.get_field_value("reviewerPHIDs"),
        Some(&FieldValue::List(vec![
            "PHID-USER-1".to_string(),
            "PHID-USER-2".to_string()
        ]))
    );
    assert_eq!(
        message.get_field_value("count"),
        Some(&FieldValue::Text("7".to_string()))
    );
    assert_eq!(message.get_field_value("missing"), None);
    assert_eq!(
        message.get_transactions(),
        Some(&serde_json::json!([{"type": "title"}]))
    );

    message.set_field_value("title", FieldValue::Text("Changed".to_string()));
    assert_eq!(
        message.get_fields().get("title"),
        Some(&FieldValue::Text("Changed".to_string()))
    );
}

#[test]
fn gap2_pull_data_parser_errors_carry_list() {
    let call = |_: &str, _: &serde_json::Value| {
        Ok(serde_json::json!({
            "fields": {"title": "Hello"},
            "errors": ["first problem", "second problem"],
        }))
    };
    let mut message = CommitMessage::new_from_raw_corpus("Hello\n").unwrap();
    let err = message.pull_data_from_conduit(&call, false).unwrap_err();
    // Fields assign before the throw, like PHP.
    assert_eq!(
        message.get_field_value("title"),
        Some(&FieldValue::Text("Hello".to_string()))
    );
    // Pre-October-2017 responses carry no transactions.
    assert_eq!(message.get_transactions(), None);
    assert_eq!(
        err,
        PullDataError::Parser(vec![
            "first problem".to_string(),
            "second problem".to_string()
        ])
    );
    assert_eq!(err.into_message(), "first problem");

    let transport = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("down".into())
    };
    let mut message = CommitMessage::new_from_raw_corpus("Hello\n").unwrap();
    let err = message
        .pull_data_from_conduit(&transport, false)
        .unwrap_err();
    assert_eq!(err, PullDataError::Other("down".to_string()));
    assert_eq!(err.into_message(), "down");
}

#[test]
fn gap2_commit_message_monogram_and_defaults() {
    let message =
        CommitMessage::new_from_raw_corpus("Title\n\nDifferential Revision: D123\n").unwrap();
    assert_eq!(message.revision_id, Some(123));
    assert_eq!(message.get_revision_monogram(), Some("D123".to_string()));
    let plain = CommitMessage::new_from_raw_corpus("Title\n").unwrap();
    assert_eq!(plain.get_revision_monogram(), None);
}

/* Gap2 D1: create-message edit loop. */

fn gap2_d1_scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-diff-d1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}

fn gap2_d1_ok_call(method: &str, _params: &serde_json::Value) -> Result<serde_json::Value, String> {
    match method {
        "differential.parsecommitmessage" => Ok(serde_json::json!({
            "fields": {"title": "Hello"},
            "errors": [],
        })),
        "differential.query" => Ok(serde_json::json!([
            {"id": 7, "title": "Mine", "authorPHID": "PHID-USER-me", "statusName": "Accepted"},
        ])),
        "user.query" => Ok(serde_json::json!({})),
        "differential.getcommitmessage" => Ok(serde_json::json!("Template title\n")),
        other => Err(format!("unexpected conduit call {other}")),
    }
}

#[test]
fn gap2_build_editor_template_shapes() {
    assert_eq!(
        build_editor_template("Title\n", &[]),
        "Title\n\n\n".to_string()
    );
    assert_eq!(
        build_editor_template("Title\r\n\r\n", &["one".to_string(), "two  ".to_string()]),
        "Title\n\n# one\n# two\n\n".to_string()
    );
    assert_eq!(
        build_editor_template("", &["".to_string()]),
        "\n\n#\n\n".to_string()
    );
}

#[test]
fn gap2_resolve_saved_template_flows() {
    use std::io::Cursor;
    let root = gap2_d1_scratch("saved");
    let meta = root.join(".git");

    // No scratch file, or verbatim: nothing to resume.
    let mut out = Vec::new();
    assert_eq!(
        resolve_saved_template(
            &mut Cursor::new(b"".to_vec()),
            &mut out,
            &meta,
            &root,
            false
        )
        .unwrap(),
        None
    );
    assert!(out.is_empty());
    diff::write_scratch_file(&meta, SCRATCH_CREATE_MESSAGE, "Saved title\nBody\n");
    let mut out = Vec::new();
    assert_eq!(
        resolve_saved_template(&mut Cursor::new(b"".to_vec()), &mut out, &meta, &root, true)
            .unwrap(),
        None
    );
    assert!(out.is_empty());

    // Acceptance reuses the template after the exact notice bytes.
    let mut out = Vec::new();
    let reused = resolve_saved_template(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &meta,
        &root,
        false,
    )
    .unwrap();
    assert_eq!(reused, Some("Saved title\nBody\n".to_string()));
    let readable = diff::readable_scratch_file_path(&meta, &root, SCRATCH_CREATE_MESSAGE);
    let expected = format!(
        "{}\n\n{}\n",
        saved_message_notice(&readable, &truncate_preview_64("Saved title\nBody\n")),
        diff::format_confirm_prompt(PROMPT_USE_SAVED_MESSAGE, false).trim_start_matches("\n\n")
    );
    assert_eq!(String::from_utf8(out).unwrap(), expected);

    // Refusal deletes the scratch file.
    let mut out = Vec::new();
    let reused = resolve_saved_template(
        &mut Cursor::new(b"n\n".to_vec()),
        &mut out,
        &meta,
        &root,
        false,
    )
    .unwrap();
    assert_eq!(reused, None);
    assert!(diff::read_scratch_file(&meta, SCRATCH_CREATE_MESSAGE).is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_check_revision_ownership_flows() {
    use std::io::Cursor;
    let mine = serde_json::json!({"id": 1, "title": "T", "authorPHID": "PHID-USER-me"});
    let mut out = Vec::new();
    check_revision_ownership(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &mine,
        "PHID-USER-me",
    )
    .unwrap();
    assert!(out.is_empty());

    let other = serde_json::json!({"id": 2, "title": "O", "authorPHID": "PHID-USER-you"});
    let mut out = Vec::new();
    check_revision_ownership(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &other,
        "PHID-USER-me",
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("You don't own revision D2: \"O\"."), "{text}");

    let mut out = Vec::new();
    let err = check_revision_ownership(
        &mut Cursor::new(b"n\n".to_vec()),
        &mut out,
        &other,
        "PHID-USER-me",
    )
    .unwrap_err();
    assert_eq!(err, diff_submit::OWNERSHIP_ABORT);
}

#[test]
fn gap2_validate_commit_message_flows() {
    use std::io::Cursor;

    // No revision, no reviewers: silent success, no conduit calls.
    let message = CommitMessage::new_from_raw_corpus("Title\n").unwrap();
    let fail = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("must not call".to_string())
    };
    let mut out = Vec::new();
    validate_commit_message(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &fail,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap();
    assert!(out.is_empty());

    // Unknown revision id.
    let message =
        CommitMessage::new_from_raw_corpus("Title\n\nDifferential Revision: D9\n").unwrap();
    let empty = |_: &str, _: &serde_json::Value| Ok(serde_json::json!([]));
    let mut out = Vec::new();
    let err = validate_commit_message(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &empty,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap_err();
    assert_eq!(err, "There is no revision D9.");

    // Owned revision validates (PHP takes the first row without
    // comparing ids, so the canned D7 row satisfies the D9 message).
    let mut out = Vec::new();
    validate_commit_message(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &gap2_d1_ok_call,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap();
    assert!(out.is_empty());
}

#[test]
fn gap2_validate_reviewer_availability_flows() {
    use std::io::Cursor;
    let mut message = CommitMessage::new_from_raw_corpus("Title\n").unwrap();
    message.set_field_value(
        "reviewerPHIDs",
        FieldValue::List(vec!["PHID-USER-away".to_string()]),
    );
    let away_user = |_: &str, _: &serde_json::Value| {
        Ok(serde_json::json!({
            "PHID-USER-away": {
                "phid": "PHID-USER-away",
                "userName": "away",
                "realName": "",
                "isDisabled": false,
                "isAway": true,
                "currentStatus": "away",
                "currentStatusUntil": 1_700_000_000u64,
            },
        }))
    };

    let mut out = Vec::new();
    validate_commit_message(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &away_user,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("away (until"), "{text}");
    assert!(text.contains(diff_submit::CONTINUE_DESPITE_AWAY), "{text}");

    let mut out = Vec::new();
    let err = validate_commit_message(
        &mut Cursor::new(b"n\n".to_vec()),
        &mut out,
        &away_user,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap_err();
    assert_eq!(err, diff_submit::SPECIFY_AVAILABLE_REVIEWERS);
}

#[test]
fn gap2_unresolved_errors_error_shapes() {
    assert_eq!(
        unresolved_errors_error(Some("somewhere")),
        "Message has unresolved errors. A copy was saved to somewhere."
    );
    assert_eq!(
        unresolved_errors_error(None),
        "Message has unresolved errors. "
    );
}

#[test]
fn gap2_edit_create_message_first_pass() {
    use std::io::Cursor;
    let root = gap2_d1_scratch("loop-first");
    let meta = root.join(".git");
    let calls = std::cell::RefCell::new(0usize);
    let edit = |_: &str| -> Result<String, String> {
        *calls.borrow_mut() += 1;
        Ok("Edited\n".to_string())
    };

    // Verbatim skips the editor on the first pass for non-default templates.
    let mut input = Cursor::new(b"".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &gap2_d1_ok_call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let message = edit_create_message(
        &mut editor,
        "Raw template\n",
        "Raw template\n",
        false,
        true,
        &[],
    )
    .unwrap();
    assert_eq!(*calls.borrow(), 0);
    assert_eq!(message.raw_corpus, "Raw template\n");
    assert_eq!(
        diff::read_scratch_file(&meta, SCRATCH_CREATE_MESSAGE),
        Some("Raw template\n".to_string())
    );

    // Default templates saved unedited fail.
    let passthrough = |template: &str| -> Result<String, String> { Ok(template.to_string()) };
    let mut input = Cursor::new(b"".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &gap2_d1_ok_call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &passthrough,
        amend_commit: None,
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let err = edit_create_message(&mut editor, "Title\n", "Title\n", true, false, &[]).unwrap_err();
    assert_eq!(err, TEMPLATE_NOT_EDITED);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_edit_create_message_reedits_on_parser_errors() {
    use std::io::Cursor;
    let root = gap2_d1_scratch("loop-reedit");
    let meta = root.join(".git");
    let flaky = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.parsecommitmessage"
            && params["corpus"].as_str().unwrap_or("").contains("Bad")
        {
            return Ok(serde_json::json!({
                "fields": {},
                "errors": ["Title is required."],
            }));
        }
        gap2_d1_ok_call(method, params)
    };
    let calls = std::cell::RefCell::new(0usize);
    let edit = |_: &str| -> Result<String, String> {
        *calls.borrow_mut() += 1;
        if *calls.borrow() == 1 {
            return Ok("Bad\n".to_string());
        }
        Ok("Good title\n".to_string())
    };

    let mut input = Cursor::new(b"y\n".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &flaky,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let message = edit_create_message(&mut editor, "Seed\n", "Seed\n", false, false, &[]).unwrap();
    assert_eq!(*calls.borrow(), 2);
    assert_eq!(message.raw_corpus, "Good title\n");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(COMMIT_MESSAGE_HAS_ERRORS), "{text}");
    assert!(text.contains("Title is required."), "{text}");
    assert!(text.contains(MUST_RESOLVE_ERRORS), "{text}");
    assert!(text.contains(PROMPT_EDIT_MESSAGE_AGAIN), "{text}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_edit_create_message_aborts_unresolved() {
    use std::io::Cursor;
    let root = gap2_d1_scratch("loop-abort");
    let meta = root.join(".git");
    let failing =
        |_: &str, _: &serde_json::Value| Ok(serde_json::json!({"fields": {}, "errors": ["Nope."]}));
    let edit = |_: &str| -> Result<String, String> { Ok("Bad\n".to_string()) };

    let mut input = Cursor::new(b"n\n".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &failing,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let err = edit_create_message(&mut editor, "Seed\n", "Seed\n", false, false, &[]).unwrap_err();
    let readable = diff::readable_scratch_file_path(&meta, &root, SCRATCH_CREATE_MESSAGE);
    assert_eq!(
        err,
        format!("Message has unresolved errors. A copy was saved to '{readable}'.")
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_edit_create_message_amends_single_commit() {
    use std::io::Cursor;
    let root = gap2_d1_scratch("loop-amend");
    let meta = root.join(".git");
    let amended = std::cell::RefCell::new(Vec::<String>::new());
    let amend = |message: &str| -> Result<(), String> {
        amended.borrow_mut().push(message.to_string());
        Ok(())
    };
    let edit = |_: &str| -> Result<String, String> { Ok("Amended title\n".to_string()) };

    let mut input = Cursor::new(b"".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &gap2_d1_ok_call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: Some(&amend),
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let message = edit_create_message(&mut editor, "Old\n", "Old\n", false, false, &[]).unwrap();
    assert_eq!(message.raw_corpus, "Amended title\n");
    assert_eq!(amended.borrow().as_slice(), ["Amended title\n".to_string()]);
    // Amending never touches the scratch file.
    assert!(diff::read_scratch_file(&meta, SCRATCH_CREATE_MESSAGE).is_none());

    // An unchanged message skips the amend hook.
    let edit_same = |_: &str| -> Result<String, String> { Ok("Old\n".to_string()) };
    let mut input = Cursor::new(b"".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &gap2_d1_ok_call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit_same,
        amend_commit: Some(&amend),
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    edit_create_message(&mut editor, "Old\n", "Old\n", false, false, &[]).unwrap();
    assert_eq!(amended.borrow().len(), 1);

    let _ = std::fs::remove_dir_all(&root);
}

fn gap2_d1_local_commit(commit: &str, summary: &str, message: &str) -> LocalCommitInfo {
    LocalCommitInfo {
        commit: commit.to_string(),
        tree: "t".to_string(),
        parents: Vec::new(),
        time: "0".to_string(),
        author: "a".to_string(),
        summary: summary.to_string(),
        message: message.to_string(),
        author_email: "a@x".to_string(),
    }
}

#[test]
fn gap2_get_default_create_fields_merges_oldest_first() {
    let call = |_: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        let corpus = params["corpus"].as_str().unwrap_or("");
        if corpus.contains("Newest") {
            return Ok(serde_json::json!({
                "fields": {"title": "Newest title", "summary": "Newest summary"},
                "errors": [],
            }));
        }
        if corpus.contains("Oldest") {
            return Ok(serde_json::json!({
                "fields": {"title": "Oldest title"},
                "errors": [],
            }));
        }
        Ok(serde_json::json!({"fields": {}, "errors": []}))
    };
    let local = vec![
        gap2_d1_local_commit("newest111111", "Newest", "Newest\n"),
        gap2_d1_local_commit("oldest222222", "Oldest", "Oldest\n"),
    ];
    let (fields, included, notes) =
        get_default_create_fields_with(&call, &local, true, Some("r1"), Some("c1"), false).unwrap();
    // Faux flags first, then newest-first commits.
    assert_eq!(included.len(), 3);
    assert!(included[0].contains("Command-Line Flags"), "{included:?}");
    assert_eq!(included[1], "newest111111 Newest");
    assert_eq!(included[2], "oldest222222 Oldest");
    assert!(notes.is_empty());
    // Oldest title wins; the newest title demotes to summary.
    assert_eq!(
        fields.get("title"),
        Some(&FieldValue::Text("Oldest title".to_string()))
    );

    // Parse failures become notes (verbatim rethrows the first error).
    let flaky = |_: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        if params["corpus"].as_str().unwrap_or("").contains("Oldest") {
            return Ok(serde_json::json!({"fields": {}, "errors": ["Bad tag."]}));
        }
        call("differential.parsecommitmessage", params)
    };
    let (_, _, notes) =
        get_default_create_fields_with(&flaky, &local, true, None, None, false).unwrap();
    assert_eq!(notes.len(), 2);
    assert!(notes[0].contains("oldest222222"), "{notes:?}");
    assert_eq!(notes[1], "  - Bad tag.");
    let err = get_default_create_fields_with(&flaky, &local, true, None, None, true).unwrap_err();
    assert_eq!(err, "Bad tag.");
}

#[test]
fn gap2_get_commit_message_from_file_and_revision() {
    use std::io::Cursor;
    let root = gap2_d1_scratch("from-file");
    let file = root.join("msg.txt");
    std::fs::write(&file, "File title\n").unwrap();

    let mut out = Vec::new();
    let message = get_commit_message_from_file_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &gap2_d1_ok_call,
        &file,
        "PHID-USER-me",
        0,
    )
    .unwrap();
    assert_eq!(message.raw_corpus, "File title\n");
    assert_eq!(
        message.get_field_value("title"),
        Some(&FieldValue::Text("Hello".to_string()))
    );

    // Unknown revisions fail before any prompt.
    let empty = |_: &str, _: &serde_json::Value| Ok(serde_json::json!([]));
    let mut out = Vec::new();
    let err = get_commit_message_from_revision_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &empty,
        "42",
        "PHID-USER-me",
    )
    .unwrap_err();
    assert_eq!(err, "Revision '42' does not exist!");

    // Owned draft revisions return the template plus the draft flag.
    let draft = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.query" {
            return Ok(serde_json::json!([
                {"id": 7, "title": "Mine", "authorPHID": "PHID-USER-me", "statusName": "Draft"},
            ]));
        }
        gap2_d1_ok_call(method, &serde_json::json!(null))
    };
    let mut out = Vec::new();
    let (message, is_draft) = get_commit_message_from_revision_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &draft,
        "7",
        "PHID-USER-me",
    )
    .unwrap();
    assert!(is_draft);
    assert_eq!(message.raw_corpus, "Template title\n");

    // Non-string templates and non-numeric ids fail exactly.
    let broken = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.getcommitmessage" {
            return Ok(serde_json::json!(null));
        }
        draft(method, &serde_json::json!(null))
    };
    let mut out = Vec::new();
    let err = get_commit_message_from_revision_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &broken,
        "7",
        "PHID-USER-me",
    )
    .unwrap_err();
    assert_eq!(err, FAILED_TO_RETRIEVE_TEMPLATE);
    // `D`-prefixed ids normalize, like `submit_mode` (`normalizeRevisionID`).
    let mut out = Vec::new();
    let (message, is_draft) = get_commit_message_from_revision_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &draft,
        "D7",
        "PHID-USER-me",
    )
    .unwrap();
    assert!(is_draft);
    assert_eq!(message.raw_corpus, "Template title\n");
    // Non-numeric ids fail exactly.
    let mut out = Vec::new();
    let err = get_commit_message_from_revision_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &draft,
        "nope",
        "PHID-USER-me",
    )
    .unwrap_err();
    assert_eq!(err, diff_submit::UPDATE_PARAM_MUST_BE_NUMBER);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gap2_resolve_message_source_dispatch() {
    assert_eq!(
        resolve_message_source(true, false, false, None, None, false, None).unwrap(),
        MessageSource::None
    );
    assert_eq!(
        resolve_message_source(false, false, false, None, None, true, None).unwrap(),
        MessageSource::None
    );
    assert_eq!(
        resolve_message_source(false, true, false, None, None, false, None).unwrap(),
        MessageSource::CreateFromUser
    );
    assert_eq!(
        resolve_message_source(false, false, false, None, None, false, Some(&[])).unwrap(),
        MessageSource::CreateFromUser
    );
    assert_eq!(
        resolve_message_source(
            false,
            false,
            false,
            None,
            None,
            false,
            Some(&[(7, "Mine".to_string())]),
        )
        .unwrap(),
        MessageSource::UpdateFromRevision(7)
    );
    let err = resolve_message_source(
        false,
        false,
        false,
        None,
        None,
        false,
        Some(&[(7, "A".to_string()), (8, "B".to_string())]),
    )
    .unwrap_err();
    assert!(err.contains("several revisions"), "{err}");
    assert!(err.contains("D7: A"), "{err}");
    assert_eq!(
        resolve_message_source(false, false, true, None, Some("m.txt"), false, None).unwrap(),
        MessageSource::CreateFromFile("m.txt".to_string())
    );
    assert_eq!(
        resolve_message_source(false, false, true, None, None, false, None).unwrap(),
        MessageSource::CreateFromUser
    );
    assert_eq!(
        resolve_message_source(false, false, false, Some("9"), None, false, None).unwrap(),
        MessageSource::UpdateFromRevision(9)
    );
    assert_eq!(
        resolve_message_source(false, false, false, Some("D9"), None, false, None).unwrap(),
        MessageSource::UpdateFromRevision(9)
    );
    let err =
        resolve_message_source(false, false, false, Some("nope"), None, false, None).unwrap_err();
    assert_eq!(err, diff_submit::UPDATE_PARAM_MUST_BE_NUMBER);
}

/* Gap2 D3: clean working-copy check. */

fn gap2_d3_vcs<'a>(edit: &'a dyn Fn(&str) -> Result<String, String>) -> CleanCheckVcs<'a> {
    CleanCheckVcs {
        kind: diff::VcsKind::Git,
        work_path: "/repo".to_string(),
        supports_local_commits: true,
        can_stash: true,
        autostash: false,
        require_clean_untracked: true,
        should_amend: false,
        head_commit_message: None,
        add_to_commit: None,
        stash_changes: None,
        do_commit: None,
        amend_commit: None,
        edit,
    }
}

#[test]
fn gap2_resolve_commit_mode_selection() {
    assert_eq!(resolve_commit_mode(true, false), CommitMode::Enable);
    assert_eq!(resolve_commit_mode(false, true), CommitMode::Disable);
    assert_eq!(resolve_commit_mode(false, false), CommitMode::Allow);
    assert_eq!(resolve_commit_mode(true, true), CommitMode::Enable);
}

#[test]
fn gap2_working_copy_changes_from_masks() {
    use std::collections::BTreeMap;
    let status: BTreeMap<String, u32> = [
        ("b.txt".to_string(), git::FLAG_UNCOMMITTED),
        (
            "a.txt".to_string(),
            git::FLAG_UNSTAGED | git::FLAG_UNCOMMITTED,
        ),
        ("u.txt".to_string(), git::FLAG_UNTRACKED),
        ("c.txt".to_string(), git::FLAG_CONFLICT),
    ]
    .into_iter()
    .collect();
    let changes = git_working_copy_changes(&status);
    assert!(changes.incomplete.is_empty() && changes.missing.is_empty());
    assert_eq!(changes.conflicts, vec!["c.txt".to_string()]);
    assert_eq!(
        changes.uncommitted,
        vec!["a.txt".to_string(), "b.txt".to_string()]
    );
    assert_eq!(changes.unstaged, vec!["a.txt".to_string()]);
    assert_eq!(changes.untracked, vec!["u.txt".to_string()]);

    let svn_status: BTreeMap<String, u32> = [
        ("i".to_string(), svn::FLAG_INCOMPLETE),
        ("m".to_string(), svn::FLAG_MISSING),
    ]
    .into_iter()
    .collect();
    let changes = svn_working_copy_changes(&svn_status);
    assert_eq!(changes.incomplete, vec!["i".to_string()]);
    assert_eq!(changes.missing, vec!["m".to_string()]);
}

#[test]
fn gap2_clean_check_fatal_gates() {
    use std::io::Cursor;
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let vcs = gap2_d3_vcs(&edit);
    let desc = working_copy_desc("/repo");

    let changes = WorkingCopyChanges {
        incomplete: vec!["sub".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert_eq!(err, incomplete_checkout_error(&desc, &["sub".to_string()]));
    assert!(err.contains("proceeding.'"), "{err}");
    assert!(err.contains("  Working copy: /repo\n\n"), "{err}");

    let changes = WorkingCopyChanges {
        conflicts: vec!["c.txt".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert!(err.contains("Conflicts in working copy:"), "{err}");

    let changes = WorkingCopyChanges {
        missing: vec!["m.txt".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert!(err.ends_with("    m.txt\n"), "{err}");
}

#[test]
fn gap2_clean_check_externals_gate() {
    use std::io::Cursor;
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let vcs = gap2_d3_vcs(&edit);
    let changes = WorkingCopyChanges {
        externals: vec!["ext".to_string()],
        ..Default::default()
    };

    let mut out = Vec::new();
    let outcome = require_clean_working_copy(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap();
    assert!(!outcome.stashed);
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("1 submodule(s) have uncommitted or untracked changes:\n\n      ext\n"),
        "{text}"
    );
    assert!(
        text.contains("Ignore the changes to these 1 submodule(s) and continue?"),
        "{text}"
    );

    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"n\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert_eq!(err, USER_ABORT);

    // Subversion zeroes the externals list: no prompt, no output.
    let mut svn_vcs = gap2_d3_vcs(&edit);
    svn_vcs.kind = diff::VcsKind::Svn;
    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &svn_vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap();
    assert!(out.is_empty());
}

#[test]
fn gap2_clean_check_untracked_gate() {
    use std::io::Cursor;
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let vcs = gap2_d3_vcs(&edit);
    let changes = WorkingCopyChanges {
        untracked: vec!["u.txt".to_string()],
        ..Default::default()
    };

    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(UNTRACKED_FILES_HERE), "{text}");
    assert!(text.contains(".git/info/exclude"), "{text}");
    assert!(
        text.contains("Ignore these 1 untracked file(s) and continue?"),
        "{text}"
    );

    assert!(untracked_hint(diff::VcsKind::Svn, 2).contains("svn:ignore"));
    assert!(untracked_hint(diff::VcsKind::Hg, 2).contains(".hgignore"));

    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"n\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert_eq!(err, USER_ABORT);

    // `--allow-untracked` skips the gate silently.
    let mut lax = gap2_d3_vcs(&edit);
    lax.require_clean_untracked = false;
    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &lax,
        &changes,
        CommitMode::Allow,
    )
    .unwrap();
    assert!(out.is_empty());
}

#[test]
fn gap2_clean_check_commit_decisions() {
    use std::io::Cursor;
    let edit = |_: &str| -> Result<String, String> { Ok("Msg\n".to_string()) };
    let added = std::cell::RefCell::new(Vec::<String>::new());
    let add = |paths: &[String]| -> Result<(), String> {
        added.borrow_mut().extend(paths.iter().cloned());
        Ok(())
    };
    let committed = std::cell::RefCell::new(Vec::<String>::new());
    let commit = |message: &str| -> Result<(), String> {
        committed.borrow_mut().push(message.to_string());
        Ok(())
    };
    let mut vcs = gap2_d3_vcs(&edit);
    vcs.add_to_commit = Some(&add);
    vcs.do_commit = Some(&commit);

    let changes = WorkingCopyChanges {
        unstaged: vec!["s.txt".to_string()],
        uncommitted: vec!["c.txt".to_string()],
        ..Default::default()
    };

    // Prompted commit: staged paths are added, the editor message commits.
    let mut out = Vec::new();
    let outcome = require_clean_working_copy(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap();
    assert!(!outcome.stashed);
    assert_eq!(added.borrow().as_slice(), ["s.txt".to_string()]);
    assert_eq!(committed.borrow().as_slice(), ["Msg\n".to_string()]);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(UNCOMMITTED_CHANGES_HERE), "{text}");
    // The unstaged header is indented, the uncommitted header is not.
    assert!(
        text.contains("  Unstaged changes in working copy:\n    s.txt"),
        "{text}"
    );
    assert!(
        text.contains("Uncommitted changes in working copy:\n    c.txt"),
        "{text}"
    );
    assert!(
        text.contains("Do you want to create a new commit with these 2 change(s)?"),
        "{text}"
    );

    // `--add-all` skips the prompt (empty input is never read).
    added.borrow_mut().clear();
    committed.borrow_mut().clear();
    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Enable,
    )
    .unwrap();
    assert_eq!(added.borrow().len(), 1);
    assert_eq!(committed.borrow().len(), 1);

    // `--uncommitted` refuses without autostash.
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Disable,
    )
    .unwrap_err();
    assert_eq!(err, CANNOT_CONTINUE_UNCOMMITTED);

    // Autostash stashes instead of failing.
    let stashed = std::cell::Cell::new(false);
    let stash = || -> Result<(), String> {
        stashed.set(true);
        Ok(())
    };
    vcs.autostash = true;
    vcs.stash_changes = Some(&stash);
    let mut out = Vec::new();
    let outcome = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Disable,
    )
    .unwrap();
    assert!(outcome.stashed);
    assert!(stashed.get());
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains(STASHING_UNCOMMITTED)
    );
}

#[test]
fn gap2_clean_check_commit_ux() {
    use std::io::Cursor;
    let edit = |template: &str| -> Result<String, String> {
        assert!(template.starts_with("\n\n# Enter a commit message.\n#\n# Changes:\n#\n"));
        assert!(template.contains("#     a.txt\n#     b.txt\n"));
        Ok("My commit\n".to_string())
    };
    let committed = std::cell::RefCell::new(Vec::<String>::new());
    let commit = |message: &str| -> Result<(), String> {
        committed.borrow_mut().push(message.to_string());
        Ok(())
    };
    let mut vcs = gap2_d3_vcs(&edit);
    vcs.do_commit = Some(&commit);
    let changes = WorkingCopyChanges {
        uncommitted: vec!["b.txt".to_string(), "a.txt".to_string()],
        ..Default::default()
    };

    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Enable,
    )
    .unwrap();
    assert_eq!(committed.borrow().as_slice(), ["My commit\n".to_string()]);

    // Unedited templates fail.
    let passthrough = |template: &str| -> Result<String, String> { Ok(template.to_string()) };
    let mut vcs = gap2_d3_vcs(&passthrough);
    vcs.do_commit = Some(&commit);
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Enable,
    )
    .unwrap_err();
    assert_eq!(err, MUST_PROVIDE_MESSAGE);

    // Amending passes the head message through.
    let amended = std::cell::RefCell::new(Vec::<String>::new());
    let amend = |message: &str| -> Result<(), String> {
        amended.borrow_mut().push(message.to_string());
        Ok(())
    };
    let mut vcs = gap2_d3_vcs(&edit);
    vcs.should_amend = true;
    vcs.head_commit_message = Some("Head msg\n".to_string());
    vcs.amend_commit = Some(&amend);
    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap();
    assert_eq!(amended.borrow().as_slice(), ["Head msg\n".to_string()]);
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("Do you want to amend these 2 change(s) to the current commit?"),
        "{text}"
    );
}

#[test]
fn gap2_format_count_grouping() {
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(5), "5");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1000), "1,000");
    assert_eq!(format_count(1_234_567), "1,234,567");
}

#[test]
fn gap2_calculate_should_amend_decisions() {
    let no_revisions = || -> Result<Vec<(i64, String)>, String> { Ok(Vec::new()) };
    let fail_call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("must not call".to_string())
    };
    let head = gap2_d1_local_commit("abc123", "Work", "Work\n");
    let base = || AmendCheck {
        history_immutable: false,
        supports_amend: true,
        head: Some(head.clone()),
        api_author: "a",
        create: false,
        repository: None,
        load_open_revisions: &no_revisions,
        call: &fail_call,
    };

    // No revision id: amend.
    assert!(calculate_should_amend(&base()).unwrap());
    // Immutable history, no amend support, no commits, foreign author.
    let mut check = base();
    check.history_immutable = true;
    assert!(!calculate_should_amend(&check).unwrap());
    let mut check = base();
    check.supports_amend = false;
    assert!(!calculate_should_amend(&check).unwrap());
    let mut check = base();
    check.head = None;
    assert!(!calculate_should_amend(&check).unwrap());
    let mut check = base();
    check.api_author = "someone-else";
    assert!(!calculate_should_amend(&check).unwrap());
    // git-svn messages never amend.
    let mut check = base();
    check.head.as_mut().unwrap().message = "Work\n\ngit-svn-id: https://x/y@9 uuid\n".to_string();
    assert!(!calculate_should_amend(&check).unwrap());
    // Revision ids only amend outside `--create` with an open revision.
    let mut check = base();
    check.head.as_mut().unwrap().message = "Work\n\nDifferential Revision: D1\n".to_string();
    check.create = true;
    assert!(!calculate_should_amend(&check).unwrap());
    check.create = false;
    assert!(!calculate_should_amend(&check).unwrap());
    let open = || -> Result<Vec<(i64, String)>, String> { Ok(vec![(1, "R".to_string())]) };
    check.load_open_revisions = &open;
    assert!(calculate_should_amend(&check).unwrap());

    // Published commits never amend.
    let known = |_: &str, _: &serde_json::Value| {
        Ok(serde_json::json!({"identifierMap": {"rXabc123": {"id": 1}}}))
    };
    let mut check = base();
    check.repository = Some(AmendRepository {
        id: 9,
        callsign: Some("X".to_string()),
    });
    check.call = &known;
    assert!(!calculate_should_amend(&check).unwrap());
    assert_eq!(
        amend_commit_name(check.repository.as_ref().unwrap(), "abc123"),
        "rXabc123"
    );
    let unknown = |_: &str, _: &serde_json::Value| Ok(serde_json::json!({"identifierMap": {}}));
    check.call = &unknown;
    assert!(calculate_should_amend(&check).unwrap());
    check.repository = Some(AmendRepository {
        id: 9,
        callsign: None,
    });
    assert_eq!(
        amend_commit_name(check.repository.as_ref().unwrap(), "abc123"),
        "R9:abc123"
    );
}

#[test]
fn ansi_lint_banners_gain_backgrounds() {
    let dir = std::env::temp_dir();
    let run = |outcome: ChildOutcome| move |_: &Path, _: &[String]| outcome.clone();
    for (code, plain_banner, colored_banner) in [
        (
            LINT_RESULT_OKAY,
            "** LINT OKAY ** No lint problems.\n",
            "\x1b[42m\x1b[1m LINT OKAY \x1b[m\x1b[49m No lint problems.\n",
        ),
        (
            LINT_RESULT_WARNINGS,
            "** LINT MESSAGES ** Lint issued unresolved warnings.\n",
            "\x1b[43m\x1b[1m LINT MESSAGES \x1b[m\x1b[49m Lint issued unresolved warnings.\n",
        ),
        (
            LINT_RESULT_ERRORS,
            "** LINT ERRORS ** Lint raised errors!\n",
            "\x1b[41m\x1b[1m LINT ERRORS \x1b[m\x1b[49m Lint raised errors!\n",
        ),
    ] {
        let mut out = Vec::new();
        run_lint_ansi(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(code, Vec::new())),
            &mut out,
            false,
        )
        .unwrap();
        assert_eq!(out, format!("Linting...\n{plain_banner}").into_bytes());
        let mut out = Vec::new();
        run_lint_ansi(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(code, Vec::new())),
            &mut out,
            true,
        )
        .unwrap();
        assert_eq!(out, format!("Linting...\n{colored_banner}").into_bytes());
    }
}

#[test]
fn ansi_unit_banners_gain_backgrounds() {
    let dir = std::env::temp_dir();
    let run = |outcome: ChildOutcome| move |_: &Path, _: &[String]| outcome.clone();
    for (code, plain_banner, colored_banner) in [
        (
            UNIT_RESULT_OKAY,
            "** UNIT OKAY ** No unit test failures.\n",
            "\x1b[42m\x1b[1m UNIT OKAY \x1b[m\x1b[49m No unit test failures.\n",
        ),
        (
            UNIT_RESULT_FAIL,
            "** UNIT ERRORS ** Unit testing raised errors!\n",
            "\x1b[41m\x1b[1m UNIT ERRORS \x1b[m\x1b[49m Unit testing raised errors!\n",
        ),
    ] {
        let mut input = std::io::BufReader::new(std::io::empty());
        let mut out = Vec::new();
        run_unit_ansi(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(code, Vec::new())),
            &mut input,
            &mut out,
            false,
        )
        .unwrap();
        assert_eq!(
            out,
            format!("Running unit tests...\n{plain_banner}").into_bytes()
        );
        let mut input = std::io::BufReader::new(std::io::empty());
        let mut out = Vec::new();
        run_unit_ansi(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(code, Vec::new())),
            &mut input,
            &mut out,
            true,
        )
        .unwrap();
        assert_eq!(
            out,
            format!("Running unit tests...\n{colored_banner}").into_bytes()
        );
    }
    // Unsound prompts (default yes), then prints the yellow banner.
    use std::io::Cursor;
    for (ansi, banner) in [
        (
            false,
            "** UNIT UNSOUND ** Unit testing raised errors, but all failing tests are unsound.\n",
        ),
        (
            true,
            "\x1b[43m\x1b[1m UNIT UNSOUND \x1b[m\x1b[49m Unit testing raised errors, but all failing tests are unsound.\n",
        ),
    ] {
        let mut input = Cursor::new(b"y\n".to_vec());
        let mut out = Vec::new();
        run_unit_ansi(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(UNIT_RESULT_UNSOUND, Vec::new())),
            &mut input,
            &mut out,
            ansi,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(banner), "{text:?}");
        assert_eq!(text.contains('\x1b'), ansi, "{text:?}");
    }
}

#[test]
fn ansi_working_copy_desc_underlines_path() {
    assert_eq!(
        working_copy_desc_ansi("/repo", false),
        "  Working copy: /repo\n\n"
    );
    assert_eq!(
        working_copy_desc_ansi("/repo", true),
        "  Working copy: \x1b[4m/repo\x1b[m\n\n"
    );
}

#[test]
fn away_check_reads_user_query_lists() {
    use std::io::Cursor;
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "user.query");
        Ok(serde_json::json!([
            {"userName": "bob", "realName": "Bob",
             "currentStatus": "away", "currentStatusUntil": 1_700_000_000},
        ]))
    };
    let mut message = CommitMessage::new_from_raw_corpus("Title\n").unwrap();
    message.set_field_value(
        "reviewerPHIDs",
        FieldValue::List(vec!["PHID-USER-9".to_string()]),
    );
    let mut out = Vec::new();
    validate_commit_message(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &call,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("bob"), "unexpected: {text}");
}

#[test]
fn from_revision_corpus_fetch_sends_edit_false() {
    use std::io::Cursor;
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        if method == "differential.getcommitmessage" {
            probe.borrow_mut().push(params.clone());
            return Ok(serde_json::json!("Template title\n"));
        }
        if method == "differential.query" {
            return Ok(serde_json::json!([
                {"id": 7, "title": "Mine",
                 "authorPHID": "PHID-USER-me", "statusName": "Draft"},
            ]));
        }
        Ok(serde_json::json!({"fields": {}}))
    };
    let mut out = Vec::new();
    get_commit_message_from_revision_with(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &call,
        "7",
        "PHID-USER-me",
    )
    .unwrap();
    assert_eq!(
        *seen.borrow(),
        vec![serde_json::json!({"revision_id": 7, "edit": false})]
    );
}

// -( R5 TTY gating: DIFF-D16 )------------------------------------------------

#[test]
fn r5_diff_d16_unsound_shows_y_n_default() {
    // `ArcanistDiffWorkflow.php:1241-1244` calls
    // `phutil_console_confirm()` with the default `$default_no`, so the
    // unsound prompt renders `[y/N]` and Enter declines.
    use std::io::Cursor;
    let dir = std::env::temp_dir();
    let run = |_: &Path, _: &[String]| ChildOutcome::Code(UNIT_RESULT_UNSOUND, Vec::new());
    let mut out = Vec::new();
    let mut input = Cursor::new(b"y\n".to_vec());
    assert_eq!(
        run_unit(&dir, None, &[], false, &run, &mut input, &mut out).unwrap(),
        (Some(UNIT_RESULT_UNSOUND), Vec::new())
    );
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("[y/N]"), "{text}");
    assert!(!text.contains("[Y/n]"), "{text}");
}

/* T1153 DIFF-D findings: numeric grammar, hg bookmarks, hg prefills. */

#[test]
fn d21_is_php_numeric_matches_ceb() {
    for numeric in [
        "123", "+123", "-123", "12.3", ".5", "5.", "1e3", "1E3", "2e-2", "+.5e+2",
    ] {
        assert!(is_php_numeric(numeric), "{numeric}");
    }
    for other in [
        "", ".", "e3", "1e", "0x10", "12x", "1.2.3", " 12", "D12", "NaN", "inf",
    ] {
        assert!(!is_php_numeric(other), "{other}");
    }
}

#[test]
fn d19_parses_hg_active_bookmark() {
    let output = "  old                       1:aaa\n* active                    3:bbb\n  other                     2:ccc\n";
    assert_eq!(parse_hg_active_bookmark(output).as_deref(), Some("active"));
    assert_eq!(parse_hg_active_bookmark("  none  1:aaa\n"), None);
    assert_eq!(parse_hg_active_bookmark(""), None);
    assert_eq!(parse_hg_active_bookmark("*\n"), None);
}

#[test]
fn d9_selects_usable_hg_messages() {
    let pairs = vec![
        ("n3".to_string(), "third\n".to_string()),
        ("n2".to_string(), "second\n".to_string()),
        ("n1".to_string(), "first\n".to_string()),
    ];
    let empty: HashSet<String> = HashSet::new();
    assert_eq!(
        select_usable_hg_messages(&pairs, &empty),
        vec![
            "third\n".to_string(),
            "second\n".to_string(),
            "first\n".to_string()
        ]
    );
    // One message means an amend workflow: nothing usable.
    assert!(select_usable_hg_messages(&pairs[..1], &empty).is_empty());
    // Attached commits stop the scan.
    let attached: HashSet<String> = ["n2".to_string()].into_iter().collect();
    assert_eq!(
        select_usable_hg_messages(&pairs, &attached),
        vec!["third\n".to_string()]
    );
}

#[test]
fn d8_partial_fields_merge_on_parser_errors() {
    // DIFF-D8 (`:1876`): conduit parser errors add note lines but the
    // partial fields still merge into the defaults.
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({
            "fields": {"title": "Part", "summary": "kept"},
            "errors": ["unknown reviewer robert"],
        }))
    };
    let local = vec![LocalCommitInfo {
        commit: "abcdef1234567890".to_string(),
        tree: String::new(),
        parents: Vec::new(),
        time: String::new(),
        author: "t".to_string(),
        summary: "Part".to_string(),
        message: "Part\n".to_string(),
        author_email: String::new(),
    }];
    let (fields, included, notes) =
        get_default_create_fields_with(&call, &local, true, None, None, false).unwrap();
    assert_eq!(
        fields.get("title"),
        Some(&FieldValue::Text("Part".to_string()))
    );
    assert_eq!(
        fields.get("summary"),
        Some(&FieldValue::Text("kept".to_string()))
    );
    assert_eq!(included, vec!["abcdef123456 Part".to_string()]);
    assert_eq!(
        notes,
        vec![
            "NOTE: commit abcdef123456 could not be completely parsed:".to_string(),
            "  - unknown reviewer robert".to_string(),
        ]
    );
}

#[test]
fn d8_saved_template_zero_scratch_is_falsy() {
    // DIFF-D8: PHP tests `if ($saved)`, so a `"0"` scratch file counts
    // as no saved message (no notice, no prompt).
    use std::io::Cursor;
    let root = gap2_d1_scratch("d8-zero");
    let meta = root.join(".git");
    diff::write_scratch_file(&meta, SCRATCH_CREATE_MESSAGE, "0");
    let mut out = Vec::new();
    let reused = resolve_saved_template(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &meta,
        &root,
        false,
    )
    .unwrap();
    assert_eq!(reused, None);
    assert!(out.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn d8_saved_notice_zero_preview_omits_block() {
    // DIFF-D8: PHP tests `if ($preview)`, so a `"0"` first line omits
    // the `Message begins:` block like an empty preview.
    let with_zero = saved_message_notice("/tmp/x", "0");
    assert!(!with_zero.contains("Message begins:"), "{with_zero}");
    assert!(with_zero.contains("You can use this message"));
    let with_text = saved_message_notice("/tmp/x", "Hello");
    assert!(with_text.contains("Message begins:"), "{with_text}");
}

/* T1170 coverage: close the diff_local/hg miss manifests with behavior tests. */

#[test]
fn t1170_git_svn_id_requires_space_after_revision() {
    // `(\d+)\s+(.*)`: digits glued to the UUID never match.
    let message =
        CommitMessage::new_from_raw_corpus("Title\ngit-svn-id: https://x@42UUID\n").unwrap();
    assert_eq!(message.git_svn_base_revision, None);
    assert_eq!(message.git_svn_uuid, None);
    let message =
        CommitMessage::new_from_raw_corpus("Title\ngit-svn-id: https://x@42 uuid\n").unwrap();
    assert_eq!(
        message.git_svn_base_revision.as_deref(),
        Some("https://x@42")
    );
}

#[test]
fn t1170_tag_lines_with_padded_or_punctuated_names_stay_body() {
    // Padded names and names with punctuation are not tags.
    let fields = parse_field_tags("Title\n Summary : x\n");
    assert_eq!(
        fields.get(FIELD_SUMMARY),
        Some(&FieldValue::Text("Summary : x".to_string()))
    );
    let fields = parse_field_tags("Title\nSum-mary: x\n");
    assert_eq!(
        fields.get(FIELD_SUMMARY),
        Some(&FieldValue::Text("Sum-mary: x".to_string()))
    );
}

/// True when the `git` binary runs (real-repo tests probe this first).
fn t1170_git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Scratch git repo with one empty commit; removed on drop. `None` when
/// `git` is unavailable (the caller skips).
struct T1170GitRepo {
    dir: std::path::PathBuf,
}

impl T1170GitRepo {
    fn named(tag: &str) -> Option<Self> {
        if !t1170_git_available() {
            return None;
        }
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-t1170-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        run(&["init"]);
        run(&[
            "-c",
            "user.email=t1170@example.test",
            "-c",
            "user.name=t1170",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ]);
        Some(Self { dir })
    }

    fn run(&self, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    }

    fn commit_file(&self, name: &str, body: &str, message: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
        self.run(&["add", "--", name]);
        self.run(&[
            "-c",
            "user.email=t1170@example.test",
            "-c",
            "user.name=t1170",
            "commit",
            "-m",
            message,
        ]);
    }
}

impl Drop for T1170GitRepo {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn t1170_git_log_failures_report_stderr() {
    let Some(repo) = T1170GitRepo::named("logfail") else {
        return;
    };
    let err = get_local_commit_information(&repo.dir, "missing-base", "HEAD").unwrap_err();
    assert!(err.contains("git log failed"), "{err}");
    let err = get_git_update_messages(&repo.dir, "missing-base", "HEAD").unwrap_err();
    assert!(err.contains("git log failed"), "{err}");
    let err = get_git_parent_history(&repo.dir, "missing-base").unwrap_err();
    assert!(err.contains("git log failed"), "{err}");
    // The magic-root arm lists HEAD instead of a range.
    let messages = get_git_update_messages(&repo.dir, GIT_MAGIC_ROOT_COMMIT, "HEAD").unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].message.contains("init"));
}

#[test]
fn t1170_symbolic_ref_unexpected_exit_fails() {
    let Some(repo) = T1170GitRepo::named("symref") else {
        return;
    };
    // Outside any repo `git symbolic-ref` exits 128: neither 0 nor 1.
    let plain =
        std::env::temp_dir().join(format!("arc-rs-diff-t1170-plain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&plain);
    std::fs::create_dir_all(&plain).unwrap();
    let err = get_branch_name(&plain).unwrap_err();
    assert!(err.contains("Command git symbolic-ref failed"), "{err}");
    let _ = std::fs::remove_dir_all(&plain);
    // Sanity: the scratch repo itself resolves its branch.
    assert!(get_branch_name(&repo.dir).unwrap().is_some());
}

#[test]
fn t1170_collect_change_entries_classifies_worktree_shapes() {
    let Some(repo) = T1170GitRepo::named("classify") else {
        return;
    };
    repo.commit_file("a.txt", "one\n", "add a");
    let base = git::stdout_text(&repo.dir, &["rev-parse", "HEAD~1"]).unwrap();
    // A path the range adds but the worktree lost reads as plain text.
    std::fs::remove_file(repo.dir.join("a.txt")).unwrap();
    let entries = collect_change_entries(&repo.dir, &base, Some("HEAD")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].change_type, TYPE_ADD);
    assert_eq!(entries[0].file_type, FILE_TEXT);
    // A worktree directory in place of the file is not a symlink either.
    std::fs::create_dir_all(repo.dir.join("a.txt")).unwrap();
    let entries = collect_change_entries(&repo.dir, &base, Some("HEAD")).unwrap();
    assert_eq!(entries[0].file_type, FILE_TEXT);
    // Bad ranges report the diff stderr.
    let err = collect_change_entries(&repo.dir, "missing-base", None).unwrap_err();
    assert!(err.contains("git diff failed"), "{err}");
}

#[cfg(unix)]
#[test]
fn t1170_collect_change_entries_spots_symlinks() {
    let Some(repo) = T1170GitRepo::named("symlink") else {
        return;
    };
    repo.commit_file("a.txt", "one\n", "add a");
    std::fs::remove_file(repo.dir.join("a.txt")).unwrap();
    std::os::unix::fs::symlink("target", repo.dir.join("a.txt")).unwrap();
    let entries = collect_change_entries(&repo.dir, "HEAD", None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].change_type, TYPE_CHANGE);
    assert_eq!(entries[0].file_type, FILE_SYMLINK);
}

#[test]
fn t1170_copy_status_maps_to_copy_here() {
    let Some(repo) = T1170GitRepo::named("copydetect") else {
        return;
    };
    let body: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    repo.commit_file("a.txt", &body, "add a");
    // `-C` reports a copy only from a modified source to a
    // similar-but-different new file.
    let mut changed = body.clone();
    changed.push_str("extra\n");
    std::fs::write(repo.dir.join("a.txt"), &changed).unwrap();
    let copied = changed.replacen("line 1\n", "CHANGED\n", 1);
    std::fs::write(repo.dir.join("b.txt"), &copied).unwrap();
    repo.run(&["add", "--", "a.txt", "b.txt"]);
    repo.run(&[
        "-c",
        "user.email=t1170@example.test",
        "-c",
        "user.name=t1170",
        "commit",
        "-m",
        "modify a, copy to b",
    ]);
    let entries = collect_change_entries(&repo.dir, "HEAD~1", Some("HEAD")).unwrap();
    let copied = entries
        .iter()
        .find(|entry| entry.current.path == "b.txt")
        .unwrap();
    assert_eq!(copied.change_type, TYPE_COPY_HERE);
    assert_eq!(copied.old_path.as_deref(), Some("a.txt"));
}

#[test]
fn t1170_working_copy_revisions_with_empty_log_returns_none() {
    let Some(repo) = T1170GitRepo::named("emptylog") else {
        return;
    };
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        panic!("conduit must not run for an empty log");
    };
    let revisions =
        load_working_copy_revisions_git(&call, &repo.dir, "HEAD", "HEAD", "PHID-U").unwrap();
    assert!(revisions.is_empty());
}

#[test]
fn t1170_raw_diff_bytes_failure_reports_stderr() {
    let Some(repo) = T1170GitRepo::named("rawfail") else {
        return;
    };
    let err = get_raw_diff_bytes(&repo.dir, "missing-base", "f.txt").unwrap_err();
    assert!(err.contains("git diff failed"), "{err}");
}

/// Minimal canned `hg` backend (unix): `log` with `{rev}` serves the
/// local-commit response, `parents` the parents response, anything else
/// the message-log response. Removed on drop.
#[cfg(unix)]
struct T1170FakeHg {
    dir: std::path::PathBuf,
    hg: hg::HgCommand,
}

#[cfg(unix)]
impl T1170FakeHg {
    fn named(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-t1170-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("fakehg.sh");
        std::fs::write(
            &sh,
            "#!/bin/sh\ncase \"$*\" in\n*\"{rev}\"*) cat .resp-local 2>/dev/null;;\n*parents*) cat .resp-parents 2>/dev/null;;\n*) cat .resp-log 2>/dev/null;;\nesac\nexit 0\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&sh).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&sh, perms).unwrap();
        }
        let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
        Self { dir, hg }
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }
}

#[cfg(unix)]
impl Drop for T1170FakeHg {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(unix)]
#[test]
fn t1170_hg_working_copy_revisions_by_footer_id() {
    // Footer IDs query `differential.query` by id and skip the hash path.
    let fake = T1170FakeHg::named("hgids");
    fake.write(
        ".resp-log",
        "n1\x01fix it\n\nDifferential Revision: D7\n\x02".as_bytes(),
    );
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.query");
        assert_eq!(params["ids"], serde_json::json!([7]));
        Ok(serde_json::json!([{"id": 7, "title": "Seven"}]))
    };
    let revisions =
        load_working_copy_revisions_hg(&call, &fake.dir, &fake.hg, "base", "default", "PHID-U")
            .unwrap();
    assert_eq!(
        revisions,
        vec![WorkingCopyRevision {
            id: 7,
            title: "Seven".to_string(),
        }]
    );
}

#[cfg(unix)]
#[test]
fn t1170_hg_working_copy_revisions_by_commit_hash() {
    // Without footer IDs the local commits feed a commit-hash lookup;
    // chained parents skip the lookup, unchained ones run it.
    let fake = T1170FakeHg::named("hghash");
    fake.write(".resp-log", "n1\x01no trailer here\x02".as_bytes());
    fake.write(
        ".resp-local",
        "n1\x01r1\x01a\x010 0\x01b\x01\x01\x01first\x02n2\x01r2\x01a\x010 0\x01b\x01\x01\x01second\x02n3\x01r3\x01a\x010 0\x01b\x01\x01p9\x01third\x02"
            .as_bytes(),
    );
    fake.write(".resp-parents", b"p9\n");
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.query");
        assert_eq!(
            params["commitHashes"],
            serde_json::json!([["hgcm", "n3"], ["hgcm", "n2"], ["hgcm", "n1"]])
        );
        Ok(serde_json::json!([{"id": 9, "title": "Nine"}]))
    };
    let revisions =
        load_working_copy_revisions_hg(&call, &fake.dir, &fake.hg, "base", "default", "PHID-U")
            .unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].id, 9);
}

#[cfg(unix)]
#[test]
fn t1170_hg_working_copy_revisions_empty_without_commits() {
    // No footer IDs and no local commits: empty without querying.
    let fake = T1170FakeHg::named("hgempty");
    fake.write(".resp-log", "n1\x01no trailer here\x02".as_bytes());
    fake.write(".resp-local", b"");
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        panic!("conduit must not run without commits");
    };
    let revisions =
        load_working_copy_revisions_hg(&call, &fake.dir, &fake.hg, "base", "default", "PHID-U")
            .unwrap();
    assert!(revisions.is_empty());
}

#[test]
fn t1170_svn_working_copy_revisions_match_checkout_path() {
    // Only rows at this checkout path with usable ids survive.
    let dir = std::env::temp_dir().join("arc-rs-diff-t1170-svnrev");
    let here = dir.to_string_lossy().to_string();
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.query");
        assert_eq!(params["status"], serde_json::json!("status-open"));
        Ok(serde_json::json!([
            {"id": 3, "title": "Here", "sourcePath": here},
            {"id": "bogus", "title": "Bad", "sourcePath": here},
            {"id": 4, "title": "Elsewhere", "sourcePath": "/other/checkout"},
        ]))
    };
    let revisions = load_working_copy_revisions_svn(&call, &dir, "PHID-U").unwrap();
    assert_eq!(
        revisions,
        vec![WorkingCopyRevision {
            id: 3,
            title: "Here".to_string(),
        }]
    );
}

#[test]
fn t1170_lint_in_process_rejects_unknown_engine_class() {
    // A configured-but-unknown engine class aborts (not NoEngine).
    let dir =
        std::env::temp_dir().join(format!("arc-rs-diff-t1170-lintbad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcconfig"), "{\"lint.engine\": \"BogusEngine\"}").unwrap();
    let outcome = run_lint_in_process(&dir, &[]);
    assert!(
        matches!(&outcome, ChildOutcome::Failed(message) if message.contains("not a subclass")),
        "{outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t1170_lint_in_process_usage_error_fails() {
    // `--help` in child argv is a usage failure after engine discovery.
    let dir =
        std::env::temp_dir().join(format!("arc-rs-diff-t1170-linthelp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        "{\"lint.engine\": \"ArcanistComprehensiveLintEngine\"}",
    )
    .unwrap();
    let outcome = run_lint_in_process(&dir, &["--help".to_string()]);
    assert!(
        matches!(&outcome, ChildOutcome::Failed(message) if message.contains("--help")),
        "{outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t1170_lint_in_process_empty_engine_has_no_effect() {
    // An `.arclint` with no linters runs nothing: NoEffect.
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-diff-t1170-lintempty-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arclint"), "{\"linters\": {}}").unwrap();
    assert_eq!(
        run_lint_in_process(&dir, &[]),
        ChildOutcome::NoEffect("No linters to run.".to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t1170_lint_in_process_comprehensive_run_collects() {
    let Some(repo) = T1170GitRepo::named("lintrun") else {
        return;
    };
    std::fs::write(
        repo.dir.join(".arcconfig"),
        "{\"lint.engine\": \"ArcanistComprehensiveLintEngine\"}",
    )
    .unwrap();
    std::fs::write(repo.dir.join("note.txt"), "hello\n").unwrap();
    // Explicit paths skip worktree status; the single commit keeps base
    // resolution prompt-free.
    let outcome = run_lint_in_process(&repo.dir, &["note.txt".to_string()]);
    assert!(
        matches!(outcome, ChildOutcome::Code(_, _)),
        "comprehensive run collects a report: {outcome:?}"
    );
}

#[test]
fn t1170_unit_in_process_usage_error_fails() {
    // `.arcunit` passes the engine gate; `--help` then fails usage.
    let dir =
        std::env::temp_dir().join(format!("arc-rs-diff-t1170-unithelp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcunit"), "{\"engines\": []}").unwrap();
    let outcome = run_unit_in_process(&dir, &["--help".to_string()]);
    assert!(
        matches!(&outcome, ChildOutcome::Failed(message) if message.contains("--help")),
        "{outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writer that always fails, for prompt/output error paths.
struct T1170FailWriter;

impl std::io::Write for T1170FailWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("t1170 no writes"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("t1170 no writes"))
    }
}

#[test]
fn t1170_run_unit_maps_no_effect_failed_and_skip_codes() {
    use std::io::Cursor;
    let dir = std::env::temp_dir();
    let run = |outcome: ChildOutcome| move |_: &Path, _: &[String]| outcome.clone();
    // NoEffect prints the message and yields no result.
    let mut out = Vec::new();
    assert_eq!(
        run_unit(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::NoEffect("nothing to run".to_string())),
            &mut Cursor::new(b"".to_vec()),
            &mut out,
        )
        .unwrap(),
        (None, Vec::new())
    );
    assert_eq!(out, b"Running unit tests...\nnothing to run\n");
    // Child failures abort after the start notice.
    let mut out = Vec::new();
    assert_eq!(
        run_unit(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Failed("boom".to_string())),
            &mut Cursor::new(b"".to_vec()),
            &mut out,
        )
        .unwrap_err(),
        "boom"
    );
    assert_eq!(out, b"Running unit tests...\n");
    // A SKIP code from the child prints no banner.
    let mut out = Vec::new();
    assert_eq!(
        run_unit(
            &dir,
            None,
            &[],
            false,
            &run(ChildOutcome::Code(3, Vec::new())),
            &mut Cursor::new(b"".to_vec()),
            &mut out,
        )
        .unwrap(),
        (Some(3), Vec::new())
    );
    assert_eq!(out, b"Running unit tests...\n");
    // Output failures name the failing write.
    let err = run_unit(
        &dir,
        None,
        &[],
        false,
        &run(ChildOutcome::Code(0, Vec::new())),
        &mut Cursor::new(b"".to_vec()),
        &mut T1170FailWriter,
    )
    .unwrap_err();
    assert!(err.contains("failed to write output"), "{err}");
}

#[test]
fn t1170_run_lint_unit_reports_unit_failure() {
    use std::io::Cursor;
    let dir = std::env::temp_dir();
    let ok = |_: &Path, _: &[String]| ChildOutcome::Code(0, Vec::new());
    let fail = |_: &Path, _: &[String]| ChildOutcome::Failed("unit boom".to_string());
    let mut out = Vec::new();
    assert_eq!(
        run_lint_unit(
            &dir,
            None,
            &[],
            &[],
            false,
            false,
            &ok,
            &fail,
            &mut Cursor::new(b"".to_vec()),
            &mut out,
        )
        .unwrap_err(),
        "unit boom"
    );
}

#[test]
fn t1170_trailing_commit_message_survives_at_end() {
    // A `commit` block with no following diff still emits its message.
    let changes = parse_unified_diff("commit abc123\nsubject here\n").unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_MESSAGE);
    assert_eq!(changes[0].commit_hash.as_deref(), Some("abc123"));
    assert!(
        changes[0]
            .message
            .as_deref()
            .unwrap()
            .contains("subject here")
    );
}

#[test]
fn t1170_combined_diff_recovery_skips_blank_and_noise_lines() {
    // Leading blanks/comments do not count as content, and near-miss
    // `diff` lines do not count as starts: recovery still finds the
    // real block.
    let text = "\n# note\ndiff --cc f\ndiffx -r f\ndiff -x f\ndiff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "g");
}

#[test]
fn t1170_hunk_stats_end_conditions() {
    // A `-` past exhausted old lines ends the hunk.
    assert_eq!(hunk_stats("-gone\n", 0, 1), (0, 0, false, false));
    // A context line past exhausted counts ends the hunk.
    assert_eq!(hunk_stats(" ctx\n", 0, 0), (0, 0, false, false));
    // Anything else ends the hunk too.
    assert_eq!(hunk_stats("!x\n", 1, 1), (0, 0, false, false));
    // Sanity: ordinary lines still count.
    assert_eq!(hunk_stats("+a\n-b\n c\n", 2, 2), (1, 1, false, false));
}

/// Bare parsed change for predicate tests.
fn t1170_parsed_change(change_type: u32, hunks: Vec<ParsedHunk>) -> ParsedChange {
    ParsedChange {
        old_path: String::new(),
        current_path: "f".to_string(),
        change_type,
        file_type: FILE_TEXT,
        hunks,
        hunk_len_explicit: Vec::new(),
        old_mode: None,
        new_mode: None,
        away_paths: Vec::new(),
        commit_hash: None,
        message: None,
    }
}

#[test]
fn t1170_needs_synthetic_matches_hunkless_moves_and_copies() {
    for change_type in [
        TYPE_MOVE_HERE,
        TYPE_COPY_HERE,
        TYPE_MOVE_AWAY,
        TYPE_MULTICOPY,
    ] {
        assert!(
            needs_synthetic_git_hunks(&t1170_parsed_change(change_type, Vec::new())),
            "{change_type}"
        );
    }
    assert!(!needs_synthetic_git_hunks(&t1170_parsed_change(
        TYPE_CHANGE,
        Vec::new()
    )));
    // Hunks of any kind disqualify, even on a move.
    let hunk = ParsedHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        corpus: " x\n".to_string(),
        synthetic_counts: None,
    };
    assert!(!needs_synthetic_git_hunks(&t1170_parsed_change(
        TYPE_MOVE_HERE,
        vec![hunk]
    )));
}

#[test]
fn t1170_synthetic_transport_matches_plain_for_ascii() {
    // Marker-free ASCII is transport-identical, so both entry points agree.
    let raw = "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n";
    let plain = synthetic_hunks_for_path(raw, "n.txt").unwrap().unwrap();
    let transported = synthetic_hunks_for_path_transport(raw, "n.txt")
        .unwrap()
        .unwrap();
    assert_eq!(transported, plain);
    assert_eq!(transported.hunks[0].corpus, " one\n two\n");
    assert!(
        synthetic_hunks_for_path_transport(raw, "elsewhere")
            .unwrap()
            .is_none()
    );

    let text =
        "diff --git a/o.txt b/n.txt\nsimilarity index 100%\nrename from o.txt\nrename to n.txt\n";
    let mut full = with_companions(&parse_unified_diff(text).unwrap());
    let raw_for = |path: &str| -> Result<String, String> {
        if path == "n.txt" {
            Ok(raw.to_string())
        } else {
            Ok("diff --git a/o.txt b/o.txt\ndeleted file mode 100644\n--- a/o.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n".to_string())
        }
    };
    attach_synthetic_git_hunks_transport(&mut full, &raw_for).unwrap();
    assert_eq!(full[0].hunks[0].corpus, " one\n two\n");
    assert_eq!(full[1].hunks[0].corpus, "-one\n-two\n");
}

#[test]
fn t1170_copy_onto_move_folds_multicopy() {
    // A copy folding onto a move-away companion escalates to multicopy.
    let mut fold = CompanionFold::new();
    let (slot, is_new) = fold.fold(TYPE_MOVE_HERE, "o.txt", "a.txt");
    assert!(is_new);
    assert_eq!(fold.companion(slot).change_type, TYPE_MOVE_AWAY);
    let (slot, is_new) = fold.fold(TYPE_COPY_HERE, "o.txt", "b.txt");
    assert!(!is_new);
    let companion = fold.companion(slot);
    assert_eq!(companion.change_type, TYPE_MULTICOPY);
    assert_eq!(
        companion.away_paths,
        vec!["a.txt".to_string(), "b.txt".to_string()]
    );
}

#[test]
fn t1170_blank_commit_message_dropped_before_next_block() {
    // A blank `commit` body between file blocks flushes to nothing.
    let first = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    let second = "diff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -1 +1 @@\n-a\n+b\n";
    let text = format!("{first}commit abc\n\n{second}");
    let changes = parse_unified_diff(&text).unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|change| !change.is_message()));
}

#[test]
fn t1170_digitless_mode_lines_match_nothing() {
    // `new file mode` without digits is ignored like PHP's `\d+`.
    let changes = parse_unified_diff(
        "diff --git a/f b/f\nnew file mode abc\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    assert_eq!(changes[0].change_type, TYPE_CHANGE);
    assert_eq!(changes[0].new_mode, None);
}

#[test]
fn t1170_svn_binary_block_skips_blank_lines() {
    // Blank lines between the marker and the mime line are consumed.
    let changes = parse_unified_diff(
        "Index: f.bin\n--- f.bin (revision 1)\n+++ f.bin (working copy)\nCannot display: file marked as a binary type.\n\n\nsvn:mime-type: application/octet-stream\n",
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());
}

#[test]
fn t1170_duplicate_blocks_run_both_mode_arms() {
    // An incoming block with both modes reads as delete-then-new.
    let delete =
        "diff --git a/f b/f\ndeleted file mode 100644\n--- a/f\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\n";
    let both = "diff --git a/f b/f\ndeleted file mode 100644\nnew file mode 100755\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(&format!("{delete}{both}")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_CHANGE);
    // An incoming `new file mode` over a plain change lands an add.
    let change = "diff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -1 +1 @@\n-a\n+b\n";
    let added =
        "diff --git a/g b/g\nnew file mode 100644\n--- /dev/null\n+++ b/g\n@@ -0,0 +1 @@\n+b\n";
    let changes = parse_unified_diff(&format!("{change}{added}")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_ADD);
}

#[test]
fn t1170_duplicate_keyed_rename_retypes_and_repaths() {
    // A keyed block carrying rename lines retypes the merged change.
    let first = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    let renamed =
        "diff --git a/f b/f\nrename from x\nrename to y\n--- a/x\n+++ b/y\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_unified_diff(&format!("{first}{renamed}")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_MOVE_HERE);
    assert_eq!(changes[0].current_path, "y");
}

#[test]
fn t1170_duplicate_binary_block_sticks() {
    // A later binary block flips the merged change binary (sticky).
    let first = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    let binary = "diff --git a/f b/f\nBinary files a/f and b/f differ\n";
    let changes = parse_unified_diff(&format!("{first}{binary}")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].file_type, FILE_BINARY);
    assert!(changes[0].hunks.is_empty());
}

#[test]
fn t1170_c_quoted_hex_and_control_escapes_decode() {
    // `\x41` decodes hex (one or two digits); a bare `\x` keeps the `x`.
    assert_eq!(stripcslashes("f\\x41", false), "fA");
    assert_eq!(stripcslashes("f\\x4", false), "f\x04");
    assert_eq!(stripcslashes("f\\xg", false), "fxg");
    assert_eq!(stripcslashes("f\\x41", true), "fA");
    // Single-letter controls decode to their bytes.
    assert_eq!(stripcslashes("x\\ry", false), "x\ry");
    assert_eq!(stripcslashes("x\\vy", false), "x\x0By");
    assert_eq!(stripcslashes("x\\fy", false), "x\x0Cy");
    assert_eq!(stripcslashes("x\\ay", false), "x\x07y");
    assert_eq!(stripcslashes("x\\by", false), "x\x08y");
    // Quoting escapes keep the escaped char.
    assert_eq!(stripcslashes("x\\\\y", false), "x\\y");
    assert_eq!(stripcslashes("x\\\"y", false), "x\"y");
    assert_eq!(stripcslashes("x\\'y", false), "x'y");
    assert_eq!(stripcslashes("x\\?y", false), "x?y");
    // End to end: the common filename decodes through the header.
    assert_eq!(
        extract_git_common_filename("\"a/f\\x41\" \"b/f\\x41\"", false).as_deref(),
        Some("fA")
    );
}

#[test]
fn t1170_unterminated_quote_falls_back_to_hunk_paths() {
    // An unterminated quoted header recovers paths from `---`/`+++`.
    let changes =
        parse_unified_diff("diff --git \"a/f b/g\n--- a/f\n+++ b/g\n@@ -1 +1 @@\n-a\n+b\n")
            .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].old_path, "f");
    assert_eq!(changes[0].current_path, "g");
}

#[test]
fn t1170_paren_suffix_without_open_paren_is_kept() {
    // An `Index:` path ending in `)` without `(` stays literal.
    let changes = parse_unified_diff("Index: f\n--- f)\n+++ f\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
    assert_eq!(changes[0].old_path, "f)");
}

#[test]
fn t1170_hg_revision_suffix_strips_in_plain_block() {
    // Non-`Index:` blocks strip ` (revision N)` and ` (working copy)`.
    let changes =
        parse_unified_diff("diff -r abc f\n--- f (revision 3)\n+++ f\n@@ -1 +1 @@\n-a\n+b\n")
            .unwrap();
    assert_eq!(changes[0].old_path, "f");
    let changes =
        parse_unified_diff("diff -r abc f\n--- f\n+++ f (working copy)\n@@ -1 +1 @@\n-a\n+b\n")
            .unwrap();
    assert_eq!(changes[0].current_path, "f");
}

#[test]
fn t1170_extra_hunk_range_comma_fails() {
    // A third range element rejects the header like PHP's anchored regex.
    let err = parse_unified_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2,3 +1 @@\n-a\n+b\n")
        .unwrap_err();
    assert!(err.contains("Expected hunk header"), "{err}");
}

#[test]
fn t1170_foreign_hunk_line_ends_with_short_counts() {
    // A line with an unexpected first byte ends the hunk, failing counts.
    let err =
        parse_unified_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n!x\n").unwrap_err();
    assert!(err.contains("wrong number of hunk lines"), "{err}");
}

/// Backend whose every invocation fails with canned stderr (hg and svn
/// share the `SvnCommand`/`HgCommand` program+prefix shape, so one
/// script serves both). Removed on drop.
struct T1170FailBackend {
    dir: std::path::PathBuf,
}

impl T1170FailBackend {
    fn failing(tag: &str, stderr_line: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-t1170-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(windows)]
        {
            let bat = dir.join("fail.bat");
            // `echo(` (not `echo `): a bare `echo` prints "ECHO is off"
            // instead of an empty line (same trap as hg's FailBackend).
            std::fs::write(
                &bat,
                format!("@echo off\r\necho({stderr_line}>&2\r\nexit /b 1\r\n"),
            )
            .unwrap();
        }
        #[cfg(not(windows))]
        {
            let sh = dir.join("fail.sh");
            std::fs::write(
                &sh,
                format!("#!/bin/sh\necho \"{stderr_line}\" >&2\nexit 1\n"),
            )
            .unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&sh).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&sh, perms).unwrap();
            }
        }
        Self { dir }
    }

    fn hg(&self) -> hg::HgCommand {
        #[cfg(windows)]
        {
            let bat = self.dir.join("fail.bat");
            hg::HgCommand::custom("cmd", &["/C", &bat.to_string_lossy()])
        }
        #[cfg(not(windows))]
        {
            let sh = self.dir.join("fail.sh");
            hg::HgCommand::custom(&sh.to_string_lossy(), &[])
        }
    }

    fn svn(&self) -> svn::SvnCommand {
        #[cfg(windows)]
        {
            let bat = self.dir.join("fail.bat");
            svn::SvnCommand::custom("cmd", &["/C", &bat.to_string_lossy()])
        }
        #[cfg(not(windows))]
        {
            let sh = self.dir.join("fail.sh");
            svn::SvnCommand::custom(&sh.to_string_lossy(), &[])
        }
    }
}

impl Drop for T1170FailBackend {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn t1170_hg_diff_failure_reports_stderr() {
    let fake = T1170FailBackend::failing("hgdiff", "abort: no repo");
    let err = get_full_hg_diff_with(&fake.dir, &fake.hg(), "base", 3).unwrap_err();
    assert!(err.contains("hg diff failed: abort: no repo"), "{err}");
}

#[test]
fn t1170_svn_info_failure_names_exit_code() {
    let fake = T1170FailBackend::failing("svninfo", "oops");
    let err = get_svn_info_with(&fake.dir, &fake.svn(), "f.txt").unwrap_err();
    assert_eq!(err, "Error #1 executing svn info against 'f.txt'.");
}

#[test]
fn t1170_svn_file_list_resolves_absolute_and_dot_paths() {
    // Absolute CLI paths match absolutely.
    let list = SvnFileList::new(
        std::path::Path::new("/repo"),
        &["/repo/sub/f.txt".to_string()],
    );
    assert!(list.contains(std::path::Path::new("/repo"), "sub/f.txt"));
    assert!(!list.contains(std::path::Path::new("/repo"), "other.txt"));
    // `.` segments collapse lexically (against a real dir so the
    // trailing-slash directory rule applies).
    let root =
        std::env::temp_dir().join(format!("arc-rs-diff-t1170-filelist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let list = SvnFileList::new(&root, &["./sub".to_string()]);
    assert!(list.contains(&root, "sub/f.txt"));
    assert!(!list.contains(&root, "other/f.txt"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn t1170_collect_svn_path_data_skips_unusable_paths() {
    // Untracked, conflicted, and missing paths never reach the backend,
    // so even a bogus backend succeeds.
    let mut status = svn::StatusMap::new();
    status.insert("u.txt".to_string(), svn::FLAG_UNTRACKED);
    status.insert("c.txt".to_string(), svn::FLAG_CONFLICT);
    status.insert("m.txt".to_string(), svn::FLAG_MISSING);
    let config = svn::SvnDiffConfig::windows(3);
    let data = collect_svn_path_data_with(
        std::path::Path::new("definitely-not-an-svn-dir-t1170"),
        &svn::SvnCommand::custom("definitely-not-a-real-svn-binary-t1170", &[]),
        &status,
        &config,
    )
    .unwrap();
    assert!(data.is_empty());
}

#[test]
fn t1170_merge_svn_path_diffs_appends_unknown_paths() {
    // A parsed block for a path with no skeleton appends as-is.
    let data = vec![gap2_path_data(
        "a.txt",
        svn::FLAG_MODIFIED,
        &[("Node Kind", "file")],
        Some(
            "Index: other.txt\n--- other.txt (revision 1)\n+++ other.txt (working copy)\n@@ -1 +1 @@\n-a\n+b\n",
        ),
    )];
    let changes = merge_svn_path_diffs(std::path::Path::new("/repo"), &data, None).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].current_path, "a.txt");
    assert_eq!(changes[1].current_path, "other.txt");
    assert_eq!(changes[1].hunks.len(), 1);
}

#[test]
fn t1170_merge_svn_path_diffs_skips_empty_diff_bytes() {
    // Present-but-empty diff bytes keep the bare skeleton.
    let data = vec![gap2_path_data(
        "a.txt",
        svn::FLAG_MODIFIED,
        &[("Node Kind", "file")],
        Some(""),
    )];
    let changes = merge_svn_path_diffs(std::path::Path::new("/repo"), &data, None).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "a.txt");
    assert!(changes[0].hunks.is_empty());
}

#[test]
fn t1170_svn_copy_old_path_compares_numeric_revs() {
    // Numeric revisions compare numerically (`09` equals `9`).
    assert_eq!(
        svn_copy_old_path("https://r/trunk/f", "9", "10", "https://r"),
        Some("trunk/f@9".to_string())
    );
    assert_eq!(
        svn_copy_old_path("https://r/f", "09", "9", "https://r"),
        Some("f".to_string())
    );
    assert_eq!(
        svn_copy_old_path("https://r/f", "9", "9", "https://r"),
        Some("f".to_string())
    );
    // Non-numeric revisions compare as strings.
    assert_eq!(
        svn_copy_old_path("https://r/f", "HEAD", "10", "https://r"),
        Some("f@HEAD".to_string())
    );
}

#[test]
fn t1170_apply_svn_copy_info_transitions_origins() {
    // An ADD origin stays an add; the copy lands copy-here.
    let mut changes = vec![
        t1170_parsed_change(TYPE_ADD, Vec::new()),
        t1170_parsed_change(TYPE_CHANGE, Vec::new()),
    ];
    changes[0].current_path = "src".to_string();
    changes[1].current_path = "dst".to_string();
    let data = vec![
        gap2_path_data(
            "dst",
            svn::FLAG_ADDED,
            &[
                ("Copied From URL", "https://r/src"),
                ("Copied From Rev", ""),
                ("Revision", "1"),
            ],
            None,
        ),
        // A copied path with no change entry is skipped, not created.
        gap2_path_data(
            "ghost",
            svn::FLAG_ADDED,
            &[
                ("Copied From URL", "https://r/src"),
                ("Copied From Rev", ""),
                ("Revision", "1"),
            ],
            None,
        ),
    ];
    apply_svn_copy_info(&mut changes, &data, Some("https://r")).unwrap();
    assert_eq!(changes[0].change_type, TYPE_ADD);
    assert_eq!(changes[0].away_paths, vec!["dst".to_string()]);
    assert_eq!(changes[1].change_type, TYPE_COPY_HERE);
    assert_eq!(changes[1].old_path, "src");
    assert_eq!(changes.len(), 2);

    // A MOVE_AWAY origin escalates to multicopy; the move lands move-here.
    let mut changes = vec![
        t1170_parsed_change(TYPE_MOVE_AWAY, Vec::new()),
        t1170_parsed_change(TYPE_CHANGE, Vec::new()),
    ];
    changes[0].current_path = "src".to_string();
    changes[1].current_path = "dst".to_string();
    apply_svn_copy_info(&mut changes, &data[..1], Some("https://r")).unwrap();
    assert_eq!(changes[0].change_type, TYPE_MULTICOPY);
    assert_eq!(changes[1].change_type, TYPE_MOVE_HERE);
}

/// Canned `svn` backend (unix) for the gigantic-diff throw: `status`
/// serves XML, `info` serves fields, `diff` serves the big text.
#[cfg(unix)]
struct T1170FakeSvn {
    dir: std::path::PathBuf,
    svn: svn::SvnCommand,
}

#[cfg(unix)]
impl T1170FakeSvn {
    fn named(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-t1170-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".svn")).unwrap();
        let sh = dir.join("fakesvn.sh");
        std::fs::write(
            &sh,
            "#!/bin/sh\ncase \"$*\" in\n*status*) cat .resp-status;;\n*info*) cat .resp-info;;\n*) cat .resp-diff;;\nesac\nexit 0\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&sh).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&sh, perms).unwrap();
        }
        let svn = svn::SvnCommand::custom(&sh.to_string_lossy(), &[]);
        Self { dir, svn }
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }
}

#[cfg(unix)]
impl Drop for T1170FakeSvn {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(unix)]
#[test]
fn t1170_collect_svn_changes_throws_on_gigantic_corpus() {
    use std::io::Cursor;
    // One modified file whose parsed corpus exceeds 4 MiB aborts the fan-out.
    let fake = T1170FakeSvn::named("svngigantic");
    fake.write(
        ".resp-status",
        b"<?xml version=\"1.0\"?>\n<status>\n<target path=\".\">\n<entry path=\"big.txt\">\n<wc-status props=\"none\" item=\"modified\" revision=\"42\">\n</wc-status>\n</entry>\n</target>\n</status>\n",
    );
    fake.write(
        ".resp-info",
        b"Path: big.txt\nURL: https://x.example/r/big.txt\nRevision: 42\nNode Kind: file\n",
    );
    let lines = 100_000usize;
    let mut diff = format!(
        "Index: big.txt\n--- big.txt (revision 42)\n+++ big.txt (working copy)\n@@ -1,{lines} +1,{lines} @@\n"
    );
    for _ in 0..lines {
        diff.push_str(" xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n");
    }
    assert!(diff.len() > GIGANTIC_DIFF_LIMIT_BYTES);
    fake.write(".resp-diff", diff.as_bytes());
    let config = svn::SvnDiffConfig::windows(3);
    let mut io = SvnPromptIo {
        reader: &mut Cursor::new(b"".to_vec()),
        writer: &mut Vec::new(),
        warned_externals: false,
    };
    let err = collect_svn_parsed_changes_with_io(&fake.dir, &fake.svn, &[], &config, &mut io)
        .unwrap_err();
    assert!(err.contains("Diff for 'big.txt' with context is"), "{err}");
}

#[test]
fn t1170_reviewer_availability_skips_unknown_and_present_reviewers() {
    // `user.query` maps select in reviewer order; missing entries are
    // skipped, and reviewers who are not away validate silently.
    use std::io::Cursor;
    let mut message = CommitMessage::new_from_raw_corpus("Title\n").unwrap();
    message.set_field_value(
        "reviewerPHIDs",
        FieldValue::List(vec!["PHID-A".to_string(), "PHID-B".to_string()]),
    );
    let call = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "user.query");
        Ok(serde_json::json!({"PHID-A": {
            "userName": "a",
            "realName": "A",
            "currentStatus": "active",
        }}))
    };
    let mut out = Vec::new();
    validate_commit_message(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &call,
        &message,
        "PHID-USER-me",
        0,
    )
    .unwrap();
    assert!(out.is_empty(), "no away block, no prompt");
}

#[test]
fn t1170_edit_create_transport_error_echoes_saved_copy() {
    // A Conduit transport failure aborts after echoing the saved-copy note.
    use std::io::Cursor;
    let root = gap2_d1_scratch("t1170-transport");
    let meta = root.join(".git");
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("conduit down".to_string())
    };
    let edit = |template: &str| -> Result<String, String> { Ok(format!("{template}edited\n")) };
    let mut input = Cursor::new(b"".to_vec());
    let mut out = Vec::new();
    let mut editor = CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let err = edit_create_message(&mut editor, "Title\n", "", false, false, &[]).unwrap_err();
    assert_eq!(err, "conduit down");
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("(Message saved to "), "{text}");
    assert!(text.contains("create-message"), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn t1170_default_create_fields_transport_error_propagates() {
    // A Conduit transport failure aborts the defaults merge (no notes).
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("conduit down".to_string())
    };
    let local = vec![gap2_d1_local_commit("abc", "s", "Title\n")];
    assert_eq!(
        get_default_create_fields_with(&call, &local, false, None, None, false).unwrap_err(),
        "conduit down"
    );
}

#[test]
fn t1170_hg_working_copy_changes_maps_every_mask() {
    use std::collections::BTreeMap;
    let mut status = BTreeMap::new();
    status.insert("e".to_string(), hg::FLAG_EXTERNALS);
    status.insert("u".to_string(), hg::FLAG_UNTRACKED);
    let changes = hg_working_copy_changes(&status);
    assert_eq!(changes.externals, vec!["e".to_string()]);
    assert_eq!(changes.untracked, vec!["u".to_string()]);
    assert!(changes.conflicts.is_empty());
    let changes = git_working_copy_changes(&status);
    assert_eq!(changes.externals, vec!["e".to_string()]);
}

/// Reader that always fails, for confirm error paths.
struct T1170ErrReader;

impl std::io::Read for T1170ErrReader {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("t1170 no reads"))
    }
}

impl std::io::BufRead for T1170ErrReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("t1170 no reads"))
    }

    fn consume(&mut self, _: usize) {}
}

#[test]
fn t1170_clean_check_confirm_read_failures_abort() {
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let vcs = gap2_d3_vcs(&edit);
    // The externals confirm cannot read its answer.
    let changes = WorkingCopyChanges {
        externals: vec!["ext".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut T1170ErrReader,
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert!(!err.is_empty());
    // The untracked confirm cannot read its answer either.
    let changes = WorkingCopyChanges {
        untracked: vec!["u.txt".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    let err = require_clean_working_copy(
        &mut T1170ErrReader,
        &mut out,
        &vcs,
        &changes,
        CommitMode::Allow,
    )
    .unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn t1170_clean_check_write_failure_names_prompt() {
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let vcs = gap2_d3_vcs(&edit);
    let changes = WorkingCopyChanges {
        uncommitted: vec!["a.txt".to_string()],
        ..Default::default()
    };
    let err = require_clean_working_copy(
        &mut T1170ErrReader,
        &mut T1170FailWriter,
        &vcs,
        &changes,
        CommitMode::Disable,
    )
    .unwrap_err();
    assert!(err.contains("failed to write prompt"), "{err}");
}

#[test]
fn t1170_clean_check_stash_without_hook_marks_stashed() {
    // Autostash with no stash hook still counts as stashed.
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let mut vcs = gap2_d3_vcs(&edit);
    vcs.autostash = true;
    let changes = WorkingCopyChanges {
        uncommitted: vec!["a.txt".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    let outcome = require_clean_working_copy(
        &mut T1170ErrReader,
        &mut out,
        &vcs,
        &changes,
        CommitMode::Disable,
    )
    .unwrap();
    assert!(outcome.stashed);
}

#[test]
fn t1170_clean_check_commit_branches() {
    use std::io::Cursor;
    let edit = |_: &str| -> Result<String, String> { Ok("New message\n".to_string()) };
    // Amending without a head commit fails.
    let mut vcs = gap2_d3_vcs(&edit);
    vcs.should_amend = true;
    let changes = WorkingCopyChanges {
        uncommitted: vec!["a.txt".to_string()],
        ..Default::default()
    };
    let mut out = Vec::new();
    assert_eq!(
        require_clean_working_copy(
            &mut Cursor::new(b"".to_vec()),
            &mut out,
            &vcs,
            &changes,
            CommitMode::Enable,
        )
        .unwrap_err(),
        "cannot amend without a local commit"
    );
    // Amending with a head but no hook succeeds silently.
    vcs.head_commit_message = Some("Head\n".to_string());
    let mut out = Vec::new();
    let outcome = require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Enable,
    )
    .unwrap();
    assert!(!outcome.stashed);
    // VCSs without local commits skip the editor entirely.
    let fail_edit = |_: &str| -> Result<String, String> { panic!("editor must not run") };
    let mut vcs = gap2_d3_vcs(&fail_edit);
    vcs.supports_local_commits = false;
    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Enable,
    )
    .unwrap();
    // The editor path commits without a hook.
    let vcs = gap2_d3_vcs(&edit);
    let mut out = Vec::new();
    require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        CommitMode::Enable,
    )
    .unwrap();
}

#[test]
fn t1170_conduit_truthy_shapes_drive_amend_decision() {
    // The published-commit check treats PHP-falsy conduit values as
    // unpublished (amend) and truthy ones as published (no amend).
    let head = gap2_d1_local_commit("abc", "s", "Plain title\n");
    let no_revisions = || -> Result<Vec<(i64, String)>, String> { Ok(Vec::new()) };
    for (value, expected) in [
        (serde_json::json!(true), false),
        (serde_json::json!(false), true),
        (serde_json::json!(1), false),
        (serde_json::json!(0), true),
        (serde_json::json!(1.5), false),
        (serde_json::json!(0.0), true),
        (serde_json::json!("x"), false),
        (serde_json::json!("0"), true),
        (serde_json::json!(""), true),
        (serde_json::json!([1]), false),
        (serde_json::json!([]), true),
    ] {
        let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({"identifierMap": {"R7:abc": value}}))
        };
        let check = AmendCheck {
            history_immutable: false,
            supports_amend: true,
            head: Some(head.clone()),
            api_author: "a",
            create: false,
            repository: Some(AmendRepository {
                id: 7,
                callsign: None,
            }),
            load_open_revisions: &no_revisions,
            call: &call,
        };
        assert_eq!(calculate_should_amend(&check).unwrap(), expected, "{value}");
    }
}
