#[path = "../src/git.rs"]
mod git;

#[path = "../src/bookmarks.rs"]
mod bookmarks;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

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
fn broken_hg_checkout_fails() {
    // A bare `.hg` directory is detected as Mercurial, but `hg bookmarks`
    // cannot succeed there (whether `hg` is missing or the checkout is
    // not a real repository); either way this must fail, never lie.
    let broken = Fixture::named("broken-hg");
    std::fs::create_dir_all(broken.path().join(".hg")).unwrap();
    assert_eq!(
        bookmarks::find_hg_root(broken.path()),
        Some(broken.path().to_path_buf())
    );
    assert!(bookmarks::execute(broken.path(), &args(&[])).is_err());
    assert_eq!(
        bookmarks::run_in(broken.path(), &args(&[])),
        ExitCode::FAILURE
    );
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
