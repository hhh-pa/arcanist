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
    assert_eq!(
        normalize_uri("https://h.example:8443/a?b=c#d").unwrap(),
        "https://h.example:8443/api/"
    );
    assert_eq!(
        normalize_uri("devtools.example.com").unwrap_err(),
        "Server URI \"devtools.example.com\" must include a protocol and domain. It should be in the form \"https://devtools.example.com/\"."
    );
    assert_eq!(
        normalize_uri("ftp://h.example/").unwrap_err(),
        "Server URI \"ftp://h.example/\" must include the \"http\" or \"https\" protocol. It should be in the form \"https://devtools.example.com/\"."
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
    assert_eq!(validate_token(CLI_TOKEN).unwrap(), TokenKind::Cli);
    assert_eq!(validate_token(API_TOKEN).unwrap(), TokenKind::Standard);
    assert_eq!(
        validate_token("cli-short").unwrap_err(),
        "The token \"cli-short\" is not formatted correctly. API tokens should be 32 characters long. Make sure you visited the correct URI and copy/pasted the token correctly."
    );
    assert!(validate_token(&"x".repeat(33)).is_err());
    assert_eq!(
        validate_token("xxx-0123456789abcdef0123456789ab").unwrap_err(),
        "The token \"xxx-0123456789abcdef0123456789ab\" is not formatted correctly. Valid API tokens should begin \"cli-\" and be 32 characters long. Make sure you visited the correct URI and copy/pasted the token correctly."
    );
}

#[test]
fn uri_resolution_follows_precedence() {
    // CORE-33: the project file only counts at a working-copy root.
    let tmp = TempDir::new("resolve");
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
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
    // CORE-12: with user/system shadowed, bare discovery still errors.
    let bare = TempDir::new("bare");
    let empty = bare.path.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    let shadow = arc_rs::get_config::LoadOptions {
        config_files: vec![empty],
        ..Default::default()
    };
    assert_eq!(
        resolve_uri_with(None, None, None, &bare.path, None, &shadow).unwrap_err(),
        "Specify an explicit URI or run this command from within a project which is configured with a .arcconfig."
    );
    assert!(resolve_uri(Some("notauri"), None, None, &tmp.path).is_err());
}

#[test]
fn login_url_tracks_auth_mode() {
    assert_eq!(
        token_login_url("https://h.example/api/", true),
        "https://h.example/conduit/login/"
    );
    assert_eq!(
        token_login_url("https://h.example/api/", false),
        "https://h.example/conduit/token/"
    );
}

#[test]
fn messages_match_php_call_sites() {
    assert_eq!(connect_title(), "CONNECT");
    assert_eq!(
        connect_message("https://h.example/api/"),
        "Connecting to \"https://h.example/api/\"..."
    );
    assert_eq!(login_header_markup(), "**LOG IN**\n");
    assert_eq!(
        login_page_block("https://h.example/conduit/login/"),
        "Open this page in your browser and log in if necessary:\n\nhttps://h.example/conduit/login/\n\nThen paste the API Token on that page below."
    );
    assert_eq!(
        standard_token_notice(),
        "You are installing a standard API token, but a CLI API token was expected. If you're writing a script, consider passing the token at runtime with --conduit-token instead of installing it."
    );
    assert_eq!(
        downloading_message(),
        "Downloading authentication certificate..."
    );
    assert_eq!(
        installing_message("alice"),
        "Installing certificate for 'alice'..."
    );
    assert_eq!(writing_message(), "Writing ~/.arcrc...");
    assert_eq!(success_title(), "SUCCESS!");
    assert_eq!(success_message(true), "API Token installed.");
    assert_eq!(success_message(false), "Certificate installed.");
}

