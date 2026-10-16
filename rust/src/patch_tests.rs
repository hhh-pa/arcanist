//! Tests for [`patch`](super): parsing, sources, bundles, status,
//! branch names, commit messages, the svn parsers, dependencies, and the
//! patch engine.

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "arc_rs_patch_{}_{}_{}",
        std::process::id(),
        id,
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

fn options_of(words: &[&str]) -> Options {
    match parse_options(&argv(words)) {
        Ok(Some(options)) => options,
        other => panic!("expected options for {words:?}, got {other:?}"),
    }
}

fn parse_err(words: &[&str]) -> String {
    match parse_options(&argv(words)) {
        Err(message) => message,
        other => panic!("expected error for {words:?}, got {other:?}"),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    git(&dir, &["init", "-b", "master"]);
    git(&dir, &["config", "user.email", "patch@test"]);
    git(&dir, &["config", "user.name", "Patch Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);
    git(&dir, &["config", "core.autocrlf", "false"]);
    std::fs::write(dir.join("file.txt"), "base\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "base"]);
    dir
}

/// A stub Conduit call returning canned per-method responses.
fn stub_call(
    responses: std::collections::HashMap<String, serde_json::Value>,
) -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    move |method: &str, _params: &serde_json::Value| {
        responses
            .get(method)
            .cloned()
            .ok_or_else(|| format!("unexpected conduit call to {method}"))
    }
}

#[test]
fn help_short_circuits() {
    assert_eq!(parse_options(&argv(&["--help"])).unwrap(), None);
    assert_eq!(parse_options(&argv(&["-h"])).unwrap(), None);
    assert_eq!(parse_options(&argv(&["D1", "--help"])).unwrap(), None);
}

#[test]
fn defaults_are_empty() {
    let options = options_of(&[]);
    assert_eq!(options, Options::default());
    assert!(options.provided.is_empty());
    assert!(options.name.is_empty());
}

#[test]
fn parses_every_flag() {
    let options = options_of(&[
        "--revision",
        "D11",
        "--diff",
        "22",
        "--arcbundle",
        "a.bundle",
        "--patch",
        "b.patch",
        "--encoding",
        "latin1",
        "--update",
        "--nocommit",
        "--skip-dependencies",
        "--force",
        "--preview",
        "--conduit-uri",
        "https://phab.test/api/",
        "D33",
    ]);
    assert_eq!(options.revision.as_deref(), Some("D11"));
    assert_eq!(options.diff.as_deref(), Some("22"));
    assert_eq!(options.arcbundle.as_deref(), Some("a.bundle"));
    assert_eq!(options.patch.as_deref(), Some("b.patch"));
    assert_eq!(options.encoding.as_deref(), Some("latin1"));
    assert!(options.update);
    assert!(options.nocommit);
    assert!(options.skip_dependencies);
    assert!(options.force);
    assert!(options.preview);
    assert_eq!(
        options.conduit_uri.as_deref(),
        Some("https://phab.test/api/")
    );
    assert_eq!(options.name, vec!["D33".to_string()]);
    assert_eq!(
        options.provided,
        vec![
            "revision",
            "diff",
            "arcbundle",
            "patch",
            "encoding",
            "update",
            "nocommit",
            "skip-dependencies",
            "force",
            "preview",
            "conduit-uri",
        ]
    );
}

#[test]
fn parses_equals_values_and_double_dash() {
    let options = options_of(&["--revision=D7", "--", "--force"]);
    assert_eq!(options.revision.as_deref(), Some("D7"));
    assert!(!options.force);
    assert_eq!(options.name, vec!["--force".to_string()]);
}

#[test]
fn bool_equals_value_splices_back() {
    let options = options_of(&["--force=yes"]);
    assert!(options.force);
    assert_eq!(options.name, vec!["yes".to_string()]);
}

#[test]
fn repeated_flags_keep_first_provision_order() {
    let options = options_of(&["--force", "--update", "--force"]);
    assert_eq!(options.provided, vec!["force", "update"]);
}

#[test]
fn unknown_long_flag_fails() {
    assert_eq!(
        parse_err(&["--bookmark"]),
        "Unknown argument 'bookmark'. Try 'arc help'."
    );
}

#[test]
fn unknown_short_flag_fails() {
    assert_eq!(parse_err(&["-x"]), "Unknown argument 'x'. Try 'arc help'.");
}

#[test]
fn missing_value_fails() {
    assert_eq!(
        parse_err(&["--revision"]),
        "Option '--revision' requires a parameter."
    );
}

#[test]
fn empty_equals_value_stores_empty() {
    let options = options_of(&["--diff="]);
    assert_eq!(options.diff.as_deref(), Some(""));
    // PATCH-P13: PHP skips falsy values, so an empty `--diff` means no
    // source was given.
    assert_eq!(
        select_source(&options).unwrap_err(),
        "You must specify changes to apply to the working copy with \"D12345\", \"--revision\", \
         \"--diff\", \"--arcbundle\", or \"--patch\"."
    );
}

#[test]
fn update_and_nobranch_conflict() {
    assert_eq!(
        parse_err(&["--update", "--nobranch"]),
        "Arguments '--update' and '--nobranch' are mutually exclusive: 1"
    );
    assert_eq!(
        parse_err(&["--nobranch", "--update"]),
        "Arguments '--nobranch' and '--update' are mutually exclusive: 1"
    );
}

#[test]
fn check_supports_restricts_vcs() {
    let options = options_of(&["--nocommit"]);
    assert!(check_supports(&options, "git").is_ok());
    assert!(check_supports(&options, "hg").is_ok());
    assert_eq!(
        check_supports(&options, "svn").unwrap_err(),
        "Option '--nocommit' is not supported under svn."
    );
    let options = options_of(&["--skip-dependencies", "--force"]);
    assert_eq!(
        check_supports(&options, "svn").unwrap_err(),
        "Option '--skip-dependencies' is not supported under svn."
    );
    let options = options_of(&["--update"]);
    assert!(check_supports(&options, "svn").is_ok());
}

#[test]
fn normalizes_ids() {
    // PATCH-P13: revisions strip one D and pass through; diffs pass raw.
    assert_eq!(normalize_revision_id("D123").unwrap(), "123");
    assert_eq!(normalize_revision_id("d123").unwrap(), "123");
    assert_eq!(normalize_revision_id("123").unwrap(), "123");
    assert_eq!(normalize_diff_id("D9").unwrap(), "D9");
    assert_eq!(normalize_revision_id("Dx").unwrap(), "x");
    assert_eq!(normalize_revision_id("").unwrap(), "");
    assert_eq!(normalize_diff_id("abc").unwrap(), "abc");
}

#[test]
fn selects_each_source() {
    assert_eq!(
        select_source(&options_of(&["D123"])).unwrap(),
        Source::Named("123".to_string())
    );
    assert_eq!(
        select_source(&options_of(&["--revision", "D7"])).unwrap(),
        Source::Revision("7".to_string())
    );
    assert_eq!(
        select_source(&options_of(&["--diff", "9"])).unwrap(),
        Source::Diff("9".to_string())
    );
    assert_eq!(
        select_source(&options_of(&["--arcbundle", "a.bundle"])).unwrap(),
        Source::Arcbundle("a.bundle".to_string())
    );
    assert_eq!(
        select_source(&options_of(&["--patch", "fix.patch"])).unwrap(),
        Source::Patch("fix.patch".to_string())
    );
}

#[test]
fn source_selection_enforces_exclusivity() {
    assert_eq!(
        select_source(&options_of(&[])).unwrap_err(),
        "You must specify changes to apply to the working copy with \"D12345\", \"--revision\", \
         \"--diff\", \"--arcbundle\", or \"--patch\"."
    );
    assert_eq!(
        select_source(&options_of(&["D1", "D2"])).unwrap_err(),
        "Specify at most one revision name."
    );
    assert_eq!(
        select_source(&options_of(&["D1", "--diff", "2"])).unwrap_err(),
        "Options \"D12345\", \"--revision\", \"--diff\", \"--arcbundle\" and \"--patch\" are \
         mutually exclusive. Choose exactly one patch source."
    );
    // PATCH-P13: non-numeric names pass through to Conduit, which fails
    // server-side; there is no local validation left.
    assert_eq!(
        select_source(&options_of(&["nope"])).unwrap(),
        Source::Named("nope".to_string())
    );
}

#[test]
fn bundle_helpers() {
    let bundle = Bundle {
        base_revision: Some("abc".to_string()),
        author_name: Some("Ada".to_string()),
        author_email: Some("ada@test".to_string()),
        ..Bundle::default()
    };
    assert!(bundle.has_base_revision());
    assert_eq!(bundle.full_author().as_deref(), Some("Ada <ada@test>"));
    let empty = Bundle::default();
    assert!(!empty.has_base_revision());
    assert_eq!(empty.full_author(), None);
    let base_empty = Bundle {
        base_revision: Some(String::new()),
        ..Bundle::default()
    };
    assert!(!base_empty.has_base_revision());
}

#[test]
fn decodes_bytes() {
    assert_eq!(decode_bytes(b"plain", None).unwrap(), "plain");
    assert_eq!(decode_bytes(b"plain", Some("UTF-8")).unwrap(), "plain");
    assert_eq!(decode_bytes(b"plain", Some("utf8")).unwrap(), "plain");
    assert_eq!(decode_bytes(b"\xff", None).unwrap(), "\u{FFFD}");
    assert_eq!(decode_bytes(&[0xE9], Some("latin1")).unwrap(), "é");
    assert_eq!(decode_bytes(&[0x80], Some("windows-1252")).unwrap(), "€");
    // Latin-1 maps 1:1 (a control scalar here, not U+FFFD).
    assert_eq!(decode_bytes(&[0x81], Some("latin1")).unwrap(), "");
    // Valid UTF-8 passes through untouched whatever the encoding reads.
    assert_eq!(
        decode_bytes("café\n".as_bytes(), Some("windows-1252")).unwrap(),
        "café\n"
    );
    // Unknown encodings throw.
    assert_eq!(
        decode_bytes(b"\xff", Some("shift-jis")).unwrap_err(),
        "Unknown encoding \"shift-jis\"."
    );
}

#[test]
fn encodes_bytes_to_target() {
    assert_eq!(encode_bytes("café\n", None).unwrap(), "café\n".as_bytes());
    assert_eq!(
        encode_bytes("café\n", Some("UTF-8")).unwrap(),
        "café\n".as_bytes()
    );
    assert_eq!(
        encode_bytes("café €\n", Some("windows-1252")).unwrap(),
        b"caf\xe9 \x80\n".to_vec()
    );
    assert_eq!(
        encode_bytes("café\n", Some("latin1")).unwrap(),
        b"caf\xe9\n".to_vec()
    );
    assert_eq!(
        encode_bytes("x", Some("shift-jis")).unwrap_err(),
        "Unknown encoding \"shift-jis\"."
    );
}

#[test]
fn gap2_cp1252_table_covers_all_bytes() {
    // Full Windows-1252 table check: the 27 defined 0x80-0x9F bytes map
    // per the standard, the 5 undefined bytes become U+FFFD, and every
    // other byte maps to the identical Latin-1 scalar.
    let specials: &[(u8, char)] = &[
        (0x80, '€'),
        (0x82, '‚'),
        (0x83, 'ƒ'),
        (0x84, '„'),
        (0x85, '…'),
        (0x86, '†'),
        (0x87, '‡'),
        (0x88, 'ˆ'),
        (0x89, '‰'),
        (0x8A, 'Š'),
        (0x8B, '‹'),
        (0x8C, 'Œ'),
        (0x8E, 'Ž'),
        (0x91, '‘'),
        (0x92, '’'),
        (0x93, '“'),
        (0x94, '”'),
        (0x95, '•'),
        (0x96, '–'),
        (0x97, '—'),
        (0x98, '˜'),
        (0x99, '™'),
        (0x9A, 'š'),
        (0x9B, '›'),
        (0x9C, 'œ'),
        (0x9E, 'ž'),
        (0x9F, 'Ÿ'),
    ];
    assert_eq!(specials.len(), 27);
    for (byte, expected) in specials {
        assert_eq!(
            decode_bytes(&[*byte], Some("windows-1252")).unwrap(),
            expected.to_string(),
            "byte 0x{byte:02X}"
        );
    }
    for byte in [0x81u8, 0x8D, 0x8F, 0x90, 0x9D] {
        assert_eq!(
            decode_bytes(&[byte], Some("windows-1252")).unwrap(),
            "\u{FFFD}",
            "byte 0x{byte:02X}"
        );
    }
    for byte in 0u8..=255 {
        if (0x80..0xA0).contains(&byte) {
            continue;
        }
        let expected = char::from_u32(u32::from(byte)).unwrap().to_string();
        assert_eq!(
            decode_bytes(&[byte], Some("cp1252")).unwrap(),
            expected,
            "byte 0x{byte:02X}"
        );
    }
    assert_eq!(decode_bytes(&[0x80], None).unwrap(), "\u{FFFD}");
}

#[test]
fn reads_patch_bytes_from_file() {
    let dir = temp_dir("patch_bytes");
    let path = dir.join("fix.patch");
    std::fs::write(&path, "patch text\n").unwrap();
    let mut io = Io::buffered();
    let bytes = read_patch_bytes(&path.to_string_lossy(), &mut io).unwrap();
    assert_eq!(bytes, b"patch text\n");
    let missing = dir.join("missing.patch");
    let error = read_patch_bytes(&missing.to_string_lossy(), &mut io).unwrap_err();
    assert!(error.starts_with("Failed to read patch file"), "{error}");
}

#[test]
fn reads_patch_bytes_from_scripted_stdin() {
    let mut io = Io::buffered();
    io.patch_stdin = Some("stdin patch\n".to_string());
    let bytes = read_patch_bytes("-", &mut io).unwrap();
    assert_eq!(bytes, b"stdin patch\n");
}

#[test]
fn file_bundle_rejects_empty_stdin() {
    let mut io = Io::buffered();
    io.patch_stdin = Some(String::new());
    assert_eq!(
        load_file_bundle("-", None, &mut io).unwrap_err(),
        NO_STDIN_PATCH
    );
    io.patch_stdin = Some("diff --git a/f.txt b/f.txt\n".to_string());
    let bundle = load_file_bundle("-", Some("utf-8"), &mut io).unwrap();
    assert_eq!(bundle.text, "diff --git a/f.txt b/f.txt\n");
    assert_eq!(
        bundle,
        Bundle {
            text: "diff --git a/f.txt b/f.txt\n".to_string(),
            ..Bundle::default()
        }
    );
}

#[test]
fn file_bundle_decodes_with_encoding() {
    let dir = temp_dir("file_bundle_enc");
    let path = dir.join("fix.patch");
    let mut bytes = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+".to_vec();
    bytes.push(0xE9);
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).unwrap();
    let mut io = Io::buffered();
    let bundle = load_file_bundle(&path.to_string_lossy(), Some("latin1"), &mut io).unwrap();
    assert_eq!(
        bundle.text,
        "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+é\n"
    );
}

#[test]
fn quotes_git_targets() {
    assert_eq!(quote_git_target("plain.txt"), "plain.txt");
    assert_eq!(quote_git_target("a\"b"), "\"a\\\"b\"");
    assert_eq!(quote_git_target("a\\b"), "\"a\\\\b\"");
    assert_eq!(quote_git_target("a\nb"), "\"a\\nb\"");
    assert_eq!(quote_git_target("a\tb"), "\"a\\tb\"");
    assert_eq!(quote_git_target("a\x01b"), "\"a\\001b\"");
}

fn parsed_hunk(
    old_start: u64,
    old_len: u64,
    new_start: u64,
    new_len: u64,
    corpus: &str,
) -> crate::diff_local::ParsedHunk {
    crate::diff_local::ParsedHunk {
        old_start,
        old_len,
        new_start,
        new_len,
        corpus: corpus.to_string(),
    }
}

fn bundle_change(change_type: u32, old: Option<&str>, current: Option<&str>) -> BundleChange {
    BundleChange {
        change_type,
        file_type: FILE_TEXT,
        old_path: old.map(str::to_string),
        current_path: current.map(str::to_string),
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

#[test]
fn renders_add_change_delete() {
    let mut changes = vec![
        BundleChange {
            hunks: vec![parsed_hunk(0, 0, 1, 1, "+hi\n")],
            ..bundle_change(TYPE_ADD, None, Some("new.txt"))
        },
        BundleChange {
            hunks: vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\n")],
            ..bundle_change(TYPE_CHANGE, Some("old.txt"), Some("old.txt"))
        },
        BundleChange {
            old_mode: Some("100644".to_string()),
            ..bundle_change(TYPE_DELETE, Some("gone.txt"), None)
        },
    ];
    assert_eq!(
        render_git_patch(&mut changes).unwrap(),
        "diff --git a/new.txt b/new.txt\n\
         new file mode 100644\n\
         --- /dev/null\n\
         +++ b/new.txt\n\
         @@ -0,0 +1 @@\n\
         +hi\n\
         diff --git a/old.txt b/old.txt\n\
         --- a/old.txt\n\
         +++ b/old.txt\n\
         @@ -1 +1 @@\n\
         -a\n\
         +b\n\
         diff --git a/gone.txt b/gone.txt\n\
         deleted file mode 100644\n\
         \n"
    );
}

#[test]
fn renders_mode_changes_and_renames() {
    let mut changes = vec![
        BundleChange {
            old_mode: Some("100644".to_string()),
            new_mode: Some("100755".to_string()),
            ..bundle_change(TYPE_CHANGE, Some("run.sh"), Some("run.sh"))
        },
        bundle_change(TYPE_MOVE_HERE, Some("a.txt"), Some("b.txt")),
        bundle_change(TYPE_COPY_HERE, Some("a.txt"), Some("c.txt")),
    ];
    assert_eq!(
        render_git_patch(&mut changes).unwrap(),
        "diff --git a/run.sh b/run.sh\n\
         old mode 100644\n\
         new mode 100755\n\
         diff --git a/a.txt b/b.txt\n\
         rename from a.txt\n\
         rename to b.txt\n\
         diff --git a/a.txt b/c.txt\n\
         copy from a.txt\n\
         copy to c.txt\n\
         \n"
    );
}

#[test]
fn skips_directories_move_aways_and_pure_copies() {
    let mut changes = vec![
        BundleChange {
            file_type: FILE_DIRECTORY,
            ..bundle_change(TYPE_ADD, None, Some("dir"))
        },
        bundle_change(TYPE_MOVE_AWAY, Some("a.txt"), None),
        bundle_change(TYPE_COPY_AWAY, Some("a.txt"), Some("a.txt")),
    ];
    assert_eq!(render_git_patch(&mut changes).unwrap(), "\n");
}

#[test]
fn rejects_binary_and_image() {
    let mut changes = vec![BundleChange {
        file_type: FILE_BINARY,
        ..bundle_change(TYPE_CHANGE, Some("a.bin"), Some("a.bin"))
    }];
    assert_eq!(
        render_git_patch(&mut changes).unwrap_err(),
        "binary patches are not yet ported by arc patch"
    );
    let mut changes = vec![BundleChange {
        file_type: FILE_IMAGE,
        ..bundle_change(TYPE_CHANGE, Some("a.png"), Some("a.png"))
    }];
    assert_eq!(
        render_git_patch(&mut changes).unwrap_err(),
        "binary patches are not yet ported by arc patch"
    );
}

#[test]
fn multicopy_decomposes_with_sibling() {
    let mut changes = vec![
        bundle_change(TYPE_MULTICOPY, Some("gone.txt"), Some("b.txt")),
        bundle_change(TYPE_COPY_HERE, Some("b.txt"), Some("c.txt")),
    ];
    let text = render_git_patch(&mut changes).unwrap();
    assert!(
        text.contains("rename from b.txt\nrename to c.txt\n"),
        "{text}"
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_MOVE_HERE);
}

#[test]
fn multicopy_without_sibling_fails() {
    let mut changes = vec![bundle_change(
        TYPE_MULTICOPY,
        Some("gone.txt"),
        Some("b.txt"),
    )];
    assert_eq!(
        render_git_patch(&mut changes).unwrap_err(),
        "Failed to decompose multicopy changeset in order to generate diff."
    );
}

#[test]
fn render_appends_missing_trailing_newline() {
    let mut changes = vec![BundleChange {
        hunks: vec![parsed_hunk(1, 1, 1, 1, "-a\n+b")],
        ..bundle_change(TYPE_CHANGE, Some("a.txt"), Some("a.txt"))
    }];
    let text = render_git_patch(&mut changes).unwrap();
    assert!(text.ends_with("+b\n\n"), "{text:?}");
}

#[test]
fn parses_bundle_meta() {
    let meta = parse_bundle_meta(
        br#"{"baseRevision":"abc","revisionID":12,"encoding":"latin1","authorName":"Ada","authorEmail":"ada@test"}"#,
    );
    assert_eq!(meta.base_revision.as_deref(), Some("abc"));
    assert_eq!(meta.revision_id, Some(12));
    assert_eq!(meta.encoding.as_deref(), Some("latin1"));
    assert_eq!(meta.author_name.as_deref(), Some("Ada"));
    assert_eq!(meta.author_email.as_deref(), Some("ada@test"));
    assert_eq!(parse_bundle_meta(b"not json"), BundleMeta::default());
    assert_eq!(parse_bundle_meta(b"{}"), BundleMeta::default());
}

/// Build an arcbundle tarball from `files` (path -> bytes) in `dir`.
fn write_arcbundle(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let stage = dir.join("stage");
    for (rel, bytes) in files {
        let full = stage.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, bytes).unwrap();
    }
    let bundle = dir.join(name);
    let output = std::process::Command::new("tar")
        .args(["-cf"])
        .arg(&bundle)
        .args(["-C"])
        .arg(&stage)
        .arg(".")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tar failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    bundle
}

#[test]
fn arcbundle_roundtrip() {
    let dir = temp_dir("arcbundle");
    let bundle_path = write_arcbundle(
        &dir,
        "test.bundle",
        &[
            (
                "meta.json",
                br#"{"baseRevision":"abc123","revisionID":7,"authorName":"Ada"}"#,
            ),
            (
                "changes.json",
                br#"[{"type":1,"fileType":1,"currentPath":"new.txt","hunks":[{"corpus":0}]}]"#,
            ),
            ("hunks/0", b"@@ -0,0 +1 @@\n+hi\n"),
        ],
    );
    let name = bundle_path.to_string_lossy().to_string();
    assert!(tar_list(&name).unwrap().contains(&"meta.json".to_string()));
    let bundle = load_arcbundle(&name, None).unwrap();
    assert!(
        bundle.text.contains("new file mode 100644"),
        "{}",
        bundle.text
    );
    assert_eq!(bundle.base_revision.as_deref(), Some("abc123"));
    assert_eq!(bundle.revision_id, Some(7));
    assert_eq!(bundle.author_name.as_deref(), Some("Ada"));
}

#[test]
fn arcbundle_errors() {
    let dir = temp_dir("arcbundle_err");
    let missing = dir.join("missing.bundle").to_string_lossy().to_string();
    let error = load_arcbundle(&missing, None).unwrap_err();
    assert!(error.starts_with("failed to read arcbundle"), "{error}");
    let no_changes = write_arcbundle(&dir, "empty.bundle", &[("meta.json", b"{}")]);
    assert_eq!(
        load_arcbundle(&no_changes.to_string_lossy(), None).unwrap_err(),
        format!(
            "failed to read arcbundle '{}': member 'changes.json' is missing",
            no_changes.to_string_lossy()
        )
    );
    let bad_corpus = write_arcbundle(
        &dir,
        "bad.bundle",
        &[("changes.json", br#"[{"hunks":[{"corpus":"x"}]}]"#)],
    );
    let error = load_arcbundle(&bad_corpus.to_string_lossy(), None).unwrap_err();
    assert!(error.ends_with("hunk corpus is not an index"), "{error}");
}

#[test]
fn parses_diff_rows_as_map_or_list() {
    let map = serde_json::json!({
        "5": {"id": 5, "revisionID": 9, "sourceControlBaseRevision": "base",
              "authorName": "Ada", "authorEmail": "ada@test"},
        "3": {"id": 3}
    });
    let rows = parse_diff_rows(&map);
    assert_eq!(rows.len(), 2);
    // PATCH-P3: server order is kept (no id sort); the map above lists 5
    // before 3, so 5 stays head.
    assert_eq!(rows[0].id, 5);
    assert_eq!(rows[1].id, 3);
    assert_eq!(rows[0].revision_id, Some(9));
    assert_eq!(rows[0].base_revision, "base");
    assert_eq!(rows[0].author_name, "Ada");
    assert_eq!(rows[0].author_email, "ada@test");
    let list = serde_json::json!([{"id": 1}]);
    assert_eq!(parse_diff_rows(&list).len(), 1);
    assert!(parse_diff_rows(&serde_json::json!("x")).is_empty());
    assert!(parse_diff_rows(&serde_json::json!({})).is_empty());
}

#[test]
fn extracts_raw_diff_text() {
    assert_eq!(
        raw_diff_text(&serde_json::json!("text")).as_deref(),
        Some("text")
    );
    assert_eq!(
        raw_diff_text(&serde_json::json!({"rawDiff": "rd"})).as_deref(),
        Some("rd")
    );
    assert_eq!(
        raw_diff_text(&serde_json::json!({"diff": "d"})).as_deref(),
        Some("d")
    );
    assert_eq!(raw_diff_text(&serde_json::json!({})), None);
}

fn diff_responses() -> std::collections::HashMap<String, serde_json::Value> {
    let change = serde_json::json!({
        "type": "2", "fileType": "1",
        "oldPath": "a.txt", "currentPath": "a.txt",
        "hunks": [{"oldOffset": "1", "oldLength": "1",
                   "newOffset": "1", "newLength": "1",
                   "corpus": "-a\n+b\n"}],
    });
    std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY_DIFFS.to_string(),
        // PATCH-P3: server order is id DESC (newest first), so the head row
        // carries the latest changes.
        serde_json::json!({
            "2": {"id": "2", "revisionID": "7", "sourceControlBaseRevision": "base2",
                  "authorName": "Ada", "authorEmail": "ada@test",
                  "changes": [change]},
            "1": {"id": "1", "revisionID": "7", "sourceControlBaseRevision": "base1",
                  "authorName": "Ada", "authorEmail": "ada@test", "changes": []},
        }),
    )])
}

#[test]
fn fetches_latest_diff_bundle() {
    let call = stub_call(diff_responses());
    let bundle = fetch_revision_bundle(&call, 7).unwrap();
    assert_eq!(
        bundle.text,
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\n"
    );
    assert_eq!(bundle.base_revision.as_deref(), Some("base2"));
    assert_eq!(bundle.revision_id, Some(7));
    assert_eq!(bundle.author_name.as_deref(), Some("Ada"));
    assert_eq!(bundle.author_email.as_deref(), Some("ada@test"));
}

#[test]
fn fetch_prefers_row_revision_when_unset() {
    let call = stub_call(diff_responses());
    let bundle = fetch_conduit_bundle(&call, &serde_json::json!({"ids": [2]}), None).unwrap();
    assert!(
        bundle.text.contains("@@ -1 +1 @@\n-a\n+b\n"),
        "{}",
        bundle.text
    );
    assert_eq!(bundle.revision_id, Some(7));
}

#[test]
fn fetch_fails_without_diffs() {
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY_DIFFS.to_string(),
        serde_json::json!({}),
    )]));
    assert_eq!(
        fetch_revision_bundle(&call, 7).unwrap_err(),
        NO_DIFF_OR_REVISION
    );
}

