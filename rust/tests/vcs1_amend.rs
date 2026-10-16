//! `amend` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};

use arc_rs::amend;
use arc_rs::conduit::Client;
use arc_rs::land;

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

/// One canned route: `(method_substring, result_json)`. Routes for the
/// same method serve in order; unexpected calls fail loudly (refused).
fn route(method: &str, canned: &str) -> (String, String) {
    (method.to_string(), canned.to_string())
}

/// Observed (request line, body) pairs from the canned Conduit server.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Serve canned Conduit responses for a multi-call flow, returning the
/// base URI and the observed (request line, body) pairs.
fn serve_routes(routes: Vec<(String, String)>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_worker = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut pending = routes;
        while !pending.is_empty() {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .or_else(|| line.strip_prefix("content-length: "))
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            if !body.is_empty() {
                let _ = stream.read_exact(&mut body);
            }
            let request_line = head.lines().next().unwrap_or("").to_string();
            let position = pending
                .iter()
                .position(|(method, _)| request_line.contains(method))
                .unwrap_or(0);
            let (_, canned) = pending.remove(position);
            seen_worker
                .lock()
                .unwrap()
                .push((request_line, String::from_utf8_lossy(&body).into_owned()));
            let envelope =
                format!("{{\"error_code\":null,\"error_info\":null,\"result\":{canned}}}");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                envelope.len(),
                envelope
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (base, seen)
}

/// Conduit methods called, in order, from observed request lines.
fn methods_of(seen: &Seen) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .map(|(line, _)| {
            line.split_whitespace()
                .nth(1)
                .unwrap_or("")
                .trim_start_matches("/api/")
                .to_string()
        })
        .collect()
}

/// One `differential.query` row.
fn revision_row(id: u64, title: &str, status: i64, author: &str) -> String {
    format!(
        "[{{\"id\":{id},\"title\":\"{title}\",\"status\":{status},\"authorPHID\":\"{author}\"}}]"
    )
}

/// `user.whoami` result for one PHID.
fn whoami(phid: &str) -> String {
    format!("{{\"phid\":\"{phid}\",\"userName\":\"u\",\"realName\":\"U\"}}")
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
    // T1153 R10: `--help` renders the generated workflow help.
    assert!(out.contains("AMEND WORKFLOW"), "unexpected: {out}");
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
        err.contains("not inside a git or Mercurial working copy"),
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
    let home = std::env::temp_dir().join(format!(
        "arc-rs-vcs1-amend-empty-home-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&home).ok();
    std::fs::create_dir_all(&home).unwrap();
    let _guard = with_owned_home(home);
    let err = amend::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("guess"), "unexpected: {err}");
    assert!(err.contains("--message"), "unexpected: {err}");
    assert!(err.contains("--revision"), "unexpected: {err}");
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
    let msg =
        serde_json::to_string("Conduit title\n\nDifferential Revision: http://phab.test/D123")
            .unwrap();
    let (base, seen) = serve_routes(vec![
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-1"),
        ),
        route("user.whoami", &whoami("PHID-USER-1")),
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-1"),
        ),
        route("differential.getcommitmessage", &msg),
    ]);
    let _home = arcrc_for(&base, "conduit-home");

    let mut io = land::Io::canned(&["n"]);
    let out = amend::execute_with_io(
        repo.path(),
        &args(&["--revision", "D123", "--conduit-uri", &base]),
        &mut io,
    )
    .unwrap();

    assert!(!out.trim().is_empty(), "expected a summary, got: {out:?}");
    assert!(
        io.out
            .stdout
            .contains("Amending commit message to reflect revision:"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("**  *  ** D123 Add thing\n"),
        "unexpected: {}",
        io.out.stdout
    );
    // No prompt fired: the canned answer is untouched.
    assert_eq!(io.canned.len(), 1);
    assert_eq!(
        methods_of(&seen),
        vec![
            "differential.query",
            "user.whoami",
            "differential.query",
            "differential.getcommitmessage",
        ]
    );
    let bodies: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(_, body)| body.clone())
        .collect();
    assert!(bodies[3].contains("revision_id"), "missing revision_id");
    assert!(
        bodies.iter().all(|body| body.contains("vcs1-amend-token")),
        "missing token"
    );

    let logged = repo.git(&["log", "-1", "--format=%B"]);
    assert!(logged.contains("Conduit title"), "unexpected: {logged}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn revision_show_prints_fetched_message() {
    let repo = Fixture::init_repo("conduit-show");
    let msg = serde_json::to_string("Fetched title").unwrap();
    let (base, seen) = serve_routes(vec![
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-1"),
        ),
        route("differential.getcommitmessage", &msg),
    ]);
    let _home = arcrc_for(&base, "conduit-show-home");

    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = amend::execute(
        repo.path(),
        &args(&["--show", "--revision", "123", "--conduit-uri", &base]),
    )
    .unwrap();
    assert_eq!(out, "Fetched title\n");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    // `--show` resolves but never confirms.
    assert_eq!(
        methods_of(&seen),
        vec!["differential.query", "differential.getcommitmessage"]
    );
}

