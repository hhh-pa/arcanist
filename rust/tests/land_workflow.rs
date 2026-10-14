#[path = "../src/land.rs"]
mod land;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-land-{name}-{}-{}",
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
        git_in(&self.dir, args)
    }

    /// Fresh repo on `master` with one commit; never touches real remotes.
    fn init_repo(name: &str) -> Self {
        let repo = Self::named(name);
        repo.git(&["init", "-q", "-b", "master", "."]);
        repo.git(&["config", "user.email", "land-test@example.test"]);
        repo.git(&["config", "user.name", "Land Test"]);
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

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
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

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn make_feature(repo: &Fixture) {
    repo.git(&["checkout", "-qb", "feature"]);
    repo.commit_file("a.txt", "a\n", "add a");
    repo.commit_file("b.txt", "b\n", "add b");
}

#[test]
fn module_name_is_land() {
    assert_eq!(land::NAME, "land");
}

#[test]
fn run_uses_current_dir() {
    let repo = Fixture::init_repo("run-contract");
    make_feature(&repo);
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(repo.path()).unwrap();
    let code = land::run(&args(&["--preview"]));
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    // Unchanged: still on the feature branch.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn help_succeeds_and_prints_usage() {
    let repo = Fixture::init_repo("help");
    let out = land::execute(repo.path(), &args(&["--help"])).unwrap();
    assert!(out.contains("usage: arc-rs land"), "unexpected: {out}");
    assert_eq!(
        land::run_in(repo.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn preview_lists_commits_without_changing_anything() {
    let repo = Fixture::init_repo("preview");
    make_feature(&repo);
    let master_before = repo.git(&["rev-parse", "master"]);

    let plan = land::plan(repo.path(), "master").unwrap();
    assert_eq!(plan.branch, "feature");
    assert_eq!(plan.onto, "master");
    assert_eq!(plan.commits.len(), 2);
    assert_eq!(plan.head, repo.git(&["rev-parse", "HEAD"]));

    let out = land::execute(repo.path(), &args(&["--preview"])).unwrap();
    assert!(out.contains("'feature'"), "missing branch: {out}");
    assert!(out.contains("'master'"), "missing onto: {out}");
    assert!(out.contains("base: "), "missing base: {out}");
    assert!(out.contains("head: "), "missing head: {out}");
    assert!(
        out.contains("add a") && out.contains("add b"),
        "missing commits: {out}"
    );

    assert_eq!(
        land::run_in(repo.path(), &args(&["--preview"])),
        ExitCode::SUCCESS
    );
    assert_eq!(repo.git(&["rev-parse", "master"]), master_before);
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
    assert_eq!(repo.git(&["rev-list", "--count", "master"]), "1");
}

#[test]
fn lands_squash_onto_master() {
    let repo = Fixture::init_repo("land-basic");
    make_feature(&repo);

    let out = land::execute(repo.path(), &args(&[])).unwrap();
    assert!(
        out.contains("Landed 'feature' onto 'master'"),
        "unexpected: {out}"
    );

    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "master");
    let body = repo.git(&["log", "-1", "--format=%B"]);
    assert!(body.contains("feature"), "message must name branch: {body}");
    assert!(
        body.contains("add a") && body.contains("add b"),
        "missing commits: {body}"
    );
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
    assert!(repo.path().join("a.txt").exists());
    assert!(repo.path().join("b.txt").exists());
    // The feature branch is kept for the user to delete.
    repo.git(&["rev-parse", "--verify", "feature"]);
}

#[test]
fn dirty_tree_blocks_land_but_not_preview() {
    let repo = Fixture::init_repo("dirty");
    make_feature(&repo);
    repo.write("a.txt", "uncommitted\n");

    let err = land::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("not clean"), "unexpected: {err}");
    assert!(land::ensure_clean(repo.path()).is_err());
    assert_eq!(land::run_in(repo.path(), &args(&[])), ExitCode::FAILURE);

    land::execute(repo.path(), &args(&["--preview"])).expect("preview allows dirt");
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn detached_head_fails() {
    let repo = Fixture::init_repo("detached");
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);
    let err = land::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("detached"), "unexpected: {err}");
}

#[test]
fn nothing_to_land_fails() {
    let repo = Fixture::init_repo("empty");
    let err = land::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("nothing to land"), "unexpected: {err}");

    repo.git(&["checkout", "-qb", "nofeature"]);
    let err = land::execute(repo.path(), &args(&[])).unwrap_err();
    assert!(err.contains("nothing to land"), "unexpected: {err}");
}

