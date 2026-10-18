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

fn expect_dl_error(words: &[&str], what: &str) -> String {
    let ParseOutcome::Error(message) = parse_args(&argv(words)) else {
        panic!("expected {what}");
    };
    message
}

fn expect_dl_tty_error(words: &[&str], what: &str) -> String {
    let ParseOutcome::Error(message) = parse_args_with_tty(&argv(words), true) else {
        panic!("expected {what}");
    };
    message
}

fn expect_dl_args(words: &[&str]) -> Options {
    let ParseOutcome::Args(opts) = parse_args(&argv(words)) else {
        panic!("expected args");
    };
    opts
}

fn expect_dl_execute_error(words: &[&str]) -> String {
    match execute(&std::env::temp_dir(), &argv(words)) {
        Err(message) => message,
        _ => panic!("expected error"),
    }
}

fn expect_dl_help(words: &[&str]) -> String {
    match execute(&std::env::temp_dir(), &argv(words)) {
        Ok(text) => text,
        Err(message) => panic!("expected help, got {message}"),
    }
}

#[test]
fn parses_id_and_as() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--", "F123"]);
    assert_eq!(opts.file, FileSymbol::Id(123));
    assert_eq!(opts.save_as, None);

    let opts = args_of(&["--as", "-", "--", "F7"]);
    assert_eq!(opts.file, FileSymbol::Id(7));
    assert_eq!(opts.symbol, "F7");
    assert_eq!(opts.save_as, Some("-".to_string()));

    let opts = args_of(&["--", "99"]);
    assert_eq!(opts.file, FileSymbol::Id(99));
    assert_eq!(opts.symbol, "99");
}

