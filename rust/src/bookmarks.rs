//! `arc-rs bookmarks`: list bookmarks in the working copy.
//!
//! PHP reference: `src/workflow/ArcanistBookmarksWorkflow.php` (through
//! `src/workflow/ArcanistMarkersWorkflow.php`). Bookmarks are a Mercurial
//! concept: inside a Mercurial working copy this lists `hg bookmarks`,
//! optionally annotated with Differential review status via
//! `--conduit-uri`; under git or svn it fails exactly like `arc` ("does
//! not support bookmarks").
//!
//! Divergence: PHP lists markers through the `arc-hg` extension and lays
//! out a commit-graph grid; this lists plain `hg bookmarks`, and annotates
//! each head from its commit message in one batched `differential.query`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{amend, config, git, hg, svn};

pub const NAME: &str = "bookmarks";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--json] [--conduit-uri URI]\n\
         \n\
         Show an enhanced view of bookmarks in the working copy.\n\
         Bookmarks are a Mercurial concept; under git or svn this command\n\
         fails, matching `arc bookmarks`.\n\
         \n\
         Options:\n\
         \x20 --json            emit a JSON array of bookmark objects\n\
         \x20 --conduit-uri URI annotate bookmarks with Differential\n\
         \x20                   review status ([D123 Accepted])\n\
         \x20 -h, --help        show this help\n"
    )
}

/// One Mercurial bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub active: bool,
    pub rev: String,
    pub node: String,
    /// Review-status annotations, filled by [`annotate`] (`--conduit-uri`).
    pub revisions: Vec<amend::AnnotatedRevision>,
}

/// Parsed `bookmarks` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub json: bool,
    pub conduit_uri: Option<String>,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut json = false;
    let mut conduit_uri = None;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--json" {
            json = true;
        } else if arg == "--conduit-uri" {
            let Some(value) = iter.next() else {
                return Err("--conduit-uri requires a non-empty value".to_string());
            };
            if value.is_empty() {
                return Err("--conduit-uri requires a non-empty value".to_string());
            }
            conduit_uri = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            if value.is_empty() {
                return Err("--conduit-uri requires a non-empty value".to_string());
            }
            conduit_uri = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            return Ok(None);
        } else if arg == "--" {
            return Err("unexpected argument '--': bookmarks takes no paths".to_string());
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(Some(Options { json, conduit_uri }))
}

/// Walk up from `dir` looking for a Mercurial working copy.
#[must_use]
pub fn find_hg_root(dir: &Path) -> Option<PathBuf> {
    hg::find_root(dir)
}

/// Parse `hg bookmarks` output: one `[*] <name> <rev>:<node>` entry per
/// line, `*` marking the active bookmark.
pub fn parse_bookmarks(text: &str) -> Result<Vec<Bookmark>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim();
        let (active, rest) = match trimmed.strip_prefix('*') {
            Some(rest) => (true, rest.trim()),
            None => (false, trimmed),
        };
        let mut tokens = rest.split_whitespace();
        let (Some(name), Some(revnode)) = (tokens.next(), tokens.next()) else {
            return Err(format!("malformed bookmarks line: {line:?}"));
        };
        let (rev, node) = match revnode.split_once(':') {
            Some((rev, node)) => (rev.to_string(), node.to_string()),
            None => (String::new(), revnode.to_string()),
        };
        out.push(Bookmark {
            name: name.to_string(),
            active,
            rev,
            node,
            revisions: Vec::new(),
        });
    }
    Ok(out)
}

/// List bookmarks under `dir` through `hg`, which must be a Mercurial
/// working copy.
pub fn list_with(dir: &Path, hg: &hg::HgCommand) -> Result<Vec<Bookmark>, String> {
    if find_hg_root(dir).is_none() {
        if git::is_inside_work_tree(dir) {
            return Err(
                "The version control system (\"git\") in the current working copy does not support bookmarks."
                    .to_string(),
            );
        }
        if svn::find_metadata_path(dir).is_some() {
            return Err(
                "The version control system (\"svn\") in the current working copy does not support bookmarks."
                    .to_string(),
            );
        }
        return Err(
            "not inside a working copy (no Mercurial '.hg' directory found; git and svn do not support bookmarks)"
                .to_string(),
        );
    }
    let output = hg::get_bookmarks_with(dir, hg)?;
    parse_bookmarks(&output)
}

