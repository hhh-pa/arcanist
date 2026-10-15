//! `branches` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::branches;
use arc_rs::conduit::Client;

/// Scripted mock conduit server: serves each canned body in order,
/// capturing (request line, body) per request.
fn serve_script(
    script: Vec<&'static str>,
) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for canned in script {
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
        }
    });
    (base, rx)
}

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-branches-{name}-{}-{}",
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

    fn init_bare(name: &str) -> Self {
        let repo = Self::named(name);
        repo.git(&["init", "-q", "--bare", "."]);
        repo
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn commit_file(&self, name: &str, body: &str, message: &str) {
        self.write(name, body);
        self.git(&["add", name]);
        self.git(&["commit", "-qm", message]);
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

#[test]
fn module_name_is_branches() {
    assert_eq!(branches::NAME, "branches");
}

#[test]
fn run_uses_current_dir() {
    let repo = Fixture::init_repo("run-contract");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(repo.path()).unwrap();
    let code = branches::run(&args(&[]));
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
}

#[test]
fn help_succeeds_and_prints_usage() {
    let repo = Fixture::init_repo("help");
    let out = branches::execute(repo.path(), &args(&["--help"])).unwrap();
    assert!(out.contains("usage: arc branches"), "unexpected: {out}");
    assert_eq!(
        branches::run_in(repo.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn lists_branches_with_current_marker() {
    let repo = Fixture::init_repo("list");
    repo.git(&["branch", "feature"]);
    let branches = branches::list(repo.path(), false).unwrap();
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0].name, "master");
    assert!(branches[0].current);
    assert_eq!(branches[1].name, "feature");
    assert!(!branches[1].current);

    // Fixed width: message truncation depends on the terminal.
    let out = branches::execute_with_client(repo.path(), &args(&[]), Some(150), None).unwrap();
    // Both markers point at the same commit, so one tree view holds two
    // marker items: the plain marker first, the active one second.
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "unexpected: {out}");
    assert!(lines[0].starts_with("• feature"), "unexpected: {out}");
    assert!(lines[1].starts_with("└  *  master"), "unexpected: {out}");
    let short = repo.git(&["rev-parse", "--short", "HEAD"]);
    assert!(out.contains(&short), "missing hash: {out}");
    assert!(out.contains("initial commit"), "missing subject: {out}");
    assert_eq!(branches::run_in(repo.path(), &args(&[])), ExitCode::SUCCESS);
}

#[test]
fn detached_head_has_no_current_branch() {
    let repo = Fixture::init_repo("detached");
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);
    let branches = branches::list(repo.path(), false).unwrap();
    assert_eq!(branches.len(), 1);
    assert!(!branches[0].current);
    let out = branches::execute_with_client(repo.path(), &args(&[]), Some(150), None).unwrap();
    assert!(!out.contains(" *  "), "unexpected: {out}");
    assert!(out.contains("• master"), "unexpected: {out}");
}

#[test]
fn empty_repo_matches_php_partition_error() {
    let repo = Fixture::named("empty");
    repo.git(&["init", "-q", "-b", "master", "."]);
    assert_eq!(branches::list(repo.path(), false).unwrap(), Vec::new());
    // No markers means no heads; PHP throws out of the graph query.
    let err = branches::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("Need head hashes or exact hashes!"),
        "unexpected: {err}"
    );
    let out = branches::execute(repo.path(), &args(&["--json"])).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::json!([])
    );
}

#[test]
fn outside_repo_fails() {
    let empty = Fixture::named("notrepo");
    let err = branches::execute(empty.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("not inside a git repository"),
        "unexpected: {err}"
    );
    assert_eq!(
        branches::run_in(empty.path(), &args(&[])),
        ExitCode::FAILURE
    );
}

#[test]
fn json_lists_branch_objects() {
    let repo = Fixture::init_repo("json");
    let out = branches::execute(repo.path(), &args(&["--json"])).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let entries = doc.as_array().unwrap();
    assert_eq!(entries.len(), 1, "{doc}");
    assert_eq!(entries[0]["name"], serde_json::json!("master"));
    assert_eq!(entries[0]["current"], serde_json::json!(true));
    assert_eq!(entries[0]["subject"], serde_json::json!("initial commit"));
    assert_eq!(entries[0]["upstream"], serde_json::json!(null));
    assert_eq!(entries[0]["remote"], serde_json::json!(false));
    assert!(!entries[0]["hash"].as_str().unwrap().is_empty());
}

