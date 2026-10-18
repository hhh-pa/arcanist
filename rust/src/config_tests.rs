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
    // T1153 CORE-33: git reads `.arcconfig` only at the working-copy root; a
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

#[test]
fn core4_credentials_gate_matrix() {
    // The classic `$need_auth` gate: a token or a complete legacy pair,
    // all under `empty()` semantics (`"0"` counts as missing).
    let full = Credentials {
        token: Some("t".to_string()),
        user: None,
        cert: None,
    };
    assert!(full.has_any());
    let legacy = Credentials {
        token: None,
        user: Some("u".to_string()),
        cert: Some("c".to_string()),
    };
    assert!(legacy.has_any());
    for creds in [
        Credentials::default(),
        Credentials {
            token: Some("0".to_string()),
            user: None,
            cert: None,
        },
        Credentials {
            token: None,
            user: Some("u".to_string()),
            cert: None,
        },
        Credentials {
            token: None,
            user: Some("0".to_string()),
            cert: Some("c".to_string()),
        },
    ] {
        assert!(!creds.has_any(), "{creds:?}");
    }
}

#[test]
fn core4_credentials_for_reads_legacy_pair() {
    let dir = std::env::temp_dir().join(format!("arc-rs-core4-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let user = dir.join("arcrc.json");
    std::fs::write(
        &user,
        r#"{"hosts": {"https://h.test/api/": {"user": "u", "cert": "c"}}}"#,
    )
    .unwrap();
    let creds = credentials_for(Some(&user), "https://h.test/", None);
    assert_eq!(creds.token, None);
    assert_eq!(creds.user.as_deref(), Some("u"));
    assert_eq!(creds.cert.as_deref(), Some("c"));
    assert!(creds.has_any());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core4_tokenless_client_builds_without_credentials() {
    // PHP's engine construction never fails for missing credentials.
    // The URI rides `--config` runtime (no VCS root needed), and the
    // shadow keeps ambient user/system sources out.
    let (dir, mut shadow) = r6_shadowed("core4");
    shadow.runtime.insert(
        "phabricator.uri".to_string(),
        serde_json::json!("https://h.test/"),
    );
    let user = dir.join("arcrc.json");
    std::fs::write(&user, "{}").unwrap();
    let client = client_for_tokenless_with(None, &dir, Some(&user), &shadow).unwrap();
    assert!(!client.has_token());
    assert_eq!(client.base_uri(), "https://h.test/api/");
    // The strict path still errors (other scopes pin the message).
    let err = client_for_with(None, &dir, Some(&user), &shadow).unwrap_err();
    assert!(err.starts_with("no token for https://h.test/"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core4_require_auth_gate_text() {
    let text = require_auth_text("phab.example", None);
    assert!(
        text.contains("YOU NEED TO AUTHENTICATE TO CONTINUE"),
        "{text}"
    );
    assert!(
        text.contains(
            "You are trying to connect to a server (phab.example) that you do not have any \
             credentials stored for."
        ),
        "{text}"
    );
    // The `**` markers are pattern markup: stripped without ANSI,
    // like PHP's `interpretFormat`.
    assert!(text.contains("$ arc install-certificate"), "{text}");
    assert!(!text.contains("**"), "{text}");
    let forced = require_auth_text("h.test", Some("https://h.test/api/"));
    assert!(
        forced.contains("$ arc --conduit-uri=https://h.test/api/ install-certificate"),
        "{forced}"
    );
}

#[test]
fn core4_uri_domain_strips_port_and_userinfo() {
    assert_eq!(uri_domain("https://h.test/api/"), "h.test");
    assert_eq!(uri_domain("http://h.test:8080/foo"), "h.test");
    assert_eq!(uri_domain("https://u@h.test/"), "h.test");
    assert_eq!(uri_domain("https://[::1]:8080/"), "[::1]");
}

#[test]
fn core8_tls_cabundle_missing_fails_client() {
    let (dir, mut shadow) = r6_shadowed("core8");
    shadow.runtime.insert(
        "phabricator.uri".to_string(),
        serde_json::json!("https://h.test/"),
    );
    shadow.runtime.insert(
        "https.cabundle".to_string(),
        serde_json::json!("/nonexistent-ca-bundle.pem"),
    );
    let user = dir.join("arcrc.json");
    std::fs::write(
        &user,
        r#"{"hosts": {"https://h.test/api/": {"token": "t"}}}"#,
    )
    .unwrap();
    let err = client_for_with(None, &dir, Some(&user), &shadow).unwrap_err();
    assert!(err.contains("failed to read CA bundle"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core10_force_token_overrides_file() {
    assert_eq!(
        resolve_token(Some("forced"), Some("file".to_string())),
        Some("forced".to_string())
    );
    assert_eq!(
        resolve_token(None, Some("file".to_string())),
        Some("file".to_string())
    );
    assert_eq!(
        resolve_token(Some(""), Some("file".to_string())),
        Some("file".to_string())
    );
    assert_eq!(resolve_token(None, None), None);
}

// -( T1170 wave 5d )-----------------------------------------------------------

#[test]
fn core4_missing_custom_arcrc_fails_client() {
    let (dir, mut shadow) = r6_shadowed("core4arcrc");
    shadow.arcrc_file = Some(dir.join("no-such-arcrc.json"));
    let err = client_for_tokenless_with(None, &dir, None, &shadow).unwrap_err();
    assert!(err.contains("arcrc"), "{err}");
}

#[test]
fn core8_relative_cabundle_resolves_against_project() {
    let (dir, mut shadow) = r6_shadowed("core8rel");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phabricator.uri": "https://h.test/"}"#,
    )
    .unwrap();
    shadow.runtime.insert(
        "https.cabundle".to_string(),
        serde_json::json!("rel/ca.pem"),
    );
    let tls = tls_config_for(&dir, None, &shadow);
    assert_eq!(tls.cabundle, Some(dir.join("rel/ca.pem")));
    // Without a project file the linted directory is the root.
    let (plain, mut shadow) = r6_shadowed("core8plain");
    shadow.runtime.insert(
        "https.cabundle".to_string(),
        serde_json::json!("rel/ca.pem"),
    );
    let tls = tls_config_for(&plain, None, &shadow);
    assert_eq!(tls.cabundle, Some(plain.join("rel/ca.pem")));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn core4_authed_client_maps_setup_failures() {
    // A URI that fails client construction (not URI discovery)
    // classifies as a generic exception, not a usage error.
    let (dir, shadow) = r6_shadowed("core4setup");
    let err = authed_client_for(Some("::::"), &dir, None, &shadow, "test").unwrap_err();
    assert!(matches!(err.kind, crate::conduit::EnvelopeKind::Exception));
    assert!(
        err.message.contains("must include a valid host"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Serve one canned Conduit envelope, then close.
fn serve_once(body: String) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).unwrap();
        let len: usize = head
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
            })
            .unwrap_or(0);
        let mut raw = vec![0u8; len];
        stream.read_exact(&mut raw).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    base
}

/// House scratch dir (pid + per-file counter); removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-config-pc1-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// Spawn this test binary with `vars` set and `removals` removed,
/// running only `probe` (child-process env staging: no in-process
/// mutation, so no `unsafe` and no serial lock).
fn spawn_probe_env(
    probe: &str,
    vars: &[(&str, &str)],
    removals: &[&str],
) -> (bool, String, String) {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = std::process::Command::new(&exe);
    for (key, value) in vars {
        command.env(key, value);
    }
    for key in removals {
        command.env_remove(key);
    }
    let out = command
        .args(["--nocapture", probe])
        .output()
        .expect("spawn probe child");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Assert a probe child passed and printed its marker.
fn assert_probe_env_ok(probe: &str, vars: &[(&str, &str)], removals: &[&str], marker: &str) {
    let (success, stdout, stderr) = spawn_probe_env(probe, vars, removals);
    assert!(
        success && stdout.contains("1 passed") && stdout.contains(marker),
        "{probe} child failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn core9_authenticate_maps_unknown_failures_raw() {
    // A `conduit.connect` failure outside the three credential codes
    // propagates raw under the generic arm (no install guidance).
    let base = serve_once(
        r#"{"result": null, "error_code": "ERR-OTHER", "error_info": "boom"}"#.to_string(),
    );
    let mut client = crate::conduit::Client::new(&base, "").unwrap();
    let creds = Credentials {
        token: None,
        user: Some("u".to_string()),
        cert: Some("c".to_string()),
    };
    let err = authenticate(&mut client, &creds, "test").unwrap_err();
    assert!(matches!(err.kind, crate::conduit::EnvelopeKind::Exception));
    assert!(err.message.contains("ERR-OTHER"), "{}", err.message);
}

// -( T1176 PC-1: env-globals )------------------------------------------------

/// Home env key `arcrc_path()` reads on this platform.
#[cfg(windows)]
const HOME_KEY: &str = "USERPROFILE";
#[cfg(not(windows))]
const HOME_KEY: &str = "HOME";

#[test]
fn pc1_arcrc_path_home_probe() {
    if std::env::var("ARC_RS_PC1_HOME_CHILD").as_deref() != Ok("1") {
        println!("PC1-HOME-PROBE-OK");
        return;
    }
    match std::env::var_os(HOME_KEY) {
        None => assert_eq!(arcrc_path(), None),
        Some(home) => assert_eq!(arcrc_path(), Some(PathBuf::from(home).join(".arcrc"))),
    }
    println!("PC1-HOME-PROBE-OK");
}

#[test]
fn pc1_arcrc_path_without_home_returns_none() {
    // HOME removed in the child: no arcrc path ...
    assert_probe_env_ok(
        "pc1_arcrc_path_home_probe",
        &[("ARC_RS_PC1_HOME_CHILD", "1")],
        &[HOME_KEY],
        "PC1-HOME-PROBE-OK",
    );
    // ... and a set HOME resolves `~/.arcrc`.
    let home = TempDir::new("home");
    let home_str = home.path().to_str().expect("utf8 home").to_string();
    assert_probe_env_ok(
        "pc1_arcrc_path_home_probe",
        &[
            ("ARC_RS_PC1_HOME_CHILD", "1"),
            (HOME_KEY, home_str.as_str()),
        ],
        &[],
        "PC1-HOME-PROBE-OK",
    );
}

#[test]
fn pc1_global_load_options_probe() {
    if std::env::var("ARC_RS_PC1_OPTS_CHILD").as_deref() != Ok("1") {
        println!("PC1-OPTS-PROBE-OK");
        return;
    }
    let opts = global_load_options();
    let case = std::env::var("ARC_RS_PC1_OPTS_CASE").expect("probe case");
    if case == "empty" {
        assert!(opts.runtime.is_empty());
        assert!(opts.config_files.is_empty());
        assert_eq!(opts.arcrc_file, None);
    } else {
        assert_eq!(
            opts.runtime.get("phabricator.uri"),
            Some(&serde_json::json!("https://rt.example/"))
        );
        assert_eq!(opts.config_files, vec![PathBuf::from("a.json")]);
        assert_eq!(opts.arcrc_file, Some(PathBuf::from("u.json")));
    }
    println!("PC1-OPTS-PROBE-OK");
}

#[test]
fn pc1_global_load_options_without_channel_reads_default() {
    assert_probe_env_ok(
        "pc1_global_load_options_probe",
        &[
            ("ARC_RS_PC1_OPTS_CHILD", "1"),
            ("ARC_RS_PC1_OPTS_CASE", "empty"),
        ],
        &["ARC_GLOBAL_LOAD_OPTIONS"],
        "PC1-OPTS-PROBE-OK",
    );
}

#[test]
fn pc1_global_load_options_with_channel_parses() {
    let raw = r#"{"runtime": {"phabricator.uri": "https://rt.example/"}, "config_files": ["a.json", 42], "arcrc_file": "u.json"}"#;
    assert_probe_env_ok(
        "pc1_global_load_options_probe",
        &[
            ("ARC_RS_PC1_OPTS_CHILD", "1"),
            ("ARC_RS_PC1_OPTS_CASE", "json"),
            ("ARC_GLOBAL_LOAD_OPTIONS", raw),
        ],
        &[],
        "PC1-OPTS-PROBE-OK",
    );
}

#[test]
fn pc1_parse_load_options_json_fixture_matrix() {
    // Full document.
    let opts = parse_load_options_json(
        r#"{"runtime": {"a": 1}, "config_files": ["x", "y"], "arcrc_file": "z"}"#,
    );
    assert_eq!(opts.runtime.len(), 1);
    assert_eq!(
        opts.config_files,
        vec![PathBuf::from("x"), PathBuf::from("y")]
    );
    assert_eq!(opts.arcrc_file, Some(PathBuf::from("z")));
    // Missing keys read empty.
    let opts = parse_load_options_json("{}");
    assert!(opts.runtime.is_empty());
    assert!(opts.config_files.is_empty());
    assert_eq!(opts.arcrc_file, None);
    // Mistyped values read empty (only the dispatcher writes this
    // channel, so leniency mirrors the pre-extraction behavior).
    let opts =
        parse_load_options_json(r#"{"runtime": [1], "config_files": {"x": 1}, "arcrc_file": 7}"#);
    assert!(opts.runtime.is_empty());
    assert!(opts.config_files.is_empty());
    assert_eq!(opts.arcrc_file, None);
    // Non-string list members drop; unparseable input reads default.
    let opts = parse_load_options_json(r#"{"config_files": ["x", null, 3]}"#);
    assert_eq!(opts.config_files, vec![PathBuf::from("x")]);
    for raw in ["not json", "", "42", "[1]"] {
        let opts = parse_load_options_json(raw);
        assert!(opts.runtime.is_empty(), "for {raw:?}");
        assert!(opts.config_files.is_empty(), "for {raw:?}");
        assert_eq!(opts.arcrc_file, None, "for {raw:?}");
    }
}
