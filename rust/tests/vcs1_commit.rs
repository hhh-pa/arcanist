//! `commit` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};

use arc_rs::commit;
use arc_rs::land;

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-commit-{name}-{}-{}",
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

/// One `differential.query` row with a working-copy source path + branch.
fn svn_row(id: u64, title: &str, status: i64, author: &str, source: &str, branch: &str) -> String {
    serde_json::json!([{
        "id": id,
        "title": title,
        "status": status,
        "authorPHID": author,
        "sourcePath": source,
        "branch": branch,
    }])
    .to_string()
}

/// `user.whoami` result for one PHID.
fn whoami(phid: &str) -> String {
    format!("{{\"phid\":\"{phid}\",\"userName\":\"u\",\"realName\":\"U\"}}")
}

/// Serve one request with a canned Conduit response; returns the base URI
/// and the received (request line, body). Loops until one request arrives.
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
        "arc-rs-vcs1-commit-home-{name}-{}-{}",
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
            format!("{uri}/api/"): {"token": "vcs1-commit-token"},
        },
    });
    std::fs::write(dir.join(".arcrc"), serde_json::to_string(&doc).unwrap()).unwrap();
    with_owned_home(dir)
}

#[test]
fn module_name_is_commit() {
    assert_eq!(commit::NAME, "commit");
}

#[test]
fn run_uses_current_dir() {
    let repo = Fixture::init_repo("run-contract");
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(repo.path()).unwrap();
    let code = commit::run(&args(&["-m", "from run"]));
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "from run");
}

#[test]
fn help_succeeds_and_prints_usage() {
    let repo = Fixture::init_repo("help");
    let out = commit::execute(repo.path(), &args(&["--help"])).unwrap();
    assert!(out.contains("usage: arc-rs commit"), "unexpected: {out}");
    assert_eq!(
        commit::run_in(repo.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn outside_repo_fails() {
    let empty = Fixture::named("notrepo");
    let err = commit::execute(empty.path(), &args(&["-m", "x"])).unwrap_err();
    assert!(
        err.contains("not inside a git or Subversion working copy"),
        "unexpected: {err}"
    );
    assert_eq!(
        commit::run_in(empty.path(), &args(&["-m", "x"])),
        ExitCode::FAILURE
    );
}

#[test]
fn requires_a_message_source() {
    let repo = Fixture::init_repo("no-message");
    let err = commit::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("--message"), "unexpected: {err}");
    assert!(err.contains("--revision"), "unexpected: {err}");
}

#[test]
fn rejects_message_and_revision_together() {
    let repo = Fixture::init_repo("both-sources");
    let err = commit::execute(repo.path(), &args(&["-m", "x", "--revision", "123"])).unwrap_err();
    assert!(err.contains("only one of"), "unexpected: {err}");
}

#[test]
fn rejects_empty_message() {
    let repo = Fixture::init_repo("empty-message");
    let err = commit::execute(repo.path(), &args(&["-m", "   "])).unwrap_err();
    assert!(err.contains("empty"), "unexpected: {err}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn commits_staged_changes() {
    let repo = Fixture::init_repo("basic");
    repo.write("feature.txt", "feature\n");
    repo.git(&["add", "feature.txt"]);
    let out = commit::execute(repo.path(), &args(&["-m", "add feature"])).unwrap();
    assert!(!out.trim().is_empty(), "expected a summary, got: {out:?}");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "add feature"
    );
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(repo.git_status(), "");
    // Repeatable -m joins with a blank line.
    repo.write("second.txt", "second\n");
    repo.git(&["add", "second.txt"]);
    commit::execute(repo.path(), &args(&["-m", "subject", "-m", "body"])).unwrap();
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "subject\n\nbody"
    );
}

#[test]
fn all_commits_unstaged_changes() {
    let repo = Fixture::init_repo("all");
    repo.write("file.txt", "modified\n");
    commit::execute(repo.path(), &args(&["--all", "-m", "update"])).unwrap();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "update");
    assert_eq!(repo.git_status(), "");
}

#[test]
fn without_all_unstaged_changes_do_not_commit() {
    let repo = Fixture::init_repo("unstaged");
    repo.write("file.txt", "modified\n");
    let err = commit::execute(repo.path(), &args(&["-m", "update"])).unwrap_err();
    assert!(err.contains("git commit failed"), "unexpected: {err}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn allow_empty_commits_a_clean_tree() {
    let repo = Fixture::init_repo("allow-empty");
    commit::execute(repo.path(), &args(&["--allow-empty", "-m", "empty"])).unwrap();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "empty");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
}

