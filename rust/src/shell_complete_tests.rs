use super::*;
use std::io::BufReader;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-shellcomplete-{name}-{}-{}",
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
fn core66_matches_prefer_exact_then_folded() {
    let candidates = vec!["download".to_string(), "diff".to_string()];
    assert_eq!(
        get_matches(&candidates, "dow"),
        vec!["download".to_string()]
    );
    assert_eq!(
        get_matches(&candidates, "DOW"),
        vec!["download".to_string()]
    );
    assert_eq!(get_matches(&candidates, ""), candidates);
    assert!(get_matches(&candidates, "zzz").is_empty());
}

#[test]
fn core66_suggest_strings_sort_without_trailing_newline() {
    assert_eq!(
        suggest_strings(vec!["b".to_string(), "a".to_string()]),
        "a\nb"
    );
    assert_eq!(suggest_strings(Vec::new()), "");
}

#[test]
fn core66_autocomplete_rejects_positions_past_end() {
    let error = autocomplete(&args(&["arc", "land"]), Some("5"), &[]).unwrap_err();
    assert!(error.contains("greater than the number of arguments provided"));
}

#[test]
fn core66_autocomplete_completes_workflows_then_flags() {
    let out = autocomplete(&args(&["arc", "dow"]), Some("1"), &[]).unwrap();
    assert_eq!(out, "download");
    let out = autocomplete(&args(&["arc", "land", "--o"]), Some("2"), &[]).unwrap();
    assert_eq!(out, "--onto\n--onto-remote");
    // Classic workflows and unknown names complete to nothing.
    assert_eq!(
        autocomplete(&args(&["arc", "diff", "--d"]), Some("2"), &[]).unwrap(),
        ""
    );
    assert_eq!(
        autocomplete(&args(&["arc", "dif"]), Some("1"), &[]).unwrap(),
        ""
    );
    // Aliases join the workflow candidates.
    let out = autocomplete(&args(&["arc", "dra"]), Some("1"), &args(&["draft"])).unwrap();
    assert_eq!(out, "draft");
}

#[test]
fn core66_autocomplete_handles_path_params_and_wildcards() {
    // `--input` takes a path: complete files.
    let out = autocomplete(&args(&["arc", "paste", "--input", ""]), Some("3"), &[]).unwrap();
    assert_eq!(out, "<compgen:file>");
    // `--title` takes text: complete nothing.
    let out = autocomplete(&args(&["arc", "paste", "--title", ""]), Some("3"), &[]).unwrap();
    assert_eq!(out, "");
    // Path wildcards complete files on empty input.
    let out = autocomplete(&args(&["arc", "weld", ""]), Some("2"), &[]).unwrap();
    assert_eq!(out, "<compgen:file>");
}

#[test]
fn core66_shell_detection_and_selection() {
    let (status, shell) = detect_shell(Some("/bin/bash"), false);
    assert!(status.contains("DETECT"));
    assert_eq!(shell.unwrap(), "bash");
    let (status, shell) = detect_shell(None, false);
    assert!(status.contains("SHELL"));
    assert!(shell.unwrap_err().contains("Unable to detect"));
    let (_, shell) = detect_shell(Some("/bin/zsh"), false);
    assert!(shell.is_err());
    assert_eq!(select_shell("bash").unwrap(), "bash");
    assert!(select_shell("zsh").unwrap_err().contains("not supported"));
}

#[test]
fn core66_profile_line_replace_and_source_quoting() {
    let (data, removed) = replace_profile_line(
        "export A=1\nsource /old # arcanist-shell-complete\n",
        "source /new # arcanist-shell-complete",
    );
    assert!(removed.unwrap().contains("/old"));
    assert!(data.contains("/new"));
    let (data, removed) =
        replace_profile_line("export A=1\n", "source /new # arcanist-shell-complete");
    assert!(removed.is_none());
    assert_eq!(data, "export A=1\n");
    assert_eq!(
        source_line("/x/y.sh"),
        "source /x/y.sh # arcanist-shell-complete"
    );
    assert!(source_line("/x/my hook.sh").contains("source '/x/my hook.sh'"));
}

