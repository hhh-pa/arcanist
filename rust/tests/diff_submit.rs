//! Bundle D2 integration tests: diff submit/update flows over a canned
//! conduit seam plus pre-land gate checks over fixture revision JSON,
//! through the wired library.

use std::cell::RefCell;
use std::collections::HashMap;

use arc_rs::{diff_submit, land_checks};

/* G1 full-state integration tests: revision flow, away reviewers, autotargets. */

#[test]
fn g1_revision_list_and_several_error() {
    let list = diff_submit::render_revision_list(&[(1, "First"), (22, "Second")]);
    assert_eq!(list, "     - D1: First\n     - D22: Second\n");
    let err = diff_submit::several_revisions_error(&list);
    assert!(
        err.starts_with("There are several revisions which match the working copy:"),
        "{err}"
    );
    assert!(
        err.ends_with("Use '--update' to choose one, or '--create' to create a new revision."),
        "{err}"
    );
    assert_eq!(
        diff_submit::UPDATE_PARAM_MUST_BE_NUMBER,
        "Parameter to --update must be a Differential Revision number."
    );
    assert!(diff_submit::check_revision_ownership("A", "A", 5, "T").is_none());
    let prompt = diff_submit::check_revision_ownership("A", "B", 5, "T").unwrap();
    assert!(prompt.ends_with("Update this revision anyway?"), "{prompt}");
    assert_eq!(
        diff_submit::CREATED_DIFF_LEAD,
        "Created a new Differential diff:"
    );
    assert_eq!(diff_submit::INCLUDED_CHANGES_LABEL, "Included changes:");
}

#[test]
fn g1_away_reviewers_block_shapes() {
    use serde_json::json;
    assert_eq!(diff_submit::format_phab_date(0), "Thursday, Jan 1 1970");
    let aways = vec![
        diff_submit::AwayReviewer {
            name: "a".to_string(),
            until: json!(100),
        },
        diff_submit::AwayReviewer {
            name: "b".to_string(),
            until: json!(200),
        },
    ];
    assert!(diff_submit::away_reviewers_block(&[], 0).is_none());
    let all = diff_submit::away_reviewers_block(&aways, 2).unwrap();
    assert!(
        all.starts_with("All reviewers are away until Thursday, Jan 1 1970:\n\n"),
        "{all}"
    );
    assert!(
        all.contains("      b (until Thursday, Jan 1 1970)\n"),
        "{all}"
    );
    let some = diff_submit::away_reviewers_block(&aways[..1], 3).unwrap();
    assert!(
        some.starts_with("Some reviewers are currently away:\n\n"),
        "{some}"
    );
    assert_eq!(
        diff_submit::CONTINUE_DESPITE_AWAY,
        "Continue even though reviewers are unavailable?"
    );
    assert_eq!(
        diff_submit::SPECIFY_AVAILABLE_REVIEWERS,
        "Specify available reviewers and retry."
    );
}

#[test]
fn g1_update_autotargets_over_canned_seam() {
    use serde_json::json;
    let canned = Canned::new().with(
        "harbormaster.queryautotargets",
        r#"{"targetMap": {"arcanist.lint": "PHID-L", "arcanist.unit": "PHID-U"}}"#,
    );
    let call = |method: &str, params: &Value| canned.call(method, params);
    // `sendmessage` has no canned response: the first send fails.
    assert!(!diff_submit::update_autotargets(
        &call,
        "PHID-D",
        &[],
        &[],
        diff_submit::UNIT_RESULT_OKAY
    ));
    assert_eq!(
        canned.methods(),
        vec![
            "harbormaster.queryautotargets".to_string(),
            "harbormaster.sendmessage".to_string()
        ]
    );

    let canned = Canned::new()
        .with(
            "harbormaster.queryautotargets",
            r#"{"targetMap": {"arcanist.lint": "PHID-L", "arcanist.unit": "PHID-U"}}"#,
        )
        .with("harbormaster.sendmessage", "null");
    let call = |method: &str, params: &Value| canned.call(method, params);
    let ok = diff_submit::update_autotargets(
        &call,
        "PHID-D",
        &[json!({"severity": "error"})],
        &[json!({"name": "t"})],
        diff_submit::UNIT_RESULT_FAIL,
    );
    assert!(ok);
    let sends = canned.params_for("harbormaster.sendmessage");
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0]["type"], json!("fail"));
    assert_eq!(sends[1]["type"], json!("fail"));
    assert_eq!(sends[0]["buildTargetPHID"], json!("PHID-L"));

    let down = Canned::new().failing("harbormaster.queryautotargets", "down");
    let call = |method: &str, params: &Value| down.call(method, params);
    assert!(!diff_submit::update_autotargets(
        &call,
        "PHID-D",
        &[],
        &[],
        diff_submit::UNIT_RESULT_OKAY
    ));
}

