use super::*;

#[test]
fn mangles_legacy_query_rows() {
    let row = serde_json::json!({
        "id": 123,
        "phid": "PHID-DREV-1",
        "status": "2",
        "statusName": "Accepted",
        "title": "Great change",
        "uri": "/D123",
        "authorPHID": "PHID-USER-1",
        "dateCreated": 1_700_000_000,
    });
    let reference = RevisionRef::from_query_row(&row);
    assert_eq!(reference.id(), 123);
    assert_eq!(reference.phid(), Some("PHID-DREV-1"));
    assert_eq!(reference.monogram(), "D123");
    assert_eq!(reference.name(), Some("Great change"));
    assert_eq!(reference.status(), Some("accepted"));
    assert!(!reference.is_status_published());
    assert_eq!(reference.status_display_name(), Some("Accepted"));
    assert_eq!(reference.status_short_name(), Some("Accepted"));
    assert_eq!(reference.status_ansi_color(), Some("green"));
}

#[test]
fn maps_legacy_status_codes() {
    for (code, value, color) in [
        ("0", "needs-review", Some("magenta")),
        ("1", "needs-revision", Some("red")),
        ("3", "published", Some("cyan")),
        ("4", "abandoned", None),
        ("5", "changes-planned", Some("red")),
    ] {
        let reference = RevisionRef::from_query_row(&serde_json::json!({
            "id": 1,
            "status": code,
            "statusName": "X",
        }));
        assert_eq!(reference.status(), Some(value), "code {code}");
        assert_eq!(reference.status_ansi_color(), color, "code {code}");
    }
    assert!(
        RevisionRef::from_query_row(&serde_json::json!({"id": 1}))
            .status()
            .is_none()
    );
}

#[test]
fn shortens_needs_review_status() {
    let reference = RevisionRef::from_search_hit(&serde_json::json!({
        "id": 7,
        "fields": {"status": {"value": "needs-review", "name": "Needs Review"}},
    }));
    assert!(reference.is_status_needs_review());
    assert_eq!(reference.status_short_name(), Some("Review"));
    assert_eq!(reference.status_display_name(), Some("Needs Review"));
}

#[test]
fn reads_ids_like_php_int_cast() {
    assert_eq!(
        RevisionRef::from_search_hit(&serde_json::json!({"id": "42"})).id(),
        42
    );
    assert_eq!(
        RevisionRef::from_search_hit(&serde_json::json!({"id": "42x"})).id(),
        42
    );
    assert_eq!(
        RevisionRef::from_search_hit(&serde_json::json!({"id": "x"})).id(),
        0
    );
    assert_eq!(RevisionRef::from_search_hit(&serde_json::json!({})).id(), 0);
}

#[test]
fn parses_revision_ids_from_messages() {
    assert_eq!(
        revision_id_from_message("subject\n\nDifferential Revision: D123\n"),
        Ok(Some(123))
    );
    assert_eq!(
        revision_id_from_message("differential revision: d7"),
        Ok(Some(7))
    );
    assert_eq!(
        revision_id_from_message("Differential Revision: https://devtools.example.com/D123\n"),
        Ok(Some(123))
    );
    assert_eq!(
        revision_id_from_message("Differential Revision: http://host:8080/D9/"),
        Ok(Some(9))
    );
    assert_eq!(revision_id_from_message("no field here"), Ok(None));
    assert_eq!(revision_id_from_message("Differential Revision:"), Ok(None));
    // A field followed only by blank lines never matches.
    assert_eq!(
        revision_id_from_message("Differential Revision:\n\n"),
        Ok(None)
    );
    // `\s*` spans line breaks: the next line is the value.
    assert_eq!(
        revision_id_from_message("Differential Revision: \nD123\n"),
        Ok(Some(123))
    );
}

#[test]
fn rejects_invalid_revision_fields() {
    let err = revision_id_from_message("Differential Revision: D0\n").unwrap_err();
    assert!(
        err.contains("Invalid \"Differential Revision\" field"),
        "{err}"
    );
    assert!(err.contains("but has \"D0\""), "{err}");
    assert!(revision_id_from_message("Differential Revision: 123\n").is_err());
    assert!(revision_id_from_message("Differential Revision: D12x\n").is_err());
    assert!(revision_id_from_message("Differential Revision: https://host/other\n").is_err());
    // Whitespace-only values match, then fail validation.
    assert!(revision_id_from_message("Differential Revision:   \n").is_err());
}

