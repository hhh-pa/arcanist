use super::*;
use std::path::PathBuf;

/// Controlled dir plus a fake `svn` backend; removed on drop.
struct FakeSvn {
    dir: PathBuf,
    svn: SvnCommand,
}

impl FakeSvn {
    /// Backend script shared by every test in the process: written once
    /// under a lock, then executed read-only with per-test CWDs (canned
    /// files and logs stay per-test, so isolation is unchanged). Sharing
    /// removes the per-test write-then-exec window that flakes on Linux
    /// with ETXTBSY under parallel load (same class as the hg fake).
    fn shared_script() -> PathBuf {
        static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir =
                std::env::temp_dir().join(format!("arc-rs-svn-shared-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            #[cfg(windows)]
            {
                const FAKE: &str = include_str!("../tests/fixtures/vcs/fakesvn.bat");
                let bat = dir.join("fakesvn.bat");
                std::fs::write(&bat, FAKE).unwrap();
                bat
            }
            #[cfg(not(windows))]
            {
                const FAKE: &str = include_str!("../tests/fixtures/vcs/fakesvn.sh");
                let sh = dir.join("fakesvn.sh");
                std::fs::write(&sh, FAKE).unwrap();
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&sh).unwrap().permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&sh, perms).unwrap();
                }
                sh
            }
        })
        .clone()
    }

    /// Shared fake unified `diff` (same write-once reasoning).
    fn shared_diff() -> PathBuf {
        static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir =
                std::env::temp_dir().join(format!("arc-rs-svn-shared-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            #[cfg(windows)]
            {
                const FAKE: &str = include_str!("../tests/fixtures/vcs/fakediff.bat");
                let bat = dir.join("fakediff.bat");
                std::fs::write(&bat, FAKE).unwrap();
                bat
            }
            #[cfg(not(windows))]
            {
                const FAKE: &str = include_str!("../tests/fixtures/vcs/fakediff.sh");
                let sh = dir.join("fakediff.sh");
                std::fs::write(&sh, FAKE).unwrap();
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&sh).unwrap().permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&sh, perms).unwrap();
                }
                sh
            }
        })
        .clone()
    }

    fn named(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("arc-rs-svn-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = Self::shared_script();
        #[cfg(windows)]
        let svn = SvnCommand::custom("cmd", &["/C", &sh.to_string_lossy()]);
        #[cfg(not(windows))]
        let svn = SvnCommand::custom(&sh.to_string_lossy(), &[]);
        Self { dir, svn }
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    /// Logged argv with quote chars stripped (Windows quotes spaced args).
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join(".fakesvn.log"))
            .unwrap_or_default()
            .replace('"', "")
    }

    /// Install the fake unified `diff`, returning `(program, prefix)`.
    fn install_diff() -> (String, Vec<String>) {
        let sh = Self::shared_diff();
        #[cfg(windows)]
        {
            (
                "cmd".to_string(),
                vec!["/C".to_string(), sh.to_string_lossy().to_string()],
            )
        }
        #[cfg(not(windows))]
        {
            (sh.to_string_lossy().to_string(), Vec::new())
        }
    }
}

