use super::*;
use serde_json::json;

/* G1 full-state tests: revision flow, away reviewers, autotargets. */

#[test]
fn g1_revision_flow_texts() {
    assert_eq!(CREATED_DIFF_LEAD, "Created a new Differential diff:");
    assert_eq!(DIFF_URI_LABEL, "Diff URI:");
    assert_eq!(
        UPDATED_REVISION_LEAD,
        "Updated an existing Differential revision:"
    );
    assert_eq!(
        CREATED_REVISION_LEAD,
        "Created a new Differential revision:"
    );
    assert_eq!(REVISION_URI_LABEL, "Revision URI:");
    assert_eq!(INCLUDED_CHANGES_LABEL, "Included changes:");

    let list = render_revision_list(&[(1, "First"), (22, "Second")]);
    assert_eq!(list, "     - D1: First\n     - D22: Second\n");
    assert!(render_revision_list(&[]).is_empty());

    let err = several_revisions_error(&list);
    assert!(
        err.starts_with(
            "There are several revisions which match the working copy:\n\n     - D1: First\n"
        ),
        "{err}"
    );
    assert!(
        err.ends_with("Use '--update' to choose one, or '--create' to create a new revision."),
        "{err}"
    );

    assert_eq!(
        UPDATE_PARAM_MUST_BE_NUMBER,
        "Parameter to --update must be a Differential Revision number."
    );
    assert_eq!(ALREADY_A_DRAFT, "ALREADY A DRAFT");

    assert!(check_revision_ownership("PHID-A", "PHID-A", 5, "T").is_none());
    let prompt = check_revision_ownership("PHID-A", "PHID-B", 5, "T").unwrap();
    assert!(prompt.contains("You don't own revision D5"), "{prompt}");
    assert!(prompt.ends_with("Update this revision anyway?"), "{prompt}");
}

#[test]
fn g1_away_reviewer_helpers() {
    assert_eq!(
        away_reviewers_all_message("Thursday, Jan 1 1970"),
        "All reviewers are away until Thursday, Jan 1 1970:"
    );
    assert_eq!(AWAY_SOME_MESSAGE, "Some reviewers are currently away:");
    assert_eq!(
        away_reviewer_list_item("alice", "Friday, Jan 2 1970"),
        "alice (until Friday, Jan 2 1970)"
    );
    assert_eq!(
        CONTINUE_DESPITE_AWAY,
        "Continue even though reviewers are unavailable?"
    );
    assert_eq!(
        SPECIFY_AVAILABLE_REVIEWERS,
        "Specify available reviewers and retry."
    );

    assert_eq!(format_phab_date(0), "Thursday, Jan 1 1970");
    assert_eq!(format_phab_date(86_400), "Friday, Jan 2 1970");
    assert_eq!(format_phab_date(-86_400), "Wednesday, Dec 31 1969");
    assert_eq!(format_phab_date(1_767_225_600), "Thursday, Jan 1 2026");
    assert_eq!(format_phab_date(1_768_089_600), "Sunday, Jan 11 2026");

    assert_eq!(away_until_timestamp(&json!(123)), 123);
    assert_eq!(away_until_timestamp(&json!(null)), 0);
    assert_eq!(away_until_timestamp(&json!("x")), 0);

    let aways = vec![
        AwayReviewer {
            name: "a".to_string(),
            until: json!(200),
        },
        AwayReviewer {
            name: "b".to_string(),
            until: json!(100),
        },
    ];
    assert_eq!(away_earliest_return(&aways), Some(100));
    assert_eq!(away_earliest_return(&[]), None);

    assert!(away_reviewers_block(&[], 0).is_none());
    let all = away_reviewers_block(&aways, 2).unwrap();
    assert!(
        all.starts_with("All reviewers are away until Thursday, Jan 1 1970:\n\n"),
        "{all}"
    );
    assert!(
        all.contains("      a (until Thursday, Jan 1 1970)\n"),
        "{all}"
    );
    let some = away_reviewers_block(&aways[..1], 2).unwrap();
    assert!(
        some.starts_with("Some reviewers are currently away:\n\n"),
        "{some}"
    );
}

#[test]
fn gap2_phab_date_offset_shifts_midnight_boundaries() {
    // 23:00 UTC Thursday: UTC keeps Thursday, +2h rolls to Friday.
    assert_eq!(
        format_phab_date_with_offset(82_800, 0),
        "Thursday, Jan 1 1970"
    );
    assert_eq!(
        format_phab_date_with_offset(82_800, 7_200),
        "Friday, Jan 2 1970"
    );
    // 01:00 UTC Thursday: -2h rolls back to Wednesday.
    assert_eq!(
        format_phab_date_with_offset(3_600, -7_200),
        "Wednesday, Dec 31 1969"
    );
    // Zero offset matches the UTC entry point everywhere.
    for ts in [0, 82_800, 1_767_225_600, -1, -86_400] {
        assert_eq!(format_phab_date_with_offset(ts, 0), format_phab_date(ts));
    }
    let aways = vec![AwayReviewer {
        name: "a".to_string(),
        until: json!(82_800),
    }];
    let utc = away_reviewers_block(&aways, 1).unwrap();
    assert!(utc.contains("(until Thursday, Jan 1 1970)"), "{utc}");
    let shifted = away_reviewers_block_with_offset(&aways, 1, 7_200).unwrap();
    assert!(
        shifted.starts_with("All reviewers are away until Friday, Jan 2 1970:\n\n"),
        "{shifted}"
    );
    assert!(
        shifted.contains("      a (until Friday, Jan 2 1970)\n"),
        "{shifted}"
    );
}

