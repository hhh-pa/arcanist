//! `download`/`liberate`/`paste`/`upload` flows through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{conduit, download, liberate, paste, upload};

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-patchflow-{tag}-{}-{}",
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

    fn write(&self, name: &str, body: &[u8]) {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

/// Serve canned response bodies in order; returns the base URI and the
/// observed (request line, body) pairs once all are served.
fn serve_seq(canned: Vec<String>) -> (String, std::sync::mpsc::Receiver<Vec<(String, String)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for body in canned {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            let len: usize = head
                .lines()
                .filter_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .next()
                .unwrap_or(0);
            let mut raw = vec![0u8; len];
            stream.read_exact(&mut raw).unwrap();
            seen.push((
                head.lines().next().unwrap_or("").to_string(),
                String::from_utf8(raw).unwrap(),
            ));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        tx.send(seen).unwrap();
    });
    (base, rx)
}

fn seen_requests(rx: &std::sync::mpsc::Receiver<Vec<(String, String)>>) -> Vec<(String, String)> {
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
}

fn client_for(base: &str) -> conduit::Client {
    conduit::Client::new(base, "api-test-token").unwrap()
}

fn offline_client() -> conduit::Client {
    conduit::Client::new("http://127.0.0.1:9/", "t").unwrap()
}

#[test]
fn module_names_match_subcommands() {
    assert_eq!(download::NAME, "download");
    assert_eq!(upload::NAME, "upload");
    assert_eq!(paste::NAME, "paste");
    assert_eq!(liberate::NAME, "liberate");
}

#[test]
fn commands_reject_bad_args_without_io() {
    assert_eq!(download::run(&argv(&[])), ExitCode::FAILURE);
    assert_eq!(download::run(&argv(&["F1", "F2"])), ExitCode::FAILURE);
    assert_eq!(upload::run(&argv(&[])), ExitCode::FAILURE);
    // T1153 R4 (UPLOAD-U15): `--browse f` parses fine; the failure is
    // conduit setup, which escapes the toolset runtime into a fatal.
    // T1153 BROWSE-B21: piped runs carry `--`.
    assert_eq!(
        upload::run(&argv(&["--browse", "f", "--"])),
        ExitCode::from(255)
    );
    assert_eq!(paste::run(&argv(&["P1", "P2"])), ExitCode::FAILURE);
    assert_eq!(paste::run(&argv(&["P1", "--lang", "x"])), ExitCode::FAILURE);
    // T1153 B21/B23: piped runs carry `--` to reach path-count validation.
    assert_eq!(liberate::run(&argv(&["a", "b", "--"])), ExitCode::FAILURE);
}

#[test]
fn help_succeeds_for_all_commands() {
    let fx = Fixture::named("help");
    // T1153 BROWSE-B21: piped `--help` carries `--`.
    assert_eq!(
        download::run_in(fx.path(), &argv(&["--help", "--"])),
        ExitCode::SUCCESS
    );
    assert_eq!(
        upload::run_in(fx.path(), &argv(&["--help", "--"])),
        ExitCode::SUCCESS
    );
    assert_eq!(
        paste::run_in(fx.path(), &argv(&["--help", "--"])),
        ExitCode::SUCCESS
    );
    assert_eq!(
        liberate::run_in(fx.path(), &argv(&["--help", "--"])),
        ExitCode::SUCCESS
    );
    // T1153 BROWSE-B21 (terminator) + BROWSE-B23 (generated help).
    assert!(
        download::execute(fx.path(), &argv(&["--help", "--"]))
            .unwrap()
            .contains("DOWNLOAD WORKFLOW")
    );
    // T1153 R10: `--help` renders the generated workflow help.
    assert!(
        liberate::execute(fx.path(), &argv(&["--help", "--"]))
            .unwrap()
            .contains("LIBERATE WORKFLOW")
    );
}

// --- download ------------------------------------------------------------

fn file_search_hit(data_uri: &str, size: usize) -> String {
    serde_json::json!({
        "result": {
            "data": [{
                "id": 9,
                "phid": "PHID-FILE-9",
                "fields": {
                    "name": "report.txt",
                    "size": size,
                    "dataURI": data_uri,
                    "uri": "/F9",
                },
            }],
        },
        "error_code": null,
    })
    .to_string()
}

