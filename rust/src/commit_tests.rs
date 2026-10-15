use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn empty_options() -> Options {
    Options {
        messages: Vec::new(),
        all: false,
        allow_empty: false,
        show: false,
        revision: None,
        conduit_uri: None,
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_options(&args(&[])), Ok(Some(empty_options())));
}

#[test]
fn parses_message_forms() {
    let mut expected = empty_options();
    expected.messages = vec!["one".to_string()];
    assert_eq!(
        parse_options(&args(&["-m", "one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options(&args(&["--message", "one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options(&args(&["--message=one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(parse_options(&args(&["-mone"])), Ok(Some(expected)));
}

#[test]
fn messages_are_repeatable() {
    let options = parse_options(&args(&["-m", "one", "-m", "two"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.messages, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn parses_flags() {
    let options = parse_options(&args(&["--all", "--allow-empty", "--show"]))
        .unwrap()
        .unwrap();
    assert!(options.all && options.allow_empty && options.show);
    let options = parse_options(&args(&["-a"])).unwrap().unwrap();
    assert!(options.all);
}

#[test]
fn parses_revision_and_conduit_uri() {
    let options = parse_options(&args(&["--revision", "D123"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.revision, Some("D123".to_string()));
    let options = parse_options(&args(&["--revision=123"])).unwrap().unwrap();
    assert_eq!(options.revision, Some("123".to_string()));
    let options = parse_options(&args(&["--conduit-uri", "http://x/"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
    let options = parse_options(&args(&["--conduit-uri=http://x/"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
}

#[test]
fn help_returns_none() {
    assert_eq!(parse_options(&args(&["--help"])), Ok(None));
    assert_eq!(parse_options(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options(&args(&["file.txt"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(parse_options(&args(&["-m"])).is_err());
    assert!(parse_options(&args(&["--revision"])).is_err());
    assert!(parse_options(&args(&["--revision="])).is_err());
    assert!(parse_options(&args(&["--conduit-uri"])).is_err());
    assert!(parse_options(&args(&["--message="])).is_err());
}

#[test]
fn parses_revision_ids() {
    assert_eq!(parse_revision_id("123"), Ok(123));
    assert_eq!(parse_revision_id("D123"), Ok(123));
    assert_eq!(parse_revision_id("d7"), Ok(7));
    for bad in ["", "D", "D0", "0", "abc", "D12x", "12.5", "-3", "D 1"] {
        assert!(parse_revision_id(bad).is_err(), "accepted: {bad:?}");
    }
}

#[test]
fn normalizes_messages() {
    assert_eq!(normalize_message("hello"), Ok("hello\n".to_string()));
    assert_eq!(
        normalize_message("  hello  \n\n"),
        Ok("hello\n".to_string())
    );
    assert!(normalize_message("").is_err());
    assert!(normalize_message("   \n\t\n").is_err());
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("usage: arc commit"));
    assert!(text.contains("--revision") && text.contains("--show"));
    assert!(text.contains("--conduit-uri"));
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-commit-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn rejects_git_only_options_under_svn() {
    let mut options = empty_options();
    options.messages = vec!["x".to_string()];
    assert!(reject_git_only_options(&options).is_err());
    let mut options = empty_options();
    options.all = true;
    assert!(reject_git_only_options(&options).is_err());
    let mut options = empty_options();
    options.allow_empty = true;
    assert!(reject_git_only_options(&options).is_err());
    assert!(reject_git_only_options(&empty_options()).is_ok());
}

#[test]
fn parent_dirs_stop_at_top() {
    assert_eq!(parent_dir("a/b/c.txt"), "a/b");
    assert_eq!(parent_dir("a/b"), "a");
    assert_eq!(parent_dir("a"), ".");
    assert_eq!(parent_dir("."), ".");
    assert_eq!(parent_dir("a/b/"), "a");
}

#[test]
fn expands_commit_dirs() {
    let commit = expand_commit_dirs(&["a/b/c.txt".to_string(), "top.txt".to_string()]);
    let sorted: Vec<&str> = commit.iter().map(String::as_str).collect();
    assert_eq!(sorted, vec!["a", "a/b", "a/b/c.txt", "top.txt"]);
    assert!(expand_commit_dirs(&[]).is_empty());
}

#[test]
fn matches_source_paths() {
    let dir = scratch("source");
    let raw = dir.to_string_lossy().into_owned();
    assert!(root_matches_source(&dir, &raw));
    assert!(!root_matches_source(&dir, "/elsewhere/entirely"));
    assert!(!root_matches_source(&dir, ""));
    let canonical = std::fs::canonicalize(&dir)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    assert!(root_matches_source(&dir, canonical));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn descendant_checks_need_dirs_on_disk() {
    let dir = scratch("descendant");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), "x").unwrap();
    std::fs::write(dir.join("file.txt"), "x").unwrap();
    assert!(is_path_descendant(&dir, "sub/inner.txt", "sub"));
    assert!(!is_path_descendant(&dir, "sub", "sub"));
    assert!(!is_path_descendant(&dir, "file.txt", "sub"));
    assert!(!is_path_descendant(&dir, "sub/inner.txt", "file.txt"));
    assert!(!is_path_descendant(&dir, "sub/missing.txt", "sub"));
    assert!(!is_path_descendant(&dir, "nope/x.txt", "nope"));
    assert!(is_path_descendant(&dir, "sub/inner.txt", "."));
    assert!(!is_path_descendant(&dir, ".", "."));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn partitions_uncommitted_status() {
    let dir = scratch("partition");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), "x").unwrap();
    std::fs::write(dir.join("other.txt"), "x").unwrap();
    let mut status = svn::StatusMap::new();
    status.insert("sub/inner.txt".to_string(), svn::FLAG_MODIFIED);
    status.insert("other.txt".to_string(), svn::FLAG_MODIFIED);
    status.insert("sub/keep.txt".to_string(), svn::FLAG_MODIFIED);

    // Everything committed: nothing skipped.
    let mut commit = expand_commit_dirs(&[
        "sub/inner.txt".to_string(),
        "other.txt".to_string(),
        "sub/keep.txt".to_string(),
    ]);
    assert!(
        partition_uncommitted(&dir, &status, &commit)
            .unwrap()
            .is_empty()
    );

    // One path outside the set: skipped, and the set keeps its dirs.
    commit.remove("other.txt");
    let skipped = partition_uncommitted(&dir, &status, &commit).unwrap();
    assert_eq!(skipped, vec!["other.txt".to_string()]);
    assert!(commit.contains("sub"));

    // A modified path under a committed directory throws.
    std::fs::write(dir.join("sub").join("keep.txt"), "x").unwrap();
    commit.remove("sub/keep.txt");
    let err = partition_uncommitted(&dir, &status, &commit).unwrap_err();
    assert!(
        err.contains("NOT included in the commit"),
        "unexpected: {err}"
    );
    assert!(err.contains("'sub/keep.txt'"), "unexpected: {err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn drops_missing_paths_but_keeps_deletions() {
    let dir = scratch("missing");
    std::fs::write(dir.join("kept.txt"), "x").unwrap();
    let mut status = svn::StatusMap::new();
    status.insert("gone.txt".to_string(), svn::FLAG_DELETED);
    let mut commit = BTreeSet::from([
        "kept.txt".to_string(),
        "gone.txt".to_string(),
        "vanished.txt".to_string(),
    ]);
    let missing = remove_missing_paths(&dir, &status, &mut commit);
    assert_eq!(missing, vec!["vanished.txt".to_string()]);
    assert!(commit.contains("kept.txt"));
    assert!(commit.contains("gone.txt"));
    assert!(!commit.contains("vanished.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn file_warnings_confirm_or_abort() {
    let mut io = land::Io::canned(&["y"]);
    prompt_file_warning(&mut io, "1 path(s):", "Proceed?", &["a.txt".to_string()]).unwrap();
    assert!(io.out.stdout.contains("    a.txt\n"), "{}", io.out.stdout);

    let mut io = land::Io::canned(&["n"]);
    let err =
        prompt_file_warning(&mut io, "1 path(s):", "Proceed?", &["a.txt".to_string()]).unwrap_err();
    assert_eq!(err, land::ABORT);

    let mut io = land::Io::canned(&["y", "n"]);
    let err = confirm_all(&mut io, &["One?".to_string(), "Two?".to_string()]).unwrap_err();
    assert_eq!(err, land::ABORT);
    let mut io = land::Io::canned(&["y", "y"]);
    confirm_all(&mut io, &["One?".to_string(), "Two?".to_string()]).unwrap();
}

#[test]
fn message_files_are_unique_under_concurrency() {
    // Regression: pid+nanos names collided when parallel tests wrote
    // within one clock tick (macOS vcs1_commit flake). A barrier forces
    // all threads to write simultaneously.
    const N: usize = 32;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let message = format!("message {i}");
                let path = write_message_file("test-race", &message).unwrap();
                (path, message)
            })
        })
        .collect();
    let mut paths = Vec::with_capacity(N);
    for handle in handles {
        let (path, message) = handle.join().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), message);
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), N);
    for path in &paths {
        std::fs::remove_file(path).ok();
    }
}
