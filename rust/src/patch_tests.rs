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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "git {args:?} failed: {stderr}");
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

/// Shared svn-flow hook stubs. One body each (covered by
/// [`hook_stubs_behave`]), so call sites that legitimately never fire a
/// hook carry no dead closure lines.
fn ok_patch(_: &str, _: &str) -> Result<(bool, String), String> {
    Ok((true, String::new()))
}
fn ok_touch(_: &str) -> Result<(), String> {
    Ok(())
}
fn ok_link(_: &str, _: &str) -> Result<(), String> {
    Ok(())
}
fn no_patch(_: &str, _: &str) -> Result<(bool, String), String> {
    panic!("no patch expected")
}
fn no_link(_: &str, _: &str) -> Result<(), String> {
    panic!("no link expected")
}

/// The amend path never opens the editor.
fn never_edit(_: &str) -> Result<String, String> {
    panic!("PATCH-P9: amend path never opens the editor")
}

/// No repository query is expected on this path.
fn no_repository_query(_: &str, _: &serde_json::Value) -> Result<serde_json::Value, String> {
    panic!("no repository query expected")
}

#[test]
fn hook_stubs_behave() {
    assert_eq!(ok_patch("a", "b"), Ok((true, String::new())));
    assert_eq!(ok_touch("a"), Ok(()));
    assert_eq!(ok_link("a", "b"), Ok(()));
}

#[test]
#[should_panic(expected = "no patch expected")]
fn no_patch_stub_panics() {
    let _result: Result<(bool, String), String> = no_patch("a", "b");
}

#[test]
#[should_panic(expected = "no link expected")]
fn no_link_stub_panics() {
    let _result: Result<(), String> = no_link("a", "b");
}

#[test]
#[should_panic(expected = "never opens the editor")]
fn never_edit_stub_panics() {
    let _result: Result<String, String> = never_edit("template");
}

#[test]
#[should_panic(expected = "no repository query expected")]
fn no_repository_query_stub_panics() {
    let _result: Result<serde_json::Value, String> =
        no_repository_query("m", &serde_json::Value::Null);
}

#[test]
#[should_panic(expected = "expected options")]
fn options_of_panics_on_parse_error() {
    options_of(&["--bogus-flag-xyz"]);
}

#[test]
#[should_panic(expected = "expected error")]
fn parse_err_panics_on_success() {
    parse_err(&["D1"]);
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
            encoding: Some("utf-8".to_string()),
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
    // T1153 EXPORT-E9: the raw byte rides as transport and the encoding
    // converts at the sinks, like `convertNonUTF8Diff()` (invalid bytes
    // become `?`, like `mb_convert_encoding()`).
    assert_eq!(bundle.text, transport_bytes(&bytes));
    assert_eq!(bundle.encoding.as_deref(), Some("latin1"));
    let mut converted = bytes.clone();
    let last = converted.len() - 2;
    converted[last] = b'?';
    assert_eq!(bundle_patch_bytes(&bundle).unwrap(), converted);
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
        synthetic_counts: None,
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "tar failed: {stderr}");
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

