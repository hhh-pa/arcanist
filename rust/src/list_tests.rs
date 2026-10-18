use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn info(id: &str, status: i64, exists: bool) -> RevisionInfo {
    RevisionInfo {
        id: id.to_string(),
        title: format!("Title {id}"),
        status,
        status_name: status_name_for(status).to_string(),
        source_path: String::new(),
        exists,
    }
}

#[test]
fn parses_defaults() {
    let options = parse_args(&args(&[])).unwrap().unwrap();
    assert!(!options.json);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_flags() {
    let options = parse_args(&args(&["--json", "--conduit-uri=https://x.test/"]))
        .unwrap()
        .unwrap();
    assert!(options.json);
    assert_eq!(options.conduit_uri, Some("https://x.test/".to_string()));
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri="])).is_err());
    assert!(parse_args(&args(&["D123"])).is_err());
    assert!(parse_args(&args(&["--", "D123"])).is_err());
}

#[test]
fn names_all_statuses() {
    assert_eq!(status_name_for(0), "Needs Review");
    assert_eq!(status_name_for(2), "Accepted");
    assert_eq!(status_name_for(3), "Closed");
    assert_eq!(status_name_for(77), "Unknown");
}

#[test]
fn parses_revision_rows() {
    let rev = parse_revision(&serde_json::json!({
        "id": 9,
        "title": "Some work",
        "status": 0,
        "statusName": "Needs Review",
        "sourcePath": "/repo",
    }))
    .unwrap();
    assert_eq!(rev.id, "9");
    assert_eq!(rev.title, "Some work");
    assert_eq!(rev.status, 0);
    assert_eq!(rev.status_name, "Needs Review");
    assert_eq!(rev.source_path, "/repo");
    assert!(!rev.exists);
    assert!(parse_revision(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn parses_list_and_map_results() {
    let row = serde_json::json!({"id": 1, "title": "t"});
    assert_eq!(parse_revisions(&serde_json::json!([row])).len(), 1);
    assert_eq!(parse_revisions(&serde_json::json!({"1": row})).len(), 1);
    assert!(parse_revisions(&serde_json::json!(null)).is_empty());
}

#[test]
fn sorts_by_exists_status_then_id() {
    let mut revs = vec![
        info("10", 2, true),
        info("2", 0, false),
        info("1", 0, false),
        info("3", 0, true),
    ];
    sort_revisions(&mut revs);
    let order: Vec<&str> = revs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(order, vec!["1", "2", "3", "10"]);
}

#[test]
fn renders_rows_with_marker() {
    assert_eq!(
        render_table(&[info("9", 2, true)], false),
        "* Accepted D9: Title 9\n"
    );
    assert_eq!(
        render_table(&[info("10", 0, false)], false),
        " Needs Review D10: Title 10\n"
    );
    assert_eq!(
        render_table(&[info("9", 2, true), info("10", 0, false)], false),
        "* Accepted     D9: Title 9\n  Needs Review D10: Title 10\n"
    );
    assert_eq!(status_color("Accepted"), "green");
    assert_eq!(status_color("Needs Review"), "magenta");
    assert_eq!(status_color("Abandoned"), "default");
    assert_eq!(status_color("In Preparation"), "default");
}

#[test]
fn builds_revisions_json() {
    assert_eq!(
        revisions_json(&[info("9", 2, true)]),
        serde_json::json!([{
            "id": "D9",
            "title": "Title 9",
            "status": 2,
            "statusName": "Accepted",
            "exists": true,
        }])
    );
}

#[test]
fn finds_hg_and_svn_roots() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!("arc-rs-list-roots-{}-{tag}", std::process::id()));

    let hg_nested = base.join("hg").join("sub");
    std::fs::create_dir_all(base.join("hg").join(".hg")).unwrap();
    std::fs::create_dir_all(&hg_nested).unwrap();
    // `find_hg_root` walks up without shelling out.
    assert_eq!(find_hg_root(&hg_nested), Some(base.join("hg")), "hg root");

    let svn_nested = base.join("svn").join("sub");
    std::fs::create_dir_all(base.join("svn").join(".svn")).unwrap();
    std::fs::create_dir_all(&svn_nested).unwrap();
    assert_eq!(
        working_copy_root(&svn_nested),
        Some(base.join("svn").to_string_lossy().to_string()),
        "svn root"
    );
    assert_eq!(
        working_copy_root(&hg_nested),
        Some(base.join("hg").to_string_lossy().to_string()),
        "hg root via working_copy_root"
    );

    let plain = base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(find_hg_root(&plain), None);
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn compares_paths() {
    assert!(same_path("/x/y", "/x/y"));
    assert!(!same_path("/x/y", "/x/z"));
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-list-samepath-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let with_dot = dir.join(".").to_string_lossy().to_string();
    let plain = dir.to_string_lossy().to_string();
    assert!(same_path(&with_dot, &plain));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn require_working_copy_matches_php_error() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base =
        std::env::temp_dir().join(format!("arc-rs-list-require-{}-{tag}", std::process::id()));

    let plain = base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(
        require_working_copy(&plain).unwrap_err(),
        "This command must be run in a Git, Mercurial or Subversion working copy."
    );

    // Mercurial and Subversion checkouts pass without shelling out to git.
    let hg = base.join("hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(require_working_copy(&hg).is_ok());
    let svn = base.join("svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert!(require_working_copy(&svn).is_ok());

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn run_in_fails_outside_repo_before_conduit() {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let plain =
        std::env::temp_dir().join(format!("arc-rs-list-runin-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&plain).unwrap();

    // No conduit is configured either, but the repository check fires
    // first, exactly like the PHP runtime ordering.
    assert_eq!(run_in(&plain, &args(&[])), ExitCode::FAILURE);
    assert_eq!(run_in(&plain, &args(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(run_in(&plain, &args(&["--help"])), ExitCode::SUCCESS);

    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn execute_propagates_conduit_errors() {
    // T1153 RB-D27 lock: execute errors (incl. conduit failures) surface as Err
    // so run() prints them to stderr with exit 1, like PHP.
    let client = crate::conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let options = Options {
        json: false,
        conduit_uri: None,
    };
    let err = execute(&client, &options).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn r4_rbd4_parse_and_working_copy_are_usage() {
    // `list` is a subprocess workflow: argument and working-copy
    // errors render `Usage Exception:` (exit 1) with no usage dump.
    assert_eq!(
        run_in(&std::env::temp_dir(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-list-r4-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(run_in(&dir, &args(&[])), std::process::ExitCode::FAILURE);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rbd8_user_phid_prefers_cached_identity() {
    // The startup-auth identity answers without I/O: an unroutable
    // client still resolves when cached.
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    client.set_authenticated_user("alice", "PHID-USER-1");
    assert_eq!(user_phid(&client).unwrap(), "PHID-USER-1");
}
#[test]
fn r10_list_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "LIST WORKFLOW\n\
         \n\
         list\n\
         \x20     list\n\
         \n\
         \x20         Supports: git, svn, hg\n\
         \x20         List your open Differential revisions.\n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("LIST WORKFLOW"), "{}", usage());
}

#[test]
fn b21_list_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `list`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_args(&args(&["--json"])).unwrap().is_some());
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    let text = usage_text(false);
    assert!(text.contains("LIST WORKFLOW"), "{text}");
}

#[test]
fn b23_list_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help list`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("LIST WORKFLOW\n"), "{text}");
    assert!(
        text.contains("List your open Differential revisions."),
        "{text}"
    );
    assert!(!text.contains("--json"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn conduit_uri_takes_a_separate_value() {
    let options = parse_args(&args(&["--conduit-uri", "https://x.test/"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("https://x.test/".to_string()));
}

#[test]
fn status_names_cover_every_code() {
    assert_eq!(status_name_for(0), "Needs Review");
    assert_eq!(status_name_for(1), "Needs Revision");
    assert_eq!(status_name_for(2), "Accepted");
    assert_eq!(status_name_for(3), "Closed");
    assert_eq!(status_name_for(4), "Abandoned");
    assert_eq!(status_name_for(5), "Changes Planned");
    assert_eq!(status_name_for(6), "In Preparation");
    assert_eq!(status_name_for(99), "Unknown");
}

#[test]
fn revision_ids_accept_big_and_prefixed_forms() {
    let revision =
        parse_revision(&serde_json::json!({"id": 18_446_744_073_709_551_615u64})).unwrap();
    assert_eq!(revision.id, "18446744073709551615");
    let revision = parse_revision(&serde_json::json!({"id": "D42"})).unwrap();
    assert_eq!(revision.id, "42");
    let revision = parse_revision(&serde_json::json!({"id": "d7"})).unwrap();
    assert_eq!(revision.id, "7");
    let revision = parse_revision(&serde_json::json!({"id": "42"})).unwrap();
    assert_eq!(revision.id, "42");
}

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-list-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(status.success());
}

fn list_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    git(&tmp.path, &["config", "user.email", "t@t"]);
    git(&tmp.path, &["config", "user.name", "t"]);
    git(&tmp.path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path.join("f.txt"), "one\n").unwrap();
    git(&tmp.path, &["add", "f.txt"]);
    git(&tmp.path, &["commit", "-qm", "add f"]);
    tmp
}

/// Loopback stub routing each conduit method to its canned envelope.
fn serve_routes(routes: std::collections::HashMap<String, String>, expected: usize) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head_text = String::from_utf8_lossy(&head).to_string();
            let len: usize = head_text
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(key, value)| {
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let method = head_text
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .strip_prefix("/api/")
                .unwrap_or("")
                .to_string();
            let canned = routes
                .get(&method)
                .cloned()
                .unwrap_or_else(|| r#"{"result": null, "error_code": "ERR-NO-ROUTE"}"#.to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    base
}

fn envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

/// The arcrc host key for a stub base, mirroring
/// `config::host_key` (`{scheme}://{authority}/api/`).
fn host_key_for(base: &str) -> String {
    let after_scheme = base.split("://").nth(1).unwrap_or(base);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let scheme = base.split("://").next().unwrap_or("https");
    format!("{scheme}://{authority}/api/")
}

/// Fake arcrc granting a token for the stub `base`.
fn fake_arcrc(dir: &std::path::Path, base: &str) -> std::path::PathBuf {
    let path = dir.join("fake.arcrc");
    let key = host_key_for(base);
    std::fs::write(
        &path,
        format!(r#"{{"hosts": {{"{key}": {{"token": "x"}}}}}}"#),
    )
    .unwrap();
    path
}

#[test]
fn authed_run_lists_open_revisions() {
    let repo = list_repo("authed-ok");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"userName": "alice", "phid": "PHID-USER-1"}"#),
    );
    routes.insert("differential.query".to_string(), envelope("[]"));
    let base = serve_routes(routes, 2);
    let arcrc = fake_arcrc(&repo.path, &base);
    assert_eq!(
        run_in_with_user_file(
            &repo.path,
            &args(&["--conduit-uri", base.as_str()]),
            Some(&arcrc)
        ),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn authed_run_reports_query_errors() {
    let repo = list_repo("authed-err");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"userName": "alice", "phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "differential.query".to_string(),
        r#"{"result": null, "error_code": "ERR-QUERY", "error_info": "boom"}"#.to_string(),
    );
    let base = serve_routes(routes, 2);
    let arcrc = fake_arcrc(&repo.path, &base);
    assert_eq!(
        run_in_with_user_file(
            &repo.path,
            &args(&["--conduit-uri", base.as_str()]),
            Some(&arcrc)
        ),
        std::process::ExitCode::FAILURE
    );
}
