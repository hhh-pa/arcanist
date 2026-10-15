//! `arc-rs branches`: list branches in the working copy.
//!
//! PHP reference: `src/workflow/ArcanistBranchesWorkflow.php` (through
//! `src/workflow/ArcanistMarkersWorkflow.php`), which lists branch markers
//! annotated with Differential review status. The text output renders the
//! full markers engine (commit graph, partitions, sets, tree grid); see
//! `markers` for the shared implementation.
//!
//! Extensions over `arc branches` (which takes no arguments): `--all`
//! heads remote-tracking branches too, `--json` emits branch objects, and
//! `--conduit-uri` overrides conduit discovery.

use std::path::Path;
use std::process::ExitCode;

use crate::{config, git, grid, markers};

pub const NAME: &str = "branches";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--all] [--json] [--conduit-uri URI]\n\
         \n\
         Show an enhanced view of branches in the working copy.\n\
         \n\
         Options:\n\
         \x20 --all              include remote-tracking branches\n\
         \x20 --json             emit a JSON array of branch objects\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI)\n\
         \x20 -h, --help         show this help\n"
    )
}

/// One listed branch (the `--json` shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub hash: String,
    pub subject: String,
    pub upstream: Option<String>,
    pub track: String,
    pub remote: bool,
}

/// Parsed `branches` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub all: bool,
    pub json: bool,
    pub conduit_uri: Option<String>,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut all = false;
    let mut json = false;
    let mut conduit_uri = None;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--all" {
            all = true;
        } else if arg == "--json" {
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
            return Err("unexpected argument '--': branches takes no paths".to_string());
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(Some(Options {
        all,
        json,
        conduit_uri,
    }))
}

const REF_FORMAT: &str =
    "%(refname:short)%1f%(objectname:short)%1f%(subject)%1f%(upstream:short)%1f%(upstream:track)";

fn current_branch(dir: &Path) -> Option<String> {
    let out = git::output(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn parse_ref_line(line: &str, current: Option<&str>, remote: bool) -> Result<Branch, String> {
    let mut fields = line.split('\x1f');
    let (Some(name), Some(hash), Some(subject), Some(upstream), Some(track)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(format!("malformed branch record: {line:?}"));
    };
    if fields.next().is_some() {
        return Err(format!("malformed branch record: {line:?}"));
    }
    Ok(Branch {
        current: !remote && current == Some(name),
        name: name.to_string(),
        hash: hash.to_string(),
        subject: subject.to_string(),
        upstream: if upstream.is_empty() {
            None
        } else {
            Some(upstream.to_string())
        },
        track: track.to_string(),
        remote,
    })
}

fn refs_in(
    dir: &Path,
    namespace: &str,
    current: Option<&str>,
    remote: bool,
) -> Result<Vec<Branch>, String> {
    let text = git::stdout_text(
        dir,
        &["for-each-ref", &format!("--format={REF_FORMAT}"), namespace],
    )?;
    text.lines()
        .map(|line| parse_ref_line(line, current, remote))
        .collect()
}

/// List branches under `dir`: local branches, plus remote-tracking ones
/// when `all` is set. The current branch sorts first, then locals before
/// remotes, then case-insensitive name order.
pub fn list(dir: &Path, all: bool) -> Result<Vec<Branch>, String> {
    if !git::is_inside_work_tree(dir) {
        return Err("not inside a git repository".to_string());
    }
    let current = current_branch(dir);
    let mut branches = refs_in(dir, "refs/heads", current.as_deref(), false)?;
    if all {
        branches.extend(refs_in(dir, "refs/remotes", current.as_deref(), true)?);
    }
    branches.sort_by(|a, b| {
        (!a.current, a.remote, a.name.to_lowercase(), &a.name).cmp(&(
            !b.current,
            b.remote,
            b.name.to_lowercase(),
            &b.name,
        ))
    });
    Ok(branches)
}

/// Render branches as a JSON array of branch objects.
#[must_use]
pub fn format_json(branches: &[Branch]) -> serde_json::Value {
    serde_json::Value::Array(
        branches
            .iter()
            .map(|branch| {
                serde_json::json!({
                    "name": branch.name,
                    "current": branch.current,
                    "hash": branch.hash,
                    "subject": branch.subject,
                    "upstream": branch.upstream,
                    "track": branch.track,
                    "remote": branch.remote,
                })
            })
            .collect(),
    )
}

fn detect_vcs(dir: &Path) -> Option<&'static str> {
    if git::is_inside_work_tree(dir) {
        Some("git")
    } else if markers::find_hg_root(dir).is_some() {
        Some("hg")
    } else if markers::find_svn_root(dir).is_some() {
        Some("svn")
    } else {
        None
    }
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let owned_client = config::client_for(options.conduit_uri.as_deref()).ok();
    execute_core(dir, &options, grid::terminal_width(), owned_client.as_ref())
}

/// Run the workflow with an explicit terminal width and conduit client.
///
/// Test seam: production callers use [`execute`].
pub fn execute_with_client(
    dir: &Path,
    argv: &[String],
    terminal: Option<usize>,
    client: Option<&crate::conduit::Client>,
) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    execute_core(dir, &options, terminal, client)
}

fn execute_core(
    dir: &Path,
    options: &Options,
    terminal: Option<usize>,
    client: Option<&crate::conduit::Client>,
) -> Result<String, String> {
    if options.json {
        return match detect_vcs(dir) {
            Some("hg") => {
                let found = markers::list(dir, markers::MARKER_TYPE_BRANCH)?;
                let branches: Vec<Branch> = found
                    .iter()
                    .map(|marker| Branch {
                        name: marker.name.clone(),
                        current: marker.active,
                        hash: markers::short_hash(&marker.hash).to_string(),
                        subject: marker.subject.clone(),
                        upstream: None,
                        track: String::new(),
                        remote: false,
                    })
                    .collect();
                let mut text = serde_json::to_string_pretty(&format_json(&branches))
                    .map_err(|e| format!("failed to encode JSON: {e}"))?;
                text.push('\n');
                Ok(text)
            }
            _ => {
                let branches = list(dir, options.all)?;
                let mut text = serde_json::to_string_pretty(&format_json(&branches))
                    .map_err(|e| format!("failed to encode JSON: {e}"))?;
                text.push('\n');
                Ok(text)
            }
        };
    }
    let vcs = detect_vcs(dir).ok_or_else(|| "not inside a git repository".to_string())?;
    if !markers::supports_marker_type(vcs, markers::MARKER_TYPE_BRANCH) {
        return Err(markers::support_error(vcs, markers::MARKER_TYPE_BRANCH));
    }
    markers::run_workflow(
        dir,
        vcs,
        markers::MARKER_TYPE_BRANCH,
        options.all,
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
#[path = "branches_tests.rs"]
mod branches_tests;
