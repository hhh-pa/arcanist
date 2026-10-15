//! Integration tests for the revmeta bundle's conduit commands
//! (close-revision, tasks, todo, list) through the wired library.
//! Conduit is a 127.0.0.1 mock only.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;

use arc_rs::{close_revision, conduit, config, git, list, tasks, todo, upload};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Serve `expected` requests, dispatching canned envelopes by conduit method.
/// Returns the base URI and the received (method, urlencoded body) pairs.
fn serve(
    routes: HashMap<String, String>,
    expected: usize,
) -> (String, mpsc::Receiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() || head.len() > 1_000_000 {
                    return;
                }
                head.push(byte[0]);
            }
            let head_text = String::from_utf8_lossy(&head).to_string();
            let len: usize = head_text
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(key, value)| {
                        if key.eq_ignore_ascii_case("content-length") {
                            value.trim().parse().ok()
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let request_line = head_text.lines().next().unwrap_or("").to_string();
            let method = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .strip_prefix("/api/")
                .unwrap_or("")
                .to_string();
            let canned = routes.get(&method).cloned().unwrap_or_else(|| {
                r#"{"result": null, "error_code": "ERR-NO-ROUTE", "error_info": "no route"}"#
                    .to_string()
            });
            tx.send((method, String::from_utf8_lossy(&body).to_string()))
                .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (base, rx)
}

fn recv(rx: &mpsc::Receiver<(String, String)>) -> (String, String) {
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
}

fn envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

fn client_for(base: &str) -> conduit::Client {
    conduit::Client::new(base, "test-token").unwrap()
}

fn options_close(revision: &str) -> close_revision::Options {
    close_revision::Options {
        revision: revision.to_string(),
        finalize: false,
        quiet: false,
        json: false,
        force: false,
        conduit_uri: None,
        repository_phid: None,
    }
}

fn options_tasks() -> tasks::Options {
    tasks::Options {
        status: None,
        owner: None,
        order: None,
        limit: None,
        unassigned: false,
        json: false,
        conduit_uri: None,
    }
}

fn options_todo(summary: &[&str]) -> todo::Options {
    todo::Options {
        summary: args(summary),
        cc: Vec::new(),
        projects: Vec::new(),
        browse: false,
        json: false,
        conduit_uri: None,
    }
}

fn options_list() -> list::Options {
    list::Options {
        json: false,
        conduit_uri: None,
    }
}

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-revmeta-{name}-{}-{}",
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn accepted_revision(author: &str) -> String {
    format!(
        r#"[{{"id": 123, "title": "Fix the thing", "status": 2, "statusName": "Accepted", "authorPHID": "{author}", "commits": []}}]"#,
    )
}

// close-revision -----------------------------------------------------------

#[test]
fn close_revision_name_matches_arc() {
    assert_eq!(close_revision::NAME, "close-revision");
}

#[test]
fn close_revision_runs_help_and_usage_paths() {
    assert_eq!(close_revision::run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(close_revision::run(&args(&[])), ExitCode::FAILURE);
    assert_eq!(close_revision::run(&args(&["D1", "D2"])), ExitCode::FAILURE);
    assert_eq!(
        close_revision::run(&args(&["--bogus", "D1"])),
        ExitCode::FAILURE
    );
}

#[test]
fn closes_accepted_own_revision() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-1")),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1", "userName": "alice"}"#),
    );
    routes.insert("differential.close".to_string(), envelope("null"));
    let (base, seen) = serve(routes, 3);
    let client = client_for(&base);

    let never = |_: &str| panic!("own revision must not prompt");
    let report = close_revision::execute(&client, &options_close("D123"), &never).unwrap();
    // PHP writeInfo/writeOkay go to stderr; only the push hint is stdout.
    assert!(
        report.stderr.contains("Closing revision"),
        "{}",
        report.stderr
    );
    assert!(
        report.stderr.contains("D123 Fix the thing"),
        "{}",
        report.stderr
    );
    assert!(
        report.stderr.contains("Done, closed revision."),
        "{}",
        report.stderr
    );
    assert!(
        report
            .stdout
            .contains("You may now push this commit upstream"),
        "{}",
        report.stdout
    );

    let mut methods = vec![recv(&seen).0, recv(&seen).0, recv(&seen).0];
    methods.sort();
    assert_eq!(
        methods,
        vec!["differential.close", "differential.query", "user.whoami"]
    );
}