#[test]
fn download_fetches_file_search_by_id() {
    let (base, rx) = serve_seq(vec![file_search_hit("http://127.0.0.1:9/x", 3)]);
    let file = download::fetch_file_ref(&client_for(&base), 9).unwrap();
    assert_eq!(file.id, 9);
    assert_eq!(file.name, "report.txt");
    assert_eq!(file.size, 3);
    assert_eq!(file.monogram(), "F9");
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].0.starts_with("POST /api/file.search "),
        "{}",
        seen[0].0
    );
    // Note: the form body is percent-encoded, so match encoding-safe pieces.
    assert!(seen[0].1.contains("ids"), "{}", seen[0].1);
    assert!(seen[0].1.contains('9'), "{}", seen[0].1);
}

#[test]
fn download_missing_file_errors() {
    let (base, _rx) = serve_seq(vec![
        r#"{"result": {"data": []}, "error_code": null}"#.to_string(),
    ]);
    let err = download::fetch_file_ref(&client_for(&base), 404).unwrap_err();
    assert!(err.contains("does not exist"), "{err}");
}

#[test]
fn download_saves_bytes_under_own_name() {
    let fx = Fixture::named("dl");
    let (data_base, data_rx) = serve_seq(vec!["hello-bytes".to_string()]);
    let (api_base, api_rx) = serve_seq(vec![file_search_hit(
        &format!("{data_base}F9/data"),
        "hello-bytes".len(),
    )]);

    let out = download::download_file(&client_for(&api_base), fx.path(), 9, None).unwrap();
    // T1153 DL-D1: statuses go to stderr; the returned text is empty.
    assert_eq!(out, "");
    assert_eq!(
        std::fs::read(fx.dir.join("report.txt")).unwrap(),
        b"hello-bytes"
    );

    assert_eq!(seen_requests(&api_rx).len(), 1);
    let data_seen = seen_requests(&data_rx);
    assert_eq!(data_seen.len(), 1);
    assert!(data_seen[0].0.starts_with("GET "), "{}", data_seen[0].0);
}

#[test]
fn download_honors_as_and_rejects_existing() {
    let fx = Fixture::named("dl-as");
    let (data_base, _data_rx) = serve_seq(vec!["zz".to_string(), "zz".to_string()]);
    let hit = file_search_hit(&format!("{data_base}F9/data"), 2);
    let (api_base, _api_rx) = serve_seq(vec![hit.clone(), hit]);

    let out =
        download::download_file(&client_for(&api_base), fx.path(), 9, Some("custom.bin")).unwrap();
    // T1153 DL-D1: statuses go to stderr; the returned text is empty.
    assert_eq!(out, "");
    assert_eq!(std::fs::read(fx.dir.join("custom.bin")).unwrap(), b"zz");

    let err = download::download_file(&client_for(&api_base), fx.path(), 9, Some("custom.bin"))
        .unwrap_err();
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn download_uniquifies_default_name() {
    let fx = Fixture::named("dl-unique");
    fx.write("report.txt", b"taken");
    let (data_base, _data_rx) = serve_seq(vec!["new".to_string()]);
    let (api_base, _api_rx) = serve_seq(vec![file_search_hit(&format!("{data_base}F9/data"), 3)]);

    let out = download::download_file(&client_for(&api_base), fx.path(), 9, None).unwrap();
    // T1153 DL-D1: statuses go to stderr; the returned text is empty.
    assert_eq!(out, "");
    assert_eq!(std::fs::read(fx.dir.join("report.txt.1")).unwrap(), b"new");
    assert_eq!(std::fs::read(fx.dir.join("report.txt")).unwrap(), b"taken");
}

#[test]
fn download_rejects_size_mismatch() {
    let fx = Fixture::named("dl-size");
    let (data_base, _data_rx) = serve_seq(vec!["short".to_string()]);
    let (api_base, _api_rx) = serve_seq(vec![file_search_hit(&format!("{data_base}F9/data"), 100)]);

    let err = download::download_file(&client_for(&api_base), fx.path(), 9, None).unwrap_err();
    assert!(err.contains("does not match expected"), "{err}");
    // T1153 DL-D7: the size check runs after the bytes land, leaving
    // the corrupt file behind like PHP.
    assert_eq!(std::fs::read(fx.dir.join("report.txt")).unwrap(), b"short");
}

#[test]
fn download_to_stdout_returns_empty_text() {
    let fx = Fixture::named("dl-stdout");
    let (data_base, _data_rx) = serve_seq(vec!["out".to_string()]);
    let (api_base, _api_rx) = serve_seq(vec![file_search_hit(&format!("{data_base}F9/data"), 3)]);

    let out = download::download_file(&client_for(&api_base), fx.path(), 9, Some("-")).unwrap();
    assert_eq!(out, "");
}

// --- upload --------------------------------------------------------------

#[test]
fn upload_sends_allocate_then_missing_chunks() {
    let allocate = r#"{"result": {"filePHID": "PHID-FILE-1", "upload": true}, "error_code": null}"#;
    let chunks = r#"{"result": [
        {"byteStart": 0, "byteEnd": 4, "complete": false},
        {"byteStart": 4, "byteEnd": 4, "complete": true}
    ], "error_code": null}"#;
    let chunk_ok = r#"{"result": {}, "error_code": null}"#;
    let (base, rx) = serve_seq(vec![
        allocate.to_string(),
        chunks.to_string(),
        chunk_ok.to_string(),
    ]);

    let phid = upload::upload_data(&client_for(&base), "t.txt", b"test", true).unwrap();
    assert_eq!(phid, "PHID-FILE-1");

    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 3);
    assert!(seen[0].0.contains("file.allocate"), "{}", seen[0].0);
    assert!(seen[0].1.contains("contentHash"), "{}", seen[0].1);
    assert!(seen[0].1.contains("contentLength"), "{}", seen[0].1);
    assert!(seen[0].1.contains("deleteAfterEpoch"), "{}", seen[0].1);
    assert!(seen[1].0.contains("file.querychunks"), "{}", seen[1].0);
    assert!(seen[2].0.contains("file.uploadchunk"), "{}", seen[2].0);
    // Only the incomplete chunk is sent, base64-encoded ("test" -> dGVzdA==;
    // the padding is percent-encoded in the form body).
    assert!(seen[2].1.contains("dGVzdA"), "{}", seen[2].1);
}

