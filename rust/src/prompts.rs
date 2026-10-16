//! `prompts` workflow: list a workflow's prompts and saved responses.
//!
//! PHP reference: `src/toolset/workflow/ArcanistPromptsWorkflow.php` at
//! ceb. Prompt maps come from each workflow's `newPrompts()`; saved
//! responses come from the merged `prompts` config (a list of
//! `{prompt, response}` dicts, `ArcanistPromptResponse`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::argparse;
use crate::conduit;
use crate::terminal;

pub const NAME: &str = "prompts";

/// `(key, description)` prompt maps by workflow name, from each
/// `newPrompts()` at ceb. Workflows absent here have no prompts.
pub fn prompt_map(workflow: &str) -> &'static [(&'static str, &'static str)] {
    match workflow {
        "amend" => &[
            (
                "arc.amend.unrelated",
                "Confirms use of a revision that does not appear to be present in the working copy.",
            ),
            (
                "arc.amend.author",
                "Confirms use of a revision that you are not the author of.",
            ),
            (
                "arc.amend.immutable",
                "Confirms history mutation in a working copy marked as immutable.",
            ),
        ],
        "land" => &[
            (
                "arc.land.large-working-set",
                "Confirms landing more than 50 commit(s) in a single operation.",
            ),
            (
                "arc.land.confirm",
                "Confirms that the correct changes have been selected to land.",
            ),
            (
                "arc.land.implicit",
                "Confirms that local commits which are not associated with a revision have been associated correctly and should land.",
            ),
            (
                "arc.land.unauthored",
                "Confirms that revisions you did not author should land.",
            ),
            (
                "arc.land.changes-planned",
                "Confirms that revisions with changes planned should land.",
            ),
            (
                "arc.land.published",
                "Confirms that revisions that are already published should land.",
            ),
            (
                "arc.land.not-accepted",
                "Confirms that revisions that are not accepted should land.",
            ),
            (
                "arc.land.open-parents",
                "Confirms that revisions with open parent revisions should land.",
            ),
            (
                "arc.land.failed-builds",
                "Confirms that revisions with failed builds should land.",
            ),
            (
                "arc.land.ongoing-builds",
                "Confirms that revisions with ongoing builds should land.",
            ),
            (
                "arc.land.create",
                "Confirms that new branches or bookmarks should be created in the remote.",
            ),
        ],
        "liberate" => &[("arc.liberate.create", "Confirms creation of a new library.")],
        "shell-complete" => &[(
            "arc.shell-complete.install",
            "Confirms writing to to \"~/.profile\" (or another similar file) to install shell completion.",
        )],
        _ => &[],
    }
}

/// `(prompt, response)` pairs from a merged `prompts` config value.
/// Malformed entries are a fatal config error, like PHP's
/// `PhutilTypeSpec::checkMap` throw.
pub fn read_saved_responses(
    value: Option<&serde_json::Value>,
) -> Result<Vec<(String, String)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(list) = value.as_array() else {
        return Err("Expected a list of prompt responses!".to_string());
    };
    let mut out = Vec::new();
    for entry in list {
        let prompt = entry.get("prompt").and_then(serde_json::Value::as_str);
        let response = entry.get("response").and_then(serde_json::Value::as_str);
        match (prompt, response) {
            (Some(prompt), Some(response)) => {
                out.push((prompt.to_string(), response.to_string()));
            }
            _ => return Err("Expected a list of prompt responses!".to_string()),
        }
    }
    Ok(out)
}

