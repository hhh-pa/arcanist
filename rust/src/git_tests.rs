use super::*;

fn fresh_repo(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-git-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["init", "-q", "-b", "master"])
        .output()
        .unwrap();
    assert!(out.status.success());
    for (key, value) in [("user.email", "t@t"), ("user.name", "t")] {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["config", key, value])
            .output()
            .unwrap();
    }
    dir
}

#[test]
fn detects_work_trees() {
    let repo = fresh_repo("tree");
    assert!(is_inside_work_tree(&repo));
    let plain = std::env::temp_dir().join(format!("arc-rs-git-plain-{}", std::process::id()));
    // Cover the stale-cleanup arm: a leftover dir is cleared first.
    std::fs::create_dir_all(&plain).unwrap();
    if plain.exists() {
        std::fs::remove_dir_all(&plain).ok();
    }
    std::fs::create_dir_all(&plain).unwrap();
    assert!(!is_inside_work_tree(&plain));
    std::fs::remove_dir_all(&repo).ok();
    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn stdout_text_ok_and_err() {
    let repo = fresh_repo("text");
    let top = stdout_text(&repo, &["rev-parse", "--show-toplevel"]).unwrap();
    assert!(!top.is_empty());
    let err = stdout_text(&repo, &["rev-parse", "--verify", "no-such-rev"]).unwrap_err();
    assert!(!err.is_empty());
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn output_and_ok_match_exit_status() {
    let repo = fresh_repo("ok");
    assert!(ok(&repo, &["rev-parse"]));
    assert!(!ok(&repo, &["no-such-subcommand"]));
    let raw = output(&repo, &["rev-parse", "--show-toplevel"]).unwrap();
    assert!(raw.status.success());
    assert!(!raw.stdout.is_empty());
    std::fs::remove_dir_all(&repo).ok();
}

// Everything below uses canned command output only: no new test spawns git.

#[test]
fn constants_match_php() {
    assert_eq!(
        GIT_MAGIC_ROOT_COMMIT,
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    );
    assert_eq!(GIT_EMPTY_TREE_HASH, GIT_MAGIC_ROOT_COMMIT);
    assert_eq!(SEARCH_LENGTH_FOR_PARENT_REVISIONS, 16);
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
    assert_eq!(UpstreamPath::TYPE_LOCAL, "local");
    assert_eq!(UpstreamPath::TYPE_REMOTE, "remote");
    assert_eq!(UpstreamType::Local.as_str(), "local");
    assert_eq!(UpstreamType::Remote.as_str(), "remote");
    assert_eq!(UPSTREAM_LOCAL_PREFIX, "refs/heads/");
    assert_eq!(UPSTREAM_REMOTE_PREFIX, "refs/remotes/");
}

#[test]
fn capabilities_match_php() {
    assert_eq!(source_control_system_name(), "git");
    assert_eq!(current_commit_symbol(), "HEAD");
    assert_eq!(source_control_path(), None);
    assert!(!is_history_default_immutable());
    assert!(supports_amend());
    assert!(supports_commit_ranges());
    assert!(supports_local_commits());
    assert!(can_stash_changes());
    assert!(finalized_revision_message().contains("git push"));
    assert!(finalized_revision_message().contains("git svn dcommit"));
    assert_eq!(
        display_hash("0123456789abcdef0123456789abcdef01234567"),
        "0123456789ab"
    );
    assert_eq!(display_hash("short"), "short");
}

#[test]
fn parses_git_version() {
    assert_eq!(parse_git_version("git version 2.39.2\n"), "2.39.2");
    assert_eq!(
        parse_git_version("git version 2.39.2.windows.1\n"),
        "2.39.2.windows.1"
    );
    assert_eq!(parse_git_version("git version 1.7.5"), "1.7.5");
}

#[test]
fn compares_versions_numerically() {
    assert!(version_at_least("2.39.2", "2.7"));
    assert!(version_at_least("2.7", "2.7"));
    assert!(version_at_least("2.11.0", "2.11.0"));
    assert!(version_at_least("2.39.2.windows.1", "2.39.2"));
    assert!(version_at_least("1.7.11", "1.7.5"));
    assert!(!version_at_least("1.7.4", "1.7.5"));
    assert!(!version_at_least("2.6.9", "2.7"));
    assert!(!version_at_least("", "1.0"));
    assert!(version_at_least("2.7", ""));
}

#[test]
fn parses_metadata_path() {
    let repo = std::path::Path::new("/repo");
    assert_eq!(parse_metadata_path(".git\n", repo), repo.join(".git"));
    assert_eq!(
        parse_metadata_path("/abs/path/.git\n", repo),
        std::path::PathBuf::from("/abs/path/.git")
    );
}

#[test]
fn parses_branch_name_from_ref() {
    assert_eq!(
        parse_branch_name_from_ref("refs/heads/main"),
        Some("main".to_string())
    );
    assert_eq!(
        parse_branch_name_from_ref("refs/heads/feat/x"),
        Some("feat/x".to_string())
    );
    assert_eq!(parse_branch_name_from_ref("refs/remotes/origin/main"), None);
    assert_eq!(parse_branch_name_from_ref("refs/heads/"), None);
    assert_eq!(parse_branch_name_from_ref("main"), None);
    assert_eq!(parse_branch_name_from_ref(""), None);
}

#[test]
fn interprets_symbolic_ref_status() {
    assert_eq!(
        branch_name_from_symbolic_ref(0, "refs/heads/main\n", "").unwrap(),
        Some("main".to_string())
    );
    let err = branch_name_from_symbolic_ref(0, "garbage\n", "").unwrap_err();
    assert!(
        err.contains("Failed to parse git symbolic-ref output"),
        "{err}"
    );
    assert_eq!(branch_name_from_symbolic_ref(1, "", "").unwrap(), None);
    let err = branch_name_from_symbolic_ref(128, "", "fatal: bad").unwrap_err();
    assert!(
        err.contains("Command git symbolic-ref failed: fatal: bad"),
        "{err}"
    );
}

#[test]
fn detects_descendants() {
    assert!(is_descendant_output("abc\n", "def", "abc"));
    assert!(!is_descendant_output("abc", "abc", "abc"));
    assert!(!is_descendant_output("xyz", "def", "abc"));
    assert!(!commit_range_is_reversed("abc\n", "def", "abc"));
    assert!(commit_range_is_reversed("abc\n", "abc", "abc"));
}

#[test]
fn formats_error_shapes() {
    let err = unknown_commit_message("nope");
    assert!(err.contains("'nope'"), "{err}");
    assert!(err.contains("Unable to find any git commit named"), "{err}");
    let err = relative_commit_not_commit_message("v1-tag");
    assert!(err.contains("'v1-tag'"), "{err}");
    assert!(err.contains("is not the name of a commit"), "{err}");
    let err = empty_commit_range_message("base1", "head1");
    assert!(err.contains("base1"), "{err}");
    assert!(err.contains("head1"), "{err}");
    assert!(err.contains("empty, backward or invalid"), "{err}");
    assert_eq!(
        svn_find_rev_missing_message("r123", "Git"),
        "Cannot find the Git equivalent of r123."
    );
    assert_eq!(commit_range_against("HEAD", "abc"), "HEAD --not abc");
    assert_eq!(
        log_local_commit_format(),
        "%H%x01%T%x01%P%x01%at%x01%an%x01%aE%x01%s%x01%s%n%n%b%x02"
    );
}

#[test]
fn parses_local_commit_information() {
    let stdout = "c1\x01t1\x01p1 p2\x011700000000\x01Ann\x01ann@x\x01Subj one\x01Subj one\n\nBody one\x02\
         c2\x01t2\x01\x011700000001\x01Bob\x01bob@x\x01Subj two\x01Subj two\x02";
    let commits = parse_local_commit_information(stdout);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].commit, "c1");
    assert_eq!(commits[0].tree, "t1");
    assert_eq!(commits[0].parents, vec!["p1".to_string(), "p2".to_string()]);
    assert_eq!(commits[0].time, "1700000000");
    assert_eq!(commits[0].author, "Ann");
    assert_eq!(commits[0].summary, "Subj one");
    assert_eq!(commits[0].message, "Subj one\n\nBody one");
    assert_eq!(commits[0].author_email, "ann@x");
    assert!(commits[1].parents.is_empty());
    assert_eq!(commits[1].message, "Subj two");
    assert!(parse_local_commit_information("  \n\x02").is_empty());
    assert!(parse_local_commit_information("short\x01record\x02").is_empty());
}

