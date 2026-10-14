//! Harness for the hg + svn API bundle through the wired library.
//!
//! The system-binary wrappers are production API that cannot run hermetically,
//! so they are unexercised by this target; everything else is covered.

use std::path::{Path, PathBuf};

use arc_rs::{hg, svn};

const HG_STATUS_SIMPLE: &str = include_str!("fixtures/vcs/hg_status_simple.txt");
const HG_STATUS_COPIES: &str = include_str!("fixtures/vcs/hg_status_copies.txt");
const HG_LOG_DEFAULT: &str = include_str!("fixtures/vcs/hg_log_default.txt");
const HG_LOG_OUTGOING: &str = include_str!("fixtures/vcs/hg_log_outgoing.txt");
const HG_BRANCHES: &str = include_str!("fixtures/vcs/hg_branches.txt");
const HG_BLAME: &str = include_str!("fixtures/vcs/hg_blame.txt");
const HG_MANIFEST: &str = include_str!("fixtures/vcs/hg_manifest.txt");
const HG_PARENTS_ONE: &str = include_str!("fixtures/vcs/hg_parents_one.txt");
const HG_HELP_COMMIT: &str = include_str!("fixtures/vcs/hg_help_commit.txt");
const HG_SVN_INFO: &str = include_str!("fixtures/vcs/hg_svn_info.txt");
const SVN_STATUS_SIMPLE: &str = include_str!("fixtures/vcs/svn_status_simple.xml");
const SVN_STATUS_EXTERNALS: &str = include_str!("fixtures/vcs/svn_status_externals.xml");
const SVN_INFO: &str = include_str!("fixtures/vcs/svn_info.txt");
const SVN_INFO_COPIED: &str = include_str!("fixtures/vcs/svn_info_copied.txt");
const SVN_BLAME: &str = include_str!("fixtures/vcs/svn_blame.txt");
const SVN_LIST: &str = include_str!("fixtures/vcs/svn_list.txt");
const SVN_DIFF: &str = include_str!("fixtures/vcs/svn_diff.txt");
const SVN_DIFF_SUMMARIZE: &str = include_str!("fixtures/vcs/svn_diff_summarize.xml");

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-vcs-hgsvn-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.path.join(name), body).unwrap();
    }

    fn install_script(&self, name: &str) -> PathBuf {
        #[cfg(windows)]
        let (file_name, content) = match name {
            "fakehg" => ("fakehg.bat", include_str!("fixtures/vcs/fakehg.bat")),
            "fakesvn" => ("fakesvn.bat", include_str!("fixtures/vcs/fakesvn.bat")),
            "fakediff" => ("fakediff.bat", include_str!("fixtures/vcs/fakediff.bat")),
            _ => unreachable!("unknown fake script {name}"),
        };
        #[cfg(not(windows))]
        let (file_name, content) = match name {
            "fakehg" => ("fakehg.sh", include_str!("fixtures/vcs/fakehg.sh")),
            "fakesvn" => ("fakesvn.sh", include_str!("fixtures/vcs/fakesvn.sh")),
            "fakediff" => ("fakediff.sh", include_str!("fixtures/vcs/fakediff.sh")),
            _ => unreachable!("unknown fake script {name}"),
        };
        let script = self.path.join(file_name);
        std::fs::write(&script, content).unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        script
    }

    fn hg_backend(&self) -> hg::HgCommand {
        let script = self.install_script("fakehg");
        #[cfg(windows)]
        {
            hg::HgCommand::custom("cmd", &["/C", &script.to_string_lossy()])
        }
        #[cfg(not(windows))]
        {
            hg::HgCommand::custom(&script.to_string_lossy(), &[])
        }
    }

    /// PowerShell backend for template argv containing `|` (which cmd.exe
    /// parses as a pipe). Unix shells pass pipes through, so this aliases
    /// the shell fake there.
    #[cfg(windows)]
    fn hg_ps1_backend(&self) -> hg::HgCommand {
        const FAKE: &str = include_str!("fixtures/vcs/fakehg.ps1");
        let script = self.path.join("fakehg.ps1");
        std::fs::write(&script, FAKE).unwrap();
        hg::HgCommand::custom(
            "powershell",
            &[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &script.to_string_lossy(),
            ],
        )
    }

    #[cfg(not(windows))]
    fn hg_ps1_backend(&self) -> hg::HgCommand {
        self.hg_backend()
    }

    fn svn_backend(&self) -> svn::SvnCommand {
        let script = self.install_script("fakesvn");
        #[cfg(windows)]
        {
            svn::SvnCommand::custom("cmd", &["/C", &script.to_string_lossy()])
        }
        #[cfg(not(windows))]
        {
            svn::SvnCommand::custom(&script.to_string_lossy(), &[])
        }
    }

    fn diff_program(&self) -> (String, Vec<String>) {
        let script = self.install_script("fakediff");
        #[cfg(windows)]
        {
            (
                "cmd".to_string(),
                vec!["/C".to_string(), script.to_string_lossy().to_string()],
            )
        }
        #[cfg(not(windows))]
        {
            (script.to_string_lossy().to_string(), Vec::new())
        }
    }

    fn read_log(&self, name: &str) -> String {
        std::fs::read_to_string(self.path.join(name))
            .unwrap_or_default()
            .replace('"', "")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn hg_fixture_status() {
    let status = hg::parse_mercurial_status(HG_STATUS_SIMPLE).unwrap();
    assert_eq!(status["modified.txt"], hg::FLAG_MODIFIED);
    assert_eq!(status["added.txt"], hg::FLAG_ADDED);
    assert_eq!(status["removed.txt"], hg::FLAG_DELETED);
    assert_eq!(status["missing.txt"], hg::FLAG_MISSING);
    assert_eq!(status["untracked.txt"], hg::FLAG_UNTRACKED);

    let details = hg::parse_mercurial_status_details(HG_STATUS_COPIES).unwrap();
    assert_eq!(details["copied.txt"].flags, hg::FLAG_ADDED);
    assert_eq!(details["copied.txt"].from.as_deref(), Some("original.txt"));
    assert_eq!(details["renamed.txt"].flags, hg::FLAG_MODIFIED);
    assert_eq!(details["renamed.txt"].from.as_deref(), Some("oldname.txt"));
    assert_eq!(details["clean.txt"].flags, 0);
    assert_eq!(details["ignored.txt"].flags, 0);
}

#[test]
fn hg_fixture_log_and_branches() {
    let logs = hg::parse_mercurial_log(HG_LOG_DEFAULT).unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].local.as_deref(), Some("15101"));
    assert_eq!(
        logs[0].rev.as_deref(),
        Some("a21ccf4412d5223a9f4d5e6b7c8d9e0f1a2b3c4")
    );
    assert_eq!(logs[0].parents.len(), 1);
    assert_eq!(logs[1].branch.as_deref(), Some("stable"));
    assert_eq!(logs[1].tag.as_deref(), Some("v1"));
    assert_eq!(
        logs[1].date_unix,
        hg::parse_hg_default_date("Wed Oct 13 09:00:00 2026 +0200")
    );
    assert_eq!(
        logs[0].date_unix,
        hg::parse_rfc822_date("Thu, 14 Oct 2026 12:34:56 +0200")
    );

    let outgoing = hg::parse_mercurial_log(HG_LOG_OUTGOING).unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].summary.as_deref(), Some("outgoing work"));

    let branches = hg::parse_mercurial_branches(HG_BRANCHES).unwrap();
    assert_eq!(branches["default"].local, "15101");
    assert_eq!(branches["default"].rev, "a21ccf4412d5");
    assert_eq!(branches["stable"].rev, "ec222a29bdf0");
    assert_eq!(branches["my feature"].local, "15090");
}

