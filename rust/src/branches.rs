//! `arc-rs branches`: list branches in the working copy.
//!
//! PHP reference: `src/workflow/ArcanistBranchesWorkflow.php` (through
//! `src/workflow/ArcanistMarkersWorkflow.php`), which lists branch markers
//! annotated with Differential review status. This port lists local git
//! branches — current marker, short hash, subject, upstream tracking —
//! Mercurial branches under hg, and annotates heads with Differential
//! review status via `--conduit-uri`. `--all` adds remote-tracking
//! branches, `--json` emits branch objects.
//!
//! Divergence: PHP lays out a commit-graph grid through the `arc-hg`
//! extension; this lists plain `git for-each-ref` / `hg branches` rows,
//! and annotates each head from its commit message in one batched
//! `differential.query`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use crate::{amend, config, git, hg, svn};

pub const NAME: &str = "branches";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--all] [--json] [--conduit-uri URI]\n\
         \n\
         Show an enhanced view of branches in the working copy.\n\
         \n\
         Options:\n\
         \x20 --all            include remote-tracking branches\n\
         \x20 --json           emit a JSON array of branch objects\n\
         \x20 --conduit-uri URI annotate branches with Differential\n\
         \x20                   review-status ([D123 Accepted])\n\
         \x20 -h, --help       show this help\n"
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
    /// Review-status annotations, filled by [`annotate`] (`--conduit-uri`).
    pub revisions: Vec<amend::AnnotatedRevision>,
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
        revisions: Vec::new(),
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

/// Sort branches: current first, then locals before remotes, then
/// case-insensitive name order.
fn sort_branches(branches: &mut [Branch]) {
    branches.sort_by(|a, b| {
        (!a.current, a.remote, a.name.to_lowercase(), &a.name).cmp(&(
            !b.current,
            b.remote,
            b.name.to_lowercase(),
            &b.name,
        ))
    });
}

/// List git branches under `dir`: local branches, plus remote-tracking
/// ones when `all` is set.
fn list_git(dir: &Path, all: bool) -> Result<Vec<Branch>, String> {
    let current = current_branch(dir);
    let mut branches = refs_in(dir, "refs/heads", current.as_deref(), false)?;
    if all {
        branches.extend(refs_in(dir, "refs/remotes", current.as_deref(), true)?);
    }
    sort_branches(&mut branches);
    Ok(branches)
}

/// One Mercurial branch row: subject from the head commit.
fn hg_branch(
    dir: &Path,
    hg: &hg::HgCommand,
    name: &str,
    head: &hg::HgBranch,
    current: &str,
) -> Result<Branch, String> {
    let subject = hg::get_commit_summary_with(dir, hg, &head.rev)?;
    Ok(Branch {
        name: name.to_string(),
        current: name == current,
        hash: head.rev.clone(),
        subject,
        upstream: None,
        track: String::new(),
        remote: false,
        revisions: Vec::new(),
    })
}

/// List Mercurial branches under `dir` (`hg branches`).
fn list_hg(dir: &Path, hg: &hg::HgCommand) -> Result<Vec<Branch>, String> {
    let output = hg::get_branches_with(dir, hg)?;
    let heads = hg::parse_mercurial_branches(&output)?;
    let current = hg::get_branch_name_with(dir, hg)?;
    let mut branches = Vec::with_capacity(heads.len());
    for (name, head) in &heads {
        branches.push(hg_branch(dir, hg, name, head, &current)?);
    }
    sort_branches(&mut branches);
    Ok(branches)
}

/// List branches under `dir` through `hg` for Mercurial working copies:
/// local branches, plus remote-tracking ones when `all` is set (git).
pub fn list_with(dir: &Path, all: bool, hg: &hg::HgCommand) -> Result<Vec<Branch>, String> {
    if git::is_inside_work_tree(dir) {
        return list_git(dir, all);
    }
    if hg::find_root(dir).is_some() {
        return list_hg(dir, hg);
    }
    if svn::find_metadata_path(dir).is_some() {
        return Err(
            "The version control system (\"svn\") in the current working copy does not support branches."
                .to_string(),
        );
    }
    Err("not inside a git or Mercurial working copy".to_string())
}

