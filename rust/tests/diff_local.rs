//! Bundle D1: diff local depth through the wired library.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use arc_rs::{diff, diff_local};

/* G1 full-state integration tests: parser on real diffs, VCS helpers. */

#[test]
fn g1_unified_parser_on_real_git_diff() {
    let fx = Fixture::named("g1-parse");
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nB2\nB3\nc\n");
    let text = diff_local::get_full_git_diff(fx.path(), &first, None, 3).unwrap();
    let changes = diff_local::parse_unified_diff(&text).unwrap();
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(change.current_path, "f.txt");
    assert_eq!(change.change_type, diff_local::TYPE_CHANGE);
    assert!(!change.is_message());
    assert_eq!(change.hunks.len(), 1);
    let hunk = &change.hunks[0];
    assert_eq!(
        (hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len),
        (1, 3, 1, 4)
    );
    assert!(hunk.corpus.contains("+B2\n"), "{}", hunk.corpus);
    assert_eq!(change.corpus_size(), hunk.corpus.len());
    assert_eq!(diff_local::without_message_changes(&changes).len(), 1);
}

#[test]
fn g1_unified_parser_vcs_shapes() {
    let hg = "diff -r abc f.txt\n--- f.txt\tdate\n+++ f.txt\tdate\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = diff_local::parse_unified_diff(hg).unwrap();
    assert_eq!(changes[0].current_path, "f.txt");
    assert_eq!(changes[0].hunks.len(), 1);

    let svn =
        "Index: s.txt\n--- s.txt\t(revision 5)\n+++ s.txt\t(working copy)\n@@ -1 +1 @@\n-a\n+b\n";
    assert_eq!(
        diff_local::parse_unified_diff(svn).unwrap()[0].current_path,
        "s.txt"
    );

    let rename =
        "diff --git a/a b/b\nrename from a\nrename to b\n--- a/a\n+++ b/b\n@@ -1 +1 @@\n x\n";
    let change = &diff_local::parse_unified_diff(rename).unwrap()[0];
    assert_eq!(change.change_type, diff_local::TYPE_MOVE_HERE);
    assert_eq!(
        (change.old_path.as_str(), change.current_path.as_str()),
        ("a", "b")
    );

    let binary = "diff --git a/i.png b/i.png\nBinary files a/i.png and b/i.png differ\n";
    let change = &diff_local::parse_unified_diff(binary).unwrap()[0];
    assert_eq!(change.file_type, diff_local::FILE_BINARY);
    assert!(change.hunks.is_empty());

    let show =
        "commit abc\n\n    subject\n\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = diff_local::parse_unified_diff(show).unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes[0].is_message());
    assert_eq!(changes[0].commit_hash.as_deref(), Some("abc"));
    assert_eq!(diff_local::without_message_changes(&changes).len(), 1);
}

#[test]
fn g1_hg_svn_helpers_smoke() {
    assert_eq!(
        diff_local::hg_diff_options(3),
        vec!["--git".to_string(), "-U3".to_string()]
    );
    let argv = diff_local::hg_full_diff_argv("abc", 3);
    assert_eq!(&argv[..5], &["diff", "--git", "-U3", "--rev", "abc"]);
    let entries = diff_local::parse_hg_status("M f\nA g\n? u\n").unwrap();
    assert_eq!(
        entries,
        vec![
            ("f".to_string(), diff_local::FLAG_MODIFIED),
            ("g".to_string(), diff_local::FLAG_ADDED),
            ("u".to_string(), diff_local::FLAG_UNTRACKED),
        ]
    );
    let log = diff_local::parse_hg_commit_log("n1\x01d1\x02");
    assert_eq!(log, vec![("n1".to_string(), "d1".to_string())]);
    assert!(diff_local::hg_supports_amend_from_help("commit --amend"));

    assert_eq!(
        diff_local::svn_props_mask("modified").unwrap(),
        diff_local::FLAG_MODIFIED
    );
    assert_eq!(
        diff_local::parse_svn_item_status("added").unwrap(),
        diff_local::FLAG_ADDED
    );
    assert_eq!(
        diff_local::check_svn_base_consistency(&[("a", 9), ("b", 9)]).unwrap(),
        9
    );
    assert!(diff_local::check_svn_base_consistency(&[("a", 9), ("b", 10)]).is_err());
    assert_eq!(
        diff_local::svn_info_argv("/"),
        vec!["info".to_string(), ".".to_string()]
    );
    assert_eq!(
        diff_local::svn_branch_name("https://s/r/t", "https://s/r"),
        "/t"
    );

    let (author, email) = diff_local::parse_hg_author("A U Thor <a@t>");
    assert_eq!(author.as_deref(), Some("A U Thor"));
    assert_eq!(email.as_deref(), Some("a@t"));
    let info = diff_local::parse_svn_info("URL: u\nRevision: 7\n");
    assert_eq!(info.get("Revision").map(String::as_str), Some("7"));
}

