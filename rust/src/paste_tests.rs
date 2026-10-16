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
fn parses_create_options() {
    let opts = args_of(&["--title", "t", "--lang", "rust", "--input", "f.txt"]);
    assert_eq!(opts.title, Some("t".to_string()));
    assert_eq!(opts.lang, Some("rust".to_string()));
    assert_eq!(opts.input, Some("f.txt".to_string()));
    assert_eq!(opts.symbol, None);
}

#[test]
fn parses_read_symbol() {
    let opts = args_of(&["P123"]);
    assert_eq!(opts.symbol, Some("P123".to_string()));
}

#[test]
fn rejects_read_mode_creation_flags() {
    for words in [
        &["P1", "--lang", "x"][..],
        &["P1", "--title", "x"][..],
        &["P1", "--input", "f"][..],
        &["P1", "--browse"][..],
    ] {
        let ParseOutcome::Error(message) = parse_args(&argv(words)) else {
            panic!("expected error for {words:?}");
        };
        assert!(message.contains("not supported when reading"), "{message}");
    }
    assert!(matches!(
        parse_args(&argv(&["P1", "P2"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_browse_on_create() {
    let opts = args_of(&["--browse"]);
    assert!(opts.browse);
    assert!(!args_of(&[]).browse);
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
}

#[test]
fn reads_created_ids_and_phids() {
    assert_eq!(created_id_of(&serde_json::json!(12)), Some(12));
    assert_eq!(created_id_of(&serde_json::json!("34")), Some(34));
    assert_eq!(created_id_of(&serde_json::json!({"id": 5})), Some(5));
    assert_eq!(
        created_id_of(&serde_json::json!({"object": {"id": "6"}})),
        Some(6)
    );
    assert_eq!(created_id_of(&serde_json::json!({"x": 1})), None);
    assert_eq!(
        created_phid_of(&serde_json::json!({"object": {"phid": "PHID-PSTE-1"}})),
        Some("PHID-PSTE-1".to_string())
    );
    assert_eq!(
        created_uri_of(&serde_json::json!({"uri": "/P1"})),
        Some("/P1".to_string())
    );
}

/// Serve one canned Conduit response; returns the base URI.
fn serve_once(canned: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
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
                line.strip_prefix("Content-Length: ")
                    .or_else(|| line.strip_prefix("content-length: "))
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            canned.len(),
            canned
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    base
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
    // CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcconfig"), null_browser_config()).unwrap();

    let base = serve_once(r#"{"result": {"id": 5, "uri": "/P5"}, "error_code": null}"#);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let opts = args_of(&["--browse"]);
    let out = create_paste_output(&dir, &client, &opts, Some("hello")).unwrap();
    assert!(out.contains("Created paste P5"), "{out}");

    // A failing browser command fails the workflow with PHP's error.
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"browser": ["definitely-not-a-browser-xyz"]}"#,
    )
    .unwrap();
    let base = serve_once(r#"{"result": {"id": 5, "uri": "/P5"}, "error_code": null}"#);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    let err = create_paste_output(&dir, &client, &opts, Some("hello")).unwrap_err();
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
