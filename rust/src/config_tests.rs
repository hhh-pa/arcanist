use super::*;

const ARCRC: &str = r#"{"hosts": {"https://orsay.hhh.pa/api/": {"token": "api-secret"}}}"#;

#[test]
fn finds_token_regardless_of_uri_shape() {
    for uri in [
        "https://orsay.hhh.pa",
        "https://orsay.hhh.pa/",
        "https://orsay.hhh.pa/api",
        "https://orsay.hhh.pa/api/",
    ] {
        assert_eq!(
            token_for_str(ARCRC, uri),
            Some("api-secret".to_string()),
            "for {uri}"
        );
    }
}

#[test]
fn misses_cleanly() {
    assert_eq!(token_for_str(ARCRC, "https://other.example/"), None);
    assert_eq!(token_for_str("{}", "https://orsay.hhh.pa/"), None);
    assert_eq!(token_for_str("not json", "https://orsay.hhh.pa/"), None);
    assert_eq!(
        token_for_str(
            r#"{"hosts": {"https://orsay.hhh.pa/api/": {}}}"#,
            "https://orsay.hhh.pa/"
        ),
        None
    );
}

#[test]
fn discovers_arcconfig_upwards() {
    // CORE-33: git reads `.arcconfig` only at the working-copy root; a
    // subdir file PHP would ignore no longer counts, wherever it sits.
    let root = std::env::temp_dir().join(format!("arc-rs-disc-{}", std::process::id()));
    let child = root.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(
        root.join("a").join(".arcconfig"),
        r#"{"phabricator.uri": "https://sub.example/"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".arcconfig"),
        r#"{"phabricator.uri": "https://orsay.hhh.pa/"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_from(&child),
        Some("https://orsay.hhh.pa/".to_string())
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Hermetic options: `--config-file` shadows the ambient user/system
/// sources, so discovery assertions never leak `~/.arcrc`.
fn r6_shadowed(tag: &str) -> (PathBuf, get_config::LoadOptions) {
    let dir = std::env::temp_dir().join(format!("arc-rs-r6-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    let opts = get_config::LoadOptions {
        config_files: vec![empty],
        ..Default::default()
    };
    (dir, opts)
}

#[test]
fn r6_core12_default_fallback_and_precedence() {
    let (dir, shadow) = r6_shadowed("core12");
    // `default` alone resolves.
    let user = dir.join("arcrc.json");
    std::fs::write(&user, r#"{"config": {"default": "https://d.example/"}}"#).unwrap();
    assert_eq!(
        discover_uri_with(&dir, Some(&user), &get_config::LoadOptions::default()),
        Some("https://d.example/".to_string())
    );
    // `phabricator.uri` beats `default` in the same file.
    std::fs::write(
        &user,
        r#"{"config": {"default": "https://d.example/", "phabricator.uri": "https://u.example/"}}"#,
    )
    .unwrap();
    assert_eq!(
        discover_uri_with(&dir, Some(&user), &get_config::LoadOptions::default()),
        Some("https://u.example/".to_string())
    );
    // Project beats user.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phabricator.uri": "https://p.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_uri_with(&dir, Some(&user), &get_config::LoadOptions::default()),
        Some("https://p.example/".to_string())
    );
    // Nothing anywhere resolves to nothing.
    std::fs::remove_file(dir.join(".arcconfig")).unwrap();
    std::fs::write(&user, "{}").unwrap();
    assert_eq!(discover_uri_with(&dir, None, &shadow), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r6_core12_legacy_conduit_uri_key() {
    let (dir, shadow) = r6_shadowed("core12legacy");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"conduit_uri": "https://old.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_uri_with(&dir, None, &shadow),
        Some("https://old.example/".to_string())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r6_core33_subdir_and_outside_repo_ignored() {
    let (dir, shadow) = r6_shadowed("core33");
    // A subdir `.arcconfig` inside a git working copy is invisible.
    let child = dir.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        child.join(".arcconfig"),
        r#"{"phabricator.uri": "https://sub.example/"}"#,
    )
    .unwrap();
    assert_eq!(discover_uri_with(&child, None, &shadow), None);
    // Outside any working copy there is no project config either.
    let plain = dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(
        plain.join(".arcconfig"),
        r#"{"phabricator.uri": "https://plain.example/"}"#,
    )
    .unwrap();
    assert_eq!(discover_uri_with(&plain, None, &shadow), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r6_core33_svn_walks_within_root_only() {
    let (dir, shadow) = r6_shadowed("core33svn");
    std::fs::create_dir_all(dir.join(".svn")).unwrap();
    let child = dir.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(
        child.join(".arcconfig"),
        r#"{"phabricator.uri": "https://svn.example/"}"#,
    )
    .unwrap();
    // Nearest wins, up to the working-copy root.
    assert_eq!(
        discover_uri_with(&child, None, &shadow),
        Some("https://svn.example/".to_string())
    );
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phabricator.uri": "https://svnroot.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_uri_with(&child, None, &shadow),
        Some("https://svn.example/".to_string())
    );
    assert_eq!(
        discover_uri_with(&dir, None, &shadow),
        Some("https://svnroot.example/".to_string())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r6_core34_runtime_config_beats_static_sources() {
    let (dir, shadow) = r6_shadowed("core34");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phabricator.uri": "https://p.example/"}"#,
    )
    .unwrap();
    let mut runtime = serde_json::Map::new();
    runtime.insert(
        "phabricator.uri".to_string(),
        serde_json::json!("https://rt.example/"),
    );
    let opts = get_config::LoadOptions {
        runtime,
        ..Default::default()
    };
    assert_eq!(
        discover_uri_with(&dir, None, &opts),
        Some("https://rt.example/".to_string())
    );
    // And `--config-file` shadows user/system: with the project file
    // gone, the user `default` below must not leak through the shadow.
    std::fs::remove_file(dir.join(".arcconfig")).unwrap();
    let user = dir.join("arcrc.json");
    std::fs::write(&user, r#"{"config": {"default": "https://d.example/"}}"#).unwrap();
    assert_eq!(discover_uri_with(&dir, Some(&user), &shadow), None);
    std::fs::remove_dir_all(&dir).ok();
}
