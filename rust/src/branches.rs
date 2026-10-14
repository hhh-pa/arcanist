//! `arc-rs branches`: list branches in the working copy.
//!
//! PHP reference: `src/workflow/ArcanistBranchesWorkflow.php` (through
//! `src/workflow/ArcanistMarkersWorkflow.php`), which lists branch markers
//! annotated with Differential review status. This port lists local git
//! branches — current marker, short hash, subject, upstream tracking — and
//! adds `--all` (remote-tracking branches) and `--json`.
//!
//! Not yet ported (see T1128): Differential review-status annotation.

use std::path::Path;
use std::process::ExitCode;

use crate::git;

pub const NAME: &str = "branches";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--all] [--json]\n\
         \n\
         Show an enhanced view of branches in the working copy.\n\
         \n\
         Options:\n\
         \x20 --all         include remote-tracking branches\n\
         \x20 --json        emit a JSON array of branch objects\n\
         \x20 -h, --help    show this help\n\
         \n\
         Not yet ported (see T1128): Differential review-status annotation.\n"
    )
}

/// One listed branch.
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
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut all = false;
    let mut json = false;
    for arg in argv {
        if arg == "--all" {
            all = true;
        } else if arg == "--json" {
            json = true;
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
    Ok(Some(Options { all, json }))
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

/// Render branches as text, one line per branch.
#[must_use]
pub fn format_text(branches: &[Branch]) -> String {
    let mut out = String::new();
    for branch in branches {
        let marker = if branch.current { '*' } else { ' ' };
        out.push_str(&format!(
            "{marker} {} {} {}",
            branch.name, branch.hash, branch.subject
        ));
        if branch.remote {
            out.push_str(" (remote)");
        } else if let Some(upstream) = &branch.upstream {
            out.push_str(&format!(" -> {upstream}"));
            if !branch.track.is_empty() {
                out.push_str(&format!(" {}", branch.track));
            }
        }
        out.push('\n');
    }
    out
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

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let branches = list(dir, options.all)?;
    if options.json {
        let mut text = serde_json::to_string_pretty(&format_json(&branches))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text)
    } else {
        Ok(format_text(&branches))
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
#[path = "branches_tests.rs"]
mod branches_tests;