#[test]
fn token_lines_trim_and_skip_blanks() {
    assert_eq!(
        accept_token_line("  cli-abc  "),
        Some("cli-abc".to_string())
    );
    assert_eq!(accept_token_line(""), None);
    assert_eq!(accept_token_line("   "), None);
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
fn certificate_storage_keeps_user_and_cert() {
    let tmp = TempDir::new("certstore");
    let path = tmp.path.join(".arcrc");
    let info = CertificateInfo {
        username: "alice".to_string(),
        certificate: "CERTDATA".to_string(),
    };
    install_certificate_entry(&path, "https://h.example/api/", &info).unwrap();
    let doc = read_token_doc(&path).unwrap();
    assert_eq!(
        doc["hosts"]["https://h.example/api/"]["user"],
        serde_json::json!("alice")
    );
    assert_eq!(
        doc["hosts"]["https://h.example/api/"]["cert"],
        serde_json::json!("CERTDATA")
    );
}

#[test]
fn staging_entries_preserves_sibling_hosts() {
    let tmp = TempDir::new("stage");
    let path = tmp.path.join(".arcrc");
    let mut doc = serde_json::json!({"hosts": {"https://old.example/api/": {"token": "t"}}});
    apply_token_entry(&mut doc, &path, "https://new.example/api/", CLI_TOKEN).unwrap();
    assert_eq!(
        doc["hosts"]["https://old.example/api/"]["token"],
        serde_json::json!("t")
    );
    assert_eq!(
        doc["hosts"]["https://new.example/api/"]["token"],
        serde_json::json!(CLI_TOKEN)
    );
    let info = CertificateInfo {
        username: "bob".to_string(),
        certificate: "C".to_string(),
    };
    apply_certificate_entry(&mut doc, &path, "https://cert.example/api/", &info).unwrap();
    assert_eq!(
        doc["hosts"]["https://cert.example/api/"]["user"],
        serde_json::json!("bob")
    );
}

fn stub_call(
    methods: std::collections::HashMap<String, Result<serde_json::Value, String>>,
) -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    move |method: &str, _params: &serde_json::Value| {
        methods
            .get(method)
            .cloned()
            .unwrap_or_else(|| Err(format!("unexpected conduit call: {method}")))
    }
}

#[test]
fn token_auth_detection_matches_capabilities() {
    assert!(token_auth_supported(
        &serde_json::json!({"authentication": ["token", "session"]})
    ));
    assert!(!token_auth_supported(
        &serde_json::json!({"authentication": ["session"]})
    ));
    assert!(!token_auth_supported(&serde_json::json!({})));

    let modern = stub_call(
        [(
            "conduit.getcapabilities".to_string(),
            Ok(serde_json::json!({"authentication": ["token"]})),
        )]
        .into_iter()
        .collect(),
    );
    assert!(detect_token_auth(&modern));

    let failing = stub_call(std::collections::HashMap::new());
    assert!(!detect_token_auth(&failing));
}

#[test]
fn ping_errors_carry_server_and_cause() {
    let failing = stub_call(
        [("conduit.ping".to_string(), Err("boom".to_string()))]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        ping_with(&failing, "https://h.example/api/").unwrap_err(),
        "Failed to connect to server (https://h.example/api/): boom"
    );
    let ok = stub_call(
        [("conduit.ping".to_string(), Ok(serde_json::json!({})))]
            .into_iter()
            .collect(),
    );
    assert!(ping_with(&ok, "https://h.example/api/").is_ok());
}

#[test]
fn whoami_errors_quote_token_and_response() {
    let failing = stub_call(
        [("user.whoami".to_string(), Err("ERR: bad token".to_string()))]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        whoami_with(&failing, CLI_TOKEN).unwrap_err(),
        format!(
            "The token \"{CLI_TOKEN}\" is not a valid API Token. The server returned this response when trying to use it as a token: ERR: bad token"
        )
    );
}

#[test]
fn certificate_exchange_parses_user_and_cert() {
    let ok = stub_call(
        [(
            "conduit.getcertificate".to_string(),
            Ok(serde_json::json!({"username": "alice", "certificate": "CERT"})),
        )]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        exchange_certificate(&ok, "https://h.example/api/", "tok").unwrap(),
        CertificateInfo {
            username: "alice".to_string(),
            certificate: "CERT".to_string(),
        }
    );
    let bad = stub_call(
        [(
            "conduit.getcertificate".to_string(),
            Ok(serde_json::json!({"username": "alice"})),
        )]
        .into_iter()
        .collect(),
    );
    assert!(exchange_certificate(&bad, "https://h.example/api/", "tok").is_err());
}

#[test]
fn certificate_request_carries_token_and_host() {
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "conduit.getcertificate");
        seen.borrow_mut().push(params.clone());
        Ok(serde_json::json!({"username": "u", "certificate": "c"}))
    };
    exchange_certificate(&call, "https://h.example/api/", "tok").unwrap();
    assert_eq!(
        seen.borrow().as_slice(),
        &[serde_json::json!({"token": "tok", "host": "https://h.example/api/"})]
    );
}

