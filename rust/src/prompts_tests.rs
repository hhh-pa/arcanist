use super::*;
use std::io::BufReader;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-prompts-{name}-{}-{}",
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
fn core66_prompt_maps_match_ceb() {
    assert_eq!(prompt_map("amend").len(), 3);
    assert_eq!(prompt_map("land").len(), 11);
    assert_eq!(prompt_map("liberate").len(), 1);
    assert_eq!(prompt_map("shell-complete").len(), 1);
    assert!(prompt_map("diff").is_empty());
    assert!(prompt_map("look").is_empty());
    assert_eq!(prompt_map("land")[0].0, "arc.land.large-working-set");
    assert!(prompt_map("land")[0].1.contains("more than 50 commit(s)"));
}

#[test]
fn core66_saved_responses_reject_malformed_config() {
    let value = serde_json::json!([{"prompt": "k", "response": "y"}]);
    assert_eq!(
        read_saved_responses(Some(&value)).unwrap(),
        vec![("k".to_string(), "y".to_string())]
    );
    assert!(read_saved_responses(None).unwrap().is_empty());
    let bad = serde_json::json!([{"prompt": "k"}]);
    assert!(read_saved_responses(Some(&bad)).is_err());
    let bad = serde_json::json!({"prompts": 1});
    assert!(read_saved_responses(Some(&bad)).is_err());
}

#[test]
fn core66_report_renders_blocks_and_saved_responses() {
    let text = render_report("diff", &[], false);
    assert!(text.contains("This workflow does not have any prompts."));
    let saved = vec![("arc.land.confirm".to_string(), "y".to_string())];
    let text = render_report("land", &saved, false);
    assert!(text.contains("PROMPT  arc.land.confirm"));
    assert!(text.contains("You have saved the response \"y\" to this prompt."));
    // Sorted by key.
    let confirm = text.find("arc.land.confirm").unwrap();
    let create = text.find("arc.land.create").unwrap();
    assert!(confirm < create);
}

#[test]
fn core66_unknown_workflow_lists_toolset() {
    let message = unknown_workflow_error("bogus");
    assert!(message.starts_with("Workflow \"bogus\" is unknown. Supported workflows are: "));
    assert!(message.contains("shell-complete"));
}

#[test]
fn core52_prompt_accepts_declines_and_explains() {
    let mut save = |_: PromptScope, _: &str| Ok(());
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    assert!(execute_prompt_saving(&mut input, "k", "d", "Q?", true, None, &mut save).is_ok());
    let mut input = BufReader::new(std::io::Cursor::new(b"n\n"));
    assert_eq!(
        execute_prompt_saving(&mut input, "k", "d", "Q?", true, None, &mut save).unwrap_err(),
        crate::prompt::ABORT
    );
    // Empty input defaults to `n`.
    let mut input = BufReader::new(std::io::Cursor::new(b"\n"));
    assert!(execute_prompt_saving(&mut input, "k", "d", "Q?", true, None, &mut save).is_err());
    // `?` prints help, then the loop continues.
    let mut input = BufReader::new(std::io::Cursor::new(b"?\ny\n"));
    assert!(execute_prompt_saving(&mut input, "k", "d", "Q?", true, None, &mut save).is_ok());
}

