//! `bookmarks` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::bookmarks;

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-bookmarks-{name}-{}-{}",
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

    /// Fresh git repo with one commit; never touches real remotes.
    fn init_repo(name: &str) -> Self {
        let repo = Self::named(name);
        repo.git(&["init", "-q", "-b", "master", "."]);
        repo.git(&["config", "user.email", "vcs1-test@example.test"]);
        repo.git(&["config", "user.name", "VCS1 Test"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        std::fs::write(repo.dir.join("file.txt"), "base\n").unwrap();
        repo.git(&["add", "file.txt"]);
        repo.git(&["commit", "-qm", "initial commit"]);
        repo
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
fn module_name_is_bookmarks() {
    assert_eq!(bookmarks::NAME, "bookmarks");
}

#[test]
fn run_uses_current_dir() {
    let dir = Fixture::named("run-contract");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let code = bookmarks::run(&args(&["--help"]));
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
}

#[test]
fn help_succeeds_and_prints_usage() {
    let empty = Fixture::named("help");
    let out = bookmarks::execute(empty.path(), &args(&["--help"])).unwrap();
    assert!(out.contains("usage: arc-rs bookmarks"), "unexpected: {out}");
    assert_eq!(
        bookmarks::run_in(empty.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn git_working_copy_reports_no_bookmark_support() {
    let repo = Fixture::init_repo("git-nosupport");
    // Matches the `arc bookmarks` error under git.
    let err = bookmarks::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("does not support bookmarks"),
        "unexpected: {err}"
    );
    assert!(err.contains("\"git\""), "unexpected: {err}");
    assert_eq!(
        bookmarks::run_in(repo.path(), &args(&[])),
        ExitCode::FAILURE
    );
    let err = bookmarks::execute(repo.path(), &args(&["--json"])).unwrap_err();
    assert!(
        err.contains("does not support bookmarks"),
        "unexpected: {err}"
    );
}

#[test]
fn list_under_git_reports_no_bookmark_support() {
    // Direct `list` coverage on the git path (no `hg` spawned): deterministic
    // on every platform, with or without Mercurial installed.
    let repo = Fixture::init_repo("git-list");
    let err = bookmarks::list(repo.path()).unwrap_err();
    assert!(
        err.contains("does not support bookmarks"),
        "unexpected: {err}"
    );
}

#[test]
fn outside_any_working_copy_fails() {
    let empty = Fixture::named("notrepo");
    assert_eq!(bookmarks::find_hg_root(empty.path()), None);
    let err = bookmarks::execute(empty.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("not inside a working copy"),
        "unexpected: {err}"
    );
    assert_eq!(
        bookmarks::run_in(empty.path(), &args(&[])),
        ExitCode::FAILURE
    );
}

#[test]
fn missing_hg_binary_fails() {
    // `.hg` detection works everywhere, but whether plain `hg bookmarks`
    // fails in a bare `.hg` dir depends on the installed Mercurial: some
    // versions exit 0 with an empty list (which is also what `arc` would
    // report). The deterministic contract, forced through the binary seam:
    // when `hg` cannot run, this must fail, never lie.
    let broken = Fixture::named("broken-hg");
    std::fs::create_dir_all(broken.path().join(".hg")).unwrap();
    assert_eq!(
        bookmarks::find_hg_root(broken.path()),
        Some(broken.path().to_path_buf())
    );
    let err =
        bookmarks::execute_with_binary(broken.path(), &args(&[]), "arc-rs-definitely-missing-hg")
            .unwrap_err();
    assert!(err.contains("failed to run"), "unexpected: {err}");
}

#[test]
fn hg_root_is_found_from_a_subdirectory() {
    let top = Fixture::named("nested-hg");
    let nested = top.path().join("a").join("b");
    std::fs::create_dir_all(nested.join(".hg")).unwrap();
    assert_eq!(bookmarks::find_hg_root(&nested), Some(nested.clone()));
    assert_eq!(bookmarks::find_hg_root(top.path()), None);
}

#[test]
fn bad_flags_fail() {
    let empty = Fixture::named("flags");
    let err = bookmarks::execute(empty.path(), &args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = bookmarks::execute(empty.path(), &args(&["mark"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert_eq!(
        bookmarks::run_in(empty.path(), &args(&["--bogus"])),
        ExitCode::FAILURE
    );
}

#[test]
fn svn_working_copy_reports_no_bookmark_support() {
    let empty = Fixture::named("svn-nosupport");
    std::fs::create_dir_all(empty.path().join(".svn")).unwrap();
    let err = bookmarks::execute(empty.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("does not support bookmarks"),
        "unexpected: {err}"
    );
    assert!(err.contains("\"svn\""), "unexpected: {err}");
}

mod common;

use common::{HomeGuard, with_owned_home};

/// One canned route: `(method_substring, result_json)`.
fn route(method: &str, canned: &str) -> (String, String) {
    (method.to_string(), canned.to_string())
}

/// Serve canned Conduit responses, returning the base URI.
fn serve_routes(routes: Vec<(String, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
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
    base
}

/// Write an `.arcrc` for `base` under a fresh home dir, and install the
/// home override. Returns the override guard.
fn arcrc_for(base: &str, name: &str) -> HomeGuard {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-vcs1-bookmarks-home-{name}-{}-{}",
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
            format!("{uri}/api/"): {"token": "vcs1-bookmarks-token"},
        },
    });
    std::fs::write(dir.join(".arcrc"), serde_json::to_string(&doc).unwrap()).unwrap();
    with_owned_home(dir)
}

/// Minimal fake-`hg` working copy with the listing fake installed.
struct HgRepo {
    dir: PathBuf,
}

impl HgRepo {
    fn named(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-bookmarks-hg-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".hg")).unwrap();
        let name = if cfg!(windows) {
            "fakehg_marks.bat"
        } else {
            "fakehg_marks.sh"
        };
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("vcs")
            .join(name);
        let to = dir.join(name);
        std::fs::copy(from, &to).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&to).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&to, perms).unwrap();
        }
        Self { dir }
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn backend(&self) -> arc_rs::hg::HgCommand {
        let name = if cfg!(windows) {
            "fakehg_marks.bat"
        } else {
            "fakehg_marks.sh"
        };
        let script = self.dir.join(name);
        #[cfg(windows)]
        {
            arc_rs::hg::HgCommand::custom("cmd", &["/C", &script.to_string_lossy()])
        }
        #[cfg(not(windows))]
        {
            arc_rs::hg::HgCommand::custom(&script.to_string_lossy(), &[])
        }
    }
}

impl Drop for HgRepo {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn lists_bookmarks_via_fake() {
    let repo = HgRepo::named("list");
    repo.write(
        ".fakehg-bookmarks",
        b"* active 12:abc123def456\n  other 3:789abcdef012\n",
    );
    let out = bookmarks::execute_with(&repo.dir, &args(&[]), &repo.backend()).unwrap();
    assert_eq!(out, "* active 12:abc123def456\n  other 3:789abcdef012\n");

    let out = bookmarks::execute_with(&repo.dir, &args(&["--json"]), &repo.backend()).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc.as_array().unwrap().len(), 2);
    assert_eq!(doc[0]["name"], serde_json::json!("active"));
    assert_eq!(doc[0]["revisions"], serde_json::json!([]));
}

#[test]
fn empty_bookmarks_list_is_empty() {
    let repo = HgRepo::named("empty");
    let out = bookmarks::execute_with(&repo.dir, &args(&[]), &repo.backend()).unwrap();
    assert_eq!(out, "");
}

#[test]
fn annotates_bookmarks_with_conduit() {
    let repo = HgRepo::named("annotate");
    repo.write(
        ".fakehg-bookmarks",
        b"* active 12:abc123def456\n  other 3:789abcdef012\n",
    );
    repo.write(".fakehg-msg", b"work\n\nDifferential Revision: D30\n");
    let base = serve_routes(vec![route(
        "differential.query",
        r#"[{"id":30,"title":"Thirty","status":2,"statusName":"Accepted","authorPHID":"PHID-USER-1"}]"#,
    )]);
    let _home = arcrc_for(&base, "annotate-home");
    let out = bookmarks::execute_with(&repo.dir, &args(&["--conduit-uri", &base]), &repo.backend())
        .unwrap();
    assert_eq!(
        out,
        "* active 12:abc123def456 [D30 Accepted]\n  other 3:789abcdef012 [D30 Accepted]\n"
    );
}
