use super::*;
use serde_json::json;

fn modern(status: &str, name: &str, closed: bool, author: &str) -> serde_json::Value {
    json!({
        "id": 7,
        "phid": "PHID-DREV-7",
        "fields": {
            "title": "Some work",
            "authorPHID": author,
            "diffPHID": "PHID-DIFF-1",
            "uri": "/D7",
            "status": {"value": status, "name": name, "closed": closed},
        },
    })
}

fn legacy(status: i64, name: &str, author: &str) -> serde_json::Value {
    json!({
        "id": 9,
        "phid": "PHID-DREV-9",
        "title": "Old work",
        "authorPHID": author,
        "status": status,
        "statusName": name,
    })
}

fn view(status: &str, author: &str) -> RevisionView {
    parse_revision(&modern(status, status, false, author)).unwrap()
}

#[test]
fn const_values_are_verbatim_from_php() {
    assert_eq!(
        [
            STATUS_NEEDS_REVIEW,
            STATUS_NEEDS_REVISION,
            STATUS_ACCEPTED,
            STATUS_PUBLISHED,
            STATUS_ABANDONED,
            STATUS_CHANGES_PLANNED,
        ],
        [
            "needs-review",
            "needs-revision",
            "accepted",
            "published",
            "abandoned",
            "changes-planned",
        ]
    );
    assert_eq!(
        [
            PROMPT_UNAUTHORED,
            PROMPT_CHANGES_PLANNED,
            PROMPT_PUBLISHED,
            PROMPT_NOT_ACCEPTED,
            PROMPT_OPEN_PARENTS,
            PROMPT_FAILED_BUILDS,
            PROMPT_ONGOING_BUILDS,
            PROMPT_LARGE_WORKING_SET,
        ],
        [
            "arc.land.unauthored",
            "arc.land.changes-planned",
            "arc.land.published",
            "arc.land.not-accepted",
            "arc.land.open-parents",
            "arc.land.failed-builds",
            "arc.land.ongoing-builds",
            "arc.land.large-working-set",
        ]
    );
    assert_eq!(PLAN_BEHAVIOR_KEY, "arc-land");
    assert_eq!(
        [PLAN_ALWAYS, PLAN_BUILDING, PLAN_COMPLETE, PLAN_NEVER],
        ["always", "building", "complete", "never"]
    );
    assert_eq!(BUILD_PASSED, "passed");
    assert_eq!(LARGE_WORKING_SET_LIMIT, 50);
}

#[test]
fn parses_modern_revision_rows() {
    let revision = parse_revision(&modern("accepted", "Accepted", false, "PHID-USER-1")).unwrap();
    assert_eq!(revision.id, 7);
    assert_eq!(revision.monogram, "D7");
    assert_eq!(revision.name, "Some work");
    assert_eq!(revision.status, "accepted");
    assert_eq!(revision.status_name, "Accepted");
    assert!(!revision.is_closed());
    assert!(revision.is_status_accepted());
    assert_eq!(revision.author_phid, "PHID-USER-1");
    assert_eq!(revision.phid, "PHID-DREV-7");
    assert_eq!(revision.diff_phid.as_deref(), Some("PHID-DIFF-1"));
}

#[test]
fn parses_legacy_rows_with_status_map() {
    let cases = [
        (0, "Needs Review", "needs-review", false),
        (1, "Needs Revision", "needs-revision", false),
        (2, "Accepted", "accepted", false),
        (3, "Closed", "published", true),
        (4, "Abandoned", "abandoned", true),
        (5, "Changes Planned", "changes-planned", false),
    ];
    for (code, name, value, closed) in cases {
        let revision = parse_revision(&legacy(code, name, "PHID-USER-1")).unwrap();
        assert_eq!(revision.status, value, "code {code}");
        assert_eq!(revision.status_name, name);
        assert_eq!(revision.is_closed(), closed, "code {code}");
        assert_eq!(revision.diff_phid, None);
    }
    let unknown = parse_revision(&legacy(99, "Weird", "PHID-USER-1")).unwrap();
    assert_eq!(unknown.status, "");
    assert!(parse_revision(&json!({"title": "no id"})).is_err());
}

#[test]
fn status_predicates_match_php() {
    assert!(view("changes-planned", "a").is_status_changes_planned());
    assert!(!view("accepted", "a").is_status_changes_planned());
    assert!(view("published", "a").is_status_published());
    assert!(!view("accepted", "a").is_status_published());
    assert!(view("accepted", "a").is_status_accepted());
    assert!(!view("needs-review", "a").is_status_accepted());
}