#[test]
fn builds_diff_options() {
    assert_eq!(
        diff_base_options(),
        vec!["--no-ext-diff", "--no-textconv", "--submodule=short"]
    );
    let full = diff_full_options(3, true);
    assert!(full.contains(&"-U3".to_string()), "{full:?}");
    assert!(full.contains(&"--no-color".to_string()), "{full:?}");
    assert!(full.contains(&"--src-prefix=a/".to_string()), "{full:?}");
    assert!(full.contains(&"--dst-prefix=b/".to_string()), "{full:?}");
    assert_eq!(
        &full[full.len() - 2..],
        ["-M".to_string(), "-C".to_string()]
    );
    let plain = diff_full_options(0x7FFF, false);
    assert!(plain.contains(&"-U32767".to_string()), "{plain:?}");
    assert!(!plain.contains(&"-M".to_string()), "{plain:?}");
}

#[test]
fn parses_git_raw_diff() {
    let stdout = ":100644 100644 aaa.... bbb.... M\tmod.txt\n\
         :100644 100644 aaa.... bbb.... A\tadd.txt\n\
         :100644 000000 aaa.... 000.... D\tdel.txt\n\
         :100644 100644 aaa.... bbb.... R100\told.txt\tnew.txt\n\
         :100644 100644 aaa.... bbb.... C75\tsrc.txt\tdup.txt\n\
         :160000 160000 111.... 222.... M\tsub.txt\n\
         :160000 160000 111.... 111.... M\tdirty-sub.txt\n\
         garbage\n";
    let files = parse_git_raw_diff(stdout);
    assert_eq!(files.get("mod.txt"), Some(&FLAG_MODIFIED));
    assert_eq!(files.get("add.txt"), Some(&FLAG_ADDED));
    assert_eq!(files.get("del.txt"), Some(&FLAG_DELETED));
    assert_eq!(files.get("old.txt"), Some(&FLAG_DELETED));
    assert_eq!(files.get("new.txt"), Some(&FLAG_ADDED));
    assert_eq!(files.get("dup.txt"), Some(&FLAG_ADDED));
    assert!(!files.contains_key("src.txt"));
    assert_eq!(files.get("sub.txt"), Some(&FLAG_MODIFIED));
    assert_eq!(files.get("dirty-sub.txt"), Some(&FLAG_EXTERNALS));
    assert!(!files.contains_key("garbage"));
}