#[test]
fn g1_autotarget_helpers() {
    assert_eq!(METHOD_QUERY_AUTOTARGETS, "harbormaster.queryautotargets");
    assert_eq!(METHOD_SENDMESSAGE, "harbormaster.sendmessage");
    assert_eq!(AUTOTARGET_LINT_KEY, "arcanist.lint");
    assert_eq!(AUTOTARGET_UNIT_KEY, "arcanist.unit");

    let params = query_autotargets_params("PHID-DIFF-1");
    assert_eq!(params["objectPHID"], json!("PHID-DIFF-1"));
    assert_eq!(
        params["targetKeys"],
        json!(["arcanist.lint", "arcanist.unit"])
    );

    let response = json!({"targetMap": {"arcanist.lint": "PHID-T-1"}});
    assert_eq!(
        autotarget_phid(&response, "arcanist.lint").as_deref(),
        Some("PHID-T-1")
    );
    assert_eq!(autotarget_phid(&response, "arcanist.unit"), None);
    assert_eq!(autotarget_phid(&json!({}), "arcanist.lint"), None);

    assert_eq!(lint_autotarget_type(&[]), "pass");
    assert_eq!(lint_autotarget_type(&["advice", "disabled"]), "pass");
    assert_eq!(lint_autotarget_type(&["warning"]), "fail");
    assert_eq!(lint_autotarget_type(&["advice", "error"]), "fail");

    let lint_params = sendmessage_lint_params("PHID-T-1", &[json!({"a": 1})], "fail");
    assert_eq!(lint_params["buildTargetPHID"], json!("PHID-T-1"));
    assert_eq!(lint_params["lint"], json!([{"a": 1}]));
    assert_eq!(lint_params["type"], json!("fail"));
    let unit_params = sendmessage_unit_params("PHID-T-2", &[], "pass");
    assert_eq!(unit_params["buildTargetPHID"], json!("PHID-T-2"));
    assert_eq!(unit_params["unit"], json!([]));
    assert_eq!(unit_params["type"], json!("pass"));
}

#[test]
fn g1_update_autotargets_over_seam() {
    use std::cell::RefCell;
    let sent = RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == "harbormaster.queryautotargets" {
            return Ok(
                json!({"targetMap": {"arcanist.lint": "PHID-L", "arcanist.unit": "PHID-U"}}),
            );
        }
        assert_eq!(method, "harbormaster.sendmessage");
        sent.borrow_mut().push(params.clone());
        Ok(json!(null))
    };
    let ok = update_autotargets(
        &call,
        "PHID-DIFF-1",
        &[json!({"severity": "warning"})],
        &[json!({"name": "t"})],
        UNIT_RESULT_OKAY,
    );
    assert!(ok);
    let sent = sent.borrow();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["buildTargetPHID"], json!("PHID-L"));
    assert_eq!(sent[0]["type"], json!("fail"));
    assert_eq!(sent[1]["buildTargetPHID"], json!("PHID-U"));
    assert_eq!(sent[1]["type"], json!("pass"));

    let failing = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("down".to_string())
    };
    assert!(!update_autotargets(
        &failing,
        "PHID-D-1",
        &[],
        &[],
        UNIT_RESULT_OKAY
    ));

    let send_fails = |method: &str, _: &serde_json::Value| {
        if method == "harbormaster.queryautotargets" {
            Ok(json!({"targetMap": {"arcanist.lint": "PHID-L"}}))
        } else {
            Err("send down".to_string())
        }
    };
    assert!(!update_autotargets(
        &send_fails,
        "PHID-D-1",
        &[],
        &[],
        UNIT_RESULT_OKAY
    ));

    let no_targets = |_: &str, _: &serde_json::Value| Ok(json!({"targetMap": {}}));
    assert!(update_autotargets(
        &no_targets,
        "PHID-D-1",
        &[],
        &[],
        UNIT_RESULT_FAIL
    ));
}

#[test]
fn g1_staging_property_data_orders_keys() {
    assert_eq!(
        staging_property_data("pushed", &[json!({"ref": "r"})]),
        "{\"status\":\"pushed\",\"refs\":[{\"ref\":\"r\"}]}"
    );
    assert_eq!(
        staging_property_data("user.skip", &[]),
        "{\"status\":\"user.skip\",\"refs\":[]}"
    );
}

