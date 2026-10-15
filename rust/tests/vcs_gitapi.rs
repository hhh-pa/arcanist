//! Git repository API queries against real throwaway repositories.
//!
//! Like the `cfgsys_*` suites, these tests use the real `git` binary: it
//! ships with every checkout, so it is always present where the suite
//! runs. Every test builds its own repo under the temp dir; nothing
//! touches the network (remotes are plain URL strings or local refs).

use std::path::{Path, PathBuf};

use arc_rs::git;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-gitapi-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git must be installed to run the git API suite");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Two commits on `main`, a `feature` branch at HEAD, an `origin` remote
/// (URL string only, never contacted), and `file.txt`, `other.md`,
/// `sub/inner.txt` tracked.
fn init_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    let dir = tmp.path();
    run_git(dir, &["init", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Test User"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    run_git(dir, &["config", "core.autocrlf", "false"]);
    std::fs::write(dir.join("file.txt"), "alpha\n").unwrap();
    run_git(dir, &["add", "file.txt"]);
    run_git(dir, &["commit", "-m", "first subject", "-m", "first body"]);
    std::fs::write(dir.join("file.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(dir.join("other.md"), "# docs\n").unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), "inner\n").unwrap();
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "-m", "second subject"]);
    run_git(dir, &["branch", "feature"]);
    run_git(
        dir,
        &["remote", "add", "origin", "https://example.com/r.git"],
    );
    tmp
}

fn head(dir: &Path) -> String {
    run_git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn version_metadata_and_branch() {
    let repo = init_repo("meta");
    let dir = repo.path();
    let version = git::git_version(dir).unwrap();
    assert!(version.contains('.'), "{version}");

    let meta = git::metadata_path(dir).unwrap();
    assert!(meta.ends_with(".git"), "{}", meta.display());
    assert!(meta.is_dir());

    assert_eq!(git::branch_name(dir).unwrap(), Some("main".to_string()));

    run_git(dir, &["checkout", "--detach", "HEAD"]);
    assert_eq!(git::branch_name(dir).unwrap(), None);
}

#[test]
fn merge_base_and_descendancy() {
    let repo = init_repo("mergebase");
    let dir = repo.path();
    let base = git::merge_base(dir, "main", "feature").unwrap();
    assert_eq!(base, head(dir));

    let child = head(dir);
    let parent = git::resolve_commit(dir, "HEAD~1").unwrap();
    assert!(git::is_descendant(dir, &child, &parent).unwrap());
    assert!(!git::is_descendant(dir, &parent, &child).unwrap());
    assert!(git::merge_base(dir, "nope-a", "nope-b").is_err());
    assert!(git::is_descendant(dir, "nope", "HEAD").is_err());
}

#[test]
fn commit_resolution() {
    let repo = init_repo("resolve");
    let dir = repo.path();
    let parent = git::resolve_commit(dir, "HEAD^").unwrap();
    assert_eq!(parent, git::resolve_commit(dir, "HEAD~1").unwrap());
    assert!(git::resolve_commit(dir, "nope").is_err());

    assert_eq!(git::head_commit(dir).unwrap(), head(dir));
    assert_eq!(git::working_copy_revision(dir).unwrap(), head(dir));
}

#[test]
fn explicit_base_symbol() {
    let repo = init_repo("base");
    let dir = repo.path();
    assert_eq!(
        git::base_commit_for_explicit_symbol(dir, git::GIT_MAGIC_ROOT_COMMIT, "HEAD").unwrap(),
        git::GIT_MAGIC_ROOT_COMMIT
    );
    assert_eq!(
        git::base_commit_for_explicit_symbol(dir, "main", "main").unwrap(),
        head(dir)
    );
    assert!(git::base_commit_for_explicit_symbol(dir, "nope", "HEAD").is_err());
}

#[test]
fn upstream_paths_cover_local_cycle_remote_and_none() {
    let repo = init_repo("upstream-none");
    let dir = repo.path();
    let path = git::get_path_to_upstream(dir, "main").unwrap();
    assert!(!path.is_connected_to_remote());

    let tracked = init_repo("upstream-local");
    let dir = tracked.path();
    run_git(dir, &["checkout", "feature"]);
    run_git(dir, &["branch", "--set-upstream-to=main"]);
    let path = git::get_path_to_upstream(dir, "feature").unwrap();
    assert!(path.get_upstream("feature").is_some());
    assert!(!path.is_connected_to_remote());

    let cycled = init_repo("upstream-cycle");
    let dir = cycled.path();
    run_git(dir, &["branch", "--set-upstream-to=feature", "main"]);
    run_git(dir, &["branch", "--set-upstream-to=main", "feature"]);
    assert!(git::get_path_to_upstream(dir, "main").is_ok());

    let remote = init_repo("upstream-remote");
    let dir = remote.path();
    let hash = head(dir);
    run_git(dir, &["update-ref", "refs/remotes/origin/main", &hash]);
    run_git(dir, &["branch", "--set-upstream-to=origin/main"]);
    let path = git::get_path_to_upstream(dir, "main").unwrap();
    assert!(path.is_connected_to_remote());
    assert_eq!(path.get_remote_remote_name(), Some("origin"));
}

#[test]
fn remote_uris_and_pushability() {
    let repo = init_repo("remote");
    let dir = repo.path();
    let fetch = git::remote_fetch_uri(dir, "origin").unwrap();
    assert!(fetch.contains("example.com"), "{fetch}");
    let push = git::remote_push_uri(dir, "origin").unwrap();
    assert!(push.contains("example.com"), "{push}");
    assert!(git::is_pushable_remote(dir, "origin"));
    assert!(git::is_fetchable_remote(dir, "origin"));

    assert_eq!(git::remote_fetch_uri(dir, "missing"), None);
    assert_eq!(git::remote_push_uri(dir, "missing"), None);
    assert!(!git::is_pushable_remote(dir, "missing"));
    assert!(!git::is_fetchable_remote(dir, "missing"));
}

#[test]
fn remote_uri_follows_tracking_or_defaults_to_origin() {
    let repo = init_repo("remoteuri");
    let dir = repo.path();
    let uri = git::remote_uri(dir).unwrap().unwrap();
    assert!(uri.contains("example.com"), "{uri}");

    run_git(dir, &["checkout", "--detach", "HEAD"]);
    let uri = git::remote_uri(dir).unwrap().unwrap();
    assert!(uri.contains("example.com"), "{uri}");
}

#[test]
fn git_config_values_and_defaults() {
    let repo = init_repo("config");
    let dir = repo.path();
    assert_eq!(
        git::git_config(dir, "user.email"),
        Some("test@example.com".to_string())
    );
    assert_eq!(git::git_config(dir, "arc.missing"), None);
    assert_eq!(
        git::git_config_or(dir, "arc.missing", Some("d")),
        Some("d".to_string())
    );
    assert_eq!(git::git_config_or(dir, "arc.missing", None), None);
}

#[test]
fn author_message_and_summary() {
    let repo = init_repo("author");
    let dir = repo.path();
    assert!(git::author(dir).unwrap().contains("Test User"));

    let full = git::commit_message(dir, "HEAD~1").unwrap();
    assert!(full.contains("first subject"), "{full}");
    assert!(full.contains("first body"), "{full}");
    assert_eq!(git::commit_summary(dir, "HEAD").unwrap(), "second subject");
    assert!(git::commit_summary(dir, git::GIT_MAGIC_ROOT_COMMIT).is_ok());
}

#[test]
fn canonical_names_and_local_commits() {
    let repo = init_repo("canon");
    let dir = repo.path();
    let hash = head(dir);
    assert_eq!(git::canonical_revision_name(dir, "HEAD").unwrap(), hash);
    // No git-svn here: the `@<rev>` path must fail cleanly.
    assert!(git::canonical_revision_name(dir, "HEAD@99").is_err());
    assert!(git::hash_from_svn_revision_number(dir, "99").is_err());
    assert!(git::svn_revision_number_from_hash(dir, &hash).is_err());

    assert!(!git::has_local_commit(dir, None));
    assert!(git::has_local_commit(dir, Some("HEAD")));
    assert!(!git::has_local_commit(dir, Some("deadbeef")));
}

#[test]
fn working_copy_status_reports_each_change_kind() {
    let repo = init_repo("status");
    let dir = repo.path();
    assert!(git::uncommitted_status(dir).unwrap().is_empty());

    std::fs::write(dir.join("file.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::remove_file(dir.join("other.md")).unwrap();
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    run_git(dir, &["add", "new.txt"]);
    let status = git::uncommitted_status(dir).unwrap();
    assert!(status.contains_key("file.txt"), "{status:?}");
    assert!(status.contains_key("other.md"), "{status:?}");
    assert!(status.contains_key("new.txt"), "{status:?}");
}

#[test]
fn commit_range_and_changed_files() {
    let repo = init_repo("range");
    let dir = repo.path();
    let range = git::commit_range_status(dir, "HEAD~1").unwrap();
    assert!(range.contains_key("file.txt"), "{range:?}");
    assert!(range.contains_key("other.md"), "{range:?}");
    assert!(range.contains_key("sub/inner.txt"), "{range:?}");

    let changed = git::changed_files(dir, "HEAD~1").unwrap();
    assert!(changed.contains_key("file.txt"), "{changed:?}");
    assert!(git::commit_range_status(dir, "nope").is_err());
}

#[test]
fn full_and_raw_diffs() {
    let repo = init_repo("diff");
    let dir = repo.path();
    let full = git::full_git_diff(dir, "HEAD~1", Some("HEAD"), 3).unwrap();
    assert!(full.contains("file.txt"), "{full}");
    assert!(full.contains("+beta"), "{full}");

    std::fs::write(dir.join("file.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let worktree = git::full_git_diff(dir, "HEAD", None, 3).unwrap();
    assert!(worktree.contains("+gamma"), "{worktree}");

    let raw = git::raw_diff_text(dir, "HEAD~1", "file.txt", 3, true).unwrap();
    assert!(raw.contains("+beta"), "{raw}");
    let raw = git::raw_diff_text(dir, "HEAD~1", "file.txt", 0, false).unwrap();
    assert!(raw.contains("file.txt"), "{raw}");
}

#[test]
fn commit_and_history_logs() {
    let repo = init_repo("log");
    let dir = repo.path();
    assert_eq!(
        git::git_commit_log(dir, false, "HEAD~1", "HEAD").unwrap(),
        ""
    );
    let all = git::git_commit_log(dir, true, git::GIT_MAGIC_ROOT_COMMIT, "HEAD").unwrap();
    assert!(all.contains("first subject"), "{all}");
    assert!(all.contains("second subject"), "{all}");
    let range = git::git_commit_log(dir, true, "HEAD~1", "HEAD").unwrap();
    assert!(range.contains("second subject"), "{range}");
    assert!(!range.contains("first subject"), "{range}");

    let history = git::git_history_log(dir, "HEAD").unwrap();
    assert!(history.contains("second subject"), "{history}");
}

#[test]
fn blame_attributes_each_line() {
    let repo = init_repo("blame");
    let dir = repo.path();
    // The root commit's line is a boundary row, which the PHP port skips.
    let rows = git::blame(dir, "HEAD", "file.txt").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].author, "Test User");
    assert_eq!(rows[0].revision, head(dir));

    std::fs::write(dir.join("file.txt"), "ALPHA\nbeta\n").unwrap();
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "-m", "third subject"]);
    let rows = git::blame(dir, "HEAD", "file.txt").unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.author == "Test User"));
    let tip = head(dir);
    let parent = git::resolve_commit(dir, "HEAD~1").unwrap();
    assert_eq!(rows[0].revision, tip);
    assert_eq!(rows[1].revision, parent);
}

