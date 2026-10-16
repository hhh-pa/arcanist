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