#[test]
fn revision_unknown_to_conduit_fails() {
    let repo = Fixture::init_repo("conduit-unknown");
    let (base, _seen) = serve_routes(vec![route("differential.query", "[]")]);
    let _home = arcrc_for(&base, "conduit-unknown-home");

    let err = amend::execute(
        repo.path(),
        &args(&["--revision", "D123", "--conduit-uri", &base]),
    )
    .unwrap_err();
    assert!(
        err.contains("Revision \"D123\" does not exist"),
        "unexpected: {err}"
    );
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

/// Init a repo whose HEAD message is `paragraphs` joined with blank lines.
fn init_repo_with_message(tag: &str, paragraphs: &[&str]) -> Fixture {
    let repo = Fixture::named(tag);
    repo.git(&["init", "-q", "-b", "master", "."]);
    repo.git(&["config", "user.email", "vcs1-test@example.test"]);
    repo.git(&["config", "user.name", "VCS1 Test"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    repo.write("file.txt", "content\n");
    repo.git(&["add", "file.txt"]);
    let mut commit = vec!["commit", "-q"];
    for paragraph in paragraphs {
        commit.push("-m");
        commit.push(paragraph);
    }
    repo.git(&commit);
    repo
}

#[test]
fn guesses_revision_from_head_trailer() {
    let repo = init_repo_with_message("guess", &["work", "Differential Revision: D123"]);
    let msg = serde_json::to_string("Guessed message\n").unwrap();
    let (base, seen) = serve_routes(vec![
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-1"),
        ),
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.getcommitmessage", &msg),
    ]);
    let _home = arcrc_for(&base, "guess-home");

    let mut io = land::Io::canned(&["n"]);
    amend::execute_with_io(repo.path(), &args(&["--conduit-uri", &base]), &mut io).unwrap();
    assert_eq!(io.canned.len(), 1);
    assert_eq!(
        methods_of(&seen),
        vec![
            "differential.query",
            "user.whoami",
            "differential.getcommitmessage",
        ]
    );
    let bodies: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(_, body)| body.clone())
        .collect();
    assert!(
        bodies[0].contains("123"),
        "expected ids query: {}",
        bodies[0]
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "Guessed message"
    );
}

#[test]
fn guess_falls_back_to_head_hash() {
    let repo = Fixture::init_repo("guess-hash");
    let head = repo.git(&["rev-parse", "HEAD"]);
    let (base, seen) = serve_routes(vec![route("differential.query", "[]")]);
    let _home = arcrc_for(&base, "guess-hash-home");

    let err = amend::execute(repo.path(), &args(&["--conduit-uri", &base])).unwrap_err();
    assert!(err.contains("no revisions found"), "unexpected: {err}");
    assert!(err.contains("Use \"--revision <id>\""), "unexpected: {err}");
    let bodies: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(_, body)| body.clone())
        .collect();
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("commitHashes"), "{}", bodies[0]);
    assert!(bodies[0].contains("gtcm"), "{}", bodies[0]);
    assert!(bodies[0].contains(&head), "{}", bodies[0]);
    assert!(bodies[0].contains("gttr"), "{}", bodies[0]);
}

