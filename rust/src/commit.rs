//! `arc-rs commit`: create a git commit from an explicit message.
//!
//! PHP reference: `src/workflow/ArcanistCommitWorkflow.php`, which commits
//! an accepted Differential revision via `svn commit`. This git port
//! creates a local commit: the message comes from `--message`, or from
//! Differential (`differential.getcommitmessage`) via `--revision`;
//! `--show` prints the message without committing. Nothing is pushed.
//!
//! Not yet ported (see T1128): selecting a revision from the working copy
//! without `--revision`, and SVN support.

use std::path::Path;
use std::process::ExitCode;

use crate::conduit::Client;
use crate::{config, git};

pub const NAME: &str = "commit";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} (-m MESSAGE | --revision ID) [--all] [--allow-empty] [--show]\n\
         \n\
         Create a git commit. The message comes from -m/--message (repeatable,\n\
         joined with a blank line), or from Differential via --revision.\n\
         Nothing is pushed.\n\
         \n\
         Options:\n\
         \x20 -m, --message MSG   commit message (repeatable)\n\
         \x20 --revision ID       fetch the message from Differential (D123 or 123)\n\
         \x20 --conduit-uri URI   Conduit URI (else ARC_CONDUIT_URI or .arcconfig)\n\
         \x20 -a, --all           stage tracked modifications before committing\n\
         \x20 --allow-empty       allow a commit with no changes\n\
         \x20 --show              print the message without committing\n\
         \x20 -h, --help          show this help\n\
         \n\
         Not yet ported (see T1128): selecting a revision from the working\n\
         copy without --revision, and SVN support.\n"
    )
}

/// Parsed `commit` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub messages: Vec<String>,
    pub all: bool,
    pub allow_empty: bool,
    pub show: bool,
    pub revision: Option<String>,
    pub conduit_uri: Option<String>,
}

impl Options {
    fn empty() -> Self {
        Self {
            messages: Vec::new(),
            all: false,
            allow_empty: false,
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

/// One argument's parse step: consumed, `--help`, or a fatal error.
enum Step {
    Handled,
    Help,
    Error(String),
}

fn flag_simple(options: &mut Options, arg: &str) -> Option<Step> {
    if arg == "-a" || arg == "--all" {
        options.all = true;
    } else if arg == "--allow-empty" {
        options.allow_empty = true;
    } else if arg == "--show" {
        options.show = true;
    } else {
        return None;
    }
    Some(Step::Handled)
}

fn flag_message(
    options: &mut Options,
    arg: &str,
    iter: &mut std::slice::Iter<'_, String>,
) -> Option<Step> {
    if arg == "-m" || arg == "--message" {
        match take_value(arg, iter) {
            Ok(value) => options.messages.push(value),
            Err(message) => return Some(Step::Error(message)),
        }
        return Some(Step::Handled);
    }
    if let Some(value) = arg.strip_prefix("--message=") {
        if value.is_empty() {
            return Some(Step::Error(
                "--message requires a non-empty value".to_string(),
            ));
        }
        options.messages.push(value.to_string());
        return Some(Step::Handled);
    }
    None
}

fn flag_message_short(options: &mut Options, arg: &str) -> Option<Step> {
    if arg.len() > 2 && arg.starts_with("-m") && !arg.starts_with("--") {
        options.messages.push(arg[2..].to_string());
        Some(Step::Handled)
    } else {
        None
    }
}

fn flag_revision(
    options: &mut Options,
    arg: &str,
    iter: &mut std::slice::Iter<'_, String>,
) -> Option<Step> {
    if arg == "--revision" {
        match take_value(arg, iter) {
            Ok(value) => options.revision = Some(value),
            Err(message) => return Some(Step::Error(message)),
        }
        return Some(Step::Handled);
    }
    if let Some(value) = arg.strip_prefix("--revision=") {
        if value.is_empty() {
            return Some(Step::Error(
                "--revision requires a non-empty value".to_string(),
            ));
        }
        options.revision = Some(value.to_string());
        return Some(Step::Handled);
    }
    None
}

fn flag_conduit(
    options: &mut Options,
    arg: &str,
    iter: &mut std::slice::Iter<'_, String>,
) -> Option<Step> {
    if arg == "--conduit-uri" {
        match take_value(arg, iter) {
            Ok(value) => options.conduit_uri = Some(value),
            Err(message) => return Some(Step::Error(message)),
        }
        return Some(Step::Handled);
    }
    if let Some(value) = arg.strip_prefix("--conduit-uri=") {
        if value.is_empty() {
            return Some(Step::Error(
                "--conduit-uri requires a non-empty value".to_string(),
            ));
        }
        options.conduit_uri = Some(value.to_string());
        return Some(Step::Handled);
    }
    None
}

fn flag_tail(arg: &str) -> Step {
    if arg == "-h" || arg == "--help" {
        Step::Help
    } else if arg == "--" {
        Step::Error(
            "unexpected argument '--': commit takes no paths (stage changes with 'git add' first)"
                .to_string(),
        )
    } else if arg.starts_with('-') {
        Step::Error(format!("unknown flag '{arg}'"))
    } else {
        Step::Error(format!(
            "unexpected argument '{arg}': commit takes no paths (stage changes with 'git add' first)"
        ))
    }
}

fn consume_arg(options: &mut Options, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
    if let Some(step) = flag_simple(options, arg) {
        return step;
    }
    if let Some(step) = flag_message(options, arg, iter) {
        return step;
    }
    if let Some(step) = flag_message_short(options, arg) {
        return step;
    }
    if let Some(step) = flag_revision(options, arg, iter) {
        return step;
    }
    if let Some(step) = flag_conduit(options, arg, iter) {
        return step;
    }
    flag_tail(arg)
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::empty();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        match consume_arg(&mut options, arg, &mut iter) {
            Step::Handled => {}
            Step::Help => return Ok(None),
            Step::Error(message) => return Err(message),
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
    Err("commit requires a message: pass --message <text> or --revision <id>".to_string())
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
    let file = write_message_file("commit", &message)?;
    let path = file.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["commit", "-F", path.as_str()];
    if options.all {
        args.push("--all");
    }
    if options.allow_empty {
        args.push("--allow-empty");
    }
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
            return Err("git commit failed".to_string());
        }
        return Err(format!("git commit failed: {detail}"));
    }
    let summary = String::from_utf8_lossy(&output.stdout);
    let summary = summary.trim();
    if summary.is_empty() {
        Ok("committed.\n".to_string())
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
#[path = "commit_tests.rs"]
mod commit_tests;