/// Render one prompt block: the green `PROMPT` header, the wrapped
/// description, and one line per saved response.
fn render_block(key: &str, description: &str, responses: &[String], ansi: bool) -> String {
    let mut block = terminal::tsprintf_ansi(
        "<bg:green>** %s **</bg> **%s**\n\n",
        &[terminal::TVal::Text("PROMPT"), terminal::TVal::Text(key)],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    block.push_str(
        &terminal::tsprintf_ansi("%W\n", &[terminal::TVal::Text(description)], ansi)
            .map(|t| t.to_string())
            .unwrap_or_default(),
    );
    if !responses.is_empty() {
        block.push('\n');
        for response in responses {
            let line = terminal::pht(
                "You have saved the response \"%s\" to this prompt.",
                &[terminal::PhtArg::Text(response)],
            );
            block.push_str(
                &terminal::tsprintf_ansi(
                    "    <bg:cyan>** > **</bg> %s\n",
                    &[terminal::TVal::Text(&line)],
                    ansi,
                )
                .map(|t| t.to_string())
                .unwrap_or_default(),
            );
        }
    }
    block
}

/// Render the full prompts report for `workflow`.
#[must_use]
pub fn render_report(workflow: &str, saved: &[(String, String)], ansi: bool) -> String {
    let mut prompts = prompt_map(workflow).to_vec();
    if prompts.is_empty() {
        return terminal::tsprintf_ansi(
            "%s\n",
            &[terminal::TVal::Text(
                "This workflow does not have any prompts.",
            )],
            ansi,
        )
        .map(|t| t.to_string())
        .unwrap_or_default();
    }
    prompts.sort_by(|a, b| a.0.cmp(b.0));
    let blocks: Vec<String> = prompts
        .iter()
        .map(|(key, description)| {
            let responses: Vec<String> = saved
                .iter()
                .filter(|(prompt, _)| prompt == key)
                .map(|(_, response)| response.clone())
                .collect();
            render_block(key, description, &responses, ansi)
        })
        .collect();
    blocks.join("\n")
}

/// Parse `prompts` arguments (wildcard only).
pub fn parse_args(argv: &[String], autocorrect: bool) -> Result<Vec<String>, String> {
    Ok(argparse::parse_toolset(argv, &[], true, autocorrect)?.positionals)
}

/// Unknown-workflow error listing the supported toolset workflows.
#[must_use]
pub fn unknown_workflow_error(workflow: &str) -> String {
    terminal::pht(
        "Workflow \"%s\" is unknown. Supported workflows are: %s.",
        &[
            terminal::PhtArg::Text(workflow),
            terminal::PhtArg::Text(&crate::alias::toolset_names().join(", ")),
        ],
    )
}

fn execute_in(
    _dir: &Path,
    user_file: Option<&Path>,
    argv: &[String],
) -> Result<String, conduit::ClassifiedError> {
    use std::io::IsTerminal;
    let parsed = argparse::parse_toolset(argv, &[], true, std::io::stdin().is_terminal())
        .map_err(conduit::ClassifiedError::toolset_parse_error)?;
    if parsed.positionals.is_empty() {
        return Err(conduit::ClassifiedError::toolset_usage(
            "Provide a workflow to list prompts for.".to_string(),
        ));
    }
    let workflow = &parsed.positionals[0];
    if !crate::alias::toolset_names().contains(&workflow.as_str()) {
        return Err(conduit::ClassifiedError::toolset_usage(
            unknown_workflow_error(workflow),
        ));
    }
    if prompt_map(workflow).is_empty() {
        return Ok(render_report(workflow, &[], terminal::ansi_enabled()));
    }
    // T1153 CORE-28: `getConfig('prompts')` runs through the toolset
    // `ArcanistUserConfigurationSource` transform (section wins), not the
    // classic section-only user map.
    let user = match user_file {
        Some(path) => crate::get_config::read_user_file_toolset(path)
            .map_err(conduit::ClassifiedError::fatal)?,
        None => serde_json::Map::new(),
    };
    let saved =
        read_saved_responses(user.get("prompts")).map_err(conduit::ClassifiedError::fatal)?;
    Ok(render_report(workflow, &saved, terminal::ansi_enabled()))
}

/// Where a `*`/`!`-suffixed prompt response is saved
/// (`ArcanistConfigurationSource::SCOPE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptScope {
    /// `*`: user configuration (`~/.arcrc`).
    User,
    /// `!`: working-copy configuration (repo-local config file).
    WorkingCopy,
}

/// The shell-complete install prompt identity.
pub const INSTALL_PROMPT_KEY: &str = "arc.shell-complete.install";
/// The shell-complete install prompt description (with PHP's `to to`).
pub const INSTALL_PROMPT_DESCRIPTION: &str =
    "Confirms writing to to \"~/.profile\" (or another similar file) to install shell completion.";
/// The liberate create prompt identity.
pub const LIBERATE_PROMPT_KEY: &str = "arc.liberate.create";
/// The liberate create prompt description.
pub const LIBERATE_PROMPT_DESCRIPTION: &str = "Confirms creation of a new library.";