#[test]
fn gap2_php_json_encode_escapes_slashes_like_php() {
    let doc = json!({
        "diffURI": "http://phab/D1",
        "path": "src/a/b.rs",
    });
    let text = php_json_encode(&doc).unwrap();
    assert_eq!(
        text,
        "{\"diffURI\":\"http:\\/\\/phab\\/D1\",\"path\":\"src\\/a\\/b.rs\"}"
    );
    // Escaped output still parses to the identical value.
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed, doc);
    // Slash-free documents encode exactly like serde.
    let plain = json!({"a": 1, "b": [true, null]});
    assert_eq!(
        php_json_encode(&plain).unwrap(),
        serde_json::to_string(&plain).unwrap()
    );
}

#[test]
fn gap2_php_json_encode_pretty_escapes_slashes_like_php() {
    let doc = json!({"diffURI": "http://phab/D1"});
    let text = php_json_encode_pretty(&doc).unwrap();
    assert!(text.contains("http:\\/\\/phab\\/D1"), "{text:?}");
    assert!(text.contains('\n'), "{text:?}");
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed, doc);
}

#[test]
fn gap2_staging_property_data_escapes_slashes() {
    assert_eq!(
        staging_property_data("ok", &[json!({"ref": "refs/heads/x"})]),
        "{\"status\":\"ok\",\"refs\":[{\"ref\":\"refs\\/heads\\/x\"}]}"
    );
}

fn spec_input() -> DiffSpecInput {
    DiffSpecInput {
        source_machine: "devbox".to_string(),
        source_path: Some("/repo".to_string()),
        branch: Some("feature".to_string()),
        bookmark: None,
        source_control_system: Some("git".to_string()),
        source_control_path: Some("/repo".to_string()),
        source_control_base_revision: Some("abc123".to_string()),
        repository_phid: None,
    }
}

fn sample_change() -> DiffChange {
    let mut change = DiffChange::new_text("old.txt", "new.txt");
    change.hunks.push(DiffHunk {
        old_offset: 1,
        new_offset: 1,
        old_length: 1,
        new_length: 2,
        // T1153 EXPORT-E11: `@@ -1 +1,2 @@` spells only the new length.
        old_len_explicit: false,
        new_len_explicit: true,
        add_lines: 1,
        del_lines: 0,
        is_missing_old_newline: false,
        is_missing_new_newline: true,
        corpus: "@@ -1 +1,2 @@\n".to_string(),
    });
    change
}