#[test]
fn fetch_renders_empty_text_without_changes() {
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY_DIFFS.to_string(),
        serde_json::json!({
            "2": {"id": "2", "revisionID": "7", "sourceControlBaseRevision": "base2",
                  "authorName": "Ada", "authorEmail": "ada@test"},
        }),
    )]));
    let bundle = fetch_diff_bundle(&call, 2).unwrap();
    assert_eq!(bundle.text, "\n");
}

#[test]
fn fetch_propagates_conduit_errors() {
    let call = stub_call(std::collections::HashMap::new());
    let error = fetch_diff_bundle(&call, 2).unwrap_err();
    assert!(error.contains("differential.querydiffs"), "{error}");
}

#[test]
fn fetches_patch_commit_message() {
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_GET_COMMIT_MESSAGE.to_string(),
        serde_json::json!("Title\n\nSummary."),
    )]));
    assert_eq!(
        fetch_patch_commit_message(&call, 7).unwrap(),
        "Title\n\nSummary."
    );
    let bad = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_GET_COMMIT_MESSAGE.to_string(),
        serde_json::json!({"nope": true}),
    )]));
    assert_eq!(
        fetch_patch_commit_message(&bad, 7).unwrap_err(),
        "differential.getcommitmessage response is not a string"
    );
}

#[test]
fn revision_suffix_finds_latest_closed() {
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY.to_string(),
        serde_json::json!([
            {"id": 1, "status": STATUS_CLOSED, "dateModified": 10},
            {"id": 2, "status": 0, "dateModified": 20},
            {"id": 3, "status": STATUS_CLOSED, "dateModified": 30}
        ]),
    )]));
    assert_eq!(
        revision_suffix_for_hash(&call, "gtcm", "abc").as_deref(),
        Some(" \\ D3")
    );
    let none = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY.to_string(),
        serde_json::json!([{"id": 2, "status": 0}]),
    )]));
    assert_eq!(revision_suffix_for_hash(&none, "gtcm", "abc"), None);
    let failing = stub_call(std::collections::HashMap::new());
    assert_eq!(revision_suffix_for_hash(&failing, "gtcm", "abc"), None);
}

