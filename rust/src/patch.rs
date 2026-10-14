//! `patch` workflow: apply a Differential change or patch file.
//!
//! PHP reference: `src/workflow/ArcanistPatchWorkflow.php`.
//!
//! Sources: a revision (`D12345`, `--revision`), one diff (`--diff`), or a
//! patch file (`--patch`, `-` for stdin). Conduit sources are downloaded as
//! a raw diff (`differential.querydiffs` to resolve the diff,
//! `differential.getrawdiff` for the text) and applied with `git apply`.
//! `--preview` prints the patch instead of applying it.
//!
//! Out of scope (rejected with a not-yet-ported message): `--arcbundle`,
//! `--encoding`, and `--update`. Accepted as no-ops: `--nocommit`,
//! `--nobranch`, and `--skip-dependencies` (arc-rs never commits, branches,
//! or follows dependencies).

use std::path::Path;
use std::process::ExitCode;

pub const NAME: &str = "patch";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} D12345 [--preview]\n\
         \x20      arc-rs {NAME} --revision ID [--preview]\n\
         \x20      arc-rs {NAME} --diff ID [--preview]\n\
         \x20      arc-rs {NAME} --patch FILE [--preview]\n\
         \n\
         Apply a Differential revision/diff or a patch file to the working\n\
         copy with `git apply`. The tree must be clean unless --force is\n\
         given.\n\
         \n\
         Options:\n\
         \x20 --revision ID  apply the most recent diff of revision D123 (or 123)\n\
         \x20 --diff ID      apply one Differential diff by numeric ID\n\
         \x20 --patch FILE   apply a patch file ('-' reads stdin)\n\
         \x20 --preview      print the patch without applying it\n\
         \x20 --force        apply even if the working copy is not clean\n\
         \x20 --nocommit, --nobranch, --skip-dependencies\n\
         \x20                accepted for compatibility; arc-rs never commits,\n\
         \x20                branches, or follows dependencies\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI / .arcconfig)\n\
         \x20 -h, --help     show this help\n\
         \n\
         Not yet ported (see T1128): --arcbundle, --encoding, --update.\n"
    )
}

/// Where the patch comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Revision(u64),
    Diff(u64),
    PatchFile(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    source: Source,
    preview: bool,
    force: bool,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    NotYetPorted(String),
    Error(String),
}

fn not_yet_ported(flag: &str) -> ParseOutcome {
    ParseOutcome::NotYetPorted(format!(
        "arc-rs {NAME}: {flag} is not yet ported (see T1128)"
    ))
}

/// `D12345` or `12345` to a numeric revision ID.
pub fn normalize_revision_id(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    let digits = trimmed
        .strip_prefix('D')
        .or_else(|| trimmed.strip_prefix('d'))
        .unwrap_or(trimmed);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits
            .parse::<u64>()
            .map_err(|_| format!("invalid revision '{text}': expected D12345 or a revision ID"))
    } else {
        Err(format!(
            "invalid revision '{text}': expected D12345 or a revision ID"
        ))
    }
}

fn parse_diff_id(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        trimmed
            .parse::<u64>()
            .map_err(|_| format!("invalid diff ID '{text}': expected a numeric diff ID"))
    } else {
        Err(format!(
            "invalid diff ID '{text}': expected a numeric diff ID"
        ))
    }
}