#[test]
fn rejects_missing_extra_and_bad_symbols() {
    // T1153 BROWSE-B21: `--` present, so validation (not the
    // terminator) reports.
    let message = expect_dl_error(&["--"], "error for no file");
    // T1153 DL-3: the live `runWorkflow()` no-argv text.
    assert_eq!(message, "Specify a file to download, like \"F123\".");

    assert!(matches!(
        parse_args(&argv(&["--", "F1", "F2"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["--", "xyz"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["F1", "--bogus"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn dl4_unknown_flag_uses_toolset_text() {
    // T1153 DL-4: unknown flags throw the full value
    // (`PhutilArgumentParser.php:819-825`); post-`--` leftovers
    // ride the wildcard into symbol validation instead.
    let message = expect_dl_tty_error(&["--bogus"], "unknown-flag error");
    assert_eq!(
        message,
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    assert!(matches!(
        parse_args(&argv(&["--", "--bogus"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_file_symbols() {
    assert_eq!(parse_file_symbol("F123").unwrap(), FileSymbol::Id(123));
    assert_eq!(parse_file_symbol("99").unwrap(), FileSymbol::Id(99));
    // T1153 DL-1: the engine prefix is `[Ff]?`, and file PHIDs
    // resolve through the `phids` search.
    assert_eq!(parse_file_symbol("f4").unwrap(), FileSymbol::Id(4));
    assert_eq!(
        parse_file_symbol("PHID-FILE-abcdef").unwrap(),
        FileSymbol::Phid("PHID-FILE-abcdef".to_string())
    );
    // T1153 DL-D3: the symbol engine rejects zero and leading zeros.
    assert!(parse_file_symbol("F007").is_err());
    assert!(parse_file_symbol("f007").is_err());
    assert!(parse_file_symbol("F0").is_err());
    assert!(parse_file_symbol("0").is_err());
    assert!(parse_file_symbol("007").is_err());
    // No trimming, no bare prefix, no doubled prefix.
    assert!(parse_file_symbol(" 99").is_err());
    assert!(parse_file_symbol("99 ").is_err());
    assert!(parse_file_symbol("F").is_err());
    assert!(parse_file_symbol("f").is_err());
    assert!(parse_file_symbol("Fx").is_err());
    assert!(parse_file_symbol("FF1").is_err());
    assert!(parse_file_symbol("").is_err());
    // T1153 DL-1: the PHID type is literal-uppercase `FILE` with a
    // non-empty `\S+` tail.
    assert!(parse_file_symbol("PHID-FILE-").is_err());
    assert!(parse_file_symbol("phid-file-abc").is_err());
    assert!(parse_file_symbol("PHID-DIFF-abc").is_err());
    assert!(parse_file_symbol("PHID-FILE-a b").is_err());
    // T1153 DL-2: garbage reports the symbol-engine text.
    assert_eq!(
        parse_file_symbol("xyz").unwrap_err(),
        "The format of symbol \"xyz\" is unrecognized. Expected a monogram like \"X123\", or an ID like \"123\", or a PHID."
    );
    // PHP `(int)` casts saturate instead of failing.
    assert_eq!(
        parse_file_symbol("99999999999999999999999").unwrap(),
        FileSymbol::Id(i64::MAX as u64)
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
    // T1153 DL-D1: the messages render inside blue `DATA`/`DONE` log
    // lines on stderr; the log engine appends the newline.
    let downloading = format_downloading("report.txt", 12_345, "report.txt");
    assert_eq!(
        downloading,
        "Downloading \"report.txt\" (12,345 byte(s)) to \"report.txt\"..."
    );
    let saved = format_saved("report.txt", "report.txt");
    assert_eq!(saved, "Saved \"report.txt\" as \"report.txt\".");
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
    // T1153 BROWSE-B21: `--` present, so validation (not the
    // terminator) reports.
    let message = expect_dl_error(&["--"], "parse error");
    assert!(!message.contains("usage:"), "{message}");
    let message = expect_dl_execute_error(&["--"]);
    assert!(!message.contains("usage:"), "{message}");
}

#[test]
fn r4_dld10_conduit_setup_is_fatal() {
    // Conduit setup failures (deterministic: no token can exist for
    // `.invalid`) escape into a fatal, like HTTP failures.
    assert_eq!(
        // T1153 BROWSE-B21: piped runs carry `--`.
        run_in(
            &std::env::temp_dir(),
            &argv(&[
                "--conduit-uri",
                "https://download-test.invalid/",
                "--",
                "F1"
            ])
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

// -( T1153 Wave 2: download retrocompat )-------------------------------------

fn dl_tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-dl-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Serve one raw byte body over HTTP; returns its URL.
fn serve_bytes_once(body: Vec<u8>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read_exact(&mut byte).is_err() {
                return;
            }
            head.push(byte[0]);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    });
    url
}

fn dl_file(id: u64, name: &str, size: u64, data_uri: &str) -> FileRef {
    FileRef {
        id,
        name: name.to_string(),
        size,
        data_uri: data_uri.to_string(),
    }
}

#[test]
fn dl_d1_dl_d15_file_download_returns_empty_text() {
    // T1153 DL-D1: statuses go to stderr, so the returned stdout
    // text is empty. T1153 DL-D15: the 100 KiB body streams through
    // the 32 KiB chunk loop, landing byte-identical.
    let body: Vec<u8> = (0..100_000u32).map(|n| (n % 251) as u8).collect();
    let url = serve_bytes_once(body.clone());
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "f9.bin", "size": body.len(), "dataURI": url},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let dir = dl_tempdir("ok");
    let out = download_file_for_symbol(&client, &dir, &FileSymbol::Id(9), "F9", None).unwrap();
    assert_eq!(out, "");
    assert_eq!(std::fs::read(dir.join("f9.bin")).unwrap(), body);
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dl_d3_zero_and_leading_zeros_are_unrecognized() {
    for symbol in ["F0", "0", "F007", "007"] {
        assert_eq!(
            parse_file_symbol(symbol).unwrap_err(),
            format!(
                "The format of symbol \"{symbol}\" is unrecognized. Expected a monogram like \"X123\", or an ID like \"123\", or a PHID."
            ),
            "{symbol}"
        );
    }
    assert_eq!(parse_file_symbol("F123").unwrap(), FileSymbol::Id(123));
    assert_eq!(parse_file_symbol("123").unwrap(), FileSymbol::Id(123));
}

#[test]
fn dl1_lowercase_and_phid_symbols_parse() {
    // T1153 DL-1: `[Ff]?` IDs keep the original spelling for error
    // echo; PHIDs ride through to the `phids` search.
    let opts = args_of(&["--", "f123"]);
    assert_eq!(opts.file, FileSymbol::Id(123));
    assert_eq!(opts.symbol, "f123");
    let opts = args_of(&["--", "PHID-FILE-x1y2"]);
    assert_eq!(opts.file, FileSymbol::Phid("PHID-FILE-x1y2".to_string()));
    assert_eq!(opts.symbol, "PHID-FILE-x1y2");
}

#[test]
fn dl1_phid_fetch_searches_by_phid() {
    // T1153 DL-1: PHID symbols query `file.search` with `phids` and
    // match on `phid` (last duplicate wins, like `ipull`).
    let page = serde_json::json!({
        "result": {"data": [
            {"id": 7, "phid": "PHID-FILE-x1y2",
             "fields": {"name": "old.bin", "size": 1,
                        "dataURI": "http://127.0.0.1:9/old"}},
            {"id": 9, "phid": "PHID-FILE-x1y2",
             "fields": {"name": "new.bin", "size": 2,
                        "dataURI": "http://127.0.0.1:9/new"}},
        ]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let file = fetch_file_ref_for_symbol(
        &client,
        &FileSymbol::Phid("PHID-FILE-x1y2".to_string()),
        "PHID-FILE-x1y2",
    )
    .unwrap();
    assert_eq!(file.id, 9);
    assert_eq!(file.name, "new.bin");
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("phids"), "{}", seen[0]);
    assert!(seen[0].contains("PHID-FILE-x1y2"), "{}", seen[0]);

    // A PHID with no hit echoes the symbol as given.
    let empty = serde_json::json!({
        "result": {"data": []},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![empty]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = fetch_file_ref_for_symbol(
        &client,
        &FileSymbol::Phid("PHID-FILE-nope".to_string()),
        "PHID-FILE-nope",
    )
    .unwrap_err();
    assert_eq!(
        err,
        "File \"PHID-FILE-nope\" does not exist, or you do not have permission to view it."
    );
    assert!(is_toolset_usage_message(&err));
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn dl_d4_backslash_kept_in_basename() {
    assert_eq!(basename("dir/report.txt"), "report.txt");
    // PHP `basename()` only strips `/` on Linux.
    #[cfg(not(windows))]
    assert_eq!(basename("dir\\report.txt"), "dir\\report.txt");
}

#[test]
#[cfg(unix)]
fn dl_d5_as_broken_symlink_already_exists() {
    let dir = dl_tempdir("aslink");
    std::os::unix::fs::symlink("missing.bin", dir.join("link.bin")).unwrap();
    let file = dl_file(1, "f.bin", 1, "http://127.0.0.1:9/x");
    let err = resolve_target(&dir, Some("link.bin"), &file).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn dl_d6_unique_name_skips_broken_symlink() {
    let dir = dl_tempdir("uniqlink");
    std::os::unix::fs::symlink("missing.bin", dir.join("f.bin")).unwrap();
    let file = dl_file(1, "f.bin", 1, "http://127.0.0.1:9/x");
    let target = resolve_target(&dir, None, &file).unwrap();
    assert_eq!(target.file_name().unwrap(), "f.bin.1");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dl_d7_size_mismatch_leaves_corrupt_file() {
    // PHP checks `filesize()` after the bytes land, so a mismatch
    // errors but leaves the corrupt file behind.
    let body = vec![7u8; 16];
    let url = serve_bytes_once(body.clone());
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "short.bin", "size": 64, "dataURI": url},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let dir = dl_tempdir("mismatch");
    let err = download_file_for_symbol(&client, &dir, &FileSymbol::Id(9), "F9", None).unwrap_err();
    assert!(err.contains("does not match expected file size"), "{err}");
    assert_eq!(std::fs::read(dir.join("short.bin")).unwrap(), body);
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dl_d8_stdout_timeout_is_300s() {
    // The stdout path never overrides the timeout, keeping the
    // `BaseHTTPFuture` default of 300 seconds.
    assert_eq!(DOWNLOAD_TIMEOUT, std::time::Duration::from_secs(300));
}

#[test]
fn dl_d9_progress_marker_matches_sprintf() {
    let sink = ProgressSink::new(1000, false);
    assert_eq!(sink.marker(), " 0.0%");
    assert_eq!(sink.message(), "[ 0.0%] Working...");
    let mut sink = ProgressSink::new(16, false);
    sink.completed = 1;
    // 6.25% rounds half away from zero, like PHP's `%.1f` (Rust
    // float display would round half to even: `6.2`).
    assert_eq!(sink.marker(), " 6.3%");
    sink.completed = 8;
    assert_eq!(sink.marker(), " 50.0%");
    sink.completed = 16;
    assert_eq!(sink.marker(), " 100.0%");
    // Overruns clamp to 100%.
    sink.completed = 99;
    assert_eq!(sink.marker(), " 100.0%");
    // The first update always publishes; completion prints a newline.
    let mut sink = ProgressSink::new(8, false);
    assert!(sink.should_publish());
    sink.made_progress(8);
    sink.complete();
}

#[test]
fn dl_d11_missing_data_uri_is_fatal_class() {
    // PHP builds an `HTTPSFuture` on the empty URI and dies in the
    // transport (fatal); the clean error here must route the same
    // way (never toolset usage).
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "x.bin", "size": 1},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = fetch_file_ref_for_symbol(&client, &FileSymbol::Id(9), "F9").unwrap_err();
    assert!(err.contains("has no download URI"), "{err}");
    assert!(!is_toolset_usage_message(&err));
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn dl_d14_empty_as_means_cwd_exists() {
    // A separate `--as ''` is an empty value (like `--as=`), which
    // resolves to the working directory: `already exists`.
    let opts = expect_dl_args(&["--as", "", "--", "F1"]);
    assert_eq!(opts.save_as, Some(String::new()));
    let dir = dl_tempdir("emptyas");
    let file = dl_file(1, "f.bin", 1, "http://127.0.0.1:9/x");
    let err = resolve_target(&dir, Some(""), &file).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
    // A missing value still errors.
    assert!(matches!(
        parse_args(&argv(&["--as"])),
        ParseOutcome::Error(_)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dl_d16_race_message_is_verbatim() {
    // The ceb `setDownloadPath()` race message carries a `%s` with
    // no argument (TOCTOU-only), reproduced byte for byte.
    let dir = dl_tempdir("race");
    let taken = dir.join("taken.bin");
    std::fs::write(&taken, "x").unwrap();
    assert_eq!(
        create_target(&taken, false).unwrap_err(),
        "Specified download path \"%s\" already exists, refusing to overwrite."
    );
    assert!(create_target(&dir.join("free.bin"), false).is_ok());
    assert!(create_target(&taken, true).is_ok());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dl_d17_display_paths_are_canonical() {
    let dir = dl_tempdir("canon");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let file = dl_file(1, "f.txt", 1, "http://127.0.0.1:9/x");
    // `..` segments resolve away, like `Filesystem::resolvePath()`.
    let target = resolve_target(&dir, Some("sub/../f.txt"), &file).unwrap();
    assert_eq!(target.file_name().unwrap(), "f.txt");
    assert_eq!(
        target.parent().unwrap(),
        dir.canonicalize().unwrap().as_path()
    );
    assert_eq!(readable_path(&dir, &target), "f.txt");
    // The directory itself (e.g. `--as ''`) shows absolute.
    assert_eq!(
        readable_path(&dir, &dir.canonicalize().unwrap()),
        dir.canonicalize().unwrap().display().to_string()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dl_b21_terminator_ordering() {
    // Piped without `--`: the terminator reports, even for `--help`
    // and missing files (PHP's `parseFull` runs before
    // `didParseArguments`).
    assert!(matches!(
        parse_args_with_tty(&argv(&["F1"]), false),
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
        parse_args_with_tty(&argv(&["--", "F1"]), false),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["F1"]), true),
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

/// Serve one raw status line with an empty body; returns its URL.
fn serve_status_once(status: u16) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read_exact(&mut byte).is_err() {
                return;
            }
            head.push(byte[0]);
        }
        let response =
            format!("HTTP/1.1 {status} Oops\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(response.as_bytes()).unwrap();
    });
    url
}

/// Serve a body shorter than its declared length, then close early;
/// returns its URL.
fn serve_truncated_once(declared: usize, body: Vec<u8>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read_exact(&mut byte).is_err() {
                return;
            }
            head.push(byte[0]);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    });
    url
}

/// A loopback URL nothing listens on: bind a port, then release it.
fn closed_port_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}/f")
}

// -( Coverage: args, targets, transport edges )---------------------------------

#[test]
fn parses_attached_flag_values() {
    let opts = args_of(&["--as=out.bin", "--conduit-uri=http://x/", "--", "F1"]);
    assert_eq!(opts.save_as, Some("out.bin".to_string()));
    assert_eq!(opts.conduit_uri, Some("http://x/".to_string()));
    // A missing `--conduit-uri` value errors immediately.
    let message = expect_dl_error(&["--conduit-uri"], "missing-value error");
    assert_eq!(message, "--conduit-uri requires a value");
}

#[test]
fn canonical_target_climbs_past_missing_parents() {
    let dir = dl_tempdir("climb");
    let file = dl_file(1, "f.bin", 1, "http://127.0.0.1:9/x");
    // `nosub` does not exist, but its parent does: the target
    // resolves through the nearest existing ancestor.
    let target = resolve_target(&dir.join("nosub"), Some("f.bin"), &file).unwrap();
    assert_eq!(
        target,
        dir.canonicalize().unwrap().join("nosub").join("f.bin")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn canonical_target_falls_back_to_lexical_join() {
    // A relative directory with no existing ancestor resolves
    // lexically (no filesystem writes involved).
    let dir = std::path::Path::new("arc-rs-dl-no-such-dir-xyz");
    let file = dl_file(1, "f.bin", 1, "http://127.0.0.1:9/x");
    let target = resolve_target(dir, Some("f.bin"), &file).unwrap();
    assert_eq!(target, dir.join("f.bin"));
}

#[test]
fn resolve_unique_skips_twice_taken_names() {
    let dir = dl_tempdir("uniq2");
    let file = dl_file(1, "f.bin", 1, "http://127.0.0.1:9/x");
    std::fs::write(dir.join("f.bin"), "a").unwrap();
    std::fs::write(dir.join("f.bin.1"), "b").unwrap();
    let target = resolve_target(&dir, None, &file).unwrap();
    assert_eq!(target.file_name().unwrap(), "f.bin.2");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn progress_sink_throttles_and_overwrites() {
    // Piped sinks publish at most every 2s: the immediate second
    // update only accumulates.
    let mut sink = ProgressSink::new(8, false);
    sink.made_progress(4);
    sink.made_progress(4);
    assert_eq!(sink.completed, 8);
    // TTY sinks overwrite the line in place.
    let mut sink = ProgressSink::new(8, true);
    sink.made_progress(8);
    assert_eq!(sink.completed, 8);
    sink.complete();
}

#[test]
fn fail_download_silences_sink_and_removes_partial() {
    let dir = dl_tempdir("faildl");
    let target = dir.join("partial.bin");
    std::fs::write(&target, "half").unwrap();
    let mut sink = Some(ProgressSink::new(10, false));
    let message = fail_download(&mut sink, &target, "boom".to_string());
    assert_eq!(message, "boom");
    assert!(sink.is_none());
    assert!(!target.exists());
    // Without a sink the partial still goes.
    let target = dir.join("partial2.bin");
    std::fs::write(&target, "half").unwrap();
    let mut none: Option<ProgressSink> = None;
    assert_eq!(
        fail_download(&mut none, &target, "bang".to_string()),
        "bang"
    );
    assert!(!target.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn create_target_reports_open_failures() {
    let dir = dl_tempdir("createrr");
    let err = create_target(&dir.join("nosub").join("f.bin"), true).unwrap_err();
    assert!(err.contains("failed to open"), "{err}");
    assert!(err.contains("f.bin"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_response_reports_transport_and_status_failures() {
    let dir = dl_tempdir("openerr");
    let target = dir.join("f.bin");
    let mut sink: Option<ProgressSink> = None;
    let err = open_download_response(
        &closed_port_url(),
        std::time::Duration::from_secs(5),
        &mut sink,
        &target,
    )
    .unwrap_err();
    assert!(err.starts_with("download failed: "), "{err}");
    // A sink fails along with the transport.
    let mut sink = Some(ProgressSink::new(4 * 1024 * 1024 + 1, false));
    let err = open_download_response(
        &closed_port_url(),
        std::time::Duration::from_secs(5),
        &mut sink,
        &target,
    )
    .unwrap_err();
    assert!(err.starts_with("download failed: "), "{err}");
    assert!(sink.is_none());
    // Non-2xx statuses fail with the status line.
    let mut sink: Option<ProgressSink> = None;
    let err = open_download_response(
        &serve_status_once(500),
        std::time::Duration::from_secs(5),
        &mut sink,
        &target,
    )
    .unwrap_err();
    assert!(err.contains("HTTP 500"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pump_reports_write_failures() {
    let dir = dl_tempdir("pumperr");
    let url = serve_bytes_once(b"hello".to_vec());
    let target = dir.join("f.bin");
    let mut sink: Option<ProgressSink> = None;
    let mut response =
        open_download_response(&url, std::time::Duration::from_secs(5), &mut sink, &target)
            .unwrap();
    let ro = dir.join("ro.bin");
    std::fs::write(&ro, "x").unwrap();
    let mut out = std::fs::File::open(&ro).unwrap();
    let err = pump_download_response(&mut response, &mut out, &mut sink, &target).unwrap_err();
    assert!(err.contains("failed to write"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pump_drives_progress_sink() {
    let dir = dl_tempdir("pumpsink");
    let url = serve_bytes_once(b"hello".to_vec());
    let target = dir.join("f.bin");
    let mut into: Option<ProgressSink> = None;
    let mut response =
        open_download_response(&url, std::time::Duration::from_secs(5), &mut into, &target)
            .unwrap();
    let mut out = create_target(&target, true).unwrap();
    let mut sink = Some(ProgressSink::new(5, false));
    let written = pump_download_response(&mut response, &mut out, &mut sink, &target).unwrap();
    assert_eq!(written, 5);
    assert_eq!(sink.as_ref().unwrap().completed, 5);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pump_reports_truncated_bodies() {
    let dir = dl_tempdir("pumptrunc");
    let url = serve_truncated_once(100, b"0123456789".to_vec());
    let target = dir.join("f.bin");
    let mut into: Option<ProgressSink> = None;
    let mut response =
        open_download_response(&url, std::time::Duration::from_secs(5), &mut into, &target)
            .unwrap();
    let mut out = create_target(&target, true).unwrap();
    let mut sink: Option<ProgressSink> = None;
    let err = pump_download_response(&mut response, &mut out, &mut sink, &target).unwrap_err();
    assert!(err.starts_with("download failed: "), "{err}");
    assert!(!target.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn download_to_path_fails_sink_on_create_error() {
    let dir = dl_tempdir("tocreate");
    let url = serve_bytes_once(b"hello".to_vec());
    // The target's parent is missing, so creation fails after the
    // response opens; only the sink fails, like PHP.
    let target = dir.join("nosub").join("f.bin");
    let err = download_to_path(
        &url,
        std::time::Duration::from_secs(5),
        &target,
        false,
        5 * 1024 * 1024,
        false,
    )
    .unwrap_err();
    assert!(err.contains("failed to open"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn download_to_path_completes_sink() {
    let dir = dl_tempdir("tocomplete");
    let body = b"hello".to_vec();
    let url = serve_bytes_once(body.clone());
    let target = dir.join("f.bin");
    let written = download_to_path(
        &url,
        std::time::Duration::from_secs(5),
        &target,
        true,
        5 * 1024 * 1024,
        false,
    )
    .unwrap();
    assert_eq!(written, body.len() as u64);
    assert_eq!(std::fs::read(&target).unwrap(), body);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stdout_path_reports_size_mismatch() {
    let body = vec![7u8; 16];
    let url = serve_bytes_once(body);
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "short.bin", "size": 64, "dataURI": url},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let dir = dl_tempdir("stdoutmm");
    let err =
        download_file_for_symbol(&client, &dir, &FileSymbol::Id(9), "F9", Some("-")).unwrap_err();
    assert!(err.contains("does not match expected file size"), "{err}");
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stdout_path_writes_matching_bytes() {
    // Matching sizes stream the body to stdout (captured) with empty
    // returned text.
    let body = vec![9u8; 32];
    let url = serve_bytes_once(body);
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "out.bin", "size": 32, "dataURI": url},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let dir = dl_tempdir("stdoutok");
    let out = download_file_for_symbol(&client, &dir, &FileSymbol::Id(9), "F9", Some("-")).unwrap();
    assert_eq!(out, "");
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn file_download_propagates_transport_errors() {
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "f9.bin", "size": 4, "dataURI": closed_port_url()},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let dir = dl_tempdir("transport");
    let err = download_file_for_symbol(&client, &dir, &FileSymbol::Id(9), "F9", None).unwrap_err();
    assert!(err.starts_with("download failed: "), "{err}");
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn download_file_resolves_plain_ids() {
    // The plain-ID wrapper lands the same bytes as the symbol path.
    let body: Vec<u8> = (0..1000u32).map(|n| (n % 251) as u8).collect();
    let url = serve_bytes_once(body.clone());
    let page = serde_json::json!({
        "result": {"data": [{
            "id": 9,
            "phid": "PHID-FILE-9",
            "fields": {"name": "f9.bin", "size": body.len(), "dataURI": url},
        }]},
        "error_code": null,
    })
    .to_string();
    let (base, rx) = serve_seq(vec![page]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let dir = dl_tempdir("plainid");
    let out = download_file(&client, &dir, 9, None).unwrap();
    assert_eq!(out, "");
    assert_eq!(std::fs::read(dir.join("f9.bin")).unwrap(), body);
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_in_reports_help_and_usage_errors() {
    // `--help` prints usage and exits zero.
    assert_eq!(
        run_in(&std::env::temp_dir(), &argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    // Parse errors are toolset usage (exit 1).
    assert_eq!(
        run_in(&std::env::temp_dir(), &argv(&["--"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_dispatches_through_cwd() {
    assert_eq!(
        run(&argv(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn dl_b23_help_matches_generated_format() {
    assert_eq!(
        usage_text(false),
        "DOWNLOAD WORKFLOW\n\
         \n\
         download [options] -- file\n\
         Download a file to local disk.\n\
         \n\
         Download a file to local disk.\n\
         \n\
         \x20     --as path\n\
         \x20         Save the file to a specific location.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `--help` still exits zero once `--` satisfies the terminator.
    let text = expect_dl_help(&["--help", "--"]);
    assert!(text.contains("DOWNLOAD WORKFLOW"), "{text}");
}

// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "expected args for")]
fn args_of_panics_on_invalid_argv() {
    // `args_of` unwraps `Args`: a bare `--` (no file) panics.
    args_of(&["--"]);
}

#[test]
fn serve_helpers_release_aborted_connections() {
    // A client that drops mid-headers takes each serve helper's
    // early-return arm instead of blocking forever: once the handler
    // returns, its listener is gone and the port refuses.
    for url in [
        serve_bytes_once(b"hi".to_vec()),
        serve_status_once(200),
        serve_truncated_once(9, b"short".to_vec()),
    ] {
        let addr = url
            .trim_start_matches("http://")
            .trim_end_matches("/f")
            .to_string();
        let mut stream = std::net::TcpStream::connect(&addr).unwrap();
        use std::io::Write as _;
        stream.write_all(b"X").unwrap();
        drop(stream);
        // Poll until the handler returns and drops its listener; the
        // bound keeps a stuck handler from hanging the suite. The
        // sleep runs first so the wait is unconditional (T1170 band
        // J: a check-first sleep flakes under coverage).
        for _ in 0..500 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if std::net::TcpStream::connect(&addr).is_err() {
                break;
            }
        }
        let released = std::net::TcpStream::connect(&addr).is_err();
        assert!(released, "aborted {url} never released");
    }
}
// -( T1176 S5: defensive-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "expected error for no file")]
fn rejects_missing_extra_and_bad_symbols_defensive_args_arm() {
    // `expect_dl_error` unwraps `Error`: a valid file symbol panics.
    expect_dl_error(&["--", "F1"], "error for no file");
}

#[test]
#[should_panic(expected = "expected unknown-flag error")]
fn dl4_unknown_flag_uses_toolset_text_defensive_args_arm() {
    // `expect_dl_tty_error` unwraps `Error`: a valid file symbol panics.
    expect_dl_tty_error(&["--", "F1"], "unknown-flag error");
}

#[test]
#[should_panic(expected = "expected args")]
fn dl_d14_empty_as_means_cwd_exists_defensive_error_arm() {
    // `expect_dl_args` unwraps `Args`: a missing file symbol panics.
    expect_dl_args(&["--"]);
}

#[test]
#[should_panic(expected = "expected error")]
fn r4_dld2_arg_errors_are_toolset_usage_defensive_ok_arm() {
    // `expect_dl_execute_error` unwraps `Err`: `--help` succeeds.
    expect_dl_execute_error(&["--help", "--"]);
}

#[test]
#[should_panic(expected = "expected help, got")]
fn dl_b23_help_matches_generated_format_defensive_error_arm() {
    // `expect_dl_help` unwraps `Ok`: a missing file symbol panics.
    expect_dl_help(&["--"]);
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
fn execute_downloads_through_configured_client() {
    let dir = dl_tempdir("live");
    let data_url = serve_bytes_once(b"payload-bytes".to_vec());
    let hit = serde_json::json!({
        "id": 7,
        "phid": "PHID-FILE-7",
        "fields": {"name": "f7.txt", "size": 13, "dataURI": data_url},
    });
    let page = serde_json::json!({"result": {"data": [hit]}, "error_code": null}).to_string();
    let (base, rx) = serve_seq(vec![page]);
    let arcrc = fake_arcrc(&dir, &base);
    let out = execute_with_user_file(
        &dir,
        &argv(&["--conduit-uri", base.as_str(), "--", "F7"]),
        Some(&arcrc),
    )
    .unwrap();
    assert_eq!(out, String::new());
    assert_eq!(std::fs::read(dir.join("f7.txt")).unwrap(), b"payload-bytes");
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}
