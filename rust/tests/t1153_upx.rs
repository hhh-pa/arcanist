//! T1153 Wave-2 UPX wire/CLI tests (upload/paste/export retrocompat).
//!
//! Finding-prefixed, additive-only: no existing test is touched here.
//! Conduit-backed CLI paths run under an owned `HOME` (with a token for
//! the mock URI) behind the shared env lock.

mod common;

use arc_rs::{export, paste, upload};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-t1153-upx-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

/// Serve `count` requests from `routes` (method -> envelope); unknown
/// methods get a conduit error. Returns the base URI and the observed
/// (request line, body) pairs.
fn serve_log(
    routes: HashMap<String, String>,
    count: usize,
) -> (String, std::sync::mpsc::Receiver<Vec<(String, String)>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..count {
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
            let line = head.lines().next().unwrap_or("").to_string();
            let method = line
                .strip_prefix("POST /api/")
                .and_then(|rest| rest.split(' ').next())
                .unwrap_or("")
                .to_string();
            seen.push((line, String::from_utf8(raw).unwrap()));
            let body = routes.get(&method).cloned().unwrap_or_else(|| {
                r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "no such route"}"#
                    .to_string()
            });
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        tx.send(seen).unwrap();
    });
    (base, rx)
}

fn seen(rx: &std::sync::mpsc::Receiver<Vec<(String, String)>>) -> Vec<(String, String)> {
    rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap()
}

