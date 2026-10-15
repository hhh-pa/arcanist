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
    let root = std::env::temp_dir().join(format!("arc-rs-disc-{}", std::process::id()));
    let child = root.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(
        root.join("a").join(".arcconfig"),
        r#"{"phabricator.uri": "https://orsay.hhh.pa/"}"#,
    )
    .unwrap();
    assert_eq!(
        discover_from(&child),
        Some("https://orsay.hhh.pa/".to_string())
    );
    assert_eq!(discover_from(&root), None);
    std::fs::remove_dir_all(&root).ok();
}
