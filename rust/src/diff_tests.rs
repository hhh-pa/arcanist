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

    if cfg!(windows) {
        assert_eq!(shell_quote("a b"), "\"a b\"");
    } else {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    assert_eq!(editor_offset_flag("vim", 0), "");
    if cfg!(windows) {
        assert_eq!(editor_offset_flag("vim", 3), "");
    } else {
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
    let append = if cfg!(windows) {
        "echo MARKED>>"
    } else {
        "printf MARKED >>"
    };
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
    if cfg!(windows) {
        assert_eq!(
            browser_open_argv("http://x/", None, &none).unwrap(),
            vec![
                "cmd".to_string(),
                "/c".to_string(),
                "start".to_string(),
                "http://x/".to_string()
            ]
        );
    } else {
        assert_eq!(
            browser_open_argv("http://x/", None, &none).unwrap_err(),
            NO_BROWSER_MESSAGE
        );
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
    let calls = RefCell::new(0);
    let counting = |_: &str, _: &[String]| {
        *calls.borrow_mut() += 1;
        Ok(())
    };
    open_submit_result_in_browser(None, true, None, &none, &counting).unwrap();
    open_submit_result_in_browser(Some("http://x/"), false, None, &none, &counting).unwrap();
    assert_eq!(*calls.borrow(), 0);
    // --browse with a URI opens through the resolved browser argv.
    let seen: RefCell<Vec<(String, Vec<String>)>> = RefCell::new(Vec::new());
    let recording = |uri: &str, browser: &[String]| {
        seen.borrow_mut().push((uri.to_string(), browser.to_vec()));
        Ok(())
    };
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
    if cfg!(windows) {
        open_submit_result_in_browser(Some("http://x/"), true, None, &none, &ok_run).unwrap();
    } else {
        assert_eq!(
            open_submit_result_in_browser(Some("http://x/"), true, None, &none, &ok_run)
                .unwrap_err(),
            NO_BROWSER_MESSAGE
        );
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
fn g1_prompt_confirm_requires_tty() {
    if !cfg!(unix) {
        return; // PHP has no TTY gate without posix_isatty (Windows).
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return; // Interactive session: the TTY gate cannot fail deterministically.
    }
    assert_eq!(
        prompt_confirm("Continue?", true).unwrap_err(),
        STDIN_NOT_TTY
    );
}

#[test]
fn gap2_tty_gate_matches_php_platform_semantics() {
    // PHP `phutil_console_require_tty()`: gate exists only where
    // `posix_isatty` exists; elsewhere prompting reads stdin directly.
    if cfg!(unix) {
        assert_eq!(
            super::tty_gate_blocks(),
            !std::io::IsTerminal::is_terminal(&std::io::stdin())
        );
    } else {
        assert!(!super::tty_gate_blocks());
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
    let summary = match fields.get(diff_local::FIELD_SUMMARY) {
        Some(diff_local::FieldValue::Text(text)) => text.clone(),
        other => panic!("missing summary: {other:?}"),
    };
    assert!(summary.contains("Old body"), "{summary}");
    assert!(summary.contains("Newest title"), "{summary}");
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
        if method == diff_submit::METHOD_CREATE_DIFF {
            Ok(serde_json::json!({"diffid": 1, "uri": "http://x/D1"}))
        } else if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            Ok(serde_json::Value::Null)
        } else {
            Err(format!("unexpected {method}"))
        }
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
        if method == diff_submit::METHOD_REVISION_EDIT {
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
            return Ok(serde_json::json!({"object": {"id": 7}}));
        }
        Err(format!("unexpected {method}"))
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
        if method == diff_submit::METHOD_REVISION_EDIT {
            return Ok(serde_json::json!({"object": {"id": 8}}));
        }
        Err(format!("unexpected {method}"))
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
        if method == diff_submit::METHOD_CREATE_REVISION {
            assert_eq!(
                params["fields"]["reviewerPHIDs"],
                serde_json::json!(["PHID-USER-1"])
            );
            assert_eq!(params["diffid"], serde_json::json!(3));
            return Ok(serde_json::json!({"revisionid": 9, "uri": "https://phab.test/D9"}));
        }
        Err(format!("unexpected {method}"))
    };
    let input = create_test_input(false, Some("T\n\nReviewers: alice\n"));
    let revision = submit_create_revision(&call, &input, 3, None, "u").unwrap();
    assert_eq!(revision.revision_id, 9);
    assert_eq!(revision.uri, "https://phab.test/D9");
}

#[test]
fn create_draft_without_transactions_errors_like_php() {
    let call = |method: &str, _: &serde_json::Value| {
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({"fields": {"title": "T"}}));
        }
        Err(format!("unexpected {method}"))
    };
    let input = create_test_input(true, Some("T\n"));
    let err = submit_create_revision(&call, &input, 3, None, "u").unwrap_err();
    assert!(err.contains("--draft"), "unexpected: {err}");
}

#[test]
fn create_without_corpus_keeps_local_fields_legacy() {
    let call = |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_CREATE_REVISION {
            assert_eq!(params["fields"]["title"], serde_json::json!("Local"));
            return Ok(serde_json::json!({"revisionid": 1, "uri": "u"}));
        }
        Err(format!("unexpected {method}"))
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
        if method == diff_submit::METHOD_REVISION_EDIT {
            return Ok(serde_json::json!({"object": {"id": 2}}));
        }
        Err(format!("unexpected {method}"))
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
    if !cfg!(windows) {
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
        if method == diff_submit::METHOD_PARSE_COMMIT_MESSAGE {
            return Ok(serde_json::json!({
                "fields": {"title": "Thing", "summary": "yes"},
            }));
        }
        Err(format!("DIFF-D test: unexpected {method}"))
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
    let edit = |_: &str, _: &str| -> Result<String, String> {
        panic!("DIFF-D6: no editor without --edit or missing --message")
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
        if method == diff_submit::METHOD_QUERY {
            return Ok(serde_json::json!([]));
        }
        Err(format!("DIFF-D6: unexpected {method}"))
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
        if method == "user.whoami" {
            return Ok(serde_json::json!({"phid": "PHID-USER-1"}));
        }
        Err(format!("DIFF-D6: unexpected {method}"))
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
    let edit = |_: &str, _: &str| -> Result<String, String> {
        panic!("DIFF-D9: raw sources never open the editor")
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
        if method == diff_submit::METHOD_UPDATE_REVISION {
            return Ok(serde_json::json!({"revisionid": 5, "uri": "http://x/D5"}));
        }
        Err(format!("DIFF-D17: unexpected {method}"))
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
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            props
                .borrow_mut()
                .push(params["name"].as_str().unwrap_or("").to_string());
            return Ok(serde_json::json!(null));
        }
        Err(format!("DIFF-D11: unexpected {method}"))
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
        if method == diff_submit::METHOD_SET_DIFF_PROPERTY {
            props
                .borrow_mut()
                .push(params["name"].as_str().unwrap_or("").to_string());
            return Ok(serde_json::json!(null));
        }
        Err(format!("DIFF-D11: unexpected {method}"))
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
    let repo_rows = std::cell::RefCell::new(serde_json::json!([{}]));
    let call = |method: &str, params: &serde_json::Value| {
        if method == "repository.query" {
            return Ok(repo_rows.borrow().clone());
        }
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
    assert!(
        recorded[2].contains("\"repository.unknown\""),
        "{}",
        recorded[2]
    );
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
    assert!(
        payload.borrow().contains("client.unsupported"),
        "{}",
        payload.borrow()
    );
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
        if method == diff_submit::METHOD_GET_COMMIT_MESSAGE {
            panic!("DIFF-D8: saved resume must not fetch the server template");
        }
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
    let edit = |_: &str, _: &str| -> Result<String, String> {
        panic!("DIFF-D8: --message skips the editor")
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
    let edit =
        |_: &str| -> Result<String, String> { panic!("DIFF-D8: --message-file never edits") };
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
