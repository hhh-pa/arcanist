//! Bundle D1: diff local depth through the wired library.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use arc_rs::diff_local;

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