#[test]
fn upload_accepts_server_side_dedup() {
    let (base, rx) = serve_seq(vec![
        r#"{"result": {"filePHID": "PHID-FILE-2", "upload": false}, "error_code": null}"#
            .to_string(),
    ]);
    let phid = upload::upload_data(&client_for(&base), "t.txt", b"test", false).unwrap();
    assert_eq!(phid, "PHID-FILE-2");
    assert_eq!(seen_requests(&rx).len(), 1);
}

#[test]
fn upload_reports_server_refusal() {
    let (base, _rx) = serve_seq(vec![
        r#"{"result": {"filePHID": null, "upload": false}, "error_code": null}"#.to_string(),
    ]);
    let err = upload::upload_data(&client_for(&base), "big.bin", b"x", false).unwrap_err();
    assert!(err.contains("too large"), "{err}");
}

#[test]
fn upload_falls_back_to_file_upload() {
    let (base, rx) = serve_seq(vec![
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "no allocate"}"#
            .to_string(),
        r#"{"result": "PHID-FILE-3", "error_code": null}"#.to_string(),
    ]);
    let phid = upload::upload_data(&client_for(&base), "t.txt", b"hi", false).unwrap();
    assert_eq!(phid, "PHID-FILE-3");
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 2);
    assert!(seen[1].0.contains("file.upload"), "{}", seen[1].0);
    assert!(seen[1].1.contains("data_base64"), "{}", seen[1].1);
}

#[test]
fn upload_paths_resolves_refs_and_prints_json() {
    let fx = Fixture::named("up");
    fx.write("a.txt", b"aaa");
    fx.write("b.txt", b"bbb");
    let search = serde_json::json!({
        "result": {
            "data": [
                {"id": 11, "phid": "PHID-FILE-11", "fields": {"name": "a.txt", "uri": "/F11"}},
                {"id": 12, "phid": "PHID-FILE-12", "fields": {"name": "b.txt", "uri": "/F12"}},
            ],
        },
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![
        r#"{"result": {"filePHID": "PHID-FILE-11", "upload": false}, "error_code": null}"#
            .to_string(),
        r#"{"result": {"filePHID": "PHID-FILE-12", "upload": false}, "error_code": null}"#
            .to_string(),
        search,
    ]);

    let out = upload::upload_paths(
        &client_for(&base),
        fx.path(),
        &["a.txt".to_string(), "b.txt".to_string()],
        false,
        true,
        Some("https://phab.example/api/"),
    )
    .unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc[0]["id"], 11);
    assert_eq!(doc[0]["uri"], "https://phab.example/F11");
    assert_eq!(doc[1]["argument"], "b.txt");
    assert_eq!(doc[1]["phid"], "PHID-FILE-12");
    assert_eq!(seen_requests(&rx).len(), 3);
}

