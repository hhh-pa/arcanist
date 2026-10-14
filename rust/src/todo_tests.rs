use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_summary_words() {
    let options = parse_args(&args(&["fix", "the", "build"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.summary, args(&["fix", "the", "build"]));
    assert!(options.cc.is_empty());
    assert!(options.projects.is_empty());
    assert!(!options.browse);
    assert!(!options.json);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_all_options() {
    let options = parse_args(&args(&[
        "do",
        "thing",
        "--cc",
        "alice",
        "-C",
        "bob",
        "--cc=carol",
        "--project",
        "web",
        "--project=api",
        "--json",
        "--conduit-uri",
        "https://example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.summary, args(&["do", "thing"]));
    assert_eq!(options.cc, args(&["alice", "bob", "carol"]));
    assert_eq!(options.projects, args(&["web", "api"]));
    assert!(options.json);
    assert_eq!(
        options.conduit_uri,
        Some("https://example.test/".to_string())
    );
}

#[test]
fn parses_double_dash_and_browse() {
    let options = parse_args(&args(&["--browse", "--", "--not-a-flag"]))
        .unwrap()
        .unwrap();
    assert!(options.browse);
    assert_eq!(options.summary, args(&["--not-a-flag"]));
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--cc"])).is_err());
    assert!(parse_args(&args(&["-C"])).is_err());
    assert!(parse_args(&args(&["--cc="])).is_err());
    assert!(parse_args(&args(&["--project"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
}

#[test]
fn parses_created_tasks() {
    let task = parse_created(&serde_json::json!({
        "id": 55,
        "title": "Do thing",
        "uri": "https://example.test/T55",
    }))
    .unwrap();
    assert_eq!(task.id, "55");
    assert_eq!(task.title, "Do thing");
    assert_eq!(task.uri, "https://example.test/T55");
    assert!(parse_created(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn renders_created_line() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    assert_eq!(
        render_created(&task),
        "Created task T55: 'Do thing' at https://example.test/T55\n"
    );
}

#[test]
fn builds_created_json() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    assert_eq!(
        created_json(&task),
        serde_json::json!({
            "id": "T55",
            "title": "Do thing",
            "uri": "https://example.test/T55",
        })
    );
}