#[test]
fn partitions_statuses_in_php_order() {
    let revisions = [
        view("accepted", "a"),
        view("changes-planned", "a"),
        view("published", "a"),
        view("needs-review", "a"),
        view("abandoned", "a"),
    ];
    let partition = partition_by_status(&revisions);
    assert_eq!(partition.planned, vec![1]);
    assert_eq!(partition.published, vec![2]);
    assert_eq!(partition.not_accepted, vec![3, 4]);
    assert!(partition_by_status(&[]).planned.is_empty());
}

#[test]
fn finds_unauthored_revisions() {
    let revisions = [
        view("accepted", "PHID-USER-1"),
        view("accepted", "PHID-USER-2"),
    ];
    assert_eq!(unauthored_indexes(&revisions, "PHID-USER-1"), vec![1]);
    assert!(unauthored_indexes(&revisions, "PHID-USER-2").contains(&0));
}

#[test]
fn groups_open_parents_skipping_landing_and_closed() {
    let open = modern("needs-review", "Needs Review", false, "a");
    let closed = modern("published", "Closed", true, "a");
    let links = [
        parent_link(0, &open).unwrap(),
        parent_link(1, &open).unwrap(),
        parent_link(2, &closed).unwrap(),
    ];
    assert!(parent_link(0, &json!({})).is_err());
    let groups = open_parent_groups(&links, &[]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].children, vec![0, 1]);
    assert_eq!(groups[0].parent.phid, "PHID-DREV-7");
    let skipped = open_parent_groups(&links, &["PHID-DREV-7"]);
    assert!(skipped.is_empty());
}

#[test]
fn parses_builds_with_behavior_default() {
    let build = json!({"fields": {"buildStatus": {"value": "failed"}}});
    let plan = json!({"fields": {"behaviors": {"arc-land": {"value": "never"}}}});
    let parsed = parse_build(&build, Some(&plan), "PHID-DREV-7");
    assert_eq!(parsed.status, "failed");
    assert_eq!(parsed.plan_behavior.as_deref(), Some("never"));
    assert_eq!(parsed.revision_phid, "PHID-DREV-7");
    let defaulted = parse_build(&build, Some(&json!({"fields": {}})), "r");
    assert_eq!(defaulted.plan_behavior.as_deref(), Some("always"));
    let planless = parse_build(&build, None, "r");
    assert_eq!(planless.plan_behavior, None);
    let missing = parse_build(&json!({}), Some(&json!({})), "r");
    assert_eq!(missing.status, "");
}

#[test]
fn build_completion_matches_php() {
    for status in ["passed", "failed", "aborted", "error", "deadlocked"] {
        assert!(is_build_complete(status), "{status}");
    }
    for status in ["building", "pending", "", "unknown"] {
        assert!(!is_build_complete(status), "{status}");
    }
    assert!(is_build_passed("passed"));
    assert!(!is_build_passed("failed"));
}

#[test]
fn classifies_builds_with_plan_filters() {
    let mk = |status: &str, behavior: Option<&str>, revision: &str| BuildView {
        status: status.to_string(),
        plan_behavior: behavior.map(str::to_string),
        revision_phid: revision.to_string(),
    };
    let builds = [
        mk("passed", Some("always"), "r1"),
        mk("failed", None, "r1"),
        mk("failed", Some("never"), "r1"),
        mk("building", Some("complete"), "r1"),
        mk("failed", Some("building"), "r1"),
        mk("building", Some("building"), "r1"),
        mk("failed", Some("complete"), "r1"),
        mk("failed", Some("always"), "r2"),
    ];
    let verdict = classify_builds(&builds);
    assert_eq!(verdict.failures, 2);
    assert_eq!(verdict.ongoing, 1);
    assert_eq!(verdict.problems, vec![5, 6, 7]);
    assert_eq!(verdict.affected_revisions, 2);
    assert_eq!(classify_builds(&[]), BuildVerdict::default());
}

#[test]
fn build_prompt_key_prefers_failures() {
    assert_eq!(build_prompt_key(true, true), Some(PROMPT_FAILED_BUILDS));
    assert_eq!(build_prompt_key(true, false), Some(PROMPT_FAILED_BUILDS));
    assert_eq!(build_prompt_key(false, true), Some(PROMPT_ONGOING_BUILDS));
    assert_eq!(build_prompt_key(false, false), None);
}

