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
fn parses_formats_and_local_base() {
    let opts = args_of(&["--git"]);
    assert_eq!(opts.format, Format::Git);
    assert_eq!(opts.source, Source::Local { base: None });

    let opts = args_of(&["--unified", "HEAD~2"]);
    assert_eq!(opts.format, Format::Unified);
    assert_eq!(
        opts.source,
        Source::Local {
            base: Some("HEAD~2".to_string())
        }
    );
}

#[test]
fn parses_conduit_sources() {
    assert_eq!(
        args_of(&["--git", "--revision", "D5"]).source,
        Source::Revision(5)
    );
    assert_eq!(
        args_of(&["--unified", "--diff=11"]).source,
        Source::Diff(11)
    );
    let opts = args_of(&["--git", "--diff", "3", "--conduit-uri", "https://x/"]);
    assert_eq!(opts.conduit_uri, Some("https://x/".to_string()));
}

#[test]
fn requires_exactly_one_format() {
    let ParseOutcome::Error(message) = parse_args(&argv(&[])) else {
        panic!("expected error for no format");
    };
    assert!(message.contains("Specify one of"), "{message}");

    let ParseOutcome::Error(message) = parse_args(&argv(&["--git", "--unified"])) else {
        panic!("expected error for two formats");
    };
    assert!(message.contains("not compatible"), "{message}");
}

#[test]
fn rejects_mixed_sources_and_stray_positionals() {
    let ParseOutcome::Error(message) =
        parse_args(&argv(&["--git", "--revision", "1", "--diff", "2"]))
    else {
        panic!("expected error for mixed sources");
    };
    assert!(message.contains("not compatible"), "{message}");

    for words in [
        &["--git", "--revision", "1", "HEAD"][..],
        &["--git", "a", "b"][..],
    ] {
        assert!(
            matches!(parse_args(&argv(words)), ParseOutcome::Error(_)),
            "{words:?}"
        );
    }
}

#[test]
fn parses_arcbundle_format() {
    let opts = args_of(&["--arcbundle", "out.tar.gz"]);
    assert_eq!(
        opts.format,
        Format::Bundle {
            path: "out.tar.gz".to_string()
        }
    );
    let opts = args_of(&["--arcbundle=out.tar.gz", "--revision", "D1"]);
    assert_eq!(
        opts.format,
        Format::Bundle {
            path: "out.tar.gz".to_string()
        }
    );
    assert!(matches!(
        parse_args(&argv(&["--arcbundle"])),
        ParseOutcome::Error(_)
    ));
    let ParseOutcome::Error(message) = parse_args(&argv(&["--arcbundle", "x", "--git"])) else {
        panic!("expected error for mixed formats");
    };
    assert_eq!(
        message,
        "Options '--git', '--unified' and '--arcbundle' are not compatible. Choose exactly one export format."
    );
    let ParseOutcome::Error(message) = parse_args(&argv(&["--revision", "1"])) else {
        panic!("expected error for no format");
    };
    assert_eq!(
        message,
        "Specify one of '--git', '--unified' or '--arcbundle <path>' to choose an export format."
    );
}

