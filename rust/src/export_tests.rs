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

    // T1153 EXPORT-E27: positionals with `--revision` are silently
    // ignored; multiple positionals still fail for local sources.
    match parse_args(&argv(&["--git", "--revision", "1", "HEAD"])) {
        ParseOutcome::Args(opts) => assert_eq!(opts.source, Source::Revision(1)),
        _ => panic!("expected ignored positional"),
    }
    assert!(
        matches!(
            parse_args(&argv(&["--git", "a", "b"])),
            ParseOutcome::Error(_)
        ),
        "multi-positional local source"
    );
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
    assert_eq!(decode_patch(b"+caf\xe9\n", None).unwrap(), "+caf�\n");
    assert_eq!(
        decode_patch(b"+caf\xe9\n", Some("windows-1252")).unwrap(),
        "+café\n"
    );
    assert_eq!(
        decode_patch(b"+caf\xe9\n", Some("UTF-8")).unwrap(),
        "+caf�\n"
    );
    // Valid UTF-8 is never re-decoded, whatever the encoding reads.
    assert_eq!(
        decode_patch("+café\n".as_bytes(), Some("windows-1252")).unwrap(),
        "+café\n"
    );
    assert_eq!(
        decode_patch(b"\xff", Some("shift-jis")).unwrap_err(),
        "Unknown encoding \"shift-jis\"."
    );
    assert_eq!(decode_conduit_text("raw"), "raw\n");
    assert_eq!(decode_conduit_text("raw\n"), "raw\n");
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
    let (add, del, missing_old, missing_new) =
        hunk_stats("-old\n\\ No newline at end of file\n+new\n", 1, 1);
    assert_eq!((add, del, missing_old, missing_new), (1, 1, true, false));
    let (add, del, missing_old, missing_new) =
        hunk_stats("-old\n+new\n\\ No newline at end of file\n", 1, 1);
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
    // T1153 EXPORT-E1/E2: the bundle covers the committed base..HEAD
    // range (explicit base here; the default needs an upstream).
    let dir = fixture_repo("local");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "init"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-qm", "add"]);

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

    let opts = args_of(&["--arcbundle", "out.tar.gz", "HEAD~1"]);
    let out = export_bundle_with_client(&dir, &opts, "out.tar.gz", &client).unwrap();
    // T1153 EXPORT-E33: the progress line prints before the tar; only
    // Done is returned.
    assert_eq!(out, "Done.\n");

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
    assert_eq!(corpus, " one\n+two\n");
    assert_eq!(changes[0]["hunks"][0]["oldOffset"], serde_json::json!(1));

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
    // T1153 EXPORT-E33: only Done is returned.
    assert_eq!(out, "Done.\n");

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

// T1153 EXPORT-E30: the sink-encoding coverage moved to
// rust/tests/t1153_upx.rs (local git needs conduit now).