#[test]
fn const_values_are_verbatim_from_php() {
    assert_eq!(METHOD_CREATE_DIFF, "differential.creatediff");
    assert_eq!(METHOD_UPDATE_REVISION, "differential.updaterevision");
    assert_eq!(METHOD_REVISION_EDIT, "differential.revision.edit");
    assert_eq!(METHOD_CREATE_REVISION, "differential.createrevision");
    assert_eq!(METHOD_GET_COMMIT_MESSAGE, "differential.getcommitmessage");
    assert_eq!(METHOD_SET_DIFF_PROPERTY, "differential.setdiffproperty");
    assert_eq!(METHOD_QUERY, "differential.query");
    assert_eq!(METHOD_USER_QUERY, "user.query");
    assert_eq!(METHOD_REVISION_SEARCH, "differential.revision.search");
    assert_eq!(METHOD_DIFF_SEARCH, "differential.diff.search");
    assert_eq!(METHOD_QUERY_DIFFS, "differential.querydiffs");
    assert_eq!(
        METHOD_PARSE_COMMIT_MESSAGE,
        "differential.parsecommitmessage"
    );
    assert_eq!(PROP_LINT, "arc:lint");
    assert_eq!(PROP_UNIT, "arc:unit");
    assert_eq!(PROP_LOCAL_COMMITS, "local:commits");
    assert_eq!(PROP_ONTO, "arc:onto");
    assert_eq!(
        [
            TYPE_ADD,
            TYPE_CHANGE,
            TYPE_DELETE,
            TYPE_MOVE_AWAY,
            TYPE_COPY_AWAY,
            TYPE_MOVE_HERE,
            TYPE_COPY_HERE,
            TYPE_MULTICOPY,
            TYPE_MESSAGE,
            TYPE_CHILD,
        ],
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    );
    assert_eq!(
        [
            FILE_TEXT,
            FILE_IMAGE,
            FILE_BINARY,
            FILE_DIRECTORY,
            FILE_SYMLINK,
            FILE_DELETED,
            FILE_NORMAL,
        ],
        [1, 2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn lint_status_maps_codes() {
    assert_eq!(lint_status(LINT_RESULT_OKAY), "okay");
    assert_eq!(lint_status(LINT_RESULT_ERRORS), "fail");
    assert_eq!(lint_status(LINT_RESULT_WARNINGS), "warn");
    assert_eq!(lint_status(LINT_RESULT_SKIP), "skip");
    assert_eq!(lint_status(99), "none");
    assert_eq!(lint_status(-1), "none");
}

#[test]
fn unit_status_maps_codes() {
    assert_eq!(unit_status(UNIT_RESULT_OKAY), "okay");
    assert_eq!(unit_status(UNIT_RESULT_FAIL), "fail");
    assert_eq!(unit_status(UNIT_RESULT_UNSOUND), "warn");
    assert_eq!(unit_status(UNIT_RESULT_SKIP), "skip");
    assert_eq!(unit_status(42), "none");
}

#[test]
fn change_defaults_match_php() {
    let change = DiffChange::default();
    assert_eq!(change.change_type, TYPE_CHANGE);
    assert_eq!(change.file_type, FILE_TEXT);
    assert!(change.hunks.is_empty());
    assert_eq!(change.commit_hash, None);
}

#[test]
fn hunk_json_uses_verbatim_keys() {
    // T1153 EXPORT-E11: offsets are strings; `@@ -3,7 +5,9 @@` spells
    // both lengths, so both render as strings.
    let value = hunk_json(&DiffHunk {
        old_offset: 3,
        new_offset: 5,
        old_length: 7,
        new_length: 9,
        old_len_explicit: true,
        new_len_explicit: true,
        add_lines: 2,
        del_lines: 1,
        is_missing_old_newline: true,
        is_missing_new_newline: false,
        corpus: "body".to_string(),
    });
    assert_eq!(
        value,
        json!({
            "oldOffset": "3",
            "newOffset": "5",
            "oldLength": "7",
            "newLength": "9",
            "addLines": 2,
            "delLines": 1,
            "isMissingOldNewline": true,
            "isMissingNewNewline": false,
            "corpus": "body",
        })
    );
}

#[test]
fn hunk_json_renders_omitted_lengths_as_int_one() {
    // T1153 EXPORT-E11: `@@ -1 +1 @@` omits both lengths, so PHP
    // substitutes integer 1 (`ArcanistDiffParser.php:922-933`) while
    // offsets stay strings.
    let value = hunk_json(&DiffHunk {
        old_offset: 1,
        new_offset: 1,
        old_length: 1,
        new_length: 1,
        old_len_explicit: false,
        new_len_explicit: false,
        ..DiffHunk::default()
    });
    assert_eq!(value["oldOffset"], json!("1"));
    assert_eq!(value["newOffset"], json!("1"));
    assert_eq!(value["oldLength"], json!(1));
    assert_eq!(value["newLength"], json!(1));
}

#[test]
fn change_json_uses_verbatim_keys() {
    let value = change_json(&sample_change());
    assert_eq!(value["oldPath"], json!("old.txt"));
    assert_eq!(value["currentPath"], json!("new.txt"));
    assert_eq!(value["awayPaths"], json!([]));
    assert_eq!(value["type"], json!(TYPE_CHANGE));
    assert_eq!(value["fileType"], json!(FILE_TEXT));
    assert_eq!(value["commitHash"], serde_json::Value::Null);
    assert_eq!(value["hunks"].as_array().unwrap().len(), 1);
    assert_eq!(value["hunks"][0]["corpus"], json!("@@ -1 +1,2 @@\n"));
}

#[test]
fn diff_spec_json_has_verbatim_shape() {
    let spec = diff_spec_json(&[sample_change()], "okay", "warn", &spec_input());
    assert_eq!(spec["lintStatus"], json!("okay"));
    assert_eq!(spec["unitStatus"], json!("warn"));
    assert_eq!(spec["sourceMachine"], json!("devbox"));
    assert_eq!(spec["sourcePath"], json!("/repo"));
    assert_eq!(spec["branch"], json!("feature"));
    assert_eq!(spec["bookmark"], serde_json::Value::Null);
    assert_eq!(spec["sourceControlSystem"], json!("git"));
    assert_eq!(spec["sourceControlPath"], json!("/repo"));
    assert_eq!(spec["sourceControlBaseRevision"], json!("abc123"));
    assert_eq!(spec["creationMethod"], json!("arc"));
    assert!(spec.get("repositoryPHID").is_none());
    assert_eq!(spec["changes"].as_array().unwrap().len(), 1);
}

#[test]
fn diff_spec_json_sends_repository_phid_when_known() {
    let mut input = spec_input();
    input.repository_phid = Some("PHID-REPO-x".to_string());
    let spec = diff_spec_json(&[], "none", "none", &input);
    assert_eq!(spec["repositoryPHID"], json!("PHID-REPO-x"));
}

#[test]
fn revision_params_use_verbatim_keys() {
    let fields = json!({"title": "T"});
    assert_eq!(
        update_revision_params(7, fields.clone(), 42, "msg"),
        json!({"fields": {"title": "T"}, "id": 7, "message": "msg", "diffid": 42})
    );
    assert_eq!(
        create_revision_params(fields, 42),
        json!({"fields": {"title": "T"}, "diffid": 42})
    );
}

#[test]
fn revision_edit_params_appends_update_and_draft() {
    let base = [json!({"type": "title", "value": "T"})];
    let params = revision_edit_params(&base, Some("PHID-DIFF-1"), true);
    assert_eq!(
        params,
        json!({"transactions": [
            {"type": "title", "value": "T"},
            {"type": "update", "value": "PHID-DIFF-1"},
            {"type": "draft", "value": true},
        ]})
    );
    let params = revision_edit_params(&[], None, false);
    assert_eq!(
        params,
        json!({"transactions": [{"type": "update", "value": null}]})
    );
}

#[test]
fn commit_message_params_cover_all_variants() {
    assert_eq!(
        commit_message_after_create_params(9),
        json!({"revision_id": 9})
    );
    assert_eq!(
        commit_message_update_params(9, json!({"a": 1})),
        json!({"revision_id": 9, "edit": "edit", "fields": {"a": 1}})
    );
    assert_eq!(
        commit_message_template_params(json!({"a": 1})),
        json!({"revision_id": null, "edit": "create", "fields": {"a": 1}})
    );
    assert_eq!(
        commit_message_from_revision_params(9),
        json!({"revision_id": 9, "edit": false})
    );
}

#[test]
fn query_builders_use_verbatim_keys() {
    assert_eq!(
        set_diff_property_params(42, "arc:unit", "{}"),
        json!({"diff_id": 42, "name": "arc:unit", "data": "{}"})
    );
    assert_eq!(differential_query_params(&[1, 2]), json!({"ids": [1, 2]}));
    assert_eq!(user_query_params(&["a", "b"]), json!({"phids": ["a", "b"]}));
    assert_eq!(
        revision_search_params(&[3]),
        json!({"constraints": {"ids": [3]}})
    );
    assert_eq!(
        diff_search_params(&["PHID-DREV-1"]),
        json!({
            "constraints": {"revisionPHIDs": ["PHID-DREV-1"]},
            "attachments": {"commits": true},
            "limit": 1,
        })
    );
    assert_eq!(query_diffs_params(&[3]), json!({"revisionIDs": [3]}));
    assert_eq!(
        parse_commit_message_params("corpus", true),
        json!({"corpus": "corpus", "partial": true})
    );
}

#[test]
fn parses_creatediff_response() {
    let created = parse_creatediff_response(&json!({
        "diffid": 42,
        "phid": "PHID-DIFF-1",
        "uri": "https://phab.test/differential/diff/42/",
    }))
    .unwrap();
    assert_eq!(created.diff_id, 42);
    assert_eq!(created.phid.as_deref(), Some("PHID-DIFF-1"));
    assert_eq!(created.uri, "https://phab.test/differential/diff/42/");
    assert!(parse_creatediff_response(&json!({"diffid": 1})).is_err());
    assert!(parse_creatediff_response(&json!({"uri": "u"})).is_err());
    let legacy = parse_creatediff_response(&json!({"diffid": 1, "uri": "u"})).unwrap();
    assert_eq!(legacy.phid, None);
}

#[test]
fn parses_revision_responses() {
    let revision = parse_revision_response(&json!({"revisionid": 7, "uri": "u"})).unwrap();
    assert_eq!(revision.revision_id, 7);
    assert!(parse_revision_response(&json!({"uri": "u"})).is_err());
    assert!(parse_revision_response(&json!({"revisionid": 7})).is_err());
    assert_eq!(
        parse_revision_edit_response(&json!({"object": {"id": 11}})).unwrap(),
        11
    );
    assert_eq!(
        parse_revision_edit_response(&json!({"object": {}})).unwrap_err(),
        "Expected a revision ID to be returned by \"differential.revision.edit\"."
    );
    assert_eq!(revision_edit_uri_path(11), "/D11");
}

#[test]
fn extracts_local_commit_hashes() {
    let from_diffs = local_commit_hashes_from_querydiffs(&json!([
        {"properties": {"local:commits": [{"commit": "aaa"}, {"commit": "bbb"}, {}]}}
    ]));
    assert_eq!(from_diffs, vec!["aaa".to_string(), "bbb".to_string()]);
    assert!(local_commit_hashes_from_querydiffs(&json!([])).is_empty());
    assert!(local_commit_hashes_from_querydiffs(&json!([{}])).is_empty());

    let from_search = local_commit_hashes_from_diff_search(&json!({"data": [{
        "attachments": {"commits": {"commits": [
            {"identifier": "aaa"}, {"identifier": "bbb"},
        ]}}
    }]}))
    .unwrap();
    assert_eq!(from_search, vec!["aaa".to_string(), "bbb".to_string()]);
    assert!(local_commit_hashes_from_diff_search(&json!({"data": [{}]})).is_none());
    assert!(
        local_commit_hashes_from_diff_search(&json!({"data": [{
            "attachments": {"commits": {"commits": null}}
        }]}))
        .is_none()
    );
    assert!(local_commit_hashes_from_diff_search(&json!({"data": []})).is_none());
}

#[test]
fn parses_commit_message_parse_responses() {
    let parsed = parse_commit_message_parse_response(&json!({
        "fields": {"title": "T"},
        "transactions": [{"type": "title"}],
    }))
    .unwrap();
    assert_eq!(parsed.fields, json!({"title": "T"}));
    assert_eq!(parsed.transactions, Some(json!([{"type": "title"}])));
    let legacy = parse_commit_message_parse_response(&json!({"fields": {}})).unwrap();
    assert_eq!(legacy.transactions, None);
    assert!(
        parse_commit_message_parse_response(&json!({
            "fields": {},
            "errors": ["bad reviewer", "bad title"],
        }))
        .unwrap_err()
        .contains("bad reviewer")
    );
    assert!(parse_commit_message_parse_response(&json!({"fields": {}, "errors": []})).is_ok());
}

#[test]
fn finds_away_reviewers() {
    let users = json!([
        {"userName": "amy", "realName": "Amy A", "currentStatus": "away", "currentStatusUntil": 5},
        {"userName": "bo", "realName": "", "currentStatus": "away"},
        {"userName": "cy", "realName": "Cy", "currentStatus": "online"},
    ]);
    let away = away_reviewers(&users);
    assert_eq!(away.len(), 2);
    assert_eq!(away[0].name, "amy (Amy A)");
    assert_eq!(away[0].until, json!(5));
    assert_eq!(away[1].name, "bo");
    let mapped = json!({"PHID-USER-1": {
        "userName": "amy", "realName": "", "currentStatus": "away",
    }});
    assert_eq!(away_reviewers(&mapped).len(), 1);
    assert!(away_reviewers(&json!([])).is_empty());
    assert!(away_reviewers(&json!(7)).is_empty());
}

#[test]
fn enforces_nonempty_changes() {
    assert!(require_changes(1).is_ok());
    assert_eq!(
        require_changes(0).unwrap_err(),
        "There are no changes to generate a diff from!"
    );
}

#[test]
fn checks_draft_flags() {
    assert_eq!(check_draft_update(false, "D7", false), DraftUpdate::Proceed);
    let update = check_draft_update(true, "D7", true);
    assert!(matches!(update, DraftUpdate::WarnAlreadyDraft(_)));
    let debug = format!("{update:?}");
    assert!(debug.contains(r#"\"D7\""#), "{debug}");
    assert!(debug.contains("already a draft"), "{debug}");
    let update = check_draft_update(true, "D7", false);
    assert!(matches!(update, DraftUpdate::AbortPublished(_)));
    let debug = format!("{update:?}");
    let refused = debug.contains("You can not turn a revision back into a draft");
    assert!(refused, "{debug}");
    assert!(check_draft_create(false, false).is_ok());
    assert!(check_draft_create(true, true).is_ok());
    let err = check_draft_create(true, false).unwrap_err();
    assert!(err.contains("too old to support draft revisions"), "{err}");
}

#[test]
fn formats_ownership_and_revision_messages() {
    assert!(needs_ownership_confirm("PHID-USER-1", "PHID-USER-2"));
    assert!(!needs_ownership_confirm("PHID-USER-1", "PHID-USER-1"));
    let prompt = ownership_prompt(7, "Some title");
    assert!(prompt.contains("D7"), "{prompt}");
    assert!(prompt.contains("Commandeer"), "{prompt}");
    assert!(prompt.contains("Update this revision anyway?"), "{prompt}");
    assert_eq!(
        OWNERSHIP_ABORT,
        "Aborted update of revision: You are not the owner."
    );
    assert_eq!(missing_revision_message(7), "There is no revision D7.");
    assert_eq!(
        unknown_revision_message("99"),
        "Revision '99' does not exist!"
    );
}

#[test]
fn seam_create_diff_parses_result() {
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "differential.creatediff");
        assert_eq!(params["creationMethod"], json!("arc"));
        Ok(json!({"diffid": 42, "phid": "PHID-DIFF-1", "uri": "u"}))
    };
    let created = create_diff(&call, &diff_spec_json(&[], "none", "none", &spec_input())).unwrap();
    assert_eq!(created.diff_id, 42);
    let failing = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("boom".to_string())
    };
    assert!(create_diff(&failing, &json!({})).is_err());
}

#[test]
fn seam_update_and_legacy_create_parse_results() {
    let call = |method: &str, _: &serde_json::Value| {
        assert!(method == "differential.updaterevision" || method == "differential.createrevision");
        Ok(json!({"revisionid": 7, "uri": "uri"}))
    };
    let updated = update_revision(&call, &json!({})).unwrap();
    assert_eq!(updated.revision_id, 7);
    let revision = create_revision_legacy(&call, &json!({})).unwrap();
    assert_eq!(revision.uri, "uri");
}

#[test]
fn seam_revision_edit_returns_object_id() {
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "differential.revision.edit");
        assert_eq!(params["transactions"].as_array().unwrap().len(), 2);
        Ok(json!({"object": {"id": 12}}))
    };
    let params = revision_edit_params(&[json!({"type": "title"})], Some("PHID-DIFF-1"), false);
    assert_eq!(create_revision_via_edit(&call, &params).unwrap(), 12);
}