#[test]
fn loads_each_bundle_kind() {
    let dir = temp_dir("load_bundle");
    let patch_path = dir.join("fix.patch");
    std::fs::write(&patch_path, "diff --git a/f b/f\n").unwrap();
    let options = Options::default();
    let mut io = Io::buffered();
    let loaded = load_bundle(
        &Source::Patch(patch_path.to_string_lossy().to_string()),
        &options,
        &mut io,
        None,
    )
    .unwrap();
    assert_eq!(loaded.bundle.text, "diff --git a/f b/f\n");
    assert!(loaded.arcbundle_dicts.is_none());

    let call = stub_call(diff_responses());
    let loaded = load_bundle(
        &Source::Revision("7".to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    assert!(loaded.bundle.text.contains("@@ -1 +1 @@\n-a\n+b\n"));
    assert_eq!(loaded.bundle.revision_id, Some(7));
    let loaded = load_bundle(
        &Source::Named("7".to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    assert_eq!(loaded.bundle.revision_id, Some(7));
    let loaded = load_bundle(
        &Source::Diff("2".to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    assert!(loaded.bundle.text.contains("@@ -1 +1 @@\n-a\n+b\n"));

    let bundle_path = write_arcbundle(
        &dir,
        "t.bundle",
        &[
            ("meta.json", br#"{"revisionID":3}"#),
            (
                "changes.json",
                br#"[{"type":2,"fileType":1,"oldPath":"a.txt","currentPath":"a.txt","hunks":[{"oldOffset":1,"oldLength":1,"newOffset":1,"newLength":1,"corpus":0}]}]"#,
            ),
            ("hunks/0", b"-a\n+b\n"),
        ],
    );
    let loaded = load_bundle(
        &Source::Arcbundle(bundle_path.to_string_lossy().to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    assert_eq!(loaded.bundle.revision_id, Some(3));
    let (dicts, corpora) = loaded.arcbundle_dicts.unwrap();
    assert_eq!(dicts.len(), 1);
    assert_eq!(corpora, vec![vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\n")]]);
    assert!(loaded.bundle.text.contains("@@ -1 +1 @@\n-a\n+b\n"));
}

#[test]
fn load_bundle_passes_raw_ids_and_requires_conduit() {
    let options = Options::default();
    let mut io = Io::buffered();
    // PATCH-P13: raw ids reach Conduit verbatim (no local validation);
    // the stub answers regardless, so the loads succeed here and the
    // server would fail on the real wire.
    let seen = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        seen.borrow_mut()
            .push((method.to_string(), params.to_string()));
        Ok(serde_json::json!({
            "2": {"id": "2", "revisionID": "7",
                  "sourceControlBaseRevision": "base2",
                  "authorName": "Ada", "authorEmail": "ada@test",
                  "changes": []},
        }))
    };
    load_bundle(
        &Source::Revision("x".to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    load_bundle(
        &Source::Diff("D12".to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    let seen = seen.borrow();
    assert!(seen[0].1.contains("\"x\""), "{}", seen[0].1);
    assert!(seen[1].1.contains("\"D12\""), "{}", seen[1].1);
    for source in [
        Source::Named("7".to_string()),
        Source::Revision("7".to_string()),
        Source::Diff("2".to_string()),
        Source::Arcbundle("a.bundle".to_string()),
    ] {
        let error = load_bundle(&source, &options, &mut io, None).unwrap_err();
        assert!(error.contains("no conduit URI"), "{error}");
    }
}

#[test]
fn temp_patch_file_roundtrip() {
    let path = {
        let file = TempPatchFile::write("hello\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "hello\n");
        file.path.clone()
    };
    assert!(!path.exists());
}

#[test]
fn vcs_names() {
    assert_eq!(Vcs::Git.name(), "git");
    assert_eq!(Vcs::Hg.name(), "hg");
    assert_eq!(Vcs::Svn.name(), "svn");
    assert_eq!(Backends::default().hg, hg::HgCommand::system());
    assert_eq!(Backends::default().svn, svn::SvnCommand::system());
}

#[test]
fn detects_vcs() {
    let repo = make_repo("detect_git");
    assert_eq!(detect_vcs(&repo), Some(Vcs::Git));
    let hg_dir = temp_dir("detect_hg");
    std::fs::create_dir(hg_dir.join(".hg")).unwrap();
    assert_eq!(detect_vcs(&hg_dir), Some(Vcs::Hg));
    let plain = temp_dir("detect_none");
    assert_eq!(detect_vcs(&plain), None);
}

#[test]
fn parses_porcelain_v2() {
    let output = b"# branch.oid abc\0? new.txt\0! ignored\x001 M. N... 100644 100644 100644 a a staged.txt\x001 .M N... 100644 100644 100644 a a dirty.txt\x001 .M S.M. 100644 100644 100644 a a submod\x002 RM N... 100644 100644 100644 a a R100 renamed.txt\0orig.txt\0u UU N... 1 2 3 4 5 6 7 merged.txt\0";
    let status = parse_porcelain_v2(output).unwrap();
    assert_eq!(status.untracked, vec!["new.txt".to_string()]);
    assert_eq!(status.uncommitted, vec!["staged.txt".to_string()]);
    assert_eq!(
        status.unstaged,
        vec![
            "dirty.txt".to_string(),
            "merged.txt".to_string(),
            "orig.txt".to_string(),
            "renamed.txt".to_string(),
            "submod".to_string(),
        ]
    );
    assert_eq!(status.externals, vec!["submod".to_string()]);
    assert!(status.conflicts.is_empty());
    assert!(parse_porcelain_v2(b"").unwrap().untracked.is_empty());
}

#[test]
fn git_status_roundtrip() {
    let repo = make_repo("git_status");
    assert_eq!(git_tree_status(&repo).unwrap(), TreeStatus::default());
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    let status = git_tree_status(&repo).unwrap();
    assert_eq!(status.unstaged, vec!["file.txt".to_string()]);
    assert_eq!(status.untracked, vec!["new.txt".to_string()]);
}

#[test]
fn splits_hg_conflicts() {
    let (conflicts, rest) = split_hg_conflicts("M a.txt\nU b.txt\n? c.txt\nU d.txt\n");
    assert_eq!(conflicts, vec!["b.txt".to_string(), "d.txt".to_string()]);
    assert_eq!(rest, "M a.txt\n? c.txt\n");
}

#[test]
fn parses_svn_status_xml() {
    let xml = r#"<status>
<entry path="mod.txt"><wc-status props="none" item="modified"></wc-status></entry>
<entry path="new.txt"><wc-status props="none" item="unversioned"></wc-status></entry>
<entry path="gone.txt"><wc-status props="none" item="missing"></wc-status></entry>
<entry path="conf.txt"><wc-status props="none" item="conflicted"></wc-status></entry>
<entry path="half"><wc-status props="none" item="incomplete"></wc-status></entry>
<entry path="tree.txt"><wc-status props="none" item="modified" tree-conflicted="true"></wc-status></entry>
<entry path="ext"><wc-status props="none" item="external"></wc-status></entry>
</status>"#;
    let status = parse_svn_status_xml(xml).unwrap();
    assert!(status.uncommitted.is_empty());
    assert!(status.unstaged.is_empty());
    assert_eq!(status.untracked, vec!["new.txt".to_string()]);
    assert_eq!(status.missing, vec!["gone.txt".to_string()]);
    assert_eq!(
        status.conflicts,
        vec!["conf.txt".to_string(), "tree.txt".to_string()]
    );
    assert_eq!(status.incomplete, vec!["half".to_string()]);
    assert_eq!(status.externals, vec!["ext".to_string()]);
}

#[test]
fn svn_status_xml_rejects_unknown() {
    let bad_item = r#"<status><entry path="a"><wc-status props="none" item="weird"></wc-status></entry></status>"#;
    assert_eq!(
        parse_svn_status_xml(bad_item).unwrap_err(),
        "Unrecognized item status 'weird'."
    );
    let bad_props = r#"<status><entry path="a"><wc-status props="conflicted" item="modified"></wc-status></entry></status>"#;
    assert_eq!(
        parse_svn_status_xml(bad_props).unwrap_err(),
        "Unrecognized property status 'conflicted'."
    );
    assert_eq!(
        parse_svn_status_xml("<status></status>").unwrap(),
        TreeStatus::default()
    );
}

#[test]
fn clean_check_passes_clean_and_force() {
    let repo = make_repo("clean_ok");
    let backends = Backends::default();
    let mut io = Io::canned(&[]);
    assert_eq!(
        require_clean_tree(&repo, Vcs::Git, &backends, &mut io, false).unwrap(),
        StashState::Clean
    );
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        require_clean_tree(&repo, Vcs::Git, &backends, &mut io, true).unwrap(),
        StashState::Clean
    );
}

#[test]
fn clean_check_prompts_for_untracked() {
    let repo = make_repo("clean_untracked");
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    let backends = Backends::default();
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        require_clean_tree(&repo, Vcs::Git, &backends, &mut io, false).unwrap(),
        StashState::Clean
    );
    assert!(
        io.out.stdout.contains("Untracked changes in working copy"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains(".git/info/exclude"),
        "{}",
        io.out.stdout
    );
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        require_clean_tree(&repo, Vcs::Git, &backends, &mut io, false).unwrap_err(),
        land::ABORT
    );
}

#[test]
fn clean_check_commit_prompt_paths() {
    // PATCH-P9: the yes-path now commits via the editor (covered below
    // through `resolve_uncommitted_tree` with a stub editor; the public
    // entry would launch the real `$EDITOR` here).
    let repo = make_repo("clean_commit");
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let backends = Backends::default();
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        require_clean_tree(&repo, Vcs::Git, &backends, &mut io, false).unwrap_err(),
        "You can not continue with uncommitted changes. Commit or discard them before proceeding."
    );
}

#[test]
fn clean_check_autostashes_with_config() {
    let repo = make_repo("clean_stash");
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.autostash": true}"#).unwrap();
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let backends = Backends::default();
    let mut io = Io::canned(&["y", "n"]);
    assert_eq!(
        require_clean_tree(&repo, Vcs::Git, &backends, &mut io, false).unwrap(),
        StashState::Stashed
    );
    assert!(
        io.out.stdout.contains("Stashing uncommitted changes"),
        "{}",
        io.out.stdout
    );
    let read_back = || {
        std::fs::read_to_string(repo.join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n")
    };
    assert_eq!(read_back(), "base\n");
    unstash_changes(&repo, StashState::Stashed).unwrap();
    assert_eq!(read_back(), "dirty\n");
    unstash_changes(&repo, StashState::Clean).unwrap();
}

#[test]
fn autostash_config_values() {
    let repo = temp_dir("autostash_cfg");
    assert!(!autostash_permitted(&repo));
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.autostash": true}"#).unwrap();
    assert!(autostash_permitted(&repo));
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.autostash": "no"}"#).unwrap();
    assert!(!autostash_permitted(&repo));
    let child = repo.join("sub");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.autostash": 1}"#).unwrap();
    assert!(autostash_permitted(&child));
}

#[test]
fn update_working_copy_pulls() {
    let origin = temp_dir("update_origin");
    git(&origin, &["init", "--bare"]);
    let repo = make_repo("update_repo");
    git(
        &repo,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    git(&repo, &["push", "-u", "origin", "master"]);
    let backends = Backends::default();
    let mut io = Io::buffered();
    update_working_copy(&repo, Vcs::Git, &backends, &mut io).unwrap();
    assert_eq!(io.out.stdout, "Updating working copy...\nDone.\n");
}

#[test]
fn branch_commit_dependency_predicates() {
    assert!(can_branch(Vcs::Git));
    assert!(can_branch(Vcs::Hg));
    assert!(!can_branch(Vcs::Svn));
    let plain = Options::default();
    assert!(should_branch(&plain));
    assert!(should_commit(&plain));
    let options = Options {
        nobranch: true,
        nocommit: true,
        ..Options::default()
    };
    assert!(!should_branch(&options));
    assert!(!should_commit(&options));
    let bundle = Bundle {
        revision_id: Some(7),
        ..Bundle::default()
    };
    assert!(should_apply_dependencies(&bundle, &plain, Vcs::Git));
    assert!(!should_apply_dependencies(&bundle, &plain, Vcs::Svn));
    assert!(!should_apply_dependencies(
        &Bundle::default(),
        &plain,
        Vcs::Git
    ));
    let skip = Options {
        skip_dependencies: true,
        ..Options::default()
    };
    assert!(!should_apply_dependencies(&bundle, &skip, Vcs::Git));
}

#[test]
fn parses_dependency_nodes() {
    let row = serde_json::json!({
        "phid": "PHID-DREV-1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]}
    });
    let node = parse_dependency_node(&row).unwrap();
    assert_eq!(node.phid, "PHID-DREV-1");
    assert_eq!(node.id, 1);
    assert_eq!(node.depends_on, vec!["PHID-DREV-2".to_string()]);
    assert!(parse_dependency_node(&serde_json::json!({})).is_none());
    let bare = serde_json::json!({"phid": "P", "id": 3});
    assert!(parse_dependency_node(&bare).unwrap().depends_on.is_empty());
}

#[test]
fn orders_dependencies_parents_first() {
    let nodes = vec![
        DependencyNode {
            phid: "start".to_string(),
            id: 1,
            depends_on: vec!["mid".to_string()],
        },
        DependencyNode {
            phid: "mid".to_string(),
            id: 2,
            depends_on: vec!["old".to_string()],
        },
        DependencyNode {
            phid: "old".to_string(),
            id: 3,
            depends_on: Vec::new(),
        },
    ];
    let (order, cyclic) = order_dependencies(&nodes);
    assert!(!cyclic);
    assert_eq!(order, vec![2, 1, 0]);
    let cyclic_nodes = vec![
        DependencyNode {
            phid: "a".to_string(),
            id: 1,
            depends_on: vec!["b".to_string()],
        },
        DependencyNode {
            phid: "b".to_string(),
            id: 2,
            depends_on: vec!["a".to_string()],
        },
    ];
    let (order, cyclic) = order_dependencies(&cyclic_nodes);
    assert!(cyclic);
    assert_eq!(order.len(), 2);
}

#[test]
fn loads_dependency_nodes_breadth_first() {
    let start = serde_json::json!([{
        "phid": "PHID-DREV-1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2", "PHID-DREV-3"]}
    }]);
    let dep2 = serde_json::json!([{
        "phid": "PHID-DREV-2", "id": 2, "hashes": {"gtcm": "local"},
        "auxiliary": {"phabricator:depends-on": []}
    }]);
    let dep3 = serde_json::json!([{
        "phid": "PHID-DREV-3", "id": 3, "hashes": {"gtcm": "remote"},
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]}
    }]);
    let calls = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        calls.borrow_mut().push(params.to_string());
        assert_eq!(method, diff_submit::METHOD_QUERY);
        if params.to_string().contains("\"ids\"") {
            Ok(start.clone())
        } else {
            let mut rows = dep2.as_array().cloned().unwrap_or_default();
            rows.extend(dep3.as_array().cloned().unwrap_or_default());
            Ok(serde_json::Value::Array(rows))
        }
    };
    let has_local = |hash: Option<&str>| hash == Some("local");
    let (nodes, start_phid) = load_dependency_nodes(&call, 1, &has_local, "gtcm").unwrap();
    assert_eq!(start_phid, "PHID-DREV-1");
    // PATCH-P4: one batched query per level (start + one frontier call).
    assert_eq!(calls.borrow().len(), 2);
    assert!(calls.borrow()[1].contains("PHID-DREV-2"));
    assert!(calls.borrow()[1].contains("PHID-DREV-3"));
    // PATCH-P5: PHID-DREV-2 is kept with empty edges (commit local);
    // PHID-DREV-3 is kept with its edges.
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[1].phid, "PHID-DREV-2");
    assert!(nodes[1].depends_on.is_empty());
    assert_eq!(nodes[2].phid, "PHID-DREV-3");
    let empty = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY.to_string(),
        serde_json::json!([]),
    )]));
    let (nodes, _) = load_dependency_nodes(&empty, 1, &|_| false, "gtcm").unwrap();
    assert!(nodes.is_empty());
}

#[test]
fn plans_dependency_children_with_flags() {
    let repo = make_repo("dep_plan");
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let options = Options::default();
    let mut io = Io::buffered();
    let start = serde_json::json!([{
        "phid": "PHID-DREV-1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]}
    }]);
    let dep = serde_json::json!([{
        "phid": "PHID-DREV-2", "id": 2, "auxiliary": {}
    }]);
    let both = serde_json::json!([
        {"phid": "PHID-DREV-1", "id": 1},
        {"phid": "PHID-DREV-2", "id": 2}
    ]);
    let graph = |_: &str, params: &serde_json::Value| {
        let text = params.to_string();
        if text.contains("\"ids\"") {
            Ok(start.clone())
        } else if text.contains("PHID-DREV-1") {
            Ok(both.clone())
        } else {
            Ok(dep.clone())
        }
    };
    let children =
        plan_dependency_children(&repo, Vcs::Git, &bundle, &options, &mut io, &graph).unwrap();
    assert_eq!(children.len(), 1);
    assert!(children[0].force);
    assert!(children[0].skip_dependencies);
    assert!(children[0].nobranch);
    assert!(!children[0].nocommit);
    assert_eq!(children[0].revision.as_deref(), Some("D2"));
    assert_eq!(
        plan_dependency_children(&repo, Vcs::Svn, &bundle, &options, &mut io, &graph).unwrap(),
        vec![]
    );
    assert_eq!(
        plan_dependency_children(
            &repo,
            Vcs::Git,
            &Bundle::default(),
            &options,
            &mut io,
            &graph
        )
        .unwrap(),
        vec![]
    );
}

#[test]
fn dependency_cycle_decline_skips() {
    let repo = make_repo("dep_cycle");
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let options = Options::default();
    let graph = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY.to_string(),
        serde_json::json!([{
            "phid": "PHID-DREV-1", "id": 1,
            "auxiliary": {"phabricator:depends-on": ["PHID-DREV-1"]}
        }]),
    )]));
    let mut io = Io::canned(&["n"]);
    // Self-cycle: start prunes to a single node; still cyclic, decline skips.
    let children =
        plan_dependency_children(&repo, Vcs::Git, &bundle, &options, &mut io, &graph).unwrap();
    assert!(children.is_empty());
    assert!(io.out.stdout.contains("cycle"), "{}", io.out.stdout);
}

