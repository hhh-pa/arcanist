use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
        Parsed::Options(Options { apply: true, .. })
    ));
    // `--yes` is accepted (it skips the `--apply` prompt).
    assert!(matches!(
        parse_args(&["--yes".to_string()]),
        Parsed::Options(Options { apply: false, .. })
    ));
    assert!(matches!(
        parse_args(&["--bogus".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(parse_args(&["pos".to_string()]), Parsed::Error(_)));
}

#[test]
fn apply_target_requires_git_and_supported_branch() {
    let plain = TempDir::new("nogit");
    let err = resolve_apply_target(&plain.path).unwrap_err();
    assert!(err.contains("is not a Git working copy"), "{err}");
    assert!(err.contains("arc upgrade"), "{err}");

    let repo = fresh_repo("branch");
    commit(&repo.path, "a.txt");
    let target = resolve_apply_target(&repo.path).unwrap();
    assert_eq!(target.branch, "master");
    assert_eq!(
        target.root.canonicalize().unwrap(),
        repo.path.canonicalize().unwrap()
    );

    git(&repo.path, &["checkout", "-q", "-b", "feature"]);
    let err = resolve_apply_target(&repo.path).unwrap_err();
    assert!(
        err.contains("not supported for automatic upgrades"),
        "{err}"
    );
    assert!(err.contains("master, stable"), "{err}");
}

#[test]
fn uncommitted_paths_cover_all_entry_kinds() {
    let repo = fresh_repo("dirty");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    // Modified (worktree), untracked, and a staged rename (both sides).
    std::fs::write(repo.path.join("a.txt"), "worktree\n").unwrap();
    std::fs::write(repo.path.join("new.txt"), "untracked\n").unwrap();
    git(&repo.path, &["mv", "b.txt", "c.txt"]);
    let paths = uncommitted_paths(&repo.path).unwrap();
    assert_eq!(paths, vec!["a.txt", "b.txt", "c.txt", "new.txt"]);

    let block = render_uncommitted_block(&repo.path, &paths, false);
    assert!(block.contains("You have uncommitted changes"), "{block}");
    assert!(block.contains("      - a.txt\n"), "{block}");
    assert!(
        block.contains("Discard these changes before running \"arc upgrade\"."),
        "{block}"
    );

    let clean = fresh_repo("clean");
    commit(&clean.path, "a.txt");
    assert!(uncommitted_paths(&clean.path).unwrap().is_empty());
}

#[test]
fn wraps_uncommitted_block_paragraphs() {
    // The opening paragraph always exceeds 78 columns, so it wraps at
    // the last space; the closing paragraph fits on one line.
    let block = render_uncommitted_block(&PathBuf::from("r"), &["a.txt".to_string()], false);
    assert_eq!(
        block,
        "You have uncommitted changes in the working copy (\"r\") for this library\n\
         (\"arc-rs\"):\n\
         \n\
         \x20     - a.txt\n\
         \n\
         Discard these changes before running \"arc upgrade\".\n\
         \n"
    );
}

#[test]
fn uncommitted_block_escapes_listed_paths() {
    let block = render_uncommitted_block(&PathBuf::from("r"), &["a\u{7}b".to_string()], false);
    assert!(block.contains("      - a<BEL>b\n"), "{block}");
}

#[test]
fn wraps_long_paragraphs_at_word_boundaries() {
    let message = format!("{} {}", "a".repeat(70), "b".repeat(20));
    assert_eq!(
        wrap_paragraph(&message, false),
        format!("{}\n{}", "a".repeat(70), "b".repeat(20))
    );
    assert_eq!(wrap_paragraph("short", false), "short");
}

#[test]
fn apply_refuses_dirty_checkouts() {
    let repo = fresh_repo("dirtyapply");
    commit(&repo.path, "a.txt");
    std::fs::write(repo.path.join("a.txt"), "dirty\n").unwrap();
    let target = resolve_apply_target(&repo.path).unwrap();
    let err = run_apply(&target).unwrap_err();
    assert_eq!(
        err,
        "\"arc upgrade\" can only upgrade clean working copies."
    );
}

#[test]
fn apply_pulls_from_origin() {
    let origin = TempDir::new("applyorigin");
    git(&origin.path, &["init", "-q", "--bare"]);
    let seed = fresh_repo("applyseed");
    commit(&seed.path, "a.txt");
    git(
        &seed.path,
        &["remote", "add", "origin", &origin.path.to_string_lossy()],
    );
    git(&seed.path, &["push", "-q", "-u", "origin", "master"]);

    let work = TempDir::new("applywork");
    git(
        &work.path,
        &["clone", "-q", &origin.path.to_string_lossy(), "."],
    );
    git(&work.path, &["config", "user.email", "t@t"]);
    git(&work.path, &["config", "user.name", "t"]);
    git(&work.path, &["config", "commit.gpgsign", "false"]);

    commit(&seed.path, "b.txt");
    git(&seed.path, &["push", "-q", "origin", "master"]);

    let target = resolve_apply_target(&work.path).unwrap();
    run_apply(&target).unwrap();
    assert!(work.path.join("b.txt").is_file());
    assert!(rebuild_note().contains("cargo build"));
}

/// One canned HTTP response, matched by request-line substring.
struct MockRoute {
    needle: &'static str,
    status: &'static str,
    body: Vec<u8>,
    content_type: &'static str,
}

/// Observed request lines, in arrival order.
type Seen = Arc<Mutex<Vec<String>>>;

fn route(needle: &'static str, status: &'static str, body: Vec<u8>) -> MockRoute {
    MockRoute {
        needle,
        status,
        body,
        content_type: "application/octet-stream",
    }
}

fn json_route(needle: &'static str, body: &str) -> MockRoute {
    MockRoute {
        needle,
        status: "200 OK",
        body: body.as_bytes().to_vec(),
        content_type: "application/json",
    }
}

/// A bound listener with its base URL and hit log (worker not started,
/// so routes can embed the base URL).
fn mock_listener() -> (TcpListener, String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    (listener, base, Arc::new(Mutex::new(Vec::new())))
}

/// Serve `routes` on `listener`: each route is served once, then
/// removed; unmatched requests get a 500 so unexpected calls fail
/// loudly instead of hanging the client.
fn run_mock(listener: TcpListener, routes: Vec<MockRoute>, seen: Seen) {
    std::thread::spawn(move || {
        let mut pending = routes;
        while !pending.is_empty() {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            serve_one(&mut stream, &mut pending, &seen);
        }
    });
}

fn serve_one(stream: &mut TcpStream, pending: &mut Vec<MockRoute>, seen: &Seen) {
    let line = read_request_line(stream);
    let route = take_route(pending, &line);
    seen.lock().unwrap().push(line);
    write_mock_response(stream, &route);
}

fn take_route(pending: &mut Vec<MockRoute>, line: &str) -> MockRoute {
    let position = pending.iter().position(|route| line.contains(route.needle));
    match position {
        Some(index) => pending.remove(index),
        None => unexpected_route(line),
    }
}

fn unexpected_route(line: &str) -> MockRoute {
    route(
        "",
        "500 Internal Server Error",
        format!("unexpected request: {line}").into_bytes(),
    )
}

/// First request line of one connection (headers skipped).
fn read_request_line(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    for _ in 0..65536 {
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if stream.read_exact(&mut byte).is_err() {
            break;
        }
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

fn write_mock_response(stream: &mut TcpStream, route: &MockRoute) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        route.status,
        route.content_type,
        route.body.len()
    );
    stream.write_all(head.as_bytes()).ok();
    stream.write_all(&route.body).ok();
}

fn saw(seen: &Seen, needle: &str) -> bool {
    seen.lock()
        .unwrap()
        .iter()
        .any(|line| line.contains(needle))
}

fn release_doc(tag: &str, assets: &str) -> String {
    format!("{{\"tag_name\": \"{tag}\", \"assets\": {assets}}}")
}

fn asset_entry(name: &str, size: u64, base: &str) -> String {
    format!(
        "{{\"name\": \"{name}\", \"size\": {size}, \"browser_download_url\": \"{base}dl/{name}\"}}"
    )
}

/// A disposable stand-in executable (the real test binary is never touched).
fn exe_copy(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn apply_opts(api_base: &str, current: &str, exe: &Path) -> ApplyOptions {
    ApplyOptions {
        api_base: api_base.to_string(),
        current_version: current.to_string(),
        exe: Some(exe.to_path_buf()),
        yes: false,
        json: false,
        ansi: false,
    }
}

fn confirm_yes(_: &str) -> Result<bool, String> {
    Ok(true)
}

fn confirm_no(_: &str) -> Result<bool, String> {
    Ok(false)
}

fn set_readonly(path: &Path, readonly: bool) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_readonly(readonly);
    std::fs::set_permissions(path, perms).unwrap();
}

/// No staging temp files or writability probes may survive a run.
fn assert_no_leftovers(dir: &Path) {
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        names.push(entry.unwrap().file_name().to_string_lossy().to_string());
    }
    assert!(
        !names
            .iter()
            .any(|name| name.contains(".arc-rs-new") || name.contains("writable")),
        "{names:?}"
    );
}

#[test]
fn yes_flag_is_recorded() {
    assert!(matches!(
        parse_args(&["--yes".to_string()]),
        Parsed::Options(Options { yes: true, .. })
    ));
    assert!(matches!(
        parse_args(&[]),
        Parsed::Options(Options { yes: false, .. })
    ));
}

#[test]
fn github_remote_parsing() {
    let cases = [
        (
            "https://github.com/m-hu/arcanist.git",
            Some("m-hu/arcanist"),
        ),
        ("https://github.com/m-hu/arcanist", Some("m-hu/arcanist")),
        ("https://github.com/m-hu/arcanist/", Some("m-hu/arcanist")),
        ("http://github.com/o/r.git", Some("o/r")),
        ("https://www.github.com/o/r", Some("o/r")),
        ("git@github.com:o/r.git", Some("o/r")),
        ("git@github.com:o/r", Some("o/r")),
        ("ssh://git@github.com/o/r.git", Some("o/r")),
        ("ssh://github.com/o/r", Some("o/r")),
        ("HTTPS://GITHUB.COM/O/R", Some("O/R")),
        ("  https://github.com/o/r.git\n", Some("o/r")),
        ("https://gitlab.com/o/r.git", None),
        ("git@gitlab.com:o/r.git", None),
        ("https://github.com/o", None),
        ("https://github.com/o/r/extra", None),
        ("https://github.com//r", None),
        ("not-a-url", None),
        ("", None),
        ("/local/path/repo.git", None),
    ];
    for (url, want) in cases {
        assert_eq!(parse_github_repo(url).as_deref(), want, "{url}");
    }
}

#[test]
fn release_repo_prefers_fork_then_origin() {
    let repo = fresh_repo("reporemote");
    assert_eq!(release_repo_for(&repo.path), DEFAULT_RELEASE_REPO);
    git(
        &repo.path,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/up/stream.git",
        ],
    );
    assert_eq!(release_repo_for(&repo.path), "up/stream");
    git(
        &repo.path,
        &["remote", "add", "fork", "git@github.com:me/fork.git"],
    );
    assert_eq!(release_repo_for(&repo.path), "me/fork");
    git(
        &repo.path,
        &["remote", "set-url", "fork", "/local/path.git"],
    );
    assert_eq!(release_repo_for(&repo.path), "up/stream");
    let plain = TempDir::new("reponone");
    assert_eq!(release_repo_for(&plain.path), DEFAULT_RELEASE_REPO);
}

