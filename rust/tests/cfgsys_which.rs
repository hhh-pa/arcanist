//! `which` behavior through the wired library.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::which;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-cfgsys-which-{tag}-{}-{n}",
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

mod common;

use common::with_temp_home;

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn module_name_matches_arc() {
    assert_eq!(which::NAME, "which");
}

#[test]
fn run_help_unported_and_flag_errors() {
    assert_eq!(which::run(&["--help".to_string()]), ExitCode::SUCCESS);
    assert_eq!(which::run(&["--base".to_string()]), ExitCode::FAILURE);
    assert_eq!(which::run(&["--any-status".to_string()]), ExitCode::FAILURE);
    assert_eq!(which::run(&["--bogus".to_string()]), ExitCode::FAILURE);
    assert_eq!(
        which::run(&["a".to_string(), "b".to_string()]),
        ExitCode::FAILURE
    );
}

#[test]
fn run_reports_current_checkout() {
    let tmp = TempDir::new("run");
    let _home = with_temp_home(tmp.path());
    assert_eq!(which::run(&[]), ExitCode::SUCCESS);
    assert_eq!(which::run(&["--show-base".to_string()]), ExitCode::SUCCESS);
    assert_eq!(which::run(&["--json".to_string()]), ExitCode::SUCCESS);
}

#[test]
fn fixture_context_resolves_every_section() {
    let tmp = TempDir::new("ctx");
    run_git(tmp.path(), &["init", "-q", "-b", "master"]);
    run_git(tmp.path(), &["config", "user.email", "t@t"]);
    run_git(tmp.path(), &["config", "user.name", "t"]);
    run_git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    run_git(
        tmp.path(),
        &["remote", "add", "origin", "https://example.test/repo.git"],
    );
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"phabricator.uri": "https://phabricator.example/", "repository.callsign": "X"}"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("a.txt"), "x\n").unwrap();
    run_git(tmp.path(), &["add", "a.txt"]);
    run_git(tmp.path(), &["commit", "-qm", "first"]);

    let home = TempDir::new("home");
    let user_file = home.path().join(".arcrc");
    std::fs::write(
        &user_file,
        include_str!("fixtures/cfgsys/arcrc.sample.json"),
    )
    .unwrap();

    let ctx = which::load_context(tmp.path(), Some(&user_file), None, None, None, "HEAD").unwrap();
    assert_eq!(ctx.vcs, "git");
    assert_eq!(ctx.branch.as_deref(), Some("master"));
    assert_eq!(
        ctx.remote_url.as_deref(),
        Some("https://example.test/repo.git")
    );
    assert_eq!(ctx.callsign.as_deref(), Some("X"));
    assert_eq!(
        ctx.conduit_uri.as_deref(),
        Some("https://phabricator.example/")
    );
    assert_eq!(ctx.conduit_source, "project (.arcconfig)");
    assert_eq!(ctx.token_installed, Some(true));
    assert!(ctx.base.is_some());

    let text = which::render(&ctx);
    assert!(text.contains("Branch: master"), "{text}");
    assert!(text.contains("origin"), "{text}");
    assert!(text.contains("Callsign: X"), "{text}");
    assert!(text.contains("Token: installed"), "{text}");

    let (uri, source) = which::resolve_conduit_in(tmp.path(), Some("https://flag.example/"), None);
    assert_eq!(uri.as_deref(), Some("https://flag.example/"));
    assert!(source.contains("--conduit-uri"));

    let missing = which::load_context(tmp.path(), None, None, None, None, "HEAD").unwrap();
    assert_eq!(missing.token_installed, Some(false));
}