/// List branches under `dir`: local branches, plus remote-tracking ones
/// when `all` is set.
pub fn list(dir: &Path, all: bool) -> Result<Vec<Branch>, String> {
    list_with(dir, all, &hg::HgCommand::system())
}

/// Attach annotations from per-head ids plus the batched lookup map.
fn attach_annotations(
    branches: &mut [Branch],
    head_ids: &[Option<u64>],
    annotations: &BTreeMap<String, amend::AnnotatedRevision>,
) {
    for (branch, id) in branches.iter_mut().zip(head_ids.iter()) {
        if let Some(id) = id
            && let Some(note) = annotations.get(&id.to_string())
        {
            branch.revisions.push(note.clone());
        }
    }
}

/// Annotate git branches with their head revisions' review status: one
/// `git log` per head plus one batched `differential.query`.
pub fn annotate_git(
    dir: &Path,
    branches: &mut [Branch],
    client: &crate::conduit::Client,
) -> Result<(), String> {
    let mut messages = Vec::with_capacity(branches.len());
    for branch in branches.iter() {
        messages.push(git::commit_message(dir, &branch.name)?);
    }
    let (head_ids, annotations) = amend::annotations_for_messages(client, &messages)?;
    attach_annotations(branches, &head_ids, &annotations);
    Ok(())
}

/// Annotate Mercurial branches with their head revisions' review status:
/// one `hg log` per head plus one batched `differential.query`.
pub fn annotate_hg(
    dir: &Path,
    hg: &hg::HgCommand,
    branches: &mut [Branch],
    client: &crate::conduit::Client,
) -> Result<(), String> {
    let mut messages = Vec::with_capacity(branches.len());
    for branch in branches.iter() {
        messages.push(hg::get_commit_message_with(dir, hg, &branch.name)?);
    }
    let (head_ids, annotations) = amend::annotations_for_messages(client, &messages)?;
    attach_annotations(branches, &head_ids, &annotations);
    Ok(())
}

/// Annotate branches through the VCS they were listed from.
fn annotate(
    dir: &Path,
    hg: &hg::HgCommand,
    branches: &mut [Branch],
    client: &crate::conduit::Client,
) -> Result<(), String> {
    if git::is_inside_work_tree(dir) {
        annotate_git(dir, branches, client)
    } else {
        annotate_hg(dir, hg, branches, client)
    }
}

/// Render one `[D123 Accepted]` annotation suffix per revision.
fn annotation_suffix(branch: &Branch) -> String {
    let mut suffix = String::new();
    for note in &branch.revisions {
        suffix.push_str(&format!(" [{} {}]", note.monogram, note.status));
    }
    suffix
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
        out.push_str(&annotation_suffix(branch));
        out.push('\n');
    }
    out
}

/// Render one branch's annotations as JSON objects.
fn revisions_json(branch: &Branch) -> Vec<serde_json::Value> {
    branch
        .revisions
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
                    "revisions": revisions_json(branch),
                })
            })
            .collect(),
    )
}

/// Run the workflow rooted at `dir` through `hg` for Mercurial working
/// copies (test seam).
pub fn execute_with(dir: &Path, argv: &[String], hg: &hg::HgCommand) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let mut branches = list_with(dir, options.all, hg)?;
    if let Some(uri) = options.conduit_uri.as_deref() {
        let client = config::client_for(Some(uri))?;
        annotate(dir, hg, &mut branches, &client)?;
    }
    if options.json {
        let mut text = serde_json::to_string_pretty(&format_json(&branches))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text)
    } else {
        Ok(format_text(&branches))
    }
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    execute_with(dir, argv, &hg::HgCommand::system())
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