#[test]
fn cherry_pick_spawn_failure_reports_and_cleans_up() {
    // The cherry-pick spawn itself fails (a nonexistent `git` program):
    // the spawn error becomes the failure detail behind the verbatim
    // banner, and the temp branch is still deleted.
    let repo = make_repo("cherry_spawn_fail");
    git(&repo, &["checkout", "-b", "temp"]);
    std::fs::write(repo.join("file.txt"), "temp\n").unwrap();
    git(&repo, &["commit", "-am", "temp"]);
    git(&repo, &["checkout", "master"]);
    let mut io = Io::buffered();
    let err = git_cherry_pick_temp_ansi_with_git(
        &repo,
        &mut io,
        "master",
        "temp",
        false,
        Some(std::path::Path::new("/nonexistent-git-xyz")),
    )
    .unwrap_err();
    assert!(err.starts_with("failed to run git:"), "{err}");
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

/// Fake `hg` via PowerShell on Windows (cmd.exe mangles `<>` in argv
/// logging, so author-carrying tests use this); plain shell fake on
/// unix, where the `.sh` fixture passes every byte through.
fn fake_hg_ps1_backend(dir: &std::path::Path) -> hg::HgCommand {
    #[cfg(windows)]
    {
        let ps1 = dir.join("fakehg-gap2.ps1");
        std::fs::write(&ps1, include_str!("../tests/fixtures/vcs/fakehg.ps1")).unwrap();
        hg::HgCommand::custom(
            "powershell",
            &[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &ps1.to_string_lossy(),
            ],
        )
    }
    #[cfg(not(windows))]
    {
        fake_hg_backend(dir)
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
        &mut ok_touch,
        &mut ok_link,
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
        &mut ok_touch,
        &mut ok_link,
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
        &mut ok_patch,
        &mut ok_touch,
        &mut ok_link,
    )
    .unwrap_err();
    assert_eq!(error, land::ABORT);
    let mut io = Io::canned(&["y"]);
    let outcome = apply_svn(
        &dir,
        &svn::SvnCommand::system(),
        &[delete],
        &mut io,
        &mut ok_patch,
        &mut ok_touch,
        &mut ok_link,
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
        &mut ok_patch,
        &mut |_| Ok(()),
        &mut ok_link,
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
    let result = run_symlink(&dir, "new.txt", "link.txt");
    let linked = result.is_ok();
    let message_ok = linked || result.unwrap_err().contains("Failed to link");
    let relinked = !linked || run_symlink(&dir, "other", "link.txt").is_ok();
    let is_link = !linked || dir.join("link.txt").is_symlink();
    assert!(linked || cfg!(windows));
    assert!(message_ok);
    assert!(relinked);
    assert!(is_link);
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
    // T1153 B21/B23: `--help` renders the generated workflow help.
    let dir = temp_dir("exec_help");
    let backends = Backends::default();
    let mut io = Io::buffered();
    // T1153 R10: `--help` renders the generated workflow help (exact
    // bytes locked by `r10_patch_help_matches_generated_format`).
    let out = execute_with_io_ansi(&dir, &argv(&["--help"]), &mut io, &backends, false).unwrap();
    assert_eq!(out.stdout, usage_text(false));
    let mut io = Io::buffered();
    let error = execute_with_io(&dir, &argv(&["--bogus"]), &mut io, &backends).unwrap_err();
    assert!(error.contains("PATCH WORKFLOW"), "{error}");
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
    // T1153 B21/B23: `--help` renders the generated workflow help.
    let dir = temp_dir("run_points");
    assert_eq!(run_in(&dir, &argv(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(
        execute_ansi(&dir, &argv(&["--help"]), false)
            .unwrap()
            .stdout,
        usage_text(false)
    );
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
            &mut ok_patch,
            &mut ok_touch,
            &mut ok_link,
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
    let edit = never_edit;
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
        assert_eq!(method, "diffusion.querycommits", "unexpected {method}");
        assert_eq!(params["names"], serde_json::json!([format!("rX{hash}")]));
        Ok(serde_json::json!({
            "identifierMap": {format!("rX{hash}"): {"id": 1}}
        }))
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
    let first = &seen[0].1;
    assert!(first.contains("\"revisionIDs\":[\"x\"]"), "{first}");
    assert!(seen[1].1.contains("\"ids\":[\"D12\"]"), "{}", seen[1].1);
}
#[test]
fn r10_patch_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "PATCH WORKFLOW\n\
         \n\
         patch\n\
         \x20     patch D12345\n\
         \x20     patch --revision revision_id\n\
         \x20     patch --diff diff_id\n\
         \x20     patch --patch file\n\
         \x20     patch --arcbundle bundlefile\n\
         \n\
         \x20         Supports: git, svn, hg\n\
         \x20         Apply the changes in a Differential revision, patchfile, or arc\n\
         \x20         bundle to the working copy.\n\
         \n\
         \x20     --arcbundle bundlefile\n\
         \x20         Apply changes from an arc bundle generated with 'arc export'.\n\
         \n\
         \x20     --diff diff_id\n\
         \x20         Apply changes from a Differential diff. Normally you want to use\n\
         \x20         --revision to get the most recent changes, but you can specifically\n\
         \x20         apply an out-of-date diff or a diff which was never attached to a\n\
         \x20         revision by using this flag.\n\
         \n\
         \x20     --encoding encoding\n\
         \x20         Attempt to convert non UTF-8 patch into specified encoding.\n\
         \n\
         \x20     --force\n\
         \x20         Do not run any sanity checks.\n\
         \n\
         \x20     --nobranch\n\
         \x20         Normally, a new branch (git) or bookmark (hg) is created and then\n\
         \x20         the patch is applied and committed in the new branch/bookmark. This\n\
         \x20         flag cherry-picks the resultant commit onto the original branch and\n\
         \x20         deletes the temporary branch.\n\
         \n\
         \x20     --nocommit\n\
         \x20         Normally under git/hg, if the patch is successful, the changes are\n\
         \x20         committed to the working copy. This flag prevents the commit.\n\
         \n\
         \x20     --patch patchfile\n\
         \x20         Apply changes from a git patchfile or unified patchfile.\n\
         \n\
         \x20     --revision revision_id\n\
         \x20         Apply changes from a Differential revision, using the most recent\n\
         \x20         diff that has been attached to it. You can run 'arc patch D12345'\n\
         \x20         as a shorthand.\n\
         \n\
         \x20     --skip-dependencies\n\
         \x20         Normally, if a patch has dependencies that are not present in the\n\
         \x20         working copy, arc tries to apply them as well. This flag prevents\n\
         \x20         such work.\n\
         \n\
         \x20     --update\n\
         \x20         Update the local working copy before applying the patch.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `execute_with_io_ansi` pushes the same text with explicit ANSI.
    assert!(usage_text(true).contains("PATCH WORKFLOW"));
}

#[test]
fn b21_patch_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `patch`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_options(&argv(&["D123"])).unwrap().is_some());
    assert_eq!(parse_options(&argv(&["--help"])).unwrap(), None);
    let text = usage_text(false);
    assert!(text.contains("PATCH WORKFLOW"), "{text}");
}

#[test]
fn b23_patch_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help patch`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("PATCH WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Apply the changes in a Differential revision"),
        "{text}"
    );
    assert!(text.contains("--nocommit"), "{text}");
    assert!(!text.contains("--preview"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// T1153 EXPORT-E9 residual: byte-faithful corpus path. Raw bytes ride
// the `String` pipeline as transport (valid UTF-8 verbatim, invalid
// bytes smuggled, real U+E000 markers doubled) and convert only at
// the sinks, matching PHP's raw passthrough plus
// `convertNonUTF8Diff()`.

const E9_ESC: char = '\u{E000}';

fn e9_smuggle(byte: u8) -> String {
    format!(
        "{E9_ESC}{}",
        char::from_u32(0xE100 + u32::from(byte)).unwrap()
    )
}

fn e9_invalid_vectors() -> Vec<Vec<u8>> {
    vec![
        vec![0xE9],
        vec![0x80],
        vec![0xFF],
        vec![0xFE, 0xFF],
        vec![0xE9, 0x80],
        vec![0xED, 0xA0, 0x80],
        vec![0xC0, 0xAF],
        vec![0xF5, 0x80, 0x80, 0x80],
        vec![0xC3],
        vec![0xE2, 0x82],
        b"caf\xe9 mix \x80\xff end".to_vec(),
        "caf\u{00e9} valid \u{20ac} ".as_bytes().to_vec(),
        b"\x00nul\x00".to_vec(),
        b"a\r\nb\r\n".to_vec(),
        vec![],
    ]
}

#[test]
fn e9_transport_round_trips_bytes() {
    for bytes in e9_invalid_vectors() {
        assert_eq!(untransport_bytes(&transport_bytes(&bytes)), bytes);
    }
    // Valid UTF-8 (however exotic) passes through verbatim.
    for text in [
        "",
        "plain ascii",
        "caf\u{00e9} \u{20ac} \u{1f600} \u{4e2d}",
        "\x00\x01\x02",
    ] {
        assert_eq!(transport_bytes(text.as_bytes()), text);
    }
}

#[test]
fn e9_transport_escapes_real_markers() {
    let marker = E9_ESC.to_string();
    assert_eq!(
        transport_bytes(marker.as_bytes()),
        format!("{E9_ESC}{E9_ESC}")
    );
    assert_eq!(
        untransport_bytes(&format!("{E9_ESC}{E9_ESC}")),
        marker.as_bytes()
    );
    // A natural marker + smuggle-range pair round-trips exactly.
    let pair = format!("{E9_ESC}\u{E100}");
    assert_eq!(
        untransport_bytes(&transport_bytes(pair.as_bytes())),
        pair.as_bytes()
    );
    // A lone marker reads literally instead of panicking.
    assert_eq!(
        untransport_bytes(&E9_ESC.to_string()),
        E9_ESC.to_string().as_bytes()
    );
}

#[test]
fn e9_transport_preserves_diff_structure() {
    let bytes = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+\xe9\n";
    let carried = transport_bytes(bytes);
    assert!(carried.starts_with("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+"));
    assert!(carried.ends_with(&format!("{}\n", e9_smuggle(0xE9))));
}

#[test]
fn e9_untransport_lossy_views() {
    assert_eq!(untransport_lossy(&e9_smuggle(0xE9)), "\u{FFFD}");
    assert_eq!(untransport_lossy("plain caf\u{00e9}"), "plain caf\u{00e9}");
    assert_eq!(
        untransport_lossy(&format!("{E9_ESC}{E9_ESC}")),
        E9_ESC.to_string()
    );
    assert_eq!(untransport_lossy(""), "");
}

#[test]
fn e9_escape_markers_round_trip() {
    assert_eq!(escape_transport_markers("plain"), "plain");
    assert_eq!(
        escape_transport_markers(&E9_ESC.to_string()),
        format!("{E9_ESC}{E9_ESC}")
    );
    let escaped = escape_transport_markers("a\u{E000}b");
    assert_eq!(untransport_lossy(&escaped), "a\u{E000}b");
}

#[test]
fn e9_convert_passes_empty_and_utf8_through() {
    let valid = "caf\u{00e9} \u{20ac}\n".as_bytes().to_vec();
    let invalid = b"caf\xe9 \x80\xff\n".to_vec();
    for encoding in [None, Some(""), Some("0"), Some("UTF-8"), Some("utf8")] {
        assert_eq!(convert_bytes(&valid, encoding).unwrap(), valid);
        assert_eq!(convert_bytes(&invalid, encoding).unwrap(), invalid);
    }
}

#[test]
fn e9_convert_cp1252_table() {
    assert_eq!(
        convert_bytes("caf\u{00e9} \u{20ac}\n".as_bytes(), Some("windows-1252")).unwrap(),
        b"caf\xe9 \x80\n"
    );
    // Unmappable scalars and invalid bytes become `?`, like mb.
    assert_eq!(
        convert_bytes("\u{4e2d}\n".as_bytes(), Some("cp1252")).unwrap(),
        b"?\n"
    );
    assert_eq!(
        convert_bytes(b"\xff\n", Some("windows-1252")).unwrap(),
        b"?\n"
    );
    // Undefined cp1252 bytes decode lossily, like PHP's converter.
    assert_eq!(
        decode_bytes(&[0x81], Some("windows-1252")).unwrap(),
        "\u{FFFD}"
    );
}

#[test]
fn e9_convert_latin1_one_to_one() {
    assert_eq!(
        convert_bytes("caf\u{00e9}\n".as_bytes(), Some("latin1")).unwrap(),
        b"caf\xe9\n"
    );
    assert_eq!(
        convert_bytes("\u{20ac}\n".as_bytes(), Some("latin1")).unwrap(),
        b"?\n"
    );
    assert_eq!(convert_bytes(b"\xe9\n", Some("latin1")).unwrap(), b"?\n");
}

#[test]
fn e9_convert_invalid_grouping_matches_mb() {
    // Pinned against `mb_convert_encoding(..., 'Windows-1252', 'UTF-8')`.
    for (input, expected) in [
        (b"\xe9\x80".as_slice(), "?"),
        (b"\xed\xa0\x80".as_slice(), "???"),
        (b"\xf0\x28\x8c\x28".as_slice(), "?(?("),
        (b"\xc0\xaf".as_slice(), "??"),
        (b"\x80".as_slice(), "?"),
        (b"\xc3".as_slice(), "?"),
        (b"\xfe\xff".as_slice(), "??"),
        (b"a\xffb".as_slice(), "a?b"),
    ] {
        for encoding in ["windows-1252", "latin1"] {
            assert_eq!(
                convert_bytes(input, Some(encoding)).unwrap(),
                expected.as_bytes(),
                "{input:?} / {encoding}"
            );
        }
    }
}

#[test]
fn e9_convert_unknown_throws() {
    assert_eq!(
        convert_bytes(b"x", Some("klingon-9")).unwrap_err(),
        "Unknown encoding \"klingon-9\"."
    );
}

#[test]
fn e9_encode_transport_text_converts_at_sink() {
    let carried = transport_bytes(b"caf\xe9\n");
    assert_eq!(encode_transport_text(&carried, None).unwrap(), b"caf\xe9\n");
    assert_eq!(
        encode_transport_text(&carried, Some("UTF-8")).unwrap(),
        b"caf\xe9\n"
    );
    assert_eq!(
        encode_transport_text(&carried, Some("windows-1252")).unwrap(),
        b"caf?\n"
    );
    let valid = transport_bytes("caf\u{00e9}\n".as_bytes());
    assert_eq!(
        encode_transport_text(&valid, Some("windows-1252")).unwrap(),
        b"caf\xe9\n"
    );
}

#[test]
fn e9_convert_transport_text_round_trip() {
    for bytes in e9_invalid_vectors() {
        let carried = transport_bytes(&bytes);
        for encoding in [None, Some("UTF-8"), Some("windows-1252"), Some("latin1")] {
            let converted = convert_transport_text(&carried, encoding).unwrap();
            assert_eq!(
                untransport_bytes(&converted),
                convert_bytes(&bytes, encoding).unwrap(),
                "{bytes:?} / {encoding:?}"
            );
        }
    }
    assert_eq!(
        convert_transport_text("x", Some("klingon-9")).unwrap_err(),
        "Unknown encoding \"klingon-9\"."
    );
}

#[test]
fn e9_bundle_patch_bytes_convert_like_php() {
    let bundle = Bundle {
        text: transport_bytes(b"+caf\xe9\n"),
        ..Bundle::default()
    };
    assert_eq!(bundle_patch_bytes(&bundle).unwrap(), b"+caf\xe9\n");
    let bundle = Bundle {
        text: transport_bytes(b"+caf\xe9\n"),
        encoding: Some("latin1".to_string()),
        ..Bundle::default()
    };
    assert_eq!(bundle_patch_bytes(&bundle).unwrap(), b"+caf?\n");
    let bundle = Bundle {
        text: "+x\n".to_string(),
        encoding: Some("klingon-9".to_string()),
        ..Bundle::default()
    };
    assert_eq!(
        bundle_patch_bytes(&bundle).unwrap_err(),
        "Unknown encoding \"klingon-9\"."
    );
}

#[test]
fn e9_load_file_bundle_carries_raw_bytes() {
    let dir = temp_dir("e9_file_bundle");
    let path = dir.join("raw.patch");
    let bytes = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\xe9\x80\n".to_vec();
    std::fs::write(&path, &bytes).unwrap();
    let name = path.to_string_lossy().to_string();
    for encoding in [None, Some("UTF-8")] {
        let mut io = Io::buffered();
        let bundle = load_file_bundle(&name, encoding, &mut io).unwrap();
        assert_eq!(bundle.text, transport_bytes(&bytes));
        assert_eq!(bundle.encoding, encoding.map(str::to_string));
        assert_eq!(bundle_patch_bytes(&bundle).unwrap(), bytes);
    }
    let mut io = Io::buffered();
    let bundle = load_file_bundle(&name, Some("windows-1252"), &mut io).unwrap();
    assert_eq!(bundle.encoding.as_deref(), Some("windows-1252"));
    assert_eq!(
        bundle_patch_bytes(&bundle).unwrap(),
        convert_bytes(&bytes, Some("windows-1252")).unwrap()
    );
    let mut io = Io::buffered();
    assert_eq!(
        load_file_bundle(&name, Some("klingon-9"), &mut io).unwrap(),
        Bundle {
            text: transport_bytes(&bytes),
            encoding: Some("klingon-9".to_string()),
            ..Bundle::default()
        }
    );
}

#[test]
fn e9_quote_git_target_preserves_transport() {
    // ASCII behavior is unchanged (also covered by `quotes_git_targets`).
    assert_eq!(quote_git_target("plain"), "plain");
    // Unquoted transport paths pass through verbatim.
    let carried = transport_bytes(b"f-\xe9.txt");
    assert_eq!(quote_git_target(&carried), carried);
    // Quoting runs over raw bytes; high bytes re-ride as transport.
    let quoted = quote_git_target(&transport_bytes(b"a\"b\xe9"));
    assert_eq!(quoted, format!("\"a\\\"b{}\"", e9_smuggle(0xE9)));
    assert_eq!(untransport_bytes(&quoted), b"\"a\\\"b\xe9\"");
}

#[test]
fn e9_unquote_git_path_decodes_high_octals() {
    assert_eq!(unquote_git_path("\"a\\tb\""), "a\tb");
    assert_eq!(unquote_git_path("plain"), "plain");
    // Valid octals decode to real chars; invalid ones ride as transport.
    assert_eq!(unquote_git_path("\"a\\303\\251\""), "a\u{00e9}");
    let decoded = unquote_git_path("\"a\\351\"");
    assert_eq!(decoded, format!("a{}", e9_smuggle(0xE9)));
    assert_eq!(untransport_bytes(&decoded), b"a\xe9");
    // `\r` maps to CR like `stripcslashes()` (covers the carriage arm
    // of the cognitive-gate split).
    assert_eq!(unquote_git_path("\"a\\rb\""), "a\rb");
}

#[test]
fn e9_strip_ansi_codes_preserves_bytes() {
    let mut raw = "caf\u{00e9} ".as_bytes().to_vec();
    raw.push(0xE9);
    let carried = transport_bytes(&raw);
    let colored = format!("\x1b[32m{carried}\x1b[m");
    assert_eq!(strip_ansi_codes(&colored), carried);
    assert_eq!(strip_ansi_codes("plain"), "plain");
}

#[test]
fn e9_validate_accepts_non_ascii_without_panic() {
    // Multibyte chars before a unified date used to panic on byte slicing.
    validate_patch_text("--- caf\u{00e9}g.txt\t2024-01-02 03:04:05\n+++ f\n").unwrap();
    validate_patch_text(&transport_bytes(
        b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@ func\xe9\n-a\n+b\n",
    ))
    .unwrap();
    assert!(validate_patch_text("not a diff\n").is_err());
}

#[test]
fn e9_parse_error_hides_transport() {
    let lines = vec![transport_bytes(b"+bad \xe9 line\n")];
    let rendered = format_parse_error("broken", &lines, 0);
    assert!(rendered.contains('\u{FFFD}'), "{rendered}");
    assert!(!rendered.contains(E9_ESC), "{rendered}");
}

#[test]
fn e9_arcbundle_carries_raw_corpora() {
    let dir = temp_dir("e9_arcbundle");
    let changes = br#"[{"type":1,"fileType":1,"oldPath":"f","currentPath":"f","hunks":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"corpus":0}]}]"#;
    let corpus = b" line\n-line\n+caf\xe9 raw\n";
    let bundle = write_arcbundle(
        &dir,
        "raw.bundle",
        &[
            ("changes.json", changes),
            ("hunks/0", corpus),
            ("meta.json", b"{}"),
        ],
    );
    let name = bundle.to_string_lossy().to_string();
    let loaded = load_arcbundle(&name, None).unwrap();
    assert_eq!(loaded.encoding, None);
    assert!(
        untransport_bytes(&loaded.text)
            .windows(corpus.len())
            .any(|w| w == corpus),
        "raw corpus survives the render"
    );
    assert_eq!(
        bundle_patch_bytes(&loaded).unwrap(),
        untransport_bytes(&loaded.text)
    );
}

#[test]
fn e9_arcbundle_meta_encoding_converts_at_sink() {
    let dir = temp_dir("e9_arcbundle_enc");
    let changes = br#"[{"type":1,"fileType":1,"oldPath":"f","currentPath":"f","hunks":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"corpus":0}]}]"#;
    let bundle = write_arcbundle(
        &dir,
        "enc.bundle",
        &[
            ("changes.json", changes),
            ("hunks/0", b" line\n-line\n+caf\xe9 raw\n"),
            ("meta.json", br#"{"encoding":"windows-1252"}"#),
        ],
    );
    let name = bundle.to_string_lossy().to_string();
    let loaded = load_arcbundle(&name, None).unwrap();
    // The meta encoding is stored, not decoded: unknown names only throw
    // when the bundle converts at an apply sink, like PHP.
    assert_eq!(loaded.encoding.as_deref(), Some("windows-1252"));
    assert!(
        bundle_patch_bytes(&loaded)
            .unwrap()
            .windows(4)
            .any(|w| w == b"caf?"),
        "converts like convertNonUTF8Diff"
    );
}

#[test]
fn e9_svn_changes_stay_lossy() {
    // The svn apply path keeps its historical lossy behavior (no svn
    // binary exists here to oracle-verify a byte port): transport in,
    // replacement chars out, never transport markers.
    let changes = parse_git_patch_for_svn(&transport_bytes(
        b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\xe9\n",
    ));
    assert_eq!(changes.len(), 1);
    let hunks = &changes[0].hunks;
    assert!(hunks.contains('\u{FFFD}'), "{hunks}");
    assert!(!changes[0].hunks.contains(E9_ESC), "{}", changes[0].hunks);
    let dicts: Vec<serde_json::Value> =
        vec![serde_json::json!({"type": 1, "fileType": 1, "oldPath": "o", "currentPath": "f"})];
    let hunks = vec![crate::diff_local::ParsedHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        corpus: transport_bytes(b"+q\xff\n"),
        synthetic_counts: None,
    }];
    let changes = svn_changes_from_dicts(&dicts, &[hunks]);
    assert!(changes[0].hunks.contains('\u{FFFD}'));
    assert!(!changes[0].hunks.contains(E9_ESC));
}

#[test]
fn e9_transport_parse_decodes_quoted_bytes() {
    // `parse_unified_diff_transport` matches the lossy parse on
    // structure, but C-quoted filenames decode byte-faithfully. (The
    // `+++` line keeps quotes verbatim, so the rename header carries
    // the quoted name: PHP keeps the decoded `rename to` path.)
    let text = "diff --git a/o b/n\nrename from o\nrename to \"n\\351\"\n--- a/o\n+++ \"b/n\\351\"\n@@ -1 +1 @@\n-a\n+b\n";
    let lossy = crate::diff_local::parse_unified_diff(text).unwrap();
    assert_eq!(lossy[0].current_path, "n\u{FFFD}");
    let carried =
        crate::diff_local::parse_unified_diff_transport(&transport_bytes(text.as_bytes())).unwrap();
    assert_eq!(untransport_bytes(&carried[0].current_path), b"n\xe9");
    assert_eq!(carried[0].hunks.len(), lossy[0].hunks.len());
    assert_eq!(carried[0].old_path, lossy[0].old_path);
}

#[test]
fn e9_apply_bundle_encoding_stores_selection() {
    let backends = Backends::default();
    let repo = make_repo("e9_apply_encoding");
    std::fs::write(repo.join(".arcconfig"), "{\"repository.callsign\": \"X\"}").unwrap();
    let answer = std::cell::RefCell::new(serde_json::json!([{"encoding": "cp1252"}]));
    let call = |method: &str, _params: &serde_json::Value| {
        assert_eq!(method, "repository.query");
        Ok(answer.borrow().clone())
    };
    let call_ref: &diff_submit::ConduitCall = &call;
    // Conduit sources store the flag, else the repository encoding.
    let mut loaded = LoadedBundle {
        bundle: Bundle {
            text: "t".to_string(),
            ..Bundle::default()
        },
        arcbundle_dicts: None,
    };
    let options = options_of(&["--encoding", "latin1"]);
    apply_patch_bundle_encoding(
        &repo,
        Vcs::Git,
        &backends,
        &Source::Revision("D1".to_string()),
        &options,
        Some(call_ref),
        &mut loaded,
    )
    .unwrap();
    assert_eq!(loaded.bundle.encoding.as_deref(), Some("latin1"));
    assert_eq!(loaded.bundle.text, "t");
    let mut loaded = LoadedBundle {
        bundle: Bundle::default(),
        arcbundle_dicts: None,
    };
    let options = options_of(&[]);
    apply_patch_bundle_encoding(
        &repo,
        Vcs::Git,
        &backends,
        &Source::Diff("123".to_string()),
        &options,
        Some(call_ref),
        &mut loaded,
    )
    .unwrap();
    assert_eq!(loaded.bundle.encoding.as_deref(), Some("cp1252"));
    // File sources keep their flag-stored encoding untouched.
    let mut loaded = LoadedBundle {
        bundle: Bundle {
            encoding: Some("utf-8".to_string()),
            ..Bundle::default()
        },
        arcbundle_dicts: None,
    };
    apply_patch_bundle_encoding(
        &repo,
        Vcs::Git,
        &backends,
        &Source::Patch("f".to_string()),
        &options_of(&[]),
        Some(call_ref),
        &mut loaded,
    )
    .unwrap();
    assert_eq!(loaded.bundle.encoding.as_deref(), Some("utf-8"));
}

// T1170 coverage tests below. Each `t1170_` test pins observable behavior
// for a miss-manifest cluster in `patch.rs`.

/// Fake `svn` backend logging argv to `.fakesvn.log` (portable .bat/.sh).
fn fake_svn_backend(dir: &std::path::Path) -> svn::SvnCommand {
    #[cfg(windows)]
    let (name, content) = (
        "fakesvn-t1170.bat",
        include_str!("../tests/fixtures/vcs/fakesvn.bat"),
    );
    #[cfg(not(windows))]
    let (name, content) = (
        "fakesvn-t1170.sh",
        include_str!("../tests/fixtures/vcs/fakesvn.sh"),
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
        svn::SvnCommand::custom("cmd", &["/C", &script.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        svn::SvnCommand::custom(&script.to_string_lossy(), &[])
    }
}

/// Write a purpose-built `hg` stub script (unix-only dispatch the shared
/// fixture cannot express: `\x01` tip templates, stderr failures, distinct
/// revisions per `-r` argument).
#[cfg(unix)]
fn stub_hg(dir: &std::path::Path, name: &str, body: &str) -> hg::HgCommand {
    let script = dir.join(name);
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    hg::HgCommand::custom(&script.to_string_lossy(), &[])
}

/// Fake `patch` binary routing through `git apply` (hermetic: system
/// patch is absent on some machines and fails valid input on Windows
/// CI). The shim applies the `-i` file in its cwd with newline
/// conversion disabled (else Windows git writes CRLF) and echoes a
/// fixed marker, propagating git's exit code. Returns `(program,
/// prefix)` for `run_patch_file_with_argv` — the `.bat` needs `cmd
/// /C` first.
fn fake_patch_shim(dir: &std::path::Path) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let bat = dir.join("fakepatch.bat");
        std::fs::write(
            &bat,
            "@echo off\r\ngit -c core.autocrlf=false apply \"%~3\"\r\nset CODE=%errorlevel%\r\necho applied patch\r\nexit /b %CODE%\r\n",
        )
        .unwrap();
        (
            "cmd".to_string(),
            vec!["/C".to_string(), bat.to_string_lossy().to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        let sh = dir.join("fakepatch.sh");
        std::fs::write(
            &sh,
            "#!/bin/sh\ngit -c core.autocrlf=false apply \"$3\"\ncode=$?\necho \"applied patch\"\nexit $code\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&sh).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&sh, perms).unwrap();
        }
        (sh.to_string_lossy().to_string(), Vec::new())
    }
}

#[test]
fn t1170_unknown_provided_entries_are_ignored() {
    // `provided` entries with no flag spec skip both gates (patch.rs:488,
    // :508); only known flags can conflict or restrict a VCS.
    let options = Options {
        provided: vec!["bogus"],
        ..Options::default()
    };
    assert_eq!(check_conflicts(&options), Ok(()));
    assert_eq!(check_supports(&options, "git"), Ok(()));
    assert_eq!(check_supports(&options, "svn"), Ok(()));
}

#[test]
fn t1170_validate_ansi_colored_diff() {
    // A leading `\x1B[<digits>m` colors the strip path (patch.rs:1011-1016,
    // :1039, :1258) and the cleaned header validates (patch.rs:1248).
    let text = "\u{1b}[32mdiff --git a/f b/f\u{1b}[0m\n";
    assert!(has_leading_ansi(text));
    assert!(!has_leading_ansi("diff --git a/f b/f\n"));
    assert_eq!(strip_ansi_codes(text), "diff --git a/f b/f\n");
    assert_eq!(validate_patch_text(text), Ok(()));
}

#[test]
fn t1170_validate_nul_only_text_errors() {
    // `"\0"` survives `trim()` but `ltrim_php()` eats it, so the split of
    // `""` yields one empty line (patch.rs:995) that fails the header
    // check with a byte-for-byte parse error.
    let err = validate_patch_text("\0").unwrap_err();
    assert!(
        err.starts_with("Diff Parse Exception: Expected a hunk header"),
        "{err}"
    );
}

#[test]
fn t1170_validate_comment_only_text_errors_at_eof() {
    // Only `#` comments: the content index runs past the last line
    // (patch.rs:1241) and the error carries no `>>>` marker, like PHP.
    let err = validate_patch_text("# one\n# two\n").unwrap_err();
    assert!(
        err.starts_with("Diff Parse Exception: Expected a hunk header"),
        "{err}"
    );
    assert!(!err.contains(">>>"), "{err}");
}

#[test]
fn t1170_parse_error_skips_distant_lines() {
    // Context spans five lines each way; farther lines are skipped
    // (patch.rs:1210), so distant content never leaks into the error.
    let mut text = String::from("# comment\nNEAR-garbage\n");
    for filler in 0..10 {
        text.push_str(&format!("filler{filler}\n"));
    }
    text.push_str("DISTANT\n");
    let err = validate_patch_text(&text).unwrap_err();
    assert!(err.contains(">>>"), "{err}");
    assert!(err.contains("NEAR-garbage"), "{err}");
    assert!(!err.contains("DISTANT"), "{err}");
}

#[test]
fn t1170_validate_hg_export_preamble() {
    // A later `diff -r` opener excuses a non-header first line
    // (patch.rs:1299-1301, :1308), covering `hg export` preambles.
    assert_eq!(
        validate_patch_text("changeset: 1:abc\nsome preamble\ndiff -r ab12cd file.txt\n"),
        Ok(())
    );
}

#[test]
fn t1170_validate_indented_later_diff_header() {
    // The loose `^\s*diff` probe (patch.rs:1304-1307) also accepts an
    // indented later `diff --git` opener.
    assert_eq!(
        validate_patch_text("preamble\n  diff --git a/f b/f\n"),
        Ok(())
    );
}

#[test]
fn t1170_validate_blank_then_garbage_fails() {
    // The first content line is blank but a later line is not: the probe
    // only excuses the failing line itself (patch.rs:1290, :1295).
    let err = validate_patch_text("# c\n\nGARBAGE\n").unwrap_err();
    assert!(
        err.starts_with("Diff Parse Exception: Expected a hunk header"),
        "{err}"
    );
}

#[test]
fn t1170_validate_rejects_malformed_hg_and_binary_headers() {
    // Each malformed opener fails its header predicate (patch.rs:1079,
    // :1122, :1135, :1148, :1153, :1157) and then the later-opener probe.
    for text in [
        "diff -r abcdef\n",
        "diff -r ab -r x\n",
        "Files foo\n",
        "Binary files a and b same\n",
        "--- 2024-01-02\n",
    ] {
        let err = validate_patch_text(text).unwrap_err();
        assert!(
            err.starts_with("Diff Parse Exception: Expected a hunk header"),
            "{text}: {err}"
        );
    }
}

#[test]
fn t1170_arcbundle_missing_hunk_member_errors() {
    // Both `member` and `./member` extraction attempts fail
    // (patch.rs:1381-1385), naming the missing hunk member.
    let dir = temp_dir("t1170_arcbundle_hunk");
    let changes = br#"[{"type":2,"fileType":1,"oldPath":"f.txt","currentPath":"f.txt","hunks":[{"corpus":0,"oldOffset":1,"oldLength":1,"newOffset":1,"newLength":1}]}]"#;
    let bundle = write_arcbundle(&dir, "h.bundle", &[("changes.json", changes)]);
    let err = load_arcbundle(&bundle.to_string_lossy(), None).unwrap_err();
    assert!(err.contains("member 'hunks/0' is missing"), "{err}");
}

#[test]
fn t1170_arcbundle_without_meta_uses_defaults() {
    // Old bundles predate `meta.json` (patch.rs:2322): metadata stays
    // `None` instead of failing.
    let dir = temp_dir("t1170_arcbundle_nometa");
    let bundle = write_arcbundle(&dir, "m.bundle", &[("changes.json", b"[]")]);
    let loaded = load_arcbundle(&bundle.to_string_lossy(), None).unwrap();
    assert_eq!(loaded.text, "\n");
    assert!(loaded.base_revision.is_none());
    assert!(loaded.revision_id.is_none());
    assert!(loaded.encoding.is_none());
    assert!(loaded.author_name.is_none());
    assert!(loaded.author_email.is_none());
    // A change dict without `hunks` skips corpus loading
    // (patch.rs:2369) and still renders its headers.
    let bundle = write_arcbundle(
        &dir,
        "h.bundle",
        &[(
            "changes.json",
            br#"[{"type":2,"fileType":1,"oldPath":"f.txt","currentPath":"f.txt"}]"#.as_slice(),
        )],
    );
    let loaded = load_arcbundle(&bundle.to_string_lossy(), None).unwrap();
    assert!(loaded.base_revision.is_none());
    assert!(
        loaded.text.contains("diff --git a/f.txt b/f.txt"),
        "{}",
        loaded.text
    );
}

#[test]
fn t1170_parse_bundle_change_carries_away_paths() {
    // `awayPaths` entries ride through (patch.rs:1535-1539) while
    // non-string entries are dropped.
    let dict = serde_json::json!({"type": 4, "fileType": 2, "awayPaths": ["d1", 7, "d2"]});
    let change = parse_bundle_change(&dict, Vec::new());
    assert_eq!(change.away_paths, vec!["d1".to_string(), "d2".to_string()]);
}

#[test]
fn t1170_base64_decodes_digits_and_symbols() {
    // Digit, `+`, and `/` values decode (patch.rs:1794-1796); other
    // bytes fail (patch.rs:1798).
    assert_eq!(base64_decode("f+/9").unwrap(), vec![0x7F, 0xEF, 0xFD]);
    assert_eq!(base64_decode("AB!D").unwrap_err(), "invalid base64 byte");
}

#[test]
fn t1170_build_binary_change_prefers_old_source() {
    // The away change supplies the old blob: inline data wins
    // (patch.rs:1888) without touching the loader, else the old phid
    // loads through it (patch.rs:1889).
    let change = BundleChange {
        file_type: FILE_BINARY,
        current_file_data: Some(vec![4]),
        ..bundle_change(TYPE_CHANGE, Some("a.bin"), Some("a.bin"))
    };
    let inline_old = BundleChange {
        original_file_data: Some(vec![1, 2, 3]),
        ..bundle_change(TYPE_MOVE_AWAY, Some("a.bin"), None)
    };
    let fail = |_: &str| -> Result<Vec<u8>, String> { panic!("no blobs needed") };
    let body = build_binary_change(&change, Some(&inline_old), &fail).unwrap();
    assert!(body.contains("literal 1\n"), "{body}");
    assert!(body.contains("literal 3\n"), "{body}");
    assert!(body.contains(&git_blob_sha1(&[1, 2, 3])), "{body}");
    let phid_old = BundleChange {
        old_binary_phid: Some("PHID-OLD".to_string()),
        ..bundle_change(TYPE_MOVE_AWAY, Some("a.bin"), None)
    };
    let seen = std::cell::RefCell::new(Vec::new());
    let loader = |phid: &str| -> Result<Vec<u8>, String> {
        seen.borrow_mut().push(phid.to_string());
        Ok(vec![9, 9])
    };
    let body = build_binary_change(&change, Some(&phid_old), &loader).unwrap();
    assert_eq!(*seen.borrow(), vec!["PHID-OLD".to_string()]);
    assert!(body.contains("literal 2\n"), "{body}");
}

#[test]
fn t1170_collect_binary_sources_maps_away_destinations() {
    // Only binary away changes map (patch.rs:1949-1950); text aways and
    // binary heres contribute nothing.
    let away = BundleChange {
        file_type: FILE_BINARY,
        away_paths: vec!["d1".to_string(), "d2".to_string()],
        ..bundle_change(TYPE_MOVE_AWAY, Some("a.bin"), None)
    };
    let map = collect_binary_sources(&[
        away,
        bundle_change(TYPE_MOVE_AWAY, Some("t.txt"), None),
        BundleChange {
            file_type: FILE_BINARY,
            ..bundle_change(TYPE_CHANGE, Some("b.bin"), Some("b.bin"))
        },
    ]);
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("d1"));
    assert!(map.contains_key("d2"));
}

#[test]
fn t1170_render_counts_backslash_and_odd_lines() {
    // `\` lines adjust the split counts (patch.rs:2093) and unprefixed
    // lines pass through the catch-all (patch.rs:2096); all survive.
    let mut changes = vec![BundleChange {
        hunks: vec![parsed_hunk(
            1,
            1,
            1,
            1,
            "-a\n+b\n\\ No newline at end of file\n? odd\n",
        )],
        ..bundle_change(TYPE_CHANGE, Some("f.txt"), Some("f.txt"))
    }];
    let out = render_git_patch(&mut changes).unwrap();
    for line in ["-a\n", "+b\n", "\\ No newline at end of file\n", "? odd\n"] {
        assert!(out.contains(line), "{out}");
    }
}

#[test]
fn t1170_render_with_blobs_skips_and_terminates() {
    // The blobs renderer skips directories, move-aways, and pure
    // copy-aways (patch.rs:2168, :2171, :2184-2187), emits headers
    // alone for bodyless changes (patch.rs:2193), and terminates a
    // `\r`-ended body (patch.rs:2200).
    let loader = |_: &str| -> Result<Vec<u8>, String> { panic!("no blobs needed") };
    let mut changes = vec![
        BundleChange {
            file_type: FILE_DIRECTORY,
            ..bundle_change(TYPE_ADD, None, Some("dir"))
        },
        bundle_change(TYPE_MOVE_AWAY, Some("gone.txt"), None),
        bundle_change(TYPE_COPY_AWAY, Some("same.txt"), Some("same.txt")),
        bundle_change(TYPE_CHANGE, Some("plain.txt"), Some("plain.txt")),
        BundleChange {
            hunks: vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\r")],
            ..bundle_change(TYPE_CHANGE, Some("cr.txt"), Some("cr.txt"))
        },
    ];
    let out = render_git_patch_with_blobs(&mut changes, &loader).unwrap();
    assert!(!out.contains("gone.txt"), "{out}");
    assert!(!out.contains("same.txt"), "{out}");
    assert!(!out.contains("a/dir "), "{out}");
    assert!(
        out.contains("diff --git a/plain.txt b/plain.txt\n"),
        "{out}"
    );
    assert!(out.contains("+b\r\n"), "{out}");
}

#[test]
fn t1170_render_text_terminates_cr_ended_body() {
    // Same `\r`-ended body through the text renderer (patch.rs:2261).
    let mut changes = vec![BundleChange {
        hunks: vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\r")],
        ..bundle_change(TYPE_CHANGE, Some("cr.txt"), Some("cr.txt"))
    }];
    let out = render_git_patch(&mut changes).unwrap();
    assert!(out.contains("+b\r\n"), "{out}");
}

#[test]
fn t1170_porcelain_skips_empty_entries() {
    // Empty `-z` records are skipped (patch.rs:2667) without disturbing
    // the real entries around them.
    let status = parse_porcelain_v2(b"? a.txt\0\0").unwrap();
    assert_eq!(status.untracked, vec!["a.txt".to_string()]);
}

#[test]
fn t1170_hg_tree_status_buckets_via_fixture() {
    // `U` lines become conflicts before parsing (patch.rs:2764); every
    // tracked path also counts as uncommitted (patch.rs:2770-2772);
    // missing, uncommitted, and untracked sort into buckets
    // (patch.rs:2777-2794).
    let dir = temp_dir("t1170_hg_status");
    let hg = fake_hg_backend(&dir);
    std::fs::write(
        dir.join(".fakehg-status"),
        "U conflict.txt\n! gone.txt\nM mod.txt\n? new.txt\nA add.txt\n",
    )
    .unwrap();
    let status = hg_tree_status(&dir, &hg, &["status"]).unwrap();
    assert_eq!(status.conflicts, vec!["conflict.txt".to_string()]);
    assert_eq!(status.missing, vec!["gone.txt".to_string()]);
    assert_eq!(
        status.uncommitted,
        vec![
            "add.txt".to_string(),
            "gone.txt".to_string(),
            "mod.txt".to_string()
        ]
    );
    assert_eq!(status.untracked, vec!["new.txt".to_string()]);
    assert!(status.unstaged.is_empty());
    assert!(status.externals.is_empty());
}

#[test]
fn t1170_svn_entry_without_status_is_ignored() {
    // Entries without a `wc-status` tag parse to `None` and are
    // skipped (patch.rs:2872).
    let status = parse_svn_status_xml("<status><entry path=\"x\"></entry></status>").unwrap();
    assert_eq!(status, TreeStatus::default());
}

#[test]
fn t1170_push_svn_bucket_ignores_unknown() {
    // Unknown bucket names hit the catch-all (patch.rs:2899) and change
    // nothing.
    let mut status = TreeStatus::default();
    push_svn_bucket(&mut status, "bogus", "p.txt");
    assert_eq!(status, TreeStatus::default());
}

#[test]
fn t1170_svn_tree_status_via_fixture() {
    // The `svn --xml status` runner parses through the fixture
    // (patch.rs:2925) into untracked and missing buckets.
    let dir = temp_dir("t1170_svn_status");
    let svn = fake_svn_backend(&dir);
    std::fs::write(
        dir.join(".fakesvn-status"),
        "<status><entry path=\"n.txt\"><wc-status props=\"none\" item=\"unversioned\"></wc-status></entry>\
         <entry path=\"g.txt\"><wc-status props=\"none\" item=\"missing\"></wc-status></entry></status>",
    )
    .unwrap();
    let status = svn_tree_status(&dir, &svn, &["--xml", "status"]).unwrap();
    assert_eq!(status.untracked, vec!["n.txt".to_string()]);
    assert_eq!(status.missing, vec!["g.txt".to_string()]);
}

#[test]
fn t1170_clean_check_hg_conflicts_and_missing_fail() {
    // The hg clean check (patch.rs:3023) fails verbatim on conflicts
    // and missing files (patch.rs:3057-3070).
    let dir = temp_dir("t1170_hg_fatal");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    std::fs::write(dir.join(".fakehg-status"), "U c.txt\n").unwrap();
    let mut io = Io::canned(&[]);
    let err = require_clean_tree_ansi(&dir, Vcs::Hg, &backends, &mut io, false, false).unwrap_err();
    assert!(err.contains("merge conflicts"), "{err}");
    assert!(err.contains("c.txt"), "{err}");
    std::fs::write(dir.join(".fakehg-status"), "! m.txt\n").unwrap();
    let mut io = Io::canned(&[]);
    let err = require_clean_tree_ansi(&dir, Vcs::Hg, &backends, &mut io, false, false).unwrap_err();
    assert!(err.contains("Missing files"), "{err}");
    assert!(err.contains("m.txt"), "{err}");
}

#[test]
fn t1170_clean_check_svn_incomplete_and_externals() {
    // The svn clean check (patch.rs:3024) fails verbatim on incomplete
    // directories (patch.rs:3048-3054); externals clear silently under
    // svn instead of prompting.
    let dir = temp_dir("t1170_svn_fatal");
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    std::fs::write(
        dir.join(".fakesvn-status"),
        "<status><entry path=\"half\"><wc-status props=\"none\" item=\"incomplete\"></wc-status></entry></status>",
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    let err =
        require_clean_tree_ansi(&dir, Vcs::Svn, &backends, &mut io, false, false).unwrap_err();
    assert!(err.contains("incompletely checked out"), "{err}");
    assert!(err.contains("half"), "{err}");
    std::fs::write(
        dir.join(".fakesvn-status"),
        "<status><entry path=\"ext\"><wc-status props=\"none\" item=\"external\"></wc-status></entry></status>",
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        require_clean_tree_ansi(&dir, Vcs::Svn, &backends, &mut io, false, false).unwrap(),
        StashState::Clean
    );
    assert!(!io.out.stdout.contains("submodule(s)"), "{}", io.out.stdout);
}

#[test]
fn t1170_confirm_tree_externals_prompts() {
    // Dirty externals list and prompt (patch.rs:3080-3090): declining
    // aborts, accepting continues.
    let status = TreeStatus {
        externals: vec!["sub".to_string()],
        ..TreeStatus::default()
    };
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        confirm_tree_externals(&mut io, &status).unwrap_err(),
        land::ABORT
    );
    assert!(
        io.out.stdout.contains("1 submodule(s) have uncommitted"),
        "{}",
        io.out.stdout
    );
    let mut io = Io::canned(&["y"]);
    assert_eq!(confirm_tree_externals(&mut io, &status), Ok(()));
}

#[test]
fn t1170_clean_check_untracked_place_names() {
    // The untracked prompt names the ignore file per VCS
    // (patch.rs:3107-3108).
    let dir = temp_dir("t1170_place_hg");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    std::fs::write(dir.join(".fakehg-status"), "? n.txt\n").unwrap();
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        require_clean_tree_ansi(&dir, Vcs::Hg, &backends, &mut io, false, false).unwrap(),
        StashState::Clean
    );
    assert!(io.out.stdout.contains(".hgignore"), "{}", io.out.stdout);
    let dir = temp_dir("t1170_place_svn");
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    std::fs::write(
        dir.join(".fakesvn-status"),
        "<status><entry path=\"n.txt\"><wc-status props=\"none\" item=\"unversioned\"></wc-status></entry></status>",
    )
    .unwrap();
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        require_clean_tree_ansi(&dir, Vcs::Svn, &backends, &mut io, false, false).unwrap(),
        StashState::Clean
    );
    assert!(io.out.stdout.contains("svn:ignore"), "{}", io.out.stdout);
}

