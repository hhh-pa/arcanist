use super::*;
use crate::diff_local;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn defaults() -> Options {
    Options {
        base: None,
        json: false,
        head: None,
        commit: None,
        message: None,
        message_file: None,
        reviewers: None,
        cc: None,
        update: None,
        create: false,
        only: false,
        conduit_uri: None,
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_options(&args(&[])).unwrap(), Some(defaults()));
}

#[test]
fn parses_base_and_json() {
    assert_eq!(
        parse_options(&args(&["--base", "HEAD~1", "--json"])).unwrap(),
        Some(Options {
            base: Some("HEAD~1".to_string()),
            json: true,
            ..defaults()
        })
    );
    assert_eq!(
        parse_options(&args(&["--base=abc123"])).unwrap(),
        Some(Options {
            base: Some("abc123".to_string()),
            ..defaults()
        })
    );
}

#[test]
fn parses_range_and_field_flags() {
    let options = parse_options(&args(&[
        "--head",
        "HEAD",
        "--message",
        "take two",
        "--message-file",
        "msg.txt",
        "--reviewers",
        "alice,bob",
        "--cc",
        "carol",
        "--conduit-uri",
        "https://phab.example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(
        options,
        Options {
            head: Some("HEAD".to_string()),
            message: Some("take two".to_string()),
            message_file: Some("msg.txt".to_string()),
            reviewers: Some("alice,bob".to_string()),
            cc: Some("carol".to_string()),
            conduit_uri: Some("https://phab.example.test/".to_string()),
            ..defaults()
        }
    );
    let options = parse_options(&args(&[
        "--head=abc",
        "--message=hi",
        "--message-file=m.txt",
        "--reviewers=al",
        "--cc=bo",
        "--conduit-uri=https://x.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.head.as_deref(), Some("abc"));
    assert_eq!(options.message.as_deref(), Some("hi"));
    assert_eq!(options.message_file.as_deref(), Some("m.txt"));
    assert_eq!(options.reviewers.as_deref(), Some("al"));
    assert_eq!(options.cc.as_deref(), Some("bo"));
    assert_eq!(options.conduit_uri.as_deref(), Some("https://x.test/"));
    // `--update` conflicts with `--reviewers`/`--cc`, so it parses alone.
    let options = parse_options(&args(&["--update=D12"])).unwrap().unwrap();
    assert_eq!(options.update.as_deref(), Some("D12"));
}

#[test]
fn parses_shorts_and_submit_modes() {
    let options = parse_options(&args(&["-m", "msg", "-F", "f.txt"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("msg"));
    assert_eq!(options.message_file.as_deref(), Some("f.txt"));

    let options = parse_options(&args(&["--create"])).unwrap().unwrap();
    assert!(options.create && !options.only && options.update.is_none());
    let options = parse_options(&args(&["--only"])).unwrap().unwrap();
    assert!(options.only && !options.create && options.update.is_none());
    let options = parse_options(&args(&["--update", "123"])).unwrap().unwrap();
    assert_eq!(options.update.as_deref(), Some("123"));
}

#[test]
fn parses_single_positional_commit() {
    let options = parse_options(&args(&["HEAD~1"])).unwrap().unwrap();
    assert_eq!(options.commit.as_deref(), Some("HEAD~1"));
    let options = parse_options(&args(&["--", "HEAD~1"])).unwrap().unwrap();
    assert_eq!(options.commit.as_deref(), Some("HEAD~1"));
    let err = parse_options(&args(&["a", "b"])).unwrap_err();
    assert!(err.contains("exactly one base commit"), "{err}");
}

#[test]
fn parses_help() {
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_options(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_options(&args(&["--base"])).is_err());
    assert!(parse_options(&args(&["--base="])).is_err());
    assert!(parse_options(&args(&["--head"])).is_err());
    assert!(parse_options(&args(&["--head="])).is_err());
    assert!(parse_options(&args(&["--message"])).is_err());
    assert!(parse_options(&args(&["-m"])).is_err());
    assert!(parse_options(&args(&["--message-file"])).is_err());
    assert!(parse_options(&args(&["-F"])).is_err());
    assert!(parse_options(&args(&["--reviewers"])).is_err());
    assert!(parse_options(&args(&["--cc"])).is_err());
    assert!(parse_options(&args(&["--update"])).is_err());
    assert!(parse_options(&args(&["--conduit-uri"])).is_err());
    assert!(parse_options(&args(&["--bogus"])).is_err());
}

#[test]
fn rejects_conflicting_submit_flags() {
    let err = parse_options(&args(&["--create", "--update", "1"])).unwrap_err();
    assert_eq!(err, "'--create' can not be used with '--update'.");
    let err = parse_options(&args(&["--create", "--only"])).unwrap_err();
    assert_eq!(err, "'--create' can not be used with '--only'.");
    let err = parse_options(&args(&["--only", "--message", "x"])).unwrap_err();
    assert!(
        err.contains("'--only' does not update any revision"),
        "{err}"
    );
    let err = parse_options(&args(&["--only", "--reviewers", "a"])).unwrap_err();
    assert_eq!(err, "'--reviewers' can not be used with '--only'.");
    let err = parse_options(&args(&["--only", "--cc", "a"])).unwrap_err();
    assert_eq!(err, "'--cc' can not be used with '--only'.");
    let err = parse_options(&args(&["--update", "1", "--reviewers", "a"])).unwrap_err();
    assert_eq!(err, "'--reviewers' can not be used with '--update'.");
    let err = parse_options(&args(&["--update", "1", "--cc", "a"])).unwrap_err();
    assert_eq!(err, "'--cc' can not be used with '--update'.");
    let err = parse_options(&args(&["--base", "abc", "def"])).unwrap_err();
    assert_eq!(err, "'--base' can not be used with a base commit.");
}

#[test]
fn derives_submit_mode() {
    assert_eq!(submit_mode(&defaults()).unwrap(), None);
    assert_eq!(
        submit_mode(&Options {
            create: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Create)
    );
    assert_eq!(
        submit_mode(&Options {
            only: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Only)
    );
    assert_eq!(
        submit_mode(&Options {
            update: Some("D12".to_string()),
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Update(12))
    );
    // `--update` wins over `--only`, like shouldOnlyCreateDiff().
    assert_eq!(
        submit_mode(&Options {
            update: Some("7".to_string()),
            only: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Update(7))
    );
    let err = submit_mode(&Options {
        update: Some("nope".to_string()),
        ..defaults()
    })
    .unwrap_err();
    assert!(err.contains("Parameter to --update"), "{err}");
}

#[test]
fn merges_fields_oldest_first_with_faux_flags() {
    let commits = vec![
        "Oldest title\n\nOld body.\n".to_string(),
        "Newest title\n\nReviewers: alice\n".to_string(),
    ];
    let fields = merge_message_fields(&commits, Some("bob"), Some("carol"));
    assert_eq!(
        fields.get(diff_local::FIELD_TITLE),
        Some(&diff_local::FieldValue::Text("Oldest title".to_string()))
    );
    let summary = match fields.get(diff_local::FIELD_SUMMARY) {
        Some(diff_local::FieldValue::Text(text)) => text.clone(),
        other => panic!("missing summary: {other:?}"),
    };
    assert!(summary.contains("Old body"), "{summary}");
    assert!(summary.contains("Newest title"), "{summary}");
    assert_eq!(
        fields.get(diff_local::FIELD_REVIEWERS),
        Some(&diff_local::FieldValue::List(vec![
            "alice".to_string(),
            "bob".to_string()
        ]))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_CCS),
        Some(&diff_local::FieldValue::List(vec!["carol".to_string()]))
    );
}

#[test]
fn renders_fields_json() {
    let mut fields = diff_local::CommitFields::new();
    fields.insert(
        diff_local::FIELD_TITLE.to_string(),
        diff_local::FieldValue::Text("T".to_string()),
    );
    fields.insert(
        diff_local::FIELD_REVIEWERS.to_string(),
        diff_local::FieldValue::List(vec!["a".to_string()]),
    );
    assert_eq!(
        fields_json(&fields),
        serde_json::json!({"title": "T", "reviewerPHIDs": ["a"]})
    );
}

#[test]
fn parses_empty_numstat() {
    assert_eq!(parse_numstat_z(b"").unwrap(), Vec::new());
}

#[test]
fn parses_numstat_entries() {
    let raw = b"2\t1\tf.txt\0-\t-\tbin.dat\0";
    assert_eq!(
        parse_numstat_z(raw).unwrap(),
        vec![
            FileDiff {
                path: "f.txt".to_string(),
                added: 2,
                deleted: 1
            },
            FileDiff {
                path: "bin.dat".to_string(),
                added: 0,
                deleted: 0
            },
        ]
    );
}

#[test]
fn parses_numstat_rename_to_new_path() {
    let raw = b"0\t0\t\0g.txt\0renamed.txt\0";
    assert_eq!(
        parse_numstat_z(raw).unwrap(),
        vec![FileDiff {
            path: "renamed.txt".to_string(),
            added: 0,
            deleted: 0
        }]
    );
}

#[test]
fn rejects_malformed_numstat() {
    assert!(parse_numstat_z(b"no-tabs-here\0").is_err());
    assert!(parse_numstat_z(b"x\t1\tf\0").is_err());
    assert!(parse_numstat_z(b"0\t0\t\0only-old\0").is_err());
}

#[test]
fn builds_json_document() {
    let files = vec![FileDiff {
        path: "f.txt".to_string(),
        added: 2,
        deleted: 1,
    }];
    let doc = diff_json("abc", &files);
    assert_eq!(
        doc,
        serde_json::json!({
            "base": "abc",
            "files": [{"path": "f.txt", "added": 2, "deleted": 1}],
            "insertions": 2,
            "deletions": 1,
        })
    );
    assert_eq!(summarize(&[]), (0, 0));
}