#[test]
fn core66_generate_writes_and_skips_identical_rules() {
    let root = scratch("generate");
    let first = generate_in(&root);
    assert!(first.contains("RULES"));
    assert!(root.join("rules").join("bash-rules.sh").exists());
    let second = generate_in(&root);
    assert!(second.contains("SKIP"));
    let rules = completion_rules();
    assert!(rules.contains("_arcanist_complete_arc"));
    assert!(rules.contains("_arcanist_complete_phage"));
}

#[test]
fn core66_install_creates_profile_with_prompt() {
    let home = scratch("install");
    let root = scratch("installroot");
    let mut save = |_: crate::prompts::PromptScope, _: &str| Ok(());
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    let out = install_in(
        None,
        Some("/bin/bash"),
        home.to_str(),
        &root,
        &mut input,
        true,
        None,
        &mut save,
    )
    .unwrap();
    assert!(out.contains("INSTALL"));
    assert!(out.contains("INSTALLED"));
    let profile = std::fs::read_to_string(home.join(".profile")).unwrap();
    assert!(profile.contains("# arcanist-shell-complete"));
    // Second install is already-installed.
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    let out = install_in(
        None,
        Some("/bin/bash"),
        home.to_str(),
        &root,
        &mut input,
        true,
        None,
        &mut save,
    )
    .unwrap();
    assert!(out.contains("already installed"));
}