#[test]
fn guess_with_several_revisions_lists_them() {
    let repo = Fixture::init_repo("guess-many");
    let rows = format!(
        "{},{}",
        revision_row(1, "One", 2, "PHID-USER-1")
            .trim_start_matches('[')
            .trim_end_matches(']'),
        revision_row(2, "Two", 2, "PHID-USER-1")
            .trim_start_matches('[')
            .trim_end_matches(']'),
    );
    let (base, _seen) = serve_routes(vec![route("differential.query", &format!("[{rows}]"))]);
    let _home = arcrc_for(&base, "guess-many-home");

    let err = amend::execute(repo.path(), &args(&["--conduit-uri", &base])).unwrap_err();
    assert!(err.contains("MULTIPLE REVISIONS IN WORKING COPY"), "{err}");
    assert!(err.contains("**  *  ** D1 One\n"), "{err}");
    assert!(err.contains("**  *  ** D2 Two\n"), "{err}");
    assert!(err.contains("Use \"--revision <id>\""), "{err}");
}

#[test]
fn unrelated_revision_prompts() {
    let repo = Fixture::init_repo("unrelated");
    let routes = || {
        vec![
            route(
                "differential.query",
                &revision_row(999, "Elsewhere", 2, "PHID-USER-1"),
            ),
            route("user.whoami", &whoami("PHID-USER-1")),
            route(
                "differential.query",
                &revision_row(123, "Add thing", 2, "PHID-USER-1"),
            ),
            route(
                "differential.getcommitmessage",
                &serde_json::to_string("Unrelated message\n").unwrap(),
            ),
        ]
    };
    // Decline: nothing is amended.
    let (base, _seen) = serve_routes(routes());
    let _home = arcrc_for(&base, "unrelated-home");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let mut io = land::Io::canned(&["n"]);
    let err = amend::execute_with_io(
        repo.path(),
        &args(&["--revision", "999", "--conduit-uri", &base]),
        &mut io,
    )
    .unwrap_err();
    assert_eq!(err, land::ABORT);
    assert!(
        io.out.stdout.contains("UNRELATED REVISION"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("Amend working copy using unrelated revision D999?"),
        "unexpected: {}",
        io.out.stdout
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);

    // Accept: the amend proceeds.
    // Release the process-wide HOME lock before re-acquiring it below.
    drop(_home);
    let (base, _seen) = serve_routes(routes());
    let _home = arcrc_for(&base, "unrelated-home-2");
    let mut io = land::Io::canned(&["y"]);
    amend::execute_with_io(
        repo.path(),
        &args(&["--revision", "999", "--conduit-uri", &base]),
        &mut io,
    )
    .unwrap();
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "Unrelated message"
    );
}

#[test]
fn foreign_author_prompts() {
    let repo = Fixture::init_repo("author");
    let user_search =
        r#"{"data":[{"phid":"PHID-USER-2","fields":{"username":"bob","realName":"Bob"}}]}"#;
    let routes = || {
        vec![
            route(
                "differential.query",
                &revision_row(123, "Add thing", 2, "PHID-USER-2"),
            ),
            route("user.whoami", &whoami("PHID-USER-1")),
            route("user.search", user_search),
            route(
                "differential.query",
                &revision_row(123, "Add thing", 2, "PHID-USER-2"),
            ),
            route(
                "differential.getcommitmessage",
                &serde_json::to_string("Author message\n").unwrap(),
            ),
        ]
    };
    // Decline: nothing is amended.
    let (base, _seen) = serve_routes(routes());
    let _home = arcrc_for(&base, "author-home");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let mut io = land::Io::canned(&["n"]);
    let err = amend::execute_with_io(
        repo.path(),
        &args(&["--revision", "123", "--conduit-uri", &base]),
        &mut io,
    )
    .unwrap_err();
    assert_eq!(err, land::ABORT);
    assert!(
        io.out.stdout.contains("NOT REVISION AUTHOR"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("**  *  ** @bob (Bob)\n"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("Amend working copy using revision owned by @bob?"),
        "unexpected: {}",
        io.out.stdout
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);

    // Accept: the amend proceeds.
    // Release the process-wide HOME lock before re-acquiring it below.
    drop(_home);
    let (base, _seen) = serve_routes(routes());
    let _home = arcrc_for(&base, "author-home-2");
    let mut io = land::Io::canned(&["y"]);
    amend::execute_with_io(
        repo.path(),
        &args(&["--revision", "123", "--conduit-uri", &base]),
        &mut io,
    )
    .unwrap();
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "Author message"
    );
}

#[test]
fn unknown_author_skips_the_warning() {
    let repo = Fixture::init_repo("author-unknown");
    let (base, _seen) = serve_routes(vec![
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-2"),
        ),
        route("user.whoami", &whoami("PHID-USER-1")),
        route("user.search", r#"{"data":[]}"#),
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-2"),
        ),
        route(
            "differential.getcommitmessage",
            &serde_json::to_string("No author warning\n").unwrap(),
        ),
    ]);
    let _home = arcrc_for(&base, "author-unknown-home");
    let mut io = land::Io::canned(&["n"]);
    amend::execute_with_io(
        repo.path(),
        &args(&["--revision", "123", "--conduit-uri", &base]),
        &mut io,
    )
    .unwrap();
    assert_eq!(io.canned.len(), 1);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "No author warning"
    );
}