#[test]
fn upload_paths_prints_text_lines() {
    let fx = Fixture::named("up-text");
    fx.write("a.txt", b"aaa");
    let search = serde_json::json!({
        "result": {
            "data": [
                {"id": 11, "phid": "PHID-FILE-11", "fields": {"name": "a.txt", "uri": "/F11"}},
            ],
        },
        "error_code": null,
    })
    .to_string();
    let (base, _rx) = serve_seq(vec![
        r#"{"result": {"filePHID": "PHID-FILE-11", "upload": false}, "error_code": null}"#
            .to_string(),
        search,
    ]);
    let out = upload::upload_paths(
        &client_for(&base),
        fx.path(),
        &["a.txt".to_string()],
        false,
        false,
        Some("https://phab.example/"),
    )
    .unwrap();
    assert!(out.contains("F11 a.txt"), "{out}");
    assert!(out.contains("https://phab.example/F11"), "{out}");
}

#[test]
fn upload_paths_rejects_missing_file_without_io() {
    let fx = Fixture::named("up-missing");
    let err = upload::upload_paths(
        &offline_client(),
        fx.path(),
        &["nope.txt".to_string()],
        false,
        false,
        None,
    )
    .unwrap_err();
    // T1153 UPLOAD-U4: `willUpload()` texts replace the generic read error.
    assert!(err.contains("does not exist"), "{err}");
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
fn upload_browse_opens_uploaded_files() {
    let fx = Fixture::named("up-browse");
    fx.write("a.txt", b"aaa");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(fx.path().join(".git")).unwrap();
    fx.write(".arcconfig", null_browser_config().as_bytes());
    let search = serde_json::json!({
        "result": {
            "data": [
                {"id": 11, "phid": "PHID-FILE-11", "fields": {"name": "a.txt", "uri": "/F11"}},
            ],
        },
        "error_code": null,
    })
    .to_string();
    let (base, _rx) = serve_seq(vec![
        r#"{"result": {"filePHID": "PHID-FILE-11", "upload": false}, "error_code": null}"#
            .to_string(),
        search,
    ]);
    let out = upload::upload_paths_with_browse(
        &client_for(&base),
        fx.path(),
        &["a.txt".to_string()],
        false,
        false,
        true,
        Some("https://phab.example/"),
    )
    .unwrap();
    assert!(out.contains("https://phab.example/F11"), "{out}");
}

// --- paste ---------------------------------------------------------------

#[test]
fn paste_reads_content_through_search() {
    let hit = serde_json::json!({
        "result": {
            "data": [{
                "id": 123,
                "phid": "PHID-PSTE-123",
                "fields": {"title": "t", "uri": "/P123"},
                "attachments": {"content": {"content": "duck list\n"}},
            }],
        },
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![hit]);
    let content = paste::read_paste(&client_for(&base), "P123").unwrap();
    assert_eq!(content, "duck list\n");
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].0.starts_with("POST /api/paste.search "),
        "{}",
        seen[0].0
    );
    assert!(seen[0].1.contains("attachments"), "{}", seen[0].1);
    assert!(seen[0].1.contains("123"), "{}", seen[0].1);
}

#[test]
fn paste_read_missing_errors() {
    let (base, _rx) = serve_seq(vec![
        r#"{"result": {"data": []}, "error_code": null}"#.to_string(),
    ]);
    let err = paste::read_paste(&client_for(&base), "P999").unwrap_err();
    assert!(err.contains("does not exist"), "{err}");
}

#[test]
fn paste_creates_through_paste_create() {
    // T1153 PASTE-P1: PHP creates via `paste.edit` transactions ONLY
    // (`paste.create` is frozen server-side); the re-search follows.
    // (Name kept for traceability; the flow is edit-only now.)
    let search = serde_json::json!({
        "result": {
            "data": [{
                "id": 5,
                "phid": "PHID-PSTE-5",
                "fields": {"title": "title", "uri": "/P5"},
            }],
        },
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![
        r#"{"result": {"object": {"phid": "PHID-PSTE-5"}}, "error_code": null}"#.to_string(),
        search,
    ]);
    let created = paste::create_paste(
        &client_for(&base),
        "title",
        Some("rust"),
        "fn main() {}",
        Some("https://phab.example/api/"),
    )
    .unwrap();
    assert_eq!(created.id, 5);
    assert_eq!(created.uri, "https://phab.example/P5");
    assert_eq!(
        paste::format_created(&created),
        "Created paste P5: https://phab.example/P5\n"
    );
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].0.starts_with("POST /api/paste.edit "),
        "{}",
        seen[0].0
    );
    assert!(seen[0].1.contains("main"), "{}", seen[0].1);
    assert!(seen[0].1.contains("language"), "{}", seen[0].1);
    assert!(seen[0].1.contains("rust"), "{}", seen[0].1);
    assert!(
        seen[1].0.starts_with("POST /api/paste.search "),
        "{}",
        seen[1].0
    );
}