#[test]
fn release_target_gates_branches() {
    let plain = TempDir::new("targetplain");
    let outside = resolve_release_target(&plain.path).unwrap();
    assert_eq!(outside.repo, DEFAULT_RELEASE_REPO);
    assert_eq!(outside.channel, "stable");

    let repo = fresh_repo("targetrepo");
    commit(&repo.path, "a.txt");
    git(
        &repo.path,
        &["remote", "add", "fork", "https://github.com/me/fork.git"],
    );
    let master = resolve_release_target(&repo.path).unwrap();
    assert_eq!(
        master,
        ReleaseTarget {
            repo: "me/fork".to_string(),
            channel: "master".to_string(),
        }
    );

    git(&repo.path, &["checkout", "-q", "-b", "feature"]);
    let err = resolve_release_target(&repo.path).unwrap_err();
    assert!(
        err.contains("not supported for automatic upgrades"),
        "{err}"
    );
    assert!(err.contains("master, stable"), "{err}");
}

#[test]
fn version_ordering() {
    let cases = [
        ("v0.1.1", "0.1.0", true),
        ("0.2.0", "0.1.9", true),
        ("1.0.0", "0.9.9", true),
        ("v10.0.0", "v9.9.9", true),
        ("1.0.1", "1.0", true),
        ("1.2.3.4", "1.2.3", true),
        ("1.0.0", "1.0.0-rc1", true),
        ("1.0.0-rc2", "1.0.0-rc1", true),
        ("V1.2.3", "1.2.2", true),
        ("v0.1.0", "0.1.0", false),
        ("0.1.0", "v0.1.0", false),
        ("0.1.0", "0.1.1", false),
        ("1.0", "1.0.0", false),
        ("1.0.0-rc1", "1.0.0", false),
        ("1.0.0-rc1", "1.0.0-rc1", false),
        ("stable", "0.1.0", false),
        ("", "0.0.0", false),
        ("version2", "0.0.0", false),
    ];
    for (latest, current, want) in cases {
        assert_eq!(is_newer(latest, current), want, "{latest} vs {current}");
    }
}