#[test]
fn hg_fixture_blame_and_manifest() {
    let blame = hg::parse_blame(HG_BLAME).unwrap();
    assert_eq!(
        blame,
        vec![
            ("Test User".to_string(), "0123456789ab".to_string()),
            ("alice".to_string(), "abcdef012345".to_string()),
        ]
    );
    assert_eq!(
        hg::parse_manifest_lines(HG_MANIFEST),
        vec![
            "file.txt".to_string(),
            "src/main.rs".to_string(),
            "docs/readme.md".to_string()
        ]
    );
}

#[test]
fn hg_pipeline_against_fake() {
    let tmp = TempDir::new("pipeline");
    let hg = tmp.hg_backend();
    tmp.write(".fakehg-status", HG_STATUS_SIMPLE.as_bytes());
    tmp.write(".fakehg-annotate", HG_BLAME.as_bytes());
    tmp.write(".fakehg-msg", b"Pipe subject\nbody\n");
    tmp.write(".fakehg-manifest", HG_MANIFEST.as_bytes());
    tmp.write(".fakehg-help", HG_HELP_COMMIT.as_bytes());
    tmp.write(".fakehg-svninfo", HG_SVN_INFO.as_bytes());
    tmp.write(".fakehg-log-pub", b"aa\nbb\n");
    tmp.write(".fakehg-cat", b"file\n");

    assert_eq!(
        hg::get_branch_name_with(tmp.path(), &hg).unwrap(),
        "default"
    );
    assert_eq!(
        hg::get_canonical_revision_name_with(tmp.path(), &hg, "tip")
            .unwrap()
            .len(),
        40
    );
    let status = hg::get_uncommitted_status_with(tmp.path(), &hg).unwrap();
    assert_eq!(
        status["modified.txt"],
        hg::FLAG_MODIFIED | hg::FLAG_UNCOMMITTED
    );
    assert_eq!(status["untracked.txt"], hg::FLAG_UNTRACKED);
    assert_eq!(
        hg::get_untracked_changes(&status),
        vec!["untracked.txt".to_string()]
    );
    let range = hg::get_commit_range_status_with(tmp.path(), &hg, "base").unwrap();
    assert_eq!(range["modified.txt"], hg::FLAG_MODIFIED);
    let blame = hg::get_blame_with(tmp.path(), &hg, "base", "file.txt").unwrap();
    assert_eq!(blame.len(), 2);
    assert!(
        hg::get_commit_message_with(tmp.path(), &hg, ".")
            .unwrap()
            .contains("Pipe subject")
    );
    assert_eq!(
        hg::get_commit_summary_with(tmp.path(), &hg, "tip").unwrap(),
        "Pipe subject"
    );
    assert_eq!(
        hg::get_remote_uri_with(tmp.path(), &hg).unwrap().as_deref(),
        Some("https://hg.example.test/repo")
    );
    assert_eq!(
        hg::get_author_with(tmp.path(), &hg).unwrap().as_deref(),
        Some("Test User")
    );
    assert!(hg::supports_amend_with(tmp.path(), &hg).unwrap());
    assert!(hg::mercurial_feature_supported_with(tmp.path(), &hg, "shelve").unwrap());
    let info = hg::get_subversion_info_with(tmp.path(), &hg).unwrap();
    assert_eq!(
        info.base_revision.as_deref(),
        Some("https://svn.example.test/repo/trunk@42")
    );
    assert_eq!(
        hg::get_published_commit_hashes_with(tmp.path(), &hg)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(hg::get_all_files_with(tmp.path(), &hg).unwrap().len(), 3);
    assert_eq!(
        hg::get_original_file_data_with(tmp.path(), &hg, "base", "file.txt").unwrap(),
        Some(b"file\n".to_vec())
    );

    let log = tmp.read_log(".fakehg.log");
    assert!(log.contains("--encoding utf-8 branch"), "{log}");
    assert!(log.contains("status --rev"), "{log}");
}

#[test]
fn hg_auto_base_finds_parent_from_fixtures() {
    let tmp = TempDir::new("autobase");
    let hg = tmp.hg_backend();
    tmp.write(".fakehg-log-draft", HG_LOG_OUTGOING.as_bytes());
    tmp.write(".fakehg-parents", HG_PARENTS_ONE.as_bytes());
    let (base, why) = hg::auto_base_commit_with(tmp.path(), &hg, "default", ".").unwrap();
    assert_eq!(base.len(), 40);
    assert!(base.chars().all(|c| c == 'b'), "{base}");
    assert!(why.contains("not outgoing"), "{why}");
}

#[test]
fn hg_base_rules_and_explicit_base() {
    let tmp = TempDir::new("rules");
    let hg = tmp.hg_backend();
    let has_id = |message: &str| message.contains("Differential Revision:");
    let found = hg::resolve_base_commit_rule_with(tmp.path(), &hg, "arc:empty", "user", &has_id)
        .unwrap()
        .unwrap();
    assert_eq!(found.0, "null");
    let (commit, why) = hg::resolve_explicit_base_with(tmp.path(), &hg, "default").unwrap();
    assert_eq!(commit.len(), 40);
    assert!(why.contains("greatest common ancestor"), "{why}");
    let (commit, _) = hg::first_matching_base_rule_with(
        tmp.path(),
        &hg,
        &[("bogus", "user"), ("arc:empty", "user")],
        &has_id,
    )
    .unwrap();
    assert_eq!(commit, "null");
}

#[test]
fn hg_local_commits_and_amend_flow() {
    let tmp = TempDir::new("flow");
    let hg = tmp.hg_ps1_backend();
    let older = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x0110\x01alice\x01Thu, 14 Oct 2026 10:34:56 +0000\x01default\x01\x01\x01first";
    let newer = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x0111\x01Bob <b@x>\x01Thu, 14 Oct 2026 11:00:00 +0000\x01default\x01\x01zzz\x01second";
    tmp.write(
        ".fakehg-log-info",
        format!("{older}\x02{newer}\x02").as_bytes(),
    );
    tmp.write(
        ".fakehg-parents-tpl",
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    );
    let commits =
        hg::get_local_commit_information_with(tmp.path(), &hg, "base", "default").unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].parents.as_ref().unwrap().len(), 1);
    assert_eq!(
        commits[1].parents,
        Some(vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()])
    );

    hg::update_working_copy_with(tmp.path(), &hg).unwrap();
    hg::add_to_commit_with(tmp.path(), &hg, &["a.txt"]).unwrap();
    hg::do_commit_with(tmp.path(), &hg, "msg").unwrap();
    tmp.write(".fakehg-status", b"M a.txt\n");
    tmp.write(".fakehg-msg", b"old\n");
    tmp.write(".fakehg-fail-prune", b"");
    hg::amend_commit_with(tmp.path(), &hg, Some("new"), None).unwrap();
    let log = tmp.read_log(".fakehg.log");
    assert!(log.contains("commit --amend --logfile"), "{log}");
}