#[test]
fn merges_refs_by_phid_with_hash_refs_first() {
    let hash_ref = RevisionRef::from_search_hit(&serde_json::json!({
        "id": 1, "phid": "PHID-DREV-a", "fields": {"title": "via hash"},
    }));
    let dup = RevisionRef::from_search_hit(&serde_json::json!({
        "id": 1, "phid": "PHID-DREV-a", "fields": {"title": "via message"},
    }));
    let message_ref = RevisionRef::from_search_hit(&serde_json::json!({
        "id": 2, "phid": "PHID-DREV-b", "fields": {"title": "other"},
    }));
    let merged = merge_revision_refs(vec![hash_ref], vec![dup, message_ref]);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].name(), Some("via hash"));
    assert_eq!(merged[1].name(), Some("other"));
}

#[test]
fn matches_php_git_path_vectors() {
    for (input, expect) in [
        ("ssh://user@domain.com/path.git", "path"),
        ("https://user@domain.com/path.git", "path"),
        ("git@domain.com:path.git", "path"),
        ("ssh://user@gitserv002.com/path.git", "path"),
        ("ssh://htaft@domain.com/path.git", "path"),
        ("ssh://user@domain.com/bananas.git", "bananas"),
        ("git@domain.com:bananas.git", "bananas"),
        ("user@domain.com:path/repo", "path/repo"),
        ("user@domain.com:path/repo/", "path/repo"),
        ("file:///path/to/local/repo.git", "path/to/local/repo"),
        ("/path/to/local/repo.git", "path/to/local/repo"),
        (
            "ssh://something.com/diffusion/X/anything.git",
            "diffusion/X",
        ),
        ("ssh://something.com/diffusion/X/", "diffusion/X"),
    ] {
        assert_eq!(normalize_git_path(input), expect, "input {input}");
    }
}

#[test]
fn matches_php_domain_vectors() {
    let mut map = HashMap::new();
    map.insert(
        "<base-uri>".to_string(),
        "base.phabricator.example.com".to_string(),
    );
    map.insert(
        "<ssh-host>".to_string(),
        "ssh.phabricator.example.com".to_string(),
    );
    for (input, expect) in [
        ("/", "<void>"),
        ("/path/to/local/repo.git", "<void>"),
        ("ssh://user@domain.com/path.git", "domain.com"),
        ("ssh://user@DOMAIN.COM/path.git", "domain.com"),
        (
            "http://base.phabricator.example.com/diffusion/X/",
            "<base-uri>",
        ),
        (
            "ssh://ssh.phabricator.example.com/diffusion/X/",
            "<ssh-host>",
        ),
        ("git@ssh.phabricator.example.com:bananas.git", "<ssh-host>"),
    ] {
        assert_eq!(normalize_git_domain(input, &map), expect, "input {input}");
    }
}

#[test]
fn normalizes_equivalent_uris_identically() {
    uri_pair_same("ssh://user@host/repo", "ssh://user@host/repo/");
    uri_pair_same("ssh://user@host/repo", "ssh://user@host:22/repo");
    uri_pair_same("ssh://user@host/repo", "user@host:/repo");
    uri_pair_same("ssh://user@host/repo", "ssh://user@host/repo.git");
}

fn uri_pair_same(left: &str, right: &str) {
    assert_eq!(
        normalize_git_uri(left),
        normalize_git_uri(right),
        "{left} vs {right}"
    );
}

#[test]
fn evaluates_permanent_ref_rules() {
    let missing = RepositoryRef::from_search_hit(&serde_json::json!({"fields": {}}));
    assert!(!missing.is_permanent_ref("master"));
    let empty = RepositoryRef::from_search_hit(&serde_json::json!({
        "fields": {"refRules": {"permanentRefRules": []}},
    }));
    assert!(empty.is_permanent_ref("anything"));
    let exact = RepositoryRef::from_search_hit(&serde_json::json!({
        "fields": {"refRules": {"permanentRefRules": ["master"]}},
    }));
    assert!(exact.is_permanent_ref("master"));
    assert!(!exact.is_permanent_ref("feature"));
    let pattern = RepositoryRef::from_search_hit(&serde_json::json!({
        "fields": {"refRules": {"permanentRefRules": ["regexp(release/.*)"]}},
    }));
    assert!(pattern.is_permanent_ref("release/1.0"));
    assert!(!pattern.is_permanent_ref("feature"));
}