#[test]
fn file_data_at_revisions() {
    let repo = init_repo("filedata");
    let dir = repo.path();
    assert_eq!(
        git::file_data_at_revision(dir, "file.txt", "HEAD").unwrap(),
        Some(b"alpha\nbeta\n".to_vec())
    );
    assert_eq!(
        git::file_data_at_revision(dir, "file.txt", "HEAD~1").unwrap(),
        Some(b"alpha\n".to_vec())
    );
    assert_eq!(
        git::file_data_at_revision(dir, "missing.txt", "HEAD").unwrap(),
        None
    );
    assert_eq!(git::file_data_at_revision(dir, "", "HEAD").unwrap(), None);
    assert_eq!(
        git::file_data_at_revision(dir, "sub", "HEAD").unwrap(),
        None
    );
    assert_eq!(
        git::original_file_data(dir, "file.txt", "HEAD~1").unwrap(),
        Some(b"alpha\n".to_vec())
    );
    assert_eq!(
        git::current_file_data(dir, "file.txt").unwrap(),
        Some(b"alpha\nbeta\n".to_vec())
    );
}

#[test]
fn branches_and_files() {
    let repo = init_repo("branches");
    let dir = repo.path();
    let hash = head(dir);
    let branches = git::all_branches(dir).unwrap();
    let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"main"), "{names:?}");
    assert!(names.contains(&"feature"), "{names:?}");
    let main = branches.iter().find(|b| b.name == "main").unwrap();
    assert!(main.current);
    assert_eq!(main.hash, hash);

    let files = git::all_files(dir).unwrap();
    assert!(files.contains(&"file.txt".to_string()), "{files:?}");
    assert!(files.contains(&"other.md".to_string()), "{files:?}");
    assert!(files.contains(&"sub/inner.txt".to_string()), "{files:?}");
}

