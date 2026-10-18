//! `land` against local git fixtures through the wired library.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{hg, land, land_checks};

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
    // LAND-6: commits name D1 so the online mock script can associate them.
    repo.git(&["checkout", "-qb", "feature"]);
    repo.commit_file("a.txt", "a\n", "add a\n\nDifferential Revision: D1");
    repo.commit_file("b.txt", "b\n", "add b\n\nDifferential Revision: D1");
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
/// discovery always fails. Only fits flows that fail before Conduit is
/// needed (LAND-6: everything else requires Conduit now).
fn run_engine(dir: &Path, list: &[&str], answers: &[&str]) -> Result<land::Out, String> {
    let _guard = temp_home("engine");
    let mut io = land::Io::canned(answers);
    // T1153 B21/B23: piped runs carry `--` (appended centrally here).
    let mut full = args(list);
    full.push("--".to_string());
    land::execute_with_io(dir, &full, &mut io)
}

/* LAND-6: mock Conduit over HTTP for the online engine flows. */

/// Read one request head (through the blank line) from a mock
/// connection.
fn read_mock_head(stream: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stream.read_exact(&mut byte).is_err() {
            break;
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    head
}

/// `Content-Length` of a mock request head (either case), else 0.
fn mock_content_length(head: &str) -> usize {
    head.lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length: ")
                .or_else(|| line.strip_prefix("content-length: "))
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0)
}

/// Conduit method from a mock request line (`/api/<method>`).
fn mock_method(head: &str) -> String {
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| path.strip_prefix("/api/"))
        .unwrap_or("")
        .to_string()
}

/// Success envelope for a canned method, or the loud
/// `ERR-UNKNOWN-METHOD` envelope.
fn mock_payload(responses: &HashMap<String, serde_json::Value>, method: &str) -> serde_json::Value {
    match responses.get(method) {
        Some(result) => serde_json::json!({
            "error_code": null,
            "error_info": null,
            "result": result,
        }),
        None => serde_json::json!({
            "error_code": "ERR-UNKNOWN-METHOD",
            "error_info": format!("<{method}> no canned response"),
            "result": null,
        }),
    }
}

/// Serve `responses` (Conduit method -> `result` payload) on a loopback
/// mock server. Returns the `--conduit-uri` base plus a receiver collecting
/// one `(method, raw body)` per request. Unknown methods fail loudly with
/// an `ERR-UNKNOWN-METHOD` envelope.
fn serve_methods(
    responses: HashMap<String, serde_json::Value>,
) -> (String, std::sync::mpsc::Receiver<(String, Vec<u8>)>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}/api/");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(_) => break,
            };
            let head = String::from_utf8_lossy(&read_mock_head(&mut stream)).to_string();
            let len = mock_content_length(&head);
            let mut body = vec![0u8; len];
            if stream.read_exact(&mut body).is_err() {
                continue;
            }
            let method = mock_method(&head);
            tx.send((method.clone(), body)).ok();
            let payload = mock_payload(&responses, &method);
            let text = serde_json::to_string(&payload).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                text.len(),
                text
            );
            if stream.write_all(response.as_bytes()).is_err() {
                break;
            }
        }
    });
    (base, rx)
}

/// Standard online script: both feature commits name D1 (accepted,
/// owner-authored), no parents or builds, and a canned landed message.
fn online_responses() -> HashMap<String, serde_json::Value> {
    HashMap::from([
        (
            "differential.revision.search".to_string(),
            serde_json::json!({"data": [{
                "id": 1,
                "phid": "PHID-DREV-1",
                "fields": {
                    "status": {"value": "accepted", "name": "Accepted", "closed": false},
                    "title": "Work 1",
                    "authorPHID": "PHID-USER-owner",
                    "diffPHID": "PHID-DIFF-1",
                },
            }]}),
        ),
        ("differential.query".to_string(), serde_json::json!([])),
        (
            "user.whoami".to_string(),
            serde_json::json!({"phid": "PHID-USER-owner", "userName": "owner"}),
        ),
        ("edge.search".to_string(), serde_json::json!({"data": []})),
        (
            "harbormaster.buildable.search".to_string(),
            serde_json::json!({"data": []}),
        ),
        (
            "differential.getcommitmessage".to_string(),
            serde_json::json!("Land feature\n\nadd a\nadd b\n"),
        ),
        ("user.search".to_string(), serde_json::json!({"data": []})),
    ])
}