#[test]
fn t1170_clean_check_lists_uncommitted_only() {
    // A staged-only change prints the uncommitted list without the
    // unstaged header (patch.rs:3541-3544), then the decline errors.
    let repo = make_repo("t1170_uncommitted_only");
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    git(&repo, &["add", "file.txt"]);
    let mut io = Io::canned(&["n"]);
    let err =
        require_clean_tree(&repo, Vcs::Git, &Backends::default(), &mut io, false).unwrap_err();
    assert_eq!(
        err,
        "You can not continue with uncommitted changes. Commit or discard them before proceeding."
    );
    assert!(
        io.out
            .stdout
            .contains("Uncommitted changes in working copy:\n    file.txt"),
        "{}",
        io.out.stdout
    );
    assert!(
        !io.out.stdout.contains("Unstaged changes"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_resolve_uncommitted_hg_and_svn_paths() {
    // The yes-path stages through the hg backend (patch.rs:3471) and
    // commits it (patch.rs:3505); under svn it stages (patch.rs:3472)
    // and reports staged-but-uncommitted (patch.rs:3507).
    let dir = temp_dir("t1170_resolve_hg");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    let mut io = Io::canned(&["y"]);
    let status = TreeStatus {
        unstaged: vec!["f.txt".to_string()],
        ..TreeStatus::default()
    };
    let edit = |_: &str| Ok("hello\n".to_string());
    assert_eq!(
        resolve_uncommitted_tree(
            &dir,
            Vcs::Hg,
            &backends,
            &mut io,
            &status,
            "DESC",
            None,
            &edit
        )
        .unwrap(),
        StashState::Clean
    );
    let log = std::fs::read_to_string(dir.join(".fakehg.log")).unwrap();
    assert!(log.contains("addremove"), "{log}");
    assert!(log.contains("commit"), "{log}");
    let dir = temp_dir("t1170_resolve_svn");
    std::fs::write(dir.join("f.txt"), "x\n").unwrap();
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        resolve_uncommitted_tree(
            &dir,
            Vcs::Svn,
            &backends,
            &mut io,
            &status,
            "DESC",
            None,
            &edit
        )
        .unwrap(),
        StashState::Clean
    );
    let log = std::fs::read_to_string(dir.join(".fakesvn.log")).unwrap();
    assert!(log.contains("add -- f.txt"), "{log}");
}

#[test]
fn t1170_commit_yes_path_svn_amend_is_noop() {
    // Amend under svn is a structural no-op (patch.rs:3492): no editor,
    // no commit, success.
    let dir = temp_dir("t1170_svn_amend");
    let edit = |_: &str| -> Result<String, String> { panic!("must not edit") };
    assert_eq!(
        commit_yes_path_changes(
            &dir,
            Vcs::Svn,
            &Backends::default(),
            true,
            Some("m"),
            "t",
            &edit
        ),
        Ok(())
    );
}

#[test]
fn t1170_commit_yes_path_strips_trailing_comments() {
    // The yes-path strips trailing `#` comment lines before
    // committing, so a message with a comment tail commits the
    // stripped text.
    let repo = make_repo("t1170_empty_msg");
    git(&repo, &["config", "user.name", "Patch Test"]);
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    git(&repo, &["add", "file.txt"]);
    let edit = |_: &str| Ok("wip save\n# Enter a commit message.\n".to_string());
    commit_yes_path_changes(
        &repo,
        Vcs::Git,
        &Backends::default(),
        false,
        None,
        "template",
        &edit,
    )
    .unwrap();
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]), "wip save");
}

