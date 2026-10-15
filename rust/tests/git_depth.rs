//! Git depth harness: parsing, upstream tracking, and error shapes from
//! canned git output, through the wired library.
//!
//! No live git runs here: every test feeds canned stdout (or fixture
//! files under `rust/tests/fixtures/vcs/`) to the pure parsers.

use arc_rs::git;

#[test]
fn fixture_log_commits_parse() {
    let stdout = include_str!("fixtures/vcs/log_commits.txt");
    let commits = git::parse_local_commit_information(stdout);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].commit, format!("c1{}", "a".repeat(38)));
    assert_eq!(commits[0].tree, format!("t1{}", "a".repeat(38)));
    assert_eq!(commits[0].parents.len(), 2);
    assert_eq!(commits[0].author, "Ann Author");
    assert_eq!(commits[0].author_email, "ann@example.test");
    assert_eq!(commits[0].summary, "Add feature");
    assert_eq!(commits[0].message, "Add feature\n\nLong body here.");
    assert!(commits[1].parents.is_empty());
    assert_eq!(commits[1].summary, "Fix bug");
}

#[test]
fn fixture_raw_diff_parse() {
    let stdout = include_str!("fixtures/vcs/raw_diff.txt");
    let files = git::parse_git_raw_diff(stdout);
    assert_eq!(files.get("app/main.rs"), Some(&git::FLAG_MODIFIED));
    assert_eq!(files.get("app/new.rs"), Some(&git::FLAG_ADDED));
    assert_eq!(files.get("app/old.rs"), Some(&git::FLAG_DELETED));
    assert_eq!(files.get("app/renamed.rs"), Some(&git::FLAG_ADDED));
    let full = git::parse_git_raw_diff_full(stdout);
    assert_eq!(
        full.get("app/renamed.rs").map(|e| e.mask),
        Some(git::FLAG_ADDED)
    );
    assert_eq!(
        full.get("app/renamed.rs").map(|e| e.r#ref.as_str()),
        Some("2222222222222222222222222222222222222222")
    );
}

#[test]
fn fixture_status_porcelain_parse() {
    let stdout = include_bytes!("fixtures/vcs/status_porcelain.bin");
    let status = git::parse_porcelain_v2_status(stdout);
    assert_eq!(
        status.get("app/main.rs"),
        Some(&(git::FLAG_UNCOMMITTED | git::FLAG_MODIFIED))
    );
    assert_eq!(status.get("orphan.txt"), Some(&git::FLAG_UNTRACKED));
    assert_eq!(
        status.get("app/renamed.rs"),
        Some(&(git::FLAG_UNCOMMITTED | git::FLAG_ADDED))
    );
    assert_eq!(
        status.get("app/old.rs"),
        Some(&(git::FLAG_UNCOMMITTED | git::FLAG_DELETED))
    );
}

#[test]
fn fixture_branches_parse() {
    let stdout = include_str!("fixtures/vcs/branches_refs.txt");
    let branches = git::parse_all_branches(stdout, Some("dev"));
    assert_eq!(branches.len(), 2);
    assert!(!branches[0].current);
    assert_eq!(branches[0].name, "main");
    assert_eq!(branches[0].epoch, 1_712_000_000);
    assert!(branches[1].current);
    assert_eq!(branches[1].name, "dev");
    assert_eq!(branches[1].text, "Second\n\nMore.");
}

#[test]
fn fixture_blame_parse() {
    let stdout = include_str!("fixtures/vcs/blame_porcelain.txt");
    let blame = git::parse_blame(stdout).unwrap();
    assert_eq!(
        blame,
        vec![git::BlameLine {
            author: "New One".to_string(),
            revision: "B".repeat(40),
        }]
    );
}

#[test]
fn fixture_tree_parse() {
    let stdout = include_str!("fixtures/vcs/tree_list.txt");
    let tree = git::parse_git_tree(stdout).unwrap();
    assert_eq!(tree.len(), 3);
    assert_eq!(
        tree.get("app/main.rs").map(|e| e.r#type.as_str()),
        Some("blob")
    );
    assert_eq!(tree.get("app").map(|e| e.r#type.as_str()), Some("tree"));
    assert_eq!(
        tree.get("vendor/sub").map(|e| e.r#type.as_str()),
        Some("commit")
    );
    assert_eq!(
        tree.get("app/main.rs").map(|e| e.mode.as_str()),
        Some("100644")
    );
}

#[test]
fn fixture_raw_commit_round_trip() {
    let blob = include_str!("fixtures/vcs/raw_commit.txt");
    let commit = git::RawCommit::new_from_raw_blob(blob).unwrap();
    assert_eq!(commit.get_parents().len(), 2);
    assert_eq!(
        commit.get_raw_author(),
        Some("Ann Author <ann@example.test> 1700000000 +0000")
    );
    assert_eq!(
        commit.get_message(),
        Some("Add feature\n\nLong body here.\n")
    );
    assert_eq!(commit.get_raw_blob(), blob);
}

#[test]
fn upstream_path_reaches_remote() {
    let mut path = git::UpstreamPath::new();
    path.add_upstream(
        "feature".to_string(),
        git::UpstreamHop::local("dev".to_string(), false),
    );
    path.add_upstream(
        "dev".to_string(),
        git::UpstreamHop::remote("main".to_string(), "origin".to_string()),
    );
    assert!(path.is_connected_to_remote());
    assert_eq!(path.get_remote_branch_name(), Some("main"));
    assert_eq!(path.get_remote_remote_name(), Some("origin"));
    assert_eq!(path.get_cycle(), None);
    assert_eq!(
        git::classify_upstream_ref("refs/heads/dev").unwrap(),
        git::UpstreamRef::LocalBranch("dev".to_string())
    );
    assert_eq!(
        git::classify_upstream_ref("refs/remotes/origin/main").unwrap(),
        git::UpstreamRef::RemoteBranch {
            remote: "origin".to_string(),
            branch: "main".to_string(),
        }
    );
}

#[test]
fn upstream_cycle_reported() {
    let mut path = git::UpstreamPath::new();
    path.add_upstream(
        "a".to_string(),
        git::UpstreamHop::local("b".to_string(), false),
    );
    path.add_upstream(
        "b".to_string(),
        git::UpstreamHop::local("a".to_string(), true),
    );
    assert!(!path.is_connected_to_remote());
    assert_eq!(
        path.get_cycle(),
        Some(vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "...".to_string()
        ])
    );
}

#[test]
fn error_shapes_match_php() {
    let err = git::unknown_commit_message("zzz");
    assert_eq!(
        err,
        "Unable to find any git commit named 'zzz' in this repository."
    );
    let err = git::relative_commit_not_commit_message("v9");
    assert_eq!(err, "Relative commit 'v9' is not the name of a commit!");
    let err = git::empty_commit_range_message("base", "head");
    assert!(err.contains("(base)"), "{err}");
    assert!(err.contains("(head)"), "{err}");
    assert!(git::commit_range_is_reversed("same\n", "same", "same"));
    assert!(!git::commit_range_is_reversed("base\n", "head", "base"));
}

#[test]
fn version_gates_match_php_fallbacks() {
    // Porcelain v2 needs git >= 2.11; remote get-url needs >= 2.7;
    // ls-remote --get-url needs >= 1.7.5.
    assert!(git::version_at_least("2.11.0", "2.11"));
    assert!(!git::version_at_least("2.10.4", "2.11"));
    assert!(git::version_at_least("2.7.0", "2.7"));
    assert!(!git::version_at_least("2.6.9", "2.7"));
    assert!(git::version_at_least("1.7.5", "1.7.5"));
    assert!(!git::version_at_least("1.7.4.1", "1.7.5"));
    // The ls-remote fallback echoes the remote name when there is no URI.
    assert_eq!(
        git::remote_uri_from_ls_remote(true, "origin\n", "origin"),
        None
    );
    assert_eq!(
        git::remote_uri_from_ls_remote(true, "https://example.test/r.git\n", "origin"),
        Some("https://example.test/r.git".to_string())
    );
}

#[test]
fn diff_option_strings_match_php() {
    assert_eq!(
        git::diff_base_options(),
        vec!["--no-ext-diff", "--no-textconv", "--submodule=short"]
    );
    let options = git::diff_full_options(32767, true);
    assert!(options.contains(&"-U32767".to_string()), "{options:?}");
    assert!(options.iter().any(|o| o == "-M"), "{options:?}");
    assert!(options.iter().any(|o| o == "-C"), "{options:?}");
    let plain = git::diff_full_options(3, false);
    assert!(!plain.iter().any(|o| o == "-M"), "{plain:?}");
}

#[test]
fn branch_and_metadata_shapes() {
    assert_eq!(
        git::branch_name_from_symbolic_ref(0, "refs/heads/main\n", "").unwrap(),
        Some("main".to_string())
    );
    assert_eq!(git::branch_name_from_symbolic_ref(1, "", "").unwrap(), None);
    assert!(git::branch_name_from_symbolic_ref(0, "bogus", "").is_err());
    assert!(git::branch_name_from_symbolic_ref(2, "", "boom").is_err());
    assert_eq!(git::parse_git_version("git version 2.39.2\n"), "2.39.2");
    let repo = std::path::Path::new("/repo");
    assert_eq!(git::parse_metadata_path(".git\n", repo), repo.join(".git"));
    assert_eq!(git::parse_author("Ada <ada@x> 1 +0000"), "Ada");
    assert_eq!(
        git::parse_commit_summary(git::GIT_MAGIC_ROOT_COMMIT, "x"),
        "(The Empty Tree)"
    );
}