#[test]
fn apply_dependencies_runs_children() {
    let repo = make_repo("dep_apply");
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let options = Options::default();
    let mut io = Io::buffered();
    let start = serde_json::json!([{
        "phid": "PHID-DREV-1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]}
    }]);
    let dep = serde_json::json!([{
        "phid": "PHID-DREV-2", "id": 2, "auxiliary": {}
    }]);
    let both = serde_json::json!([
        {"phid": "PHID-DREV-1", "id": 1},
        {"phid": "PHID-DREV-2", "id": 2}
    ]);
    let graph = |_: &str, params: &serde_json::Value| {
        let text = params.to_string();
        if text.contains("\"ids\"") {
            Ok(start.clone())
        } else if text.contains("PHID-DREV-1") {
            Ok(both.clone())
        } else {
            Ok(dep.clone())
        }
    };
    let mut seen = Vec::new();
    apply_dependencies(
        &repo,
        Vcs::Git,
        &bundle,
        &options,
        &mut io,
        &graph,
        &mut |child| {
            seen.push(child.revision.clone().unwrap());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen, vec!["D2".to_string()]);
}

/// A bundle changing `file.txt` from `base` to `patched`.
fn change_bundle() -> Bundle {
    Bundle {
        text: "diff --git a/file.txt b/file.txt\n\
               --- a/file.txt\n\
               +++ b/file.txt\n\
               @@ -1 +1 @@\n\
               -base\n\
               +patched\n"
            .to_string(),
        ..Bundle::default()
    }
}

#[test]
fn git_branch_name_suffixes() {
    let mut io = Io::buffered();
    let plain = Bundle::default();
    assert_eq!(
        git_branch_name(&plain, &|_| false, &mut io).unwrap(),
        "arcpatch"
    );
    let rev = Bundle {
        revision_id: Some(7),
        ..Bundle::default()
    };
    assert_eq!(
        git_branch_name(&rev, &|_| false, &mut io).unwrap(),
        "arcpatch-D7"
    );
    let taken = ["arcpatch-D7", "arcpatch-D7_1"];
    assert_eq!(
        git_branch_name(&rev, &|name| taken.contains(&name), &mut io).unwrap(),
        "arcpatch-D7_2"
    );
    assert!(
        io.out
            .stdout
            .contains("Branch name arcpatch-D7 already exists"),
        "{}",
        io.out.stdout
    );
    let mut io = Io::buffered();
    assert_eq!(
        git_branch_name(&rev, &|_| true, &mut io).unwrap_err(),
        "Arc was unable to automagically make a name for this patch. Please clean up your working \
         copy and try again."
    );
}

#[test]
fn hg_bookmark_name_suffixes() {
    let mut io = Io::buffered();
    let plain = Bundle::default();
    assert_eq!(
        hg_bookmark_name(&plain, &|_| false, &mut io).unwrap(),
        "arcpatch"
    );
    let rev = Bundle {
        revision_id: Some(7),
        ..Bundle::default()
    };
    assert_eq!(
        hg_bookmark_name(&rev, &|_| false, &mut io).unwrap(),
        "arcpatch-D7"
    );
    let taken = ["arcpatch-D7", "arcpatch-D7-1"];
    assert_eq!(
        hg_bookmark_name(&rev, &|name| taken.contains(&name), &mut io).unwrap(),
        "arcpatch-D7-2"
    );
    assert!(
        io.out
            .stdout
            .contains("Bookmark name arcpatch-D7 already exists"),
        "{}",
        io.out.stdout
    );
    let mut io = Io::buffered();
    assert!(hg_bookmark_name(&rev, &|_| true, &mut io).is_err());
}

#[test]
fn git_create_branch_checks_out() {
    let repo = make_repo("git_branch");
    let mut io = Io::buffered();
    let bundle = Bundle::default();
    let name = git_create_branch(&repo, &bundle, false, &mut io).unwrap();
    assert_eq!(name, "arcpatch");
    assert_eq!(git(&repo, &["branch", "--show-current"]), "arcpatch");
    assert!(
        io.out
            .stdout
            .contains("Created and checked out branch arcpatch."),
        "{}",
        io.out.stdout
    );
    let head = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "master"]);
    let mut io = Io::buffered();
    let based = Bundle {
        base_revision: Some(head.clone()),
        ..Bundle::default()
    };
    let name = git_create_branch(&repo, &based, true, &mut io).unwrap();
    assert_eq!(name, "arcpatch_1");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
}

#[test]
fn git_submodule_update_passes_through() {
    let repo = make_repo("submodule");
    let mut io = Io::buffered();
    git_submodule_update(&repo, &mut io).unwrap();
    assert!(io.out.stdout.is_empty());
}

#[test]
fn git_apply_patch_applies_and_fails() {
    let repo = make_repo("git_apply");
    let mut io = Io::buffered();
    git_apply_patch(&repo, &change_bundle(), &mut io).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("file.txt")).unwrap(),
        "patched\n"
    );
    // `--index` staged the change.
    assert!(!git(&repo, &["diff", "--cached", "--name-only"]).is_empty());
    let mut io = Io::buffered();
    let bad = Bundle {
        text: "not a patch\n".to_string(),
        ..Bundle::default()
    };
    assert_eq!(
        git_apply_patch(&repo, &bad, &mut io).unwrap_err(),
        "Unable to apply patch!"
    );
    assert!(
        io.out.stdout.contains("** Patch Failed! **"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn git_commit_patch_commits_with_author() {
    let repo = make_repo("git_commit");
    let mut io = Io::buffered();
    let bundle = Bundle {
        author_name: Some("Ada".to_string()),
        author_email: Some("ada@test".to_string()),
        ..change_bundle()
    };
    git_apply_patch(&repo, &bundle, &mut io).unwrap();
    git_commit_patch(&repo, &bundle, "Title\n\nBody.").unwrap();
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]), "Title");
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%an <%ae>"]),
        "Ada <ada@test>"
    );
}

#[test]
fn resolve_commit_message_paths() {
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_GET_COMMIT_MESSAGE.to_string(),
        serde_json::json!("Fetched message"),
    )]));
    let rev = Bundle {
        revision_id: Some(7),
        ..Bundle::default()
    };
    assert_eq!(
        resolve_commit_message(&rev, Some(&call)).unwrap(),
        "Fetched message"
    );
    assert_eq!(
        resolve_commit_message(&rev, None).unwrap_err(),
        "Unable to determine a commit message for this patch. Re-run with --nocommit to apply \
         without committing."
    );
    assert!(
        resolve_commit_message(&Bundle::default(), Some(&call))
            .unwrap_err()
            .contains("--nocommit")
    );
}

#[test]
fn apply_git_nocommit_reports_applied() {
    let repo = make_repo("apply_nocommit");
    let options = Options {
        nocommit: true,
        nobranch: true,
        ..Options::default()
    };
    let mut io = Io::buffered();
    apply_git(
        &repo,
        &change_bundle(),
        &options,
        &mut io,
        None,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("file.txt")).unwrap(),
        "patched\n"
    );
    assert!(
        io.out
            .stderr
            .contains("** APPLIED ** Successfully applied patch."),
        "{}",
        io.out.stderr
    );
}

#[test]
fn apply_git_commits_and_cherry_picks_temp() {
    let repo = make_repo("apply_cherry");
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_GET_COMMIT_MESSAGE.to_string(),
        serde_json::json!("Cherry me"),
    )]));
    // Simulate the nobranch+commit flow: temp branch holds the commit.
    let mut io = Io::buffered();
    let bundle = Bundle {
        revision_id: Some(9),
        ..change_bundle()
    };
    let new = git_create_branch(&repo, &bundle, false, &mut io).unwrap();
    let options = Options {
        nobranch: true,
        ..Options::default()
    };
    apply_git(
        &repo,
        &bundle,
        &options,
        &mut io,
        Some(&call),
        true,
        Some("master"),
        Some(&new),
    )
    .unwrap();
    assert!(
        io.out
            .stderr
            .contains("** COMMITTED ** Successfully committed patch."),
        "{}",
        io.out.stderr
    );
    assert_eq!(git(&repo, &["branch", "--show-current"]), "master");
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]), "Cherry me");
    assert!(!git(&repo, &["branch", "--list", &new]).contains(&new));
}

#[test]
fn cherry_pick_failure_banner_and_cleanup() {
    let repo = make_repo("cherry_fail");
    git(&repo, &["checkout", "-b", "temp"]);
    std::fs::write(repo.join("file.txt"), "temp\n").unwrap();
    git(&repo, &["commit", "-am", "temp"]);
    git(&repo, &["checkout", "master"]);
    std::fs::write(repo.join("file.txt"), "master\n").unwrap();
    git(&repo, &["commit", "-am", "master"]);
    let mut io = Io::buffered();
    assert!(git_cherry_pick_temp(&repo, &mut io, "master", "temp").is_err());
    assert!(
        io.out.stdout.contains("** Cherry Pick Failed!**"),
        "{}",
        io.out.stdout
    );
    assert!(!git(&repo, &["branch", "--list", "temp"]).contains("temp"));
}