#[test]
fn svn_fixture_status() {
    let status = svn::parse_svn_status_xml(SVN_STATUS_SIMPLE).unwrap();
    assert_eq!(status.files["modified.txt"], svn::FLAG_MODIFIED);
    assert_eq!(status.files["added.txt"], svn::FLAG_ADDED);
    assert_eq!(status.files["props.txt"], svn::FLAG_MODIFIED);
    assert_eq!(status.files["newdir"], svn::FLAG_UNTRACKED);
    assert_eq!(status.files["gone.txt"], svn::FLAG_MISSING);
    assert_eq!(status.files["del.txt"], svn::FLAG_DELETED);
    assert_eq!(status.files["conflict.txt"], svn::FLAG_CONFLICT);
    assert_eq!(status.files["tree.txt"], svn::FLAG_CONFLICT);
    assert_eq!(status.base_revisions["modified.txt"], "42");

    let status = svn::parse_svn_status_xml(SVN_STATUS_EXTERNALS).unwrap();
    assert_eq!(status.files["ext"], svn::FLAG_EXTERNALS);
    assert_eq!(
        status.files["ext/inner.txt"],
        svn::FLAG_MODIFIED | svn::FLAG_EXTERNALS
    );
    assert_eq!(status.files["top.txt"], svn::FLAG_MODIFIED);
    assert_eq!(svn::filter_externals(&status.files, false).len(), 1);
    assert_eq!(svn::filter_externals(&status.files, true).len(), 3);
    assert_eq!(svn::get_dirty_external_changes(&status.files).len(), 2);
}

