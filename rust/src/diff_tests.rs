use super::*;
use crate::diff_local;

/* G1 full-state tests: editor, scratch files, browser, worktree checks. */

#[test]
fn g1_editor_helpers() {
    assert_eq!(sanitize_editor_name("a/b\\c:d e"), "abcde");
    assert_eq!(sanitize_editor_name("A-9._x"), "A-9._x");
    assert_eq!(editor_file_name("ok-name"), "ok-name");
    assert_eq!(editor_file_name("///"), "untitled");
    assert_eq!(editor_file_name(""), "untitled");
    assert_eq!(
        launching_editor_notice("/usr/bin/vim"),
        "Launching editor \"vim\"..."
    );
    assert_eq!(
        launching_editor_notice("C:\\Tools\\ed.exe"),
        "Launching editor \"ed.exe\"..."
    );

    let none = |_: &str| false;
    assert_eq!(
        resolve_editor(Some("p"), Some("e"), Some("f"), &none).unwrap(),
        "p"
    );
    assert_eq!(
        resolve_editor(None, Some("e"), Some("f"), &none).unwrap(),
        "e"
    );
    assert_eq!(
        resolve_editor(Some(""), Some(""), Some("f"), &none).unwrap(),
        "f"
    );
    let has_vi = |name: &str| name == "vi";
    assert_eq!(resolve_editor(None, None, None, &has_vi).unwrap(), "vi");
    assert_eq!(
        resolve_editor(None, None, None, &none).unwrap_err(),
        NO_EDITOR_MESSAGE
    );

    assert!(binary_exists("git"));
    assert!(!binary_exists("definitely-not-a-binary-g1"));
    let dir = scratch_dir("g1-binary");
    let script = dir.join("tool.sh");
    std::fs::write(&script, "x").unwrap();
    assert!(binary_exists(script.to_string_lossy().as_ref()));
    assert!(!binary_exists(
        dir.join("missing.sh").to_string_lossy().as_ref()
    ));

    #[cfg(windows)]
    assert_eq!(shell_quote("a b"), "\"a b\"");
    #[cfg(not(windows))]
    assert_eq!(shell_quote("a'b"), "'a'\\''b'");

    assert_eq!(editor_offset_flag("vim", 0), "");
    #[cfg(windows)]
    assert_eq!(editor_offset_flag("vim", 3), "");
    #[cfg(not(windows))]
    {
        assert_eq!(editor_offset_flag("vim", 3), "+3");
        assert_eq!(editor_offset_flag("mate", 3), "-l 3");
    }

    assert!(forgive_editor_exit("vi", true));
    assert!(forgive_editor_exit("vim", true));
    assert!(!forgive_editor_exit("vi", false));
    assert!(!forgive_editor_exit("nano", true));
    assert_eq!(
        editor_exit_error(3),
        "Editor exited with an error code (#3)."
    );

    let temp = unique_temp_subdir("g1-edit.").unwrap();
    assert!(temp.is_dir());
    std::fs::remove_dir_all(&temp).ok();
}

#[test]
fn g1_edit_interactively_roundtrip() {
    // No-op editors return the template unchanged.
    let noop = if cfg!(windows) { "rem" } else { "true" };
    let out = edit_interactively("template\n", "msg", None, Some(noop), 0).unwrap();
    assert_eq!(out, "template\n");

    // Editors can rewrite the document through the shell.
    #[cfg(windows)]
    let append = "echo MARKED>>";
    #[cfg(not(windows))]
    let append = "printf MARKED >>";
    let out = edit_interactively("base\n", "msg", Some("Task line."), Some(append), 0).unwrap();
    assert!(out.contains("MARKED"), "{out:?}");

    // Missing binaries fail with the editor error.
    assert!(edit_interactively("x", "msg", None, Some("definitely-not-an-editor-g1"), 0).is_err());
}

#[test]
fn gap2_readable_scratch_file_path() {
    let root = scratch_dir("gap2-readable");
    let meta = root.join(".git");
    let expected = Path::new(".git")
        .join("arc")
        .join("create-message")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        readable_scratch_file_path(&meta, &root, "create-message"),
        expected
    );
    // Dotted roots resolve lexically on the second attempt.
    assert_eq!(
        readable_scratch_file_path(&meta, &root.join("sub").join(".."), "create-message"),
        expected
    );
    // Roots that do not contain the file keep the absolute path.
    let elsewhere = scratch_dir("gap2-readable-elsewhere");
    assert_eq!(
        readable_scratch_file_path(&meta, &elsewhere, "create-message"),
        scratch_file_path(&meta, "create-message")
            .to_string_lossy()
            .into_owned()
    );
}

#[test]
fn g1_scratch_files() {
    let meta = scratch_dir("g1-scratch");
    assert_eq!(
        scratch_file_path(&meta, "create-message"),
        meta.join("arc").join("create-message")
    );
    assert_eq!(read_scratch_file(&meta, "missing"), None);
    assert!(write_scratch_file(&meta, "create-message", "hello"));
    assert_eq!(
        read_scratch_file(&meta, "create-message").as_deref(),
        Some("hello")
    );
    assert!(remove_scratch_file(&meta, "create-message"));
    assert_eq!(read_scratch_file(&meta, "create-message"), None);
    assert!(!remove_scratch_file(&meta, "create-message"));

    assert_eq!(read_scratch_json(&meta, "absent"), serde_json::json!({}));
    assert!(write_scratch_json(
        &meta,
        "data",
        &serde_json::json!({"a": 1})
    ));
    assert_eq!(
        read_scratch_json(&meta, "data"),
        serde_json::json!({"a": 1})
    );
    assert!(write_scratch_file(&meta, "broken", "not json"));
    assert_eq!(read_scratch_json(&meta, "broken"), serde_json::json!({}));

    let git = git_dir("g1-meta");
    let resolved = git_metadata_dir(&git).unwrap();
    assert!(resolved.ends_with(".git"), "{resolved:?}");
    assert!(git_metadata_dir(Path::new("definitely-not-a-repo-g1")).is_err());
}

#[test]
fn g1_browser_helpers() {
    let none = |_: &str| false;
    let configured = ["my-browser".to_string(), "--new".to_string()];
    assert_eq!(
        browser_open_argv("http://x/", Some(&configured), &none).unwrap(),
        vec![
            "my-browser".to_string(),
            "--new".to_string(),
            "http://x/".to_string()
        ]
    );
    // Windows always resolves through `cmd /c start`.
    #[cfg(windows)]
    assert_eq!(
        browser_open_argv("http://x/", None, &none).unwrap(),
        vec![
            "cmd".to_string(),
            "/c".to_string(),
            "start".to_string(),
            "http://x/".to_string()
        ]
    );
    // Elsewhere an unresolvable browser surfaces the PHP message.
    #[cfg(not(windows))]
    assert_eq!(
        browser_open_argv("http://x/", None, &none).unwrap_err(),
        NO_BROWSER_MESSAGE
    );
    #[cfg(not(windows))]
    {
        let has_xdg = |name: &str| name == "xdg-open";
        assert_eq!(
            browser_open_argv("http://x/", None, &has_xdg).unwrap(),
            vec!["xdg-open".to_string(), "http://x/".to_string()]
        );
    }
    let err = browser_open_error("http://x/", &["b".to_string()]);
    assert!(
        err.contains("Failed to open URI \"http://x/\" in browser (\"b\")"),
        "{err}"
    );

    assert!(
        open_uri_in_browser("http://x/", &["definitely-not-a-browser-g1".to_string()]).is_err()
    );
}

#[test]
fn gap2_open_submit_result_in_browser_matrix() {
    use std::cell::RefCell;
    let none = |_: &str| false;
    // No URI or no --browse: the runner never runs.
    let seen: RefCell<Vec<(String, Vec<String>)>> = RefCell::new(Vec::new());
    let recording = |uri: &str, browser: &[String]| {
        seen.borrow_mut().push((uri.to_string(), browser.to_vec()));
        Ok(())
    };
    open_submit_result_in_browser(None, true, None, &none, &recording).unwrap();
    open_submit_result_in_browser(Some("http://x/"), false, None, &none, &recording).unwrap();
    assert!(seen.borrow().is_empty());
    // --browse with a URI opens through the resolved browser argv.
    let configured = ["my-browser".to_string(), "--new".to_string()];
    open_submit_result_in_browser(
        Some("http://x/D1"),
        true,
        Some(&configured),
        &none,
        &recording,
    )
    .unwrap();
    assert_eq!(
        *seen.borrow(),
        vec![("http://x/D1".to_string(), configured.to_vec())]
    );
    // Runner failures propagate (production maps them to the PHP text).
    let failing = |_: &str, _: &[String]| Err("boom".to_string());
    assert_eq!(
        open_submit_result_in_browser(Some("http://x/"), true, Some(&configured), &none, &failing)
            .unwrap_err(),
        "boom"
    );
    // Unresolvable browser surfaces the PHP message (off Windows, where
    // `cmd /c start` always resolves).
    let ok_run = |_: &str, _: &[String]| Ok(());
    #[cfg(windows)]
    open_submit_result_in_browser(Some("http://x/"), true, None, &none, &ok_run).unwrap();
    #[cfg(not(windows))]
    assert_eq!(
        open_submit_result_in_browser(Some("http://x/"), true, None, &none, &ok_run).unwrap_err(),
        NO_BROWSER_MESSAGE
    );
    #[cfg(not(windows))]
    {
        let has_xdg = |name: &str| name == "xdg-open";
        open_submit_result_in_browser(Some("http://x/"), true, None, &has_xdg, &recording).unwrap();
        assert_eq!(
            seen.borrow().last().unwrap(),
            &("http://x/".to_string(), vec!["xdg-open".to_string()])
        );
    }
}

#[test]
fn gap2_configured_browser_command_shapes() {
    let dir = scratch_dir("gap2-browser-cfg");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcconfig"), r#"{"browser": "plain-browser"}"#).unwrap();
    assert_eq!(
        configured_browser_command_in(&dir, None),
        Some(vec!["plain-browser".to_string()])
    );
    std::fs::write(dir.join(".arcconfig"), r#"{"browser": ["a", "--b"]}"#).unwrap();
    assert_eq!(
        configured_browser_command_in(&dir, None),
        Some(vec!["a".to_string(), "--b".to_string()])
    );
    for body in [
        r#"{"browser": ""}"#,
        r#"{"browser": []}"#,
        r#"{"browser": 42}"#,
        r#"{}"#,
    ] {
        std::fs::write(dir.join(".arcconfig"), body).unwrap();
        assert_eq!(configured_browser_command_in(&dir, None), None, "{body}");
    }
    std::fs::write(dir.join(".arcconfig"), r#"{"browser": ["ok", 7]}"#).unwrap();
    assert_eq!(
        configured_browser_command_in(&dir, None),
        Some(vec!["ok".to_string()])
    );
}

#[test]
fn gap2_binary_exists_in_dirs_probes_files() {
    let dir = scratch_dir("gap2-bin-probe");
    let other = scratch_dir("gap2-bin-probe2");
    std::fs::write(dir.join("tool"), b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dir.join("tool")).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dir.join("tool"), perms).unwrap();
        std::fs::write(dir.join("plain"), b"x").unwrap();
        assert!(!binary_exists_in_dirs("plain", std::slice::from_ref(&dir)));
    }
    assert!(binary_exists_in_dirs("tool", &[dir.clone(), other.clone()]));
    assert!(binary_exists_in_dirs("tool", &[other.clone(), dir.clone()]));
    assert!(!binary_exists_in_dirs(
        "missing",
        std::slice::from_ref(&dir)
    ));
    assert!(!binary_exists_in_dirs("tool", std::slice::from_ref(&other)));
    assert!(!binary_exists_in_dirs("tool", &[]));
}

#[test]
fn g1_amend_helpers() {
    assert!(amend_worktree_message_git(Path::new("definitely-not-a-repo-g1"), "msg").is_err());
    assert!(!hg_supports_amend(Path::new("definitely-not-a-repo-g1")));
}

#[test]
fn g1_clean_worktree_texts() {
    assert_eq!(working_copy_desc("/w"), "  Working copy: /w\n\n");

    let err = clean_worktree_error(CleanWorktreeIssue::Incomplete, "/w", &["a".to_string()]);
    assert!(err.contains("incompletely checked out"), "{err}");
    assert!(
        err.contains("Incomplete directories in working copy:\n    a"),
        "{err}"
    );
    assert!(err.contains("svn update"), "{err}");
    let err = clean_worktree_error(CleanWorktreeIssue::Conflicts, "/w", &["b".to_string()]);
    assert!(err.contains("merge conflicts"), "{err}");
    assert!(err.contains("Conflicts in working copy:\n    b"), "{err}");
    let err = clean_worktree_error(CleanWorktreeIssue::Missing, "/w", &["c".to_string()]);
    assert!(err.contains("missing files"), "{err}");
    assert!(
        err.ends_with("Missing files in working copy:\n    c\n"),
        "{err}"
    );

    assert_eq!(untracked_hint("git"), ".git/info/exclude");
    assert_eq!(untracked_hint("svn"), "svn:ignore");
    assert_eq!(untracked_hint("hg"), ".hgignore");
    let block = untracked_block("/w", "git", &["u1".to_string(), "u2".to_string()]);
    assert!(
        block.contains("You have untracked files in this working copy.\n\n  Working copy: /w\n\n"),
        "{block}"
    );
    assert!(
        block.contains("(To ignore these 2 change(s), add them to \".git/info/exclude\".)"),
        "{block}"
    );
    assert!(block.contains("    u1\n    u2"), "{block}");
    assert_eq!(
        untracked_prompt(3),
        "Ignore these 3 untracked file(s) and continue?"
    );

    let block = uncommitted_block("/w", &["s".to_string()], &["c".to_string()]);
    assert!(
        block.contains("You have uncommitted changes in this working copy."),
        "{block}"
    );
    assert!(
        block.contains("Unstaged changes in working copy:\n    s"),
        "{block}"
    );
    assert!(
        block.contains("Uncommitted changes in working copy:\n    c"),
        "{block}"
    );
    assert!(block.ends_with('\n'));
    let staged_only = uncommitted_block("/w", &[], &["c".to_string()]);
    assert!(!staged_only.contains("Unstaged"), "{staged_only}");

    let template = commit_message_template(&["b".to_string(), "a".to_string(), "a".to_string()]);
    assert!(template.contains("# Enter a commit message."), "{template}");
    assert!(template.contains("#     a\n#     b\n"), "{template}");
    assert_eq!(
        TASK_COMMIT_MESSAGE,
        "Supply commit message for uncommitted changes, then save and exit."
    );
    assert_eq!(MUST_PROVIDE_MESSAGE, "You must provide a commit message.");
    assert_eq!(
        MUST_PROVIDE_NONEMPTY_MESSAGE,
        "You must provide a nonempty commit message."
    );
    assert!(CANNOT_CONTINUE_UNCOMMITTED.contains("You can not continue with uncommitted changes."));
    assert!(STASHING_NOTICE.contains("git stash pop"));

    assert_eq!(
        externals_message(2),
        "2 submodule(s) have uncommitted or untracked changes:"
    );
    assert_eq!(
        externals_prompt(2),
        "Ignore the changes to these 2 submodule(s) and continue?"
    );
}

/* G1 full-state tests: submit assembly and full inspection. */

#[test]
fn g1_collect_submit_changes_branches() {
    // Raw: plumbing works; `git --version` output parses to no changes.
    let mut options = defaults();
    options.raw_command = Some("git --version".to_string());
    let changes = collect_submit_changes(Path::new("."), &options, "", None).unwrap();
    assert!(changes.is_empty());

    // Raw without a source fails before touching any VCS.
    assert!(collect_submit_changes(Path::new("."), &defaults(), "", None).is_err());

    // Svn marker without a binary fails on spawn, deterministically.
    let svn = scratch_dir("g1-collect-svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert!(collect_submit_changes(&svn, &defaults(), "", None).is_err());

    // Hg marker without a binary fails on spawn, deterministically.
    let hg = scratch_dir("g1-collect-hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(collect_submit_changes(&hg, &defaults(), "tip", None).is_err());

    // Empty git worktree reports no changes.
    let git = git_dir("g1-collect-empty");
    let commit = std::process::Command::new("git")
        .current_dir(&git)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--allow-empty",
            "-qm",
            "x",
        ])
        .status()
        .unwrap();
    assert!(commit.success());
    let head = std::process::Command::new("git")
        .current_dir(&git)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(head.status.success());
    let base = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let err = collect_submit_changes(&git, &defaults(), &base, None).unwrap_err();
    assert_eq!(
        err,
        "No changes found. (Did you specify the wrong commit range?)"
    );
}

#[test]
fn gap2_prepare_submit_with_report_stamps_status_words() {
    let git = git_dir("gap2-lint-status");
    std::fs::write(git.join("f.txt"), "a\n").unwrap();
    let add = std::process::Command::new("git")
        .current_dir(&git)
        .args(["add", "f.txt"])
        .status()
        .unwrap();
    assert!(add.success());
    let commit = std::process::Command::new("git")
        .current_dir(&git)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "one",
        ])
        .status()
        .unwrap();
    assert!(commit.success());
    std::fs::write(git.join("f.txt"), "a\nb\n").unwrap();

    let options = defaults();
    let report = diff_local::LintUnitReport {
        lint_result: Some(1),
        unresolved_lint: Vec::new(),
        unit_result: Some(0),
        test_results: Vec::new(),
    };
    let input = prepare_submit_with_report(&git, &options, &SubmitMode::Only, &report).unwrap();
    assert_eq!(input.spec["lintStatus"], serde_json::json!("warn"));
    assert_eq!(input.spec["unitStatus"], serde_json::json!("okay"));
    // Legacy entry keeps the empty report (none/none).
    let input = prepare_submit(&git, &options, &SubmitMode::Only).unwrap();
    assert_eq!(input.spec["lintStatus"], serde_json::json!("none"));
    assert_eq!(input.spec["unitStatus"], serde_json::json!("none"));
}

#[test]
fn g1_full_spec_fields_properties() {
    // Raw spec: every repository field unset.
    let spec = full_spec_input(Path::new("."), None, "").unwrap();
    assert_eq!(spec.source_control_system, None);
    assert!(!spec.source_machine.is_empty());

    // Bogus dirs fail for VCS-bound specs.
    let bogus = Path::new("definitely-not-a-repo-g1");
    assert!(full_spec_input(bogus, Some(VcsKind::Git), "HEAD").is_err());
    assert!(full_spec_input(bogus, Some(VcsKind::Hg), "tip").is_err());
    assert!(full_spec_input(bogus, Some(VcsKind::Svn), "").is_err());

    // Fields: diff-only is null, plain updates are empty.
    assert_eq!(
        full_submit_fields(bogus, &defaults(), None, &SubmitMode::Only, "", None).unwrap(),
        serde_json::Value::Null
    );
    assert_eq!(
        full_submit_fields(bogus, &defaults(), None, &SubmitMode::Update(1), "", None).unwrap(),
        serde_json::json!({})
    );
    // Raw creates have no local messages.
    assert_eq!(
        full_submit_fields(bogus, &defaults(), None, &SubmitMode::Create, "", None).unwrap(),
        serde_json::json!({})
    );
    // Svn merges only the faux reviewers/cc message (pure: no binary needed).
    let options = Options {
        reviewers: Some("alice".to_string()),
        ..defaults()
    };
    let fields = full_submit_fields(
        bogus,
        &options,
        Some(VcsKind::Svn),
        &SubmitMode::Create,
        "",
        None,
    )
    .unwrap();
    assert_eq!(fields["reviewerPHIDs"], serde_json::json!(["alice"]));
    // Message files win on every VCS.
    let dir = scratch_dir("g1-fields-msg");
    std::fs::write(dir.join("msg.txt"), "Filed title\n").unwrap();
    let options = Options {
        message_file: Some(dir.join("msg.txt").to_string_lossy().to_string()),
        ..defaults()
    };
    let fields = full_submit_fields(
        bogus,
        &options,
        Some(VcsKind::Svn),
        &SubmitMode::Create,
        "",
        None,
    )
    .unwrap();
    assert_eq!(fields["title"], serde_json::json!("Filed title"));

    // Properties: raw and svn send nothing.
    assert!(
        full_submit_properties(bogus, None, "", None)
            .unwrap()
            .is_empty()
    );
    assert!(
        full_submit_properties(bogus, Some(VcsKind::Svn), "", None)
            .unwrap()
            .is_empty()
    );
    assert!(full_submit_properties(bogus, Some(VcsKind::Git), "HEAD", None).is_err());
    assert!(full_submit_properties(bogus, Some(VcsKind::Hg), "tip", None).is_err());
}

