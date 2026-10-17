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
