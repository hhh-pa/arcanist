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
    assert_eq!(options.repository_phid, None);
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
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri="])).is_err());
    // LAND-42: `Unknown argument 'bogus'. Try 'arc help'.`
    assert_eq!(
        parse_args(&args(&["--bogus", "D1"])).unwrap_err(),
        "Unknown argument 'bogus'. Try 'arc help'."
    );
    assert_eq!(
        parse_args(&args(&["-x", "D1"])).unwrap_err(),
        "Unknown argument 'x'. Try 'arc help'."
    );
    assert_eq!(
        parse_args(&args(&["--bogus=1", "D1"])).unwrap_err(),
        "Unknown argument 'bogus'. Try 'arc help'."
    );
}

#[test]
fn normalizes_revision_ids() {
    assert_eq!(normalize_revision_id("123"), "123");
    assert_eq!(normalize_revision_id("D123"), "123");
    assert_eq!(normalize_revision_id("d123"), "123");
    // Upstream never validates the format: one prefix is stripped and the
    // rest flows to `differential.query` untouched.
    assert_eq!(normalize_revision_id(""), "");
    assert_eq!(normalize_revision_id("D"), "");
    assert_eq!(normalize_revision_id("abc"), "abc");
    assert_eq!(normalize_revision_id("12x"), "12x");
    assert_eq!(normalize_revision_id("DD123"), "D123");
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
    // LAND-32: the normalized ID travels as a string, as PHP sends it.
    assert_eq!(id_param("123"), serde_json::json!("123"));
}

#[test]
fn finalize_skips_tracked_repositories() {
    // No conduit traffic: the client points at a dead port and the skip
    // happens before any call.
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let rev = parse_revision(&revision(2, "PHID-USER-1")).unwrap();
    let mut options = parse_args(&args(&["--finalize", "D123"])).unwrap().unwrap();
    options.repository_phid = Some("PHID-REPO-1".to_string());
    let step = close_if_accepted(&client, &options, &rev, "123", "D123", false).unwrap();
    assert!(!step.closed);
    assert!(step.progress.is_empty());
    assert!(step.warnings.is_empty());
}

#[test]
fn selects_only_unique_repository_matches() {
    assert_eq!(
        select_unique_phid(&serde_json::json!([{"phid": "PHID-REPO-1"}])),
        Some("PHID-REPO-1".to_string())
    );
    assert_eq!(select_unique_phid(&serde_json::json!([])), None);
    assert_eq!(
        select_unique_phid(&serde_json::json!([{"phid": "A"}, {"phid": "B"}])),
        None
    );
    assert_eq!(
        select_unique_phid(&serde_json::json!([{"callsign": "X"}])),
        None
    );
}

fn empty_sources() -> get_config::Sources {
    get_config::Sources {
        local: serde_json::Map::new(),
        project: serde_json::Map::new(),
        user: serde_json::Map::new(),
        system: serde_json::Map::new(),
        default: get_config::default_config(),
        project_file: None,
    }
}

#[test]
fn repository_query_orders_callsign_uuid_remote() {
    let mut with_callsign = empty_sources();
    with_callsign
        .project
        .insert("repository.callsign".to_string(), serde_json::json!("X"));
    assert_eq!(configured_callsign(&with_callsign), Some("X".to_string()));
    assert_eq!(
        select_repository_query(&with_callsign, Some("uuid-1"), Some("https://x.test/")),
        Some(serde_json::json!({"callsigns": ["X"]}))
    );

    let empty = empty_sources();
    assert_eq!(configured_callsign(&empty), None);
    assert_eq!(
        select_repository_query(&empty, Some("uuid-1"), Some("https://x.test/")),
        Some(serde_json::json!({"uuids": ["uuid-1"]}))
    );
    assert_eq!(
        select_repository_query(&empty, None, Some("https://x.test/")),
        Some(serde_json::json!({"remoteURIs": ["https://x.test/"]}))
    );
    assert_eq!(select_repository_query(&empty, None, None), None);
}

#[test]
fn repository_query_prefers_configured_callsign() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-close-callsign-{}-{tag}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // The project source beats user/system sources, so this holds no
    // matter what the ambient machine configures.
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcconfig"), r#"{"repository.callsign": "X"}"#).unwrap();
    assert_eq!(
        repository_query(&dir).unwrap(),
        Some(serde_json::json!({"callsigns": ["X"]}))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn detects_working_copy_flavors() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!("arc-rs-close-vcs-{}-{tag}", std::process::id()));
    let hg = base.join("hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(matches!(detect_working_copy(&hg), Some(WorkingCopy::Hg)));
    let svn = base.join("svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert!(matches!(detect_working_copy(&svn), Some(WorkingCopy::Svn)));
    let plain = base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert!(detect_working_copy(&plain).is_none());
    std::fs::remove_dir_all(&base).ok();
}

// -( T1153 Wave 2: LAND findings )--------------------------------------------

#[test]
fn land21_finalized_message_fails_outside_repo() {
    // Outside a repository the finalized message throws like PHP's
    // `ArcanistCapabilityNotSupportedException`; `--quiet` still skips it.
    let rev = parse_revision(&revision(STATUS_ACCEPTED, "PHID-USER-1")).unwrap();
    let plain = std::env::temp_dir().join(format!("arc-rs-close-plain-{}", std::process::id()));
    std::fs::create_dir_all(&plain).unwrap();
    let err = closing_message(&Some(rev.clone()), false, &plain).unwrap_err();
    assert_eq!(
        err,
        "This repository API ('filesystem') does not support the requested capability."
    );
    assert_eq!(closing_message(&Some(rev), true, &plain).unwrap(), "");
    let repo = plain.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let status = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q", "-b", "master"])
        .status()
        .unwrap();
    assert!(status.success());
    let rev = parse_revision(&revision(STATUS_ACCEPTED, "PHID-USER-1")).unwrap();
    let message = closing_message(&Some(rev), false, &repo).unwrap();
    assert!(message.contains("git push"), "{message}");
    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn land32_close_sends_string_revision_ids() {
    // Both `differential.query` ids and `differential.close` revisionID
    // travel as strings, as PHP sends them.
    assert_eq!(id_param("123"), serde_json::json!("123"));
    assert_eq!(id_param("D123"), serde_json::json!("D123"));
}

#[test]
fn land22_user_phid_reuses_handshake_identity() {
    // T1153 LAND-22: `user_phid` answers from the cached handshake
    // identity without a `user.whoami` call (no network here: the
    // client points at an unroutable host, so a call would fail).
    // Amend (`confirm_author`) and commit (`resolve_svn_user_phid`)
    // share this helper, so both adopt the cache too.
    let client = conduit::Client::new("https://conduit.test/", "api-test").unwrap();
    client.set_authenticated_user("alice", "PHID-USER-1");
    assert_eq!(user_phid(&client).unwrap(), "PHID-USER-1");
}

#[test]
fn b21_close_revision_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `close-revision`: it is a classic
    // workflow (`ArcanistWorkflow`, `supportsToolset()` false), so
    // PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_args(&args(&["D123"])).unwrap().is_some());
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert!(parse_args(&args(&[])).is_err());
    let text = usage_text(false);
    assert!(text.contains("CLOSE-REVISION WORKFLOW"), "{text}");
}

#[test]
fn b23_close_revision_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help close-revision`
    // (classic: source indentation kept verbatim, no unwrap).
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("CLOSE-REVISION WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Close a revision which has been committed"),
        "{text}"
    );
    assert!(text.contains("--finalize"), "{text}");
    assert!(text.contains("--quiet"), "{text}");
    assert!(!text.contains("--force"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