/// List bookmarks under `dir`, which must be a Mercurial working copy.
pub fn list(dir: &Path) -> Result<Vec<Bookmark>, String> {
    list_with(dir, &hg::HgCommand::system())
}

/// List bookmarks under `dir` invoking `hg_binary`.
///
/// The binary seam lets tests force the Mercurial-missing path without
/// depending on what is installed on PATH.
pub fn list_with_binary(dir: &Path, hg_binary: &str) -> Result<Vec<Bookmark>, String> {
    list_with(dir, &hg::HgCommand::custom(hg_binary, &[]))
}

/// Annotate bookmarks with their head revisions' review status: one
/// `hg log` per head plus one batched `differential.query`.
pub fn annotate(
    dir: &Path,
    hg: &hg::HgCommand,
    marks: &mut [Bookmark],
    client: &crate::conduit::Client,
) -> Result<(), String> {
    let mut messages = Vec::with_capacity(marks.len());
    for mark in marks.iter() {
        messages.push(hg::get_commit_message_with(dir, hg, &mark.node)?);
    }
    let (head_ids, annotations) = amend::annotations_for_messages(client, &messages)?;
    for (mark, id) in marks.iter_mut().zip(head_ids.iter()) {
        if let Some(id) = id
            && let Some(note) = annotations.get(&id.to_string())
        {
            mark.revisions.push(note.clone());
        }
    }
    Ok(())
}

/// Render one `[D123 Accepted]` annotation suffix per revision.
fn annotation_suffix(mark: &Bookmark) -> String {
    let mut suffix = String::new();
    for note in &mark.revisions {
        suffix.push_str(&format!(" [{} {}]", note.monogram, note.status));
    }
    suffix
}

/// Render bookmarks as text, one line per bookmark.
#[must_use]
pub fn format_text(marks: &[Bookmark]) -> String {
    let mut out = String::new();
    for mark in marks {
        let star = if mark.active { '*' } else { ' ' };
        if mark.rev.is_empty() {
            out.push_str(&format!("{star} {} {}", mark.name, mark.node));
        } else {
            out.push_str(&format!("{star} {} {}:{}", mark.name, mark.rev, mark.node));
        }
        out.push_str(&annotation_suffix(mark));
        out.push('\n');
    }
    out
}

/// Render one bookmark's annotations as JSON objects.
fn revisions_json(mark: &Bookmark) -> Vec<serde_json::Value> {
    mark.revisions
        .iter()
        .map(|note| {
            serde_json::json!({
                "monogram": note.monogram,
                "status": note.status,
                "title": note.title,
            })
        })
        .collect()
}

/// Render bookmarks as a JSON array of bookmark objects.
#[must_use]
pub fn format_json(marks: &[Bookmark]) -> serde_json::Value {
    serde_json::Value::Array(
        marks
            .iter()
            .map(|mark| {
                serde_json::json!({
                    "name": mark.name,
                    "active": mark.active,
                    "rev": mark.rev,
                    "node": mark.node,
                    "revisions": revisions_json(mark),
                })
            })
            .collect(),
    )
}

/// Run the workflow rooted at `dir` through `hg` (test seam).
pub fn execute_with(dir: &Path, argv: &[String], hg: &hg::HgCommand) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let mut marks = list_with(dir, hg)?;
    if let Some(uri) = options.conduit_uri.as_deref() {
        let client = config::client_for(Some(uri))?;
        annotate(dir, hg, &mut marks, &client)?;
    }
    if options.json {
        let mut text = serde_json::to_string_pretty(&format_json(&marks))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text)
    } else {
        Ok(format_text(&marks))
    }
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    execute_with(dir, argv, &hg::HgCommand::system())
}

/// Run the workflow rooted at `dir` invoking `hg_binary` (test seam).
pub fn execute_with_binary(dir: &Path, argv: &[String], hg_binary: &str) -> Result<String, String> {
    execute_with(dir, argv, &hg::HgCommand::custom(hg_binary, &[]))
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
#[path = "bookmarks_tests.rs"]
mod bookmarks_tests;