/// Fake `hg` backend logging argv to `.fakehg.log` (portable .bat/.sh).
fn fake_hg_backend(dir: &std::path::Path) -> hg::HgCommand {
    #[cfg(windows)]
    let (name, content) = (
        "fakehg-gap2.bat",
        include_str!("../tests/fixtures/vcs/fakehg.bat"),
    );
    #[cfg(not(windows))]
    let (name, content) = (
        "fakehg-gap2.sh",
        include_str!("../tests/fixtures/vcs/fakehg.sh"),
    );
    let script = dir.join(name);
    std::fs::write(&script, content).unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    #[cfg(windows)]
    {
        hg::HgCommand::custom("cmd", &["/C", &script.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        hg::HgCommand::custom(&script.to_string_lossy(), &[])
    }
}

#[test]
fn gap2_hg_dotted_revisions_pass_verbatim() {
    // PHP never interprets revision text in the patch flow: revisions
    // ride `%s` / `hgsprintf` straight into `hg` argv (e.g.
    // `ArcanistPatchWorkflow.php:795`, `getBookmarkName()`), so `hg`
    // alone gives `...` meaning. The port must not parse or split them.
    let dir = temp_dir("hg_dots_verbatim");
    let hg = fake_hg_backend(&dir);
    hg_rebase_temp(&dir, &hg, "master...tip", "tempmark").unwrap();
    let log = std::fs::read_to_string(dir.join(".fakehg.log")).unwrap();
    assert!(
        log.contains("-r master...tip"),
        "three-dot revision must reach hg unmodified: {log:?}"
    );
    assert!(log.contains("bookmark --delete tempmark"), "{log:?}");
}

#[test]
fn hg_and_svn_flows_fail_without_binaries() {
    // No hg/svn binaries ship on this machine; the flows must surface the
    // spawn errors instead of hanging or panicking.
    let dir = temp_dir("no_hg_svn");
    let backends = Backends::default();
    let bundle = Bundle::default();
    let mut io = Io::buffered();
    assert!(hg_tree_status(&dir, &backends.hg, &["status"]).is_err());
    assert!(svn_tree_status(&dir, &backends.svn, &["--xml", "status"]).is_err());
    assert!(hg_create_bookmark(&dir, &backends.hg, &bundle, false, &mut io).is_err());
    assert!(hg_import_patch(&dir, &backends.hg, &bundle, &mut io).is_err());
    assert!(hg_commit_patch(&dir, &backends.hg, &bundle, "msg").is_err());
    assert!(hg_rebase_temp(&dir, &backends.hg, "a", "b").is_err());
    let options = Options::default();
    assert!(
        apply_hg(
            &dir,
            &backends.hg,
            &bundle,
            &options,
            &mut io,
            None,
            false,
            None,
            None
        )
        .is_err()
    );
    let mut io = Io::buffered();
    assert!(
        maybe_create_branch(&dir, Vcs::Hg, &backends, &bundle, &options, &mut io, false).is_err()
    );
}

#[test]
fn unquotes_git_paths() {
    assert_eq!(unquote_git_path("plain"), "plain");
    assert_eq!(unquote_git_path("\"a\\\"b\""), "a\"b");
    assert_eq!(unquote_git_path("\"a\\\\b\""), "a\\b");
    assert_eq!(unquote_git_path("\"a\\nb\""), "a\nb");
    assert_eq!(unquote_git_path("\"a\\tb\""), "a\tb");
    assert_eq!(unquote_git_path("\"a\\001b\""), "a\x01b");
    assert_eq!(unquote_git_path("\"trailing\\\""), "trailing\\");
    assert_eq!(unquote_git_path("\"unclosed"), "\"unclosed");
}

#[test]
fn parses_git_patch_sections_for_svn() {
    let text = "diff --git a/mod.txt b/mod.txt\n\
                --- a/mod.txt\n\
                +++ b/mod.txt\n\
                @@ -1 +1 @@\n\
                -a\n\
                +b\n\
                diff --git a/new.txt b/new.txt\n\
                new file mode 100644\n\
                --- /dev/null\n\
                +++ b/new.txt\n\
                @@ -0,0 +1 @@\n\
                +hi\n\
                diff --git a/gone.txt b/gone.txt\n\
                deleted file mode 100644\n\
                --- a/gone.txt\n\
                +++ /dev/null\n\
                @@ -1 +0,0 @@\n\
                -bye\n\
                diff --git a/old.txt b/new.txt\n\
                rename from old.txt\n\
                rename to new.txt\n\
                --- a/old.txt\n\
                +++ b/new.txt\n\
                @@ -1 +1 @@\n\
                -a\n\
                +b\n\
                diff --git a/run.sh b/run.sh\n\
                old mode 100644\n\
                new mode 100755\n";
    let changes = parse_git_patch_for_svn(text);
    assert_eq!(changes.len(), 6);
    assert_eq!(changes[0].change_type, TYPE_CHANGE);
    assert_eq!(changes[0].old_path.as_deref(), Some("mod.txt"));
    assert!(changes[0].hunks.starts_with("@@ -1 +1 @@\n"));
    assert_eq!(changes[1].change_type, TYPE_ADD);
    assert_eq!(changes[1].old_path, None);
    assert_eq!(changes[1].current_path.as_deref(), Some("new.txt"));
    assert_eq!(
        changes[1]
            .new_props
            .get("unix:filemode")
            .map(String::as_str),
        Some("100644")
    );
    assert_eq!(changes[2].change_type, TYPE_DELETE);
    assert_eq!(changes[2].current_path, None);
    // Renames emit MOVE_AWAY plus MOVE_HERE.
    assert_eq!(changes[3].change_type, TYPE_MOVE_AWAY);
    assert_eq!(changes[3].current_path.as_deref(), Some("old.txt"));
    assert_eq!(changes[4].change_type, TYPE_MOVE_HERE);
    assert_eq!(changes[4].old_path.as_deref(), Some("old.txt"));
    assert_eq!(changes[4].current_path.as_deref(), Some("new.txt"));
    assert_eq!(changes[5].change_type, TYPE_CHANGE);
    assert_eq!(changes[5].old_path.as_deref(), Some("run.sh"));
    assert!(changes[5].hunks.is_empty());
    assert_eq!(
        changes[5]
            .new_props
            .get("unix:filemode")
            .map(String::as_str),
        Some("100755")
    );
}

#[test]
fn svn_text_parsing_marks_symlinks_and_binary() {
    let link = parse_git_patch_for_svn(
        "diff --git a/link b/link\n\
         new file mode 120000\n\
         --- /dev/null\n\
         +++ b/link\n\
         @@ -0,0 +1 @@\n\
         +target/path\n",
    );
    assert_eq!(link.len(), 1);
    assert_eq!(link[0].file_type, FILE_SYMLINK);
    assert_eq!(link[0].symlink_target.as_deref(), Some("target/path"));
    let binary = parse_git_patch_for_svn(
        "diff --git a/a.bin b/a.bin\nBinary files a/a.bin and b/a.bin differ\n",
    );
    assert_eq!(binary.len(), 1);
    assert!(binary[0].hunks.is_empty());
    let quoted = parse_git_patch_for_svn(
        "diff --git \"a/sp ace.txt\" \"b/sp ace.txt\"\n\
         --- \"a/sp ace.txt\"\n\
         +++ \"b/sp ace.txt\"\n\
         @@ -1 +1 @@\n-a\n+b\n",
    );
    assert_eq!(quoted[0].old_path.as_deref(), Some("sp ace.txt"));
}

#[test]
fn svn_changes_from_dicts_map() {
    let dicts = vec![
        serde_json::json!({
            "type": 1, "fileType": 1, "currentPath": "n.txt",
            "oldProperties": {}, "newProperties": {"unix:filemode": "100755"}
        }),
        serde_json::json!({"type": 5, "fileType": 5, "oldPath": "l", "currentPath": "l2"}),
    ];
    let corpora = vec![
        vec![parsed_hunk(0, 0, 1, 1, "+x\n")],
        vec![parsed_hunk(1, 1, 1, 1, "-t\n+u\n")],
    ];
    let changes = svn_changes_from_dicts(&dicts, &corpora);
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].change_type, TYPE_ADD);
    assert_eq!(changes[0].hunks, "@@ -0,0 +1 @@\n+x\n");
    assert_eq!(
        changes[0]
            .new_props
            .get("unix:filemode")
            .map(String::as_str),
        Some("100755")
    );
    assert_eq!(changes[1].file_type, FILE_SYMLINK);
    assert_eq!(changes[1].symlink_target.as_deref(), Some("u"));
}

#[test]
fn unified_for_svn_renders() {
    let change = SvnChange {
        change_type: TYPE_CHANGE,
        file_type: FILE_TEXT,
        old_path: Some("a.txt".to_string()),
        current_path: Some("a.txt".to_string()),
        hunks: "@@ -1 +1 @@\n-a\n+b\n".to_string(),
        ..SvnChange::default()
    };
    let text = unified_for_svn(&change);
    assert!(text.starts_with("Index: a.txt\n"), "{text}");
    assert!(text.contains(&"=".repeat(67)), "{text}");
    assert!(text.contains("--- a.txt\n+++ a.txt\n"), "{text}");
    let add = SvnChange {
        change_type: TYPE_ADD,
        current_path: Some("n.txt".to_string()),
        hunks: "@@\n".to_string(),
        ..SvnChange::default()
    };
    assert!(unified_for_svn(&add).contains("--- /dev/null\n+++ n.txt\n"));
    assert!(unified_for_svn(&SvnChange::default()).is_empty());
}

#[test]
fn escapes_svn_paths_and_maps_props() {
    assert_eq!(escape_svn_path("plain"), "plain");
    assert_eq!(escape_svn_path("a@b"), "a@b@");
    assert_eq!(
        svn_prop_mapping("other", Some("v".to_string())),
        ("other".to_string(), Some("v".to_string()))
    );
    assert_eq!(
        svn_prop_mapping("unix:filemode", Some("100755".to_string())),
        ("svn:executable".to_string(), Some("on".to_string()))
    );
    assert_eq!(
        svn_prop_mapping("unix:filemode", Some("100644".to_string())),
        ("svn:executable".to_string(), None)
    );
    assert_eq!(
        svn_prop_mapping("unix:filemode", None),
        ("svn:executable".to_string(), None)
    );
}

#[test]
fn svn_create_parents_skips_existing() {
    let dir = temp_dir("svn_parents");
    std::fs::create_dir(dir.join("sub")).unwrap();
    // Existing parents need no `svn add`, so no binary is required.
    svn_create_parents(&dir, &svn::SvnCommand::system(), "sub/file.txt").unwrap();
    svn_create_parents(&dir, &svn::SvnCommand::system(), "top.txt").unwrap();
    // Missing parents fail at `svn add` (no binary) after creating the dir.
    assert!(svn_create_parents(&dir, &svn::SvnCommand::system(), "new/file.txt").is_err());
    assert!(dir.join("new").exists());
}

fn svn_modification(path: &str) -> SvnChange {
    SvnChange {
        change_type: TYPE_CHANGE,
        file_type: FILE_TEXT,
        old_path: Some(path.to_string()),
        current_path: Some(path.to_string()),
        hunks: "@@ -1 +1 @@\n-a\n+b\n".to_string(),
        ..SvnChange::default()
    }
}