#[test]
fn g1_staging_lint_review_texts() {
    let config = diff_local::StagingConfig::new(true, Some("ssh://x"), true);
    assert_eq!(
        diff_local::staging_skip_reason(false, false, Some("P"), Some(&config)),
        None
    );
    assert_eq!(
        diff_local::staging_skip_reason(true, false, Some("P"), Some(&config)),
        Some(diff_local::STAGING_USER_SKIP)
    );
    assert_eq!(
        diff_local::staging_skip_body(diff_local::STAGING_DIFF_RAW),
        "Raw changes can not be pushed to a staging area."
    );
    assert_eq!(
        diff_local::staging_refspecs("b", "h", "phabricator", 7, false),
        vec![
            "b:refs/tags/phabricator/base/7".to_string(),
            "h:refs/tags/phabricator/diff/7".to_string()
        ]
    );
    assert!(diff_local::git_supports_push_no_verify("2.0"));

    assert_eq!(
        diff_local::lint_result_notice(diff_local::LINT_RESULT_OKAY),
        Some(("LINT OKAY", "No lint problems."))
    );
    assert_eq!(
        diff_local::unit_result_notice(diff_local::UNIT_RESULT_FAIL),
        Some(("UNIT ERRORS", "Unit testing raised errors!"))
    );
    assert!(diff_local::utf8_warning_body(1).contains("1 file(s)"));
    assert!(diff_local::gigantic_confirm("f", 1_000_000).contains("Mark this file as 'binary'"));
    assert!(diff_local::needs_binary_upload(
        diff_local::FILE_BINARY,
        false
    ));
    assert_eq!(
        diff_local::included_commits_block(Some("main"), &["c".to_string()])[1],
        "Included commits in branch main:"
    );
}

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-local-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo(&self) {
        self.git(&["init", "-q", "-b", "main", "."]);
        self.git(&["config", "user.email", "test@example.test"]);
        self.git(&["config", "user.name", "test"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["config", "core.autocrlf", "false"]);
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn write_bytes(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.dir.join(name), bytes).unwrap();
    }

    fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn commit_subject_body(&self, subject: &str, body: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", subject, "-m", body]);
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn fixture_message_parses_revision_and_tags() {
    let corpus = include_str!("fixtures/diff/commit-message.txt");
    let message = diff_local::CommitMessage::new_from_raw_corpus(corpus).unwrap();
    assert_eq!(message.revision_id, Some(789));
    assert_eq!(message.git_svn_base_revision, None);

    let fields = diff_local::parse_field_tags(corpus);
    let text = |value: &str| diff_local::FieldValue::Text(value.to_string());
    assert_eq!(
        fields.get(diff_local::FIELD_TITLE),
        Some(&text("Add retry helper to uploader"))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_SUMMARY),
        Some(&text(
            "Retries failed chunk uploads with exponential backoff so flaky\n\
             connections no longer fail the whole run.\n\
             \n\
             Retry failed chunk uploads with backoff."
        ))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_TEST_PLAN),
        Some(&text("cargo test uploader"))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_REVIEWERS),
        Some(&diff_local::FieldValue::List(vec![
            "alice".to_string(),
            "bob".to_string()
        ]))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_CCS),
        Some(&diff_local::FieldValue::List(vec!["carol".to_string()]))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_TASKS),
        Some(&diff_local::FieldValue::List(vec![
            "T123".to_string(),
            "T456".to_string()
        ]))
    );
}