#[test]
fn g1_staging_property_data_orders_keys() {
    use serde_json::json;
    assert_eq!(
        diff_submit::staging_property_data("pushed", &[json!({"ref": "r"})]),
        "{\"status\":\"pushed\",\"refs\":[{\"ref\":\"r\"}]}"
    );
}
use serde_json::Value;

/// Canned conduit seam: records every call and replays fixed responses.
#[derive(Default)]
struct Canned {
    calls: RefCell<Vec<(String, Value)>>,
    responses: HashMap<String, Value>,
    errors: HashMap<String, String>,
}

impl Canned {
    fn new() -> Self {
        Self::default()
    }

    fn with(mut self, method: &str, response: &str) -> Self {
        let value: Value = serde_json::from_str(response).unwrap();
        self.responses.insert(method.to_string(), value);
        self
    }

    fn failing(mut self, method: &str, error: &str) -> Self {
        self.errors.insert(method.to_string(), error.to_string());
        self
    }

    fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        self.calls
            .borrow_mut()
            .push((method.to_string(), params.clone()));
        if let Some(error) = self.errors.get(method) {
            return Err(error.clone());
        }
        self.responses
            .get(method)
            .cloned()
            .ok_or_else(|| format!("no canned response for {method}"))
    }

    fn methods(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|(m, _)| m.clone()).collect()
    }

    fn params_for(&self, method: &str) -> Vec<Value> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|(m, p)| if m == method { Some(p.clone()) } else { None })
            .collect()
    }
}

/// Canned Differential write-call responses (`creatediff`, `edit`, ...).
fn fixture_mutation(name: &str) -> Option<&'static str> {
    match name {
        "creatediff" => Some(include_str!("fixtures/diffsubmit/creatediff.json")),
        "updaterevision" => Some(include_str!("fixtures/diffsubmit/updaterevision.json")),
        "revisionedit" => Some(include_str!("fixtures/diffsubmit/revisionedit.json")),
        "createrevision" => Some(include_str!("fixtures/diffsubmit/createrevision.json")),
        "setdiffproperty" => Some(include_str!("fixtures/diffsubmit/setdiffproperty.json")),
        _ => None,
    }
}

/// Canned Differential read-call responses (`query`, `search`, `parse`, ...).
fn fixture_lookup(name: &str) -> Option<&'static str> {
    match name {
        "commitmessage" => Some(include_str!("fixtures/diffsubmit/commitmessage.json")),
        "query" => Some(include_str!("fixtures/diffsubmit/query.json")),
        "userquery" => Some(include_str!("fixtures/diffsubmit/userquery.json")),
        "revisionsearch" => Some(include_str!("fixtures/diffsubmit/revisionsearch.json")),
        "diffsearch" => Some(include_str!("fixtures/diffsubmit/diffsearch.json")),
        "diffsearchempty" => Some(include_str!("fixtures/diffsubmit/diffsearchempty.json")),
        "querydiffs" => Some(include_str!("fixtures/diffsubmit/querydiffs.json")),
        "parsecommit" => Some(include_str!("fixtures/diffsubmit/parsecommit.json")),
        "parsecommitbad" => Some(include_str!("fixtures/diffsubmit/parsecommitbad.json")),
        _ => None,
    }
}