#[test]
fn t1170_patch_add_to_commit_empty_is_noop() {
    // Nothing unstaged means nothing to stage (patch.rs:3466).
    let dir = temp_dir("t1170_add_empty");
    assert_eq!(
        patch_add_to_commit(&dir, Vcs::Git, &Backends::default(), &[]),
        Ok(())
    );
}

#[test]
fn t1170_update_working_copy_hg_and_svn() {
    // The hg and svn update arms (patch.rs:3602, :3605) print the same
    // wrapper lines around the backend `up`.
    let dir = temp_dir("t1170_update_hg");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    let mut io = Io::buffered();
    update_working_copy(&dir, Vcs::Hg, &backends, &mut io).unwrap();
    assert_eq!(io.out.stdout, "Updating working copy...\nDone.\n");
    let dir = temp_dir("t1170_update_svn");
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    let mut io = Io::buffered();
    update_working_copy(&dir, Vcs::Svn, &backends, &mut io).unwrap();
    assert_eq!(io.out.stdout, "Updating working copy...\nDone.\n");
}

#[test]
fn t1170_amend_declines_when_history_immutable() {
    // Immutable history declines before any HEAD lookup
    // (patch.rs:3378). The project pin beats user config.
    let repo = make_repo("t1170_immutable");
    std::fs::write(repo.join(".arcconfig"), "{\"history.immutable\": true}").unwrap();
    assert_eq!(
        calculate_patch_should_amend(&repo, Vcs::Git, &Backends::default(), None).unwrap(),
        (false, None)
    );
}

#[test]
fn t1170_amend_declines_without_head() {
    // No commits means no HEAD identity (patch.rs:3188), so amend
    // declines (patch.rs:3381).
    let dir = temp_dir("t1170_nohead");
    git(&dir, &["init", "-b", "master"]);
    assert_eq!(
        calculate_patch_should_amend(&dir, Vcs::Git, &Backends::default(), None).unwrap(),
        (false, None)
    );
}

#[test]
fn t1170_amend_declines_git_svn_message() {
    // A HEAD carrying a `git-svn-id` footer declines (patch.rs:3385).
    let repo = make_repo("t1170_gitsvn");
    std::fs::write(repo.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    git(
        &repo,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "x\n\ngit-svn-id: https://svn.example.test/r@42 uuid-1234",
        ],
    );
    assert_eq!(
        calculate_patch_should_amend(&repo, Vcs::Git, &Backends::default(), None).unwrap(),
        (false, None)
    );
}

#[test]
fn t1170_amend_declines_author_mismatch() {
    // A HEAD committed by someone else declines (patch.rs:3388).
    let repo = make_repo("t1170_author_mismatch");
    std::fs::write(repo.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    git(&repo, &["config", "user.name", "Someone Else"]);
    assert_eq!(
        calculate_patch_should_amend(&repo, Vcs::Git, &Backends::default(), None).unwrap(),
        (false, None)
    );
}

#[test]
fn t1170_amend_accepts_unpublished_matching_head() {
    // A matching unpublished HEAD with no repository query amends: the
    // published check skips without a query (patch.rs:3453) instead of
    // calling Conduit (the stub panics if called).
    let repo = make_repo("t1170_amend_true");
    std::fs::write(
        repo.join(".arcconfig"),
        "{\"history.immutable\": false, \"repository.callsign\": \"\"}",
    )
    .unwrap();
    let call = no_repository_query;
    let (amend, message) =
        calculate_patch_should_amend(&repo, Vcs::Git, &Backends::default(), Some(&call)).unwrap();
    assert!(amend);
    assert!(
        message.as_ref().is_some_and(|text| text.contains("base")),
        "{message:?}"
    );
}

#[test]
fn t1170_amend_support_and_head_lookup_by_vcs() {
    // Amend support is per-VCS (patch.rs:3406-3407) and HEAD lookup
    // dispatches per-VCS (patch.rs:3420).
    let repo = make_repo("t1170_supports");
    assert!(patch_vcs_supports_amend(
        &repo,
        Vcs::Git,
        &Backends::default()
    ));
    assert!(!patch_vcs_supports_amend(
        &repo,
        Vcs::Svn,
        &Backends::default()
    ));
    let dir = temp_dir("t1170_supports_hg");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    assert!(patch_vcs_supports_amend(&dir, Vcs::Hg, &backends));
    std::fs::write(dir.join(".fakehg-noamend"), "").unwrap();
    assert!(!patch_vcs_supports_amend(&dir, Vcs::Hg, &backends));
    let empty = temp_dir("t1170_head_empty");
    git(&empty, &["init", "-b", "master"]);
    assert!(
        patch_head_info_for(&empty, Vcs::Git, &Backends::default())
            .unwrap()
            .is_none()
    );
    assert!(
        patch_head_info_for(&dir, Vcs::Hg, &backends)
            .unwrap()
            .is_none()
    );
}

#[test]
fn t1170_head_author_matches_hg_backend() {
    // The hg working-copy author comes from the backend showconfig
    // (patch.rs:3434): a matching HEAD author passes, others fail.
    let dir = temp_dir("t1170_hg_author");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    let matching = HeadInfo {
        hash: "n".to_string(),
        author: "Test User".to_string(),
        message: "m".to_string(),
    };
    assert!(patch_head_author_matches(&dir, Vcs::Hg, &backends, &matching).unwrap());
    let other = HeadInfo {
        author: "Other".to_string(),
        ..matching
    };
    assert!(!patch_head_author_matches(&dir, Vcs::Hg, &backends, &other).unwrap());
}

#[test]
fn t1170_history_immutable_defaults_on_config_failure() {
    // An unreadable project config falls back to the VCS default
    // (patch.rs:3246): mutable for git, immutable otherwise.
    let repo = make_repo("t1170_immutable_broken");
    std::fs::write(repo.join(".arcconfig"), "{not json").unwrap();
    assert!(!patch_history_immutable(&repo, Vcs::Git));
    assert!(patch_history_immutable(&repo, Vcs::Hg));
}

#[test]
fn t1170_repository_encoding_without_query_is_utf8() {
    // With no callsign and no remote the query is `None` (patch.rs:3270,
    // :3280), so the encoding defaults to UTF-8 without calling
    // Conduit (patch.rs:3301; the stub panics if called).
    let repo = make_repo("t1170_enc_none");
    std::fs::write(repo.join(".arcconfig"), "{\"repository.callsign\": \"\"}").unwrap();
    let call = no_repository_query;
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Git, &Backends::default(), &call).unwrap(),
        Some("UTF-8".to_string())
    );
}

#[test]
fn t1170_repository_query_uses_hg_remote() {
    // Without a callsign the hg remote URI becomes the query
    // (patch.rs:3271, :3280).
    let repo = make_repo("t1170_enc_hg");
    std::fs::write(repo.join(".arcconfig"), "{\"repository.callsign\": \"\"}").unwrap();
    let backends = Backends {
        hg: fake_hg_backend(&repo),
        svn: svn::SvnCommand::system(),
    };
    let seen = std::cell::RefCell::new(serde_json::Value::Null);
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "repository.query");
        *seen.borrow_mut() = params.clone();
        Ok(serde_json::json!([]))
    };
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Hg, &backends, &call).unwrap(),
        Some("UTF-8".to_string())
    );
    let seen_text = seen.borrow().to_string();
    assert!(
        seen_text.contains("https://hg.example.test/repo"),
        "{seen_text}"
    );
}