#[test]
fn close_sends_revision_id_to_differential_close() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-1")),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.close".to_string(), envelope("null"));
    let (base, seen) = serve(routes, 3);
    let client = client_for(&base);

    close_revision::execute(&client, &options_close("123"), &|_| Ok(true)).unwrap();
    let mut calls = [recv(&seen), recv(&seen), recv(&seen)];
    calls.sort();
    let close = calls
        .iter()
        .find(|(method, _)| method == "differential.close")
        .unwrap();
    assert!(close.1.contains("revisionID"), "{}", close.1);
    assert!(close.1.contains("123"), "{}", close.1);
    let query = calls
        .iter()
        .find(|(method, _)| method == "differential.query")
        .unwrap();
    assert!(query.1.contains("123"), "{}", query.1);
}

#[test]
fn refuses_non_accepted_revision() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(r#"[{"id": 9, "title": "WIP", "status": 0, "statusName": "Needs Review", "authorPHID": "PHID-USER-1", "commits": []}]"#),
    );
    let (base, seen) = serve(routes, 1);
    let client = client_for(&base);

    let err = close_revision::execute(&client, &options_close("D9"), &|_| Ok(true)).unwrap_err();
    assert!(err.contains("can not be closed"), "{err}");
    assert!(err.contains("accepted"), "{err}");
    assert_eq!(recv(&seen).0, "differential.query");
}

#[test]
fn missing_revision_errors_but_finalize_leaves_it_open() {
    let mut routes = HashMap::new();
    routes.insert("differential.query".to_string(), envelope("[]"));
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let err = close_revision::execute(&client, &options_close("D404"), &|_| Ok(true)).unwrap_err();
    assert!(err.contains("does not exist"), "{err}");

    let mut finalize = options_close("D404");
    finalize.finalize = true;
    let report = close_revision::execute(&client, &finalize, &|_| Ok(true)).unwrap();
    assert_eq!(report.stdout, "Done.\n");
    assert_eq!(recv(&seen).0, "differential.query");
    assert_eq!(recv(&seen).0, "differential.query");
}

#[test]
fn finalize_warns_and_skips_unaccepted_revision() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(r#"[{"id": 9, "title": "WIP", "status": 0, "statusName": "Needs Review", "authorPHID": "PHID-USER-1", "commits": []}]"#),
    );
    let (base, seen) = serve(routes, 1);
    let client = client_for(&base);

    let mut options = options_close("D9");
    options.finalize = true;
    let report = close_revision::execute(&client, &options, &|_| Ok(true)).unwrap();
    assert!(report.stderr.contains("OPEN REVISION"), "{}", report.stderr);
    assert!(report.stderr.contains("left open"), "{}", report.stderr);
    assert!(
        !report.stdout.contains("Closing revision"),
        "{}",
        report.stdout
    );
    assert_eq!(recv(&seen).0, "differential.query");
}

#[test]
fn finalize_skips_tracked_repository_silently() {
    // A `--finalize` close in a tracked working copy (repository PHID
    // identified) leaves the revision alone with no close call and no
    // warning, mirroring `getRepositoryPHID()`.
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-1")),
    );
    let (base, seen) = serve(routes, 1);
    let client = client_for(&base);

    let mut options = options_close("D123");
    options.finalize = true;
    options.repository_phid = Some("PHID-REPO-1".to_string());
    let report = close_revision::execute(&client, &options, &|_| {
        panic!("tracked finalize must not prompt")
    })
    .unwrap();
    assert!(
        !report.stdout.contains("Closing revision"),
        "{}",
        report.stdout
    );
    assert!(report.stderr.is_empty(), "{}", report.stderr);
    assert_eq!(recv(&seen).0, "differential.query");
}