#[test]
fn paste_create_falls_back_to_paste_edit() {
    // T1153 PASTE-P1/PASTE-P12: no `paste.create` attempt and no retry on
    // ANY error (which masked real errors); a `paste.edit` failure
    // surfaces after exactly one call.
    // (Name kept for traceability; nothing falls back anymore.)
    let (base, rx) = serve_seq(vec![
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "bad edit"}"#
            .to_string(),
    ]);
    let err = paste::create_paste(
        &client_for(&base),
        "t",
        None,
        "body",
        Some("https://phab.example/"),
    )
    .unwrap_err();
    assert!(err.contains("bad edit"), "{err}");
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.contains("paste.edit"), "{}", seen[0].0);
    assert!(seen[0].1.contains("transactions"), "{}", seen[0].1);
}

// --- liberate ------------------------------------------------------------

fn empty_input() -> std::io::BufReader<std::io::Cursor<Vec<u8>>> {
    std::io::BufReader::new(std::io::Cursor::new(Vec::new()))
}

fn filled_input(text: &str) -> std::io::BufReader<std::io::Cursor<Vec<u8>>> {
    std::io::BufReader::new(std::io::Cursor::new(text.as_bytes().to_vec()))
}

#[test]
fn liberate_scan_finds_nothing_without_libraries() {
    let fx = Fixture::named("lib-none");
    let err =
        liberate::execute_with_input(fx.path(), &argv(&[]), &mut empty_input(), true).unwrap_err();
    assert!(err.contains("Unable to find any libraries"), "{err}");
}

#[test]
fn liberate_rejects_ancient_and_unknown_versions() {
    let fx = Fixture::named("lib-old");
    std::fs::create_dir_all(fx.dir.join("oldlib")).unwrap();
    std::fs::write(
        fx.dir.join("oldlib").join("__phutil_library_init__.php"),
        "<?php\n",
    )
    .unwrap();
    let err = liberate::execute_with_input(fx.path(), &argv(&["oldlib"]), &mut empty_input(), true)
        .unwrap_err();
    assert!(err.contains("no longer supported"), "{err}");

    std::fs::write(
        fx.dir.join("oldlib").join("__phutil_library_map__.php"),
        "<?php // @phutil-library-version 9\n",
    )
    .unwrap();
    let err = liberate::execute_with_input(fx.path(), &argv(&["oldlib"]), &mut empty_input(), true)
        .unwrap_err();
    assert!(err.contains("Unknown library version '9'"), "{err}");
}

#[test]
fn liberate_create_ignores_rebuild_status() {
    // T1153 CORE-50: PHP at ceb ignores the rebuild status entirely
    // (`ArcanistLiberateWorkflow.php:82-93` discards `liberatePath`'s
    // return): the run succeeds with no rebuild text, even with no
    // rebuild script above the library (DONE goes to the console,
    // like PHP's echo, not the return value).
    let fx = Fixture::named("lib-create");
    std::fs::create_dir_all(fx.dir.join("newlib")).unwrap();
    let out = liberate::execute_with_input(
        fx.path(),
        &argv(&["newlib"]),
        &mut filled_input("my-lib\n"),
        true,
    )
    .unwrap();
    assert!(!out.contains("rebuild-map.php"), "{out}");
    let init =
        std::fs::read_to_string(fx.dir.join("newlib").join("__phutil_library_init__.php")).unwrap();
    assert!(init.contains("phutil_register_library('my-lib'"), "{init}");
}

