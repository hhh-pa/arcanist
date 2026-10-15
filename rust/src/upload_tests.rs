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