#[test]
fn g1_prepare_submit_full_errors() {
    let bogus = Path::new("definitely-not-a-repo-g1");
    let err = prepare_submit_full(bogus, &defaults(), &SubmitMode::Only).unwrap_err();
    assert_eq!(err, "not inside a git, hg, or svn working copy");

    // Raw with an empty diff fails the changes check before any network use.
    let mut options = defaults();
    options.raw_command = Some("git --version".to_string());
    let err = prepare_submit_full(bogus, &options, &SubmitMode::Only).unwrap_err();
    assert_eq!(err, "There are no changes to generate a diff from!");

    // Hg markers fail fast on base resolution without a binary.
    let hg = scratch_dir("g1-full-hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(prepare_submit_full(&hg, &defaults(), &SubmitMode::Only).is_err());

    // Fresh git repos fail base resolution before the draft guard runs
    // (the guard itself is covered against a committed fixture below).
    let git = git_dir("g1-full-draft");
    let options = Options {
        draft: true,
        ..defaults()
    };
    assert!(prepare_submit_full(&git, &options, &SubmitMode::Create).is_err());
}

#[test]
fn g1_prepare_full_branches() {
    // Raw sources skip detection.
    let mut options = defaults();
    options.raw = true;
    let (resolved, kind) = prepare_full(Path::new("."), &options).unwrap();
    assert!(resolved.raw);
    assert_eq!(kind, None);

    // Empty head normalizes; unknown dirs pass through.
    let options = Options {
        head: Some(String::new()),
        ..defaults()
    };
    let (resolved, kind) = prepare_full(&scratch_dir("g1-full-none"), &options).unwrap();
    assert_eq!(resolved.head, None);
    assert_eq!(kind, None);

    // Every VCS detects (markers need no binaries).
    assert_eq!(
        prepare_full(&git_dir("g1-full-git"), &defaults())
            .unwrap()
            .1,
        Some(VcsKind::Git)
    );
    let hg = scratch_dir("g1-full-hg2");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert_eq!(prepare_full(&hg, &defaults()).unwrap().1, Some(VcsKind::Hg));
    let svn = scratch_dir("g1-full-svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert_eq!(
        prepare_full(&svn, &defaults()).unwrap().1,
        Some(VcsKind::Svn)
    );

    // Supports checks mirror `prepare()`.
    let options = Options {
        head: Some("abc".to_string()),
        provided: vec!["head"],
        ..defaults()
    };
    let err = prepare_full(&hg, &options).unwrap_err();
    assert_eq!(
        err,
        "Option '--head' is not supported under hg. Mercurial does not support --head yet."
    );
    // Git derives the base commit from paths; svn keeps them as filters.
    let options = Options {
        paths: vec!["abc".to_string()],
        ..defaults()
    };
    let (resolved, _) = prepare_full(&git_dir("g1-full-commit"), &options).unwrap();
    assert_eq!(resolved.commit.as_deref(), Some("abc"));
    let (resolved, kind) = prepare_full(&svn, &options).unwrap();
    assert_eq!(kind, Some(VcsKind::Svn));
    assert_eq!(resolved.paths, vec!["abc".to_string()]);
    assert_eq!(resolved.commit, None);
}

#[test]
fn g1_inspect_full_branches() {
    // Raw read-only output is the command text.
    let mut options = defaults();
    options.raw_command = Some("git --version".to_string());
    let out = inspect_full(Path::new("."), &options).unwrap();
    assert!(String::from_utf8_lossy(&out).contains("git version"));

    // Unknown dirs fail without a VCS.
    let err = inspect_full(&scratch_dir("g1-inspect-none"), &defaults()).unwrap_err();
    assert_eq!(err, "not inside a git, hg, or svn working copy");

    // Submit modes fail before any network use when collection fails.
    let mut options = defaults();
    options.only = true;
    let err = inspect_full(Path::new("definitely-not-a-repo-g1"), &options).unwrap_err();
    assert_eq!(err, "not inside a git, hg, or svn working copy");

    // Svn/hg read-only paths fail fast without binaries.
    let svn = scratch_dir("g1-inspect-svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert!(inspect_full(&svn, &defaults()).is_err());
    let hg = scratch_dir("g1-inspect-hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(inspect_full(&hg, &defaults()).is_err());
}

/* G1 full-state tests: prompts, raw sources, change collection. */

#[test]
fn g1_full_help_and_wrap() {
    // T1153 R10: generated workflow help now (the stripped `FULL_HELP`
    // is gone); exact bytes are locked by
    // `r10_diff_help_matches_generated_format`.
    let full = help_text(false);
    assert!(full.contains("Supports: git, svn, hg"), "{full}");
    assert!(full.contains("merge base"), "{full}");
    assert!(full.contains("Under svn"), "{full}");

    assert_eq!(console_wrap("hi", 4, true), "    hi");
    assert_eq!(console_wrap("hi", 0, true), "hi");
    assert_eq!(console_wrap("a\nb", 2, true), "  a\n  b");
    assert_eq!(console_wrap("a\nb", 2, false), "a\n  b");
    assert!(console_wrap("", 4, true).is_empty());
    let long = "word ".repeat(20);
    let wrapped = console_wrap(long.trim_end(), 4, true);
    assert!(wrapped.starts_with("    word"), "{wrapped}");
    assert!(wrapped.contains('\n'), "{wrapped}");
    for line in wrapped.split('\n') {
        assert!(line.len() <= 78, "{line:?}");
    }
    // ANSI escapes do not count toward the width.
    let colored = format!("\u{1b}[1m{}\u{1b}[0m", "x".repeat(70));
    assert!(!console_wrap(&colored, 0, true).contains('\n'));
}

#[test]
fn merge_routes_hg_to_full_flows() {
    let dir = scratch_dir("route-hg");
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    let err = inspect(&dir, &defaults()).unwrap_err();
    assert!(
        !err.contains("not yet ported"),
        "still baseline-gated: {err}"
    );
}

#[test]
fn merge_routes_svn_to_full_flows() {
    let dir = scratch_dir("route-svn");
    std::fs::create_dir_all(dir.join(".svn")).unwrap();
    let err = inspect(&dir, &defaults()).unwrap_err();
    assert!(
        !err.contains("not yet ported"),
        "still baseline-gated: {err}"
    );
}

#[test]
fn merge_routes_raw_sources_without_working_copy() {
    let dir = scratch_dir("route-raw");
    let options = Options {
        raw_command: Some("echo hello-raw".to_string()),
        ..defaults()
    };
    let out = inspect(&dir, &options).unwrap();
    assert!(
        String::from_utf8(out).unwrap().contains("hello-raw"),
        "raw source did not execute"
    );
}

#[test]
fn merge_help_text_combines_usage_and_full_help() {
    // T1153 R10: `--help` renders the generated `DIFF WORKFLOW` reference;
    // the short `USAGE` stays for error dumps only.
    let text = help_text(false);
    assert!(text.starts_with("DIFF WORKFLOW\n"), "{text:?}");
    assert!(text.contains("Supports: git, svn, hg"), "{text:?}");
    assert!(text.contains("--verbatim"), "{text:?}");
}

#[test]
fn g1_confirm_prompt_helpers() {
    let prompt = format_confirm_prompt("Continue?", true);
    assert!(prompt.starts_with("\n\n    Continue? [y/N]"), "{prompt:?}");
    let prompt_yes = format_confirm_prompt("Continue?", false);
    assert!(prompt_yes.contains("[Y/n]"), "{prompt_yes:?}");

    assert_eq!(interpret_confirm_answer("y", true), Some(true));
    assert_eq!(interpret_confirm_answer("Y", true), Some(true));
    assert_eq!(interpret_confirm_answer("n", false), Some(false));
    assert_eq!(interpret_confirm_answer("", true), Some(false));
    assert_eq!(interpret_confirm_answer("", false), Some(true));
    assert_eq!(interpret_confirm_answer("  \n", true), Some(false));
    assert_eq!(interpret_confirm_answer("bogus", true), None);
    // PHP `trim()` strips " \t\n\r\0\x0B" only: vertical tab and NUL
    // trim, but Unicode whitespace (NBSP, EM SPACE) reprompts.
    assert_eq!(interpret_confirm_answer("\x0BY\x0B", true), Some(true));
    assert_eq!(interpret_confirm_answer("\0n\0", false), Some(false));
    assert_eq!(interpret_confirm_answer("\u{a0}y\u{a0}", true), None);
    assert_eq!(interpret_confirm_answer("\u{2003}n\u{2003}", false), None);
    assert_eq!(interpret_confirm_answer("\u{a0}", true), None);

    let mut out = Vec::new();
    let answer = prompt_confirm_from(
        &mut std::io::Cursor::new("bogus\ny\n"),
        &mut out,
        "Continue?",
        true,
    )
    .unwrap();
    assert!(answer);
    let rendered = String::from_utf8(out).unwrap();
    assert_eq!(rendered.matches("[y/N]").count(), 2, "{rendered:?}");
    assert!(rendered.ends_with('\n'));

    let mut out = Vec::new();
    let answer = prompt_confirm_from(&mut std::io::Cursor::new(""), &mut out, "P?", true).unwrap();
    assert!(!answer);
    let mut out = Vec::new();
    let answer = prompt_confirm_from(&mut std::io::Cursor::new(""), &mut out, "P?", false).unwrap();
    assert!(answer);
}

#[test]
#[cfg(unix)] // PHP has no TTY gate without posix_isatty (Windows).
fn g1_prompt_confirm_requires_tty() {
    // Interactive session: the TTY gate cannot fail deterministically
    // (it would block on input), so an interactive run checks nothing.
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let gated = (!interactive).then(|| prompt_confirm("Continue?", true).unwrap_err());
    assert_eq!(gated.as_deref(), (!interactive).then_some(STDIN_NOT_TTY));
}

#[test]
fn gap2_tty_gate_matches_php_platform_semantics() {
    // PHP `phutil_console_require_tty()`: gate exists only where
    // `posix_isatty` exists; elsewhere prompting reads stdin directly.
    #[cfg(unix)]
    {
        assert!(super::tty_gate_blocks_with(false));
        assert!(!super::tty_gate_blocks_with(true));
    }
    #[cfg(not(unix))]
    {
        assert!(!super::tty_gate_blocks_with(false));
        assert!(!super::tty_gate_blocks_with(true));
    }
}

#[test]
fn g1_raw_diff_source() {
    assert_eq!(
        raw_diff_source(&defaults()).unwrap_err(),
        "Unknown raw diff source."
    );
    let mut options = defaults();
    options.raw = true;
    assert_eq!(raw_diff_source(&options).unwrap(), RawSource::Stdin);
    let mut options = defaults();
    options.raw_command = Some("git diff".to_string());
    assert_eq!(
        raw_diff_source(&options).unwrap(),
        RawSource::Command("git diff".to_string())
    );

    assert!(read_raw_diff_text(&defaults()).is_err());
    let mut options = defaults();
    options.raw_command = Some("echo hello-raw".to_string());
    let text = read_raw_diff_text(&options).unwrap();
    assert!(text.contains("hello-raw"), "{text}");
}

#[test]
fn g1_hunk_and_change_conversion() {
    // Corpora are body-only (the `@@` header is consumed by the parser,
    // never stored); `,length` bits ride alongside (T1153 EXPORT-E11).
    let both_explicit = diff_local::HunkLenExplicit {
        old_explicit: true,
        new_explicit: true,
    };
    let hunk = diff_local::ParsedHunk {
        old_start: 1,
        old_len: 2,
        new_start: 1,
        new_len: 3,
        corpus: " a\n+b\n-c\n".to_string(),
        synthetic_counts: None,
    };
    let submit = submit_hunk_from_parsed(&hunk, &both_explicit);
    assert_eq!(submit.old_offset, 1);
    assert_eq!(submit.old_length, 2);
    assert_eq!(submit.new_length, 3);
    assert!(submit.old_len_explicit);
    assert!(submit.new_len_explicit);
    assert_eq!(submit.add_lines, 1);
    assert_eq!(submit.del_lines, 1);
    assert!(!submit.is_missing_old_newline);
    assert!(!submit.is_missing_new_newline);

    let neither_explicit = diff_local::HunkLenExplicit {
        old_explicit: false,
        new_explicit: false,
    };
    let missing = diff_local::ParsedHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        corpus: "-a\n\\ No newline at end of file\n+b\n".to_string(),
        synthetic_counts: None,
    };
    let submit = submit_hunk_from_parsed(&missing, &neither_explicit);
    assert!(!submit.old_len_explicit);
    assert!(!submit.new_len_explicit);
    assert!(submit.is_missing_old_newline);
    assert!(!submit.is_missing_new_newline);

    let change = diff_local::ParsedChange {
        old_path: "o.txt".to_string(),
        current_path: "n.txt".to_string(),
        change_type: diff_local::TYPE_MOVE_HERE,
        file_type: diff_local::FILE_TEXT,
        hunks: vec![hunk],
        hunk_len_explicit: vec![diff_local::HunkLenExplicit {
            old_explicit: true,
            new_explicit: true,
        }],
        old_mode: None,
        new_mode: None,
        away_paths: vec!["away.txt".to_string()],
        commit_hash: None,
        message: None,
    };
    let submit = submit_change_from_parsed(&change);
    assert_eq!(submit.old_path.as_deref(), Some("o.txt"));
    assert_eq!(submit.current_path, "n.txt");
    assert_eq!(submit.change_type, i64::from(diff_local::TYPE_MOVE_HERE));
    assert_eq!(submit.hunks.len(), 1);
    assert_eq!(submit.away_paths, vec!["away.txt".to_string()]);

    let entry = change_entry_from_parsed(&change);
    assert_eq!(entry.old_path.as_deref(), Some("o.txt"));
    assert_eq!(entry.away_paths, vec!["away.txt".to_string()]);
    assert_eq!(entry.current.added, 1);
    assert_eq!(entry.current.deleted, 1);

    let text = "commit abc\n\n    subject\n\ndiff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_raw_diff_changes(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "f.txt");
    assert_eq!(changes[0].hunks.len(), 1);
}

#[test]
fn old_path_submit_pipelines_report_dev_null_for_add() {
    // T1153 OLD-PATH: both submit pipelines report `/dev/null` for
    // ADD (`ArcanistDiffParser.php:801-808`), not empty or the
    // current path.
    let parsed = diff_local::ParsedChange {
        old_path: "/dev/null".to_string(),
        current_path: "n.txt".to_string(),
        change_type: diff_local::TYPE_ADD,
        file_type: diff_local::FILE_TEXT,
        hunks: Vec::new(),
        hunk_len_explicit: Vec::new(),
        old_mode: None,
        new_mode: None,
        away_paths: Vec::new(),
        commit_hash: None,
        message: None,
    };
    let submit = submit_change_from_parsed(&parsed);
    assert_eq!(submit.old_path.as_deref(), Some("/dev/null"));
    assert_eq!(submit.current_path, "n.txt");
    // Raw-diff pipeline parses the `--- /dev/null` line.
    let text = "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hi\n";
    let changes = parse_raw_diff_changes(text).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].old_path.as_deref(), Some("/dev/null"));
    assert_eq!(changes[0].current_path, "n.txt");
    // Entry pipeline (git range skeletons) maps ADD to `/dev/null`.
    let entry = diff_local::ChangeEntry {
        change_type: diff_local::TYPE_ADD,
        file_type: diff_local::FILE_TEXT,
        old_path: None,
        away_paths: Vec::new(),
        current: FileDiff {
            path: "e.txt".to_string(),
            added: 1,
            deleted: 0,
        },
    };
    let submit = submit_change(&entry);
    assert_eq!(submit.old_path.as_deref(), Some("/dev/null"));
    assert_eq!(submit.current_path, "e.txt");
    // DELETE keeps the path (PHP discards the `+++` side).
    let entry = diff_local::ChangeEntry {
        change_type: diff_local::TYPE_DELETE,
        file_type: diff_local::FILE_TEXT,
        old_path: None,
        away_paths: Vec::new(),
        current: FileDiff {
            path: "d.txt".to_string(),
            added: 0,
            deleted: 1,
        },
    };
    let submit = submit_change(&entry);
    assert_eq!(submit.old_path.as_deref(), Some("d.txt"));
}

