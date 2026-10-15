use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_empty_and_thing_words() {
    assert_eq!(parse_args(&args(&[])).unwrap(), Some(Vec::new()));
    assert_eq!(
        parse_args(&args(&["remotes"])).unwrap(),
        Some(args(&["remotes"]))
    );
    assert_eq!(
        parse_args(&args(&["--", "big", "rock"])).unwrap(),
        Some(args(&["big", "rock"]))
    );
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_unknown_flags() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
}

#[test]
fn computes_utc_hour() {
    assert_eq!(hour_of(0), 0);
    assert_eq!(hour_of(3600 * 13 + 59), 13);
    assert_eq!(hour_of(3600 * 24 + 3600 * 5), 5);
}

#[test]
fn names_every_hour_range() {
    assert!(hour_text(6, LunarPhase::Full).contains("early morning"));
    assert!(hour_text(9, LunarPhase::Full).contains("It is morning."));
    assert!(hour_text(12, LunarPhase::Full).contains("midday"));
    assert!(hour_text(15, LunarPhase::Full).contains("afternoon"));
    assert!(hour_text(18, LunarPhase::Full).contains("dusk"));
    assert!(hour_text(22, LunarPhase::Full).contains("evening"));
}

#[test]
fn night_hours_use_lunar_phase() {
    assert!(hour_text(2, LunarPhase::New).contains("pitch black"));
    assert!(hour_text(2, LunarPhase::Full).contains("full moon"));
    assert!(hour_text(2, LunarPhase::Waxing).contains("waxing"));
    assert!(hour_text(2, LunarPhase::Waning).contains("waning"));
    assert!(hour_text(20, LunarPhase::Full).contains("full moon"));
    assert!(hour_text(0, LunarPhase::Full).contains("full moon"));
}

#[test]
fn hour_boundaries_match_php_ranges() {
    assert!(hour_text(5, LunarPhase::Full).contains("early morning"));
    assert!(hour_text(7, LunarPhase::Full).contains("early morning"));
    assert!(hour_text(4, LunarPhase::Full).contains("Night has fallen"));
    assert!(hour_text(20, LunarPhase::New).contains("Night has fallen"));
    assert!(hour_text(21, LunarPhase::New).contains("evening"));
    assert!(hour_text(23, LunarPhase::New).contains("evening"));
}

#[test]
fn approximates_lunar_cycle() {
    let new_moon = LUNAR_NEW_MOON_EPOCH as i64;
    let quarter = (LUNAR_PHASE_LENGTH / 4.0) as i64;
    let half = (LUNAR_PHASE_LENGTH / 2.0) as i64;
    assert!((lunar_fraction(new_moon)).abs() < 1e-9);
    assert_eq!(lunar_phase(new_moon), LunarPhase::New);
    assert_eq!(lunar_phase(new_moon + quarter), LunarPhase::Waxing);
    assert_eq!(lunar_phase(new_moon + half), LunarPhase::Full);
    assert_eq!(lunar_phase(new_moon + 3 * quarter), LunarPhase::Waning);
}

#[test]
fn lunar_thresholds_match_phutil() {
    let new_moon = LUNAR_NEW_MOON_EPOCH as i64;
    let full = new_moon + (LUNAR_PHASE_LENGTH / 2.0) as i64;
    // Just inside/outside the 0.02 full-moon window.
    assert_eq!(
        lunar_phase(full - (LUNAR_PHASE_LENGTH * 0.01) as i64),
        LunarPhase::Full
    );
    assert_eq!(
        lunar_phase(full - (LUNAR_PHASE_LENGTH * 0.03) as i64),
        LunarPhase::Waxing
    );
    assert_eq!(
        lunar_phase(new_moon - (LUNAR_PHASE_LENGTH * 0.01) as i64),
        LunarPhase::New
    );
    assert_eq!(
        lunar_phase(new_moon + (LUNAR_PHASE_LENGTH * 0.03) as i64),
        LunarPhase::Waxing
    );
}

