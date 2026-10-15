use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-bookmarks-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn parses_defaults() {
    assert_eq!(
        parse_options(&args(&[])),
        Ok(Some(Options {
            json: false,
            conduit_uri: None,
        }))
    );
}

#[test]
fn parses_json_and_conduit_uri() {
    assert_eq!(
        parse_options(&args(&["--json"])),
        Ok(Some(Options {
            json: true,
            conduit_uri: None,
        }))
    );
    assert_eq!(
        parse_options(&args(&["--conduit-uri=https://host/api/"])),
        Ok(Some(Options {
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
}

#[test]
fn help_returns_none() {
    assert_eq!(parse_options(&args(&["--help"])), Ok(None));
    assert_eq!(parse_options(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options(&args(&["mark"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--conduit-uri"])).unwrap_err();
    assert!(err.contains("requires a value"), "unexpected: {err}");
}

#[test]
fn finds_hg_root_upward() {
    let dir = scratch("root");
    let nested = dir.join("a").join("b");
    std::fs::create_dir_all(nested.join(".hg")).unwrap();
    assert_eq!(find_hg_root(&nested), Some(nested.clone()));
    assert_eq!(find_hg_root(&dir.join("sibling-without-hg")), None);
    assert_eq!(find_hg_root(&dir), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_like_arc_outside_mercurial() {
    let dir = scratch("nowhere");
    let err = list(&dir).unwrap_err();
    assert!(
        err.contains("not inside a working copy"),
        "unexpected: {err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn formats_json_objects() {
    let marks = vec![Bookmark {
        name: "active".to_string(),
        active: true,
        rev: String::new(),
        node: "abc123".to_string(),
    }];
    assert_eq!(
        format_json(&marks),
        serde_json::json!([{
            "name": "active",
            "active": true,
            "rev": "",
            "node": "abc123",
        }])
    );
}

#[test]
fn usage_covers_options() {
    let text = usage();
    assert!(text.contains("usage: arc-rs bookmarks"));
    assert!(text.contains("--json"));
    assert!(text.contains("--conduit-uri"));
    assert!(text.contains("Mercurial"));
}
