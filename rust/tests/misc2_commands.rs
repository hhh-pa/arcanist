//! Public-behavior tests for the misc bundle: `linters`, `anoid`, `work`,
//! `inspect`, and `cover` through the wired library.

use std::path::{Path, PathBuf};

use arc_rs::{anoid, cover, inspect, linters, work};

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-misc2-{tag}-{}-{n}", std::process::id()));
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init"]);
    git(tmp.path(), &["config", "user.email", "misc2@test"]);
    git(tmp.path(), &["config", "user.name", "misc2"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    tmp
}

fn commit_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-m", contents]);
}

#[test]
fn command_names_match_arc() {
    assert_eq!(linters::NAME, "linters");
    assert_eq!(anoid::NAME, "anoid");
    assert_eq!(work::NAME, "work");
    assert_eq!(inspect::NAME, "inspect");
    assert_eq!(cover::NAME, "cover");
}

#[test]
fn every_command_reports_help() {
    assert_eq!(
        linters::run(&args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    // T1153 BROWSE-B21: `anoid` is toolset; piped runs carry `--`.
    assert_eq!(
        anoid::run(&args(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        work::run(&args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        inspect::run(&args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        cover::run(&args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn every_command_rejects_unknown_flags() {
    assert_eq!(
        linters::run(&args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        anoid::run(&args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        work::run(&args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        inspect::run(&args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        cover::run(&args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn linters_lists_the_full_registry() {
    let all = linters::available_linters();
    // TXT plus all 33 round-3 linters (JSON is listed twice: the native
    // linter and the jsonlint wrapper share the PHP name, as in `arc`).
    assert_eq!(all.len(), 34);
    assert!(all.iter().any(|info| info.name == "TXT"));
    assert!(all.iter().all(|info| !info.description.is_empty()));

    // Hermetic working copy: `run()` would resolve the process cwd (the
    // package dir under `cargo test`) and read this repo's own `.arclint`,
    // so drive `run_in` against a temp repo with a fixture config instead.
    // The `text` spec's `exclude` key and the `xhpast` spec keep the
    // per-spec exclude parse and the XHPAST version probe covered.
    let tmp = init_repo("linters-config");
    commit_file(
        tmp.path(),
        ".arclint",
        r#"{"linters": {"text": {"type": "text", "exclude": "(\\.txt$)"}, "xhpast": {"type": "xhpast", "include": "(\\.php$)"}}}"#,
    );
    let configured = linters::configured_classes(tmp.path()).unwrap();
    assert!(configured.contains(&"ArcanistTextLinter"));
    assert!(configured.contains(&"ArcanistXHPASTLinter"));
    assert_eq!(
        linters::run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        linters::run_in(tmp.path(), &args(&["--search", "zzz-no-such-linter"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        linters::run_in(tmp.path(), &args(&["--verbose", "TXT"])),
        std::process::ExitCode::SUCCESS
    );

    // A repo without `.arclint` keeps the unconfigured path covered.
    let bare = init_repo("linters-bare");
    commit_file(bare.path(), "f.txt", "one");
    assert!(linters::configured_classes(bare.path()).unwrap().is_empty());
    assert_eq!(
        linters::run_in(bare.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn anoid_discovers_scripts_and_binaries() {
    let tmp = TempDir::new("anoid");
    assert!(anoid::find_script(tmp.path()).is_none());
    let script = tmp
        .path()
        .join("support")
        .join("arcanoid")
        .join("arcanoid.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, b"print('probe')\n").unwrap();
    assert_eq!(anoid::find_script(tmp.path()), Some(script));

    let path = std::env::var_os("PATH").expect("tests need PATH");
    assert!(anoid::find_on_path(&path, "git").is_some());
    assert_eq!(
        anoid::run_in(tmp.path(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn work_tracks_branch_switches() {
    let tmp = init_repo("work-switch");
    commit_file(tmp.path(), "f.txt", "one");
    let first = git(tmp.path(), &["symbolic-ref", "--short", "HEAD"]);

    assert_eq!(
        work::run_in(tmp.path(), &args(&["feature"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        git(tmp.path(), &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );

    assert_eq!(
        work::run_in(tmp.path(), &args(&[&first])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(git(tmp.path(), &["symbolic-ref", "--short", "HEAD"]), first);
}

#[test]
fn inspect_shows_working_copy_state() {
    let tmp = init_repo("inspect");
    commit_file(tmp.path(), "f.txt", "one");
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);

    assert!(inspect::SUPPORTED_TYPES.contains(&"working-copy"));
    // T1153 RB-D14: without `--explore` the raw argument renders
    // unresolved; exploration resolves and loads the message.
    let block = inspect::inspect_one(tmp.path(), "working-copy(HEAD)", false).unwrap();
    assert!(block.contains("Commit \"HEAD\""), "{block}");
    assert!(block.contains("ArcanistWorkingCopyStateRef"));
    let explored = inspect::inspect_one(tmp.path(), "working-copy(HEAD)", true).unwrap();
    assert!(explored.contains(&head), "{explored}");
    assert!(explored.contains("one"));
    assert_eq!(
        inspect::run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn cover_handles_clean_and_changed_trees() {
    let tmp = init_repo("cover");
    commit_file(tmp.path(), "f.txt", "one\ntwo\n");
    assert_eq!(
        cover::run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );

    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\n").unwrap();
    assert_eq!(
        cover::run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        cover::run_in(tmp.path(), &args(&["--rev", "HEAD"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(cover::blame_file(tmp.path(), "f.txt").unwrap().len(), 2);
    assert_eq!(cover::readable_sequence(&[1, 2, 3, 5]), "1-3, 5");
}

// T1153 Wave-2 imports for the appended finding tests below (kept here
// so the shared suite stays append-only).
mod common;
use arc_rs::{conduit, look};
use common::with_temp_home;

// T1153 RB-D12: `look remotes`/`published` resolve repository
// hardpoints through Conduit in PHP, so a missing URI fails fatal
// (255, like the `raiseURIException()` escape) instead of rendering
// degraded output. The bare clearing needs no conduit and still works.
#[test]
fn rbd12_look_conduit_modes_fail_without_uri() {
    let tmp = init_repo("rbd12");
    commit_file(tmp.path(), "f.txt", "one\n");
    let home = TempDir::new("rbd12-home");
    let _guard = with_temp_home(home.path());
    let fatal = std::process::ExitCode::from(conduit::EXIT_FATAL);
    assert_eq!(look::run_in(tmp.path(), &args(&["remotes"])), fatal);
    assert_eq!(look::run_in(tmp.path(), &args(&["published"])), fatal);
    assert_eq!(
        look::run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
}

// T1153 CORE-4: `inspect --explore browse(...)` builds its engine
// tokenless; with no URI anywhere it fails fatal with the toolset
// no-URI block instead of the strict "no token" path.
#[test]
fn core4_inspect_browse_fails_without_uri() {
    let tmp = init_repo("core4-inspect");
    commit_file(tmp.path(), "f.txt", "one\n");
    let home = TempDir::new("core4-inspect-home");
    let _guard = with_temp_home(home.path());
    assert_eq!(
        inspect::run_in(tmp.path(), &args(&["--explore", "browse(HEAD)"])),
        std::process::ExitCode::from(conduit::EXIT_FATAL)
    );
}