impl Drop for FakeSvn {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn flags_and_getters_match_php() {
    assert_eq!(FLAG_MODIFIED, 1);
    assert_eq!(FLAG_ADDED, 2);
    assert_eq!(FLAG_DELETED, 4);
    assert_eq!(FLAG_UNTRACKED, 8);
    assert_eq!(FLAG_CONFLICT, 16);
    assert_eq!(FLAG_MISSING, 32);
    assert_eq!(FLAG_UNSTAGED, 64);
    assert_eq!(FLAG_UNCOMMITTED, 128);
    assert_eq!(FLAG_EXTERNALS, 256);
    assert_eq!(FLAG_OBSTRUCTED, 512);
    assert_eq!(FLAG_INCOMPLETE, 1024);
    assert_eq!(get_source_control_system_name(), "svn");
    assert!(is_history_default_immutable());
    assert!(!supports_amend());
    assert!(!supports_commit_ranges());
    assert!(!supports_local_commits());
    assert!(!has_local_commit("anything"));
    assert_eq!(get_finalized_revision_message(), "Done.");
    assert!(!can_stash_changes());
    assert!(stash_changes().is_err());
    assert!(unstash_changes().is_err());
    assert!(get_commit_range_status().is_empty());
}

#[test]
fn parses_status_items() {
    assert_eq!(parse_svn_status_item("none").unwrap(), 0);
    assert_eq!(parse_svn_status_item("normal").unwrap(), 0);
    assert_eq!(parse_svn_status_item("external").unwrap(), FLAG_EXTERNALS);
    assert_eq!(
        parse_svn_status_item("unversioned").unwrap(),
        FLAG_UNTRACKED
    );
    assert_eq!(
        parse_svn_status_item("obstructed").unwrap(),
        FLAG_OBSTRUCTED
    );
    assert_eq!(parse_svn_status_item("missing").unwrap(), FLAG_MISSING);
    assert_eq!(parse_svn_status_item("added").unwrap(), FLAG_ADDED);
    assert_eq!(parse_svn_status_item("replaced").unwrap(), FLAG_MODIFIED);
    assert_eq!(parse_svn_status_item("modified").unwrap(), FLAG_MODIFIED);
    assert_eq!(parse_svn_status_item("deleted").unwrap(), FLAG_DELETED);
    assert_eq!(parse_svn_status_item("conflicted").unwrap(), FLAG_CONFLICT);
    assert_eq!(
        parse_svn_status_item("incomplete").unwrap(),
        FLAG_INCOMPLETE
    );
    let err = parse_svn_status_item("bogus").unwrap_err();
    assert!(err.contains("Unrecognized item status 'bogus'."), "{err}");
}

const STATUS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<status>
<target path=".">
<entry path="m.txt">
<wc-status props="none" item="modified" revision="42">
</wc-status>
</entry>
<entry path="p.txt">
<wc-status props="modified" item="normal" revision="42">
</wc-status>
</entry>
<entry path="t.txt">
<wc-status props="none" item="normal" revision="42" tree-conflicted="true">
</wc-status>
</entry>
<entry path="a &amp; b.txt">
<wc-status props="none" item="added" revision="42">
</wc-status>
</entry>
<entry path="win\sep.txt">
<wc-status props="none" item="deleted" revision="7">
</wc-status>
</entry>
</target>
</status>
"#;

#[test]
fn parses_status_xml() {
    let status = parse_svn_status_xml(STATUS_XML).unwrap();
    assert_eq!(status.files["m.txt"], FLAG_MODIFIED);
    assert_eq!(status.files["p.txt"], FLAG_MODIFIED);
    assert_eq!(status.files["t.txt"], FLAG_CONFLICT);
    assert_eq!(status.files["a & b.txt"], FLAG_ADDED);
    assert_eq!(status.files["win/sep.txt"], FLAG_DELETED);
    assert_eq!(status.base_revisions["m.txt"], "42");
    assert_eq!(status.base_revisions["win/sep.txt"], "7");
    let empty = parse_svn_status_xml("<status/>").unwrap();
    assert!(empty.files.is_empty());
    assert!(empty.base_revisions.is_empty());
}

#[test]
fn rejects_bad_status_xml() {
    let bad_item = STATUS_XML.replace(r#"item="modified""#, r#"item="bogus""#);
    assert!(
        parse_svn_status_xml(&bad_item)
            .unwrap_err()
            .contains("Unrecognized item status")
    );
    let bad_props = STATUS_XML.replace(
        r#"props="none" item="modified""#,
        r#"props="bogus" item="modified""#,
    );
    assert!(
        parse_svn_status_xml(&bad_props)
            .unwrap_err()
            .contains("Unrecognized property status")
    );
}

#[test]
fn flags_externals_and_filters() {
    let xml = r#"<status><target path="."><entry path="ext"><wc-status props="none" item="external" revision="1"></wc-status></entry><entry path="ext/in.txt"><wc-status props="none" item="modified" revision="1"></wc-status></entry><entry path="top.txt"><wc-status props="none" item="modified" revision="1"></wc-status></entry></target></status>"#;
    let status = parse_svn_status_xml(xml).unwrap();
    assert_eq!(status.files["ext"], FLAG_EXTERNALS);
    assert_eq!(status.files["ext/in.txt"], FLAG_MODIFIED | FLAG_EXTERNALS);
    assert_eq!(status.files["top.txt"], FLAG_MODIFIED);
    let filtered = filter_externals(&status.files, false);
    assert_eq!(filtered.len(), 1);
    assert!(filtered.contains_key("top.txt"));
    assert_eq!(filter_externals(&status.files, true).len(), 3);
    assert_eq!(status_argv(&[]), vec!["--xml", "status"]);
    assert_eq!(status_argv(&["a", "b"]), vec!["--xml", "status", "a", "b"]);
}

#[test]
fn parses_diff_summarize_and_splits_revisions() {
    let xml = r#"<diff><paths><path props="none" item="modified" kind="file">u/a.txt</path><path props="none" item="added" kind="file">u/b.txt</path></paths></diff>"#;
    let map = parse_svn_diff_summarize_xml(xml).unwrap();
    assert_eq!(map["u/a.txt"], FLAG_MODIFIED);
    assert_eq!(map["u/b.txt"], FLAG_ADDED);
    assert_eq!(
        split_url_at_revision("https://x/repo@40"),
        ("https://x/repo".to_string(), "40".to_string())
    );
    assert_eq!(
        split_url_at_revision("a@b@c"),
        ("a@b".to_string(), "c".to_string())
    );
    assert_eq!(
        split_url_at_revision("40"),
        ("".to_string(), "40".to_string())
    );
}

#[test]
fn parses_svn_dates() {
    // Independent oracle: 2026-10-13T09:00:00Z == 1791882000.
    assert_eq!(
        parse_svn_date("2026-10-13 09:00:00 +0000"),
        Some(1791882000)
    );
    assert_eq!(
        parse_svn_date("2026-10-13 11:00:00 +0200"),
        Some(1791882000)
    );
    assert_eq!(parse_svn_date("garbage"), None);
    assert_eq!(parse_svn_date("2026-13-13 09:00:00 +0000"), None);
}

const INFO_TEXT: &str = "Path: .\r\nURL: https://svn.example.test/repo/trunk\r\nRepository Root: https://svn.example.test/repo\r\nRepository UUID: uuid-1\r\nRevision: 42\r\nNode Kind: directory\r\nLast Changed Author: alice\r\nLast Changed Rev: 41\r\nLast Changed Date: 2026-10-13 09:00:00 +0000 (Tue, 13 Oct 2026)\r\nCopied From URL: https://svn.example.test/repo/orig\r\nCopied From Rev: 40\r\n";

#[test]
fn parses_svn_info() {
    let info = parse_svn_info(INFO_TEXT).unwrap();
    assert_eq!(info["URL"], "https://svn.example.test/repo/trunk");
    assert_eq!(info["Revision"], "42");
    assert_eq!(info["Last Changed Author"], "alice");
    assert_eq!(info["Last Changed Rev"], "41");
    assert_eq!(info["Last Changed Date"], "2026-10-13 09:00:00 +0000");
    assert_eq!(
        info["Copied From URL"],
        "https://svn.example.test/repo/orig"
    );
    assert_eq!(info["Copied From Rev"], "40");
    assert_eq!(info["Repository Root"], "https://svn.example.test/repo");
    assert_eq!(info["Repository UUID"], "uuid-1");
    assert_eq!(info["Node Kind"], "directory");
    assert_eq!(parse_svn_date(&info["Last Changed Date"]), Some(1791882000));
    assert_eq!(branch_name_from_info(&info), "/trunk");
    assert_eq!(
        get_source_control_base_revision(&info, Some("42")).as_deref(),
        Some("https://svn.example.test/repo/trunk@42")
    );
    assert_eq!(svn_base_revision_number(&info, None).as_deref(), Some("42"));
    assert_eq!(
        svn_base_revision_number(&info, Some("9")).as_deref(),
        Some("9")
    );
    let err = parse_svn_info("nothing useful here\n").unwrap_err();
    assert!(err.contains("Unable to parse SVN info."), "{err}");
    let first_wins = parse_svn_info("URL: https://a/\nURL: https://b/\n").unwrap();
    assert_eq!(first_wins["URL"], "https://a/");
}

#[test]
fn derives_names_from_info() {
    let mut info = BTreeMap::new();
    info.insert("URL".to_string(), "https://x/repo/trunk".to_string());
    info.insert("Repository Root".to_string(), "https://x/repo".to_string());
    assert_eq!(branch_name_from_info(&info), "/trunk");
    info.insert("URL".to_string(), "https://other/elsewhere".to_string());
    assert_eq!(branch_name_from_info(&info), "svn");
    assert_eq!(get_source_control_base_revision(&info, None), None);
    assert_eq!(get_canonical_revision_name("42"), Some("42".to_string()));
    assert_eq!(get_canonical_revision_name("+42"), Some("+42".to_string()));
    assert_eq!(get_canonical_revision_name("4.5"), Some("4.5".to_string()));
    assert_eq!(
        get_canonical_revision_name(" 42 "),
        Some(" 42 ".to_string())
    );
    assert_eq!(get_canonical_revision_name("r42"), None);
    assert_eq!(get_canonical_revision_name(""), None);
    assert_eq!(get_canonical_revision_name("abc"), None);
}

#[test]
fn builds_argv() {
    let root = std::path::Path::new("/r");
    assert_eq!(info_argv(root, "/"), vec!["info", "."]);
    let argv = info_argv(root, "a.txt");
    assert_eq!(argv[0], "info");
    assert!(argv[1].ends_with("a.txt@"), "{}", argv[1]);
    assert_eq!(
        diff_argv("p", 3, None),
        vec!["diff", "--depth", "empty", "p"]
    );
    assert_eq!(
        diff_argv("p", 3, Some("/s.sh")),
        vec![
            "diff",
            "--depth",
            "empty",
            "--diff-cmd",
            "/s.sh",
            "-x",
            "-U3",
            "p"
        ]
    );
    let cmd = default_diff_cmd(std::path::Path::new("/arc"));
    assert!(
        cmd.to_string_lossy().ends_with("binary_safe_diff.sh"),
        "{cmd:?}"
    );
    assert_eq!(SvnDiffConfig::unix(3, "/s.sh").diff_cmd, Some("/s.sh"));
    assert_eq!(SvnDiffConfig::windows(3).diff_cmd, None);
    assert_eq!(SvnDiffConfig::default().context, 0x7FFF);
}

#[test]
fn parses_svn_blame() {
    let blame = parse_svn_blame("      41 alice      one\n      42 bob        two\n").unwrap();
    assert_eq!(
        blame,
        vec![
            ("alice".to_string(), "41".to_string()),
            ("bob".to_string(), "42".to_string()),
        ]
    );
    assert!(parse_svn_blame("").unwrap().is_empty());
    assert!(parse_svn_blame("   \n").unwrap().is_empty());
    let err = parse_svn_blame("not blame at all\n").unwrap_err();
    assert!(err.contains("Bad blame? `not blame at all'"), "{err}");
}

#[test]
fn filters_escapes_and_detects_binary() {
    assert_eq!(filter_files("a.txt"), Some("a.txt".to_string()));
    assert_eq!(filter_files(""), None);
    assert_eq!(filter_files("dir/"), None);
    assert_eq!(escape_file_name_for_svn("plain.txt"), "plain.txt");
    assert_eq!(escape_file_name_for_svn("sprite@2x.png"), "sprite@2x.png@");
    assert_eq!(
        escape_file_names_for_svn(&["a", "b@c"]),
        vec!["a".to_string(), "b@c@".to_string()]
    );
    for name in [
        "a.gif", "a.PNG", "a.jpg", "a.jpeg", "a.swf", "a.pdf", "a.ico", ".gif",
    ] {
        assert!(is_binary_filename(name), "{name}");
    }
    for name in ["a.txt", "agif", "a.gif.txt", "gif", ""] {
        assert!(!is_binary_filename(name), "{name}");
    }
}

#[test]
fn builds_synthetic_texts() {
    let body = binary_diff_text("a.png");
    assert!(body.starts_with("Index: a.png\n==="), "{body}");
    assert!(
        body.contains("svn:mime-type = application/octet-stream\n\n"),
        "{body}"
    );
    let diff = synthetic_unchanged_diff_bytes("a.txt", b"one\ntwo\n");
    let text = String::from_utf8(diff).unwrap();
    assert!(
        text.contains(
            "--- a.txt (synthetic)\n+++ a.txt (synthetic)\n@@ -1,3 +1,3 @@\n one\n two\n \n\n"
        ),
        "{text}"
    );
    let diff = synthetic_unchanged_diff_bytes("a.txt", b"one");
    assert!(
        String::from_utf8(diff)
            .unwrap()
            .contains("@@ -1,1 +1,1 @@\n one\n\n")
    );
    assert_eq!(
        strip_source_prefix("https://x/repo/trunk/o.txt", "https://x/repo"),
        "trunk/o.txt"
    );
    let added = addition_diff_bytes("a.txt", b"--- old\n+++ new\n");
    assert_eq!(
        String::from_utf8(added).unwrap(),
        "Index: a.txt\n===================================================================\n--- old\n+++ new\n\n\n"
    );
}

#[test]
fn selects_status_paths_and_helpers() {
    let mut status = StatusMap::new();
    status.insert("a".to_string(), FLAG_MODIFIED);
    status.insert("b".to_string(), FLAG_UNTRACKED);
    assert_eq!(get_untracked_changes(&status), vec!["b".to_string()]);
    assert_eq!(get_unstaged_changes(&status), Vec::<String>::new());
    assert_eq!(get_uncommitted_changes(&status), Vec::<String>::new());
    assert_eq!(get_merge_conflicts(&status), Vec::<String>::new());
    assert_eq!(get_incomplete_changes(&status), Vec::<String>::new());
    assert_eq!(get_missing_changes(&status), Vec::<String>::new());
    assert_eq!(get_dirty_external_changes(&status), Vec::<String>::new());
    let mut range = StatusMap::new();
    range.insert("a".to_string(), FLAG_ADDED);
    assert_eq!(
        merge_status(&range, &status)["a"],
        FLAG_ADDED | FLAG_MODIFIED
    );
    assert_eq!(get_display_hash("0123456789abcdef"), "0123456789ab");
    let root = std::path::Path::new("/r");
    assert_eq!(join_path(root, None), root.to_path_buf());
    assert_eq!(join_path(root, Some("a")), root.join("a"));
    let dir = std::env::temp_dir().join(format!("arc-rs-svn-scratch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(write_scratch_file(&dir, "x", "data"));
    assert_eq!(read_scratch_file(&dir, "x").as_deref(), Some("data"));
    assert!(remove_scratch_file(&dir, "x"));
    assert!(!remove_scratch_file(&dir, "x"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn walks_to_metadata() {
    let base = std::env::temp_dir().join(format!("arc-rs-svn-meta-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let deep = base.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(find_metadata_path(&deep), None);
    std::fs::create_dir_all(base.join(".svn")).unwrap();
    assert_eq!(find_metadata_path(&deep), Some(base.join(".svn")));
    assert_eq!(get_metadata_path(&base), Some(base.join(".svn")));
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn runner_reports_exit_status() {
    let fake = FakeSvn::named("runner");
    assert!(ok_with(&fake.dir, &fake.svn, &["--xml", "status"]));
    assert!(!ok_with(&fake.dir, &fake.svn, &["cat", "absent.txt@"]));
    let out = output_with(&fake.dir, &fake.svn, &["--xml", "status"]).unwrap();
    assert!(out.status.success());
    let manual = manual_with(&fake.dir, &fake.svn, &["cat", "absent.txt@"]).unwrap();
    assert_ne!(manual.code, 0);
    assert!(stdout_text_with(&fake.dir, &fake.svn, &["cat", "absent.txt@"]).is_err());
    assert!(fake.log().contains("--xml status"));
}

#[test]
fn reads_status_and_revisions() {
    let fake = FakeSvn::named("status");
    fake.write(".fakesvn-status", STATUS_XML.as_bytes());
    let status = get_svn_status_with(&fake.dir, &fake.svn, &[], false).unwrap();
    assert_eq!(status.files["m.txt"], FLAG_MODIFIED);
    assert_eq!(status.base_revisions["m.txt"], "42");
    let uncommitted = get_uncommitted_status_with(&fake.dir, &fake.svn, &["m.txt"]).unwrap();
    assert_eq!(uncommitted["m.txt"], FLAG_MODIFIED);
    assert!(fake.log().contains("--xml status m.txt"));
}

#[test]
fn reads_info_derivatives() {
    let fake = FakeSvn::named("info");
    fake.write(".fakesvn-info", INFO_TEXT.as_bytes());
    let info = get_svn_info_with(&fake.dir, &fake.svn, "/").unwrap();
    assert_eq!(info["Revision"], "42");
    assert_eq!(
        get_branch_name_with(&fake.dir, &fake.svn).unwrap(),
        "/trunk"
    );
    assert_eq!(
        get_remote_uri_with(&fake.dir, &fake.svn)
            .unwrap()
            .as_deref(),
        Some("https://svn.example.test/repo")
    );
    assert_eq!(
        get_repository_uuid_with(&fake.dir, &fake.svn)
            .unwrap()
            .as_deref(),
        Some("uuid-1")
    );
    assert_eq!(
        get_source_control_path_with(&fake.dir, &fake.svn)
            .unwrap()
            .as_deref(),
        Some("https://svn.example.test/repo/trunk")
    );
    assert_eq!(
        get_working_copy_revision_with(&fake.dir, &fake.svn, None)
            .unwrap()
            .as_deref(),
        Some("https://svn.example.test/repo/trunk@42")
    );
    assert_eq!(
        get_working_copy_revision_with(&fake.dir, &fake.svn, Some("9"))
            .unwrap()
            .as_deref(),
        Some("https://svn.example.test/repo/trunk@9")
    );
    assert!(fake.log().contains("info ."));
    assert_eq!(
        get_svn_property_with(&fake.dir, &fake.svn, "a.txt", "svn:mime-type").unwrap(),
        "text/plain"
    );
}

#[test]
fn reports_info_failures() {
    let failing = FakeSvn::named("infofail");
    failing.write(".fakesvn-fail-info", b"");
    let err = get_svn_info_with(&failing.dir, &failing.svn, "/").unwrap_err();
    assert!(
        err.contains("Error #1 executing svn info against '/'."),
        "{err}"
    );
    let empty = FakeSvn::named("infoempty");
    let err = get_svn_info_with(&empty.dir, &empty.svn, "/").unwrap_err();
    assert!(err.contains("Unable to parse SVN info."), "{err}");
}

#[test]
fn reads_files_lists_and_blame() {
    let fake = FakeSvn::named("files");
    assert_eq!(
        get_original_file_data_with(&fake.dir, &fake.svn, "a.txt").unwrap(),
        Some(b"file-bytes".to_vec())
    );
    assert_eq!(
        get_original_file_data_with(&fake.dir, &fake.svn, "absent.txt").unwrap(),
        None
    );
    fake.write("a.txt", b"current");
    assert_eq!(
        get_current_file_data(&fake.dir, "a.txt"),
        Some(b"current".to_vec())
    );
    assert_eq!(get_current_file_data(&fake.dir, "missing.txt"), None);
    let bulk =
        get_bulk_original_file_data_with(&fake.dir, &fake.svn, &["a.txt", "absent.txt"]).unwrap();
    assert_eq!(bulk["a.txt"], Some(b"file-bytes".to_vec()));
    assert_eq!(bulk["absent.txt"], None);
    let bulk = get_bulk_current_file_data(&fake.dir, &["a.txt", "missing.txt"]);
    assert_eq!(bulk["a.txt"], Some(b"current".to_vec()));
    assert_eq!(bulk["missing.txt"], None);
    fake.write(".fakesvn-list", b"a.txt\nsrc/\n");
    assert_eq!(
        get_all_files_with(&fake.dir, &fake.svn).unwrap(),
        vec!["a.txt".to_string()]
    );
    fake.write(".fakesvn-blame", b"      41 alice      one\n");
    assert_eq!(
        get_blame_with(&fake.dir, &fake.svn, "a.txt").unwrap(),
        vec![("alice".to_string(), "41".to_string())]
    );
    assert!(fake.log().contains("cat a.txt@"));
}

#[test]
fn reads_changed_files() {
    let fake = FakeSvn::named("changed");
    fake.write(
        ".fakesvn-diffsumm",
        br#"<diff><paths><path props="none" item="modified" kind="file">u/a.txt</path></paths></diff>"#,
    );
    let map = get_changed_files_with(&fake.dir, &fake.svn, "https://x/repo@40").unwrap();
    assert_eq!(map["u/a.txt"], FLAG_MODIFIED);
    let log = fake.log();
    assert!(
        log.contains("--revision 40:HEAD --summarize https://x/repo"),
        "{log}"
    );
}

#[test]
fn adds_and_updates() {
    let fake = FakeSvn::named("add");
    fake.write("kept.txt", b"x");
    add_to_commit_with(&fake.dir, &fake.svn, &["kept.txt", "gone.txt"]).unwrap();
    update_working_copy_with(&fake.dir, &fake.svn).unwrap();
    let log = fake.log();
    assert!(log.contains("add -- kept.txt"), "{log}");
    assert!(log.contains("delete -- gone.txt"), "{log}");
    assert!(log.contains("up"), "{log}");
}

fn status_of(pairs: &[(&str, u32)]) -> StatusMap {
    pairs.iter().map(|(p, m)| ((*p).to_string(), *m)).collect()
}

#[test]
fn diffs_known_paths() {
    let fake = FakeSvn::named("diff");
    fake.write(".fakesvn-diff", b"Index: a.txt\n--- old\n+++ new\n");
    let status = status_of(&[("a.txt", FLAG_MODIFIED)]);
    let config = SvnDiffConfig::windows(3);
    let diff = get_raw_diff_text_with(&fake.dir, &fake.svn, "a.txt", &status, &config)
        .unwrap()
        .unwrap();
    assert_eq!(diff, b"Index: a.txt\n--- old\n+++ new\n");
    assert_eq!(
        get_raw_diff_text_with(&fake.dir, &fake.svn, "unknown.txt", &status, &config).unwrap(),
        None
    );
    let log = fake.log();
    assert!(log.contains("diff --depth empty a.txt"), "{log}");
    let unix_config = SvnDiffConfig::unix(3, "/scripts/binary_safe_diff.sh");
    get_raw_diff_text_with(&fake.dir, &fake.svn, "a.txt", &status, &unix_config).unwrap();
    assert!(
        fake.log()
            .contains("--diff-cmd /scripts/binary_safe_diff.sh -x -U3"),
        "{}",
        fake.log() // LCOV_EXCL_LINE // Reason: assert format arg; evaluated only on failure
    );
}

#[test]
fn diffs_empty_output_as_unchanged() {
    let fake = FakeSvn::named("diffempty");
    fake.write("a.txt", b"one\ntwo\n");
    let status = status_of(&[("a.txt", FLAG_MODIFIED)]);
    let diff = get_raw_diff_text_with(
        &fake.dir,
        &fake.svn,
        "a.txt",
        &status,
        &SvnDiffConfig::windows(3),
    )
    .unwrap()
    .unwrap();
    let text = String::from_utf8(diff).unwrap();
    assert!(text.contains("--- a.txt (synthetic)"), "{text}");
    assert!(text.contains("@@ -1,3 +1,3 @@"), "{text}");
    assert_eq!(
        build_synthetic_unchanged_diff(&fake.dir, "missing.txt"),
        None
    );
    std::fs::create_dir_all(fake.dir.join("sub")).unwrap();
    assert_eq!(build_synthetic_unchanged_diff(&fake.dir, "sub"), None);
}

#[test]
fn diffs_binary_files() {
    let fake = FakeSvn::named("bindiff");
    let deleted = status_of(&[("a.png", FLAG_DELETED)]);
    let body = get_raw_diff_text_with(
        &fake.dir,
        &fake.svn,
        "a.png",
        &deleted,
        &SvnDiffConfig::windows(3),
    )
    .unwrap()
    .unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), binary_diff_text("a.png"));
    assert!(!fake.log().contains("diff --depth"));

    fake.write(".fakesvn-diff", b"canned\n");
    let modified = status_of(&[("a.png", FLAG_MODIFIED)]);
    get_raw_diff_text_with(
        &fake.dir,
        &fake.svn,
        "a.png",
        &modified,
        &SvnDiffConfig::windows(3),
    )
    .unwrap();
    assert!(
        fake.log()
            .contains("propset svn:mime-type application/octet-stream"),
        "{}",
        fake.log() // LCOV_EXCL_LINE // Reason: assert format arg; evaluated only on failure
    );

    let octet = FakeSvn::named("octet");
    octet.write(".fakesvn-diff", b"canned\n");
    octet.write(".fakesvn-propget", b"application/octet-stream\n");
    get_raw_diff_text_with(
        &octet.dir,
        &octet.svn,
        "a.png",
        &modified,
        &SvnDiffConfig::windows(3),
    )
    .unwrap();
    assert!(!octet.log().contains("propset"), "{}", octet.log());
}

#[test]
fn tolerates_diff_exit_two_only() {
    let tolerated = FakeSvn::named("diff2");
    tolerated.write(".fakesvn-diff", b"canned\n");
    tolerated.write(".fakesvn-diff2", b"");
    tolerated.write(".fakesvn-diff2err", b"svn: 'diff' returned 2\n");
    let status = status_of(&[("a.txt", FLAG_MODIFIED)]);
    let diff = get_raw_diff_text_with(
        &tolerated.dir,
        &tolerated.svn,
        "a.txt",
        &status,
        &SvnDiffConfig::windows(3),
    )
    .unwrap()
    .unwrap();
    assert_eq!(diff, b"canned\n");
    let failing = FakeSvn::named("difffail");
    failing.write(".fakesvn-fail-diff", b"");
    let err = get_raw_diff_text_with(
        &failing.dir,
        &failing.svn,
        "a.txt",
        &status,
        &SvnDiffConfig::windows(3),
    )
    .unwrap_err();
    assert!(
        err.contains("svn diff returned unexpected error code: 1"),
        "{err}"
    );
}

#[test]
fn diffs_copied_additions() {
    let fake = FakeSvn::named("copyadd");
    fake.write("copied.txt", b"new data\n");
    fake.write(
        ".fakesvn-info",
        b"URL: https://svn.example.test/repo/trunk\nRepository Root: https://svn.example.test/repo\nRevision: 42\nCopied From URL: https://svn.example.test/repo/trunk/orig.txt\nCopied From Rev: 40\n",
    );
    fake.write(".fakesvn-cat-url", b"old data\n");
    fake.write(
        ".fakediff-canned",
        b"--- a/orig.txt\n+++ b/copied.txt\n@@ -1 +1 @@\n-old\n+new\n",
    );
    let (program, prefix) = FakeSvn::install_diff();
    let prefix_refs: Vec<&str> = prefix.iter().map(String::as_str).collect();
    let config = SvnDiffConfig {
        context: 3,
        diff_cmd: None,
        diff_program: &program,
        diff_prefix: &prefix_refs,
    };
    let status = status_of(&[("copied.txt", FLAG_ADDED)]);
    let diff = get_raw_diff_text_with(&fake.dir, &fake.svn, "copied.txt", &status, &config)
        .unwrap()
        .unwrap();
    let text = String::from_utf8(diff).unwrap();
    assert!(text.starts_with("Index: copied.txt\n==="), "{text}");
    assert!(text.contains("+++ b/copied.txt"), "{text}");
    let diff_log = std::fs::read_to_string(fake.dir.join(".fakediff.log"))
        .unwrap()
        .replace('"', "");
    assert!(
        diff_log.contains("-L a/orig.txt -L b/copied.txt -U3"),
        "{diff_log}"
    );
    assert!(
        fake.log()
            .contains("cat https://svn.example.test/repo/trunk/orig.txt@40"),
        "{}",
        fake.log() // LCOV_EXCL_LINE // Reason: assert format arg; evaluated only on failure
    );

    let same = FakeSvn::named("copysame");
    same.write("copied.txt", b"same\n");
    same.write(".fakesvn-info", INFO_TEXT.as_bytes());
    same.write(".fakesvn-cat-url", b"same\n");
    same.write(".fakediff-empty", b"");
    let (program, prefix) = FakeSvn::install_diff();
    let prefix_refs: Vec<&str> = prefix.iter().map(String::as_str).collect();
    let config = SvnDiffConfig {
        context: 3,
        diff_cmd: None,
        diff_program: &program,
        diff_prefix: &prefix_refs,
    };
    let diff = get_raw_diff_text_with(&same.dir, &same.svn, "copied.txt", &status, &config)
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(diff).unwrap().contains("(synthetic)"));
}

#[test]
fn addition_short_circuits() {
    let fake = FakeSvn::named("addshort");
    std::fs::create_dir_all(fake.dir.join("sub")).unwrap();
    let config = SvnDiffConfig::windows(3);
    assert_eq!(
        build_synthetic_addition_diff_with(&fake.dir, &fake.svn, "sub", "s", "1", "scp", &config)
            .unwrap(),
        None
    );
    fake.write("a.png", b"bytes");
    fake.write(".fakesvn-propget", b"application/octet-stream\n");
    let body =
        build_synthetic_addition_diff_with(&fake.dir, &fake.svn, "a.png", "s", "1", "scp", &config)
            .unwrap()
            .unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), binary_diff_text("a.png"));
}

#[test]
fn added_without_copy_uses_plain_diff() {
    let fake = FakeSvn::named("plainadd");
    fake.write(".fakesvn-diff", b"canned\n");
    fake.write(
        ".fakesvn-info",
        b"URL: https://x/repo/a.txt\nRevision: 42\nRepository Root: https://x/repo\n",
    );
    let status = status_of(&[("a.txt", FLAG_ADDED)]);
    let diff = get_raw_diff_text_with(
        &fake.dir,
        &fake.svn,
        "a.txt",
        &status,
        &SvnDiffConfig::windows(3),
    )
    .unwrap()
    .unwrap();
    assert_eq!(diff, b"canned\n");
}

/// ETXTBSY retry: hold a script open for writing, release it mid-retry,
/// and the spawn must succeed. Unix-only (Windows has no ETXTBSY).
#[cfg(unix)]
#[test]
fn output_retries_while_script_is_busy() {
    let fake = FakeSvn::named("busy");
    let sh = fake.dir.join("busy.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh
echo hi
",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&sh).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sh, perms).unwrap();
    }
    let held = std::fs::OpenOptions::new().write(true).open(&sh).unwrap();
    let svn = SvnCommand::custom(&sh.to_string_lossy(), &[]);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(held);
    });
    let out = output_with(&fake.dir, &svn, &[]).unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    releaser.join().unwrap();
}

/// True when exec refuses a write-held script with ETXTBSY (Linux
/// does; macOS does not enforce it). Probed once per process so the
/// stuck-script test pins behavior only where the errno exists.
#[cfg(unix)]
fn platform_reports_txtbsy() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| probe_txtbsy().unwrap_or(false))
}

#[cfg(unix)]
fn probe_txtbsy() -> Option<bool> {
    let dir = std::env::temp_dir().join(format!("arc-rs-txtbsy-probe-svn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let sh = dir.join("probe.sh");
    std::fs::write(&sh, "#!/bin/sh\nexit 0\n").ok()?;
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&sh).ok()?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sh, perms).ok()?;
    }
    let _held = std::fs::OpenOptions::new().write(true).open(&sh).ok()?;
    let failed_busy = std::process::Command::new(&sh)
        .output()
        .is_err_and(|error| error.raw_os_error() == Some(26));
    std::fs::remove_dir_all(&dir).ok();
    Some(failed_busy)
}