/// Canned pre-land gate inputs (revision states, parents, builds).
fn fixture_gate(name: &str) -> Option<&'static str> {
    match name {
        "landaccepted" => Some(include_str!("fixtures/diffsubmit/landaccepted.json")),
        "landplanned" => Some(include_str!("fixtures/diffsubmit/landplanned.json")),
        "parentopen" => Some(include_str!("fixtures/diffsubmit/parentopen.json")),
        "parentclosed" => Some(include_str!("fixtures/diffsubmit/parentclosed.json")),
        "builds" => Some(include_str!("fixtures/diffsubmit/builds.json")),
        _ => None,
    }
}

fn fixture(name: &str) -> &'static str {
    if let Some(content) = fixture_mutation(name) {
        return content;
    }
    if let Some(content) = fixture_lookup(name) {
        return content;
    }
    if let Some(content) = fixture_gate(name) {
        return content;
    }
    panic!("unknown fixture {name}")
}

fn json_fixture(name: &str) -> Value {
    serde_json::from_str(fixture(name)).unwrap()
}

fn sample_spec() -> Value {
    let mut change = diff_submit::DiffChange::new_text("old.txt", "new.txt");
    change.hunks.push(diff_submit::DiffHunk {
        old_offset: 1,
        new_offset: 1,
        old_length: 1,
        new_length: 1,
        add_lines: 1,
        del_lines: 1,
        corpus: "@@ -1 +1 @@\n".to_string(),
        ..diff_submit::DiffHunk::default()
    });
    let input = diff_submit::DiffSpecInput {
        source_machine: "devbox".to_string(),
        source_path: Some("/repo".to_string()),
        branch: Some("feature".to_string()),
        bookmark: None,
        source_control_system: Some("git".to_string()),
        source_control_path: Some("/repo".to_string()),
        source_control_base_revision: Some("abc123".to_string()),
        repository_phid: None,
    };
    diff_submit::diff_spec_json(
        &[change],
        diff_submit::lint_status(diff_submit::LINT_RESULT_OKAY),
        diff_submit::unit_status(diff_submit::UNIT_RESULT_UNSOUND),
        &input,
    )
}

#[test]
fn update_flow_sends_creatediff_then_updaterevision() {
    let canned = Canned::new()
        .with("differential.creatediff", fixture("creatediff"))
        .with("differential.updaterevision", fixture("updaterevision"));
    let call = |method: &str, params: &Value| canned.call(method, params);
    let fields = json_fixture("parsecommit")["fields"].clone();
    let (created, updated) =
        diff_submit::submit_update_revision(&call, &sample_spec(), 123, fields, "take two")
            .unwrap();
    assert_eq!(created.diff_id, 42);
    assert_eq!(created.phid.as_deref(), Some("PHID-DIFF-aaaa"));
    assert_eq!(updated.revision_id, 123);
    assert_eq!(
        canned.methods(),
        vec!["differential.creatediff", "differential.updaterevision"]
    );
    let specs = canned.params_for("differential.creatediff");
    assert_eq!(specs[0]["unitStatus"], Value::String("warn".to_string()));
    assert_eq!(specs[0]["creationMethod"], Value::String("arc".to_string()));
    let updates = canned.params_for("differential.updaterevision");
    assert_eq!(updates[0]["id"], Value::from(123));
    assert_eq!(updates[0]["diffid"], Value::from(42));
    assert_eq!(updates[0]["message"], Value::String("take two".to_string()));
}