#[test]
fn texts_match_php_call_sites() {
    assert_eq!(TITLE, "Arcventure");
    assert_eq!(
        clearing_text(),
        "You stand in the middle of a small clearing in the woods."
    );
    assert_eq!(
        trails_text(),
        "Several small trails and footpaths cross here, twisting away from you among the trees."
    );
    assert_eq!(
        north_markup(),
        "Just ahead to the north, you can see **remotes**.\n"
    );
    assert_eq!(
        unknown_message(&args(&["big", "rock"])),
        "You do not see \"big rock\" anywhere."
    );
    assert_eq!(
        grove_text(),
        "You follow a wide, straight path to the north and arrive in a grove of fruit trees after a few minutes of walking. The grass underfoot is thick and small insects flit through the air."
    );
    assert_eq!(
        grove_remotes_text(),
        "At the far edge of the grove, you see remotes:"
    );
    assert_eq!(
        stream_markup(),
        "Across the grove, a stream flows north toward **published** commits.\n"
    );
    assert_eq!(
        river_text(),
        "You walk along the narrow bank of the stream as it winds lazily downhill and turns east, gradually widening into a river."
    );
    assert_eq!(
        floating_text(),
        "Floating on the water, you see published commits:"
    );
    assert_eq!(
        bubbles_quiet_text(),
        "The river bubbles quietly, but you do not see any published commits anywhere."
    );
    assert_eq!(bubbles_text(), "They river bubbles peacefully.");
}

#[test]
fn remote_view_lines_use_pht_patterns() {
    let remote = RemoteInfo {
        name: "origin".to_string(),
        push_uri: Some("git@example.test:repo.git".to_string()),
        fetch_uri: None,
        push_repository: Some("repo".to_string()),
        fetch_repository: None,
    };
    assert_eq!(
        remote_view_lines(&remote),
        [
            "Push URI: git@example.test:repo.git".to_string(),
            "Push Repository: repo".to_string(),
            "Fetch URI: -".to_string(),
            "Fetch Repository: -".to_string(),
        ]
    );
}

#[test]
fn remote_ref_view_sets_object_name_and_lines() {
    let remote = RemoteInfo::new("origin".to_string(), None, None);
    let view = remote_ref_view(&remote);
    assert_eq!(view.object_name, Some("origin".to_string()));
    assert_eq!(view.title, None);
    assert_eq!(view.uri, None);
    assert_eq!(view.lines.len(), 4);
    assert!(view.children.is_empty());
}

#[test]
fn git_uri_normalization_matches_normalizer() {
    // Docblock equivalents from ArcanistRepositoryURINormalizer.
    let equivalents = [
        "ssh://user@host/repo",
        "ssh://user@host/repo/",
        "ssh://user@host:22/repo",
        "user@host:/repo",
        "ssh://user@host/repo.git",
    ];
    for uri in equivalents {
        assert_eq!(normalize_git_uri(uri), "host/repo", "for {uri}");
    }
    // Domain lowercases; the `.git` strip is case-sensitive like PHP.
    assert_eq!(normalize_git_uri("GIT@HOST:Repo.GIT"), "host/Repo.GIT");
    assert_eq!(normalize_git_uri("GIT@HOST:Repo.git"), "host/Repo");
    assert_eq!(
        normalize_git_uri("https://h.example/diffusion/X/anything.git"),
        "h.example/diffusion/X"
    );
    assert_eq!(
        normalize_git_uri("https://h.example/diffusion/123/x"),
        "h.example/diffusion/123"
    );
    assert_eq!(
        normalize_git_uri("/srv/local/repo"),
        "<void>/srv/local/repo"
    );
}

#[test]
fn repository_search_parses_names_uris_and_rules() {
    let value = serde_json::json!({
        "data": [
            {
                "phid": "PHID-REPO-1",
                "fields": {
                    "name": "repo",
                    "refRules": {"permanentRefRules": ["master"]},
                },
                "attachments": {
                    "uris": {
                        "uris": [
                            {"fields": {"uri": {"effective": "git@h.test:repo.git"}}},
                            {"fields": {"uri": {"effective": null}}},
                        ],
                    },
                },
            },
            {
                "phid": "PHID-REPO-2",
                "fields": {"name": "bare"},
                "attachments": {},
            },
        ],
    });
    assert_eq!(
        parse_repository_search(&value),
        vec![
            RepositoryRef {
                name: "repo".to_string(),
                uris: vec!["git@h.test:repo.git".to_string()],
                permanent_rules: Some(vec!["master".to_string()]),
            },
            RepositoryRef {
                name: "bare".to_string(),
                uris: Vec::new(),
                permanent_rules: None,
            },
        ]
    );
    assert!(parse_repository_search(&serde_json::json!({})).is_empty());
}