#[test]
fn t1170_repository_query_prefers_svn_uuid() {
    // The svn UUID becomes a `uuids` query when known
    // (patch.rs:3273-3274, :3277-3278), else the root becomes a
    // `remoteURIs` query (patch.rs:3280).
    let repo = make_repo("t1170_enc_svn");
    std::fs::write(repo.join(".arcconfig"), "{\"repository.callsign\": \"\"}").unwrap();
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&repo),
    };
    std::fs::write(
        repo.join(".fakesvn-info"),
        "Path: /\nRepository Root: https://svn.example.test/repo\nRepository UUID: uuid-9\n",
    )
    .unwrap();
    let seen = std::cell::RefCell::new(serde_json::Value::Null);
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "repository.query");
        *seen.borrow_mut() = params.clone();
        Ok(serde_json::json!([]))
    };
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Svn, &backends, &call).unwrap(),
        Some("UTF-8".to_string())
    );
    let seen_text = seen.borrow().to_string();
    assert!(seen_text.contains("\"uuids\""), "{seen_text}");
    assert!(seen_text.contains("uuid-9"), "{seen_text}");
    std::fs::write(
        repo.join(".fakesvn-info"),
        "Path: /\nRepository Root: https://svn.example.test/repo\n",
    )
    .unwrap();
    assert_eq!(
        patch_repository_encoding(&repo, Vcs::Svn, &backends, &call).unwrap(),
        Some("UTF-8".to_string())
    );
    let seen_text = seen.borrow().to_string();
    assert!(seen_text.contains("remoteURIs"), "{seen_text}");
}

#[test]
fn t1170_commit_published_probes() {
    // Zero or ambiguous repository matches mean unpublished
    // (patch.rs:3341); without a callsign the commit name uses the
    // `R{id}:{hash}` shape (patch.rs:3347-3348).
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "repository.query");
        Ok(serde_json::json!([]))
    };
    assert!(
        !patch_commit_published(&call, &serde_json::json!({"callsigns": ["X"]}), "abc").unwrap()
    );
    let seen = std::cell::RefCell::new(String::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == "repository.query" {
            return Ok(serde_json::json!([{"id": 7}]));
        }
        assert_eq!(method, "diffusion.querycommits");
        *seen.borrow_mut() = params.to_string();
        Ok(serde_json::json!({"identifierMap": {"R7:abc": {"x": 1}}}))
    };
    assert!(patch_commit_published(&call, &serde_json::json!({}), "abc").unwrap());
    assert!(seen.borrow().contains("R7:abc"), "{}", seen.borrow());
}

#[test]
fn t1170_load_dependency_nodes_skips_bad_rows() {
    // Rows without a PHID are skipped (patch.rs:3720); unseen
    // transitive dependencies queue for the next level (patch.rs:3734);
    // locally-present commits keep empty edges.
    let start = serde_json::json!([{
        "phid": "P1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PA", "PB"]}
    }]);
    let level = serde_json::json!([
        {"noid": true},
        {"phid": "PA", "id": 2, "auxiliary": {"phabricator:depends-on": ["PC"]}},
        {"phid": "PB", "id": 3, "hashes": {"gtcm": "LOCAL"},
         "auxiliary": {"phabricator:depends-on": ["PD"]}},
    ]);
    let graph = |_: &str, params: &serde_json::Value| {
        let text = params.to_string();
        if text.contains("\"ids\"") {
            Ok(start.clone())
        } else if text.contains("PC") {
            Ok(serde_json::json!([]))
        } else {
            Ok(level.clone())
        }
    };
    let (nodes, start_phid) =
        load_dependency_nodes(&graph, 1, &|hash| hash == Some("LOCAL"), "gtcm").unwrap();
    assert_eq!(start_phid, "P1");
    assert_eq!(nodes.len(), 3);
    let local = nodes.iter().find(|node| node.phid == "PB").unwrap();
    assert!(local.depends_on.is_empty());
    let parent = nodes.iter().find(|node| node.phid == "PA").unwrap();
    assert_eq!(parent.depends_on, vec!["PC".to_string()]);
}

#[test]
fn t1170_plan_children_empty_and_error() {
    // An unknown start revision plans no children (patch.rs:5872);
    // Conduit failures propagate (patch.rs:5870).
    let dir = temp_dir("t1170_plan_empty");
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    let empty = |_: &str, _: &serde_json::Value| Ok(serde_json::json!([]));
    assert!(
        plan_dependency_children(
            &dir,
            Vcs::Git,
            &bundle,
            &Options::default(),
            &mut io,
            &empty
        )
        .unwrap()
        .is_empty()
    );
    let failing = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("boom".to_string())
    };
    assert_eq!(
        plan_dependency_children(
            &dir,
            Vcs::Git,
            &bundle,
            &Options::default(),
            &mut io,
            &failing
        )
        .unwrap_err(),
        "boom"
    );
}

#[test]
fn t1170_build_children_skips_unresolved() {
    // Dependencies without a resolved monogram are skipped
    // (patch.rs:5970), so only resolvable children plan.
    let dir = temp_dir("t1170_plan_skip");
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let start = serde_json::json!([{
        "phid": "P1", "id": 1,
        "auxiliary": {"phabricator:depends-on": ["PA"]}
    }]);
    let level = serde_json::json!([{"phid": "PA", "id": 2, "auxiliary": {}}]);
    let monograms = serde_json::json!([{"phid": "P1", "id": 1}]);
    let graph = |_: &str, params: &serde_json::Value| {
        let text = params.to_string();
        if text.contains("\"ids\"") {
            Ok(start.clone())
        } else if text.contains("P1") {
            Ok(monograms.clone())
        } else {
            Ok(level.clone())
        }
    };
    let mut io = Io::buffered();
    let children = plan_dependency_children(
        &dir,
        Vcs::Git,
        &bundle,
        &Options::default(),
        &mut io,
        &graph,
    )
    .unwrap();
    assert!(children.is_empty());
}