#[test]
fn release_url_shapes() {
    assert_eq!(
        latest_release_url("https://api.github.com", "o/r"),
        "https://api.github.com/repos/o/r/releases/latest"
    );
    assert_eq!(
        latest_release_url("http://127.0.0.1:9/", "o/r"),
        "http://127.0.0.1:9/repos/o/r/releases/latest"
    );
    assert!(platform_tag().contains('-'));
    assert_eq!(
        platform_tag(),
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    );
}

#[test]
fn select_asset_prefers_exact_platform() {
    let doc = serde_json::json!({
        "tag_name": "v9.9.9",
        "assets": [
            {"name": "checksums.txt", "size": 100, "browser_download_url": "http://x/sums"},
            {"name": "arc-rs-linux-x86_64", "size": 10, "browser_download_url": "http://x/lin"},
            {"name": "arc-rs-windows-x86_64.exe", "size": 20, "browser_download_url": "http://x/win"},
        ]
    });
    let win = select_asset(&doc, "windows-x86_64").unwrap();
    assert_eq!(win.url, "http://x/win");
    assert_eq!(win.size, 20);
    let lin = select_asset(&doc, "linux-x86_64").unwrap();
    assert_eq!(lin.url, "http://x/lin");
}

#[test]
fn select_asset_falls_back_to_lone_candidate() {
    let doc = serde_json::json!({
        "tag_name": "v9.9.9",
        "assets": [
            {"name": "arc-rs-mystery", "size": 7, "browser_download_url": "http://x/only"},
        ]
    });
    let only = select_asset(&doc, "plan9-arm").unwrap();
    assert_eq!(only.url, "http://x/only");
}