/// Install an isolated HOME whose `.arcrc` carries a token for `base`.
fn home_with_token(tag: &str, base: &str) -> common::HomeGuard {
    let home = std::env::temp_dir().join(format!(
        "arc-rs-land-home-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&home).ok();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join(".arcrc"),
        serde_json::json!({"hosts": {base: {"token": "test-token"}}}).to_string(),
    )
    .unwrap();
    with_owned_home(home)
}

/// Run the engine online against `responses`, with scripted answers.
fn run_online_with(
    responses: HashMap<String, serde_json::Value>,
    dir: &Path,
    list: &[&str],
    answers: &[&str],
) -> Result<land::Out, String> {
    let (base, _seen) = serve_methods(responses);
    let _guard = home_with_token("engine", &base);
    let mut full: Vec<String> = list.iter().map(|item| (*item).to_string()).collect();
    full.push("--conduit-uri".to_string());
    full.push(base);
    // T1153 B21/B23: piped runs carry `--` (appended centrally here).
    full.push("--".to_string());
    let mut io = land::Io::canned(answers);
    land::execute_with_io(dir, &full, &mut io)
}

/// Run the engine online against the standard script.
fn run_online(dir: &Path, list: &[&str], answers: &[&str]) -> Result<land::Out, String> {
    run_online_with(online_responses(), dir, list, answers)
}

#[test]
fn module_name_is_land() {
    assert_eq!(land::NAME, "land");
}

#[test]
fn run_uses_current_dir() {
    // T1153 B21/B23: piped runs carry `--`.
    // LAND-6: `--preview` needs Conduit too, so this contract runs online.
    let (repo, _bare) = repo_with_origin("run-contract");
    // CWD-then-ENV lock order (see `common::CwdGuard`): the CWD guard
    // must be declared before the HOME guard.
    let _cwd = common::with_current_dir(repo.path());
    let (base, _seen) = serve_methods(online_responses());
    let _guard = home_with_token("run-contract", &base);
    let code = land::run(&args(&["--preview", "--conduit-uri", &base, "--"]));
    assert_eq!(code, ExitCode::SUCCESS);
    // Unchanged: still on the feature branch.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn help_succeeds_and_prints_usage() {
    // T1153 B21/B23: piped runs carry `--`; `--help` asserts the generated `LAND WORKFLOW` help.
    let repo = Fixture::init_repo("help");
    let out = land::execute(repo.path(), &args(&["--help", "--"])).unwrap();
    // T1153 R10: `--help` renders the generated workflow help.
    assert!(out.stdout.contains("LAND WORKFLOW"), "{}", out.stdout);
    assert_eq!(
        land::run_in(repo.path(), &args(&["--help", "--"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn preview_lists_commits_without_changing_anything() {
    // LAND-6: preview resolves revisions online; the offline note is gone.
    let (repo, _bare) = repo_with_origin("preview");
    let master_before = repo.git(&["rev-parse", "master"]);

    let out = run_online(repo.path(), &["--preview"], &[]).unwrap();
    assert!(out.stderr.contains("** STRATEGY **"), "{}", out.stderr);
    assert!(out.stderr.contains("** ONTO TARGET **"), "{}", out.stderr);
    assert!(out.stderr.contains("** LANDING **"), "{}", out.stderr);
    assert!(out.stderr.contains("** PREVIEW **"), "{}", out.stderr);
    assert!(
        !out.stderr.contains("revision info unavailable"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.contains("D1"), "{}", out.stdout);
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
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["branch", "--set-upstream-to=origin/master", "master"]);

    let out = run_online(repo.path(), &[], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );

    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "master");
    assert!(out.stderr.contains("** UPDATE **"), "{}", out.stderr);
    assert!(out.stderr.contains("** CHECKOUT **"), "{}", out.stderr);
    // The squash lands on the remote and updates the tracking local ref.
    let body = bare.git(&["log", "-1", "--format=%B", "refs/heads/master"]);
    assert!(body.contains("Land feature"), "unexpected message: {body}");
    assert!(
        body.contains("add a") && body.contains("add b"),
        "missing commits: {body}"
    );
    assert_eq!(bare.git(&["rev-list", "--count", "refs/heads/master"]), "2");
    assert_eq!(repo.git(&["rev-list", "--count", "master"]), "2");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );
    // Landed branches are pruned by default.
    assert!(repo.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn land_with_hold_skips_push() {
    let (repo, bare) = repo_with_origin("hold");
    let remote_before = bare.git(&["rev-parse", "refs/heads/master"]);

    let out = run_online(repo.path(), &["--hold"], &["y", "y"]).unwrap();
    assert!(out.stdout.contains("HOLD CHANGES"), "{}", out.stdout);
    assert!(out.stdout.contains("git push --"), "{}", out.stdout);

    assert_eq!(bare.git(&["rev-parse", "refs/heads/master"]), remote_before);
    // Hold merges locally but leaves branches alone.
    assert!(!repo.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn keep_branches_preserves_feature() {
    let (repo, _bare) = repo_with_origin("keep");
    let out = run_online(repo.path(), &["--keep-branches"], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );
    assert!(!repo.git(&["branch", "--list", "feature"]).is_empty());
    assert!(out.stderr.contains("** RETURN **"), "{}", out.stderr);
}

#[test]
fn dirty_tree_stashes_and_restores() {
    let (repo, _bare) = repo_with_origin("dirty");
    repo.write("a.txt", "uncommitted\n");

    run_online(repo.path(), &["--preview"], &[]).expect("preview allows dirt");

    // Accepting the stash prompt lands, then applies the stash back.
    let out = run_online(repo.path(), &[], &["y", "y"]).unwrap();
    assert!(
        out.stdout.contains("** UNCOMMITTED CHANGES **"),
        "{}",
        out.stdout
    );
    assert!(out.stderr.contains("** SAVE STASH **"), "{}", out.stderr);
    // Nothing tracks the pushed ref, so the stash applies onto the detached
    // landed commit.
    assert!(out.stderr.contains("** DETACHED HEAD **"), "{}", out.stderr);
    assert!(out.stderr.contains("** LOAD STASH **"), "{}", out.stderr);
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "uncommitted\n"
    );
}

#[test]
fn dirty_tree_decline_aborts() {
    let (repo, _bare) = repo_with_origin("dirty-decline");
    repo.write("a.txt", "uncommitted\n");
    let err = run_online(repo.path(), &[], &["y", "n"]).unwrap_err();
    assert_eq!(err, "User aborted the workflow.");
    // Nothing landed: the feature branch still exists.
    assert!(!repo.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn detached_head_lands_head() {
    let (repo, _bare) = repo_with_origin("detached");
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);
    let out = run_online(repo.path(), &["--preview"], &[]).unwrap();
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
    let err = land::execute(empty.path(), &args(&["--"])).unwrap_err();
    assert_eq!(err, land::NOT_GIT_OR_HG);
    assert_eq!(
        land::run_in(empty.path(), &args(&["--"])),
        ExitCode::FAILURE
    );
}

#[test]
fn onto_flag_targets_another_branch() {
    let (repo, bare) = repo_with_origin("onto");
    repo.git(&["checkout", "-q", "master"]);
    repo.git(&["branch", "stable"]);
    repo.git(&["push", "-q", "origin", "stable"]);
    repo.git(&["checkout", "-q", "feature"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["branch", "--set-upstream-to=origin/stable", "stable"]);
    let master_before = repo.git(&["rev-parse", "master"]);

    let out = run_online(repo.path(), &["--onto", "stable"], &["y", "y"]).unwrap();
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );

    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "stable");
    assert!(out.stderr.contains("** UPDATE **"), "{}", out.stderr);
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
    // LAND-6: into-empty ranges include the unassociated base commit, so
    // `--revision` forces it under D1 (PHP errors otherwise, like the port).
    let (repo, bare) = repo_with_origin("onto-create");
    let out = run_online(
        repo.path(),
        &["--onto", "fresh", "--revision", "D1"],
        &["y", "y", "y"],
    )
    .unwrap();
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
    let out = run_online(repo.path(), &["--preview", "feature"], &[]).unwrap();
    assert!(
        out.stdout.contains("add a") && out.stdout.contains("add b"),
        "{}",
        out.stdout
    );
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "master");
}

#[test]
fn revision_preview_resolves_forced_revision_online() {
    // LAND-6 (ex `revision_preview_skips_gates_without_conduit`): the
    // forced revision resolves through Conduit; nothing is skipped.
    let (repo, _bare) = repo_with_origin("revskip-preview");

    let out = run_online(repo.path(), &["--preview", "--revision", "D1"], &[]).unwrap();
    assert!(
        !out.stderr.contains("revision info unavailable"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("** PREVIEW **"), "{}", out.stderr);
    assert!(out.stdout.contains("D1"), "{}", out.stdout);
    assert!(
        out.stdout.contains("add a") && out.stdout.contains("add b"),
        "{}",
        out.stdout
    );
    // Unchanged: still on the feature branch.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "feature");
}

#[test]
fn revision_land_runs_gates_online() {
    // LAND-6 (ex `revision_land_skips_gates_without_conduit`): landing
    // with `--revision` runs discovery and gates online.
    let (repo, _bare) = repo_with_origin("revskip-land");
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["branch", "--set-upstream-to=origin/master", "master"]);

    let out = run_online(repo.path(), &["--revision", "1"], &["y", "y"]).unwrap();
    assert!(
        !out.stderr.contains("revision info unavailable"),
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
fn land11_message_failure_aborts_before_merge() {
    // The landed message loads before any merge runs: a fetch failure
    // aborts with the target untouched.
    let (repo, _bare) = repo_with_origin("land11-msgfail");
    let master_before = repo.git(&["rev-parse", "master"]);
    let mut responses = online_responses();
    responses.remove("differential.getcommitmessage");
    let err = run_online_with(responses, repo.path(), &[], &["y", "y"]).unwrap_err();
    assert!(err.contains("no canned response"), "{err}");
    assert_eq!(repo.git(&["rev-parse", "master"]), master_before);
}

#[test]
fn land31_unauthored_banner_shows_monograms() {
    // The `NOT REVISION AUTHOR` gate names the viewer and the author by
    // `@monogram`, loaded over `user.search`.
    let (repo, _bare) = repo_with_origin("land31-author");
    let mut responses = online_responses();
    responses.insert(
        "differential.revision.search".to_string(),
        serde_json::json!({"data": [{
            "id": 1,
            "phid": "PHID-DREV-1",
            "fields": {
                "status": {"value": "accepted", "name": "Accepted", "closed": false},
                "title": "Work 1",
                "authorPHID": "PHID-USER-bob",
                "diffPHID": "PHID-DIFF-1",
            },
        }]}),
    );
    responses.insert(
        "user.search".to_string(),
        serde_json::json!({"data": [
            {"phid": "PHID-USER-bob", "fields": {"username": "bob"}},
        ]}),
    );
    let out = run_online_with(responses, repo.path(), &[], &["y", "y"]).unwrap();
    assert!(
        out.stdout.contains("(\"@owner\") are not the author"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("Author: @bob"), "{}", out.stdout);
    assert!(
        out.stderr.contains("** DONE ** Landed changes."),
        "{}",
        out.stderr
    );
}

#[test]
fn land6_land_without_conduit_fails() {
    // LAND-6: without Conduit the engine fails instead of landing an
    // unassociated offline set.
    let (repo, _bare) = repo_with_origin("land6-offline");
    // Either discovery error fits (an ambient `ARC_CONDUIT_URI` shifts
    // the failure from URI discovery to the token lookup).
    let err = run_engine(repo.path(), &["--preview"], &["y", "y"]).unwrap_err();
    assert!(
        err.contains("no conduit URI") || err.contains("no token for"),
        "{err}"
    );
    let err = run_engine(repo.path(), &[], &["y", "y"]).unwrap_err();
    assert!(
        err.contains("no conduit URI") || err.contains("no token for"),
        "{err}"
    );
}

#[test]
fn revision_flag_rejects_bad_values() {
    // T1153 B21/B23: missing-param scan errors stay bare (they report before the terminator check).
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
    // T1153 B21/B23: scan errors stay bare (they report before the terminator check).
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

/* L10: hg land flows through HgCommand with a fake binary. */

/// Scratch dir with an `.hg` marker for fake-hg tests.
fn hg_fixture(name: &str) -> Fixture {
    let repo = Fixture::named(name);
    std::fs::create_dir_all(repo.path().join(".hg")).unwrap();
    repo
}

/// Fake `hg` for land tests: a `.cmd` forwarder driving the `.ps1` fake on
/// Windows (PowerShell `-File` binding rejects the bare `-` of `--logfile -`,
/// so the forwarder re-quotes that one token; quoted template argv keeps its
/// quotes through `%1`, so the commit-list `|` survives cmd.exe), shell
/// elsewhere. Mirrors the D13 fake-binary pattern.
fn fake_hg_land_command(dir: &Path) -> hg::HgCommand {
    #[cfg(windows)]
    {
        let script = dir.join("fakehg-land.ps1");
        std::fs::write(&script, include_str!("fixtures/vcs/fakehg-land.ps1")).unwrap();
        let forward = dir.join("fakehg-land.cmd");
        std::fs::write(&forward, include_str!("fixtures/vcs/fakehg-land.cmd")).unwrap();
        // `cmd /d /c call ...`: `/d` skips AutoRun (its output would pollute
        // the fake's stdout); the leading `call` keeps the `/c` string from
        // starting with a quote, defeating cmd's quote-strip rule.
        hg::HgCommand::custom("cmd", &["/d", "/c", "call", &forward.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        let script = dir.join("fakehg-land.sh");
        std::fs::write(&script, include_str!("fixtures/vcs/fakehg-land.sh")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        hg::HgCommand::custom(&script.to_string_lossy(), &[])
    }
}

fn fake_log(repo: &Fixture) -> String {
    std::fs::read_to_string(repo.path().join(".fakehg.log")).unwrap_or_default()
}

const FAKE_NODE: &str = "0123456789abcdef0123456789abcdef01234567";
const FAKE_TIP: &str = "89abcdef0123456789abcdef0123456789abcdef01";

#[test]
fn hg_selection_flow_through_fake_binary() {
    let repo = hg_fixture("hg-select");
    let hg_cmd = fake_hg_land_command(repo.path());
    let mut io = land::Io::canned(&[]);

    let symbols = land::hg_default_symbols_with(repo.path(), &hg_cmd, &mut io).unwrap();
    assert_eq!(symbols, vec!["feature".to_string()]);
    assert!(io.out.stderr.contains("Landing the active bookmark"));

    let resolved = land::hg_resolve_symbols_with(repo.path(), &hg_cmd, &mut io, &symbols).unwrap();
    assert_eq!(resolved, vec![FAKE_NODE.to_string()]);

    let options = land::Options::default();
    let remote = land::hg_select_onto_remote_with(repo.path(), &hg_cmd, &mut io, &options).unwrap();
    assert_eq!(remote, "default");

    let refs = land::hg_select_onto_refs(repo.path(), &mut io, &options).unwrap();
    assert_eq!(refs, vec!["default".to_string()]);

    // `default` exists remotely and is not a local branch: no prompt.
    let marker =
        land::hg_confirm_onto_refs_with(repo.path(), &hg_cmd, &mut io, &remote, &refs, false)
            .unwrap();
    assert_eq!(marker, None);

    let into = land::hg_select_into_commit_with(
        repo.path(),
        &hg_cmd,
        &mut io,
        &land::IntoState::Remote {
            remote: "default".to_string(),
            name: "default".to_string(),
        },
        None,
    )
    .unwrap();
    assert_eq!(into.as_deref(), Some(FAKE_NODE));

    repo.write(
        ".fakehg-log-commits",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb -add a\n",
    );
    let symbols = vec![land::GitSymbol {
        symbol: "feature".to_string(),
        commit: FAKE_NODE.to_string(),
    }];
    let commits =
        land::hg_select_commits_with(repo.path(), &hg_cmd, &symbols, into.as_deref()).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "add a");
    assert_eq!(commits[0].direct_symbols, vec!["feature".to_string()]);

    let log = fake_log(&repo);
    assert!(log.contains("bookmarks"), "{log}");
    assert!(log.contains("paths default"), "{log}");
    assert!(log.contains("pull --rev default default"), "{log}");
}

#[test]
fn hg_land_loop_lands_through_fake_binary() {
    let repo = hg_fixture("hg-land");
    let hg_cmd = fake_hg_land_command(repo.path());
    let options = land::Options::default();
    let commits = vec![land::LandCommit {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        parents: vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()],
        summary: "add a".to_string(),
        direct_symbols: vec!["feature".to_string()],
        indirect_symbols: vec!["feature".to_string()],
        explicit: None,
        related: Vec::new(),
        is_head: true,
        is_implicit: false,
    }];
    let sets = vec![land::CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = land::Io::canned(&[]);
    land::land_hg_with(
        repo.path(),
        &hg_cmd,
        &mut io,
        &options,
        true,
        &["feature".to_string()],
        "default",
        &["feature".to_string()],
        Some(FAKE_NODE),
        None,
        &sets,
        &commits,
        None,
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("** DONE ** Landed changes."),
        "{:?}",
        io.out.stderr
    );
    let log = fake_log(&repo);
    // The bookmark moves onto the rebased tip before the push.
    assert!(
        log.contains(&format!("bookmark --force --rev {FAKE_TIP} -- feature")),
        "{log}"
    );
    for command in [
        "update --rev",
        "rebase --dest",
        "push --bookmark feature",
        "pull --bookmark feature",
        "strip --rev",
        "update --",
    ] {
        assert!(log.contains(command), "{command} missing from:\n{log}");
    }
}

#[test]
fn hg_land_hold_prints_restore_commands() {
    let repo = hg_fixture("hg-hold");
    let hg_cmd = fake_hg_land_command(repo.path());
    let options = land::Options {
        hold: true,
        ..land::Options::default()
    };
    let commits = vec![land::LandCommit {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        parents: Vec::new(),
        summary: "add a".to_string(),
        direct_symbols: vec!["feature".to_string()],
        indirect_symbols: vec!["feature".to_string()],
        explicit: None,
        related: Vec::new(),
        is_head: true,
        is_implicit: false,
    }];
    let sets = vec![land::CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = land::Io::canned(&[]);
    land::land_hg_with(
        repo.path(),
        &hg_cmd,
        &mut io,
        &options,
        true,
        &["feature".to_string()],
        "default",
        &["feature".to_string()],
        Some(FAKE_NODE),
        None,
        &sets,
        &commits,
        None,
    )
    .unwrap();
    assert!(
        io.out.stdout.contains("** HOLD CHANGES **"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("hg push --bookmark feature"),
        "{}",
        io.out.stdout
    );
    // Restore commands come from the saved state: the working-copy
    // parent, the branch, and the active bookmark.
    assert!(
        io.out.stdout.contains("hg update -- 0123456789ab"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("hg branch --force -- default"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("hg bookmark --force -- feature"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn hg_stash_flow_shelves_through_fake_binary() {
    let repo = hg_fixture("hg-stash");
    repo.write(".fakehg-status", "M file.txt\n? new.txt\n");
    let hg_cmd = fake_hg_land_command(repo.path());
    let mut io = land::Io::canned(&["y"]);
    let state = land::save_local_state_hg_with(repo.path(), &hg_cmd, &mut io).unwrap();
    assert_eq!(state.local_commit, FAKE_NODE);
    assert_eq!(state.local_branch, "default");
    assert_eq!(state.local_bookmark.as_deref(), Some("feature"));
    let stash = state.stash.clone().expect("shelved");
    assert!(stash.starts_with("arc-"), "{stash}");
    assert_eq!(stash.len(), 16);
    assert!(
        io.out.stdout.contains("** UNCOMMITTED CHANGES **"),
        "{}",
        io.out.stdout
    );
    assert!(io.out.stderr.contains("** SHELVE **"), "{}", io.out.stderr);
    land::discard_local_state_hg_with(repo.path(), &hg_cmd, &mut io, &state).unwrap();
    assert!(
        io.out.stderr.contains("** UNSHELVE **"),
        "{}",
        io.out.stderr
    );
    let log = fake_log(&repo);
    assert!(
        log.contains(&format!("shelve --unknown --name {stash} --")),
        "{log}"
    );
    assert!(
        log.contains(&format!("unshelve --keep --name {stash} --")),
        "{log}"
    );
    assert!(
        log.contains(&format!("shelve --delete {stash} --")),
        "{log}"
    );
}

#[test]
fn hg_shelve_unsupported_fails() {
    let repo = hg_fixture("hg-no-shelve");
    repo.write(".fakehg-status", "M file.txt\n");
    repo.write(".fakehg-fail-shelve", "");
    let hg_cmd = fake_hg_land_command(repo.path());
    let mut io = land::Io::canned(&["y"]);
    let err = land::save_local_state_hg_with(repo.path(), &hg_cmd, &mut io).unwrap_err();
    // LAND-35: the verbatim PHP usage error.
    assert_eq!(
        err,
        "You can not continue with uncommitted changes. Commit or discard them before proceeding."
    );
}

#[test]
fn hg_merge_failure_restores_saved_state() {
    let repo = hg_fixture("hg-restore");
    repo.write(".fakehg-fail-rebase", "");
    let hg_cmd = fake_hg_land_command(repo.path());
    let options = land::Options::default();
    let commits = vec![land::LandCommit {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        parents: Vec::new(),
        summary: "add a".to_string(),
        direct_symbols: vec!["feature".to_string()],
        indirect_symbols: vec!["feature".to_string()],
        explicit: None,
        related: Vec::new(),
        is_head: true,
        is_implicit: false,
    }];
    let sets = vec![land::CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = land::Io::canned(&[]);
    let err = land::land_hg_with(
        repo.path(),
        &hg_cmd,
        &mut io,
        &options,
        true,
        &["feature".to_string()],
        "default",
        &["feature".to_string()],
        Some(FAKE_NODE),
        None,
        &sets,
        &commits,
        None,
    )
    .unwrap_err();
    assert_eq!(err, "rebase failed");
    assert!(
        io.out.stderr.contains("** LOAD STATE **"),
        "{:?}",
        io.out.stderr
    );
}

#[test]
fn svn_working_copy_is_rejected_like_any_non_git_hg_tree() {
    // `arc land` supports git and Mercurial only; Subversion working
    // copies fail detection with the shared error (PHP parity), so no
    // SvnCommand path exists for land.
    let svn = Fixture::named("svn-tree");
    std::fs::create_dir_all(svn.path().join(".svn")).unwrap();
    let err = land::execute(svn.path(), &args(&["--"])).unwrap_err();
    assert_eq!(err, land::NOT_GIT_OR_HG);
}