#[test]
fn resolve_repository_phid_queries_by_callsign() {
    let mut routes = HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        envelope(r#"[{"phid": "PHID-REPO-9", "callsign": "X"}]"#),
    );
    let (base, seen) = serve(routes, 1);
    let client = client_for(&base);

    let fx = Fixture::named("close-repo-phid");
    std::fs::write(fx.dir.join(".arcconfig"), r#"{"repository.callsign": "X"}"#).unwrap();
    assert_eq!(
        close_revision::resolve_repository_phid(&client, &fx.dir).unwrap(),
        Some("PHID-REPO-9".to_string())
    );
    let query = recv(&seen);
    assert_eq!(query.0, "repository.query");
    assert!(query.1.contains("callsigns"), "{}", query.1);
}

#[test]
fn resolve_repository_phid_treats_old_servers_as_untracked() {
    let mut routes = HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        r#"{"result": null, "error_code": "ERR-CONDUIT-CALL", "error_info": "old"}"#.to_string(),
    );
    let (base, _seen) = serve(routes, 1);
    let client = client_for(&base);

    let fx = Fixture::named("close-repo-old");
    std::fs::write(fx.dir.join(".arcconfig"), r#"{"repository.callsign": "X"}"#).unwrap();
    assert_eq!(
        close_revision::resolve_repository_phid(&client, &fx.dir).unwrap(),
        None
    );
}

#[test]
fn prompts_before_closing_someone_elses_revision() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-OTHER")),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.close".to_string(), envelope("null"));
    // Abort path: query + whoami, then the close path replays query + whoami
    // + close on a second server below.
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let prompted = std::cell::RefCell::new(Vec::new());
    let err = close_revision::execute(&client, &options_close("D123"), &|prompt| {
        prompted.borrow_mut().push(prompt.to_string());
        Ok(false)
    })
    .unwrap_err();
    assert_eq!(err, "User aborted the workflow.");
    let prompted = prompted.borrow();
    assert_eq!(prompted.len(), 1);
    assert!(prompted[0].contains("not the author"), "{}", prompted[0]);
    assert_eq!(recv(&seen).0, "differential.query");
    assert_eq!(recv(&seen).0, "user.whoami");

    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-OTHER")),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.close".to_string(), envelope("null"));
    let (base, seen) = serve(routes, 3);
    let client = client_for(&base);
    let report = close_revision::execute(&client, &options_close("D123"), &|_| Ok(true)).unwrap();
    assert!(
        report.stderr.contains("Done, closed revision."),
        "{}",
        report.stderr
    );
    assert_eq!(recv(&seen).0, "differential.query");
    assert_eq!(recv(&seen).0, "user.whoami");
    assert_eq!(recv(&seen).0, "differential.close");
}

#[test]
fn force_skips_the_author_prompt() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-OTHER")),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.close".to_string(), envelope("null"));
    let (base, seen) = serve(routes, 3);
    let client = client_for(&base);

    let mut options = options_close("D123");
    options.force = true;
    let report =
        close_revision::execute(&client, &options, &|_| panic!("force must not prompt")).unwrap();
    assert!(
        report.stderr.contains("Done, closed revision."),
        "{}",
        report.stderr
    );
    assert_eq!(recv(&seen).0, "differential.query");
    assert_eq!(recv(&seen).0, "user.whoami");
    assert_eq!(recv(&seen).0, "differential.close");
}

