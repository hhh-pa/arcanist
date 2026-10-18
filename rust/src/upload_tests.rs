use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn args_of(words: &[&str]) -> Options {
    let ParseOutcome::Args(opts) = parse_args(&argv(words)) else {
        panic!("expected args for {words:?}");
    };
    opts
}

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-upload-tests-{tag}-{}-{n}",
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

#[test]
fn parses_paths_and_flags() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["a.bin", "b.bin", "--"]);
    assert_eq!(opts.paths, vec!["a.bin".to_string(), "b.bin".to_string()]);
    assert!(!opts.json);
    assert!(!opts.temporary);

    let opts = args_of(&["--json", "--temporary", "--", "a.bin"]);
    assert!(opts.json);
    assert!(opts.temporary);
    assert_eq!(opts.paths, vec!["a.bin".to_string()]);
}

#[test]
fn requires_paths_and_rejects_unknown_flags() {
    // T1153 BROWSE-B21: `--` present, so validation (not the
    // terminator) reports.
    let outcome = parse_args(&argv(&["--"]));
    assert!(
        matches!(&outcome, ParseOutcome::Error(message) if message.contains("one or more paths")),
        "expected no-paths error"
    );

    assert!(matches!(
        parse_args(&argv(&["--bogus", "a"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_browse() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--browse", "a", "--"]);
    assert!(opts.browse);
    assert!(!args_of(&["a", "--"]).browse);
}

#[test]
fn encodes_base64_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64_encode(b"\x00\xff\x10\x80"), "AP8QgA==");
}

#[test]
fn hashes_sha256_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    // Multi-block input (over 55 and over 64 bytes).
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(
        sha256_hex(&vec![b'a'; 1000]),
        "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
}

#[test]
fn absolutizes_uris() {
    assert_eq!(
        absolute_uri(Some("https://phab.example/api/"), "/F123"),
        "https://phab.example/F123"
    );
    assert_eq!(
        absolute_uri(Some("https://phab.example/"), "https://cdn.example/f"),
        "https://cdn.example/f"
    );
    assert_eq!(absolute_uri(None, "/F1"), "/F1");
    assert_eq!(conduit_root("https://h/api/"), "https://h");
    assert_eq!(conduit_root("https://h/"), "https://h");
    // T1153 UPLOAD-U14: relative joins drop query/fragment; absolute
    // and protocol-relative URIs pass through untouched.
    assert_eq!(
        absolute_uri(Some("https://h/api/"), "/F1?x=1#y"),
        "https://h/F1"
    );
    assert_eq!(
        absolute_uri(Some("https://h/"), "https://cdn.example/f?a=b"),
        "https://cdn.example/f?a=b"
    );
    assert_eq!(
        absolute_uri(Some("https://h/"), "//cdn.example/f"),
        "//cdn.example/f"
    );
}

#[test]
fn conduit_flags_follow_php_truthiness() {
    assert!(!conduit_truthy(None));
    assert!(!conduit_truthy(Some(&serde_json::json!(null))));
    assert!(conduit_truthy(Some(&serde_json::json!(true))));
    assert!(!conduit_truthy(Some(&serde_json::json!(false))));
    assert!(conduit_truthy(Some(&serde_json::json!(1))));
    assert!(!conduit_truthy(Some(&serde_json::json!(0))));
    assert!(!conduit_truthy(Some(&serde_json::json!(""))));
    assert!(!conduit_truthy(Some(&serde_json::json!("0"))));
    assert!(conduit_truthy(Some(&serde_json::json!("yes"))));
}

#[test]
fn formats_uploaded_ref_json() {
    let r = UploadedRef {
        argument: "a.bin".to_string(),
        id: 7,
        phid: "PHID-FILE-x".to_string(),
        name: "a.bin".to_string(),
        uri: "https://h/F7".to_string(),
    };
    assert_eq!(
        r.to_json(),
        serde_json::json!({
            "argument": "a.bin",
            "id": 7,
            "phid": "PHID-FILE-x",
            "name": "a.bin",
            "uri": "https://h/F7",
        })
    );
}

#[test]
fn ref_lines_carry_no_markup() {
    // T1153 UPLOAD-U1: non-JSON output is a RefView per file
    // (`ArcanistRefView.php:117-128`), markup-stripped off-TTY.
    let refs = vec![UploadedRef {
        argument: "a.bin".to_string(),
        id: 7,
        phid: "PHID-FILE-x".to_string(),
        name: "a.bin".to_string(),
        uri: "https://h/F7".to_string(),
    }];
    let out = render_uploaded_refs_ansi(&refs, false, 80, false).unwrap();
    assert_eq!(out, "  *   F7 a.bin\n ://    https://h/F7\n");
    assert!(!out.contains('\x1b'), "{out}");
    assert!(!out.contains("**"), "{out}");
}

#[test]
fn r9_upload_u13_no_browser_text_matches_ceb() {
    // `browse_uploaded_refs` returns the shared scan error when no browser
    // command resolves; it must be the ceb rendering (PHP
    // `ArcanistWorkflow.php:2204-2206` @ ceb082ef).
    assert_eq!(
        crate::diff::NO_BROWSER_MESSAGE,
        "Unable to find a browser command to run. Set 'browser' in your Arcanist config to specify a command to use."
    );
}

/// Serve canned Conduit envelopes in order; returns the base URI and the
/// observed request bodies once all are served.
fn serve_seq(canned: Vec<String>) -> (String, std::sync::mpsc::Receiver<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
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
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut raw = vec![0u8; len];
            stream.read_exact(&mut raw).unwrap();
            seen.push(String::from_utf8(raw).unwrap());
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

#[test]
fn r3_uploadu16_file_search_follows_after_cursor() {
    fn hit(id: u64, name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "phid": format!("PHID-FILE-{id}"),
            "fields": {"name": name, "uri": format!("/F{id}")},
        })
    }
    let page1 = serde_json::json!({
        "result": {"data": [hit(1, "other.bin")], "cursor": {"after": "p2"}},
        "error_code": null,
    })
    .to_string();
    let page2 = serde_json::json!({
        "result": {"data": [hit(2, "a.bin")]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page1, page2]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let refs = resolve_refs(
        &client,
        &[("a.bin".to_string(), "PHID-FILE-2".to_string())],
        None,
    )
    .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].id, 2);
    assert_eq!(refs[0].name, "a.bin");
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(!seen[0].contains("after"), "{}", seen[0]);
    assert!(seen[1].contains("after"), "{}", seen[1]);
}

#[test]
fn r4_uploadu15_aggregate_errors_are_fatal() {
    // `upload` is a toolset workflow: aggregate and resolve errors
    // are uncaught `Exception`s (fatal, 255), while argument errors
    // stay toolset usage (exit 1) with no usage dump.
    // T1153 BROWSE-B21: `--` present, so validation (not the
    // terminator) reports.
    let outcome = execute(&std::env::temp_dir(), &argv(&["--"]));
    assert!(
        matches!(&outcome, Err(message) if !message.contains("usage:")),
        "expected parse error"
    );
    assert_eq!(
        run_in(&std::env::temp_dir(), &argv(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    // Conduit setup failures (deterministic: no token can exist for
    // `.invalid`) escape into a fatal, like aggregate errors.
    // T1153 BROWSE-B21: piped runs carry `--`.
    assert_eq!(
        run_in(
            &std::env::temp_dir(),
            &argv(&[
                "--conduit-uri",
                "https://upload-test.invalid/",
                "f.bin",
                "--"
            ])
        ),
        std::process::ExitCode::from(255)
    );
}

fn upload_u_fixture(name: &str) -> UploadedRef {
    UploadedRef {
        argument: "a.bin".to_string(),
        id: 7,
        phid: "PHID-FILE-x".to_string(),
        name: name.to_string(),
        uri: "https://h/F7".to_string(),
    }
}

#[test]
fn upload_u1_refview_carries_ansi_on_tty() {
    let refs = vec![upload_u_fixture("a.bin")];
    let out = render_uploaded_refs_ansi(&refs, false, 80, true).unwrap();
    assert!(out.contains('\x1b'), "{out}");
    assert!(out.contains("F7"), "{out}");
    assert!(out.contains("a.bin"), "{out}");
    assert!(out.contains("https://h/F7"), "{out}");
}

#[test]
fn upload_u2_title_truncates_to_terminal_width() {
    // Usable width is 80 - 6 (marker) - 3 (`F7` + space) = 71 glyphs;
    // `PhutilUTF8StringTruncator` keeps 70 + `…` with no word boundary.
    let refs = vec![upload_u_fixture(&"a".repeat(100))];
    let out = render_uploaded_refs_ansi(&refs, false, 80, false).unwrap();
    let first = out.lines().next().unwrap();
    assert_eq!(first, format!("  *   F7 {}…", "a".repeat(70)));
}

#[test]
fn upload_u4_local_file_error_texts() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-upload-u4-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        read_upload_bytes(&dir, "missing.bin").unwrap_err(),
        "Unable to upload file: path \"missing.bin\" does not exist."
    );
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    assert_eq!(
        read_upload_bytes(&dir, "sub").unwrap_err(),
        "Unable to upload file: path \"sub\" is not a file."
    );
    // Not-readable: best effort (root and Windows ignore permission bits).
    let locked = dir.join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let probed = read_upload_bytes(&dir, "locked.bin");
        let denied = "Unable to upload file: path \"locked.bin\" is not readable.";
        assert!(probed.is_ok() || probed.unwrap_err() == denied);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    std::fs::write(dir.join("ok.bin"), b"data").unwrap();
    assert_eq!(read_upload_bytes(&dir, "ok.bin").unwrap(), b"data");
    std::fs::remove_dir_all(&dir).ok();
}

fn upload_u_envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

#[test]
fn upload_u5_empty_phid_falls_back_to_legacy_upload() {
    // `upload:true` + empty PHID stays in `$files` and runs the legacy
    // `file.upload` loop (`ArcanistFileUploader.php:159-166,183-194`).
    let (base, rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": ""}"#),
        upload_u_envelope(r#""PHID-FILE-9""#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    assert_eq!(
        upload_data(&client, "a.bin", b"data", false).unwrap(),
        "PHID-FILE-9"
    );
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("contentLength"), "{}", seen[0]);
    assert!(seen[1].contains("data_base64"), "{}", seen[1]);
    assert!(seen[1].contains("ZGF0YQ"), "{}", seen[1]);
}

#[test]
fn upload_u6_non_array_querychunks_errors() {
    // Rule 2(a) KEEP: PHP `foreach` over a non-array silently completes
    // the upload with zero chunks (data loss); the clean error stays.
    let (base, _rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
        upload_u_envelope(r#"{"chunks": []}"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_data(&client, "a.bin", b"data", false).unwrap_err();
    assert!(err.contains("unexpected response"), "{err}");
}

#[test]
fn upload_u7_out_of_range_chunk_errors() {
    // Rule 2(a) KEEP: PHP `fseek`s past EOF and uploads an empty chunk
    // (silent corruption); the range error stays.
    let (base, _rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
        upload_u_envelope(r#"[{"complete": false, "byteStart": 5, "byteEnd": 99}]"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_data(&client, "a.bin", b"0123456789", false).unwrap_err();
    assert!(err.contains("out-of-range"), "{err}");
}

#[test]
fn upload_u8_missing_chunk_bounds_error() {
    // Rule 2(a) KEEP: PHP reads null bounds (notices, then wrong/odd
    // data); same failure outcome, stable text.
    for chunks in [
        r#"[{"complete": false, "byteEnd": 4}]"#,
        r#"[{"complete": false, "byteStart": 0}]"#,
    ] {
        let (base, _rx) = serve_seq(vec![
            upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
            upload_u_envelope(chunks),
        ]);
        let client = crate::conduit::Client::new(&base, "t").unwrap();
        let err = upload_data(&client, "a.bin", b"data", false).unwrap_err();
        assert!(err.contains("without byte"), "{err}");
    }
}

#[test]
fn upload_u9_json_escapes_non_ascii_like_php() {
    // `PhutilJSON::encodeFormattedValue()`: pretty layout (matches serde)
    // with `\uXXXX` escapes (lowercase, surrogate pairs past the BMP).
    let refs = vec![upload_u_fixture("café \u{1F600}")];
    let out = render_uploaded_refs_ansi(&refs, true, 80, false).unwrap();
    assert!(out.contains(r#""name": "caf\u00e9 \ud83d\ude00""#), "{out}");
    assert!(!out.contains('é'), "{out}");
    // Structural characters and ASCII are untouched.
    assert!(out.starts_with("[\n  {"), "{out}");
    assert!(out.ends_with("\n]\n"), "{out}");
}

#[test]
fn upload_u10_progress_bar_renders_like_php() {
    // `[%-{max}.{max}s] %5.1f%%`, 12 columns of chrome, `-`/`~` tip.
    assert_eq!(
        render_chunk_bar(0, 4, 78),
        format!("[{}]   0.0%", " ".repeat(66))
    );
    let full = render_chunk_bar(4, 4, 78);
    assert_eq!(full, format!("[{}] 100.0%", "=".repeat(66)));
    // Fractional tip: 1/4 of 66 = 16.5 -> 16 `=` + `-` (index 1).
    let quarter = render_chunk_bar(1, 4, 78);
    assert_eq!(
        quarter,
        format!("[{}-{}]  25.0%", "=".repeat(16), " ".repeat(49))
    );
    assert_eq!(quarter.len(), 66 + 9);
}

#[test]
fn upload_u11_past_eof_range_errors_not_short() {
    // Rule 2(a) KEEP (with UPLOAD-U7): PHP's short `fread` past EOF sends
    // truncated data silently; the whole-file read errors instead.
    let (base, _rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
        upload_u_envelope(r#"[{"complete": false, "byteStart": 0, "byteEnd": 5}]"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_data(&client, "a.bin", b"ab", false).unwrap_err();
    assert!(err.contains("out-of-range"), "{err}");
}

#[test]
fn upload_u12_allocates_sequentially_in_order() {
    // Rule 2(c) KEEP: PHP allocates with `FutureIterator->limit(4)` but
    // yields results in key order; same calls, same order, no byte effect.
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-upload-u12-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("first.bin"), b"1").unwrap();
    std::fs::write(dir.join("second.bin"), b"2").unwrap();
    let (base, rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": false, "filePHID": "PHID-FILE-1"}"#),
        upload_u_envelope(r#"{"upload": false, "filePHID": "PHID-FILE-2"}"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let paths = ["first.bin".to_string(), "second.bin".to_string()];
    let uploads = collect_uploads(&client, &dir, &paths, false).unwrap();
    assert_eq!(uploads.len(), 2);
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("first.bin"), "{}", seen[0]);
    assert!(seen[1].contains("second.bin"), "{}", seen[1]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn upx_b21_upload_requires_terminator_when_piped() {
    // Piped without `--`: the terminator reports, even for `--help`
    // and missing paths (PHP's `parseFull` runs before
    // `didParseArguments`).
    assert!(matches!(
        parse_args_with_tty(&argv(&["a.bin"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--help"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&[]), false),
        ParseOutcome::MissingTerminator
    ));
    // Unknown flags still report first.
    assert!(matches!(
        parse_args_with_tty(&argv(&["--bogus"]), false),
        ParseOutcome::Error(_)
    ));
    // `--` (or a TTY) proceeds to normal validation.
    assert!(matches!(
        parse_args_with_tty(&argv(&["--", "a.bin"]), false),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["a.bin"]), true),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--"]), false),
        ParseOutcome::Error(_)
    ));
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn upx_b23_upload_help_matches_generated_format() {
    assert_eq!(
        usage_text(false),
        "UPLOAD WORKFLOW\n\
         \n\
         upload [options] -- file [file ...]\n\
         Upload files.\n\
         \n\
         Upload one or more files from local disk to Phabricator.\n\
         \n\
         \x20     --browse\n\
         \x20         After the upload completes, open the files in a web browser.\n\
         \n\
         \x20     --json\n\
         \x20         Output upload information in JSON format.\n\
         \n\
         \x20     --temporary\n\
         \x20         Mark the file as temporary. Temporary files will be deleted after\n\
         \x20         24 hours.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `--help` still exits zero once `--` satisfies the terminator.
    let text = execute(&std::env::temp_dir(), &argv(&["--help", "--"])).expect("expected help");
    assert!(text.contains("UPLOAD WORKFLOW"), "{text}");
}

// -( T1170 COV5: close upload.rs )------------------------------------------------

#[test]
#[should_panic(expected = "expected args")]
fn args_of_panics_on_parse_error() {
    args_of(&["--"]);
}

#[test]
fn conduit_uri_flag_takes_space_equals_and_missing_forms() {
    for words in [&["--conduit-uri"][..], &["--conduit-uri", ""][..]] {
        let outcome = parse_args_with_tty(&argv(words), true);
        assert!(matches!(outcome, ParseOutcome::Error(_)), "{words:?}");
    }
    let opts = args_of(&["--conduit-uri=https://h/", "a.bin", "--"]);
    assert_eq!(opts.conduit_uri, Some("https://h/".to_string()));
    let opts = args_of(&["--conduit-uri", "https://h/", "a.bin", "--"]);
    assert_eq!(opts.conduit_uri, Some("https://h/".to_string()));
}

#[test]
fn php_escape_keeps_short_escapes_and_slashes() {
    assert_eq!(
        php_escape_json_text("{\"a\":\"x\\ny\\/z\"}", true),
        "{\"a\":\"x\\ny\\/z\"}"
    );
    assert_eq!(
        php_escape_json_text("{\"a\":\"x\\ny/z\",\"b\":\"caf\u{e9}\"}", false),
        "{\"a\":\"x\\ny/z\",\"b\":\"caf\\u00e9\"}"
    );
}

#[test]
fn phid_of_reads_strings_objects_and_missing() {
    assert_eq!(
        phid_of(&serde_json::json!("PHID-FILE-1")),
        Some("PHID-FILE-1".to_string())
    );
    assert_eq!(phid_of(&serde_json::json!("")), None);
    assert_eq!(
        phid_of(&serde_json::json!({"phid": "PHID-FILE-1"})),
        Some("PHID-FILE-1".to_string())
    );
    assert_eq!(
        phid_of(&serde_json::json!({"filePHID": "PHID-FILE-2"})),
        Some("PHID-FILE-2".to_string())
    );
    assert_eq!(phid_of(&serde_json::json!({"phid": ""})), None);
    assert_eq!(phid_of(&serde_json::json!({"other": 1})), None);
    assert_eq!(phid_of(&serde_json::json!(7)), None);
    assert_eq!(phid_of(&serde_json::json!(null)), None);
}

#[test]
fn conduit_flags_treat_arrays_and_objects_as_nonempty() {
    assert!(conduit_truthy(Some(&serde_json::json!([0]))));
    assert!(!conduit_truthy(Some(&serde_json::json!([]))));
    assert!(conduit_truthy(Some(&serde_json::json!({"a": 0}))));
    assert!(!conduit_truthy(Some(&serde_json::json!({}))));
    assert!(conduit_truthy(Some(&serde_json::json!(2.5))));
    assert!(!conduit_truthy(Some(&serde_json::json!(0.0))));
}

#[test]
fn querychunks_failure_reports_the_conduit_error() {
    let (base, _rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "chunks boom"}"#
            .to_string(),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_data(&client, "a.bin", b"data", false).unwrap_err();
    assert!(err.contains("chunks boom"), "{err}");
}

#[test]
fn progress_bar_width_throttle_and_erase() {
    assert!(ChunkProgress::width() <= 78);
    let mut bar = ChunkProgress::new();
    bar.set_total(4);
    bar.update(1);
    // A fresh draw timestamp throttles the next redraw.
    bar.last_update = Some(std::time::Instant::now());
    bar.redraw();
    // A drawn bar erases on finish.
    bar.drawn = true;
    bar.finish();
    ChunkProgress::new().finish();
}

#[test]
fn chunks_upload_missing_ranges_and_resume() {
    // One finished chunk (resume announce) plus one missing range.
    let (base, rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
        upload_u_envelope(
            r#"[{"complete": true, "byteStart": 0, "byteEnd": 5},
                {"complete": false, "byteStart": 5, "byteEnd": 10}]"#,
        ),
        upload_u_envelope(r#"{"ok": true}"#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    assert_eq!(
        upload_data(&client, "a.bin", b"0123456789", true).unwrap(),
        "PHID-FILE-1"
    );
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].contains("deleteAfterEpoch"), "{}", seen[0]);
    assert!(seen[2].contains("NTY3ODk"), "{}", seen[2]);
}

#[test]
fn chunk_upload_failure_reports_chunks_error() {
    let (base, _rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": true, "filePHID": "PHID-FILE-1"}"#),
        upload_u_envelope(r#"[{"complete": false, "byteStart": 0, "byteEnd": 4}]"#),
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "chunk boom"}"#
            .to_string(),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_data(&client, "a.bin", b"data", false).unwrap_err();
    assert!(err.contains("Unable to upload file chunks"), "{err}");
    assert!(err.contains("chunk boom"), "{err}");
}

#[test]
fn refused_files_and_old_servers_take_legacy_paths() {
    // `upload:false` with an empty PHID: the server refused the file.
    let (base, _rx) = serve_seq(vec![upload_u_envelope(
        r#"{"upload": false, "filePHID": ""}"#,
    )]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_data(&client, "big.bin", b"data", false).unwrap_err();
    assert!(err.contains("refused to accept"), "{err}");
    // `file.allocate` missing (old server): straight to `file.upload`.
    let (base, rx) = serve_seq(vec![
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "no allocate"}"#
            .to_string(),
        upload_u_envelope(r#""PHID-FILE-3""#),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    assert_eq!(
        upload_data(&client, "a.bin", b"data", false).unwrap(),
        "PHID-FILE-3"
    );
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].contains("data_base64"), "{}", seen[1]);
}

#[test]
fn non_alpha_scheme_is_not_a_domain() {
    assert_eq!(
        absolute_uri(Some("https://h/"), "9x://foo/bar"),
        "https://h9x://foo/bar"
    );
}

#[test]
fn failed_search_propagates_the_conduit_error() {
    let (base, _rx) = serve_seq(vec![
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "search boom"}"#
            .to_string(),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = resolve_refs(
        &client,
        &[("a.bin".to_string(), "PHID-FILE-1".to_string())],
        None,
    )
    .unwrap_err();
    assert!(err.contains("search boom"), "{err}");
}

#[test]
fn upload_paths_renders_refs_for_each_file() {
    let tmp = TempDir::new("paths");
    std::fs::write(tmp.path().join("a.bin"), b"1").unwrap();
    let (base, rx) = serve_seq(vec![
        upload_u_envelope(r#"{"upload": false, "filePHID": "PHID-FILE-1"}"#),
        serde_json::json!({
            "result": {"data": [{
                "id": 1, "phid": "PHID-FILE-1",
                "fields": {"name": "a.bin", "uri": "/F1"},
            }]},
            "error_code": null,
        })
        .to_string(),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let out = upload_paths(
        &client,
        tmp.path(),
        &["a.bin".to_string()],
        false,
        false,
        Some("https://phab.example/api/"),
    )
    .unwrap();
    assert!(out.contains("F1"), "{out}");
    assert!(out.contains("https://phab.example/F1"), "{out}");
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
}

#[test]
fn collect_errors_report_the_first_failure() {
    let tmp = TempDir::new("collect");
    std::fs::write(tmp.path().join("ok.bin"), b"1").unwrap();
    let (base, _rx) = serve_seq(vec![upload_u_envelope(
        r#"{"upload": false, "filePHID": "PHID-FILE-1"}"#,
    )]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = upload_paths(
        &client,
        tmp.path(),
        &["missing.bin".to_string(), "ok.bin".to_string()],
        false,
        false,
        None,
    )
    .unwrap_err();
    assert!(err.contains("missing.bin"), "{err}");
}

#[test]
fn run_reports_usage_errors_without_browsing() {
    assert_eq!(run(&argv(&["--bogus"])), std::process::ExitCode::FAILURE);
}

/// Non-permission read failures surface the `readBytes()` open-failure
/// text. `/proc/self/mem` stats as a regular file but every read
/// faults (EIO) — the only deterministic trigger, hence Linux-only
/// (macOS/Windows have no equivalent). Skips when `/proc` is
/// unavailable (some containers).
#[cfg(target_os = "linux")]
#[test]
fn upload_read_error_reports_open_failure() {
    if std::fs::symlink_metadata("/proc/self/mem").is_err() {
        return;
    }
    let tmp = TempDir::new("read-error");
    let err = read_upload_bytes(tmp.path(), "/proc/self/mem").unwrap_err();
    assert!(err.contains("failed to open"), "{err}");
}