#[test]
fn local_commit_information_lists_range() {
    let repo = init_repo("localcommits");
    let dir = repo.path();
    let commits = git::local_commit_information(dir, "HEAD").unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].summary, "second subject");
    assert_eq!(commits[1].summary, "first subject");
    assert_eq!(commits[0].author, "Test User");
    assert_eq!(commits[0].author_email, "test@example.com");
    assert_eq!(commits[0].commit, head(dir));
    assert_eq!(commits[1].parents.len(), 0);
    assert!(git::local_commit_information(dir, "nope").is_err());
}

#[test]
fn raw_commit_round_trip() {
    let repo = init_repo("rawcommit");
    let dir = repo.path();
    let hash = head(dir);
    let raw = git::read_raw_commit(dir, &hash).unwrap();
    assert_eq!(git::write_raw_commit(dir, &raw).unwrap(), hash);
    assert!(git::read_raw_commit(dir, "deadbeef").is_err());
}

#[test]
fn stage_commit_and_amend_flow() {
    let repo = init_repo("commitflow");
    let dir = repo.path();
    std::fs::write(dir.join("stage.txt"), "staged\n").unwrap();
    git::add_to_commit(dir, &["stage.txt"]).unwrap();
    git::do_commit(dir, "third subject").unwrap();
    assert_eq!(git::commit_summary(dir, "HEAD").unwrap(), "third subject");

    git::amend_commit(dir, Some("amended subject")).unwrap();
    assert_eq!(git::commit_summary(dir, "HEAD").unwrap(), "amended subject");
    git::amend_commit(dir, None).unwrap();
    assert_eq!(git::commit_summary(dir, "HEAD").unwrap(), "amended subject");
}