#[test]
fn r4_exporte7_empty_local_export_errors() {
    // `parseDiff()` throws on blank input; only local sources check
    // (conduit sources never pass through the parser).
    assert_eq!(
        reject_empty_diff(b"").unwrap_err(),
        "Can't parse an empty diff!"
    );
    assert!(reject_empty_diff(b"  \n\t ").is_err());
    assert!(reject_empty_diff(b"diff --git a/f b/f\n").is_ok());
    let dir = fixture_repo("r4-e7");
    std::fs::write(dir.join("f.txt"), "f\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "x"]);
    // T1153 EXPORT-E30: local exports require conduit config before
    // anything else (the empty-diff error moved to an integration test
    // with conduit configured). The `.invalid` URI keeps this hermetic:
    // no token can exist for it.
    let err = execute(
        &dir,
        &argv(&["--git", "--conduit-uri", "https://export-e30.invalid/"]),
    )
    .unwrap_err();
    assert!(err.contains("no token for"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r4_exporte28_multi_positional_message_and_exit() {
    // PHP's `parseBaseCommitArgument()` text; the exit was and stays
    // 1 (`export` is a subprocess workflow — never 255).
    match parse_args(&argv(&["--git", "a", "b"])) {
        ParseOutcome::Error(message) => assert_eq!(
            message,
            "Specify exactly one base commit. The end of the commit range is always the working copy state."
        ),
        _ => panic!("expected parse error"),
    }
    let dir = fixture_repo("r4-e28");
    assert_eq!(
        run_in(&dir, &argv(&["--git", "a", "b"])),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn export_e_repo(tag: &str, commits: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = fixture_repo(tag);
    for (name, body) in commits {
        std::fs::write(dir.join(name), body).unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "x"]);
    }
    dir
}

fn export_e_rev(dir: &std::path::Path, rev: &str) -> String {
    crate::git::stdout_text(dir, &["rev-parse", rev]).unwrap()
}

#[test]
fn export_e1_default_base_prefers_upstream() {
    let scratch = std::env::temp_dir().join(format!(
        "arc-rs-export-e1-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let remote = scratch.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());
    let work = scratch.join("work");
    let status = std::process::Command::new("git")
        .args(["clone", "-q"])
        .arg(&remote)
        .arg(&work)
        .status()
        .unwrap();
    assert!(status.success());
    git(&work, &["config", "user.email", "t@t"]);
    git(&work, &["config", "user.name", "t"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    git(&work, &["checkout", "-qb", "master"]);
    std::fs::write(work.join("f.txt"), "one\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "one"]);
    git(&work, &["push", "-q", "-u", "origin", "master"]);
    let pushed = export_e_rev(&work, "HEAD");
    std::fs::write(work.join("f.txt"), "one\ntwo\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "two"]);
    // No prompt: the upstream answers.
    let base =
        resolve_export_base_with_prompt(&work, None, None, &|_| panic!("must not prompt")).unwrap();
    assert_eq!(base, pushed);
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn export_e1_single_commit_repo_uses_magic_root() {
    let dir = export_e_repo("e1-magic", &[("f.txt", "one\n")]);
    let base =
        resolve_export_base_with_prompt(&dir, None, None, &|_| panic!("must not prompt")).unwrap();
    assert_eq!(base, crate::which::GIT_MAGIC_ROOT_COMMIT);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e1_no_upstream_without_tty_errors() {
    let dir = export_e_repo("e1-tty", &[("f.txt", "one\n"), ("f.txt", "one\ntwo\n")]);
    let err = resolve_export_base_with_prompt(&dir, None, None, &|_| {
        EXPORT_PROMPT_TTY_FAILED.set(true);
        None
    })
    .unwrap_err();
    assert_eq!(err, crate::prompt::NOT_INTERACTIVE);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e2_range_end_is_head() {
    let dir = export_e_repo("e2", &[("f.txt", "one\n"), ("f.txt", "one\ntwo\n")]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\nUNCOMMITTED\n").unwrap();
    let bytes = local_diff_bytes(&dir, Format::Git, "HEAD~1").unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("+two"), "{text}");
    assert!(!text.contains("UNCOMMITTED"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e3_explicit_base_merges_with_head() {
    let dir = export_e_repo("e3", &[("f.txt", "one\n"), ("f.txt", "one\ntwo\n")]);
    let no_prompt = &|_: &str| -> Option<String> { panic!("must not prompt") };
    assert_eq!(
        resolve_export_base_with_prompt(&dir, Some("HEAD"), None, no_prompt).unwrap(),
        export_e_rev(&dir, "HEAD")
    );
    assert_eq!(
        resolve_export_base_with_prompt(&dir, Some("HEAD~1"), None, no_prompt).unwrap(),
        export_e_rev(&dir, "HEAD~1")
    );
    assert_eq!(
        resolve_export_base_with_prompt(&dir, Some("nope"), None, no_prompt).unwrap_err(),
        "Unable to find any git commit named 'nope' in this repository."
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e36_diff_pins_context_and_renames() {
    // -U32767: two changes 100 lines apart stay in one hunk.
    let mut body = String::new();
    for index in 0..120 {
        body.push_str(&format!("line {index}\n"));
    }
    let dir = export_e_repo("e36", &[("f.txt", &body)]);
    let changed =
        body.replacen("line 10\n", "CHANGED 10\n", 1)
            .replacen("line 110\n", "CHANGED 110\n", 1);
    std::fs::write(dir.join("f.txt"), &changed).unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "x"]);
    let bytes = local_diff_bytes(&dir, Format::Git, "HEAD~1").unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.matches("@@").count(), 2, "{text}");
    // -M: exact renames are detected.
    git(&dir, &["mv", "f.txt", "g.txt"]);
    git(&dir, &["commit", "-qm", "mv"]);
    let bytes = local_diff_bytes(&dir, Format::Git, "HEAD~1").unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("rename from"), "{text}");
    assert!(text.contains("rename to"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e31_not_found_wraps_at_78_cols() {
    let message = not_found_message();
    assert!(message.contains('\n'), "{message:?}");
    for line in message.lines() {
        assert!(line.len() <= 78, "{line:?}");
    }
    assert_eq!(
        message.lines().collect::<Vec<_>>().join(" "),
        "The diff or revision you specified is either invalid or you don't have permission to view it."
    );
}

#[test]
fn export_e32_email_author_splits_like_phutil() {
    assert_eq!(
        split_email_author("Ada <a@x>"),
        (Some("Ada".to_string()), "a@x".to_string())
    );
    assert_eq!(
        split_email_author("\"Doe, John\" <j@x>"),
        (Some("Doe, John".to_string()), "j@x".to_string())
    );
    assert_eq!(split_email_author("plain@x"), (None, "plain@x".to_string()));
    assert_eq!(
        split_email_author("a <b> <c@d>"),
        (Some("a <b>".to_string()), "c@d".to_string())
    );
    assert_eq!(
        split_email_author("x <>"),
        (Some("x".to_string()), String::new())
    );
}

#[test]
fn export_e35_base64_decodes_laxly() {
    assert_eq!(lax_base64_decode("YmxvYg=="), b"blob");
    // Wrapped/whitespace input decodes (PHP skips invalid bytes).
    assert_eq!(lax_base64_decode("Ym\nxv\tYg=="), b"blob");
    // Missing padding is repaired.
    assert_eq!(lax_base64_decode("YWI"), b"ab");
    // Undecodable input yields empty bytes (PHP `false` writes `""`).
    assert_eq!(lax_base64_decode("!!!"), b"");
    assert_eq!(lax_base64_decode("A"), b"");
}

#[test]
fn export_e17_normalizes_conduit_dicts() {
    let change = serde_json::json!({
        "id": 7,
        "extra": true,
        "metadata": {},
        "oldPath": "a.txt",
        "currentPath": "a.txt",
        "type": 2,
        "fileType": 1,
        "commitHash": null,
        "hunks": [{
            "oldOffset": "1", "oldLength": "1", "newOffset": "1", "newLength": "1",
            "addLines": 1, "delLines": 1,
            "isMissingOldNewline": false, "isMissingNewNewline": false,
            "corpus": "-a\n+b\n", "stray": 1,
        }],
    });
    let normalized = normalize_conduit_change(&change);
    let keys: Vec<&str> = normalized
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 10, "{keys:?}");
    assert!(!keys.contains(&"id"), "{keys:?}");
    assert!(!keys.contains(&"extra"), "{keys:?}");
    assert_eq!(normalized["awayPaths"], serde_json::json!([]));
    assert_eq!(normalized["oldProperties"], serde_json::json!([]));
    let hunk_keys: Vec<&str> = normalized["hunks"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(hunk_keys.len(), 9, "{hunk_keys:?}");
    // Null corpora stringify to "" (`(string)$corpus`).
    let null_corpus = serde_json::json!({"hunks": [{"corpus": null}]});
    assert_eq!(
        normalize_conduit_change(&null_corpus)["hunks"][0]["corpus"],
        serde_json::json!("")
    );
}

#[test]
fn export_e25_raw_ids_parse_and_send_raw() {
    match parse_args(&argv(&["--git", "--revision", "Dx"])) {
        ParseOutcome::Args(opts) => assert_eq!(opts.source, Source::RevisionRaw("x".to_string())),
        _ => panic!("expected raw revision"),
    }
    match parse_args(&argv(&["--git", "--revision", "DD1"])) {
        ParseOutcome::Args(opts) => assert_eq!(opts.source, Source::RevisionRaw("D1".to_string())),
        _ => panic!("expected one-strip revision"),
    }
    match parse_args(&argv(&["--git", "--diff", "Dx"])) {
        ParseOutcome::Args(opts) => assert_eq!(opts.source, Source::DiffRaw("Dx".to_string())),
        _ => panic!("expected raw diff"),
    }
    assert_eq!(
        querydiffs_params(&Source::Revision(9)),
        serde_json::json!({"revisionIDs": ["9"]})
    );
    assert_eq!(
        querydiffs_params(&Source::RevisionRaw("x".to_string())),
        serde_json::json!({"revisionIDs": ["x"]})
    );
    assert_eq!(
        querydiffs_params(&Source::DiffRaw("Dx".to_string())),
        serde_json::json!({"ids": ["Dx"]})
    );
}

#[test]
fn export_e27_multi_positionals_ignored_for_conduit() {
    for words in [
        &["--git", "--revision", "1", "a", "b"][..],
        &["--unified", "--diff", "2", "a"][..],
    ] {
        assert!(
            matches!(parse_args(&argv(words)), ParseOutcome::Args(_)),
            "{words:?}"
        );
    }
}

#[test]
fn export_e29_source_conflict_beats_missing_format() {
    match parse_args(&argv(&["--revision", "1", "--diff", "2"])) {
        ParseOutcome::Error(message) => assert!(message.contains("not compatible"), "{message}"),
        _ => panic!("expected conflict"),
    }
}

fn export_e_render_changes(dir: &std::path::Path, base: &str) -> Vec<crate::patch::BundleChange> {
    let bytes = local_diff_bytes(dir, Format::Git, base).unwrap();
    reject_empty_diff(&bytes).unwrap();
    let text = decode_patch(&bytes, None).unwrap();
    let parsed = crate::diff_local::parse_unified_diff(&text).unwrap();
    let head = resolve_export_head(dir).unwrap();
    local_render_changes(dir, base, &head, &parsed)
}

fn export_e_no_blobs(_phid: &str, _name: Option<&str>) -> Result<Vec<u8>, String> {
    panic!("text renders load no blobs")
}

#[test]
fn export_e6_local_git_renders_from_bundle() {
    let dir = export_e_repo(
        "e6",
        &[
            ("f.txt", "one\n"),
            ("del.txt", "bye\n"),
            ("f.txt", "one\ntwo\n"),
            ("new.txt", "hi\n"),
        ],
    );
    // NOTE: export_e_repo commits sequentially, so shape the two states.
    std::fs::remove_dir_all(&dir).ok();
    let dir = fixture_repo("e6");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    std::fs::write(dir.join("del.txt"), "bye\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
    std::fs::write(dir.join("new.txt"), "hi\n").unwrap();
    git(&dir, &["rm", "-q", "del.txt"]);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "two"]);
    let mut changes = export_e_render_changes(&dir, "HEAD~1");
    let patch = render_export_git_patch(&mut changes, &export_e_no_blobs).unwrap();
    assert_eq!(
        patch,
        "diff --git a/del.txt b/del.txt\n\
         deleted file mode 100644\n\
         --- a/del.txt\n\
         +++ /dev/null\n\
         @@ -1 +0,0 @@\n\
         -bye\n\
         diff --git a/f.txt b/f.txt\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -1 +1,2 @@\n\
         \x20one\n\
         +two\n\
         diff --git a/new.txt b/new.txt\n\
         new file mode 100644\n\
         --- /dev/null\n\
         +++ b/new.txt\n\
         @@ -0,0 +1 @@\n\
         +hi\n\
         \n"
    );
    // Oracle: the render applies onto the base and reproduces HEAD.
    git(&dir, &["checkout", "-q", "HEAD~1"]);
    let patch_file =
        std::env::temp_dir().join(format!("arc-rs-export-e6-{}.patch", std::process::id()));
    std::fs::write(&patch_file, &patch).unwrap();
    let patch_arg = patch_file.to_string_lossy().to_string();
    git(&dir, &["apply", "--check", &patch_arg]);
    git(&dir, &["apply", &patch_arg]);
    git(&dir, &["add", "-A"]);
    let status = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["diff", "--quiet", "--cached", "master"])
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::remove_file(&patch_file).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e6_rename_headers_and_tab_suffix() {
    let dir = fixture_repo("e6tab");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    git(&dir, &["mv", "f.txt", "g.txt"]);
    std::fs::write(dir.join("a b.txt"), "spaced\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "two"]);
    let mut changes = export_e_render_changes(&dir, "HEAD~1");
    let patch = render_export_git_patch(&mut changes, &export_e_no_blobs).unwrap();
    assert!(
        patch.contains("rename from f.txt\nrename to g.txt\n"),
        "{patch}"
    );
    assert!(patch.contains("+++ b/a b.txt\t\n"), "{patch:?}");
    assert!(!patch.contains("similarity"), "{patch}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e5_local_unified_renders_from_bundle() {
    let dir = fixture_repo("e5");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "two"]);
    let changes = export_e_render_changes(&dir, "HEAD~1");
    assert_eq!(
        render_export_unified(&changes),
        "Index: f.txt\n\
         ===================================================================\n\
         --- f.txt\n\
         +++ f.txt\n\
         @@ -1 +1,2 @@\n\
         \x20one\n\
         +two\n"
    );
    // Empty bundles render "".
    assert_eq!(render_export_unified(&[]), "");
    std::fs::remove_dir_all(&dir).ok();
}

fn export_e_conduit_change() -> serde_json::Value {
    serde_json::json!({
        "id": 31,
        "metadata": {},
        "oldPath": "a.txt",
        "currentPath": "a.txt",
        "awayPaths": [],
        "oldProperties": [],
        "newProperties": {"unix:filemode": "100755"},
        "type": "2",
        "fileType": "1",
        "commitHash": null,
        "hunks": [{
            "oldOffset": "10", "oldLength": "1", "newOffset": "20", "newLength": "2",
            "addLines": 1, "delLines": 1,
            "isMissingOldNewline": false, "isMissingNewNewline": false,
            "corpus": "-a\n+b\n+c\n",
        }],
    })
}

#[test]
fn export_e8_conduit_git_renders_from_bundle() {
    let mut changes = vec![conduit_render_change(&export_e_conduit_change())];
    let patch = render_export_git_patch(&mut changes, &export_e_no_blobs).unwrap();
    assert_eq!(
        patch,
        "diff --git a/a.txt b/a.txt\n\
         old mode 100644\n\
         new mode 100755\n\
         --- a/a.txt\n\
         +++ b/a.txt\n\
         @@ -10 +20,2 @@\n\
         -a\n\
         +b\n\
         +c\n\
         \n"
    );
}

#[test]
fn export_e4_conduit_unified_renders_from_bundle() {
    let changes = vec![conduit_render_change(&export_e_conduit_change())];
    let unified = render_export_unified(&changes);
    assert!(unified.starts_with("Index: a.txt\n"), "{unified}");
    assert!(unified.contains("@@ -10 +20,2 @@\n"), "{unified}");
    assert!(!unified.contains("diff --git"), "{unified}");
}

#[test]
fn export_e_multicopy_without_source_fails_like_php() {
    let mut changes = vec![crate::patch::BundleChange {
        change_type: crate::diff_local::TYPE_MULTICOPY,
        file_type: crate::diff_local::FILE_TEXT,
        old_path: None,
        current_path: Some("src.txt".to_string()),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
        old_binary_phid: None,
        new_binary_phid: None,
        away_paths: vec!["a.txt".to_string()],
        original_file_data: None,
        current_file_data: None,
    }];
    assert_eq!(
        render_export_git_patch(&mut changes, &export_e_no_blobs).unwrap_err(),
        "Failed to decompose multicopy changeset in order to generate diff."
    );
}

#[test]
fn export_e19_binary_regenerates_from_disk() {
    let old_bytes: &[u8] = b"OLD-BINARY\x00BYTES-old";
    let new_bytes: &[u8] = b"NEW-BINARY\x00BYTES-new!!";
    let dir = fixture_repo("e19");
    std::fs::write(dir.join("a.bin"), old_bytes).unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    std::fs::write(dir.join("a.bin"), new_bytes).unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "two"]);
    let mut changes = export_e_render_changes(&dir, "HEAD~1");
    assert_eq!(changes.len(), 1);
    assert!(changes[0].original_file_data.is_some());
    assert!(changes[0].current_file_data.is_some());
    let patch = render_export_git_patch(&mut changes, &export_e_no_blobs).unwrap();
    // Independent oracle: blob SHAs from git itself over both versions.
    let old_sha = crate::git::stdout_text(&dir, &["rev-parse", "HEAD~1:a.bin"]).unwrap();
    let new_sha = crate::git::stdout_text(&dir, &["rev-parse", "HEAD:a.bin"]).unwrap();
    assert!(
        patch.contains(&format!("index {old_sha}..{new_sha}\n")),
        "{patch}"
    );
    assert!(patch.contains("GIT binary patch\n"), "{patch}");
    assert!(
        patch.contains(&format!("literal {}\n", new_bytes.len())),
        "{patch}"
    );
    assert!(
        patch.contains(&format!("literal {}\n", old_bytes.len())),
        "{patch}"
    );
    assert!(!patch.contains("Binary files"), "{patch}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_e18_bundle_json_escapes_slash_and_unicode() {
    let dir = fixture_repo("e18");
    let changes = vec![serde_json::json!({
        "currentPath": "caf\u{e9}/x",
        "hunks": [],
    })];
    let meta = serde_json::json!({"authorName": "Ada"});
    let out = dir.join("out.tar.gz");
    write_bundle(&out, &changes, &meta, &[], &[]).unwrap();
    let unpacked = unpack(&out);
    let text = std::fs::read_to_string(unpacked.join("changes.json")).unwrap();
    assert!(text.contains("caf\\u00e9\\/x"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&unpacked).ok();
}
