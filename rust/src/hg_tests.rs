use super::*;
use std::path::PathBuf;

const NODE_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NODE_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FAKE_NODE: &str = "0123456789abcdef0123456789abcdef01234567";

/// Controlled dir plus a fake `hg` backend; removed on drop.
struct FakeHg {
    dir: PathBuf,
    hg: HgCommand,
}

impl FakeHg {
    /// Backend script shared by every test in the process: written once
    /// under a lock, then executed read-only with per-test CWDs (canned
    /// files and logs stay per-test, so isolation is unchanged). Sharing
    /// removes the per-test write-then-exec window that flakes on Linux
    /// with ETXTBSY under parallel load.
    fn shared_script() -> PathBuf {
        static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("arc-rs-hg-shared-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            #[cfg(windows)]
            {
                const FAKE: &str = include_str!("../tests/fixtures/vcs/fakehg.bat");
                let bat = dir.join("fakehg.bat");
                std::fs::write(&bat, FAKE).unwrap();
                bat
            }
            #[cfg(not(windows))]
            {
                const FAKE: &str = include_str!("../tests/fixtures/vcs/fakehg.sh");
                let sh = dir.join("fakehg.sh");
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

    /// Shared PowerShell backend (Windows only).
    #[cfg(windows)]
    fn shared_ps1() -> PathBuf {
        static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            const FAKE: &str = include_str!("../tests/fixtures/vcs/fakehg.ps1");
            let dir = std::env::temp_dir().join(format!("arc-rs-hg-shared-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let ps1 = dir.join("fakehg.ps1");
            std::fs::write(&ps1, FAKE).unwrap();
            ps1
        })
        .clone()
    }

    fn named(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("arc-rs-hg-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = Self::shared_script();
        #[cfg(windows)]
        let hg = HgCommand::custom("cmd", &["/C", &sh.to_string_lossy()]);
        #[cfg(not(windows))]
        let hg = HgCommand::custom(&sh.to_string_lossy(), &[]);
        Self { dir, hg }
    }

    /// PowerShell backend for template argv containing `|` (which cmd.exe
    /// parses as a pipe). Unix shells pass pipes through, so this aliases
    /// the shell fake there.
    #[cfg(windows)]
    fn named_ps1(tag: &str) -> Self {
        let mut fake = Self::named(tag);
        let ps1 = Self::shared_ps1();
        fake.hg = HgCommand::custom(
            "powershell",
            &[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &ps1.to_string_lossy(),
            ],
        );
        fake
    }

    #[cfg(not(windows))]
    fn named_ps1(tag: &str) -> Self {
        Self::named(tag)
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    /// Logged argv with quote chars stripped (Windows quotes spaced args).
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join(".fakehg.log"))
            .unwrap_or_default()
            .replace('"', "")
    }
}

impl Drop for FakeHg {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn flags_match_php_values() {
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
}

#[test]
fn trivial_getters_match_php() {
    assert_eq!(get_source_control_system_name(), "hg");
    assert_eq!(get_source_control_path(), "/");
    assert_eq!(get_working_copy_revision(), ".");
    assert_eq!(new_current_commit_symbol(), ".");
    assert!(is_history_default_immutable());
    assert!(supports_commit_ranges());
    assert!(supports_local_commits());
    assert!(!can_stash_changes());
    assert!(stash_changes().is_err());
    assert!(unstash_changes().is_err());
    assert_eq!(get_repository_uuid(), None);
    assert!(get_finalized_revision_message().contains("'hg push'"));
    assert_eq!(SUPPORTED_MARKER_TYPES, &["branch", "bookmark"]);
    let root = std::path::Path::new("/repo");
    assert_eq!(get_metadata_path(root), root.join(".hg"));
}

#[test]
fn parses_simple_status() {
    let status = parse_mercurial_status("M a.txt\nA b.txt\nR c.txt\n! d.txt\n? e.txt\n").unwrap();
    assert_eq!(status["a.txt"], FLAG_MODIFIED);
    assert_eq!(status["b.txt"], FLAG_ADDED);
    assert_eq!(status["c.txt"], FLAG_DELETED);
    assert_eq!(status["d.txt"], FLAG_MISSING);
    assert_eq!(status["e.txt"], FLAG_UNTRACKED);
    assert!(parse_mercurial_status("").unwrap().is_empty());
    assert!(parse_mercurial_status("  \n").unwrap().is_empty());
}

#[test]
fn parses_status_details_with_copies() {
    let details =
        parse_mercurial_status_details("A new.txt\n  old.txt\nC k.txt\nI i.txt\n").unwrap();
    assert_eq!(details["new.txt"].flags, FLAG_ADDED);
    assert_eq!(details["new.txt"].from.as_deref(), Some("old.txt"));
    assert_eq!(details["k.txt"].flags, 0);
    assert_eq!(details["i.txt"].flags, 0);
}

#[test]
fn rejects_bad_status_lines() {
    assert!(
        parse_mercurial_status_details("X a.txt\n")
            .unwrap_err()
            .contains("Unknown Mercurial status 'X'")
    );
    assert!(
        parse_mercurial_status_details("Ma.txt\n")
            .unwrap_err()
            .contains("Unparsable Mercurial status line")
    );
    assert!(
        parse_mercurial_status_details("M\n")
            .unwrap_err()
            .contains("Unparsable Mercurial status line")
    );
    // NOTE: the "Unexpected copy source" branch is unreachable through the
    // public parser: leading `trim` strips a first-line continuation, and
    // every other line sets the predecessor first. The branch stays as a
    // verbatim port of `ArcanistMercurialParser.php:70-78`.
}

#[test]
fn parses_default_log() {
    let stdout = "changeset:   2:bbbb\nparent:      1:aaaa\nuser:        Bob\nsummary:     two\n\nchangeset:   1:aaaa\nuser:        Bob\ndate:        Thu Oct 14 12:34:56 2026 +0000\nbranch:      stable\ntag:         v1\nbookmark:    mark\nobsolete:    x\nsummary:     one\n";
    let logs = parse_mercurial_log(stdout).unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].local.as_deref(), Some("2"));
    assert_eq!(logs[0].rev.as_deref(), Some("bbbb"));
    assert_eq!(logs[0].parents.len(), 1);
    assert_eq!(logs[0].parents[0].local, "1");
    assert_eq!(logs[0].parents[0].rev.as_deref(), Some("aaaa"));
    assert_eq!(logs[1].branch.as_deref(), Some("stable"));
    assert_eq!(logs[1].tag.as_deref(), Some("v1"));
    assert_eq!(logs[1].bookmark.as_deref(), Some("mark"));
    assert_eq!(
        logs[1].date_unix,
        parse_hg_default_date("Thu Oct 14 12:34:56 2026 +0000")
    );
    assert!(parse_mercurial_log("").unwrap().is_empty());
}

#[test]
fn skips_outgoing_preamble_and_rejects_unknown_fields() {
    let logs = parse_mercurial_log("comparing with x\nsearching for changes\nremote: err\nchangeset:   1:aaaa\nsummary:     s\n").unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].rev.as_deref(), Some("aaaa"));
    let err = parse_mercurial_log("changeset:   1:aaaa\nfrobnicate: yes\n").unwrap_err();
    assert!(
        err.contains("Unknown Mercurial log field 'frobnicate'!"),
        "{err}"
    );
}