#[test]
fn reads_repository_uris() {
    let reference = RepositoryRef::from_search_hit(&serde_json::json!({
        "attachments": {"uris": {"uris": [
            {"fields": {"uri": {"effective": "ssh://host/repo"}}},
            {"fields": {"uri": {}}},
        ]}},
    }));
    assert_eq!(reference.uris(), vec!["ssh://host/repo".to_string()]);
    assert!(
        RepositoryRef::from_search_hit(&serde_json::json!({}))
            .uris()
            .is_empty()
    );
}

#[test]
fn matches_permanent_rule_patterns() {
    assert!(regexp_match("master", "master"));
    assert!(regexp_match("master", "xmasterx"));
    assert!(!regexp_match("^master$", "xmaster"));
    assert!(regexp_match("^master$", "master"));
    assert!(regexp_match("release/.*", "release/1.0"));
    assert!(!regexp_match("release/.*", "feature"));
    assert!(regexp_match("(stable|master)", "stable"));
    assert!(regexp_match("v[0-9]+", "v123"));
    assert!(!regexp_match("v[0-9]+", "v"));
    assert!(regexp_match("a{2,3}", "aaa"));
    assert!(!regexp_match("a{2,3}", "a"));
    assert!(regexp_match("a+", "aaa"));
    assert!(regexp_match("colou?r", "color"));
    assert!(regexp_match(r"\d+", "abc123"));
    // Unsupported constructs never match.
    assert!(!regexp_match("(?=a)a", "a"));
    assert!(!regexp_match("(a)\\1", "aa"));
    assert!(!regexp_match("(unclosed", "unclosed"));
}

#[test]
fn search_params_builder_nests_constraints() {
    assert_eq!(
        conduit_search_params(&serde_json::json!({"phids": ["PHID-USER-9"]}), None),
        serde_json::json!({"constraints": {"phids": ["PHID-USER-9"]}})
    );
    assert_eq!(
        conduit_search_params(
            &serde_json::json!({"ids": [123]}),
            Some(&serde_json::json!({"uris": true})),
        ),
        serde_json::json!({
            "constraints": {"ids": [123]},
            "attachments": {"uris": true},
        })
    );
}

/// Every new-style `.search` call site must send its constraints nested
/// under a top-level `constraints` key (T1153): the server rejects flat
/// constraint params with `ERR-CONDUIT-CALL`, exactly like PHP
/// `ConduitSearchFuture` wraps them.
///
/// This test enumerates the call sites textually (a method literal like
/// `"user.search"` or a `METHOD_*_SEARCH` constant at a call) so a
/// future flat addition fails loudly. `edge.search` is the one
/// flat-by-design exception (PHP sends it flat via `yieldConduit`).
#[test]
fn every_search_call_site_nests_constraints() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut entries: Vec<_> = std::fs::read_dir(&src)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    entries.sort_by_key(|entry| entry.file_name());
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with("_tests.rs") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let is_literal = line.contains(".search\"");
            let is_constant = line.contains("METHOD_") && line.contains("SEARCH");
            if !is_literal && !is_constant {
                continue;
            }
            if line.trim_start().starts_with('/') || line.contains("pub const") {
                continue;
            }
            if line.contains("edge.search") || line.contains("EDGE_SEARCH") {
                continue;
            }
            checked += 1;
            let low = index.saturating_sub(6);
            let high = (index + 7).min(lines.len());
            let window = lines[low..high].join("\n");
            let nested = window.contains("\"constraints\"")
                || window.contains("conduit_search")
                || window.contains("search_params(")
                || window.contains("search_constraints(");
            if !nested {
                failures.push(format!("{}:{}: {}", name, index + 1, line.trim()));
            }
        }
    }
    assert!(checked > 0, "search call-site scan found nothing");
    assert!(
        failures.is_empty(),
        "flat .search params (missing top-level constraints):\n{}",
        failures.join("\n")
    );
}
