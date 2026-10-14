//! `upgrade` behavior through the wired library.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::upgrade;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-cfgsys-up-{tag}-{}-{n}", std::process::id()));
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
    assert_eq!(upgrade::NAME, "upgrade");
    assert_eq!(upgrade::version(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn run_help_json_and_unported_paths() {
    assert_eq!(upgrade::run(&["--help".to_string()]), ExitCode::SUCCESS);
    assert_eq!(upgrade::run(&[]), ExitCode::SUCCESS);
    assert_eq!(upgrade::run(&["--json".to_string()]), ExitCode::SUCCESS);
    assert_eq!(upgrade::run(&["--apply".to_string()]), ExitCode::FAILURE);
    assert_eq!(upgrade::run(&["--bogus".to_string()]), ExitCode::FAILURE);
    assert_eq!(upgrade::run(&["extra".to_string()]), ExitCode::FAILURE);
}

#[test]
fn fixture_repo_report_is_read_only() {
    let tmp = TempDir::new("report");
    run_git(tmp.path(), &["init", "-q", "-b", "master"]);
    run_git(tmp.path(), &["config", "user.email", "t@t"]);
    run_git(tmp.path(), &["config", "user.name", "t"]);
    run_git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path().join("a.txt"), "x\n").unwrap();
    run_git(tmp.path(), &["add", "a.txt"]);
    run_git(tmp.path(), &["commit", "-qm", "init"]);

    let status = upgrade::inspect(tmp.path()).unwrap();
    assert!(status.in_repo);
    assert_eq!(status.branch.as_deref(), Some("master"));
    let text = upgrade::render(&status);
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
    assert!(text.contains("Branch: master"), "{text}");
    assert!(text.contains("never updates itself"), "{text}");
    let json = upgrade::report_json(&status);
    assert_eq!(json["branch"], serde_json::json!("master"));

    // The report must not change the checkout: still clean afterwards.
    let again = upgrade::inspect(tmp.path()).unwrap();
    assert_eq!(again.clean, Some(true));
}
