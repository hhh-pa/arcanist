//! End-to-end coverage for main-only global cells (T1170 COV).
//!
//! The `--ansi` / `--conduit-uri` / `--conduit-token` / `--anonymous` /
//! `--arcrc-file` setters live behind process-global `OnceLock`s that
//! only `main` may write (unit tests stay hermetic by never touching
//! them), so these tests drive the real `arc` binary in a child process
//! — like `dispatch_core.rs` — with an empty `HOME`, scrubbed env, and
//! piped stdin. The conduit half talks to a loopback mock, never the
//! network.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn fixture(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "arc-rs-globals-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let home = root.join("home");
    let repo = root.join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    (home, repo)
}

fn arc(home: &PathBuf, repo: &PathBuf, argv: &[&str], stdin_bytes: Option<&[u8]>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_arc"))
        .args(argv)
        .env("HOME", home)
        .env("ARC_TRACE", "")
        .env("ARC_ANSI", "")
        .env("ARC_GLOBAL_CONDUIT_URI", "")
        .env("ARC_GLOBAL_CONDUIT_TOKEN", "")
        .env("ARC_ANONYMOUS", "")
        .env("ARC_GLOBAL_LOAD_OPTIONS", "")
        .env("ARC_CONDUIT_URI", "")
        .env("ARC_CONDUIT_TOKEN", "")
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(bytes) = stdin_bytes {
        child.stdin.as_mut().unwrap().write_all(bytes).unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Minimal loopback mock: serves one canned response, reports the raw
/// request body it saw.
fn serve_once(canned: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                let want: usize = head
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(key, value)| {
                            key.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap_or(0))
                        })
                    })
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + want {
                    break;
                }
            }
            if buf.len() > 1_000_000 {
                break;
            }
        }
        let body = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|pos| buf[pos + 4..].to_vec())
            .unwrap_or_default();
        let _ = tx.send(String::from_utf8_lossy(&body).to_string());
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            canned.len(),
            canned
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (base, rx)
}

#[test]
fn global_cells_set_and_force_ansi_output() {
    // T1170 COV: every global setter runs in the child (globals.rs
    // set_ansi/set_conduit_uri/set_conduit_token/set_anonymous/
    // set_load_options), and `--ansi` forces ANSI styling into piped
    // help output via the ansi_override Some arm.
    let (home, repo) = fixture("setters");
    let arcrc = home.join("arcrc");
    std::fs::write(&arcrc, "{}").unwrap();
    let arcrc = arcrc.to_string_lossy().to_string();
    let out = arc(
        &home,
        &repo,
        &[
            "--ansi",
            "--conduit-uri",
            "https://conduit.invalid/",
            "--conduit-token",
            "tok",
            "--anonymous",
            "--arcrc-file",
            &arcrc,
            "help",
            "diff",
            "--",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("DIFF WORKFLOW"), "{stdout:?}");
    assert!(
        stdout.contains("\x1b["),
        "expected forced ANSI codes, got: {stdout:?}"
    );
    assert!(text(&out.stderr).is_empty());
}

#[test]
fn anonymous_flag_suppresses_token_on_the_wire() {
    // T1170 COV: `--anonymous` flows through credentials_for (the
    // anonymous Some arm) so even an explicit `--conduit-token` never
    // reaches the request body.
    let (base, seen) = serve_once(r#"{"result": "ok-ping", "error_code": null}"#);
    let (home, repo) = fixture("anon");
    let out = arc(
        &home,
        &repo,
        &[
            "--anonymous",
            "--conduit-token",
            "SECRET-should-not-appear",
            "call-conduit",
            "--conduit-uri",
            &base,
            "--",
            "conduit.ping",
        ],
        Some(b"{}"),
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("ok-ping"),
        "{:?}",
        text(&out.stdout)
    );
    let body = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(
        !body.contains("SECRET-should-not-appear"),
        "token leaked despite --anonymous: {body}"
    );
}