#[test]
fn commit_message_from_file_reads_and_parses() {
    let fx = Fixture::named("message-file");
    fx.init_repo();
    fx.write(
        "revision.txt",
        include_str!("fixtures/diff/message-file.txt"),
    );
    let message = diff_local::commit_message_from_file(&fx.path().join("revision.txt")).unwrap();
    assert_eq!(message.revision_id, None);
    let fields = diff_local::parse_field_tags(&message.raw_corpus);
    assert_eq!(
        fields.get(diff_local::FIELD_TITLE),
        Some(&diff_local::FieldValue::Text(
            "Fix off-by-one in pager".to_string()
        ))
    );

    let err = diff_local::commit_message_from_file(&fx.path().join("missing.txt")).unwrap_err();
    assert!(err.contains("failed to read message file"), "{err}");
}

#[test]
fn explicit_base_resolves_merge_base() {
    let fx = Fixture::named("explicit-base");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    fx.commit_all("two");

    assert_eq!(
        diff_local::resolve_explicit_base(fx.path(), &first, "HEAD").unwrap(),
        first
    );
    assert_eq!(
        diff_local::resolve_explicit_base(fx.path(), "HEAD~1", "HEAD").unwrap(),
        first
    );
    assert_eq!(
        diff_local::resolve_explicit_base(fx.path(), "nope", "HEAD").unwrap_err(),
        "Unable to find any git commit named 'nope' in this repository."
    );
}

#[test]
fn head_descendant_check_rejects_reversed_ranges() {
    let fx = Fixture::named("range-check");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    let head = fx.commit_all("two");

    assert!(diff_local::require_head_descends_base(fx.path(), &first, "HEAD").is_ok());
    let err = diff_local::require_head_descends_base(fx.path(), &head, &first).unwrap_err();
    assert!(err.contains("is not an ancestor of"), "{err}");
    assert!(err.contains(&first), "{err}");
    assert_eq!(
        diff_local::require_head_descends_base(fx.path(), &first, "nope").unwrap_err(),
        "Unable to find any git commit named 'nope' in this repository."
    );
}

#[test]
fn local_commit_information_lists_newest_first() {
    let fx = Fixture::named("local-info");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_subject_body("one", "first body");
    fx.write("f.txt", "a\nb\n");
    let second = fx.commit_subject_body("two", "second body");

    let infos = diff_local::get_local_commit_information(fx.path(), &first, "HEAD").unwrap();
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    assert_eq!(info.commit, second);
    assert_eq!(info.parents, vec![first.clone()]);
    assert_eq!(info.author, "test");
    assert_eq!(info.author_email, "test@example.test");
    assert_eq!(info.summary, "two");
    assert!(info.message.contains("second body"), "{}", info.message);
    assert!(info.time.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(info.tree.len(), 40);

    let all = diff_local::get_local_commit_information(
        fx.path(),
        diff_local::GIT_MAGIC_ROOT_COMMIT,
        "HEAD",
    )
    .unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn local_commit_information_covers_single_and_empty_repos() {
    let fx = Fixture::named("single-commit");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("only");
    let infos = diff_local::get_local_commit_information(
        fx.path(),
        diff_local::GIT_MAGIC_ROOT_COMMIT,
        "HEAD",
    )
    .unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].commit, head);
    assert!(infos[0].parents.is_empty());

    let bare = Fixture::named("empty-repo");
    bare.init_repo();
    assert_eq!(
        diff_local::get_local_commit_information(bare.path(), "HEAD", "HEAD").unwrap_err(),
        "You can't get local commit information for a repository with no commits."
    );
}