#[test]
fn output_retry_loop_is_bounded() {
    // A permanently busy spawn fails after the bound with the last
    // error. The predicate injection forces the ETXTBSY path where
    // the platform never reports it (macOS, Windows).
    let dir = std::env::temp_dir().join(format!("arc-rs-svn-retry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cmd = std::process::Command::new("definitely-not-a-real-binary-t1170");
    cmd.current_dir(&dir);
    let err = spawn_output_retry(&mut cmd, &|_| true).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lang_env_var_falls_back_when_probe_fails() {
    let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no locale probe");
    assert_eq!(lang_env_var_from_probe(Err(err)), "en_US.utf8");
}

/// A permanently busy script still fails (after the bound) with the
/// original error shape.
#[cfg(unix)]
#[test]
fn output_gives_up_on_permanently_busy_script() {
    if !platform_reports_txtbsy() {
        return; // LCOV_EXCL_LINE // Reason: probe is always true where gates measure (Linux reports ETXTBSY); the skip only trips on macOS
    } // LCOV_EXCL_START // Reason: ETXTBSY-only test body; macOS exec never reports text-file-busy (probe-gated)
    let fake = FakeSvn::named("stuck");
    let sh = fake.dir.join("stuck.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh
echo hi
",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&sh).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sh, perms).unwrap();
    }
    let _held = std::fs::OpenOptions::new().write(true).open(&sh).unwrap();
    let svn = SvnCommand::custom(&sh.to_string_lossy(), &[]);
    let err = output_with(&fake.dir, &svn, &[]).unwrap_err();
    assert!(err.contains("failed to run"), "{err}"); // LCOV_EXCL_STOP
}
#[test]
fn normalizes_status_path_separators() {
    assert_eq!(normalize_status_path("sub\\file.txt"), "sub/file.txt");
    assert_eq!(normalize_status_path("plain/path.txt"), "plain/path.txt");
    assert_eq!(normalize_status_path(""), "");
}

#[test]
fn apply_externals_flags_only_paths_under_externals() {
    let mut files = status_of(&[
        ("lib/a.txt", FLAG_MODIFIED),
        ("lib/sub/b.txt", FLAG_MODIFIED),
        ("lib-other/c.txt", FLAG_MODIFIED),
        ("top.txt", FLAG_MODIFIED),
    ]);
    apply_externals(&mut files, &["lib".to_string()]);
    assert_eq!(files["lib/a.txt"], FLAG_MODIFIED | FLAG_EXTERNALS);
    assert_eq!(files["lib/sub/b.txt"], FLAG_MODIFIED | FLAG_EXTERNALS);
    assert_eq!(files["lib-other/c.txt"], FLAG_MODIFIED);
    assert_eq!(files["top.txt"], FLAG_MODIFIED);
}

#[test]
fn paths_with_mask_selects_flagged_paths_in_order() {
    let files = status_of(&[
        ("b.txt", FLAG_UNTRACKED),
        ("a.txt", FLAG_MODIFIED | FLAG_UNTRACKED),
        ("c.txt", FLAG_MODIFIED),
    ]);
    assert_eq!(
        paths_with_mask(&files, FLAG_UNTRACKED),
        vec!["a.txt".to_string(), "b.txt".to_string()]
    );
    assert!(paths_with_mask(&files, FLAG_CONFLICT).is_empty());
}

#[test]
fn migrates_legacy_scratch_dir() {
    let base = std::env::temp_dir().join(format!("arc-rs-svn-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let meta = base.join("wc").join(".svn");
    std::fs::create_dir_all(base.join("old").join(".arc")).unwrap();
    std::fs::write(base.join("old").join(".arc").join("x"), "data").unwrap();
    assert!(migrate_scratch_dir(&base.join("old"), &meta));
    assert_eq!(read_scratch_file(&meta, "x").as_deref(), Some("data"));
    assert!(!base.join("old").join(".arc").exists());

    // Nothing to migrate: missing old dir, or an existing new dir.
    assert!(migrate_scratch_dir(&base.join("absent"), &meta));
    std::fs::create_dir_all(base.join("both").join(".arc")).unwrap();
    std::fs::write(base.join("both").join(".arc").join("x"), "old").unwrap();
    assert!(write_scratch_file(&meta, "x", "new"));
    assert!(migrate_scratch_dir(&base.join("both"), &meta));
    assert_eq!(read_scratch_file(&meta, "x").as_deref(), Some("new"));

    // A subdirectory aborts the migration without deleting anything.
    std::fs::create_dir_all(base.join("sub").join(".arc").join("dir")).unwrap();
    std::fs::write(base.join("sub").join(".arc").join("x"), "data").unwrap();
    assert!(!migrate_scratch_dir(
        &base.join("sub"),
        &base.join("fresh-meta")
    ));
    assert!(base.join("sub").join(".arc").join("x").exists());
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn selects_svn_locale() {
    assert_eq!(select_svn_locale("C\nen_US.UTF-8\nPOSIX\n"), "en_US.UTF-8");
    assert_eq!(select_svn_locale("C\nPOSIX\n"), "en_US.utf8");
    assert_eq!(select_svn_locale(""), "en_US.utf8");
    assert_eq!(select_svn_locale("en_US.UTF-8 \n"), "en_US.utf8");
}

#[test]
fn auto_locale_is_a_known_value() {
    assert!(["en_US.UTF-8", "en_US.utf8"].contains(&svn_lang_env_var()));
}

#[test]
fn commit_passes_files_encoding_and_message_file() {
    let fake = FakeSvn::named("commit");
    commit_with_locale(&fake.dir, &fake.svn, &["a.txt", "b.txt"], "msg\n", None).unwrap();
    let log = fake.log();
    assert!(
        log.contains("commit a.txt b.txt --encoding utf-8 -F"),
        "{log}"
    );
}

#[test]
fn commit_failure_reports_the_php_sentence() {
    // T1153 CORE-65: the failure text is the bare PHP sentence
    // (`ArcanistCommitWorkflow.php:163` at ceb); the live child output
    // is already on the console, so nothing rides along.
    let fake = FakeSvn::named("commit-fail");
    fake.write(".fakesvn-fail-commit", b"");
    let err = commit_with_locale(&fake.dir, &fake.svn, &["a.txt"], "msg\n", None).unwrap_err();
    assert_eq!(err, "Executing 'svn commit' failed!");
}

#[test]
fn commit_sets_lang_for_the_child() {
    // T1153 CORE-65: the child streams live (nothing is captured), so
    // the probe records `LANG` in a file instead of stdout.
    let dir = std::env::temp_dir().join(format!("arc-rs-svn-lang-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let seen = dir.join("lang.seen");
    #[cfg(windows)]
    let svn = {
        let bat = dir.join("langprobe.bat");
        std::fs::write(
            &bat,
            format!("@echo off\r\necho LANG=%LANG%>{}\r\n", seen.display()),
        )
        .unwrap();
        SvnCommand::custom("cmd", &["/C", &bat.to_string_lossy()])
    };
    #[cfg(not(windows))]
    let svn = {
        let sh = dir.join("langprobe.sh");
        std::fs::write(
            &sh,
            format!("#!/bin/sh\necho \"LANG=$LANG\" > \"{}\"\n", seen.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&sh).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&sh, perms).unwrap();
        }
        SvnCommand::custom(&sh.to_string_lossy(), &[])
    };
    commit_with_locale(&dir, &svn, &[], "msg\n", Some("en_US.UTF-8")).unwrap();
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap().trim(),
        "LANG=en_US.UTF-8"
    );
    #[cfg(not(windows))]
    {
        std::fs::remove_file(&seen).ok();
        commit_with(&dir, &svn, &[], "msg\n").unwrap();
        let recorded = std::fs::read_to_string(&seen).unwrap();
        assert!(recorded.contains("LANG=en_US."), "unexpected: {recorded}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core65_commit_streams_both_pipes_live() {
    // T1153 CORE-65: like `phutil_passthru`, both pipes inherit the
    // terminal (two lines land on the test's console here); only the
    // status comes back.
    let dir = std::env::temp_dir().join(format!("arc-rs-svn-pipes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(windows)]
    let svn = {
        let bat = dir.join("pipes.bat");
        std::fs::write(&bat, "@echo off\r\necho Sending .\r\necho noise>&2\r\n").unwrap();
        SvnCommand::custom("cmd", &["/C", &bat.to_string_lossy()])
    };
    #[cfg(not(windows))]
    let svn = {
        let sh = dir.join("pipes.sh");
        std::fs::write(&sh, "#!/bin/sh\necho \"Sending .\"\necho noise >&2\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&sh).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&sh, perms).unwrap();
        }
        SvnCommand::custom(&sh.to_string_lossy(), &[])
    };
    commit_with_locale(&dir, &svn, &[], "msg\n", None).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/* T1170 coverage: system-binary wrappers fail closed without a repo. A
 * spawned `svn` can never start in a missing directory, so every wrapper
 * below is deterministic whether or not `svn` is installed. */

/// Definitely-missing directory: every spawn fails before `svn` runs.
fn t1170_bogus_dir() -> PathBuf {
    std::env::temp_dir().join("arc-rs-svn-t1170-bogus-dir-that-cannot-exist")
}

#[test]
fn t1170_system_runners_fail_closed_on_bogus_dir() {
    let dir = t1170_bogus_dir();
    assert!(!dir.exists());
    assert!(output(&dir, &["--version"]).is_err());
    assert!(stdout_text(&dir, &["--version"]).is_err());
    assert!(!ok(&dir, &["--version"]));
    assert!(manual(&dir, &["--version"]).is_err());
}

#[test]
fn t1170_system_readers_fail_closed_on_bogus_dir() {
    let dir = t1170_bogus_dir();
    assert!(!dir.exists());
    assert!(get_source_control_path(&dir).is_err());
    assert!(get_remote_uri(&dir).is_err());
    assert!(get_repository_uuid(&dir).is_err());
    assert!(get_svn_status(&dir, &[], false).is_err());
    assert!(get_uncommitted_status(&dir, &[]).is_err());
    assert!(get_svn_info(&dir, "/").is_err());
    assert!(get_svn_property(&dir, "a.txt", "svn:mime-type").is_err());
    assert!(get_working_copy_revision(&dir, None).is_err());
    assert!(get_branch_name(&dir).is_err());
    assert!(get_all_files(&dir).is_err());
    assert!(get_changed_files(&dir, "https://x/repo@40").is_err());
    assert!(get_blame(&dir, "a.txt").is_err());
    assert!(get_original_file_data(&dir, "a.txt").is_err());
    assert!(get_bulk_original_file_data(&dir, &["a.txt"]).is_err());
}

#[test]
fn t1170_system_writers_fail_closed_on_bogus_dir() {
    let dir = t1170_bogus_dir();
    assert!(!dir.exists());
    assert!(add_to_commit(&dir, &["a.txt"]).is_err());
    assert!(update_working_copy(&dir).is_err());
    assert!(commit(&dir, &["a.txt"], "msg\n").is_err());
    assert!(build_synthetic_addition_diff(&dir, "a.txt", "s", "1", "scp", 3).is_err());
    // Unknown paths short-circuit before any spawn.
    assert_eq!(
        get_raw_diff_text(&dir, "unknown.txt", &StatusMap::new(), 3).unwrap(),
        None
    );
}

#[test]
fn stdout_text_reports_failing_output() {
    // A failing invocation with output reports the trimmed combination
    // (the silent-failure sentence is covered elsewhere).
    let fake = FakeSvn::named("noisyfail");
    fake.write(".fakesvn-diff", b"canned\n");
    fake.write(".fakesvn-diff2", b"");
    fake.write(".fakesvn-diff2err", b"boom\n");
    let err = stdout_text_with(&fake.dir, &fake.svn, &["diff"]).unwrap_err();
    assert!(err.contains("canned"), "{err}");
    assert!(err.contains("boom"), "{err}");
}

fn status_xml_for(entries: &str) -> String {
    format!("<status><target path=\".\">{entries}</target></status>")
}

fn entry_xml(path_attr: &str) -> String {
    format!(
        "<entry path=\"{path_attr}\"><wc-status props=\"none\" item=\"modified\" revision=\"1\"></wc-status></entry>"
    )
}

#[test]
fn decodes_xml_entities_in_status_paths() {
    let xml = status_xml_for(&format!(
        "{}{}{}{}",
        entry_xml("a&#x41;.txt"),
        entry_xml("b&#X42;.txt"),
        entry_xml("c&#65;.txt"),
        entry_xml("a&lt;b&gt;c&quot;d&apos;e.txt"),
    ));
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["aA.txt"], FLAG_MODIFIED);
    assert_eq!(status.files["bB.txt"], FLAG_MODIFIED);
    assert_eq!(status.files["cA.txt"], FLAG_MODIFIED);
    assert_eq!(status.files["a<b>c\"d'e.txt"], FLAG_MODIFIED);
    // Unknown entities survive verbatim.
    let xml = status_xml_for(&entry_xml("a&bogus;.txt"));
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["a&bogus;.txt"], FLAG_MODIFIED);
    // A bare `&` without `;` survives verbatim.
    let xml = status_xml_for(&entry_xml("a&b.txt"));
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["a&b.txt"], FLAG_MODIFIED);
    // Entities holding `<` or `&` are not entities at all.
    let xml = status_xml_for(&entry_xml("x&y<z;.txt"));
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["x&y<z;.txt"], FLAG_MODIFIED);
    let xml = status_xml_for(&entry_xml("a&b&c;.txt"));
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["a&b&c;.txt"], FLAG_MODIFIED);
}

#[test]
fn tolerates_odd_tag_shapes_in_status_xml() {
    // Whitespace around `=` still parses the attribute.
    let xml = status_xml_for(
        "<entry path = \"m.txt\"><wc-status props=\"none\" item=\"modified\" revision=\"1\"></wc-status></entry>",
    );
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["m.txt"], FLAG_MODIFIED);
    // Malformed attributes halt the tag, leaving the path empty.
    for tag in ["<entry path>", "<entry path=>", "<entry path=x>"] {
        let xml = status_xml_for(&format!(
            "{tag}<wc-status props=\"none\" item=\"modified\" revision=\"1\"></wc-status></entry>"
        ));
        let status = parse_svn_status_xml(&xml).unwrap();
        assert_eq!(status.files[""], FLAG_MODIFIED, "{tag}");
    }
    // `<entryfoo` is not an entry; scanning resumes after it.
    let xml = status_xml_for(&format!(
        "<entryfoo path=\"x\"></entryfoo>{}",
        entry_xml("m.txt")
    ));
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["m.txt"], FLAG_MODIFIED);
    // Truncated input ends the scan without error.
    let status = parse_svn_status_xml("<status><target path=\".\"><entry path=\"x\"").unwrap();
    assert!(status.files.is_empty());
    let status = parse_svn_status_xml(
        "<status><target path=\".\"><entry path=\"x\"><wc-status props=\"none\" item=\"modified\" revision=\"1\"/></target></status>",
    )
    .unwrap();
    assert!(status.files.is_empty());
    // A self-closing entry has no `wc-status`, so its empty props fail.
    let xml = status_xml_for("<entry path=\"x\"/>");
    assert!(
        parse_svn_status_xml(&xml)
            .unwrap_err()
            .contains("Unrecognized property status ''")
    );
    // `<wc-status-foo` is not a status; scanning resumes after it.
    let xml = status_xml_for(
        "<entry path=\"x\"><wc-status-foo/><wc-status props=\"none\" item=\"modified\" revision=\"1\"/></entry>",
    );
    let status = parse_svn_status_xml(&xml).unwrap();
    assert_eq!(status.files["x"], FLAG_MODIFIED);
}

#[test]
fn tolerates_odd_tag_shapes_in_diff_summarize_xml() {
    let map = parse_svn_diff_summarize_xml("<diff><paths><path item=\"modified\"").unwrap();
    assert!(map.is_empty());
    let map = parse_svn_diff_summarize_xml(
        "<diff><paths><path item=\"modified\" kind=\"file\">u/a.txt</paths></diff>",
    )
    .unwrap();
    assert!(map.is_empty());
    let map = parse_svn_diff_summarize_xml(
        "<diff><paths><path item=\"modified\" kind=\"file\"/></paths></diff>",
    )
    .unwrap();
    assert_eq!(map[""], FLAG_MODIFIED);
}

#[test]
fn skips_info_values_with_whitespace() {
    // A simple key whose value is empty or holds whitespace does not
    // match, like PHP's `\S+` value pattern.
    let info = parse_svn_info("URL: has space\nRevision: 42\n").unwrap();
    assert!(!info.contains_key("URL"));
    assert_eq!(info["Revision"], "42");
    let info = parse_svn_info("URL: \nRevision: 42\n").unwrap();
    assert!(!info.contains_key("URL"));
    assert_eq!(info["Revision"], "42");
}

#[test]
fn rejects_bad_svn_dates() {
    assert_eq!(parse_svn_date("2026-10-13 09:00:00 xx"), None);
    assert_eq!(parse_svn_date("2026-10-13 09:00:00 +2500"), None);
    assert_eq!(parse_svn_date("2026-10-13 09:00:00 +0061"), None);
    assert_eq!(parse_svn_date("2026-10-13 25:00:00 +0000"), None);
    assert_eq!(parse_svn_date("2026-10-13 09:61:00 +0000"), None);
    assert_eq!(parse_svn_date("2026-10-13 09:00:61 +0000"), None);
    assert_eq!(parse_svn_date("2026-10-13 09:00:00:00 +0000"), None);
    assert_eq!(parse_svn_date("2026-10-13 09:00:00 +0000 extra"), None);
    // A negative zone shifts the timestamp forward: 04:00 -0500 is
    // the same instant as 09:00 +0000 (1791882000).
    assert_eq!(
        parse_svn_date("2026-10-13 04:00:00 -0500"),
        Some(1791882000)
    );
}

#[test]
fn ignores_dateless_info_lines() {
    // `Last Changed Date` without its ` (...)` group never matches.
    let info =
        parse_svn_info("URL: https://a/\nLast Changed Date: 2026-10-13 09:00:00 +0000\n").unwrap();
    assert!(!info.contains_key("Last Changed Date"));
    // Empty values and empty groups never match either.
    let info = parse_svn_info("URL: https://a/\nLast Changed Date:  ()\n").unwrap();
    assert!(!info.contains_key("Last Changed Date"));
    let info = parse_svn_info("URL: https://a/\nLast Changed Date: v ()\n").unwrap();
    assert!(!info.contains_key("Last Changed Date"));
}

#[test]
fn parses_php_numeric_exponents() {
    assert_eq!(get_canonical_revision_name("1e3"), Some("1e3".to_string()));
    assert_eq!(
        get_canonical_revision_name("1E+3"),
        Some("1E+3".to_string())
    );
    assert_eq!(
        get_canonical_revision_name("1.5e-3"),
        Some("1.5e-3".to_string())
    );
    assert_eq!(get_canonical_revision_name("42e"), None);
    assert_eq!(get_canonical_revision_name("1.5e+"), None);
}

#[test]
fn rejects_bad_blame_lines() {
    let err = parse_svn_blame("42x foo\n").unwrap_err();
    assert!(err.contains("Bad blame? `42x foo'"), "{err}");
    let err = parse_svn_blame("42  \n43 alice x\n").unwrap_err();
    assert!(err.contains("Bad blame? `42  '"), "{err}");
}

#[test]
fn reports_synthetic_addition_cat_failure() {
    let fake = FakeSvn::named("catfail");
    fake.write("copied.txt", b"new data\n");
    fake.write(
        ".fakesvn-info",
        b"URL: https://svn.example.test/repo/trunk\nRepository Root: https://svn.example.test/repo\nRevision: 42\nCopied From URL: https://svn.example.test/repo/trunk/orig.txt\nCopied From Rev: 40\n",
    );
    fake.write(".fakesvn-fail-cat", b"");
    let (program, prefix) = FakeSvn::install_diff();
    let prefix_refs: Vec<&str> = prefix.iter().map(String::as_str).collect();
    let config = SvnDiffConfig {
        context: 3,
        diff_cmd: None,
        diff_program: &program,
        diff_prefix: &prefix_refs,
    };
    let status = status_of(&[("copied.txt", FLAG_ADDED)]);
    let err =
        get_raw_diff_text_with(&fake.dir, &fake.svn, "copied.txt", &status, &config).unwrap_err();
    assert!(err.contains("cat"), "{err}");
    assert!(err.contains("failed"), "{err}");
}

#[test]
fn reports_synthetic_addition_diff_failure() {
    // An unspawnable `diff` program fails the whole synthetic diff.
    let fake = FakeSvn::named("nodiff");
    fake.write("copied.txt", b"new data\n");
    fake.write(
        ".fakesvn-info",
        b"URL: https://svn.example.test/repo/trunk\nRepository Root: https://svn.example.test/repo\nRevision: 42\nCopied From URL: https://svn.example.test/repo/trunk/orig.txt\nCopied From Rev: 40\n",
    );
    fake.write(".fakesvn-cat-url", b"old data\n");
    let config = SvnDiffConfig {
        context: 3,
        diff_cmd: None,
        diff_program: "definitely-not-a-diff-binary-t1170",
        diff_prefix: &[],
    };
    let status = status_of(&[("copied.txt", FLAG_ADDED)]);
    let err =
        get_raw_diff_text_with(&fake.dir, &fake.svn, "copied.txt", &status, &config).unwrap_err();
    assert!(err.contains("failed to run"), "{err}");
}

#[test]
fn reports_binary_propset_failure() {
    let fake = FakeSvn::named("propsetfail");
    fake.write("a.png", b"bytes");
    fake.write(".fakesvn-fail-propset", b"");
    let status = status_of(&[("a.png", FLAG_MODIFIED)]);
    let err = get_raw_diff_text_with(
        &fake.dir,
        &fake.svn,
        "a.png",
        &status,
        &SvnDiffConfig::windows(3),
    )
    .unwrap_err();
    assert!(err.contains("failed"), "{err}");
}

#[test]
fn reports_changed_files_failure() {
    let fake = FakeSvn::named("summfail");
    fake.write(".fakesvn-fail-diff", b"");
    let err = get_changed_files_with(&fake.dir, &fake.svn, "https://x/repo@40").unwrap_err();
    assert!(err.contains("failed"), "{err}");
}

#[test]
fn reports_add_failure() {
    let fake = FakeSvn::named("addfail");
    fake.write("kept.txt", b"x");
    fake.write(".fakesvn-fail-add", b"");
    let err = add_to_commit_with(&fake.dir, &fake.svn, &["kept.txt"]).unwrap_err();
    assert!(err.contains("failed"), "{err}");
}

#[test]
fn scratch_migration_reports_filesystem_failures() {
    let base =
        std::env::temp_dir().join(format!("arc-rs-svn-t1170-scratch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    // The new location cannot be created under a plain file.
    let work = base.join("work");
    std::fs::create_dir_all(work.join(".arc")).unwrap();
    std::fs::write(work.join(".arc").join("x"), "data").unwrap();
    let blocker = base.join("blocker");
    std::fs::write(&blocker, "not a dir").unwrap();
    assert!(!migrate_scratch_dir(&work, &blocker));
    assert!(work.join(".arc").join("x").exists());
    assert!(!write_scratch_file(&blocker, "x", "data"));

    // An unreadable legacy file fails the copy; nothing is deleted.
    #[cfg(unix)]
    {
        let locked = base.join("locked");
        std::fs::create_dir_all(locked.join(".arc")).unwrap();
        let secret = locked.join(".arc").join("secret");
        std::fs::write(&secret, "data").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&secret).unwrap().permissions();
            perms.set_mode(0o000);
            std::fs::set_permissions(&secret, perms).unwrap();
        }
        // Root reads through permission bits; only assert where the
        // file is actually unreadable (same guard as the hg twin).
        if std::fs::read(&secret).is_err() {
            assert!(!migrate_scratch_dir(&locked, &base.join("fresh-meta"))); // LCOV_EXCL_LINE // Reason: root reads through chmod 000, so the guard never opens where gates measure; covered on macOS runs
            assert!(secret.exists()); // LCOV_EXCL_LINE // Reason: root reads through chmod 000, so the guard never opens where gates measure; covered on macOS runs
        } // LCOV_EXCL_LINE // Reason: brace carries no llvm count though the block executes
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&secret).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&secret, perms).unwrap();
        }
    }
    std::fs::remove_dir_all(&base).ok();
}
