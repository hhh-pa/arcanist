//! Subprocess tests with explicitly controlled child environments
//! (T1176 DEMARK S13). Arms that depend on ambient process state
//! (`PATH`, `ARC_CONDUIT_URI`) cannot stage in-process — unit tests
//! must not mutate the parent environment — so these spawn the built
//! `arc` binary with a hermetic child env instead. Only the child
//! `Command` carries `env`/`env_remove`; the parent process
//! environment is never touched, and no test reads the real HOME or
//! the network (git fixtures use local paths only).

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// House scratch dir (pid + per-file counter); removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-env-hermetic-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// Run `git` in `dir`, asserting success (test-process env untouched).
fn git(dir: &Path, args: &[&str]) {
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
}

/// Run `arc` in `repo` with an empty `home` and every conduit channel
/// cleared; `strip_path` additionally removes `PATH` from the child.
fn arc(home: &Path, repo: &Path, argv: &[&str], strip_path: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arc"));
    cmd.args(argv)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("ARC_TRACE", "")
        .env("ARC_ANSI", "")
        .env_remove("ARC_GLOBAL_CONDUIT_URI")
        .env_remove("ARC_GLOBAL_CONDUIT_TOKEN")
        .env_remove("ARC_ANONYMOUS")
        .env_remove("ARC_GLOBAL_LOAD_OPTIONS")
        .env_remove("ARC_CONDUIT_URI")
        .env_remove("ARC_CONDUIT_TOKEN")
        .current_dir(repo)
        .stdin(Stdio::null());
    if strip_path {
        cmd.env_remove("PATH");
    }
    cmd.output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Unset-`PATH` arm (`lint_external::binary_exists`): with no `PATH`
/// in the child, a bare `bin` candidate resolves nothing and the
/// `.arclint` parse error surfaces.
#[test]
fn lint_bin_lookup_without_path_reports_unlocatable() {
    let root = TempDir::new("lint-nopath");
    let home = root.path().join("home");
    let repo = root.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(
        repo.join(".arclint"),
        r#"{"linters": {"cpp": {"type": "cpplint", "bin": "definitely-missing-xyz"}}}"#,
    )
    .unwrap();
    let out = arc(&home, &repo, &["lint", "--"], true);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).is_empty());
    assert!(
        text(&out.stderr).contains("None of the configured binaries can be located."),
        "{}",
        text(&out.stderr)
    );
}

/// Missing-URI arm (`land` client setup): with no flag, no
/// `ARC_CONDUIT_URI`, and an empty HOME (no `~/.arcrc`, no
/// `.arcconfig`), landing onto a local origin reports the missing
/// conduit URI instead of contacting Conduit.
#[test]
fn land_without_conduit_uri_reports_missing_uri() {
    let root = TempDir::new("land-nouri");
    let home = root.path().join("home");
    let repo = root.path().join("repo");
    let origin = root.path().join("origin.git");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "master"]);
    git(&repo, &["config", "user.email", "env-test@example.test"]);
    git(&repo, &["config", "user.name", "Env Test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("file.txt"), "base\n").unwrap();
    git(&repo, &["add", "file.txt"]);
    git(&repo, &["commit", "-qm", "initial commit"]);
    git(root.path(), &["init", "-q", "--bare", "origin.git"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "master"]);
    git(&repo, &["checkout", "-qb", "feature"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-qm", "add a"]);
    let out = arc(&home, &repo, &["land", "--"], false);
    assert!(!out.status.success());
    assert!(text(&out.stdout).is_empty());
    assert!(
        text(&out.stderr).contains("no conduit URI: pass --conduit-uri"),
        "{}",
        text(&out.stderr)
    );
}
