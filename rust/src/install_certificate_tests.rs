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
    // T1153 CORE-43: the ceb example host is `phabricator.example.com`.
    assert_eq!(CLI_TOKEN.len(), 32);
    assert_eq!(API_TOKEN.len(), 32);
    for raw in [
        "https://phabricator.example.com/",
        "https://phabricator.example.com",
        "https://phabricator.example.com/api/",
        "https://phabricator.example.com/api",
        "https://phabricator.example.com/some/path",
    ] {
        assert_eq!(
            normalize_uri(raw).unwrap(),
            "https://phabricator.example.com/api/",
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
        normalize_uri("phabricator.example.com").unwrap_err(),
        "Server URI \"phabricator.example.com\" must include a protocol and domain. It should be in the form \"https://phabricator.example.com/\"."
    );
    assert_eq!(
        normalize_uri("ftp://h.example/").unwrap_err(),
        "Server URI \"ftp://h.example/\" must include the \"http\" or \"https\" protocol. It should be in the form \"https://phabricator.example.com/\"."
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
    // T1153 CORE-33: the project file only counts at a working-copy root.
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
    // T1153 CORE-12: with user/system shadowed, bare discovery still errors.
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
    assert_eq!(login_header_markup(), "**LOGIN TO PHABRICATOR**\n");
    assert_eq!(
        login_page_block("https://h.example/conduit/login/"),
        "Open this page in your browser and login to Phabricator if necessary:\n\nhttps://h.example/conduit/login/\n\nThen paste the API Token on that page below."
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
    // T1153 CORE-42: the `readUserConfigurationFile()` text, literal `~/.arcrc`.
    assert_eq!(
        read_token_doc(&path).unwrap_err(),
        "Your '~/.arcrc' file is not a valid JSON file."
    );
    assert!(install_token(&path, "https://h.example/api/", CLI_TOKEN).is_err());
    // T1153 CORE-35 (kept): non-object docs are rejected upfront
    // (PHP returns them raw and fatals downstream).
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
    // T1153 CORE-45 (kept): a missing certificate is rejected.
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
    assert_eq!(
        render_login_header(false).unwrap(),
        "LOGIN TO PHABRICATOR\n"
    );
    assert_eq!(
        render_success(true, false),
        " SUCCESS!  API Token installed.\n"
    );
    assert_eq!(
        render_success(false, false),
        " SUCCESS!  Certificate installed.\n"
    );
    // T1153 CORE-44: status nesting (`<bg:color>** %s **</bg>`), not the
    // log-engine order — plain text stays identical.
    assert_eq!(
        render_connect("https://h.example/api/", true),
        "\x1b[44m\x1b[1m CONNECT \x1b[m\x1b[49m Connecting to \"https://h.example/api/\"...\n"
    );
    assert_eq!(
        render_success(true, true),
        "\x1b[42m\x1b[1m SUCCESS! \x1b[m\x1b[49m API Token installed.\n"
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

// -( R5 TTY gating: T1153 CORE-39/T1153 CORE-40, both CLEAN )-----------------------------------

#[test]
fn r5_core39_token_prompt_gates_pipes() {
    // CLEAN: `phutil_console_prompt()` requires a TTY
    // (`ArcanistInstallCertificateWorkflow.php:103-109` via
    // `src/console/format.php:46-52`); blanks re-prompt, EOF fails fast
    // (T1153 CORE-46 keep).
    let mut piped = std::io::BufReader::new("cli-tok\n".as_bytes());
    assert_eq!(
        prompt_token_from(&mut piped, false).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    let mut blanks = std::io::BufReader::new("\n  \ncli-tok\n".as_bytes());
    assert_eq!(
        prompt_token_from(&mut blanks, true).unwrap(),
        "cli-tok".to_string()
    );
    let mut eof = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        prompt_token_from(&mut eof, true).unwrap_err(),
        "no token provided (EOF on stdin)".to_string()
    );
}

#[test]
fn r6_core38_uri_uses_full_precedence_and_default() {
    // T1153 CORE-38: discovery reads `phabricator.uri` from any source, then
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
fn r5_core40_install_anyway_enter_declines() {
    // CLEAN: `ArcanistInstallCertificateWorkflow.php:126` defaults to
    // no (`[y/N]`), so Enter declines and the token is not installed.
    let mut enter = std::io::BufReader::new("\n".as_bytes());
    assert!(!prompt_confirm_from(&mut enter, "Install this token anyway?", true, true).unwrap());
    let mut yes = std::io::BufReader::new("y\n".as_bytes());
    assert!(prompt_confirm_from(&mut yes, "Install this token anyway?", true, true).unwrap());
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

// T1153 CORE-41: the `readUserConfigurationFile()` permission gate.
// Unix-only (PHP skips it on Windows); chmod works as root, so no
// readability tricks are needed.
#[cfg(unix)]
fn open_arcrc(tmp: &TempDir, mode: u32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = tmp.path.join(".arcrc");
    std::fs::write(&path, "{}").unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(mode);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
fn arcrc_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
#[cfg(unix)]
fn core41_open_arcrc_prompts_to_chmod() {
    // Accept narrows to 0600; decline keeps the mode and errors.
    let tmp = TempDir::new("chmod-yes");
    let path = open_arcrc(&tmp, 0o644);
    let mut input = std::io::BufReader::new(std::io::Cursor::new(b"y\n".to_vec()));
    ensure_arcrc_permissions_from(&mut input, &path, true).unwrap();
    assert_eq!(arcrc_mode(&path), 0o600);

    let tmp = TempDir::new("chmod-no");
    let path = open_arcrc(&tmp, 0o644);
    let mut input = std::io::BufReader::new(std::io::Cursor::new(b"n\n".to_vec()));
    assert_eq!(
        ensure_arcrc_permissions_from(&mut input, &path, true).unwrap_err(),
        "Set ~/.arcrc to file mode 600."
    );
    assert_eq!(arcrc_mode(&path), 0o644);
}

#[test]
#[cfg(unix)]
fn core41_chmod_prompt_defaults_yes() {
    // T1153 CORE-40/T1153 CORE-41: PHP passes `$default_no = false`
    // ([Y/n]): empty input chmods.
    let tmp = TempDir::new("chmod-default");
    let path = open_arcrc(&tmp, 0o640);
    let mut input = std::io::BufReader::new(std::io::Cursor::new(b"\n".to_vec()));
    ensure_arcrc_permissions_from(&mut input, &path, true).unwrap();
    assert_eq!(arcrc_mode(&path), 0o600);
}

#[test]
#[cfg(unix)]
fn core41_closed_or_missing_arcrc_skips_prompt() {
    // A 0600 or missing file never prompts: with a non-TTY even an
    // accidental prompt would fail, so Ok proves the skip.
    let tmp = TempDir::new("chmod-skip");
    let path = open_arcrc(&tmp, 0o600);
    let mut input = std::io::BufReader::new(std::io::Cursor::new(Vec::new()));
    ensure_arcrc_permissions_from(&mut input, &path, false).unwrap();
    assert_eq!(arcrc_mode(&path), 0o600);
    let missing = tmp.path.join("no-arcrc");
    let mut input = std::io::BufReader::new(std::io::Cursor::new(Vec::new()));
    ensure_arcrc_permissions_from(&mut input, &missing, false).unwrap();
}

#[test]
#[cfg(unix)]
fn core41_open_arcrc_fails_when_piped() {
    // Non-TTY stdin echoes the prompt once, then fails like PHP's
    // `phutil_console_require_tty` throw.
    let tmp = TempDir::new("chmod-pipe");
    let path = open_arcrc(&tmp, 0o644);
    let mut input = std::io::BufReader::new(std::io::Cursor::new(Vec::new()));
    let err = ensure_arcrc_permissions_from(&mut input, &path, false).unwrap_err();
    assert!(err.contains("not a TTY"), "{err}");
    assert_eq!(arcrc_mode(&path), 0o644);
}
/// Serve `expected` requests from `routes` (method -> envelope
/// result JSON); returns the base URI. (Local replicate of the
/// `export_tests` stub.)
fn serve_install_routes(
    routes: std::collections::HashMap<String, String>,
    expected: usize,
) -> String {
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
            let result = routes.get(&method).cloned().unwrap_or("null".to_string());
            let canned = format!(r#"{{"result": {result}, "error_code": null}}"#);
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
fn token_auth_check_passes_on_modern_servers() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "conduit.getcapabilities".to_string(),
        r#"{"authentication": ["token"]}"#.to_string(),
    );
    let base = serve_install_routes(routes, 1);
    assert!(check_token_auth(&base).is_ok());
}

#[test]
fn whoami_verifies_tokens_against_the_server() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        r#"{"phid": "PHID-USER-1"}"#.to_string(),
    );
    let base = serve_install_routes(routes, 1);
    assert!(whoami(&base, CLI_TOKEN).is_ok());
}

#[test]
fn certificate_flow_stages_exchanged_credentials() {
    let tmp = TempDir::new("certflow-ok");
    let path = tmp.path.join(".arcrc");
    let ok = stub_call(
        [(
            "conduit.getcertificate".to_string(),
            Ok(serde_json::json!({"username": "alice", "certificate": "CERT"})),
        )]
        .into_iter()
        .collect(),
    );
    let mut doc = serde_json::json!({});
    let info =
        install_certificate_flow(&ok, "https://h.example/api/", "tok", &mut doc, &path).unwrap();
    assert_eq!(info.username, "alice");
    assert_eq!(
        doc["hosts"]["https://h.example/api/"]["cert"],
        serde_json::json!("CERT")
    );
}

#[test]
fn stdin_prompts_on_pipes_fail_fast() {
    // Piped stdin is never a TTY under `cargo test`; fail loud
    // (rather than block on a real terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    assert_eq!(prompt_token().unwrap_err(), crate::prompt::NOT_INTERACTIVE);
    assert_eq!(
        prompt_confirm("Q?", true).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    assert_eq!(
        prompt_confirm("Q?", false).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
}

#[test]
fn args_cover_conduit_uri_and_double_positionals() {
    let opts = parse_args(&["--conduit-uri".to_string(), "http://x/".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(opts.conduit_uri.as_deref(), Some("http://x/"));
    assert_eq!(
        parse_args(&["--".to_string(), "a".to_string(), "b".to_string()]).unwrap_err(),
        "Specify at most one URI."
    );
}

fn install_test_options() -> Options {
    Options {
        uri: None,
        conduit_uri: None,
        token: None,
        force: false,
        config: Vec::new(),
        config_files: Vec::new(),
        arcrc_file: None,
    }
}

#[test]
fn install_uri_reports_bad_cli_config() {
    let tmp = TempDir::new("badcli");
    let mut options = install_test_options();
    options.config = vec!["noequals".to_string()];
    assert_eq!(
        resolve_install_uri(&options, &tmp.path),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn install_arcrc_reports_missing_custom_files() {
    let tmp = TempDir::new("noarcrc");
    let missing = tmp.path.join("missing").to_string_lossy().to_string();
    assert_eq!(
        install_arcrc_for(Some(&missing)),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn install_token_reads_prompt_without_a_flag() {
    // Piped stdin is never a TTY under `cargo test`; fail loud
    // (rather than block on a real terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    let options = install_test_options();
    assert_eq!(
        install_token_value(&options),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn standard_token_confirm_skips_with_force() {
    assert!(confirm_standard_token(true).is_ok());
    // Piped stdin is never a TTY under `cargo test`; fail loud
    // (rather than block on a real terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    assert_eq!(
        confirm_standard_token(false),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn install_token_value_with_stages_prompt_arms() {
    // `--token` wins trimmed without prompting; otherwise the
    // injected prompt's success and failure arms route through.
    let mut options = install_test_options();
    options.token = Some("  cli-token  ".to_string());
    assert_eq!(
        install_token_value_with(&options, &|| panic!("must not prompt")),
        Ok("cli-token".to_string())
    );
    let options = install_test_options();
    assert_eq!(
        install_token_value_with(&options, &|| Ok("prompted".to_string())),
        Ok("prompted".to_string())
    );
    assert_eq!(
        install_token_value_with(&options, &|| Err("piped".to_string())),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn confirm_standard_token_with_stages_confirm_arms() {
    // `--force` skips the ask; otherwise the injected confirm's
    // yes/no/error arms route through.
    assert!(confirm_standard_token_with(true, &|_| panic!("must not prompt")).is_ok());
    assert!(confirm_standard_token_with(false, &|_| Ok(true)).is_ok());
    assert_eq!(
        confirm_standard_token_with(false, &|_| Ok(false)),
        Err(std::process::ExitCode::FAILURE)
    );
    assert_eq!(
        confirm_standard_token_with(false, &|_| Err("piped".to_string())),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn token_run_flow_rejects_bad_tokens() {
    let tmp = TempDir::new("tokflow");
    let arcrc = tmp.path.join(".arcrc");
    let mut doc = serde_json::json!({});
    let mut client = conduit::Client::new("https://h.example/", "").unwrap();
    assert_eq!(
        run_token_flow(
            "https://h.example/api/",
            "bad",
            &arcrc,
            &mut doc,
            false,
            &mut client
        ),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn token_run_flow_confirms_standard_tokens() {
    // Piped stdin is never a TTY under `cargo test`; fail loud
    // (rather than block on a real terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    let tmp = TempDir::new("tokflow-std");
    let arcrc = tmp.path.join(".arcrc");
    let mut doc = serde_json::json!({});
    let mut client = conduit::Client::new("https://h.example/", "").unwrap();
    assert_eq!(
        run_token_flow(
            "https://h.example/api/",
            API_TOKEN,
            &arcrc,
            &mut doc,
            false,
            &mut client
        ),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn certificate_run_flow_exchanges_and_stages() {
    let tmp = TempDir::new("certrun");
    let arcrc = tmp.path.join(".arcrc");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "conduit.getcertificate".to_string(),
        r#"{"username": "alice", "certificate": "CERT"}"#.to_string(),
    );
    let base = serve_install_routes(routes, 1);
    let client = conduit::Client::new(&base, "").unwrap();
    let mut doc = serde_json::json!({});
    run_certificate_flow(&format!("{base}api/"), "tok", &arcrc, &mut doc, &client).unwrap();
    assert_eq!(
        doc["hosts"][&format!("{base}api/")]["user"],
        serde_json::json!("alice")
    );
    // An invalid exchange response fails the flow.
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "conduit.getcertificate".to_string(),
        r#"{"username": "alice"}"#.to_string(),
    );
    let base = serve_install_routes(routes, 1);
    let client = conduit::Client::new(&base, "").unwrap();
    let mut doc = serde_json::json!({});
    assert_eq!(
        run_certificate_flow(&format!("{base}api/"), "tok", &arcrc, &mut doc, &client),
        Err(std::process::ExitCode::FAILURE)
    );
}

fn expect_prepare_failure(argv: &[String]) -> std::process::ExitCode {
    let Err(code) = prepare_install(argv) else {
        panic!("expected prepare_install to fail");
    };
    code
}

#[test]
#[cfg(unix)]
fn prepare_gates_on_open_arcrc_permissions() {
    // Piped stdin is never a TTY under `cargo test`; fail loud
    // (rather than block on a real terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    let tmp = TempDir::new("prep-chmod");
    let path = open_arcrc(&tmp, 0o644);
    let argv = vec![
        "https://h.example/".to_string(),
        "--arcrc-file".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let code = expect_prepare_failure(&argv);
    assert_eq!(code, std::process::ExitCode::FAILURE);
}

#[test]
fn prepare_rejects_invalid_arcrc_json() {
    let tmp = TempDir::new("prep-json");
    let path = tmp.path.join(".arcrc");
    std::fs::write(&path, "nope").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    let argv = vec![
        "https://h.example/".to_string(),
        "--arcrc-file".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let code = expect_prepare_failure(&argv);
    assert_eq!(code, std::process::ExitCode::FAILURE);
}

#[test]
fn connect_reports_unbuildable_clients() {
    assert_eq!(
        connect_install_client("bogus", false).unwrap_err(),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn install_flow_writes_legacy_certificates() {
    let tmp = TempDir::new("flows-cert");
    let arcrc = tmp.path.join(".arcrc");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "conduit.getcertificate".to_string(),
        r#"{"username": "alice", "certificate": "CERT"}"#.to_string(),
    );
    let base = serve_install_routes(routes, 1);
    let mut client = conduit::Client::new(&base, "").unwrap();
    let mut doc = serde_json::json!({});
    let uri = format!("{base}api/");
    run_install_flow(&uri, "tok", &arcrc, &mut doc, false, &mut client, false).unwrap();
    let stored = read_token_doc(&arcrc).unwrap();
    assert_eq!(stored["hosts"][&uri]["cert"], serde_json::json!("CERT"));
}

#[test]
fn run_fails_fast_without_a_token() {
    // Piped stdin is never a TTY under `cargo test`; fail loud
    // (rather than block on a real terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    let tmp = TempDir::new("run-notoken");
    let mut routes = std::collections::HashMap::new();
    routes.insert("conduit.ping".to_string(), "{}".to_string());
    routes.insert("conduit.getcapabilities".to_string(), "{}".to_string());
    let base = serve_install_routes(routes, 2);
    let arcrc = tmp.path.join(".arcrc");
    std::fs::write(&arcrc, "{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&arcrc).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&arcrc, permissions).unwrap();
    }
    let argv = vec![
        base.clone(),
        "--arcrc-file".to_string(),
        arcrc.to_string_lossy().to_string(),
    ];
    assert_eq!(run(&argv), std::process::ExitCode::FAILURE);
}

#[test]
fn r10_install_certificate_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "INSTALL-CERTIFICATE WORKFLOW\n\
         \n\
         install-certificate\n\
         \x20     install-certificate [uri]\n\
         \n\
         \x20         Supports: http, https\n\
         \x20         Installs Conduit credentials into your ~/.arcrc for the given\n\
         install\n\
         \x20         of Phabricator. You need to do this before you can use 'arc', as it\n\
         \x20         enables 'arc' to link your command-line activity with your account\n\
         on\n\
         \x20         the web. Run this command from within a project directory to\n\
         install\n\
         \x20         that project's certificate, or specify an explicit URI (like\n\
         \x20         \"https://phabricator.example.com/\").\n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    let text = usage();
    assert!(text.contains("INSTALL-CERTIFICATE WORKFLOW"), "{text}");
}

#[test]
fn b21_install_certificate_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `install-certificate`: it is a
    // classic workflow, so PHP runs it via the
    // `scripts/arcanist.php` re-exec, which never requires `--`.
    // Parsing takes no TTY input, so piped-style argv parses
    // directly, and `--help` without `--` still succeeds.
    assert!(
        parse_args(&["https://host/".to_string()])
            .unwrap()
            .is_some()
    );
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    let text = usage_text(false);
    assert!(text.contains("INSTALL-CERTIFICATE WORKFLOW"), "{text}");
}

#[test]
fn b23_install_certificate_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to
    // `arc help install-certificate`. Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("INSTALL-CERTIFICATE WORKFLOW\n"), "{text}");
    assert!(text.contains("Installs Conduit credentials"), "{text}");
    assert!(!text.contains("--token"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// -( T1170 coverage wave 6g band G, batch E )----------------------------------

/// Best-effort cover for the `metadata` failure arm of
/// `arcrc_mode_is_open`: the error needs the file to vanish between
/// `is_file` and `metadata`, which only a removal race can arrange.
/// Either outcome is accepted, so the test passes deterministically
/// while giving the race every chance to hit.
#[test]
#[cfg(unix)]
fn cov6g_arcrc_mode_metadata_race() {
    let tmp = TempDir::new("cov6g-moderace");
    let target = tmp.path.join("arcrc");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn = std::thread::spawn({
        let (target, stop) = (target.clone(), stop.clone());
        move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::fs::write(&target, "x").ok();
                std::fs::remove_file(&target).ok();
            }
        }
    });
    for _ in 0..10_000 {
        match arcrc_mode_is_open(&target) {
            Ok(_) => {}
            Err(message) => assert!(
                message.contains("Unable to read file permissions"),
                "{message}"
            ),
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    churn.join().unwrap();
}
// -( T1176 S5: defensive-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "expected prepare_install to fail")]
fn prepare_gates_on_open_arcrc_permissions_defensive_ok_arm() {
    // `expect_prepare_failure` unwraps `Err`: a valid private arcrc stages `Ok`.
    let tmp = TempDir::new("prep-ok");
    let path = tmp.path.join("fresh-arcrc");
    std::fs::write(&path, "{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    let argv = vec![
        "https://h.example/".to_string(),
        "--arcrc-file".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let _code = expect_prepare_failure(&argv);
}