#[test]
fn immutable_history_prompts() {
    let repo = Fixture::init_repo("immutable");
    repo.write(".arcconfig", "{\"history.immutable\": true}\n");
    // Decline: nothing is amended.
    let before = repo.git(&["rev-parse", "HEAD"]);
    let mut io = land::Io::canned(&["n"]);
    let err = amend::execute_with_io(repo.path(), &args(&["-m", "nope"]), &mut io).unwrap_err();
    assert_eq!(err, land::ABORT);
    assert!(
        io.out.stdout.contains("IMMUTABLE WORKING COPY"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("Are you sure you want to mutate history?"),
        "unexpected: {}",
        io.out.stdout
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);

    // Accept: the amend proceeds.
    let mut io = land::Io::canned(&["y"]);
    amend::execute_with_io(repo.path(), &args(&["-m", "mutated"]), &mut io).unwrap();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "mutated");
}

#[test]
fn mutable_config_skips_the_prompt() {
    let repo = Fixture::init_repo("mutable");
    repo.write(".arcconfig", "{\"history.immutable\": false}\n");
    let mut io = land::Io::canned(&["n"]);
    amend::execute_with_io(repo.path(), &args(&["-m", "free"]), &mut io).unwrap();
    assert_eq!(io.canned.len(), 1);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "free");
}

#[test]
fn show_with_message_needs_no_repo() {
    let empty = Fixture::named("show-norepo");
    let out = amend::execute(empty.path(), &args(&["--show", "-m", "preview"])).unwrap();
    assert_eq!(out, "preview\n");
}

#[test]
fn show_with_revision_needs_no_repo() {
    let empty = Fixture::named("show-rev-norepo");
    let msg = serde_json::to_string("Remote title\n").unwrap();
    let (base, seen) = serve_routes(vec![
        route(
            "differential.query",
            &revision_row(123, "Add thing", 2, "PHID-USER-1"),
        ),
        route("differential.getcommitmessage", &msg),
    ]);
    let _home = arcrc_for(&base, "show-rev-norepo-home");
    let out = amend::execute(
        empty.path(),
        &args(&["--show", "--revision", "123", "--conduit-uri", &base]),
    )
    .unwrap();
    assert_eq!(out, "Remote title\n");
    assert_eq!(
        methods_of(&seen),
        vec!["differential.query", "differential.getcommitmessage"]
    );
}

#[test]
fn svn_checkout_reports_amend_unsupported() {
    let empty = Fixture::named("svn-amend");
    std::fs::create_dir_all(empty.path().join(".svn")).unwrap();
    let err = amend::execute(empty.path(), &args(&["-m", "x"])).unwrap_err();
    assert!(
        err.contains("supports amending commits"),
        "unexpected: {err}"
    );
    assert!(err.contains("Mercurial"), "unexpected: {err}");
}

/// Minimal fake-`hg` working copy for the Mercurial amend flow.
struct HgRepo {
    dir: PathBuf,
}

impl HgRepo {
    fn named(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-amend-hg-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".hg")).unwrap();
        Self { dir }
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn install(&self, name: &str) {
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("vcs")
            .join(name);
        let to = self.dir.join(name);
        std::fs::copy(from, &to).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&to).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&to, perms).unwrap();
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join(".fakehg.log"))
            .unwrap_or_default()
            .replace('"', "")
    }
}