#[test]
fn repository_search_uses_constraints_shape() {
    assert_eq!(
        repository_search_params(&["git@h.test:r.git".to_string()]),
        serde_json::json!({
            "constraints": {"uris": ["git@h.test:r.git"]},
            "attachments": {"uris": true},
        })
    );
}

#[test]
fn repository_matching_uses_normalized_uris() {
    let repos = vec![RepositoryRef {
        name: "repo".to_string(),
        uris: vec!["ssh://git@h.test/repo.git".to_string()],
        permanent_rules: None,
    }];
    assert_eq!(
        match_repository(&repos, "git@h.test:repo").map(|r| r.name.as_str()),
        Some("repo")
    );
    assert_eq!(match_repository(&repos, "git@h.test:other"), None);
    assert_eq!(match_repository(&[], "git@h.test:repo"), None);
}

#[test]
fn remote_uri_collection_dedupes() {
    let remotes = vec![
        RemoteInfo::new(
            "origin".to_string(),
            Some("u".to_string()),
            Some("u".to_string()),
        ),
        RemoteInfo::new("up".to_string(), None, Some("v".to_string())),
        RemoteInfo::new("bare".to_string(), None, None),
    ];
    assert_eq!(
        collect_remote_uris(&remotes),
        vec!["u".to_string(), "v".to_string()]
    );
}

#[test]
fn fill_repositories_maps_push_and_fetch() {
    let repos = vec![RepositoryRef {
        name: "repo".to_string(),
        uris: vec!["git@h.test:repo.git".to_string()],
        permanent_rules: None,
    }];
    let mut remotes = vec![RemoteInfo::new(
        "origin".to_string(),
        Some("ssh://git@h.test/repo".to_string()),
        Some("https://h.test/other".to_string()),
    )];
    fill_remote_repositories(&mut remotes, &repos);
    assert_eq!(remotes[0].push_repository, Some("repo".to_string()));
    assert_eq!(remotes[0].fetch_repository, None);
}

#[test]
fn resolve_repositories_skips_empty_search() {
    let calls: std::cell::Cell<u32> = std::cell::Cell::new(0);
    let call = |_method: &str, _params: &serde_json::Value| {
        calls.set(calls.get() + 1);
        Ok(serde_json::json!({}))
    };
    assert_eq!(resolve_repositories(&call, &[]).unwrap(), Vec::new());
    assert_eq!(calls.get(), 0);
}

#[test]
fn resolve_repositories_searches_by_uri() {
    let seen: std::cell::RefCell<Vec<(String, serde_json::Value)>> =
        std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        seen.borrow_mut().push((method.to_string(), params.clone()));
        Ok(serde_json::json!({"data": []}))
    };
    let uris = vec!["u".to_string()];
    assert_eq!(resolve_repositories(&call, &uris).unwrap(), Vec::new());
    assert_eq!(
        seen.borrow().as_slice(),
        &[(
            "diffusion.repository.search".to_string(),
            repository_search_params(&uris),
        )]
    );
}

#[test]
fn marker_parsing_splits_remote_and_branch() {
    let out = "refs/remotes/origin/master abc123\nrefs/remotes/up/feat/x def456\n";
    assert_eq!(
        parse_marker_refs(out),
        vec![
            Marker {
                remote: "origin".to_string(),
                name: "master".to_string(),
                hash: "abc123".to_string(),
            },
            Marker {
                remote: "up".to_string(),
                name: "feat/x".to_string(),
                hash: "def456".to_string(),
            },
        ]
    );
    assert!(parse_marker_refs("refs/heads/main abc123\n").is_empty());
    assert!(parse_marker_refs("refs/remotes/lonely abc123\n").is_empty());
    assert!(parse_marker_refs("garbage\n").is_empty());
}

#[test]
fn permanent_ref_rules_match_repository_ref() {
    assert!(!is_permanent_ref(None, "master"));
    assert!(is_permanent_ref(Some(&[]), "anything"));
    assert!(is_permanent_ref(Some(&["master".to_string()]), "master"));
    assert!(!is_permanent_ref(Some(&["master".to_string()]), "develop"));
    assert!(!is_permanent_ref(
        Some(&["regexp(/.*/)".to_string()]),
        "master"
    ));
}

