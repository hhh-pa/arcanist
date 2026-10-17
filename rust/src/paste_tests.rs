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
            "arc-rs-paste-tests-{tag}-{}-{n}",
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
fn parses_create_options() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--title", "t", "--lang", "rust", "--input", "f.txt", "--"]);
    assert_eq!(opts.title, Some("t".to_string()));
    assert_eq!(opts.lang, Some("rust".to_string()));
    assert_eq!(opts.input, Some("f.txt".to_string()));
    assert_eq!(opts.symbol, None);
}

#[test]
fn parses_read_symbol() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["P123", "--"]);
    assert_eq!(opts.symbol, Some("P123".to_string()));
}

#[test]
fn rejects_read_mode_creation_flags() {
    // T1153 BROWSE-B21: `--` present, so validation (not the
    // terminator) reports.
    for words in [
        &["P1", "--lang", "x", "--"][..],
        &["P1", "--title", "x", "--"][..],
        &["P1", "--input", "f", "--"][..],
        &["P1", "--browse", "--"][..],
    ] {
        let outcome = parse_args(&argv(words));
        assert!(
            matches!(&outcome, ParseOutcome::Error(message) if message.contains("not supported when reading")),
            "expected read-mode error for {words:?}"
        );
    }
    assert!(matches!(
        parse_args(&argv(&["P1", "P2", "--"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_browse_on_create() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--browse", "--"]);
    assert!(opts.browse);
    assert!(!args_of(&["--"]).browse);
}

#[test]
fn parses_paste_symbols() {
    assert_eq!(
        parse_paste_symbol("P123").unwrap(),
        PasteSymbol::Ids(vec![123])
    );
    assert_eq!(parse_paste_symbol("p4").unwrap(), PasteSymbol::Ids(vec![4]));
    assert_eq!(
        parse_paste_symbol("PHID-PSTE-abc").unwrap(),
        PasteSymbol::Phids(vec!["PHID-PSTE-abc".to_string()])
    );
    assert!(parse_paste_symbol("Px").is_err());
    assert!(parse_paste_symbol("").is_err());
}

#[test]
fn absolutizes_uris() {
    assert_eq!(
        absolute_uri(Some("https://phab.example/api/"), "/P9"),
        "https://phab.example/P9"
    );
    assert_eq!(absolute_uri(None, "/P9"), "/P9");
    // T1153 PASTE-P17: relative joins drop query/fragment; absolute
    // and protocol-relative URIs pass through untouched.
    assert_eq!(
        absolute_uri(Some("https://h/api/"), "/P9?x=1#y"),
        "https://h/P9"
    );
    assert_eq!(
        absolute_uri(Some("https://h/"), "https://cdn.example/p?a=b"),
        "https://cdn.example/p?a=b"
    );
    assert_eq!(
        absolute_uri(Some("https://h/"), "//cdn.example/p"),
        "//cdn.example/p"
    );
}

// T1153 PASTE-P1: the `paste.create` response helpers are gone with the
// `paste.create` call (PHP uses `paste.edit` transactions only), so the
// `reads_created_ids_and_phids` test went with them.

/// Serve canned Conduit envelopes in order; returns the base URI and the
/// observed (request line, body) pairs once all are served.
fn serve_seq(canned: Vec<String>) -> (String, std::sync::mpsc::Receiver<Vec<(String, String)>>) {
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
            let line = head.lines().next().unwrap_or("").to_string();
            seen.push((line, String::from_utf8(raw).unwrap()));
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

fn paste_p_envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

fn paste_p_search_hit(id: u64, title: &str) -> String {
    paste_p_envelope(&format!(
        r#"{{"data": [{{"id": {id}, "phid": "PHID-PSTE-{id}", "fields": {{"title": "{title}", "uri": "/P{id}"}}}}]}}"#
    ))
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
fn browse_opens_the_created_paste() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-paste-browse-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcconfig"), null_browser_config()).unwrap();

    // T1153 PASTE-P1/PASTE-P16: create via `paste.edit`, then the
    // mandatory re-search; T1153 PASTE-P2: stdout is a RefView.
    let edit = paste_p_envelope(r#"{"object": {"id": 5, "phid": "PHID-PSTE-5"}}"#);
    let (base, _rx) = serve_seq(vec![edit, paste_p_search_hit(5, "t")]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--browse", "--"]);
    // T1153 PASTE-P3: parts carry the text plus the browse URI (the CLI
    // prints first, then browses).
    let parts = create_paste_parts(&dir, &client, &opts, Some("hello")).unwrap();
    assert!(parts.text.contains("P5"), "{}", parts.text);
    assert!(parts.text.contains("/P5"), "{}", parts.text);
    let uri = parts.browse_uri.clone().unwrap();
    assert!(uri.contains("/P5"), "{uri}");
    let configured = crate::browser::configured_browser(&dir);
    crate::browser::open_uris_in_browser(std::slice::from_ref(&uri), configured.as_deref())
        .unwrap();

    // A failing browser command fails the open with PHP's error.
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"browser": ["definitely-not-a-browser-xyz"]}"#,
    )
    .unwrap();
    let configured = crate::browser::configured_browser(&dir);
    let err =
        crate::browser::open_uris_in_browser(std::slice::from_ref(&uri), configured.as_deref())
            .unwrap_err();
    assert!(err.contains("Failed to open URI"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn created_line_carries_no_markup() {
    let created = CreatedPaste {
        id: 5,
        uri: "https://h.example/P5".to_string(),
    };
    let out = format_created(&created);
    assert_eq!(out, "Created paste P5: https://h.example/P5\n");
    assert!(!out.contains('\x1b'), "{out}");
    assert!(!out.contains("**"), "{out}");
}

#[test]
fn paste_p1_creates_via_edit_only() {
    // No `paste.create` call: `paste.edit` transactions, then the re-search.
    let edit = paste_p_envelope(r#"{"object": {"id": 5, "phid": "PHID-PSTE-5"}}"#);
    let (base, rx) = serve_seq(vec![edit, paste_p_search_hit(5, "hello")]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let created = create_paste(
        &client,
        "hello",
        Some("rust"),
        "fn main() {}",
        Some("https://phab.example/api/"),
    )
    .unwrap();
    assert_eq!(created.id, 5);
    assert_eq!(created.uri, "https://phab.example/P5");
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].0.contains("paste.edit"), "{}", seen[0].0);
    assert!(seen[0].1.contains("transactions"), "{}", seen[0].1);
    assert!(seen[0].1.contains("language"), "{}", seen[0].1);
    assert!(seen[1].0.contains("paste.search"), "{}", seen[1].0);
    assert!(!seen[0].0.contains("paste.create"), "{}", seen[0].0);
}

#[test]
fn paste_p2_created_view_is_a_refview() {
    let detail = CreatedPasteDetail {
        id: 5,
        uri: "https://h/P5".to_string(),
        title: Some("hello".to_string()),
    };
    let out = format_created_detail(&detail, 80, false);
    assert_eq!(out, "  *   P5 hello\n ://    https://h/P5\n");
    let untitled = CreatedPasteDetail {
        id: 5,
        uri: "https://h/P5".to_string(),
        title: None,
    };
    let out = format_created_detail(&untitled, 80, false);
    assert_eq!(out, "  *   P5\n ://    https://h/P5\n");
}

#[test]
fn paste_p4_read_prints_content_raw() {
    let hit = paste_p_envelope(
        r#"{"data": [{"id": 1, "attachments": {"content": {"content": "no-newline"}}}]}"#,
    );
    let (base, _rx) = serve_seq(vec![hit]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    assert_eq!(read_paste(&client, "P1").unwrap(), "no-newline");
}

#[test]
fn paste_p5_empty_content_is_sent() {
    // The server decides on `text:''`; no local refusal.
    let edit = paste_p_envelope(r#"{"object": {"phid": "PHID-PSTE-5"}}"#);
    let (base, rx) = serve_seq(vec![edit, paste_p_search_hit(5, "t")]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let created = create_paste(&client, "t", None, "", None).unwrap();
    assert_eq!(created.id, 5);
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].1.contains("transactions"), "{}", seen[0].1);
}

#[test]
fn paste_p6_symbol_whitespace_rejected() {
    for text in [" P1", "P1 ", " 123 ", "\tP1\n"] {
        assert_eq!(
            parse_paste_symbol(text).unwrap_err(),
            unrecognized_symbol(text)
        );
    }
}

#[test]
fn paste_p7_zero_and_leading_zeros_rejected() {
    for text in ["P0", "p0", "0", "00", "007", "P007"] {
        assert!(
            parse_paste_symbol(text).is_err(),
            "{text} should be unrecognized"
        );
    }
    assert_eq!(parse_paste_symbol("P1").unwrap(), PasteSymbol::Ids(vec![1]));
    assert_eq!(
        parse_paste_symbol("10").unwrap(),
        PasteSymbol::Ids(vec![10])
    );
}

#[test]
fn paste_p8_overflow_digits_saturate() {
    assert_eq!(
        parse_paste_symbol("99999999999999999999999").unwrap(),
        PasteSymbol::Ids(vec![i64::MAX as u64])
    );
    // Between `i64::MAX` and `u64::MAX` still saturates (`(int)` cast).
    assert_eq!(
        parse_paste_symbol("10000000000000000000").unwrap(),
        PasteSymbol::Ids(vec![i64::MAX as u64])
    );
    assert_eq!(
        parse_paste_symbol("9223372036854775807").unwrap(),
        PasteSymbol::Ids(vec![9223372036854775807])
    );
}

#[test]
fn paste_p9_only_pste_phids_search() {
    assert_eq!(
        parse_paste_symbol("PHID-PSTE-abc123").unwrap(),
        PasteSymbol::Phids(vec!["PHID-PSTE-abc123".to_string()])
    );
    for text in ["PHID-USER-x", "PHID-FILE-x", "PHID-PSTE-", "PHID-"] {
        assert!(
            parse_paste_symbol(text).is_err(),
            "{text} should be unrecognized"
        );
    }
}

#[test]
fn paste_p10_bad_symbol_text_matches_php() {
    assert_eq!(
        parse_paste_symbol("Px").unwrap_err(),
        "The format of symbol \"Px\" is unrecognized. Expected a monogram like \
         \"X123\", or an ID like \"123\", or a PHID."
    );
}

#[test]
fn paste_p11_missing_content_echoes_empty() {
    let hit = paste_p_envelope(r#"{"data": [{"id": 1}]}"#);
    let (base, _rx) = serve_seq(vec![hit]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    assert_eq!(read_paste(&client, "P1").unwrap(), "");
}

#[test]
fn paste_p13_input_read_error_texts() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-paste-p13-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let canonical = std::fs::canonicalize(&dir).unwrap();
    let display = canonical.join("missing.txt").to_string_lossy().to_string();
    assert_eq!(
        read_input_file(&dir, "missing.txt").unwrap_err(),
        format!("Filesystem path \"{display}\" does not exist.")
    );
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let sub = canonical.join("sub").to_string_lossy().to_string();
    assert_eq!(
        read_input_file(&dir, "sub").unwrap_err(),
        format!("Requested path '{sub}' is not a file.")
    );
    std::fs::write(dir.join("ok.txt"), b"data").unwrap();
    assert_eq!(read_input_file(&dir, "ok.txt").unwrap(), b"data");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn paste_p14_non_utf8_input_sends_empty_params() {
    // PHP's `json_encode()` fails, so the server sees an empty request and
    // decides; the empty-params call must carry no transactions.
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-paste-p14-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("latin1.txt"), b"caf\xe9").unwrap();
    let (base, rx) = serve_seq(vec![
        r#"{"result": null, "error_code": "ERR-CONDUIT-CORE", "error_info": "validation failed"}"#
            .to_string(),
    ]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    // T1153 BROWSE-B21: piped runs carry `--`.
    let opts = args_of(&["--input", "latin1.txt", "--"]);
    let err = create_paste_parts(&dir, &client, &opts, None).unwrap_err();
    assert!(err.message.contains("validation failed"), "{}", err.message);
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.contains("paste.edit"), "{}", seen[0].0);
    assert!(!seen[0].1.contains("transactions"), "{}", seen[0].1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn paste_p15_stdin_hint_bytes() {
    // `writeWaitingForInput()`: blue `INPUT` status on stderr. The live
    // call is TTY-gated (`read_stdin_bytes`); pin the rendered bytes here.
    assert_eq!(
        crate::terminal::format_log("blue", "INPUT", "Waiting for input on stdin...", false),
        " INPUT  Waiting for input on stdin...\n"
    );
}

#[test]
fn paste_p16_create_always_researches_by_phid() {
    // Even when `paste.edit` reports an id, the displayed paste comes from
    // the re-search (`loadPasteForSymbol($paste_phid)`).
    let edit = paste_p_envelope(r#"{"object": {"id": 99, "phid": "PHID-PSTE-6"}}"#);
    let (base, rx) = serve_seq(vec![edit, paste_p_search_hit(6, "six")]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let detail = create_paste_detail(&client, "t", None, "body", None).unwrap();
    assert_eq!(detail.id, 6);
    assert_eq!(detail.title, Some("six".to_string()));
    let seen = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].0.contains("paste.search"), "{}", seen[1].0);
    assert!(seen[1].1.contains("PHID-PSTE-6"), "{}", seen[1].1);
}

#[test]
fn paste_p16_missing_research_is_a_clean_error() {
    // PHP calls `getURI()` on null (a crash); the clean error stays.
    let edit = paste_p_envelope(r#"{"object": {"phid": "PHID-PSTE-7"}}"#);
    let empty = paste_p_envelope(r#"{"data": []}"#);
    let (base, _rx) = serve_seq(vec![edit, empty]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = create_paste_detail(&client, "t", None, "body", None).unwrap_err();
    assert!(err.contains("could not be found"), "{err}");
}

#[test]
fn paste_p18_usage_errors_classify_toolset() {
    use crate::conduit::EnvelopeKind;
    let err = read_paste_classified(
        &crate::conduit::Client::new("http://127.0.0.1:9/", "t").unwrap(),
        "Px",
    )
    .unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::ToolsetUsage);
    let hit = paste_p_envelope(r#"{"data": []}"#);
    let (base, _rx) = serve_seq(vec![hit]);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = read_paste_classified(&client, "P999").unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::ToolsetUsage);
    assert!(err.message.contains("does not exist"), "{}", err.message);
}

#[test]
fn upx_b21_paste_requires_terminator_when_piped() {
    // Piped without `--`: the terminator reports, even for `--help`
    // and empty argv (PHP's `parseFull` runs before
    // `didParseArguments`).
    assert!(matches!(
        parse_args_with_tty(&argv(&["P123"]), false),
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
        parse_args_with_tty(&argv(&["--", "P123"]), false),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["P123"]), true),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["P1", "P2", "--"]), false),
        ParseOutcome::Error(_)
    ));
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn upx_b23_paste_help_matches_generated_format() {
    // PHP sets no synopsis for `paste`, so the synopsis line renders
    // empty (the blank line after the examples).
    assert_eq!(
        usage_text(false),
        "PASTE WORKFLOW\n\
         \n\
         paste [options] --\n\
         paste [options] -- object\n\
         \n\
         \n\
         Share and grab text using the Paste application. To create a paste, use the\n\
         \"--input\" flag or provide the text on stdin:\n\
         \n\
         \x20 $ cat list_of_ducks.txt | arc paste\n\
         \x20 $ arc paste --input list_of_ducks.txt\n\
         \n\
         To retrieve a paste, specify the paste ID:\n\
         \n\
         \x20 $ arc paste P123\n\
         \n\
         \x20     --browse\n\
         \x20         After creating a paste, open it in a web browser.\n\
         \n\
         \x20     --input path\n\
         \x20         Create a paste using the content in a file.\n\
         \n\
         \x20     --lang language\n\
         \x20         Language for the paste.\n\
         \n\
         \x20     --title title\n\
         \x20         Title for the paste.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `--help` still exits zero once `--` satisfies the terminator.
    let text = execute(&std::env::temp_dir(), &argv(&["--help", "--"])).expect("expected help");
    assert!(text.contains("PASTE WORKFLOW"), "{text}");
}

// -( T1170 COV5: close paste.rs )------------------------------------------------

#[test]
#[should_panic(expected = "expected args")]
fn args_of_panics_on_parse_error() {
    args_of(&["P1", "P2", "--"]);
}

#[test]
fn valued_flags_take_space_equals_and_missing_forms() {
    // Missing values (and empty values) report per flag.
    for words in [
        &["--title"][..],
        &["--title", ""][..],
        &["--lang"][..],
        &["--input"][..],
        &["--conduit-uri"][..],
    ] {
        let outcome = parse_args_with_tty(&argv(words), true);
        assert!(matches!(outcome, ParseOutcome::Error(_)), "{words:?}");
    }
    // `--flag value` and `--flag=value` both land.
    let opts = args_of(&["--title", "t", "--lang=r", "--"]);
    assert_eq!(opts.title, Some("t".to_string()));
    assert_eq!(opts.lang, Some("r".to_string()));
    let opts = args_of(&["--title=t2", "--input=f.txt", "--"]);
    assert_eq!(opts.title, Some("t2".to_string()));
    assert_eq!(opts.input, Some("f.txt".to_string()));
    let opts = args_of(&["--input=f2.txt", "--conduit-uri=https://h/", "--"]);
    assert_eq!(opts.input, Some("f2.txt".to_string()));
    assert_eq!(opts.conduit_uri, Some("https://h/".to_string()));
    let opts = args_of(&["--conduit-uri", "https://h/", "--"]);
    assert_eq!(opts.conduit_uri, Some("https://h/".to_string()));
}

#[test]
fn non_alpha_scheme_is_not_a_domain() {
    assert_eq!(
        absolute_uri(Some("https://h/"), "9x://foo/bar"),
        "https://h9x://foo/bar"
    );
}

#[test]
fn search_hit_ids_read_numbers_strings_and_missing() {
    let hit = serde_json::json!({
        "id": "42",
        "fields": {"title": "t", "uri": "/P42"},
    });
    let detail = detail_from_search_hit(&hit, None);
    assert_eq!(detail.id, 42);
    assert_eq!(detail.title, Some("t".to_string()));
    for hit in [
        serde_json::json!({"id": null}),
        serde_json::json!({"id": true}),
    ] {
        let detail = detail_from_search_hit(&hit, None);
        assert_eq!(detail.id, 0);
        assert_eq!(detail.uri, "/P0");
    }
}

#[test]
fn missing_paths_resolve_through_ancestors() {
    let tmp = TempDir::new("resolve");
    let canonical = std::fs::canonicalize(tmp.path()).unwrap();
    // Two missing levels walk up twice before resolving.
    assert_eq!(
        resolve_input_path(tmp.path(), "a/b/c.txt"),
        canonical.join("a/b/c.txt").to_string_lossy().to_string()
    );
    std::fs::create_dir_all(tmp.path().join("a")).unwrap();
    assert_eq!(
        resolve_input_path(tmp.path(), "a/b/c.txt"),
        canonical.join("a/b/c.txt").to_string_lossy().to_string()
    );
    // Nothing resolvable: the joined path passes through.
    assert_eq!(
        resolve_input_path(
            std::path::Path::new("definitely-missing-arc-rs-xyz-5a"),
            "f.txt"
        ),
        std::path::Path::new("definitely-missing-arc-rs-xyz-5a")
            .join("f.txt")
            .to_string_lossy()
            .to_string()
    );
}

#[cfg(unix)]
#[test]
fn symlinked_input_resolves_through_parent() {
    let tmp = TempDir::new("symlink");
    std::fs::write(tmp.path().join("target.txt"), b"data").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("target.txt"), tmp.path().join("link.txt")).unwrap();
    let canonical = std::fs::canonicalize(tmp.path()).unwrap();
    assert_eq!(
        resolve_input_path(tmp.path(), "link.txt"),
        canonical.join("link.txt").to_string_lossy().to_string()
    );
    assert_eq!(read_input_file(tmp.path(), "link.txt").unwrap(), b"data");
}

#[cfg(unix)]
#[test]
fn blocked_parent_reports_noexec() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("noexec");
    let sub = tmp.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o600)).unwrap();
    let err = read_input_file(tmp.path(), "sub/f.txt").unwrap_err();
    assert!(err.contains("not executable"), "{err}");
    assert!(err.contains("+x"), "{err}");
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn run_reports_usage_errors() {
    assert_eq!(run(&argv(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[cfg(unix)]
#[test]
fn unreadable_input_reports_not_readable() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("unreadable");
    let locked = tmp.path().join("locked.txt");
    std::fs::write(&locked, b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Best effort: root ignores permission bits.
    let probed = read_input_file(tmp.path(), "locked.txt");
    assert!(probed.is_ok() || probed.unwrap_err().contains("not readable"));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
}
