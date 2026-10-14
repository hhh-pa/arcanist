//! `arc-rs markers`: list branch/bookmark markers with commit context.
//!
//! PHP reference: `src/workflow/ArcanistMarkersWorkflow.php` (the abstract
//! engine: marker query, 1000-commit guard, grid columns
//! marker/commits/status/revisions/messages),
//! `src/workflow/ArcanistBookmarksWorkflow.php` and
//! `src/workflow/ArcanistBranchesWorkflow.php` (marker types and support
//! errors), `src/repository/marker/ArcanistMarkerRef.php` (type constants),
//! `src/repository/marker/ArcanistMercurialRepositoryMarkerQuery.php` (the
//! `hg arc-ls-markers` JSON shape), `src/console/grid/ArcanistGridView.php`
//! plus `src/console/grid/ArcanistGridColumn.php` (grid layout), and
//! `src/repository/graph/view/ArcanistCommitGraphSetView.php` (cell content).
//!
//! Each marker renders one grid row: the depth-zero tree bullet plus the
//! marker name (`*` when active), the 7-character commit hash, and the commit
//! subject (`ArcanistCommitGraphSetView.php:302-310,362-364,425-432,472-482`).
//! Rows sort active-first, then by commit epoch, then case-insensitive name,
//! mirroring the PHP set vectors (`ArcanistMarkersWorkflow.php:214-256`).
//! Columns join with two spaces plus one trailing newline, like the PHP grid
//! (`ArcanistGridView.php:54-56,195-197`).
//!
//! Divergences (see T1128):
//! - PHP partitions marker heads into commit-graph sets with tree depth,
//!   disjoint-head loading, and collapsed commit runs
//!   (`ArcanistMarkersWorkflow.php:54-67,94-140`,
//!   `ArcanistCommitGraphSetView.php:526-566`); rendering here is one flat
//!   row per marker at depth zero.
//! - Differential status/revision annotation is unported (same as the
//!   `bookmarks`/`branches` ports): `ArcanistMarkersWorkflow.php:69-82`
//!   hardpoints and `ArcanistCommitGraphSetView.php:434-524` feed the status
//!   and revisions columns, which stay blank here.
//! - No ANSI styling: the active marker shows as `* name` instead of the
//!   green-highlighted label (`ArcanistCommitGraphSetView.php:336-345`), and
//!   narrow-terminal truncation is not applied
//!   (`ArcanistGridView.php:86-119`).
//! - Name order uses plain case-folded comparison; PHP uses natural
//!   case-insensitive order (`ArcanistMarkersWorkflow.php:237-239,274-276`).
//! - Under Mercurial without the `arc-hg` extension, markers fall back to
//!   native `hg bookmarks` / `hg branches` output; PHP only speaks
//!   `hg arc-ls-markers` and fails without it
//!   (`ArcanistMercurialRepositoryMarkerQuery.php:14-56`).
//! - The unpublished-commit guard (`ArcanistMarkersWorkflow.php:24-44`) runs
//!   exactly under git; under Mercurial a failed revset count skips the guard
//!   instead of failing, because flat rendering never walks the commit graph.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

pub const NAME: &str = "markers";

/// `ArcanistMarkerRef::TYPE_BRANCH` (`src/repository/marker/...`, line 10).
pub const MARKER_TYPE_BRANCH: &str = "branch";
/// `ArcanistMarkerRef::TYPE_BOOKMARK` (`src/repository/marker/...`, line 11).
pub const MARKER_TYPE_BOOKMARK: &str = "bookmark";

/// Guard from `ArcanistMarkersWorkflow.php`: at most this many unpublished
/// commits may sit behind marker heads.
pub const UNPUBLISHED_LIMIT: usize = 1000;

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--type branch|bookmark] [--json]\n\
         \n\
         Show an enhanced view of markers in the working copy.\n\
         Without --type, git checkouts list branches and Mercurial\n\
         checkouts list bookmarks.\n\
         \n\
         Options:\n\
         \x20 --type TYPE   list 'branch' or 'bookmark' markers\n\
         \x20 --json        emit a JSON array of marker objects\n\
         \x20 -h, --help    show this help\n\
         \n\
         Not yet ported (see T1128): Differential review-status annotation.\n"
    )
}