#[test]
fn select_asset_refuses_missing_and_ambiguous() {
    let empty = serde_json::json!({"tag_name": "v9", "assets": []});
    let err = select_asset(&empty, "windows-x86_64").unwrap_err();
    assert!(err.contains("no installable asset"), "{err}");

    // Malformed entries (missing name/URL, zero size) never qualify.
    let bad = serde_json::json!({
        "tag_name": "v9",
        "assets": [
            {"name": "arc-rs-windows-x86_64.exe", "size": 0, "browser_download_url": "http://x/e"},
            {"name": "", "size": 5, "browser_download_url": "http://x/n"},
            {"name": "arc-rs-x", "size": 5},
        ]
    });
    let err = select_asset(&bad, "windows-x86_64").unwrap_err();
    assert!(err.contains("no installable asset"), "{err}");

    let multi = serde_json::json!({
        "tag_name": "v9",
        "assets": [
            {"name": "arc-rs-a", "size": 1, "browser_download_url": "http://x/a"},
            {"name": "arc-rs-b", "size": 2, "browser_download_url": "http://x/b"},
        ]
    });
    let err = select_asset(&multi, "windows-x86_64").unwrap_err();
    assert!(err.contains("candidates: arc-rs-a, arc-rs-b"), "{err}");

    let no_list = serde_json::json!({"tag_name": "v9"});
    let err = select_asset(&no_list, "windows-x86_64").unwrap_err();
    assert!(err.contains("no \"assets\" list"), "{err}");

    let no_tag = serde_json::json!({"assets": []});
    let err = release_tag(&no_tag).unwrap_err();
    assert!(err.contains("no \"tag_name\""), "{err}");
}

