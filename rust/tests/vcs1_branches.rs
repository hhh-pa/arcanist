//! `branches` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::branches;

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
    assert!(out.contains("usage: arc-rs branches"), "unexpected: {out}");
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

    let out = branches::execute(repo.path(), &args(&[])).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "unexpected: {out}");
    assert!(lines[0].starts_with("* master"), "unexpected: {out}");
    assert!(lines[1].starts_with("  feature"), "unexpected: {out}");
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
    let out = branches::execute(repo.path(), &args(&[])).unwrap();
    assert!(
        out.lines().all(|line| !line.starts_with('*')),
        "unexpected: {out}"
    );
}

#[test]
fn empty_repo_lists_nothing() {
    let repo = Fixture::named("empty");
    repo.git(&["init", "-q", "-b", "master", "."]);
    assert_eq!(branches::list(repo.path(), false).unwrap(), Vec::new());
    assert_eq!(branches::execute(repo.path(), &args(&[])).unwrap(), "");
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
        err.contains("not inside a git or Mercurial working copy"),
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

    let out = branches::execute(repo.path(), &args(&[])).unwrap();
    assert!(out.contains("-> frozen [ahead 1]"), "unexpected: {out}");
}

#[test]
fn all_includes_remote_tracking_branches() {
    let repo = Fixture::init_repo("all");
    let bare = Fixture::init_bare("all-bare");
    repo.git(&["remote", "add", "origin", &bare.dir.to_string_lossy()]);
    repo.git(&["push", "-q", "origin", "master"]);

    let local_only = branches::execute(repo.path(), &args(&[])).unwrap();
    assert!(
        !local_only.contains("origin/master"),
        "unexpected: {local_only}"
    );

    let all = branches::execute(repo.path(), &args(&["--all"])).unwrap();
    assert!(all.contains("origin/master"), "unexpected: {all}");
    assert!(all.contains("(remote)"), "unexpected: {all}");

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
        "arc-rs-vcs1-branches-home-{name}-{}-{}",
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
            format!("{uri}/api/"): {"token": "vcs1-branches-token"},
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
            "arc-rs-vcs1-branches-hg-{tag}-{}-{}",
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
fn lists_hg_branches_via_fake() {
    let repo = HgRepo::named("list");
    repo.write(
        ".fakehg-branches",
        b"default 15101:a21ccf4412d5\nmy feature 14900:deadbeef1234 (inactive)\n",
    );
    repo.write(".fakehg-branch", b"my feature\n");
    repo.write(".fakehg-msg", b"Head subject\n\nBody.\n");
    let out = branches::execute_with(&repo.dir, &args(&[]), &repo.backend()).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "unexpected: {out}");
    // Current branch first, inactive suffix parsed away.
    assert!(
        lines[0].starts_with("* my feature deadbeef1234 Head subject"),
        "unexpected: {out}"
    );
    assert!(
        lines[1].starts_with("  default a21ccf4412d5 Head subject"),
        "unexpected: {out}"
    );
}

#[test]
fn annotates_git_branches_with_conduit() {
    let repo = Fixture::named("annotate");
    repo.git(&["init", "-q", "-b", "master", "."]);
    repo.git(&["config", "user.email", "vcs1-test@example.test"]);
    repo.git(&["config", "user.name", "VCS1 Test"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    repo.write("file.txt", "base\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&[
        "commit",
        "-q",
        "-m",
        "base",
        "-m",
        "Differential Revision: D11",
    ]);
    repo.git(&["branch", "feature"]);
    repo.git(&["checkout", "-q", "feature"]);
    repo.write("feat.txt", "feat\n");
    repo.git(&["add", "feat.txt"]);
    repo.git(&[
        "commit",
        "-q",
        "-m",
        "feat",
        "-m",
        "Differential Revision: D22",
    ]);

    let rows = r#"[{"id":22,"title":"Twenty","status":2,"statusName":"Accepted","authorPHID":"PHID-USER-1"},{"id":11,"title":"Eleven","status":1,"statusName":"Needs Review","authorPHID":"PHID-USER-1"}]"#;
    // One query per `execute` call below.
    let base = serve_routes(vec![
        route("differential.query", rows),
        route("differential.query", rows),
    ]);
    let _home = arcrc_for(&base, "annotate-home");
    let out = branches::execute(repo.path(), &args(&["--conduit-uri", &base])).unwrap();
    assert!(
        out.contains("feature ") && out.contains("[D22 Accepted]"),
        "unexpected: {out}"
    );
    assert!(
        out.contains("master ") && out.contains("[D11 Needs Review]"),
        "unexpected: {out}"
    );

    let out = branches::execute(repo.path(), &args(&["--conduit-uri", &base, "--json"])).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let feature = doc
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == serde_json::json!("feature"))
        .unwrap();
    assert_eq!(
        feature["revisions"][0]["monogram"],
        serde_json::json!("D22")
    );
}

#[test]
fn annotates_hg_branches_with_conduit() {
    let repo = HgRepo::named("annotate");
    repo.write(".fakehg-branches", b"default 15101:a21ccf4412d5\n");
    repo.write(".fakehg-branch", b"default\n");
    repo.write(".fakehg-msg", b"work\n\nDifferential Revision: D40\n");
    let base = serve_routes(vec![route(
        "differential.query",
        r#"[{"id":40,"title":"Forty","status":2,"statusName":"Accepted","authorPHID":"PHID-USER-1"}]"#,
    )]);
    let _home = arcrc_for(&base, "hg-annotate-home");
    let out = branches::execute_with(&repo.dir, &args(&["--conduit-uri", &base]), &repo.backend())
        .unwrap();
    assert!(
        out.contains("* default a21ccf4412d5 ") && out.contains("[D40 Accepted]"),
        "unexpected: {out}"
    );
}