#[test]
fn svn_fixture_info() {
    let info = svn::parse_svn_info(SVN_INFO).unwrap();
    assert_eq!(info["URL"], "https://svn.example.test/repo/trunk");
    assert_eq!(info["Repository Root"], "https://svn.example.test/repo");
    assert_eq!(
        info["Repository UUID"],
        "12345678-1234-1234-1234-1234567890ab"
    );
    assert_eq!(
        svn::parse_svn_date(&info["Last Changed Date"]),
        Some(1791882000)
    );
    assert_eq!(svn::branch_name_from_info(&info), "/trunk");
    assert_eq!(
        svn::get_source_control_base_revision(&info, Some("42")).as_deref(),
        Some("https://svn.example.test/repo/trunk@42")
    );
    let copied = svn::parse_svn_info(SVN_INFO_COPIED).unwrap();
    assert_eq!(
        copied["Copied From URL"],
        "https://svn.example.test/repo/trunk/orig.txt"
    );
    assert_eq!(copied["Copied From Rev"], "40");
}

#[test]
fn svn_fixture_blame_list_diff() {
    let blame = svn::parse_svn_blame(SVN_BLAME).unwrap();
    assert_eq!(
        blame,
        vec![
            ("alice".to_string(), "41".to_string()),
            ("bob".to_string(), "42".to_string()),
        ]
    );
    let files: Vec<String> = SVN_LIST.lines().filter_map(svn::filter_files).collect();
    assert_eq!(
        files,
        vec!["file.txt".to_string(), "src/main.rs".to_string()]
    );
    let map = svn::parse_svn_diff_summarize_xml(SVN_DIFF_SUMMARIZE).unwrap();
    assert_eq!(
        map["https://svn.example.test/repo/trunk/a.txt"],
        svn::FLAG_MODIFIED
    );
    assert_eq!(
        map["https://svn.example.test/repo/trunk/b.txt"],
        svn::FLAG_ADDED
    );
    assert_eq!(
        map["https://svn.example.test/repo/trunk/c.txt"],
        svn::FLAG_DELETED
    );
}

