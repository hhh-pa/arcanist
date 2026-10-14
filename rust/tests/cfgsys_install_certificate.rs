// Shared-helper shims: the module under test addresses the library as
// `arc_rs::...` (via its crate alias), which resolves to these includes
// in test builds and to the real library once the module is wired in.
#[allow(dead_code)]
#[path = "../src/conduit.rs"]
mod conduit;
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/git.rs"]
mod git;

#[path = "../src/install_certificate.rs"]
mod install_certificate;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const CLI_TOKEN: &str = "cli-0123456789abcdef0123456789ab";
const API_TOKEN: &str = "api-0123456789abcdef0123456789ab";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-cfgsys-cert-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct HomeGuard {
    saved_home: Option<std::ffi::OsString>,
    saved_profile: Option<std::ffi::OsString>,
}

fn with_temp_home(dir: &Path) -> HomeGuard {
    let guard = HomeGuard {
        saved_home: std::env::var_os("HOME"),
        saved_profile: std::env::var_os("USERPROFILE"),
    };
    unsafe {
        std::env::set_var("HOME", dir);
        std::env::set_var("USERPROFILE", dir);
    }
    guard
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.saved_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.saved_profile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }
}

/// Serve a script of canned Conduit responses on 127.0.0.1, one per
/// connection, returning the base URI and the observed request lines.
fn serve_script(canned: Vec<&'static str>) -> (String, std::sync::mpsc::Receiver<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for body in canned {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .ok();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).to_string();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .or_else(|| line.strip_prefix("content-length: "))
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap_or(0);
            let mut discard = vec![0u8; len];
            stream.read_exact(&mut discard).ok();
            seen.push(head.lines().next().unwrap_or("").to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).ok();
        }
        tx.send(seen).ok();
    });
    (base, rx)
}

fn recv_lines(rx: &std::sync::mpsc::Receiver<Vec<String>>) -> Vec<String> {
    rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap()
}

#[test]
fn module_name_matches_arc() {
    assert_eq!(install_certificate::NAME, "install-certificate");
    assert_eq!(CLI_TOKEN.len(), 32);
    assert_eq!(API_TOKEN.len(), 32);
}

#[test]
fn run_help_and_arg_errors_need_no_network() {
    assert_eq!(
        install_certificate::run(&["--help".to_string()]),
        ExitCode::SUCCESS
    );
    assert_eq!(
        install_certificate::run(&["a".to_string(), "b".to_string()]),
        ExitCode::FAILURE
    );
    assert_eq!(
        install_certificate::run(&["notauri".to_string()]),
        ExitCode::FAILURE
    );
    assert_eq!(
        install_certificate::run(&["--bogus".to_string()]),
        ExitCode::FAILURE
    );
}

#[test]
fn bad_token_fails_before_network() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("badtoken");
    let _home = with_temp_home(tmp.path());
    // Unroutable URI: success would require network, so FAILURE here
    // proves format validation runs before any connection attempt.
    assert_eq!(
        install_certificate::run(&[
            "http://127.0.0.1:9/".to_string(),
            "--token".to_string(),
            "short".to_string()
        ]),
        ExitCode::FAILURE
    );
    assert!(!tmp.path().join(".arcrc").exists());
    assert_eq!(
        install_certificate::run(&[
            "http://127.0.0.1:9/".to_string(),
            "--token".to_string(),
            API_TOKEN.to_string()
        ]),
        ExitCode::FAILURE
    );
}

#[test]
fn full_install_pings_verifies_and_writes_arcrc() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("install");
    let _home = with_temp_home(tmp.path());
    let (base, rx) = serve_script(vec![
        r#"{"result": true, "error_code": null}"#,
        r#"{"result": {"authentication": ["token"]}, "error_code": null}"#,
        r#"{"result": {"user": "alice"}, "error_code": null}"#,
    ]);
    assert_eq!(
        install_certificate::run(&[base.clone(), "--token".to_string(), CLI_TOKEN.to_string()]),
        ExitCode::SUCCESS
    );
    let lines = recv_lines(&rx);
    assert_eq!(lines.len(), 3);
    assert!(
        lines[0].starts_with("POST /api/conduit.ping "),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("POST /api/conduit.getcapabilities "),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("POST /api/user.whoami "),
        "{}",
        lines[2]
    );

    let arcrc = tmp.path().join(".arcrc");
    let text = std::fs::read_to_string(&arcrc).unwrap();
    let expected_key = format!("{base}api/");
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        doc["hosts"][expected_key.as_str()]["token"],
        serde_json::json!(CLI_TOKEN)
    );
    assert_eq!(
        config::token_for_str(&text, &base),
        Some(CLI_TOKEN.to_string())
    );
}

#[test]
fn rejected_token_writes_nothing() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("rejected");
    let _home = with_temp_home(tmp.path());
    let (base, rx) = serve_script(vec![
        r#"{"result": true, "error_code": null}"#,
        r#"{"result": {"authentication": ["token"]}, "error_code": null}"#,
        r#"{"result": null, "error_code": "ERR-INVALID-AUTH", "error_info": "bad token"}"#,
    ]);
    assert_eq!(
        install_certificate::run(&[base, "--token".to_string(), CLI_TOKEN.to_string()]),
        ExitCode::FAILURE
    );
    assert_eq!(recv_lines(&rx).len(), 3);
    assert!(!tmp.path().join(".arcrc").exists());
}

#[test]
fn dead_server_and_legacy_auth_fail_cleanly() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("dead");
    let _home = with_temp_home(tmp.path());
    let (base, rx) = serve_script(vec![
        r#"{"result": null, "error_code": "E", "error_info": "down"}"#,
    ]);
    assert_eq!(
        install_certificate::run(&[base, "--token".to_string(), CLI_TOKEN.to_string()]),
        ExitCode::FAILURE
    );
    assert_eq!(recv_lines(&rx).len(), 1);
    assert!(!tmp.path().join(".arcrc").exists());

    let (legacy, legacy_rx) = serve_script(vec![
        r#"{"result": true, "error_code": null}"#,
        r#"{"result": {"authentication": ["web"]}, "error_code": null}"#,
    ]);
    let uri = install_certificate::normalize_uri(&legacy).unwrap();
    assert!(install_certificate::ping(&uri).is_ok());
    assert!(install_certificate::check_token_auth(&uri).is_err());
    assert_eq!(recv_lines(&legacy_rx).len(), 2);
}