#[test]
fn g1_vcs_kind_detection() {
    assert_eq!(VcsKind::Git.name(), "git");
    assert_eq!(VcsKind::Hg.name(), "hg");
    assert_eq!(VcsKind::Svn.name(), "svn");

    let git = git_dir("g1-kind");
    assert_eq!(detect_vcs_kind(&git), Some(VcsKind::Git));
    let hg = scratch_dir("g1-kind-hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert_eq!(detect_vcs_kind(&hg), Some(VcsKind::Hg));
    let svn = scratch_dir("g1-kind-svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert_eq!(detect_vcs_kind(&svn), Some(VcsKind::Svn));
    assert_eq!(detect_vcs_kind(&scratch_dir("g1-kind-none")), None);
}

#[test]
fn g1_hg_base_errors_without_hg() {
    let bogus = scratch_dir("g1-hg-bogus");
    assert_eq!(hg_rev_to_commit(&bogus, "tip"), None);
    assert!(hg_ancestor(&bogus, "tip", ".").is_err());
    assert_eq!(
        resolve_hg_base(
            &bogus,
            &Options {
                base: Some("a".to_string()),
                commit: Some("b".to_string()),
                ..defaults()
            }
        )
        .unwrap_err(),
        "'--base' can not be used with a base commit."
    );
    assert!(
        resolve_hg_base(
            &bogus,
            &Options {
                base: Some("tip".to_string()),
                ..defaults()
            }
        )
        .is_err()
    );
    assert!(
        resolve_hg_base(
            &bogus,
            &Options {
                commit: Some("tip".to_string()),
                ..defaults()
            }
        )
        .is_err()
    );
    assert!(resolve_hg_base(&bogus, &defaults()).is_err());
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn defaults() -> Options {
    Options {
        base: None,
        json: false,
        head: None,
        commit: None,
        message: None,
        message_file: None,
        reviewers: None,
        cc: None,
        update: None,
        create: false,
        only: false,
        conduit_uri: None,
        edit: false,
        raw: false,
        raw_command: None,
        draft: false,
        nounit: false,
        nolint: false,
        allow_untracked: false,
        apply_patches: false,
        never_apply_patches: false,
        amend_all: false,
        amend_autofixes: false,
        add_all: false,
        no_amend: false,
        uncommitted: false,
        verbatim: false,
        skip_binaries: false,
        skip_staging: false,
        coverage: false,
        no_coverage: false,
        browse: false,
        paths: Vec::new(),
        provided: Vec::new(),
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_options(&args(&[])).unwrap(), Some(defaults()));
}

#[test]
fn parses_base_and_json() {
    assert_eq!(
        parse_options(&args(&["--base", "HEAD~1", "--json"])).unwrap(),
        Some(Options {
            base: Some("HEAD~1".to_string()),
            json: true,
            provided: vec!["base", "json"],
            ..defaults()
        })
    );
    assert_eq!(
        parse_options(&args(&["--base=abc123"])).unwrap(),
        Some(Options {
            base: Some("abc123".to_string()),
            provided: vec!["base"],
            ..defaults()
        })
    );
}

#[test]
fn parses_range_and_field_flags() {
    let options = parse_options(&args(&[
        "--head",
        "HEAD",
        "--message",
        "take two",
        "--message-file",
        "msg.txt",
        "--reviewers",
        "alice,bob",
        "--cc",
        "carol",
        "--conduit-uri",
        "https://phab.example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(
        options,
        Options {
            head: Some("HEAD".to_string()),
            message: Some("take two".to_string()),
            message_file: Some("msg.txt".to_string()),
            reviewers: Some("alice,bob".to_string()),
            cc: Some("carol".to_string()),
            conduit_uri: Some("https://phab.example.test/".to_string()),
            provided: vec![
                "head",
                "message",
                "message-file",
                "reviewers",
                "cc",
                "conduit-uri"
            ],
            ..defaults()
        }
    );
    let options = parse_options(&args(&[
        "--head=abc",
        "--message=hi",
        "--message-file=m.txt",
        "--reviewers=al",
        "--cc=bo",
        "--conduit-uri=https://x.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.head.as_deref(), Some("abc"));
    assert_eq!(options.message.as_deref(), Some("hi"));
    assert_eq!(options.message_file.as_deref(), Some("m.txt"));
    assert_eq!(options.reviewers.as_deref(), Some("al"));
    assert_eq!(options.cc.as_deref(), Some("bo"));
    assert_eq!(options.conduit_uri.as_deref(), Some("https://x.test/"));
    // `--update` conflicts with `--reviewers`/`--cc`, so it parses alone.
    let options = parse_options(&args(&["--update=D12"])).unwrap().unwrap();
    assert_eq!(options.update.as_deref(), Some("D12"));
}

#[test]
fn parses_shorts_and_submit_modes() {
    let options = parse_options(&args(&["-m", "msg", "-F", "f.txt", "-a"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("msg"));
    assert_eq!(options.message_file.as_deref(), Some("f.txt"));
    assert!(options.add_all);
    assert_eq!(options.provided, vec!["message", "message-file", "add-all"]);

    let options = parse_options(&args(&["--create"])).unwrap().unwrap();
    assert!(options.create && !options.only && options.update.is_none());
    let options = parse_options(&args(&["--only"])).unwrap().unwrap();
    assert!(options.only && !options.create && options.update.is_none());
    let options = parse_options(&args(&["--update", "123"])).unwrap().unwrap();
    assert_eq!(options.update.as_deref(), Some("123"));
}

#[test]
fn parses_all_spec_flags() {
    // `--json` conflicts with nothing, so it pairs with every flag.
    let options = parse_options(&args(&["--json", "--edit"]))
        .unwrap()
        .unwrap();
    assert!(options.edit);
    let options = parse_options(&args(&["--json", "--raw"])).unwrap().unwrap();
    assert!(options.raw);
    let options = parse_options(&args(&["--json", "--raw-command", "git diff"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.raw_command.as_deref(), Some("git diff"));
    let options = parse_options(&args(&["--json", "--raw-command=git log"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.raw_command.as_deref(), Some("git log"));
    let options = parse_options(&args(&["--json", "--draft"]))
        .unwrap()
        .unwrap();
    assert!(options.draft);
    let options = parse_options(&args(&["--json", "--nounit"]))
        .unwrap()
        .unwrap();
    assert!(options.nounit);
    let options = parse_options(&args(&["--json", "--nolint"]))
        .unwrap()
        .unwrap();
    assert!(options.nolint);
    let options = parse_options(&args(&["--json", "--allow-untracked"]))
        .unwrap()
        .unwrap();
    assert!(options.allow_untracked);
    let options = parse_options(&args(&["--json", "--apply-patches"]))
        .unwrap()
        .unwrap();
    assert!(options.apply_patches);
    let options = parse_options(&args(&["--json", "--never-apply-patches"]))
        .unwrap()
        .unwrap();
    assert!(options.never_apply_patches);
    let options = parse_options(&args(&["--json", "--amend-all"]))
        .unwrap()
        .unwrap();
    assert!(options.amend_all);
    let options = parse_options(&args(&["--json", "--amend-autofixes"]))
        .unwrap()
        .unwrap();
    assert!(options.amend_autofixes);
    let options = parse_options(&args(&["--json", "--add-all"]))
        .unwrap()
        .unwrap();
    assert!(options.add_all);
    let options = parse_options(&args(&["--json", "--no-amend"]))
        .unwrap()
        .unwrap();
    assert!(options.no_amend);
    let options = parse_options(&args(&["--json", "--uncommitted"]))
        .unwrap()
        .unwrap();
    assert!(options.uncommitted);
    let options = parse_options(&args(&["--json", "--verbatim"]))
        .unwrap()
        .unwrap();
    assert!(options.verbatim);
    let options = parse_options(&args(&["--json", "--skip-binaries"]))
        .unwrap()
        .unwrap();
    assert!(options.skip_binaries);
    let options = parse_options(&args(&["--json", "--skip-staging"]))
        .unwrap()
        .unwrap();
    assert!(options.skip_staging);
    let options = parse_options(&args(&["--json", "--coverage"]))
        .unwrap()
        .unwrap();
    assert!(options.coverage);
    let options = parse_options(&args(&["--json", "--no-coverage"]))
        .unwrap()
        .unwrap();
    assert!(options.no_coverage);
    let options = parse_options(&args(&["--json", "--browse"]))
        .unwrap()
        .unwrap();
    assert!(options.browse);
    // Re-providing a flag overwrites without error.
    let options = parse_options(&args(&["--json", "--message", "a", "--message", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("b"));
    assert_eq!(options.provided, vec!["json", "message"]);
}

#[test]
fn parses_positional_paths() {
    // Positionals collect into `paths`; the base commit derives later.
    let options = parse_options(&args(&["HEAD~1"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["HEAD~1".to_string()]);
    assert_eq!(options.commit, None);
    let options = parse_options(&args(&["--", "HEAD~1"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["HEAD~1".to_string()]);
    // Several positionals (and a second `--`) parse; arity errors come later.
    let options = parse_options(&args(&["--", "a", "--", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(
        options.paths,
        vec!["a".to_string(), "--".to_string(), "b".to_string()]
    );
    let options = parse_options(&args(&["a", "b"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn parses_bool_equals_value_as_next_token() {
    // PHP splices `--bool=value` (`:826-832`): the value parses as the next
    // token, so it lands in `paths` here ...
    let options = parse_options(&args(&["--json=x"])).unwrap().unwrap();
    assert!(options.json);
    assert_eq!(options.paths, vec!["x".to_string()]);
    // ... and a flag-looking value even enables that flag.
    let err = parse_options(&args(&["--create=--only"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--create' and '--only' are mutually exclusive: \
         '--create' can not be used with '--only'."
    );
}

#[test]
fn parses_help() {
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_options(&args(&["-h"])).unwrap(), None);
    // Past `--`, help looks like a path.
    let options = parse_options(&args(&["--", "--help"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["--help".to_string()]);
    // Unknown names error even when they start with a known prefix.
    let err = parse_options(&args(&["--help=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'help'. Try 'arc help'.");
}

#[test]
fn rejects_bad_args() {
    // A trailing `param` flag names its typed spelling.
    for argv in [
        vec!["--base"],
        vec!["--head"],
        vec!["--message"],
        vec!["--message-file"],
        vec!["--raw-command"],
        vec!["--reviewers"],
        vec!["--cc"],
        vec!["--update"],
        vec!["--conduit-uri"],
    ] {
        let err = parse_options(&args(&argv)).unwrap_err();
        assert_eq!(err, format!("Option '{}' requires a parameter.", argv[0]));
    }
    let err = parse_options(&args(&["-m"])).unwrap_err();
    assert_eq!(err, "Option '-m' requires a parameter.");
    let err = parse_options(&args(&["-F"])).unwrap_err();
    assert_eq!(err, "Option '-F' requires a parameter.");
    // Empty values are fine; flag-looking values are consumed, not parsed.
    let options = parse_options(&args(&["--base="])).unwrap().unwrap();
    assert_eq!(options.base.as_deref(), Some(""));
    let options = parse_options(&args(&["--head="])).unwrap().unwrap();
    assert_eq!(options.head.as_deref(), Some(""));
    let options = parse_options(&args(&["--message", "--json"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("--json"));
    assert!(!options.json);
    // Unknown names report the dashes-stripped spelling.
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
    let err = parse_options(&args(&["--bogus=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
    let err = parse_options(&args(&["-x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'x'. Try 'arc help'.");
    // Short flags never combine.
    let err = parse_options(&args(&["-mx"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'mx'. Try 'arc help'.");
    let err = parse_options(&args(&["-"])).unwrap_err();
    assert_eq!(err, "Unknown argument ''. Try 'arc help'.");
}

#[test]
fn rejects_conflicting_submit_flags() {
    let err = parse_options(&args(&["--create", "--update", "1"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--create' and '--update' are mutually exclusive: \
         '--create' can not be used with '--update'."
    );
    let err = parse_options(&args(&["--create", "--only"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--create' and '--only' are mutually exclusive: \
         '--create' can not be used with '--only'."
    );
    let err = parse_options(&args(&["--only", "--message", "x"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--only' and '--message' are mutually exclusive: \
         '--only' does not update any revision."
    );
    // `--update` plus `--only` stays legal: `--update` wins.
    let options = parse_options(&args(&["--update", "1", "--only"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.update.as_deref(), Some("1"));
    assert!(options.only);
}

#[test]
fn rejects_every_declared_conflict() {
    // (argv, expected message): one case per table row, in table order.
    let cases: &[(&[&str], &str)] = &[
        (
            &["--raw", "--apply-patches"],
            "Arguments '--raw' and '--apply-patches' are mutually exclusive: '--raw' disables lint.",
        ),
        (
            &["--raw", "--never-apply-patches"],
            "Arguments '--raw' and '--never-apply-patches' are mutually exclusive: '--raw' disables lint.",
        ),
        (
            &["--raw", "--create"],
            "Arguments '--raw' and '--create' are mutually exclusive: '--raw' and '--create' both need stdin. Use '--raw-command'.",
        ),
        (
            &["--raw", "--edit"],
            "Arguments '--raw' and '--edit' are mutually exclusive: '--raw' and '--edit' both need stdin. Use '--raw-command'.",
        ),
        (
            &["--raw", "--raw-command", "cmd"],
            "Arguments '--raw' and '--raw-command' are mutually exclusive.",
        ),
        (
            &["--raw-command", "cmd", "--apply-patches"],
            "Arguments '--raw-command' and '--apply-patches' are mutually exclusive: '--raw-command' disables lint.",
        ),
        (
            &["--raw-command", "cmd", "--never-apply-patches"],
            "Arguments '--raw-command' and '--never-apply-patches' are mutually exclusive: '--raw-command' disables lint.",
        ),
        (
            &["--create", "--edit"],
            "Arguments '--create' and '--edit' are mutually exclusive: '--create' can not be used with '--edit'.",
        ),
        (
            &["--draft", "--edit"],
            "Arguments '--draft' and '--edit' are mutually exclusive.",
        ),
        (
            &["--draft", "--only"],
            "Arguments '--draft' and '--only' are mutually exclusive.",
        ),
        (
            &["--draft", "--update", "1"],
            "Arguments '--draft' and '--update' are mutually exclusive.",
        ),
        (
            &["--nolint", "--apply-patches"],
            "Arguments '--nolint' and '--apply-patches' are mutually exclusive: '--nolint' suppresses lint.",
        ),
        (
            &["--nolint", "--never-apply-patches"],
            "Arguments '--nolint' and '--never-apply-patches' are mutually exclusive: '--nolint' suppresses lint.",
        ),
        (
            &["--only", "--edit"],
            "Arguments '--only' and '--edit' are mutually exclusive: '--only' does affect revisions.",
        ),
        (
            &["--verbatim", "--update", "1"],
            "Arguments '--verbatim' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--update", "1", "--verbatim"],
            "Arguments '--verbatim' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--only"],
            "Arguments '--verbatim' and '--only' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--raw"],
            "Arguments '--verbatim' and '--raw' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--raw-command", "cmd"],
            "Arguments '--verbatim' and '--raw-command' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--message-file", "f"],
            "Arguments '--verbatim' and '--message-file' are mutually exclusive: 1",
        ),
        (
            &["--only", "--reviewers", "a"],
            "Arguments '--reviewers' and '--only' are mutually exclusive: 1",
        ),
        (
            &["--update", "1", "--reviewers", "a"],
            "Arguments '--reviewers' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--only", "--cc", "a"],
            "Arguments '--cc' and '--only' are mutually exclusive: 1",
        ),
        (
            &["--update", "1", "--cc", "a"],
            "Arguments '--cc' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--coverage", "--no-coverage"],
            "Arguments '--coverage' and '--no-coverage' are mutually exclusive.",
        ),
    ];
    for (argv, expected) in cases {
        let err = parse_options(&args(argv)).unwrap_err();
        assert_eq!(err, expected.to_string(), "argv: {argv:?}");
    }
    // Symmetric pairs name the earlier flag first.
    let err = parse_options(&args(&["--apply-patches", "--never-apply-patches"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--apply-patches' and '--never-apply-patches' are mutually exclusive: 1"
    );
    let err = parse_options(&args(&["--never-apply-patches", "--apply-patches"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--never-apply-patches' and '--apply-patches' are mutually exclusive: 1"
    );
    // Unknown flags beat conflicts: the whole line parses first.
    let err = parse_options(&args(&["--create", "--bogus", "--only"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
}

#[test]
fn checks_vcs_supports() {
    // Supported combinations pass quietly.
    assert!(check_supports(Vcs::Git, &["head", "json"]).is_ok());
    assert!(check_supports(Vcs::Hg, &["uncommitted", "verbatim"]).is_ok());
    assert!(check_supports(Vcs::Git, &[]).is_ok());
    // Unsupported flags report the VCS plus any `nosupport` note.
    let err = check_supports(Vcs::Git, &["uncommitted"]).unwrap_err();
    assert_eq!(err, "Option '--uncommitted' is not supported under git.");
    let err = check_supports(Vcs::Svn, &["uncommitted"]).unwrap_err();
    assert_eq!(err, "Option '--uncommitted' is not supported under svn.");
    let err = check_supports(Vcs::Svn, &["verbatim"]).unwrap_err();
    assert_eq!(err, "Option '--verbatim' is not supported under svn.");
    let err = check_supports(Vcs::Svn, &["edit"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--edit' is not supported under svn. \
         Edit revisions via the web interface when using SVN."
    );
    let err = check_supports(Vcs::Svn, &["base"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--base' is not supported under svn. Subversion does not use base commits."
    );
    let err = check_supports(Vcs::Svn, &["head"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--head' is not supported under svn. Subversion does not support commit ranges."
    );
    let err = check_supports(Vcs::Hg, &["head"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--head' is not supported under hg. Mercurial does not support --head yet."
    );
    // First unsupported flag in provision order wins.
    let err = check_supports(Vcs::Svn, &["head", "uncommitted"]).unwrap_err();
    assert!(err.contains("'--head'"), "{err}");
    let err = check_supports(Vcs::Svn, &["uncommitted", "head"]).unwrap_err();
    assert!(err.contains("'--uncommitted'"), "{err}");
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-diff-d1-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git_dir(tag: &str) -> PathBuf {
    let dir = scratch_dir(tag);
    let status = std::process::Command::new("git")
        .arg("init")
        .arg(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

#[test]
fn detects_working_copy_vcs() {
    assert_eq!(detect_vcs(&scratch_dir("none")), None);
    let hg = scratch_dir("hg");
    std::fs::create_dir_all(hg.join("sub")).unwrap();
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert_eq!(detect_vcs(&hg.join("sub")), Some(Vcs::Hg));
    let svn = scratch_dir("svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert_eq!(detect_vcs(&svn), Some(Vcs::Svn));
    assert_eq!(detect_vcs(&git_dir("git")), Some(Vcs::Git));
    // Git wins over enclosing markers.
    let nested = git_dir("nested");
    std::fs::create_dir_all(nested.join(".hg")).unwrap();
    assert_eq!(detect_vcs(&nested), Some(Vcs::Git));
}

#[test]
fn prepares_options_per_working_copy() {
    // No working copy: options pass through untouched.
    let options = Options {
        head: Some(String::new()),
        ..defaults()
    };
    let resolved = prepare(&scratch_dir("pass"), &options).unwrap();
    assert_eq!(resolved.head, None);
    assert_eq!(resolved.commit, None);
    // Git derives the base commit from `paths`.
    let options = Options {
        paths: vec!["abc".to_string()],
        provided: vec!["json"],
        ..defaults()
    };
    let resolved = prepare(&git_dir("commit"), &options).unwrap();
    assert_eq!(resolved.commit.as_deref(), Some("abc"));
    // ... and rejects several positionals.
    let options = Options {
        paths: vec!["a".to_string(), "b".to_string()],
        ..defaults()
    };
    let err = prepare(&git_dir("two"), &options).unwrap_err();
    assert!(err.contains("exactly one base commit"), "{err}");
    // Supports errors beat arity errors.
    let options = Options {
        paths: vec!["a".to_string(), "b".to_string()],
        provided: vec!["uncommitted"],
        uncommitted: true,
        ..defaults()
    };
    let err = prepare(&git_dir("order"), &options).unwrap_err();
    assert_eq!(err, "Option '--uncommitted' is not supported under git.");
    // Svn positionals are file paths, not base commits.
    let svn = scratch_dir("paths");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    let options = Options {
        paths: vec!["a".to_string()],
        ..defaults()
    };
    let err = prepare(&svn, &options).unwrap_err();
    assert_eq!(
        err,
        "This version control system does not support commit ranges."
    );
    // Detected non-git working copies fail honestly until their flows land.
    let err = prepare(&svn, &defaults()).unwrap_err();
    assert_eq!(err, "svn working copies are not yet ported (see T1128)");
    let hg = scratch_dir("ported");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    let err = prepare(&hg, &defaults()).unwrap_err();
    assert_eq!(err, "hg working copies are not yet ported (see T1128)");
}

#[test]
fn resolve_range_rejects_base_with_commit() {
    let options = Options {
        base: Some("abc".to_string()),
        commit: Some("def".to_string()),
        ..defaults()
    };
    let err = resolve_range(Path::new("."), &options).unwrap_err();
    assert_eq!(err, "'--base' can not be used with a base commit.");
}

#[test]
fn derives_submit_mode() {
    assert_eq!(submit_mode(&defaults()).unwrap(), None);
    assert_eq!(
        submit_mode(&Options {
            create: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Create)
    );
    assert_eq!(
        submit_mode(&Options {
            only: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Only)
    );
    assert_eq!(
        submit_mode(&Options {
            update: Some("D12".to_string()),
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Update(12))
    );
    // `--update` wins over `--only`, like shouldOnlyCreateDiff().
    assert_eq!(
        submit_mode(&Options {
            update: Some("7".to_string()),
            only: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Update(7))
    );
    let err = submit_mode(&Options {
        update: Some("nope".to_string()),
        ..defaults()
    })
    .unwrap_err();
    assert!(err.contains("Parameter to --update"), "{err}");
}

#[test]
fn merges_fields_oldest_first_with_faux_flags() {
    let commits = vec![
        "Oldest title\n\nOld body.\n".to_string(),
        "Newest title\n\nReviewers: alice\n".to_string(),
    ];
    let fields = merge_message_fields(&commits, Some("bob"), Some("carol"));
    assert_eq!(
        fields.get(diff_local::FIELD_TITLE),
        Some(&diff_local::FieldValue::Text("Oldest title".to_string()))
    );
    let summary = fields.get(diff_local::FIELD_SUMMARY);
    assert!(
        matches!(summary, Some(diff_local::FieldValue::Text(text)) if text.contains("Old body") && text.contains("Newest title")),
        "missing summary: {summary:?}"
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
}

#[test]
fn renders_fields_json() {
    let mut fields = diff_local::CommitFields::new();
    fields.insert(
        diff_local::FIELD_TITLE.to_string(),
        diff_local::FieldValue::Text("T".to_string()),
    );
    fields.insert(
        diff_local::FIELD_REVIEWERS.to_string(),
        diff_local::FieldValue::List(vec!["a".to_string()]),
    );
    assert_eq!(
        fields_json(&fields),
        serde_json::json!({"title": "T", "reviewerPHIDs": ["a"]})
    );
}

#[test]
fn parses_empty_numstat() {
    assert_eq!(parse_numstat_z(b"").unwrap(), Vec::new());
}

#[test]
fn parses_numstat_entries() {
    let raw = b"2\t1\tf.txt\0-\t-\tbin.dat\0";
    assert_eq!(
        parse_numstat_z(raw).unwrap(),
        vec![
            FileDiff {
                path: "f.txt".to_string(),
                added: 2,
                deleted: 1
            },
            FileDiff {
                path: "bin.dat".to_string(),
                added: 0,
                deleted: 0
            },
        ]
    );
}

#[test]
fn parses_numstat_rename_to_new_path() {
    let raw = b"0\t0\t\0g.txt\0renamed.txt\0";
    assert_eq!(
        parse_numstat_z(raw).unwrap(),
        vec![FileDiff {
            path: "renamed.txt".to_string(),
            added: 0,
            deleted: 0
        }]
    );
}

#[test]
fn rejects_malformed_numstat() {
    assert!(parse_numstat_z(b"no-tabs-here\0").is_err());
    assert!(parse_numstat_z(b"x\t1\tf\0").is_err());
    assert!(parse_numstat_z(b"0\t0\t\0only-old\0").is_err());
}

#[test]
fn builds_json_document() {
    let files = vec![FileDiff {
        path: "f.txt".to_string(),
        added: 2,
        deleted: 1,
    }];
    let doc = diff_json("abc", &files);
    assert_eq!(
        doc,
        serde_json::json!({
            "base": "abc",
            "files": [{"path": "f.txt", "added": 2, "deleted": 1}],
            "insertions": 2,
            "deletions": 1,
        })
    );
    assert_eq!(summarize(&[]), (0, 0));
}

#[test]
fn ansi_console_wrap_delegates_to_terminal() {
    for (text, indent, prefix) in [
        ("hi".to_string(), 4, true),
        ("a\nb".to_string(), 2, false),
        (String::new(), 0, true),
        ("word ".repeat(20), 4, true),
    ] {
        assert_eq!(
            console_wrap(&text, indent, prefix),
            crate::terminal::console_wrap(&text, indent, prefix),
            "{text:?} {indent} {prefix}"
        );
    }
    let colored = "\x1b[1mhi\x1b[m there";
    assert_eq!(
        console_wrap(colored, 0, true),
        crate::terminal::console_wrap(colored, 0, true)
    );
}

#[test]
fn ansi_working_copy_desc_underlines_path() {
    assert_eq!(
        working_copy_desc_ansi("/w", false),
        "  Working copy: /w\n\n"
    );
    assert_eq!(
        working_copy_desc_ansi("/w", true),
        "  Working copy: \x1b[4m/w\x1b[m\n\n"
    );
    assert_eq!(working_copy_desc("/w"), "  Working copy: /w\n\n");
}

#[test]
fn ansi_worktree_blocks_thread_underline() {
    let paths = vec!["a".to_string()];
    let plain = clean_worktree_error_ansi(CleanWorktreeIssue::Conflicts, "/w", &paths, false);
    assert!(plain.contains("  Working copy: /w\n\n"), "{plain:?}");
    assert!(!plain.contains('\x1b'), "{plain:?}");
    let colored = clean_worktree_error_ansi(CleanWorktreeIssue::Conflicts, "/w", &paths, true);
    assert!(
        colored.contains("  Working copy: \x1b[4m/w\x1b[m\n\n"),
        "{colored:?}"
    );

    let plain = untracked_block_ansi("/w", "git", &paths, false);
    assert!(plain.contains("  Working copy: /w\n\n"), "{plain:?}");
    assert!(!plain.contains('\x1b'));
    let colored = untracked_block_ansi("/w", "git", &paths, true);
    assert!(colored.contains("\x1b[4m/w\x1b[m"), "{colored:?}");

    let plain = uncommitted_block_ansi("/w", &paths, &[], false);
    assert!(plain.contains("  Working copy: /w\n\n"), "{plain:?}");
    assert!(!plain.contains('\x1b'));
    let colored = uncommitted_block_ansi("/w", &paths, &[], true);
    assert!(colored.contains("\x1b[4m/w\x1b[m"), "{colored:?}");
}

#[test]
fn ansi_submit_reports_bold_label_and_underline_uri() {
    let call = |method: &str, _params: &serde_json::Value| {
        assert_eq!(
            method,
            diff_submit::METHOD_CREATE_DIFF,
            "unexpected {method}"
        );
        Ok(serde_json::json!({"diffid": 1, "uri": "http://x/D1"}))
    };
    let input = SubmitInput {
        mode: SubmitMode::Only,
        message: None,
        draft: false,
        message_corpus: None,
        fields: serde_json::Value::Null,
        spec: serde_json::json!({}),
        properties: Vec::new(),
        lint_unit: diff_local::LintUnitReport::empty(),
        included_changes: "Included changes:\n".to_string(),
        json: false,
    };
    let plain = submit_over_ansi(&call, &input, false).unwrap();
    assert_eq!(
        plain,
        "Created a new Differential diff:\n        Diff URI: http://x/D1\n\nIncluded changes:\n"
    );
    let colored = submit_over_ansi(&call, &input, true).unwrap();
    assert_eq!(
        colored,
        "Created a new Differential diff:\n        \x1b[1mDiff URI:\x1b[m \x1b[4mhttp://x/D1\x1b[m\n\nIncluded changes:\n"
    );
}

fn create_test_input(draft: bool, corpus: Option<&str>) -> SubmitInput {
    SubmitInput {
        mode: SubmitMode::Create,
        message: None,
        draft,
        message_corpus: corpus.map(str::to_string),
        fields: serde_json::json!({"title": "Local"}),
        spec: serde_json::json!({}),
        properties: Vec::new(),
        lint_unit: diff_local::LintUnitReport::empty(),
        included_changes: String::new(),
        json: false,
    }
}

#[test]
fn create_uses_revision_edit_when_server_sends_transactions() {
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            assert_eq!(
                params["corpus"],
                serde_json::json!("Add thing\n\nReviewers: alice\n")
            );
            return Ok(serde_json::json!({
                "fields": {"title": "Add thing",
                           "reviewerPHIDs": ["PHID-USER-1"]},
                "transactions": [
                    {"type": "title", "value": "Add thing"},
                    {"type": "reviewers.set", "value": ["PHID-USER-1"]},
                ],
            }));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_REVISION_EDIT,
            "unexpected {method}"
        );
        let types: Vec<&str> = params["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, vec!["title", "reviewers.set", "update", "draft"]);
        assert_eq!(
            params["transactions"][2]["value"],
            serde_json::json!("PHID-DIFF-9")
        );
        Ok(serde_json::json!({"object": {"id": 7}}))
    };
    let input = create_test_input(true, Some("Add thing\n\nReviewers: alice\n"));
    let revision = submit_create_revision(
        &call,
        &input,
        3,
        Some("PHID-DIFF-9"),
        "https://phab.test/differential/diff/3/",
    )
    .unwrap();
    assert_eq!(revision.revision_id, 7);
    assert_eq!(revision.uri, "https://phab.test/D7");
}

#[test]
fn create_edits_without_draft_transaction_when_not_draft() {
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({
                "fields": {"title": "T"},
                "transactions": [{"type": "title", "value": "T"}],
            }));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_REVISION_EDIT,
            "unexpected {method}"
        );
        Ok(serde_json::json!({"object": {"id": 8}}))
    };
    let input = create_test_input(false, Some("T\n"));
    let revision = submit_create_revision(&call, &input, 3, None, "u").unwrap();
    assert_eq!(revision.revision_id, 8);
}

#[test]
fn create_falls_back_to_legacy_without_transactions() {
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({
                "fields": {"title": "T", "reviewerPHIDs": ["PHID-USER-1"]},
            }));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_CREATE_REVISION,
            "unexpected {method}"
        );
        assert_eq!(
            params["fields"]["reviewerPHIDs"],
            serde_json::json!(["PHID-USER-1"])
        );
        assert_eq!(params["diffid"], serde_json::json!(3));
        Ok(serde_json::json!({"revisionid": 9, "uri": "https://phab.test/D9"}))
    };
    let input = create_test_input(false, Some("T\n\nReviewers: alice\n"));
    let revision = submit_create_revision(&call, &input, 3, None, "u").unwrap();
    assert_eq!(revision.revision_id, 9);
    assert_eq!(revision.uri, "https://phab.test/D9");
}

#[test]
fn create_draft_without_transactions_errors_like_php() {
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(
            method,
            diff_submit::METHOD_PARSE_COMMIT_MESSAGE,
            "unexpected {method}"
        );
        Ok(serde_json::json!({"fields": {"title": "T"}}))
    };
    let input = create_test_input(true, Some("T\n"));
    let err = submit_create_revision(&call, &input, 3, None, "u").unwrap_err();
    assert!(err.contains("--draft"), "unexpected: {err}");
}

#[test]
fn create_without_corpus_keeps_local_fields_legacy() {
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(
            method,
            diff_submit::METHOD_CREATE_REVISION,
            "unexpected {method}"
        );
        assert_eq!(params["fields"]["title"], serde_json::json!("Local"));
        Ok(serde_json::json!({"revisionid": 1, "uri": "u"}))
    };
    let input = create_test_input(false, None);
    submit_create_revision(&call, &input, 3, None, "u").unwrap();
}

#[test]
fn create_scrubs_placeholder_titles() {
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({
                "fields": {"title": "<placeholder>",
                           "reviewerPHIDs": ["PHID-USER-1"]},
                "transactions": [
                    {"type": "title", "value": "<placeholder>"},
                    {"type": "reviewers.set", "value": ["PHID-USER-1"]},
                ],
            }));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_REVISION_EDIT,
            "unexpected {method}"
        );
        Ok(serde_json::json!({"object": {"id": 2}}))
    };
    // Flags-only corpus: placeholder title dropped, reviewers kept, edit
    // path still taken on the remaining transaction.
    let input = create_test_input(false, Some("<placeholder>\n\nReviewers: alice\n"));
    let revision = submit_create_revision(&call, &input, 3, None, "u").unwrap();
    assert_eq!(revision.revision_id, 2);
    assert_eq!(
        scrub_placeholder_xactions(&[
            serde_json::json!({"type": "title", "value": "<placeholder>"}),
            serde_json::json!({"type": "title", "value": "Real"}),
        ])
        .len(),
        1
    );
}

#[test]
fn join_create_corpus_orders_messages_and_strips_placeholder() {
    assert_eq!(join_create_corpus(&[], None, None), None);
    assert_eq!(
        join_create_corpus(&["one".to_string()], None, None),
        Some("one".to_string())
    );
    let corpus = join_create_corpus(
        &["First".to_string(), "Second".to_string()],
        Some("a"),
        None,
    )
    .unwrap();
    assert!(
        corpus.starts_with("First\n\nSecond\n\n"),
        "unexpected: {corpus}"
    );
    assert!(corpus.contains("Reviewers: a"), "unexpected: {corpus}");
    assert!(!corpus.contains("<placeholder>"), "unexpected: {corpus}");
    let faux_only = join_create_corpus(&[], Some("a"), None).unwrap();
    assert!(
        faux_only.contains("<placeholder>"),
        "unexpected: {faux_only}"
    );
}

#[test]
fn revision_uri_derives_from_diff_origin() {
    assert_eq!(
        revision_uri_from_diff_uri("https://phab.test/differential/diff/3/", 7),
        "https://phab.test/D7"
    );
    assert_eq!(revision_uri_from_diff_uri("garbage", 7), "/D7");
}

#[test]
fn r9_core62_no_browser_message_matches_ceb() {
    // PHP `ArcanistWorkflow.php:2204-2206` @ ceb082ef renders exactly this.
    let expected = "Unable to find a browser command to run. Set 'browser' in your Arcanist config to specify a command to use.";
    assert_eq!(NO_BROWSER_MESSAGE, expected);
    // The scan-failure path surfaces the same bytes (off Windows, where
    // `cmd /c start` always resolves).
    #[cfg(not(windows))]
    {
        let none = |_: &str| false;
        assert_eq!(
            browser_open_argv("http://example.test/", None, &none).unwrap_err(),
            expected
        );
    }
}

/* T1153 DIFF-D findings: update resolution, json, phid stamping. */

fn update_stub_call() -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_QUERY {
            assert_eq!(params["ids"], serde_json::json!([5]));
            return Ok(serde_json::json!([{
                "id": 5, "title": "Thing",
                "authorPHID": "PHID-USER-1", "statusName": "Accepted",
            }]));
        }
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            assert_eq!(params["edit"], serde_json::json!(false));
            return Ok(serde_json::json!("Thing\n\nSummary: yes\n"));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_PARSE_COMMIT_MESSAGE,
            "DIFF-D test: unexpected {method}"
        );
        Ok(serde_json::json!({
            "fields": {"title": "Thing", "summary": "yes"},
        }))
    }
}

fn update_test_options(message: Option<&str>, edit: bool) -> Options {
    Options {
        update: Some("5".to_string()),
        message: message.map(str::to_string),
        edit,
        ..Options::default()
    }
}

fn update_test_target(fields: &serde_json::Value) -> UpdateTarget<'_> {
    UpdateTarget {
        revision_id: 5,
        revision_arg: "5",
        verbatim_fields: fields,
        check_ownership: true,
        edit_fields: None,
    }
}

#[test]
fn d6_update_resolves_remote_fields_not_empty() {
    let meta = scratch_dir("d6-fields");
    let call = update_stub_call();
    let edit = |_: &str, _: &str| -> Result<String, String> { panic!("DIFF-D6: no editor") };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(Some("take two"), false);
    let resolved = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap();
    assert_eq!(
        resolved.fields,
        serde_json::json!({"title": "Thing", "summary": "yes"})
    );
    assert_eq!(resolved.message, "take two");
}

#[test]
fn d6_update_missing_revision_names_arg() {
    let meta = scratch_dir("d6-missing");
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(
            method,
            diff_submit::METHOD_QUERY,
            "DIFF-D6: unexpected {method}"
        );
        Ok(serde_json::json!([]))
    };
    let edit = |_: &str, _: &str| -> Result<String, String> { Ok(String::new()) };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let mut options = update_test_options(Some("m"), false);
    options.update = Some("D5".to_string());
    let err = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap_err();
    assert_eq!(err, "Revision '5' does not exist!");
}

#[test]
fn d6_update_foreign_revision_confirms() {
    let meta = scratch_dir("d6-ownership");
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_QUERY {
            return Ok(serde_json::json!([{
                "id": 5, "title": "Theirs", "authorPHID": "PHID-USER-9",
            }]));
        }
        assert_eq!(method, "user.whoami", "DIFF-D6: unexpected {method}");
        Ok(serde_json::json!({"phid": "PHID-USER-1"}))
    };
    let edit = |_: &str, _: &str| -> Result<String, String> { Ok(String::new()) };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new("n\n".as_bytes());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(Some("m"), false);
    let err = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap_err();
    assert_eq!(err, diff_submit::OWNERSHIP_ABORT);
    let prompt = String::from_utf8(writer).unwrap();
    assert!(prompt.contains("You don't own revision D5"), "{prompt}");
}

#[test]
fn d9_empty_message_opens_editor_and_caches() {
    let meta = scratch_dir("d9-editor");
    let call = update_stub_call();
    let seen = std::cell::RefCell::new(Vec::new());
    let edit = |template: &str, name: &str| -> Result<String, String> {
        seen.borrow_mut()
            .push((template.to_string(), name.to_string()));
        Ok("second try\n# a comment\n".to_string())
    };
    let prefill = || -> Result<Option<String>, String> { Ok(Some("  - foo\n".to_string())) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    // Empty `--message` still opens the editor, like ceb (`:701-702`).
    let options = update_test_options(Some(""), false);
    let resolved = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap();
    assert_eq!(resolved.message, "second try\n");
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, EDITOR_DIFF_UPDATE);
    assert!(seen[0].0.contains("# Updating D5: Thing"), "{}", seen[0].0);
    assert!(seen[0].0.contains("  - foo"), "{}", seen[0].0);
    // The raw editor output (comments included) is cached.
    let saved = read_scratch_json(&meta, SCRATCH_UPDATE_MESSAGES);
    assert_eq!(saved["5"], serde_json::json!("second try\n# a comment\n"));
}

#[test]
fn d9_blank_editor_output_aborts() {
    let meta = scratch_dir("d9-abort");
    let call = update_stub_call();
    let edit = |_: &str, _: &str| -> Result<String, String> { Ok("# nothing\n".to_string()) };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(None, false);
    let err = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap_err();
    assert_eq!(err, diff_local::USER_ABORT);
}

#[test]
fn d9_raw_update_without_message_errors() {
    let meta = scratch_dir("d9-raw");
    let call = update_stub_call();
    let edit = |_: &str, _: &str| -> Result<String, String> { panic!("DIFF-D9: no editor") };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let mut options = update_test_options(None, false);
    options.raw = true;
    let err = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap_err();
    assert_eq!(err, diff_local::RAW_UPDATE_MESSAGE_ERROR);
}

#[test]
fn d10_edit_caches_corpus_for_next_run() {
    let meta = scratch_dir("d10-edit");
    let call = update_stub_call();
    let seen = std::cell::RefCell::new(Vec::new());
    let edit = |template: &str, name: &str| -> Result<String, String> {
        seen.borrow_mut()
            .push((template.to_string(), name.to_string()));
        Ok("Thing\n\nSummary: edited\n".to_string())
    };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(Some("m"), true);
    resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, EDITOR_DIFF_EDIT);
    // Without `--edit` the cache is ignored; with it, the cached corpus
    // is the editor input.
    assert_eq!(seen[0].0, "Thing\n\nSummary: yes\n");
    let saved = read_scratch_json(&meta, SCRATCH_EDIT_MESSAGES);
    assert_eq!(saved["5"], serde_json::json!("Thing\n\nSummary: edited\n"));
}

#[test]
fn d10_verbatim_fields_defeat_edit_cache() {
    let meta = scratch_dir("d10-verbatim");
    write_scratch_json(
        &meta,
        SCRATCH_EDIT_MESSAGES,
        &serde_json::json!({"5": "stale cache\n"}),
    );
    let call = update_stub_call();
    let seen = std::cell::RefCell::new(String::new());
    let edit = |template: &str, _: &str| -> Result<String, String> {
        *seen.borrow_mut() = template.to_string();
        Ok(template.to_string())
    };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(Some("m"), true);
    resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({"title": "Local"})),
    )
    .unwrap();
    // Non-empty local fields refetch the base corpus (`:659-670`).
    assert_eq!(*seen.borrow(), "Thing\n\nSummary: yes\n");
}

#[test]
fn d17_json_updates_and_creates_print_nothing() {
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(serde_json::json!({"diffid": 3, "uri": "http://x/D1"}));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_UPDATE_REVISION,
            "DIFF-D17: unexpected {method}"
        );
        Ok(serde_json::json!({"revisionid": 5, "uri": "http://x/D5"}))
    };
    let input = SubmitInput {
        mode: SubmitMode::Update(5),
        message: Some("m".to_string()),
        draft: false,
        message_corpus: None,
        fields: serde_json::json!({}),
        spec: serde_json::json!({}),
        properties: Vec::new(),
        lint_unit: diff_local::LintUnitReport::empty(),
        included_changes: String::new(),
        json: true,
    };
    let report = submit_over_with_uri_ansi(&call, &input, false).unwrap();
    assert_eq!(report.text, "");
    assert_eq!(report.uri.as_deref(), Some("http://x/D5"));
}

#[test]
fn d7_amend_after_create_rewrites_git_message() {
    let git = git_dir("d7-amend");
    // Pin amendability regardless of ambient user config.
    std::fs::write(git.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    std::fs::write(git.join("f.txt"), "a\n").unwrap();
    // Repo-local identity for both the seed commit and the later amend
    // (one-shot `-c` would not cover the amend on machines without a
    // global identity, like CI runners).
    for (key, value) in [("user.name", "t"), ("user.email", "t@t")] {
        let set = std::process::Command::new("git")
            .current_dir(&git)
            .args(["config", key, value])
            .status()
            .unwrap();
        assert!(set.success());
    }
    let add = std::process::Command::new("git")
        .current_dir(&git)
        .args(["add", "-A"])
        .status()
        .unwrap();
    assert!(add.success());
    let commit = std::process::Command::new("git")
        .current_dir(&git)
        .args(["commit", "-qam", "old"])
        .status()
        .unwrap();
    assert!(commit.success());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        assert_eq!(params["revision_id"], serde_json::json!(7));
        Ok(serde_json::json!("new\n\nDifferential Revision: D7\n"))
    };
    let options = Options::default();
    amend_after_create(
        &git,
        &hg::HgCommand::default(),
        &options,
        &call,
        7,
        Some(VcsKind::Git),
    )
    .unwrap();
    let out = std::process::Command::new("git")
        .current_dir(&git)
        .args(["log", "-1", "--format=%B"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        "new\n\nDifferential Revision: D7"
    );
}

#[test]
fn d7_no_amend_flag_skips_rewrite() {
    let git = git_dir("d7-noamend");
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        Ok(serde_json::json!("revised\n"))
    };
    let options = Options {
        no_amend: true,
        ..Options::default()
    };
    // Fetches (like PHP) but rewrites nothing.
    amend_after_create(
        &git,
        &hg::HgCommand::default(),
        &options,
        &call,
        7,
        Some(VcsKind::Git),
    )
    .unwrap();
}

#[test]
fn d11_autotarget_hit_gates_lint_unit_props() {
    let props = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(serde_json::json!({
                "diffid": 3, "uri": "http://x/D1", "phid": "PHID-DIFF-9",
            }));
        }
        if method == diff_submit::METHOD_QUERY_AUTOTARGETS {
            return Ok(serde_json::json!({"targetMap": {"arcanist.lint": "PHID-L"}}));
        }
        if method == diff_submit::METHOD_SENDMESSAGE {
            return Ok(serde_json::json!(null));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_SET_DIFF_PROPERTY,
            "DIFF-D11: unexpected {method}"
        );
        props
            .borrow_mut()
            .push(params["name"].as_str().unwrap_or("").to_string());
        Ok(serde_json::json!(null))
    };
    let input = SubmitInput {
        mode: SubmitMode::Only,
        message: None,
        draft: false,
        message_corpus: None,
        fields: serde_json::Value::Null,
        spec: serde_json::json!({}),
        properties: vec![
            diff_submit::DiffProperty {
                name: diff_submit::PROP_LINT.to_string(),
                data: "[]".to_string(),
            },
            diff_submit::DiffProperty {
                name: diff_submit::PROP_LOCAL_COMMITS.to_string(),
                data: "[]".to_string(),
            },
        ],
        lint_unit: diff_local::LintUnitReport::empty(),
        included_changes: String::new(),
        json: false,
    };
    submit_over_with_uri_ansi(&call, &input, false).unwrap();
    // The hit gates `arc:lint` off; `local:commits` still sends.
    assert_eq!(*props.borrow(), vec!["local:commits".to_string()]);
}

#[test]
fn d11_autotarget_miss_keeps_legacy_props() {
    let props = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(serde_json::json!({"diffid": 3, "uri": "http://x/D1"}));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_SET_DIFF_PROPERTY,
            "DIFF-D11: unexpected {method}"
        );
        props
            .borrow_mut()
            .push(params["name"].as_str().unwrap_or("").to_string());
        Ok(serde_json::json!(null))
    };
    let input = SubmitInput {
        mode: SubmitMode::Only,
        message: None,
        draft: false,
        message_corpus: None,
        fields: serde_json::Value::Null,
        spec: serde_json::json!({}),
        properties: vec![diff_submit::DiffProperty {
            name: diff_submit::PROP_UNIT.to_string(),
            data: "[]".to_string(),
        }],
        lint_unit: diff_local::LintUnitReport::empty(),
        included_changes: String::new(),
        json: false,
    };
    submit_over_with_uri_ansi(&call, &input, false).unwrap();
    // No diff PHID means no upload attempt; the legacy prop sends.
    assert_eq!(*props.borrow(), vec!["arc:unit".to_string()]);
}

#[test]
fn d13_staging_skip_ladder_records_statuses() {
    let recorded = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_SET_DIFF_PROPERTY);
        assert_eq!(params["name"], serde_json::json!("arc.staging"));
        assert_eq!(params["diff_id"], serde_json::json!(9));
        recorded
            .borrow_mut()
            .push(params["data"].as_str().unwrap().to_string());
        Ok(serde_json::json!(null))
    };
    let dir = git_dir("d13-ladder");
    // Flag skip short-circuits before any query.
    let options = Options {
        skip_staging: true,
        ..Options::default()
    };
    submit_changes_to_staging_area(&dir, &options, &call, Some(VcsKind::Git), 9, "b", false)
        .unwrap();
    // Raw sources skip next.
    let options = Options {
        raw: true,
        ..Options::default()
    };
    submit_changes_to_staging_area(&dir, &options, &call, Some(VcsKind::Git), 9, "b", false)
        .unwrap();
    // No callsign or remote: repository unknown (no query succeeds).
    let options = Options::default();
    submit_changes_to_staging_area(&dir, &options, &call, Some(VcsKind::Git), 9, "b", false)
        .unwrap();
    let recorded = recorded.borrow();
    assert_eq!(recorded.len(), 3);
    assert!(recorded[0].contains("\"user.skip\""), "{}", recorded[0]);
    assert!(recorded[1].contains("\"diff.raw\""), "{}", recorded[1]);
    let third = &recorded[2];
    assert!(third.contains("\"repository.unknown\""), "{third}");
}

#[test]
fn d13_staging_server_ladder_records_statuses() {
    let repo_rows = std::cell::RefCell::new(serde_json::json!([]));
    let dir = git_dir("d13-server");
    std::fs::write(dir.join(".arcconfig"), "{\"repository.callsign\": \"X\"}").unwrap();
    let options = Options::default();
    // Each server-side answer maps to its skip status.
    for (rows, status) in [
        (serde_json::json!([]), "repository.unknown"),
        (serde_json::json!([{}]), "repository.unavailable"),
        (
            serde_json::json!([{"staging": {"supported": false}}]),
            "repository.unsupported",
        ),
        (
            serde_json::json!([{"staging": {"supported": true}}]),
            "repository.unconfigured",
        ),
    ] {
        *repo_rows.borrow_mut() = rows;
        let payload = std::cell::RefCell::new(String::new());
        let call = |method: &str, params: &serde_json::Value| {
            if method == "repository.query" {
                return Ok(repo_rows.borrow().clone());
            }
            *payload.borrow_mut() = params["data"].as_str().unwrap_or("").to_string();
            Ok(serde_json::json!(null))
        };
        submit_changes_to_staging_area(&dir, &options, &call, Some(VcsKind::Git), 9, "b", false)
            .unwrap();
        assert!(payload.borrow().contains(status), "{}", payload.borrow());
    }
    // A non-git client with a configured area records client.unsupported.
    *repo_rows.borrow_mut() = serde_json::json!([{
        "staging": {"supported": true, "uri": "s"},
    }]);
    let payload = std::cell::RefCell::new(String::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == "repository.query" {
            return Ok(repo_rows.borrow().clone());
        }
        *payload.borrow_mut() = params["data"].as_str().unwrap_or("").to_string();
        Ok(serde_json::json!(null))
    };
    submit_changes_to_staging_area(&dir, &options, &call, Some(VcsKind::Svn), 9, "b", false)
        .unwrap();
    let body = payload.borrow();
    assert!(body.contains("client.unsupported"), "{body}");
    assert!(is_old_server_error("ERR-CONDUIT-CALL: x"));
    assert!(!is_old_server_error("ERR-OTHER: x"));
}

#[test]
fn d13_staging_push_records_refs() {
    // Push to a local bare repo standing in for the staging area.
    let bare = scratch_dir("d13-bare");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(&bare)
            .status()
            .unwrap()
            .success()
    );
    let dir = git_dir("d13-push");
    std::fs::write(dir.join(".arcconfig"), "{\"repository.callsign\": \"X\"}").unwrap();
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(["add", "-A"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(&dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qam",
                "one"
            ])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(dir.join("f.txt"), "a\nb\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .current_dir(&dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qam",
                "two"
            ])
            .status()
            .unwrap()
            .success()
    );
    let base = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["rev-parse", "HEAD~1"])
        .output()
        .unwrap();
    let base = String::from_utf8_lossy(&base.stdout).trim().to_string();
    let uri = bare.to_string_lossy().to_string();
    let payload = std::cell::RefCell::new(String::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == "repository.query" {
            return Ok(serde_json::json!([{
                "staging": {"supported": true, "uri": uri},
            }]));
        }
        assert_eq!(method, diff_submit::METHOD_SET_DIFF_PROPERTY);
        *payload.borrow_mut() = params["data"].as_str().unwrap_or("").to_string();
        Ok(serde_json::json!(null))
    };
    let options = Options::default();
    submit_changes_to_staging_area(&dir, &options, &call, Some(VcsKind::Git), 7, &base, false)
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload.borrow()).unwrap();
    assert_eq!(payload["status"], serde_json::json!("pushed"));
    assert_eq!(payload["refs"].as_array().unwrap().len(), 2);
    let tags = std::process::Command::new("git")
        .current_dir(&bare)
        .args(["tag", "-l", "phabricator/diff/7"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&tags.stdout).trim(),
        "phabricator/diff/7"
    );
}

#[test]
fn d14_stamp_skips_raw_and_unknown_repos() {
    // Offline-safe: raw skips before any call, and a non-repo dir
    // resolves to no query without touching the network.
    let client = conduit::Client::new("http://127.0.0.1:9/", "x").unwrap();
    let plain = scratch_dir("d14-raw");
    let mut spec = serde_json::json!({"changes": []});
    stamp_repository_phid(&client, &plain, true, &mut spec).unwrap();
    assert_eq!(spec, serde_json::json!({"changes": []}));
    let outside = scratch_dir("d14-outside");
    stamp_repository_phid(&client, &outside, false, &mut spec).unwrap();
    assert_eq!(spec, serde_json::json!({"changes": []}));
}

fn d8_local_commit(hash: &str, summary: &str, message: &str) -> diff_local::LocalCommitInfo {
    diff_local::LocalCommitInfo {
        commit: hash.to_string(),
        tree: String::new(),
        parents: Vec::new(),
        time: String::new(),
        author: "t".to_string(),
        summary: summary.to_string(),
        message: message.to_string(),
        author_email: String::new(),
    }
}

#[test]
fn d8_create_message_defaults_run_before_saved_resume() {
    // DIFF-D8 (`:1398-1403`): default-field conduit pulls run even when
    // a saved template resumes, and the resume skips the server
    // template fetch.
    let root = scratch_dir("d8-saved");
    let meta = root.join(".git");
    write_scratch_file(
        &meta,
        diff_local::SCRATCH_CREATE_MESSAGE,
        "Saved title\nBody\n",
    );
    let seen = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, _: &serde_json::Value| {
        seen.borrow_mut().push(method.to_string());
        assert_ne!(
            method,
            diff_submit::METHOD_GET_COMMIT_MESSAGE,
            "DIFF-D8: saved resume must not fetch the server template"
        );
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "X"}}))
    };
    let edit = |template: &str| -> Result<String, String> {
        assert!(template.contains("Saved title"), "{template}");
        Ok("Edited title\n".to_string())
    };
    let mut reader = std::io::BufReader::new(std::io::Cursor::new(b"y\n".to_vec()));
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let local = vec![d8_local_commit("abcdef1234567890", "Subj", "Subj\n")];
    let message =
        create_message_from_user(&mut context, &defaults(), &local, Some("main")).unwrap();
    assert_eq!(message.raw_corpus, "Edited title\n");
    // One pull for the local commit plus one for the edited draft.
    assert_eq!(
        *seen.borrow(),
        vec![
            diff_submit::METHOD_PARSE_COMMIT_MESSAGE.to_string(),
            diff_submit::METHOD_PARSE_COMMIT_MESSAGE.to_string(),
        ]
    );
}

#[test]
fn d8_create_message_notes_use_head_commit_message() {
    // DIFF-D8 (`:1411-1412`): parse notes switch the template to the
    // head commit message instead of the server template.
    let root = scratch_dir("d8-notes");
    let meta = root.join(".git");
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({
            "fields": {"title": "Head subject"},
            "errors": ["unknown reviewer robert"],
        }))
    };
    let captured = std::cell::RefCell::new(String::new());
    let edit = |template: &str| -> Result<String, String> {
        *captured.borrow_mut() = template.to_string();
        Ok("Fixed title\n".to_string())
    };
    // The re-edit pull succeeds so the fixed draft validates.
    let fixed = std::cell::RefCell::new(false);
    let call = |method: &str, params: &serde_json::Value| {
        if *fixed.borrow() {
            assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
            return Ok(serde_json::json!({"fields": {"title": "Fixed title"}}));
        }
        *fixed.borrow_mut() = true;
        call(method, params)
    };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let local = vec![d8_local_commit(
        "abcdef1234567890",
        "Head subject",
        "Head subject\n\nBody text\n",
    )];
    let message =
        create_message_from_user(&mut context, &defaults(), &local, Some("main")).unwrap();
    assert_eq!(message.raw_corpus, "Fixed title\n");
    let template = captured.borrow().clone();
    assert!(
        template.starts_with("Head subject\n\nBody text\n\n# NEW DIFFERENTIAL REVISION"),
        "{template}"
    );
}

#[test]
fn d8_create_message_included_block_names_branch() {
    // DIFF-D8 (`:1428-1448`): the editor template wraps included
    // commits with the branch header.
    let root = scratch_dir("d8-included");
    let meta = root.join(".git");
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            return Ok(serde_json::json!("Server template\n"));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "T"}}))
    };
    let captured = std::cell::RefCell::new(String::new());
    let edit = |template: &str| -> Result<String, String> {
        *captured.borrow_mut() = template.to_string();
        Ok("Final title\n".to_string())
    };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let local = vec![d8_local_commit("abcdef1234567890", "Subj", "Subj\n")];
    let message =
        create_message_from_user(&mut context, &defaults(), &local, Some("main")).unwrap();
    assert_eq!(message.raw_corpus, "Final title\n");
    let template = captured.borrow().clone();
    assert!(
        template.contains("# Included commits in branch main:"),
        "{template}"
    );
    assert!(template.contains("abcdef123456 Subj"), "{template}");
}

#[test]
fn d8_bare_detection_creates_updates_or_chooses() {
    // DIFF-D8 (`:1303-1334`): no open revision creates, exactly one
    // updates, several raise the chooser error.
    let dir = git_dir("d8-detect");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    };
    run(&["commit", "--allow-empty", "-qm", "first"]);
    let base = git::stdout_text(&dir, &["rev-parse", "HEAD"]).unwrap();
    // No footers and no open revisions: create.
    run(&["commit", "--allow-empty", "-qm", "second"]);
    let queried = std::cell::RefCell::new(false);
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_QUERY);
        assert!(params.get("commitHashes").is_some());
        *queried.borrow_mut() = true;
        Ok(serde_json::json!([]))
    };
    let mode = resolve_bare_submit_mode(
        &call,
        &dir,
        &hg::HgCommand::default(),
        Some(VcsKind::Git),
        base.trim(),
        Some("HEAD"),
        "PHID-USER-1",
    )
    .unwrap();
    assert_eq!(mode, SubmitMode::Create);
    assert!(*queried.borrow());

    // A footer id plus one open revision: update. Two rows: chooser.
    run(&[
        "commit",
        "--allow-empty",
        "-qm",
        "third",
        "-m",
        "Differential Revision: D7",
    ]);
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_QUERY);
        assert_eq!(params["ids"], serde_json::json!([7]));
        Ok(serde_json::json!([{"id": 7, "title": "Seven"}]))
    };
    let mode = resolve_bare_submit_mode(
        &call,
        &dir,
        &hg::HgCommand::default(),
        Some(VcsKind::Git),
        base.trim(),
        Some("HEAD"),
        "PHID-USER-1",
    )
    .unwrap();
    assert_eq!(mode, SubmitMode::Update(7));
    let call = |_: &str, _: &serde_json::Value| {
        Ok(serde_json::json!([
            {"id": 7, "title": "Seven"},
            {"id": 8, "title": "Eight"},
        ]))
    };
    let err = resolve_bare_submit_mode(
        &call,
        &dir,
        &hg::HgCommand::default(),
        Some(VcsKind::Git),
        base.trim(),
        Some("HEAD"),
        "PHID-USER-1",
    )
    .unwrap_err();
    assert!(err.contains("several revisions"), "{err}");
    assert!(err.contains("D7: Seven"), "{err}");
    assert!(err.contains("'--update'"), "{err}");
    assert!(err.contains("'--create'"), "{err}");
}

#[test]
fn d8_footer_target_skips_ownership_uses_edit_template() {
    // DIFF-D8 (`:667-670`): footer redirects skip the ownership query
    // and seed the `edit` template with the pulled fields.
    let meta = scratch_dir("d8-footer");
    let use_fields = serde_json::json!({"title": "Pulled"});
    let call = |method: &str, params: &serde_json::Value| {
        assert_ne!(method, diff_submit::METHOD_QUERY);
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            assert_eq!(params["edit"], serde_json::json!("edit"));
            assert_eq!(params["revision_id"], serde_json::json!(7));
            assert_eq!(params["fields"], serde_json::json!({"title": "Pulled"}));
            return Ok(serde_json::json!("Seven\n\nSummary: s\n"));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "Seven"}}))
    };
    let edit = |_: &str, _: &str| -> Result<String, String> { panic!("DIFF-D8: no editor") };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let mut options = update_test_options(Some("m"), false);
    options.update = None;
    let target = UpdateTarget {
        revision_id: 7,
        revision_arg: "7",
        verbatim_fields: &use_fields,
        check_ownership: false,
        edit_fields: Some(&use_fields),
    };
    let resolved = resolve_update_revision(&mut resolver, &options, &target).unwrap();
    assert_eq!(resolved.fields, serde_json::json!({"title": "Seven"}));
    assert_eq!(resolved.message, "m");
}

#[test]
fn d8_bare_verbatim_skips_detection() {
    // DIFF-D8 (`:1298-1300`): bare `--verbatim` always creates.
    assert!(!bare_submit_skips_detection(&defaults()));
    let verbatim = Options {
        verbatim: true,
        ..defaults()
    };
    assert!(bare_submit_skips_detection(&verbatim));
}

#[test]
fn d8_resolve_create_message_file_returns_footer_redirect() {
    // DIFF-D8 (`:1357-1373`): `--message-file` validates without
    // editing; a footer revision id redirects to that update.
    let root = scratch_dir("d8-msgfile");
    let path = root.join("msg.txt");
    std::fs::write(&path, "File title\n\nDifferential Revision: D9\n").unwrap();
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_QUERY {
            return Ok(serde_json::json!([{
                "id": 9, "title": "Nine", "authorPHID": "PHID-USER-1",
            }]));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "File title"}}))
    };
    let edit = |_: &str| -> Result<String, String> { panic!("DIFF-D8: no edit") };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &root,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let options = Options {
        message_file: Some(path.to_string_lossy().into_owned()),
        ..defaults()
    };
    let resolved = resolve_create_message(&mut context, &options, &[], Some("main")).unwrap();
    assert_eq!(resolved.corpus, "File title\n\nDifferential Revision: D9\n");
    assert_eq!(resolved.footer_revision_id, Some(9));
}
#[test]
fn r10_diff_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        help_text(false),
        "DIFF WORKFLOW\n\
         \n\
         diff\n\
         \x20     diff [paths] (svn)\n\
         \x20     diff [commit] (git, hg)\n\
         \n\
         \x20         Supports: git, svn, hg\n\
         \x20         Generate a Differential diff or revision from local changes.\n\
         \n\
         \x20         Under git and mercurial, you can specify a commit (like HEAD^^^\n\
         \x20         or master) and Differential will generate a diff against the\n\
         \x20         merge base of that commit and your current working directory\n\
         parent.\n\
         \n\
         \x20         Under svn, you can choose to include only some of the modified\n\
         files\n\
         \x20         in the working copy in the diff by specifying their paths. If you\n\
         \x20         omit paths, all changes are included in the diff.\n\
         \n\
         \x20     --add-all, -a\n\
         \x20         Automatically add all unstaged and uncommitted files to the commit.\n\
         \n\
         \x20     --allow-untracked\n\
         \x20         Skip checks for untracked files in the working copy.\n\
         \n\
         \x20     --amend-all\n\
         \x20         When linting git repositories, amend HEAD with all patches\n\
         \x20         suggested by lint without prompting.\n\
         \n\
         \x20     --amend-autofixes\n\
         \x20         When linting git repositories, amend HEAD with autofix patches\n\
         \x20         suggested by lint without prompting.\n\
         \n\
         \x20     --apply-patches\n\
         \x20         Apply patches suggested by lint to the working copy without\n\
         \x20         prompting.\n\
         \n\
         \x20     --base rules\n\
         \x20         Additional rules for determining base revision.\n\
         \n\
         \x20     --browse\n\
         \x20         After creating a diff or revision, open it in a web browser.\n\
         \n\
         \x20     --cc usernames\n\
         \x20         When creating a revision, add CCs.\n\
         \n\
         \x20     --coverage\n\
         \x20         Always enable coverage information.\n\
         \n\
         \x20     --create\n\
         \x20         Always create a new revision.\n\
         \n\
         \x20     --draft\n\
         \x20         Create a draft revision so you can look over your changes before\n\
         \x20         involving anyone else. Other users will not be notified about the\n\
         \x20         revision until you later use \"Request Review\" to publish it. You\n\
         \x20         can still share the draft by giving someone the link.\n\
         \n\
         \x20     --edit\n\
         \x20         When updating a revision under git, edit revision information\n\
         \x20         before updating.\n\
         \n\
         \x20     --head commit\n\
         \x20         Specify the end of the commit range. This disables many\n\
         \x20         Arcanist/Phabricator features which depend on having access to the\n\
         \x20         working copy.\n\
         \n\
         \x20     --json\n\
         \x20         Emit machine-readable JSON. EXPERIMENTAL! Probably does not work!\n\
         \n\
         \x20     --message message, -m message\n\
         \x20         When updating a revision, use the specified message instead of\n\
         \x20         prompting.\n\
         \n\
         \x20     --message-file file, -F file\n\
         \x20         When creating a revision, read revision information from this file.\n\
         \n\
         \x20     --never-apply-patches\n\
         \x20         Never apply patches suggested by lint.\n\
         \n\
         \x20     --no-amend\n\
         \x20         Never amend commits in the working copy with lint patches.\n\
         \n\
         \x20     --no-coverage\n\
         \x20         Always disable coverage information.\n\
         \n\
         \x20     --nolint\n\
         \x20         Do not run lint.\n\
         \n\
         \x20     --nounit\n\
         \x20         Do not run unit tests.\n\
         \n\
         \x20     --only\n\
         \x20         Instead of creating or updating a revision, only create a diff,\n\
         \x20         which you may later attach to a revision.\n\
         \n\
         \x20     --raw\n\
         \x20         Read diff from stdin, not from the working copy. This disables many\n\
         \x20         Arcanist/Phabricator features which depend on having access to the\n\
         \x20         working copy.\n\
         \n\
         \x20     --raw-command command\n\
         \x20         Generate diff by executing a specified command, not from the\n\
         \x20         working copy. This disables many Arcanist/Phabricator features\n\
         \x20         which depend on having access to the working copy.\n\
         \n\
         \x20     --reviewers usernames\n\
         \x20         When creating a revision, add reviewers.\n\
         \n\
         \x20     --skip-binaries\n\
         \x20         Do not upload binaries (like images).\n\
         \n\
         \x20     --skip-staging\n\
         \x20         Do not copy changes to the staging area.\n\
         \n\
         \x20     --uncommitted\n\
         \x20         Suppress warning about uncommitted changes.\n\
         \n\
         \x20     --update revision_id\n\
         \x20         Always update a specific revision.\n\
         \n\
         \x20     --verbatim\n\
         \x20         When creating a revision, try to use the working copy commit\n\
         \x20         message verbatim, without prompting to edit it. When updating a\n\
         \x20         revision, update some fields from the local commit message.\n\
         \n\
         \n"
    );
    let ansi = help_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints the same text with ambient ANSI mode.
    assert!(help_text(crate::terminal::ansi_enabled()).contains("DIFF WORKFLOW"));
}

#[test]
fn b21_diff_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `diff`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_options(&args(&["--verbatim"])).unwrap().is_some());
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    let text = help_text(false);
    assert!(text.contains("DIFF WORKFLOW"), "{text}");
}

#[test]
fn b23_diff_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help diff`.
    let text = help_text(false);
    assert!(text.starts_with("DIFF WORKFLOW\n"), "{text}");
    assert!(text.contains("--reviewers"), "{text}");
    assert!(text.contains("--base"), "{text}");
    let ansi = help_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

/* T1170 coverage: git ranges, pure helpers, scratch metadata. */

fn t1170_commit(dir: &Path, message: &str) {
    let add = std::process::Command::new("git")
        .current_dir(dir)
        .args(["add", "-A"])
        .status()
        .unwrap();
    assert!(add.success());
    let commit = std::process::Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qam",
            message,
        ])
        .status()
        .unwrap();
    assert!(commit.success());
}

fn t1170_rev(dir: &Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn t1170_three_commit_repo(tag: &str) -> PathBuf {
    let dir = git_dir(tag);
    for (file, body, message) in [
        ("a.txt", "a\n", "one"),
        ("b.txt", "b\n", "two"),
        ("c.txt", "c\n", "three"),
    ] {
        std::fs::write(dir.join(file), body).unwrap();
        t1170_commit(&dir, message);
    }
    dir
}

#[cfg(unix)]
fn t1170_make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn t1170_resolve_base_explicit_and_default() {
    let dir = git_dir("t1170-base");
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    t1170_commit(&dir, "one");
    let head = t1170_rev(&dir, "HEAD");
    assert_eq!(resolve_base(&dir, None).unwrap(), head);
    assert_eq!(resolve_base(&dir, Some("HEAD")).unwrap(), head);
    let err = resolve_base(&dir, Some("no-such-rev-t1170")).unwrap_err();
    assert_eq!(err, "invalid base revision 'no-such-rev-t1170'");
}

#[test]
fn t1170_numstat_range_reports_git_failure() {
    let dir = git_dir("t1170-numstat");
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    t1170_commit(&dir, "one");
    let err = numstat_range(&dir, "no-such-rev-t1170", None).unwrap_err();
    assert!(err.starts_with("git diff --numstat failed:"), "{err}");
    let head = t1170_rev(&dir, "HEAD");
    assert_eq!(numstat_range(&dir, &head, None).unwrap(), vec![]);
}

#[test]
fn t1170_default_diff_output_range_head_and_bad_base() {
    let dir = t1170_three_commit_repo("t1170-output");
    let base = t1170_rev(&dir, "HEAD~1");
    let out = default_diff_output_range(&dir, &base, Some("HEAD")).unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("c.txt"), "{text}");
    assert!(text.contains("diff --git"), "{text}");
    let err = default_diff_output_range(&dir, "no-such-rev-t1170", None).unwrap_err();
    assert!(err.starts_with("git diff failed:"), "{err}");
}

#[test]
fn t1170_submit_changes_with_hunks_head_and_bad_base() {
    let dir = t1170_three_commit_repo("t1170-hunks");
    let base = t1170_rev(&dir, "HEAD~1");
    let entries = diff_local::collect_change_entries(&dir, &base, Some("HEAD")).unwrap();
    assert!(!entries.is_empty());
    let changes = submit_changes_with_hunks(&dir, &base, Some("HEAD"), &entries).unwrap();
    assert_eq!(changes.len(), entries.len());
    assert!(changes.iter().any(|c| c.current_path == "c.txt"));
    let err = submit_changes_with_hunks(&dir, "no-such-rev-t1170", None, &entries).unwrap_err();
    assert!(err.starts_with("git diff failed:"), "{err}");
}

#[test]
fn t1170_set_child_flags_splits_lint_unit() {
    let options = Options {
        apply_patches: true,
        amend_autofixes: true,
        coverage: true,
        ..Options::default()
    };
    let (lint, unit) = set_child_flags(&options);
    assert_eq!(
        lint,
        vec!["apply-patches".to_string(), "amend-autofixes".to_string()]
    );
    assert_eq!(unit, vec!["coverage".to_string()]);
    let (lint, unit) = set_child_flags(&Options::default());
    assert!(lint.is_empty() && unit.is_empty());
}

#[test]
fn t1170_standard_update_target_display_arg() {
    let fields = serde_json::json!({"title": "T"});
    let mut display = String::new();
    let options = Options {
        update: Some("D5".to_string()),
        ..Options::default()
    };
    {
        let target = standard_update_target(&options, &fields, 5, &mut display);
        assert_eq!(target.revision_id, 5);
        assert_eq!(target.revision_arg, "5");
        assert!(target.check_ownership);
        assert_eq!(target.edit_fields, None);
        assert_eq!(target.verbatim_fields, &fields);
    }
    let options = Options::default();
    let target = standard_update_target(&options, &fields, 7, &mut display);
    assert_eq!(target.revision_arg, "7");
}

#[test]
fn t1170_select_remote_corpus_matrix() {
    assert_eq!(
        select_remote_corpus("base", Some("cached".to_string()), true, false),
        "cached"
    );
    assert_eq!(
        select_remote_corpus("base", Some("cached".to_string()), false, false),
        "base"
    );
    assert_eq!(select_remote_corpus("base", None, true, false), "base");
    assert_eq!(
        select_remote_corpus("base", Some("cached".to_string()), true, true),
        "base"
    );
}

#[test]
fn t1170_update_message_title_kinds() {
    let mut message = diff_local::CommitMessage::new_from_raw_corpus("Title here\n").unwrap();
    assert_eq!(update_message_title(&message), "");
    message.fields.insert(
        "title".to_string(),
        diff_local::FieldValue::Text("Pulled".to_string()),
    );
    assert_eq!(update_message_title(&message), "Pulled");
    message.fields.insert(
        "title".to_string(),
        diff_local::FieldValue::List(vec!["a".to_string()]),
    );
    assert_eq!(update_message_title(&message), "");
}

#[test]
fn t1170_join_create_corpus_blank_is_none() {
    assert_eq!(join_create_corpus(&["  \n ".to_string()], None, None), None);
    assert_eq!(join_create_corpus(&[], None, None), None);
}

#[test]
fn t1170_revision_uri_from_diff_uri_fallback() {
    assert_eq!(revision_uri_from_diff_uri("not-a-uri", 5), "/D5");
    assert_eq!(revision_uri_from_diff_uri("https://", 5), "/D5");
    assert_eq!(
        revision_uri_from_diff_uri("https://phab.test/D3", 9),
        "https://phab.test/D9"
    );
}

#[test]
fn t1170_update_scratch_non_object_and_clear() {
    let meta = scratch_dir("t1170-uscratch");
    write_scratch_json(&meta, SCRATCH_EDIT_MESSAGES, &serde_json::json!([1, 2]));
    assert_eq!(
        read_update_scratch(&meta, SCRATCH_EDIT_MESSAGES),
        serde_json::json!({})
    );
    for name in [SCRATCH_EDIT_MESSAGES, SCRATCH_UPDATE_MESSAGES] {
        write_scratch_json(&meta, name, &serde_json::json!({"5": "old", "6": "keep"}));
    }
    clear_update_scratch(&meta, 5);
    for name in [SCRATCH_EDIT_MESSAGES, SCRATCH_UPDATE_MESSAGES] {
        assert_eq!(
            read_update_scratch(&meta, name),
            serde_json::json!({"6": "keep"})
        );
    }
    clear_update_scratch(&meta, 99);
}

#[test]
fn t1170_update_metadata_dir_per_vcs() {
    let git = git_dir("t1170-meta");
    assert_eq!(update_metadata_dir(&git, None).unwrap(), git.join(".git"));
    assert_eq!(
        update_metadata_dir(&git, Some(VcsKind::Git)).unwrap(),
        git.join(".git")
    );
    let hg = scratch_dir("t1170-meta-hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert_eq!(update_metadata_dir(&hg, None).unwrap(), hg.join(".hg"));
    let svn = scratch_dir("t1170-meta-svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert_eq!(update_metadata_dir(&svn, None).unwrap(), svn.join(".svn"));
    let plain = scratch_dir("t1170-meta-plain");
    assert_eq!(
        update_metadata_dir(&plain, None).unwrap_err(),
        "not inside a git repository"
    );
}

#[test]
fn t1170_git_metadata_dir_relative_and_worktree() {
    let dir = git_dir("t1170-gitdir");
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    t1170_commit(&dir, "one");
    assert_eq!(git_metadata_dir(&dir).unwrap(), dir.join(".git"));
    assert_eq!(
        git_metadata_dir(&scratch_dir("t1170-gitdir-plain")).unwrap_err(),
        "not inside a git repository"
    );
    // Linked worktrees resolve to an absolute git dir.
    let wt = std::env::temp_dir().join(format!("arc-rs-diff-wt-{}", std::process::id()));
    let added = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["worktree", "add", &wt.to_string_lossy()])
        .status()
        .unwrap();
    assert!(added.success());
    let resolved = git_metadata_dir(&wt).unwrap();
    assert!(resolved.is_absolute(), "{}", resolved.display());
}

#[test]
fn t1170_history_default_immutable_per_vcs() {
    assert!(!history_default_immutable(&git_dir("t1170-imm-git")));
    let hg = scratch_dir("t1170-imm-hg");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert!(history_default_immutable(&hg));
    let svn = scratch_dir("t1170-imm-svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert!(history_default_immutable(&svn));
    assert!(!history_default_immutable(&scratch_dir("t1170-imm-plain")));
}

#[test]
fn t1170_scratch_write_failures_return_false() {
    let dir = scratch_dir("t1170-scratchfail");
    let file = dir.join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    assert!(!write_scratch_file(&file, "create-message", "hello"));
    assert!(!write_scratch_json(
        &file,
        "data",
        &serde_json::json!({"a": 1})
    ));
}

#[test]
fn t1170_readable_scratch_with_curdir_root() {
    let root = scratch_dir("t1170-curdir");
    let meta = root.join(".git");
    let expected = Path::new(".git")
        .join("arc")
        .join("create-message")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        readable_scratch_file_path(&meta, &root.join("."), "create-message"),
        expected
    );
}

/* T1170 coverage: update resolution, properties, raw sources, submit-over. */

#[test]
fn t1170_update_uses_cached_template_on_second_run() {
    let meta = scratch_dir("t1170-d9cache");
    write_scratch_json(
        &meta,
        SCRATCH_UPDATE_MESSAGES,
        &serde_json::json!({"5": "cached template"}),
    );
    let call = update_stub_call();
    let seen = std::cell::RefCell::new(Vec::new());
    let edit = |template: &str, name: &str| -> Result<String, String> {
        seen.borrow_mut()
            .push((template.to_string(), name.to_string()));
        Ok("final words\n".to_string())
    };
    let prefill = || -> Result<Option<String>, String> { panic!("T1170: no prefill") };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(None, false);
    let resolved = resolve_update_revision(
        &mut resolver,
        &options,
        &update_test_target(&serde_json::json!({})),
    )
    .unwrap();
    assert_eq!(resolved.message, "final words\n");
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "cached template");
    assert_eq!(seen[0].1, EDITOR_DIFF_UPDATE);
}

#[test]
fn t1170_maybe_apply_full_update_non_update_skips() {
    let call = t1170_no_conduit;
    let dir = scratch_dir("t1170-noupdate");
    let options = Options::default();
    let mut input = create_test_input(false, None);
    assert_eq!(
        maybe_apply_full_update(&dir, &options, &call, Some(VcsKind::Git), &mut input).unwrap(),
        None
    );
    input.mode = SubmitMode::Only;
    assert_eq!(
        maybe_apply_full_update(&dir, &options, &call, Some(VcsKind::Git), &mut input).unwrap(),
        None
    );
}

#[test]
fn t1170_lint_unit_properties_emit_only_nonempty() {
    assert!(
        lint_unit_properties(&diff_local::LintUnitReport::empty())
            .unwrap()
            .is_empty()
    );
    let lint_only = diff_local::LintUnitReport {
        lint_result: Some(1),
        unresolved_lint: vec![serde_json::json!({"path": "f"})],
        unit_result: None,
        test_results: Vec::new(),
    };
    let props = lint_unit_properties(&lint_only).unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].name, diff_submit::PROP_LINT);
    assert!(props[0].data.contains("\"path\""), "{}", props[0].data);
    let unit_only = diff_local::LintUnitReport {
        lint_result: None,
        unresolved_lint: Vec::new(),
        unit_result: Some(0),
        test_results: vec![serde_json::json!({"name": "t"})],
    };
    let props = lint_unit_properties(&unit_only).unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].name, diff_submit::PROP_UNIT);
}

#[test]
fn t1170_diff_properties_onto_needs_upstream() {
    // A plain repo reports local commits but no onto targets.
    let dir = t1170_three_commit_repo("t1170-props");
    let base = t1170_rev(&dir, "HEAD~1");
    let props = diff_properties(&dir, &base, Some("HEAD")).unwrap();
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec![diff_submit::PROP_LOCAL_COMMITS]);
    // A clone tracks its origin, so onto targets appear.
    let origin = t1170_three_commit_repo("t1170-origin");
    let clone = std::env::temp_dir().join(format!("arc-rs-diff-clone-{}", std::process::id()));
    let cloned = std::process::Command::new("git")
        .args(["clone", "-q"])
        .arg(&origin)
        .arg(&clone)
        .status()
        .unwrap();
    assert!(cloned.success());
    let base = t1170_rev(&clone, "HEAD~1");
    let props = diff_properties(&clone, &base, Some("HEAD")).unwrap();
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec![diff_submit::PROP_LOCAL_COMMITS, diff_submit::PROP_ONTO]
    );
}

#[test]
fn t1170_raw_command_feeds_submit_entries() {
    let dir = scratch_dir("t1170-raw");
    let text = "diff --git a/f.txt b/f.txt\nnew file mode 100644\nindex 0000000..257cc56\n--- /dev/null\n+++ b/f.txt\n@@ -0,0 +1 @@\n+hi\n";
    let file = dir.join("raw.diff");
    std::fs::write(&file, text).unwrap();
    let cat = if cfg!(windows) { "type" } else { "cat" };
    let options = Options {
        raw_command: Some(format!("{cat} \"{}\"", file.to_string_lossy())),
        ..Options::default()
    };
    let entries = submit_raw_entries(&options).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].current.path, "f.txt");
    // The same source flows through the full collector and prepare path.
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let changes = collect_submit_changes_with_io(
        &dir,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        &options,
        "",
        None,
        &mut io,
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let input = prepare_submit_full_with_commands_io(
        &dir,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        &options,
        &SubmitMode::Only,
        &diff_local::LintUnitReport::empty(),
        &mut io,
    )
    .unwrap();
    assert!(
        input.included_changes.contains("f.txt"),
        "{}",
        input.included_changes
    );
}

#[test]
fn t1170_lint_unit_base_matrix() {
    let dir = scratch_dir("t1170-lub");
    let raw = Options {
        raw: true,
        ..Options::default()
    };
    assert_eq!(
        lint_unit_base(&dir, &raw, Some(VcsKind::Git), true).unwrap(),
        (None, None)
    );
    let plain = Options::default();
    assert_eq!(
        lint_unit_base(&dir, &plain, Some(VcsKind::Svn), false).unwrap(),
        (None, None)
    );
    assert_eq!(
        lint_unit_base(&dir, &plain, None, false).unwrap(),
        (None, None)
    );
    let git = t1170_three_commit_repo("t1170-lub-git");
    let head = t1170_rev(&git, "HEAD");
    assert_eq!(
        lint_unit_base(&git, &plain, Some(VcsKind::Git), false).unwrap(),
        (Some(head), None)
    );
    // Outside a working copy the report stays empty without children.
    let report = full_lint_unit_report(&dir, &plain, None, false, false).unwrap();
    assert_eq!(report.lint_result, None);
    assert_eq!(report.unit_result, None);
    assert!(report.unresolved_lint.is_empty() && report.test_results.is_empty());
}

#[test]
fn t1170_open_uri_in_browser_success_and_failure() {
    #[cfg(not(windows))]
    {
        assert!(open_uri_in_browser("https://example.test/D1", &["true".to_string()]).is_ok());
        let err =
            open_uri_in_browser("https://example.test/D1", &["false".to_string()]).unwrap_err();
        assert!(err.contains("https://example.test/D1"), "{err}");
    }
    let err = open_uri_in_browser(
        "https://example.test/D1",
        &["definitely-not-a-browser-t1170".to_string()],
    )
    .unwrap_err();
    assert!(err.contains("browser"), "{err}");
}

#[test]
fn t1170_binary_exists_on_path_probes_path() {
    // Hermetic: probe a scratch dir standing in for one PATH entry —
    // never a system binary. (`git` resolves via PATHEXT on Windows,
    // which the probe intentionally skips: prod never calls it there,
    // Windows takes `cmd /c start` without probing.)
    let dir = scratch_dir("t1170-binprobe");
    let probe = dir.join("probe-t1170");
    std::fs::write(&probe, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&probe).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&probe, perms).unwrap();
    }
    assert!(binary_exists_in_dirs(
        "probe-t1170",
        std::slice::from_ref(&dir)
    ));
    assert!(!binary_exists_in_dirs(
        "definitely-not-a-binary-t1170",
        std::slice::from_ref(&dir)
    ));
    assert!(!binary_exists_on_path("definitely-not-a-binary-t1170"));
}

#[test]
fn t1170_submit_over_with_browse_reports_then_errors() {
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_CREATE_DIFF);
        Ok(serde_json::json!({"diffid": 3, "uri": "https://phab.test/diff/3/"}))
    };
    let mut input = create_test_input(false, None);
    input.mode = SubmitMode::Only;
    input.fields = serde_json::Value::Null;
    // A git root: the project file only counts at a working-copy root, and
    // it pins a missing browser so the user config cannot leak in.
    let dir = git_dir("t1170-browse");
    std::fs::write(
        dir.join(".arcconfig"),
        "{\"browser\": \"definitely-not-a-browser-t1170\"}",
    )
    .unwrap();
    let report =
        submit_over_with_browse_ansi(&dir, &Options::default(), &call, &input, false).unwrap();
    assert!(report.text.contains("Differential diff"), "{}", report.text);
    assert_eq!(report.diff_id, 3);
    let options = Options {
        browse: true,
        ..Options::default()
    };
    let err = submit_over_with_browse_ansi(&dir, &options, &call, &input, false)
        .err()
        .expect("T1170: missing browser must fail the submit");
    assert!(err.contains("https://phab.test/diff/3/"), "{err}");
    assert!(err.contains("definitely-not-a-browser-t1170"), "{err}");
}

#[test]
fn t1170_submit_over_and_stage_records_skip() {
    let recorded = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(serde_json::json!({"diffid": 3, "uri": "https://phab.test/diff/3/"}));
        }
        assert_eq!(method, diff_submit::METHOD_SET_DIFF_PROPERTY);
        recorded
            .borrow_mut()
            .push(params["name"].as_str().unwrap_or("").to_string());
        Ok(serde_json::json!(null))
    };
    let mut input = create_test_input(false, None);
    input.mode = SubmitMode::Only;
    input.fields = serde_json::Value::Null;
    input.spec = serde_json::json!({"sourceControlBaseRevision": "abc"});
    let options = Options {
        skip_staging: true,
        ..Options::default()
    };
    let report = submit_over_and_stage(
        &scratch_dir("t1170-stage"),
        &options,
        &call,
        Some(VcsKind::Git),
        &input,
        false,
    )
    .unwrap();
    assert_eq!(report.diff_id, 3);
    assert_eq!(
        *recorded.borrow(),
        vec![diff_submit::PROP_STAGING.to_string()]
    );
}

#[test]
fn t1170_finalize_client_submit_create_and_update() {
    let dir = git_dir("t1170-final");
    let fetched = std::cell::Cell::new(false);
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        fetched.set(true);
        Ok(serde_json::json!("revised\n"))
    };
    // Create with --no-amend fetches but rewrites nothing.
    let options = Options {
        no_amend: true,
        ..Options::default()
    };
    finalize_client_submit(
        &dir,
        &hg::HgCommand::default(),
        &options,
        &call,
        Some(VcsKind::Git),
        &SubmitMode::Create,
        Some(7),
        None,
    )
    .unwrap();
    assert!(fetched.get());
    // Update clears that revision's cached messages.
    let meta = update_metadata_dir(&dir, Some(VcsKind::Git)).unwrap();
    write_scratch_json(
        &meta,
        SCRATCH_UPDATE_MESSAGES,
        &serde_json::json!({"5": "stale"}),
    );
    finalize_client_submit(
        &dir,
        &hg::HgCommand::default(),
        &Options::default(),
        &call,
        Some(VcsKind::Git),
        &SubmitMode::Update(5),
        None,
        Some(5),
    )
    .unwrap();
    assert_eq!(
        read_update_scratch(&meta, SCRATCH_UPDATE_MESSAGES),
        serde_json::json!({})
    );
}

#[test]
fn t1170_submit_redirect_passthrough() {
    let call = t1170_no_conduit;
    let options = Options::default();
    let dir = scratch_dir("t1170-redir");
    let redirect =
        resolve_submit_create_redirect(&dir, &options, &call, &SubmitMode::Update(5), "abc", None)
            .unwrap();
    assert_eq!(redirect.mode, SubmitMode::Update(5));
    assert!(redirect.corpus.is_none() && redirect.footer.is_none());
    let mut input = create_test_input(false, None);
    assert!(!apply_footer_redirect(&dir, &options, &call, &mut input, &None).unwrap());
}

/* T1170 coverage: amend, staging, repository queries, hg/svn seams. */

#[test]
fn t1170_amend_after_create_unsupported_kind_noops() {
    let dir = scratch_dir("t1170-amendnone");
    // Pin mutability regardless of ambient user config.
    std::fs::write(dir.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    let fetched = std::cell::Cell::new(0);
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        fetched.set(fetched.get() + 1);
        Ok(serde_json::json!("revised\n"))
    };
    for kind in [None, Some(VcsKind::Svn)] {
        amend_after_create(
            &dir,
            &hg::HgCommand::default(),
            &Options::default(),
            &call,
            7,
            kind,
        )
        .unwrap();
    }
    // Hg with an unusable binary fails closed to unsupported.
    let bogus_hg = hg::HgCommand::custom("definitely-not-hg-t1170", &[]);
    amend_after_create(
        &dir,
        &bogus_hg,
        &Options::default(),
        &call,
        7,
        Some(VcsKind::Hg),
    )
    .unwrap();
    // Every run fetches like PHP, then no-ops on the unsupported kind.
    assert_eq!(fetched.get(), 3);
}

#[test]
fn t1170_amend_worktree_message_git_empty_repo_errors() {
    let dir = git_dir("t1170-amendempty");
    let err = amend_worktree_message_git(&dir, "msg").unwrap_err();
    assert!(err.starts_with("git amend failed:"), "{err}");
}

#[test]
fn t1170_push_staging_refs_failed_push_errors() {
    let dir = git_dir("t1170-pushfail");
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    t1170_commit(&dir, "one");
    let base = t1170_rev(&dir, "HEAD");
    // A failed push records nothing: any record attempt fails the test.
    let record = |_: &str, _: &[serde_json::Value]| panic!("T1170: no record on failure");
    let bogus = dir.join("no-such-remote-t1170");
    let err = push_staging_refs(
        &dir,
        &bogus.to_string_lossy(),
        "phabricator",
        7,
        &base,
        false,
        &record,
    )
    .unwrap_err();
    assert!(err.contains("--skip-staging"), "{err}");
    // The ANSI notice path reports the same failure.
    let err = push_staging_refs(
        &dir,
        &bogus.to_string_lossy(),
        "phabricator",
        7,
        &base,
        true,
        &record,
    )
    .unwrap_err();
    assert!(err.contains("--skip-staging"), "{err}");
}

#[test]
fn t1170_push_staging_refs_lfs_pushes_forced_diff_tag() {
    // A bare repo stands in for the staging area; `.gitattributes` marks
    // the worktree LFS without needing the git-lfs binary.
    let bare = scratch_dir("t1170-lfs-bare");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(&bare)
            .status()
            .unwrap()
            .success()
    );
    let dir = git_dir("t1170-lfs-push");
    std::fs::write(dir.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
    std::fs::write(dir.join("a.bin"), "lfs?\n").unwrap();
    t1170_commit(&dir, "one");
    assert!(git::is_git_lfs_working_copy(&dir));
    let base = t1170_rev(&dir, "HEAD");
    let recorded = std::cell::RefCell::new(Vec::new());
    let record = |status: &str, refs: &[serde_json::Value]| {
        recorded.borrow_mut().push(status.to_string());
        assert_eq!(refs.len(), 2);
        Ok(())
    };
    push_staging_refs(
        &dir,
        &bare.to_string_lossy(),
        "phabricator",
        7,
        &base,
        false,
        &record,
    )
    .unwrap();
    assert_eq!(
        *recorded.borrow(),
        vec![diff_submit::STAGING_PUSHED.to_string()]
    );
    let tags = std::process::Command::new("git")
        .current_dir(&bare)
        .args(["tag", "-l", "phabricator/diff/7"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&tags.stdout).trim(),
        "phabricator/diff/7"
    );
}

#[test]
fn t1170_run_unique_repository_query_matrix() {
    let query = serde_json::json!({"callsigns": ["X"]});
    let old = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("ERR-CONDUIT-CALL: gone".to_string())
    };
    assert_eq!(run_unique_repository_query(&old, &query).unwrap(), None);
    let other = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("boom".to_string())
    };
    assert_eq!(
        run_unique_repository_query(&other, &query).unwrap_err(),
        "boom"
    );
    for rows in [
        serde_json::json!([]),
        serde_json::json!([{"a": 1}, {"a": 2}]),
    ] {
        let call = |_: &str, _: &serde_json::Value| Ok(rows.clone());
        assert_eq!(run_unique_repository_query(&call, &query).unwrap(), None);
    }
    let one = |_: &str, _: &serde_json::Value| Ok(serde_json::json!([{"id": 1}]));
    assert_eq!(
        run_unique_repository_query(&one, &query).unwrap(),
        Some(serde_json::json!({"id": 1}))
    );
}

#[test]
fn t1170_vcs_repository_query_git_remote_and_missing_binaries() {
    let dir = git_dir("t1170-vcsq");
    assert_eq!(
        vcs_repository_query(&dir, Some(VcsKind::Git)).unwrap(),
        None
    );
    let added = std::process::Command::new("git")
        .current_dir(&dir)
        .args([
            "remote",
            "add",
            "origin",
            "https://phab.test/diffusion/X.git",
        ])
        .status()
        .unwrap();
    assert!(added.success());
    assert_eq!(
        vcs_repository_query(&dir, Some(VcsKind::Git)).unwrap(),
        Some(serde_json::json!({"remoteURIs": ["https://phab.test/diffusion/X.git"]}))
    );
    // Without a working copy the hg/svn lookups fail honestly.
    let plain = scratch_dir("t1170-vcsq-plain");
    assert!(vcs_repository_query(&plain, None).is_err());
    assert!(vcs_repository_query(&plain, Some(VcsKind::Svn)).is_err());
}

#[cfg(not(windows))]
fn t1170_fake_hg(dir: &Path) -> hg::HgCommand {
    // Every invocation carries a `--encoding utf-8` prefix, so match the
    // full argv (`$*`), never `$1`.
    let sh = dir.join("fakehg.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh\n\
         case \"$*\" in\n\
         \"--encoding utf-8 branch\") echo default; exit 0;;\n\
         *--style*) exit 0;;\n\
         *-r\\ bogus*) exit 1;;\n\
         *-r\\ *) echo deadbeefnode; exit 0;;\n\
         *failme*) exit 1;;\n\
         *emptyme*) exit 0;;\n\
         *) echo cafenode; exit 0;;\n\
         esac\n",
    )
    .unwrap();
    t1170_make_executable(&sh);
    hg::HgCommand::custom(&sh.to_string_lossy(), &[])
}

#[test]
#[cfg(not(windows))]
fn t1170_hg_rev_ancestor_resolve_via_fake() {
    let dir = scratch_dir("t1170-fakehg");
    let hg = t1170_fake_hg(&dir);
    assert_eq!(
        hg_rev_to_commit_with(&dir, &hg, "tip").as_deref(),
        Some("deadbeefnode")
    );
    assert_eq!(hg_rev_to_commit_with(&dir, &hg, "bogus"), None);
    assert_eq!(hg_ancestor_with(&dir, &hg, "tip", ".").unwrap(), "cafenode");
    assert_eq!(
        hg_ancestor_with(&dir, &hg, "failme", ".").unwrap_err(),
        "Unable to find any hg commit named 'failme'."
    );
    assert_eq!(
        hg_ancestor_with(&dir, &hg, "emptyme", ".").unwrap_err(),
        "Unable to find any hg commit named 'emptyme'."
    );
}

#[test]
#[cfg(not(windows))]
fn t1170_resolve_hg_base_auto_via_fake() {
    let dir = scratch_dir("t1170-hgauto");
    let hg = t1170_fake_hg(&dir);
    // No base or commit: branch plus an empty outgoing log yields ".".
    assert_eq!(
        resolve_hg_base_with(&dir, &hg, &Options::default()).unwrap(),
        "."
    );
    let options = Options {
        base: Some("tip".to_string()),
        ..Options::default()
    };
    assert_eq!(
        resolve_hg_base_with(&dir, &hg, &options).unwrap(),
        "deadbeefnode"
    );
    let options = Options {
        base: Some("bogus".to_string()),
        ..Options::default()
    };
    assert_eq!(
        resolve_hg_base_with(&dir, &hg, &options).unwrap_err(),
        "Unable to find any hg commit named 'bogus'."
    );
}

#[test]
fn t1170_submit_full_hg_base_rejects_head() {
    let options = Options {
        head: Some("tip".to_string()),
        ..Options::default()
    };
    let err = submit_full_hg_base(
        &scratch_dir("t1170-hghead"),
        &hg::HgCommand::default(),
        &options,
    )
    .unwrap_err();
    assert!(err.contains("'--head' is not supported under hg"), "{err}");
    // Positional paths become the base commit before hg resolution.
    #[cfg(not(windows))]
    {
        let dir = scratch_dir("t1170-hgpaths");
        let hg = t1170_fake_hg(&dir);
        let options = Options {
            paths: vec!["tip".to_string()],
            ..Options::default()
        };
        assert_eq!(
            submit_full_hg_base(&dir, &hg, &options).unwrap(),
            "cafenode"
        );
    }
}

#[test]
fn t1170_svn_or_hg_diff_text_outside_repo_errors() {
    let err = svn_or_hg_diff_text_with(
        &scratch_dir("t1170-nohvcs"),
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        &Options::default(),
        "",
    )
    .unwrap_err();
    assert_eq!(err, "not inside an hg or svn working copy");
}

#[test]
#[cfg(not(windows))]
fn t1170_svn_diff_failure_reports_stderr() {
    let dir = scratch_dir("t1170-fakesvn");
    std::fs::create_dir_all(dir.join(".svn")).unwrap();
    let sh = dir.join("fakesvn.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh\necho \"$@\" > args.txt\necho 'cannot talk' >&2\nexit 1\n",
    )
    .unwrap();
    t1170_make_executable(&sh);
    let svn = svn::SvnCommand::custom(&sh.to_string_lossy(), &[]);
    let options = Options {
        paths: vec!["a.txt".to_string()],
        ..Options::default()
    };
    let err =
        svn_or_hg_diff_text_with(&dir, &hg::HgCommand::default(), &svn, &options, "").unwrap_err();
    assert_eq!(err, "svn diff failed: cannot talk");
    let argv = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(argv.contains("-- a.txt"), "{argv}");
}

#[test]
#[cfg(not(windows))]
fn t1170_full_submit_properties_hg_lists_local_commits() {
    let dir = scratch_dir("t1170-hgprops");
    let hg = t1170_hg_log_fake(&dir);
    let props = full_submit_properties_with(&dir, &hg, Some(VcsKind::Hg), "base", None).unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].name, diff_submit::PROP_LOCAL_COMMITS);
    assert!(props[0].data.contains("n1"), "{}", props[0].data);
}

/* T1170 coverage: inspect-full, prefill, editors, companions, message file. */

#[test]
fn t1170_inspect_full_json_base_and_fields() {
    assert_eq!(
        inspect_full_json_base(
            &scratch_dir("t1170-ijson"),
            &Options::default(),
            VcsKind::Svn
        )
        .unwrap(),
        (String::new(), None)
    );
    // Non-git kinds get empty fields even with field flags.
    let options = Options {
        reviewers: Some("alice".to_string()),
        ..Options::default()
    };
    let mut doc = serde_json::json!({"base": "x"});
    attach_inspect_full_fields(
        &scratch_dir("t1170-ijson"),
        &options,
        VcsKind::Svn,
        "",
        None,
        &mut doc,
    )
    .unwrap();
    assert_eq!(doc["fields"], serde_json::json!({}));
    // Git collects fields from the message file.
    let dir = scratch_dir("t1170-ijson-git");
    let file = dir.join("msg.txt");
    std::fs::write(&file, "Hello title\n\nSummary: yes\n").unwrap();
    let options = Options {
        message_file: Some(file.to_string_lossy().into_owned()),
        ..Options::default()
    };
    let mut doc = serde_json::json!({"base": "x"});
    attach_inspect_full_fields(&dir, &options, VcsKind::Git, "", None, &mut doc).unwrap();
    assert_eq!(doc["fields"]["title"], serde_json::json!("Hello title"));
}

#[test]
fn t1170_inspect_full_text_git_and_entries_errors() {
    let dir = t1170_three_commit_repo("t1170-itext");
    std::fs::write(dir.join("a.txt"), "a\nchanged\n").unwrap();
    let out = inspect_full_text_git(&dir, &Options::default()).unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("a.txt") && text.contains("diff --git"),
        "{text}"
    );
    // Non-git entry rendering without a working copy errors honestly.
    let err = inspect_full_json_entries(
        &scratch_dir("t1170-ientries"),
        &Options::default(),
        VcsKind::Svn,
        "",
        None,
    )
    .unwrap_err();
    assert_eq!(err, "not inside an hg or svn working copy");
}

#[test]
fn t1170_collect_submit_changes_no_working_copy_errors() {
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let err = collect_submit_changes_with_io(
        &scratch_dir("t1170-norepo"),
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        &Options::default(),
        "",
        None,
        &mut io,
    )
    .unwrap_err();
    assert_eq!(err, "not inside a git, hg, or svn working copy");
}

#[test]
fn t1170_git_update_prefill_usable_and_covered() {
    let dir = t1170_three_commit_repo("t1170-prefill");
    let base = t1170_rev(&dir, "HEAD~2");
    // No active hashes: both range messages are usable.
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            return Err("ERR-CONDUIT-CALL".to_string());
        }
        assert_eq!(method, diff_submit::METHOD_QUERY_DIFFS);
        Ok(serde_json::json!([{"properties": {"local:commits": []}}]))
    };
    let prefill = git_update_prefill(&dir, &call, 5, &base, None)
        .unwrap()
        .unwrap();
    assert!(
        prefill.contains("two") && prefill.contains("three"),
        "{prefill}"
    );
    // Every hash attached: nothing usable.
    let head = t1170_rev(&dir, "HEAD");
    let prev = t1170_rev(&dir, "HEAD~1");
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            return Err("ERR-CONDUIT-CALL".to_string());
        }
        Ok(serde_json::json!([{"properties": {"local:commits": [
            {"commit": head.as_str()},
            {"commit": prev.as_str()},
        ]}}]))
    };
    assert_eq!(
        git_update_prefill(&dir, &call, 5, &base, None).unwrap(),
        None
    );
}

#[test]
fn t1170_default_update_prefill_git_and_none() {
    let dir = t1170_three_commit_repo("t1170-dprefill");
    // Svn and unknown kinds never prefill.
    let call = t1170_no_conduit;
    let options = Options::default();
    assert_eq!(
        default_update_prefill(
            &dir,
            &hg::HgCommand::default(),
            &options,
            Some(VcsKind::Svn),
            &call,
            5
        )
        .unwrap(),
        None
    );
    assert_eq!(
        default_update_prefill(&dir, &hg::HgCommand::default(), &options, None, &call, 5).unwrap(),
        None
    );
    // Git resolves the range and formats usable messages.
    let base = t1170_rev(&dir, "HEAD~2");
    let options = Options {
        base: Some(base),
        ..Options::default()
    };
    let qcall = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            return Err("ERR-CONDUIT-CALL".to_string());
        }
        Ok(serde_json::json!([{"properties": {"local:commits": []}}]))
    };
    let prefill = default_update_prefill(
        &dir,
        &hg::HgCommand::default(),
        &options,
        Some(VcsKind::Git),
        &qcall,
        5,
    )
    .unwrap()
    .unwrap();
    assert!(prefill.contains("three"), "{prefill}");
}

#[test]
#[cfg(not(windows))]
fn t1170_hg_update_prefill_via_fake() {
    let dir = scratch_dir("t1170-hgprefill");
    let sh = dir.join("fakehg.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh\n\
         if [ \"$3\" = \"branch\" ]; then echo default; exit 0; fi\n\
         printf 'aaa\\001first\\002bbb\\001second\\002'\nexit 0\n",
    )
    .unwrap();
    t1170_make_executable(&sh);
    let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            return Err("ERR-CONDUIT-CALL".to_string());
        }
        Ok(serde_json::json!([{"properties": {"local:commits": []}}]))
    };
    let prefill = hg_update_prefill(&dir, &hg, &call, 5, "base")
        .unwrap()
        .unwrap();
    assert!(
        prefill.contains("first") && prefill.contains("second"),
        "{prefill}"
    );
}

#[test]
fn t1170_create_local_commits_matrix() {
    let hg = hg::HgCommand::default();
    let dir = scratch_dir("t1170-clc");
    assert!(
        create_local_commits(&dir, &hg, Some(VcsKind::Svn), "x", None, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        create_local_commits(&dir, &hg, None, "x", None, None)
            .unwrap()
            .is_empty()
    );
    let git = t1170_three_commit_repo("t1170-clc-git");
    let base = t1170_rev(&git, "HEAD~1");
    let infos =
        create_local_commits(&git, &hg, Some(VcsKind::Git), &base, Some("HEAD"), None).unwrap();
    assert_eq!(infos.len(), 1);
    assert!(infos[0].message.contains("three"), "{}", infos[0].message);
    #[cfg(not(windows))]
    {
        let dir = scratch_dir("t1170-clc-hg");
        let fake = t1170_hg_log_fake(&dir);
        let infos =
            create_local_commits(&dir, &fake, Some(VcsKind::Hg), "base", None, None).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].commit, "n1");
        let first = &infos[0].message;
        assert!(first.contains("First line"), "{first}");
    }
}

#[test]
fn t1170_bare_submit_mode_without_working_copy_creates() {
    let call = t1170_no_conduit;
    let dir = scratch_dir("t1170-bare");
    let hg = hg::HgCommand::default();
    assert!(
        load_working_copy_revisions(&call, &dir, &hg, None, "", None, "PHID-USER-1")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        resolve_bare_submit_mode(&call, &dir, &hg, None, "", None, "PHID-USER-1").unwrap(),
        SubmitMode::Create
    );
}

#[test]
#[cfg(not(windows))]
fn t1170_edit_interactively_failing_editor_errors() {
    let err = edit_interactively("x\n", "msg", None, Some("false"), 3).unwrap_err();
    assert_eq!(err, "Editor exited with an error code (#1).");
}

#[test]
#[cfg(not(windows))]
fn t1170_edit_interactively_deleted_file_errors() {
    let err = edit_interactively("x\n", "msg", None, Some("rm"), 0).unwrap_err();
    assert_eq!(err, "failed to read editor temporary file");
}

#[test]
fn t1170_maybe_forgive_editor_exit_passthrough() {
    assert_eq!(maybe_forgive_editor_exit("nano", 1), 1);
    assert_eq!(maybe_forgive_editor_exit("nano", 0), 0);
}

#[test]
fn t1170_fold_entry_companion_new_update_guards() {
    let rename = "diff --git a/old.txt b/new.txt\nsimilarity index 90%\nrename from old.txt\nrename to new.txt\nindex 1111111..2222222 100644\n--- a/old.txt\n+++ b/new.txt\n@@ -1 +1 @@\n-a\n+b\n";
    let parsed = diff_local::parse_unified_diff(rename).unwrap();
    let full = diff_local::with_companions(&parsed);
    assert!(!full.is_empty());
    let entry = |change_type: u32, old: Option<&str>, current: &str| diff_local::ChangeEntry {
        change_type,
        file_type: diff_local::FILE_TEXT,
        old_path: old.map(str::to_string),
        away_paths: Vec::new(),
        current: FileDiff {
            path: current.to_string(),
            added: 1,
            deleted: 1,
        },
    };
    let mut fold = diff_local::CompanionFold::new();
    let mut slots: Vec<usize> = Vec::new();
    let mut changes: Vec<diff_submit::DiffChange> = Vec::new();
    // Guards leave everything untouched.
    for guarded in [
        entry(diff_local::TYPE_CHANGE, Some("old.txt"), "new.txt"),
        entry(diff_local::TYPE_MOVE_HERE, None, "new.txt"),
        entry(diff_local::TYPE_MOVE_HERE, Some(""), "new.txt"),
        entry(diff_local::TYPE_MOVE_HERE, Some("same.txt"), "same.txt"),
    ] {
        fold_entry_companion(&mut fold, &mut slots, &mut changes, &guarded, &full);
    }
    assert!(changes.is_empty() && slots.is_empty());
    // The first reference emits the companion for the old path.
    fold_entry_companion(
        &mut fold,
        &mut slots,
        &mut changes,
        &entry(diff_local::TYPE_MOVE_HERE, Some("old.txt"), "new.txt"),
        &full,
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "old.txt");
    assert_eq!(changes[0].away_paths, vec!["new.txt".to_string()]);
    assert_eq!(changes[0].old_path, None);
    // A second reference to the same old path updates it in place.
    fold_entry_companion(
        &mut fold,
        &mut slots,
        &mut changes,
        &entry(diff_local::TYPE_COPY_HERE, Some("old.txt"), "new2.txt"),
        &full,
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].away_paths,
        vec!["new.txt".to_string(), "new2.txt".to_string()]
    );
}

#[test]
fn t1170_prepare_submit_create_with_message_file() {
    let dir = t1170_three_commit_repo("t1170-prepmsg");
    std::fs::write(dir.join("a.txt"), "a\nchanged\n").unwrap();
    let file = dir.join("msg.txt");
    std::fs::write(&file, "Filed title\n\nSummary: from file\n").unwrap();
    let options = Options {
        message_file: Some(file.to_string_lossy().into_owned()),
        ..Options::default()
    };
    let input = prepare_submit(&dir, &options, &SubmitMode::Create).unwrap();
    assert!(
        input
            .message_corpus
            .as_deref()
            .unwrap()
            .contains("Filed title"),
        "{:?}",
        input.message_corpus
    );
    assert_eq!(input.fields["title"], serde_json::json!("Filed title"));
}

#[test]
fn t1170_run_help_returns_success() {
    assert_eq!(run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus-flag-t1170"])), ExitCode::FAILURE);
    // Read-only `--json` inspects and writes successfully in a checkout
    // (cargo always runs tests from the package root, inside the repo).
    assert!(is_inside_git_repo(Path::new(".")));
    assert_eq!(run(&args(&["--json"])), ExitCode::SUCCESS);
}

#[test]
#[cfg(not(windows))]
fn t1170_collect_hg_empty_diff_errors() {
    let dir = scratch_dir("t1170-hgempty");
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    let sh = dir.join("fakehg.sh");
    std::fs::write(&sh, "#!/bin/sh\nexit 0\n").unwrap();
    t1170_make_executable(&sh);
    let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let err = collect_submit_changes_with_io(
        &dir,
        &hg,
        &svn::SvnCommand::default(),
        &Options::default(),
        "base",
        None,
        &mut io,
    )
    .unwrap_err();
    assert_eq!(err, diff_local::NO_CHANGES_FOUND);
}

#[cfg(not(windows))]
fn t1170_hg_log_fake(dir: &Path) -> hg::HgCommand {
    let sh = dir.join("fakehg.sh");
    let record =
        "n1\\0015\\001Jane <j@x>\\0011700000000 0\\001default\\001\\001\\001First line\\002";
    std::fs::write(
        &sh,
        format!(
            "#!/bin/sh\n\
             if [ \"$3\" = \"branch\" ]; then echo default; exit 0; fi\n\
             if [ \"$3\" = \"parents\" ]; then echo ppnode; exit 0; fi\n\
             printf '{record}'\nexit 0\n"
        ),
    )
    .unwrap();
    t1170_make_executable(&sh);
    hg::HgCommand::custom(&sh.to_string_lossy(), &[])
}

#[test]
fn t1170_create_message_without_commits_uses_server_template() {
    // DIFF-D8 with no local commits and no flags: no included block, so
    // the server template feeds the edit cycle directly.
    let root = scratch_dir("t1170-nocommits");
    let meta = root.join(".git");
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            return Ok(serde_json::json!("Server template\n"));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "Fresh title"}}))
    };
    let captured = std::cell::RefCell::new(String::new());
    let edit = |template: &str| -> Result<String, String> {
        *captured.borrow_mut() = template.to_string();
        Ok("Fresh title\n".to_string())
    };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let message = create_message_from_user(&mut context, &defaults(), &[], Some("main")).unwrap();
    assert_eq!(message.raw_corpus, "Fresh title\n");
    let template = captured.borrow().clone();
    assert!(template.contains("Server template"), "{template}");
    assert!(!template.contains("Included changes"), "{template}");
}

#[test]
fn t1170_create_submit_json_discards_report() {
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(serde_json::json!({"diffid": 3, "uri": "https://phab.test/diff/3/"}));
        }
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({"fields": {"title": "T"}}));
        }
        assert_eq!(
            method,
            diff_submit::METHOD_CREATE_REVISION,
            "unexpected {method}"
        );
        assert_eq!(params["diffid"], serde_json::json!(3));
        Ok(serde_json::json!({"revisionid": 9, "uri": "https://phab.test/D9"}))
    };
    let mut input = create_test_input(false, Some("T\n"));
    input.json = true;
    let report = submit_over_with_uri_ansi(&call, &input, false).unwrap();
    assert_eq!(report.text, "");
    assert_eq!(report.uri.as_deref(), Some("https://phab.test/D9"));
    assert_eq!(report.revision_id, Some(9));
}

/* T1170 wave-3: diff.rs to 100% canonical LCOV. */

#[test]
fn t1170_strip_placeholder_passthrough() {
    // A faux message without the placeholder title rides through
    // untouched (only `<placeholder>` first lines strip).
    assert_eq!(
        strip_placeholder_line("Real title\nReviewers: a\n"),
        "Real title\nReviewers: a\n"
    );
    assert_eq!(strip_placeholder_line("solo"), "solo");
}

#[test]
fn t1170_set_child_flags_matrix() {
    // Every passthru flag maps to its option; unknown names stay off.
    // (The lookup table preserves each list's spec order.)
    let options = Options {
        apply_patches: true,
        never_apply_patches: true,
        amend_all: true,
        amend_autofixes: true,
        coverage: true,
        no_coverage: true,
        ..defaults()
    };
    let (lint, unit) = set_child_flags(&options);
    for (got, wanted) in [
        (&lint, diff_local::LINT_PASSTHRU_FLAGS),
        (&unit, diff_local::UNIT_PASSTHRU_FLAGS),
    ] {
        let expected: Vec<String> = wanted.iter().map(|flag| flag.to_string()).collect();
        assert_eq!(got, &expected);
    }
    let (lint, unit) = set_child_flags(&defaults());
    assert!(lint.is_empty());
    assert!(unit.is_empty());
    // A single flag lands only where its list names it.
    let options = Options {
        apply_patches: true,
        ..defaults()
    };
    let (lint, unit) = set_child_flags(&options);
    assert!(lint.contains(&"apply-patches".to_string()));
    assert!(!unit.contains(&"apply-patches".to_string()));
}

#[test]
fn t1170_report_create_submit_propagates_revision_failure() {
    // A failing `differential.createrevision` aborts the create report.
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_CREATE_REVISION);
        Err("conduit down".to_string())
    };
    let created = diff_submit::CreatedDiff {
        diff_id: 3,
        phid: None,
        uri: "https://phab.test/diff/3/".to_string(),
    };
    let err = report_create_submit(&call, &created, &create_test_input(false, None), false)
        .err()
        .expect("expected the conduit failure to propagate");
    assert_eq!(err, "conduit down");
}

#[test]
fn t1170_resolve_update_revision_propagates_template_failure() {
    // A failing commit-message fetch aborts update resolution.
    let meta = scratch_dir("t1170-updtempl");
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        Err("template down".to_string())
    };
    let edit = |_: &str, _: &str| -> Result<String, String> { Ok(String::new()) };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(Some("m"), false);
    let target = UpdateTarget {
        revision_id: 5,
        revision_arg: "5",
        verbatim_fields: &serde_json::json!({}),
        check_ownership: false,
        edit_fields: None,
    };
    let err = resolve_update_revision(&mut resolver, &options, &target).unwrap_err();
    assert_eq!(err, "template down");
}

#[test]
fn t1170_create_message_default_fields_failure_aborts() {
    // A failing default-fields pull aborts before the saved template.
    let root = scratch_dir("t1170-cmdf");
    let meta = root.join(".git");
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Err("parse down".to_string())
    };
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let local = vec![d8_local_commit("abcdef1234567890", "Subj", "Subj\n")];
    let err =
        create_message_from_user(&mut context, &defaults(), &local, Some("main")).unwrap_err();
    assert_eq!(err, "parse down");
}

struct FailWriter;

impl std::io::Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("t1170 no writes"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("t1170 no writes"))
    }
}

#[test]
fn t1170_saved_template_prompt_write_failure_aborts() {
    // A saved template that cannot print its notice aborts the create.
    let root = scratch_dir("t1170-cmsave");
    let meta = root.join(".git");
    write_scratch_file(
        &meta,
        diff_local::SCRATCH_CREATE_MESSAGE,
        "Saved title\nBody\n",
    );
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "X"}}))
    };
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = FailWriter;
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let local = vec![d8_local_commit("abcdef1234567890", "Subj", "Subj\n")];
    let err =
        create_message_from_user(&mut context, &defaults(), &local, Some("main")).unwrap_err();
    assert!(err.contains("failed to write prompt"), "{err}");
    // The double fails flush too (production never reaches it on this path).
    assert!(std::io::Write::flush(&mut writer).is_err());
}

#[test]
fn t1170_resolve_create_message_missing_file_fails() {
    // A missing `--message-file` fails before any conduit traffic.
    let root = scratch_dir("t1170-cmmiss");
    let call = t1170_no_conduit;
    let edit = |_: &str| -> Result<String, String> { Ok(String::new()) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &root,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let options = Options {
        message_file: Some(root.join("absent.txt").to_string_lossy().into_owned()),
        ..defaults()
    };
    assert!(resolve_create_message(&mut context, &options, &[], Some("main")).is_err());
}

#[test]
fn t1170_submit_svn_parsed_failure_propagates() {
    // A failing svn backend aborts the collected parse.
    let dir = scratch_dir("t1170-svnparsed");
    std::fs::create_dir_all(dir.join(".svn")).unwrap();
    let svn = svn::SvnCommand::custom("definitely-not-svn-t1170", &[]);
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let err =
        submit_svn_parsed(&dir, &svn, &defaults(), false, Some(VcsKind::Svn), &mut io).unwrap_err();
    assert!(!err.is_empty(), "expected an svn failure");
    // Raw sources and non-svn working copies skip collection entirely.
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    assert!(
        submit_svn_parsed(&dir, &svn, &defaults(), true, Some(VcsKind::Svn), &mut io)
            .unwrap()
            .is_none()
    );
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    assert!(
        submit_svn_parsed(&dir, &svn, &defaults(), false, Some(VcsKind::Git), &mut io)
            .unwrap()
            .is_none()
    );
}

#[test]
fn t1170_uuid_or_remote_query_matrix() {
    // The UUID constraint wins; the remote URI is the fallback.
    assert_eq!(
        uuid_or_remote_query(Some("uuid-1".to_string()), Some("https://x/".to_string())).unwrap(),
        Some(serde_json::json!({"uuids": ["uuid-1"]}))
    );
    assert_eq!(
        uuid_or_remote_query(None, Some("https://x/".to_string())).unwrap(),
        Some(serde_json::json!({"remoteURIs": ["https://x/"]}))
    );
    assert_eq!(uuid_or_remote_query(None, None).unwrap(), None);
}

#[test]
fn t1170_readable_scratch_collapses_dot_segments() {
    // A work root with `.` segments resolves lexically.
    let root = scratch_dir("t1170-dotroot");
    let meta = root.join(".git");
    let dotted = root.join(".").join("sub").join("..");
    let expected = Path::new(".git")
        .join("arc")
        .join("create-message")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        readable_scratch_file_path(&meta, &dotted, "create-message"),
        expected
    );
}

#[test]
fn t1170_git_metadata_dir_outside_repo_errors() {
    let err = git_metadata_dir(&scratch_dir("t1170-nometa")).unwrap_err();
    assert_eq!(err, "not inside a git repository");
}

#[test]
fn t1170_binary_exists_with_path_matrix() {
    // An unset PATH finds nothing; a set one scans each dir.
    assert!(!binary_exists_with_path("anything-at-all", None));
    let dir = scratch_dir("t1170-pathtools");
    std::fs::write(dir.join("mytool"), "x").unwrap();
    let path = format!(
        "{}{}{}",
        scratch_dir("t1170-pathempty").to_string_lossy(),
        if cfg!(windows) { ';' } else { ':' },
        dir.to_string_lossy()
    );
    assert!(binary_exists_with_path("mytool", Some(path.clone())));
    assert!(!binary_exists_with_path("missing-tool", Some(path)));
    assert!(!binary_exists_with_path(
        "missing-tool",
        Some(String::new())
    ));
}

#[test]
fn t1170_maybe_forgive_with_runner_matrix() {
    use std::process::ExitStatus;
    #[cfg(unix)]
    fn status(code: u32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        // Exit codes ride the high byte of the raw wait status.
        ExitStatusExt::from_raw((code << 8) as i32)
    }
    #[cfg(windows)]
    fn status(code: u32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatusExt::from_raw(code)
    }
    // Clean exits and non-vi editors pass through untouched.
    assert_eq!(
        maybe_forgive_editor_exit_with("vim", 0, &|_| { panic!("no diagnostic on clean exit") }),
        0
    );
    assert_eq!(
        maybe_forgive_editor_exit_with("nano", 3, &|_| { panic!("no diagnostic outside vi/vim") }),
        3
    );
    // A healthy `vi +Q +q` keeps the failure; a failing one forgives.
    let healthy = |_: &str| -> Result<ExitStatus, String> { Ok(status(0)) };
    assert_eq!(maybe_forgive_editor_exit_with("vi", 3, &healthy), 3);
    let sick = |_: &str| -> Result<ExitStatus, String> { Ok(status(1)) };
    assert_eq!(maybe_forgive_editor_exit_with("vi", 3, &sick), 0);
    assert_eq!(maybe_forgive_editor_exit_with("/usr/bin/vim", 3, &sick), 0);
    let broken = |_: &str| -> Result<ExitStatus, String> { Err("no shell".to_string()) };
    assert_eq!(maybe_forgive_editor_exit_with("vim", 3, &broken), 0);
    // The diagnostic names the editor binary it probes.
    let seen = std::cell::RefCell::new(String::new());
    let probe = |command: &str| -> Result<ExitStatus, String> {
        *seen.borrow_mut() = command.to_string();
        Ok(status(0))
    };
    assert_eq!(maybe_forgive_editor_exit_with("vi", 3, &probe), 3);
    assert_eq!(*seen.borrow(), "vi +Q +q");
}

#[test]
fn t1170_create_edit_document_write_failure_cleans_up() {
    let fail = |_: &Path, _: &str| false;
    let err = create_edit_document_with("text\n", "msg", &fail).unwrap_err();
    assert_eq!(err, "failed to write editor temporary file");
    // The happy path still writes the converted document.
    let dir = create_edit_document_with("text\n", "msg", &|path, content| {
        std::fs::write(path, content).is_ok()
    })
    .unwrap();
    assert!(dir.1.is_file());
    std::fs::remove_dir_all(&dir.0).ok();
}

#[test]
fn t1170_edit_interactively_with_no_editor_errors() {
    // No preferred editor, no `$EDITOR`, no fallback candidate.
    let none = |_: &str| false;
    let err = edit_interactively_with("text\n", "msg", None, None, 0, None, &none).unwrap_err();
    assert_eq!(err, NO_EDITOR_MESSAGE);
    let err =
        edit_interactively_with("text\n", "msg", None, Some(""), 0, Some(""), &none).unwrap_err();
    assert_eq!(err, NO_EDITOR_MESSAGE);
}

#[test]
fn t1170_amend_message_write_failures_report() {
    let dir = scratch_dir("t1170-amendwrite");
    let fail = |_: &Path, _: &str| false;
    let err = amend_worktree_message_git_with(&dir, "m", &fail).unwrap_err();
    assert_eq!(err, "failed to write amend message file");
    let err = amend_worktree_message_hg_with_writer(
        &dir,
        &hg::HgCommand::custom("definitely-not-hg-t1170", &[]),
        "m",
        &fail,
    )
    .unwrap_err();
    assert_eq!(err, "failed to write amend message file");
}

#[test]
fn t1170_prompt_confirm_with_tty_matrix() {
    // A TTY stdin reads EOF in tests, answering with the default.
    assert!(!prompt_confirm_with_tty("P?", true, true).unwrap());
    assert!(prompt_confirm_with_tty("P?", false, true).unwrap());
    // A non-TTY stdin blocks on Unix, reads through elsewhere.
    #[cfg(unix)]
    assert_eq!(
        prompt_confirm_with_tty("P?", true, false).unwrap_err(),
        STDIN_NOT_TTY
    );
    #[cfg(not(unix))]
    assert!(!prompt_confirm_with_tty("P?", true, false).unwrap());
}

#[test]
fn t1170_read_raw_diff_text_stdin_returns_body() {
    // Test stdin is at EOF, so the stdin source yields an empty body.
    let options = Options {
        raw: true,
        ..defaults()
    };
    assert_eq!(read_raw_diff_text(&options).unwrap(), "");
}

struct FailSink;

impl std::io::Write for FailSink {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("t1170 sink closed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn t1170_seed_git_repo(tag: &str, commits: usize) -> PathBuf {
    let dir = git_dir(tag);
    for (key, value) in [("user.name", "t"), ("user.email", "t@t")] {
        let set = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["config", key, value])
            .status()
            .unwrap();
        assert!(set.success());
    }
    for index in 0..commits {
        std::fs::write(dir.join("f.txt"), format!("line {index}\n")).unwrap();
        let add = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["add", "-A"])
            .status()
            .unwrap();
        assert!(add.success());
        let commit = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["commit", "-qam", &format!("commit {index}")])
            .status()
            .unwrap();
        assert!(commit.success());
    }
    dir
}

#[test]
fn t1170_run_in_matrix() {
    // `--help` prints through the wrapper without touching the sink.
    let mut sink = Vec::new();
    assert_eq!(
        run_in(&args(&["--help"]), Path::new("."), &mut sink),
        ExitCode::SUCCESS
    );
    // Unknown flags fail before inspection.
    let mut sink = Vec::new();
    assert_eq!(
        run_in(&args(&["--bogus-flag-t1170"]), Path::new("."), &mut sink),
        ExitCode::FAILURE
    );
    // Read-only `--json` succeeds into a working sink ...
    let repo = t1170_seed_git_repo("t1170-runin", 1);
    let mut sink = Vec::new();
    assert_eq!(
        run_in(&args(&["--json"]), &repo, &mut sink),
        ExitCode::SUCCESS
    );
    assert!(!sink.is_empty());
    // ... and reports a closed one.
    assert_eq!(
        run_in(&args(&["--json"]), &repo, &mut FailSink),
        ExitCode::FAILURE
    );
    // The double's flush is a no-op (writes are what fail).
    assert!(std::io::Write::flush(&mut FailSink).is_ok());
    // Inspection errors fail without writing.
    let mut sink = Vec::new();
    assert_eq!(
        run_in(
            &args(&["--json"]),
            &scratch_dir("t1170-runin-no-repo"),
            &mut sink
        ),
        ExitCode::FAILURE
    );
    assert!(sink.is_empty());
}

#[test]
fn t1170_inspect_ansi_submit_arm_reports_client_error() {
    // A dead-port conduit URI fails credential lookup deterministically:
    // no arcrc on record carries a token for it.
    let repo = t1170_seed_git_repo("t1170-submitarm", 1);
    let options = Options {
        only: true,
        conduit_uri: Some("http://127.0.0.1:9/".to_string()),
        ..defaults()
    };
    let err = inspect_ansi(&repo, &options, false).unwrap_err();
    assert!(err.contains("no token for"), "{err}");
}

#[test]
fn t1170_resolve_base_prefers_upstream_merge_base() {
    // A fresh clone tracks its origin, so the base is the merge-base
    // with the upstream (here: HEAD itself).
    let origin = t1170_seed_git_repo("t1170-upstream-origin", 2);
    let parent = scratch_dir("t1170-upstream-parent");
    let clone = parent.join("clone");
    let status = std::process::Command::new("git")
        .args(["clone", &origin.to_string_lossy(), &clone.to_string_lossy()])
        .status()
        .unwrap();
    assert!(status.success());
    let head = t1170_rev(&clone, "HEAD");
    assert_eq!(resolve_base(&clone, None).unwrap(), head);
    assert_eq!(upstream_merge_base(&clone).as_deref(), Some(head.as_str()));
    // Without an upstream there is no merge-base to prefer.
    assert_eq!(upstream_merge_base(&origin), None);
    assert_eq!(
        resolve_base(&origin, None).unwrap(),
        t1170_rev(&origin, "HEAD")
    );
}

#[test]
fn t1170_submit_changes_with_hunks_tolerates_unmatched_entries() {
    // An entry with no parsed twin keeps its skeleton: the companion
    // fallback runs and also finds nothing.
    let repo = t1170_seed_git_repo("t1170-ghost", 1);
    let base = t1170_rev(&repo, "HEAD");
    let entries = vec![diff_local::ChangeEntry {
        change_type: diff_local::TYPE_CHANGE,
        file_type: diff_local::FILE_TEXT,
        old_path: None,
        away_paths: Vec::new(),
        current: FileDiff {
            path: "ghost.txt".to_string(),
            added: 0,
            deleted: 0,
        },
    }];
    let changes = submit_changes_with_hunks(&repo, &base, None, &entries).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "ghost.txt");
    assert!(changes[0].hunks.is_empty());
}

#[test]
fn t1170_run_lint_unit_stdio_skips_both_children() {
    // Skipped children return skip results without touching stdio.
    let repo = t1170_seed_git_repo("t1170-lintskip", 1);
    let options = Options {
        nounit: true,
        nolint: true,
        ..defaults()
    };
    let report =
        run_lint_unit_stdio_ansi(&repo, &options, Some("HEAD"), None, false, false).unwrap();
    assert_eq!(report.lint_result, Some(diff_local::LINT_RESULT_SKIP));
    assert_eq!(report.unit_result, Some(diff_local::UNIT_RESULT_SKIP));
    assert!(report.unresolved_lint.is_empty());
    assert!(report.test_results.is_empty());
}

#[test]
fn t1170_run_lint_unit_stdio_reports_lint_failures() {
    // Outside a working copy the lint child fails, aborting before
    // the unit phase runs.
    let dir = scratch_dir("t1170-lintfail");
    let options = defaults();
    assert!(run_lint_unit_stdio_ansi(&dir, &options, None, None, false, false).is_err());
}

#[test]
fn t1170_run_lint_unit_stdio_reports_unit_failures() {
    // With lint skipped, a broken unit setup fails the unit phase.
    let dir = scratch_dir("t1170-unitfail");
    std::fs::write(dir.join(".arcunit"), "{nope").unwrap();
    let options = Options {
        nolint: true,
        ..defaults()
    };
    assert!(run_lint_unit_stdio_ansi(&dir, &options, None, None, false, false).is_err());
}

#[test]
fn t1170_prepare_submit_spec_failure_on_bare_symref() {
    // HEAD retargeted at a tag resolves for the range but breaks
    // branch-name parsing when the spec loads.
    let repo = t1170_seed_git_repo("t1170-symref", 1);
    std::fs::write(repo.join("f.txt"), "dirty\n").unwrap();
    let tag = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["tag", "v1"])
        .status()
        .unwrap();
    assert!(tag.success());
    std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/tags/v1\n").unwrap();
    let err = prepare_submit(&repo, &defaults(), &SubmitMode::Only).unwrap_err();
    assert!(err.contains("Failed to parse git symbolic-ref"), "{err}");
}

#[test]
fn t1170_inspect_full_text_git_arm() {
    let repo = t1170_seed_git_repo("t1170-fulltext", 1);
    std::fs::write(repo.join("f.txt"), "dirty\n").unwrap();
    let out = inspect_full_text(&repo, &defaults(), VcsKind::Git).unwrap();
    assert!(!out.is_empty());
}

#[test]
#[cfg(not(windows))]
fn t1170_hg_update_prefill_empty_returns_none() {
    // A single log record has no usable predecessor, so no prefill.
    let dir = scratch_dir("t1170-hguseprefill");
    let sh = dir.join("fakehg.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh\ncase \"$*\" in\n*--template*) printf 'node1\\001desc one\\002';;\n*branch*) echo default;;\n*) exit 0;;\nesac\n",
    )
    .unwrap();
    t1170_make_executable(&sh);
    let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            return Err("old server".to_string());
        }
        assert_eq!(method, diff_submit::METHOD_QUERY_DIFFS);
        Ok(serde_json::json!([]))
    };
    assert_eq!(hg_update_prefill(&dir, &hg, &call, 5, ".").unwrap(), None);
}

#[test]
#[cfg(not(windows))]
fn t1170_default_update_prefill_hg_arm() {
    let dir = scratch_dir("t1170-defprefill");
    let hg = t1170_fake_hg(&dir);
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            return Err("old server".to_string());
        }
        assert_eq!(method, diff_submit::METHOD_QUERY_DIFFS);
        Ok(serde_json::json!([]))
    };
    assert_eq!(
        default_update_prefill(&dir, &hg, &defaults(), Some(VcsKind::Hg), &call, 5).unwrap(),
        None
    );
}

#[test]
#[cfg(not(windows))]
fn t1170_create_local_commits_hg_explicit_branch() {
    // An explicit branch skips the branch lookup; an empty log yields
    // no commits.
    let dir = scratch_dir("t1170-hglocal");
    let hg = t1170_fake_hg(&dir);
    let commits =
        create_local_commits(&dir, &hg, Some(VcsKind::Hg), ".", None, Some("br")).unwrap();
    assert!(commits.is_empty());
}

#[test]
#[cfg(not(windows))]
fn t1170_load_working_copy_revisions_hg_and_svn() {
    // Hg with an empty log finds no revisions without conduit traffic.
    let dir = scratch_dir("t1170-wcrev");
    let hg = t1170_fake_hg(&dir);
    let call = t1170_no_conduit;
    let revisions = load_working_copy_revisions(
        &call,
        &dir,
        &hg,
        Some(VcsKind::Hg),
        ".",
        None,
        "PHID-USER-1",
    )
    .unwrap();
    assert!(revisions.is_empty());
    // Svn filters the open revisions to this checkout path.
    let here = dir.to_string_lossy().into_owned();
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "differential.query");
        Ok(serde_json::json!([
            {"id": 5, "title": "Here", "sourcePath": here},
            {"id": 6, "title": "Elsewhere", "sourcePath": "/nope"},
        ]))
    };
    let revisions = load_working_copy_revisions(
        &call,
        &dir,
        &hg::HgCommand::default(),
        Some(VcsKind::Svn),
        "",
        None,
        "PHID-USER-1",
    )
    .unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].id, 5);
}

#[test]
#[cfg(not(windows))]
fn t1170_inspect_full_json_base_hg() {
    let dir = scratch_dir("t1170-jsonbase");
    let hg = t1170_fake_hg(&dir);
    assert_eq!(
        inspect_full_json_base_with(&dir, &defaults(), VcsKind::Hg, &hg).unwrap(),
        (".".to_string(), None)
    );
}

#[test]
#[cfg(not(windows))]
fn t1170_inspect_full_json_entries_hg() {
    // Hg entries render from the worktree diff text.
    let dir = scratch_dir("t1170-jsonentries");
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    let sh = dir.join("fakehg.sh");
    std::fs::write(&sh, "#!/bin/sh\ncat hg.diff\n").unwrap();
    t1170_make_executable(&sh);
    std::fs::write(
        dir.join("hg.diff"),
        "diff --git a/f.txt b/f.txt\nnew file mode 100644\n--- /dev/null\n+++ b/f.txt\n@@ -0,0 +1 @@\n+hi\n",
    )
    .unwrap();
    let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
    let entries = inspect_full_json_entries_with(
        &dir,
        &defaults(),
        VcsKind::Hg,
        ".",
        None,
        &hg,
        &svn::SvnCommand::default(),
    )
    .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].current.path, "f.txt");
    assert_eq!(entries[0].current.added, 1);
}

#[test]
#[cfg(not(windows))]
fn t1170_inspect_full_text_hg_with_modes() {
    // A nonempty hg diff passes through; an empty one reports no changes.
    for (tag, body, expect_ok) in [
        ("t1170-texthg-full", "diff --git a/f b/f\n", true),
        ("t1170-texthg-empty", "", false),
    ] {
        let dir = scratch_dir(tag);
        std::fs::create_dir_all(dir.join(".hg")).unwrap();
        std::fs::write(dir.join("hg.diff"), body).unwrap();
        let sh = dir.join("fakehg.sh");
        std::fs::write(
            &sh,
            "#!/bin/sh\ncase \"$*\" in\n\"--encoding utf-8 branch\") echo default;;\n*diff*) cat hg.diff;;\n*) exit 0;;\nesac\n",
        )
        .unwrap();
        t1170_make_executable(&sh);
        let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
        let out = inspect_full_text_hg_with(&dir, &defaults(), &hg, &svn::SvnCommand::default());
        if expect_ok {
            assert_eq!(out.unwrap(), body.as_bytes());
        } else {
            assert_eq!(out.unwrap_err(), diff_local::NO_CHANGES_FOUND);
        }
    }
}

#[test]
#[cfg(not(windows))]
fn t1170_lint_unit_base_hg_with_paths() {
    let dir = scratch_dir("t1170-lubhg");
    let hg = t1170_fake_hg(&dir);
    assert_eq!(
        lint_unit_base_with(&dir, &defaults(), Some(VcsKind::Hg), false, &hg).unwrap(),
        (Some(".".to_string()), None)
    );
    let options = Options {
        paths: vec!["tip".to_string()],
        ..defaults()
    };
    assert_eq!(
        lint_unit_base_with(&dir, &options, Some(VcsKind::Hg), false, &hg).unwrap(),
        (Some("cafenode".to_string()), None)
    );
}

#[test]
#[cfg(not(windows))]
fn t1170_full_submit_properties_hg_empty() {
    // No local commits means no `local:commits` property.
    let dir = scratch_dir("t1170-hgpropsempty");
    let hg = t1170_fake_hg(&dir);
    let properties = full_submit_properties_with(&dir, &hg, Some(VcsKind::Hg), ".", None).unwrap();
    assert!(properties.is_empty());
}

#[test]
#[cfg(not(windows))]
fn t1170_amend_worktree_message_hg_with_modes() {
    // Exit 0 amends; anything else reports stderr.
    let dir = scratch_dir("t1170-hgamend");
    let ok = dir.join("okhg.sh");
    std::fs::write(&ok, "#!/bin/sh\nexit 0\n").unwrap();
    t1170_make_executable(&ok);
    amend_worktree_message_hg_with(
        &dir,
        &hg::HgCommand::custom(&ok.to_string_lossy(), &[]),
        "new message\n",
    )
    .unwrap();
    let bad = dir.join("badhg.sh");
    std::fs::write(&bad, "#!/bin/sh\necho 'locked' >&2\nexit 1\n").unwrap();
    t1170_make_executable(&bad);
    let err = amend_worktree_message_hg_with(
        &dir,
        &hg::HgCommand::custom(&bad.to_string_lossy(), &[]),
        "new message\n",
    )
    .unwrap_err();
    assert_eq!(err, "hg amend failed: locked");
}

#[test]
#[cfg(not(windows))]
fn t1170_prepare_submit_full_entries_failure_propagates() {
    // The entry render re-reads the hg diff: a failure there (after a
    // good change collection) aborts the submit.
    let dir = scratch_dir("t1170-hgentriesfail");
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    std::fs::write(
        dir.join("valid.diff"),
        "diff --git a/f.txt b/f.txt\nnew file mode 100644\n--- /dev/null\n+++ b/f.txt\n@@ -0,0 +1 @@\n+hi\n",
    )
    .unwrap();
    let sh = dir.join("fakehg.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh\ncase \"$*\" in\n\"--encoding utf-8 branch\") echo default;;\n*\"bookmarks\"*) exit 0;;\n*\" root\"|\"root \"*) pwd;;\n*diff\\ --git*)\n  n=$(cat diffcount 2>/dev/null || echo 0); n=$((n+1)); echo $n > diffcount\n  if [ \"$n\" -eq 1 ]; then cat valid.diff; else echo boom >&2; exit 1; fi;;\n*) exit 0;;\nesac\n",
    )
    .unwrap();
    t1170_make_executable(&sh);
    let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let err = prepare_submit_full_with_commands_io(
        &dir,
        &hg,
        &svn::SvnCommand::default(),
        &defaults(),
        &SubmitMode::Only,
        &diff_local::LintUnitReport::empty(),
        &mut io,
    )
    .unwrap_err();
    assert!(err.contains("hg diff failed"), "{err}");
}

#[test]
#[cfg(not(windows))]
fn t1170_svn_diff_success_reports_stdout() {
    let dir = scratch_dir("t1170-svnok");
    std::fs::create_dir_all(dir.join(".svn")).unwrap();
    let sh = dir.join("fakesvn.sh");
    std::fs::write(&sh, "#!/bin/sh\necho 'Index: a.txt'\nexit 0\n").unwrap();
    t1170_make_executable(&sh);
    let svn = svn::SvnCommand::custom(&sh.to_string_lossy(), &[]);
    let options = Options {
        paths: vec!["a.txt".to_string()],
        ..Options::default()
    };
    let text =
        svn_or_hg_diff_text_with(&dir, &hg::HgCommand::default(), &svn, &options, "").unwrap();
    assert!(text.contains("Index: a.txt"), "{text:?}");
}

#[test]
#[cfg(not(windows))]
fn t1170_inspect_full_text_svn_with_modes() {
    // A nonempty svn diff passes through; an empty one reports no changes.
    for (tag, body, expect_ok) in [
        ("t1170-textsvn-full", "Index: a.txt\n", true),
        ("t1170-textsvn-empty", "", false),
    ] {
        let dir = scratch_dir(tag);
        std::fs::create_dir_all(dir.join(".svn")).unwrap();
        std::fs::write(dir.join("svn.diff"), body).unwrap();
        let sh = dir.join("fakesvn.sh");
        std::fs::write(&sh, "#!/bin/sh\ncat svn.diff\n").unwrap();
        t1170_make_executable(&sh);
        let svn = svn::SvnCommand::custom(&sh.to_string_lossy(), &[]);
        let out = inspect_full_text_svn_with(&dir, &defaults(), &hg::HgCommand::default(), &svn);
        if expect_ok {
            assert_eq!(out.unwrap(), body.as_bytes());
        } else {
            assert_eq!(out.unwrap_err(), diff_local::NO_CHANGES_FOUND);
        }
    }
}

#[test]
#[cfg(not(windows))]
fn t1170_collect_svn_submit_changes_success() {
    // The per-path fan-out converts one modified file to one change.
    let dir = scratch_dir("t1170-svnfanout");
    std::fs::create_dir_all(dir.join(".svn")).unwrap();
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    let script = dir.join("fakesvn.sh");
    std::fs::write(&script, include_str!("../tests/fixtures/vcs/fakesvn-d5.sh")).unwrap();
    t1170_make_executable(&script);
    let svn = svn::SvnCommand::custom(&script.to_string_lossy(), &[]);
    std::fs::write(
        dir.join(".fakesvn-d5-status"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<status>\n<target path=\".\">\n<entry path=\"a.txt\">\n<wc-status props=\"none\" item=\"modified\" revision=\"9\">\n</wc-status>\n</entry>\n</target>\n</status>\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(".fakesvn-d5-info-a.txt"),
        "Path: a.txt\nURL: https://svn.example.test/repo/trunk/a.txt\nRepository Root: https://svn.example.test/repo\nRevision: 9\nNode Kind: file\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(".fakesvn-d5-diff-a.txt"),
        "Index: a.txt\n===================================================================\n--- a.txt\t(revision 9)\n+++ a.txt\t(working copy)\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let changes = collect_svn_submit_changes(&dir, &svn, &defaults(), &mut io).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "a.txt");
}

#[test]
fn t1170_resolve_create_message_user_path() {
    // Without `--message-file` the interactive cycle builds the message.
    let root = scratch_dir("t1170-cmuser");
    let meta = root.join(".git");
    write_scratch_file(
        &meta,
        diff_local::SCRATCH_CREATE_MESSAGE,
        "Saved title\nBody\n",
    );
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "X"}}))
    };
    let edit = |template: &str| -> Result<String, String> {
        assert!(template.contains("Saved title"), "{template}");
        Ok("Edited title\n".to_string())
    };
    let mut reader = std::io::BufReader::new(std::io::Cursor::new(b"y\n".to_vec()));
    let mut writer = Vec::new();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        work_root: &root,
        edit: &edit,
        amend_commit: None,
        user_phid: "PHID-USER-1",
    };
    let local = vec![d8_local_commit("abcdef1234567890", "Subj", "Subj\n")];
    let resolved = resolve_create_message(&mut context, &defaults(), &local, Some("main")).unwrap();
    assert_eq!(resolved.corpus, "Edited title\n");
    assert_eq!(resolved.footer_revision_id, None);
    assert_eq!(
        resolved.fields.get("title"),
        Some(&diff_local::FieldValue::Text("X".to_string()))
    );
}

fn t1170_file_message_call()
-> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    |method: &str, _: &serde_json::Value| {
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "Filed title"}}))
    }
}

#[test]
fn t1170_resolve_create_message_online_git() {
    // One included git commit hooks the amend path; `--no-amend` drops it.
    let repo = t1170_seed_git_repo("t1170-cmonline", 2);
    std::fs::write(repo.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    let meta = git_metadata_dir(&repo).unwrap();
    let path = repo.join("msg.txt");
    std::fs::write(&path, "Filed title\n\nSummary: s\n").unwrap();
    let base = t1170_rev(&repo, "HEAD~1");
    // The hook itself fires only on the interactive path; both modes
    // resolve the same file message here.
    for (tag, no_amend) in [("hook", false), ("nohook", true)] {
        let options = Options {
            message_file: Some(path.to_string_lossy().into_owned()),
            no_amend,
            ..defaults()
        };
        let call = t1170_file_message_call();
        let resolved = resolve_create_message_online(
            &repo,
            &hg::HgCommand::default(),
            &options,
            &call,
            Some(VcsKind::Git),
            &meta,
            &base,
            None,
            Some(""),
        )
        .unwrap();
        assert_eq!(resolved.corpus, "Filed title\n\nSummary: s\n", "{tag}");
        assert_eq!(resolved.footer_revision_id, None, "{tag}");
    }
}

#[test]
fn t1170_resolve_submit_create_redirect_matrix() {
    // Non-create modes pass through untouched.
    let repo = t1170_seed_git_repo("t1170-redir", 2);
    let call = t1170_file_message_call();
    let base = t1170_rev(&repo, "HEAD~1");
    let redirect =
        resolve_submit_create_redirect(&repo, &defaults(), &call, &SubmitMode::Only, &base, None)
            .unwrap();
    assert_eq!(redirect.mode, SubmitMode::Only);
    assert_eq!(redirect.corpus, None);
    assert!(redirect.footer.is_none());
    // A plain message file resolves to a create corpus ...
    let path = repo.join("msg.txt");
    std::fs::write(&path, "Filed title\n\nSummary: s\n").unwrap();
    let options = Options {
        message_file: Some(path.to_string_lossy().into_owned()),
        ..defaults()
    };
    let redirect =
        resolve_submit_create_redirect(&repo, &options, &call, &SubmitMode::Create, &base, None)
            .unwrap();
    assert_eq!(redirect.mode, SubmitMode::Create);
    assert_eq!(
        redirect.corpus.as_deref(),
        Some("Filed title\n\nSummary: s\n")
    );
    assert!(redirect.footer.is_none());
    // ... while a footer revision redirects to that update.
    std::fs::write(&path, "Filed title\n\nDifferential Revision: D9\n").unwrap();
    let call = |method: &str, _: &serde_json::Value| {
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        if method == diff_submit::METHOD_QUERY {
            return Ok(serde_json::json!([{
                "id": 9, "title": "Nine", "authorPHID": "PHID-USER-1",
            }]));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "Filed title"}}))
    };
    let redirect =
        resolve_submit_create_redirect(&repo, &options, &call, &SubmitMode::Create, &base, None)
            .unwrap();
    assert_eq!(redirect.mode, SubmitMode::Update(9));
    assert_eq!(redirect.corpus, None);
    assert_eq!(redirect.footer.as_ref().unwrap().revision_id, 9);
}

#[test]
fn t1170_apply_footer_redirect_matrix() {
    // A footer redirect resolves fields plus message; `--verbatim`
    // seeds the edit template with the pulled fields.
    let repo = t1170_seed_git_repo("t1170-footer", 1);
    for (verbatim, expected_fields) in [
        (false, serde_json::json!({})),
        (true, serde_json::json!({"title": "Pulled"})),
    ] {
        let call = |method: &str, params: &serde_json::Value| {
            if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
                assert_eq!(params["revision_id"], serde_json::json!(7));
                assert_eq!(params["fields"], expected_fields);
                return Ok(serde_json::json!("Seven\n\nSummary: s\n"));
            }
            assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
            Ok(serde_json::json!({"fields": {"title": "Seven"}}))
        };
        let mut options = update_test_options(Some("m"), false);
        options.update = None;
        options.verbatim = verbatim;
        let mut input = create_test_input(false, None);
        let redirect = FooterRedirect {
            revision_id: 7,
            fields: diff_local::parse_field_tags("Pulled\n"),
        };
        let skip =
            apply_footer_redirect(&repo, &options, &call, &mut input, &Some(redirect)).unwrap();
        assert!(skip);
        assert_eq!(input.fields, serde_json::json!({"title": "Seven"}));
        assert_eq!(input.message.as_deref(), Some("m"));
    }
    // Without a redirect there is nothing to pre-resolve.
    let call = t1170_no_conduit;
    let mut input = create_test_input(false, None);
    let skip = apply_footer_redirect(&repo, &defaults(), &call, &mut input, &None).unwrap();
    assert!(!skip);
}

#[test]
fn t1170_apply_client_update_resolution_standard() {
    // The standard `--update` target resolves fields plus message.
    let repo = t1170_seed_git_repo("t1170-updres", 1);
    let call = update_stub_call();
    let options = update_test_options(Some("take two"), false);
    let mut input = create_test_input(false, None);
    input.mode = SubmitMode::Update(5);
    apply_client_update_resolution(
        &repo,
        &hg::HgCommand::default(),
        &options,
        &call,
        Some(VcsKind::Git),
        &mut input,
        5,
    )
    .unwrap();
    assert_eq!(
        input.fields,
        serde_json::json!({"title": "Thing", "summary": "yes"})
    );
    assert_eq!(input.message.as_deref(), Some("take two"));
}

fn t1170_after_create_call()
-> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        assert_eq!(params["revision_id"], serde_json::json!(7));
        Ok(serde_json::json!("new\n\nDifferential Revision: D7\n"))
    }
}

#[test]
fn t1170_amend_after_create_unsupported_kinds_skip() {
    // Svn sources and a missing working copy cannot amend: the notice
    // prints outside `--json` and the submit continues.
    let dir = scratch_dir("t1170-amendunsupported");
    let svn = scratch_dir("t1170-amendsvn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    std::fs::write(svn.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    assert!(!history_immutable_for(&svn, Some(VcsKind::Svn)));
    for (cwd, kind) in [(&svn, Some(VcsKind::Svn)), (&dir, None)] {
        let call = t1170_after_create_call();
        amend_after_create(cwd, &hg::HgCommand::default(), &defaults(), &call, 7, kind).unwrap();
    }
    // Under `--json` the notice is discarded with the report.
    let call = t1170_after_create_call();
    let options = Options {
        json: true,
        ..defaults()
    };
    amend_after_create(&dir, &hg::HgCommand::default(), &options, &call, 7, None).unwrap();
}

#[test]
#[cfg(not(windows))]
fn t1170_amend_after_create_hg_modes() {
    // A mutable hg history amends through the backend ...
    let dir = scratch_dir("t1170-amendhg");
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    std::fs::write(dir.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    assert!(!history_immutable_for(&dir, Some(VcsKind::Hg)));
    let sh = dir.join("fakehg.sh");
    std::fs::write(
        &sh,
        "#!/bin/sh\necho \"$*\" >> hg.log\ncase \"$*\" in\n*help*) echo 'commit --amend';;\n*commit*) exit 0;;\n*) exit 0;;\nesac\n",
    )
    .unwrap();
    t1170_make_executable(&sh);
    let hg = hg::HgCommand::custom(&sh.to_string_lossy(), &[]);
    let call = t1170_after_create_call();
    amend_after_create(&dir, &hg, &defaults(), &call, 7, Some(VcsKind::Hg)).unwrap();
    let log = std::fs::read_to_string(dir.join("hg.log")).unwrap();
    assert!(log.contains("help commit"), "{log}");
    assert!(log.contains("--amend"), "{log}");
    // ... a failing amend surfaces its stderr ...
    let bad = dir.join("badhg.sh");
    std::fs::write(&bad, "#!/bin/sh\necho 'stuck' >&2\nexit 1\n").unwrap();
    t1170_make_executable(&bad);
    // `help commit` fails too here, so amend support is missing: the
    // run skips instead of erroring.
    let call = t1170_after_create_call();
    amend_after_create(
        &dir,
        &hg::HgCommand::custom(&bad.to_string_lossy(), &[]),
        &defaults(),
        &call,
        7,
        Some(VcsKind::Hg),
    )
    .unwrap();
    // ... and an old hg without `commit --amend` skips the rewrite.
    let old = dir.join("oldhg.sh");
    std::fs::write(
        &old,
        "#!/bin/sh\necho \"$*\" >> oldhg.log\ncase \"$*\" in\n*help*) echo 'commit: plain help output';;\n*) echo 'must not rewrite' >&2; exit 1;;\nesac\n",
    )
    .unwrap();
    t1170_make_executable(&old);
    let call = t1170_after_create_call();
    amend_after_create(
        &dir,
        &hg::HgCommand::custom(&old.to_string_lossy(), &[]),
        &defaults(),
        &call,
        7,
        Some(VcsKind::Hg),
    )
    .unwrap();
    let log = std::fs::read_to_string(dir.join("oldhg.log")).unwrap();
    assert!(log.contains("help commit"), "{log}");
    assert!(!log.contains("--amend"), "{log}");
}

/* T1170 wave-3: loopback Conduit stub for the client-bound submit paths. */

/// Routes one `POST /api/<method>` to its canned `result`, or to an
/// `(error_code, error_info)` failure. Total: unknown methods must fail
/// loudly, never panic (panics in the server thread would surface as
/// transport errors).
type MockRoute =
    dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, (String, String)> + Send + Sync;

/// Route guard for [`MockConduit`] servers that must see no traffic: any
/// request fails the test loudly. (Each multi-route server instead ends
/// its chain with an `assert_eq!` on the last method, so no fallback arm
/// sits uncovered.)
fn t1170_no_traffic(
    method: &str,
    _: &serde_json::Value,
) -> Result<serde_json::Value, (String, String)> {
    panic!("t1170: no route for {method}")
}

#[test]
#[should_panic(expected = "t1170: no route")]
fn t1170_no_traffic_guard_fails_loudly() {
    let _ = t1170_no_traffic("bogus.method", &serde_json::json!({}));
}

/// Never-call conduit guard for scenarios that must stay offline: any
/// request fails the test loudly. A plain `fn` (not a closure) so the
/// higher-ranked `Fn(&str, &Value)` bound is satisfied at every use.
/// Covered by [`t1170_no_conduit_guard_fails_loudly`].
fn t1170_no_conduit(method: &str, _: &serde_json::Value) -> Result<serde_json::Value, String> {
    panic!("T1170: unexpected conduit call {method}")
}

#[test]
#[should_panic(expected = "T1170: unexpected conduit call")]
fn t1170_no_conduit_guard_fails_loudly() {
    let _ = t1170_no_conduit("bogus.method", &serde_json::json!({}));
}

/// A hermetic Conduit server: loopback-only, ephemeral port, canned
/// JSON envelopes. No external network.
struct MockConduit {
    addr: std::net::SocketAddr,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    calls: std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
}

impl MockConduit {
    fn start(
        route: impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, (String, String)>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let route: std::sync::Arc<MockRoute> = std::sync::Arc::new(route);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let thread = std::thread::spawn({
            let stop = std::sync::Arc::clone(&stop);
            let calls = std::sync::Arc::clone(&calls);
            move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => Self::handle(stream, &calls, route.as_ref()),
                        // Nonblocking poll: WouldBlock is the idle beat; a
                        // real accept error just retries (stop() bounds us).
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
                    }
                }
            }
        });
        Self {
            addr,
            stop,
            thread: Some(thread),
            calls,
        }
    }

    fn client(&self) -> conduit::Client {
        conduit::Client::new(&format!("http://{}/", self.addr), "test-token").unwrap()
    }

    fn calls(&self) -> Vec<(String, serde_json::Value)> {
        self.calls.lock().unwrap().clone()
    }

    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }

    fn handle(
        mut stream: std::net::TcpStream,
        calls: &std::sync::Mutex<Vec<(String, serde_json::Value)>>,
        route: &MockRoute,
    ) {
        use std::io::{Read, Write};
        // Accepted streams inherit the listener's nonblocking mode;
        // serve each connection blocking (the read timeout bounds it).
        stream.set_nonblocking(false).ok();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let mut buf = [0u8; 4096];
        let Some(raw) = Self::read_head(&mut stream, &mut buf) else {
            return;
        };
        let head_end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
        let method = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|path| path.strip_prefix("/api/"))
            .unwrap_or_default()
            .to_string();
        let length = head
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                if key.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .next()
            .unwrap_or(0);
        let mut body = raw[head_end..].to_vec();
        while body.len() < length {
            // Hermetic loopback: reads fail only on test bugs (loud).
            let count = stream.read(&mut buf).unwrap();
            if count == 0 {
                break;
            }
            body.extend_from_slice(&buf[..count]);
        }
        let params = Self::form_params(&body);
        calls.lock().unwrap().push((method.clone(), params.clone()));
        let envelope = match route(&method, &params) {
            Ok(result) => serde_json::json!({
                "error_code": null,
                "error_info": null,
                "result": result,
            }),
            Err((code, info)) => serde_json::json!({
                "error_code": code,
                "error_info": info,
                "result": null,
            }),
        };
        let text = serde_json::to_string(&envelope).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len(),
        );
        stream.write_all(response.as_bytes()).ok();
    }

    /// Read through the end of the HTTP headers (`\r\n\r\n`), or None
    /// when the stream ends or exceeds the cap first.
    fn read_head(stream: &mut std::net::TcpStream, buf: &mut [u8; 4096]) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut raw = Vec::new();
        loop {
            // Hermetic loopback: reads fail only on test bugs (loud).
            let count = stream.read(buf).unwrap();
            if count == 0 {
                return None;
            }
            raw.extend_from_slice(&buf[..count]);
            if raw.windows(4).any(|window| window == b"\r\n\r\n") {
                return Some(raw);
            }
            if raw.len() > 1_000_000 {
                return None;
            }
        }
    }

    /// The `params` form field, percent-decoded into JSON.
    fn form_params(body: &[u8]) -> serde_json::Value {
        let text = String::from_utf8_lossy(body);
        for pair in text.split('&') {
            if let Some(encoded) = pair.strip_prefix("params=")
                && let Ok(value) = serde_json::from_str(&Self::percent_decode(encoded))
            {
                return value;
            }
        }
        serde_json::Value::Null
    }

    fn percent_decode(text: &str) -> String {
        fn hex(byte: u8) -> Option<u8> {
            match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            }
        }
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%'
                && index + 2 < bytes.len()
                && let Some(high) = hex(bytes[index + 1])
                && let Some(low) = hex(bytes[index + 2])
            {
                out.push(high << 4 | low);
                index += 3;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

impl Drop for MockConduit {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn t1170_mock_conduit_tolerates_aborted_and_oversized_requests() {
    use std::io::Write;
    let mut mock = MockConduit::start(|_, _| Ok(serde_json::json!({"ok": true})));
    // Abort mid-body: headers promise more than ever arrives.
    let mut hung = std::net::TcpStream::connect(mock.addr).unwrap();
    hung.write_all(b"POST /api/x HTTP/1.1\r\nContent-Length: 100\r\n\r\nshort")
        .unwrap();
    drop(hung);
    // A headless megabyte trips the read cap.
    let mut big = std::net::TcpStream::connect(mock.addr).unwrap();
    big.write_all(&vec![b'x'; 1_100_000]).unwrap();
    drop(big);
    // A bare close ends the head read.
    drop(std::net::TcpStream::connect(mock.addr).unwrap());
    // The blocking round trip below is the barrier: the server answers
    // connections in accept order, so when it lands, the three aborted
    // connections above are fully handled too. Its 9KB body also forces
    // the multi-read body loop (reads top out at 4KB).
    let spool = "v".repeat(9000);
    let out = mock
        .client()
        .call("x", &serde_json::json!({"p": spool}))
        .unwrap();
    assert_eq!(out, serde_json::json!({"ok": true}));
    // Only the aborted-body and round-trip requests carried a head.
    assert_eq!(mock.calls().len(), 2);
    mock.stop();
}

#[test]
fn t1170_percent_decode_handles_lowercase_and_stray_percent() {
    assert_eq!(MockConduit::percent_decode("%3a%3A%zz%"), "::%zz%");
}

fn t1170_creatediff_result() -> serde_json::Value {
    // No `phid`: old servers skip the autotarget upload.
    serde_json::json!({"diffid": 3, "uri": "https://phab.test/diff/3/"})
}

#[test]
fn t1170_stamp_repository_phid_matrix() {
    // Raw sources never stamp, without any conduit traffic.
    let mut mock = MockConduit::start(t1170_no_traffic);
    let client = mock.client();
    let repo = t1170_seed_git_repo("t1170-stamp", 1);
    let mut spec = serde_json::json!({});
    stamp_repository_phid(&client, &repo, true, &mut spec).unwrap();
    assert_eq!(spec, serde_json::json!({}));
    assert!(mock.calls().is_empty());
    // A remote-less checkout identifies no repository, also quietly.
    stamp_repository_phid(&client, &repo, false, &mut spec).unwrap();
    assert_eq!(spec, serde_json::json!({}));
    assert!(mock.calls().is_empty());
    mock.stop();
    // A unique repository match stamps its PHID into the spec.
    let remote = std::process::Command::new("git")
        .current_dir(&repo)
        .args([
            "remote",
            "add",
            "origin",
            "https://phab.test/diffusion/X.git",
        ])
        .status()
        .unwrap();
    assert!(remote.success());
    let mut mock = MockConduit::start(|method, _| {
        assert_eq!(method, "repository.query");
        Ok(serde_json::json!([{"phid": "PHID-REPO-9"}]))
    });
    let client = mock.client();
    stamp_repository_phid(&client, &repo, false, &mut spec).unwrap();
    assert_eq!(spec["repositoryPHID"], serde_json::json!("PHID-REPO-9"));
    mock.stop();
}

fn t1170_dirty_submit_repo(tag: &str) -> PathBuf {
    let repo = t1170_seed_git_repo(tag, 2);
    // Pin amendability regardless of ambient user config (the create
    // flow amends the post-submit footer into the worktree).
    std::fs::write(repo.join(".arcconfig"), "{\"history.immutable\": false}").unwrap();
    std::fs::write(repo.join("f.txt"), "dirty work\n").unwrap();
    repo
}

fn t1170_submit_options(mode: &str) -> Options {
    Options {
        only: mode == "only",
        update: if mode == "update" {
            Some("5".to_string())
        } else {
            None
        },
        create: mode == "create",
        message: if mode == "update" {
            Some("take two".to_string())
        } else {
            None
        },
        nounit: true,
        nolint: true,
        skip_staging: true,
        ..defaults()
    }
}

#[test]
fn t1170_submit_via_client_only_mode() {
    let repo = t1170_dirty_submit_repo("t1170-subonly");
    let mock = MockConduit::start(|method, _| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        assert_eq!(method, diff_submit::METHOD_SET_DIFF_PROPERTY);
        Ok(serde_json::json!({}))
    });
    let options = t1170_submit_options("only");
    let text = submit_via_client_with_client_ansi(
        &repo,
        &options,
        &SubmitMode::Only,
        &|| Ok(mock.client()),
        false,
    )
    .unwrap();
    assert!(text.contains("Created a new Differential diff:"), "{text}");
    assert!(text.contains("f.txt"), "{text}");
}

#[test]
fn t1170_submit_via_client_update_mode() {
    let repo = t1170_dirty_submit_repo("t1170-subupd");
    let mock = MockConduit::start(|method, params| {
        if method == diff_submit::METHOD_QUERY {
            return Ok(serde_json::json!([{
                "id": 5, "title": "Thing",
                "authorPHID": "PHID-USER-1", "statusName": "Accepted",
            }]));
        }
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            assert_eq!(params["edit"], serde_json::json!(false));
            return Ok(serde_json::json!("Thing\n\nSummary: yes\n"));
        }
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({
                "fields": {"title": "Thing", "summary": "yes"},
            }));
        }
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            return Ok(serde_json::json!({}));
        }
        assert_eq!(method, diff_submit::METHOD_UPDATE_REVISION);
        assert_eq!(params["message"], serde_json::json!("take two"));
        Ok(serde_json::json!({"revisionid": 5, "uri": "https://phab.test/D5"}))
    });
    let options = t1170_submit_options("update");
    let mode = submit_mode(&options).unwrap().unwrap();
    let text =
        submit_via_client_with_client_ansi(&repo, &options, &mode, &|| Ok(mock.client()), false)
            .unwrap();
    assert!(
        text.contains("Updated an existing Differential revision:"),
        "{text}"
    );
}

#[test]
fn t1170_submit_via_client_create_mode_amends() {
    let repo = t1170_dirty_submit_repo("t1170-subcreate");
    let path = repo.join("msg.txt");
    std::fs::write(&path, "Filed title\n\nSummary: s\n").unwrap();
    let mock = MockConduit::start(|method, params| {
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({"fields": {"title": "Filed title"}}));
        }
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            return Ok(serde_json::json!(
                "Amended title\n\nDifferential Revision: D11\n"
            ));
        }
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            return Ok(serde_json::json!({}));
        }
        assert_eq!(method, diff_submit::METHOD_CREATE_REVISION);
        assert_eq!(params["diffid"], serde_json::json!(3));
        Ok(serde_json::json!({"revisionid": 11, "uri": "https://phab.test/D11"}))
    });
    let mut options = t1170_submit_options("create");
    options.message_file = Some(path.to_string_lossy().into_owned());
    let text = submit_via_client_with_client_ansi(
        &repo,
        &options,
        &SubmitMode::Create,
        &|| Ok(mock.client()),
        false,
    )
    .unwrap();
    assert!(
        text.contains("Created a new Differential revision:"),
        "{text}"
    );
    // The post-create footer amends into the worktree commit.
    let log = t1170_rev(&repo, "HEAD");
    assert!(!log.is_empty());
    let out = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["log", "-1", "--format=%B"])
        .output()
        .unwrap();
    let amended = String::from_utf8_lossy(&out.stdout);
    assert!(amended.contains("Amended title"), "{amended:?}");
}

#[test]
fn t1170_submit_prepared_skip_resolution_keeps_message() {
    // A pre-resolved footer skips update resolution: the preset message
    // rides through to `differential.updaterevision`.
    let repo = t1170_dirty_submit_repo("t1170-subskip");
    let mock = MockConduit::start(|method, _| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            return Ok(serde_json::json!({}));
        }
        assert_eq!(method, diff_submit::METHOD_UPDATE_REVISION);
        Ok(serde_json::json!({"revisionid": 5, "uri": "https://phab.test/D5"}))
    });
    let options = t1170_submit_options("update");
    let client = mock.client();
    let mut input = prepare_submit_with_report(
        &repo,
        &options,
        &SubmitMode::Only,
        &diff_local::LintUnitReport::empty(),
    )
    .unwrap();
    input.mode = SubmitMode::Update(5);
    input.message = Some("preset".to_string());
    let text = submit_prepared_via_client_ansi(
        &repo,
        &hg::HgCommand::default(),
        &options,
        &client,
        &mut input,
        Some(VcsKind::Git),
        false,
        true,
        false,
    )
    .unwrap();
    assert!(
        text.contains("Updated an existing Differential revision:"),
        "{text}"
    );
    let update = mock
        .calls()
        .into_iter()
        .find(|(method, _)| method == diff_submit::METHOD_UPDATE_REVISION)
        .unwrap();
    assert_eq!(update.1["message"], serde_json::json!("preset"));
}

#[test]
fn t1170_submit_via_client_factory_error_propagates() {
    let repo = t1170_dirty_submit_repo("t1170-subfactory");
    let options = t1170_submit_options("only");
    let err = submit_via_client_with_client_ansi(
        &repo,
        &options,
        &SubmitMode::Only,
        &|| Err("no client".to_string()),
        false,
    )
    .unwrap_err();
    assert_eq!(err, "no client");
}

#[test]
fn t1170_submit_bare_git_modes() {
    // `--verbatim` creates without detection; the dead-port URI then
    // fails credential lookup.
    let repo = t1170_dirty_submit_repo("t1170-bareverb");
    let range = Range {
        base: t1170_rev(&repo, "HEAD"),
        head: None,
    };
    let options = Options {
        verbatim: true,
        conduit_uri: Some("http://127.0.0.1:9/".to_string()),
        ..defaults()
    };
    let err = submit_bare_git_ansi(&repo, &options, &range, false).unwrap_err();
    assert!(err.contains("no token for"), "{err}");
    // Without `--verbatim`, an empty working-copy lookup creates; the
    // inner submit then fails the same credential lookup.
    let repo = t1170_dirty_submit_repo("t1170-barenonverb");
    let mock = MockConduit::start(|method, _| {
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        assert_eq!(method, "differential.query");
        Ok(serde_json::json!([]))
    });
    let range = Range {
        base: t1170_rev(&repo, "HEAD~1"),
        head: None,
    };
    let options = Options {
        conduit_uri: Some("http://127.0.0.1:9/".to_string()),
        ..defaults()
    };
    let err =
        submit_bare_git_with_client_ansi(&repo, &options, &range, &|| Ok(mock.client()), false)
            .unwrap_err();
    assert!(err.contains("no token for"), "{err}");
}

#[test]
fn t1170_submit_full_via_client_only_mode() {
    let repo = t1170_dirty_submit_repo("t1170-fullonly");
    let mock = MockConduit::start(|method, _| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        assert_eq!(method, diff_submit::METHOD_SET_DIFF_PROPERTY);
        Ok(serde_json::json!({}))
    });
    let options = t1170_submit_options("only");
    let text = submit_full_via_client_with_client_ansi(
        &repo,
        &options,
        &SubmitMode::Only,
        &|| Ok(mock.client()),
        false,
    )
    .unwrap();
    assert!(text.contains("Created a new Differential diff:"), "{text}");
}

#[test]
fn t1170_submit_full_via_client_update_mode() {
    let repo = t1170_dirty_submit_repo("t1170-fullupd");
    let mock = MockConduit::start(|method, params| {
        if method == diff_submit::METHOD_QUERY {
            return Ok(serde_json::json!([{
                "id": 5, "title": "Thing",
                "authorPHID": "PHID-USER-1", "statusName": "Accepted",
            }]));
        }
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            return Ok(serde_json::json!("Thing\n\nSummary: yes\n"));
        }
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({
                "fields": {"title": "Thing", "summary": "yes"},
            }));
        }
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            return Ok(serde_json::json!({}));
        }
        assert_eq!(method, diff_submit::METHOD_UPDATE_REVISION);
        assert_eq!(params["message"], serde_json::json!("take two"));
        Ok(serde_json::json!({"revisionid": 5, "uri": "https://phab.test/D5"}))
    });
    let options = t1170_submit_options("update");
    let text = submit_full_via_client_with_client_ansi(
        &repo,
        &options,
        &SubmitMode::Update(5),
        &|| Ok(mock.client()),
        false,
    )
    .unwrap();
    assert!(
        text.contains("Updated an existing Differential revision:"),
        "{text}"
    );
}

/* T1170 wave-3: final 17-line cleanup. */

#[test]
fn t1170_run_lint_unit_stdio_runs_children_without_engines() {
    // Unskipped children run in-process; with no engines configured
    // both report absence instead of prompting.
    let repo = t1170_seed_git_repo("t1170-lintrun", 1);
    let report =
        run_lint_unit_stdio_ansi(&repo, &defaults(), Some("HEAD"), None, false, false).unwrap();
    assert_eq!(report.lint_result, None);
    assert_eq!(report.unit_result, None);
}

#[test]
fn t1170_upstream_merge_base_ignores_unrelated_upstream() {
    // An upstream with no common history fails merge-base, so the base
    // falls back to HEAD.
    let repo = t1170_seed_git_repo("t1170-unrelated", 1);
    // Same content at the same second would hash identically, so root
    // the other history in different content.
    let other = t1170_seed_git_repo("t1170-unrelated-other", 0);
    std::fs::write(other.join("f.txt"), "unrelated\n").unwrap();
    for argv in [vec!["add", "f.txt"], vec!["commit", "-qm", "other root"]] {
        let status = std::process::Command::new("git")
            .current_dir(&other)
            .args(&argv)
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert_ne!(t1170_rev(&repo, "HEAD"), t1170_rev(&other, "HEAD"));
    for argv in [
        vec!["remote", "add", "origin", &other.to_string_lossy()],
        vec!["fetch", "origin"],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(&repo)
            .args(&argv)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let branch = git::branch_name(&repo).unwrap().unwrap();
    let status = std::process::Command::new("git")
        .current_dir(&repo)
        .args([
            "branch",
            "--set-upstream-to",
            &format!("origin/{branch}"),
            &branch,
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(upstream_merge_base(&repo), None);
    assert_eq!(resolve_base(&repo, None).unwrap(), t1170_rev(&repo, "HEAD"));
}

#[test]
fn t1170_inspect_full_submit_arm_reports_client_error() {
    // The full pipeline's submit arm builds its client through the
    // same factory seam as the baseline one.
    let repo = t1170_dirty_submit_repo("t1170-fullsubmitarm");
    let options = Options {
        only: true,
        conduit_uri: Some("http://127.0.0.1:9/".to_string()),
        ..defaults()
    };
    let err = inspect_full_ansi(&repo, &options, false).unwrap_err();
    assert!(err.contains("no token for"), "{err}");
}

#[test]
fn t1170_prepare_submit_full_raw_command_failure_propagates() {
    // A failing `--raw-command` aborts the full change collection.
    let dir = scratch_dir("t1170-rawfail");
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut io = diff_local::SvnPromptIo {
        reader: &mut reader,
        writer: &mut writer,
        warned_externals: false,
    };
    let options = Options {
        raw_command: Some("definitely-not-a-command-t1170".to_string()),
        ..defaults()
    };
    let err = prepare_submit_full_with_commands_io(
        &dir,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        &options,
        &SubmitMode::Only,
        &diff_local::LintUnitReport::empty(),
        &mut io,
    )
    .unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn t1170_resolve_submit_create_redirect_viewer_failure_propagates() {
    // A failing viewer lookup aborts create resolution.
    let repo = t1170_seed_git_repo("t1170-redirfail", 2);
    let path = repo.join("msg.txt");
    std::fs::write(&path, "Filed title\n").unwrap();
    let options = Options {
        message_file: Some(path.to_string_lossy().into_owned()),
        ..defaults()
    };
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "user.whoami");
        Err("whoami down".to_string())
    };
    let base = t1170_rev(&repo, "HEAD~1");
    let err =
        resolve_submit_create_redirect(&repo, &options, &call, &SubmitMode::Create, &base, None)
            .err()
            .expect("expected the viewer failure to propagate");
    assert_eq!(err, "whoami down");
}

#[test]
fn t1170_apply_footer_redirect_template_failure_propagates() {
    // A failing template fetch aborts the footer update.
    let repo = t1170_seed_git_repo("t1170-footerfail", 1);
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        Err("template down".to_string())
    };
    let options = update_test_options(Some("m"), false);
    let mut input = create_test_input(false, None);
    let redirect = FooterRedirect {
        revision_id: 7,
        fields: diff_local::parse_field_tags("Title\n"),
    };
    let err =
        apply_footer_redirect(&repo, &options, &call, &mut input, &Some(redirect)).unwrap_err();
    assert_eq!(err, "template down");
}

#[test]
fn t1170_submit_prepared_finalize_failure_propagates() {
    // A failing post-create fetch aborts finalization after the
    // revision exists.
    let repo = t1170_dirty_submit_repo("t1170-finfail");
    let mock = MockConduit::start(|method, _| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            return Ok(serde_json::json!({}));
        }
        if method == diff_submit::METHOD_CREATE_REVISION {
            return Ok(serde_json::json!({"revisionid": 11, "uri": "https://phab.test/D11"}));
        }
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        Err(("ERR-BOOM".to_string(), "fetch down".to_string()))
    });
    let client = mock.client();
    let options = t1170_submit_options("only");
    let mut input = prepare_submit_with_report(
        &repo,
        &options,
        &SubmitMode::Only,
        &diff_local::LintUnitReport::empty(),
    )
    .unwrap();
    input.mode = SubmitMode::Create;
    let err = submit_prepared_via_client_ansi(
        &repo,
        &hg::HgCommand::default(),
        &options,
        &client,
        &mut input,
        Some(VcsKind::Git),
        false,
        true,
        false,
    )
    .unwrap_err();
    assert!(err.contains("fetch down"), "{err}");
}

#[test]
fn t1170_resolve_update_revision_editor_failure_propagates() {
    // A failing editor aborts the remote-corpus edit.
    let meta = scratch_dir("t1170-updedit");
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            return Ok(serde_json::json!("Base\n"));
        }
        assert_eq!(method, diff_submit::METHOD_PARSE_COMMIT_MESSAGE);
        Ok(serde_json::json!({"fields": {"title": "Base"}}))
    };
    let edit = |_: &str, _: &str| -> Result<String, String> { Err("editor down".to_string()) };
    let prefill = || -> Result<Option<String>, String> { Ok(None) };
    let mut reader = std::io::BufReader::new(std::io::empty());
    let mut writer = Vec::new();
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call: &call,
        metadata_dir: &meta,
        edit: &edit,
        prefill: &prefill,
    };
    let options = update_test_options(Some("m"), true);
    let target = UpdateTarget {
        revision_id: 5,
        revision_arg: "5",
        verbatim_fields: &serde_json::json!({}),
        check_ownership: false,
        edit_fields: None,
    };
    let err = resolve_update_revision(&mut resolver, &options, &target).unwrap_err();
    assert_eq!(err, "editor down");
}

#[test]
fn t1170_submit_bare_detection_failure_propagates() {
    // An unresolvable range fails detection before the inner submit.
    let repo = t1170_seed_git_repo("t1170-barefail", 1);
    let mock = MockConduit::start(|method, _| {
        assert_eq!(method, "user.whoami");
        Ok(serde_json::json!({"phid": "PHID-USER-1"}))
    });
    let range = Range {
        base: "bogus-base-t1170".to_string(),
        head: None,
    };
    let err =
        submit_bare_git_with_client_ansi(&repo, &defaults(), &range, &|| Ok(mock.client()), false)
            .unwrap_err();
    assert!(err.contains("git log failed"), "{err}");
}

#[test]
fn t1170_submit_full_finalize_failure_propagates() {
    // A failing post-create fetch aborts full finalization too.
    let repo = t1170_dirty_submit_repo("t1170-fullfinfail");
    let mock = MockConduit::start(|method, _| {
        if method == diff_submit::METHOD_CREATE_DIFF {
            return Ok(t1170_creatediff_result());
        }
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            return Ok(serde_json::json!({}));
        }
        if method == diff_submit::METHOD_CREATE_REVISION {
            return Ok(serde_json::json!({"revisionid": 11, "uri": "https://phab.test/D11"}));
        }
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        Err(("ERR-BOOM".to_string(), "fetch down".to_string()))
    });
    let options = t1170_submit_options("create");
    let err = submit_full_via_client_with_client_ansi(
        &repo,
        &options,
        &SubmitMode::Create,
        &|| Ok(mock.client()),
        false,
    )
    .unwrap_err();
    assert!(err.contains("fetch down"), "{err}");
}

#[test]
fn t1170_maybe_apply_full_update_failure_propagates() {
    // A failing update lookup aborts the full update application.
    let repo = t1170_seed_git_repo("t1170-fullupdfail", 1);
    let call = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_QUERY);
        Err("query down".to_string())
    };
    let options = update_test_options(Some("m"), false);
    let mut input = create_test_input(false, None);
    input.mode = SubmitMode::Update(5);
    let err = maybe_apply_full_update(&repo, &options, &call, Some(VcsKind::Git), &mut input)
        .unwrap_err();
    assert_eq!(err, "query down");
}

#[test]
fn t1173_stdio_split_phases_report_empty_without_engines() {
    // T1173: `run_lint_unit_stdio_ansi` runs the lint phase first with
    // no stdin guard held, then locks stdin fresh for the unit phase,
    // assembling the same `LintUnitReport` the combined
    // `run_lint_unit_ansi` would. A marker-only repo has neither
    // engine, so both real children report `NoEngine` and the report
    // is empty.
    let dir = scratch_dir("t1173-noengines");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let report = run_lint_unit_stdio_ansi(&dir, &defaults(), None, None, false, false).unwrap();
    assert_eq!(report, diff_local::LintUnitReport::empty());
}

#[test]
fn t1173_stdio_split_phases_report_skip_when_flagged() {
    // T1173: `--nolint`/`--nounit` skip each split phase exactly like
    // the combined `run_lint_unit_ansi` (same order, flags, report).
    let dir = scratch_dir("t1173-skip");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let options = Options {
        nolint: true,
        nounit: true,
        ..defaults()
    };
    let report = run_lint_unit_stdio_ansi(&dir, &options, None, None, false, false).unwrap();
    assert_eq!(
        report,
        diff_local::LintUnitReport {
            lint_result: Some(diff_local::LINT_RESULT_SKIP),
            unresolved_lint: Vec::new(),
            unit_result: Some(diff_local::UNIT_RESULT_SKIP),
            test_results: Vec::new(),
        }
    );
}

#[test]
fn t1173_stdio_lint_child_locks_stdin_without_deadlock() {
    // T1173 regression: the lint child locks stdin itself, so holding
    // a `StdinLock` guard across it (the pre-fix shape) deadlocks this
    // thread. An `.arclint` with zero linters resolves to an empty
    // engine, driving the real child past its probe to its own
    // `stdin.lock()` — then the workflow bails with `NoEffect` before
    // any git or prompt interaction, so this completes promptly (and
    // would hang forever on the old code).
    let dir = scratch_dir("t1173-arclint");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arclint"), "{\"linters\": {}}\n").unwrap();
    let report = run_lint_unit_stdio_ansi(&dir, &defaults(), None, None, false, false).unwrap();
    assert_eq!(report, diff_local::LintUnitReport::empty());
}

#[test]
fn t1173_stdio_lint_failure_aborts_before_unit() {
    // T1173: a failing lint child aborts the split-phase run with its
    // message, before stdin is locked for the unit phase. A bogus
    // `lint.engine` fails the probe deterministically, without
    // touching git or prompting.
    let dir = scratch_dir("t1173-lintfail");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        "{\"lint.engine\": \"BogusEngine\"}\n",
    )
    .unwrap();
    let err = run_lint_unit_stdio_ansi(&dir, &defaults(), None, None, false, false).unwrap_err();
    assert_eq!(
        err,
        "Configured lint engine \"BogusEngine\" is not a subclass of \"ArcanistLintEngine\", but must be."
    );
}

#[test]
fn t1173_stdio_unit_failure_aborts_after_lint() {
    // T1173: a failing unit child aborts the split-phase run with its
    // message after the lint phase completes. A bogus `unit.engine`
    // fails unit discovery deterministically (no lint engine here, so
    // the lint phase reports `NoEngine` first), without prompting. A
    // real `git init` repo is needed: the unit working-copy gate
    // rejects marker-only dirs before discovery runs.
    let dir = git_dir("t1173-unitfail");
    std::fs::write(
        dir.join(".arcconfig"),
        "{\"unit.engine\": \"BogusEngine\"}\n",
    )
    .unwrap();
    let err = run_lint_unit_stdio_ansi(&dir, &defaults(), None, None, false, false).unwrap_err();
    assert_eq!(
        err,
        "Configured unit test engine \"BogusEngine\" is not a subclass of \"ArcanistUnitTestEngine\", but must be."
    );
}
