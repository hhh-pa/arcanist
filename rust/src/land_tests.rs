use super::*;
use crate::land_checks;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn defaults() -> Options {
    Options {
        onto: "master".to_string(),
        preview: false,
        push: false,
        revision: None,
        conduit_uri: None,
    }
}

fn sample_plan() -> Plan {
    Plan {
        branch: "feature".to_string(),
        onto: "master".to_string(),
        base: "base-sha".to_string(),
        head: "head-sha".to_string(),
        commits: vec![
            Commit {
                sha: "aaa1111aaa1111aaa1111aaa1111aaa1111aaa1".to_string(),
                subject: "add a".to_string(),
            },
            Commit {
                sha: "bbb2222bbb2222bbb2222bbb2222bbb2222bbb2".to_string(),
                subject: "add b".to_string(),
            },
        ],
    }
}

fn revision_row(status: &str, name: &str, author: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 123,
        "phid": "PHID-DREV-x",
        "fields": {
            "title": "Add widgets",
            "authorPHID": author,
            "status": {"value": status, "name": name, "closed": false},
        },
    })
}

fn search_result(rows: &[serde_json::Value]) -> serde_json::Value {
    serde_json::json!({"data": rows})
}

fn canned(
    pairs: Vec<(String, serde_json::Value)>,
) -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    move |method: &str, _params: &serde_json::Value| {
        pairs
            .iter()
            .find(|(name, _)| name == method)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("no canned response for {method}"))
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_args(&args(&[])), Ok(defaults()));
}

#[test]
fn parses_onto_forms_and_flags() {
    assert_eq!(
        parse_args(&args(&["--onto", "stable"])).unwrap().onto,
        "stable"
    );
    assert_eq!(
        parse_args(&args(&["--onto=stable"])).unwrap().onto,
        "stable"
    );
    let opts = parse_args(&args(&["--preview", "--push", "--onto", "x"])).unwrap();
    assert!(opts.preview && opts.push && opts.onto == "x");
}

#[test]
fn parses_revision_and_conduit_uri() {
    let opts = parse_args(&args(&["--revision", "D123"])).unwrap();
    assert_eq!(opts.revision.as_deref(), Some("D123"));
    let opts = parse_args(&args(&["--revision=456"])).unwrap();
    assert_eq!(opts.revision.as_deref(), Some("456"));
    let opts = parse_args(&args(&["--conduit-uri", "https://phab.example.test/"])).unwrap();
    assert_eq!(
        opts.conduit_uri.as_deref(),
        Some("https://phab.example.test/")
    );
    let opts = parse_args(&args(&["--conduit-uri=https://x.test/"])).unwrap();
    assert_eq!(opts.conduit_uri.as_deref(), Some("https://x.test/"));
}