impl Drop for HgRepo {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn hg_backend(repo: &HgRepo) -> arc_rs::hg::HgCommand {
    let name = if cfg!(windows) {
        "fakehg.bat"
    } else {
        "fakehg.sh"
    };
    repo.install(name);
    let script = repo.dir.join(name);
    #[cfg(windows)]
    {
        arc_rs::hg::HgCommand::custom("cmd", &["/C", &script.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        arc_rs::hg::HgCommand::custom(&script.to_string_lossy(), &[])
    }
}

#[test]
fn hg_amend_guesses_and_amends() {
    let repo = HgRepo::named("happy");
    let hg = hg_backend(&repo);
    repo.write(".fakehg-status", b"");
    repo.write(".fakehg-msg", b"work\n\nDifferential Revision: D7\n");
    let msg = serde_json::to_string("Hg message\n").unwrap();
    let (base, seen) = serve_routes(vec![
        route(
            "differential.query",
            &revision_row(7, "Hg thing", 2, "PHID-USER-1"),
        ),
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.getcommitmessage", &msg),
    ]);
    let _home = arcrc_for(&base, "hg-happy-home");

    // Mercurial histories are immutable by default: answer that prompt.
    let mut io = land::Io::canned(&["y"]);
    let out =
        amend::execute_with_io_backends(&repo.dir, &args(&["--conduit-uri", &base]), &mut io, &hg)
            .unwrap();
    assert_eq!(out, "amended.\n");
    assert!(
        io.out.stdout.contains("IMMUTABLE WORKING COPY"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("**  *  ** D7 Hg thing\n"),
        "unexpected: {}",
        io.out.stdout
    );
    assert_eq!(
        methods_of(&seen),
        vec![
            "differential.query",
            "user.whoami",
            "differential.getcommitmessage",
        ]
    );
    assert!(repo.log().contains("--logfile"), "{}", repo.log());
}

#[test]
fn hg_amend_needs_modern_mercurial() {
    let repo = HgRepo::named("old");
    let hg = hg_backend(&repo);
    repo.write(".fakehg-noamend", b"");
    let mut io = land::Io::canned(&["n"]);
    let err =
        amend::execute_with_io_backends(&repo.dir, &args(&["-m", "x"]), &mut io, &hg).unwrap_err();
    assert!(err.contains("2.2 or newer"), "unexpected: {err}");
    assert_eq!(io.canned.len(), 1);
}

#[test]
fn hg_amend_refuses_dirty_files() {
    let repo = HgRepo::named("dirty");
    let hg = hg_backend(&repo);
    repo.write(".fakehg-status", b"M dirty.txt\n");
    let mut io = land::Io::canned(&["y"]);
    let err =
        amend::execute_with_io_backends(&repo.dir, &args(&["-m", "x"]), &mut io, &hg).unwrap_err();
    assert!(err.contains("uncommitted changes"), "unexpected: {err}");
    assert!(repo.log().contains("status"), "{}", repo.log());
}

/// Decode one `application/x-www-form-urlencoded` value (`%XX` escapes;
/// `+` stays literal, matching `rawurlencode`).
fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap()
}

/// The `params` JSON object of one observed Conduit request body.
fn request_params(body: &str) -> serde_json::Value {
    let encoded = body
        .split('&')
        .find_map(|pair| pair.strip_prefix("params="))
        .unwrap();
    serde_json::from_str(&url_decode(encoded)).unwrap()
}

#[test]
fn user_lookup_nests_search_constraints() {
    let canned = r#"{"error_code":null,"error_info":null,"result":{"data":[{"phid":"PHID-USER-9","fields":{"username":"bob","realName":"Bob"}}]}}"#.to_string();
    let (base, seen) = serve_once(canned);
    let client = Client::new(&base, "t").unwrap();
    let found = amend::query_user(&client, "PHID-USER-9").unwrap();
    assert_eq!(found, Some(("bob".to_string(), "Bob".to_string())));
    let (line, body) = seen.recv().unwrap();
    assert!(line.contains("user.search"), "unexpected: {line}");
    let params = request_params(&body);
    assert_eq!(
        params.get("constraints"),
        Some(&serde_json::json!({"phids": ["PHID-USER-9"]})),
        "unexpected params: {params}"
    );
    assert!(
        params.get("phids").is_none(),
        "flat constraints rejected by the server: {params}"
    );
}
