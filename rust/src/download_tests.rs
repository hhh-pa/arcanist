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
fn parses_id_and_as() {
    let opts = args_of(&["F123"]);
    assert_eq!(opts.file_id, 123);
    assert_eq!(opts.save_as, None);

    let opts = args_of(&["--as", "-", "--", "F7"]);
    assert_eq!(opts.file_id, 7);
    assert_eq!(opts.symbol, "F7");
    assert_eq!(opts.save_as, Some("-".to_string()));

    let opts = args_of(&["99"]);
    assert_eq!(opts.file_id, 99);
    assert_eq!(opts.symbol, "99");
}

#[test]
fn rejects_missing_extra_and_bad_symbols() {
    let ParseOutcome::Error(message) = parse_args(&argv(&[])) else {
        panic!("expected error for no file");
    };
    assert!(message.contains("Specify a file"), "{message}");

    assert!(matches!(
        parse_args(&argv(&["F1", "F2"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["xyz"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["F1", "--bogus"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_file_symbols() {
    assert_eq!(parse_file_symbol("F123").unwrap(), 123);
    assert_eq!(parse_file_symbol("99").unwrap(), 99);
    assert_eq!(parse_file_symbol("F007").unwrap(), 7);
    // PHP `/^F?\d+$/`: uppercase `F` only, nothing else.
    assert!(parse_file_symbol("f4").is_err());
    assert!(parse_file_symbol(" 99").is_err());
    assert!(parse_file_symbol("99 ").is_err());
    assert!(parse_file_symbol("F").is_err());
    assert!(parse_file_symbol("Fx").is_err());
    assert!(parse_file_symbol("").is_err());
    assert_eq!(
        parse_file_symbol("xyz").unwrap_err(),
        "Specify file by ID, e.g. F123."
    );
    // PHP `(int)` casts saturate instead of failing.
    assert_eq!(
        parse_file_symbol("99999999999999999999999").unwrap(),
        i64::MAX as u64
    );
}

#[test]
fn counts_format_with_thousands_separators() {
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1000), "1,000");
    assert_eq!(format_count(1234567), "1,234,567");
}

#[test]
fn timeouts_grow_with_expected_size() {
    assert_eq!(download_timeout(0), std::time::Duration::from_secs(120));
    assert_eq!(
        download_timeout(32 * 1024),
        std::time::Duration::from_secs(121)
    );
    assert_eq!(
        download_timeout(4 * 1024 * 1024),
        std::time::Duration::from_secs(248)
    );
}

#[test]
fn readable_paths_show_relative_under_dir() {
    let dir = std::env::temp_dir().join("arc-rs-download-readable");
    assert_eq!(readable_path(&dir, &dir.join("report.txt")), "report.txt");
    let outside = std::env::temp_dir().join("elsewhere.bin");
    assert_eq!(readable_path(&dir, &outside), outside.display().to_string());
}

#[test]
fn size_mismatch_uses_grouped_counts() {
    assert_eq!(
        size_mismatch_error(5, 100000),
        "Downloaded file size (5 bytes) does not match expected file size (100,000 bytes). \
         This download may be incomplete or corrupt."
    );
}

#[test]
fn monogram_and_display_name() {
    let file = FileRef {
        id: 5,
        name: String::new(),
        size: 0,
        data_uri: String::new(),
    };
    assert_eq!(file.monogram(), "F5");
    assert_eq!(file.display_name(), "F5");
    let named = FileRef {
        name: "a.txt".to_string(),
        ..file.clone()
    };
    assert_eq!(named.display_name(), "a.txt");
}

#[test]
fn resolves_targets_and_uniquifies() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-download-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = FileRef {
        id: 1,
        name: "report.txt".to_string(),
        size: 3,
        data_uri: String::new(),
    };

    let first = resolve_target(&dir, None, &file).unwrap();
    assert_eq!(first.file_name().unwrap(), "report.txt");
    std::fs::write(&first, "abc").unwrap();
    let second = resolve_target(&dir, None, &file).unwrap();
    assert_eq!(second.file_name().unwrap(), "report.txt.1");

    let explicit = resolve_target(&dir, Some("out.bin"), &file).unwrap();
    assert_eq!(explicit.file_name().unwrap(), "out.bin");
    std::fs::write(&explicit, "x").unwrap();
    assert!(resolve_target(&dir, Some("out.bin"), &file).is_err());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn status_lines_carry_no_markup() {
    let downloading = format_downloading("report.txt", 12_345, "report.txt");
    assert_eq!(
        downloading,
        "Downloading \"report.txt\" (12,345 byte(s)) to \"report.txt\"...\n"
    );
    let saved = format_saved("report.txt", "report.txt");
    assert_eq!(saved, "Saved \"report.txt\" as \"report.txt\".\n");
    for line in [&downloading, &saved] {
        assert!(!line.contains('\x1b'), "{line}");
        assert!(!line.contains("**"), "{line}");
    }
}

#[test]
fn r3_dld12_select_file_hit_matches_by_id() {
    let hits = serde_json::json!([
        {"id": 7, "fields": {}},
        {"id": "9", "fields": {}},
        {"id": 9, "fields": {}},
    ]);
    let hits = hits.as_array().unwrap();
    // Numeric-string ids match (PHP loose keys); last duplicate wins
    // (PHP `ipull` overwrite).
    assert_eq!(
        select_file_hit(hits, 9).unwrap().get("id").unwrap(),
        &serde_json::json!(9)
    );
    assert!(select_file_hit(hits, 404).is_none());
    assert!(select_file_hit(&[], 9).is_none());
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
fn r3_dld12_file_search_follows_after_cursor() {
    fn hit(id: u64) -> serde_json::Value {
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
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let file = fetch_file_ref(&client, 9).unwrap();
    assert_eq!(file.id, 9);
    assert_eq!(file.name, "f9.txt");
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(!seen[0].contains("after"), "{}", seen[0]);
    assert!(seen[1].contains("after"), "{}", seen[1]);
}

#[test]
fn r4_dld2_arg_errors_are_toolset_usage() {
    // `didParseArguments()` raising `ArcanistUsageException` is dead
    // in the toolset path: the live errors are toolset usage (exit
    // 1), so only the presentation changes, never the exit.
    assert!(is_toolset_usage_message(
        "File \"F9\" does not exist, or you do not have permission to view it."
    ));
    assert!(is_toolset_usage_message("File \"/tmp/x\" already exists."));
    // Parse errors carry no usage dump.
    match parse_args(&argv(&[])) {
        ParseOutcome::Error(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected parse error"),
    }
    match execute(&std::env::temp_dir(), &argv(&[])) {
        Err(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected error"),
    }
}

#[test]
fn r4_dld10_conduit_setup_is_fatal() {
    // Conduit setup failures (deterministic: no token can exist for
    // `.invalid`) escape into a fatal, like HTTP failures.
    assert_eq!(
        run_in(
            &std::env::temp_dir(),
            &argv(&["--conduit-uri", "https://download-test.invalid/", "F1"])
        ),
        std::process::ExitCode::from(255)
    );
}

#[test]
fn r4_dld10_http_errors_are_fatal() {
    // HTTP, size-mismatch, and write failures are uncaught
    // `Exception`s: fatal (255), never toolset usage.
    assert!(!is_toolset_usage_message("download failed: HTTP 500"));
    assert!(!is_toolset_usage_message(&size_mismatch_error(1, 2)));
    assert!(!is_toolset_usage_message("failed to write \"x\": boom"));
}