#[test]
fn token_flow_validates_verifies_and_stages() {
    let tmp = TempDir::new("flow");
    let path = tmp.path.join(".arcrc");
    let mut doc = read_token_doc(&path).unwrap();
    let ok = stub_call(
        [("user.whoami".to_string(), Ok(serde_json::json!({})))]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        install_token_flow(&ok, "https://h.example/api/", CLI_TOKEN, &mut doc, &path).unwrap(),
        TokenKind::Cli
    );
    assert_eq!(
        doc["hosts"]["https://h.example/api/"]["token"],
        serde_json::json!(CLI_TOKEN)
    );

    let mut doc = serde_json::json!({});
    assert!(install_token_flow(&ok, "https://h.example/api/", "short", &mut doc, &path).is_err());
    assert_eq!(doc, serde_json::json!({}));

    let failing = stub_call(
        [("user.whoami".to_string(), Err("nope".to_string()))]
            .into_iter()
            .collect(),
    );
    let mut doc = serde_json::json!({});
    assert!(
        install_token_flow(
            &failing,
            "https://h.example/api/",
            CLI_TOKEN,
            &mut doc,
            &path
        )
        .is_err()
    );
    assert_eq!(doc, serde_json::json!({}));
}

#[test]
fn certificate_flow_stages_nothing_on_failure() {
    let tmp = TempDir::new("certflow");
    let path = tmp.path.join(".arcrc");
    let failing = stub_call(std::collections::HashMap::new());
    let mut doc = serde_json::json!({});
    assert!(
        install_certificate_flow(&failing, "https://h.example/api/", "tok", &mut doc, &path)
            .is_err()
    );
    assert_eq!(doc, serde_json::json!({}));
}

#[test]
fn styled_renders_match_console_output() {
    assert_eq!(
        render_connect("https://h.example/api/", false),
        " CONNECT  Connecting to \"https://h.example/api/\"...\n"
    );
    assert_eq!(render_login_header(false).unwrap(), "LOG IN\n");
    assert_eq!(
        render_success(true, false),
        " SUCCESS!  API Token installed.\n"
    );
    assert_eq!(
        render_success(false, false),
        " SUCCESS!  Certificate installed.\n"
    );
}

#[test]
fn args_accept_one_uri_and_flags() {
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    assert_eq!(
        parse_args(&["a".to_string(), "b".to_string()]).unwrap_err(),
        "Specify at most one URI."
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
            config: vec![],
            config_files: vec![],
            arcrc_file: None,
        }
    );
    let dashed = parse_args(&["--".to_string(), "https://h.example/".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(dashed.uri, Some("https://h.example/".to_string()));
}

#[test]
fn r6_core38_uri_uses_full_precedence_and_default() {
    // CORE-38: discovery reads `phabricator.uri` from any source, then
    // `default` — including the project legacy `conduit_uri`.
    let tmp = TempDir::new("core38");
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    let user = tmp.path.join("arcrc.json");
    std::fs::write(&user, r#"{"config": {"default": "https://d.example/"}}"#).unwrap();
    let opts = arc_rs::get_config::LoadOptions::default();
    assert_eq!(
        resolve_uri_with(None, None, None, &tmp.path, Some(&user), &opts).unwrap(),
        "https://d.example/api/"
    );
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"conduit_uri": "https://old.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        resolve_uri_with(None, None, None, &tmp.path, Some(&user), &opts).unwrap(),
        "https://old.example/api/"
    );
    // `--config` beats the project file.
    let rt = arc_rs::get_config::load_options_from_cli(
        &["phabricator.uri=https://rt.example/".to_string()],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(
        resolve_uri_with(None, None, None, &tmp.path, Some(&user), &rt).unwrap(),
        "https://rt.example/api/"
    );
    // Explicit inputs still beat discovery.
    assert_eq!(
        resolve_uri_with(
            None,
            Some("https://flag.example/"),
            None,
            &tmp.path,
            Some(&user),
            &rt
        )
        .unwrap(),
        "https://flag.example/api/"
    );
}

#[test]
fn r6_core38_mistyped_uri_errors() {
    let tmp = TempDir::new("core38type");
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(tmp.path.join(".arcconfig"), r#"{"phabricator.uri": 42}"#).unwrap();
    let opts = arc_rs::get_config::LoadOptions::default();
    assert_eq!(
        resolve_uri_with(None, None, None, &tmp.path, None, &opts).unwrap_err(),
        "Type of setting 'phabricator.uri' must be string."
    );
}

#[test]
fn r6_core34_install_config_flags_parse() {
    let opts = parse_args(&[
        "https://h.example/".to_string(),
        "--config".to_string(),
        "a=1".to_string(),
        "--config-file=f".to_string(),
        "--arcrc-file".to_string(),
        "g".to_string(),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(opts.config, ["a=1"]);
    assert_eq!(opts.config_files, ["f"]);
    assert_eq!(opts.arcrc_file.as_deref(), Some("g"));
    assert!(parse_args(&["--config".to_string()]).is_err());
}