#[test]
fn seam_fetch_commit_message_requires_string() {
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "differential.getcommitmessage");
        Ok(json!("the corpus"))
    };
    assert_eq!(
        fetch_commit_message(&call, &json!({})).unwrap(),
        "the corpus"
    );
    let bad = |_: &str, _: &serde_json::Value| Ok(json!({"nope": true}));
    assert!(fetch_commit_message(&bad, &json!({})).is_err());
}

#[test]
fn seam_set_diff_properties_calls_each() {
    use std::cell::RefCell;
    let seen = RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "differential.setdiffproperty");
        seen.borrow_mut()
            .push(params["name"].as_str().unwrap().to_string());
        Ok(json!(true))
    };
    let props = [
        DiffProperty {
            name: PROP_LINT.to_string(),
            data: "[]".to_string(),
        },
        DiffProperty {
            name: PROP_UNIT.to_string(),
            data: "{}".to_string(),
        },
    ];
    set_diff_properties(&call, 42, &props).unwrap();
    assert_eq!(
        *seen.borrow(),
        vec!["arc:lint".to_string(), "arc:unit".to_string()]
    );
}

#[test]
fn seam_query_revision_row_returns_head() {
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "differential.query");
        assert_eq!(params["ids"], json!([7]));
        Ok(json!([{"id": 7}]))
    };
    assert_eq!(
        query_revision_row(&call, 7).unwrap(),
        Some(json!({"id": 7}))
    );
    let empty = |_: &str, _: &serde_json::Value| Ok(json!([]));
    assert_eq!(query_revision_row(&empty, 7).unwrap(), None);
}