/// Owned `HOME` with a token for `conduit_base`; returns the home dir.
/// The guard must stay alive for the whole test.
fn home_with_token(tag: &str, conduit_base: &str) -> (PathBuf, common::HomeGuard) {
    let home = tempdir(tag);
    let after_scheme = conduit_base.split("://").nth(1).unwrap_or(conduit_base);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let scheme = conduit_base.split("://").next().unwrap_or("http");
    let key = format!("{scheme}://{authority}/api/");
    std::fs::write(
        home.join(".arcrc"),
        format!(r#"{{"hosts": {{"{key}": {{"token": "t"}}}}}}"#),
    )
    .unwrap();
    let guard = common::with_owned_home(home.clone());
    (home, guard)
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_repo(tag: &str) -> PathBuf {
    let dir = tempdir(tag);
    run_git(&dir, &["init", "-q", "-b", "master"]);
    run_git(&dir, &["config", "user.email", "t@t"]);
    run_git(&dir, &["config", "user.name", "t"]);
    run_git(&dir, &["config", "commit.gpgsign", "false"]);
    dir
}

#[test]
fn t1153_upx_u3_upload_prints_before_browse() {
    let dir = tempdir("u3");
    std::fs::write(dir.join("a.bin"), b"data").unwrap();
    // A bogus browser fails deterministically (a fallback opener may
    // exist); the project file counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"browser": ["definitely-not-a-browser-xyz"]}"#,
    )
    .unwrap();
    let mut routes = HashMap::new();
    routes.insert(
        "file.allocate".to_string(),
        envelope(r#"{"upload": false, "filePHID": "PHID-FILE-7"}"#),
    );
    routes.insert(
        "file.search".to_string(),
        envelope(
            r#"{"data": [{"id": 7, "phid": "PHID-FILE-7", "fields": {"name": "a.bin", "uri": "/F7"}}]}"#,
        ),
    );
    let (base, rx) = serve_log(routes, 2);
    let (_home, _guard) = home_with_token("u3-home", &base);
    // The broken browser fails AFTER the refs print.
    let mut printed = String::new();
    let code = upload::run_in_with_sink(
        &dir,
        // T1153 BROWSE-B21: piped runs carry `--`.
        &argv(&["--conduit-uri", &base, "--browse", "a.bin", "--"]),
        &mut |text| printed.push_str(text),
    );
    assert!(printed.contains("F7 a.bin"), "{printed:?}");
    assert!(printed.contains("/F7"), "{printed:?}");
    assert_eq!(code, std::process::ExitCode::from(255));
    assert_eq!(seen(&rx).len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_p3_paste_prints_before_browse() {
    let dir = tempdir("p3");
    std::fs::write(dir.join("body.txt"), "hello\n").unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"browser": ["definitely-not-a-browser-xyz"]}"#,
    )
    .unwrap();
    let mut routes = HashMap::new();
    routes.insert(
        "paste.edit".to_string(),
        envelope(r#"{"object": {"phid": "PHID-PSTE-5"}}"#),
    );
    routes.insert(
        "paste.search".to_string(),
        envelope(
            r#"{"data": [{"id": 5, "phid": "PHID-PSTE-5", "fields": {"title": "t", "uri": "/P5"}}]}"#,
        ),
    );
    let (base, rx) = serve_log(routes, 2);
    let (_home, _guard) = home_with_token("p3-home", &base);
    // The broken browser fails AFTER the paste prints.
    let mut printed = String::new();
    let code = paste::run_in_with_sink(
        &dir,
        // T1153 BROWSE-B21: piped runs carry `--`.
        &argv(&[
            "--conduit-uri",
            &base,
            "--browse",
            "--input",
            "body.txt",
            "--title",
            "t",
            "--",
        ]),
        &mut |text| printed.push_str(text),
    );
    assert!(printed.contains("P5"), "{printed:?}");
    assert!(printed.contains("/P5"), "{printed:?}");
    assert_eq!(code, std::process::ExitCode::from(255));
    assert_eq!(seen(&rx).len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_p18_paste_envelope_classes() {
    // Usage errors (bad symbol): exit 1 with no conduit traffic (conduit
    // must be configured so validation is reached, like PHP's runtime).
    let dir = tempdir("p18");
    let (base, rx) = serve_log(HashMap::new(), 0);
    let (_home, _guard) = home_with_token("p18-home", &base);
    let mut printed = String::new();
    // T1153 BROWSE-B21: `--` present, so symbol validation (not
    // the terminator) reports.
    let code = paste::run_in_with_sink(
        &dir,
        &argv(&["--conduit-uri", &base, "Px!", "--"]),
        &mut |text| printed.push_str(text),
    );
    assert_eq!(code, std::process::ExitCode::FAILURE);
    assert!(printed.is_empty(), "{printed:?}");
    assert!(seen(&rx).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_e30_local_git_needs_conduit_and_renders() {
    let dir = fixture_repo("e30");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "one"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "two"]);
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "user.query".to_string(),
        envelope(r#"{"1": {"realName": "Ada"}}"#),
    );
    let (base, rx) = serve_log(routes, 2);
    let (_home, _guard) = home_with_token("e30-home", &base);
    let text = export::execute(&dir, &argv(&["--git", "--conduit-uri", &base, "HEAD~1"])).unwrap();
    assert!(text.contains("diff --git a/f.txt b/f.txt\n"), "{text}");
    assert!(text.contains("@@ -1 +1,2 @@\n"), "{text}");
    assert!(text.ends_with("two\n\n"), "{text:?}");
    let calls = seen(&rx);
    assert_eq!(calls.len(), 2);
    assert!(
        calls.iter().any(|(line, _)| line.contains("user.query")),
        "{calls:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_e7_empty_local_errors_with_conduit() {
    let dir = fixture_repo("e7");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "one"]);
    let (base, rx) = serve_log(HashMap::new(), 0);
    let (_home, _guard) = home_with_token("e7-home", &base);
    // Explicit HEAD: empty range, and the failure precedes any conduit
    // traffic (like PHP's parseDiff-before-user.query).
    let err = export::execute(&dir, &argv(&["--git", "--conduit-uri", &base, "HEAD"])).unwrap_err();
    assert_eq!(err, "Can't parse an empty diff!");
    assert!(seen(&rx).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_encode_sink_with_conduit() {
    // Relocated from export_tests (T1153 EXPORT-E30): local git needs
    // conduit now, so the sink-encoding coverage lives here.
    let dir = fixture_repo("enc");
    std::fs::write(dir.join("f.txt"), "plain\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "one"]);
    std::fs::write(dir.join("f.txt"), "caf\u{e9}\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "two"]);
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "user.query".to_string(),
        envelope(r#"{"1": {"realName": "Ada"}}"#),
    );
    let (base, rx) = serve_log(routes, 4);
    let (_home, _guard) = home_with_token("enc-home", &base);
    let bytes = export::execute_bytes(
        &dir,
        &argv(&[
            "--git",
            "--encoding",
            "windows-1252",
            "--conduit-uri",
            &base,
            "HEAD~1",
        ]),
    )
    .unwrap();
    assert!(bytes.windows(6).any(|w| w == b"+caf\xE9\n"), "{bytes:?}");
    let err = export::execute_bytes(
        &dir,
        &argv(&[
            "--git",
            "--encoding",
            "shift-jis",
            "--conduit-uri",
            &base,
            "HEAD~1",
        ]),
    )
    .unwrap_err();
    assert_eq!(err, "Unknown encoding \"shift-jis\".");
    assert_eq!(seen(&rx).len(), 4);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_e8_conduit_git_skips_getrawdiff() {
    let dir = tempdir("e8");
    let change = serde_json::json!({
        "result": {
            "31": {
                "id": 31,
                "changes": [{
                    "metadata": {},
                    "oldPath": "a.txt",
                    "currentPath": "a.txt",
                    "awayPaths": [],
                    "oldProperties": [],
                    "newProperties": [],
                    "type": "2",
                    "fileType": "1",
                    "commitHash": null,
                    "hunks": [{
                        "oldOffset": "1", "oldLength": "1",
                        "newOffset": "1", "newLength": "1",
                        "addLines": 1, "delLines": 1,
                        "isMissingOldNewline": false,
                        "isMissingNewNewline": false,
                        "corpus": "-a\n+b\n",
                    }],
                }],
            },
        },
        "error_code": null,
    })
    .to_string();
    let mut routes = HashMap::new();
    routes.insert("differential.querydiffs".to_string(), change);
    // Exactly one call served: any getrawdiff attempt fails the request.
    let (base, rx) = serve_log(routes, 1);
    let (_home, _guard) = home_with_token("e8-home", &base);
    let text = export::execute(
        &dir,
        &argv(&["--git", "--conduit-uri", &base, "--revision", "1"]),
    )
    .unwrap();
    assert!(text.contains("diff --git a/a.txt b/a.txt\n"), "{text}");
    assert!(text.contains("@@ -1 +1 @@\n-a\n+b\n"), "{text}");
    let calls = seen(&rx);
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.contains("differential.querydiffs"), "{calls:?}");
    assert!(calls[0].1.contains("revisionIDs"), "{calls:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_e25_raw_revision_server_decides() {
    let dir = tempdir("e25");
    let mut routes = HashMap::new();
    routes.insert(
        "differential.querydiffs".to_string(),
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "Expected a numeric scalar."}"#
            .to_string(),
    );
    let (base, rx) = serve_log(routes, 1);
    let (_home, _guard) = home_with_token("e25-home", &base);
    let err = export::execute(
        &dir,
        &argv(&["--git", "--conduit-uri", &base, "--revision", "abc"]),
    )
    .unwrap_err();
    assert!(err.contains("Expected a numeric scalar"), "{err}");
    let calls = seen(&rx);
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.contains("abc"), "{calls:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1153_upx_e1_default_base_end_to_end() {
    // Full chain: default base from upstream, committed range, bundle render.
    let scratch = tempdir("e2e");
    let remote = scratch.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());
    let work = scratch.join("work");
    let status = std::process::Command::new("git")
        .args(["clone", "-q"])
        .arg(&remote)
        .arg(&work)
        .status()
        .unwrap();
    assert!(status.success());
    run_git(&work, &["config", "user.email", "t@t"]);
    run_git(&work, &["config", "user.name", "t"]);
    run_git(&work, &["config", "commit.gpgsign", "false"]);
    run_git(&work, &["checkout", "-qb", "master"]);
    std::fs::write(work.join("f.txt"), "one\n").unwrap();
    run_git(&work, &["add", "."]);
    run_git(&work, &["commit", "-qm", "one"]);
    run_git(&work, &["push", "-q", "-u", "origin", "master"]);
    std::fs::write(work.join("f.txt"), "one\ntwo\n").unwrap();
    run_git(&work, &["add", "."]);
    run_git(&work, &["commit", "-qm", "two"]);
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "user.query".to_string(),
        envelope(r#"{"1": {"realName": "Ada"}}"#),
    );
    let (base, rx) = serve_log(routes, 2);
    let (_home, _guard) = home_with_token("e2e-home", &base);
    // No positional: the upstream answers (any prompt would fail the test).
    let text = export::execute(&work, &argv(&["--git", "--conduit-uri", &base])).unwrap();
    assert!(text.contains("diff --git a/f.txt b/f.txt\n"), "{text}");
    assert!(text.contains("@@ -1 +1,2 @@\n"), "{text}");
    assert!(text.contains("+two\n"), "{text}");
    assert_eq!(seen(&rx).len(), 2);
    std::fs::remove_dir_all(&scratch).ok();
}

fn piped_stdin() -> bool {
    use std::io::IsTerminal;
    !std::io::stdin().is_terminal()
}

#[test]
fn b21_upload_requires_terminator_when_piped() {
    // T1153 BROWSE-B21: `upload` is a toolset workflow, so piped
    // runs without `--` fail (exit 1), even for `--help`; `--`
    // proceeds to normal handling.
    if !piped_stdin() {
        return;
    }
    let dir = std::env::temp_dir();
    assert_eq!(
        upload::run_in(&dir, &argv(&["--help"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        upload::run_in(&dir, &argv(&["a.bin"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        upload::run_in(&dir, &argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn b21_paste_requires_terminator_when_piped() {
    // T1153 BROWSE-B21: `paste` is a toolset workflow (same gate as
    // `upload` above).
    if !piped_stdin() {
        return;
    }
    let dir = std::env::temp_dir();
    assert_eq!(
        paste::run_in(&dir, &argv(&["--help"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        paste::run_in(&dir, &argv(&["P123"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        paste::run_in(&dir, &argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn b21_export_needs_no_terminator_when_piped() {
    // T1153 BROWSE-B21 VERIFIED for `export`: it is a classic
    // workflow, so PHP never requires `--` (the
    // `scripts/arcanist.php` re-exec sets no terminator
    // requirement). `--help` without `--` still succeeds.
    if !piped_stdin() {
        return;
    }
    let dir = std::env::temp_dir();
    assert_eq!(
        export::run_in(&dir, &argv(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn b23_upx_help_exits_zero_with_terminator() {
    // T1153 BROWSE-B23: TTY-independent -- `--` satisfies the check
    // everywhere. Help bodies are pinned byte for byte in the
    // module unit tests.
    let dir = std::env::temp_dir();
    assert_eq!(
        upload::run_in(&dir, &argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        paste::run_in(&dir, &argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        export::run_in(&dir, &argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
}