#[test]
fn close_json_reports_result_on_stdout() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        envelope(&accepted_revision("PHID-USER-1")),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.close".to_string(), envelope("null"));
    let (base, _seen) = serve(routes, 3);
    let client = client_for(&base);

    let mut options = options_close("D123");
    options.json = true;
    let report = close_revision::execute(&client, &options, &|_| Ok(true)).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&report.stdout).unwrap();
    assert_eq!(
        doc,
        serde_json::json!({
            "revision": "D123",
            "closed": true,
            "status": 2,
            "statusName": "Accepted",
        })
    );
    assert!(
        report.stderr.contains("Closing revision"),
        "{}",
        report.stderr
    );
}

// tasks --------------------------------------------------------------------

#[test]
fn tasks_name_matches_arc() {
    assert_eq!(tasks::NAME, "tasks");
}

#[test]
fn tasks_runs_help_and_usage_paths() {
    assert_eq!(tasks::run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(tasks::run(&args(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(tasks::run(&args(&["positional"])), ExitCode::FAILURE);
    assert_eq!(
        tasks::run(&args(&["--owner", "alice", "--unassigned"])),
        ExitCode::FAILURE
    );
}

#[test]
fn lists_own_open_tasks_from_fixture() {
    let canned = std::fs::read_to_string("tests/fixtures/revmeta/maniphest_query.json").unwrap();
    let mut routes = HashMap::new();
    routes.insert("maniphest.query".to_string(), canned);
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let out = tasks::execute(&client, &options_tasks()).unwrap();
    assert!(out.contains("T7"), "{out}");
    assert!(out.contains("Fix the login redirect"), "{out}");
    assert!(out.contains("T123"), "{out}");
    assert!(out.contains("Ship the widget"), "{out}");
    assert!(out.contains("Normal"), "{out}");
    assert!(out.contains("High"), "{out}");

    let first = recv(&seen);
    let second = recv(&seen);
    assert_eq!(first.0, "user.whoami");
    assert_eq!(second.0, "maniphest.query");
    assert!(second.1.contains("PHID-USER-1"), "{}", second.1);
    assert!(second.1.contains("status-open"), "{}", second.1);
    assert!(second.1.contains("order-priority"), "{}", second.1);
}

#[test]
fn tasks_reports_no_tasks_found() {
    let mut routes = HashMap::new();
    routes.insert("maniphest.query".to_string(), envelope("[]"));
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    assert_eq!(
        tasks::execute(&client, &options_tasks()).unwrap(),
        "No tasks found.\n"
    );
}

#[test]
fn tasks_all_and_unassigned_omit_owner_filter() {
    for options in [
        {
            let mut o = options_tasks();
            o.owner = Some("@all".to_string());
            o
        },
        {
            let mut o = options_tasks();
            o.unassigned = true;
            o
        },
    ] {
        let mut routes = HashMap::new();
        routes.insert(
            "maniphest.query".to_string(),
            envelope(r#"[{"id": 1, "title": "t", "priority": "Normal", "statusName": "Open", "isClosed": false}]"#),
        );
        let (base, seen) = serve(routes, 1);
        let client = client_for(&base);
        let out = tasks::execute(&client, &options).unwrap();
        assert!(out.contains("T1"), "{out}");
        let (method, body) = recv(&seen);
        assert_eq!(method, "maniphest.query");
        assert!(!body.contains("ownerPHIDs"), "{body}");
    }
}

#[test]
fn tasks_resolves_owner_username() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.query".to_string(),
        envelope(r#"[{"phid": "PHID-USER-9", "userName": "bob"}]"#),
    );
    routes.insert(
        "maniphest.query".to_string(),
        envelope(r#"[{"id": 3, "title": "bob task", "priority": "Low", "statusName": "Open", "isClosed": false}]"#),
    );
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_tasks();
    options.owner = Some("bob".to_string());
    let out = tasks::execute(&client, &options).unwrap();
    assert!(out.contains("T3"), "{out}");
    let lookup = recv(&seen);
    assert_eq!(lookup.0, "user.query");
    assert!(lookup.1.contains("bob"), "{}", lookup.1);
    let query = recv(&seen);
    assert_eq!(query.0, "maniphest.query");
    assert!(query.1.contains("PHID-USER-9"), "{}", query.1);
}

#[test]
fn tasks_passes_status_order_and_limit() {
    let mut routes = HashMap::new();
    routes.insert(
        "maniphest.query".to_string(),
        envelope(r#"[{"id": 1, "title": "t"}]"#),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_tasks();
    options.status = Some("closed".to_string());
    options.order = Some("created".to_string());
    options.limit = Some(5);
    tasks::execute(&client, &options).unwrap();
    assert_eq!(recv(&seen).0, "user.whoami");
    let query = recv(&seen);
    assert!(query.1.contains("status-closed"), "{}", query.1);
    assert!(query.1.contains("order-created"), "{}", query.1);
    assert!(query.1.contains("limit"), "{}", query.1);
}

#[test]
fn tasks_json_lists_tasks() {
    let mut routes = HashMap::new();
    routes.insert(
        "maniphest.query".to_string(),
        envelope(r#"[{"id": 7, "title": "T", "priority": "Normal", "statusName": "Open", "isClosed": false}]"#),
    );
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_tasks();
    options.json = true;
    let out = tasks::execute(&client, &options).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        doc,
        serde_json::json!([{
            "id": "T7",
            "title": "T",
            "priority": "Normal",
            "status": "Open",
            "isClosed": false,
        }])
    );
}

// todo ---------------------------------------------------------------------

#[test]
fn todo_name_matches_arc() {
    assert_eq!(todo::NAME, "todo");
}

#[test]
fn todo_runs_help_and_usage_paths() {
    assert_eq!(todo::run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(todo::run(&args(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(todo::run(&args(&["summary", "--cc"])), ExitCode::FAILURE);
}

#[test]
fn creates_task_owned_by_viewer() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 55, "title": "Do thing", "uri": "https://example.test/T55"}"#),
    );
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let out = todo::execute(&client, &options_todo(&["Do", "thing"])).unwrap();
    assert_eq!(
        out,
        "Created task T55: 'Do thing' at https://example.test/T55\n"
    );
    assert_eq!(recv(&seen).0, "user.whoami");
    let create = recv(&seen);
    assert_eq!(create.0, "maniphest.createtask");
    // PHP `rawurlencode()` encodes spaces as `%20` (never `+`).
    assert!(create.1.contains("Do%20thing"), "{}", create.1);
    assert!(create.1.contains("PHID-USER-1"), "{}", create.1);
}

#[test]
fn blank_summary_creates_nothing() {
    // Zero expected requests: any conduit call would hang this test.
    let (base, _seen) = serve(HashMap::new(), 0);
    let client = client_for(&base);
    let out = todo::execute(&client, &options_todo(&[])).unwrap();
    assert_eq!(out, "Please provide a summary.\n");
    let out = todo::execute(&client, &options_todo(&["  "])).unwrap();
    assert_eq!(out, "Please provide a summary.\n");
}

#[test]
fn resolves_cc_users_and_project_slugs() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "user.query".to_string(),
        envelope(r#"[{"phid": "PHID-USER-2"}, {"phid": "PHID-USER-3"}]"#),
    );
    routes.insert(
        "project.query".to_string(),
        envelope(r#"{"slugMap": {"web": "PHID-PROJ-1"}}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 1, "title": "t", "uri": "https://example.test/T1"}"#),
    );
    let (base, seen) = serve(routes, 4);
    let client = client_for(&base);

    let mut options = options_todo(&["t"]);
    options.cc = args(&["alice", "bob"]);
    options.projects = args(&["web"]);
    todo::execute(&client, &options).unwrap();

    let mut calls = [recv(&seen), recv(&seen), recv(&seen), recv(&seen)];
    calls.sort();
    let methods: Vec<&str> = calls.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(
        methods,
        vec![
            "maniphest.createtask",
            "project.query",
            "user.query",
            "user.whoami"
        ]
    );
    let lookup = calls.iter().find(|(m, _)| m == "user.query").unwrap();
    assert!(lookup.1.contains("alice"), "{}", lookup.1);
    let create = calls
        .iter()
        .find(|(m, _)| m == "maniphest.createtask")
        .unwrap();
    assert!(create.1.contains("ccPHIDs"), "{}", create.1);
    assert!(create.1.contains("PHID-USER-2"), "{}", create.1);
    assert!(create.1.contains("projectPHIDs"), "{}", create.1);
    assert!(create.1.contains("PHID-PROJ-1"), "{}", create.1);
}

#[test]
fn unknown_project_slug_fails() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("project.query".to_string(), envelope(r#"{"slugMap": {}}"#));
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_todo(&["t"]);
    options.projects = args(&["nope"]);
    let err = todo::execute(&client, &options).unwrap_err();
    assert_eq!(err, "No such project: \"nope\"");
}

/// A fake browser command which exits zero without doing anything.
#[cfg(windows)]
fn null_browser_config() -> &'static str {
    r#"{"browser": ["cmd", "/c", "rem"]}"#
}

/// A fake browser command which exits zero without doing anything.
#[cfg(not(windows))]
fn null_browser_config() -> &'static str {
    r#"{"browser": ["true"]}"#
}

#[test]
fn browse_opens_the_created_task() {
    let fx = Fixture::named("todo-browse");
    std::fs::write(fx.dir.join(".arcconfig"), null_browser_config()).unwrap();
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 55, "title": "Do thing", "uri": "https://example.test/T55"}"#),
    );
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_todo(&["Do", "thing"]);
    options.browse = true;
    let out = todo::execute_in(&fx.dir, &client, &options).unwrap();
    assert_eq!(
        out,
        "Created task T55: 'Do thing' at https://example.test/T55\n"
    );

    // A failing browser command fails the workflow with PHP's error.
    std::fs::write(
        fx.dir.join(".arcconfig"),
        r#"{"browser": ["definitely-not-a-browser-xyz"]}"#,
    )
    .unwrap();
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 55, "title": "Do thing", "uri": "https://example.test/T55"}"#),
    );
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);
    let err = todo::execute_in(&fx.dir, &client, &options).unwrap_err();
    assert!(err.contains("Failed to open URI"), "{err}");
}

#[test]
fn todo_json_reports_created_task() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 55, "title": "Do thing", "uri": "https://example.test/T55"}"#),
    );
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_todo(&["Do", "thing"]);
    options.json = true;
    let out = todo::execute(&client, &options).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        doc,
        serde_json::json!({
            "id": "T55",
            "title": "Do thing",
            "uri": "https://example.test/T55",
        })
    );
}

