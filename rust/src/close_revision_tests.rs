use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn revision(status: i64, author: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 123,
        "title": "Fix the thing",
        "status": status,
        "statusName": status_name_for(status),
        "authorPHID": author,
        "commits": [],
    })
}

#[test]
fn parses_revision_and_defaults() {
    let options = parse_args(&args(&["D123"])).unwrap().unwrap();
    assert_eq!(options.revision, "D123");
    assert!(!options.finalize);
    assert!(!options.quiet);
    assert!(!options.json);
    assert!(!options.force);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_all_flags() {
    let options = parse_args(&args(&[
        "--finalize",
        "--quiet",
        "--json",
        "--force",
        "--conduit-uri",
        "https://example.test/",
        "456",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.revision, "456");
    assert!(options.finalize);
    assert!(options.quiet);
    assert!(options.json);
    assert!(options.force);
    assert_eq!(
        options.conduit_uri,
        Some("https://example.test/".to_string())
    );
}

#[test]
fn parses_conduit_uri_equals_and_double_dash() {
    let options = parse_args(&args(&["--conduit-uri=https://x.test/", "--", "D7"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.revision, "D7");
    assert_eq!(options.conduit_uri, Some("https://x.test/".to_string()));
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&[])).is_err());
    assert!(parse_args(&args(&["D1", "D2"])).is_err());
    assert!(parse_args(&args(&["--bogus", "D1"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri="])).is_err());
}

#[test]
fn normalizes_revision_ids() {
    assert_eq!(normalize_revision_id("123").unwrap(), "123");
    assert_eq!(normalize_revision_id("D123").unwrap(), "123");
    assert_eq!(normalize_revision_id("d123").unwrap(), "123");
    assert!(normalize_revision_id("").is_err());
    assert!(normalize_revision_id("D").is_err());
    assert!(normalize_revision_id("abc").is_err());
    assert!(normalize_revision_id("12x").is_err());
    assert!(normalize_revision_id("DD123").is_err());
}

#[test]
fn names_all_statuses() {
    assert_eq!(status_name_for(0), "Needs Review");
    assert_eq!(status_name_for(1), "Needs Revision");
    assert_eq!(status_name_for(2), "Accepted");
    assert_eq!(status_name_for(3), "Closed");
    assert_eq!(status_name_for(4), "Abandoned");
    assert_eq!(status_name_for(5), "Changes Planned");
    assert_eq!(status_name_for(6), "In Preparation");
    assert_eq!(status_name_for(99), "Unknown");
    assert_eq!(STATUS_ACCEPTED, 2);
    assert_eq!(STATUS_CLOSED, 3);
}

#[test]
fn parses_revision_rows() {
    let rev = parse_revision(&revision(2, "PHID-USER-1")).unwrap();
    assert_eq!(rev.id, "123");
    assert_eq!(rev.title, "Fix the thing");
    assert_eq!(rev.status, 2);
    assert_eq!(rev.status_name, "Accepted");
    assert_eq!(rev.author_phid, "PHID-USER-1");
    assert!(rev.commits.is_empty());
    assert!(parse_revision(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn falls_back_for_missing_status_name() {
    let rev = parse_revision(&serde_json::json!({"id": 9, "status": 4})).unwrap();
    assert_eq!(rev.status_name, "Abandoned");
    let rev = parse_revision(&serde_json::json!({"id": "D9"})).unwrap();
    assert_eq!(rev.status, -1);
    assert_eq!(rev.status_name, "Unknown");
}

#[test]
fn builds_result_json() {
    let rev = parse_revision(&revision(2, "PHID-USER-1")).unwrap();
    assert_eq!(
        result_json("D123", Some(&rev), true),
        serde_json::json!({
            "revision": "D123",
            "closed": true,
            "status": 2,
            "statusName": "Accepted",
        })
    );
    assert_eq!(
        result_json("D123", None, false),
        serde_json::json!({
            "revision": "D123",
            "closed": false,
            "status": serde_json::Value::Null,
            "statusName": serde_json::Value::Null,
        })
    );
}

#[test]
fn close_targets_revision_id_number() {
    assert_eq!(id_param("123"), serde_json::json!(123));
}
