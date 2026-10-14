use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-upgrade-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn git(dir: &Path, args: &[&str]) {
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

fn fresh_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    git(&tmp.path, &["config", "user.email", "t@t"]);
    git(&tmp.path, &["config", "user.name", "t"]);
    git(&tmp.path, &["config", "commit.gpgsign", "false"]);
    tmp
}

fn commit(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-qm", name]);
}

#[test]
fn version_matches_package() {
    assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    assert!(!version().is_empty());
}

#[test]
fn outside_repos_everything_is_unknown() {
    let tmp = TempDir::new("plain");
    let status = inspect(&tmp.path).unwrap();
    assert!(!status.in_repo);
    assert!(status.upstream.is_none());
    assert_eq!(update_available(&status), None);
    let text = render(&status);
    assert!(text.contains("Not inside a git checkout"), "{text}");
    assert!(text.contains("never updates itself"), "{text}");
    let json = report_json(&status);
    assert_eq!(json["in_repo"], serde_json::json!(false));
    assert_eq!(json["update_available"], serde_json::Value::Null);
}

#[test]
fn fresh_repo_reports_branch_head_and_cleanliness() {
    let repo = fresh_repo("fresh");
    commit(&repo.path, "a.txt");
    let status = inspect(&repo.path).unwrap();
    assert!(status.in_repo);
    assert_eq!(status.branch.as_deref(), Some("master"));
    assert!(status.head.is_some());
    assert_eq!(status.clean, Some(true));
    assert_eq!(status.changed_files, Some(0));
    assert!(status.upstream.is_none());
    assert_eq!(update_available(&status), None);

    std::fs::write(repo.path.join("a.txt"), "dirty\n").unwrap();
    let dirty = inspect(&repo.path).unwrap();
    assert_eq!(dirty.clean, Some(false));
    assert_eq!(dirty.changed_files, Some(1));
    let text = render(&dirty);
    assert!(text.contains("dirty (1 changed files)"), "{text}");
    assert!(text.contains("Branch: master"), "{text}");
}

#[test]
fn behind_counts_come_from_local_refs_only() {
    let origin = TempDir::new("origin");
    git(&origin.path, &["init", "-q", "--bare"]);
    let repo = fresh_repo("ahead");
    commit(&repo.path, "a.txt");
    git(
        &repo.path,
        &["remote", "add", "origin", &origin.path.to_string_lossy()],
    );
    git(&repo.path, &["push", "-q", "-u", "origin", "master"]);
    let synced = inspect(&repo.path).unwrap();
    assert_eq!(synced.upstream.as_deref(), Some("origin/master"));
    assert_eq!((synced.ahead, synced.behind), (Some(0), Some(0)));
    assert_eq!(update_available(&synced), Some(false));

    let other = TempDir::new("other");
    git(
        &other.path,
        &["clone", "-q", &origin.path.to_string_lossy(), "."],
    );
    git(&other.path, &["config", "user.email", "t@t"]);
    git(&other.path, &["config", "user.name", "t"]);
    git(&other.path, &["config", "commit.gpgsign", "false"]);
    commit(&other.path, "b.txt");
    git(&other.path, &["push", "-q", "origin", "master"]);
    git(&repo.path, &["fetch", "-q", "origin"]);
    let behind = inspect(&repo.path).unwrap();
    assert_eq!(behind.behind, Some(1));
    assert_eq!(update_available(&behind), Some(true));
    let text = render(&behind);
    assert!(text.contains("update available"), "{text}");
    let json = report_json(&behind);
    assert_eq!(json["behind"], serde_json::json!(1));
    assert!(how_to_update().contains("git pull --rebase"));
}

#[test]
fn args_cover_check_json_apply_and_help() {
    assert!(matches!(parse_args(&[]), Parsed::Options(_)));
    assert!(matches!(
        parse_args(&["--check".to_string()]),
        Parsed::Options(_)
    ));
    assert!(matches!(
        parse_args(&["--json".to_string()]),
        Parsed::Options(_)
    ));
    assert!(matches!(parse_args(&["--help".to_string()]), Parsed::Help));
    assert!(matches!(
        parse_args(&["--apply".to_string()]),
        Parsed::NotYetPorted(_)
    ));
    assert!(matches!(
        parse_args(&["--yes".to_string()]),
        Parsed::NotYetPorted(_)
    ));
    assert!(matches!(
        parse_args(&["--bogus".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(parse_args(&["pos".to_string()]), Parsed::Error(_)));
}