/// Repo-local config file under `dir` (`.git/arc/config`,
/// `.hg/arc/config`, `.svn/arc/config`), if `dir` is in a working copy.
#[must_use]
pub fn working_copy_config_file(dir: &Path) -> Option<PathBuf> {
    let (kind, root) = crate::get_config::find_vcs_root(dir)?;
    let name = match kind {
        crate::get_config::VcsKind::Git => ".git",
        crate::get_config::VcsKind::Hg => ".hg",
        crate::get_config::VcsKind::Svn => ".svn",
    };
    Some(root.join(name).join("arc").join("config"))
}

/// Saved responses for every scope merged (user file first, then the
/// working-copy file), like `getConfig()`.
#[must_use]
pub fn load_saved_responses(
    user_file: Option<&Path>,
    local_file: Option<&Path>,
) -> Vec<(String, String)> {
    let mut saved = Vec::new();
    for path in [user_file, local_file].into_iter().flatten() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Ok(mut entries) = read_saved_responses(doc.get("prompts")) {
            saved.append(&mut entries);
        }
    }
    saved
}

/// The last saved response for `key`, if any (later scopes win, like
/// PHP's overwrite loop).
#[must_use]
pub fn load_saved_response(dir: &Path, user_file: Option<&Path>, key: &str) -> Option<String> {
    let local_file = working_copy_config_file(dir);
    load_saved_responses(user_file, local_file.as_deref())
        .into_iter()
        .filter(|(prompt, _)| prompt == key)
        .map(|(_, response)| response)
        .next_back()
}

/// Persist a prompt response: drop same-key entries, append the new
/// one (`saveResponse()`).
pub fn save_prompt_response(file: &Path, key: &str, response: &str) -> Result<(), String> {
    let mut doc = match std::fs::read_to_string(file) {
        Ok(text) => {
            serde_json::from_str::<serde_json::Value>(&text).unwrap_or(serde_json::Value::Null)
        }
        Err(_) => serde_json::Value::Null,
    };
    if !doc.is_object() {
        doc = serde_json::json!({});
    }
    let mut entries = read_saved_responses(doc.get("prompts"))?;
    entries.retain(|(prompt, _)| prompt != key);
    entries.push((key.to_string(), response.to_string()));
    let list: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|(prompt, response)| serde_json::json!({"prompt": prompt, "response": response}))
        .collect();
    doc.as_object_mut()
        .expect("prompt save object")
        .insert("prompts".to_string(), serde_json::Value::Array(list));
    if let Some(parent) = file.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot write '{}': {e}", file.display()))?;
    }
    std::fs::write(file, serde_json::to_string_pretty(&doc).unwrap_or_default())
        .map_err(|e| format!("cannot write '{}': {e}", file.display()))
}

/// A `save` callback for [`execute_prompt_saving`]: writes `*`
/// responses for `key` to `user_file` and `!` responses to
/// `local_file` (missing files are written).
pub fn response_saver(
    key: &str,
    user_file: Option<PathBuf>,
    local_file: Option<PathBuf>,
) -> impl FnMut(PromptScope, &str) -> Result<(), String> {
    let key = key.to_string();
    move |scope: PromptScope, response: &str| {
        let file = match scope {
            PromptScope::User => user_file.clone(),
            PromptScope::WorkingCopy => local_file.clone(),
        };
        match file {
            Some(file) => save_prompt_response(&file, &key, response),
            None => Err(format!(
                "cannot save prompt response for \"{key}\": no config file"
            )),
        }
    }
}

