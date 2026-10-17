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
fn decodes_conduit_text_with_trailing_newline() {
    // (T1153 E9: the removed `decode_patch` wrapper's decoding is still
    // covered at its source, `patch::decode_bytes`.)
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
    // T1153 EXPORT-E13: `new file mode` rides the new properties.
    assert_eq!(add["oldProperties"], serde_json::json!([]));
    assert_eq!(
        add["newProperties"],
        serde_json::json!({"unix:filemode": "100644"})
    );
    let hunk = &add["hunks"][0];
    // T1153 EXPORT-E11: `@@ -0,0 +1,2 @@` spells both lengths, so
    // offsets and lengths all render as strings.
    assert_eq!(hunk["oldOffset"], serde_json::json!("0"));
    assert_eq!(hunk["oldLength"], serde_json::json!("0"));
    assert_eq!(hunk["newOffset"], serde_json::json!("1"));
    assert_eq!(hunk["newLength"], serde_json::json!("2"));
    assert_eq!(hunk["addLines"], serde_json::json!(2));
    assert_eq!(hunk["delLines"], serde_json::json!(0));
    assert_eq!(hunk["isMissingOldNewline"], serde_json::json!(false));
    assert_eq!(hunk["isMissingNewNewline"], serde_json::json!(false));

    let moved = &dicts[1];
    assert_eq!(moved["type"], serde_json::json!(6));
    // T1153 EXPORT-E11: `@@ -1 +1 @@` omits both lengths, so they
    // render as integer 1 while offsets stay strings.
    let relocated = &moved["hunks"][0];
    assert_eq!(relocated["oldOffset"], serde_json::json!("1"));
    assert_eq!(relocated["oldLength"], serde_json::json!(1));
    assert_eq!(relocated["newOffset"], serde_json::json!("1"));
    assert_eq!(relocated["newLength"], serde_json::json!(1));
    let away = &dicts[2];
    assert_eq!(away["type"], serde_json::json!(4));
    assert_eq!(away["currentPath"], serde_json::json!("g.txt"));
    assert_eq!(away["oldPath"], serde_json::json!(null));
    assert_eq!(away["awayPaths"], serde_json::json!(["h.txt"]));
    assert_eq!(away["hunks"], serde_json::json!([]));
}