#[test]
fn active_hashes_prefers_search_then_falls_back() {
    let modern = |method: &str, _: &serde_json::Value| {
        if method == "differential.revision.search" {
            Ok(json!({"data": [{"phid": "PHID-DREV-1"}]}))
        } else {
            assert_eq!(method, "differential.diff.search");
            Ok(json!({"data": [{
                "attachments": {"commits": {"commits": [{"identifier": "aaa"}]}}
            }]}))
        }
    };
    assert_eq!(
        active_diff_hashes(&modern, 7).unwrap(),
        vec!["aaa".to_string()]
    );

    let failing_search = |method: &str, _: &serde_json::Value| {
        if method == "differential.revision.search" {
            Err("old server".to_string())
        } else {
            assert_eq!(method, "differential.querydiffs");
            Ok(json!([{"properties": {"local:commits": [{"commit": "bbb"}]}}]))
        }
    };
    assert_eq!(
        active_diff_hashes(&failing_search, 7).unwrap(),
        vec!["bbb".to_string()]
    );

    let missing_attachment = |method: &str, _: &serde_json::Value| {
        if method == "differential.revision.search" {
            Ok(json!({"data": [{"phid": "PHID-DREV-1"}]}))
        } else if method == "differential.diff.search" {
            Ok(json!({"data": [{}]}))
        } else {
            assert_eq!(method, "differential.querydiffs");
            Ok(json!([{"properties": {"local:commits": [{"commit": "ccc"}]}}]))
        }
    };
    assert_eq!(
        active_diff_hashes(&missing_attachment, 7).unwrap(),
        vec!["ccc".to_string()]
    );
}