#[test]
fn gates_report_verbatim_prompts_and_queries() {
    let revisions = [
        view("accepted", "PHID-USER-1"),
        view("accepted", "PHID-USER-2"),
    ];
    let finding = unauthored_gate(&revisions, "PHID-USER-1").unwrap();
    assert_eq!(finding.prompt, "arc.land.unauthored");
    assert_eq!(finding.title, "NOT REVISION AUTHOR");
    assert_eq!(finding.query, "Land revisions you are not the author of?");
    assert_eq!(finding.count, 1);
    assert!(unauthored_gate(&revisions[1..], "PHID-USER-2").is_none());

    let partition = partition_by_status(&[
        view("changes-planned", "a"),
        view("changes-planned", "a"),
        view("published", "a"),
        view("needs-review", "a"),
    ]);
    let planned = planned_gate(&partition).unwrap();
    assert_eq!(planned.prompt, "arc.land.changes-planned");
    assert_eq!(planned.title, "2 REVISION(S) HAVE CHANGES PLANNED");
    assert_eq!(planned.query, "Land 2 revision(s) with changes planned?");
    let published = published_gate(&partition).unwrap();
    assert_eq!(
        published.query,
        "Land 1 revision(s) that are already published?"
    );
    let not_accepted = not_accepted_gate(&partition).unwrap();
    assert_eq!(not_accepted.query, "Land 1 revision(s) in the wrong state?");
    let clean = partition_by_status(&[view("accepted", "a")]);
    assert!(planned_gate(&clean).is_none());
    assert!(published_gate(&clean).is_none());
    assert!(not_accepted_gate(&clean).is_none());
}

#[test]
fn open_parents_and_builds_gates() {
    let open = parent_link(0, &modern("needs-review", "Needs Review", false, "a")).unwrap();
    let groups = open_parent_groups(&[open], &[]);
    let finding = open_parents_gate(&groups).unwrap();
    assert_eq!(finding.prompt, "arc.land.open-parents");
    assert_eq!(finding.title, "1 OPEN PARENT REVISION(S)");
    assert_eq!(
        finding.query,
        "Land changes that depend on 1 open revision(s)?"
    );
    assert!(open_parents_gate(&[]).is_none());

    let both = BuildVerdict {
        failures: 1,
        ongoing: 2,
        problems: vec![0, 1, 2],
        affected_revisions: 2,
    };
    let gate = builds_gate(&both).unwrap();
    assert_eq!(gate.prompt, "arc.land.failed-builds");
    assert!(
        gate.query.contains("ongoing and failed builds"),
        "{}",
        gate.query
    );
    let failed = BuildVerdict {
        failures: 1,
        ongoing: 0,
        problems: vec![0],
        affected_revisions: 1,
    };
    let gate = builds_gate(&failed).unwrap();
    assert!(
        gate.query.contains("despite failed builds"),
        "{}",
        gate.query
    );
    let ongoing = BuildVerdict {
        failures: 0,
        ongoing: 1,
        problems: vec![0],
        affected_revisions: 3,
    };
    let gate = builds_gate(&ongoing).unwrap();
    assert_eq!(gate.prompt, "arc.land.ongoing-builds");
    assert_eq!(gate.title, "ONGOING BUILDS");
    assert!(builds_gate(&BuildVerdict::default()).is_none());
}

#[test]
fn large_working_set_gate_fires_above_limit() {
    assert!(!large_working_set_exceeded(50, LARGE_WORKING_SET_LIMIT));
    assert!(large_working_set_exceeded(51, LARGE_WORKING_SET_LIMIT));
    let gate = large_working_set_gate(51, LARGE_WORKING_SET_LIMIT).unwrap();
    assert_eq!(gate.prompt, "arc.land.large-working-set");
    assert_eq!(gate.title, "LARGE WORKING SET");
    assert_eq!(gate.query, "Land 51 commit(s)?");
    assert!(large_working_set_gate(50, LARGE_WORKING_SET_LIMIT).is_none());
}

#[test]
fn evaluate_gates_runs_in_php_order() {
    let revisions = [
        parse_revision(&modern(
            "changes-planned",
            "Changes Planned",
            false,
            "PHID-USER-2",
        ))
        .unwrap(),
        parse_revision(&modern(
            "needs-review",
            "Needs Review",
            false,
            "PHID-USER-1",
        ))
        .unwrap(),
    ];
    let open = parent_link(1, &modern("needs-review", "Needs Review", false, "a")).unwrap();
    let builds = [BuildView {
        status: "failed".to_string(),
        plan_behavior: Some("always".to_string()),
        revision_phid: "PHID-DREV-7".to_string(),
    }];
    let findings = evaluate_gates(&revisions, "PHID-USER-1", &[open], &[], &builds, Some(60));
    let prompts: Vec<&str> = findings.iter().map(|finding| finding.prompt).collect();
    assert_eq!(
        prompts,
        vec![
            "arc.land.unauthored",
            "arc.land.changes-planned",
            "arc.land.not-accepted",
            "arc.land.open-parents",
            "arc.land.failed-builds",
            "arc.land.large-working-set",
        ]
    );
    let clean = [view("accepted", "PHID-USER-1")];
    let quiet = evaluate_gates(&clean, "PHID-USER-1", &[], &[], &[], Some(3));
    assert!(quiet.is_empty());
    let uncounted = evaluate_gates(&clean, "PHID-USER-1", &[], &[], &[], None);
    assert!(uncounted.is_empty());
}