#[test]
fn create_flow_via_edit_appends_update_and_draft() {
    let canned = Canned::new()
        .with("differential.creatediff", fixture("creatediff"))
        .with("differential.revision.edit", fixture("revisionedit"));
    let call = |method: &str, params: &Value| canned.call(method, params);
    let base = json_fixture("parsecommit")["transactions"]
        .as_array()
        .unwrap()
        .clone();
    let (created, id) =
        diff_submit::submit_create_revision_via_edit(&call, &sample_spec(), &base, true).unwrap();
    assert_eq!(created.diff_id, 42);
    assert_eq!(id, 124);
    assert_eq!(diff_submit::revision_edit_uri_path(id), "/D124");
    let edits = canned.params_for("differential.revision.edit");
    let transactions = edits[0]["transactions"].as_array().unwrap();
    assert_eq!(transactions.len(), 4);
    assert_eq!(transactions[2]["type"], Value::String("update".to_string()));
    assert_eq!(
        transactions[2]["value"],
        Value::String("PHID-DIFF-aaaa".to_string())
    );
    assert_eq!(transactions[3]["type"], Value::String("draft".to_string()));
}

#[test]
fn create_flow_legacy_then_fetches_commit_message() {
    let canned = Canned::new()
        .with("differential.creatediff", fixture("creatediff"))
        .with("differential.createrevision", fixture("createrevision"))
        .with("differential.getcommitmessage", fixture("commitmessage"));
    let call = |method: &str, params: &Value| canned.call(method, params);
    let fields = json_fixture("parsecommit")["fields"].clone();
    let (_, revision) =
        diff_submit::submit_create_revision_legacy(&call, &sample_spec(), fields).unwrap();
    assert_eq!(revision.revision_id, 125);
    let params = diff_submit::commit_message_after_create_params(revision.revision_id);
    let corpus = diff_submit::fetch_commit_message(&call, &params).unwrap();
    assert!(corpus.contains("Differential Revision:"), "{corpus}");
    let fetched = canned.params_for("differential.getcommitmessage");
    assert_eq!(fetched[0]["revision_id"], Value::from(125));
}