#[test]
fn parses_branches() {
    let branches = parse_mercurial_branches(
        "default  15101:a21ccf4412d5\nstable  15095:ec222a29bdf0 (inactive)\nmy feature  3:ab12\n",
    )
    .unwrap();
    assert_eq!(branches["default"].local, "15101");
    assert_eq!(branches["default"].rev, "a21ccf4412d5");
    assert_eq!(branches["stable"].local, "15095");
    assert_eq!(branches["my feature"].rev, "ab12");
    assert!(parse_mercurial_branches("").unwrap().is_empty());
    let err = parse_mercurial_branches("bogus line\n").unwrap_err();
    assert!(
        err.contains("Failed to parse 'hg branches' output"),
        "{err}"
    );
    assert!(parse_mercurial_branches("bad  1:XYZ\n").is_err());
}

#[test]
fn parses_dates() {
    // Independent oracle: 2026-10-14T10:34:56Z == 1791974096.
    assert_eq!(
        parse_rfc822_date("Thu, 14 Oct 2026 12:34:56 +0200"),
        Some(1791974096)
    );
    assert_eq!(
        parse_hg_default_date("Thu Oct 14 10:34:56 2026 +0000"),
        Some(1791974096)
    );
    assert_eq!(
        parse_rfc822_date("Thu, 14 Oct 2026 10:34:56 +0000"),
        Some(1791974096)
    );
    assert_eq!(
        parse_rfc822_date("Thu, 14 Oct 2026 05:34:56 -0500"),
        Some(1791974096)
    );
    assert_eq!(parse_rfc822_date("not a date"), None);
    assert_eq!(parse_hg_default_date("Thu Oct 14 10:34:56 2026"), None);
    assert_eq!(
        parse_hg_default_date("Thu Foo 14 10:34:56 2026 +0000"),
        None
    );
}

#[test]
fn parses_blame() {
    let blame = parse_blame("Test User 0123456789ab: one\nalice abcdef012345: two\n").unwrap();
    assert_eq!(
        blame,
        vec![
            ("Test User".to_string(), "0123456789ab".to_string()),
            ("alice".to_string(), "abcdef012345".to_string()),
        ]
    );
    let err = parse_blame("garbage line\n").unwrap_err();
    assert!(
        err.contains("Unable to parse Mercurial blame line"),
        "{err}"
    );
    let err = parse_blame("bob 123: short\n").unwrap_err();
    assert!(
        err.contains("Unable to parse Mercurial blame line"),
        "{err}"
    );
}

#[test]
fn parses_full_authors() {
    assert_eq!(
        parse_full_author("plain name"),
        (Some("plain name".to_string()), None)
    );
    assert_eq!(
        parse_full_author("Bob <b@example.test>"),
        (Some("Bob".to_string()), Some("b@example.test".to_string()))
    );
    assert_eq!(
        parse_full_author("b@example.test"),
        (None, Some("b@example.test".to_string()))
    );
    assert_eq!(
        parse_full_author("\"Bob B\" <b@example.test>"),
        (
            Some("Bob B".to_string()),
            Some("b@example.test".to_string())
        )
    );
    assert_eq!(
        parse_full_author("a<b@c> d<e@f>"),
        (Some("a<b@c> d".to_string()), Some("e@f".to_string()))
    );
}

