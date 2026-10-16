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

#[test]
fn parses_paths_and_flags() {
    let opts = args_of(&["a.bin", "b.bin"]);
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
    let ParseOutcome::Error(message) = parse_args(&argv(&[])) else {
        panic!("expected error for no paths");
    };
    assert!(message.contains("one or more paths"), "{message}");

    assert!(matches!(
        parse_args(&argv(&["--bogus", "a"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_browse() {
    let opts = args_of(&["--browse", "a"]);
    assert!(opts.browse);
    assert!(!args_of(&["a"]).browse);
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
    let refs = vec![UploadedRef {
        argument: "a.bin".to_string(),
        id: 7,
        phid: "PHID-FILE-x".to_string(),
        name: "a.bin".to_string(),
        uri: "https://h/F7".to_string(),
    }];
    let out = render_uploaded_refs(&refs, false).unwrap();
    assert_eq!(out, "F7 a.bin\nhttps://h/F7\n");
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
    match execute(&std::env::temp_dir(), &argv(&[])) {
        Err(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected parse error"),
    }
    assert_eq!(
        run_in(&std::env::temp_dir(), &argv(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    // Conduit setup failures (deterministic: no token can exist for
    // `.invalid`) escape into a fatal, like aggregate errors.
    assert_eq!(
        run_in(
            &std::env::temp_dir(),
            &argv(&["--conduit-uri", "https://upload-test.invalid/", "f.bin"])
        ),
        std::process::ExitCode::from(255)
    );
}