#[test]
fn update_messages_feed_selection_and_prefill() {
    let fx = Fixture::named("update-messages");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_subject_body("one", "first");
    fx.write("f.txt", "a\nb\n");
    fx.commit_subject_body("two", "second");
    fx.write("f.txt", "a\nb\nc\n");
    fx.commit_subject_body("three", "third");

    let messages = diff_local::get_git_update_messages(fx.path(), &first, "HEAD").unwrap();
    assert_eq!(messages.len(), 2);
    assert!(
        messages[0].message.starts_with("three"),
        "{}",
        messages[0].message
    );
    assert!(
        messages[1].message.starts_with("two"),
        "{}",
        messages[1].message
    );

    let empty = HashSet::new();
    let usable = diff_local::select_usable_messages(&messages, &empty).unwrap();
    assert_eq!(usable.len(), 2);
    assert_eq!(
        diff_local::format_usable_logs(&usable),
        "  - two\n  - three\n"
    );

    let active: HashSet<String> = [messages[0].commit_hash.clone()].into_iter().collect();
    assert!(
        diff_local::select_usable_messages(&messages, &active)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn parent_history_discovers_revision_and_svn_metadata() {
    let fx = Fixture::named("parent-history");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_subject_body("plain", "nothing special");
    fx.write("f.txt", "a\nb\n");
    fx.commit_subject_body(
        "import",
        "git-svn-id: http://svn.example.test/repo/trunk@9 uuid-9",
    );
    fx.write("f.txt", "a\nb\nc\n");
    fx.commit_subject_body("feature", "Differential Revision: D55");

    let history = diff_local::get_git_parent_history(fx.path(), "HEAD").unwrap();
    assert_eq!(history.len(), 3);
    let texts: Vec<String> = history.iter().map(|entry| entry.message.clone()).collect();
    let info = diff_local::parent_log_info(&texts);
    assert_eq!(info.parent, Some(55));
    assert_eq!(
        info.base_revision.as_deref(),
        Some("http://svn.example.test/repo/trunk@9")
    );
    assert_eq!(
        info.base_path.as_deref(),
        Some("http://svn.example.test/repo/trunk")
    );
    assert_eq!(info.uuid.as_deref(), Some("uuid-9"));
}

#[test]
fn branch_name_reports_branch_and_detached() {
    let fx = Fixture::named("branch");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    assert_eq!(
        diff_local::get_branch_name(fx.path()).unwrap().as_deref(),
        Some("main")
    );
    fx.git(&["checkout", "-q", "--detach", "HEAD"]);
    assert_eq!(diff_local::get_branch_name(fx.path()).unwrap(), None);
}

#[test]
fn change_entries_classify_add_modify_delete_rename_binary() {
    let fx = Fixture::named("changes");
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    fx.write("g.txt", "x\n");
    fx.write("h.txt", "keep\n");
    let first = fx.commit_all("base");

    fx.write("f.txt", "a\nB2\nB3\nc\n");
    fx.git(&["rm", "-q", "g.txt"]);
    fx.git(&["mv", "h.txt", "renamed.txt"]);
    fx.write("new.txt", "n\n");
    fx.git(&["add", "new.txt"]);
    fx.write_bytes("blob.bin", &[0x89, b'P', b'N', b'G', 0, 1, 2, 3]);
    fx.git(&["add", "blob.bin"]);

    let entries = diff_local::collect_change_entries(fx.path(), &first, None).unwrap();
    assert_eq!(entries.len(), 5);
    let by_path = |path: &str| {
        entries
            .iter()
            .find(|entry| entry.current.path == path)
            .unwrap_or_else(|| panic!("missing {path} in {entries:?}"))
    };

    let modified = by_path("f.txt");
    assert_eq!(modified.change_type, diff_local::TYPE_CHANGE);
    assert_eq!(modified.file_type, diff_local::FILE_TEXT);
    assert_eq!((modified.current.added, modified.current.deleted), (2, 1));

    let added = by_path("new.txt");
    assert_eq!(added.change_type, diff_local::TYPE_ADD);
    assert_eq!((added.current.added, added.current.deleted), (1, 0));

    let deleted = by_path("g.txt");
    assert_eq!(deleted.change_type, diff_local::TYPE_DELETE);
    assert_eq!((deleted.current.added, deleted.current.deleted), (0, 1));

    let renamed = by_path("renamed.txt");
    assert_eq!(renamed.change_type, diff_local::TYPE_MOVE_HERE);
    assert_eq!(renamed.old_path.as_deref(), Some("h.txt"));

    let blob = by_path("blob.bin");
    assert_eq!(blob.change_type, diff_local::TYPE_ADD);
    assert_eq!(blob.file_type, diff_local::FILE_BINARY);

    let block = diff_local::render_included_changes(&entries);
    assert!(block.starts_with("Included changes:\n"), "{block}");
    assert!(block.contains("A (bin) blob.bin"), "{block}");
    assert!(block.contains("from: h.txt"), "{block}");
}

#[test]
fn full_diff_text_or_fail_rejects_empty_ranges() {
    let fx = Fixture::named("full-diff");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");

    assert_eq!(
        diff_local::full_diff_text_or_fail(fx.path(), &first, None, 3).unwrap_err(),
        diff_local::NO_CHANGES_FOUND
    );
    let ranged = diff_local::full_diff_text_or_fail(fx.path(), &first, Some("HEAD"), 3);
    assert_eq!(ranged.unwrap_err(), diff_local::NO_CHANGES_FOUND);

    fx.write("f.txt", "a\nb\n");
    let text = diff_local::full_diff_text_or_fail(fx.path(), &first, None, 3).unwrap();
    assert!(text.contains("diff --git a/f.txt b/f.txt"), "{text}");

    let backed = diff_local::get_full_git_diff(fx.path(), "HEAD", None, 0).unwrap();
    assert!(backed.contains("diff --git"), "{backed}");
}

#[test]
fn diff_specification_loads_local_fields() {
    let fx = Fixture::named("diff-spec");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("one");
    let toplevel = fx.git(&["rev-parse", "--show-toplevel"]);

    let spec = diff_local::load_diff_specification(
        fx.path(),
        "HEAD",
        "test-box",
        Some("PHID-REPO-1"),
        false,
    )
    .unwrap();
    assert_eq!(spec.source_machine, "test-box");
    assert_eq!(spec.source_path.as_deref(), Some(toplevel.as_str()));
    assert_eq!(spec.branch.as_deref(), Some("main"));
    assert_eq!(spec.bookmark, None);
    assert_eq!(
        spec.source_control_system.as_deref(),
        Some(diff_local::SOURCE_CONTROL_SYSTEM_GIT)
    );
    assert_eq!(spec.source_control_path, None);
    assert_eq!(
        spec.source_control_base_revision.as_deref(),
        Some(head.as_str())
    );
    assert_eq!(spec.creation_method, diff_local::CREATION_METHOD);
    let json = spec.to_json();
    assert_eq!(json["repositoryPHID"], serde_json::json!("PHID-REPO-1"));

    let raw =
        diff_local::load_diff_specification(fx.path(), "HEAD", "test-box", None, true).unwrap();
    assert_eq!(raw.source_control_system, None);
    assert_eq!(raw.source_control_base_revision, None);
    assert!(raw.to_json().get("repositoryPHID").is_none());
}

#[test]
fn upstream_branch_feeds_onto_targets() {
    let fx = Fixture::named("onto");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    assert_eq!(diff_local::resolve_upstream_branch(fx.path()), None);

    fx.git(&["branch", "frozen", &first]);
    fx.git(&["branch", "--set-upstream-to=frozen"]);
    assert_eq!(
        diff_local::resolve_upstream_branch(fx.path()).as_deref(),
        Some("frozen")
    );

    let upstream = diff_local::resolve_upstream_branch(fx.path());
    let targets = diff_local::diff_onto_targets(upstream.as_deref(), Some("stable"));
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].kind, diff_local::ONTO_KIND_UPSTREAM);
    assert_eq!(targets[0].name, "frozen");

    let configured = diff_local::diff_onto_targets(None, Some("stable"));
    assert_eq!(configured[0].kind, diff_local::DIFF_ONTO_CONFIG_KEY);
}