/// One branch or bookmark marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub name: String,
    pub marker_type: String,
    pub active: bool,
    pub hash: String,
    pub epoch: u64,
    pub subject: String,
}

/// Parsed `markers` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub marker_type: Option<String>,
    pub json: bool,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut marker_type = None;
    let mut json = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--json" {
            json = true;
        } else if arg == "--type" {
            let Some(value) = iter.next() else {
                return Err("--type requires a value".to_string());
            };
            marker_type = Some(check_marker_type(value)?);
        } else if let Some(value) = arg.strip_prefix("--type=") {
            marker_type = Some(check_marker_type(value)?);
        } else if arg == "-h" || arg == "--help" {
            return Ok(None);
        } else if arg == "--" {
            return Err("unexpected argument '--': markers takes no paths".to_string());
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(Some(Options { marker_type, json }))
}

fn check_marker_type(value: &str) -> Result<String, String> {
    if value == MARKER_TYPE_BRANCH || value == MARKER_TYPE_BOOKMARK {
        Ok(value.to_string())
    } else {
        Err(format!(
            "unknown marker type '{value}' (expected '{MARKER_TYPE_BRANCH}' or '{MARKER_TYPE_BOOKMARK}')"
        ))
    }
}

/// Walk up from `dir` looking for a Mercurial working copy.
#[must_use]
pub fn find_hg_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|dir| dir.join(".hg").exists())
        .map(Path::to_path_buf)
}

/// Resolve the effective marker type, mirroring the PHP support checks:
/// bookmarks fail under git-only checkouts exactly like `arc bookmarks`.
fn resolve_type(dir: &Path, requested: Option<&str>) -> Result<String, String> {
    let in_git = arc_rs::git::is_inside_work_tree(dir);
    let in_hg = find_hg_root(dir).is_some();
    match requested {
        Some(MARKER_TYPE_BOOKMARK) => {
            if in_hg {
                Ok(MARKER_TYPE_BOOKMARK.to_string())
            } else if in_git {
                Err(
                    "The version control system (\"git\") in the current working copy does not support bookmarks."
                        .to_string(),
                )
            } else {
                Err("not inside a working copy (no Mercurial '.hg' directory found; git does not support bookmarks)".to_string())
            }
        }
        Some(MARKER_TYPE_BRANCH) => {
            if in_git || in_hg {
                Ok(MARKER_TYPE_BRANCH.to_string())
            } else {
                Err("not inside a working copy (no git repository or Mercurial '.hg' directory found)".to_string())
            }
        }
        Some(other) => Err(format!("unknown marker type '{other}'")),
        None => {
            if in_git {
                Ok(MARKER_TYPE_BRANCH.to_string())
            } else if in_hg {
                Ok(MARKER_TYPE_BOOKMARK.to_string())
            } else {
                Err("not inside a working copy (no git repository or Mercurial '.hg' directory found)".to_string())
            }
        }
    }
}

/// Enforce the PHP 1000-unpublished-commit guard
/// (`ArcanistMarkersWorkflow.php`).
fn check_limit(count: usize) -> Result<(), String> {
    if count > UNPUBLISHED_LIMIT {
        return Err(format!(
            "Found more than {UNPUBLISHED_LIMIT} unpublished commits which are ancestors of heads."
        ));
    }
    Ok(())
}

/// First seven characters of a commit hash, like the PHP commit labels.
#[must_use]
pub fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// Sort markers like the PHP set vectors: active first, then commit epoch,
/// then case-insensitive name.
pub fn sort_markers(markers: &mut [Marker]) {
    markers.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.epoch.cmp(&a.epoch))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then(a.name.cmp(&b.name))
            .then(a.hash.cmp(&b.hash))
    });
}

const GIT_REF_FORMAT: &str =
    "%(refname:short)%1f%(objectname)%1f%(committerdate:unix)%1f%(subject)";

