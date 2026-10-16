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
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--", "F123"]);
    assert_eq!(opts.file_id, 123);
    assert_eq!(opts.save_as, None);

    let opts = args_of(&["--as", "-", "--", "F7"]);
    assert_eq!(opts.file_id, 7);
    assert_eq!(opts.symbol, "F7");
    assert_eq!(opts.save_as, Some("-".to_string()));

    let opts = args_of(&["--", "99"]);
    assert_eq!(opts.file_id, 99);
    assert_eq!(opts.symbol, "99");
}

#[test]
fn rejects_missing_extra_and_bad_symbols() {
    // T1153 BROWSE-B21: `--` present, so validation (not the
    // terminator) reports.
    let ParseOutcome::Error(message) = parse_args(&argv(&["--"])) else {
        panic!("expected error for no file");
    };
    assert!(message.contains("Specify a file"), "{message}");

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
fn parses_file_symbols() {
    assert_eq!(parse_file_symbol("F123").unwrap(), 123);
    assert_eq!(parse_file_symbol("99").unwrap(), 99);
    // T1153 DL-D3: the symbol engine rejects zero and leading zeros.
    assert!(parse_file_symbol("F007").is_err());
    assert!(parse_file_symbol("F0").is_err());
    assert!(parse_file_symbol("0").is_err());
    assert!(parse_file_symbol("007").is_err());
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
    match parse_args(&argv(&["--"])) {
        ParseOutcome::Error(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected parse error"),
    }
    match execute(&std::env::temp_dir(), &argv(&["--"])) {
        Err(message) => assert!(!message.contains("usage:"), "{message}"),
        _ => panic!("expected error"),
    }
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
    let out = download_file_for_symbol(&client, &dir, 9, "F9", None).unwrap();
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
    assert_eq!(parse_file_symbol("F123").unwrap(), 123);
    assert_eq!(parse_file_symbol("123").unwrap(), 123);
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
    let err = download_file_for_symbol(&client, &dir, 9, "F9", None).unwrap_err();
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
    let err = fetch_file_ref_for_symbol(&client, 9, "F9").unwrap_err();
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
    match parse_args(&argv(&["--as", "", "--", "F1"])) {
        ParseOutcome::Args(opts) => assert_eq!(opts.save_as, Some(String::new())),
        _ => panic!("expected args"),
    }
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
    match execute(&std::env::temp_dir(), &argv(&["--help", "--"])) {
        Ok(text) => assert!(text.contains("DOWNLOAD WORKFLOW"), "{text}"),
        Err(message) => panic!("expected help, got {message}"),
    }
}
