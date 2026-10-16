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

fn never_prompt(_: &str) -> Option<String> {
    None
}

fn offline_resolution(runtime_rules: &str) -> which::Resolution<'_> {
    which::Resolution {
        runtime_rules,
        prompt: &never_prompt,
        match_config: which::MatchConfig::Unavailable("no conduit in test".to_string()),
    }
}

#[test]
fn run_help_unported_and_flag_errors() {
    assert_eq!(which::run(&["--help".to_string()]), ExitCode::SUCCESS);
    // `--base` without rules is a usage error, not an unported flag.
    assert_eq!(which::run(&["--base".to_string()]), ExitCode::FAILURE);
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

    let resolution = offline_resolution("");
    let ctx = which::load_context(
        tmp.path(),
        Some(&user_file),
        None,
        None,
        None,
        "HEAD",
        false,
        &resolution,
    )
    .unwrap();
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

    let resolution = offline_resolution("");
    let missing = which::load_context(
        tmp.path(),
        None,
        None,
        None,
        None,
        "HEAD",
        false,
        &resolution,
    )
    .unwrap();
    assert_eq!(missing.token_installed, Some(false));
}

fn fixture_git_repo(tag: &str, commits: usize) -> TempDir {
    let tmp = TempDir::new(tag);
    run_git(tmp.path(), &["init", "-q", "-b", "master"]);
    run_git(tmp.path(), &["config", "user.email", "t@t"]);
    run_git(tmp.path(), &["config", "user.name", "t"]);
    run_git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    for n in 0..commits {
        let name = format!("f{n}.txt");
        std::fs::write(tmp.path().join(&name), "x\n").unwrap();
        run_git(tmp.path(), &["add", &name]);
        run_git(tmp.path(), &["commit", "-qm", &name]);
    }
    tmp
}

fn resolve_with_rules(dir: &Path, runtime_rules: &str) -> Result<(String, String), String> {
    let loaded = which::load_config_sources(dir, None).unwrap();
    let inputs = which::BaseInputs {
        runtime_rules,
        prompt: &never_prompt,
    };
    which::resolve_base(dir, None, "HEAD", false, &loaded, &inputs)
}

#[test]
fn base_empty_repo_reports_no_commits() {
    let tmp = fixture_git_repo("empty", 0);
    let (base, why) = resolve_with_rules(tmp.path(), "").unwrap();
    assert_eq!(base, which::GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("no commits"), "{why}");
}

#[test]
fn base_single_commit_repo_uses_empty_tree() {
    let tmp = fixture_git_repo("single", 1);
    let (base, why) = resolve_with_rules(tmp.path(), "").unwrap();
    assert_eq!(base, which::GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("only one commit"), "{why}");
}

#[test]
fn base_explicit_unknown_commit_errors() {
    let tmp = fixture_git_repo("unknown", 2);
    let loaded = which::load_config_sources(tmp.path(), None).unwrap();
    let inputs = which::BaseInputs {
        runtime_rules: "",
        prompt: &never_prompt,
    };
    let err = which::resolve_base(
        tmp.path(),
        Some("no-such-rev"),
        "HEAD",
        false,
        &loaded,
        &inputs,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Unable to find any git commit named 'no-such-rev' in this repository."
    );
}

#[test]
fn base_garbage_rule_string_errors() {
    let tmp = fixture_git_repo("garbage", 2);
    let err = resolve_with_rules(tmp.path(), "{{not valid").unwrap_err();
    assert!(
        err.contains("is invalid, it must have a type and name"),
        "{err}"
    );
}

#[test]
fn base_exec_rule_runs_command() {
    let tmp = fixture_git_repo("exec", 2);
    let (base, _) = resolve_with_rules(tmp.path(), "arc:exec(echo hi)").unwrap();
    assert_eq!(base, "hi");
}

#[test]
fn r6_core12_bare_discovery_under_temp_home() {
    // CORE-12: discovery reads `phabricator.uri`, else `default`, from any
    // source. Under an empty temp HOME a bare dir resolves to nothing; a
    // `default` in the temp user file resolves like stock PHP.
    let home = TempDir::new("home");
    let _home = with_temp_home(home.path());
    let bare = TempDir::new("bare");
    assert_eq!(
        which::resolve_conduit_in(bare.path(), None, None),
        (None, "none")
    );
    std::fs::write(
        home.path().join(".arcrc"),
        r#"{"config": {"default": "https://d.example/"}}"#,
    )
    .unwrap();
    assert_eq!(
        which::resolve_conduit_in(bare.path(), None, None).0,
        Some("https://d.example/".to_string())
    );
}

#[test]
fn carryover_user_uri_labels_the_user_source() {
    // Wave-2 carry-over: a user-file URI labels `user (~/.arcrc)` from
    // `merged_value`, not the old hardcoded `project (.arcconfig)`.
    let home = TempDir::new("carryhome");
    let _home = with_temp_home(home.path());
    let bare = TempDir::new("carrybare");
    std::fs::write(
        home.path().join(".arcrc"),
        r#"{"config": {"phabricator.uri": "https://u.example/"}}"#,
    )
    .unwrap();
    assert_eq!(
        which::resolve_conduit_in(bare.path(), None, None),
        (Some("https://u.example/".to_string()), "user (~/.arcrc)")
    );
}
