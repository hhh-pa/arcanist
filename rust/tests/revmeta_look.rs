// Integration tests for `arc-rs look`. Git fixtures are local temp repos
// only. Including the command source also compiles its sibling unit tests.

#[path = "../src/git.rs"]
mod git;
#[path = "../src/look.rs"]
mod look;

use std::path::PathBuf;
use std::process::ExitCode;

/// 2026-01-01T12:00:00Z (midday UTC).
const NOON: u64 = 1_767_268_800;
/// 2026-01-01T02:00:00Z (night UTC).
const NIGHT: u64 = 1_767_232_800;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-revmeta-look-{name}-{}-{}",
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

    fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
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

    fn init_repo(name: &str) -> Self {
        let repo = Self::named(name);
        repo.git(&["init", "-q", "-b", "master", "."]);
        repo.git(&["config", "user.email", "look-test@example.test"]);
        repo.git(&["config", "user.name", "Look Test"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        std::fs::write(repo.dir.join("file.txt"), "base\n").unwrap();
        repo.git(&["add", "file.txt"]);
        repo.git(&["commit", "-qm", "initial commit"]);
        repo
    }

    fn init_bare(name: &str) -> Self {
        let repo = Self::named(name);
        repo.git(&["init", "-q", "--bare", "."]);
        repo
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn look_name_matches_arc() {
    assert_eq!(look::NAME, "look");
}

#[test]
fn look_runs_help_and_usage_paths() {
    assert_eq!(look::run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(look::run(&args(&["--bogus"])), ExitCode::FAILURE);
}

#[test]
fn look_with_no_arguments_succeeds() {
    // No git interaction on the clearing path; the real cwd is fine.
    assert_eq!(look::run(&args(&[])), ExitCode::SUCCESS);
    assert_eq!(look::run(&args(&["big", "rock"])), ExitCode::FAILURE);
}

#[test]
fn clearing_opens_with_arcventure_header() {
    let plain = Fixture::named("clearing");
    let (out, code) = look::render(&plain.dir, &[], NOON).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.starts_with("Arcventure\n\n"), "{out}");
    assert!(out.contains("small clearing in the woods"), "{out}");
    assert!(out.contains("midday"), "{out}");
    assert!(out.contains("small trails and footpaths"), "{out}");
    assert!(out.contains("you can see remotes"), "{out}");
}

#[test]
fn clearing_at_night_uses_moon_phase() {
    let plain = Fixture::named("night");
    let (out, code) = look::render(&plain.dir, &[], NIGHT).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("Night has fallen"), "{out}");
}

#[test]
fn unknown_things_are_not_seen() {
    let plain = Fixture::named("unknown");
    let (out, code) = look::render(&plain.dir, &args(&["big", "rock"]), NOON).unwrap();
    assert_eq!(code, ExitCode::FAILURE);
    assert!(out.starts_with("Arcventure\n\n"), "{out}");
    assert!(
        out.contains("You do not see \"big rock\" anywhere."),
        "{out}"
    );
}

#[test]
fn remotes_lists_git_remotes() {
    let repo = Fixture::init_repo("remotes");
    let bare = Fixture::init_bare("remotes-bare");
    let url = bare.dir.to_string_lossy().to_string();
    repo.git(&["remote", "add", "origin", &url]);

    let (out, code) = look::render(&repo.dir, &args(&["remotes"]), NOON).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.starts_with("Arcventure\n\n"), "{out}");
    assert!(out.contains("grove of fruit trees"), "{out}");
    assert!(out.contains("you see remotes:"), "{out}");
    assert!(out.contains("Remote \"origin\":"), "{out}");
    assert!(out.contains(&format!("Fetch URI: {url}")), "{out}");
    assert!(out.contains(&format!("Push URI: {url}")), "{out}");
    assert!(out.contains("Push Repository: -"), "{out}");
    assert!(out.contains("toward published commits"), "{out}");
}

#[test]
fn remotes_without_remotes_still_shows_grove() {
    let repo = Fixture::init_repo("no-remotes");
    let (out, code) = look::render(&repo.dir, &args(&["remotes"]), NOON).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("grove of fruit trees"), "{out}");
    assert!(!out.contains("Remote \""), "{out}");
    assert!(out.contains("toward published commits"), "{out}");
}

#[test]
fn published_lists_remote_tracking_tips() {
    let repo = Fixture::init_repo("published");
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["update-ref", "refs/remotes/origin/master", &head]);

    let (out, code) = look::render(&repo.dir, &args(&["published"]), NOON).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("bank of the stream"), "{out}");
    assert!(out.contains("Floating on the water"), "{out}");
    assert!(out.contains(&head), "{out}");
    assert!(out.contains("They river bubbles peacefully."), "{out}");
}

#[test]
fn published_without_branches_shows_quiet_river() {
    let repo = Fixture::init_repo("unpublished");
    let (out, code) = look::render(&repo.dir, &args(&["published"]), NOON).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("bubbles quietly"), "{out}");
    assert!(!out.contains("Floating"), "{out}");
}

#[test]
fn remote_paths_require_git_repo() {
    let plain = Fixture::named("notrepo");
    assert!(!git::is_inside_work_tree(&plain.dir));
    let err = look::render(&plain.dir, &args(&["remotes"]), NOON).unwrap_err();
    assert!(err.contains("not inside a git repository"), "{err}");
    let err = look::render(&plain.dir, &args(&["published"]), NOON).unwrap_err();
    assert!(err.contains("not inside a git repository"), "{err}");
    // The clearing needs no repository.
    let (out, code) = look::render(&plain.dir, &[], NOON).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("small clearing"), "{out}");
}
