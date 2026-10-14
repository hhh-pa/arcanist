//! `arc-rs bookmarks`: list bookmarks in the working copy.
//!
//! PHP reference: `src/workflow/ArcanistBookmarksWorkflow.php` (through
//! `src/workflow/ArcanistMarkersWorkflow.php`). Bookmarks are a Mercurial
//! concept: inside a Mercurial working copy this lists `hg bookmarks`;
//! under git it fails exactly like `arc` ("does not support bookmarks").
//!
//! Not yet ported (see T1128): Differential review-status annotation.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::git;

pub const NAME: &str = "bookmarks";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--json]\n\
         \n\
         Show an enhanced view of bookmarks in the working copy.\n\
         Bookmarks are a Mercurial concept; under git this command fails,\n\
         matching `arc bookmarks`.\n\
         \n\
         Options:\n\
         \x20 --json        emit a JSON array of bookmark objects\n\
         \x20 -h, --help    show this help\n\
         \n\
         Not yet ported (see T1128): Differential review-status annotation.\n"
    )
}

/// One Mercurial bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub active: bool,
    pub rev: String,
    pub node: String,
}

/// Parsed `bookmarks` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub json: bool,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut json = false;
    for arg in argv {
        if arg == "--json" {
            json = true;
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
    Ok(Some(Options { json }))
}

/// Walk up from `dir` looking for a Mercurial working copy.
#[must_use]
pub fn find_hg_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|dir| dir.join(".hg").exists())
        .map(Path::to_path_buf)
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
        });
    }
    Ok(out)
}

/// List bookmarks under `dir`, which must be a Mercurial working copy.
pub fn list(dir: &Path) -> Result<Vec<Bookmark>, String> {
    if find_hg_root(dir).is_none() {
        if git::is_inside_work_tree(dir) {
            return Err(
                "The version control system (\"git\") in the current working copy does not support bookmarks."
                    .to_string(),
            );
        }
        return Err(
            "not inside a working copy (no Mercurial '.hg' directory found; git does not support bookmarks)"
                .to_string(),
        );
    }
    let output = std::process::Command::new("hg")
        .current_dir(dir)
        .args(["bookmarks"])
        .output()
        .map_err(|e| format!("bookmarks require Mercurial ('hg' failed to run: {e})"))?;
    if !output.status.success() {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = detail.trim();
        if detail.is_empty() {
            return Err("hg bookmarks failed".to_string());
        }
        return Err(format!("hg bookmarks failed: {detail}"));
    }
    parse_bookmarks(&String::from_utf8_lossy(&output.stdout))
}

/// Render bookmarks as text, one line per bookmark.
#[must_use]
pub fn format_text(marks: &[Bookmark]) -> String {
    let mut out = String::new();
    for mark in marks {
        let star = if mark.active { '*' } else { ' ' };
        if mark.rev.is_empty() {
            out.push_str(&format!("{star} {} {}\n", mark.name, mark.node));
        } else {
            out.push_str(&format!(
                "{star} {} {}:{}\n",
                mark.name, mark.rev, mark.node
            ));
        }
    }
    out
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
                })
            })
            .collect(),
    )
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let marks = list(dir)?;
    if options.json {
        let mut text = serde_json::to_string_pretty(&format_json(&marks))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text)
    } else {
        Ok(format_text(&marks))
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
#[path = "bookmarks_tests.rs"]
mod bookmarks_tests;