fn current_git_branch(dir: &Path) -> Option<String> {
    let out = arc_rs::git::output(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn parse_git_ref_line(line: &str, current: Option<&str>) -> Result<Marker, String> {
    let mut fields = line.split('\x1f');
    let (Some(name), Some(hash), Some(epoch), Some(subject)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(format!("malformed branch record: {line:?}"));
    };
    if fields.next().is_some() {
        return Err(format!("malformed branch record: {line:?}"));
    }
    let epoch = epoch
        .parse::<u64>()
        .map_err(|_| format!("malformed branch record: {line:?}"))?;
    Ok(Marker {
        active: current == Some(name),
        name: name.to_string(),
        marker_type: MARKER_TYPE_BRANCH.to_string(),
        hash: hash.to_string(),
        epoch,
        subject: subject.to_string(),
    })
}

fn git_unpublished_count(dir: &Path, heads: &[String], tails: &[String]) -> Result<usize, String> {
    let mut args: Vec<&str> = vec!["rev-list", "--count"];
    for head in heads {
        args.push(head);
    }
    if !tails.is_empty() {
        args.push("--not");
        for tail in tails {
            args.push(tail);
        }
    }
    let text = arc_rs::git::stdout_text(dir, &args)?;
    text.trim()
        .parse::<usize>()
        .map_err(|_| format!("malformed rev-list count: {text:?}"))
}

/// List local git branches as markers, with the unpublished-commit guard.
/// Tails are the remote-tracking refs (the published side of the PHP query).
pub fn list_git_branches(dir: &Path) -> Result<Vec<Marker>, String> {
    if !arc_rs::git::is_inside_work_tree(dir) {
        return Err("not inside a git repository".to_string());
    }
    let current = current_git_branch(dir);
    let text = arc_rs::git::stdout_text(
        dir,
        &[
            "for-each-ref",
            &format!("--format={GIT_REF_FORMAT}"),
            "refs/heads",
        ],
    )?;
    let mut markers: Vec<Marker> = text
        .lines()
        .map(|line| parse_git_ref_line(line, current.as_deref()))
        .collect::<Result<_, _>>()?;
    if !markers.is_empty() {
        let heads: Vec<String> = markers.iter().map(|marker| marker.hash.clone()).collect();
        let tails_text = arc_rs::git::stdout_text(
            dir,
            &["for-each-ref", "--format=%(objectname)", "refs/remotes"],
        )?;
        let tails: Vec<String> = tails_text.lines().map(str::to_string).collect();
        let count = git_unpublished_count(dir, &heads, &tails)?;
        check_limit(count)?;
    }
    sort_markers(&mut markers);
    Ok(markers)
}

/// One decoded `hg arc-ls-markers` item.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HgLsMarker {
    name: String,
    node: String,
    active: bool,
    subject: String,
}

/// Decode `hg arc-ls-markers` JSON, skipping closed heads and filtering to
/// `marker_type`, like `ArcanistMercurialRepositoryMarkerQuery.php`.
fn parse_hg_ls_markers(text: &str, marker_type: &str) -> Result<Vec<HgLsMarker>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_str(text)
        .map_err(|e| format!("Call to \"hg arc-ls-markers\" returned invalid JSON: {e}"))?;
    let mut out = Vec::new();
    for item in &items {
        let item_type = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        match item_type {
            "branch" | "bookmark" | "commit-state" | "branch-state" => {}
            _ => {
                return Err(format!(
                    "Call to \"hg arc-ls-markers\" returned marker of unknown type \"{item_type}\"."
                ));
            }
        }
        if item
            .get("isClosed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        if item_type != marker_type {
            continue;
        }
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let node = item
            .get("node")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if name.is_empty() || node.is_empty() {
            return Err(
                "Call to \"hg arc-ls-markers\" returned a marker without a name or node."
                    .to_string(),
            );
        }
        let subject = item
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        out.push(HgLsMarker {
            name: name.to_string(),
            node: node.to_string(),
            active: item
                .get("isActive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            subject,
        });
    }
    Ok(out)
}

/// Parse one `name rev:node` line shared by `hg bookmarks` and `hg branches`.
/// A leading `*` marks the active bookmark.
fn parse_hg_name_line(line: &str) -> Result<(String, bool, String), String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("malformed marker line: empty".to_string());
    }
    let (active, rest) = match trimmed.strip_prefix('*') {
        Some(rest) => (true, rest.trim()),
        None => (false, trimmed),
    };
    let mut tokens = rest.split_whitespace();
    let (Some(name), Some(revnode)) = (tokens.next(), tokens.next()) else {
        return Err(format!("malformed marker line: {line:?}"));
    };
    let node = revnode.rsplit(':').next().unwrap_or(revnode);
    Ok((name.to_string(), active, node.to_string()))
}