#[test]
fn core66_install_needs_home_and_shell() {
    let root = scratch("installerr");
    let mut save = |_: crate::prompts::PromptScope, _: &str| Ok(());
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    assert!(
        install_in(
            None,
            Some("/bin/bash"),
            None,
            &root,
            &mut input,
            true,
            None,
            &mut save
        )
        .is_err()
    );
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    assert!(
        install_in(
            Some("zsh"),
            Some("/bin/bash"),
            Some("/tmp"),
            &root,
            &mut input,
            true,
            None,
            &mut save
        )
        .is_err()
    );
}
#[test]
fn r10_shell_complete_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "SHELL-COMPLETE WORKFLOW\n\
         \n\
         shell-complete\n\
         Install shell completion.\n\
         \n\
         Install shell completion so you can use the \"tab\" key to autocomplete\n\
         commands and flags in your shell for Arcanist toolsets and workflows.\n\
         \n\
         The bash shell is supported.\n\
         \n\
         Installing Completion\n\
         \n\
         To install shell completion, run the command:\n\
         \n\
         \x20 $ arc shell-complete\n\
         \n\
         This will install shell completion into your current shell. After installing,\n\
         you may need to start a new shell (or open a new terminal window) to pick up\n\
         the updated configuration.\n\
         \n\
         Once installed, completion should work across all Arcanist toolsets.\n\
         \n\
         Using Completion\n\
         \n\
         After completion is installed, use the \"tab\" key to automatically complete\n\
         workflows and flags. For example, if you type:\n\
         \n\
         \x20 $ arc diff --draf<tab>\n\
         \n\
         ...your shell should automatically expand the flag to:\n\
         \n\
         \x20 $ arc diff --draft\n\
         \n\
         Updating Completion\n\
         \n\
         To update shell completion, run the same command:\n\
         \n\
         \x20 $ arc shell-complete\n\
         \n\
         You can update shell completion without reinstalling it by running:\n\
         \n\
         \x20 $ arc shell-complete --generate\n\
         \n\
         You may need to update shell completion if:\n\
         \n\
         \x20 - you install new Arcanist toolsets; or\n\
         \x20 - you move the Arcanist directory; or\n\
         \x20 - you upgrade Arcanist and the new version fixes shell completion bugs.\n\
         \n\
         \x20     --current cursor-position\n\
         \x20         Internal. Current term in the argument list being completed.\n\
         \n\
         \x20     --generate\n\
         \x20         Regenerate shell completion rules, without installing any\n\
         \x20         configuration.\n\
         \n\
         \x20     --shell shell-name\n\
         \x20         Install completion support for a particular shell.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    assert!(wants_help(&["--help".to_string()]));
    assert!(!wants_help(&["--".to_string(), "--help".to_string()]));
    assert_eq!(
        run(&["--help".to_string()]),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn t1170_workflow_args_cover_every_shape() {
    let (flags, wildcard) = workflow_args("amend");
    assert!(!wildcard);
    assert!(flags.contains(&("show", None, false)));
    assert!(flags.contains(&("revision", Some("id"), false)));
    let (flags, wildcard) = workflow_args("upload");
    assert!(wildcard);
    assert!(flags.contains(&("temporary", None, false)));
    let (flags, wildcard) = workflow_args("work");
    assert!(!wildcard);
    assert!(flags.contains(&("start", Some("symbol"), false)));
    let (flags, wildcard) = workflow_args("prompts");
    assert!(!wildcard);
    assert!(flags.is_empty());
    let (flags, wildcard) = workflow_args("shell-complete");
    assert!(!wildcard);
    assert!(flags.contains(&("generate", None, false)));
    assert!(flags.contains(&("shell", Some("shell-name"), false)));
    let (flags, wildcard) = workflow_args("no-such-workflow");
    assert!(!wildcard);
    assert!(flags.is_empty());
}

#[test]
fn t1170_shell_roots_resolve_beside_executable() {
    assert_eq!(
        shell_root_for(Path::new("/opt/arc/bin")),
        PathBuf::from("/opt/arc/bin/support/shell")
    );
    let root = default_shell_root();
    assert!(root.ends_with(Path::new("support/shell")), "{root:?}");
}

#[test]
fn t1170_completion_cursor_handles_garbage_and_missing() {
    assert_eq!(completion_cursor(Some("abc"), 3), (0, Some(String::new())));
    assert_eq!(completion_cursor(None, 3), (2, None));
    assert_eq!(completion_cursor(Some("2"), 3), (2, None));
}

#[test]
fn t1170_valued_param_completion_skips_boolean_flags() {
    let (flags, _) = workflow_args("land");
    // After a boolean flag there is no parameter slot: fall through.
    let words = args(&["arc", "land", "--hold", ""]);
    assert_eq!(valued_param_completion(&words, 3, flags), None);
    // After a valued flag the slot completes (empty for text params).
    let words = args(&["arc", "land", "--onto", ""]);
    assert_eq!(
        valued_param_completion(&words, 3, flags),
        Some(String::new())
    );
    let (paste_flags, _) = workflow_args("paste");
    let words = args(&["arc", "paste", "--input", ""]);
    assert_eq!(
        valued_param_completion(&words, 3, paste_flags),
        Some("<compgen:file>".to_string())
    );
}

#[test]
fn t1170_install_prompt_text_covers_rewrite_and_append() {
    let (prompt, echo) = install_prompt_text(
        "bash",
        Some("source /old # arcanist-shell-complete"),
        "source /new # arcanist-shell-complete",
        false,
    );
    assert_eq!(prompt, "Rewrite this file?");
    assert!(echo.contains("will be removed"), "{echo}");
    assert!(echo.contains("/old"), "{echo}");
    let (prompt, echo) =
        install_prompt_text("bash", None, "source /new # arcanist-shell-complete", false);
    assert_eq!(prompt, "Append to this file?");
    assert!(echo.contains("will be added to your existing"), "{echo}");
    let (prompt, _) =
        install_prompt_text("bash", None, "source /new # arcanist-shell-complete", true);
    assert_eq!(prompt, "Create this file?");
}

#[test]
fn t1170_install_final_data_picks_rewrite_create_or_append() {
    assert_eq!(
        install_final_data("new".to_string(), "old", "line", true, false),
        "new"
    );
    assert_eq!(
        install_final_data("new".to_string(), "", "line", false, true),
        "line\n"
    );
    assert_eq!(
        install_final_data("new".to_string(), "old\n", "line", false, false),
        "old\n\nline\n"
    );
}

#[test]
fn t1170_install_prompt_error_maps_abort_and_fatal() {
    let abort = install_prompt_error(crate::prompt::ABORT.to_string());
    assert_eq!(abort.kind, crate::conduit::EnvelopeKind::UserAbort);
    assert_eq!(abort.message, crate::prompt::ABORT);
    let fatal = install_prompt_error("eof".to_string());
    assert_eq!(fatal.kind, crate::conduit::EnvelopeKind::Fatal);
    assert_eq!(fatal.message, "eof");
}

#[test]
fn t1170_install_reports_unwritable_profile() {
    let home = scratch("installnowrite");
    let root = scratch("installnowrite-root");
    // A directory at `.profile` makes the final write fail.
    std::fs::create_dir(home.join(".profile")).unwrap();
    let mut save = |_: crate::prompts::PromptScope, _: &str| Ok(());
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    let err = install_in(
        None,
        Some("/bin/bash"),
        home.to_str(),
        &root,
        &mut input,
        true,
        None,
        &mut save,
    )
    .unwrap_err();
    assert_eq!(err.kind, crate::conduit::EnvelopeKind::Fatal);
    assert!(err.message.contains("cannot write"), "{}", err.message);
}

#[test]
fn t1170_generate_skips_when_rules_unwritable() {
    let root = scratch("generatenowrite");
    // A file at `rules/` makes `create_dir_all` fail.
    std::fs::write(root.join("rules"), b"not a dir").unwrap();
    let out = generate_in(&root);
    assert!(out.contains("GENERATE"), "{out}");
    assert!(out.contains("SKIP"), "{out}");
    assert!(out.contains("already up to date"), "{out}");
}

#[test]
fn t1170_run_complete_rejects_install_flags_and_bad_positions() {
    use std::process::ExitCode;
    assert_eq!(
        run_complete(vec!["arc".to_string()], None, true, false),
        ExitCode::FAILURE
    );
    assert_eq!(
        run_complete(vec!["arc".to_string()], None, false, true),
        ExitCode::FAILURE
    );
    assert_ne!(
        run_complete(args(&["arc", "x"]), Some("9".to_string()), false, false),
        ExitCode::SUCCESS
    );
    assert_eq!(
        run_complete(args(&["arc", "dow"]), Some("1".to_string()), false, false),
        ExitCode::SUCCESS
    );
}

#[test]
fn t1170_run_generate_or_install_rejects_bad_flag_combos() {
    use std::process::ExitCode;
    assert_eq!(
        run_generate_or_install(true, Some("1".to_string()), String::new(), false),
        Err(ExitCode::FAILURE)
    );
    assert_eq!(
        run_generate_or_install(true, None, "bash".to_string(), false),
        Err(ExitCode::FAILURE)
    );
    assert_eq!(
        run_generate_or_install(false, Some("1".to_string()), String::new(), false),
        Err(ExitCode::FAILURE)
    );
}

#[test]
fn t1170_run_rejects_parse_errors_and_bad_combos() {
    use std::process::ExitCode;
    assert_eq!(run(&args(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(
        run(&args(&["--generate", "--current", "1"])),
        ExitCode::FAILURE
    );
}

#[test]
fn parse_notices_print_to_stderr() {
    report_parse_notices(&[]);
    report_parse_notices(&["did you mean --generate?".to_string()]);
}

#[test]
fn install_flow_rejects_unsupported_shells_before_prompting() {
    // Shell resolution fails before any prompt or write, so the ambient
    // config reads stay side-effect free under any HOME.
    assert_eq!(
        run_generate_or_install(false, None, "bogus".to_string(), false),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn run_install_with_installs_on_tty_yes() {
    // The injected seam stages the install-success arm: a TTY "y"
    // writes the hook line into a scratch HOME's `.profile`.
    let home = scratch("install-yes");
    let home_text = home.to_string_lossy().to_string();
    let mut input = std::io::Cursor::new("y\n");
    let mut save = |_: crate::prompts::PromptScope, _: &str| -> Result<(), String> { Ok(()) };
    assert!(
        run_install_with(
            "bash",
            None,
            Some(&home_text),
            &mut input,
            true,
            None,
            &mut save
        )
        .is_ok()
    );
    let profile = std::fs::read_to_string(home.join(".profile")).unwrap();
    assert!(profile.contains("arcanist-shell-complete"), "{profile}");
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn run_install_with_reports_install_errors() {
    // The injected seam stages the failure arm: no HOME means no
    // install target.
    let mut input = std::io::Cursor::new("");
    let mut save = |_: crate::prompts::PromptScope, _: &str| -> Result<(), String> { Ok(()) };
    assert_eq!(
        run_install_with("bash", None, None, &mut input, false, None, &mut save),
        Err(std::process::ExitCode::FAILURE)
    );
}

#[test]
fn t1170_run_generate_regenerates_rules() {
    use std::process::ExitCode;
    assert_eq!(run(&args(&["--generate"])), ExitCode::SUCCESS);
    let rules = default_shell_root().join("rules").join("bash-rules.sh");
    assert!(rules.is_file());
    assert_eq!(std::fs::read_to_string(&rules).unwrap(), completion_rules());
    std::fs::remove_file(&rules).ok();
}
