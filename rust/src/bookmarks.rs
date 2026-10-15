//! `arc bookmarks`: list bookmarks in the working copy.
//!
//! PHP reference: `src/workflow/ArcanistBookmarksWorkflow.php` (through
//! `src/workflow/ArcanistMarkersWorkflow.php`). Bookmarks are a Mercurial
//! concept: inside a Mercurial working copy this renders the full markers
//! engine over `hg arc-ls-markers` bookmarks; under git or Subversion it
//! fails exactly like `arc` ("does not support bookmarks").
//!
//! Extensions over `arc bookmarks` (which takes no arguments): `--json`
//! emits bookmark objects, and `--conduit-uri` overrides conduit
//! discovery.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{config, git, grid, hg, markers};

pub const NAME: &str = "bookmarks";

fn usage() -> String {
    format!(
        "usage: arc {NAME} [--json] [--conduit-uri URI]\n\
         \n\
         Show an enhanced view of bookmarks in the working copy.\n\
         Bookmarks are a Mercurial concept; under git this command fails,\n\
         matching `arc bookmarks`.\n\
         \n\
         Options:\n\
         \x20 --json             emit a JSON array of bookmark objects\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI)\n\
         \x20 -h, --help         show this help\n"
    )
}

/// One Mercurial bookmark (the `--json` shape).
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
                return Err("--conduit-uri requires a value".to_string());
            };
            conduit_uri = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
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
    dir.ancestors()
        .find(|dir| dir.join(".hg").exists())
        .map(Path::to_path_buf)
}

/// List bookmarks under `dir`, which must be a Mercurial working copy.
pub fn list(dir: &Path) -> Result<Vec<Bookmark>, String> {
    list_with(dir, &hg::HgCommand::system())
}

/// List bookmarks invoking a custom `hg` program.
///
/// The binary seam lets tests force the Mercurial-missing path without
/// depending on what is installed on PATH.
pub fn list_with_binary(dir: &Path, hg_binary: &str) -> Result<Vec<Bookmark>, String> {
    list_with(dir, &hg::HgCommand::custom(hg_binary, &[]))
}

fn list_with(dir: &Path, hg_command: &hg::HgCommand) -> Result<Vec<Bookmark>, String> {
    if find_hg_root(dir).is_none() {
        if git::is_inside_work_tree(dir) {
            return Err(markers::support_error("git", markers::MARKER_TYPE_BOOKMARK));
        }
        return Err(
            "not inside a working copy (no Mercurial '.hg' directory found; git does not support bookmarks)"
                .to_string(),
        );
    }
    let refs = markers::query_hg_markers_with(dir, hg_command, markers::MARKER_TYPE_BOOKMARK)?;
    let mut marks: Vec<Bookmark> = refs
        .iter()
        .map(|marker| Bookmark {
            name: marker.name.clone(),
            active: marker.active,
            // `hg arc-ls-markers` reports nodes, not revision numbers.
            rev: String::new(),
            node: marker.commit_hash.clone(),
        })
        .collect();
    marks.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(crate::commitgraph::natural_cmp(
                a.name.as_bytes(),
                b.name.as_bytes(),
                true,
            ))
    });
    Ok(marks)
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
    let owned_client = config::client_for(options.conduit_uri.as_deref()).ok();
    execute_core(
        dir,
        &options,
        &hg::HgCommand::system(),
        grid::terminal_width(),
        owned_client.as_ref(),
    )
}

/// Run the workflow rooted at `dir` invoking `hg_binary` (test seam).
pub fn execute_with_binary(dir: &Path, argv: &[String], hg_binary: &str) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let owned_client = config::client_for(options.conduit_uri.as_deref()).ok();
    execute_core(
        dir,
        &options,
        &hg::HgCommand::custom(hg_binary, &[]),
        grid::terminal_width(),
        owned_client.as_ref(),
    )
}

fn execute_core(
    dir: &Path,
    options: &Options,
    hg_command: &hg::HgCommand,
    terminal: Option<usize>,
    client: Option<&crate::conduit::Client>,
) -> Result<String, String> {
    if options.json {
        let marks = list_with(dir, hg_command)?;
        let mut text = serde_json::to_string_pretty(&format_json(&marks))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        return Ok(text);
    }
    if find_hg_root(dir).is_none() {
        if git::is_inside_work_tree(dir) {
            return Err(markers::support_error("git", markers::MARKER_TYPE_BOOKMARK));
        }
        if markers::find_svn_root(dir).is_some() {
            return Err(markers::support_error("svn", markers::MARKER_TYPE_BOOKMARK));
        }
        return Err(
            "not inside a working copy (no Mercurial '.hg' directory found; git does not support bookmarks)"
                .to_string(),
        );
    }
    markers::run_workflow_with(
        dir,
        hg_command,
        "hg",
        markers::MARKER_TYPE_BOOKMARK,
        false,
        client,
        terminal,
    )
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
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