#[test]
fn core52_prompt_captures_save_suffixes() {
    let mut saved: Vec<(PromptScope, String)> = Vec::new();
    let mut input = BufReader::new(std::io::Cursor::new(b"y*\n"));
    execute_prompt_saving(
        &mut input,
        "k",
        "d",
        "Q?",
        true,
        None,
        &mut |scope, response| {
            saved.push((scope, response.to_string()));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(saved, vec![(PromptScope::User, "y".to_string())]);
    let mut input = BufReader::new(std::io::Cursor::new(b"n!\n"));
    let error = execute_prompt_saving(
        &mut input,
        "k",
        "d",
        "Q?",
        true,
        None,
        &mut |scope, response| {
            saved.push((scope, response.to_string()));
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error, crate::prompt::ABORT);
    assert_eq!(
        saved.last(),
        Some(&(PromptScope::WorkingCopy, "n".to_string()))
    );
}

#[test]
fn core52_prompt_requires_tty_and_uses_saved_responses() {
    let mut save = |_: PromptScope, _: &str| Ok(());
    let mut input = BufReader::new(std::io::Cursor::new(b"y\n"));
    assert_eq!(
        execute_prompt_saving(&mut input, "k", "d", "Q?", false, None, &mut save).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    let mut input = BufReader::new(std::io::Cursor::new(b""));
    assert!(execute_prompt_saving(&mut input, "k", "d", "Q?", true, Some("y"), &mut save).is_ok());
    let mut input = BufReader::new(std::io::Cursor::new(b""));
    assert!(
        execute_prompt_saving(&mut input, "k", "d", "", true, None, &mut save)
            .unwrap_err()
            .contains("has no query text")
    );
}

#[test]
fn core52_prompt_responses_round_trip_through_files() {
    let dir = scratch("roundtrip");
    let user = dir.join("arcrc");
    let local = dir.join("local");
    save_prompt_response(&user, "k", "y").unwrap();
    save_prompt_response(&user, "k", "n").unwrap();
    save_prompt_response(&local, "j", "y").unwrap();
    let saved = load_saved_responses(Some(&user), Some(&local));
    assert_eq!(
        saved,
        vec![
            ("k".to_string(), "n".to_string()),
            ("j".to_string(), "y".to_string())
        ]
    );
    let _ = args(&[]);
}

#[test]
fn core28_prompts_read_top_level_through_toolset_transform() {
    // `prompts` reads `getConfig('prompts')` through the toolset user
    // transform: top-level responses survive next to a `config` section
    // (classic section-only would hide them).
    let dir = scratch("core28");
    let arcrc = dir.join(".arcrc");
    std::fs::write(
        &arcrc,
        r#"{
            "config": {"editor": "nano"},
            "prompts": [{"prompt": "arc.amend.author", "response": "saved!"}]
        }"#,
    )
    .unwrap();
    let text = execute_in(&dir, Some(&arcrc), &args(&["amend"])).unwrap();
    assert!(
        text.contains("You have saved the response \"saved!\" to this prompt."),
        "{text}"
    );
}
#[test]
fn parse_args_returns_positionals() {
    assert_eq!(
        parse_args(&args(&["amend"]), false).unwrap(),
        args(&["amend"])
    );
    assert!(parse_args(&args(&[]), false).unwrap().is_empty());
}

#[test]
fn execute_rejects_empty_and_unknown_workflows() {
    let dir = scratch("validate");
    let error = execute_in(&dir, None, &[]).unwrap_err();
    assert!(
        error.message.contains("Provide a workflow"),
        "{}",
        error.message
    );
    let error = execute_in(&dir, None, &args(&["bogus"])).unwrap_err();
    assert!(error.message.contains("unknown"), "{}", error.message);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn execute_reports_promptless_workflows_without_config() {
    // `look` is a toolset workflow with no prompts: no config reads.
    let dir = scratch("noprompts");
    let text = execute_in(&dir, None, &args(&["look"])).unwrap();
    assert!(text.contains("does not have any prompts"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn execute_reads_no_saved_responses_without_a_user_file() {
    let dir = scratch("nouser");
    let text = execute_in(&dir, None, &args(&["amend"])).unwrap();
    assert!(text.contains("arc.amend.author"), "{text}");
    assert!(!text.contains("You have saved the response"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn working_copy_config_files_resolve_per_vcs() {
    for (marker, joined) in [(".git", ".git"), (".hg", ".hg"), (".svn", ".svn")] {
        let dir = scratch(&format!("vcs-{marker}"));
        let nested = dir.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(dir.join(marker)).unwrap();
        let expected = dir.join(joined).join("arc").join("config");
        assert_eq!(working_copy_config_file(&nested), Some(expected));
        let _ = std::fs::remove_dir_all(&dir);
    }
    let bare = scratch("novcs");
    assert_eq!(working_copy_config_file(&bare), None);
    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn saved_response_loading_skips_unreadable_files() {
    let dir = scratch("loadskip");
    let bad = dir.join("bad.json");
    std::fs::write(&bad, "not json").unwrap();
    assert!(load_saved_responses(Some(&bad), None).is_empty());
    assert!(load_saved_responses(Some(&dir.join("missing")), None).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saved_response_lookup_prefers_the_last_match() {
    let dir = scratch("lookup");
    let user = dir.join("arcrc");
    std::fs::write(
        &user,
        r#"{"prompts": [{"prompt": "k", "response": "n"}, {"prompt": "k", "response": "y"}]}"#,
    )
    .unwrap();
    assert_eq!(
        load_saved_response(&dir, Some(&user), "k"),
        Some("y".to_string())
    );
    assert_eq!(load_saved_response(&dir, Some(&user), "other"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_fails_when_the_parent_is_not_a_directory() {
    let dir = scratch("savefail");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let error = save_prompt_response(&blocker.join("child"), "k", "y").unwrap_err();
    assert!(error.contains("cannot write"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_without_a_parent_dir_skips_mkdir() {
    // A parentless path (`/` has no parent) skips directory creation;
    // the write itself still fails, without touching the filesystem.
    let error = save_prompt_response(std::path::Path::new("/"), "k", "y").unwrap_err();
    assert!(error.contains("cannot write"), "{error}");
}

#[test]
fn response_saver_routes_scopes_to_files() {
    let dir = scratch("saver");
    let user = dir.join("user.json");
    let local = dir.join("local.json");
    let mut saver = response_saver("k", Some(user.clone()), Some(local.clone()));
    saver(PromptScope::User, "y").unwrap();
    saver(PromptScope::WorkingCopy, "n").unwrap();
    assert_eq!(
        load_saved_responses(Some(&user), None),
        vec![("k".to_string(), "y".to_string())]
    );
    assert_eq!(
        load_saved_responses(Some(&local), None),
        vec![("k".to_string(), "n".to_string())]
    );
    let mut homeless = response_saver("k", None, None);
    assert!(homeless(PromptScope::User, "y").is_err());
    assert!(homeless(PromptScope::WorkingCopy, "n").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prompt_retries_unknown_answers_silently() {
    let mut save = |_: PromptScope, _: &str| Ok(());
    let mut input = BufReader::new(std::io::Cursor::new(b"maybe\ny\n"));
    assert!(execute_prompt_saving(&mut input, "k", "d", "Q?", true, None, &mut save).is_ok());
}

#[test]
fn run_reports_unknown_workflows() {
    // Unknown workflows fail before any config is read.
    assert_eq!(
        run(&args(&["bogus-workflow"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn r10_prompts_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "PROMPTS WORKFLOW\n\
         \n\
         prompts workflow\n\
         \n\
         \n\
         Show information about prompts a workflow may execute, and review saved\n\
         responses.\n\
         \n\
         Show Prompts\n\
         \n\
         To show possible prompts a workflow may execute, run:\n\
         \n\
         \x20 $ arc prompts workflow\n\
         \n\
         Saving Responses\n\
         \n\
         If you always want to answer a particular prompt in a certain way, you can\n\
         save your response to the prompt. When you encounter the prompt again, your\n\
         saved response will be used automatically.\n\
         \n\
         To save a response, add \"*\" or \"!\" to the end of the response you want to\n\
         save when you answer the prompt:\n\
         \n\
         \x20 - Using \"*\" will save the response in user configuration. In the future,\n\
         \x20   the saved answer will be used any time you encounter the prompt (in any\n\
         \x20   project).\n\
         \x20 - Using \"!\" will save the response in working copy configuration. In the\n\
         \x20   future, the saved answer will be used when you encounter the prompt in\n\
         \x20   the current working copy.\n\
         \n\
         For example, if you would like to always answer \"y\" to a particular prompt,\n\
         respond with \"y*\" or \"y!\" to save your response.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    assert!(wants_help(&["--help".to_string()]));
    assert!(!wants_help(&["--".to_string(), "--help".to_string()]));
    // `execute_in` reroutes `--help` to the usage text.
    let out = execute_in(std::path::Path::new("."), None, &["--help".to_string()]).unwrap();
    assert!(out.contains("PROMPTS WORKFLOW"), "{out}");
}