/* Gap2 D1: create-message edit loop over canned conduit. */

#[test]
fn gap2_create_message_loop_reedits_then_succeeds() {
    use std::io::Cursor;
    let fx = Fixture::named("gap2-d1-loop");
    let meta = fx.path().join(".git");
    std::fs::create_dir_all(&meta).unwrap();

    let flaky = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.parsecommitmessage"
            && params["corpus"].as_str().unwrap_or("").contains("Bad")
        {
            return Ok(serde_json::json!({"fields": {}, "errors": ["Title is required."]}));
        }
        match method {
            "differential.parsecommitmessage" => {
                Ok(serde_json::json!({"fields": {"title": "Good"}, "errors": []}))
            }
            "differential.query" => Ok(serde_json::json!([])),
            "user.query" => Ok(serde_json::json!({})),
            other => Err(format!("unexpected conduit call {other}")),
        }
    };
    let calls = std::cell::RefCell::new(0usize);
    let edit = |_: &str| -> Result<String, String> {
        *calls.borrow_mut() += 1;
        if *calls.borrow() == 1 {
            return Ok("Bad\n".to_string());
        }
        Ok("Good title\n".to_string())
    };

    let mut input = Cursor::new(b"y\n".to_vec());
    let mut out = Vec::new();
    let mut editor = diff_local::CreateMessageEditor {
        reader: &mut input,
        writer: &mut out,
        call: &flaky,
        metadata_dir: &meta,
        work_root: fx.path(),
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-me",
        away_offset: 0,
    };
    let message =
        diff_local::edit_create_message(&mut editor, "Seed\n", "Seed\n", false, false, &[])
            .unwrap();
    assert_eq!(*calls.borrow(), 2);
    assert_eq!(message.raw_corpus, "Good title\n");
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(diff_local::COMMIT_MESSAGE_HAS_ERRORS),
        "{text}"
    );
    assert!(text.contains("Title is required."), "{text}");
    assert!(text.contains(diff_local::MUST_RESOLVE_ERRORS), "{text}");
    // The final draft lands in the scratch file.
    assert_eq!(
        diff::read_scratch_file(&meta, diff_local::SCRATCH_CREATE_MESSAGE).as_deref(),
        Some("Good title\n")
    );
}

