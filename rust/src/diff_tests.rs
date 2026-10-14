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
    assert!(
        FULL_HELP.starts_with("Supports: git, svn, hg\n"),
        "{FULL_HELP}"
    );
    assert!(FULL_HELP.contains("merge base"), "{FULL_HELP}");
    assert!(FULL_HELP.contains("Under svn"), "{FULL_HELP}");

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
    let text = help_text();
    assert!(text.starts_with(USAGE), "{text:?}");
    assert!(text.contains("Supports: git, svn, hg"), "{text:?}");
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
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return; // Interactive session: the TTY gate cannot fail deterministically.
    }
    assert_eq!(
        prompt_confirm("Continue?", true).unwrap_err(),
        STDIN_NOT_TTY
    );
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
    let hunk = diff_local::ParsedHunk {
        old_start: 1,
        old_len: 2,
        new_start: 1,
        new_len: 3,
        corpus: "@@ -1,2 +1,3 @@\n a\n+b\n-c\n".to_string(),
    };
    let submit = submit_hunk_from_parsed(&hunk);
    assert_eq!(submit.old_offset, 1);
    assert_eq!(submit.old_length, 2);
    assert_eq!(submit.new_length, 3);
    assert_eq!(submit.add_lines, 1);
    assert_eq!(submit.del_lines, 1);
    assert!(!submit.is_missing_old_newline);
    assert!(!submit.is_missing_new_newline);

    let missing = diff_local::ParsedHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        corpus: "@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n".to_string(),
    };
    let submit = submit_hunk_from_parsed(&missing);
    assert!(submit.is_missing_old_newline);
    assert!(!submit.is_missing_new_newline);

    let change = diff_local::ParsedChange {
        old_path: "o.txt".to_string(),
        current_path: "n.txt".to_string(),
        change_type: diff_local::TYPE_MOVE_HERE,
        file_type: diff_local::FILE_TEXT,
        hunks: vec![hunk],
        commit_hash: None,
        message: None,
    };
    let submit = submit_change_from_parsed(&change);
    assert_eq!(submit.old_path, "o.txt");
    assert_eq!(submit.current_path, "n.txt");
    assert_eq!(submit.change_type, i64::from(diff_local::TYPE_MOVE_HERE));
    assert_eq!(submit.hunks.len(), 1);

    let entry = change_entry_from_parsed(&change);
    assert_eq!(entry.old_path.as_deref(), Some("o.txt"));
    assert_eq!(entry.current.added, 1);
    assert_eq!(entry.current.deleted, 1);

    let text = "commit abc\n\n    subject\n\ndiff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n";
    let changes = parse_raw_diff_changes(text);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].current_path, "f.txt");
    assert_eq!(changes[0].hunks.len(), 1);
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