#[test]
fn parses_git_raw_diff_full_refs() {
    let stdout = ":100644 100644 aaa.... bbb.... M\tmod.txt\n\
         :100644 100644 aaa.... bbb.... R100\told.txt\tnew.txt\n";
    let files = parse_git_raw_diff_full(stdout);
    assert_eq!(
        files.get("mod.txt"),
        Some(&RawDiffEntry {
            mask: FLAG_MODIFIED,
            r#ref: "bbb".to_string()
        })
    );
    assert_eq!(
        files.get("old.txt"),
        Some(&RawDiffEntry {
            mask: FLAG_DELETED,
            r#ref: "0000000000000000000000000000000000000000".to_string(),
        })
    );
    assert_eq!(files.get("new.txt").map(|e| e.mask), Some(FLAG_ADDED));
    assert!(parse_git_raw_diff("").is_empty());
}

#[test]
fn parses_porcelain_v2_status() {
    let mut stdout: Vec<u8> = Vec::new();
    for entry in [
        "1 M. N... 100644 100644 100644 h1 h2 staged.txt",
        "1 .M N... 100644 100644 100644 h1 h2 unstaged.txt",
        "1 M. xxMU 100644 100644 100644 h1 h2 sub.txt",
        "? new.txt",
        "u UU N... 100644 100644 100644 100644 h1 h2 h3 conflict.txt",
        "2 R. N... 100644 100644 100644 h1 h2 R100 renamed.txt",
        "orig.txt",
        "9 nonsense",
    ] {
        stdout.extend_from_slice(entry.as_bytes());
        stdout.push(0);
    }
    let status = parse_porcelain_v2_status(&stdout);
    assert_eq!(
        status.get("staged.txt"),
        Some(&(FLAG_UNCOMMITTED | FLAG_MODIFIED))
    );
    assert_eq!(
        status.get("unstaged.txt"),
        Some(&(FLAG_UNCOMMITTED | FLAG_UNSTAGED | FLAG_MODIFIED))
    );
    assert_eq!(
        status.get("sub.txt"),
        Some(&(FLAG_UNCOMMITTED | FLAG_MODIFIED | FLAG_EXTERNALS))
    );
    assert_eq!(status.get("new.txt"), Some(&FLAG_UNTRACKED));
    assert_eq!(
        status.get("conflict.txt"),
        Some(&(FLAG_UNCOMMITTED | FLAG_UNSTAGED))
    );
    assert_eq!(
        status.get("renamed.txt"),
        Some(&(FLAG_UNCOMMITTED | FLAG_ADDED))
    );
    assert_eq!(
        status.get("orig.txt"),
        Some(&(FLAG_UNCOMMITTED | FLAG_DELETED))
    );
    assert!(!status.contains_key("9 nonsense"));
    assert!(parse_porcelain_v2_status(b"").is_empty());
    assert!(parse_porcelain_v2_status(b"1 M. short\0").is_empty());
}