#[test]
fn fetch_latest_release_reads_mock() {
    let (listener, base, _) = mock_listener();
    let doc = release_doc("v9.9.9", "[]");
    run_mock(
        listener,
        vec![json_route("releases/latest", &doc)],
        Arc::new(Mutex::new(Vec::new())),
    );
    let release = fetch_latest_release(&base, "o/r").unwrap();
    assert_eq!(release_tag(&release).unwrap(), "v9.9.9");
}

#[test]
fn fetch_latest_release_reports_http_errors() {
    let (listener, base, _) = mock_listener();
    run_mock(
        listener,
        vec![route("releases/latest", "404 Not Found", b"{}".to_vec())],
        Arc::new(Mutex::new(Vec::new())),
    );
    let err = fetch_latest_release(&base, "o/r").unwrap_err();
    assert!(err.contains("HTTP 404"), "{err}");
    assert!(err.contains("o/r"), "{err}");

    let (listener, base, _) = mock_listener();
    run_mock(
        listener,
        vec![route("releases/latest", "200 OK", b"not json".to_vec())],
        Arc::new(Mutex::new(Vec::new())),
    );
    let err = fetch_latest_release(&base, "o/r").unwrap_err();
    assert!(err.contains("parse release metadata"), "{err}");
}

#[test]
fn download_accepts_matching_size() {
    let (listener, base, _) = mock_listener();
    run_mock(
        listener,
        vec![route("/dl/", "200 OK", b"payload-bytes".to_vec())],
        Arc::new(Mutex::new(Vec::new())),
    );
    let bytes = download_release_bytes(&format!("{base}dl/x"), 13).unwrap();
    assert_eq!(bytes, b"payload-bytes");
}

#[test]
fn download_refuses_mismatch_empty_and_http_errors() {
    let (listener, base, _) = mock_listener();
    run_mock(
        listener,
        vec![route("/dl/", "200 OK", b"short".to_vec())],
        Arc::new(Mutex::new(Vec::new())),
    );
    let err = download_release_bytes(&format!("{base}dl/x"), 50).unwrap_err();
    assert!(err.contains("does not match expected file size"), "{err}");

    let (listener, base, _) = mock_listener();
    run_mock(
        listener,
        vec![route("/dl/", "200 OK", Vec::new())],
        Arc::new(Mutex::new(Vec::new())),
    );
    let err = download_release_bytes(&format!("{base}dl/x"), 0).unwrap_err();
    assert!(err.contains("empty release asset"), "{err}");

    let (listener, base, _) = mock_listener();
    run_mock(
        listener,
        vec![route("/dl/", "500 Internal Server Error", b"boom".to_vec())],
        Arc::new(Mutex::new(Vec::new())),
    );
    let err = download_release_bytes(&format!("{base}dl/x"), 4).unwrap_err();
    assert!(err.contains("download failed"), "{err}");
}