#[test]
fn svn_pipeline_against_fake() {
    let tmp = TempDir::new("pipeline");
    let svn = tmp.svn_backend();
    tmp.write(".fakesvn-status", SVN_STATUS_SIMPLE.as_bytes());
    tmp.write(".fakesvn-info", SVN_INFO.as_bytes());
    tmp.write(".fakesvn-blame", SVN_BLAME.as_bytes());
    tmp.write(".fakesvn-list", SVN_LIST.as_bytes());
    tmp.write(".fakesvn-diff", SVN_DIFF.as_bytes());
    tmp.write(".fakesvn-diffsumm", SVN_DIFF_SUMMARIZE.as_bytes());

    let status = svn::get_svn_status_with(tmp.path(), &svn, &[], false).unwrap();
    assert_eq!(status.files["modified.txt"], svn::FLAG_MODIFIED);
    assert_eq!(status.base_revisions["modified.txt"], "42");
    assert_eq!(
        svn::get_branch_name_with(tmp.path(), &svn).unwrap(),
        "/trunk"
    );
    assert_eq!(
        svn::get_working_copy_revision_with(tmp.path(), &svn, None)
            .unwrap()
            .as_deref(),
        Some("https://svn.example.test/repo/trunk@42")
    );
    assert_eq!(
        svn::get_original_file_data_with(tmp.path(), &svn, "f.txt").unwrap(),
        Some(b"file-bytes".to_vec())
    );
    tmp.write("f.txt", b"cur");
    assert_eq!(
        svn::get_current_file_data(tmp.path(), "f.txt"),
        Some(b"cur".to_vec())
    );
    assert_eq!(
        svn::get_blame_with(tmp.path(), &svn, "f.txt")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(svn::get_all_files_with(tmp.path(), &svn).unwrap().len(), 2);
    let changed = svn::get_changed_files_with(tmp.path(), &svn, "https://x/repo@40").unwrap();
    assert_eq!(changed.len(), 3);
    tmp.write("kept.txt", b"x");
    svn::add_to_commit_with(tmp.path(), &svn, &["kept.txt", "gone.txt"]).unwrap();
    svn::update_working_copy_with(tmp.path(), &svn).unwrap();

    let mut map = svn::StatusMap::new();
    map.insert("modified.txt".to_string(), svn::FLAG_MODIFIED);
    let diff = svn::get_raw_diff_text_with(
        tmp.path(),
        &svn,
        "modified.txt",
        &map,
        &svn::SvnDiffConfig::windows(3),
    )
    .unwrap()
    .unwrap();
    assert_eq!(diff, SVN_DIFF.as_bytes());

    let log = tmp.read_log(".fakesvn.log");
    assert!(log.contains("--xml status"), "{log}");
    assert!(log.contains("info ."), "{log}");
    assert!(log.contains("add -- kept.txt"), "{log}");
    assert!(log.contains("delete -- gone.txt"), "{log}");
    assert!(log.contains("diff --depth empty modified.txt"), "{log}");
}

#[test]
fn svn_copied_addition_from_fixture() {
    let tmp = TempDir::new("copyadd");
    let svn = tmp.svn_backend();
    tmp.write("copied.txt", b"new\n");
    tmp.write(".fakesvn-info", SVN_INFO_COPIED.as_bytes());
    tmp.write(".fakesvn-cat-url", b"old\n");
    tmp.write(
        ".fakediff-canned",
        b"--- a/orig.txt\n+++ b/copied.txt\n@@ -1 +1 @@\n-old\n+new\n",
    );
    let (program, prefix) = tmp.diff_program();
    let prefix_refs: Vec<&str> = prefix.iter().map(String::as_str).collect();
    let config = svn::SvnDiffConfig {
        context: 3,
        diff_cmd: None,
        diff_program: &program,
        diff_prefix: &prefix_refs,
    };
    let mut map = svn::StatusMap::new();
    map.insert("copied.txt".to_string(), svn::FLAG_ADDED);
    let diff = svn::get_raw_diff_text_with(tmp.path(), &svn, "copied.txt", &map, &config)
        .unwrap()
        .unwrap();
    let text = String::from_utf8(diff).unwrap();
    assert!(text.starts_with("Index: copied.txt\n==="), "{text}");
    assert!(text.contains("+++ b/copied.txt"), "{text}");
}

#[test]
fn flags_and_helpers_agree_across_vcs() {
    assert_eq!(hg::FLAG_MODIFIED, svn::FLAG_MODIFIED);
    assert_eq!(hg::FLAG_ADDED, svn::FLAG_ADDED);
    assert_eq!(hg::FLAG_DELETED, svn::FLAG_DELETED);
    assert_eq!(hg::FLAG_UNTRACKED, svn::FLAG_UNTRACKED);
    assert_eq!(hg::FLAG_CONFLICT, svn::FLAG_CONFLICT);
    assert_eq!(hg::FLAG_MISSING, svn::FLAG_MISSING);
    assert_eq!(hg::FLAG_UNSTAGED, svn::FLAG_UNSTAGED);
    assert_eq!(hg::FLAG_UNCOMMITTED, svn::FLAG_UNCOMMITTED);
    assert_eq!(hg::FLAG_EXTERNALS, svn::FLAG_EXTERNALS);
    assert_eq!(hg::FLAG_OBSTRUCTED, svn::FLAG_OBSTRUCTED);
    assert_eq!(hg::FLAG_INCOMPLETE, svn::FLAG_INCOMPLETE);
    assert_eq!(
        hg::get_display_hash("0123456789abcdef"),
        svn::get_display_hash("0123456789abcdef")
    );
    let root = Path::new("/r");
    assert_eq!(
        hg::join_path(root, Some("a")),
        svn::join_path(root, Some("a"))
    );
    let mut range = hg::StatusMap::new();
    range.insert("a".to_string(), hg::FLAG_ADDED);
    let mut uncommitted = hg::StatusMap::new();
    uncommitted.insert("a".to_string(), hg::FLAG_MODIFIED);
    assert_eq!(
        hg::merge_status(&range, &uncommitted),
        svn::merge_status(&range, &uncommitted)
    );
    let tmp = TempDir::new("scratch");
    assert!(hg::write_scratch_file(tmp.path(), "x", "data"));
    assert_eq!(
        svn::read_scratch_file(tmp.path(), "x").as_deref(),
        Some("data")
    );
    assert!(svn::remove_scratch_file(tmp.path(), "x"));
    assert_eq!(hg::read_scratch_file(tmp.path(), "x"), None);
}

#[test]
fn metadata_layouts_differ_per_vcs() {
    let tmp = TempDir::new("meta");
    std::fs::create_dir_all(tmp.path().join("wc").join(".hg")).unwrap();
    std::fs::create_dir_all(tmp.path().join("wc").join("sub")).unwrap();
    std::fs::create_dir_all(tmp.path().join("wc2").join(".svn")).unwrap();
    std::fs::create_dir_all(tmp.path().join("wc2").join("sub")).unwrap();
    assert_eq!(
        hg::get_metadata_path(tmp.path().join("wc").as_path()),
        tmp.path().join("wc").join(".hg")
    );
    assert_eq!(
        svn::find_metadata_path(tmp.path().join("wc2").join("sub").as_path()),
        Some(tmp.path().join("wc2").join(".svn"))
    );
    assert_eq!(
        svn::find_metadata_path(tmp.path().join("wc").join("sub").as_path()),
        None
    );
}
