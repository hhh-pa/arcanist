use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_empty_and_thing_words() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_args_with_tty(&args(&[]), true).unwrap(),
        Some(Vec::new())
    );
    assert_eq!(
        parse_args_with_tty(&args(&["remotes"]), true).unwrap(),
        Some(args(&["remotes"]))
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--", "big", "rock"]), true).unwrap(),
        Some(args(&["big", "rock"]))
    );
}

#[test]
fn parses_help() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(parse_args_with_tty(&args(&["--help"]), true).unwrap(), None);
    assert_eq!(parse_args_with_tty(&args(&["-h"]), true).unwrap(), None);
}

#[test]
fn rejects_unknown_flags() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert!(parse_args_with_tty(&args(&["--bogus"]), true).is_err());
}

#[test]
fn b21_look_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv. Unknown flags still report first.
    assert_eq!(
        parse_args_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_args_with_tty(&args(&["remotes"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    let err = parse_args_with_tty(&args(&["--bogus"]), false).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    assert!(
        parse_args_with_tty(&args(&["--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(
        parse_args_with_tty(&args(&["--", "remotes"]), false)
            .unwrap()
            .is_some()
    );
    assert!(parse_args_with_tty(&args(&[]), true).unwrap().is_some());
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn b23_look_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help look`.
    let text = usage_text(false);
    assert!(text.starts_with("LOOK WORKFLOW\n"), "{text}");
    assert!(text.contains("small clearing"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
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
fn rbd10_normalization_follows_phutil_uri_edges() {
    // The shared `revision` port decides the `PhutilURI` edges: dotless
    // `host:path` is a scheme (not scp), digit tails are ports (not
    // paths), query strings never reach the path, garbage ports void
    // the URI, and drive letters split as schemes.
    for uri in [
        "localhost:repo",
        "host:22",
        "a:b:c",
        "ssh://host/repo?x=1",
        "ssh://host:abc/repo",
        "ssh://host:22/repo",
        "C:\\repo",
        "C:/repo",
        "user@host:/repo",
        "git@domain.com:bananas.git",
    ] {
        assert_eq!(
            normalize_git_uri(uri),
            crate::revision::normalize_git_uri(uri),
            "for {uri}"
        );
    }
    assert_eq!(normalize_git_uri("localhost:repo"), "<void>/repo");
    assert_eq!(normalize_git_uri("host:22"), "<void>/22");
    assert_eq!(normalize_git_uri("ssh://host/repo?x=1"), "host/repo");
    assert_eq!(normalize_git_uri("ssh://host:abc/repo"), "<void>/");
    assert_eq!(normalize_git_uri("C:\\repo"), "<void>/\\repo");
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
    // T1153 RB-D11: normalized comparison, last match wins on
    // collisions (like PHP's `$uri_map` overwrite).
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
    let colliding = vec![
        RepositoryRef {
            name: "first".to_string(),
            uris: vec!["ssh://git@h.test/repo.git".to_string()],
            permanent_rules: None,
        },
        RepositoryRef {
            name: "second".to_string(),
            uris: vec!["ssh://git@h.test/repo.git".to_string()],
            permanent_rules: None,
        },
    ];
    assert_eq!(
        match_repository(&colliding, "git@h.test:repo").map(|r| r.name.as_str()),
        Some("second")
    );
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

/// 2026-01-15T12:00:00Z (northern winter).
const WINTER: u64 = 1_768_478_400;
/// 2026-07-15T12:00:00Z (northern summer).
const SUMMER: u64 = 1_784_116_800;
/// Heartbeats around the 2026 US spring-forward (Mar 8, 07:00:00Z).
const SPRING_BEFORE: u64 = 1_772_953_199;
const SPRING_AFTER: u64 = 1_772_953_201;
/// Heartbeats around the 2026 US fall-back (Nov 1, 06:00:00Z).
const FALL_BEFORE: u64 = 1_793_512_799;
const FALL_AFTER: u64 = 1_793_512_801;
/// 2000-07-01T12:00:00Z, before the fixtures' first transitions.
const PRE_FIRST: u64 = 962_452_800;
/// 2030-07-01T12:00:00Z, after the fixtures' last transitions.
const POST_LAST: u64 = 1_909_137_600;

/// US Eastern POSIX rules with explicit 02:00 transition times.
const EASTERN_POSIX: &str = "EST5EDT,M3.2.0/2,M11.1.0/2";

/// `zic` output for a 2007-max Eastern zone (slim: one explicit
/// transition plus the `EST5EDT,M3.2.0,M11.1.0` footer).
const TESTBURG_SLIM: &[u8] = &[
    0x54, 0x5a, 0x69, 0x66, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x54, 0x5a, 0x69, 0x66, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00,
    0x00, 0x00, 0x00, 0x45, 0xf3, 0xa8, 0xf0, 0x00, 0xff, 0xff, 0xc7, 0xc0, 0x01, 0x00, 0x45, 0x44,
    0x54, 0x00, 0x0a, 0x45, 0x53, 0x54, 0x35, 0x45, 0x44, 0x54, 0x2c, 0x4d, 0x33, 0x2e, 0x32, 0x2e,
    0x30, 0x2c, 0x4d, 0x31, 0x31, 0x2e, 0x31, 0x2e, 0x30, 0x0a,
];

/// `zic -b fat` output for a 2025-2027 Eastern zone (six explicit
/// transitions plus the fixed `EST5` footer).
const TESTBURG_FAT: &[u8] = &[
    0x54, 0x5a, 0x69, 0x66, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x08, 0x67, 0xcd, 0x3c, 0x70,
    0x69, 0x06, 0xf3, 0x60, 0x69, 0xad, 0x1e, 0x70, 0x6a, 0xe6, 0xd5, 0x60, 0x6b, 0x96, 0x3a, 0xf0,
    0x6c, 0xcf, 0xf1, 0xe0, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0xff, 0xff, 0xb9, 0xb0, 0x00, 0x04,
    0xff, 0xff, 0xc7, 0xc0, 0x01, 0x00, 0xff, 0xff, 0xc7, 0xc0, 0x01, 0x00, 0xff, 0xff, 0xb9, 0xb0,
    0x00, 0x04, 0x45, 0x44, 0x54, 0x00, 0x45, 0x53, 0x54, 0x00, 0x54, 0x5a, 0x69, 0x66, 0x32, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00,
    0x00, 0x04, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x67, 0xcd, 0x3c, 0x70, 0x00, 0x00,
    0x00, 0x00, 0x69, 0x06, 0xf3, 0x60, 0x00, 0x00, 0x00, 0x00, 0x69, 0xad, 0x1e, 0x70, 0x00, 0x00,
    0x00, 0x00, 0x6a, 0xe6, 0xd5, 0x60, 0x00, 0x00, 0x00, 0x00, 0x6b, 0x96, 0x3a, 0xf0, 0x00, 0x00,
    0x00, 0x00, 0x6c, 0xcf, 0xf1, 0xe0, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0xff, 0xff, 0xb9, 0xb0,
    0x00, 0x04, 0xff, 0xff, 0xc7, 0xc0, 0x01, 0x00, 0xff, 0xff, 0xc7, 0xc0, 0x01, 0x00, 0xff, 0xff,
    0xb9, 0xb0, 0x00, 0x04, 0x45, 0x44, 0x54, 0x00, 0x45, 0x53, 0x54, 0x00, 0x0a, 0x45, 0x53, 0x54,
    0x35, 0x0a,
];

/// Minimal v1 (32-bit, footerless) Eastern zone with the two 2026
/// transitions, verified with `zdump`.
const TESTBURG_V1: &[u8] = &[
    0x54, 0x5a, 0x69, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x08, 0x69, 0xad, 0x1e, 0x70,
    0x6a, 0xe6, 0xd5, 0x60, 0x01, 0x00, 0xff, 0xff, 0xb9, 0xb0, 0x00, 0x00, 0xff, 0xff, 0xc7, 0xc0,
    0x01, 0x04, 0x45, 0x53, 0x54, 0x00, 0x45, 0x44, 0x54, 0x00,
];

#[test]
fn applies_utc_offsets_to_hours() {
    assert_eq!(hour_with_offset(0, 0), 0);
    assert_eq!(hour_with_offset(3_600 * 13 + 59, 0), 13);
    assert_eq!(hour_with_offset(0, 3_600), 1);
    assert_eq!(hour_with_offset(0, -3_600), 23);
    // Half-hour zones and the date line wrap within the day.
    assert_eq!(hour_with_offset(12 * 3_600, 19_800), 17);
    assert_eq!(hour_with_offset(12 * 3_600, -19_800), 6);
    assert_eq!(hour_with_offset(0, 14 * 3_600), 14);
    assert_eq!(hour_with_offset(0, -12 * 3_600), 12);
    assert_eq!(hour_with_offset(3_600 * 24 - 1, 1), 0);
}

#[test]
fn civil_calendar_round_trips() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2026, 1, 1), 20_454);
    assert_eq!(days_from_civil(2026, 3, 8), 20_520);
    for (year, month, day) in [
        (1970, 1, 1),
        (2026, 1, 15),
        (2000, 2, 29),
        (1900, 2, 28),
        (1969, 12, 31),
        (2038, 1, 19),
    ] {
        assert_eq!(
            civil_from_days(days_from_civil(year, month, day)),
            (year, month, day)
        );
    }
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
    assert!(is_leap_year(2000));
    assert!(!is_leap_year(1900));
    assert!(is_leap_year(2024));
    assert!(!is_leap_year(2026));
    // 2026-03-08 is a Sunday.
    assert_eq!(weekday_sunday_zero(20_520), 0);
}

#[test]
fn computes_dst_rule_dates() {
    // Second Sunday of March 2026 and first Sunday of November 2026.
    assert_eq!(
        dst_rule_month_day(
            DstRule::MonthWeekday {
                month: 3,
                week: 2,
                day: 0
            },
            2026
        ),
        (3, 8)
    );
    assert_eq!(
        dst_rule_month_day(
            DstRule::MonthWeekday {
                month: 11,
                week: 1,
                day: 0
            },
            2026
        ),
        (11, 1)
    );
    // Week 5 means the last matching weekday of the month.
    assert_eq!(
        dst_rule_month_day(
            DstRule::MonthWeekday {
                month: 10,
                week: 5,
                day: 0
            },
            2026
        ),
        (10, 25)
    );
    assert_eq!(
        dst_rule_month_day(
            DstRule::MonthWeekday {
                month: 2,
                week: 5,
                day: 0
            },
            2026
        ),
        (2, 22)
    );
    // Julian days skip (J) or count (n) the leap day.
    assert_eq!(dst_rule_month_day(DstRule::JulianSkip(60), 2026), (3, 1));
    assert_eq!(dst_rule_month_day(DstRule::JulianSkip(60), 2000), (3, 1));
    assert_eq!(dst_rule_month_day(DstRule::JulianCount(59), 2026), (3, 1));
    assert_eq!(dst_rule_month_day(DstRule::JulianCount(59), 2000), (2, 29));
    assert_eq!(
        dst_rule_month_day(DstRule::JulianCount(365), 2026),
        (12, 31)
    );
}

#[test]
fn parses_posix_zone_strings() {
    let eastern = parse_posix_tz(EASTERN_POSIX).expect("parse eastern");
    assert_eq!(eastern.std_offset, -18_000);
    let dst = eastern.dst.expect("eastern dst");
    assert_eq!(dst.offset, -14_400);
    assert_eq!(
        dst.start,
        (
            DstRule::MonthWeekday {
                month: 3,
                week: 2,
                day: 0
            },
            7_200
        )
    );
    assert_eq!(
        dst.end,
        (
            DstRule::MonthWeekday {
                month: 11,
                week: 1,
                day: 0
            },
            7_200
        )
    );
    // Fixed zones have no daylight half.
    assert_eq!(parse_posix_tz("EST5").expect("parse").std_offset, -18_000);
    assert!(parse_posix_tz("EST5").expect("parse").dst.is_none());
    // A bare name needs the zoneinfo file (the offset is required, like
    // glibc); `TZ=UTC` still resolves through the file or UTC fallback.
    assert_eq!(parse_posix_tz("UTC"), None);
    assert_eq!(parse_posix_tz("UTC0").expect("parse").std_offset, 0);
    // POSIX offsets count west as positive, so negative values sit east.
    assert_eq!(parse_posix_tz("CET-1").expect("parse").std_offset, 3_600);
    assert_eq!(
        parse_posix_tz("NST3:30").expect("parse").std_offset,
        -12_600
    );
    assert_eq!(parse_posix_tz("<-03>3").expect("parse").std_offset, -10_800);
    // Bare `dst` defaults to one hour east with US rules.
    let bare = parse_posix_tz("EST5EDT").expect("parse");
    let bare_dst = bare.dst.expect("bare dst");
    assert_eq!(bare_dst.offset, -14_400);
    assert_eq!(
        bare_dst.start.0,
        DstRule::MonthWeekday {
            month: 3,
            week: 2,
            day: 0
        }
    );
    assert_eq!(
        bare_dst.end.0,
        DstRule::MonthWeekday {
            month: 11,
            week: 1,
            day: 0
        }
    );
    // Julian rule spellings.
    let julian = parse_posix_tz("EST5EDT,J59/2,J300/2")
        .expect("parse")
        .dst
        .expect("dst");
    assert_eq!(julian.start.0, DstRule::JulianSkip(59));
    assert_eq!(julian.end.0, DstRule::JulianSkip(300));
    let counted = parse_posix_tz("EST5EDT,59/2,300/2")
        .expect("parse")
        .dst
        .expect("dst");
    assert_eq!(counted.start.0, DstRule::JulianCount(59));
    // Malformed strings fail closed (the lookup falls back to UTC).
    for bad in [
        "",
        "5EST",
        "EST",
        "<>",
        "EST5EDT,",
        "EST5EDT,M3.2.0",
        "EST5EDT,M13.1.0/2,M11.1.0/2",
        "EST5EDT,M3.2.0/2,M11.1.0/2,",
    ] {
        assert_eq!(parse_posix_tz(bad), None, "for {bad:?}");
    }
}

#[test]
fn evaluates_posix_offsets_at_boundaries() {
    for (now, want) in [
        (WINTER, -18_000),
        (SUMMER, -14_400),
        (SPRING_BEFORE, -18_000),
        (SPRING_AFTER, -14_400),
        (FALL_BEFORE, -14_400),
        (FALL_AFTER, -18_000),
    ] {
        assert_eq!(
            posix_offset(EASTERN_POSIX, now as i64),
            Some(want),
            "for {now}"
        );
    }
    // Fixed zones ignore the calendar; southern zones wrap the year end.
    assert_eq!(posix_offset("EST5", SUMMER as i64), Some(-18_000));
    assert_eq!(posix_offset("UTC0", SUMMER as i64), Some(0));
    assert_eq!(
        posix_offset("AEST-10AEDT,M10.1.0/2,M4.1.0/2", WINTER as i64),
        Some(39_600)
    );
    assert_eq!(
        posix_offset("AEST-10AEDT,M10.1.0/2,M4.1.0/2", SUMMER as i64),
        Some(36_000)
    );
    assert_eq!(posix_offset("bogus", WINTER as i64), None);
}

#[test]
fn reads_zic_slim_footer_zone() {
    assert_eq!(TESTBURG_SLIM.len(), 138);
    let parsed = parse_tzif(TESTBURG_SLIM).expect("parse slim");
    // One explicit transition (2007-03-11T07:00:00Z); everything modern
    // resolves through the footer.
    assert_eq!(parsed.transitions, vec![(1_173_596_400, 0)]);
    assert_eq!(parsed.footer.as_deref(), Some("EST5EDT,M3.2.0,M11.1.0"));
    for (now, want) in [
        (WINTER, -18_000),
        (SUMMER, -14_400),
        (SPRING_BEFORE, -18_000),
        (SPRING_AFTER, -14_400),
        (FALL_BEFORE, -14_400),
        (FALL_AFTER, -18_000),
        (POST_LAST, -14_400),
    ] {
        assert_eq!(
            tzif_offset(TESTBURG_SLIM, now as i64),
            Some(want),
            "for {now}"
        );
    }
    // Before the first transition the only type (daylight) applies.
    assert_eq!(tzif_offset(TESTBURG_SLIM, PRE_FIRST as i64), Some(-14_400));
}

#[test]
fn reads_zic_fat_explicit_zone() {
    assert_eq!(TESTBURG_FAT.len(), 242);
    let parsed = parse_tzif(TESTBURG_FAT).expect("parse fat");
    assert_eq!(parsed.transitions.len(), 6);
    assert_eq!(parsed.footer.as_deref(), Some("EST5"));
    for (now, want) in [
        (WINTER, -18_000),
        (SUMMER, -14_400),
        (SPRING_BEFORE, -18_000),
        (SPRING_AFTER, -14_400),
        (FALL_BEFORE, -14_400),
        (FALL_AFTER, -18_000),
    ] {
        assert_eq!(
            tzif_offset(TESTBURG_FAT, now as i64),
            Some(want),
            "for {now}"
        );
    }
    // Before the first transition the standard type applies; after the
    // last one the fixed `EST5` footer does.
    assert_eq!(tzif_offset(TESTBURG_FAT, PRE_FIRST as i64), Some(-18_000));
    assert_eq!(tzif_offset(TESTBURG_FAT, POST_LAST as i64), Some(-18_000));
}

#[test]
fn reads_v1_narrow_zone() {
    assert_eq!(TESTBURG_V1.len(), 74);
    let parsed = parse_tzif(TESTBURG_V1).expect("parse v1");
    assert_eq!(parsed.transitions.len(), 2);
    assert_eq!(parsed.footer, None);
    for (now, want) in [
        (WINTER, -18_000),
        (SUMMER, -14_400),
        (SPRING_BEFORE, -18_000),
        (SPRING_AFTER, -14_400),
        (FALL_BEFORE, -14_400),
        (FALL_AFTER, -18_000),
        (PRE_FIRST, -18_000),
        // Footerless files hold the last type past the last transition.
        (POST_LAST, -18_000),
    ] {
        assert_eq!(
            tzif_offset(TESTBURG_V1, now as i64),
            Some(want),
            "for {now}"
        );
    }
}

#[test]
fn rejects_corrupt_zoneinfo() {
    assert_eq!(parse_tzif(b""), None);
    assert_eq!(parse_tzif(b"TZif"), None);
    assert_eq!(parse_tzif(&TESTBURG_SLIM[..40]), None);
    assert_eq!(parse_tzif(&TESTBURG_FAT[..100]), None);
    let mut bad = TESTBURG_FAT.to_vec();
    bad[0] = b'X';
    assert_eq!(parse_tzif(&bad), None);
    bad = TESTBURG_FAT.to_vec();
    bad[4] = b'9';
    assert_eq!(parse_tzif(&bad), None);
}

#[test]
fn reads_system_new_york_when_present() {
    // Unix zoneinfo layout; skipped silently where it does not exist
    // (Windows). Expectations pin the file's zone, never the machine's.
    let Ok(bytes) = std::fs::read("/usr/share/zoneinfo/America/New_York") else {
        return;
    };
    for (now, want) in [
        (WINTER, -18_000),
        (SUMMER, -14_400),
        (SPRING_BEFORE, -18_000),
        (SPRING_AFTER, -14_400),
        (FALL_BEFORE, -14_400),
        (FALL_AFTER, -18_000),
    ] {
        assert_eq!(tzif_offset(&bytes, now as i64), Some(want), "for {now}");
    }
}

/// Zone-independent sanity for the machine zone: current IANA offsets
/// stay within UTC-12..UTC+14 at minute granularity.
fn assert_local_invariants() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(WINTER);
    let offset = local_utc_offset(now);
    assert!((-43_200..=50_400).contains(&offset), "offset {offset}");
    assert_eq!(offset % 60, 0, "offset {offset}");
    assert_eq!(local_utc_offset(now), offset);
    assert_eq!(local_hour(now), hour_with_offset(now, offset));
}

#[test]
fn local_lookup_invariants_hold_in_machine_zone() {
    assert_local_invariants();
}

/// Probe body for `fixed_tz_subprocesses_match_oracles`: asserts exact
/// offsets under a pinned `TZ`, or machine-zone invariants otherwise.
/// Never spawns; the parent filter runs only this test in each child.
#[test]
fn tz_probe_child_fixed() {
    println!("LOOK-TZ-PROBE-OK");
    match std::env::var("TZ").as_deref() {
        Ok("UTC0") => {
            for now in [
                WINTER,
                SUMMER,
                SPRING_BEFORE,
                SPRING_AFTER,
                FALL_BEFORE,
                FALL_AFTER,
            ] {
                assert_eq!(local_utc_offset(now), 0, "for {now}");
                assert_eq!(local_hour(now), hour_of(now), "for {now}");
            }
        }
        Ok("EST5") => {
            for now in [
                WINTER,
                SUMMER,
                SPRING_BEFORE,
                SPRING_AFTER,
                FALL_BEFORE,
                FALL_AFTER,
            ] {
                assert_eq!(local_utc_offset(now), -18_000, "for {now}");
            }
            assert_eq!(local_hour(WINTER), 7);
            assert_eq!(local_hour(SUMMER), 7);
        }
        Ok(tz) if tz == EASTERN_POSIX => {
            assert_eq!(local_utc_offset(WINTER), -18_000);
            assert_eq!(local_utc_offset(SUMMER), -14_400);
            assert_eq!(local_utc_offset(SPRING_BEFORE), -18_000);
            assert_eq!(local_utc_offset(SPRING_AFTER), -14_400);
            assert_eq!(local_utc_offset(FALL_BEFORE), -14_400);
            assert_eq!(local_utc_offset(FALL_AFTER), -18_000);
            assert_eq!(local_hour(WINTER), 7);
            assert_eq!(local_hour(SUMMER), 8);
        }
        _ => assert_local_invariants(),
    }
}

/// Fixed-`TZ` subprocess oracle: each child re-runs only the probe test
/// with a pinned zone, so the suite never mutates its own `TZ`.
#[test]
fn fixed_tz_subprocesses_match_oracles() {
    let exe = std::env::current_exe().expect("test binary path");
    for tz in ["UTC0", "EST5", EASTERN_POSIX] {
        let out = std::process::Command::new(&exe)
            .env("TZ", tz)
            .args(["--nocapture", "tz_probe_child_fixed"])
            .output()
            .expect("spawn probe child");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success()
                && stdout.contains("1 passed")
                && stdout.contains("LOOK-TZ-PROBE-OK"),
            "TZ={tz} probe child failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
    }
}

#[test]
fn r3_rbd11_resolve_repositories_follows_after_cursor() {
    fn repo_row(name: &str, uri: &str) -> serde_json::Value {
        serde_json::json!({
            "fields": {"name": name},
            "attachments": {"uris": {"uris": [
                {"fields": {"uri": {"effective": uri}}},
            ]}},
        })
    }
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "diffusion.repository.search");
        seen.borrow_mut().push(params.clone());
        if seen.borrow().len() == 1 {
            Ok(serde_json::json!({
                "data": [repo_row("one", "git@h.test:one.git")],
                "cursor": {"after": "p2"},
            }))
        } else {
            Ok(serde_json::json!({
                "data": [repo_row("two", "git@h.test:two.git")],
            }))
        }
    };
    let uris = vec!["git@h.test:one.git".to_string()];
    let repos = resolve_repositories(&call, &uris).unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].name, "one");
    assert_eq!(repos[1].name, "two");
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].get("after").is_none());
    assert_eq!(seen[1]["after"], serde_json::json!("p2"));
    assert_eq!(
        seen[1]["attachments"],
        serde_json::json!({"uris": true}),
        "attachments survive onto page two"
    );
}

#[test]
fn r3_rbd11_match_repository_prefers_last_on_collision() {
    let repos = vec![
        RepositoryRef {
            name: "first".to_string(),
            uris: vec!["git@h.test:repo.git".to_string()],
            permanent_rules: None,
        },
        RepositoryRef {
            name: "second".to_string(),
            uris: vec!["ssh://git@h.test/repo".to_string()],
            permanent_rules: None,
        },
    ];
    assert_eq!(
        match_repository(&repos, "git@h.test:repo").map(|r| r.name.as_str()),
        Some("second")
    );
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `look` runs in the toolset runtime: argument errors render
    // `USAGE EXCEPTION` (exit 1) with no usage dump.
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}
#[test]
fn r10_look_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "LOOK WORKFLOW\n\
         \n\
         look\n\
         look [options] -- thing\n\
         You stand in the middle of a small clearing.\n\
         \n\
         Look around, or look at a specific thing.\n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("LOOK WORKFLOW"), "{}", usage());
}

/// Unique scratch dir (pid + nanos + counter), removed on drop.
struct LookTmp {
    path: std::path::PathBuf,
}

impl LookTmp {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-look-{tag}-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for LookTmp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn look_git(dir: &std::path::Path, argv: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(argv)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        argv.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn look_repo(tag: &str) -> LookTmp {
    let repo = LookTmp::new(tag);
    look_git(repo.path(), &["init", "-q", "-b", "master"]);
    look_git(repo.path(), &["config", "user.email", "t@t"]);
    look_git(repo.path(), &["config", "user.name", "t"]);
    look_git(repo.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.path().join("f.txt"), "one\n").unwrap();
    look_git(repo.path(), &["add", "f.txt"]);
    look_git(repo.path(), &["commit", "-qm", "add f"]);
    repo
}

fn write_arcconfig(dir: &std::path::Path, uri: &str) {
    std::fs::write(
        dir.join(".arcconfig"),
        format!("{{\"phabricator.uri\": \"{uri}\"}}"),
    )
    .unwrap();
}

fn look_envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

/// Serve `expected` requests from `routes` (method -> envelope); returns
/// the base URI. (Local replicate of the `export_tests` stub.)
fn look_serve(routes: std::collections::HashMap<String, String>, expected: usize) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head_text = String::from_utf8_lossy(&head).to_string();
            let len: usize = head_text
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(key, value)| {
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let method = head_text
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .strip_prefix("/api/")
                .unwrap_or("")
                .to_string();
            let canned = routes
                .get(&method)
                .cloned()
                .unwrap_or_else(|| r#"{"result": null, "error_code": "ERR-NO-ROUTE"}"#.to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    base
}

#[test]
fn dst_rule_heads_reject_out_of_range_days() {
    // `J` days run 1-365 and counted days 0-365; anything else fails
    // closed (the lookup falls back to UTC).
    for bad in ["J0", "J366", "366"] {
        assert_eq!(take_dst_rule_head(bad), None, "for {bad:?}");
    }
    for good in ["J1", "J365", "0", "365"] {
        assert!(take_dst_rule_head(good).is_some(), "for {good:?}");
    }
    assert_eq!(parse_posix_tz("EST5EDT,J0/2,M11.1.0/2"), None);
    assert_eq!(parse_posix_tz("EST5EDT,366/2,M11.1.0/2"), None);
}

#[test]
fn parses_posix_dst_with_explicit_offset() {
    // An explicit daylight offset wins over the one-hour-east default.
    let dst = parse_posix_tz("EST5EDT3,M3.2.0/2,M11.1.0/2")
        .expect("parse")
        .dst
        .expect("dst");
    assert_eq!(dst.offset, -10_800);
    // ...including with the default US rules.
    let bare = parse_posix_tz("EST5EDT3").expect("parse").dst.expect("dst");
    assert_eq!(bare.offset, -10_800);
    assert_eq!(
        posix_offset("EST5EDT3,M3.2.0/2,M11.1.0/2", SUMMER as i64),
        Some(-10_800)
    );
}

#[test]
fn posix_offset_ignores_coincident_transitions() {
    // Start and end at the same instant (02:00 standard == 03:00
    // daylight on the same day) means no daylight saving at all.
    let tz = "EST5EDT,M3.2.0/2,M3.2.0/3";
    assert_eq!(posix_offset(tz, WINTER as i64), Some(-18_000));
    assert_eq!(posix_offset(tz, SUMMER as i64), Some(-18_000));
}

#[test]
fn rejects_transition_index_past_type_table() {
    // Type indices past the table fail the whole file closed.
    let mut bad = TESTBURG_V1.to_vec();
    // Two transition times follow the 44-byte header; the two type
    // indices sit right after them.
    bad[52] = 0xff;
    bad[53] = 0xff;
    assert_eq!(parse_tzif(&bad), None);
    assert_eq!(tzif_offset(&bad, WINTER as i64), None);
}

#[test]
fn rejects_zone_without_time_types() {
    // A block with no local-time types cannot resolve anything.
    let mut bad = TESTBURG_V1.to_vec();
    // Header counts: transitions at 32..36, types at 36..40.
    bad[32..36].copy_from_slice(&0u32.to_be_bytes());
    bad[36..40].copy_from_slice(&0u32.to_be_bytes());
    assert_eq!(parse_tzif(&bad), None);
}

#[test]
fn ignores_footer_with_embedded_newline() {
    // A v2+ footer whose body holds a newline is not well-formed, so
    // the file parses footerless and holds its last explicit type.
    let mut bad = TESTBURG_FAT.to_vec();
    let end = bad.len();
    // Footer "\nEST5\n": puncture the body with a second newline.
    bad[end - 4] = b'\n';
    let parsed = parse_tzif(&bad).expect("parse footerless");
    assert_eq!(parsed.footer, None);
    assert_eq!(tzif_offset(&bad, WINTER as i64), Some(-18_000));
}

#[test]
fn colon_tz_reads_zone_file_directly() {
    let tmp = LookTmp::new("tzfile");
    let path = tmp.path().join("v1.tz");
    std::fs::write(&path, TESTBURG_V1).unwrap();
    let spec = format!(":{}", path.display());
    assert_eq!(env_tz_offset(&spec, WINTER as i64), -18_000);
    assert_eq!(env_tz_offset(&spec, SUMMER as i64), -14_400);
    // Missing or corrupt files fall back to UTC.
    assert_eq!(env_tz_offset(":/nonexistent-look-tz", WINTER as i64), 0);
    let bad_path = tmp.path().join("bad.tz");
    std::fs::write(&bad_path, b"not a tzfile").unwrap();
    assert_eq!(
        env_tz_offset(&format!(":{}", bad_path.display()), WINTER as i64),
        0
    );
}

#[test]
fn slash_tz_name_reads_literal_path_first() {
    // A `TZ` name with a slash is also tried as a literal path, like
    // glibc, before the zoneinfo directories.
    let tmp = LookTmp::new("tzslash");
    let path = tmp.path().join("v1.tz");
    std::fs::write(&path, TESTBURG_V1).unwrap();
    let name = path.to_str().expect("utf8 tmp path").to_string();
    assert!(name.contains('/'));
    assert_eq!(env_tz_offset(&name, WINTER as i64), -18_000);
    assert_eq!(env_tz_offset(&name, SUMMER as i64), -14_400);
    // First parseable file wins; missing ones are skipped.
    assert_eq!(
        files_tz_offset(&["/nonexistent-look-tz".to_string(), name], WINTER as i64),
        Some(-18_000)
    );
    assert_eq!(
        files_tz_offset(&["/nonexistent-look-tz".to_string()], WINTER as i64),
        None
    );
}

#[test]
fn fill_repositories_skips_missing_uris() {
    let repos = vec![RepositoryRef {
        name: "repo".to_string(),
        uris: vec!["git@h.test:repo.git".to_string()],
        permanent_rules: None,
    }];
    let mut remotes = vec![RemoteInfo::new("bare".to_string(), None, None)];
    fill_remote_repositories(&mut remotes, &repos);
    assert_eq!(remotes[0].push_repository, None);
    assert_eq!(remotes[0].fetch_repository, None);
}

#[test]
fn marker_parsing_skips_empty_remote_or_branch() {
    assert!(parse_marker_refs("refs/remotes//master abc123\n").is_empty());
    assert!(parse_marker_refs("refs/remotes/origin/ abc123\n").is_empty());
}

#[test]
fn query_markers_lists_remote_tracking_tips() {
    let repo = look_repo("markers");
    assert!(query_markers(repo.path()).unwrap().is_empty());
    let head = look_git(repo.path(), &["rev-parse", "HEAD"]);
    look_git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/master", &head],
    );
    assert_eq!(
        query_markers(repo.path()).unwrap(),
        vec![Marker {
            remote: "origin".to_string(),
            name: "master".to_string(),
            hash: head,
        }]
    );
    let plain = LookTmp::new("markers-plain");
    assert!(query_markers(plain.path()).is_err());
}

#[test]
fn query_markers_reports_git_failures() {
    // A corrupt packed-refs still parses as a work tree, but listing
    // refs fails: the git error surfaces instead of silent tips.
    let repo = look_repo("markers-corrupt");
    std::fs::write(repo.path().join(".git/packed-refs"), "garbage{{{not-refs\n").unwrap();
    assert!(query_markers(repo.path()).is_err());
}

#[test]
fn published_hashes_skip_remotes_without_push_uri() {
    let markers = vec![Marker {
        remote: "origin".to_string(),
        name: "master".to_string(),
        hash: "aaa".to_string(),
    }];
    let remotes = vec![RemoteInfo::new(
        "origin".to_string(),
        None,
        Some("git@h.test:repo.git".to_string()),
    )];
    let repos = vec![RepositoryRef {
        name: "repo".to_string(),
        uris: vec!["git@h.test:repo.git".to_string()],
        permanent_rules: Some(Vec::new()),
    }];
    assert!(published_hashes(&markers, &remotes, &repos).is_empty());
}

#[test]
fn look_remotes_data_resolves_optionally() {
    let repo = look_repo("remotes-data");
    look_git(
        repo.path(),
        &["remote", "add", "origin", "https://h.test/repo.git"],
    );
    // Local-only: names stay unresolved.
    let local = look_remotes_data(repo.path(), None).unwrap();
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].name, "origin");
    assert_eq!(local[0].push_repository, None);
    // With conduit: the search fills display names.
    let call = |_method: &str, _params: &serde_json::Value| {
        Ok(serde_json::json!({"data": [{
            "fields": {"name": "repo"},
            "attachments": {"uris": {"uris": [
                {"fields": {"uri": {"effective": "https://h.test/repo.git"}}},
            ]}},
        }]}))
    };
    let resolved = look_remotes_data(repo.path(), Some(&call)).unwrap();
    assert_eq!(resolved[0].push_repository, Some("repo".to_string()));
    assert_eq!(resolved[0].fetch_repository, Some("repo".to_string()));
}

#[test]
fn look_published_data_lists_tips_or_filters() {
    let repo = look_repo("published-data");
    let head = look_git(repo.path(), &["rev-parse", "HEAD"]);
    look_git(
        repo.path(),
        &["remote", "add", "origin", "https://h.test/repo.git"],
    );
    look_git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/master", &head],
    );
    look_git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/wip", &head],
    );
    // Offline: every remote-tracking tip.
    let mut tips = look_published_data(repo.path(), None).unwrap();
    tips.sort();
    assert_eq!(tips, vec![head.clone(), head.clone()]);
    // With conduit: only permanent push refs.
    let call = |_method: &str, _params: &serde_json::Value| {
        Ok(serde_json::json!({"data": [{
            "fields": {
                "name": "repo",
                "refRules": {"permanentRefRules": ["master"]},
            },
            "attachments": {"uris": {"uris": [
                {"fields": {"uri": {"effective": "https://h.test/repo.git"}}},
            ]}},
        }]}))
    };
    assert_eq!(
        look_published_data(repo.path(), Some(&call)).unwrap(),
        vec![head]
    );
}

#[test]
fn fail_mapped_reports_auth_or_fatal() {
    let client = crate::conduit::Client::new("http://127.0.0.1:9/", "").expect("client");
    assert_eq!(
        fail_mapped(&client, "ERR-INVALID-AUTH: bad token".to_string()),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        fail_mapped(&client, "not inside a git repository".to_string()),
        std::process::ExitCode::from(255)
    );
}

#[test]
fn call_or_unavailable_maps_transport_errors() {
    let client = crate::conduit::Client::new("http://127.0.0.1:9/", "").expect("client");
    let err = call_or_unavailable(
        Some(&client),
        "diffusion.repository.search",
        &serde_json::json!({}),
    )
    .expect_err("refused");
    assert!(!err.is_empty(), "expected a transport message");
    assert_eq!(
        call_or_unavailable(None, "diffusion.repository.search", &serde_json::json!({})),
        Err("conduit unavailable".to_string())
    );
}

#[test]
fn render_remotes_output_lists_every_remote() {
    let remote = RemoteInfo {
        name: "origin".to_string(),
        push_uri: Some("https://h.test/repo.git".to_string()),
        fetch_uri: Some("https://h.test/repo.git".to_string()),
        push_repository: Some("repo".to_string()),
        fetch_repository: Some("repo".to_string()),
    };
    let out = render_remotes_output(std::slice::from_ref(&remote), false).unwrap();
    assert!(out.contains("Arcventure"), "{out}");
    assert!(out.contains("origin"), "{out}");
    assert!(out.contains("Push Repository: repo"), "{out}");
    assert!(out.contains("published"), "{out}");
    let empty = render_remotes_output(&[], false).unwrap();
    assert!(empty.contains("grove"), "{empty}");
}

#[test]
fn render_published_output_handles_empty_river() {
    let empty = render_published_output(&[], false).unwrap();
    assert!(empty.contains("bubbles quietly"), "{empty}");
    assert!(!empty.contains("Floating"), "{empty}");
    let out = render_published_output(&["abc123".to_string()], false).unwrap();
    assert!(out.contains("Floating on the water"), "{out}");
    assert!(out.contains("abc123"), "{out}");
    assert!(out.contains("bubbles peacefully"), "{out}");
}

#[test]
fn run_clearing_renders_live_clearing() {
    assert_eq!(run_clearing(false), std::process::ExitCode::SUCCESS);
    assert_eq!(run_clearing(true), std::process::ExitCode::SUCCESS);
}

#[test]
fn run_unknown_reports_missing_thing() {
    assert_eq!(
        run_unknown(&args(&["big", "rock"]), false),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_remotes_fails_in_remote_less_repo() {
    let repo = look_repo("run-remotes-err");
    write_arcconfig(repo.path(), "http://127.0.0.1:9/");
    assert_eq!(
        run_remotes(repo.path(), false),
        std::process::ExitCode::from(255)
    );
}

#[test]
fn run_remotes_lists_repo_remotes() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "diffusion.repository.search".to_string(),
        look_envelope(
            r#"{"data": [{
                "fields": {"name": "repo"},
                "attachments": {"uris": {"uris": [
                    {"fields": {"uri": {"effective": "https://h.test/repo.git"}}}
                ]}}
            }]}"#,
        ),
    );
    let base = look_serve(routes, 1);
    let repo = look_repo("run-remotes-ok");
    look_git(
        repo.path(),
        &["remote", "add", "origin", "https://h.test/repo.git"],
    );
    write_arcconfig(repo.path(), &base);
    // Guard: the project URI must win discovery, so the run below can
    // only talk to the loopback stub.
    let user_file = crate::config::arcrc_path();
    let client =
        look_conduit_client(repo.path(), user_file.as_deref()).expect("stub client resolves");
    assert_eq!(client.base_uri(), format!("{base}api/"));
    assert_eq!(
        run_remotes(repo.path(), false),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_published_fails_outside_repo() {
    let plain = LookTmp::new("run-published-err");
    write_arcconfig(plain.path(), "http://127.0.0.1:9/");
    assert_eq!(
        run_published(plain.path(), false),
        std::process::ExitCode::from(255)
    );
}

#[test]
fn run_published_lists_remote_tips() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "diffusion.repository.search".to_string(),
        look_envelope(r#"{"data": []}"#),
    );
    let base = look_serve(routes, 1);
    let repo = look_repo("run-published-ok");
    look_git(
        repo.path(),
        &["remote", "add", "origin", "https://h.test/repo.git"],
    );
    write_arcconfig(repo.path(), &base);
    // Guard: the project URI must win discovery, so the run below can
    // only talk to the loopback stub.
    let user_file = crate::config::arcrc_path();
    let client =
        look_conduit_client(repo.path(), user_file.as_deref()).expect("stub client resolves");
    assert_eq!(client.base_uri(), format!("{base}api/"));
    assert_eq!(
        run_published(repo.path(), false),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_in_reports_missing_terminator_when_piped() {
    // Under `cargo test` stdin is not a TTY, so an argv without `--`
    // reports the terminator hints plus the usage envelope.
    let plain = LookTmp::new("run-terminator");
    assert_eq!(run_in(plain.path(), &[]), std::process::ExitCode::FAILURE);
}

#[test]
fn system_zone_prefers_localtime_bytes() {
    // Valid localtime bytes win, even with a timezone name present.
    assert_eq!(
        system_tz_offset_from(Some(TESTBURG_V1), Some("UTC"), WINTER as i64),
        -18_000
    );
    assert_eq!(
        system_tz_offset_from(Some(TESTBURG_V1), None, SUMMER as i64),
        -14_400
    );
}

#[test]
fn system_zone_falls_back_past_unparseable_localtime() {
    // Corrupt localtime bytes fall through to the timezone name, then
    // to UTC when nothing resolves.
    let tmp = LookTmp::new("syszone");
    let path = tmp.path().join("v1.tz");
    std::fs::write(&path, TESTBURG_V1).unwrap();
    let name = path.to_str().expect("utf8 tmp path").to_string();
    assert!(name.contains('/'));
    assert_eq!(
        system_tz_offset_from(Some(b"not a tzfile"), Some(&name), WINTER as i64),
        -18_000
    );
    assert_eq!(
        system_tz_offset_from(Some(b"not a tzfile"), None, WINTER as i64),
        0
    );
    assert_eq!(system_tz_offset_from(None, None, WINTER as i64), 0);
}

#[test]
fn system_zone_ignores_blank_and_unknown_names() {
    // Blank names skip the zoneinfo lookup; unknown names resolve to
    // nothing; both land on UTC.
    assert_eq!(system_tz_offset_from(None, Some(""), WINTER as i64), 0);
    assert_eq!(system_tz_offset_from(None, Some("  \n "), WINTER as i64), 0);
    assert_eq!(
        system_tz_offset_from(None, Some("arc-rs-no-such-zone"), WINTER as i64),
        0
    );
}

#[test]
fn system_zone_matches_live_etc_contents() {
    // The wrapper reads exactly `/etc/localtime` + `/etc/timezone` and
    // delegates: same inputs, same offset as the live call.
    let localtime = std::fs::read("/etc/localtime").ok();
    let timezone = std::fs::read_to_string("/etc/timezone").ok();
    let now = WINTER as i64;
    assert_eq!(
        system_tz_offset(now),
        system_tz_offset_from(localtime.as_deref(), timezone.as_deref(), now)
    );
}