fn take_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, ParseOutcome> {
    match iter.next() {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        _ => Err(ParseOutcome::Error(format!("{flag} requires a value"))),
    }
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut revision: Option<String> = None;
    let mut diff: Option<String> = None;
    let mut patch: Option<String> = None;
    let mut name: Option<String> = None;
    let mut preview = false;
    let mut force = false;
    let mut conduit_uri: Option<String> = None;
    let mut end_of_flags = false;

    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            if name.is_some() {
                return ParseOutcome::Error("Specify at most one revision name.".to_string());
            }
            name = Some(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--revision" {
            match take_value(&mut iter, "--revision") {
                Ok(value) => revision = Some(value),
                Err(outcome) => return outcome,
            }
        } else if let Some(value) = arg.strip_prefix("--revision=") {
            revision = Some(value.to_string());
        } else if arg == "--diff" {
            match take_value(&mut iter, "--diff") {
                Ok(value) => diff = Some(value),
                Err(outcome) => return outcome,
            }
        } else if let Some(value) = arg.strip_prefix("--diff=") {
            diff = Some(value.to_string());
        } else if arg == "--patch" {
            match take_value(&mut iter, "--patch") {
                Ok(value) => patch = Some(value),
                Err(outcome) => return outcome,
            }
        } else if let Some(value) = arg.strip_prefix("--patch=") {
            patch = Some(value.to_string());
        } else if arg == "--conduit-uri" {
            match take_value(&mut iter, "--conduit-uri") {
                Ok(value) => conduit_uri = Some(value),
                Err(outcome) => return outcome,
            }
        } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            conduit_uri = Some(value.to_string());
        } else if arg == "--preview" {
            preview = true;
        } else if arg == "--force" {
            force = true;
        } else if arg == "--nocommit" || arg == "--nobranch" || arg == "--skip-dependencies" {
            // No-op: this port never commits, branches, or follows dependencies.
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg == "--arcbundle" || arg.starts_with("--arcbundle=") {
            return not_yet_ported("--arcbundle");
        } else if arg == "--encoding" || arg.starts_with("--encoding=") {
            return not_yet_ported("--encoding");
        } else if arg == "--update" {
            return not_yet_ported("--update");
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            // A positional revision name, like `D12345`.
            if name.is_some() {
                return ParseOutcome::Error("Specify at most one revision name.".to_string());
            }
            name = Some(arg.clone());
        }
    }

    let mut sources = 0;
    if revision.is_some() {
        sources += 1;
    }
    if diff.is_some() {
        sources += 1;
    }
    if patch.is_some() {
        sources += 1;
    }
    if name.is_some() {
        sources += 1;
    }
    if sources == 0 {
        return ParseOutcome::Error(
            "You must specify changes to apply to the working copy with \
             \"D12345\", \"--revision\", \"--diff\", or \"--patch\"."
                .to_string(),
        );
    }
    if sources > 1 {
        return ParseOutcome::Error(
            "Options \"D12345\", \"--revision\", \"--diff\" and \"--patch\" are \
             mutually exclusive. Choose exactly one patch source."
                .to_string(),
        );
    }

    let source = if let Some(value) = revision {
        match normalize_revision_id(&value) {
            Ok(id) => Source::Revision(id),
            Err(message) => return ParseOutcome::Error(message),
        }
    } else if let Some(value) = diff {
        match parse_diff_id(&value) {
            Ok(id) => Source::Diff(id),
            Err(message) => return ParseOutcome::Error(message),
        }
    } else if let Some(path) = patch {
        Source::PatchFile(path)
    } else if let Some(value) = name {
        match normalize_revision_id(&value) {
            Ok(id) => Source::Revision(id),
            Err(message) => return ParseOutcome::Error(message),
        }
    } else {
        unreachable!("source count checked above");
    };

    ParseOutcome::Args(Options {
        source,
        preview,
        force,
        conduit_uri,
    })
}

/// First diff ID in a `differential.querydiffs` result (a dict keyed by diff
/// ID, or a list).
fn first_diff_id(result: &serde_json::Value) -> Result<String, String> {
    let missing = || {
        "The diff or revision you specified is either invalid or you don't \
         have permission to view it."
            .to_string()
    };
    let diff = match result {
        serde_json::Value::Object(map) => map.values().next().ok_or_else(missing)?,
        serde_json::Value::Array(items) => items.first().ok_or_else(missing)?,
        _ => return Err(missing()),
    };
    match diff.get("id") {
        Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        _ => Err(missing()),
    }
}