#[test]
fn gap2_saved_template_resume_roundtrip() {
    use std::io::Cursor;
    let fx = Fixture::named("gap2-d1-resume");
    let meta = fx.path().join(".git");
    assert!(diff::write_scratch_file(
        &meta,
        diff_local::SCRATCH_CREATE_MESSAGE,
        "Saved title\n"
    ));

    let mut out = Vec::new();
    let reused = diff_local::resolve_saved_template(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &meta,
        fx.path(),
        false,
    )
    .unwrap();
    assert_eq!(reused.as_deref(), Some("Saved title\n"));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("You have a saved revision message"), "{text}");
    assert!(
        text.contains(diff_local::PROMPT_USE_SAVED_MESSAGE),
        "{text}"
    );
}

/* Gap2 D3: clean working-copy check over seams. */

#[test]
fn gap2_clean_check_commits_unstaged() {
    use std::io::Cursor;
    let added = std::cell::RefCell::new(Vec::<String>::new());
    let add = |paths: &[String]| -> Result<(), String> {
        added.borrow_mut().extend(paths.iter().cloned());
        Ok(())
    };
    let committed = std::cell::RefCell::new(Vec::<String>::new());
    let commit = |message: &str| -> Result<(), String> {
        committed.borrow_mut().push(message.to_string());
        Ok(())
    };
    let edit = |_: &str| -> Result<String, String> { Ok("Msg\n".to_string()) };
    let vcs = diff_local::CleanCheckVcs {
        kind: diff::VcsKind::Git,
        work_path: "/repo".to_string(),
        supports_local_commits: true,
        can_stash: true,
        autostash: false,
        require_clean_untracked: true,
        should_amend: false,
        head_commit_message: None,
        add_to_commit: Some(&add),
        stash_changes: None,
        do_commit: Some(&commit),
        amend_commit: None,
        edit: &edit,
    };
    let changes = diff_local::WorkingCopyChanges {
        unstaged: vec!["s.txt".to_string()],
        ..Default::default()
    };

    let mut out = Vec::new();
    let outcome = diff_local::require_clean_working_copy(
        &mut Cursor::new(b"y\n".to_vec()),
        &mut out,
        &vcs,
        &changes,
        diff_local::CommitMode::Allow,
    )
    .unwrap();
    assert!(!outcome.stashed);
    assert_eq!(added.borrow().as_slice(), ["s.txt".to_string()]);
    assert_eq!(committed.borrow().as_slice(), ["Msg\n".to_string()]);
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(diff_local::UNCOMMITTED_CHANGES_HERE),
        "{text}"
    );
}

#[test]
fn gap2_clean_check_fatal_conflicts() {
    use std::io::Cursor;
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let vcs = diff_local::CleanCheckVcs {
        kind: diff::VcsKind::Git,
        work_path: "/repo".to_string(),
        supports_local_commits: true,
        can_stash: true,
        autostash: false,
        require_clean_untracked: true,
        should_amend: false,
        head_commit_message: None,
        add_to_commit: None,
        stash_changes: None,
        do_commit: None,
        amend_commit: None,
        edit: &edit,
    };
    let changes = diff_local::WorkingCopyChanges {
        conflicts: vec!["c.txt".to_string()],
        ..Default::default()
    };

    let mut out = Vec::new();
    let err = diff_local::require_clean_working_copy(
        &mut Cursor::new(b"".to_vec()),
        &mut out,
        &vcs,
        &changes,
        diff_local::CommitMode::Allow,
    )
    .unwrap_err();
    assert!(err.contains("Conflicts in working copy:"), "{err}");
    assert!(err.contains("    c.txt"), "{err}");
}
