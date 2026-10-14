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
    assert_eq!(parse_options(&args(&[])), Ok(Some(Options { json: false })));
}

#[test]
fn parses_json() {
    assert_eq!(
        parse_options(&args(&["--json"])),
        Ok(Some(Options { json: true }))
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
}

#[test]
fn parses_bookmark_entries() {
    let marks = parse_bookmarks(" * active   12:abc123def456\n   other    3:789abc\n").unwrap();
    assert_eq!(
        marks,
        vec![
            Bookmark {
                name: "active".to_string(),
                active: true,
                rev: "12".to_string(),
                node: "abc123def456".to_string(),
            },
            Bookmark {
                name: "other".to_string(),
                active: false,
                rev: "3".to_string(),
                node: "789abc".to_string(),
            },
        ]
    );
}

#[test]
fn parses_empty_output() {
    assert_eq!(parse_bookmarks(""), Ok(Vec::new()));
    assert_eq!(parse_bookmarks("\n\n"), Ok(Vec::new()));
}

#[test]
fn rejects_malformed_lines() {
    let err = parse_bookmarks("lonely\n").unwrap_err();
    assert!(err.contains("malformed"), "unexpected: {err}");
}

#[test]
fn tolerates_nodes_without_rev() {
    let marks = parse_bookmarks("   mark   abc123\n").unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].rev, "");
    assert_eq!(marks[0].node, "abc123");
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
fn formats_text_with_active_marker() {
    let marks = vec![
        Bookmark {
            name: "active".to_string(),
            active: true,
            rev: "12".to_string(),
            node: "abc123".to_string(),
        },
        Bookmark {
            name: "other".to_string(),
            active: false,
            rev: "3".to_string(),
            node: "789abc".to_string(),
        },
    ];
    assert_eq!(
        format_text(&marks),
        "* active 12:abc123\n  other 3:789abc\n"
    );
}

#[test]
fn formats_json_objects() {
    let marks = vec![Bookmark {
        name: "active".to_string(),
        active: true,
        rev: "12".to_string(),
        node: "abc123".to_string(),
    }];
    assert_eq!(
        format_json(&marks),
        serde_json::json!([{
            "name": "active",
            "active": true,
            "rev": "12",
            "node": "abc123",
        }])
    );
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("usage: arc-rs bookmarks"));
    assert!(text.contains("--json"));
    assert!(text.contains("Mercurial"));
}