fn raw_diff_text(result: &serde_json::Value) -> Option<String> {
    if let Some(text) = result.as_str() {
        return Some(text.to_string());
    }
    result
        .get("rawDiff")
        .or_else(|| result.get("diff"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Download one raw diff by numeric diff ID.
pub fn fetch_raw_diff_for_diff(
    client: &crate::conduit::Client,
    diff_id: &str,
) -> Result<String, String> {
    let result = client
        .call(
            "differential.getrawdiff",
            &serde_json::json!({"diffID": diff_id}),
        )
        .map_err(|e| e.to_string())?;
    raw_diff_text(&result)
        .ok_or_else(|| "differential.getrawdiff returned an unexpected response".to_string())
}

/// Download the most recent raw diff attached to a revision.
pub fn fetch_raw_diff_for_revision(
    client: &crate::conduit::Client,
    revision: u64,
) -> Result<String, String> {
    let result = client
        .call(
            "differential.querydiffs",
            &serde_json::json!({"revisionIDs": [revision]}),
        )
        .map_err(|e| e.to_string())?;
    let diff_id = first_diff_id(&result)?;
    fetch_raw_diff_for_diff(client, &diff_id)
}

fn read_stdin_text() -> Result<String, String> {
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| format!("failed to read patch from stdin: {e}"))?;
    Ok(text)
}

fn load_patch_text(
    source: &Source,
    conduit_uri: Option<&str>,
    stdin_text: Option<&str>,
) -> Result<String, String> {
    match source {
        Source::PatchFile(path) if path == "-" => {
            let text = match stdin_text {
                Some(text) => text.to_string(),
                None => read_stdin_text()?,
            };
            if text.is_empty() {
                return Err("Failed to read patch from stdin!".to_string());
            }
            Ok(text)
        }
        Source::PatchFile(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read patch file '{path}': {e}")),
        Source::Revision(id) => {
            let client = crate::config::client_for(conduit_uri)?;
            fetch_raw_diff_for_revision(&client, *id)
        }
        Source::Diff(id) => {
            let client = crate::config::client_for(conduit_uri)?;
            fetch_raw_diff_for_diff(&client, &id.to_string())
        }
    }
}

/// Fail unless the working tree has no staged, unstaged, or untracked changes.
pub fn ensure_clean(dir: &Path) -> Result<(), String> {
    let status = crate::git::stdout_text(dir, &["status", "--porcelain"])?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(
            "working copy is not clean; commit or stash your changes first (or use --force)"
                .to_string(),
        )
    }
}

/// Apply `patch` to the git working tree rooted at `dir`.
pub fn apply_patch_text(dir: &Path, patch: &str) -> Result<(), String> {
    if !crate::git::is_inside_work_tree(dir) {
        return Err("not inside a git working copy".to_string());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!("arc-rs-patch-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| format!("failed to create scratch dir: {e}"))?;
    let patch_file = scratch.join("change.patch");
    let result = (|| -> Result<(), String> {
        std::fs::write(&patch_file, patch).map_err(|e| format!("failed to write patch: {e}"))?;
        let path = patch_file.to_string_lossy().to_string();
        let out = crate::git::output(dir, &["apply", &path])?;
        if out.status.success() {
            Ok(())
        } else {
            let detail = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let trimmed = detail.trim();
            if trimmed.is_empty() {
                Err("git apply failed".to_string())
            } else {
                Err(format!("git apply failed: {trimmed}"))
            }
        }
    })();
    std::fs::remove_dir_all(&scratch).ok();
    result
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Run the workflow; `stdin_text` overrides stdin when `--patch -` is used
/// (`None` reads real stdin).
pub fn execute_with_stdin(
    dir: &Path,
    argv: &[String],
    stdin_text: Option<&str>,
) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::NotYetPorted(message) => return Err(message),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let patch = load_patch_text(&opts.source, opts.conduit_uri.as_deref(), stdin_text)?;
    if opts.preview {
        return Ok(ensure_trailing_newline(patch));
    }
    if !opts.force {
        ensure_clean(dir)?;
    }
    apply_patch_text(dir, &patch)?;
    Ok("Applied patch to working copy.\n".to_string())
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    execute_with_stdin(dir, argv, None)
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

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "patch_tests.rs"]
mod patch_tests;