#[test]
fn liberate_missing_directory_decline_and_accept() {
    let fx = Fixture::named("lib-mkdir");
    // Declining the mkdir prompt aborts like PHP's `ArcanistPrompt`
    // (`ArcanistUserAbortException`); the "does not exist" text stays in
    // the banner, not the error.
    let err =
        liberate::execute_with_input(fx.path(), &argv(&["ghost"]), &mut filled_input("n\n"), true)
            .unwrap_err();
    assert!(err.contains("User aborted the workflow."), "{err}");
    assert!(!fx.dir.join("ghost").exists());

    // PHP only accepts exactly `y`/`n` (lowercased) at toolset prompts
    // (`ArcanistPrompt.php`); anything else re-prompts.
    let out = liberate::execute_with_input(
        fx.path(),
        &argv(&["fresh"]),
        &mut filled_input("y\nfresh-lib\n"),
        true,
    )
    .unwrap();
    assert!(!out.contains("rebuild-map.php"), "{out}");
    assert!(
        fx.dir
            .join("fresh")
            .join("__phutil_library_init__.php")
            .is_file()
    );
}

#[test]
fn liberate_v2_ignores_rebuild_status() {
    // T1153 CORE-50: the rebuild status is ignored entirely (see
    // `liberate_create_ignores_rebuild_status`); the run exits 0.
    let fx = Fixture::named("lib-v2");
    std::fs::create_dir_all(fx.dir.join("v2lib")).unwrap();
    std::fs::write(
        fx.dir.join("v2lib").join("__phutil_library_init__.php"),
        "<?php\n",
    )
    .unwrap();
    std::fs::write(
        fx.dir.join("v2lib").join("__phutil_library_map__.php"),
        "<?php // @phutil-library-version 2\n",
    )
    .unwrap();
    let out = liberate::execute_with_input(fx.path(), &argv(&["v2lib"]), &mut empty_input(), true)
        .unwrap();
    assert!(!out.contains("rebuild-map.php"), "{out}");
    // T1153 B21/B23: piped `run_in` carries `--` (`execute_with_input`
    // models a TTY above, so it needs none).
    assert_eq!(
        liberate::run_in(fx.path(), &argv(&["--", "v2lib"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn liberate_scan_updates_found_library() {
    let fx = Fixture::named("lib-scan");
    std::fs::create_dir_all(fx.dir.join("found")).unwrap();
    std::fs::write(
        fx.dir.join("found").join("__phutil_library_init__.php"),
        "<?php\n",
    )
    .unwrap();
    // v1 (no map): a fatal usage error aborting the workflow, like
    // `arc liberate` (the library was named on the WORK line instead).
    let err =
        liberate::execute_with_input(fx.path(), &argv(&[]), &mut empty_input(), true).unwrap_err();
    assert!(err.contains("no longer supported"), "{err}");
}

// --- T1153 R3 auto-pagination ---------------------------------------------

#[test]
fn r3_dld12_download_follows_file_search_pages() {
    fn hit(id: u32) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "phid": format!("PHID-FILE-{id}"),
            "fields": {
                "name": format!("f{id}.txt"),
                "size": 1,
                "dataURI": "http://127.0.0.1:9/x",
            },
        })
    }
    let page1 = serde_json::json!({
        "result": {"data": [hit(7)], "cursor": {"after": "p2"}},
        "error_code": null,
    })
    .to_string();
    let page2 = serde_json::json!({
        "result": {"data": [hit(9)]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page1, page2]);
    let file = download::fetch_file_ref(&client_for(&base), 9).unwrap();
    assert_eq!(file.id, 9);
    assert_eq!(file.name, "f9.txt");
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 2);
    assert!(seen[0].0.contains("file.search"), "{}", seen[0].0);
    assert!(!seen[0].1.contains("after"), "{}", seen[0].1);
    assert!(seen[1].0.contains("file.search"), "{}", seen[1].0);
    assert!(seen[1].1.contains("after"), "{}", seen[1].1);
}

#[test]
fn r3_uploadu16_resolve_refs_follows_file_search_pages() {
    fn hit(id: u32) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "phid": format!("PHID-FILE-{id}"),
            "fields": {"name": format!("f{id}.txt"), "uri": format!("/F{id}")},
        })
    }
    let page1 = serde_json::json!({
        "result": {"data": [hit(1)], "cursor": {"after": "p2"}},
        "error_code": null,
    })
    .to_string();
    let page2 = serde_json::json!({
        "result": {"data": [hit(2)]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page1, page2]);
    let refs = upload::resolve_refs(
        &client_for(&base),
        &[("f2.txt".to_string(), "PHID-FILE-2".to_string())],
        None,
    )
    .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].id, 2);
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 2);
    assert!(seen[1].1.contains("after"), "{}", seen[1].1);
}
