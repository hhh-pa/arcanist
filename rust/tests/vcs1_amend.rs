//! `amend` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::amend;

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-amend-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_status(&self) -> String {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Fresh repo on `master` with one commit; never touches real remotes.
    fn init_repo(name: &str) -> Self {
        let repo = Self::named(name);
        repo.git(&["init", "-q", "-b", "master", "."]);
        repo.git(&["config", "user.email", "vcs1-test@example.test"]);
        repo.git(&["config", "user.name", "VCS1 Test"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.write("file.txt", "base\n");
        repo.git(&["add", "file.txt"]);
        repo.git(&["commit", "-qm", "initial commit"]);
        repo
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

/// Serve one request with a canned Conduit response; returns the base URI
/// and the received (request line, body).
fn serve_once(canned: String) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
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
                line.strip_prefix("Content-Length: ")
                    .or_else(|| line.strip_prefix("content-length: "))
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).unwrap();
        let request_line = head.lines().next().unwrap_or("").to_string();
        tx.send((request_line, String::from_utf8(body).unwrap()))
            .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            canned.len(),
            canned
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (base, rx)
}

mod common;

use common::{HomeGuard, with_owned_home};

/// Write an `.arcrc` for `base` (a `serve_once` URI) under a fresh home
/// dir, and install the home override. Returns the override guard.
fn arcrc_for(base: &str, name: &str) -> HomeGuard {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-vcs1-amend-home-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let uri = base.trim_end_matches('/');
    let doc = serde_json::json!({
        "hosts": {
            format!("{uri}/api/"): {"token": "vcs1-amend-token"},
        },
    });
    std::fs::write(dir.join(".arcrc"), serde_json::to_string(&doc).unwrap()).unwrap();
    with_owned_home(dir)
}

#[test]
fn module_name_is_amend() {
    assert_eq!(amend::NAME, "amend");
}

#[test]
fn run_uses_current_dir() {
    let repo = Fixture::init_repo("run-contract");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(repo.path()).unwrap();
    let code = amend::run(&args(&["-m", "from run"]));
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "from run");
}

#[test]
fn help_succeeds_and_prints_usage() {
    let repo = Fixture::init_repo("help");
    let out = amend::execute(repo.path(), &args(&["--help"])).unwrap();
    assert!(out.contains("usage: arc-rs amend"), "unexpected: {out}");
    assert_eq!(
        amend::run_in(repo.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn outside_repo_fails() {
    let empty = Fixture::named("notrepo");
    let err = amend::execute(empty.path(), &args(&["-m", "x"])).unwrap_err();
    assert!(
        err.contains("not inside a git repository"),
        "unexpected: {err}"
    );
    assert_eq!(
        amend::run_in(empty.path(), &args(&["-m", "x"])),
        ExitCode::FAILURE
    );
}

#[test]
fn requires_a_message_source() {
    let repo = Fixture::init_repo("no-message");
    let err = amend::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("--message"), "unexpected: {err}");
    assert!(err.contains("--revision"), "unexpected: {err}");
    assert!(err.contains("not yet ported"), "unexpected: {err}");
}

#[test]
fn rejects_message_and_revision_together() {
    let repo = Fixture::init_repo("both-sources");
    let err = amend::execute(repo.path(), &args(&["-m", "x", "--revision", "123"])).unwrap_err();
    assert!(err.contains("only one of"), "unexpected: {err}");
}

#[test]
fn rejects_empty_message() {
    let repo = Fixture::init_repo("empty-message");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let err = amend::execute(repo.path(), &args(&["-m", "   "])).unwrap_err();
    assert!(err.contains("empty"), "unexpected: {err}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
}

#[test]
fn amends_the_head_message() {
    let repo = Fixture::init_repo("basic");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = amend::execute(repo.path(), &args(&["-m", "amended subject"])).unwrap();
    assert!(!out.trim().is_empty(), "expected a summary, got: {out:?}");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "amended subject"
    );
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
    assert_ne!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(repo.git_status(), "");
}

#[test]
fn staged_changes_are_refused() {
    let repo = Fixture::init_repo("staged");
    let before = repo.git(&["rev-parse", "HEAD"]);
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    let err = amend::execute(repo.path(), &args(&["-m", "nope"])).unwrap_err();
    assert!(err.contains("staged"), "unexpected: {err}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(
        amend::run_in(repo.path(), &args(&["-m", "nope"])),
        ExitCode::FAILURE
    );
}

#[test]
fn unstaged_and_untracked_changes_are_allowed() {
    let repo = Fixture::init_repo("unstaged");
    repo.write("file.txt", "modified\n");
    repo.write("untracked.txt", "untracked\n");
    amend::execute(repo.path(), &args(&["-m", "amended anyway"])).unwrap();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "amended anyway");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("file.txt")).unwrap(),
        "modified\n"
    );
    assert!(repo.path().join("untracked.txt").exists());
}