#[test]
fn outside_repo_fails() {
    let empty = Fixture::named("notrepo");
    let err = land::execute(empty.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("not inside a git repository"),
        "unexpected: {err}"
    );
    assert_eq!(land::run_in(empty.path(), &args(&[])), ExitCode::FAILURE);
}

#[test]
fn onto_flag_targets_another_branch() {
    let repo = Fixture::init_repo("onto");
    repo.git(&["branch", "stable"]);
    make_feature(&repo);
    let master_before = repo.git(&["rev-parse", "master"]);

    let out = land::execute(repo.path(), &args(&["--onto", "stable"])).unwrap();
    assert!(
        out.contains("Landed 'feature' onto 'stable'"),
        "unexpected: {out}"
    );

    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "stable");
    assert_eq!(repo.git(&["rev-parse", "master"]), master_before);
    let body = repo.git(&["log", "-1", "--format=%B", "stable"]);
    assert!(body.contains("feature"), "message must name branch: {body}");
}

#[test]
fn missing_onto_branch_fails() {
    let repo = Fixture::init_repo("onto-missing");
    make_feature(&repo);
    let err = land::execute(repo.path(), &args(&["--onto", "nope"])).unwrap_err();
    assert!(err.contains("does not exist"), "unexpected: {err}");
}

#[test]
fn remote_tracking_onto_is_not_yet_ported() {
    let repo = Fixture::init_repo("remotetrack");
    let bare = Fixture::init_bare("remotetrack-bare");
    repo.git(&["remote", "add", "origin", &bare.dir.to_string_lossy()]);
    repo.git(&["push", "-q", "origin", "master"]);
    repo.git(&["fetch", "-q", "origin"]);
    make_feature(&repo);

    let err = land::execute(repo.path(), &args(&["--onto", "origin/master"])).unwrap_err();
    assert!(err.contains("not yet ported"), "unexpected: {err}");
    assert_eq!(
        land::run_in(repo.path(), &args(&["--onto", "origin/master"])),
        ExitCode::FAILURE
    );
    // Nothing changed: still on the feature branch.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn push_sends_onto_to_remote() {
    let repo = Fixture::init_repo("push");
    let bare = Fixture::init_bare("push-bare");
    repo.git(&["remote", "add", "origin", &bare.dir.to_string_lossy()]);
    repo.git(&["push", "-q", "origin", "master"]);
    make_feature(&repo);

    let out = land::execute(repo.path(), &args(&["--push"])).unwrap();
    assert!(
        out.contains("Pushed 'master' to 'origin'"),
        "unexpected: {out}"
    );

    let local = repo.git(&["rev-parse", "master"]);
    let remote = bare.git(&["rev-parse", "refs/heads/master"]);
    assert_eq!(local, remote);
}

#[test]
fn land_without_push_leaves_remote_alone() {
    let repo = Fixture::init_repo("nopush");
    let bare = Fixture::init_bare("nopush-bare");
    repo.git(&["remote", "add", "origin", &bare.dir.to_string_lossy()]);
    repo.git(&["push", "-q", "origin", "master"]);
    let remote_before = bare.git(&["rev-parse", "refs/heads/master"]);
    make_feature(&repo);

    land::execute(repo.path(), &args(&[])).unwrap();

    assert_eq!(bare.git(&["rev-parse", "refs/heads/master"]), remote_before);
    assert_ne!(repo.git(&["rev-parse", "master"]), remote_before);
}

#[test]
fn bad_flags_fail() {
    let repo = Fixture::init_repo("flags");
    let err = land::execute(repo.path(), &args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = land::execute(repo.path(), &args(&["--onto"])).unwrap_err();
    assert!(err.contains("--onto requires"), "unexpected: {err}");
    let err = land::execute(repo.path(), &args(&["feature"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert_eq!(
        land::run_in(repo.path(), &args(&["--bogus"])),
        ExitCode::FAILURE
    );
}