/// Parse `hg log --template '{node}<US>{date|hgdate}<US>{desc|firstline}'`.
fn parse_hg_log_line(line: &str) -> Result<(String, u64, String), String> {
    let mut fields = line.split('\x1f');
    let (Some(node), Some(hgdate), Some(subject)) = (fields.next(), fields.next(), fields.next())
    else {
        return Err(format!("malformed hg log record: {line:?}"));
    };
    if fields.next().is_some() {
        return Err(format!("malformed hg log record: {line:?}"));
    }
    let epoch = hgdate
        .split_whitespace()
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("")
        .parse::<u64>()
        .map_err(|_| format!("malformed hg log record: {line:?}"))?;
    Ok((node.to_string(), epoch, subject.to_string()))
}

const HG_LOG_TEMPLATE: &str = "{node}\u{1f}{date|hgdate}\u{1f}{desc|firstline}\n";

/// Run `hg` in `dir`, returning trimmed stdout or the combined trimmed
/// output on failure (same shape as the shared git helper).
fn hg_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("hg")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run hg: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            Err(format!("hg {} failed", args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

/// Commit details for `nodes` via one `hg log` call.
fn hg_log_for_nodes(
    dir: &Path,
    nodes: &[String],
) -> Result<HashMap<String, (u64, String)>, String> {
    let mut map = HashMap::new();
    if nodes.is_empty() {
        return Ok(map);
    }
    let revset = nodes.join(" or ");
    let text = hg_text(dir, &["log", "--template", HG_LOG_TEMPLATE, "-r", &revset])?;
    for line in text.lines() {
        let (node, epoch, subject) = parse_hg_log_line(line)?;
        map.insert(node, (epoch, subject));
    }
    Ok(map)
}

fn hg_markers_from_native(
    dir: &Path,
    marker_type: &str,
    entries: Vec<(String, bool, String)>,
) -> Result<Vec<Marker>, String> {
    let nodes: Vec<String> = entries.iter().map(|(_, _, node)| node.clone()).collect();
    let details = hg_log_for_nodes(dir, &nodes)?;
    let mut markers = Vec::new();
    for (name, active, node) in entries {
        // Full nodes from `hg log` subsume the short nodes in the listing;
        // match by prefix when the exact node is absent.
        let detail = details.get(&node).cloned().or_else(|| {
            details
                .iter()
                .find(|(full, _)| full.starts_with(&node))
                .map(|(_, value)| value.clone())
        });
        let (epoch, subject) = detail.unwrap_or((0, String::new()));
        let hash = details
            .keys()
            .find(|full| *full == &node || full.starts_with(&node))
            .cloned()
            .unwrap_or(node);
        markers.push(Marker {
            name,
            marker_type: marker_type.to_string(),
            active,
            hash,
            epoch,
            subject,
        });
    }
    Ok(markers)
}

/// Native fallback when the `arc-hg` extension is unavailable: `hg bookmarks`
/// for bookmarks, `hg branch` plus `hg branches` for branches.
fn list_hg_native(dir: &Path, marker_type: &str) -> Result<Vec<Marker>, String> {
    if marker_type == MARKER_TYPE_BOOKMARK {
        let text = hg_text(dir, &["bookmarks"])?;
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            entries.push(parse_hg_name_line(line)?);
        }
        hg_markers_from_native(dir, marker_type, entries)
    } else {
        let current = hg_text(dir, &["branch"])?;
        let current = current.lines().next().unwrap_or("").trim();
        let text = hg_text(dir, &["branches"])?;
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let (name, _, node) = parse_hg_name_line(line)?;
            entries.push((name.clone(), name == current, node));
        }
        hg_markers_from_native(dir, marker_type, entries)
    }
}

/// Count unpublished commits behind `nodes`: ancestors of the heads which
/// are not public. A failed revset skips the guard (see divergences).
fn hg_unpublished_count(dir: &Path, nodes: &[String]) -> Option<usize> {
    if nodes.is_empty() {
        return Some(0);
    }
    let revset = format!("ancestors({}) and not public()", nodes.join(", "));
    let text = hg_text(dir, &["log", "--template", ".", "-r", &revset]).ok()?;
    Some(text.chars().filter(|char| *char == '.').count())
}