#[test]
fn show_prints_the_message_without_amending() {
    let repo = Fixture::init_repo("show");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = amend::execute(repo.path(), &args(&["--show", "-m", "preview me"])).unwrap();
    assert_eq!(out, "preview me\n");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
}

#[test]
fn amend_without_any_commit_fails() {
    let repo = Fixture::named("empty");
    repo.git(&["init", "-q", "-b", "master", "."]);
    repo.git(&["config", "user.email", "vcs1-test@example.test"]);
    repo.git(&["config", "user.name", "VCS1 Test"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    let err = amend::execute(repo.path(), &args(&["-m", "nothing to amend"])).unwrap_err();
    assert!(err.contains("amend failed"), "unexpected: {err}");
}

#[test]
fn revision_fetches_message_from_conduit() {
    let repo = Fixture::init_repo("conduit");
    let canned = serde_json::json!({
        "error_code": null,
        "error_info": null,
        "result": "Conduit title\n\nDifferential Revision: http://phab.test/D123",
    })
    .to_string();
    let (base, rx) = serve_once(canned);
    let _home = arcrc_for(&base, "conduit-home");

    amend::execute(
        repo.path(),
        &args(&["--revision", "D123", "--conduit-uri", &base]),
    )
    .unwrap();

    let (request_line, body) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(
        request_line.contains("differential.getcommitmessage"),
        "{request_line}"
    );
    assert!(body.contains("revision_id"), "missing revision_id: {body}");
    assert!(body.contains("vcs1-amend-token"), "missing token: {body}");

    let logged = repo.git(&["log", "-1", "--format=%B"]);
    assert!(logged.contains("Conduit title"), "unexpected: {logged}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn revision_show_prints_fetched_message() {
    let repo = Fixture::init_repo("conduit-show");
    let canned = serde_json::json!({
        "error_code": null,
        "error_info": null,
        "result": "Fetched title",
    })
    .to_string();
    let (base, _rx) = serve_once(canned);
    let _home = arcrc_for(&base, "conduit-show-home");

    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = amend::execute(
        repo.path(),
        &args(&["--show", "--revision", "123", "--conduit-uri", &base]),
    )
    .unwrap();
    assert_eq!(out, "Fetched title\n");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
}

#[test]
fn revision_conduit_error_fails() {
    let repo = Fixture::init_repo("conduit-error");
    let canned = serde_json::json!({
        "error_code": "ERR-UNKNOWN",
        "error_info": "nope",
        "result": null,
    })
    .to_string();
    let (base, _rx) = serve_once(canned);
    let _home = arcrc_for(&base, "conduit-error-home");

    let before = repo.git(&["rev-parse", "HEAD"]);
    let err = amend::execute(
        repo.path(),
        &args(&["--revision", "123", "--conduit-uri", &base]),
    )
    .unwrap_err();
    assert!(err.contains("ERR-UNKNOWN"), "unexpected: {err}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
}

#[test]
fn bad_flags_fail() {
    let repo = Fixture::init_repo("flags");
    let err = amend::execute(repo.path(), &args(&["--bogus", "-m", "x"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = amend::execute(repo.path(), &args(&["-m", "x", "file.txt"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = amend::execute(repo.path(), &args(&["-m"])).unwrap_err();
    assert!(err.contains("requires"), "unexpected: {err}");
    let err = amend::execute(repo.path(), &args(&["--revision", "abc"])).unwrap_err();
    assert!(err.contains("invalid revision id"), "unexpected: {err}");
    assert_eq!(
        amend::run_in(repo.path(), &args(&["--bogus"])),
        ExitCode::FAILURE
    );
}
