//! `branches` behavior through the wired library.

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