#[test]
fn parses_encoding_flag() {
    let opts = args_of(&["--git", "--encoding", "windows-1252"]);
    assert_eq!(opts.encoding, Some("windows-1252".to_string()));
    let opts = args_of(&["--git", "--encoding=latin1"]);
    assert_eq!(opts.encoding, Some("latin1".to_string()));
    let opts = args_of(&["--git"]);
    assert_eq!(opts.encoding, None);
    assert!(matches!(
        parse_args(&argv(&["--git", "--encoding"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn normalizes_revision_ids() {
    assert_eq!(normalize_revision_id("D123").unwrap(), 123);
    assert_eq!(normalize_revision_id("d4").unwrap(), 4);
    assert_eq!(normalize_revision_id("9").unwrap(), 9);
    assert!(normalize_revision_id("D").is_err());
    // PHP does not trim.
    assert!(normalize_revision_id(" 9").is_err());
    assert!(normalize_revision_id("9 ").is_err());
}

#[test]
fn decodes_patches_with_encoding() {
    // 0xE9 is `é` in Windows-1252 but invalid UTF-8.
    assert_eq!(decode_patch(b"+caf\xe9\n", None), "+caf�\n");
    assert_eq!(decode_patch(b"+caf\xe9\n", Some("windows-1252")), "+café\n");
    assert_eq!(decode_patch(b"+caf\xe9\n", Some("UTF-8")), "+caf�\n");
    assert_eq!(decode_conduit_text("raw", None), "raw\n");
    assert_eq!(decode_conduit_text("raw\n", None), "raw\n");
}

#[test]
fn finds_first_diff_id() {
    let dict = serde_json::json!({"3": {"id": 3}});
    assert_eq!(first_diff_id(&dict).unwrap(), "3");
    assert!(first_diff_id(&serde_json::json!({})).is_err());
}

const SAMPLE_DIFF: &str = "diff --git a/f.txt b/f.txt\n\
     new file mode 100644\n\
     index 0000000..3b18e51\n\
     --- /dev/null\n\
     +++ b/f.txt\n\
     @@ -0,0 +1,2 @@\n\
     +one\n\
     +two\n\
     diff --git a/g.txt b/h.txt\n\
     similarity index 90%\n\
     rename from g.txt\n\
     rename to h.txt\n\
     index 3b18e51..a23bf0f 100644\n\
     --- a/g.txt\n\
     +++ b/h.txt\n\
     @@ -1 +1 @@\n\
     -old\n\
     +new\n";

#[test]
fn serializes_changes_with_stats_and_companions() {
    let parsed = crate::diff_local::parse_unified_diff(SAMPLE_DIFF).unwrap();
    assert_eq!(parsed.len(), 2);
    let dicts = changes_json(&parsed);
    assert_eq!(dicts.len(), 3);

    let add = &dicts[0];
    assert_eq!(add["oldPath"], serde_json::json!("/dev/null"));
    assert_eq!(add["currentPath"], serde_json::json!("f.txt"));
    assert_eq!(add["type"], serde_json::json!(1));
    assert_eq!(add["metadata"], serde_json::json!([]));
    let hunk = &add["hunks"][0];
    assert_eq!(hunk["oldOffset"], serde_json::json!(0));
    assert_eq!(hunk["oldLength"], serde_json::json!(0));
    assert_eq!(hunk["newOffset"], serde_json::json!(1));
    assert_eq!(hunk["newLength"], serde_json::json!(2));
    assert_eq!(hunk["addLines"], serde_json::json!(2));
    assert_eq!(hunk["delLines"], serde_json::json!(0));
    assert_eq!(hunk["isMissingOldNewline"], serde_json::json!(false));
    assert_eq!(hunk["isMissingNewNewline"], serde_json::json!(false));

    let moved = &dicts[1];
    assert_eq!(moved["type"], serde_json::json!(6));
    let away = &dicts[2];
    assert_eq!(away["type"], serde_json::json!(4));
    assert_eq!(away["currentPath"], serde_json::json!("g.txt"));
    assert_eq!(away["oldPath"], serde_json::json!(null));
    assert_eq!(away["awayPaths"], serde_json::json!(["h.txt"]));
    assert_eq!(away["hunks"], serde_json::json!([]));
}

#[test]
fn counts_missing_newlines_like_php() {
    let (add, del, missing_old, missing_new) = hunk_stats(
        "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n",
        1,
        1,
    );
    assert_eq!((add, del, missing_old, missing_new), (1, 1, true, false));
    let (add, del, missing_old, missing_new) = hunk_stats(
        "@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n",
        1,
        1,
    );
    assert_eq!((add, del, missing_old, missing_new), (1, 1, false, true));
}

#[test]
fn strips_corpora_and_collects_blob_phids() {
    let mut changes = vec![serde_json::json!({
        "metadata": {"old:binary-phid": "PHID-FILE-1", "new:binary-phid": "PHID-FILE-2"},
        "hunks": [{"corpus": "a"}, {"corpus": "b"}],
    })];
    let hunks = strip_corpora(&mut changes);
    assert_eq!(hunks, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(changes[0]["hunks"][0]["corpus"], serde_json::json!(0));
    assert_eq!(changes[0]["hunks"][1]["corpus"], serde_json::json!(1));
    assert_eq!(
        blob_phids(&changes),
        vec!["PHID-FILE-1".to_string(), "PHID-FILE-2".to_string()]
    );
    let empty = vec![serde_json::json!({"metadata": {"old:binary-phid": ""}})];
    assert!(blob_phids(&empty).is_empty());
}

/// Serve `expected` requests from `routes` (method -> envelope).
fn serve(routes: std::collections::HashMap<String, String>, expected: usize) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head_text = String::from_utf8_lossy(&head).to_string();
            let len: usize = head_text
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(key, value)| {
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let method = head_text
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .strip_prefix("/api/")
                .unwrap_or("")
                .to_string();
            let canned = routes
                .get(&method)
                .cloned()
                .unwrap_or_else(|| r#"{"result": null, "error_code": "ERR-NO-ROUTE"}"#.to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    base
}

fn envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(status.success());
}

fn fixture_repo(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-export-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "master"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);
    dir
}

/// Unpack a bundle tarball into a fresh directory; returns it.
fn unpack(bundle: &std::path::Path) -> std::path::PathBuf {
    let dir = bundle.with_extension("unpacked");
    std::fs::create_dir_all(&dir).unwrap();
    let status = std::process::Command::new("tar")
        .current_dir(&dir)
        .arg("-xzf")
        .arg(bundle)
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

#[test]
fn local_bundle_roundtrips_through_tar() {
    let dir = fixture_repo("local");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "add"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();

    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.whoami".to_string(),
        envelope(r#"{"phid": "PHID-USER-1"}"#),
    );
    routes.insert(
        "user.query".to_string(),
        envelope(r#"{"1": {"realName": "Ada"}}"#),
    );
    let base = serve(routes, 2);
    let client = crate::conduit::Client::new(&base, "t").unwrap();

    let opts = args_of(&["--arcbundle", "out.tar.gz"]);
    let out = export_bundle_with_client(&dir, &opts, "out.tar.gz", &client).unwrap();
    assert_eq!(out, "Writing bundle to 'out.tar.gz'...\nDone.\n");

    let bundle = dir.join("out.tar.gz");
    assert!(bundle.is_file());
    let unpacked = unpack(&bundle);
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(unpacked.join("meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["version"], serde_json::json!(5));
    assert_eq!(meta["authorName"], serde_json::json!("Ada"));
    assert_eq!(meta["authorEmail"], serde_json::json!("t@t"));
    assert_eq!(meta["revisionID"], serde_json::json!(null));
    assert_eq!(meta["baseRevision"].as_str().unwrap().len(), 40);
    let changes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(unpacked.join("changes.json")).unwrap())
            .unwrap();
    assert_eq!(changes[0]["currentPath"], serde_json::json!("f.txt"));
    assert_eq!(changes[0]["hunks"][0]["corpus"], serde_json::json!(0));
    let corpus = std::fs::read_to_string(unpacked.join("hunks/0")).unwrap();
    assert!(corpus.starts_with("@@ -1 +1,2 @@"), "{corpus:?}");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&unpacked).ok();
}

#[test]
fn conduit_bundle_passes_dicts_through_and_downloads_blobs() {
    let dir = fixture_repo("conduit");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.querydiffs".to_string(),
        envelope(
            r#"{"7": {"id": 7, "revisionID": 9, "sourceControlBaseRevision": "abc",
             "authorName": "Bo", "authorEmail": "bo@x",
             "changes": [{"metadata": {"new:binary-phid": "PHID-FILE-9"},
                          "oldPath": "a.bin", "currentPath": "a.bin", "awayPaths": [],
                          "oldProperties": [], "newProperties": [], "type": 2,
                          "fileType": 3, "commitHash": null, "hunks": []}]}}"#,
        ),
    );
    // "blob-bytes" in base64.
    routes.insert(
        "file.download".to_string(),
        envelope(r#""YmxvYi1ieXRlcw==""#),
    );
    let base = serve(routes, 2);
    let client = crate::conduit::Client::new(&base, "t").unwrap();

    let opts = args_of(&["--arcbundle", "out.tar.gz", "--diff", "7"]);
    let out = export_bundle_with_client(&dir, &opts, "out.tar.gz", &client).unwrap();
    assert_eq!(out, "Writing bundle to 'out.tar.gz'...\nDone.\n");

    let unpacked = unpack(&dir.join("out.tar.gz"));
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(unpacked.join("meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["revisionID"], serde_json::json!(9));
    assert_eq!(meta["baseRevision"], serde_json::json!("abc"));
    assert_eq!(meta["authorName"], serde_json::json!("Bo"));
    let blob = std::fs::read(unpacked.join("blobs/PHID-FILE-9")).unwrap();
    assert_eq!(blob, b"blob-bytes");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&unpacked).ok();
}