#[test]
fn writability_gate() {
    let tmp = TempDir::new("writable");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"x");
    ensure_executable_writable(&exe).unwrap();
    assert_no_leftovers(&tmp.path);

    set_readonly(&exe, true);
    let err = ensure_executable_writable(&exe).unwrap_err();
    set_readonly(&exe, false);
    assert!(err.contains("not writable"), "{err}");

    let missing_parent = tmp.path.join("nope").join("tool");
    let err = ensure_executable_writable(&missing_parent).unwrap_err();
    assert!(err.contains("not writable"), "{err}");
}

#[test]
fn install_replaces_copy() {
    let tmp = TempDir::new("install");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let backup = install_bytes_at(&exe, b"new-bytes").unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new-bytes");
    if cfg!(windows) {
        let old = backup.expect("windows keeps the old image");
        assert_eq!(std::fs::read(&old).unwrap(), b"old-bytes");
    } else {
        assert!(backup.is_none());
    }
    assert_no_leftovers(&tmp.path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

#[test]
fn install_refuses_readonly_keeping_old() {
    let tmp = TempDir::new("installro");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    set_readonly(&exe, true);
    let result = install_bytes_at(&exe, b"new-bytes");
    set_readonly(&exe, false);
    let err = result.unwrap_err();
    assert!(err.contains("not writable"), "{err}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes");
    assert_no_leftovers(&tmp.path);
}

#[test]
fn prompt_names_library_and_versions() {
    assert_eq!(
        upgrade_prompt("0.1.0", "v0.1.1"),
        "Upgrade \"arc-rs\" from \"0.1.0\" to \"v0.1.1\"?"
    );
}

#[test]
fn status_lines_match_php_without_tty() {
    let preparing = preparing_line(false);
    assert!(preparing.contains("PREPARING"), "{preparing:?}");
    assert!(
        preparing.contains("Preparing to upgrade \"arc-rs\"..."),
        "{preparing:?}"
    );
    assert!(!preparing.contains('\x1b'));
    let upgrading = upgrading_line("stable", false);
    assert!(
        upgrading.contains("Upgrading \"arc-rs\" (on branch \"stable\")."),
        "{upgrading:?}"
    );
    assert!(!upgrading.contains('\x1b'));
    let upgraded = upgraded_line(false);
    assert!(upgraded.contains("UPGRADED"), "{upgraded:?}");
    assert!(
        upgraded.contains("This software is now up to date."),
        "{upgraded:?}"
    );
    assert!(!upgraded.contains('\x1b'));
    assert!(preparing_line(true).contains('\x1b'));
    assert!(upgrading_line("m", true).contains('\x1b'));
    assert!(upgraded_line(true).contains('\x1b'));
}

#[test]
fn detail_lines_are_plain_and_escaped() {
    assert_eq!(
        downloading_line("a.bin", 1234, false),
        "Downloading \"a.bin\" (1,234 byte(s))...\n"
    );
    assert_eq!(
        already_line("0.1.0", "v0.1.1", false),
        "arc-rs \"0.1.0\" is already up to date (latest release: \"v0.1.1\").\n"
    );
    let exe = PathBuf::from("tool.exe");
    assert_eq!(
        replaced_line(&exe, "v0.1.1", None, false),
        "Replaced \"tool.exe\" with arc-rs \"v0.1.1\".\n"
    );
    let old = PathBuf::from("tool.exe.old");
    let both = replaced_line(&exe, "v0.1.1", Some(&old), false);
    assert!(
        both.contains("Previous executable kept as \"tool.exe.old\""),
        "{both:?}"
    );
    let evil = downloading_line("a\u{7}b", 1, false);
    assert!(evil.contains("a<BEL>b"), "{evil:?}");
    assert!(!downloading_line("a", 1, true).contains('\x1b'));
}

#[test]
fn execute_apply_updates_copy_end_to_end() {
    let tmp = TempDir::new("e2e");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let body = b"new-binary-bytes-v999";
    let name = format!("arc-rs-{}.exe", platform_tag());
    let (listener, base, seen) = mock_listener();
    let doc = release_doc(
        "v9.9.9",
        &format!("[{}]", asset_entry(&name, body.len() as u64, &base)),
    );
    run_mock(
        listener,
        vec![
            json_route("releases/latest", &doc),
            route("/dl/", "200 OK", body.to_vec()),
        ],
        seen.clone(),
    );
    let opts = apply_opts(&base, "0.1.0", &exe);
    let report = execute_apply(&tmp.path, &opts, &confirm_yes).unwrap();
    assert!(report.updated);
    assert!(report.update_available);
    assert_eq!(report.latest, "v9.9.9");
    assert_eq!(report.repo, DEFAULT_RELEASE_REPO);
    assert_eq!(report.channel, "stable");
    assert_eq!(std::fs::read(&exe).unwrap(), body);
    assert!(
        report
            .stdout_text
            .contains(&format!("Downloading \"{name}\"")),
        "{:?}",
        report.stdout_text
    );
    assert!(
        report.stdout_text.contains("Replaced "),
        "{:?}",
        report.stdout_text
    );
    assert!(
        report.stderr_text.contains("UPGRADED"),
        "{:?}",
        report.stderr_text
    );
    assert!(saw(&seen, "releases/latest"));
    assert!(saw(&seen, "/dl/"));
    if cfg!(windows) {
        let backup = report.backup.clone().expect("windows keeps the old image");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old-bytes");
        assert!(
            report.stdout_text.contains("Previous executable kept as"),
            "{:?}",
            report.stdout_text
        );
    } else {
        assert!(report.backup.is_none());
    }
    assert_no_leftovers(&tmp.path);
    let json = apply_result_json(&report);
    assert_eq!(json["updated"], serde_json::json!(true));
    assert_eq!(json["asset"]["name"], serde_json::json!(name));
    assert_eq!(json["latest"], serde_json::json!("v9.9.9"));
}

#[test]
fn execute_apply_up_to_date_changes_nothing() {
    let tmp = TempDir::new("uptodate");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let (listener, base, seen) = mock_listener();
    let doc = release_doc("v0.1.0", "[]");
    run_mock(
        listener,
        vec![json_route("releases/latest", &doc)],
        seen.clone(),
    );
    let opts = apply_opts(&base, "0.1.0", &exe);
    let report = execute_apply(&tmp.path, &opts, &confirm_yes).unwrap();
    assert!(!report.updated);
    assert!(!report.update_available);
    assert!(report.asset.is_none());
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes");
    assert!(
        report.stdout_text.contains("already up to date"),
        "{:?}",
        report.stdout_text
    );
    assert!(report.stderr_text.is_empty());
    assert!(saw(&seen, "releases/latest"));
    let json = apply_result_json(&report);
    assert_eq!(json["updated"], serde_json::json!(false));
    assert_eq!(json["asset"], serde_json::Value::Null);
}

#[test]
fn execute_apply_decline_aborts_cleanly() {
    let tmp = TempDir::new("decline");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let body = b"new-bytes";
    let name = format!("arc-rs-{}.exe", platform_tag());
    let (listener, base, seen) = mock_listener();
    let doc = release_doc(
        "v9.9.9",
        &format!("[{}]", asset_entry(&name, body.len() as u64, &base)),
    );
    run_mock(
        listener,
        vec![
            json_route("releases/latest", &doc),
            route("/dl/", "200 OK", body.to_vec()),
        ],
        seen.clone(),
    );
    let opts = apply_opts(&base, "0.1.0", &exe);
    let err = execute_apply(&tmp.path, &opts, &confirm_no).unwrap_err();
    assert!(err.contains("declined"), "{err}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes");
    assert!(saw(&seen, "releases/latest"));
    assert!(!saw(&seen, "/dl/"));
    assert_no_leftovers(&tmp.path);
}

#[test]
fn execute_apply_json_needs_yes() {
    let tmp = TempDir::new("jsonyes");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let (listener, base, seen) = mock_listener();
    let name = format!("arc-rs-{}.exe", platform_tag());
    let doc = release_doc("v9.9.9", &format!("[{}]", asset_entry(&name, 3, &base)));
    run_mock(
        listener,
        vec![json_route("releases/latest", &doc)],
        seen.clone(),
    );
    let mut opts = apply_opts(&base, "0.1.0", &exe);
    opts.json = true;
    let err = execute_apply(&tmp.path, &opts, &confirm_yes).unwrap_err();
    assert!(err.contains("--yes"), "{err}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes");
    assert!(!saw(&seen, "/dl/"));
}

#[test]
fn execute_apply_json_reports_result() {
    let tmp = TempDir::new("jsonresult");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let body = b"new-bytes";
    let name = format!("arc-rs-{}.exe", platform_tag());
    let (listener, base, _) = mock_listener();
    let doc = release_doc(
        "v9.9.9",
        &format!("[{}]", asset_entry(&name, body.len() as u64, &base)),
    );
    run_mock(
        listener,
        vec![
            json_route("releases/latest", &doc),
            route("/dl/", "200 OK", body.to_vec()),
        ],
        Arc::new(Mutex::new(Vec::new())),
    );
    let mut opts = apply_opts(&base, "0.1.0", &exe);
    opts.json = true;
    opts.yes = true;
    let report = execute_apply(&tmp.path, &opts, &confirm_yes).unwrap();
    assert!(report.updated);
    let json = apply_result_json(&report);
    assert_eq!(json["version"], serde_json::json!("0.1.0"));
    assert_eq!(json["repo"], serde_json::json!(DEFAULT_RELEASE_REPO));
    assert_eq!(json["channel"], serde_json::json!("stable"));
    assert_eq!(json["update_available"], serde_json::json!(true));
    assert_eq!(json["asset"]["size"], serde_json::json!(body.len()));
}

#[test]
fn execute_apply_download_failure_keeps_old() {
    let tmp = TempDir::new("dlfail");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let name = format!("arc-rs-{}.exe", platform_tag());
    let (listener, base, _) = mock_listener();
    let doc = release_doc("v9.9.9", &format!("[{}]", asset_entry(&name, 9, &base)));
    run_mock(
        listener,
        vec![
            json_route("releases/latest", &doc),
            route("/dl/", "500 Internal Server Error", b"boom".to_vec()),
        ],
        Arc::new(Mutex::new(Vec::new())),
    );
    let opts = apply_opts(&base, "0.1.0", &exe);
    let err = execute_apply(&tmp.path, &opts, &confirm_yes).unwrap_err();
    assert!(err.contains("download failed"), "{err}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes");
    assert_no_leftovers(&tmp.path);
}

#[test]
fn execute_apply_readonly_fails_before_download() {
    let tmp = TempDir::new("rofail");
    let exe = exe_copy(&tmp.path, "tool-copy.exe", b"old-bytes");
    let name = format!("arc-rs-{}.exe", platform_tag());
    let (listener, base, seen) = mock_listener();
    let doc = release_doc("v9.9.9", &format!("[{}]", asset_entry(&name, 9, &base)));
    run_mock(
        listener,
        vec![
            json_route("releases/latest", &doc),
            route("/dl/", "200 OK", b"new-bytes".to_vec()),
        ],
        seen.clone(),
    );
    let opts = apply_opts(&base, "0.1.0", &exe);
    set_readonly(&exe, true);
    let result = execute_apply(&tmp.path, &opts, &confirm_yes);
    set_readonly(&exe, false);
    let err = result.unwrap_err();
    assert!(err.contains("not writable"), "{err}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes");
    assert!(saw(&seen, "releases/latest"));
    assert!(!saw(&seen, "/dl/"));
    assert_no_leftovers(&tmp.path);
}