#[test]
fn t1170_git_create_branch_fails_when_names_taken() {
    // Exhausting the `arcpatch{,_1,_2,_3}` names propagates the
    // verbatim error (patch.rs:4018) after logging each collision.
    let repo = make_repo("t1170_branch_taken");
    for name in ["arcpatch", "arcpatch_1", "arcpatch_2", "arcpatch_3"] {
        git(&repo, &["branch", name]);
    }
    let mut io = Io::buffered();
    assert_eq!(
        git_create_branch(&repo, &Bundle::default(), false, &mut io).unwrap_err(),
        "Arc was unable to automagically make a name for this patch. Please clean up your \
         working copy and try again."
    );
    assert!(
        io.out.stdout.contains("already exists; trying a new name"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_git_submodule_update_fails_outside_repo() {
    // A non-zero `git submodule update` fails with the stderr tail
    // (patch.rs:4035-4038).
    let dir = temp_dir("t1170_submodule_outside");
    let mut io = Io::buffered();
    let err = git_submodule_update(&dir, &mut io).unwrap_err();
    assert!(err.starts_with("git submodule update failed:"), "{err}");
}

#[test]
fn t1170_git_commit_patch_fails_when_clean() {
    // Nothing to commit fails the commit (patch.rs:4114-4117).
    let repo = make_repo("t1170_commit_clean");
    let err = git_commit_patch(&repo, &Bundle::default(), "msg").unwrap_err();
    assert!(err.starts_with("git commit failed:"), "{err}");
}

#[test]
fn t1170_cherry_pick_reports_submodule_failure() {
    // A cherry-pick that lands but breaks submodule update reports the
    // update error (patch.rs:4186) with the failure banner, and still
    // deletes the temp branch. The work branch carries a real gitlink
    // plus a bogus URL, so only the post-pick update fails.
    let repo = make_repo("t1170_cherry_sub");
    let origin = temp_dir("t1170_cherry_origin");
    git(&origin, &["init", "-b", "master"]);
    git(&origin, &["config", "user.email", "patch@test"]);
    git(&origin, &["config", "user.name", "Patch Test"]);
    std::fs::write(origin.join("x.txt"), "x\n").unwrap();
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-m", "sub"]);
    git(&repo, &["checkout", "-b", "work"]);
    git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &origin.to_string_lossy(),
            "x",
        ],
    );
    let modules = std::fs::read_to_string(repo.join(".gitmodules")).unwrap();
    std::fs::write(
        repo.join(".gitmodules"),
        modules.replace(
            &origin.to_string_lossy().into_owned(),
            "file:///nonexistent-submodule-xyz",
        ),
    )
    .unwrap();
    git(&repo, &["add", ".gitmodules"]);
    git(&repo, &["commit", "-m", "add submodule"]);
    git(&repo, &["checkout", "master"]);
    // Scrub the branch-independent registration so the post-pick
    // update clones from the bogus URL instead of the stale config.
    git(&repo, &["config", "--remove-section", "submodule.x"]);
    let _ = std::fs::remove_dir_all(repo.join("x"));
    let _ = std::fs::remove_dir_all(repo.join(".git/modules/x"));
    let mut io = Io::buffered();
    let err = git_cherry_pick_temp_ansi(&repo, &mut io, "master", "work", false).unwrap_err();
    assert!(err.starts_with("git submodule update failed:"), "{err}");
    assert!(
        io.out.stdout.contains("Cherry Pick Failed!"),
        "{}",
        io.out.stdout
    );
    assert!(git(&repo, &["branch", "--list", "work"]).is_empty());
}

#[test]
#[cfg(unix)]
fn t1170_hg_tip_info_paths() {
    // The working-parent lookup parses the `\x01`-separated template
    // (patch.rs:3204-3233); failures, missing separators, and empty
    // nodes all read as no commits.
    let dir = temp_dir("t1170_tip");
    let hg = stub_hg(
        &dir,
        "stubhg-tip.sh",
        "#!/bin/sh\necho \"$*\" >> .stubhg.log\n\
         if [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         if [ \"$1\" = \"log\" ] && [ -e .stubhg-fail-log ]; then exit 1; fi\n\
         if [ \"$1\" = \"log\" ] && [ -e .stubhg-log ]; then cat .stubhg-log; exit 0; fi\n\
         exit 0\n",
    );
    std::fs::write(
        dir.join(".stubhg-log"),
        b"abc123\x01Author Name <a@t>\x01hello desc",
    )
    .unwrap();
    let tip = hg_tip_info(&dir, &hg).unwrap().unwrap();
    assert_eq!(tip.hash, "abc123");
    assert_eq!(tip.author, "Author Name");
    assert_eq!(tip.message, "hello desc");
    std::fs::write(dir.join(".stubhg-fail-log"), "").unwrap();
    assert!(hg_tip_info(&dir, &hg).unwrap().is_none());
    std::fs::remove_file(dir.join(".stubhg-fail-log")).unwrap();
    std::fs::write(dir.join(".stubhg-log"), "plain-node").unwrap();
    assert!(hg_tip_info(&dir, &hg).unwrap().is_none());
    std::fs::write(dir.join(".stubhg-log"), b"\x01a@t\x01d").unwrap();
    assert!(hg_tip_info(&dir, &hg).unwrap().is_none());
}

#[test]
#[cfg(unix)]
fn t1170_hg_amend_commit_path() {
    // A matching unpublished hg tip amends through the backend
    // (patch.rs:3491) without opening the editor. The project pin
    // keeps history mutable regardless of user config.
    let dir = make_repo("t1170_hg_amend");
    std::fs::write(dir.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    let hg = stub_hg(
        &dir,
        "stubhg-amend.sh",
        "#!/bin/sh\necho \"$*\" >> .stubhg.log\n\
         if [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         case \"$1\" in\n\
         log) printf 'abc123\\001Amender <a@t>\\001plain message' ;;\n\
         showconfig) printf 'Amender <a@t>' ;;\n\
         help) printf 'hg commit [OPTION]...\\n  --amend flag\\n' ;;\n\
         *) exit 0 ;;\nesac\nexit 0\n",
    );
    let backends = Backends {
        hg,
        svn: svn::SvnCommand::system(),
    };
    let mut io = Io::canned(&["y"]);
    let status = TreeStatus {
        unstaged: vec!["f.txt".to_string()],
        ..TreeStatus::default()
    };
    let edit = |_: &str| -> Result<String, String> { panic!("amend never edits") };
    assert_eq!(
        resolve_uncommitted_tree(
            &dir,
            Vcs::Hg,
            &backends,
            &mut io,
            &status,
            "DESC",
            None,
            &edit
        )
        .unwrap(),
        StashState::Clean
    );
    assert!(
        io.out.stdout.contains("amend these 1 change(s)"),
        "{}",
        io.out.stdout
    );
    let log = std::fs::read_to_string(dir.join(".stubhg.log")).unwrap();
    assert!(log.contains("--amend"), "{log}");
}

#[test]
#[cfg(unix)]
fn t1170_hg_create_bookmark_paths() {
    // Bookmark creation updates to the base first when asked
    // (patch.rs:4398-4407) and reports the new bookmark
    // (patch.rs:4410-4411); update failures propagate.
    let dir = temp_dir("t1170_bookmark");
    let hg = stub_hg(
        &dir,
        "stubhg-bookmark.sh",
        "#!/bin/sh\necho \"$*\" >> .stubhg.log\n\
         if [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         case \"$1\" in\n\
         log) case \"$*\" in *arcpatch*) exit 1 ;; *) printf 'NODE999' ;; esac ;;\n\
         bookmark) exit 0 ;;\n\
         update) if [ -e .stubhg-fail-update ]; then echo update exploded 1>&2; exit 1; fi\n\
         echo moved ;; \n*) exit 0 ;;\nesac\n",
    );
    let mut io = Io::buffered();
    assert_eq!(
        hg_create_bookmark(&dir, &hg, &Bundle::default(), false, &mut io).unwrap(),
        "arcpatch"
    );
    assert!(
        io.out
            .stdout
            .contains("Created and checked out bookmark arcpatch."),
        "{}",
        io.out.stdout
    );
    let bundle = Bundle {
        base_revision: Some(".".to_string()),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    assert_eq!(
        hg_create_bookmark(&dir, &hg, &bundle, true, &mut io).unwrap(),
        "arcpatch"
    );
    assert!(
        io.out
            .stdout
            .contains("Updating to the revision's base commit"),
        "{}",
        io.out.stdout
    );
    assert!(io.out.stdout.contains("moved"), "{}", io.out.stdout);
    std::fs::write(dir.join(".stubhg-fail-update"), "").unwrap();
    let mut io = Io::buffered();
    let err = hg_create_bookmark(&dir, &hg, &bundle, true, &mut io).unwrap_err();
    assert!(err.contains("hg update failed: update exploded"), "{err}");
}

#[test]
#[cfg(unix)]
fn t1170_hg_create_bookmark_names_taken() {
    // Exhausting the bookmark names propagates the verbatim error
    // (patch.rs:4396) after logging each collision.
    let dir = temp_dir("t1170_bookmark_taken");
    let hg = stub_hg(
        &dir,
        "stubhg-taken.sh",
        "#!/bin/sh\necho \"$*\" >> .stubhg.log\n\
         if [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         if [ \"$1\" = \"log\" ]; then printf 'TAKEN'; exit 0; fi\n\
         exit 0\n",
    );
    let mut io = Io::buffered();
    assert_eq!(
        hg_create_bookmark(&dir, &hg, &Bundle::default(), false, &mut io).unwrap_err(),
        "Arc was unable to automagically make a name for this patch. Please clean up your \
         working copy and try again."
    );
    assert!(
        io.out
            .stdout
            .contains("Bookmark name arcpatch-3 already exists"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_unquote_rejects_bad_octal() {
    // An octal escape that overflows a byte is dropped
    // (patch.rs:4805).
    assert_eq!(unquote_git_path("\"a\\8\""), "a");
}

#[test]
fn t1170_hg_import_failure_reports_banner() {
    // A failed import prints the patch-failed banner (patch.rs:4487)
    // and propagates the trimmed hg output (patch.rs:4492-4496).
    let dir = temp_dir("t1170_import_fail");
    let hg = fake_hg_backend(&dir);
    std::fs::write(dir.join(".fakehg-fail-import"), "").unwrap();
    let bundle = Bundle {
        text: "diff --git a/f b/f\n".to_string(),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    assert_eq!(
        hg_import_patch_ansi(&dir, &hg, &bundle, &mut io, false).unwrap_err(),
        ""
    );
    assert!(io.out.stdout.contains("Patch Failed!"), "{}", io.out.stdout);
}

#[test]
#[cfg(unix)]
fn t1170_hg_import_case_warning() {
    // A `case-folding collision` stderr adds the WARNING block
    // (patch.rs:4489-4491) after the banner.
    let dir = temp_dir("t1170_import_case");
    let hg = stub_hg(
        &dir,
        "stubhg-import.sh",
        "#!/bin/sh\nif [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         echo 'oops case-folding collision xyz' 1>&2\nexit 1\n",
    );
    let bundle = Bundle {
        text: "diff --git a/f b/f\n".to_string(),
        ..Bundle::default()
    };
    let mut io = Io::buffered();
    let err = hg_import_patch_ansi(&dir, &hg, &bundle, &mut io, false).unwrap_err();
    assert!(err.contains("case-folding collision"), "{err}");
    assert!(io.out.stdout.contains("Patch Failed!"), "{}", io.out.stdout);
    assert!(io.out.stdout.contains("** WARNING **"), "{}", io.out.stdout);
    assert!(
        io.out.stdout.contains("case of a filename"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_hg_commit_uses_author_and_reports_failure() {
    // The full author rides as `-u` (patch.rs:4514-4515); failures
    // report the stderr tail (patch.rs:4520-4525).
    let dir = temp_dir("t1170_hg_commit");
    let hg = fake_hg_ps1_backend(&dir);
    let bundle = Bundle {
        author_name: Some("Ada".to_string()),
        author_email: Some("ada@t".to_string()),
        ..Bundle::default()
    };
    hg_commit_patch(&dir, &hg, &bundle, "msg").unwrap();
    let log = std::fs::read_to_string(dir.join(".fakehg.log")).unwrap();
    assert!(log.contains("-u"), "{log}");
    assert!(log.contains("Ada <ada@t>"), "{log}");
    std::fs::write(dir.join(".fakehg-fail-commit"), "").unwrap();
    let err = hg_commit_patch(&dir, &hg, &bundle, "msg").unwrap_err();
    assert!(err.starts_with("hg commit failed:"), "{err}");
}

#[test]
#[cfg(unix)]
fn t1170_hg_rebase_temp_aborts_on_failure() {
    // A rebase onto a diverged parent that fails aborts and reports
    // the verbatim banner (patch.rs:4563-4572).
    let dir = temp_dir("t1170_rebase");
    let hg = stub_hg(
        &dir,
        "stubhg-rebase.sh",
        "#!/bin/sh\necho \"$*\" >> .stubhg.log\n\
         if [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         case \"$1\" in\n\
         log) case \"$*\" in *orig*) printf 'AAA' ;; *) printf 'BBB' ;; esac ;;\n\
         rebase) case \"$*\" in *--abort*) exit 0 ;; *) echo boom 1>&2; exit 1 ;; esac ;;\n\
         bookmark) exit 0 ;;\n*) exit 0 ;;\nesac\n",
    );
    let err = hg_rebase_temp_ansi(&dir, &hg, "orig", "new", false).unwrap_err();
    assert!(err.contains("Rebase onto orig failed!"), "{err}");
    let log = std::fs::read_to_string(dir.join(".stubhg.log")).unwrap();
    assert!(log.contains("rebase --abort"), "{log}");
    assert!(log.contains("bookmark --delete new"), "{log}");
}

#[test]
fn t1170_sanity_check_hg_revision() {
    // The hg sanity check resolves the working revision through the
    // backend (patch.rs:3864, :3873-3878) and prompts; declining
    // aborts.
    let dir = temp_dir("t1170_sanity_hg");
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    let bundle = Bundle {
        base_revision: Some("abc".to_string()),
        ..Bundle::default()
    };
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        sanity_check(&dir, Vcs::Hg, &backends, &mut io, None, &bundle, false),
        Ok(())
    );
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        sanity_check(&dir, Vcs::Hg, &backends, &mut io, None, &bundle, false).unwrap_err(),
        land::ABORT
    );
}

#[test]
#[cfg(unix)]
fn t1170_maybe_create_branch_hg() {
    // The hg branch captures the active bookmark and creates a new
    // one (patch.rs:5810-5821).
    let dir = temp_dir("t1170_branch_hg");
    let hg = stub_hg(
        &dir,
        "stubhg-branch.sh",
        "#!/bin/sh\necho \"$*\" >> .stubhg.log\n\
         if [ \"$1\" = \"--encoding\" ]; then shift; shift; fi\n\
         case \"$1\" in\n\
         bookmarks) printf ' * active  3:aaa\\n   other  2:bbb\\n' ;;\n\
         log) case \"$*\" in *arcpatch*) exit 1 ;; *) printf 'NODE1' ;; esac ;;\n\
         bookmark) exit 0 ;;\n*) exit 0 ;;\nesac\n",
    );
    let backends = Backends {
        hg,
        svn: svn::SvnCommand::system(),
    };
    let mut io = Io::buffered();
    let state = maybe_create_branch(
        &dir,
        Vcs::Hg,
        &backends,
        &Bundle::default(),
        &Options::default(),
        &mut io,
        false,
    )
    .unwrap();
    assert_eq!(
        state,
        BranchState {
            original: Some("active".to_string()),
            new: Some("arcpatch".to_string()),
        }
    );
}

#[test]
fn t1170_split_git_header_quoted_and_malformed() {
    // The `diff --git` fallback honors C-quoting (patch.rs:4863-4868);
    // a header without two paths yields nothing (patch.rs:4884).
    let changes = parse_git_patch_for_svn(
        "diff --git \"a/a b\" \"b/a b\"\nold mode 100644\nnew mode 100755\n",
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].old_path.as_deref(), Some("a b"));
    assert_eq!(changes[0].current_path.as_deref(), Some("a b"));
    let changes = parse_git_patch_for_svn("diff --git loner\nold mode 1\nnew mode 2\n");
    assert_eq!(changes.len(), 1);
    assert!(changes[0].old_path.is_none());
    assert!(changes[0].current_path.is_none());
}

#[test]
fn t1170_parse_svn_copy_section() {
    // `copy from`/`copy to` headers parse (patch.rs:4989-4992) and
    // classify the change as a copy-here with header paths
    // (patch.rs:5082-5090).
    let changes = parse_git_patch_for_svn(
        "diff --git a/o.txt b/c.txt\ncopy from o.txt\ncopy to c.txt\n--- a/o.txt\n+++ b/c.txt\n@@ -1 +1 @@\n same\n",
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, TYPE_COPY_HERE);
    assert_eq!(changes[0].old_path.as_deref(), Some("o.txt"));
    assert_eq!(changes[0].current_path.as_deref(), Some("c.txt"));
}

#[test]
fn t1170_svn_create_parents_top_level_is_noop() {
    // A path with no parent needs no directories (patch.rs:5166).
    let dir = temp_dir("t1170_parents_noop");
    assert_eq!(
        svn_create_parents(&dir, &svn::SvnCommand::system(), ""),
        Ok(())
    );
}

#[test]
fn t1170_plan_svn_changes_collects_ops() {
    // Planning collects deletes for existing files (patch.rs:5346),
    // adds (patch.rs:5318-5320), copies for existing sources
    // (patch.rs:5374-5376), and propset entries for new-only
    // properties (patch.rs:5387).
    let dir = temp_dir("t1170_plan");
    std::fs::write(dir.join("del.txt"), "x").unwrap();
    std::fs::write(dir.join("src.txt"), "x").unwrap();
    let changes = vec![
        SvnChange {
            change_type: TYPE_DELETE,
            current_path: Some("del.txt".to_string()),
            ..SvnChange::default()
        },
        SvnChange {
            change_type: TYPE_ADD,
            current_path: Some("new.txt".to_string()),
            ..SvnChange::default()
        },
        SvnChange {
            change_type: TYPE_MOVE_HERE,
            old_path: Some("src.txt".to_string()),
            current_path: Some("dst.txt".to_string()),
            ..SvnChange::default()
        },
        SvnChange {
            change_type: TYPE_CHANGE,
            old_path: Some("m.txt".to_string()),
            current_path: Some("m.txt".to_string()),
            new_props: [("k".to_string(), "v".to_string())].into_iter().collect(),
            ..SvnChange::default()
        },
    ];
    let mut io = Io::canned(&[]);
    let plan = plan_svn_changes(&dir, &changes, &mut io).unwrap();
    assert_eq!(plan.deletes, vec!["del.txt".to_string()]);
    assert_eq!(plan.adds, vec!["new.txt".to_string()]);
    assert_eq!(
        plan.copies,
        vec![("src.txt".to_string(), "dst.txt".to_string())]
    );
    assert!(
        plan.propset.iter().any(|(path, key, value)| {
            path == "m.txt" && key == "k" && value.as_deref() == Some("v")
        }),
        "{:?}",
        plan.propset
    );
}

#[test]
fn t1170_plan_svn_copy_missing_declines() {
    // A move from a missing source prompts with the move verb
    // (patch.rs:5364); declining aborts (patch.rs:5371).
    let dir = temp_dir("t1170_plan_decline");
    let changes = vec![SvnChange {
        change_type: TYPE_MOVE_HERE,
        old_path: Some("missing.txt".to_string()),
        current_path: Some("dst.txt".to_string()),
        ..SvnChange::default()
    }];
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        plan_svn_changes(&dir, &changes, &mut io).unwrap_err(),
        land::ABORT
    );
    assert!(
        io.out.stdout.contains("moves 'missing.txt'"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_create_svn_parents_for_plan() {
    // Copy destinations and added paths gain parents through the
    // backend (patch.rs:5405, :5411).
    let dir = temp_dir("t1170_svn_parents");
    let svn = fake_svn_backend(&dir);
    let plan = SvnPlan {
        copies: vec![("s.txt".to_string(), "sub/d.txt".to_string())],
        adds: vec!["sub2/e.txt".to_string()],
        ..SvnPlan::default()
    };
    create_svn_parents(&dir, &svn, &plan).unwrap();
    assert!(dir.join("sub").is_dir());
    assert!(dir.join("sub2").is_dir());
    let log = std::fs::read_to_string(dir.join(".fakesvn.log")).unwrap();
    assert!(log.contains("add sub"), "{log}");
    assert!(log.contains("add sub2"), "{log}");
}

#[test]
fn t1170_run_svn_copies_deletes() {
    // Planned copies and deletes run as `svn cp`/`svn rm`
    // (patch.rs:5419-5426).
    let dir = temp_dir("t1170_svn_cprm");
    let svn = fake_svn_backend(&dir);
    let plan = SvnPlan {
        copies: vec![("s.txt".to_string(), "d.txt".to_string())],
        deletes: vec!["g.txt".to_string()],
        ..SvnPlan::default()
    };
    run_svn_copies_deletes(&dir, &svn, &plan).unwrap();
    let log = std::fs::read_to_string(dir.join(".fakesvn.log")).unwrap();
    assert!(log.contains("cp s.txt d.txt"), "{log}");
    assert!(log.contains("rm g.txt"), "{log}");
}

#[test]
fn t1170_run_svn_symlinks_by_type() {
    // Added, changed, and moved symlinks link (patch.rs:5440,
    // :5445); other types skip (patch.rs:5447).
    let symlinks = vec![
        SvnChange {
            change_type: TYPE_MOVE_HERE,
            current_path: Some("l".to_string()),
            symlink_target: Some("t".to_string()),
            ..SvnChange::default()
        },
        SvnChange {
            change_type: TYPE_DELETE,
            current_path: Some("d".to_string()),
            symlink_target: Some("t".to_string()),
            ..SvnChange::default()
        },
    ];
    let mut linked = Vec::new();
    run_svn_symlinks(&symlinks, &mut |target, path| {
        linked.push((target.to_string(), path.to_string()));
        Ok(())
    })
    .unwrap();
    assert_eq!(linked, vec![("t".to_string(), "l".to_string())]);
}

#[test]
fn t1170_run_svn_patches_touch_failure() {
    // An empty patch whose touch fails counts as a hunk failure
    // (patch.rs:5465).
    let plan = SvnPlan {
        patches: vec![("e.txt".to_string(), String::new())],
        ..SvnPlan::default()
    };
    let mut io = Io::buffered();
    let failed = run_svn_patches(&plan, &mut io, &mut no_patch, &mut |_| {
        Err("no touch".to_string())
    })
    .unwrap();
    assert!(failed);
}

#[test]
fn t1170_run_svn_adds_and_props() {
    // Adds run `svn add` (patch.rs:5486); removals run `propdel`
    // (patch.rs:5492) and values run `propset` (patch.rs:5499), with
    // `unix:filemode` mapped to `svn:executable`.
    let dir = temp_dir("t1170_svn_addprop");
    let svn = fake_svn_backend(&dir);
    let plan = SvnPlan {
        adds: vec!["n.txt".to_string()],
        propset: vec![
            (
                "f.txt".to_string(),
                "unix:filemode".to_string(),
                Some("100755".to_string()),
            ),
            ("g.txt".to_string(), "svn:ignore".to_string(), None),
        ],
        ..SvnPlan::default()
    };
    run_svn_adds_props(&dir, &svn, &plan).unwrap();
    let log = std::fs::read_to_string(dir.join(".fakesvn.log")).unwrap();
    assert!(log.contains("add n.txt"), "{log}");
    assert!(log.contains("propset svn:executable on f.txt"), "{log}");
    assert!(log.contains("propdel svn:ignore g.txt"), "{log}");
}

#[test]
fn t1170_run_patch_file_applies_and_reports() {
    // The `patch -p0` runner (patch.rs:5527-5529) reports success and
    // stdout (patch.rs:5547-5550) for both outcomes. Runs through the
    // git-apply shim: hermetic on every OS (system patch is absent on
    // some machines and fails valid input on Windows CI).
    let dir = temp_dir("t1170_patch_run");
    std::fs::write(dir.join("v.txt"), "x\na\nx\n").unwrap();
    let (program, prefix) = fake_patch_shim(&dir);
    let prefs: Vec<&str> = prefix.iter().map(String::as_str).collect();
    let patch = "--- v.txt\n+++ v.txt\n@@ -1,3 +1,3 @@\n x\n-a\n+b\n x\n";
    let (ok, out) = run_patch_file_with_argv(&dir, patch, &program, &prefs).unwrap();
    assert!(ok, "{out}");
    assert!(out.contains("applied patch"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.join("v.txt")).unwrap(),
        "x\nb\nx\n"
    );
    std::fs::write(dir.join("w.txt"), "zzz\n").unwrap();
    let (ok, _) = run_patch_file_with_argv(
        &dir,
        "--- w.txt\n+++ w.txt\n@@ -1 +1 @@\n-a\n+b\n",
        &program,
        &prefs,
    )
    .unwrap();
    assert!(!ok);
}

#[test]
fn t1170_run_patch_file_converted_applies() {
    // The encoding-converting runner (patch.rs:5555-5560) converts
    // first, then applies like the plain runner (git-apply shim, as
    // above — no system patch).
    let dir = temp_dir("t1170_patch_converted");
    std::fs::write(dir.join("v.txt"), "x\na\nx\n").unwrap();
    let (program, prefix) = fake_patch_shim(&dir);
    let prefs: Vec<&str> = prefix.iter().map(String::as_str).collect();
    let (ok, _) = run_patch_file_converted_with_argv(
        &dir,
        "--- v.txt\n+++ v.txt\n@@ -1,3 +1,3 @@\n x\n-a\n+b\n x\n",
        Some("UTF-8"),
        &program,
        &prefs,
    )
    .unwrap();
    assert!(ok);
    assert_eq!(
        std::fs::read_to_string(dir.join("v.txt")).unwrap(),
        "x\nb\nx\n"
    );
}

#[test]
fn t1170_flow_update_failure_aborts() {
    // The flow runs the working-copy update first (patch.rs:6066); a
    // failing `git pull` aborts before any patching.
    let repo = make_repo("t1170_flow_update");
    let patch = repo.join("fix.patch");
    std::fs::write(
        &patch,
        "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-base\n+new\n",
    )
    .unwrap();
    let path = patch.to_string_lossy().to_string();
    let err = execute_with_stdin_ansi(
        &repo,
        &argv(&["--patch", &path, "--force", "--update"]),
        None,
        false,
    )
    .unwrap_err();
    assert!(!err.is_empty(), "expected the pull failure");
}

#[test]
fn t1170_setup_patch_client_requires_token() {
    // Non-patch sources build a Conduit client (patch.rs:6094-6097),
    // which fails without a token for the URI.
    let dir = temp_dir("t1170_setup_client");
    let options = options_of(&["--conduit-uri", "https://patch-test.invalid/api/"]);
    assert_eq!(
        setup_patch_client_with(&Source::Named("1".to_string()), &options, &dir, None).unwrap_err(),
        "no token for https://patch-test.invalid/api/ in ~/.arcrc"
    );
}

#[test]
fn t1170_apply_bundle_encoding_without_call() {
    // Without a Conduit channel there is no repository encoding to
    // look up (patch.rs:6123), so the bundle keeps no encoding.
    let dir = temp_dir("t1170_enc_nocall");
    let mut loaded = LoadedBundle {
        bundle: Bundle::default(),
        arcbundle_dicts: None,
    };
    apply_patch_bundle_encoding(
        &dir,
        Vcs::Git,
        &Backends::default(),
        &Source::Revision("1".to_string()),
        &options_of(&[]),
        None,
        &mut loaded,
    )
    .unwrap();
    assert!(loaded.bundle.encoding.is_none());
}

#[test]
fn t1170_combine_flow_outcome_prefers_errors() {
    // A failed stash pop fails an otherwise clean run
    // (patch.rs:6142).
    assert_eq!(
        combine_patch_flow_outcome(Ok(FlowOutcome::Done), Err("pop".to_string())),
        Err("pop".to_string())
    );
}

#[test]
fn t1170_flow_branch_collision_aborts() {
    // Branch creation failure inside the flow propagates
    // (patch.rs:6209) before anything applies.
    let repo = make_repo("t1170_flow_branch");
    for name in ["arcpatch", "arcpatch_1", "arcpatch_2", "arcpatch_3"] {
        git(&repo, &["branch", name]);
    }
    let patch = repo.join("fix.patch");
    std::fs::write(&patch, "diff --git a/f.txt b/f.txt\n").unwrap();
    let path = patch.to_string_lossy().to_string();
    let err = execute_with_stdin_ansi(
        &repo,
        &argv(&["--patch", &path, "--force", "--nocommit"]),
        None,
        false,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Arc was unable to automagically make a name for this patch. Please clean up your \
         working copy and try again."
    );
}

#[test]
fn t1170_flow_hg_apply_failure_propagates() {
    // An hg apply failure inside the flow propagates (patch.rs:6317)
    // after printing the failure banner.
    let dir = temp_dir("t1170_flow_hg");
    std::fs::create_dir(dir.join(".hg")).unwrap();
    let backends = Backends {
        hg: fake_hg_backend(&dir),
        svn: svn::SvnCommand::system(),
    };
    std::fs::write(dir.join(".fakehg-fail-import"), "").unwrap();
    let patch = dir.join("fix.patch");
    std::fs::write(&patch, "diff --git a/f b/f\n").unwrap();
    let path = patch.to_string_lossy().to_string();
    let mut io = Io::buffered();
    let err = execute_with_io_ansi(
        &dir,
        &argv(&["--patch", &path, "--force", "--nobranch", "--nocommit"]),
        &mut io,
        &backends,
        false,
    )
    .unwrap_err();
    assert_eq!(err, String::new());
    assert!(io.out.stdout.contains("Patch Failed!"), "{}", io.out.stdout);
}

#[test]
fn t1170_apply_patch_svn_from_dicts() {
    // The svn applier takes arcbundle dicts when present, patching,
    // touching, and linking per change; clean runs report `Done` and
    // failed hunks report `SvnHunksFailed`. The patch runner is a
    // recording stub (no system patch); touch/link run for real.
    let dir = temp_dir("t1170_svn_dicts");
    std::fs::write(dir.join("v.txt"), "a\n").unwrap();
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    // `mut` feeds the unix-only symlink pushes below; unused on Windows.
    #[allow(unused_mut)]
    let mut dicts = vec![
        serde_json::json!({"type": 2, "fileType": 1, "oldPath": "v.txt", "currentPath": "v.txt",
            "oldProperties": {}, "newProperties": {}}),
        serde_json::json!({"type": 1, "fileType": 1, "currentPath": "touched.txt",
            "oldProperties": {}, "newProperties": {}}),
    ];
    #[allow(unused_mut)]
    let mut corpora = vec![vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\n")], Vec::new()];
    #[cfg(unix)]
    {
        dicts.push(
            serde_json::json!({"type": 1, "fileType": 5, "currentPath": "link",
                "oldProperties": {}, "newProperties": {}}),
        );
        corpora.push(vec![parsed_hunk(1, 0, 1, 1, "+tgt\n")]);
    }
    let bundle = Bundle::default();
    let mut io = Io::buffered();
    let mut received: Vec<(String, String)> = Vec::new();
    assert_eq!(
        apply_patch_svn_with(
            &dir,
            &backends,
            &bundle,
            Some(&(dicts, corpora)),
            &mut io,
            false,
            &mut |path, patch| {
                received.push((path.to_string(), patch.to_string()));
                Ok((true, "applied".to_string()))
            },
        )
        .unwrap(),
        FlowOutcome::Done
    );
    // The v.txt change rode the patch runner (the stub records instead
    // of writing, so no content assert here); touch/link ran for real.
    assert_eq!(received.len(), 1);
    assert!(received[0].0.contains("v.txt"), "{received:?}");
    assert!(received[0].1.contains("-a"), "{:?}", received[0].1);
    assert!(received[0].1.contains("+b"), "{:?}", received[0].1);
    assert!(dir.join("touched.txt").is_file());
    #[cfg(unix)]
    assert!(
        std::fs::symlink_metadata(dir.join("link"))
            .unwrap()
            .is_symlink(),
        "expected a symlink"
    );
    let dir = temp_dir("t1170_svn_dicts_fail");
    std::fs::write(dir.join("v.txt"), "zzz\n").unwrap();
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    let dicts = vec![
        serde_json::json!({"type": 2, "fileType": 1, "oldPath": "v.txt", "currentPath": "v.txt",
            "oldProperties": {}, "newProperties": {}}),
    ];
    let corpora = vec![vec![parsed_hunk(1, 1, 1, 1, "-a\n+b\n")]];
    let mut io = Io::buffered();
    assert_eq!(
        apply_patch_svn_with(
            &dir,
            &backends,
            &bundle,
            Some(&(dicts, corpora)),
            &mut io,
            false,
            &mut |_, _| Ok((false, "hunk failed".to_string())),
        )
        .unwrap(),
        FlowOutcome::SvnHunksFailed
    );
    assert!(io.out.stdout.contains("WARNING"), "{}", io.out.stdout);
}

#[test]
fn t1170_flow_svn_hunks_failure_exits_empty() {
    // Failed svn hunks print the warning and exit empty
    // (patch.rs:6392): the caller exits 1 without printing more.
    // `patch` ships on every CI runner (build-essential), so no probe.
    let dir = temp_dir("t1170_flow_svn");
    std::fs::create_dir(dir.join(".svn")).unwrap();
    std::fs::write(dir.join("v.txt"), "zzz\n").unwrap();
    let backends = Backends {
        hg: hg::HgCommand::system(),
        svn: fake_svn_backend(&dir),
    };
    let patch = dir.join("fix.patch");
    std::fs::write(
        &patch,
        "diff --git a/v.txt b/v.txt\n--- a/v.txt\n+++ b/v.txt\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    let path = patch.to_string_lossy().to_string();
    let mut io = Io::buffered();
    let err = execute_with_io_ansi(
        &dir,
        &argv(&["--patch", &path, "--force"]),
        &mut io,
        &backends,
        false,
    )
    .unwrap_err();
    assert_eq!(err, String::new());
    assert!(io.out.stdout.contains("WARNING"), "{}", io.out.stdout);
}

#[test]
fn t1170_preview_conduit_requires_token() {
    // Previewing a Conduit source builds a client (patch.rs:6401),
    // which fails without a token for the URI.
    let dir = temp_dir("t1170_preview_token");
    let options = options_of(&[
        "--revision",
        "D1",
        "--conduit-uri",
        "https://patch-test.invalid/api/",
    ]);
    let mut io = Io::buffered();
    assert_eq!(
        execute_preview_with(&dir, &options, &mut io, None).unwrap_err(),
        "no token for https://patch-test.invalid/api/ in ~/.arcrc"
    );
}

/// Serve one canned conduit response, whatever the method.
fn serve_one(canned: String) -> String {
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
        let head = String::from_utf8_lossy(&head).to_string();
        let len: usize = head
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
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            canned.len(),
            canned
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    base
}

fn patch_envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
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
fn fake_arcrc(dir: &Path, base: &str) -> PathBuf {
    let path = dir.join("fake.arcrc");
    let key = host_key_for(base);
    std::fs::write(
        &path,
        format!(r#"{{"hosts": {{"{key}": {{"token": "x"}}}}}}"#),
    )
    .unwrap();
    path
}

/// One `differential.querydiffs` row with no changes.
fn empty_diffs_envelope() -> String {
    patch_envelope(
        r#"{"7": {"id": 7, "revisionID": 1, "sourceControlBaseRevision": "", "authorName": "", "authorEmail": "", "changes": []}}"#,
    )
}

#[test]
fn revision_flow_reports_project_config_failure_after_conduit_load() {
    // A conduit revision load succeeds against the stub (covering the
    // live call body), then the encoding lookup fails on the
    // malformed project config (covering the `?` propagation).
    let repo = make_repo("t1176_s11b_encoding");
    std::fs::write(repo.join(".arcconfig"), "{invalid-json").unwrap();
    git(&repo, &["add", ".arcconfig"]);
    git(&repo, &["commit", "-m", "bad config"]);
    let base = serve_one(empty_diffs_envelope());
    let creds = temp_dir("t1176_s11b_creds");
    let arcrc = fake_arcrc(&creds, &base);
    let backends = Backends::default();
    let mut io = Io::buffered();
    let err = execute_with_io_ansi_and_user_file(
        &repo,
        &argv(&["--revision", "D1", "--conduit-uri", &base]),
        &mut io,
        &backends,
        false,
        Some(&arcrc),
    )
    .unwrap_err();
    assert!(err.contains(".arcconfig"), "{err}");
}

#[test]
fn preview_loads_revision_bundle_with_staged_credentials() {
    let dir = temp_dir("t1176_s11b_preview");
    let base = serve_one(empty_diffs_envelope());
    let arcrc = fake_arcrc(&dir, &base);
    let options = options_of(&["--preview", "--revision", "D1", "--conduit-uri", &base]);
    let mut io = Io::buffered();
    let out = execute_preview_with(&dir, &options, &mut io, Some(&arcrc)).unwrap();
    // The empty bundle renders as the bare trailing newline.
    assert_eq!(out.stdout, "\n");
}

// -( T1170 COV5: close patch.rs )-------------------------------------------------

#[test]
fn source_selection_requires_a_source() {
    let err = select_source(&Options::default()).unwrap_err();
    assert!(err.contains("You must specify changes"), "{err}");
}

#[test]
fn unterminated_ansi_falls_through() {
    let text = "\x1b[31Xoops\n\x1b[31mok\ndiff --git a/f b/f\n";
    assert_eq!(validate_patch_text(text), Ok(()));
}

#[test]
fn base64_padding_decodes() {
    assert_eq!(base64_decode("Zg==").unwrap(), b"f");
    assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
}

#[test]
fn hg_tip_propagates_spawn_failures() {
    let dir = temp_dir("hg-tip-spawn");
    let hg = hg::HgCommand::custom("definitely-not-an-hg-binary-xyz", &[]);
    assert!(hg_tip_info(&dir, &hg).is_err());
}

#[test]
fn svn_head_helpers_are_trivial() {
    let dir = temp_dir("svn-head");
    let backends = Backends::default();
    assert!(
        patch_head_info_for(&dir, Vcs::Svn, &backends)
            .unwrap()
            .is_none()
    );
    let head = HeadInfo {
        hash: "abc".to_string(),
        author: "a".to_string(),
        message: "m".to_string(),
    };
    assert!(!patch_head_author_matches(&dir, Vcs::Svn, &backends, &head).unwrap());
    assert_eq!(
        working_source_revision(&dir, Vcs::Svn, &backends).unwrap(),
        ""
    );
}

#[test]
fn git_apply_reports_spawn_failures() {
    let missing = temp_dir("apply-missing").join("no-such-dir");
    let bundle = Bundle {
        text: "diff --git a/f b/f\n".to_string(),
        encoding: None,
        base_revision: None,
        revision_id: None,
        author_name: None,
        author_email: None,
    };
    let mut io = Io::canned(&[]);
    assert!(git_apply_patch_ansi(&missing, &bundle, &mut io, false).is_err());
}

#[test]
fn svn_copy_and_propset_failures_propagate() {
    let dir = temp_dir("svn-fail");
    let svn = svn::SvnCommand::custom("definitely-not-an-svn-binary-xyz", &[]);
    let copies = SvnPlan {
        copies: vec![("a".to_string(), "b".to_string())],
        deletes: Vec::new(),
        patches: Vec::new(),
        propset: Vec::new(),
        adds: Vec::new(),
        symlinks: Vec::new(),
    };
    assert!(run_svn_copies_deletes(&dir, &svn, &copies).is_err());
    let props = SvnPlan {
        copies: Vec::new(),
        deletes: Vec::new(),
        patches: Vec::new(),
        propset: vec![(
            "f".to_string(),
            "svn:eol-style".to_string(),
            Some("LF".to_string()),
        )],
        adds: Vec::new(),
        symlinks: Vec::new(),
    };
    assert!(run_svn_adds_props(&dir, &svn, &props).is_err());
}

#[test]
fn apply_hg_commits_applies_and_rebases() {
    let dir = temp_dir("apply-hg");
    let hg = fake_hg_backend(&dir);
    let bundle = Bundle {
        text: "diff --git a/f b/f\n".to_string(),
        revision_id: Some(1),
        ..Bundle::default()
    };
    let mut responses = std::collections::HashMap::new();
    responses.insert(
        diff_submit::METHOD_GET_COMMIT_MESSAGE.to_string(),
        serde_json::json!("hello"),
    );
    let call = stub_call(responses);
    let call_ref: &diff_submit::ConduitCall = &call;
    // Commit on a fresh bookmark: no rebase.
    let mut io = Io::canned(&[]);
    apply_hg_ansi(
        &dir,
        &hg,
        &bundle,
        &Options::default(),
        &mut io,
        Some(call_ref),
        false,
        None,
        None,
        false,
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("Successfully committed patch."),
        "{}",
        io.out.stderr
    );
    // Nobranch with a local base and live branches: rebase the temp.
    let mut options = Options {
        nobranch: true,
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    apply_hg_ansi(
        &dir,
        &hg,
        &bundle,
        &options,
        &mut io,
        Some(call_ref),
        true,
        Some("master"),
        Some("arcpatch"),
        false,
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("Successfully committed patch."),
        "{}",
        io.out.stderr
    );
    // Nocommit: apply only.
    options.nocommit = true;
    let mut io = Io::canned(&[]);
    apply_hg_ansi(
        &dir,
        &hg,
        &bundle,
        &options,
        &mut io,
        Some(call_ref),
        false,
        None,
        None,
        false,
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("Successfully applied patch."),
        "{}",
        io.out.stderr
    );
}

#[test]
fn flow_inner_decline_aborts_at_sanity() {
    let dir = make_repo("flow-inner-decline");
    let bundle = Bundle {
        text: "diff --git a/f b/f\n".to_string(),
        base_revision: Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string()),
        ..Bundle::default()
    };
    let loaded = LoadedBundle {
        bundle,
        arcbundle_dicts: None,
    };
    let options = Options {
        skip_dependencies: true,
        nobranch: true,
        nocommit: true,
        ..Options::default()
    };
    let mut io = Io::canned(&["n"]);
    let backends = Backends::default();
    let err = run_patch_flow_inner(
        &dir,
        &options,
        &mut io,
        &backends,
        Vcs::Git,
        false,
        None,
        loaded,
    )
    .unwrap_err();
    assert_eq!(err, land::ABORT);
}

#[test]
fn flow_inner_dependency_child_failure_propagates() {
    let dir = make_repo("flow-inner-deps");
    let bundle = Bundle {
        text: "diff --git a/f b/f\n".to_string(),
        base_revision: Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string()),
        revision_id: Some(1),
        ..Bundle::default()
    };
    let loaded = LoadedBundle {
        bundle,
        arcbundle_dicts: None,
    };
    let options = Options {
        nobranch: true,
        nocommit: true,
        ..Options::default()
    };
    let rows = serde_json::json!([
        {"phid": "PHID-DREV-1", "id": 1,
         "auxiliary": {"phabricator:depends-on": ["PHID-DREV-2"]},
         "hashes": {"gtcm": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"}},
        {"phid": "PHID-DREV-2", "id": 2, "auxiliary": {}},
    ]);
    let mut responses = std::collections::HashMap::new();
    responses.insert(diff_submit::METHOD_QUERY.to_string(), rows);
    let call = stub_call(responses);
    let call_ref: &diff_submit::ConduitCall = &call;
    let mut io = Io::canned(&[]);
    let backends = Backends::default();
    // The child revision flow fails (no conduit client in hermetic
    // runs; an unknown revision elsewhere) and the error propagates.
    let result = run_patch_flow_inner(
        &dir,
        &options,
        &mut io,
        &backends,
        Vcs::Git,
        false,
        Some(call_ref),
        loaded,
    );
    assert!(result.is_err());
}

#[test]
fn svn_apply_failure_propagates() {
    let dir = temp_dir("svn-apply-fail");
    let backends = Backends {
        svn: svn::SvnCommand::custom("definitely-not-an-svn-binary-xyz", &[]),
        ..Backends::default()
    };
    let bundle = Bundle {
        text: "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..3b18e51\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n"
            .to_string(),
        ..Bundle::default()
    };
    let mut io = Io::canned(&[]);
    let mut run_patch = |_: &str, _: &str| Ok((true, String::new()));
    assert!(
        apply_patch_svn_with(
            &dir,
            &backends,
            &bundle,
            None,
            &mut io,
            false,
            &mut run_patch
        )
        .is_err()
    );
}

#[test]
fn targetless_symlinks_are_skipped() {
    let change = SvnChange {
        change_type: TYPE_ADD,
        file_type: 0,
        old_path: None,
        current_path: Some("link".to_string()),
        hunks: String::new(),
        old_props: std::collections::HashMap::new(),
        new_props: std::collections::HashMap::new(),
        symlink_target: None,
    };
    // A firing hook would panic: success proves the targetless change
    // was skipped.
    run_svn_symlinks(&[change], &mut no_link).unwrap();
}

#[test]
fn cov6g_read_patch_bytes_uses_scripted_stdin_for_dash() {
    // T1170 wave 6g: the scripted-stdin arm (read_patch_bytes, `-`
    // with `io.patch_stdin` set).
    let mut io = Io::buffered();
    io.patch_stdin = Some("diff --git a/f b/f\n".to_string());
    assert_eq!(
        read_patch_bytes("-", &mut io).unwrap(),
        b"diff --git a/f b/f\n"
    );
    // A real path still reads from disk (the scripted value only
    // applies to `-`).
    let dir = temp_dir("cov6g_patch_stdin");
    std::fs::write(dir.join("p.diff"), "patch-bytes").unwrap();
    let path = dir.join("p.diff").to_string_lossy().to_string();
    assert_eq!(read_patch_bytes(&path, &mut io).unwrap(), b"patch-bytes");
}

#[test]
fn read_patch_bytes_reads_real_stdin_when_unscripted() {
    // Without scripted stdin, `-` reads real stdin to EOF; cargo-test
    // stdin is always EOF. Fail loud (rather than block on a real
    // terminal) if that ever changes.
    assert!(
        !crate::prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    let mut io = Io::buffered();
    assert_eq!(read_patch_bytes("-", &mut io).unwrap(), b"");
}

// -( T1170 endgame band I: direct helper-arm tests )---------------------------

#[test]
fn transport_pushes_ascii_verbatim() {
    // `push_transport_byte` documents ASCII-verbatim passthrough, but
    // live callers only feed it invalid-UTF-8 bytes (always >= 0x80),
    // so the ASCII arm is covered here directly.
    let mut out = String::new();
    push_transport_byte(&mut out, b'A');
    assert_eq!(out, "A");
    let mut out = String::new();
    push_transport_byte(&mut out, 0x80);
    assert!(out.starts_with(TRANSPORT_ESC));
}

#[test]
fn base64_padding_byte_maps_to_none() {
    // `base64_val` documents `=` as padding, but the decoder skips
    // padding positions before consulting the table (and rejects
    // stray `=` earlier), so the arm is covered here directly.
    assert_eq!(base64_val(b'='), None);
    assert_eq!(base64_val(b'A'), Some(0));
    assert_eq!(base64_val(b'?'), None);
}

// -( T1176 PB-4: interactive-editor trampolines + live-conduit deps )----------

/// Spawn this test binary with `vars` set, running only `probe`.
fn spawn_probe(probe: &str, vars: &[(&str, &str)]) -> (bool, String, String) {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = std::process::Command::new(&exe);
    for (key, value) in vars {
        command.env(key, value);
    }
    let out = command
        .args(["--nocapture", probe])
        .output()
        .expect("spawn probe child");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stdout, stderr)
}

/// Assert a probe child passed and printed its marker.
fn assert_probe_ok(probe: &str, vars: &[(&str, &str)], marker: &str) {
    let (success, stdout, stderr) = spawn_probe(probe, vars);
    assert!(
        success && stdout.contains("1 passed") && stdout.contains(marker),
        "{probe} child failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// The yes-path editor closure over a pinned non-interactive `$EDITOR`.
///
/// Runs only with `EDITOR=true`/`false` (see
/// [`t1176_pb4_clean_tree_editor_trampoline`]); otherwise this would
/// launch the ambient editor.
#[test]
fn pb4_clean_tree_editor_probe() {
    let editor = std::env::var("EDITOR").unwrap_or_default();
    if editor != "true" && editor != "false" {
        println!("PB4-CLEAN-TREE-PROBE-OK");
        return;
    }
    // A revision footer declines the amend, so the yes-path edits a new
    // message through the real closure (the p9_yes_path_commits_via_editor
    // setup, driven through the public entry).
    let repo = make_repo("pb4_clean_tree_probe");
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
    let err = require_clean_tree_with_call(&repo, Vcs::Git, &backends, &mut io, false, false, None)
        .unwrap_err();
    if editor == "true" {
        // `true` leaves the template untouched: the yes-path refuses.
        assert_eq!(err, MUST_PROVIDE_MESSAGE);
    } else {
        assert_eq!(err, diff::editor_exit_error(1));
    }
    assert!(
        io.out.stdout.contains("create a new commit"),
        "{}",
        io.out.stdout
    );
    println!("PB4-CLEAN-TREE-PROBE-OK");
}

/// The commit-message editor closure over a pinned non-interactive
/// `$EDITOR` (same ambient-editor guard as
/// [`pb4_clean_tree_editor_probe`]).
#[test]
fn pb4_commit_message_editor_probe() {
    let editor = std::env::var("EDITOR").unwrap_or_default();
    if editor != "true" && editor != "false" {
        println!("PB4-COMMIT-MESSAGE-PROBE-OK");
        return;
    }
    // No revision ID, so the message resolves through the real editor
    // closure.
    let bundle = Bundle::default();
    let err = resolve_commit_message_interactive(&bundle, None).unwrap_err();
    if editor == "true" {
        // `true` leaves the all-comments template untouched: abort.
        assert_eq!(err, diff_local::USER_ABORT);
    } else {
        assert_eq!(err, diff::editor_exit_error(1));
    }
    println!("PB4-COMMIT-MESSAGE-PROBE-OK");
}

#[test]
fn t1176_pb4_clean_tree_editor_trampoline() {
    // The child carries `$EDITOR`, so the suite never mutates its own
    // environment; each arm drives the closure end to end.
    assert_probe_ok(
        "pb4_clean_tree_editor_probe",
        &[("EDITOR", "true")],
        "PB4-CLEAN-TREE-PROBE-OK",
    );
    assert_probe_ok(
        "pb4_clean_tree_editor_probe",
        &[("EDITOR", "false")],
        "PB4-CLEAN-TREE-PROBE-OK",
    );
}

#[test]
fn t1176_pb4_commit_message_editor_trampoline() {
    assert_probe_ok(
        "pb4_commit_message_editor_probe",
        &[("EDITOR", "true")],
        "PB4-COMMIT-MESSAGE-PROBE-OK",
    );
    assert_probe_ok(
        "pb4_commit_message_editor_probe",
        &[("EDITOR", "false")],
        "PB4-COMMIT-MESSAGE-PROBE-OK",
    );
}

/// Serve canned conduit envelopes routed by method name, for as many
/// sequential requests as the flow makes.
fn serve_routes(routes: std::collections::HashMap<String, String>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        // `flatten` skips transient accept errors (an aborted client
        // must not kill the stub); the listener never closes in
        // tests, so no permanent-error spin is possible.
        for mut stream in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                if stream.read_exact(&mut byte).is_err() {
                    break;
                }
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&head).to_string();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(key, value)| {
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            if stream.read_exact(&mut body).is_err() {
                break;
            }
            let target = head.lines().next().unwrap_or("").to_string();
            let canned = routes
                .iter()
                .find(|(method, _)| target.contains(method.as_str()))
                .map(|(_, canned)| canned.clone())
                .unwrap_or_else(|| patch_envelope("null"));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            // Best-effort: a vanished client must not end the stub.
            let _ = stream.write_all(response.as_bytes());
        }
    });
    base
}

#[test]
fn stub_server_tolerates_aborted_clients() {
    // Misbehaving clients trip the stub's robustness arms: an
    // immediate close ends header reading at EOF, and headers
    // promising a body that never arrives end body reading at EOF.
    // Each stub serves one aborted client then exits its loop.
    let base = serve_routes(std::collections::HashMap::new());
    let addr = base
        .strip_prefix("http://")
        .and_then(|s| s.strip_suffix('/'))
        .unwrap()
        .to_string();
    drop(std::net::TcpStream::connect(&addr).unwrap());
    let base = serve_routes(std::collections::HashMap::new());
    let addr = base
        .strip_prefix("http://")
        .and_then(|s| s.strip_suffix('/'))
        .unwrap()
        .to_string();
    let mut stream = std::net::TcpStream::connect(&addr).unwrap();
    use std::io::Write;
    stream
        .write_all(b"POST /api/x HTTP/1.1\r\nContent-Length: 100\r\n\r\n")
        .unwrap();
    drop(stream);
}

/// Drive one dependency child through the real flow against a loopback
/// stub.
///
/// Runs only in the [`t1176_pb4_dependencies_drive_child_flow`] child
/// (which pins `HOME` to a fixture dir for the temp credentials the
/// child flow reads through the ambient arcrc path).
#[test]
fn pb4_dependency_child_probe() {
    if std::env::var("ARC_RS_PB4_DEPS_CHILD").as_deref() != Ok("1") {
        println!("PB4-DEPS-PROBE-OK");
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").expect("pinned HOME for temp credentials"));
    let repo = make_repo("pb4_deps_probe");
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-m", "add a.txt"]);
    // The stub serves the dependency revision bundle; temp creds live
    // under the pinned HOME.
    let row = serde_json::json!({
        "7": {"id": "7", "revisionID": "2", "sourceControlBaseRevision": "base-dep",
              "authorName": "Ada", "authorEmail": "ada@test",
              "changes": [{
                  "type": "2", "fileType": "1",
                  "oldPath": "a.txt", "currentPath": "a.txt",
                  "hunks": [{"oldOffset": "1", "oldLength": "1",
                             "newOffset": "1", "newLength": "1",
                             "corpus": "-a\n+b\n"}],
              }]},
    });
    let base = serve_routes(std::collections::HashMap::from([(
        diff_submit::METHOD_QUERY_DIFFS.to_string(),
        patch_envelope(&row.to_string()),
    )]));
    let key = host_key_for(&base);
    std::fs::write(
        home.join(".arcrc"),
        format!(r#"{{"hosts": {{"{key}": {{"token": "x"}}}}}}"#),
    )
    .unwrap();
    // D1 depends on D2 (the apply_dependencies_runs_children graph).
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
    let graph_ref: &diff_submit::ConduitCall = &graph;
    let bundle = Bundle {
        revision_id: Some(1),
        ..Bundle::default()
    };
    let options = Options {
        conduit_uri: Some(base),
        nocommit: true,
        ..Options::default()
    };
    let backends = Backends::default();
    let mut io = Io::buffered();
    run_patch_dependencies(
        &repo,
        &options,
        &mut io,
        &backends,
        &bundle,
        Vcs::Git,
        false,
        Some(graph_ref),
        false,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "b\n");
    assert!(io.out.stderr.contains("APPLIED"), "{}", io.out.stderr);
    println!("PB4-DEPS-PROBE-OK");
}

#[test]
fn t1176_pb4_dependencies_drive_child_flow() {
    // The child flow authenticates through the ambient arcrc path, so
    // HOME is pinned to a fixture dir in the child only; EDITOR=false
    // fails fast instead of hanging if the flow ever edits.
    let home = temp_dir("t1176_pb4_deps_home");
    let home_str = home.to_str().expect("utf8 home");
    assert_probe_ok(
        "pb4_dependency_child_probe",
        &[
            ("ARC_RS_PB4_DEPS_CHILD", "1"),
            ("HOME", home_str),
            ("USERPROFILE", home_str),
            ("EDITOR", "false"),
        ],
        "PB4-DEPS-PROBE-OK",
    );
}

#[test]
fn t1176_pb4_dependency_child_outcome_combines() {
    assert_eq!(combine_dependency_child_outcome(FlowOutcome::Done), Ok(()));
    assert_eq!(
        combine_dependency_child_outcome(FlowOutcome::SvnHunksFailed),
        Err(String::new())
    );
}