#[test]
fn upstream_tracking_is_shown() {
    let repo = Fixture::init_repo("tracking");
    repo.git(&["branch", "frozen"]);
    repo.commit_file("extra.txt", "extra\n", "second commit");
    repo.git(&["branch", "--set-upstream-to=frozen"]);

    let branches = branches::list(repo.path(), false).unwrap();
    let master = branches.iter().find(|b| b.name == "master").unwrap();
    assert_eq!(master.upstream.as_deref(), Some("frozen"));
    assert_eq!(master.track, "[ahead 1]");

    // The tree grid shows no upstream tracking (matching `arc`); the
    // tracking data stays available through `list()`.
    let out = branches::execute_with_client(repo.path(), &args(&[]), Some(150), None).unwrap();
    assert!(out.contains("• frozen"), "unexpected: {out}");
    assert!(out.contains("└  *  master"), "unexpected: {out}");
    assert!(out.contains("second commit"), "unexpected: {out}");
    assert!(!out.contains("[ahead 1]"), "unexpected: {out}");
}

#[test]
fn all_includes_remote_tracking_branches() {
    let repo = Fixture::init_repo("all");
    let bare = Fixture::init_bare("all-bare");
    repo.git(&["remote", "add", "origin", &bare.dir.to_string_lossy()]);
    repo.git(&["push", "-q", "origin", "master"]);

    let local_only =
        branches::execute_with_client(repo.path(), &args(&[]), Some(150), None).unwrap();
    assert_eq!(
        local_only
            .lines()
            .filter(|line| line.contains("master"))
            .count(),
        1,
        "unexpected: {local_only}"
    );

    // Remote-tracking markers render under their branch name, so `--all`
    // adds a second `master` item for `origin/master`.
    let all =
        branches::execute_with_client(repo.path(), &args(&["--all"]), Some(150), None).unwrap();
    assert_eq!(
        all.lines().filter(|line| line.contains("master")).count(),
        2,
        "unexpected: {all}"
    );

    let branches = branches::list(repo.path(), true).unwrap();
    let remote = branches.iter().find(|b| b.name == "origin/master").unwrap();
    assert!(remote.remote);
    assert!(!remote.current);
}

#[test]
fn bad_flags_fail() {
    let repo = Fixture::init_repo("flags");
    let err = branches::execute(repo.path(), &args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = branches::execute(repo.path(), &args(&["master"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert_eq!(
        branches::run_in(repo.path(), &args(&["--bogus"])),
        ExitCode::FAILURE
    );
}

#[test]
fn svn_working_copy_reports_no_branch_support() {
    let empty = Fixture::named("svn-nosupport");
    std::fs::create_dir_all(empty.path().join(".svn")).unwrap();
    let err = branches::execute(empty.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("does not support branches"),
        "unexpected: {err}"
    );
    assert!(err.contains("\"svn\""), "unexpected: {err}");
}

#[test]
fn mock_server_answers_direct_calls() {
    let (base, seen) = serve_script(vec![r#"{"result": [], "error_code": null}"#]);
    let client = Client::new(&base, "test-token").unwrap();
    let result = client
        .call(
            "differential.query",
            &serde_json::json!({"commitHashes": []}),
        )
        .unwrap();
    assert_eq!(result, serde_json::json!([]));
    let (request_line, _) = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(
        request_line.contains("differential.query"),
        "{request_line}"
    );
}

#[test]
fn conduit_revisions_annotate_the_grid() {
    let repo = Fixture::init_repo("annotated");
    repo.git(&["checkout", "-qb", "feature"]);
    repo.commit_file("extra.txt", "extra\n", "second commit");
    let head = repo.git(&["rev-parse", "HEAD"]);

    let row = format!(
        r#"{{"id": 1, "phid": "PHID-DREV-1", "status": "0", "statusName": "Needs Review", "title": "Add the thing", "uri": "/D1", "authorPHID": "PHID-USER-1", "dateCreated": 1, "hashes": [["gtcm", "{head}"]]}}"#
    );
    let body: &'static str =
        Box::leak(format!(r#"{{"result": [{row}], "error_code": null}}"#).into_boxed_str());
    let (base, seen) = serve_script(vec![body]);
    let client = Client::new(&base, "test-token").unwrap();

    // Wide terminal: full status display names.
    let out =
        branches::execute_with_client(repo.path(), &args(&[]), Some(150), Some(&client)).unwrap();
    assert!(out.contains("D1"), "unexpected: {out}");
    assert!(out.contains("Needs Review"), "unexpected: {out}");
    // The waypoint color splits the commits into two sets and two rows.
    assert!(out.contains("• master"), "unexpected: {out}");
    assert!(out.contains("┗━━"), "unexpected: {out}");
    // Merged revisions replace the commit summary in messages.
    assert!(out.contains("Add the thing"), "unexpected: {out}");
    assert!(!out.contains("second commit"), "unexpected: {out}");

    let (request_line, params) = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(
        request_line.starts_with("POST /api/differential.query "),
        "{request_line}"
    );
    assert!(params.contains("commitHashes"), "{params}");
    assert!(params.contains(&head), "{params}");

    // Without a client the same grid renders with blank annotation.
    let plain = branches::execute_with_client(repo.path(), &args(&[]), Some(150), None).unwrap();
    assert!(!plain.contains("D1"), "unexpected: {plain}");
    assert!(plain.contains("second commit"), "unexpected: {plain}");
}