#[test]
fn apply_svn_applies_with_stubs() {
    let dir = temp_dir("svn_apply");
    std::fs::write(dir.join("mod.txt"), "a\n").unwrap();
    let changes = vec![
        svn_modification("mod.txt"),
        SvnChange {
            change_type: TYPE_CHANGE,
            file_type: FILE_SYMLINK,
            old_path: Some("link".to_string()),
            current_path: Some("link".to_string()),
            symlink_target: Some("target".to_string()),
            ..SvnChange::default()
        },
        SvnChange {
            change_type: TYPE_CHANGE,
            file_type: FILE_TEXT,
            old_path: Some("empty.txt".to_string()),
            current_path: Some("empty.txt".to_string()),
            ..SvnChange::default()
        },
    ];
    let mut io = Io::buffered();
    let mut patched = Vec::new();
    let mut touched = Vec::new();
    let mut linked = Vec::new();
    let outcome = apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        &changes,
        &mut io,
        &mut |path, patch| {
            patched.push(path.to_string());
            assert!(patch.contains("Index: mod.txt"), "{patch}");
            Ok((true, "patching mod.txt\n".to_string()))
        },
        &mut |path| {
            touched.push(path.to_string());
            Ok(())
        },
        &mut |target, path| {
            linked.push((target.to_string(), path.to_string()));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(outcome, SvnOutcome::Applied);
    assert_eq!(patched, vec!["mod.txt".to_string()]);
    assert_eq!(touched, vec!["empty.txt".to_string()]);
    assert_eq!(linked, vec![("target".to_string(), "link".to_string())]);
    assert!(
        io.out.stdout.contains("patching mod.txt"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("** OKAY ** Successfully applied patch"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn apply_svn_reports_hunk_failures() {
    let dir = temp_dir("svn_fail");
    let mut io = Io::buffered();
    let outcome = apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        &[svn_modification("mod.txt")],
        &mut io,
        &mut |_, _| Ok((false, String::new())),
        &mut |_| Ok(()),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(outcome, SvnOutcome::HunksFailed);
    assert!(
        io.out
            .stdout
            .contains("** WARNING ** Some hunks could not be applied"),
        "{}",
        io.out.stdout
    );
    let mut io = Io::buffered();
    let error = apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        &[svn_modification("mod.txt")],
        &mut io,
        &mut |_, _| Err("boom".to_string()),
        &mut |_| Ok(()),
        &mut |_, _| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error, "boom");
}

#[test]
fn apply_svn_prompts_for_missing_sources() {
    let dir = temp_dir("svn_prompt");
    let delete = SvnChange {
        change_type: TYPE_DELETE,
        current_path: Some("gone.txt".to_string()),
        ..SvnChange::default()
    };
    let mut io = Io::canned(&["n"]);
    let error = apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        std::slice::from_ref(&delete),
        &mut io,
        &mut |_, _| Ok((true, String::new())),
        &mut |_| Ok(()),
        &mut |_, _| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error, land::ABORT);
    let mut io = Io::canned(&["y"]);
    let outcome = apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        &[delete],
        &mut io,
        &mut |_, _| Ok((true, String::new())),
        &mut |_| Ok(()),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(outcome, SvnOutcome::Applied);
    let copy = SvnChange {
        change_type: TYPE_COPY_HERE,
        old_path: Some("missing.txt".to_string()),
        current_path: Some("copy.txt".to_string()),
        ..SvnChange::default()
    };
    let mut io = Io::canned(&["y"]);
    apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        &[copy],
        &mut io,
        &mut |_, _| Ok((true, String::new())),
        &mut |_| Ok(()),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    assert!(
        io.out.stdout.contains("copies 'missing.txt'"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn run_touch_and_symlink() {
    let dir = temp_dir("touch_link");
    run_touch_file(&dir, "new.txt").unwrap();
    assert!(dir.join("new.txt").exists());
    std::fs::write(dir.join("new.txt"), "keep\n").unwrap();
    run_touch_file(&dir, "new.txt").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("new.txt")).unwrap(),
        "keep\n"
    );
    // Symlinks need privileges on Windows; accept either outcome there.
    match run_symlink(&dir, "new.txt", "link.txt") {
        Ok(()) => {
            assert!(dir.join("link.txt").is_symlink());
            run_symlink(&dir, "other", "link.txt").unwrap();
        }
        #[cfg(windows)]
        Err(error) => {
            assert!(error.contains("Failed to link"), "{error}");
        }
        #[cfg(not(windows))]
        Err(error) => panic!("unexpected symlink failure: {error}"),
    }
}

#[test]
fn run_patch_file_fails_without_binary() {
    let dir = temp_dir("no_patch_bin");
    let error = run_patch_file_with(&dir, "text\n", "arc-rs-no-such-patch-binary").unwrap_err();
    assert!(error.starts_with("failed to run patch"), "{error}");
}

#[test]
fn ensure_base_present_checks_local() {
    let repo = make_repo("ensure_base");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let mut io = Io::buffered();
    let local = Bundle {
        base_revision: Some(head),
        ..Bundle::default()
    };
    assert!(ensure_base_present(&repo, Vcs::Git, &local, &mut io).unwrap());
    assert!(io.out.stdout.is_empty());
    let mut io = Io::buffered();
    assert!(!ensure_base_present(&repo, Vcs::Git, &Bundle::default(), &mut io).unwrap());
    assert!(
        io.out
            .stdout
            .contains("Base commit is not in local repository"),
        "{}",
        io.out.stdout
    );
    let unknown = Bundle {
        base_revision: Some("deadbeef".to_string()),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    assert!(!ensure_base_present(&repo, Vcs::Git, &unknown, &mut io).unwrap());
}

#[test]
fn maybe_create_branch_git_and_svn() {
    let repo = make_repo("maybe_branch");
    let backends = Backends::default();
    let bundle = Bundle {
        revision_id: Some(4),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    let state = maybe_create_branch(
        &repo,
        Vcs::Git,
        &backends,
        &bundle,
        &Options::default(),
        &mut io,
        true,
    )
    .unwrap();
    assert_eq!(state.original.as_deref(), Some("master"));
    assert_eq!(state.new.as_deref(), Some("arcpatch-D4"));
    let plain = temp_dir("maybe_svn");
    let mut io = Io::buffered();
    assert_eq!(
        maybe_create_branch(
            &plain,
            Vcs::Svn,
            &backends,
            &bundle,
            &Options::default(),
            &mut io,
            true
        )
        .unwrap(),
        BranchState::default()
    );
    let options = Options {
        nobranch: true,
        nocommit: true,
        ..Options::default()
    };
    let mut io = Io::buffered();
    assert_eq!(
        maybe_create_branch(&repo, Vcs::Git, &backends, &bundle, &options, &mut io, true).unwrap(),
        BranchState::default()
    );
}

#[test]
fn sanity_check_paths() {
    let repo = make_repo("sanity");
    let backends = Backends::default();
    let mut io = Io::buffered();
    assert!(
        sanity_check(
            &repo,
            Vcs::Svn,
            &backends,
            &mut io,
            None,
            &Bundle::default(),
            false
        )
        .is_ok()
    );
    assert!(
        sanity_check(
            &repo,
            Vcs::Git,
            &backends,
            &mut io,
            None,
            &Bundle::default(),
            false
        )
        .is_ok()
    );
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let local = Bundle {
        base_revision: Some(head),
        ..Bundle::default()
    };
    assert!(sanity_check(&repo, Vcs::Git, &backends, &mut io, None, &local, true).is_ok());
    let remote = Bundle {
        base_revision: Some("deadbeef".to_string()),
        ..Bundle::default()
    };
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        sanity_check(&repo, Vcs::Git, &backends, &mut io, None, &remote, false).unwrap_err(),
        land::ABORT
    );
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY.to_string(),
        serde_json::json!([{"id": 3, "status": STATUS_CLOSED, "dateModified": 1}]),
    )]));
    let mut io = Io::canned(&["y"]);
    sanity_check(
        &repo,
        Vcs::Git,
        &backends,
        &mut io,
        Some(&call),
        &remote,
        false,
    )
    .unwrap();
    assert!(
        io.out.stdout.contains("against commit deadbeef \\ D3"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn commit_template_is_verbatim() {
    // PATCH-P12: the ceb template text (`:900-909`), not the invented one.
    assert_eq!(
        COMMIT_TEMPLATE,
        "\n\n# Enter a commit message for this patch. If you just want to apply the patch to the \
         working copy without committing, re-run arc patch with the --nocommit flag.\n"
    );
    assert_eq!(patch_commit_template(None), COMMIT_TEMPLATE);
    assert!(patch_commit_template(Some("494")).contains("for revision D494."));
}

#[test]
fn gap2_preview_passes_patch_bytes_through_verbatim() {
    // Neither PHP (`ArcanistPatchWorkflow` has no `json_encode`) nor
    // this port emits JSON on the patch path, so slashes, quotes, and
    // non-ASCII bytes must never gain `\/` / `\uXXXX` escapes.
    let dir = temp_dir("preview_verbatim");
    let body = "diff --git a/u/ñ.txt b/u/ñ.txt\n--- \"a/u/ñ.txt\"\n+++ \"b/u/ñ.txt\"\n@@ -1 +1 @@\n-https://phab.test/D1\n+\"quoted\" \\ backslash\n";
    std::fs::write(dir.join("fix.patch"), body).unwrap();
    let patch = dir.join("fix.patch").to_string_lossy().to_string();
    let backends = Backends::default();
    let mut io = Io::buffered();
    let out = execute_with_io(
        &dir,
        &argv(&["--preview", "--patch", &patch]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert_eq!(out.stdout, body);
    assert!(!out.stdout.contains("\\/"), "{}", out.stdout);
}

#[test]
fn execute_help_preview_and_errors() {
    let dir = temp_dir("exec_help");
    let backends = Backends::default();
    let mut io = Io::buffered();
    let out = execute_with_io(&dir, &argv(&["--help"]), &mut io, &backends).unwrap();
    assert_eq!(out.stdout, USAGE);
    let mut io = Io::buffered();
    let error = execute_with_io(&dir, &argv(&["--bogus"]), &mut io, &backends).unwrap_err();
    assert!(error.ends_with(USAGE), "{error}");
    // `--preview` needs no repository for file sources.
    std::fs::write(dir.join("fix.patch"), "diff --git a/f b/f\n").unwrap();
    let patch = dir.join("fix.patch").to_string_lossy().to_string();
    let mut io = Io::buffered();
    let out = execute_with_io(
        &dir,
        &argv(&["--preview", "--patch", &patch]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert_eq!(out.stdout, "diff --git a/f b/f\n");
    let out = execute_with_stdin(
        &dir,
        &argv(&["--preview", "--patch", "-"]),
        Some("diff --git a/g b/g\n"),
    )
    .unwrap();
    assert_eq!(out.stdout, "diff --git a/g b/g\n");
    // Real flows need a working copy and a source.
    let mut io = Io::buffered();
    assert_eq!(
        execute_with_io(&dir, &argv(&["--patch", &patch]), &mut io, &backends).unwrap_err(),
        NOT_A_WORKING_COPY
    );
    let repo = make_repo("exec_source");
    let mut io = Io::buffered();
    let error = execute_with_io(&repo, &argv(&[]), &mut io, &backends).unwrap_err();
    assert!(error.contains("You must specify changes"), "{error}");
}

#[test]
fn execute_git_patch_flow() {
    let repo = make_repo("exec_flow");
    let patch = repo.join("fix.patch");
    std::fs::write(&patch, &change_bundle().text).unwrap();
    let patch = patch.to_string_lossy().to_string();
    let backends = Backends::default();
    let mut io = Io::canned(&["y"]);
    let out = execute_with_io(
        &repo,
        &argv(&["--patch", &patch, "--nocommit", "--nobranch"]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("file.txt")).unwrap(),
        "patched\n"
    );
    assert!(out.stderr.contains("** APPLIED **"), "{}", out.stderr);
    assert_eq!(git(&repo, &["branch", "--show-current"]), "master");
    // Default flags branch: a fresh repo stays clean (patch file ignored here).
    let repo = make_repo("exec_branch");
    let patch = repo.join("fix.patch");
    std::fs::write(&patch, &change_bundle().text).unwrap();
    let patch = patch.to_string_lossy().to_string();
    let mut io = Io::canned(&["y"]);
    execute_with_io(
        &repo,
        &argv(&["--patch", &patch, "--nocommit", "--force"]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert_eq!(git(&repo, &["branch", "--show-current"]), "arcpatch");
}

#[test]
fn execute_patch_nocommit_applies_without_editor() {
    // PATCH-P8: committing without a message now opens the editor (covered
    // at the resolver level); `--nocommit` still applies cleanly.
    let repo = make_repo("exec_commit");
    let patch = repo.join("fix.patch");
    std::fs::write(&patch, &change_bundle().text).unwrap();
    let patch = patch.to_string_lossy().to_string();
    let backends = Backends::default();
    let mut io = Io::canned(&["y"]);
    let out = execute_with_io(
        &repo,
        &argv(&["--patch", &patch, "--nobranch", "--force", "--nocommit"]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert!(out.stderr.contains("APPLIED"), "{}", out.stderr);
}

#[test]
fn execute_pops_stash_on_failure() {
    let repo = make_repo("exec_stash_fail");
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.autostash": true}"#).unwrap();
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let patch = repo.join("fix.patch");
    std::fs::write(
        &patch,
        "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    let patch = patch.to_string_lossy().to_string();
    let backends = Backends::default();
    let mut io = Io::canned(&["y", "n"]);
    let error = execute_with_io(
        &repo,
        &argv(&["--patch", &patch, "--nocommit", "--nobranch"]),
        &mut io,
        &backends,
    )
    .unwrap_err();
    assert_eq!(error, "Unable to apply patch!");
    assert_eq!(
        std::fs::read_to_string(repo.join("file.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "dirty\n"
    );
    assert!(git(&repo, &["stash", "list"]).is_empty());
}

#[test]
fn run_entry_points() {
    let dir = temp_dir("run_points");
    assert_eq!(run_in(&dir, &argv(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(execute(&dir, &argv(&["--help"])).unwrap().stdout, USAGE);
    assert_eq!(run(&argv(&["--help"])), ExitCode::SUCCESS);
}

#[test]
fn gap2_sha1_vectors() {
    assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        git_blob_sha1(b""),
        "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
    );
    assert_eq!(
        git_blob_sha1(b"test\n"),
        "9daeafb9864cf43055ae93beb0afd6c7d144bfa4"
    );
}

#[test]
fn gap2_adler_and_zlib_store() {
    assert_eq!(adler32(b""), 1);
    assert_eq!(adler32(b"a"), 0x00620062);
    assert_eq!(adler32(b"abc"), 0x024D0127);
    assert_eq!(
        zlib_store(b""),
        vec![
            0x78, 0x01, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01
        ]
    );
    let stored = zlib_store(b"hi");
    assert_eq!(&stored[0..2], &[0x78, 0x01]);
    assert_eq!(&stored[stored.len() - 4..], &adler32(b"hi").to_be_bytes());
}

#[test]
fn gap2_base85_shapes() {
    assert_eq!(base85_encode(b""), "@\n");
    let one = base85_encode(b"a");
    assert!(one.starts_with('A'), "{one:?}");
    assert!(one.ends_with('\n'), "{one:?}");
    assert_eq!(one.len(), 1 + 5 + 1);
    for byte in one.bytes() {
        assert!(
            byte.is_ascii() && !byte.is_ascii_control() || byte == b'\n',
            "{one:?}"
        );
    }
    let long = base85_encode(&[7u8; 100]);
    assert_eq!(long.lines().count(), 2);
    assert!(long.starts_with('z'), "{long:?}");
}

#[test]
fn gap2_base64_roundtrip() {
    assert_eq!(base64_decode("dGVzdA==").unwrap(), b"test");
    assert_eq!(base64_decode("TQ==").unwrap(), b"M");
    assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    assert_eq!(base64_decode("dGVz\ndA==").unwrap(), b"test");
    assert!(base64_decode("abc").is_err());
    assert!(base64_decode("====").is_err());
    assert!(base64_decode("ab=c").is_err());
}

#[test]
fn gap2_build_binary_add() {
    let change = BundleChange {
        file_type: FILE_BINARY,
        current_file_data: Some(b"hi".to_vec()),
        ..bundle_change(TYPE_ADD, None, Some("a.bin"))
    };
    let fail = |_: &str| -> Result<Vec<u8>, String> { panic!("no blobs needed") };
    let body = build_binary_change(&change, None, &fail).unwrap();
    assert!(body.contains("GIT binary patch\n"), "{body}");
    assert!(body.contains("literal 2\n"), "{body}");
    assert!(body.contains("literal 0\n"), "{body}");
    assert!(body.contains(&"0".repeat(40)), "{body}");
    assert!(body.contains(&git_blob_sha1(b"hi")), "{body}");
}

#[test]
fn gap2_render_with_blobs_matches_text() {
    let mut text_changes = vec![BundleChange {
        hunks: vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\n")],
        ..bundle_change(TYPE_CHANGE, Some("a.txt"), Some("a.txt"))
    }];
    let expected = render_git_patch(&mut text_changes.clone()).unwrap();
    let fail = |_: &str| -> Result<Vec<u8>, String> { panic!("no blobs") };
    let actual = render_git_patch_with_blobs(&mut text_changes, &fail).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn gap2_render_binary_image() {
    let mut changes = vec![
        BundleChange {
            file_type: FILE_BINARY,
            current_file_data: Some(vec![0, 1, 2, 3]),
            ..bundle_change(TYPE_ADD, None, Some("a.bin"))
        },
        BundleChange {
            file_type: FILE_IMAGE,
            current_file_data: Some(vec![9, 9]),
            original_file_data: Some(vec![8]),
            ..bundle_change(TYPE_CHANGE, Some("b.png"), Some("b.png"))
        },
    ];
    let fail = |_: &str| -> Result<Vec<u8>, String> { panic!("inline data") };
    let text = render_git_patch_with_blobs(&mut changes, &fail).unwrap();
    assert!(text.contains("diff --git a/a.bin b/a.bin\n"), "{text}");
    assert!(text.contains("new file mode 100644\n"), "{text}");
    assert!(!text.contains("--- /dev/null"), "{text}");
    assert_eq!(text.matches("GIT binary patch\n").count(), 2);
}

#[test]
fn gap2_fetch_file_blob_decodes() {
    let call = stub_call(std::collections::HashMap::from([(
        "file.download".to_string(),
        serde_json::json!("aGk="),
    )]));
    assert_eq!(fetch_file_blob(&call, "PHID-FILE-1").unwrap(), b"hi");
    let bad = stub_call(std::collections::HashMap::from([(
        "file.download".to_string(),
        serde_json::json!({"nope": true}),
    )]));
    assert_eq!(
        fetch_file_blob(&bad, "PHID-FILE-1").unwrap_err(),
        "file.download returned an unexpected response"
    );
}

#[test]
fn gap2_arcbundle_binary_roundtrip() {
    let dir = temp_dir("arcbundle_bin");
    let new_bytes: &[u8] = &[0, 1, 2, 3, 250, 251];
    let changes = [
        br#"[{"type":1,"fileType":3,"currentPath":"a.bin","metadata":"#.as_slice(),
        br#"{"new:binary-phid":"PHID-FILE-NEW"},"hunks":[]}]"#.as_slice(),
    ]
    .concat();
    let bundle_path = write_arcbundle(
        &dir,
        "bin.bundle",
        &[
            ("meta.json", br#"{"revisionID": 11}"#),
            ("changes.json", &changes),
            ("blobs/PHID-FILE-NEW", new_bytes),
        ],
    );
    let bundle = load_arcbundle(&bundle_path.to_string_lossy(), None).unwrap();
    assert_eq!(bundle.revision_id, Some(11));
    assert!(
        bundle.text.contains("GIT binary patch\n"),
        "{}",
        bundle.text
    );
    assert!(bundle.text.contains("literal 6\n"), "{}", bundle.text);
    assert!(
        bundle.text.contains(&git_blob_sha1(new_bytes)),
        "{}",
        bundle.text
    );
}

#[test]
fn gap2_binary_patch_applies_via_git() {
    let repo = make_repo("bin_apply");
    let old_bytes: &[u8] = &[0, 1, 2, 3, 4, 5, 250, 251, 252];
    let new_bytes: &[u8] = &[0, 1, 2, 3, 9, 9, 9, 250, 251, 252, 10, 11];
    std::fs::write(repo.join("a.bin"), old_bytes).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "bin"]);
    let mut changes = vec![BundleChange {
        file_type: FILE_BINARY,
        original_file_data: Some(old_bytes.to_vec()),
        current_file_data: Some(new_bytes.to_vec()),
        ..bundle_change(TYPE_CHANGE, Some("a.bin"), Some("a.bin"))
    }];
    let fail = |_: &str| -> Result<Vec<u8>, String> { panic!("inline") };
    let text = render_git_patch_with_blobs(&mut changes, &fail).unwrap();
    let bundle = Bundle {
        text,
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    git_apply_patch(&repo, &bundle, &mut io).unwrap();
    assert_eq!(std::fs::read(repo.join("a.bin")).unwrap(), new_bytes);
}

#[test]
fn gap2_validate_empty_and_headers() {
    assert_eq!(validate_patch_text("").unwrap_err(), EMPTY_DIFF_ERROR);
    assert_eq!(
        validate_patch_text("  \n\t\n").unwrap_err(),
        EMPTY_DIFF_ERROR
    );
    assert_eq!(validate_patch_text("diff --git a/f b/f\n"), Ok(()));
    for valid in [
        "Index: /path/to/file.ext\n",
        "Modified: foo\n",
        "Property changes on: foo\n",
        "commit 59bcc3ad6775562f845953cf01624225\n",
        "commit abc123 (tag)\n",
        "diff --git a/f b/f\n",
        "rcsdiff -u a b\n",
        "--- a/old.txt\t2024-01-02 03:04:05\n",
        "Binary files a 2024-01-02 and b 2024-01-02 differ\n",
        "diff -r abc123 file.txt\n",
        "diff -r abc123 -r def456 file.txt\n",
        "# comment\ndiff --git a/f b/f\n",
        "\n\n diff --git a/f b/f\n",
    ] {
        assert!(validate_patch_text(valid).is_ok(), "{valid:?}");
    }
    for invalid in [
        "commit ABC123\n",
        "commit \n",
        "diff -r XYZ file\n",
        "--- no-date-here\n",
        "Binary files a and b differ\n",
    ] {
        let err = validate_patch_text(invalid).unwrap_err();
        assert!(
            err.starts_with("Diff Parse Exception: Expected a hunk header"),
            "{err}"
        );
    }
}

#[test]
fn gap2_malformed_header_error_is_verbatim() {
    let err = validate_patch_text("not a patch\n").unwrap_err();
    let message = "Expected a hunk header, like 'Index: /path/to/file.ext' (svn), \
         'Property changes on: /path/to/file.ext' (svn properties), \
         'commit 59bcc3ad6775562f845953cf01624225' (git show), 'diff --git' \
         (git diff), '--- filename' (unified diff), or 'diff -r' (hg diff or patch).";
    let full = format!("Diff Parse Exception: {message}\n\n   >>>        1   not a patch\n");
    assert_eq!(err, full);
}

#[test]
fn gap2_load_file_bundle_validates() {
    let dir = temp_dir("validate_load");
    std::fs::write(dir.join("empty.patch"), "").unwrap();
    std::fs::write(dir.join("bad.patch"), "hello\n").unwrap();
    std::fs::write(dir.join("good.patch"), "diff --git a/f b/f\n").unwrap();
    let mut io = Io::buffered();
    assert_eq!(
        load_file_bundle(&dir.join("empty.patch").to_string_lossy(), None, &mut io).unwrap_err(),
        EMPTY_DIFF_ERROR
    );
    let err =
        load_file_bundle(&dir.join("bad.patch").to_string_lossy(), None, &mut io).unwrap_err();
    assert!(err.starts_with("Diff Parse Exception: "), "{err}");
    let bundle =
        load_file_bundle(&dir.join("good.patch").to_string_lossy(), None, &mut io).unwrap();
    assert_eq!(bundle.text, "diff --git a/f b/f\n");
}

#[test]
fn gap2_ensure_base_present_with_uses_fake_hg() {
    let dir = temp_dir("base_with_fake");
    let hg = fake_hg_backend(&dir);
    let backends = Backends {
        hg,
        svn: svn::SvnCommand::system(),
    };
    let bundle = Bundle {
        base_revision: Some("abc123".to_string()),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    assert!(ensure_base_present_with(&dir, Vcs::Hg, &bundle, &mut io, &backends).unwrap());
    let log = std::fs::read_to_string(dir.join(".fakehg.log")).unwrap();
    assert!(log.contains("log -r abc123"), "{log}");
    std::fs::write(dir.join(".fakehg-fail-log"), "").unwrap();
    let mut io = Io::buffered();
    assert!(!ensure_base_present_with(&dir, Vcs::Hg, &bundle, &mut io, &backends).unwrap());
}

#[test]
fn gap2_plan_children_with_uses_fake_hg() {
    let dir = temp_dir("plan_with_fake");
    let hg = fake_hg_backend(&dir);
    let backends = Backends {
        hg,
        svn: svn::SvnCommand::system(),
    };
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let options = Options::default();
    let mut io = Io::buffered();
    let start = serde_json::json!([{
        "phid": "PHID-DREV-1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]}
    }]);
    let dep = serde_json::json!([{
        "phid": "PHID-DREV-2", "id": 2, "hashes": {"hgcm": "nope-missing"},
        "auxiliary": {}
    }]);
    let both = serde_json::json!([
        {"phid": "PHID-DREV-1", "id": 1},
        {"phid": "PHID-DREV-2", "id": 2}
    ]);
    let graph = |_: &str, params: &serde_json::Value| {
        let text = params.to_string();
        if text.contains("\"ids\"") {
            Ok(start.clone())
        } else if text.contains("PHID-DREV-1") {
            Ok(both.clone())
        } else {
            Ok(dep.clone())
        }
    };
    let children =
        plan_dependency_children_with(&dir, Vcs::Hg, &bundle, &options, &mut io, &graph, &backends)
            .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].revision.as_deref(), Some("D2"));
    let log = std::fs::read_to_string(dir.join(".fakehg.log")).unwrap();
    assert!(log.contains("nope-missing"), "{log}");
}

#[test]
fn ansi_status_lines_gain_bold_labels_and_backgrounds() {
    assert_eq!(
        status_line_ansi("green", "OKAY", "Successfully applied patch.", false),
        "** OKAY ** Successfully applied patch.\n"
    );
    assert_eq!(
        status_line_ansi("green", "OKAY", "Successfully applied patch.", true),
        "\x1b[42m\x1b[1m OKAY \x1b[m\x1b[49m Successfully applied patch.\n"
    );
    assert_eq!(
        status_line_ansi(
            "blue",
            "INFO",
            "Base commit is not in local repository; trying to fetch.",
            false
        ),
        "** INFO ** Base commit is not in local repository; trying to fetch.\n"
    );
    assert_eq!(
        status_line_ansi(
            "blue",
            "INFO",
            "Base commit is not in local repository; trying to fetch.",
            true
        ),
        "\x1b[44m\x1b[1m INFO \x1b[m\x1b[49m Base commit is not in local repository; trying to fetch.\n"
    );
    assert_eq!(
        status_line_ansi("green", "COMMITTED", "Successfully committed patch.", true),
        "\x1b[42m\x1b[1m COMMITTED \x1b[m\x1b[49m Successfully committed patch.\n"
    );
    assert_eq!(
        status_line_ansi("green", "APPLIED", "Successfully applied patch.", true),
        "\x1b[42m\x1b[1m APPLIED \x1b[m\x1b[49m Successfully applied patch.\n"
    );
}

#[test]
fn ansi_failure_banners_gain_red_backgrounds() {
    assert_eq!(
        cherry_pick_failed_banner_ansi(false),
        "\n** Cherry Pick Failed!**\n"
    );
    assert_eq!(
        cherry_pick_failed_banner_ansi(true),
        "\n\x1b[41m\x1b[1m Cherry Pick Failed!\x1b[m\x1b[49m\n"
    );
    assert_eq!(patch_failed_banner_ansi(false), "\n** Patch Failed! **\n");
    assert_eq!(
        patch_failed_banner_ansi(true),
        "\n\x1b[41m\x1b[1m Patch Failed! \x1b[m\x1b[49m\n"
    );
    assert_eq!(
        rebase_failed_banner_ansi("main", false),
        "\n** Rebase onto main failed!**\n"
    );
    assert_eq!(
        rebase_failed_banner_ansi("main", true),
        "\n\x1b[41m\x1b[1m Rebase onto main failed!\x1b[m\x1b[49m\n"
    );
}

#[test]
fn ansi_warning_blocks_gain_yellow_backgrounds() {
    let plain = hg_case_warning_ansi(false);
    assert!(
        plain.starts_with("\n** WARNING ** This patch may have failed"),
        "{plain:?}"
    );
    assert!(!plain.contains('\x1b'), "{plain:?}");
    let colored = hg_case_warning_ansi(true);
    assert!(
        colored.starts_with("\n\x1b[43m\x1b[1m WARNING \x1b[m\x1b[49m This patch may have failed"),
        "{colored:?}"
    );
    let plain = svn_hunks_warning_ansi(false);
    assert!(
        plain.starts_with("\n\n** WARNING ** Some hunks could not be applied"),
        "{plain:?}"
    );
    assert!(!plain.contains('\x1b'), "{plain:?}");
    let colored = svn_hunks_warning_ansi(true);
    assert!(
        colored.starts_with(
            "\n\n\x1b[43m\x1b[1m WARNING \x1b[m\x1b[49m Some hunks could not be applied"
        ),
        "{colored:?}"
    );
}

#[test]
fn ansi_apply_svn_okay_uses_green_background() {
    let dir = temp_dir("svn_ansi_okay");
    for ansi in [false, true] {
        let mut io = Io::buffered();
        let outcome = apply_svn_ansi(
            &dir,
            &svn::SvnCommand::system(),
            &[],
            &mut io,
            &mut |_, _| Ok((true, String::new())),
            &mut |_| Ok(()),
            &mut |_, _| Ok(()),
            ansi,
        )
        .unwrap();
        assert_eq!(outcome, SvnOutcome::Applied);
        if ansi {
            assert!(
                io.out
                    .stdout
                    .contains("\x1b[42m\x1b[1m OKAY \x1b[m\x1b[49m"),
                "{}",
                io.out.stdout
            );
        } else {
            assert!(
                io.out
                    .stdout
                    .contains("** OKAY ** Successfully applied patch"),
                "{}",
                io.out.stdout
            );
            assert!(!io.out.stdout.contains('\x1b'), "{}", io.out.stdout);
        }
    }
}

#[test]
fn ansi_git_apply_failure_banner_modes() {
    for ansi in [false, true] {
        let repo = make_repo(&format!("git_apply_ansi_{ansi}"));
        let mut io = Io::buffered();
        let bad = Bundle {
            text: "not a patch\n".to_string(),
            ..Bundle::default()
        };
        assert_eq!(
            git_apply_patch_ansi(&repo, &bad, &mut io, ansi).unwrap_err(),
            "Unable to apply patch!"
        );
        if ansi {
            assert!(
                io.out
                    .stdout
                    .contains("\n\x1b[41m\x1b[1m Patch Failed! \x1b[m\x1b[49m\n"),
                "{}",
                io.out.stdout
            );
        } else {
            assert!(
                io.out.stdout.contains("\n** Patch Failed! **\n"),
                "{}",
                io.out.stdout
            );
            assert!(!io.out.stdout.contains('\x1b'), "{}", io.out.stdout);
        }
    }
}

#[test]
fn ansi_clean_tree_desc_underlines_path() {
    assert_eq!(
        working_copy_desc_ansi(Path::new("/repo"), false),
        "  Working copy: /repo\n\n"
    );
    assert_eq!(
        working_copy_desc_ansi(Path::new("/repo"), true),
        "  Working copy: \x1b[4m/repo\x1b[m\n\n"
    );
    for ansi in [false, true] {
        let repo = make_repo(&format!("clean_ansi_{ansi}"));
        std::fs::write(repo.join("new.txt"), "new\n").unwrap();
        let backends = Backends::default();
        let mut io = Io::canned(&["y"]);
        require_clean_tree_ansi(&repo, Vcs::Git, &backends, &mut io, false, ansi).unwrap();
        let path = repo.display().to_string();
        if ansi {
            assert!(
                io.out
                    .stdout
                    .contains(&format!("  Working copy: \x1b[4m{path}\x1b[m\n\n")),
                "{}",
                io.out.stdout
            );
        } else {
            assert!(
                io.out
                    .stdout
                    .contains(&format!("  Working copy: {path}\n\n")),
                "{}",
                io.out.stdout
            );
            assert!(!io.out.stdout.contains('\x1b'), "{}", io.out.stdout);
        }
    }
}

#[test]
fn dependency_nodes_accept_string_ids() {
    let node = parse_dependency_node(&serde_json::json!({
        "phid": "PHID-DREV-1",
        "id": "494",
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]},
    }))
    .unwrap();
    assert_eq!(node.id, 494);
    assert_eq!(node.depends_on, vec!["PHID-DREV-2".to_string()]);
    let node = parse_dependency_node(&serde_json::json!({
        "phid": "PHID-DREV-1",
        "id": 494,
    }))
    .unwrap();
    assert_eq!(node.id, 494);
}

#[test]
fn diff_rows_accept_string_ids() {
    let rows = parse_diff_rows(&serde_json::json!({
        "1633": {"id": "1633", "revisionID": "494", "authorName": "a", "authorEmail": "e"},
    }));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, 1633);
    assert_eq!(rows[0].revision_id, Some(494));
}

#[test]
fn revision_suffix_matches_closed_string_rows() {
    let call = |_: &str, _: &serde_json::Value| {
        Ok(serde_json::json!([
            {"id": "491", "status": "3", "dateModified": "100"},
            {"id": "492", "status": "1", "dateModified": "200"},
        ]))
    };
    assert_eq!(
        revision_suffix_for_hash(&call, "gtcm", "abc"),
        Some(" \\ D491".to_string())
    );
}

#[test]
fn render_hunks_rebuilds_headers_from_offsets() {
    // Length 1 omits `,len` like git; missing trailing newline is added.
    assert_eq!(
        render_hunks(&[parsed_hunk(1, 1, 1, 1, "-a\n+b")]),
        "@@ -1 +1 @@\n-a\n+b\n"
    );
    assert_eq!(
        render_hunks(&[parsed_hunk(0, 0, 1, 3, "+a\n+b\n+c\n")]),
        "@@ -0,0 +1,3 @@\n+a\n+b\n+c\n"
    );
    assert_eq!(render_hunks(&[]), "");
}

#[test]
fn conduit_hunks_coerce_string_offsets() {
    let hunks = conduit_hunks_from_dict(&serde_json::json!({
        "hunks": [
            {"oldOffset": "1", "oldLength": "1", "newOffset": "1",
             "newLength": "1", "corpus": "-a\n+b\n"},
            {"corpus": "+x\n"},
        ],
    }));
    assert_eq!(
        hunks,
        vec![
            parsed_hunk(1, 1, 1, 1, "-a\n+b\n"),
            parsed_hunk(0, 0, 0, 0, "+x\n"),
        ]
    );
    assert!(conduit_hunks_from_dict(&serde_json::json!({})).is_empty());
}

#[test]
fn break_hunk_splits_seven_line_gaps_like_diff_u() {
    // PHI838: 7 unchanged lines between changes split into two hunks.
    let mut body = vec!["-old".to_string(), "+new".to_string()];
    for i in 0..7 {
        body.push(format!(" ctx{i}"));
    }
    body.push("-gone".to_string());
    body.push("+here".to_string());
    let corpus = body.join("\n") + "\n";
    let small = break_hunk_into_small_hunks(&parsed_hunk(10, 9, 10, 9, &corpus));
    assert_eq!(small.len(), 2);
    assert_eq!(
        (
            small[0].old_start,
            small[0].old_len,
            small[0].new_start,
            small[0].new_len
        ),
        (10, 4, 10, 4)
    );
    assert_eq!(
        (
            small[1].old_start,
            small[1].old_len,
            small[1].new_start,
            small[1].new_len
        ),
        (15, 4, 15, 4)
    );
    assert!(small[0].corpus.ends_with(" ctx2\n"));
    assert!(small[1].corpus.starts_with(" ctx4\n"));
}

#[test]
fn break_hunk_keeps_small_changes_whole() {
    // Lengths recompute from the corpus; input lengths are ignored.
    let small = break_hunk_into_small_hunks(&parsed_hunk(1, 99, 1, 99, " a\n-b\n+b\n c\n"));
    assert_eq!(small.len(), 1);
    assert_eq!(
        (
            small[0].old_start,
            small[0].old_len,
            small[0].new_start,
            small[0].new_len
        ),
        (1, 3, 1, 3)
    );
    assert_eq!(small[0].corpus, " a\n-b\n+b\n c\n");
}

#[test]
fn break_hunk_matches_server_split_live_vector() {
    // D494 `.arclint`: one stored hunk (1,86,1,109) renders as the same
    // two headers orsay serves.
    let corpus = include_str!("../tests/fixtures/patch/arclint-live-corpus.txt");
    let small = break_hunk_into_small_hunks(&parsed_hunk(1, 86, 1, 109, corpus));
    assert_eq!(small.len(), 2);
    assert_eq!(
        (
            small[0].old_start,
            small[0].old_len,
            small[0].new_start,
            small[0].new_len
        ),
        (8, 6, 8, 13)
    );
    assert_eq!(
        (
            small[1].old_start,
            small[1].old_len,
            small[1].new_start,
            small[1].new_len
        ),
        (58, 15, 65, 31)
    );
    let rendered = render_hunks(&[parsed_hunk(1, 86, 1, 109, corpus)]);
    assert!(rendered.contains("@@ -8,6 +8,13 @@\n"), "{rendered:.200}");
    assert!(
        rendered.contains("@@ -58,15 +65,31 @@\n"),
        "{rendered:.200}"
    );
}

/* T1153 PATCH-P findings: string-id monograms, raw id fetches. */

#[test]
fn p6_dependency_monograms_accept_string_ids() {
    // Orsay types `differential.query` ids as strings; the whole plan
    // must still yield children.
    let repo = make_repo("p6_string_monograms");
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let options = Options::default();
    let mut io = Io::buffered();
    let start = serde_json::json!([{
        "phid": "PHID-DREV-1", "id": "1",
        "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]}
    }]);
    let dep = serde_json::json!([{
        "phid": "PHID-DREV-2", "id": "2", "auxiliary": {}
    }]);
    let both = serde_json::json!([
        {"phid": "PHID-DREV-1", "id": "1"},
        {"phid": "PHID-DREV-2", "id": "2"}
    ]);
    let graph = |_: &str, params: &serde_json::Value| {
        let text = params.to_string();
        if text.contains("\"ids\"") {
            Ok(start.clone())
        } else if text.contains("PHID-DREV-1") {
            Ok(both.clone())
        } else {
            Ok(dep.clone())
        }
    };
    let children =
        plan_dependency_children(&repo, Vcs::Git, &bundle, &options, &mut io, &graph).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].revision.as_deref(), Some("D2"));
}

#[test]
fn p8_editor_fallback_commits_without_revision() {
    let bundle = Bundle::default();
    let edit = |template: &str| -> Result<String, String> {
        assert_eq!(template, COMMIT_TEMPLATE);
        Ok("my patch\n# comment\n".to_string())
    };
    assert_eq!(
        resolve_commit_message_with_editor(&bundle, None, Some(&edit)).unwrap(),
        "my patch\n"
    );
    // A blank edit aborts.
    let blank = |_: &str| -> Result<String, String> { Ok("# only\n".to_string()) };
    assert_eq!(
        resolve_commit_message_with_editor(&bundle, None, Some(&blank)).unwrap_err(),
        diff_local::USER_ABORT
    );
    // Without an editor the guidance error survives.
    assert_eq!(
        resolve_commit_message_with_editor(&bundle, None, None).unwrap_err(),
        NO_COMMIT_MESSAGE
    );
}

#[test]
fn p8_empty_fetch_opens_noted_template() {
    let call = stub_call(std::collections::HashMap::from([(
        diff_submit::METHOD_GET_COMMIT_MESSAGE.to_string(),
        serde_json::json!(""),
    )]));
    let bundle = Bundle {
        revision_id: Some(494),
        ..Bundle::default()
    };
    let edit = |template: &str| -> Result<String, String> {
        assert!(template.contains("for revision D494."), "{template}");
        Ok("typed\n".to_string())
    };
    assert_eq!(
        resolve_commit_message_with_editor(&bundle, Some(&call), Some(&edit)).unwrap(),
        "typed\n"
    );
}

#[test]
fn p9_yes_path_commits_via_editor() {
    // HEAD carries a revision footer, so the prompt offers a new commit
    // (the D8 working-copy lookup is unported; revision IDs decline).
    let repo = make_repo("p9_create");
    git(
        &repo,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "x\n\nDifferential Revision: D1",
        ],
    );
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let backends = Backends::default();
    let mut io = Io::canned(&["y"]);
    let status = TreeStatus {
        unstaged: vec!["file.txt".to_string()],
        ..TreeStatus::default()
    };
    let seen = std::cell::RefCell::new(String::new());
    let edit = |template: &str| -> Result<String, String> {
        *seen.borrow_mut() = template.to_string();
        Ok("wip save\n".to_string())
    };
    let outcome = resolve_uncommitted_tree(
        &repo,
        Vcs::Git,
        &backends,
        &mut io,
        &status,
        "desc",
        None,
        &edit,
    )
    .unwrap();
    assert_eq!(outcome, StashState::Clean);
    assert!(
        io.out.stdout.contains("create a new commit"),
        "{}",
        io.out.stdout
    );
    let template = seen.borrow().clone();
    assert!(template.contains("# Enter a commit message."), "{template}");
    assert!(template.contains("#     file.txt"), "{template}");
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]), "wip save");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
}

#[test]
fn p9_yes_path_amends_when_detected() {
    // Plain HEAD by the repo author: amend wording, no editor, same count.
    let repo = make_repo("p9_amend");
    std::fs::write(repo.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let backends = Backends::default();
    let mut io = Io::canned(&["y"]);
    let status = TreeStatus {
        unstaged: vec!["file.txt".to_string()],
        ..TreeStatus::default()
    };
    let edit = |_: &str| -> Result<String, String> {
        panic!("PATCH-P9: amend path never opens the editor")
    };
    resolve_uncommitted_tree(
        &repo,
        Vcs::Git,
        &backends,
        &mut io,
        &status,
        "desc",
        None,
        &edit,
    )
    .unwrap();
    assert!(
        io.out.stdout.contains("amend these 1 change(s)"),
        "{}",
        io.out.stdout
    );
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&repo, &["status", "--porcelain", "--", "file.txt"]), "");
}

#[test]
fn p9_unchanged_and_empty_templates_error() {
    let repo = make_repo("p9_template");
    git(
        &repo,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "x\n\nDifferential Revision: D1",
        ],
    );
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let backends = Backends::default();
    let status = TreeStatus {
        unstaged: vec!["file.txt".to_string()],
        ..TreeStatus::default()
    };
    let mut io = Io::canned(&["y"]);
    let echo = |template: &str| -> Result<String, String> { Ok(template.to_string()) };
    assert_eq!(
        resolve_uncommitted_tree(
            &repo,
            Vcs::Git,
            &backends,
            &mut io,
            &status,
            "desc",
            None,
            &echo,
        )
        .unwrap_err(),
        MUST_PROVIDE_MESSAGE
    );
    // An all-comments edit strips to `"\n"` (never `""`, like PHP), so
    // the nonempty check cannot trip — `git commit` itself refuses.
    let mut io = Io::canned(&["y"]);
    let empty = |_: &str| -> Result<String, String> { Ok("# only\n".to_string()) };
    let error = resolve_uncommitted_tree(
        &repo,
        Vcs::Git,
        &backends,
        &mut io,
        &status,
        "desc",
        None,
        &empty,
    )
    .unwrap_err();
    assert!(error.contains("empty commit message"), "{error}");
    assert_eq!(
        MUST_PROVIDE_NONEMPTY_MESSAGE,
        "You must provide a nonempty commit message."
    );
}

#[test]
fn p9_published_head_declines_amend() {
    // A `diffusion.querycommits` hit means published: create wording.
    let repo = make_repo("p9_published");
    std::fs::write(
        repo.join(".arcconfig"),
        "{\"history.immutable\": false, \"repository.callsign\": \"X\"}",
    )
    .unwrap();
    let backends = Backends::default();
    let hash = git(&repo, &["rev-parse", "HEAD"]);
    let call = |method: &str, params: &serde_json::Value| {
        if method == "repository.query" {
            return Ok(serde_json::json!([{"id": "7", "callsign": "X"}]));
        }
        if method == "diffusion.querycommits" {
            assert_eq!(params["names"], serde_json::json!([format!("rX{hash}")]));
            return Ok(serde_json::json!({
                "identifierMap": {format!("rX{hash}"): {"id": 1}}
            }));
        }
        Err(format!("PATCH-P9: unexpected {method}"))
    };
    let (amend, _) = calculate_patch_should_amend(&repo, Vcs::Git, &backends, Some(&call)).unwrap();
    assert!(!amend);
    let (amend, head) = calculate_patch_should_amend(&repo, Vcs::Git, &backends, None).unwrap();
    assert!(amend);
    assert!(head.unwrap().contains("base"));
}

#[test]
fn p10_invalid_session_reauthenticates_and_retries_once() {
    let options = Options::default();
    let mut io = Io::buffered();
    let seen = std::cell::RefCell::new(Vec::new());
    let calls = std::cell::Cell::new(0u32);
    let call = |method: &str, params: &serde_json::Value| {
        seen.borrow_mut()
            .push((method.to_string(), params.to_string()));
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            return Err("ERR-INVALID-SESSION: session expired".to_string());
        }
        Ok(serde_json::json!({
            "12": {"id": "12", "revisionID": "7",
                   "sourceControlBaseRevision": "base",
                   "authorName": "Ada", "authorEmail": "ada@test",
                   "changes": []},
        }))
    };
    let loaded = load_bundle_with_reauth(
        &Source::Revision("7".to_string()),
        &options,
        &mut io,
        Some(&call),
    )
    .unwrap();
    assert_eq!(loaded.bundle.revision_id, Some(7));
    let seen = seen.borrow();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].0, diff_submit::METHOD_QUERY_DIFFS);
    assert_eq!(seen[1].0, "user.whoami");
    assert_eq!(seen[2].0, diff_submit::METHOD_QUERY_DIFFS);
    assert!(is_invalid_session_error("ERR-INVALID-SESSION: x"));
    assert!(is_invalid_session_error("ERR-INVALID-SESSION"));
    assert!(!is_invalid_session_error("ERR-OTHER: x"));
}

#[test]
fn p10_reauth_failure_propagates() {
    let options = Options::default();
    let mut io = Io::buffered();
    let call = |method: &str, _: &serde_json::Value| {
        if method == "user.whoami" {
            return Err("ERR-INVALID-AUTH: bad token".to_string());
        }
        Err("ERR-INVALID-SESSION: expired".to_string())
    };
    // Like PHP, the re-auth error (not the load error) surfaces.
    assert_eq!(
        load_bundle_with_reauth(
            &Source::Revision("7".to_string()),
            &options,
            &mut io,
            Some(&call),
        )
        .unwrap_err(),
        "ERR-INVALID-AUTH: bad token"
    );
}

#[test]
fn p11_repository_encoding_lookup() {
    let repo = make_repo("p11_encoding");
    std::fs::write(repo.join(".arcconfig"), "{\"repository.callsign\": \"X\"}").unwrap();
    let backends = Backends::default();
    let answer = std::cell::RefCell::new(serde_json::json!([{"encoding": "cp1252"}]));
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "repository.query");
        assert!(params.to_string().contains("\"X\""), "{}", params);
        Ok(answer.borrow().clone())
    };
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Git, &backends, &call).unwrap(),
        Some("cp1252".to_string())
    );
    // Missing/empty encodings and ambiguous matches default to UTF-8.
    for rows in [
        serde_json::json!([{}]),
        serde_json::json!([{"encoding": ""}]),
        serde_json::json!([]),
        serde_json::json!([{"encoding": "a"}, {"encoding": "b"}]),
    ] {
        *answer.borrow_mut() = rows;
        assert_eq!(
            patch_repository_encoding(&repo, Vcs::Git, &backends, &call).unwrap(),
            Some("UTF-8".to_string())
        );
    }
}