#[test]
fn renders_clearing_with_trails_hint() {
    // 2026-01-01T12:00:00Z is midday UTC.
    let out = render_clearing(1_767_268_800);
    assert!(out.contains("small clearing in the woods"), "{out}");
    assert!(out.contains("midday"), "{out}");
    assert!(out.contains("small trails and footpaths"), "{out}");
    assert!(out.contains("you can see remotes"), "{out}");
}

#[test]
fn renders_unknown_thing() {
    assert_eq!(
        render_unknown(&args(&["big", "rock"])),
        "You do not see \"big rock\" anywhere.\n"
    );
}

#[test]
fn renders_remote_block() {
    let remote = RemoteInfo {
        name: "origin".to_string(),
        push_uri: Some("git@example.test:repo.git".to_string()),
        fetch_uri: None,
        push_repository: None,
        fetch_repository: None,
    };
    let out = render_remote(&remote);
    assert!(out.contains("Remote \"origin\":"), "{out}");
    assert!(out.contains("Push URI: git@example.test:repo.git"), "{out}");
    assert!(out.contains("Fetch URI: -"), "{out}");
    assert!(out.contains("Push Repository: -"), "{out}");
    assert!(out.contains("Fetch Repository: -"), "{out}");
}

#[test]
fn renders_published_and_empty_river() {
    let out = render_published(&["abc123".to_string()]);
    assert!(out.contains("Floating on the water"), "{out}");
    assert!(out.contains("abc123"), "{out}");
    assert!(out.contains("They river bubbles peacefully."), "{out}");

    let out = render_published(&[]);
    assert!(out.contains("bubbles quietly"), "{out}");
    assert!(!out.contains("Floating"), "{out}");
}

#[test]
fn styled_renders_match_console_output() {
    assert_eq!(render_title(false).unwrap(), " <!> Arcventure \n\n");
    assert_eq!(
        render_paragraph(grove_text(), false).unwrap(),
        "You follow a wide, straight path to the north and arrive in a grove of fruit\ntrees after a few minutes of walking. The grass underfoot is thick and small\ninsects flit through the air.\n\n"
    );
    assert_eq!(
        render_markup(north_markup(), false).unwrap(),
        "Just ahead to the north, you can see remotes.\n"
    );
    assert_eq!(render_line("abc123", false).unwrap(), "abc123\n");
    assert_eq!(
        render_quiet_river(false).unwrap(),
        "The river bubbles quietly, but you do not see any published commits anywhere.\n"
    );
    assert_eq!(
        render_bubbles_tail(false).unwrap(),
        "\nThey river bubbles peacefully.\n"
    );
    let remote = RemoteInfo::new(
        "origin".to_string(),
        Some("git@example.test:repo.git".to_string()),
        Some("git@example.test:repo.git".to_string()),
    );
    assert_eq!(
        render_remote_view(&remote, 80, false),
        "  *   origin\n        Push URI: git@example.test:repo.git\n        Push Repository: -\n        Fetch URI: git@example.test:repo.git\n        Fetch Repository: -\n"
    );
}

#[test]
fn published_hashes_keep_permanent_push_refs() {
    let markers = vec![
        Marker {
            remote: "origin".to_string(),
            name: "master".to_string(),
            hash: "aaa".to_string(),
        },
        Marker {
            remote: "origin".to_string(),
            name: "wip".to_string(),
            hash: "bbb".to_string(),
        },
        Marker {
            remote: "ghost".to_string(),
            name: "master".to_string(),
            hash: "ccc".to_string(),
        },
    ];
    let remotes = vec![RemoteInfo::new(
        "origin".to_string(),
        Some("git@h.test:repo.git".to_string()),
        None,
    )];
    let repos = vec![RepositoryRef {
        name: "repo".to_string(),
        uris: vec!["ssh://git@h.test/repo".to_string()],
        permanent_rules: Some(vec!["master".to_string()]),
    }];
    assert_eq!(
        published_hashes(&markers, &remotes, &repos),
        vec!["aaa".to_string()]
    );
    assert!(published_hashes(&markers, &remotes, &[]).is_empty());
}
