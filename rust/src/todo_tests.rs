use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_summary_words() {
    let options = parse_args(&args(&["fix", "the", "build"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.summary, args(&["fix", "the", "build"]));
    assert!(options.cc.is_empty());
    assert!(options.projects.is_empty());
    assert!(!options.browse);
    assert!(!options.json);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_all_options() {
    let options = parse_args(&args(&[
        "do",
        "thing",
        "--cc",
        "alice",
        "-C",
        "bob",
        "--cc=carol",
        "--project",
        "web",
        "--project=api",
        "--json",
        "--conduit-uri",
        "https://example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.summary, args(&["do", "thing"]));
    assert_eq!(options.cc, args(&["alice", "bob", "carol"]));
    assert_eq!(options.projects, args(&["web", "api"]));
    assert!(options.json);
    assert_eq!(
        options.conduit_uri,
        Some("https://example.test/".to_string())
    );
}

#[test]
fn parses_double_dash_and_browse() {
    let options = parse_args(&args(&["--browse", "--", "--not-a-flag"]))
        .unwrap()
        .unwrap();
    assert!(options.browse);
    assert_eq!(options.summary, args(&["--not-a-flag"]));
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--cc"])).is_err());
    assert!(parse_args(&args(&["-C"])).is_err());
    assert!(parse_args(&args(&["--cc="])).is_err());
    assert!(parse_args(&args(&["--project"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
}

#[test]
fn parses_created_tasks() {
    let task = parse_created(&serde_json::json!({
        "id": 55,
        "title": "Do thing",
        "uri": "https://example.test/T55",
    }))
    .unwrap();
    assert_eq!(task.id, "55");
    assert_eq!(task.title, "Do thing");
    assert_eq!(task.uri, "https://example.test/T55");
    assert!(parse_created(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn renders_created_line() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    assert_eq!(
        render_created(&task, false),
        "Created task T55: 'Do thing' at https://example.test/T55\n"
    );
    // Titles substitute raw (never escaped) and keep colors on TTYs.
    let tricky = CreatedTask {
        id: "1".to_string(),
        title: "a\nb".to_string(),
        uri: "u".to_string(),
    };
    assert_eq!(
        render_created(&tricky, false),
        "Created task T1: 'a\nb' at u\n"
    );
    assert_eq!(
        render_created(&task, true),
        "Created task T55: '\u{1b}[32m\u{1b}[1mDo thing\u{1b}[m\u{1b}[39m' at \
         \u{1b}[34m\u{1b}[1mhttps://example.test/T55\u{1b}[m\u{1b}[39m\n"
    );
}

fn plain_options(summary: &[&str]) -> Options {
    Options {
        summary: args(summary),
        cc: Vec::new(),
        projects: Vec::new(),
        browse: false,
        json: false,
        conduit_uri: None,
    }
}

#[test]
fn execute_threads_ansi_through_finish() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    let options = plain_options(&["Do", "thing"]);
    assert_eq!(
        render_created_task(&task, &options, false).unwrap(),
        render_created(&task, false)
    );
    let styled = render_created_task(&task, &options, true).unwrap();
    assert_eq!(styled, render_created(&task, true));
    assert!(styled.contains('\u{1b}'), "got: {styled}");
}

#[test]
fn rbd29b_create_returns_task_for_browse_after_print() {
    // `--browse` no longer runs inside `execute`: the outcome carries
    // the task so the caller prints first, then browses (like PHP).
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let options = plain_options(&[]);
    let outcome = create_in_ansi(std::env::temp_dir().as_path(), &client, &options, false).unwrap();
    assert_eq!(outcome.text, "Please provide a summary.\n");
    assert_eq!(outcome.task, None);
}

#[test]
fn blank_summary_creates_nothing_in_both_modes() {
    // No conduit traffic: the client points at a dead port and blank
    // summaries return before any call.
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let options = plain_options(&[]);
    for ansi in [false, true] {
        assert_eq!(
            execute_in_ansi(std::env::temp_dir().as_path(), &client, &options, ansi).unwrap(),
            "Please provide a summary.\n"
        );
    }
}

#[test]
fn builds_created_json() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    assert_eq!(
        created_json(&task),
        serde_json::json!({
            "id": "T55",
            "title": "Do thing",
            "uri": "https://example.test/T55",
        })
    );
}

#[test]
fn parses_created_ids_beyond_i64_and_strings() {
    let task = parse_created(&serde_json::json!({
        "id": 18446744073709551615u64,
        "title": "Big",
        "uri": "https://example.test/T1",
    }))
    .unwrap();
    assert_eq!(task.id, "18446744073709551615");
    let task = parse_created(&serde_json::json!({
        "id": "77",
        "title": "Str",
        "uri": "https://example.test/T77",
    }))
    .unwrap();
    assert_eq!(task.id, "77");
}

#[test]
fn error_text_helpers_render_verbatim() {
    assert_eq!(
        conduit_error_text(conduit::Error::Transport("down".to_string())),
        "down"
    );
    assert_eq!(
        missing_task_id(),
        "conduit maniphest.createtask response has no id"
    );
    let parse = serde_json::from_str::<serde_json::Value>("{bad").unwrap_err();
    assert!(json_encode_error(parse).starts_with("failed to encode JSON: "),);
}

/// Serve one canned Conduit response on loopback, returning the base URI.
/// The handler reads the request headers, then answers `response`.
fn serve_one(response: &str) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}/", listener.local_addr().unwrap());
    let body = response.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
            if seen.windows(4).any(|w| w == b"\r\n\r\n") || seen.len() > 65536 {
                break;
            }
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(reply.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    (uri, handle)
}

#[test]
fn cc_resolution_ignores_scalar_results() {
    let (uri, handle) = serve_one(r#"{"error_code":null,"error_info":null,"result":"scalar"}"#);
    let client = conduit::Client::new(&uri, "t").unwrap();
    let phids = resolve_cc_phids(&client, &["alice".to_string()]).unwrap();
    assert!(phids.is_empty());
    handle.join().unwrap();
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `todo` is a subprocess workflow: argument errors render `Usage
    // Exception:` (exit 1) with no usage dump.
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
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
fn r10_todo_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "TODO WORKFLOW\n\
         \n\
         todo\n\
         \x20     todo summary [options]\n\
         \n\
         \x20       Quickly create a task for yourself.\n\
         \n\
         \x20     --browse\n\
         \x20         After creating the task, open it in a web browser.\n\
         \n\
         \x20     --cc cc, -C cc\n\
         \x20         Other users to CC on the new task.\n\
         \n\
         \x20     --project project\n\
         \x20         Projects to assign to the task.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("TODO WORKFLOW"), "{}", usage());
}

#[test]
fn b21_todo_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `todo`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_args(&args(&["fix", "it"])).unwrap().is_some());
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    let text = usage_text(false);
    assert!(text.contains("TODO WORKFLOW"), "{text}");
}

#[test]
fn b23_todo_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help todo`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("TODO WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Quickly create a task for yourself."),
        "{text}"
    );
    assert!(text.contains("--browse"), "{text}");
    assert!(!text.contains("--json"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn run_reports_auth_failure_for_unreachable_uri() {
    // T1170 coverage: `run()` with valid args reaches the classic
    // startup gate. The URI is unroutable (localhost discard port),
    // so `authed_client_for` fails deterministically on every
    // machine: no stored credential matches that host key (and even
    // an ambient `ARC_CONDUIT_TOKEN` only leads to a refused
    // `user.whoami`). No writes, no external network, and the
    // outcome (a non-zero envelope) is HOME-independent.
    let code = run(&args(&[
        "file",
        "the",
        "report",
        "--conduit-uri",
        "http://127.0.0.1:9/",
    ]));
    assert_ne!(code, std::process::ExitCode::SUCCESS);
}
// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

fn serve_one_addr(uri: &str) -> String {
    uri.trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

#[test]
fn serve_one_answers_connections_that_close_early() {
    // A client that half-closes without sending headers reads EOF
    // (`n == 0`): the handler still answers instead of hanging.
    use std::io::Read as _;
    let (uri, handle) = serve_one("{}");
    let stream = std::net::TcpStream::connect(serve_one_addr(&uri)).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reply = Vec::new();
    let mut stream = stream;
    stream.read_to_end(&mut reply).unwrap();
    handle.join().unwrap();
    assert!(reply.windows(2).any(|w| w == b"{}"));
}

#[test]
fn serve_one_tolerates_partially_sent_headers() {
    // Headers arriving in two chunks take the keep-reading edge: the
    // first read holds an incomplete head, the second completes it.
    use std::io::{Read as _, Write as _};
    let (uri, handle) = serve_one("{}");
    let mut stream = std::net::TcpStream::connect(serve_one_addr(&uri)).unwrap();
    stream.write_all(b"P").unwrap();
    stream.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    stream
        .write_all(b"OST / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    handle.join().unwrap();
    assert!(reply.windows(2).any(|w| w == b"{}"));
}

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-todo-tests-{tag}-{}-{n}",
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
fn authed_run_creates_task() {
    let tmp = TempDir::new("authed-ok");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"userName": "alice", "phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 77, "title": "my summary", "uri": "http://phab.example/T77"}"#),
    );
    let base = serve_routes(routes, 2);
    let arcrc = fake_arcrc(&tmp.path, &base);
    assert_eq!(
        run_in_with_user_file(
            &tmp.path,
            &args(&["--conduit-uri", base.as_str(), "my", "summary"]),
            Some(&arcrc)
        ),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn authed_run_reports_create_errors() {
    let tmp = TempDir::new("authed-err");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"userName": "alice", "phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        r#"{"result": null, "error_code": "ERR-CREATE", "error_info": "boom"}"#.to_string(),
    );
    let base = serve_routes(routes, 2);
    let arcrc = fake_arcrc(&tmp.path, &base);
    assert_eq!(
        run_in_with_user_file(
            &tmp.path,
            &args(&["--conduit-uri", base.as_str(), "my", "summary"]),
            Some(&arcrc)
        ),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn authed_run_browse_failure_reports_usage() {
    // `--browse` after a successful create opens the task URI. The
    // fixture cwd configures a bogus `browser`, so the open fails
    // hermetically (no PATH scan, never a real browser — the
    // badbrowser precedent in browse_tests).
    let tmp = TempDir::new("authed-browse");
    // Project config needs a VCS root; `.git` existence suffices.
    std::fs::create_dir(tmp.path.join(".git")).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"browser": "definitely-not-a-browser-t1176"}"#,
    )
    .unwrap();
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"userName": "alice", "phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "maniphest.createtask".to_string(),
        envelope(r#"{"id": 77, "title": "my summary", "uri": "http://phab.example/T77"}"#),
    );
    let base = serve_routes(routes, 2);
    let arcrc = fake_arcrc(&tmp.path, &base);
    assert_eq!(
        run_in_with_user_file(
            &tmp.path,
            &args(&["--conduit-uri", base.as_str(), "--browse", "my", "summary"]),
            Some(&arcrc)
        ),
        std::process::ExitCode::FAILURE
    );
}

// -( T1176 PC-1: env-globals )------------------------------------------------

/// Probe marker: the deleted-cwd child re-runs only the probe test,
/// so the suite never chdirs its own process (TZ-precedent shape).
const PC1_DELETED_CWD_ENV: &str = "ARC_RS_PC1_TODO_DELETED_CWD";
#[cfg(unix)]
const PC1_DELETED_CWD_PROBE: &str = "pc1_deleted_cwd_probe_blank_summary";

/// Probe body for `pc1_deleted_cwd_falls_back_to_dot`: with a deleted
/// cwd, `execute_ansi` falls back to `"."` and the blank summary
/// short-circuits before any conduit traffic. Direct suite runs (cwd
/// intact, env unset) pass through.
#[test]
fn pc1_deleted_cwd_probe_blank_summary() {
    if std::env::var_os(PC1_DELETED_CWD_ENV).is_none() {
        return;
    }
    println!("TODO-PC1-DELETED-CWD-OK");
    assert!(
        std::env::current_dir().is_err(),
        "probe premise: the child runs with a deleted cwd"
    );
    let client = conduit::Client::new("https://probe.invalid/", "dummy")
        .expect("client construction performs no I/O");
    let options = plain_options(&[]);
    assert_eq!(
        execute_ansi(&client, &options, false),
        Ok("Please provide a summary.\n".to_string())
    );
}

/// Deleted-cwd child: `sh` owns the cwd deletion (never the test
/// process), then re-execs this test binary on the probe filter.
#[test]
#[cfg(unix)]
fn pc1_deleted_cwd_falls_back_to_dot() {
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg("d=$(mktemp -d); cd \"$d\"; rmdir \"$d\"; exec \"$0\" \"$@\"")
        .arg(&exe)
        .args(["--nocapture", PC1_DELETED_CWD_PROBE])
        .env(PC1_DELETED_CWD_ENV, "1")
        .output()
        .expect("spawn deleted-cwd probe child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success()
            && stdout.contains("1 passed")
            && stdout.contains("TODO-PC1-DELETED-CWD-OK"),
        "deleted-cwd probe child failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}
