//! `arc-rs amend`: amend the commit message from an explicit message.
//!
//! PHP reference: `src/workflow/ArcanistAmendWorkflow.php`, which amends
//! the working copy while synchronizing the local commit message from
//! Differential. This git port rewrites the HEAD message via
//! `git commit --amend`: the message comes from `--message`, or from
//! Differential (`differential.getcommitmessage`) via `--revision`;
//! `--show` prints the message without amending. Staged changes are
//! refused so content is never amended silently.
//!
//! Not yet ported (see T1128): guessing the revision from the working
//! copy, interactive author/unrelated-revision confirmations, and the
//! `history.immutable` guard.

use std::path::Path;
use std::process::ExitCode;

use crate::conduit::Client;
use crate::{config, git};

pub const NAME: &str = "amend";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} (-m MESSAGE | --revision ID) [--show]\n\
         \n\
         Amend the HEAD commit message. The message comes from -m/--message\n\
         (repeatable, joined with a blank line), or from Differential via\n\
         --revision. Staged changes are refused; unstaged and untracked\n\
         changes are left alone.\n\
         \n\
         Options:\n\
         \x20 -m, --message MSG   new commit message (repeatable)\n\
         \x20 --revision ID       fetch the message from Differential (D123 or 123)\n\
         \x20 --conduit-uri URI   Conduit URI (else ARC_CONDUIT_URI or .arcconfig)\n\
         \x20 --show              print the message without amending\n\
         \x20 -h, --help          show this help\n\
         \n\
         Not yet ported (see T1128): guessing the revision from the working\n\
         copy, interactive confirmations, and the history.immutable guard.\n"
    )
}

/// Parsed `amend` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub messages: Vec<String>,
    pub show: bool,
    pub revision: Option<String>,
    pub conduit_uri: Option<String>,
}

impl Options {
    fn empty() -> Self {
        Self {
            messages: Vec::new(),
            show: false,
            revision: None,
            conduit_uri: None,
        }
    }
}

fn take_value(flag: &str, iter: &mut std::slice::Iter<'_, String>) -> Result<String, String> {
    match iter.next() {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        _ => Err(format!("{flag} requires a non-empty value")),
    }
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::empty();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--show" {
            options.show = true;
        } else if arg == "-m" || arg == "--message" {
            options.messages.push(take_value(arg, &mut iter)?);
        } else if let Some(value) = arg.strip_prefix("--message=") {
            if value.is_empty() {
                return Err("--message requires a non-empty value".to_string());
            }
            options.messages.push(value.to_string());
        } else if arg.len() > 2 && arg.starts_with("-m") && !arg.starts_with("--") {
            options.messages.push(arg[2..].to_string());
        } else if arg == "--revision" {
            options.revision = Some(take_value(arg, &mut iter)?);
        } else if let Some(value) = arg.strip_prefix("--revision=") {
            if value.is_empty() {
                return Err("--revision requires a non-empty value".to_string());
            }
            options.revision = Some(value.to_string());
        } else if arg == "--conduit-uri" {
            options.conduit_uri = Some(take_value(arg, &mut iter)?);
        } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            if value.is_empty() {
                return Err("--conduit-uri requires a non-empty value".to_string());
            }
            options.conduit_uri = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            return Ok(None);
        } else if arg == "--" {
            return Err("unexpected argument '--': amend takes no paths".to_string());
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}': amend takes no paths"));
        }
    }
    Ok(Some(options))
}

/// Accept `123` or `D123` revision symbols, mirroring
/// `ArcanistWorkflow::normalizeRevisionID`.
pub fn parse_revision_id(text: &str) -> Result<u64, String> {
    let digits = text
        .strip_prefix('D')
        .or_else(|| text.strip_prefix('d'))
        .unwrap_or(text);
    if !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(id) = digits.parse::<u64>()
        && id > 0
    {
        return Ok(id);
    }
    Err(format!(
        "invalid revision id '{text}' (expected '123' or 'D123')"
    ))
}

/// Apply arc message conventions: the message must be non-empty and ends
/// with exactly one newline.
pub fn normalize_message(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("commit message is empty".to_string());
    }
    Ok(format!("{trimmed}\n"))
}

/// Fetch the commit message for `revision_id` from Differential.
pub fn fetch_commit_message(client: &Client, revision_id: u64) -> Result<String, String> {
    let params = serde_json::json!({"revision_id": revision_id, "edit": false});
    let result = client
        .call("differential.getcommitmessage", &params)
        .map_err(|e| e.to_string())?;
    result.as_str().map(str::to_string).ok_or_else(|| {
        "conduit differential.getcommitmessage returned a non-string result".to_string()
    })
}

fn resolve_message(options: &Options) -> Result<String, String> {
    if !options.messages.is_empty() && options.revision.is_some() {
        return Err("pass only one of --message and --revision".to_string());
    }
    if !options.messages.is_empty() {
        return normalize_message(&options.messages.join("\n\n"));
    }
    if let Some(revision) = &options.revision {
        let id = parse_revision_id(revision)?;
        let client = config::client_for(options.conduit_uri.as_deref())?;
        return normalize_message(&fetch_commit_message(&client, id)?);
    }
    Err(
        "amend requires --message <text> or --revision <id> (guessing the revision from the working copy is not yet ported; see T1128)"
            .to_string(),
    )
}

/// Refuse when staged changes exist, so `amend` only ever rewrites the
/// message and never folds staged content into HEAD silently.
fn ensure_nothing_staged(dir: &Path) -> Result<(), String> {
    let output = git::output(dir, &["diff", "--cached", "--quiet"])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(
            "you have staged changes; commit or unstage them before amending (refusing to amend content silently)"
                .to_string(),
        )
    }
}

fn write_message_file(tag: &str, message: &str) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "arc-rs-{tag}-{}-{}.msg",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, message)
        .map_err(|e| format!("failed to write commit message file: {e}"))?;
    Ok(path)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    if !git::is_inside_work_tree(dir) {
        return Err("not inside a git repository".to_string());
    }
    let message = resolve_message(&options)?;
    if options.show {
        return Ok(message);
    }
    ensure_nothing_staged(dir)?;
    let file = write_message_file("amend", &message)?;
    let path = file.to_string_lossy().to_string();
    let args: Vec<&str> = vec!["commit", "--amend", "--allow-empty", "-F", path.as_str()];
    let output = git::output(dir, &args);
    std::fs::remove_file(&file).ok();
    let output = output?;
    if !output.status.success() {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = detail.trim();
        if detail.is_empty() {
            return Err("git commit --amend failed".to_string());
        }
        return Err(format!("git commit --amend failed: {detail}"));
    }
    let summary = String::from_utf8_lossy(&output.stdout);
    let summary = summary.trim();
    if summary.is_empty() {
        Ok("amended.\n".to_string())
    } else {
        Ok(format!("{summary}\n"))
    }
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "amend_tests.rs"]
mod amend_tests;