/// Echo the `SAVE PROMPT` line for a suffixed answer.
fn save_echo(key: &str, response: &str, ansi: bool) -> String {
    terminal::tsprintf_ansi(
        "<bg:green>** %s **</bg> %s\n",
        &[
            terminal::TVal::Text("SAVE PROMPT"),
            terminal::TVal::Text(&terminal::pht(
                "Saving response \"%s\" to prompt \"%s\".",
                &[
                    terminal::PhtArg::Text(response),
                    terminal::PhtArg::Text(key),
                ],
            )),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default()
}

/// Read one interactive prompt answer: echo the query line, trim
/// PHP's whitespace set, and default an empty answer to `n`.
fn read_prompt_response(input: &mut dyn std::io::BufRead, query: &str) -> String {
    use std::io::Write as _;
    print!("{}", crate::prompt::prompt_line(query));
    let _ = std::io::stdout().flush();
    let raw = crate::prompt::read_answer(input).unwrap_or_default();
    let trimmed = raw
        .trim_matches([' ', '\t', '\n', '\r', '\0', '\x0B'])
        .to_string();
    if trimmed.is_empty() {
        "n".to_string()
    } else {
        trimmed
    }
}

/// The next prompt response: the saved response once (flagged, so no
/// save suffix applies), else an interactive answer.
fn next_prompt_response(
    input: &mut dyn std::io::BufRead,
    query: &str,
    saved: &mut Option<&str>,
) -> (String, bool) {
    match saved.take() {
        Some(response) => (response.to_string(), true),
        None => (read_prompt_response(input, query), false),
    }
}

/// Split a `*`/`!` save suffix off an interactive response (saved
/// responses never carry one).
fn split_save_suffix(response: &str, is_saved: bool) -> (String, Option<PromptScope>) {
    if is_saved {
        (response.to_string(), None)
    } else if let Some(stripped) = response.strip_suffix('*') {
        (stripped.to_string(), Some(PromptScope::User))
    } else if let Some(stripped) = response.strip_suffix('!') {
        (stripped.to_string(), Some(PromptScope::WorkingCopy))
    } else {
        (response.to_string(), None)
    }
}

/// One prompt answer's meaning.
enum PromptAnswer {
    /// `y`: persist any save suffix, then accept.
    Accept,
    /// `n`: persist any save suffix, then abort.
    Decline,
    /// `?`: print the prompt help and ask again.
    Help,
    /// Anything else: ask again silently.
    Retry,
}

/// Classify one prompt answer (case-insensitive).
fn classify_prompt_answer(response: &str) -> PromptAnswer {
    match response.to_lowercase().as_str() {
        "y" => PromptAnswer::Accept,
        "n" => PromptAnswer::Decline,
        "?" => PromptAnswer::Help,
        _ => PromptAnswer::Retry,
    }
}

/// Run a `y`/`n` prompt like `ArcanistPrompt::execute()`, capturing
/// `*`/`!` save suffixes: `save` persists the stripped response under
/// its scope (after echoing the `SAVE PROMPT` line), on accept and on
/// decline alike. Returns `Ok` on `y`, `Err(ABORT)` on `n`, and
/// `Err(NOT_INTERACTIVE)` off-TTY (after echoing the query).
pub fn execute_prompt_saving(
    input: &mut dyn std::io::BufRead,
    key: &str,
    description: &str,
    query: &str,
    stdin_tty: bool,
    saved: Option<&str>,
    save: &mut dyn FnMut(PromptScope, &str) -> Result<(), String>,
) -> Result<(), String> {
    if query.is_empty() {
        return Err(format!("Prompt (\"{key}\") has no query text!"));
    }
    let mut saved_response = saved;
    if !stdin_tty {
        println!("{query}");
        return Err(crate::prompt::NOT_INTERACTIVE.to_string());
    }
    println!();
    let ansi = terminal::ansi_enabled();
    loop {
        let (response, is_saved) = next_prompt_response(input, query, &mut saved_response);
        let (response, scope) = split_save_suffix(&response, is_saved);
        let accepted = match classify_prompt_answer(&response) {
            PromptAnswer::Accept => true,
            PromptAnswer::Decline => false,
            PromptAnswer::Help => {
                print!("{}", crate::prompt::prompt_help(key, description));
                continue;
            }
            PromptAnswer::Retry => continue,
        };
        if is_saved {
            print!(
                "{}",
                crate::prompt::saved_response_echo(query, &response, key)
            );
        }
        if let Some(scope) = scope {
            print!("{}", save_echo(key, &response, ansi));
            save(scope, &response)?;
        }
        if accepted {
            return Ok(());
        }
        return Err(crate::prompt::ABORT.to_string());
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let user_file = crate::config::arcrc_path();
    match execute_in(&cwd, user_file.as_deref(), argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => error.report(),
    }
}

#[cfg(test)]
#[path = "prompts_tests.rs"]
mod prompts_tests;