#[test]
fn p11_repository_encoding_failures() {
    let repo = make_repo("p11_failures");
    std::fs::write(repo.join(".arcconfig"), "{\"repository.callsign\": \"X\"}").unwrap();
    let backends = Backends::default();
    // `ERR-CONDUIT-CALL` (unknown method) still yields UTF-8; other
    // conduit failures yield no override.
    let call = |_: &str, _: &serde_json::Value| {
        Err::<serde_json::Value, String>("ERR-CONDUIT-CALL: nope".to_string())
    };
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Git, &backends, &call).unwrap(),
        Some("UTF-8".to_string())
    );
    let call = |_: &str, _: &serde_json::Value| {
        Err::<serde_json::Value, String>("ERR-OTHER: boom".to_string())
    };
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Git, &backends, &call).unwrap(),
        None
    );
    assert!(is_conduit_call_error("ERR-CONDUIT-CALL: x"));
    assert!(!is_conduit_call_error("ERR-OTHER: x"));
}

#[test]
fn p13_raw_fetches_send_verbatim_string_ids() {
    let seen = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        seen.borrow_mut()
            .push((method.to_string(), params.to_string()));
        Ok(serde_json::json!({
            "12": {"id": "12", "revisionID": "7",
                   "sourceControlBaseRevision": "base",
                   "authorName": "Ada", "authorEmail": "ada@test",
                   "changes": []},
        }))
    };
    let bundle = fetch_revision_bundle_raw(&call, "x").unwrap();
    assert_eq!(bundle.revision_id, Some(7));
    fetch_diff_bundle_raw(&call, "D12").unwrap();
    let seen = seen.borrow();
    assert!(
        seen[0].1.contains("\"revisionIDs\":[\"x\"]"),
        "{}",
        seen[0].1
    );
    assert!(seen[1].1.contains("\"ids\":[\"D12\"]"), "{}", seen[1].1);
}