#[test]
fn stash_and_unstash_flow() {
    let repo = init_repo("stash");
    let dir = repo.path();
    std::fs::write(dir.join("file.txt"), "alpha\nbeta\nstashed\n").unwrap();
    git::stash_changes(dir).unwrap();
    assert!(git::uncommitted_status(dir).unwrap().is_empty());
    git::unstash_changes(dir).unwrap();
    let body = std::fs::read(dir.join("file.txt")).unwrap();
    assert_eq!(body, b"alpha\nbeta\nstashed\n");
}

#[test]
fn repo_kind_probes() {
    let repo = init_repo("kinds");
    let dir = repo.path();
    assert!(git::update_working_copy(dir).is_err());
    assert!(!git::is_git_subversion_repo(dir));
    assert!(!git::is_perforce_remote(dir, "origin"));
    assert!(!git::is_perforce_remote(dir, "p4"));
    assert!(!git::is_git_lfs_working_copy(dir));
}

/// Point `git svn` at a shell function instead of git-svn: aliases win
/// over external subcommands, so the helpers below run hermetically (no
/// git-svn/perl needed, and a real git-svn install cannot leak in).
fn alias_git_svn(dir: &Path, shell: &str) {
    run_git(dir, &["config", "alias.svn", shell]);
}

#[test]
fn git_svn_find_rev_through_alias() {
    let repo = init_repo("gitsvn");
    let dir = repo.path();
    let hash = head(dir);
    alias_git_svn(dir, &format!("!f() {{ echo {hash}; }}; f"));
    assert_eq!(git::execute_svn_find_rev(dir, "r42", "Git").unwrap(), hash);
    assert_eq!(git::hash_from_svn_revision_number(dir, "42").unwrap(), hash);

    alias_git_svn(dir, "!f() { echo 456; }; f");
    assert_eq!(
        git::svn_revision_number_from_hash(dir, &hash).unwrap(),
        "456"
    );
    // A partial rebuild prints progress first: the second-to-last line
    // wins, mirroring `executeSVNFindRev`.
    alias_git_svn(dir, &format!("!f() {{ echo rebuilding; echo {hash}; }}; f"));
    assert_eq!(git::execute_svn_find_rev(dir, "r42", "Git").unwrap(), hash);
}

#[test]
fn git_svn_find_rev_missing_reports_both_directions() {
    let repo = init_repo("gitsvn-missing");
    let dir = repo.path();
    alias_git_svn(dir, "!f() { exit 1; }; f");
    assert_eq!(
        git::execute_svn_find_rev(dir, "r42", "Git").unwrap_err(),
        "Cannot find the Git equivalent of r42."
    );
    assert_eq!(
        git::hash_from_svn_revision_number(dir, "42").unwrap_err(),
        "Cannot find the Git equivalent of r42."
    );
    let hash = head(dir);
    assert_eq!(
        git::svn_revision_number_from_hash(dir, &hash).unwrap_err(),
        format!("Cannot find the SVN equivalent of {hash}.")
    );
    // Empty output is missing too.
    alias_git_svn(dir, "!f() { :; }; f");
    assert_eq!(
        git::execute_svn_find_rev(dir, "r42", "Git").unwrap_err(),
        "Cannot find the Git equivalent of r42."
    );
}