#[test]
fn old_path_local_render_maps_dev_null_to_no_old_path() {
    // T1153 OLD-PATH: local bundles map the parser's `/dev/null`
    // (ADD) to no old path, so renders emit `--- /dev/null`
    // without a mode lookup or blob fetch for it.
    let dir = fixture_repo("oldpath");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    let parsed = crate::diff_local::parse_unified_diff(SAMPLE_DIFF).unwrap();
    assert_eq!(parsed[0].old_path, "/dev/null");
    let head = resolve_export_head(&dir).unwrap();
    let rendered = local_render_changes(&dir, "HEAD", &head, &parsed);
    assert_eq!(rendered[0].old_path, None);
    assert_eq!(rendered[0].current_path.as_deref(), Some("f.txt"));
    assert_eq!(rendered[0].old_mode, None);
    // The git render still emits the `/dev/null` old target.
    let mut changes = rendered;
    let patch = render_export_git_patch(&mut changes, &export_e_no_blobs).unwrap();
    assert!(patch.contains("--- /dev/null\n+++ b/f.txt\n"), "{patch}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn counts_missing_newlines_like_php() {
    // T1153 EXPORT-E15: the shared length-driven loop (the marker flags
    // the old side while new lines remain, the new side once exhausted).
    let (add, del, missing_old, missing_new) =
        crate::diff_local::hunk_stats("-old\n\\ No newline at end of file\n+new\n", 1, 1);
    assert_eq!((add, del, missing_old, missing_new), (1, 1, true, false));
    let (add, del, missing_old, missing_new) =
        crate::diff_local::hunk_stats("-old\n+new\n\\ No newline at end of file\n", 1, 1);
    assert_eq!((add, del, missing_old, missing_new), (1, 1, false, true));
    // A pure-deletion hunk flags the NEW side (new length is exhausted):
    // `if ($new_len)` at `:951`.
    let (_, _, missing_old, missing_new) =
        crate::diff_local::hunk_stats("-x\n\\ No newline at end of file\n", 1, 0);
    assert_eq!((missing_old, missing_new), (false, true));
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
    // T1153 EXPORT-E11: offsets render as strings in local bundles.
    assert_eq!(changes[0]["hunks"][0]["oldOffset"], serde_json::json!("1"));

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
    let text = crate::patch::decode_bytes(&bytes, None).unwrap();
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

#[test]
fn upx_b21_export_needs_no_terminator() {
    // T1153 BROWSE-B21 VERIFIED for `export`: it is a classic
    // workflow (`ArcanistWorkflow`, `supportsToolset()` false), so
    // PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    let opts = args_of(&["--git", "HEAD"]);
    assert_eq!(opts.format, Format::Git);
    match execute(&std::env::temp_dir(), &argv(&["--help"])) {
        Ok(text) => assert!(text.contains("EXPORT WORKFLOW"), "{text}"),
        Err(message) => panic!("expected help, got {message}"),
    }
}

#[test]
fn upx_b23_export_help_matches_generated_format() {
    // Legacy generated help: the `PhutilArgumentWorkflow` default
    // example, `getCommandSynopses()`/`getCommandHelp()` bodies, and
    // the legacy specs in `msort` order.
    assert_eq!(
        usage_text(false),
        "EXPORT WORKFLOW\n\
         \n\
         export\n\
         \x20     export [paths] format (svn)\n\
         \x20     export [commit_range] format (git, hg)\n\
         \x20     export --revision revision_id format\n\
         \x20     export --diff diff_id format\n\
         \n\
         \x20         Supports: svn, git, hg\n\
         \x20         Export the local changeset (or a Differential changeset) to a file,\n\
         \x20         in some format: git diff (--git), unified diff\n\
         \x20         (--unified), or arc bundle (--arcbundle path) format.\n\
         \n\
         \x20     --arcbundle file\n\
         \x20         Export change as an arc bundle. This format can represent all\n\
         \x20         changes. These bundles can be applied with 'arc patch'.\n\
         \n\
         \x20     --diff diff_id\n\
         \x20         Instead of exporting changes from the working copy, export them\n\
         \x20         from a Differential diff.\n\
         \n\
         \x20     --encoding encoding\n\
         \x20         Attempt to convert non UTF-8 patch into specified encoding.\n\
         \n\
         \x20     --git\n\
         \x20         Export change as a git patch. This format is more complete than\n\
         \x20         unified, but less complete than arc bundles. These patches can be\n\
         \x20         applied with 'git apply' or 'arc patch'.\n\
         \n\
         \x20     --revision revision_id\n\
         \x20         Instead of exporting changes from the working copy, export them\n\
         \x20         from a Differential revision.\n\
         \n\
         \x20     --unified\n\
         \x20         Export change as a unified patch. This format is less complete than\n\
         \x20         git patches or arc bundles. These patches can be applied with\n\
         \x20         'patch' or 'arc patch'.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
}

/// Plain temp dir (not a git repo); the caller removes it.
fn plain_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-export-plain-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Binary `BundleChange` with no data and no phids; tests set what they need.
fn cov_binary_change() -> crate::patch::BundleChange {
    crate::patch::BundleChange {
        change_type: crate::diff_local::TYPE_CHANGE,
        file_type: crate::diff_local::FILE_BINARY,
        old_path: Some("dir/old.bin".to_string()),
        current_path: Some("dir/new.bin".to_string()),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
        old_binary_phid: None,
        new_binary_phid: None,
        away_paths: Vec::new(),
        original_file_data: None,
        current_file_data: None,
    }
}

fn cov_hunk(corpus: &str) -> crate::diff_local::ParsedHunk {
    crate::diff_local::ParsedHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        corpus: corpus.to_string(),
        synthetic_counts: None,
    }
}

#[test]
fn missing_values_for_value_flags_error() {
    for (flag, words) in [
        ("--revision", &["--git", "--revision"][..]),
        ("--diff", &["--git", "--diff"][..]),
        ("--conduit-uri", &["--git", "--conduit-uri"][..]),
    ] {
        match parse_args(&argv(words)) {
            ParseOutcome::Error(message) => {
                assert_eq!(message, format!("{flag} requires a value"));
            }
            _ => panic!("expected missing-value error for {flag}"),
        }
    }
    // Empty values are missing too (`take_value` rejects `""`).
    assert!(matches!(
        parse_args(&argv(&["--git", "--revision", ""])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_equals_forms_for_revision_and_conduit_uri() {
    assert_eq!(
        args_of(&["--git", "--revision=D5"]).source,
        Source::Revision(5)
    );
    let opts = args_of(&["--git", "--conduit-uri=https://x/"]);
    assert_eq!(opts.conduit_uri, Some("https://x/".to_string()));
    let opts = args_of(&["--git", "--conduit-uri", "https://y/"]);
    assert_eq!(opts.conduit_uri, Some("https://y/".to_string()));
}

#[test]
fn double_dash_ends_flag_parsing() {
    let opts = args_of(&["--git", "--", "HEAD"]);
    assert_eq!(
        opts.source,
        Source::Local {
            base: Some("HEAD".to_string())
        }
    );
    // Flag-like positionals survive after `--`.
    let opts = args_of(&["--git", "--", "--git"]);
    assert_eq!(
        opts.source,
        Source::Local {
            base: Some("--git".to_string())
        }
    );
}

#[test]
fn unknown_flag_errors() {
    match parse_args(&argv(&["--git", "--bogus"])) {
        ParseOutcome::Error(message) => assert_eq!(message, "unknown flag '--bogus'"),
        _ => panic!("expected unknown-flag error"),
    }
}

#[test]
fn first_diff_id_rejects_non_tables_and_reads_string_ids() {
    // Non-table results are not-found (message asserted structurally).
    assert!(first_diff_id(&serde_json::json!("x")).is_err());
    assert!(first_diff_id(&serde_json::json!(7)).is_err());
    // String IDs pass through; missing/other IDs are not-found.
    let dict = serde_json::json!({"3": {"id": "7"}});
    assert_eq!(first_diff_id(&dict).unwrap(), "7");
    assert!(first_diff_id(&serde_json::json!({"3": {}})).is_err());
    assert!(first_diff_id(&serde_json::json!({"3": {"id": true}})).is_err());
    let list = serde_json::json!([{"id": "9"}]);
    assert_eq!(first_diff_id(&list).unwrap(), "9");
}

#[test]
fn fetch_raw_diff_reads_all_response_shapes() {
    let fetch = |result: &str| {
        let mut routes = std::collections::HashMap::new();
        routes.insert("differential.getrawdiff".to_string(), envelope(result));
        let base = serve(routes, 1);
        let client = crate::conduit::Client::new(&base, "t").unwrap();
        fetch_raw_diff_for_diff(&client, "3")
    };
    assert_eq!(fetch(r#""S""#).unwrap(), "S");
    assert_eq!(fetch(r#"{"rawDiff": "R1"}"#).unwrap(), "R1");
    assert_eq!(fetch(r#"{"diff": "R2"}"#).unwrap(), "R2");
    assert_eq!(
        fetch(r#"{}"#).unwrap_err(),
        "differential.getrawdiff returned an unexpected response"
    );
}

#[test]
fn querydiffs_params_covers_local_and_diff_sources() {
    assert_eq!(
        querydiffs_params(&Source::Local { base: None }),
        serde_json::json!({})
    );
    assert_eq!(
        querydiffs_params(&Source::Diff(3)),
        serde_json::json!({"ids": ["3"]})
    );
}

#[test]
fn first_row_covers_arrays_and_scalars() {
    let row = serde_json::json!([{"realName": "Ada"}]);
    assert_eq!(
        first_row(&row),
        Some(&serde_json::json!({"realName": "Ada"}))
    );
    assert_eq!(first_row(&serde_json::json!("x")), None);
    assert_eq!(first_row(&serde_json::json!(null)), None);
}

#[test]
fn nonempty_or_empty_array_covers_all_json_shapes() {
    // Through `normalize_conduit_change`, which applies it to
    // `awayPaths`/`oldProperties`/`newProperties`.
    let normalized = |value: serde_json::Value| {
        normalize_conduit_change(&serde_json::json!({"awayPaths": value}))["awayPaths"].clone()
    };
    assert_eq!(normalized(serde_json::json!(null)), serde_json::json!([]));
    assert_eq!(normalized(serde_json::json!(false)), serde_json::json!([]));
    assert_eq!(normalized(serde_json::json!(true)), serde_json::json!(true));
    assert_eq!(normalized(serde_json::json!(0)), serde_json::json!([]));
    assert_eq!(normalized(serde_json::json!(0.0)), serde_json::json!([]));
    assert_eq!(normalized(serde_json::json!(5)), serde_json::json!(5));
    assert_eq!(normalized(serde_json::json!("")), serde_json::json!([]));
    assert_eq!(normalized(serde_json::json!("0")), serde_json::json!([]));
    assert_eq!(normalized(serde_json::json!("x")), serde_json::json!("x"));
    assert_eq!(normalized(serde_json::json!({})), serde_json::json!([]));
    assert_eq!(
        normalized(serde_json::json!({"a": 1})),
        serde_json::json!({"a": 1})
    );
}

#[test]
fn conduit_hunk_corpus_stringifies_scalars() {
    let corpus_of = |value: serde_json::Value| {
        let change = serde_json::json!({"hunks": [{"corpus": value}]});
        normalize_conduit_change(&change)["hunks"][0]["corpus"].clone()
    };
    assert_eq!(corpus_of(serde_json::json!(5)), serde_json::json!("5"));
    assert_eq!(
        corpus_of(serde_json::json!(true)),
        serde_json::json!("true")
    );
    assert_eq!(corpus_of(serde_json::json!(["x"])), serde_json::json!(""));
    assert_eq!(
        corpus_of(serde_json::json!({"a": 1})),
        serde_json::json!("")
    );
}

#[test]
fn strip_corpora_skips_changes_without_hunks() {
    let mut changes = vec![
        serde_json::json!({"metadata": {}}),
        serde_json::json!({"hunks": "x"}),
    ];
    assert!(strip_corpora(&mut changes).is_empty());
}

#[test]
fn blob_phids_skips_changes_without_metadata() {
    let changes = vec![serde_json::json!({"hunks": []})];
    assert!(blob_phids(&changes).is_empty());
}

#[test]
fn export_prompt_line_fails_without_tty() {
    // Piped stdin is never a TTY under `cargo test`; bail out (rather
    // than block on a real terminal) if that ever changes.
    if crate::prompt::stdin_is_terminal() {
        return;
    }
    assert_eq!(export_prompt_line("Base?"), None);
    // The flag was set; `take` asserts that and resets it.
    assert!(EXPORT_PROMPT_TTY_FAILED.take());
}

#[test]
fn local_diff_bytes_rejects_non_repo_and_bad_base() {
    let plain = plain_dir("diff");
    assert_eq!(
        local_diff_bytes(&plain, Format::Git, "HEAD").unwrap_err(),
        "not inside a git working copy"
    );
    std::fs::remove_dir_all(&plain).ok();

    let dir = export_e_repo("diff-repo", &[("f.txt", "one\n")]);
    // The failure echoes our own ref name (no git prose asserted).
    let err = local_diff_bytes(&dir, Format::Git, "no-such-base-xyz").unwrap_err();
    assert!(err.contains("no-such-base-xyz"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn create_tar_reports_member_failures() {
    // Missing members fail `tar -czf` with stderr naming them (our own
    // member names, not tar prose, are asserted). Unix-only: Windows
    // bsdtar words member errors differently, and coverage flows from
    // the Linux gates run anyway.
    let stage = plain_dir("tar");
    let out = stage.join("o.tar.gz");
    let err = create_tar(&stage, &out).unwrap_err();
    assert!(err.contains("blobs"), "{err}");
    std::fs::remove_dir_all(&stage).ok();
}

#[test]
fn bundle_out_path_prefers_absolute_paths() {
    let dir = plain_dir("outpath");
    // Absolute on every platform (a bare `/abs/...` is drive-relative
    // on Windows, so build one from the temp dir instead; nothing is
    // created on disk).
    let abs = std::env::temp_dir().join("t1170-abs-out.tar.gz");
    assert_eq!(
        bundle_out_path(&dir, abs.to_str().expect("utf8 tmp path")),
        abs
    );
    assert_eq!(bundle_out_path(&dir, "o.tar.gz"), dir.join("o.tar.gz"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn raw_diff_transport_diffs_single_path() {
    let dir = export_e_repo("rawdt", &[("f.txt", "one\n"), ("f.txt", "one\ntwo\n")]);
    let out = raw_diff_transport(&dir, "HEAD~1", "f.txt").unwrap();
    assert!(
        crate::patch::untransport_lossy(&out).contains("+two"),
        "{out:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn local_bundle_covers_pure_rename() {
    // A pure rename needs synthetic hunks, which re-diff through
    // `raw_diff_transport`.
    let dir = fixture_repo("rename");
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    git(&dir, &["mv", "f.txt", "g.txt"]);
    git(&dir, &["commit", "-qm", "mv"]);
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
    assert_eq!(
        export_bundle_with_client(&dir, &opts, "out.tar.gz", &client).unwrap(),
        "Done.\n"
    );
    let unpacked = unpack(&dir.join("out.tar.gz"));
    let changes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(unpacked.join("changes.json")).unwrap())
            .unwrap();
    let paths: Vec<&str> = changes
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|change| change.get("currentPath").and_then(|path| path.as_str()))
        .collect();
    assert!(paths.contains(&"g.txt"), "{paths:?}");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&unpacked).ok();
}

#[test]
fn file_data_at_revision_rejects_empty_paths() {
    let dir = plain_dir("filedata");
    assert_eq!(file_data_at_revision(&dir, "HEAD", ""), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn local_file_modes_tolerates_git_failures() {
    // Spawn failure (missing dir) and command failure (not a repo)
    // both read as no modes.
    let missing = std::env::temp_dir().join(format!(
        "arc-rs-export-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!missing.exists());
    assert!(local_file_modes(&missing, "HEAD", &["f.txt".to_string()]).is_empty());
    let plain = plain_dir("modes");
    assert!(local_file_modes(&plain, "HEAD", &["f.txt".to_string()]).is_empty());
    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn local_render_changes_maps_empty_current_path_to_none() {
    let dir = export_e_repo("emptycur", &[("f.txt", "one\n")]);
    let head = resolve_export_head(&dir).unwrap();
    let parsed = crate::diff_local::ParsedChange {
        old_path: "/dev/null".to_string(),
        current_path: String::new(),
        change_type: crate::diff_local::TYPE_CHANGE,
        file_type: crate::diff_local::FILE_TEXT,
        hunks: Vec::new(),
        hunk_len_explicit: Vec::new(),
        old_mode: None,
        new_mode: None,
        away_paths: Vec::new(),
        commit_hash: None,
        message: None,
    };
    let rendered = local_render_changes(&dir, "HEAD", &head, &[parsed]);
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].old_path, None);
    assert_eq!(rendered[0].current_path, None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn render_export_binary_body_zeroes_missing_data() {
    let zeros = "0".repeat(40);
    let body = render_export_binary_body(None, None).unwrap();
    assert!(
        body.contains(&format!("index {zeros}..{zeros}\n")),
        "{body}"
    );
    assert!(body.contains("literal 0\n"), "{body}");
    let body = render_export_binary_body(Some(b"ab"), Some(b"c")).unwrap();
    let old = crate::patch::git_blob_sha1(b"ab");
    let new = crate::patch::git_blob_sha1(b"c");
    assert!(body.contains(&format!("index {old}..{new}\n")), "{body}");
    assert!(body.contains("literal 1\n"), "{body}");
    assert!(body.contains("literal 2\n"), "{body}");
}

#[test]
fn build_export_binary_body_loads_blobs_by_phid() {
    use std::cell::RefCell;
    let seen: RefCell<Vec<(String, Option<String>)>> = RefCell::new(Vec::new());
    let loader = |phid: &str, name: Option<&str>| {
        seen.borrow_mut()
            .push((phid.to_string(), name.map(str::to_string)));
        Ok(format!("bytes-for-{phid}").into_bytes())
    };

    // Away change with inline data: no download.
    let change = cov_binary_change();
    let mut away = cov_binary_change();
    away.original_file_data = Some(b"OLD".to_vec());
    let body = build_export_binary_body(&change, Some(&away), &export_e_no_blobs).unwrap();
    assert!(body.contains("literal 3\n"), "{body}");
    assert!(body.contains("literal 0\n"), "{body}");

    // Away change with only a phid: downloads, naming the basename
    // (trailing slashes trimmed).
    let mut trailing = cov_binary_change();
    trailing.old_path = Some("a/b/".to_string());
    let mut away_phid = cov_binary_change();
    away_phid.old_binary_phid = Some("PHID-O".to_string());
    let body = build_export_binary_body(&trailing, Some(&away_phid), &loader).unwrap();
    assert!(body.contains("literal 16\n"), "{body}");
    assert_eq!(
        seen.borrow().as_slice(),
        &[("PHID-O".to_string(), Some("b".to_string()))]
    );
    seen.borrow_mut().clear();

    // Own old phid without data: downloads too.
    let mut old_phid = cov_binary_change();
    old_phid.old_binary_phid = Some("PHID-O2".to_string());
    old_phid.old_path = Some("x/y.bin".to_string());
    build_export_binary_body(&old_phid, None, &loader).unwrap();
    assert_eq!(
        seen.borrow().as_slice(),
        &[("PHID-O2".to_string(), Some("y.bin".to_string()))]
    );
    seen.borrow_mut().clear();

    // Neither data nor phids: zeros, no download.
    let bare = cov_binary_change();
    let body = build_export_binary_body(&bare, None, &export_e_no_blobs).unwrap();
    assert!(body.contains(&"0".repeat(40)), "{body}");

    // New phid without data: downloads naming the current basename.
    let mut new_phid = cov_binary_change();
    new_phid.new_binary_phid = Some("PHID-N".to_string());
    new_phid.current_path = Some("dir/n.bin".to_string());
    let body = build_export_binary_body(&new_phid, None, &loader).unwrap();
    assert!(body.contains("literal 16\n"), "{body}");
    assert_eq!(
        seen.borrow().as_slice(),
        &[("PHID-N".to_string(), Some("n.bin".to_string()))]
    );
}

#[test]
fn break_export_hunk_handles_context_only_corpora() {
    let hunk = crate::diff_local::ParsedHunk {
        old_start: 1,
        old_len: 2,
        new_start: 1,
        new_len: 2,
        corpus: " a\n b\n".to_string(),
        synthetic_counts: None,
    };
    assert!(break_export_hunk(&hunk).is_empty());
    assert!(render_export_hunks(&[hunk]).is_empty());
}

#[test]
fn scan_export_hunk_end_covers_context_breaks_and_markers() {
    // Change, short context, change: break point set, then cleared.
    let scan = scan_export_hunk_end(&["-a", " c1", " c2", " c3", " c4", "+b"], 0, 3);
    assert_eq!(scan.end, 6);
    assert_eq!((scan.old_lines, scan.new_lines, scan.adjust), (1, 1, 0));
    assert_eq!(scan.break_at, None);
    // Long context runs stop the hunk, keeping the break point.
    let scan = scan_export_hunk_end(
        &["-a", " c1", " c2", " c3", " c4", " c5", " c6", " c7", " c8"],
        0,
        3,
    );
    assert_eq!(scan.end, 7);
    assert_eq!(scan.break_at, Some(4));
    // Missing-newline markers adjust the counts.
    let scan = scan_export_hunk_end(&["-a", "\\ No newline at end of file", "+b"], 0, 3);
    assert_eq!(scan.adjust, 1);
    assert_eq!(scan.end, 3);
    // Foreign leading bytes count as changed lines.
    let scan = scan_export_hunk_end(&["-a", "X", "+b"], 0, 3);
    assert_eq!(scan.end, 3);
    assert_eq!((scan.old_lines, scan.new_lines), (1, 1));
}

#[test]
fn render_export_hunks_terminates_unterminated_corpora() {
    let hunk = crate::diff_local::ParsedHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 0,
        corpus: "-a".to_string(),
        synthetic_counts: None,
    };
    assert_eq!(render_export_hunks(&[hunk]), "@@ -1 +1,0 @@\n-a\n");
}

#[test]
fn collect_and_lookup_cover_binary_move_sources() {
    let mut away = cov_binary_change();
    away.change_type = crate::diff_local::TYPE_COPY_AWAY;
    away.away_paths = vec!["n.bin".to_string()];
    let mut here = cov_binary_change();
    here.change_type = crate::diff_local::TYPE_COPY_HERE;
    here.current_path = Some("n.bin".to_string());
    let changes = vec![away, here];
    let sources = collect_export_binary_sources(&changes);
    assert_eq!(sources.get("n.bin"), Some(&0));
    let found = lookup_export_old_binary(&changes, &sources, &changes[1]).unwrap();
    assert_eq!(found.change_type, crate::diff_local::TYPE_COPY_AWAY);
    assert!(lookup_export_old_binary(&changes, &sources, &changes[0]).is_none());
}

#[test]
fn render_export_git_change_covers_copy_and_skip_shapes() {
    // Copies print `copy from/to` headers.
    let copy = crate::patch::BundleChange {
        change_type: crate::diff_local::TYPE_COPY_HERE,
        file_type: crate::diff_local::FILE_TEXT,
        old_path: Some("o.txt".to_string()),
        current_path: Some("c.txt".to_string()),
        old_mode: None,
        new_mode: None,
        hunks: vec![cov_hunk("+x\n")],
        old_binary_phid: None,
        new_binary_phid: None,
        away_paths: Vec::new(),
        original_file_data: None,
        current_file_data: None,
    };
    let text = render_export_git_change(&copy, None, &export_e_no_blobs)
        .unwrap()
        .unwrap();
    assert!(text.contains("copy from o.txt\ncopy to c.txt\n"), "{text}");
    assert!(text.starts_with("diff --git a/o.txt b/c.txt\n"), "{text}");

    // Directories and move-away halves carry no output.
    let mut dir = cov_binary_change();
    dir.change_type = crate::diff_local::TYPE_CHANGE;
    dir.file_type = crate::diff_local::FILE_DIRECTORY;
    assert!(
        render_export_git_change(&dir, None, &export_e_no_blobs)
            .unwrap()
            .is_none()
    );
    let mut move_away = cov_binary_change();
    move_away.change_type = crate::diff_local::TYPE_MOVE_AWAY;
    move_away.file_type = crate::diff_local::FILE_TEXT;
    assert!(
        render_export_git_change(&move_away, None, &export_e_no_blobs)
            .unwrap()
            .is_none()
    );
    // Bodiless copy-away halves without mode changes are skipped.
    let mut copy_away = cov_binary_change();
    copy_away.change_type = crate::diff_local::TYPE_COPY_AWAY;
    copy_away.file_type = crate::diff_local::FILE_TEXT;
    assert!(
        render_export_git_change(&copy_away, None, &export_e_no_blobs)
            .unwrap()
            .is_none()
    );
}

#[test]
fn render_export_unified_skips_bodyless_changes() {
    let mut change = cov_binary_change();
    change.change_type = crate::diff_local::TYPE_CHANGE;
    change.file_type = crate::diff_local::FILE_TEXT;
    assert!(render_export_unified(&[change]).is_empty());
}

#[test]
fn export_e_multicopy_decomposes_with_two_sources() {
    let multicopy = |current: &str| crate::patch::BundleChange {
        change_type: crate::diff_local::TYPE_MULTICOPY,
        file_type: crate::diff_local::FILE_TEXT,
        old_path: None,
        current_path: Some(current.to_string()),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
        old_binary_phid: None,
        new_binary_phid: None,
        away_paths: Vec::new(),
        original_file_data: None,
        current_file_data: None,
    };
    let copy_here = |old: &str, current: &str| crate::patch::BundleChange {
        change_type: crate::diff_local::TYPE_COPY_HERE,
        file_type: crate::diff_local::FILE_TEXT,
        old_path: Some(old.to_string()),
        current_path: Some(current.to_string()),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
        old_binary_phid: None,
        new_binary_phid: None,
        away_paths: Vec::new(),
        original_file_data: None,
        current_file_data: None,
    };
    let mut changes = vec![
        multicopy("A"),
        copy_here("A", "A2"),
        multicopy("B"),
        copy_here("B", "B2"),
    ];
    decompose_export_multicopies(&mut changes).unwrap();
    assert_eq!(changes.len(), 2);
    assert!(
        changes
            .iter()
            .all(|change| change.change_type == crate::diff_local::TYPE_MOVE_HERE)
    );
    assert_eq!(changes[0].current_path.as_deref(), Some("A2"));
    assert_eq!(changes[1].current_path.as_deref(), Some("B2"));
}

#[test]
fn split_email_author_keeps_bare_names() {
    assert_eq!(
        split_email_author("justname"),
        (None, "justname".to_string())
    );
}

#[test]
fn export_bundle_reports_missing_credentials() {
    // The `.invalid` URI keeps this hermetic: no token can exist for it,
    // so `client_for` fails before any conduit traffic.
    let dir = plain_dir("bundlecreds");
    let opts = args_of(&[
        "--arcbundle",
        "o.tar.gz",
        "--revision",
        "1",
        "--conduit-uri",
        "https://export-bundle.invalid/",
    ]);
    let err = export_bundle(&dir, &opts, "o.tar.gz").unwrap_err();
    assert!(err.contains("no token for"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn download_export_blob_names_files_like_php() {
    // "bin-data" in base64.
    let mut routes = std::collections::HashMap::new();
    routes.insert("file.download".to_string(), envelope(r#""YmluLWRhdGE=""#));
    let base = serve(routes, 2);
    let client = crate::conduit::Client::new(&base, "t").unwrap();
    assert_eq!(
        download_export_blob(&client, "PHID-X", Some("a.bin")).unwrap(),
        b"bin-data"
    );
    assert_eq!(
        download_export_blob(&client, "PHID-Y", None).unwrap(),
        b"bin-data"
    );
}

#[test]
fn execute_dispatches_bundles_before_credentials() {
    let dir = plain_dir("execbundle");
    let err = execute(
        &dir,
        &argv(&[
            "--arcbundle",
            "o.tar.gz",
            "--revision",
            "1",
            "--conduit-uri",
            "https://export-2094.invalid/",
        ]),
    )
    .unwrap_err();
    assert!(err.contains("no token for"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_in_reports_runtime_failures_as_exceptions() {
    // Parses fine but fails at runtime (no conduit credentials), so the
    // red-banner exception envelope applies (exit 1, never 255).
    let dir = plain_dir("runexc");
    assert_eq!(
        run_in(
            &dir,
            &argv(&["--git", "--conduit-uri", "https://export-2147.invalid/"])
        ),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quiet_failures_use_the_silent_wording() {
    // Real `git`/`tar` failures always print something, so the silent
    // branches are pinned directly through the shared helper.
    assert_eq!(failure_detail_or("", "git diff failed"), "git diff failed");
    assert_eq!(
        failure_detail_or("  \n\t ", "tar -czf failed"),
        "tar -czf failed"
    );
    assert_eq!(failure_detail_or("boom\n", "silent"), "boom");
    assert_eq!(failure_detail_or("\n  spaced  \n", "silent"), "spaced");
}

#[test]
fn bundle_parts_surface_staging_failures() {
    // Against a missing stage dir the first member write fails; with a
    // directory squatting on `meta.json`, the second one does.
    let parent = plain_dir("parts-missing");
    let missing = parent.join("no-such-stage");
    let err = write_bundle_parts(
        &missing,
        &missing.join("o.tar.gz"),
        &[],
        &serde_json::Value::Null,
        &[],
        &[],
    )
    .unwrap_err();
    assert!(err.contains("failed to stage bundle files"), "{err}");
    std::fs::remove_dir_all(&parent).ok();

    let stage = plain_dir("parts-clash");
    std::fs::create_dir_all(stage.join("meta.json")).unwrap();
    let err = write_bundle_parts(
        &stage,
        &stage.join("o.tar.gz"),
        &[],
        &serde_json::Value::Null,
        &[],
        &[],
    )
    .unwrap_err();
    assert!(err.contains("failed to stage bundle files"), "{err}");
    std::fs::remove_dir_all(&stage).ok();
}

#[test]
fn ls_tree_entries_parse_or_fall_back() {
    // Well-formed `-z` entries yield `(name, mode)`; anything corrupt
    // (which real `git` never emits) yields `None` for the `100644`
    // fallback.
    assert_eq!(
        ls_tree_entry_mode(b"100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\tfile.txt"),
        Some(("file.txt".to_string(), "100644".to_string()))
    );
    assert_eq!(ls_tree_entry_mode(b""), None);
    assert_eq!(ls_tree_entry_mode(b"no-tab-here"), None);
    assert_eq!(ls_tree_entry_mode(b"100644\tfile.txt"), None);
    assert_eq!(ls_tree_entry_mode(b"\tnameless"), None);
    assert_eq!(ls_tree_entry_mode(b"\xff\xfe blob abc\tfile.txt"), None);
    assert_eq!(ls_tree_entry_mode(b"100644 blob abc\t\xff\xfe"), None);
}

/// A sink that always fails, standing in for a closed/broken stdout.
struct FailSink;

impl std::io::Write for FailSink {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("closed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn run_sink_failure_is_fatal() {
    // `--help` renders without conduit, so it exercises the sink arms:
    // a broken stdout fails, a working sink gets the usage bytes.
    let dir = plain_dir("runsink");
    let argv = argv(&["--help"]);
    assert_eq!(
        run_in_with(&dir, &argv, &mut FailSink),
        std::process::ExitCode::FAILURE
    );
    let mut buf = Vec::new();
    assert_eq!(
        run_in_with(&dir, &argv, &mut buf),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(buf, usage().into_bytes());
    std::fs::remove_dir_all(&dir).ok();
}