#[test]
fn parses_git_tree() {
    let stdout = format!(
        "100644 blob {}\tfile.txt\n040000 tree {}\tsub\n",
        "a".repeat(40),
        "b".repeat(40)
    );
    let tree = parse_git_tree(&stdout).unwrap();
    assert_eq!(
        tree.get("file.txt"),
        Some(&TreeEntry {
            mode: "100644".to_string(),
            r#type: "blob".to_string(),
            r#ref: "a".repeat(40),
        })
    );
    assert_eq!(tree.get("sub").map(|e| e.r#type.as_str()), Some("tree"));
    assert!(parse_git_tree("  \n").unwrap().is_empty());
    let err = parse_git_tree("bogus line\n").unwrap_err();
    assert!(err.contains("Failed to parse git ls-tree output"), "{err}");
    let err = parse_git_tree("100644 blob short\tfile.txt\n").unwrap_err();
    assert!(err.contains("Failed to parse git ls-tree output"), "{err}");
}

#[test]
fn parses_all_branches() {
    assert_eq!(
        all_branches_format(),
        "%(refname)%01%(objectname)%01%(committerdate:raw)%01%(tree)%01%(subject)%01\
         %(subject)%0a%0a%(body)%01%02"
    );
    let stdout = "refs/heads/main\x01h1\x011712000000 +0000\x01t1\x01One\x01One\x02\
         refs/heads/dev\x01h2\x011712000001 +0000\x01t2\x01Two\x01Two body\x02\
         short\x01record\x02\
         refs/remotes/origin/main\x01h3\x011\x01t3\x01X\x01X\x02";
    let branches = parse_all_branches(stdout, Some("main"));
    assert_eq!(branches.len(), 2);
    assert!(branches[0].current);
    assert_eq!(branches[0].name, "main");
    assert_eq!(branches[0].r#ref, "refs/heads/main");
    assert_eq!(branches[0].hash, "h1");
    assert_eq!(branches[0].tree, "t1");
    assert_eq!(branches[0].epoch, 1_712_000_000);
    assert_eq!(branches[0].desc, "One");
    assert!(!branches[1].current);
    assert_eq!(branches[1].text, "Two body");
    let detached = parse_all_branches(stdout, None);
    assert!(detached.iter().all(|b| !b.current));
}

#[test]
fn parses_blame() {
    let old = "A".repeat(40);
    let new = "B".repeat(40);
    let stdout = format!(
        "{old} 1 1 1\nauthor Old\nboundary\nfilename f.txt\n\told line\n\
         {new} 2 2 1\nauthor New\nfilename f.txt\n\tnew line\n"
    );
    let blame = parse_blame(&stdout).unwrap();
    assert_eq!(
        blame,
        vec![BlameLine {
            author: "New".to_string(),
            revision: new
        }]
    );
    let err = parse_blame(&format!("{old} 1 1 1\nsummary x\n\tline\n")).unwrap_err();
    assert!(err.contains("no author for commit"), "{err}");
    assert!(parse_blame("").is_err());
}

#[test]
fn parses_config_author_and_summary() {
    assert_eq!(
        config_value_from_output(true, "value\n"),
        Some("value".to_string())
    );
    assert_eq!(config_value_from_output(false, ""), None);
    assert_eq!(parse_author("Ada <ada@x> 1700000000 +0000\n"), "Ada");
    assert_eq!(parse_author("NoEmailName\n"), "NoEmailName");
    assert_eq!(parse_author("A<B"), "A<B");
    assert_eq!(
        parse_commit_summary(GIT_MAGIC_ROOT_COMMIT, "whatever"),
        "(The Empty Tree)"
    );
    assert_eq!(parse_commit_summary("abc", "  Subj\n"), "Subj");
}

#[test]
fn parses_svn_helpers() {
    assert_eq!(svn_revision_suffix("foo@123"), Some("123".to_string()));
    assert_eq!(svn_revision_suffix("foo"), None);
    assert_eq!(svn_revision_suffix("foo@"), None);
    assert_eq!(svn_revision_suffix("foo@12b"), None);
    assert_eq!(svn_revision_suffix("a@1@2"), Some("2".to_string()));
    assert_eq!(
        parse_svn_find_rev("Partial rebuild...\nabc123\n"),
        Some("abc123".to_string())
    );
    assert_eq!(parse_svn_find_rev("only\n"), Some("only".to_string()));
    assert_eq!(parse_svn_find_rev(""), None);
    assert_eq!(parse_svn_find_rev("noline"), None);
}

#[test]
fn parses_base_commit_rules() {
    assert_eq!(
        parse_base_commit_rule("git:merge-base(x)"),
        Some(("git".to_string(), "merge-base(x)".to_string()))
    );
    assert_eq!(
        parse_base_commit_rule("arc:empty"),
        Some(("arc".to_string(), "empty".to_string()))
    );
    assert_eq!(parse_base_commit_rule("no-colon"), None);
    let known = ["main", "dev"];
    assert_eq!(
        filter_branch_contains_line("* main", &known),
        Some("main".to_string())
    );
    assert_eq!(
        filter_branch_contains_line("  dev", &known),
        Some("dev".to_string())
    );
    assert_eq!(
        filter_branch_contains_line("(HEAD detached at abc)", &known),
        None
    );
    assert_eq!(filter_branch_contains_line("* (no branch)", &known), None);
}

#[test]
fn normalizes_remote_uris() {
    assert_eq!(
        normalize_remote_uri("https://example.test/r.git\n"),
        Some("https://example.test/r.git".to_string())
    );
    assert_eq!(normalize_remote_uri("  \n"), None);
    assert_eq!(normalize_remote_uri(""), None);
    assert_eq!(
        remote_uri_from_ls_remote(true, "https://example.test/r.git\n", "origin"),
        Some("https://example.test/r.git".to_string())
    );
    assert_eq!(remote_uri_from_ls_remote(true, "origin\n", "origin"), None);
    assert_eq!(remote_uri_from_ls_remote(false, "", "origin"), None);
}

#[test]
fn detects_lfs_output() {
    assert!(lfs_output_indicates_lfs(true, b"file.bin\0"));
    assert!(!lfs_output_indicates_lfs(true, b""));
    assert!(!lfs_output_indicates_lfs(false, b"file.bin\0"));
}

#[test]
fn classifies_upstream_refs() {
    assert_eq!(
        classify_upstream_ref("refs/heads/dev").unwrap(),
        UpstreamRef::LocalBranch("dev".to_string())
    );
    assert_eq!(
        classify_upstream_ref("refs/remotes/origin/feat/x").unwrap(),
        UpstreamRef::RemoteBranch {
            remote: "origin".to_string(),
            branch: "feat/x".to_string()
        }
    );
    let err = classify_upstream_ref("refs/tags/v1").unwrap_err();
    assert!(err.contains("unrecognized upstream format"), "{err}");
    assert!(classify_upstream_ref("refs/remotes/bare").is_err());
}

#[test]
fn tracks_upstream_paths() {
    let mut path = UpstreamPath::new();
    assert_eq!(path.get_length(), 0);
    assert!(!path.is_connected_to_remote());
    assert_eq!(path.get_remote_branch_name(), None);
    assert_eq!(path.get_remote_remote_name(), None);
    assert_eq!(path.get_cycle(), None);
    path.add_upstream("a".to_string(), UpstreamHop::local("b".to_string(), false));
    path.add_upstream(
        "b".to_string(),
        UpstreamHop::remote("main".to_string(), "origin".to_string()),
    );
    assert_eq!(path.get_length(), 2);
    assert_eq!(
        path.get_local_branches(),
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(path.is_connected_to_remote());
    assert_eq!(path.get_remote_branch_name(), Some("main"));
    assert_eq!(path.get_remote_remote_name(), Some("origin"));
    assert_eq!(path.get_cycle(), None);
    assert_eq!(path.get_upstream("a").map(|h| h.name.as_str()), Some("b"));
    assert_eq!(path.get_upstream("missing"), None);
    path.remove_upstream("a");
    assert_eq!(path.get_length(), 1);
    path.remove_upstream("missing");

    let mut cycle = UpstreamPath::new();
    cycle.add_upstream("x".to_string(), UpstreamHop::local("y".to_string(), false));
    cycle.add_upstream("y".to_string(), UpstreamHop::local("x".to_string(), true));
    assert!(!cycle.is_connected_to_remote());
    assert_eq!(
        cycle.get_cycle(),
        Some(vec![
            "x".to_string(),
            "y".to_string(),
            "x".to_string(),
            "...".to_string()
        ])
    );
}

#[test]
fn round_trips_raw_commits() {
    let blob = "tree ttt\nparent p1\nparent p2\nauthor A <a> 1 +0000\ncommitter C <c> 2 +0000\n\nSubject\n\nBody\n";
    let commit = RawCommit::new_from_raw_blob(blob).unwrap();
    assert_eq!(commit.get_tree_hash(), Some("ttt"));
    assert_eq!(commit.get_parents(), &["p1".to_string(), "p2".to_string()]);
    assert_eq!(commit.get_raw_author(), Some("A <a> 1 +0000"));
    assert_eq!(commit.get_raw_committer(), Some("C <c> 2 +0000"));
    assert_eq!(commit.get_message(), Some("Subject\n\nBody\n"));
    assert_eq!(commit.get_raw_blob(), blob);

    let empty = RawCommit::new_empty_commit();
    assert_eq!(empty.get_tree_hash(), Some(GIT_EMPTY_TREE_HASH));
    assert!(empty.get_parents().is_empty());
    assert_eq!(
        empty.get_raw_blob(),
        format!("tree {GIT_EMPTY_TREE_HASH}\n\n")
    );

    let mut edited = RawCommit::default();
    edited
        .set_tree_hash(Some("t".to_string()))
        .set_raw_author(Some("a".to_string()))
        .set_raw_committer(None)
        .set_parents(vec!["p".to_string()])
        .add_parent("q".to_string())
        .set_message(Some("m".to_string()));
    assert_eq!(edited.get_parents(), &["p".to_string(), "q".to_string()]);
    assert_eq!(
        edited.get_raw_blob(),
        "tree t\nparent p\nparent q\nauthor a\n\nm"
    );

    assert!(RawCommit::new_from_raw_blob("tree t\nno-spaces-here\n").is_err());
    assert!(RawCommit::new_from_raw_blob("frob x\n").is_err());
    assert!(RawCommit::new_from_raw_blob("tree a\ntree b\n").is_err());
    let headers_only = RawCommit::new_from_raw_blob("tree t\n").unwrap();
    assert_eq!(headers_only.get_message(), Some(""));
}

fn plain_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-git-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// House scratch dir (pid + per-file counter); removed on drop.
struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-git-tests-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn commit_one(dir: &std::path::Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    for args in [
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "core.autocrlf", "false"],
        vec!["add", name],
        vec!["commit", "-q", "-m", contents],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }
}

#[test]
fn missing_directories_are_not_work_trees() {
    let missing = std::env::temp_dir().join(format!(
        "arc-rs-git-no-such-dir-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    assert!(!missing.exists());
    assert!(!is_inside_work_tree(&missing));
    assert!(!is_git_lfs_working_copy(&missing));
    assert!(remote_fetch_uri(&missing, "origin").is_none());
    assert!(commit_range_status(&missing, "HEAD").is_err());
}

#[test]
fn upstream_hops_replace_existing_keys() {
    let mut path = UpstreamPath::new();
    path.add_upstream("a".to_string(), UpstreamHop::local("b".to_string(), false));
    path.add_upstream("a".to_string(), UpstreamHop::local("c".to_string(), false));
    assert_eq!(path.get_length(), 1);
    assert_eq!(path.get_upstream("a").map(|h| h.name.as_str()), Some("c"));
}

#[test]
fn blob_without_trailing_newline_ends_headers() {
    let raw = RawCommit::new_from_raw_blob("tree abcdef").unwrap();
    assert_eq!(raw.get_tree_hash(), Some("abcdef"));
    assert_eq!(raw.get_message(), Some(""));
}

#[test]
fn raw_diff_skips_renames_and_copies_without_tabs() {
    let old = "a".repeat(40);
    let new = "b".repeat(40);
    let status = format!(
        ":100644 100644 {old} {new} R100 oldonly\n\
         :100644 100644 {old} {new} C100 copyonly\n\
         :100644 100644 {old} {new} M\tfile.txt\n"
    );
    let parsed = parse_git_raw_diff(&status);
    assert_eq!(parsed.len(), 1);
    assert!(parsed.contains_key("file.txt"));
}

#[test]
fn porcelain_v2_skips_headless_entries() {
    assert!(parse_porcelain_v2_status(b"?\0").is_empty());
}

#[test]
fn tree_lines_reject_extra_fields() {
    assert!(parse_git_tree_line("100644 blob abcdef extra\tname").is_none());
    let hash = "a".repeat(40);
    let parsed = parse_git_tree_line(&format!("100644 blob {hash}\tname")).unwrap();
    assert_eq!(parsed.0, "name");
}

#[test]
fn blame_caches_authors_per_revision() {
    let sha = "C".repeat(40);
    let stdout = format!(
        "{sha} 1 1 1\nauthor Same\nfilename f.txt\n\tfirst\n\
         {sha} 2 2 1\nauthor Same\nfilename f.txt\n\tsecond\n"
    );
    let blame = parse_blame(&stdout).unwrap();
    assert_eq!(
        blame,
        vec![
            BlameLine {
                author: "Same".to_string(),
                revision: sha.clone()
            },
            BlameLine {
                author: "Same".to_string(),
                revision: sha
            },
        ]
    );
}

#[test]
fn remote_uri_follows_the_tracking_remote() {
    let repo = fresh_repo("remote");
    commit_one(&repo, "a.txt", "one\n");
    let bare = plain_dir("cov-remote-bare");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&bare)
        .args(["init", "-q", "--bare"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let bare_s = bare.to_string_lossy().into_owned();
    for args in [
        vec!["remote", "add", "origin", bare_s.as_str()],
        vec!["push", "-q", "-u", "origin", "master"],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }
    let uri = remote_uri(&repo).unwrap();
    assert!(uri.is_some_and(|uri| uri.contains("cov-remote-bare")));
    std::fs::remove_dir_all(&repo).ok();
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn author_fails_on_broken_repo_config() {
    let repo = fresh_repo("author");
    std::fs::write(repo.join(".git/config"), "[[[\n").unwrap();
    assert_eq!(
        author(&repo).unwrap_err(),
        "git var GIT_AUTHOR_IDENT failed"
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn status_failures_outside_repos_report() {
    let plain = plain_dir("cov-plain");
    assert_eq!(
        uncommitted_status(&plain).unwrap_err(),
        "git status --porcelain=2 failed"
    );
    assert_eq!(
        changed_files(&plain, "HEAD").unwrap_err(),
        "git diff --raw failed"
    );
    assert_eq!(all_files(&plain).unwrap_err(), "git ls-files failed");
    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn missing_blob_objects_fail_file_reads() {
    let repo = fresh_repo("blob");
    commit_one(&repo, "a.txt", "one\n");
    let sha = stdout_text(&repo, &["rev-parse", "HEAD:a.txt"]).unwrap();
    let object = repo.join(".git/objects").join(&sha[..2]).join(&sha[2..]);
    std::fs::remove_file(&object).unwrap();
    assert_eq!(
        file_data_at_revision(&repo, "a.txt", "HEAD").unwrap_err(),
        "git cat-file blob failed"
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn corrupt_objects_fail_branch_listing() {
    // `symbolic-ref` only reads `.git/HEAD`, so the branch resolves
    // while `for-each-ref` fails dereferencing the corrupt commit.
    let repo = fresh_repo("branches");
    commit_one(&repo, "a.txt", "one\n");
    assert_eq!(branch_name(&repo).unwrap().as_deref(), Some("master"));
    let sha = stdout_text(&repo, &["rev-parse", "HEAD"]).unwrap();
    let object = repo.join(".git/objects").join(&sha[..2]).join(&sha[2..]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = std::fs::metadata(&object).unwrap().permissions();
        perms.set_readonly(false);
        std::fs::set_permissions(&object, perms).unwrap();
    }
    std::fs::write(&object, b"garbage-garbage-garbage").unwrap();
    assert_eq!(all_branches(&repo).unwrap_err(), "git for-each-ref failed");
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn raw_commit_writes_tolerate_early_git_exit() {
    // Outside a repository git fatals before reading stdin; the
    // megabyte blob cannot fit the pipe buffer, so the write
    // deterministically hits EPIPE and the status check reports.
    let plain = plain_dir("cov-raw");
    let mut commit = RawCommit::default();
    commit.set_message(Some("x".repeat(1 << 20)));
    assert_eq!(
        write_raw_commit(&plain, &commit).unwrap_err(),
        "git hash-object failed"
    );
    std::fs::remove_dir_all(&plain).ok();
}

// -( T1170 coverage wave 6g band G, batch E )----------------------------------

/// Unique scratch git repo with one empty commit (the shared
/// `fresh_repo` keys on pid+name only, which a second caller cannot
/// safely reuse).
fn cov6g_repo(tag: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("arc-rs-git-cov6g-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    };
    run(&["init", "-q", "-b", "master"]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    run(&["commit", "-q", "--allow-empty", "-m", "init"]);
    dir
}

#[test]
fn cov6g_raw_diff_copy_flag_records_new_path() {
    // `C` with a tab-separated pair records only the new path, then
    // takes the copy arm's trailing `continue`.
    let status = ":100644 100644 aaaaaaa bbbbbbb C75\told.cs\tnew.cs\n";
    let full = parse_git_raw_diff_full(status);
    assert_eq!(full.len(), 1);
    let entry = &full["new.cs"];
    assert_eq!(entry.mask, FLAG_ADDED);
    assert_eq!(entry.r#ref, "bbbbbbb");
    assert_eq!(parse_git_raw_diff(status).get("new.cs"), Some(&FLAG_ADDED));
}

#[test]
fn cov6g_porcelain_v2_truncated_rename_is_skipped() {
    // No trailing NUL: the final `2` record has no orig-path entry, so
    // it is skipped instead of recorded.
    let bytes = b"? new.txt\x002 R. N... 100644 100644 100644 abcabc abcabc extra new.txt";
    let got = parse_porcelain_v2_status(bytes);
    assert_eq!(got.len(), 1);
    assert_eq!(got["new.txt"], FLAG_UNTRACKED);
    // Control: the same `2` record with its orig-path entry present is
    // recorded, proving the record above is well-formed and can only
    // have been skipped at the truncated-record arm.
    let full = b"2 R. N... 100644 100644 100644 abcabc abcabc extra renamed.txt\x00orig.txt\x00";
    let recorded = parse_porcelain_v2_status(full);
    assert_ne!(recorded["renamed.txt"] & FLAG_ADDED, 0);
    assert_ne!(recorded["orig.txt"] & FLAG_DELETED, 0);
}

#[test]
fn cov6g_all_branches_tolerates_spawn_race() {
    // `all_branches` fails its `output(...)?` only when the
    // for-each-ref spawn fails after `branch_name` succeeded: a TOCTOU
    // only a rename race can reach. Outcomes vary by timing, so
    // nothing is asserted about them; the test passes deterministically
    // while giving the race window every chance to hit.
    let repo = cov6g_repo("allbr");
    assert_eq!(branch_name(&repo).unwrap(), Some("master".to_string()));
    let away = repo.with_extension("cov6g-away");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let toggler = std::thread::spawn({
        let (repo, away, stop) = (repo.clone(), away.clone(), stop.clone());
        move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::fs::rename(&repo, &away).ok();
                std::fs::rename(&away, &repo).ok();
            }
        }
    });
    for _ in 0..200 {
        let _ = all_branches(&repo);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    toggler.join().unwrap();
    // The toggler only exits at the loop guard, i.e. after a complete
    // swap-swap iteration (or none), and renames inside a writable
    // temp dir cannot fail on unix, so `repo` unconditionally exists
    // after `join`: the old swap-restore arm was dead. This assert
    // enforces that invariant on every run.
    assert!(repo.exists(), "swap toggler restores the repo before exit");
    std::fs::remove_dir_all(&repo).ok();
    std::fs::remove_dir_all(&away).ok();
}
// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
fn repo_helpers_clear_stale_dirs() {
    // Reusing a tag clears the previous run's leftovers.
    let first = fresh_repo("stale-reuse");
    std::fs::write(first.join("junk.txt"), "junk").unwrap();
    let second = fresh_repo("stale-reuse");
    assert!(!second.join("junk.txt").exists());
    let plain_first = plain_dir("stale-plain");
    std::fs::write(plain_first.join("junk.txt"), "junk").unwrap();
    let plain_second = plain_dir("stale-plain");
    assert!(!plain_second.join("junk.txt").exists());
    std::fs::remove_dir_all(&second).ok();
    std::fs::remove_dir_all(&plain_second).ok();
}

/// Ancient-git fallback chain: a fake `git` reporting 1.7.4 without
/// `remote get-url` (added in 2.7) or working `ls-remote --get-url`
/// resolves the URI through `config remote.<name>.url`, staging the
/// config fallback guard. Unix-only (shell script).
#[cfg(unix)]
#[test]
fn ancient_git_config_fallback_resolves_remote_uri() {
    let tmp = TempDir::new("ancient-git");
    let fake = tmp.path().join("git");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo \"git version 1.7.4\"; exit 0; fi\n\
         if [ \"$1\" = \"remote\" ]; then exit 1; fi\n\
         if [ \"$1\" = \"ls-remote\" ]; then exit 1; fi\n\
         if [ \"$1\" = \"config\" ]; then echo \"https://example.test/repo.git\"; exit 0; fi\n\
         exit 1\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).unwrap();
    }
    assert_eq!(
        load_git_remote_uri_with(tmp.path(), "origin", false, Some(&fake)).as_deref(),
        Some("https://example.test/repo.git\n")
    );
    assert_eq!(
        load_git_remote_uri_with(tmp.path(), "origin", true, Some(&fake)).as_deref(),
        Some("https://example.test/repo.git\n")
    );
}
// -( T1176 PB-5: legacy-toolchain fallback coverage )----------------------------

/// `ls-remote` fallback body: a fake `git` reporting 1.9 without
/// `remote get-url` (added in 2.7) resolves the URI through
/// `ls-remote --get-url`, staging the git < 2.7 fallback.
/// Unix-only (shell script).
#[cfg(unix)]
#[test]
fn ancient_git_ls_remote_fallback_resolves_remote_uri() {
    let tmp = TempDir::new("ancient-lsremote");
    let fake = tmp.path().join("git");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo \"git version 1.9.5\"; exit 0; fi\n\
         if [ \"$1\" = \"remote\" ]; then exit 1; fi\n\
         if [ \"$1\" = \"ls-remote\" ]; then echo \"https://example.test/ls.git\"; exit 0; fi\n\
         exit 1\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).unwrap();
    }
    // Unlike the config fallback, the ls-remote arm returns the
    // normalized URI (no trailing newline).
    assert_eq!(
        load_git_remote_uri_with(tmp.path(), "origin", false, Some(&fake)).as_deref(),
        Some("https://example.test/ls.git")
    );
    assert_eq!(
        load_git_remote_uri_with(tmp.path(), "origin", true, Some(&fake)).as_deref(),
        Some("https://example.test/ls.git")
    );
}

/// git 1.7.5-2.7 early-out: a fake `git` reporting 2.0 whose
/// `remote get-url` and `ls-remote --get-url` both miss returns
/// `None` without reaching the `config` fallback. The `config` arm
/// serves a sentinel URI, so `None` proves the early-out.
/// Unix-only (shell script).
#[cfg(unix)]
#[test]
fn git_2_0_skips_config_fallback_after_ls_remote_miss() {
    let tmp = TempDir::new("git20-earlyout");
    let fake = tmp.path().join("git");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo \"git version 2.0.0\"; exit 0; fi\n\
         if [ \"$1\" = \"remote\" ]; then exit 1; fi\n\
         if [ \"$1\" = \"ls-remote\" ]; then exit 1; fi\n\
         if [ \"$1\" = \"config\" ]; then echo \"https://must.not/be-reached.invalid\"; exit 0; fi\n\
         exit 1\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).unwrap();
    }
    assert!(version_at_least("2.0.0", "1.7.5"));
    assert_eq!(
        load_git_remote_uri_with(tmp.path(), "origin", false, Some(&fake)),
        None
    );
    assert_eq!(
        load_git_remote_uri_with(tmp.path(), "origin", true, Some(&fake)),
        None
    );
}

// -( T1183: shared ETXTBSY spawn retry )-------------------------------------------

/// The `git` seam rides through transient ETXTBSY via the shared
/// [`crate::proc_env::spawn_output`] (Linux-only: only Linux reports
/// ETXTBSY for a held write handle — see the `hg` retry tests).
#[cfg(target_os = "linux")]
#[test]
fn output_with_git_retries_txtbsy_to_success() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("git-etxtbsy-retry");
    let fake = tmp.path().join("git");
    std::fs::write(&fake, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let held = std::fs::OpenOptions::new().write(true).open(&fake).unwrap();
    // Probe: Linux must report ETXTBSY while the write handle is held;
    // fail loud instead of passing without covering the retry arm.
    let err = std::process::Command::new(&fake)
        .output()
        .expect_err("expected ETXTBSY probe failure");
    assert!(
        crate::proc_env::is_text_file_busy(&err),
        "expected ETXTBSY, got {err:?}"
    );
    // Release after 50ms; the retry loop must ride through to success.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(held);
        });
        let out = output_with_git(tmp.path(), &["--version"], Some(&fake))
            .expect("retry must succeed after the write handle releases");
        assert!(out.status.success());
    });
}

// -( T1176 PB-5: stdin-write classifier )---------------------------------------

#[test]
fn stdin_write_tolerates_ok_and_epipe() {
    // White-box: the `hash-object --stdin` classifier accepts a clean
    // write and EPIPE (git exited early); both covered arms.
    tolerate_stdin_write(Ok(()));
    tolerate_stdin_write(Err(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "reader closed",
    )));
}

#[test]
#[should_panic(expected = "fail only as EPIPE")]
fn stdin_write_non_epipe_is_unreachable() {
    // White-box proof: `ChildStdin` is a blocking pipe to a
    // live-or-exited child, so only EPIPE can occur; anything else
    // violates the guarantee and panics.
    tolerate_stdin_write(Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "synthetic",
    )));
}