/// List Mercurial markers: `hg arc-ls-markers` first (the exact PHP query),
/// native `hg` output when the extension is unavailable.
pub fn list_hg_markers(dir: &Path, marker_type: &str) -> Result<Vec<Marker>, String> {
    if find_hg_root(dir).is_none() {
        return Err("not inside a Mercurial working copy".to_string());
    }
    let extension = hg_text(dir, &["arc-ls-markers", "--"]);
    let mut markers = match extension {
        Ok(text) => {
            let items = parse_hg_ls_markers(&text, marker_type)?;
            let nodes: Vec<String> = items.iter().map(|item| item.node.clone()).collect();
            let details = hg_log_for_nodes(dir, &nodes).unwrap_or_default();
            items
                .into_iter()
                .map(|item| {
                    let (epoch, logged) = details
                        .get(&item.node)
                        .cloned()
                        .unwrap_or((0, String::new()));
                    let subject = if logged.is_empty() {
                        item.subject
                    } else {
                        logged
                    };
                    Marker {
                        name: item.name,
                        marker_type: marker_type.to_string(),
                        active: item.active,
                        hash: item.node,
                        epoch,
                        subject,
                    }
                })
                .collect()
        }
        Err(_) => {
            // Without the extension PHP fails outright (`Call to
            // "hg arc-ls-markers" failed ...`); fall back to native output
            // so plain Mercurial checkouts still list markers.
            list_hg_native(dir, marker_type)?
        }
    };
    let nodes: Vec<String> = markers.iter().map(|marker| marker.hash.clone()).collect();
    if let Some(count) = hg_unpublished_count(dir, &nodes) {
        check_limit(count)?;
    }
    sort_markers(&mut markers);
    Ok(markers)
}

/// List markers of `marker_type` under `dir`, dispatching on the VCS.
pub fn list(dir: &Path, marker_type: &str) -> Result<Vec<Marker>, String> {
    let resolved = resolve_type(dir, Some(marker_type))?;
    if resolved == MARKER_TYPE_BOOKMARK {
        return list_hg_markers(dir, &resolved);
    }
    if arc_rs::git::is_inside_work_tree(dir) {
        list_git_branches(dir)
    } else {
        list_hg_markers(dir, &resolved)
    }
}

/// Pad `text` with trailing spaces to `width` glyphs (left alignment, the
/// `ArcanistGridColumn` default).
fn pad_cell(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_string();
    }
    let mut out = text.to_string();
    out.push_str(&" ".repeat(width - len));
    out
}

/// Render markers as the PHP grid: columns marker/commits/status/revisions/
/// messages joined with two spaces, one trailing newline.
#[must_use]
pub fn render_grid(markers: &[Marker]) -> String {
    if markers.is_empty() {
        return "\n".to_string();
    }
    let mut rows: Vec<[String; 5]> = Vec::new();
    for marker in markers {
        let label = if marker.active {
            format!("* {}", marker.name)
        } else {
            marker.name.clone()
        };
        rows.push([
            format!("\u{2022} {label}"),
            short_hash(&marker.hash).to_string(),
            String::new(),
            String::new(),
            marker.subject.clone(),
        ]);
    }
    let mut widths = [1usize; 5];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(index, cell)| pad_cell(cell, widths[index]))
            .collect();
        out.push_str(&cells.join("  "));
        out.push('\n');
    }
    out
}

/// Render markers as a JSON array of marker objects.
#[must_use]
pub fn format_json(markers: &[Marker]) -> serde_json::Value {
    serde_json::Value::Array(
        markers
            .iter()
            .map(|marker| {
                serde_json::json!({
                    "name": marker.name,
                    "type": marker.marker_type,
                    "active": marker.active,
                    "hash": marker.hash,
                    "epoch": marker.epoch,
                    "subject": marker.subject,
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
    let marker_type = resolve_type(dir, options.marker_type.as_deref())?;
    let markers = list(dir, &marker_type)?;
    if options.json {
        let mut text = serde_json::to_string_pretty(&format_json(&markers))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text)
    } else {
        Ok(render_grid(&markers))
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
#[path = "markers_tests.rs"]
mod markers_tests;