#[test]
fn show_prints_the_message_without_committing() {
    let repo = Fixture::init_repo("show");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = commit::execute(repo.path(), &args(&["--show", "-m", "preview me"])).unwrap();
    assert_eq!(out, "preview me\n");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn revision_fetches_message_from_conduit() {
    let repo = Fixture::init_repo("conduit");
    repo.write("feature.txt", "feature\n");
    repo.git(&["add", "feature.txt"]);
    let canned = serde_json::json!({
        "error_code": null,
        "error_info": null,
        "result": "Conduit title\n\nDifferential Revision: http://phab.test/D123",
    })
    .to_string();
    let (base, rx) = serve_once(canned);
    let _home = arcrc_for(&base, "conduit-home");

    let out = commit::execute(
        repo.path(),
        &args(&["--revision", "D123", "--conduit-uri", &base]),
    )
    .unwrap();
    assert!(!out.trim().is_empty());

    let (request_line, body) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(
        request_line.contains("differential.getcommitmessage"),
        "{request_line}"
    );
    assert!(body.contains("revision_id"), "missing revision_id: {body}");
    assert!(body.contains("vcs1-commit-token"), "missing token: {body}");

    let logged = repo.git(&["log", "-1", "--format=%B"]);
    assert!(logged.contains("Conduit title"), "unexpected: {logged}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
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
    let out = commit::execute(
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

    let err = commit::execute(
        repo.path(),
        &args(&["--revision", "123", "--conduit-uri", &base]),
    )
    .unwrap_err();
    assert!(err.contains("ERR-UNKNOWN"), "unexpected: {err}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        commit::run_in(
            repo.path(),
            &args(&[
                "--show",
                "--revision",
                "1",
                "--conduit-uri",
                "http://127.0.0.1:9/"
            ])
        ),
        ExitCode::FAILURE
    );
}

#[test]
fn bad_flags_fail() {
    let repo = Fixture::init_repo("flags");
    let err = commit::execute(repo.path(), &args(&["--bogus", "-m", "x"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = commit::execute(repo.path(), &args(&["-m", "x", "file.txt"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = commit::execute(repo.path(), &args(&["-m"])).unwrap_err();
    assert!(err.contains("requires"), "unexpected: {err}");
    let err = commit::execute(repo.path(), &args(&["--revision", "abc"])).unwrap_err();
    assert!(err.contains("invalid revision id"), "unexpected: {err}");
    assert_eq!(
        commit::run_in(repo.path(), &args(&["--bogus"])),
        ExitCode::FAILURE
    );
}

/// Minimal fake-`svn` working copy for the Subversion commit flow.
struct SvnRepo {
    dir: PathBuf,
}

impl SvnRepo {
    fn named(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-commit-svn-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".svn")).unwrap();
        Self { dir }
    }

    fn write(&self, name: &str, body: &[u8]) {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
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
        std::fs::read_to_string(self.dir.join(".fakesvn.log"))
            .unwrap_or_default()
            .replace('"', "")
    }

    /// Symlink-free root string, matching the server's `sourcePath`.
    fn canonical_root(&self) -> String {
        let root = std::fs::canonicalize(&self.dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        root.strip_prefix(r"\\?\").unwrap_or(&root).to_string()
    }
}

impl Drop for SvnRepo {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn svn_backend(repo: &SvnRepo) -> arc_rs::svn::SvnCommand {
    let name = if cfg!(windows) {
        "fakesvn.bat"
    } else {
        "fakesvn.sh"
    };
    repo.install(name);
    let script = repo.dir.join(name);
    #[cfg(windows)]
    {
        arc_rs::svn::SvnCommand::custom("cmd", &["/C", &script.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        arc_rs::svn::SvnCommand::custom(&script.to_string_lossy(), &[])
    }
}

/// Canned `svn info` reporting `branch` below the repo root.
fn canned_info(branch: &str) -> String {
    format!(
        "Path: .\nURL: https://svn.example.test/repo{branch}\nRepository Root: https://svn.example.test/repo\nRevision: 42\n"
    )
}

/// Canned `svn --xml status` with one modified entry per path.
fn canned_status(paths: &[&str]) -> String {
    let mut xml =
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<status>\n<target path=\".\">\n".to_string();
    for path in paths {
        xml.push_str(&format!(
            "<entry path=\"{path}\">\n<wc-status props=\"none\" item=\"modified\" revision=\"42\">\n</wc-status>\n</entry>\n"
        ));
    }
    xml.push_str("</target>\n</status>\n");
    xml
}

/// Baseline checkout: `keep.txt` on disk + modified, on `/trunk`.
fn trunk_repo(tag: &str) -> (SvnRepo, arc_rs::svn::SvnCommand) {
    let repo = SvnRepo::named(tag);
    let svn = svn_backend(&repo);
    repo.write(".fakesvn-info", canned_info("/trunk").as_bytes());
    repo.write(".fakesvn-status", canned_status(&["keep.txt"]).as_bytes());
    repo.write("keep.txt", b"new\n");
    (repo, svn)
}

#[test]
fn svn_rejects_git_only_options() {
    let (repo, svn) = trunk_repo("gitonly");
    let mut io = land::Io::canned(&["n"]);
    for argv in [
        args(&["-m", "x"]),
        args(&["--all", "--revision", "5"]),
        args(&["--allow-empty", "--revision", "5"]),
    ] {
        let err = commit::execute_with_io_backends(&repo.dir, &argv, &mut io, &svn).unwrap_err();
        assert!(err.contains("only supported for git"), "unexpected: {err}");
    }
    assert_eq!(io.canned.len(), 1);
}

#[test]
fn svn_guess_none_and_multiple_fail() {
    let (repo, svn) = trunk_repo("guess-fail");
    // None found.
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", "[]"),
    ]);
    let _home = arcrc_for(&base, "svn-guess-none-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert!(err.contains("Unable to identify the revision"), "{err}");

    // Several found: the list names every candidate.
    let root = repo.canonical_root();
    let rows = format!(
        "{},{}",
        svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk")
            .trim_start_matches('[')
            .trim_end_matches(']'),
        svn_row(6, "Six", 2, "PHID-USER-1", &root, "/trunk")
            .trim_start_matches('[')
            .trim_end_matches(']'),
    );
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", &format!("[{rows}]")),
    ]);
    // Release the process-wide HOME lock before re-acquiring it below.
    drop(_home);
    let _home = arcrc_for(&base, "svn-guess-many-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert!(err.contains("More than one revision exists"), "{err}");
    assert!(err.contains("     - D5: Five\n"), "{err}");
    assert!(err.contains("     - D6: Six\n"), "{err}");
}

#[test]
fn svn_explicit_unknown_revision_fails() {
    let (repo, svn) = trunk_repo("unknown");
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", "[]"),
    ]);
    let _home = arcrc_for(&base, "svn-unknown-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--revision", "9", "--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert!(err.contains("Revision 'D9' does not exist."), "{err}");
}

#[test]
fn svn_show_prints_message_without_committing() {
    let (repo, svn) = trunk_repo("show");
    let msg = serde_json::to_string("Svn message\n").unwrap();
    let root = repo.canonical_root();
    let (base, seen) = serve_routes(vec![
        route(
            "differential.query",
            &svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk"),
        ),
        route("differential.getcommitmessage", &msg),
    ]);
    let _home = arcrc_for(&base, "svn-show-home");
    let mut io = land::Io::canned(&["n"]);
    let out = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--show", "--revision", "5", "--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap();
    assert_eq!(out, "Svn message\n");
    assert_eq!(io.canned.len(), 1);
    // Explicit `--show` skips whoami, sanity checks, and the commit.
    assert_eq!(
        methods_of(&seen),
        vec!["differential.query", "differential.getcommitmessage"]
    );
    assert!(!repo.log().contains("commit "), "{}", repo.log());
}

#[test]
fn svn_happy_path_commits_and_finalizes() {
    let (repo, svn) = trunk_repo("happy");
    let root = repo.canonical_root();
    let msg = serde_json::to_string("Commit message\n").unwrap();
    let row = svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk");
    let (base, seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", &row),
        route("differential.getcommitmessage", &msg),
        route("differential.getcommitpaths", r#"["keep.txt"]"#),
        route("differential.query", &row),
        route("differential.close", "true"),
    ]);
    let _home = arcrc_for(&base, "svn-happy-home");
    let mut io = land::Io::canned(&["n"]);
    let out = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap();
    // Guessed, accepted, owned, same branch: no prompt fires.
    assert_eq!(io.canned.len(), 1);
    assert!(
        io.out.stdout.contains("Committing 'D5: Five'...\n"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(out.contains("Done, closed revision."), "unexpected: {out}");
    assert!(out.contains("You may now push"), "unexpected: {out}");
    assert_eq!(
        methods_of(&seen),
        vec![
            "user.whoami",
            "differential.query",
            "differential.getcommitmessage",
            "differential.getcommitpaths",
            "differential.query",
            "differential.close",
        ]
    );
    let log = repo.log();
    assert!(log.contains("commit keep.txt --encoding utf-8 -F"), "{log}");
    // Sanity checks ran against `svn info`: no warning, one info call.
    assert!(log.contains("info"), "{log}");
}

#[test]
fn svn_sanity_checks_prompt() {
    let (repo, svn) = trunk_repo("sanity");
    let root = repo.canonical_root();
    let row = svn_row(5, "Five", 1, "PHID-USER-2", &root, "/branches/other");
    let msg = serde_json::to_string("Risky message\n").unwrap();
    let routes = || {
        vec![
            route("user.whoami", &whoami("PHID-USER-1")),
            route("differential.query", &row),
            route("differential.getcommitmessage", &msg),
            route("differential.getcommitpaths", r#"["keep.txt"]"#),
            route("differential.query", &row),
        ]
    };
    // Decline the first check: nothing commits.
    let (base, _seen) = serve_routes(routes());
    let _home = arcrc_for(&base, "svn-sanity-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--revision", "5", "--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert_eq!(err, land::ABORT);
    assert!(
        io.out
            .stdout
            .contains("has not been accepted. Commit this revision anyway?"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(!repo.log().contains("commit "), "{}", repo.log());

    // Accept all three: the commit runs, and finalize leaves the
    // still-open revision alone.
    let (base, seen) = serve_routes(routes());
    // Release the process-wide HOME lock before re-acquiring it below.
    drop(_home);
    let _home = arcrc_for(&base, "svn-sanity-home-2");
    let mut io = land::Io::canned(&["y", "y", "y"]);
    let out = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--revision", "5", "--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap();
    assert!(
        io.out
            .stdout
            .contains("You are not the author of 'D5: Five'"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("was generated from '/branches/other'"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(out.contains("OPEN REVISION"), "unexpected: {out}");
    assert_eq!(
        methods_of(&seen),
        vec![
            "user.whoami",
            "differential.query",
            "differential.getcommitmessage",
            "differential.getcommitpaths",
            "differential.query",
        ]
    );
    assert!(repo.log().contains("commit keep.txt"), "{}", repo.log());
}

#[test]
fn svn_modified_not_included_warns() {
    let (repo, svn) = trunk_repo("skipped");
    repo.write(
        ".fakesvn-status",
        canned_status(&["keep.txt", "extra.txt"]).as_bytes(),
    );
    repo.write("extra.txt", b"extra\n");
    let root = repo.canonical_root();
    let row = svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk");
    let msg = serde_json::to_string("Partial message\n").unwrap();
    let routes = || {
        vec![
            route("user.whoami", &whoami("PHID-USER-1")),
            route("differential.query", &row),
            route("differential.getcommitmessage", &msg),
            route("differential.getcommitpaths", r#"["keep.txt"]"#),
            route("differential.query", &row),
            route("differential.close", "true"),
        ]
    };
    // Decline: nothing commits.
    let (base, _seen) = serve_routes(routes());
    let _home = arcrc_for(&base, "svn-skipped-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert_eq!(err, land::ABORT);
    assert!(
        io.out
            .stdout
            .contains("1 locally modified path(s) are not included"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("    extra.txt\n"),
        "unexpected: {}",
        io.out.stdout
    );

    // Accept: only the revision path commits.
    let (base, _seen) = serve_routes(routes());
    // Release the process-wide HOME lock before re-acquiring it below.
    drop(_home);
    let _home = arcrc_for(&base, "svn-skipped-home-2");
    let mut io = land::Io::canned(&["y"]);
    commit::execute_with_io_backends(&repo.dir, &args(&["--conduit-uri", &base]), &mut io, &svn)
        .unwrap();
    let log = repo.log();
    assert!(log.contains("commit keep.txt"), "{log}");
    assert!(!log.contains("extra.txt"), "{log}");
}

#[test]
fn svn_missing_paths_warn_and_drop_out() {
    let (repo, svn) = trunk_repo("missing");
    let root = repo.canonical_root();
    let row = svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk");
    let msg = serde_json::to_string("Missing message\n").unwrap();
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", &row),
        route("differential.getcommitmessage", &msg),
        route(
            "differential.getcommitpaths",
            r#"["keep.txt","vanished.txt"]"#,
        ),
        route("differential.query", &row),
        route("differential.close", "true"),
    ]);
    let _home = arcrc_for(&base, "svn-missing-home");
    let mut io = land::Io::canned(&["y"]);
    commit::execute_with_io_backends(&repo.dir, &args(&["--conduit-uri", &base]), &mut io, &svn)
        .unwrap();
    assert!(
        io.out.stdout.contains("1 path(s) that do not exist"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("    vanished.txt\n"),
        "unexpected: {}",
        io.out.stdout
    );
    let log = repo.log();
    assert!(log.contains("commit keep.txt"), "{log}");
    assert!(!log.contains("vanished"), "{log}");
}

#[test]
fn svn_descendant_of_committed_dir_throws() {
    let (repo, svn) = trunk_repo("descendant");
    repo.write(
        ".fakesvn-status",
        canned_status(&["sub/inner.txt"]).as_bytes(),
    );
    repo.write("sub/committed.txt", b"in\n");
    repo.write("sub/inner.txt", b"out\n");
    let root = repo.canonical_root();
    let row = svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk");
    let msg = serde_json::to_string("Descendant message\n").unwrap();
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", &row),
        route("differential.getcommitmessage", &msg),
        route("differential.getcommitpaths", r#"["sub/committed.txt"]"#),
    ]);
    let _home = arcrc_for(&base, "svn-descendant-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert!(err.contains("NOT included in the commit"), "{err}");
    assert!(err.contains("'sub/inner.txt'"), "{err}");
    assert!(!repo.log().contains("commit "), "{}", repo.log());
}

#[test]
fn svn_nothing_left_to_commit_fails() {
    let (repo, svn) = trunk_repo("nothing-left");
    let root = repo.canonical_root();
    let row = svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk");
    let msg = serde_json::to_string("Empty message\n").unwrap();
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", &row),
        route("differential.getcommitmessage", &msg),
        route("differential.getcommitpaths", r#"["vanished.txt"]"#),
    ]);
    let _home = arcrc_for(&base, "svn-nothing-home");
    // Two prompts fire in order: keep.txt is modified-but-excluded, then
    // vanished.txt does not exist; only then is the list empty.
    let mut io = land::Io::canned(&["y", "y"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert!(
        err.contains("There is nothing left to commit"),
        "unexpected: {err}"
    );
    assert!(!repo.log().contains("commit "), "{}", repo.log());
}

#[test]
fn svn_commit_failure_reports() {
    let (repo, svn) = trunk_repo("commit-fail");
    repo.write(".fakesvn-fail-commit", b"");
    let root = repo.canonical_root();
    let row = svn_row(5, "Five", 2, "PHID-USER-1", &root, "/trunk");
    let msg = serde_json::to_string("Failing message\n").unwrap();
    let (base, _seen) = serve_routes(vec![
        route("user.whoami", &whoami("PHID-USER-1")),
        route("differential.query", &row),
        route("differential.getcommitmessage", &msg),
        route("differential.getcommitpaths", r#"["keep.txt"]"#),
    ]);
    let _home = arcrc_for(&base, "svn-commit-fail-home");
    let mut io = land::Io::canned(&["n"]);
    let err = commit::execute_with_io_backends(
        &repo.dir,
        &args(&["--conduit-uri", &base]),
        &mut io,
        &svn,
    )
    .unwrap_err();
    assert!(
        err.contains("Executing 'svn commit' failed!"),
        "unexpected: {err}"
    );
}

#[test]
fn hg_checkout_reports_commit_unsupported() {
    let empty = Fixture::named("hg-commit");
    std::fs::create_dir_all(empty.path().join(".hg")).unwrap();
    let err = commit::execute(empty.path(), &args(&["-m", "x"])).unwrap_err();
    assert!(
        err.contains("not inside a git or Subversion working copy"),
        "unexpected: {err}"
    );
}
