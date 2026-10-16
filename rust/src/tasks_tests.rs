use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn task(id: u64, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        priority: "Normal".to_string(),
        priority_color: None,
        status_name: "Open".to_string(),
        is_closed: Some(false),
    }
}

#[test]
fn parses_defaults() {
    let options = parse_args(&args(&[])).unwrap().unwrap();
    assert_eq!(options.status, None);
    assert_eq!(options.owner, None);
    assert_eq!(options.order, None);
    assert_eq!(options.limit, None);
    assert!(!options.unassigned);
    assert!(!options.json);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_all_options() {
    let options = parse_args(&args(&[
        "--status",
        "closed",
        "--owner=alice",
        "--order",
        "created",
        "--limit",
        "10",
        "--json",
        "--conduit-uri=https://example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.status, Some("closed".to_string()));
    assert_eq!(options.owner, Some("alice".to_string()));
    assert_eq!(options.order, Some("created".to_string()));
    assert_eq!(options.limit, Some(10));
    assert!(options.json);
    assert_eq!(
        options.conduit_uri,
        Some("https://example.test/".to_string())
    );
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--status"])).is_err());
    assert!(parse_args(&args(&["--status="])).is_err());
    assert!(parse_args(&args(&["--limit"])).is_err());
    assert!(parse_args(&args(&["--limit", "many"])).is_err());
    assert!(parse_args(&args(&["--limit", "-1"])).is_err());
    assert!(parse_args(&args(&["positional"])).is_err());
    assert!(parse_args(&args(&["--", "positional"])).is_err());
}

#[test]
fn rejects_owner_and_unassigned_conflict() {
    assert!(parse_args(&args(&["--owner", "alice", "--unassigned"])).is_err());
}

#[test]
fn builds_default_query_params() {
    assert_eq!(
        query_params(Some("PHID-USER-1"), None, None, None),
        serde_json::json!({
            "ownerPHIDs": ["PHID-USER-1"],
            "order": "order-priority",
            "status": "status-open",
        })
    );
}

#[test]
fn builds_custom_query_params() {
    assert_eq!(
        query_params(None, Some("all"), Some("created"), Some(5)),
        serde_json::json!({
            "limit": 5,
            "order": "order-created",
            "status": "status-any",
        })
    );
    assert_eq!(
        query_params(None, Some("closed"), None, None),
        serde_json::json!({
            "order": "order-priority",
            "status": "status-closed",
        })
    );
}

#[test]
fn parses_task_rows() {
    let task = parse_task(&serde_json::json!({
        "id": 42,
        "title": "Fix login  ",
        "priority": "High",
        "priorityColor": "red",
        "statusName": "Open",
        "isClosed": false,
    }))
    .unwrap();
    assert_eq!(task.id, "42");
    assert_eq!(task.title, "Fix login");
    assert_eq!(task.priority, "High");
    assert_eq!(task.priority_color, Some("red".to_string()));
    assert_eq!(task.status_name, "Open");
    assert_eq!(task.is_closed, Some(false));
    let numeric = parse_task(&serde_json::json!({"id": 1, "isClosed": 1})).unwrap();
    assert_eq!(numeric.is_closed, Some(true));
    let missing =
        parse_task(&serde_json::json!({"id": 1, "isClosed": serde_json::Value::Null})).unwrap();
    assert_eq!(missing.is_closed, None);
    assert!(parse_task(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn parses_list_and_map_results() {
    let row = serde_json::json!({"id": 1, "title": "t"});
    assert_eq!(parse_tasks(&serde_json::json!([row])).len(), 1);
    assert_eq!(
        parse_tasks(&serde_json::json!({"PHID-TASK-1": row})).len(),
        1
    );
    assert!(parse_tasks(&serde_json::json!(null)).is_empty());
}

#[test]
fn renders_table_rows() {
    let out = render_table(&[task(7, "First"), task(123, "Second")], false);
    assert_eq!(
        out,
        "T7   First    Normal   Open\nT123 Second   Normal   Open\n"
    );
}

#[test]
fn blank_status_without_is_closed() {
    let mut row = task(7, "First");
    row.is_closed = None;
    let out = render_table(&[row], false);
    assert!(!out.contains("Open"), "{out}");
    assert!(out.contains("First"), "{out}");
    assert_eq!(priority_color(Some("red")), "red");
    assert_eq!(priority_color(Some("lightsky")), "blue");
    assert_eq!(priority_color(Some("chartreuse")), "white");
    assert_eq!(priority_color(None), "white");
}

#[test]
fn builds_tasks_json() {
    assert_eq!(
        tasks_json(&[task(7, "First")]),
        serde_json::json!([{
            "id": "T7",
            "title": "First",
            "priority": "Normal",
            "status": "Open",
            "isClosed": false,
        }])
    );
}

#[test]
fn parse_tasks_keeps_server_row_order() {
    // PHID-keyed maps keep wire order (preserve_order), not PHID sort.
    let result: serde_json::Value = serde_json::from_str(
        r#"{"PHID-TASK-zzz": {"id": 2, "title": "Second"},
            "PHID-TASK-aaa": {"id": 1, "title": "First"}}"#,
    )
    .unwrap();
    let tasks = parse_tasks(&result);
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        vec!["2", "1"]
    );
}

#[test]
fn execute_propagates_conduit_errors() {
    // RB-D27 lock: execute errors (incl. conduit failures) surface as Err
    // so run() prints them to stderr with exit 1, like PHP.
    let client = crate::conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let options = Options {
        status: None,
        owner: None,
        order: None,
        limit: None,
        unassigned: false,
        json: false,
        conduit_uri: None,
    };
    let err = execute(&client, &options).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `tasks` is a subprocess workflow: argument errors render `Usage
    // Exception:` (exit 1) with no usage dump.
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn rbd8_user_phid_prefers_cached_identity() {
    // The startup-auth identity answers without I/O: an unroutable
    // client still resolves when cached.
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    client.set_authenticated_user("alice", "PHID-USER-1");
    assert_eq!(user_phid(&client).unwrap(), "PHID-USER-1");
}
