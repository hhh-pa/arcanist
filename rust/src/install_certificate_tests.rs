use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const CLI_TOKEN: &str = "cli-0123456789abcdef0123456789ab";
const API_TOKEN: &str = "api-0123456789abcdef0123456789ab";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-instcert-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn uri_normalization_requires_http_and_domain() {
    assert_eq!(CLI_TOKEN.len(), 32);
    assert_eq!(API_TOKEN.len(), 32);
    for raw in [
        "https://devtools.example.com/",
        "https://devtools.example.com",
        "https://devtools.example.com/api/",
        "https://devtools.example.com/api",
        "https://devtools.example.com/some/path",
    ] {
        assert_eq!(
            normalize_uri(raw).unwrap(),
            "https://devtools.example.com/api/",
            "for {raw}"
        );
    }
    assert_eq!(
        normalize_uri("http://h.example/").unwrap(),
        "http://h.example/api/"
    );
    assert!(
        normalize_uri("devtools.example.com")
            .unwrap_err()
            .contains("protocol and domain")
    );
    assert!(
        normalize_uri("ftp://h.example/")
            .unwrap_err()
            .contains("\"http\" or \"https\"")
    );
    assert!(
        normalize_uri("https://")
            .unwrap_err()
            .contains("protocol and domain")
    );
    assert!(normalize_uri("").is_err());
}

#[test]
fn token_shapes_validate() {
    assert_eq!(validate_token(CLI_TOKEN, false).unwrap(), TokenKind::Cli);
    assert_eq!(
        validate_token(API_TOKEN, true).unwrap(),
        TokenKind::Standard
    );
    assert!(
        validate_token(API_TOKEN, false)
            .unwrap_err()
            .contains("--force")
    );
    assert!(
        validate_token("cli-short", false)
            .unwrap_err()
            .contains("32 characters")
    );
    assert!(validate_token(&"x".repeat(33), false).is_err());
    assert!(
        validate_token("xxx-0123456789abcdef0123456789ab", false)
            .unwrap_err()
            .contains("cli-")
    );
}

#[test]
fn uri_resolution_follows_precedence() {
    let tmp = TempDir::new("resolve");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://cfg.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        resolve_uri(
            Some("https://pos.example/"),
            Some("https://flag.example/"),
            Some("https://env.example/"),
            &tmp.path
        )
        .unwrap(),
        "https://pos.example/api/"
    );
    assert_eq!(
        resolve_uri(
            None,
            Some("https://flag.example/"),
            Some("https://env.example/"),
            &tmp.path
        )
        .unwrap(),
        "https://flag.example/api/"
    );
    assert_eq!(
        resolve_uri(None, None, Some("https://env.example/"), &tmp.path).unwrap(),
        "https://env.example/api/"
    );
    assert_eq!(
        resolve_uri(None, None, None, &tmp.path).unwrap(),
        "https://cfg.example/api/"
    );
    let bare = TempDir::new("bare");
    assert!(
        resolve_uri(None, None, None, &bare.path)
            .unwrap_err()
            .contains(".arcconfig")
    );
    assert!(resolve_uri(Some("notauri"), None, None, &tmp.path).is_err());
    assert_eq!(
        token_login_url("https://h.example/api/"),
        "https://h.example/conduit/login/"
    );
}

#[test]
fn token_storage_roundtrips_and_preserves_config() {
    let tmp = TempDir::new("store");
    let path = tmp.path.join(".arcrc");
    assert_eq!(read_token_doc(&path).unwrap(), serde_json::json!({}));
    std::fs::write(&path, r#"{"config": {"editor": "nano"}}"#).unwrap();
    install_token(&path, "https://h.example/api/", CLI_TOKEN).unwrap();
    let doc = read_token_doc(&path).unwrap();
    assert_eq!(
        doc["hosts"]["https://h.example/api/"]["token"],
        serde_json::json!(CLI_TOKEN)
    );
    assert_eq!(doc["config"]["editor"], serde_json::json!("nano"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        crate::config::token_for_str(&text, "https://h.example/"),
        Some(CLI_TOKEN.to_string())
    );
    std::fs::write(&path, "nope").unwrap();
    assert!(read_token_doc(&path).is_err());
    assert!(install_token(&path, "https://h.example/api/", CLI_TOKEN).is_err());
    std::fs::write(&path, "[1]").unwrap();
    assert!(read_token_doc(&path).is_err());
    std::fs::write(&path, r#"{"hosts": 7}"#).unwrap();
    assert!(install_token(&path, "https://h.example/api/", CLI_TOKEN).is_err());
}

#[test]
fn args_accept_one_uri_and_flags() {
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    assert!(
        parse_args(&["a".to_string(), "b".to_string()])
            .unwrap_err()
            .contains("at most one")
    );
    assert!(parse_args(&["--bogus".to_string()]).is_err());
    assert!(parse_args(&["--token".to_string()]).is_err());
    assert!(parse_args(&["--conduit-uri".to_string()]).is_err());
    let opts = parse_args(&[
        "https://h.example/".to_string(),
        "--token".to_string(),
        "t".to_string(),
        "--force".to_string(),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(
        opts,
        Options {
            uri: Some("https://h.example/".to_string()),
            conduit_uri: None,
            token: Some("t".to_string()),
            force: true,
        }
    );
    let dashed = parse_args(&["--".to_string(), "https://h.example/".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(dashed.uri, Some("https://h.example/".to_string()));
}