#[test]
fn active_hashes_search_misses_fall_back_to_querydiffs() {
    // Every search-miss shape (empty rows, scalar rows, mapped rows,
    // rows without a PHID) misses the search path and falls back to
    // `querydiffs`, which answers empty here.
    for data in [
        json!([]),
        json!(7),
        json!({"k": {"x": 1}}),
        json!([{"x": 1}]),
    ] {
        let call = |method: &str, _: &serde_json::Value| {
            if method == "differential.revision.search" {
                Ok(json!({"data": data.clone()}))
            } else {
                assert_eq!(method, "differential.querydiffs");
                Ok(json!([]))
            }
        };
        assert!(active_diff_hashes(&call, 7).unwrap().is_empty());
    }
}

#[test]
fn submit_flows_chain_creatediff_and_revision_calls() {
    use std::cell::RefCell;
    let methods = RefCell::new(Vec::new());
    let call = |method: &str, _: &serde_json::Value| {
        methods.borrow_mut().push(method.to_string());
        if method == "differential.creatediff" {
            Ok(json!({"diffid": 42, "phid": "PHID-DIFF-1", "uri": "u"}))
        } else if method == "differential.updaterevision" || method == "differential.createrevision"
        {
            Ok(json!({"revisionid": 7, "uri": "r"}))
        } else {
            assert_eq!(method, "differential.revision.edit");
            Ok(json!({"object": {"id": 8}}))
        }
    };
    let spec = diff_spec_json(&[sample_change()], "okay", "okay", &spec_input());
    let (created, updated) =
        submit_update_revision(&call, &spec, 7, json!({"title": "T"}), "msg").unwrap();
    assert_eq!((created.diff_id, updated.revision_id), (42, 7));
    let (_, revision) = submit_create_revision_legacy(&call, &spec, json!({"title": "T"})).unwrap();
    assert_eq!(revision.revision_id, 7);
    let (_, id) =
        submit_create_revision_via_edit(&call, &spec, &[json!({"type": "title"})], true).unwrap();
    assert_eq!(id, 8);
    assert_eq!(
        *methods.borrow(),
        vec![
            "differential.creatediff",
            "differential.updaterevision",
            "differential.creatediff",
            "differential.createrevision",
            "differential.creatediff",
            "differential.revision.edit",
        ]
    );
}