#[test]
fn parses_small_outputs() {
    assert_eq!(
        parse_manifest_lines("a\nb\n"),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(parse_config_value("", "d"), "d");
    assert_eq!(parse_config_value("v\r\n", "d"), "v");
    assert_eq!(
        parse_children_list("aa bb "),
        vec!["aa".to_string(), "bb".to_string()]
    );
    assert!(parse_children_list("").is_empty());
    assert_eq!(parse_commit_summary("null", "whatever"), "(The Empty Void)");
    assert_eq!(parse_commit_summary("abc", "Sub\nbody\n"), "Sub");
    assert_eq!(parse_branch_name("default\r\n"), "default");
}

#[test]
fn parses_message_log_newest_first() {
    let stdout = "aaa\x01first\x02bbb\x01second\x02".to_string();
    assert_eq!(
        parse_commit_message_log(&stdout),
        vec![
            ("bbb".to_string(), "second".to_string()),
            ("aaa".to_string(), "first".to_string()),
        ]
    );
    assert!(parse_commit_message_log("").is_empty());
    let stdout = "aaa\x01with\x01extra\x02".to_string();
    assert_eq!(
        parse_commit_message_log(&stdout),
        vec![("aaa".to_string(), "with".to_string())]
    );
    let stdout = "bbb\x01keep\x01this\x02".to_string();
    assert_eq!(
        parse_nodiff_records(&stdout),
        vec![("bbb".to_string(), "keep\x01this".to_string())]
    );
}

#[test]
fn parses_subversion_info() {
    let info = parse_subversion_info(
        "URL: https://x/repo\nRepository UUID: u1\nRevision: 7\nOther: z\nno-colon\n",
    );
    assert_eq!(info.base_path.as_deref(), Some("https://x/repo"));
    assert_eq!(info.uuid.as_deref(), Some("u1"));
    assert_eq!(info.base_revision.as_deref(), Some("https://x/repo@7"));
    let info = parse_subversion_info("URL: https://x/repo\n");
    assert_eq!(info.base_revision, None);
    assert_eq!(parse_subversion_info(""), HgSubversionInfo::default());
}

#[test]
fn parses_published_hashes_and_diff_options() {
    assert_eq!(
        parse_published_hashes("aa\n\n  \nbb\n"),
        vec!["aa".to_string(), "bb".to_string()]
    );
    assert_eq!(DEFAULT_DIFF_CONTEXT, 0x7FFF);
    assert_eq!(
        diff_options(3),
        vec!["--git".to_string(), "-U3".to_string()]
    );
    assert_eq!(diff_options_text(3), "--git -U3");
    assert_eq!(
        raw_diff_cache_key("--git -U3", "abc", "f"),
        "--git -U3 abc f"
    );
}

#[test]
fn walks_base_parents() {
    let entry = |rev: Option<&str>| HgLogEntry {
        rev: rev.map(str::to_string),
        ..HgLogEntry::default()
    };
    let outgoing = vec!["o1".to_string()];
    assert_eq!(
        base_walk_step(&outgoing, &[entry(Some("n1")), entry(Some("o1"))]),
        BaseWalkStep::Found("n1".to_string())
    );
    assert_eq!(
        base_walk_step(&outgoing, &[entry(Some("o1")), entry(Some("n2"))]),
        BaseWalkStep::Found("n2".to_string())
    );
    assert_eq!(
        base_walk_step(&outgoing, &[entry(Some("o1"))]),
        BaseWalkStep::Descend("o1".to_string())
    );
    assert_eq!(base_walk_step(&outgoing, &[]), BaseWalkStep::Null);
    assert_eq!(
        base_walk_step(&outgoing, &[entry(None)]),
        BaseWalkStep::Null
    );
}

#[test]
fn builds_revsets_and_rule_targets() {
    assert!(LOCAL_COMMIT_TEMPLATE.contains("{date|rfc822date}"));
    assert!(LOCAL_COMMIT_TEMPLATE.contains('\x01'));
    assert!(LOCAL_COMMIT_TEMPLATE.ends_with('\x02'));
    assert_eq!(COMMIT_MESSAGE_TEMPLATE, "{node}\x01{desc}\x02");
    assert_eq!(local_commit_revset("abc"), "(abc::. - abc)");
    assert_eq!(gca_revset("stable"), "ancestor(., stable)");
    assert_eq!(nodiff_revset("stable"), "ancestor(.,stable)::.^");
    assert_eq!(gca_rule_target("gca(x)"), Some("x".to_string()));
    assert_eq!(gca_rule_target("gca()"), None);
    assert_eq!(gca_rule_target("plain"), None);
    assert_eq!(nodiff_rule_target("nodiff(x)"), Some("x".to_string()));
    assert_eq!(nodiff_rule_target("nodiff()"), None);
    assert_eq!(
        split_base_rule("hg:stable"),
        Some(("hg".to_string(), "stable".to_string()))
    );
    assert_eq!(
        split_base_rule("a:b:c"),
        Some(("a".to_string(), "b:c".to_string()))
    );
    assert_eq!(split_base_rule("plain"), None);
    let records = vec![
        ("n1".to_string(), "no trailer".to_string()),
        ("n2".to_string(), "Differential Revision: D1".to_string()),
    ];
    assert_eq!(
        select_nodiff_base(&records, &|m| m.contains("Differential Revision:")),
        Some("n2".to_string())
    );
    assert_eq!(select_nodiff_base(&records, &|_| false), None);
}

#[test]
fn parses_local_commit_records() {
    let log = format!(
        "{NODE_A}\x0115\x01Bob <b@x>\x01Thu, 14 Oct 2026 10:34:56 +0000\x01default\x01\x01\x01sum line\nbody"
    );
    let commit = parse_local_commit_record(&log, None).unwrap();
    assert_eq!(commit.commit, NODE_A);
    assert_eq!(commit.rev, NODE_A);
    assert_eq!(commit.local, "15");
    assert_eq!(commit.author.as_deref(), Some("Bob"));
    assert_eq!(commit.author_email.as_deref(), Some("b@x"));
    assert_eq!(commit.time, Some(1791974096));
    assert_eq!(commit.summary, "sum line");
    assert_eq!(commit.parents, None);
    let commit = parse_local_commit_record(&log, Some("prev")).unwrap();
    assert_eq!(commit.parents, Some(vec!["prev".to_string()]));
    let with_parents = log.replace("\x01\x01sum", "\x0112:abc\x01sum");
    let commit = parse_local_commit_record(&with_parents, Some("prev")).unwrap();
    assert_eq!(commit.parents, None);
    assert!(parse_local_commit_record("short\x01record", None).is_err());
}

#[test]
fn builds_extension_commands() {
    let script = std::path::Path::new("/arc/support/hg/arc-hg.py");
    assert_eq!(
        extension_config_arg("arc-hg", Some(script)).unwrap(),
        (
            "--config".to_string(),
            "extensions.arc-hg=/arc/support/hg/arc-hg.py".to_string()
        )
    );
    assert_eq!(
        extension_config_arg("rebase", None).unwrap().1,
        "extensions.rebase="
    );
    assert!(extension_config_arg("arc-hg", None).is_err());
    let err = extension_config_arg("bogus", None).unwrap_err();
    assert!(
        err.contains("Unknown Mercurial Extension: \"bogus\"."),
        "{err}"
    );
    assert_eq!(
        extension_command("strip", &["strip", "--rev", "x", "--"], None).unwrap(),
        vec!["--config", "extensions.strip=", "strip", "--rev", "x", "--"]
    );
    assert_eq!(
        feature_probe_argv("shelve").unwrap(),
        vec!["--config", "extensions.shelve=", "shelve", "--help", "--"]
    );
    assert_eq!(
        feature_probe_argv("evolve").unwrap(),
        vec!["prune", "--help", "--"]
    );
    let err = feature_probe_argv("bogus").unwrap_err();
    assert!(
        err.contains("Unknown Mercurial feature \"bogus\"."),
        "{err}"
    );
}

#[test]
fn selects_status_paths_and_merges() {
    let mut status = StatusMap::new();
    status.insert("a".to_string(), FLAG_MODIFIED | FLAG_UNCOMMITTED);
    status.insert("b".to_string(), FLAG_UNTRACKED);
    status.insert(
        "c".to_string(),
        FLAG_CONFLICT | FLAG_MISSING | FLAG_INCOMPLETE | FLAG_EXTERNALS,
    );
    status.insert("d".to_string(), FLAG_UNSTAGED);
    assert_eq!(get_untracked_changes(&status), vec!["b".to_string()]);
    assert_eq!(get_unstaged_changes(&status), vec!["d".to_string()]);
    assert_eq!(get_uncommitted_changes(&status), vec!["a".to_string()]);
    assert_eq!(get_merge_conflicts(&status), vec!["c".to_string()]);
    assert_eq!(get_incomplete_changes(&status), vec!["c".to_string()]);
    assert_eq!(get_missing_changes(&status), vec!["c".to_string()]);
    assert_eq!(get_dirty_external_changes(&status), vec!["c".to_string()]);
    let mut range = StatusMap::new();
    range.insert("a".to_string(), FLAG_ADDED);
    range.insert("z".to_string(), FLAG_DELETED);
    let merged = merge_status(&range, &status);
    assert_eq!(merged["a"], FLAG_ADDED | FLAG_MODIFIED | FLAG_UNCOMMITTED);
    assert_eq!(merged["b"], FLAG_UNTRACKED);
    assert_eq!(merged["z"], FLAG_DELETED);
    assert_eq!(get_display_hash("0123456789abcdef"), "0123456789ab");
    assert_eq!(get_display_hash("short"), "short");
    let root = std::path::Path::new("/r");
    assert_eq!(join_path(root, None), root.to_path_buf());
    assert_eq!(join_path(root, Some("a/b")), root.join("a/b"));
    assert_eq!(join_path(root, Some("/a")), root.join("a"));
}

#[test]
fn roundtrips_scratch_files() {
    let dir = std::env::temp_dir().join(format!("arc-rs-hg-scratch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(read_scratch_file(&dir, "x"), None);
    assert!(write_scratch_file(&dir, "x", "data"));
    assert_eq!(read_scratch_file(&dir, "x").as_deref(), Some("data"));
    assert_eq!(scratch_file_path(&dir, "x"), dir.join("arc").join("x"));
    assert!(remove_scratch_file(&dir, "x"));
    assert_eq!(read_scratch_file(&dir, "x"), None);
    assert!(!remove_scratch_file(&dir, "x"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn runner_injects_encoding_and_env() {
    let fake = FakeHg::named("runner");
    let out = output_with(&fake.dir, &fake.hg, &["branch"]).unwrap();
    assert!(out.status.success());
    assert!(ok_with(&fake.dir, &fake.hg, &["branch"]));
    assert!(!ok_with(
        &fake.dir,
        &fake.hg,
        &["log", "-l", "1", "--template", "{node}", "-r", "nope", "--"]
    ));
    assert_eq!(
        stdout_text_with(&fake.dir, &fake.hg, &["envprobe"]).unwrap(),
        "HGPLAIN=1"
    );
    let manual = manual_with(&fake.dir, &fake.hg, &["branch"]).unwrap();
    assert_eq!(manual.code, 0);
    assert!(manual.stdout.contains("default"));
    let log = fake.log();
    assert!(log.contains("--encoding utf-8 branch"), "{log}");
    let err = stdout_text_with(
        &fake.dir,
        &fake.hg,
        &["log", "--template={desc}", "--rev", "nope"],
    )
    .unwrap_err();
    assert!(!err.is_empty(), "failing hg surfaces output");
}

#[test]
fn reads_branch_and_canonical_revision() {
    let fake = FakeHg::named("branch");
    assert_eq!(
        get_branch_name_with(&fake.dir, &fake.hg).unwrap(),
        "default"
    );
    assert_eq!(
        get_canonical_revision_name_with(&fake.dir, &fake.hg, "tip").unwrap(),
        FAKE_NODE
    );
    assert_eq!(
        get_source_control_base_revision_with(&fake.dir, &fake.hg, "tip").unwrap(),
        FAKE_NODE
    );
    assert!(has_local_commit_with(&fake.dir, &fake.hg, "tip").unwrap());
    assert!(!has_local_commit_with(&fake.dir, &fake.hg, "nope").unwrap());
}

#[test]
fn resolves_explicit_base_or_usage_error() {
    let fake = FakeHg::named("explicit");
    let (commit, why) = resolve_explicit_base_with(&fake.dir, &fake.hg, "stable").unwrap();
    assert_eq!(commit, FAKE_NODE);
    assert!(
        why.contains("greatest common ancestor of the working directory"),
        "{why}"
    );
    assert!(fake.log().contains("ancestor(stable,.)"));
    let err = resolve_explicit_base_with(&fake.dir, &fake.hg, "nope").unwrap_err();
    assert!(
        err.contains("Commit 'nope' is not a valid Mercurial commit identifier."),
        "{err}"
    );
}

#[test]
fn reads_status_variants() {
    let fake = FakeHg::named("status");
    fake.write(".fakehg-status", b"M a.txt\n? b.txt\n");
    let status = get_uncommitted_status_with(&fake.dir, &fake.hg).unwrap();
    assert_eq!(status["a.txt"], FLAG_MODIFIED | FLAG_UNCOMMITTED);
    assert_eq!(status["b.txt"], FLAG_UNTRACKED);
    let range = get_commit_range_status_with(&fake.dir, &fake.hg, "base").unwrap();
    assert_eq!(range["a.txt"], FLAG_MODIFIED);
    assert!(!range.contains_key("unrelated"));
    let changed = get_changed_files_with(&fake.dir, &fake.hg, "base").unwrap();
    assert_eq!(changed["a.txt"], FLAG_MODIFIED);
    let log = fake.log();
    assert!(log.contains("status --rev base --rev tip"), "{log}");
    assert!(uncommitted_status_from_output("").unwrap().is_empty());
}

#[test]
fn reads_blame_message_summary_remote() {
    let fake = FakeHg::named("read");
    fake.write(".fakehg-annotate", b"alice 0123456789ab: line\n");
    fake.write(".fakehg-msg", b"Subject here\nbody\n");
    let blame = get_blame_with(&fake.dir, &fake.hg, "base", "a.txt").unwrap();
    assert_eq!(
        blame,
        vec![("alice".to_string(), "0123456789ab".to_string())]
    );
    assert!(
        get_commit_message_with(&fake.dir, &fake.hg, ".")
            .unwrap()
            .contains("Subject here")
    );
    assert_eq!(
        get_commit_summary_with(&fake.dir, &fake.hg, "abc").unwrap(),
        "Subject here"
    );
    assert_eq!(
        get_commit_summary_with(&fake.dir, &fake.hg, "null").unwrap(),
        "(The Empty Void)"
    );
    assert_eq!(
        get_remote_uri_with(&fake.dir, &fake.hg).unwrap().as_deref(),
        Some("https://hg.example.test/repo")
    );
    let log = fake.log();
    assert!(
        log.contains("annotate -u -v -c --rev base -- a.txt"),
        "{log}"
    );
}

#[test]
fn remote_uri_empty_is_none() {
    let fake = FakeHg::named("nopaths");
    fake.write(".fakehg-nopaths", b"");
    assert_eq!(get_remote_uri_with(&fake.dir, &fake.hg).unwrap(), None);
}

#[test]
fn detects_amend_support() {
    let fake = FakeHg::named("amendhelp");
    assert!(supports_amend_with(&fake.dir, &fake.hg).unwrap());
    assert!(mercurial_feature_supported_with(&fake.dir, &fake.hg, "evolve").unwrap());
    fake.write(".fakehg-fail-help", b"");
    assert!(!supports_amend_with(&fake.dir, &fake.hg).unwrap());
    fake.write(".fakehg-fail-prune", b"");
    assert!(!mercurial_feature_supported_with(&fake.dir, &fake.hg, "evolve").unwrap());
    assert!(mercurial_feature_supported_with(&fake.dir, &fake.hg, "bogus").is_err());
    let plain = FakeHg::named("noamend");
    plain.write(".fakehg-noamend", b"");
    assert!(!supports_amend_with(&plain.dir, &plain.hg).unwrap());
}

#[test]
fn reads_author() {
    let fake = FakeHg::named("author");
    assert_eq!(
        get_author_with(&fake.dir, &fake.hg).unwrap().as_deref(),
        Some("Test User")
    );
    assert!(
        get_mercurial_config_with(&fake.dir, &fake.hg, "ui.username", "d")
            .unwrap()
            .contains("Test User")
    );
    fake.write(".fakehg-fail-showconfig", b"");
    assert_eq!(get_author_with(&fake.dir, &fake.hg).unwrap(), None);
    assert!(get_mercurial_config_with(&fake.dir, &fake.hg, "ui.username", "d").is_err());
}

#[test]
fn auto_base_covers_all_branches() {
    let empty = FakeHg::named("autoempty");
    let (base, why) = auto_base_commit_with(&empty.dir, &empty.hg, "default", ".").unwrap();
    assert_eq!(base, ".");
    assert!(why.contains("no outgoing commits"), "{why}");

    let failed = FakeHg::named("autofail");
    failed.write(".fakehg-fail-log", b"");
    let (base, _) = auto_base_commit_with(&failed.dir, &failed.hg, "default", ".").unwrap();
    assert_eq!(base, ".");

    let draft = format!("changeset:   3:{NODE_A}\nuser:        u\nsummary:     out\n");
    let found = FakeHg::named("autofound");
    found.write(".fakehg-log-draft", draft.as_bytes());
    found.write(
        ".fakehg-parents",
        format!("changeset:   2:{NODE_B}\nuser:        u\nsummary:     p\n").as_bytes(),
    );
    let (base, why) = auto_base_commit_with(&found.dir, &found.hg, "default", ".").unwrap();
    assert_eq!(base, NODE_B);
    assert!(why.contains("not outgoing"), "{why}");

    let null = FakeHg::named("autonull");
    null.write(".fakehg-log-draft", draft.as_bytes());
    let (base, why) = auto_base_commit_with(&null.dir, &null.hg, "default", ".").unwrap();
    assert_eq!(base, "null");
    assert!(why.contains("new repository"), "{why}");
}

#[test]
fn reads_local_commit_information() {
    let fake = FakeHg::named_ps1("localinfo");
    let older = format!(
        "{NODE_A}\x0110\x01alice\x01Thu, 14 Oct 2026 10:34:56 +0000\x01default\x01\x01\x01first\nbody"
    );
    let newer = format!(
        "{NODE_B}\x0111\x01Bob <b@x>\x01Thu, 14 Oct 2026 11:00:00 +0000\x01default\x01\x01\x01second"
    );
    fake.write(
        ".fakehg-log-info",
        format!("{older}\x02{newer}\x02").as_bytes(),
    );
    fake.write(".fakehg-parents-tpl", format!("{NODE_A}\n").as_bytes());
    let commits =
        get_local_commit_information_with(&fake.dir, &fake.hg, "base", "default").unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].commit, NODE_B);
    assert_eq!(commits[0].parents, Some(vec![NODE_A.to_string()]));
    assert_eq!(commits[0].author.as_deref(), Some("Bob"));
    assert_eq!(commits[1].commit, NODE_A);
    assert_eq!(commits[1].parents, Some(vec![NODE_A.to_string()]));
    assert_eq!(commits[1].author.as_deref(), Some("alice"));
    let log = fake.log();
    assert!(log.contains("--branch default --"), "{log}");
}

#[test]
fn reads_message_log_and_files() {
    let fake = FakeHg::named("msglog");
    fake.write(".fakehg-log-msg", "aaa\x01one\x02bbb\x01two\x02".as_bytes());
    let log = get_commit_message_log_with(&fake.dir, &fake.hg, "base", "default").unwrap();
    assert_eq!(log[0].0, "bbb");
    assert_eq!(log[1].0, "aaa");
    fake.write(".fakehg-manifest", b"a.txt\nb.txt\n");
    assert_eq!(
        get_all_files_with(&fake.dir, &fake.hg).unwrap(),
        vec!["a.txt".to_string(), "b.txt".to_string()]
    );
}

#[test]
fn reads_file_data() {
    let fake = FakeHg::named("filedata");
    fake.write(".fakehg-cat", b"bytes\xffhere");
    assert_eq!(
        get_original_file_data_with(&fake.dir, &fake.hg, "base", "a.txt").unwrap(),
        Some(b"bytes\xffhere".to_vec())
    );
    assert_eq!(
        get_current_file_data_with(&fake.dir, &fake.hg, ".", "a.txt").unwrap(),
        Some(b"bytes\xffhere".to_vec())
    );
    assert_eq!(
        get_original_file_data_with(&fake.dir, &fake.hg, "base", "absent.txt").unwrap(),
        None
    );
}

#[test]
fn bulks_file_data_into_tmp() {
    let fake = FakeHg::named("bulk");
    let tmp = fake.dir.join("tmp");
    std::fs::create_dir_all(tmp.join("sub")).unwrap();
    std::fs::write(tmp.join("a.txt"), b"aaa").unwrap();
    std::fs::write(tmp.join("sub").join("b.txt"), b"bbb").unwrap();
    let data = bulk_file_data_at_revision_in(
        &fake.dir,
        &fake.hg,
        &["a.txt", "sub/b.txt", "missing.txt"],
        "base",
        &tmp,
    )
    .unwrap();
    assert_eq!(data["a.txt"], b"aaa");
    assert_eq!(data["sub/b.txt"], b"bbb");
    assert!(!data.contains_key("missing.txt"));
    let log = fake.log();
    assert!(log.contains("--output"), "{log}");
    assert!(log.contains("%p"), "{log}");
    assert!(cat_output_arg(&tmp).ends_with("%p"));
    let data = bulk_file_data_at_revision_with(&fake.dir, &fake.hg, &["zzz.txt"], "base").unwrap();
    assert!(data.is_empty());
}

#[test]
fn reads_raw_diff() {
    let fake = FakeHg::named("diff");
    fake.write(".fakehg-diff", b"diff --git text\n");
    let diff = get_raw_diff_text_with(&fake.dir, &fake.hg, "base", "a.txt", 3).unwrap();
    assert!(diff.contains("diff --git text"), "{diff}");
    assert!(
        get_full_mercurial_diff_with(&fake.dir, &fake.hg, "base", 3)
            .unwrap()
            .contains("diff --git")
    );
    let log = fake.log();
    assert!(log.contains("diff --git -U3 --rev base -- a.txt"), "{log}");
}

#[test]
fn mutates_working_copy() {
    let fake = FakeHg::named("mutate");
    update_working_copy_with(&fake.dir, &fake.hg).unwrap();
    add_to_commit_with(&fake.dir, &fake.hg, &["a.txt", "b.txt"]).unwrap();
    do_commit_with(&fake.dir, &fake.hg, "msg").unwrap();
    let log = fake.log();
    assert!(log.contains("up"), "{log}");
    assert!(log.contains("addremove -- a.txt b.txt"), "{log}");
    assert!(log.contains("commit --logfile"), "{log}");
}

#[test]
fn amends_nothing_when_unchanged() {
    let fake = FakeHg::named("amendnoop");
    fake.write(".fakehg-msg", b"same message");
    amend_commit_with(&fake.dir, &fake.hg, None, None).unwrap();
    amend_commit_with(&fake.dir, &fake.hg, Some("same message"), None).unwrap();
    let log = fake.log();
    assert!(!log.contains("amend"), "{log}");
}

#[test]
fn amends_head_and_non_head() {
    let head = FakeHg::named("amendhead");
    head.write(".fakehg-status", b"M a.txt\n");
    head.write(".fakehg-msg", b"old message");
    head.write(".fakehg-fail-prune", b"");
    amend_commit_with(&head.dir, &head.hg, Some("new message"), None).unwrap();
    let log = head.log();
    assert!(log.contains("commit --amend --logfile"), "{log}");

    let non_head = FakeHg::named("amendnonhead");
    non_head.write(".fakehg-status", b"M a.txt\n");
    non_head.write(".fakehg-msg", b"old message");
    non_head.write(".fakehg-fail-prune", b"");
    non_head.write(".fakehg-children", format!("{NODE_A} {NODE_B} ").as_bytes());
    let script = non_head.dir.join("arc-hg.py");
    std::fs::write(&script, b"# fake").unwrap();
    amend_non_head_commit_with(
        &non_head.dir,
        &non_head.hg,
        &[NODE_A.to_string(), NODE_B.to_string()],
        "msg.tmp",
        Some(&script),
    )
    .unwrap();
    let log = non_head.log();
    assert!(log.contains("arc-amend --logfile msg.tmp"), "{log}");
    assert!(log.contains("--dest"), "{log}");
    assert!(log.contains("--source"), "{log}");
    assert!(log.contains("strip --rev"), "{log}");
    assert!(log.contains("extensions.arc-hg="), "{log}");
}

#[test]
fn amends_with_evolve() {
    let fake = FakeHg::named("amendevolve");
    fake.write(".fakehg-status", b"M a.txt\n");
    fake.write(".fakehg-msg", b"old message");
    amend_commit_with(&fake.dir, &fake.hg, Some("new message"), None).unwrap();
    let log = fake.log();
    assert!(log.contains("amend --logfile"), "{log}");
    assert!(log.contains("evolve --all --"), "{log}");
}

#[test]
fn resolves_base_rules() {
    let has_id = |message: &str| message.contains("Differential Revision:");
    let fake = FakeHg::named("rules");
    let (commit, why) =
        resolve_base_commit_rule_with(&fake.dir, &fake.hg, "hg:gca(stable)", "user", &has_id)
            .unwrap()
            .unwrap();
    assert_eq!(commit, FAKE_NODE);
    assert!(
        why.contains("greatest common ancestor of 'stable'"),
        "{why}"
    );
    let (commit, why) =
        resolve_base_commit_rule_with(&fake.dir, &fake.hg, "hg:stable", "user", &has_id)
            .unwrap()
            .unwrap();
    assert_eq!(commit, FAKE_NODE);
    assert!(why.contains("specified by 'hg:stable'"), "{why}");
    let found = resolve_base_commit_rule_with(&fake.dir, &fake.hg, "arc:empty", "user", &has_id)
        .unwrap()
        .unwrap();
    assert_eq!(found.0, "null");
    let found = resolve_base_commit_rule_with(&fake.dir, &fake.hg, "arc:this", "user", &has_id)
        .unwrap()
        .unwrap();
    assert_eq!(found.0, FAKE_NODE);
    let found = resolve_base_commit_rule_with(&fake.dir, &fake.hg, "arc:outgoing", "user", &has_id)
        .unwrap()
        .unwrap();
    assert_eq!(found.0, FAKE_NODE);
    assert!(
        found.1.contains("matched the rule arc:outgoing"),
        "{:?}",
        found.1
    );
    let found = resolve_base_commit_rule_with(&fake.dir, &fake.hg, "arc:bookmark", "user", &has_id)
        .unwrap()
        .unwrap();
    assert_eq!(found.0, FAKE_NODE);
    assert!(!found.1.ends_with('.'));
    assert_eq!(
        resolve_base_commit_rule_with(&fake.dir, &fake.hg, "git:stable", "user", &has_id).unwrap(),
        None
    );
    assert_eq!(
        resolve_base_commit_rule_with(&fake.dir, &fake.hg, "plain", "user", &has_id).unwrap(),
        None
    );
    assert_eq!(
        resolve_base_commit_rule_with(&fake.dir, &fake.hg, "arc:bogus", "user", &has_id).unwrap(),
        None
    );

    let amended = FakeHg::named("amended");
    amended.write(".fakehg-msg", b"work\n\nDifferential Revision: D123\n");
    let found =
        resolve_base_commit_rule_with(&amended.dir, &amended.hg, "arc:amended", "user", &has_id)
            .unwrap()
            .unwrap();
    assert_eq!(found.0, FAKE_NODE);
    assert!(found.1.contains("has been amended"), "{:?}", found.1);
    let plain = FakeHg::named("notamended");
    plain.write(".fakehg-msg", b"work without trailer\n");
    assert_eq!(
        resolve_base_commit_rule_with(&plain.dir, &plain.hg, "arc:amended", "user", &has_id)
            .unwrap(),
        None
    );

    let nodiff = FakeHg::named("nodiff");
    nodiff.write(
        ".fakehg-log-nodiff",
        "aaa\x01old work\x02bbb\x01has Differential Revision: D9\x02".as_bytes(),
    );
    let found = resolve_base_commit_rule_with(
        &nodiff.dir,
        &nodiff.hg,
        "arc:nodiff(stable)",
        "local",
        &has_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.0, "bbb");
    assert!(found.1.contains("gca with 'stable'"), "{:?}", found.1);

    let missing = FakeHg::named("nomatch");
    missing.write(".fakehg-fail-log", b"");
    assert_eq!(
        resolve_base_commit_rule_with(&missing.dir, &missing.hg, "hg:gca(stable)", "user", &has_id)
            .unwrap(),
        None
    );
    let err = first_matching_base_rule_with(
        &missing.dir,
        &missing.hg,
        &[("hg:gca(stable)", "user")],
        &has_id,
    )
    .unwrap_err();
    assert!(
        err.contains("None of the rules in your 'base' configuration"),
        "{err}"
    );
    let (commit, _) = first_matching_base_rule_with(
        &fake.dir,
        &fake.hg,
        &[("bogus", "user"), ("arc:empty", "user")],
        &has_id,
    )
    .unwrap();
    assert_eq!(commit, "null");
}

#[test]
fn reads_subversion_info_and_hashes() {
    let fake = FakeHg::named("svninfo");
    fake.write(
        ".fakehg-svninfo",
        b"URL: https://x/repo\nRepository UUID: u1\nRevision: 7\n",
    );
    let info = get_subversion_info_with(&fake.dir, &fake.hg).unwrap();
    assert_eq!(info.base_revision.as_deref(), Some("https://x/repo@7"));
    fake.write(".fakehg-log-pub", b"aa\nbb\n");
    assert_eq!(
        get_published_commit_hashes_with(&fake.dir, &fake.hg).unwrap(),
        vec!["aa".to_string(), "bb".to_string()]
    );
    let failing = FakeHg::named("svnfail");
    failing.write(".fakehg-fail-svn", b"");
    assert_eq!(
        get_subversion_info_with(&failing.dir, &failing.hg).unwrap(),
        HgSubversionInfo::default()
    );
}

#[test]
fn executes_with_extensions() {
    let fake = FakeHg::named("ext");
    exec_with_extension_with(
        &fake.dir,
        &fake.hg,
        "shelve",
        &["shelve", "--help", "--"],
        None,
    )
    .unwrap();
    exec_manual_with_extension_with(
        &fake.dir,
        &fake.hg,
        "strip",
        &["strip", "--rev", "x", "--"],
        None,
    )
    .unwrap();
    assert!(exec_with_extension_with(&fake.dir, &fake.hg, "bogus", &["x"], None).is_err());
    let log = fake.log();
    assert!(log.contains("extensions.shelve="), "{log}");
    assert!(log.contains("extensions.strip="), "{log}");
}

#[test]
fn bulks_through_public_wrappers() {
    let fake = FakeHg::named("bulkwrap");
    assert!(
        get_bulk_original_file_data_with(&fake.dir, &fake.hg, &["zzz.txt"], "base")
            .unwrap()
            .is_empty()
    );
    assert!(
        get_bulk_current_file_data_with(&fake.dir, &fake.hg, &["zzz.txt"], ".")
            .unwrap()
            .is_empty()
    );
    assert!(fake.log().contains("cat --rev base --output"));
}

#[test]
fn passes_stdio_through() {
    let fake = FakeHg::named("passthru");
    // Failing invocations stay silent (stdio is inherited, not captured).
    fake.write(".fakehg-fail-log", b"");
    assert_eq!(
        exec_passthru_with(
            &fake.dir,
            &fake.hg,
            &["log", "--template={desc}", "--rev", "x"]
        )
        .unwrap(),
        1
    );
}

/// ETXTBSY retry: hold a script open for writing, release it mid-retry,
/// and the spawn must succeed. Unix-only (Windows has no ETXTBSY).
#[cfg(unix)]
#[test]
fn output_retries_while_script_is_busy() {
    let fake = FakeHg::named("busy");
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
    let hg = HgCommand::custom(&sh.to_string_lossy(), &[]);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(held);
    });
    let out = output_with(&fake.dir, &hg, &[]).unwrap();
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
    let dir = std::env::temp_dir().join(format!("arc-rs-txtbsy-probe-hg-{}", std::process::id()));
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

/// A permanently busy script still fails (after the bound) with the
/// original error shape.
#[cfg(unix)]
#[test]
fn output_gives_up_on_permanently_busy_script() {
    if !platform_reports_txtbsy() {
        return;
    }
    let fake = FakeHg::named("stuck");
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
    let hg = HgCommand::custom(&sh.to_string_lossy(), &[]);
    let err = output_with(&fake.dir, &hg, &[]).unwrap_err();
    assert!(err.contains("failed to run"), "{err}");
}
#[test]
fn commit_range_status_parses_like_plain_status() {
    let sample = "M a.txt\nA b.txt\nR c.txt\n! d.txt\n? e.txt\n";
    assert_eq!(
        commit_range_status_from_output(sample).unwrap(),
        parse_mercurial_status(sample).unwrap()
    );
    assert!(commit_range_status_from_output("").unwrap().is_empty());
}

#[test]
fn paths_with_mask_selects_flagged_paths_in_order() {
    let mut status = StatusMap::new();
    status.insert("b".to_string(), FLAG_UNTRACKED);
    status.insert("a".to_string(), FLAG_MODIFIED | FLAG_UNTRACKED);
    status.insert("c".to_string(), FLAG_MODIFIED);
    assert_eq!(
        paths_with_mask(&status, FLAG_UNTRACKED),
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(paths_with_mask(&status, FLAG_CONFLICT).is_empty());
}
