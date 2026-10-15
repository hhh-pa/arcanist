use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn info(id: &str, status: i64, exists: bool) -> RevisionInfo {
    RevisionInfo {
        id: id.to_string(),
        title: format!("Title {id}"),
        status,
        status_name: status_name_for(status).to_string(),
        source_path: String::new(),
        exists,
    }
}

#[test]
fn parses_defaults() {
    let options = parse_args(&args(&[])).unwrap().unwrap();
    assert!(!options.json);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_flags() {
    let options = parse_args(&args(&["--json", "--conduit-uri=https://x.test/"]))
        .unwrap()
        .unwrap();
    assert!(options.json);
    assert_eq!(options.conduit_uri, Some("https://x.test/".to_string()));
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri="])).is_err());
    assert!(parse_args(&args(&["D123"])).is_err());
    assert!(parse_args(&args(&["--", "D123"])).is_err());
}

#[test]
fn names_all_statuses() {
    assert_eq!(status_name_for(0), "Needs Review");
    assert_eq!(status_name_for(2), "Accepted");
    assert_eq!(status_name_for(3), "Closed");
    assert_eq!(status_name_for(77), "Unknown");
}

#[test]
fn parses_revision_rows() {
    let rev = parse_revision(&serde_json::json!({
        "id": 9,
        "title": "Some work",
        "status": 0,
        "statusName": "Needs Review",
        "sourcePath": "/repo",
    }))
    .unwrap();
    assert_eq!(rev.id, "9");
    assert_eq!(rev.title, "Some work");
    assert_eq!(rev.status, 0);
    assert_eq!(rev.status_name, "Needs Review");
    assert_eq!(rev.source_path, "/repo");
    assert!(!rev.exists);
    assert!(parse_revision(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn parses_list_and_map_results() {
    let row = serde_json::json!({"id": 1, "title": "t"});
    assert_eq!(parse_revisions(&serde_json::json!([row])).len(), 1);
    assert_eq!(parse_revisions(&serde_json::json!({"1": row})).len(), 1);
    assert!(parse_revisions(&serde_json::json!(null)).is_empty());
}

#[test]
fn sorts_by_exists_status_then_id() {
    let mut revs = vec![
        info("10", 2, true),
        info("2", 0, false),
        info("1", 0, false),
        info("3", 0, true),
    ];
    sort_revisions(&mut revs);
    let order: Vec<&str> = revs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(order, vec!["1", "2", "3", "10"]);
}

#[test]
fn renders_rows_with_marker() {
    assert_eq!(
        render_table(&[info("9", 2, true)], false),
        "* Accepted D9: Title 9\n"
    );
    assert_eq!(
        render_table(&[info("10", 0, false)], false),
        " Needs Review D10: Title 10\n"
    );
    assert_eq!(
        render_table(&[info("9", 2, true), info("10", 0, false)], false),
        "* Accepted     D9: Title 9\n  Needs Review D10: Title 10\n"
    );
    assert_eq!(status_color("Accepted"), "green");
    assert_eq!(status_color("Needs Review"), "magenta");
    assert_eq!(status_color("Abandoned"), "default");
    assert_eq!(status_color("In Preparation"), "default");
}

#[test]
fn builds_revisions_json() {
    assert_eq!(
        revisions_json(&[info("9", 2, true)]),
        serde_json::json!([{
            "id": "D9",
            "title": "Title 9",
            "status": 2,
            "statusName": "Accepted",
            "exists": true,
        }])
    );
}

#[test]
fn finds_hg_and_svn_roots() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!("arc-rs-list-roots-{}-{tag}", std::process::id()));

    let hg_nested = base.join("hg").join("sub");
    std::fs::create_dir_all(base.join("hg").join(".hg")).unwrap();
    std::fs::create_dir_all(&hg_nested).unwrap();
    // `find_hg_root` walks up without shelling out.
    assert_eq!(find_hg_root(&hg_nested), Some(base.join("hg")), "hg root");

    let svn_nested = base.join("svn").join("sub");
    std::fs::create_dir_all(base.join("svn").join(".svn")).unwrap();
    std::fs::create_dir_all(&svn_nested).unwrap();
    assert_eq!(
        working_copy_root(&svn_nested),
        Some(base.join("svn").to_string_lossy().to_string()),
        "svn root"
    );
    assert_eq!(
        working_copy_root(&hg_nested),
        Some(base.join("hg").to_string_lossy().to_string()),
        "hg root via working_copy_root"
    );

    let plain = base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(find_hg_root(&plain), None);
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn compares_paths() {
    assert!(same_path("/x/y", "/x/y"));
    assert!(!same_path("/x/y", "/x/z"));
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-list-samepath-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let with_dot = dir.join(".").to_string_lossy().to_string();
    let plain = dir.to_string_lossy().to_string();
    assert!(same_path(&with_dot, &plain));
    std::fs::remove_dir_all(&dir).ok();
}