#[test]
fn submit_via_edit_sends_null_phid_when_server_omits_it() {
    let call = |method: &str, params: &serde_json::Value| {
        if method == "differential.creatediff" {
            Ok(json!({"diffid": 1, "uri": "u"}))
        } else {
            assert_eq!(method, "differential.revision.edit");
            assert_eq!(params["transactions"][0]["value"], serde_json::Value::Null);
            Ok(json!({"object": {"id": 3}}))
        }
    };
    let (_, id) = submit_create_revision_via_edit(&call, &json!({}), &[], false).unwrap();
    assert_eq!(id, 3);
}

#[test]
fn loose_int_coercion_accepts_numbers_and_strings() {
    assert_eq!(loose_i64(&json!(3)), Some(3));
    assert_eq!(loose_i64(&json!("3")), Some(3));
    assert_eq!(loose_i64(&json!(-1)), Some(-1));
    assert_eq!(loose_i64(&json!("-1")), Some(-1));
    assert_eq!(loose_i64(&json!("bogus")), None);
    assert_eq!(loose_i64(&json!(null)), None);
    assert_eq!(loose_u64(&json!(1633)), Some(1633));
    assert_eq!(loose_u64(&json!("1633")), Some(1633));
    assert_eq!(loose_u64(&json!("-1")), None);
    assert_eq!(loose_u64(&json!("bogus")), None);
}

#[test]
fn d20_draft_create_uses_ceb_server_wording() {
    let err = check_draft_create(true, false).unwrap_err();
    assert!(
        err.contains("the version of Phabricator on the server"),
        "{err}"
    );
    assert!(check_draft_create(false, false).is_ok());
    assert!(check_draft_create(true, true).is_ok());
}

#[test]
fn d18_empty_metadata_encodes_as_list() {
    let change = DiffChange::default();
    assert_eq!(change_json(&change)["metadata"], json!([]));
}

#[test]
fn d12_modern_dictionaries_drop_nulls_and_move_details() {
    assert_eq!(
        modern_lint_dictionary(&json!({"severity": "error", "line": null})),
        json!({"severity": "error"})
    );
    assert_eq!(
        modern_unit_dictionary(&json!({"name": "t", "userData": "out", "x": null})),
        json!({"name": "t", "details": "out"})
    );
    // Empty `userData` vanishes without a `details` key.
    assert_eq!(
        modern_unit_dictionary(&json!({"name": "t", "userData": ""})),
        json!({"name": "t"})
    );
    // Non-objects pass through untouched.
    assert_eq!(modern_lint_dictionary(&json!(7)), json!(7));
    assert_eq!(modern_unit_dictionary(&json!(7)), json!(7));
}

#[test]
fn d13_staging_push_refs() {
    let refs = staging_push_refs("s", "phab", 7, "base1", "head1", false);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0]["ref"], json!("refs/tags/phab/base/7"));
    assert_eq!(refs[0]["type"], json!("base"));
    assert_eq!(refs[0]["commit"], json!("base1"));
    assert_eq!(refs[0]["remote"], json!({"uri": "s"}));
    assert_eq!(refs[1]["ref"], json!("refs/tags/phab/diff/7"));
    assert_eq!(refs[1]["commit"], json!("head1"));
    // The magic root commit skips the base tag.
    let refs = staging_push_refs(
        "s",
        "phab",
        7,
        crate::git::GIT_MAGIC_ROOT_COMMIT,
        "head1",
        false,
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["type"], json!("diff"));
    // LFS pushes the base commit to the diff tag.
    let refs = staging_push_refs("s", "phab", 7, "base1", "head1", true);
    assert_eq!(refs[1]["commit"], json!("base1"));
    // The payload round-trips (slashes come back escaped, like PHP).
    let payload: serde_json::Value =
        serde_json::from_str(&staging_property_data(STAGING_PUSHED, &refs)).unwrap();
    assert_eq!(payload["status"], json!("pushed"));
    assert_eq!(payload["refs"], serde_json::Value::Array(refs));
}

#[test]
fn d11_autotargets_send_modernized_messages() {
    use std::cell::RefCell;
    let sent = RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == METHOD_QUERY_AUTOTARGETS {
            assert_eq!(
                params["targetKeys"],
                json!([AUTOTARGET_LINT_KEY, AUTOTARGET_UNIT_KEY])
            );
            return Ok(
                json!({"targetMap": {"arcanist.lint": "PHID-L", "arcanist.unit": "PHID-U"}}),
            );
        }
        assert_eq!(method, METHOD_SENDMESSAGE);
        sent.borrow_mut().push(params.clone());
        Ok(json!(null))
    };
    assert!(update_autotargets(
        &call,
        "PHID-DIFF-1",
        &[json!({"severity": "warning", "dropped": null})],
        &[json!({"name": "t", "userData": "log"})],
        0,
    ));
    let sent = sent.borrow();
    assert_eq!(sent[0]["lint"], json!([{"severity": "warning"}]));
    assert_eq!(sent[0]["type"], json!("fail"));
    assert_eq!(sent[1]["unit"], json!([{"name": "t", "details": "log"}]));
    assert_eq!(sent[1]["type"], json!("pass"));
}
