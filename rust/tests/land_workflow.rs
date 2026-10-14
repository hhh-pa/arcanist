//! `land` against local git fixtures through the wired library.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{land, land_checks};

mod common;

use common::with_owned_home;

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

/// Repo with a local `origin` (land requires a pushable remote) plus the
/// bare remote. The feature branch is checked out with two commits.
fn repo_with_origin(name: &str) -> (Fixture, Fixture) {
    let repo = Fixture::init_repo(name);
    let bare = Fixture::init_bare(&format!("{name}-bare"));
    repo.git(&["remote", "add", "origin", &bare.dir.to_string_lossy()]);
    repo.git(&["push", "-q", "origin", "master"]);
    make_feature(&repo);
    (repo, bare)
}

fn temp_home(tag: &str) -> common::HomeGuard {
    let home = std::env::temp_dir().join(format!(
        "arc-rs-land-home-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    with_owned_home(home)
}

/// Run the engine with scripted answers and an isolated HOME, so conduit
/// discovery always takes the offline path.
fn run_engine(dir: &Path, list: &[&str], answers: &[&str]) -> Result<land::Out, String> {
    let _guard = temp_home("engine");
    let mut io = land::Io::canned(answers);
    land::execute_with_io(dir, &args(list), &mut io)
}

#[test]
fn module_name_is_land() {
    assert_eq!(land::NAME, "land");
}

#[test]
fn run_uses_current_dir() {
    let (repo, _bare) = repo_with_origin("run-contract");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(repo.path()).unwrap();
    let _guard = temp_home("run-contract");
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
    assert!(out.stdout.contains("usage: arc-rs land"), "{}", out.stdout);
    assert_eq!(
        land::run_in(repo.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn preview_lists_commits_without_changing_anything() {
    let (repo, _bare) = repo_with_origin("preview");
    let master_before = repo.git(&["rev-parse", "master"]);

    let out = run_engine(repo.path(), &["--preview"], &[]).unwrap();
    assert!(out.stderr.contains("** STRATEGY **"), "{}", out.stderr);
    assert!(out.stderr.contains("** ONTO TARGET **"), "{}", out.stderr);
    assert!(out.stderr.contains("** LANDING **"), "{}", out.stderr);
    assert!(out.stderr.contains("** PREVIEW **"), "{}", out.stderr);
    assert!(
        out.stderr.contains("revision info unavailable"),
        "{}",
        out.stderr
    );
    assert!(
        out.stdout.contains("add a") && out.stdout.contains("add b"),
        "{}",
        out.stdout
    );

    assert_eq!(repo.git(&["rev-parse", "master"]), master_before);
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
    assert_eq!(repo.git(&["rev-list", "--count", "master"]), "1");
}

#[test]
fn lands_squash_onto_master() {
    let (repo, bare) = repo_with_origin("land-basic");

    let out = run_engine(repo.path(), &[], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );

    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "master");
    // The squash lands on the remote; the local onto ref stays stale.
    let body = bare.git(&["log", "-1", "--format=%B", "refs/heads/master"]);
    assert!(body.contains("Land feature"), "unexpected message: {body}");
    assert!(
        body.contains("add a") && body.contains("add b"),
        "missing commits: {body}"
    );
    assert_eq!(bare.git(&["rev-list", "--count", "refs/heads/master"]), "2");
    assert_eq!(repo.git(&["rev-list", "--count", "master"]), "1");
    assert!(!repo.path().join("a.txt").exists());
    // Landed branches are pruned by default.
    assert!(repo.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn land_with_hold_skips_push() {
    let (repo, bare) = repo_with_origin("hold");
    let remote_before = bare.git(&["rev-parse", "refs/heads/master"]);

    let out = run_engine(repo.path(), &["--hold"], &["y", "y"]).unwrap();
    assert!(out.stdout.contains("HOLD CHANGES"), "{}", out.stdout);
    assert!(out.stdout.contains("git push --"), "{}", out.stdout);

    assert_eq!(bare.git(&["rev-parse", "refs/heads/master"]), remote_before);
    // Hold merges locally but leaves branches alone.
    assert!(!repo.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn keep_branches_preserves_feature() {
    let (repo, _bare) = repo_with_origin("keep");
    let out = run_engine(repo.path(), &["--keep-branches"], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );
    assert!(!repo.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn dirty_tree_blocks_land_but_not_preview() {
    let (repo, _bare) = repo_with_origin("dirty");
    repo.write("a.txt", "uncommitted\n");

    let err = run_engine(repo.path(), &[], &["y", "y"]).unwrap_err();
    assert!(err.contains("not clean"), "unexpected: {err}");
    assert!(land::ensure_clean_tree(repo.path(), land::Vcs::Git).is_err());

    run_engine(repo.path(), &["--preview"], &[]).expect("preview allows dirt");
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn detached_head_lands_head() {
    let (repo, _bare) = repo_with_origin("detached");
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);
    let out = run_engine(repo.path(), &["--preview"], &[]).unwrap();
    assert!(
        out.stderr.contains("Landing the current HEAD"),
        "{}",
        out.stderr
    );
    assert!(
        out.stdout.contains("add a") && out.stdout.contains("add b"),
        "{}",
        out.stdout
    );
}

#[test]
fn nothing_to_land_fails() {
    let (repo, _bare) = repo_with_origin("empty");
    repo.git(&["checkout", "-q", "master"]);
    let err = run_engine(repo.path(), &[], &["y", "y"]).unwrap_err();
    assert_eq!(err, "There are no commits to land.");

    repo.git(&["checkout", "-qb", "nofeature"]);
    let err = run_engine(repo.path(), &[], &["y", "y"]).unwrap_err();
    assert_eq!(err, "There are no commits to land.");
}

#[test]
fn outside_repo_fails() {
    let empty = Fixture::named("notrepo");
    let err = land::execute(empty.path(), &args(&[])).unwrap_err();
    assert_eq!(err, land::NOT_GIT_OR_HG);
    assert_eq!(land::run_in(empty.path(), &args(&[])), ExitCode::FAILURE);
}

#[test]
fn onto_flag_targets_another_branch() {
    let (repo, bare) = repo_with_origin("onto");
    repo.git(&["checkout", "-q", "master"]);
    repo.git(&["branch", "stable"]);
    repo.git(&["push", "-q", "origin", "stable"]);
    repo.git(&["checkout", "-q", "feature"]);
    let master_before = repo.git(&["rev-parse", "master"]);

    let out = run_engine(repo.path(), &["--onto", "stable"], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );

    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "stable");
    assert_eq!(repo.git(&["rev-parse", "master"]), master_before);
    let body = bare.git(&["log", "-1", "--format=%B", "refs/heads/stable"]);
    assert!(body.contains("Land feature"), "unexpected message: {body}");
    assert_ne!(bare.git(&["rev-parse", "refs/heads/stable"]), master_before);
}

#[test]
fn unknown_source_symbol_fails() {
    let (repo, _bare) = repo_with_origin("unknown-source");
    let err = run_engine(repo.path(), &["nope"], &[]).unwrap_err();
    assert!(
        err.contains("Branch \"nope\" does not exist in the local working copy."),
        "unexpected: {err}"
    );
}

#[test]
fn new_onto_branch_prompts_and_aborts() {
    let (repo, _bare) = repo_with_origin("onto-new");
    let err = run_engine(repo.path(), &["--onto", "nope"], &["n"]).unwrap_err();
    assert_eq!(err, land::ABORT);
    // Nothing changed: still on the feature branch.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn new_onto_branch_is_created_when_confirmed() {
    let (repo, bare) = repo_with_origin("onto-create");
    let out = run_engine(repo.path(), &["--onto", "fresh"], &["y", "y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );
    let body = bare.git(&["log", "-1", "--format=%B", "refs/heads/fresh"]);
    assert!(body.contains("Land feature"), "unexpected message: {body}");
}

#[test]
fn positional_ref_selects_source() {
    let (repo, _bare) = repo_with_origin("positional");
    repo.git(&["checkout", "-q", "master"]);
    let out = run_engine(repo.path(), &["--preview", "feature"], &[]).unwrap();
    assert!(
        out.stdout.contains("add a") && out.stdout.contains("add b"),
        "{}",
        out.stdout
    );
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "master");
}

#[test]
fn revision_preview_skips_gates_without_conduit() {
    let (repo, _bare) = repo_with_origin("revskip-preview");

    let out = run_engine(repo.path(), &["--preview", "--revision", "D123"], &[]).unwrap();
    assert!(
        out.stderr.contains("revision info unavailable"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("** PREVIEW **"), "{}", out.stderr);
    assert!(
        out.stdout.contains("add a") && out.stdout.contains("add b"),
        "{}",
        out.stdout
    );
    // Unchanged: still on the feature branch.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn revision_land_skips_gates_without_conduit() {
    let (repo, _bare) = repo_with_origin("revskip-land");

    let out = run_engine(repo.path(), &["--revision", "123"], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("revision info unavailable"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "master");
}

#[test]
fn revision_flag_rejects_bad_values() {
    let (repo, _bare) = repo_with_origin("revbad");
    let err = run_engine(repo.path(), &["--preview", "--revision", "nope"], &[]).unwrap_err();
    assert!(
        err.contains("The format of symbol \"nope\" is unrecognized"),
        "{err}"
    );
    let err = land::execute(repo.path(), &args(&["--revision"])).unwrap_err();
    assert!(
        err.contains("Option '--revision' requires a parameter."),
        "{err}"
    );
}

#[test]
fn bad_flags_fail() {
    let repo = Fixture::init_repo("flags");
    let err = land::execute(repo.path(), &args(&["--bogus"])).unwrap_err();
    assert!(
        err.contains("Unknown argument 'bogus'"),
        "unexpected: {err}"
    );
    let err = land::execute(repo.path(), &args(&["--onto"])).unwrap_err();
    assert!(
        err.contains("Option '--onto' requires a parameter."),
        "unexpected: {err}"
    );
    let err = land::execute(repo.path(), &args(&["--push"])).unwrap_err();
    assert!(err.contains("Unknown argument 'push'"), "unexpected: {err}");
    assert_eq!(
        land::run_in(repo.path(), &args(&["--bogus"])),
        ExitCode::FAILURE
    );
}

#[test]
fn land_without_remote_fails_with_guidance() {
    let repo = Fixture::init_repo("noremote");
    make_feature(&repo);
    let err = run_engine(repo.path(), &["--preview"], &[]).unwrap_err();
    assert!(err.contains("No pushable remote"), "unexpected: {err}");
}

/// Canned conduit seam: records calls, replays fixture responses, never
/// touches the network.
#[derive(Default)]
struct Canned {
    calls: RefCell<Vec<(String, serde_json::Value)>>,
    responses: HashMap<String, serde_json::Value>,
}

impl Canned {
    fn with_json(mut self, method: &str, response: serde_json::Value) -> Self {
        self.responses.insert(method.to_string(), response);
        self
    }

    fn call(&self, method: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.calls
            .borrow_mut()
            .push((method.to_string(), params.clone()));
        self.responses
            .get(method)
            .cloned()
            .ok_or_else(|| format!("no canned response for {method}"))
    }

    fn methods(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|(m, _)| m.clone()).collect()
    }
}

fn wire_fixture(name: &str) -> &'static str {
    match name {
        "accepted" => include_str!("fixtures/wire_diff/revisionsearch_accepted.json"),
        "planned" => include_str!("fixtures/wire_diff/revisionsearch_planned.json"),
        "empty" => include_str!("fixtures/wire_diff/revisionsearch_empty.json"),
        "whoami" => include_str!("fixtures/wire_diff/whoami.json"),
        _ => panic!("unknown wire fixture {name}"),
    }
}

fn views_fixture(name: &str) -> Vec<land_checks::RevisionView> {
    let doc: serde_json::Value = serde_json::from_str(wire_fixture(name)).unwrap();
    doc.get("data")
        .and_then(|data| data.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| land_checks::parse_revision(row).unwrap())
        .collect()
}

/// Gate plumbing plus empty parent/build responses.
fn gate_seam(whoami: serde_json::Value) -> Canned {
    Canned::default()
        .with_json("user.whoami", whoami)
        .with_json("edge.search", serde_json::json!({"data": []}))
        .with_json(
            "harbormaster.buildable.search",
            serde_json::json!({"data": []}),
        )
}

fn owner_whoami() -> serde_json::Value {
    serde_json::from_str(wire_fixture("whoami")).unwrap()
}

#[test]
fn gates_over_seam_report_status() {
    let revisions = views_fixture("accepted");
    assert_eq!(revisions.len(), 1);
    let canned = gate_seam(owner_whoami());
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    assert!(
        land::load_gate_findings(&call, &revisions, 2)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        canned.methods(),
        vec![
            "user.whoami",
            "edge.search",
            "harbormaster.buildable.search",
        ]
    );

    let planned = views_fixture("planned");
    let canned = gate_seam(owner_whoami());
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let findings = land::load_gate_findings(&call, &planned, 2).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].prompt, "arc.land.changes-planned");

    let stranger =
        gate_seam(serde_json::json!({"userName": "stranger", "phid": "PHID-USER-stranger"}));
    let call = |method: &str, params: &serde_json::Value| stranger.call(method, params);
    let findings = land::load_gate_findings(&call, &revisions, 2).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].prompt, "arc.land.unauthored");

    let big = gate_seam(owner_whoami());
    let call = |method: &str, params: &serde_json::Value| big.call(method, params);
    let findings = land::load_gate_findings(&call, &revisions, 60).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].prompt, "arc.land.large-working-set");

    let empty = views_fixture("empty");
    assert!(empty.is_empty());
    let canned = gate_seam(owner_whoami());
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    assert!(
        land::load_gate_findings(&call, &empty, 2)
            .unwrap()
            .is_empty()
    );
}