#[test]
fn rejects_bad_arguments() {
    assert!(parse_args(&args(&["--onto"])).is_err());
    assert!(parse_args(&args(&["--onto="])).is_err());
    assert!(parse_args(&args(&["--onto", "--preview"])).is_err());
    assert!(parse_args(&args(&["--revision"])).is_err());
    assert!(parse_args(&args(&["--revision="])).is_err());
    assert!(parse_args(&args(&["--revision", "--preview"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri="])).is_err());
    let err = parse_args(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_args(&args(&["feature"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
}

#[test]
fn detects_help() {
    assert!(wants_help(&args(&["--help"])));
    assert!(wants_help(&args(&["-h"])));
    assert!(wants_help(&args(&["help"])));
    assert!(!wants_help(&args(&[])));
    assert!(!wants_help(&args(&["--preview"])));
}

#[test]
fn parses_revision_argument() {
    assert_eq!(parse_revision_argument("D123").unwrap(), 123);
    assert_eq!(parse_revision_argument("d4").unwrap(), 4);
    assert_eq!(parse_revision_argument("56").unwrap(), 56);
    assert_eq!(parse_revision_argument("  89  ").unwrap(), 89);
    assert_eq!(parse_revision_argument("D 12").unwrap(), 12);
    for bad in ["", "D", "D0x", "12.3", "abc", "1 2"] {
        let err = parse_revision_argument(bad).unwrap_err();
        assert_eq!(
            err,
            "Parameter to --revision must be a Differential Revision number."
        );
    }
}

#[test]
fn accepted_revision_passes_gates() {
    let row = revision_row("accepted", "Accepted", "PHID-USER-owner");
    let call = canned(vec![
        (
            "differential.revision.search".to_string(),
            search_result(&[row]),
        ),
        (
            "user.whoami".to_string(),
            serde_json::json!({"userName": "owner", "phid": "PHID-USER-owner"}),
        ),
    ]);
    assert!(load_gate_findings(&call, 123, 2).unwrap().is_empty());
}

#[test]
fn planned_and_foreign_revisions_raise_findings() {
    let row = revision_row("changes-planned", "Changes Planned", "PHID-USER-owner");
    let call = canned(vec![
        (
            "differential.revision.search".to_string(),
            search_result(&[row]),
        ),
        (
            "user.whoami".to_string(),
            serde_json::json!({"userName": "stranger", "phid": "PHID-USER-stranger"}),
        ),
    ]);
    let findings = load_gate_findings(&call, 123, 2).unwrap();
    let prompts: Vec<&str> = findings.iter().map(|finding| finding.prompt).collect();
    assert_eq!(
        prompts,
        vec![
            land_checks::PROMPT_UNAUTHORED,
            land_checks::PROMPT_CHANGES_PLANNED,
        ]
    );
}

#[test]
fn missing_viewer_skips_author_check() {
    let row = revision_row("accepted", "Accepted", "PHID-USER-owner");
    let call = canned(vec![(
        "differential.revision.search".to_string(),
        search_result(&[row]),
    )]);
    assert!(load_gate_findings(&call, 123, 2).unwrap().is_empty());
}

#[test]
fn large_working_set_fires_on_commit_count() {
    let row = revision_row("accepted", "Accepted", "PHID-USER-owner");
    let call = canned(vec![
        (
            "differential.revision.search".to_string(),
            search_result(&[row]),
        ),
        (
            "user.whoami".to_string(),
            serde_json::json!({"userName": "owner", "phid": "PHID-USER-owner"}),
        ),
    ]);
    let findings = load_gate_findings(&call, 123, 60).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].prompt, land_checks::PROMPT_LARGE_WORKING_SET);
}

#[test]
fn empty_search_result_is_unavailable() {
    let call = canned(vec![(
        "differential.revision.search".to_string(),
        search_result(&[]),
    )]);
    let err = load_gate_findings(&call, 123, 2).unwrap_err();
    assert!(
        err.contains("no differential.revision.search result"),
        "{err}"
    );

    let failing = canned(vec![]);
    let err = load_gate_findings(&failing, 123, 2).unwrap_err();
    assert!(err.contains("no canned response"), "{err}");
}

#[test]
fn formats_skip_note_preview_and_failure() {
    let note = format_skip_note("no token");
    assert!(
        note.contains("revision info unavailable (no token)"),
        "{note}"
    );
    assert!(note.contains("skipping pre-land checks"), "{note}");

    let passing = format_gate_preview(&[], 123);
    assert!(
        passing.contains("Pre-land checks passed (--revision D123)"),
        "{passing}"
    );

    let findings = vec![land_checks::GateFinding {
        prompt: land_checks::PROMPT_NOT_ACCEPTED,
        title: "1 REVISION(S) ARE NOT ACCEPTED".to_string(),
        query: "Land 1 revision(s) in the wrong state?".to_string(),
        count: 1,
    }];
    let advisory = format_gate_preview(&findings, 123);
    assert!(
        advisory.contains("Pre-land checks (--revision D123)"),
        "{advisory}"
    );
    assert!(advisory.contains("arc.land.not-accepted"), "{advisory}");
    let blocked = format_gate_failure(&findings, 123);
    assert!(
        blocked.contains("Pre-land checks failed (--revision D123)"),
        "{blocked}"
    );
    assert!(blocked.contains("arc.land.not-accepted"), "{blocked}");
}

#[test]
fn preview_names_target_base_head_and_commits() {
    let plan = sample_plan();
    let text = format_preview(&plan);
    assert!(text.contains("'feature'"), "missing branch: {text}");
    assert!(text.contains("'master'"), "missing onto: {text}");
    assert!(text.contains("base-sha"), "missing base: {text}");
    assert!(text.contains("head-sha"), "missing head: {text}");
    assert!(
        text.contains("add a") && text.contains("add b"),
        "missing commits: {text}"
    );
}

#[test]
fn message_names_branch_and_lists_commits() {
    let plan = sample_plan();
    let message = build_message(&plan);
    let first: String = message.lines().take(1).collect();
    assert!(
        first.contains("feature"),
        "subject must name branch: {message}"
    );
    assert!(
        message.contains("add a") && message.contains("add b"),
        "missing commits: {message}"
    );
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("--onto") && text.contains("--preview") && text.contains("--push"));
    assert!(text.contains("--revision") && text.contains("--conduit-uri"));
    assert!(text.contains("remote-tracking"));
}