#[test]
fn diff_properties_write_each_name_in_order() {
    let canned = Canned::new().with("differential.setdiffproperty", fixture("setdiffproperty"));
    let call = |method: &str, params: &Value| canned.call(method, params);
    let properties = [
        diff_submit::DiffProperty {
            name: diff_submit::PROP_LINT.to_string(),
            data: "[]".to_string(),
        },
        diff_submit::DiffProperty {
            name: diff_submit::PROP_UNIT.to_string(),
            data: "{}".to_string(),
        },
        diff_submit::DiffProperty {
            name: diff_submit::PROP_LOCAL_COMMITS.to_string(),
            data: "[]".to_string(),
        },
        diff_submit::DiffProperty {
            name: diff_submit::PROP_ONTO.to_string(),
            data: "[]".to_string(),
        },
    ];
    diff_submit::set_diff_properties(&call, 42, &properties).unwrap();
    let writes = canned.params_for("differential.setdiffproperty");
    let names: Vec<&str> = writes
        .iter()
        .map(|params| params["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["arc:lint", "arc:unit", "local:commits", "arc:onto"]
    );
    assert!(
        writes
            .iter()
            .all(|params| params["diff_id"].as_i64() == Some(42))
    );
}

#[test]
fn ownership_and_parser_gates_use_fixtures() {
    let row = diff_submit::query_revision_row(
        &|method: &str, params: &Value| {
            assert_eq!(method, "differential.query");
            Canned::new()
                .with("differential.query", fixture("query"))
                .call(method, params)
        },
        123,
    )
    .unwrap()
    .unwrap();
    assert!(!diff_submit::needs_ownership_confirm(
        row["authorPHID"].as_str().unwrap(),
        "PHID-USER-owner",
    ));
    assert!(diff_submit::needs_ownership_confirm(
        row["authorPHID"].as_str().unwrap(),
        "PHID-USER-stranger",
    ));

    let parsed =
        diff_submit::parse_commit_message_parse_response(&json_fixture("parsecommit")).unwrap();
    assert_eq!(
        parsed.fields["title"],
        Value::String("Add widgets".to_string())
    );
    assert!(parsed.transactions.is_some());
    let bad = diff_submit::parse_commit_message_parse_response(&json_fixture("parsecommitbad"));
    assert!(bad.unwrap_err().contains("Unknown reviewer"));

    let away = diff_submit::away_reviewers(&json_fixture("userquery"));
    assert_eq!(away.len(), 1);
    assert_eq!(away[0].name, "rita (Rita Reviewer)");
}

#[test]
fn active_hashes_read_search_or_fall_back_to_querydiffs() {
    let modern = Canned::new()
        .with("differential.revision.search", fixture("revisionsearch"))
        .with("differential.diff.search", fixture("diffsearch"));
    let call = |method: &str, params: &Value| modern.call(method, params);
    assert_eq!(
        diff_submit::active_diff_hashes(&call, 123).unwrap(),
        vec!["aaa111".to_string(), "bbb222".to_string()]
    );
    let searched = modern.params_for("differential.diff.search");
    assert_eq!(
        searched[0]["constraints"]["revisionPHIDs"],
        serde_json::json!(["PHID-DREV-cccc"])
    );

    let legacy = Canned::new()
        .with("differential.revision.search", fixture("revisionsearch"))
        .with("differential.diff.search", fixture("diffsearchempty"))
        .with("differential.querydiffs", fixture("querydiffs"));
    let call = |method: &str, params: &Value| legacy.call(method, params);
    assert_eq!(
        diff_submit::active_diff_hashes(&call, 123).unwrap(),
        vec!["aaa111".to_string(), "bbb222".to_string()]
    );

    let failing = Canned::new()
        .failing("differential.revision.search", "old server")
        .with("differential.querydiffs", fixture("querydiffs"));
    let call = |method: &str, params: &Value| failing.call(method, params);
    assert_eq!(
        diff_submit::active_diff_hashes(&call, 123).unwrap(),
        vec!["aaa111".to_string(), "bbb222".to_string()]
    );
}

#[test]
fn accepted_revision_lands_quietly() {
    let revision = land_checks::parse_revision(&json_fixture("landaccepted")).unwrap();
    assert!(revision.is_status_accepted());
    let findings =
        land_checks::evaluate_gates(&[revision], "PHID-USER-owner", &[], &[], &[], Some(3));
    assert!(findings.is_empty());
}

#[test]
fn mixed_land_state_fires_every_gate_in_order() {
    let planned = land_checks::parse_revision(&json_fixture("landplanned")).unwrap();
    assert!(planned.is_status_changes_planned());
    let accepted = land_checks::parse_revision(&json_fixture("landaccepted")).unwrap();
    let revisions = vec![planned, accepted];

    let open = land_checks::parent_link(0, &json_fixture("parentopen")).unwrap();
    let closed = land_checks::parent_link(1, &json_fixture("parentclosed")).unwrap();
    let links = vec![open, closed];

    let entries = json_fixture("builds");
    let builds: Vec<land_checks::BuildView> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let plan = entry.get("plan").filter(|plan| !plan.is_null());
            land_checks::parse_build(&entry["build"], plan, entry["revision"].as_str().unwrap())
        })
        .collect();

    let findings = land_checks::evaluate_gates(
        &revisions,
        "PHID-USER-stranger",
        &links,
        &["PHID-DREV-cccc", "PHID-DREV-dddd"],
        &builds,
        Some(60),
    );
    let prompts: Vec<&str> = findings.iter().map(|finding| finding.prompt).collect();
    assert_eq!(
        prompts,
        vec![
            "arc.land.unauthored",
            "arc.land.changes-planned",
            "arc.land.open-parents",
            "arc.land.failed-builds",
            "arc.land.large-working-set",
        ]
    );
    let failed = findings
        .iter()
        .find(|finding| finding.prompt == "arc.land.failed-builds")
        .unwrap();
    assert_eq!(failed.count, 1);
    assert!(
        failed.query.contains("despite ongoing and failed builds"),
        "{}",
        failed.query
    );
    let parents = findings
        .iter()
        .find(|finding| finding.prompt == "arc.land.open-parents")
        .unwrap();
    assert_eq!(parents.count, 1);
}