// list ---------------------------------------------------------------------

#[test]
fn list_name_matches_arc() {
    assert_eq!(list::NAME, "list");
}

#[test]
fn list_runs_help_and_usage_paths() {
    assert_eq!(list::run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(list::run(&args(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(list::run(&args(&["D123"])), ExitCode::FAILURE);
}

#[test]
fn list_reports_no_open_revisions() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.query".to_string(), envelope("[]"));
    let (base, seen) = serve(routes, 2);
    let client = client_for(&base);

    let scratch = Fixture::named("list-empty");
    let out = list::execute_in(&client, &options_list(), &scratch.dir).unwrap();
    assert_eq!(out, "You have no open Differential revisions.\n");
    assert_eq!(recv(&seen).0, "user.whoami");
    let query = recv(&seen);
    assert_eq!(query.0, "differential.query");
    assert!(query.1.contains("PHID-USER-1"), "{}", query.1);
    assert!(query.1.contains("status-open"), "{}", query.1);
}

#[test]
fn list_marks_and_sorts_revisions() {
    let repo = Fixture::named("list-repo");
    repo.git(&["init", "-q", "-b", "master", "."]);
    let toplevel = repo.git(&["rev-parse", "--show-toplevel"]);

    let rows = format!(
        "[{{\"id\": 2, \"title\": \"Elsewhere work\", \"status\": 0, \"statusName\": \"Needs Review\", \"sourcePath\": \"/elsewhere\"}}, \
         {{\"id\": 1, \"title\": \"Here work\", \"status\": 2, \"statusName\": \"Accepted\", \"sourcePath\": \"{toplevel}\"}}]",
    );
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.query".to_string(), envelope(&rows));
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let out = list::execute_in(&client, &options_list(), &repo.dir).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    // PHP sort key (exists, status, id) ascending: the foreign revision first.
    assert!(lines[0].contains("D2"), "{out}");
    assert!(!lines[0].starts_with('*'), "{out}");
    // PhutilConsoleTable pads the status column to the widest cell
    // ("Needs Review") plus one separator space.
    assert_eq!(
        lines[1],
        format!("* {:<12} D1: Here work", "Accepted"),
        "{out}"
    );
}

#[test]
fn list_json_includes_exists_flag() {
    let repo = Fixture::named("list-json");
    repo.git(&["init", "-q", "-b", "master", "."]);
    let toplevel = repo.git(&["rev-parse", "--show-toplevel"]);

    let rows = format!(
        "[{{\"id\": 1, \"title\": \"Here work\", \"status\": 2, \"statusName\": \"Accepted\", \"sourcePath\": \"{toplevel}\"}}]",
    );
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.query".to_string(), envelope(&rows));
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let mut options = options_list();
    options.json = true;
    let out = list::execute_in(&client, &options, &repo.dir).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        doc,
        serde_json::json!([{
            "id": "D1",
            "title": "Here work",
            "status": 2,
            "statusName": "Accepted",
            "exists": true,
        }])
    );
}

#[test]
fn list_outside_repo_marks_nothing() {
    let mut routes = HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "differential.query".to_string(),
        envelope(r#"[{"id": 1, "title": "Work", "status": 0, "statusName": "Needs Review", "sourcePath": "/repo"}]"#),
    );
    let (base, _seen) = serve(routes, 2);
    let client = client_for(&base);

    let plain = Fixture::named("list-plain");
    let out = list::execute_in(&client, &options_list(), &plain.dir).unwrap();
    assert!(!out.contains('*'), "{out}");
    assert!(out.contains("D1"), "{out}");
}

#[test]
fn upload_continues_past_a_missing_file_then_reports_it() {
    // PHP uploads every file first, then throws the first file's errors.
    let mut routes = HashMap::new();
    routes.insert(
        "file.allocate".to_string(),
        envelope(r#"{"filePHID": "PHID-FILE-1", "upload": false}"#),
    );
    let (base, seen) = serve(routes, 1);
    let client = client_for(&base);

    let fx = Fixture::named("upload-continue");
    std::fs::write(fx.dir.join("good.bin"), b"data").unwrap();
    let err = upload::upload_paths(
        &client,
        &fx.dir,
        &["missing.bin".to_string(), "good.bin".to_string()],
        false,
        false,
        None,
    )
    .unwrap_err();
    assert!(err.contains("missing.bin"), "{err}");
    // The good file still reached the server.
    assert_eq!(recv(&seen).0, "file.allocate");
}

#[test]
fn conduit_modules_are_reachable_through_shared_helpers() {
    // `config` and `git` are exercised via the commands above; keep direct
    // references so the shared-helper wiring stays covered.
    assert!(config::discover_from(Path::new("/nonexistent-revmeta-dir")).is_none());
    let scratch = Fixture::named("list-git-helper");
    assert!(!git::is_inside_work_tree(&scratch.dir));
}
