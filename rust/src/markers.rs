//! `arc markers`: list branch/bookmark markers with commit context.
//!
//! PHP reference: `src/workflow/ArcanistMarkersWorkflow.php` (the engine),
//! `src/workflow/ArcanistBookmarksWorkflow.php` and
//! `src/workflow/ArcanistBranchesWorkflow.php` (marker types and support
//! errors), `src/repository/marker/ArcanistMarkerRef.php` (type constants),
//! `src/repository/marker/ArcanistGitRepositoryMarkerQuery.php` and
//! `src/repository/marker/ArcanistMercurialRepositoryMarkerQuery.php`
//! (marker queries), `src/repository/remote/` (remote refs, URI
//! normalization, permanent-ref rules), `src/repository/graph/` (commit
//! graph, partitions, sets), `src/repository/graph/view/` (tree and cell
//! views), and `src/console/grid/` (grid layout).
//!
//! `arc` itself has no `markers` command: this workflow exposes the shared
//! markers engine with `--type` picking the marker flavor. `branches` and
//! `bookmarks` are thin wrappers over the same engine. `--json` emits the
//! marker list; `--conduit-uri` overrides conduit discovery like the other
//! commands.
//!
//! Console output goes through the shared terminal API: marker labels
//! use the `ArcanistCommitGraphSetView.php:336-345` patterns (active:
//! `<bg:green>**%s**</bg> **%s**`, inactive: `**%s**`) and revision
//! statuses use the `:509-524` `<fg:color>` wrap, all gated on the
//! process TTY setting like `tsprintf()`.
//!
//! Divergences:
//! - `regexp(...)` permanent-ref rules use the matcher's supported subset
//!   (see `revision`); exact-name rules behave identically.
//! - A `refs/remotes/` ref without a `/` (which git never emits) is
//!   skipped instead of producing a null-named marker.
//! - `hg arc-ls-markers` decode failures keep the Rust-side error text:
//!   PHP's detail bytes come from the external JsonLint library.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::terminal::TVal;
use crate::{
    argparse, commitgraph,
    conduit::{self, EnvelopeKind},
    config, get_config, grid, hg, revision, terminal,
};

pub const NAME: &str = "markers";

/// `ArcanistMarkerRef::TYPE_BRANCH`.
pub const MARKER_TYPE_BRANCH: &str = "branch";
/// `ArcanistMarkerRef::TYPE_BOOKMARK`.
pub const MARKER_TYPE_BOOKMARK: &str = "bookmark";
/// `ArcanistMarkerRef::TYPE_COMMIT_STATE`.
pub const MARKER_TYPE_COMMIT_STATE: &str = "commit-state";
/// `ArcanistMarkerRef::TYPE_BRANCH_STATE`.
pub const MARKER_TYPE_BRANCH_STATE: &str = "branch-state";

/// Guard from `ArcanistMarkersWorkflow.php`: at most this many unpublished
/// commits may sit behind marker heads.
pub const UNPUBLISHED_LIMIT: usize = 1000;

fn usage() -> String {
    format!(
        "usage: arc {NAME} [--type branch|bookmark] [--json] [--conduit-uri URI]\n\
         \n\
         Show an enhanced view of markers in the working copy.\n\
         Without --type, git checkouts list branches and Mercurial\n\
         checkouts list bookmarks.\n\
         \n\
         Options:\n\
         \x20 --type TYPE        list 'branch' or 'bookmark' markers\n\
         \x20 --json             emit a JSON array of marker objects\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI)\n\
         \x20 -h, --help         show this help\n"
    )
}

/// One branch or bookmark marker (the `--json` shape).
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
    pub conduit_uri: Option<String>,
}

/// True when `-h`/`--help` appears before any `--` terminator (direct
/// calls still reroute to usage; the binary strips standard help flags
/// globally first).
fn wants_help(argv: &[String]) -> bool {
    let mut terminated = false;
    for arg in argv {
        if arg == "--" {
            terminated = true;
        } else if !terminated && (arg == "-h" || arg == "--help") {
            return true;
        }
    }
    false
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
///
/// T1153 LAND-42: toolset workflows parse through
/// `PhutilArgumentParser` (`parseWorkflowsFull`), so this shares the
/// ported texts (`Argument "--x" requires a parameter.`,
/// `Argument "--x" does not take a parameter.`,
/// `Argument "--x" was provided twice.`,
/// `Argument "--x" is unrecognized. ...`, and
/// `Unrecognized argument "y".` for positionals). Interactive
/// British-spelling correction stays off (its stderr notices have no
/// plumbing here).
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    if wants_help(argv) {
        return Ok(None);
    }
    let parsed = argparse::parse_toolset(
        argv,
        &[
            argparse::FlagSpec::valued("type", "type"),
            argparse::FlagSpec::boolean("json"),
            argparse::FlagSpec::valued("conduit-uri", "uri"),
        ],
        false,
        false,
    )?;
    let marker_type = match parsed.value("type") {
        None => None,
        Some(raw) => Some(check_marker_type(raw)?),
    };
    Ok(Some(Options {
        marker_type,
        json: parsed.flag("json"),
        conduit_uri: parsed.value("conduit-uri").map(str::to_string),
    }))
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

/// Walk up from `dir` looking for a Subversion working copy.
#[must_use]
pub fn find_svn_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|dir| dir.join(".svn").exists())
        .map(Path::to_path_buf)
}

/// Resolve the effective marker type, mirroring the PHP support checks.
fn resolve_type(dir: &Path, requested: Option<&str>) -> Result<String, String> {
    let in_git = crate::git::is_inside_work_tree(dir);
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

/// The PHP branch/bookmark support error for `vcs` (`git`, `hg`, `svn`).
#[must_use]
pub fn support_error(vcs: &str, marker_type: &str) -> String {
    let noun = if marker_type == MARKER_TYPE_BOOKMARK {
        "bookmarks"
    } else {
        "branches"
    };
    format!(
        "The version control system (\"{vcs}\") in the current working copy does not support {noun}."
    )
}

/// True when `vcs` supports `marker_type` (`newSupportedMarkerTypes()`):
/// git supports branches, Mercurial supports branches and bookmarks,
/// Subversion supports neither.
#[must_use]
pub fn supports_marker_type(vcs: &str, marker_type: &str) -> bool {
    match vcs {
        "git" => marker_type == MARKER_TYPE_BRANCH,
        "hg" => marker_type == MARKER_TYPE_BRANCH || marker_type == MARKER_TYPE_BOOKMARK,
        _ => false,
    }
}

/// Enforce the PHP 1000-unpublished-commit guard.
fn check_limit(count: usize) -> Result<(), String> {
    if count > UNPUBLISHED_LIMIT {
        return Err(format!(
            "Found more than {} unpublished commits which are ancestors of heads.",
            grid::group_number(UNPUBLISHED_LIMIT as u64)
        ));
    }
    Ok(())
}

/// First seven characters of a commit hash, like the PHP commit labels.
#[must_use]
pub fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// First twelve characters of a hash (`getDisplayHash()`).
#[must_use]
pub fn display_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

/// Sort markers active-first, then by commit epoch, then by
/// case-insensitive natural name order (the PHP set-vector order).
pub fn sort_markers(markers: &mut [Marker]) {
    markers.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.epoch.cmp(&a.epoch))
            .then(commitgraph::natural_cmp(
                a.name.as_bytes(),
                b.name.as_bytes(),
                true,
            ))
            .then(a.name.cmp(&b.name))
            .then(a.hash.cmp(&b.hash))
    });
}

/// One repository marker (`ArcanistMarkerRef`).
#[derive(Debug, Clone)]
pub struct MarkerRef {
    /// Marker name (branch or bookmark name).
    pub name: String,
    /// `branch`, `bookmark`, `commit-state`, or `branch-state`.
    pub marker_type: String,
    /// Committer epoch (git only).
    pub epoch: i64,
    /// The ref hash itself (git only).
    pub marker_hash: Option<String>,
    /// The commit the marker points at.
    pub commit_hash: String,
    /// First line of the commit message.
    pub summary: String,
    /// Full commit message, when the marker query provides one.
    pub message: Option<String>,
    /// Whether this is the checked-out branch or active bookmark.
    pub active: bool,
    /// Remote name for remote-tracking markers.
    pub remote_name: Option<String>,
}

impl MarkerRef {
    /// Whether this marker came from the remote cache (`refs/remotes/`).
    #[must_use]
    pub fn is_remote_cache(&self) -> bool {
        self.remote_name.is_some()
    }
}

/// `git for-each-ref` format for the exact PHP marker field list.
const GIT_REF_FORMAT: &str = "%(refname)%01%(objectname)%01%(committerdate:raw)%01%(tree)%01%(*objectname)%01%(subject)%01%(subject)%0a%0a%(body)%01%02";

/// Parse one `\x02`-terminated `for-each-ref` record (PHP's 8-field list).
fn parse_git_ref_record(record: &str) -> Result<Option<MarkerRef>, String> {
    let line = record.trim();
    if line.is_empty() {
        return Ok(None);
    }
    // `explode("\1", $line, 8)`: at most 8 pieces, exactly 8 expected.
    let fields: Vec<&str> = line.splitn(8, '\x01').collect();
    if fields.len() != 8 {
        return Err(format!(
            "Unexpected field count when parsing line \"{line}\", got {} but expected {}.",
            grid::group_number(fields.len() as u64),
            grid::group_number(8)
        ));
    }
    let (reference, hash, epoch, tree, dst_hash, summary, text) = (
        fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6],
    );
    let _ = tree;
    let (name, remote_name) = if let Some(name) = reference.strip_prefix("refs/heads/") {
        (name.to_string(), None)
    } else if let Some(label) = reference.strip_prefix("refs/remotes/") {
        // `explode('/', $label, 2)`; a bare remote name never occurs.
        let Some((remote, name)) = label.split_once('/') else {
            return Ok(None);
        };
        (name.to_string(), Some(remote.to_string()))
    } else {
        // For now, discard other refs.
        return Ok(None);
    };
    let commit_hash = if dst_hash.is_empty() {
        hash.to_string()
    } else {
        dst_hash.to_string()
    };
    Ok(Some(MarkerRef {
        name,
        marker_type: MARKER_TYPE_BRANCH.to_string(),
        epoch: php_int_prefix(epoch),
        marker_hash: Some(hash.to_string()),
        commit_hash,
        summary: summary.to_string(),
        message: Some(text.to_string()),
        active: false,
        remote_name,
    }))
}

/// Leading-integer parse like PHP's `(int)` cast.
fn php_int_prefix(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let mut value: i64 = 0;
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value
            .saturating_mul(10)
            .saturating_add((byte - b'0') as i64);
    }
    value.saturating_mul(sign)
}

/// Current branch name via `symbolic-ref --quiet HEAD --`.
fn current_git_branch(dir: &Path) -> Option<String> {
    let out = crate::git::output(dir, &["symbolic-ref", "--quiet", "HEAD", "--"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let trimmed = stdout.trim();
    trimmed.strip_prefix("refs/heads/").map(str::to_string)
}

/// List local git branch markers, marking the checked-out branch active.
///
/// Mirrors `ArcanistGitRepositoryMarkerQuery::newLocalRefMarkers()` with
/// the repository query filters applied: `marker_type` keeps one flavor
/// and `remote_cache` selects remote-tracking markers instead of local
/// ones (defaulting to local, like `withIsRemoteCache(false)`).
pub fn query_git_markers(
    dir: &Path,
    marker_type: &str,
    remote_cache: bool,
) -> Result<Vec<MarkerRef>, String> {
    let current = current_git_branch(dir);
    let text = crate::git::stdout_text(
        dir,
        &[
            "for-each-ref",
            &format!("--format={GIT_REF_FORMAT}"),
            "--",
            "refs/",
        ],
    )?;
    let mut markers = Vec::new();
    for record in text.split('\x02') {
        let Some(marker) = parse_git_ref_record(record)? else {
            continue;
        };
        if marker.marker_type != marker_type {
            continue;
        }
        if marker.is_remote_cache() != remote_cache {
            continue;
        }
        markers.push(marker);
    }
    // PHP compares the bare marker name, so a remote-tracking marker
    // sharing the checked-out branch name also shows as active.
    if let Some(current) = current {
        for marker in &mut markers {
            if marker.name == current {
                marker.active = true;
            }
        }
    }
    Ok(markers)
}

/// One git remote (`ArcanistRemoteRef`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    pub name: String,
    pub fetch_uri: Option<String>,
    pub push_uri: Option<String>,
}

/// Parse one `git remote --verbose` line.
fn parse_remote_line(line: &str) -> Result<(String, String, String), String> {
    // `^(?P<name>[^\t]+)\t(?P<uri>[^\s]+) \((?P<mode>fetch|push)\)\z`
    let (name, rest) = line.split_once('\t').unwrap_or(("", ""));
    let (uri, mode) = rest.rsplit_once(' ').unwrap_or(("", ""));
    let mode = mode
        .strip_prefix('(')
        .and_then(|mode| mode.strip_suffix(')'));
    let valid = !name.is_empty()
        && !name.contains('\t')
        && !uri.is_empty()
        && !uri.chars().any(char::is_whitespace)
        && matches!(mode, Some("fetch" | "push"));
    if !valid {
        return Err(format!(
            "Failed to match remote pattern against line \"{line}\"."
        ));
    }
    Ok((
        name.to_string(),
        uri.to_string(),
        mode.unwrap_or("").to_string(),
    ))
}

/// List git remotes (`ArcanistGitRepositoryRemoteQuery`).
///
/// T1153 RB-D9: empty output throws like PHP (`phutil_split_lines`
/// yields one empty line, which misses the remote pattern), so a
/// remote-less checkout fails instead of listing nothing.
pub fn query_git_remotes(dir: &Path) -> Result<Vec<RemoteRef>, String> {
    let text = crate::git::stdout_text(dir, &["remote", "--verbose"])?;
    let mut map: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in grid::split_lines(&text) {
        let (name, uri, mode) = parse_remote_line(line)?;
        match map.iter_mut().find(|(key, _)| key == &name) {
            Some((_, uris)) => {
                uris.insert(mode, uri);
            }
            None => {
                let mut uris = HashMap::new();
                uris.insert(mode, uri);
                map.push((name, uris));
            }
        }
    }
    Ok(map
        .into_iter()
        .map(|(name, mut uris)| RemoteRef {
            name,
            fetch_uri: uris.remove("fetch"),
            push_uri: uris.remove("push"),
        })
        .collect())
}

/// The bundled `arc-hg` extension (`support/hg/arc-hg.py`), compiled in
/// so Mercurial calls always pass a script path like PHP
/// (`getMercurialExtensionArguments`), even outside a source checkout.
const ARC_HG_BUNDLED: &str = include_str!("../../support/hg/arc-hg.py");

/// FNV-1a over the bundled script: a stable content tag for the
/// materialized copy's file name.
fn arc_hg_content_tag() -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in ARC_HG_BUNDLED.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Materialize the bundled `arc-hg` script under the temp dir,
/// atomically installing it when the bytes differ. Best-effort:
/// `None` leaves the caller to run bare.
fn materialize_bundled_arc_hg() -> Option<PathBuf> {
    let target = std::env::temp_dir().join(format!("arc-hg-{:016x}.py", arc_hg_content_tag()));
    if std::fs::read(&target).is_ok_and(|bytes| bytes == ARC_HG_BUNDLED.as_bytes()) {
        return Some(target);
    }
    let tmp = target.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&tmp, ARC_HG_BUNDLED).is_err() {
        return None;
    }
    if std::fs::rename(&tmp, &target).is_err() {
        std::fs::remove_file(&tmp).ok();
        // A lost install race still converges when the winner matches.
        if std::fs::read(&target).is_ok_and(|bytes| bytes == ARC_HG_BUNDLED.as_bytes()) {
            return Some(target);
        }
        return None;
    }
    Some(target)
}

/// Locate the `arc-hg` extension script: `ARC_HG_EXTENSION` first, then an
/// upward `support/hg/arc-hg.py` search from `dir`, then the bundled
/// copy (T1153 LAND-25: PHP always passes its bundled script path, so
/// the query never runs bare on user configuration alone).
#[must_use]
pub fn arc_hg_script(dir: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ARC_HG_EXTENSION") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(found) = dir
        .ancestors()
        .map(|dir| dir.join("support").join("hg").join("arc-hg.py"))
        .find(|path| path.is_file())
    {
        return Some(found);
    }
    materialize_bundled_arc_hg()
}

/// PHP `empty()` truthiness for `hg arc-ls-markers` flags.
fn php_truthy(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::Number(number)) => number.as_f64().unwrap_or(0.0) != 0.0,
        Some(serde_json::Value::String(text)) => !text.is_empty() && text != "0",
        Some(serde_json::Value::Array(items)) => !items.is_empty(),
        Some(serde_json::Value::Object(map)) => !map.is_empty(),
    }
}

/// Decode `hg arc-ls-markers` JSON, skipping closed heads, like
/// `ArcanistMercurialRepositoryMarkerQuery.php`.
fn parse_hg_ls_markers(text: &str) -> Result<Vec<MarkerRef>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_str(text)
        .map_err(|e| format!("Call to \"hg arc-ls-markers\" returned invalid JSON: {e}"))?;
    let mut out = Vec::new();
    for item in &items {
        if php_truthy(item.get("isClosed")) {
            continue;
        }
        let item_type = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let marker_type = match item_type {
            "branch" => MARKER_TYPE_BRANCH,
            "bookmark" => MARKER_TYPE_BOOKMARK,
            "commit-state" => MARKER_TYPE_COMMIT_STATE,
            "branch-state" => MARKER_TYPE_BRANCH_STATE,
            _ => {
                return Err(format!(
                    "Call to \"hg arc-ls-markers\" returned marker of unknown type \"{item_type}\"."
                ));
            }
        };
        // T1153 LAND-27 / T1153 RB-D34: PHP reads `name`/`node`
        // unchecked (a missing key is a notice, not a failure), so
        // markers without them proceed with empty strings instead of
        // hard-failing.
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let node = item
            .get("node")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        // T1153 RB-D39: without `description` PHP attaches no message
        // and leaves the summary empty.
        let (summary, message) = match item.get("description").and_then(serde_json::Value::as_str) {
            Some(description) => (
                grid::split_lines(description)
                    .first()
                    .unwrap_or(&"")
                    .to_string(),
                Some(description.to_string()),
            ),
            None => (String::new(), None),
        };
        out.push(MarkerRef {
            name: name.to_string(),
            marker_type: marker_type.to_string(),
            epoch: 0,
            marker_hash: None,
            commit_hash: node.to_string(),
            summary,
            message,
            active: php_truthy(item.get("isActive")),
            remote_name: None,
        });
    }
    Ok(out)
}

/// Summarize one `CommandException` section: `(empty)` when blank,
/// else the head plus a `... (N more byte(s)) ...` tail past 1000
/// bytes (skipped when the tail outgrows the cut, like PHP).
fn summarize_command_section(text: &str) -> String {
    if text.is_empty() {
        return "(empty)".to_string();
    }
    let bytes = text.as_bytes();
    if bytes.len() > 1000 {
        let cut = bytes.len() - 1000;
        let suffix = format!("... ({} more byte(s)) ...", grid::group_number(cut as u64));
        if cut > suffix.len() {
            // PHP's `substr` cuts bytes; a split character renders
            // lossy here instead of raw partial bytes.
            return format!("{}{}", String::from_utf8_lossy(&bytes[..1000]), suffix);
        }
    }
    text.to_string()
}

/// Render an exec failure like PHP's `CommandException`
/// (`src/future/exec/CommandException.php:17-34`): the
/// `Command failed with error #N!` head plus `COMMAND`/`STDOUT`/
/// `STDERR` sections. `command` is the pre-rendered command line.
///
/// Residual: PHP censors credentials in the summary
/// (`phutil_censor_credentials`, libphutil-only); this port renders
/// sections verbatim.
#[must_use]
pub fn command_exception_text(code: i32, command: &str, stdout: &str, stderr: &str) -> String {
    [
        format!("Command failed with error #{code}!"),
        "COMMAND".to_string(),
        summarize_command_section(command),
        String::new(),
        "STDOUT".to_string(),
        summarize_command_section(stdout),
        String::new(),
        "STDERR".to_string(),
        summarize_command_section(stderr),
    ]
    .join("\n")
}

/// List Mercurial markers via `hg arc-ls-markers`.
///
/// PHP fails without the extension; this port does the same (there is no
/// native-output fallback).
pub fn query_hg_markers(dir: &Path, marker_type: &str) -> Result<Vec<MarkerRef>, String> {
    query_hg_markers_with(dir, &hg::HgCommand::system(), marker_type)
}

/// List Mercurial markers invoking a custom `hg` program (test seam).
pub fn query_hg_markers_with(
    dir: &Path,
    hg_command: &hg::HgCommand,
    marker_type: &str,
) -> Result<Vec<MarkerRef>, String> {
    if find_hg_root(dir).is_none() {
        return Err("not inside a Mercurial working copy".to_string());
    }
    let script = arc_hg_script(dir);
    let mut args = vec!["arc-ls-markers".to_string(), "--".to_string()];
    if let Some(script) = &script {
        let (flag, value) = hg::extension_config_arg("arc-hg", Some(script))?;
        args.insert(0, value);
        args.insert(0, flag);
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = hg::output_with(dir, hg_command, &argv)?;
    // T1153 LAND-26: PHP's future throws on a nonzero exit, so the
    // failure surfaces as a `CommandException` dump (command plus
    // stdout/stderr sections), never as hg's bare stderr.
    if !output.status.success() {
        let mut display = vec![hg_command.program.clone()];
        display.extend(hg_command.prefix.iter().cloned());
        display.extend(args.iter().cloned());
        return Err(command_exception_text(
            output.status.code().unwrap_or(1),
            &terminal::shell_join(&display),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    let markers = parse_hg_ls_markers(&String::from_utf8_lossy(&output.stdout))?;
    Ok(markers
        .into_iter()
        .filter(|marker| marker.marker_type == marker_type)
        .collect())
}

/// Published commit hashes under git: commit hashes of remote-tracking
/// markers on permanent refs (`newPublishedCommitHashes()`).
///
/// Without a conduit client the load fails like PHP (T1153 LAND-23).
pub fn published_git_hashes(
    dir: &Path,
    client: Option<&crate::conduit::Client>,
) -> Result<Vec<String>, String> {
    let remotes = query_git_remotes(dir)?;
    if remotes.is_empty() {
        return Ok(Vec::new());
    }
    let markers = query_git_markers(dir, MARKER_TYPE_BRANCH, true)?;
    if markers.is_empty() {
        return Ok(Vec::new());
    }
    // T1153 LAND-23: without conduit PHP's hardpoint load fails the
    // whole workflow — the grid never renders unannotated.
    let Some(client) = client else {
        return Err(conduit::no_uri_block());
    };
    // T1153 RB-D30: fetch-then-push per remote, deduped in
    // first-appearance order (`array_fuse`), not sorted.
    let mut uris = Vec::new();
    for remote in &remotes {
        for uri in [&remote.fetch_uri, &remote.push_uri].into_iter().flatten() {
            if !uris.contains(uri) {
                uris.push(uri.clone());
            }
        }
    }
    let repositories = revision::load_repositories_by_uris(client, &uris)?;
    let by_name: HashMap<&str, &RemoteRef> = remotes
        .iter()
        .map(|remote| (remote.name.as_str(), remote))
        .collect();
    let mut hashes = Vec::new();
    for marker in &markers {
        let Some(remote_name) = &marker.remote_name else {
            continue;
        };
        let Some(remote) = by_name.get(remote_name.as_str()) else {
            continue;
        };
        if !is_permanent_git_ref(&repositories, remote, &marker.name) {
            continue;
        }
        hashes.push(marker.commit_hash.clone());
    }
    Ok(hashes)
}

/// True when the remote's push repository marks `ref_name` permanent,
/// matching push URIs through git normalization on both sides.
///
/// T1153 RB-D30: on URI collisions the LAST repository wins, like
/// PHP's `$uri_map` overwrite.
fn is_permanent_git_ref(
    repositories: &[revision::RepositoryRef],
    remote: &RemoteRef,
    ref_name: &str,
) -> bool {
    let Some(push_uri) = &remote.push_uri else {
        return false;
    };
    let push_uri = revision::normalize_git_uri(push_uri);
    let mut winner: Option<&revision::RepositoryRef> = None;
    for repository in repositories {
        for uri in repository.uris() {
            if revision::normalize_git_uri(&uri) == push_uri {
                winner = Some(repository);
                break;
            }
        }
    }
    winner.is_some_and(|repository| repository.is_permanent_ref(ref_name))
}

/// Published commit hashes under Mercurial:
/// `hg log -r 'parents(draft()) - draft()'`.
pub fn published_hg_hashes(dir: &Path) -> Result<Vec<String>, String> {
    published_hg_hashes_with(dir, &hg::HgCommand::system())
}

/// Published Mercurial hashes invoking a custom `hg` program (test seam).
pub fn published_hg_hashes_with(
    dir: &Path,
    hg_command: &hg::HgCommand,
) -> Result<Vec<String>, String> {
    let text = hg::stdout_text_with(
        dir,
        hg_command,
        &[
            "log",
            "--rev",
            "parents(draft()) - draft()",
            "--template",
            "{node}\n",
        ],
    )?;
    let mut hashes = Vec::new();
    for line in grid::split_lines(&text) {
        if line.trim().is_empty() {
            continue;
        }
        hashes.push(line.to_string());
    }
    Ok(hashes)
}

/// One working-copy state: a graph node hash with its commit message.
#[derive(Debug, Clone)]
pub struct StateCommit {
    pub hash: String,
    pub message: String,
}

/// Empty revision-ref lists keyed by state hash.
fn empty_revision_refs(states: &[StateCommit]) -> HashMap<String, Vec<revision::RevisionRef>> {
    let mut refs: HashMap<String, Vec<revision::RevisionRef>> = HashMap::new();
    for state in states {
        refs.insert(state.hash.clone(), Vec::new());
    }
    refs
}

/// Commit-hash pair kind: `gtcm` for git, `hgcm` for Mercurial
/// (T1153 LAND-24: the hg working-copy query sends `hgcm` pairs).
fn hash_pair_kind(vcs: &str) -> &'static str {
    if vcs == "hg" { "hgcm" } else { "gtcm" }
}

/// State commit refs carry no tree hash, so one pair per state applies.
fn state_hash_pairs(states: &[StateCommit], kind: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for state in states {
        pairs.push((kind.to_string(), state.hash.clone()));
    }
    pairs
}

/// Strip a `kind:` pair prefix (`gtcm:`/`hgcm:`) from a loader key.
fn strip_pair_prefix<'a>(key: &'a str, kind: &str) -> Option<&'a str> {
    key.strip_prefix(kind)?.strip_prefix(':')
}

/// Merge hash-query refs into the per-hash lists, deduplicated by
/// PHID like `ArcanistObjectListHardpoint` (T1153 RB-D32).
fn merge_hash_query_refs(
    client: &crate::conduit::Client,
    states: &[StateCommit],
    merged: &mut HashMap<String, Vec<revision::RevisionRef>>,
    kind: &str,
) -> Result<(), String> {
    let pairs = state_hash_pairs(states, kind);
    for (key, reference) in revision::load_by_commit_hashes(client, &pairs)? {
        if let Some(hash) = strip_pair_prefix(&key, kind) {
            push_revision_ref_once(merged, hash, &reference);
        }
    }
    Ok(())
}

/// Map revision ids from commit messages to state hashes, plus the
/// first-seen id order for the conduit query.
fn revision_id_index(states: &[StateCommit]) -> (HashMap<u32, Vec<String>>, Vec<u32>) {
    let mut id_map: HashMap<u32, Vec<String>> = HashMap::new();
    let mut id_order: Vec<u32> = Vec::new();
    for state in states {
        // Usage errors (invalid fields) skip the commit, like the PHP
        // message query's `catch (ArcanistUsageException)`.
        let Ok(id) = revision::revision_id_from_message(&state.message) else {
            continue;
        };
        let Some(id) = id else {
            continue;
        };
        if let Some(hashes) = id_map.get_mut(&id) {
            hashes.push(state.hash.clone());
        } else {
            id_order.push(id);
            id_map.insert(id, vec![state.hash.clone()]);
        }
    }
    (id_map, id_order)
}

/// Push `reference` onto `hash`'s list unless its PHID is already
/// there. The list is keyed exactly like PHP's
/// `mergeHardpointValues` (`$old[$phid]`): a missing PHID keys as
/// `""`, so only the first PHID-less ref is kept (T1153 RB-D32).
fn push_revision_ref_once(
    merged: &mut HashMap<String, Vec<revision::RevisionRef>>,
    hash: &str,
    reference: &revision::RevisionRef,
) {
    let Some(list) = merged.get_mut(hash) else {
        return;
    };
    let key = reference.phid().unwrap_or("");
    for item in list.iter() {
        if item.phid().unwrap_or("") == key {
            return;
        }
    }
    list.push(reference.clone());
}

/// Merge message-query (id) refs into the per-hash lists.
fn merge_id_query_refs(
    client: &crate::conduit::Client,
    ids: &[u32],
    id_map: &HashMap<u32, Vec<String>>,
    merged: &mut HashMap<String, Vec<revision::RevisionRef>>,
) -> Result<(), String> {
    for reference in revision::load_by_ids(client, ids)? {
        let id = reference.id();
        if let Some(hashes) = id_map.get(&id) {
            for hash in hashes {
                push_revision_ref_once(merged, hash, &reference);
            }
        }
    }
    Ok(())
}

/// Load revision refs for every state hash: hash-query refs first, then
/// message-query refs, deduplicated by PHID.
///
/// `states` runs in graph order, which also orders the conduit request
/// parameters like the PHP hardpoint queries. Without a conduit client
/// the load fails like PHP (T1153 LAND-23).
pub fn load_revision_refs(
    client: Option<&crate::conduit::Client>,
    states: &[StateCommit],
) -> Result<HashMap<String, Vec<revision::RevisionRef>>, String> {
    load_revision_refs_for_vcs(client, states, "git")
}

/// Load revision refs with the pair kind for `vcs` (`gtcm` for git,
/// `hgcm` for Mercurial).
pub fn load_revision_refs_for_vcs(
    client: Option<&crate::conduit::Client>,
    states: &[StateCommit],
    vcs: &str,
) -> Result<HashMap<String, Vec<revision::RevisionRef>>, String> {
    let mut merged = empty_revision_refs(states);
    // T1153 LAND-23: without conduit PHP's hardpoint load fails the
    // whole workflow.
    let Some(client) = client else {
        return Err(conduit::no_uri_block());
    };
    merge_hash_query_refs(client, states, &mut merged, hash_pair_kind(vcs))?;
    let (id_map, ids) = revision_id_index(states);
    merge_id_query_refs(client, &ids, &id_map, &mut merged)?;
    Ok(merged)
}

/// True when any marker on `hash` is the active marker.
fn hash_has_active_marker(groups: &HashMap<String, Vec<MarkerRef>>, hash: &str) -> bool {
    let Some(markers) = groups.get(hash) else {
        return false;
    };
    for marker in markers {
        if marker.active {
            return true;
        }
    }
    false
}

/// True when any commit in `set` carries an active marker.
fn set_has_active_marker(
    set: &commitgraph::CommitSet,
    groups: &HashMap<String, Vec<MarkerRef>>,
) -> bool {
    for hash in set.hashes() {
        if hash_has_active_marker(groups, hash) {
            return true;
        }
    }
    false
}

/// Ids of sets holding an active marker.
fn find_active_sets(
    sets: &[commitgraph::CommitSet],
    groups: &HashMap<String, Vec<MarkerRef>>,
) -> HashMap<u64, ()> {
    let mut active: HashMap<u64, ()> = HashMap::new();
    for set in sets {
        if set_has_active_marker(set, groups) {
            active.insert(set.set_id(), ());
        }
    }
    active
}

/// Parent set ids of set `id`.
fn parent_set_ids(sets: &[commitgraph::CommitSet], id: u64) -> Vec<u64> {
    for set in sets {
        if set.set_id() == id {
            return set.parent_sets().to_vec();
        }
    }
    Vec::new()
}

/// Mark ancestors of active sets active too.
fn mark_set_ancestors(sets: &[commitgraph::CommitSet], active: &mut HashMap<u64, ()>) {
    let mut stack: Vec<u64> = Vec::new();
    for set in sets {
        if active.contains_key(&set.set_id()) {
            stack.push(set.set_id());
        }
    }
    while let Some(id) = stack.pop() {
        for parent in parent_set_ids(sets, id) {
            if active.contains_key(&parent) {
                continue;
            }
            active.insert(parent, ());
            stack.push(parent);
        }
    }
}

/// Borrowed view of owned marker names for natural-order comparison.
fn borrowed_names(marker_names: &[String]) -> Vec<&str> {
    let mut borrowed = Vec::new();
    for name in marker_names {
        borrowed.push(name.as_str());
    }
    borrowed
}

/// The (has-markers, max-name) vector parts for one set's marker names,
/// recording the max name for the partition vector.
fn set_marker_parts(marker_names: &[String], partition_names: &mut Vec<String>) -> (i64, String) {
    if marker_names.is_empty() {
        return (0, String::new());
    }
    let borrowed = borrowed_names(marker_names);
    let max = commitgraph::natural_max(&borrowed)
        .unwrap_or("")
        .to_string();
    partition_names.push(max.clone());
    (1, max)
}

/// Sort vector for one set: active flag, newest epoch, marker presence
/// and max name. Folds the set's epoch and max name into the partition
/// accumulators.
fn set_sort_vector(
    graph: &commitgraph::CommitGraph,
    set: &commitgraph::CommitSet,
    groups: &HashMap<String, Vec<MarkerRef>>,
    has_active: bool,
    partition_epoch: &mut i64,
    partition_names: &mut Vec<String>,
) -> commitgraph::SortVector {
    let mut max_epoch = 0;
    let mut marker_names: Vec<String> = Vec::new();
    for hash in set.hashes() {
        if let Some(node) = graph.node(hash) {
            max_epoch = max_epoch.max(node.epoch().unwrap_or(0));
        }
        if let Some(markers) = groups.get(hash) {
            for marker in markers {
                marker_names.push(marker.name.clone());
            }
        }
    }
    *partition_epoch = (*partition_epoch).max(max_epoch);
    let (has_markers, max_name) = set_marker_parts(&marker_names, partition_names);
    let mut vector = commitgraph::SortVector::new();
    vector.add_int(i64::from(has_active));
    vector.add_int(max_epoch);
    vector.add_int(has_markers);
    vector
        .add_string(&max_name)
        .expect("marker names lack NUL bytes");
    vector
}

/// Compare set order entries by vector, ties by original position.
fn set_order_cmp(left: &(String, usize), right: &(String, usize)) -> std::cmp::Ordering {
    commitgraph::natural_cmp(left.0.as_bytes(), right.0.as_bytes(), true).then(left.1.cmp(&right.1))
}

/// Set ids ordered by their sort vectors, byte-compared and stable.
fn order_sets_by_vector(vectors: &[(u64, commitgraph::SortVector)]) -> Vec<u64> {
    let mut order: Vec<(String, usize)> = Vec::new();
    for (index, (_, vector)) in vectors.iter().enumerate() {
        order.push((vector.sort_string(), index));
    }
    order.sort_by(set_order_cmp);
    let mut keys = Vec::new();
    for (_, index) in &order {
        keys.push(vectors[*index].0);
    }
    keys
}

/// Position of set `id`, which vector keys always cover.
fn find_set_position(sets: &[commitgraph::CommitSet], id: u64) -> usize {
    for (position, set) in sets.iter().enumerate() {
        if set.set_id() == id {
            return position;
        }
    }
    panic!("vector keys cover every set");
}

/// Restrict display children to `vector_keys` and reorder the sets.
fn reorder_sets(sets: &mut Vec<commitgraph::CommitSet>, vector_keys: &[u64]) {
    for set in sets.iter_mut() {
        set.reorder_display_children(vector_keys);
    }
    let mut ordered: Vec<commitgraph::CommitSet> = Vec::with_capacity(sets.len());
    for id in vector_keys {
        ordered.push(sets[find_set_position(sets, *id)].clone());
    }
    *sets = ordered;
}

/// Partition sort vector: any-active flag, newest epoch, marker presence
/// and max name.
fn partition_sort_vector(
    any_active: bool,
    partition_epoch: i64,
    partition_names: &[String],
) -> commitgraph::SortVector {
    let mut vector = commitgraph::SortVector::new();
    vector.add_int(i64::from(any_active));
    vector.add_int(partition_epoch);
    if partition_names.is_empty() {
        vector.add_int(0);
        vector.add_string("").expect("marker names lack NUL bytes");
    } else {
        let borrowed = borrowed_names(partition_names);
        let max = commitgraph::natural_max(&borrowed)
            .unwrap_or("")
            .to_string();
        vector.add_int(1);
        vector
            .add_string(&max)
            .expect("marker names lack NUL bytes");
    }
    vector
}

/// Sort a partition's sets and restrict their display children, returning
/// the ordered sets plus the partition sort vector.
///
/// Mirrors `ArcanistMarkersWorkflow::sortSets()`: active markers and
/// their ancestors sort first, then by newest commit epoch, then by
/// marker presence and name.
pub fn sort_sets(
    graph: &commitgraph::CommitGraph,
    sets: &mut Vec<commitgraph::CommitSet>,
    groups: &HashMap<String, Vec<MarkerRef>>,
) -> commitgraph::SortVector {
    let mut active = find_active_sets(sets, groups);
    mark_set_ancestors(sets, &mut active);
    let mut vectors: Vec<(u64, commitgraph::SortVector)> = Vec::new();
    let mut partition_epoch = 0;
    let mut partition_names: Vec<String> = Vec::new();
    for set in sets.iter() {
        let has_active = active.contains_key(&set.set_id());
        let vector = set_sort_vector(
            graph,
            set,
            groups,
            has_active,
            &mut partition_epoch,
            &mut partition_names,
        );
        vectors.push((set.set_id(), vector));
    }
    let vector_keys = order_sets_by_vector(&vectors);
    reorder_sets(sets, &vector_keys);
    partition_sort_vector(!active.is_empty(), partition_epoch, &partition_names)
}

/// One set tree view (`ArcanistCommitGraphSetView`): hierarchy, grouping,
/// and depth information for rendering.
#[derive(Debug, Clone)]
struct SetView {
    set_id: u64,
    parent: Option<u64>,
    children: Vec<u64>,
    group: u64,
    members: Vec<u64>,
    depth: usize,
}

/// Build preorder set views for a root set over display children.
fn build_set_views(sets: &HashMap<u64, &commitgraph::CommitSet>, root: u64) -> Vec<SetView> {
    let mut views = Vec::new();
    let mut order = Vec::new();
    // Preorder matching the PHP recursion.
    fn visit(sets: &HashMap<u64, &commitgraph::CommitSet>, id: u64, order: &mut Vec<u64>) {
        order.push(id);
        if let Some(set) = sets.get(&id) {
            for child in set.display_child_sets() {
                visit(sets, *child, order);
            }
        }
    }
    visit(sets, root, &mut order);
    let mut parents: HashMap<u64, Option<u64>> = HashMap::new();
    parents.insert(root, None);
    for id in &order {
        if let Some(set) = sets.get(id) {
            for child in set.display_child_sets() {
                parents.insert(*child, Some(*id));
            }
        }
    }
    let children_of = |id: u64| -> Vec<u64> {
        sets.get(&id)
            .map(|set| set.display_child_sets().to_vec())
            .unwrap_or_default()
    };
    for id in order {
        views.push(SetView {
            set_id: id,
            parent: parents.get(&id).copied().flatten(),
            children: children_of(id),
            group: id,
            members: Vec::new(),
            depth: 0,
        });
    }
    views
}

/// Position of the view for set `id`, which always resolves.
fn view_index(views: &[SetView], id: u64) -> usize {
    for (index, view) in views.iter().enumerate() {
        if view.set_id == id {
            return index;
        }
    }
    panic!("view ids resolve");
}

/// Group for one view (`getGroupForView()`): revised, marked, and
/// childless views group alone, otherwise a view joins its parent's
/// group when it is the only child.
fn group_for_view(
    views: &[SetView],
    position: usize,
    revisions: &HashMap<u64, bool>,
    marked: &HashMap<u64, bool>,
) -> u64 {
    let id = views[position].set_id;
    if revisions.get(&id).copied().unwrap_or(false)
        || marked.get(&id).copied().unwrap_or(false)
        || views[position].children.is_empty()
    {
        return id;
    }
    let Some(parent) = views[position].parent else {
        return id;
    };
    let siblings = views[view_index(views, parent)].children.len();
    if siblings != 1 {
        id
    } else {
        views[view_index(views, parent)].group
    }
}

/// Assign each view to a group. Views run preorder, so parents group
/// before their children.
fn assign_view_groups(
    views: &mut [SetView],
    revisions: &HashMap<u64, bool>,
    marked: &HashMap<u64, bool>,
) {
    for position in 0..views.len() {
        let grouped = group_for_view(views, position, revisions, marked);
        views[position].group = grouped;
    }
}

/// Collect each view id into its group's member list.
fn collect_view_members(views: &mut [SetView]) {
    for position in 0..views.len() {
        let group = views[position].group;
        let member = views[position].set_id;
        let target = view_index(views, group);
        views[target].members.push(member);
    }
}

/// Break groups of 2-3 members apart into singletons.
fn split_small_groups(views: &mut [SetView]) {
    for position in 0..views.len() {
        let count = views[position].members.len();
        if count > 1 && count < 4 {
            let members = views[position].members.clone();
            for member in members {
                let target = view_index(views, member);
                views[target].group = member;
                views[target].members = vec![member];
            }
        }
    }
}

/// Depth of one view from its parent's group holder.
fn view_depth(views: &[SetView], position: usize) -> usize {
    let Some(parent) = views[position].parent else {
        return 0;
    };
    let group = views[view_index(views, parent)].group;
    let holder = view_index(views, group);
    let base = views[holder].depth;
    if views[holder].members.len() > 1 {
        base + 2
    } else {
        base + 1
    }
}

/// Assign depths from parent groups.
fn assign_view_depths(views: &mut [SetView]) {
    for position in 0..views.len() {
        let depth = view_depth(views, position);
        views[position].depth = depth;
    }
}

/// Assign each view to a group (`getGroupForView()`).
///
/// `revisions` maps a set id to whether the set's head commit carries an
/// unpublished revision; `marked` maps a set id to whether any commit in
/// the set carries a marker.
fn group_views(views: &mut [SetView], revisions: &HashMap<u64, bool>, marked: &HashMap<u64, bool>) {
    assign_view_groups(views, revisions, marked);
    collect_view_members(views);
    split_small_groups(views);
    assign_view_depths(views);
}

/// One laid-out row item: a commit, a revision, a marker, a collapse
/// placeholder, or a group trailer.
#[derive(Debug, Clone, Default)]
struct Item {
    commit: Option<usize>,
    revision: Option<usize>,
    marker: Option<MarkerRef>,
    collapse: Option<usize>,
    group: bool,
}

/// Merge key: the lowercased first 32 bytes of a summary.
fn merge_key(text: &str) -> String {
    let prefix: String = text.bytes().take(32).map(|byte| byte as char).collect();
    grid::utf8_lower(&prefix)
}

/// Revision indexes keyed by merge key: lowercase first 32 summary bytes.
fn revision_merge_strings(revisions: &[revision::RevisionRef]) -> HashMap<String, Vec<usize>> {
    let mut merge_strings: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, reference) in revisions.iter().enumerate() {
        merge_strings
            .entry(merge_key(reference.name().unwrap_or("")))
            .or_default()
            .push(index);
    }
    merge_strings
}

/// Revision index merging with one commit message: the single revision,
/// or the unique revision sharing the commit summary's merge key.
fn commit_merge_target(
    revisions: &[revision::RevisionRef],
    merge_strings: &HashMap<String, Vec<usize>>,
    message: &str,
) -> Option<usize> {
    if revisions.len() == 1 {
        return Some(0);
    }
    let key = merge_key(&commitgraph::commit_summary(message));
    match merge_strings.get(&key) {
        Some(refs) if refs.len() == 1 => Some(refs[0]),
        _ => None,
    }
}

/// Commit indexes keyed by merged revision PHID.
fn commit_merge_map(
    commits: &[(String, String)],
    revisions: &[revision::RevisionRef],
    merge_strings: &HashMap<String, Vec<usize>>,
) -> HashMap<String, Vec<usize>> {
    let mut merge_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (commit_index, (_, message)) in commits.iter().enumerate() {
        let Some(index) = commit_merge_target(revisions, merge_strings, message) else {
            continue;
        };
        let Some(phid) = revisions[index].phid() else {
            continue;
        };
        merge_map
            .entry(phid.to_string())
            .or_default()
            .push(commit_index);
    }
    merge_map
}

/// Index of the revision carrying `phid`.
fn find_revision_by_phid(revisions: &[revision::RevisionRef], phid: &str) -> Option<usize> {
    for (index, reference) in revisions.iter().enumerate() {
        if reference.phid() == Some(phid) {
            return Some(index);
        }
    }
    None
}

/// `result_map` (commit hash to revision, only for 1:1 merges) plus the
/// merged PHIDs.
fn paired_revision_indexes(
    commits: &[(String, String)],
    revisions: &[revision::RevisionRef],
    merge_map: &HashMap<String, Vec<usize>>,
) -> (HashMap<String, usize>, HashMap<String, ()>) {
    let mut result_map: HashMap<String, usize> = HashMap::new();
    let mut merged_phids: HashMap<String, ()> = HashMap::new();
    for (phid, commit_list) in merge_map {
        if commit_list.len() != 1 {
            continue;
        }
        let Some(index) = find_revision_by_phid(revisions, phid) else {
            continue;
        };
        result_map.insert(commits[commit_list[0]].0.clone(), index);
        merged_phids.insert(phid.clone(), ());
    }
    (result_map, merged_phids)
}

/// One item per unmerged revision.
fn revision_items(
    revisions: &[revision::RevisionRef],
    merged_phids: &HashMap<String, ()>,
) -> Vec<Item> {
    let mut items = Vec::new();
    for (index, reference) in revisions.iter().enumerate() {
        let phid = reference.phid().unwrap_or("").to_string();
        if merged_phids.contains_key(&phid) {
            continue;
        }
        items.push(Item {
            revision: Some(index),
            ..Item::default()
        });
    }
    items
}

/// One item per commit, carrying its paired revision if any.
fn commit_items(commits: &[(String, String)], result_map: &HashMap<String, usize>) -> Vec<Item> {
    let mut items = Vec::new();
    for (commit_index, (hash, _)) in commits.iter().enumerate() {
        items.push(Item {
            commit: Some(commit_index),
            revision: result_map.get(hash).copied(),
            ..Item::default()
        });
    }
    items
}

/// Non-empty marker list for an item's commit, if any.
fn commit_marker_list(
    item: &Item,
    commits: &[(String, String)],
    markers: &HashMap<String, Vec<MarkerRef>>,
) -> Option<Vec<MarkerRef>> {
    let index = item.commit?;
    let found = markers.get(&commits[index].0)?;
    if found.is_empty() {
        return None;
    }
    Some(found.clone())
}

/// Attach markers: the head marker joins the commit item, extras get
/// their own marker-only items.
fn attach_markers(
    items: Vec<Item>,
    commits: &[(String, String)],
    markers: &HashMap<String, Vec<MarkerRef>>,
) -> Vec<Item> {
    let mut with_markers = Vec::new();
    for mut item in items {
        if let Some(mut list) = commit_marker_list(&item, commits, markers) {
            item.marker = Some(list.remove(0));
            with_markers.push(item);
            for marker in list {
                with_markers.push(Item {
                    marker: Some(marker),
                    ..Item::default()
                });
            }
        } else {
            with_markers.push(item);
        }
    }
    with_markers
}

/// Lay out a view's commits, revisions, and markers into items.
fn build_items(
    commits: &[(String, String)],
    revisions: &[revision::RevisionRef],
    markers: &HashMap<String, Vec<MarkerRef>>,
) -> Vec<Item> {
    let merge_strings = revision_merge_strings(revisions);
    let merge_map = commit_merge_map(commits, revisions, &merge_strings);
    let (result_map, merged_phids) = paired_revision_indexes(commits, revisions, &merge_map);
    let mut items = revision_items(revisions, &merged_phids);
    items.extend(commit_items(commits, &result_map));
    collapse_items(attach_markers(items, commits, markers))
}

/// True for bare commit items, the only collapsible kind.
fn is_collapsible_item(item: &Item) -> bool {
    item.commit.is_some()
        && item.revision.is_none()
        && item.marker.is_none()
        && !item.group
        && item.collapse.is_none()
}

/// Partition flags into runs of equal values, keys preserved.
fn collapsible_runs(collapsible: &[bool]) -> Vec<(bool, Vec<usize>)> {
    let mut runs: Vec<(bool, Vec<usize>)> = Vec::new();
    for (index, value) in collapsible.iter().enumerate() {
        let extend = match runs.last() {
            Some((head, _)) => head == value,
            None => false,
        };
        if extend {
            if let Some((_, run)) = runs.last_mut() {
                run.push(index);
            }
        } else {
            runs.push((*value, vec![index]));
        }
    }
    runs
}

/// Context items kept on each side of a collapsed run.
const COLLAPSE_CONTEXT: usize = 3;

/// Collapse one long run: the first middle item becomes a placeholder
/// carrying the full run count, the rest of the middle is dropped.
fn collapse_run(items: &mut Vec<Item>, run: &[usize]) {
    let middle = &run[COLLAPSE_CONTEXT..run.len() - COLLAPSE_CONTEXT];
    let mut first = true;
    let mut drop = Vec::new();
    for key in middle {
        if first {
            items[*key].collapse = Some(run.len());
            first = false;
        } else {
            drop.push(*key);
        }
    }
    for key in drop.into_iter().rev() {
        items.remove(key);
    }
}

/// Collapse long leading runs of bare commits, keeping 3 of context on
/// each side (`collapseItems()`).
///
/// PHP breaks out of the partition loop at the first non-collapsible
/// run, so only a leading collapsible run ever collapses; the
/// placeholder carries the full run count, not the hidden count.
fn collapse_items(mut items: Vec<Item>) -> Vec<Item> {
    const MIN_RUN: usize = COLLAPSE_CONTEXT * 2 + 3;
    let mut collapsible = Vec::new();
    for item in &items {
        collapsible.push(is_collapsible_item(item));
    }
    for (value, run) in &collapsible_runs(&collapsible) {
        if !value {
            break;
        }
        if run.len() < MIN_RUN {
            continue;
        }
        collapse_run(&mut items, run);
    }
    items
}

// Tree marks, as exact codepoints (`drawMarkerCell()` byte escapes).
const MARK_BULLET: &str = "\u{2022}";
const MARK_NE: &str = "\u{2517}";
const MARK_EW: &str = "\u{2501}";
const MARK_ESW: &str = "\u{2533}";
const MARK_SW: &str = "\u{2513}";
const MARK_NS_LIGHT: &str = "\u{2502}";
const MARK_NE_LIGHT: &str = "\u{2514}";
const MARK_ESW_LIGHT: &str = "\u{252F}";

/// View data needed to draw one grid row.
struct RowData<'a> {
    commits: &'a [(String, String)],
    revisions: &'a [revision::RevisionRef],
    items: Vec<Item>,
    members: usize,
    depth: usize,
    has_children: bool,
    terminal: Option<usize>,
    ansi: bool,
}

/// Active-marker label: `tsprintf('<bg:green>**%s**</bg> **%s**', ' * ',
/// $name)`, which strips to ` *  name` without a TTY.
fn active_marker_label(name: &str, ansi: bool) -> String {
    terminal::tsprintf_ansi(
        "<bg:green>**%s**</bg> **%s**",
        &[TVal::Text(" * "), TVal::Text(name)],
        ansi,
    )
    .map(|styled| styled.to_string())
    .unwrap_or_default()
}

/// Inactive-marker label: `tsprintf('**%s**', $name)`.
fn inactive_marker_label(name: &str, ansi: bool) -> String {
    terminal::tsprintf_ansi("**%s**", &[TVal::Text(name)], ansi)
        .map(|styled| styled.to_string())
        .unwrap_or_default()
}

/// Revision status label: `drawRevisionStatus()` wraps the short (under
/// 120 columns) or full display name in `<fg:color>` when the server
/// sent a truthy ANSI color.
fn revision_status_label(reference: &revision::RevisionRef, narrow: bool, ansi: bool) -> String {
    let status = if narrow {
        reference.status_short_name()
    } else {
        reference.status_display_name()
    }
    .unwrap_or("");
    let color = reference.status_ansi_color().unwrap_or("");
    if color.is_empty() || color == "0" {
        return status.to_string();
    }
    terminal::tsprintf_ansi(&format!("<fg:{color}>%s</fg>"), &[TVal::Text(status)], ansi)
        .map(|styled| styled.to_string())
        .unwrap_or_else(|_| status.to_string())
}

/// Commit label for the first row: one short hash, or a first..last
/// range for multi-commit views.
fn commit_range_label(commits: &[(String, String)]) -> String {
    if commits.len() == 1 {
        return short_hash(&commits[0].0).to_string();
    }
    if commits.is_empty() {
        return String::new();
    }
    let first = commits
        .first()
        .map(|commit| commit.0.as_str())
        .unwrap_or("");
    let last = commits.last().map(|commit| commit.0.as_str()).unwrap_or("");
    format!("{}..{}", short_hash(first), short_hash(last))
}

/// Display depth (capped by terminal width) and available label width.
fn marker_cell_geometry(depth: usize, terminal: Option<usize>) -> (usize, usize) {
    let width = terminal.unwrap_or(0);
    let max_depth = 3 + width.saturating_sub(72) / 6;
    let display_depth = if depth <= max_depth {
        depth * 2
    } else {
        max_depth * 2
    };
    let max_width = max_depth * 2 + 16;
    (display_depth, max_width.saturating_sub(display_depth))
}

/// Label for one marker-cell row: the marker name (active markers get a
/// ` *  ` prefix), the group trailer, the commit label on the first row,
/// or empty.
fn marker_item_label(
    item: &Item,
    available: usize,
    is_first: bool,
    commit_label: &str,
    members: usize,
    ansi: bool,
) -> String {
    if let Some(marker) = &item.marker {
        let width = if marker.active {
            available.saturating_sub(4)
        } else {
            available
        };
        let name = grid::truncate_glyphs(&marker.name, width);
        if marker.active {
            active_marker_label(&name, ansi)
        } else {
            inactive_marker_label(&name, ansi)
        }
    } else if item.group {
        format!(
            "(... {} more revisions ...)",
            grid::group_number(members.saturating_sub(1) as u64)
        )
    } else if is_first {
        commit_label.to_string()
    } else {
        String::new()
    }
}

/// Tree path for the first row of a marker cell.
fn first_row_path(display_depth: usize, has_children: bool, is_last: bool) -> String {
    if display_depth == 0 {
        format!("{MARK_BULLET} ")
    } else if has_children {
        format!("{MARK_NE}{MARK_EW}{MARK_ESW} ")
    } else if !is_last {
        format!("{MARK_NE}{MARK_EW}{MARK_ESW_LIGHT} ")
    } else {
        format!("{MARK_NE}{MARK_EW}{MARK_EW} ")
    }
}

/// Tree path for one marker-cell row.
fn marker_item_path(
    item: &Item,
    is_first: bool,
    is_last: bool,
    display_depth: usize,
    has_children: bool,
) -> String {
    if is_first {
        return first_row_path(display_depth, has_children, is_last);
    }
    if item.group {
        return format!("{MARK_NE}/{MARK_SW} ");
    }
    let mut path = if is_last && !has_children {
        format!("{MARK_NE_LIGHT} ")
    } else {
        format!("{MARK_NS_LIGHT} ")
    };
    if display_depth > 0 {
        path = format!("  {path}");
    }
    path
}

/// Draw the marker cell: tree path plus marker names, commit labels, and
/// the group trailer.
fn draw_marker_cell(data: &RowData<'_>) -> Vec<String> {
    let mut items = data.items.clone();
    if data.members > 1 {
        items.push(Item {
            group: true,
            ..Item::default()
        });
    }
    let commit_label = commit_range_label(data.commits);
    let (display_depth, available) = marker_cell_geometry(data.depth, data.terminal);
    let last_key = items.len().saturating_sub(1);
    let mut cell = Vec::new();
    for (key, item) in items.iter().enumerate() {
        let is_first = key == 0;
        let is_last = key == last_key;
        let label = marker_item_label(
            item,
            available,
            is_first,
            &commit_label,
            data.members,
            data.ansi,
        );
        let indent = if display_depth > 2 {
            " ".repeat(display_depth - 2)
        } else {
            String::new()
        };
        let path = marker_item_path(item, is_first, is_last, display_depth, data.has_children);
        cell.push(format!("{indent}{path}{label}\n"));
    }
    cell
}

fn draw_commits_cell(data: &RowData<'_>) -> Vec<String> {
    data.items
        .iter()
        .map(|item| {
            if item.collapse.is_some() {
                "   :   \n".to_string()
            } else if let Some(index) = item.commit {
                format!("{}\n", short_hash(&data.commits[index].0))
            } else {
                "\n".to_string()
            }
        })
        .collect()
}

fn draw_revisions_cell(data: &RowData<'_>) -> Vec<String> {
    data.items
        .iter()
        .map(|item| {
            if let Some(index) = item.revision {
                format!("{}\n", data.revisions[index].monogram())
            } else {
                "\n".to_string()
            }
        })
        .collect()
}

fn draw_status_cell(data: &RowData<'_>) -> Vec<String> {
    let narrow = data.terminal.unwrap_or(0) < 120;
    data.items
        .iter()
        .map(|item| {
            if let Some(index) = item.revision {
                let label = revision_status_label(&data.revisions[index], narrow, data.ansi);
                format!("{label}\n")
            } else {
                "\n".to_string()
            }
        })
        .collect()
}

fn draw_messages_cell(data: &RowData<'_>) -> Vec<String> {
    data.items
        .iter()
        .map(|item| {
            if let Some(count) = item.collapse {
                return format!(
                    "<... {} more commits ...>\n",
                    grid::group_number(count as u64)
                );
            }
            if let Some(index) = item.revision {
                return format!("{}\n", data.revisions[index].name().unwrap_or(""));
            }
            if let Some(index) = item.commit {
                return format!("{}\n", commitgraph::commit_summary(&data.commits[index].1));
            }
            "\n".to_string()
        })
        .collect()
}

/// Draw one grid row for a set view.
#[allow(clippy::too_many_arguments)]
fn draw_view_row(
    commits: &[(String, String)],
    revisions: &[revision::RevisionRef],
    items: Vec<Item>,
    members: usize,
    depth: usize,
    has_children: bool,
    terminal: Option<usize>,
    ansi: bool,
) -> Vec<grid::GridCell> {
    let data = RowData {
        commits,
        revisions,
        items,
        members,
        depth,
        has_children,
        terminal,
        ansi,
    };
    vec![
        grid::GridCell::new("marker", draw_marker_cell(&data)),
        grid::GridCell::new("commits", draw_commits_cell(&data)),
        grid::GridCell::new("status", draw_status_cell(&data)),
        grid::GridCell::new("revisions", draw_revisions_cell(&data)),
        grid::GridCell::new("messages", draw_messages_cell(&data)),
    ]
}

/// Render one partition: one row list per root set.
fn render_partition(
    sets: &[commitgraph::CommitSet],
    states: &HashMap<String, String>,
    revisions: &HashMap<String, Vec<revision::RevisionRef>>,
    groups: &HashMap<String, Vec<MarkerRef>>,
    terminal: Option<usize>,
    ansi: bool,
) -> Result<Vec<Vec<grid::GridCell>>, String> {
    let by_id: HashMap<u64, &commitgraph::CommitSet> =
        sets.iter().map(|set| (set.set_id(), set)).collect();
    let mut rows = Vec::new();
    for set in sets {
        if !set.parent_sets().is_empty() {
            continue;
        }
        let mut views = build_set_views(&by_id, set.set_id());
        // Per-view commit, revision, and marker refs.
        let mut view_commits: HashMap<u64, Vec<(String, String)>> = HashMap::new();
        let mut view_revisions: HashMap<u64, Vec<revision::RevisionRef>> = HashMap::new();
        for view in &views {
            let set = by_id.get(&view.set_id).expect("views cover known sets");
            let mut commits = Vec::new();
            for hash in set.hashes() {
                let Some(message) = states.get(hash) else {
                    return Err(format!("Found no state ref for hash \"{hash}\"."));
                };
                commits.push((hash.clone(), message.clone()));
            }
            let head = set.hashes().first().cloned().unwrap_or_default();
            view_revisions.insert(
                view.set_id,
                revisions.get(&head).cloned().unwrap_or_default(),
            );
            view_commits.insert(view.set_id, commits);
        }
        let unpublished: HashMap<u64, bool> = view_revisions
            .iter()
            .map(|(id, refs)| {
                (
                    *id,
                    refs.iter()
                        .any(|reference| !reference.is_status_published()),
                )
            })
            .collect();
        let marked: HashMap<u64, bool> = view_commits
            .iter()
            .map(|(id, commits)| {
                (
                    *id,
                    commits
                        .iter()
                        .any(|(hash, _)| groups.get(hash).is_some_and(|list| !list.is_empty())),
                )
            })
            .collect();
        group_views(&mut views, &unpublished, &marked);
        for view in &views {
            if view.members.is_empty() {
                continue;
            }
            let commits = view_commits.get(&view.set_id).cloned().unwrap_or_default();
            let refs = view_revisions
                .get(&view.set_id)
                .cloned()
                .unwrap_or_default();
            let items = build_items(&commits, &refs, groups);
            rows.push(draw_view_row(
                &commits,
                &refs,
                items,
                view.members.len(),
                view.depth,
                !view.children.is_empty(),
                terminal,
                ansi,
            ));
        }
    }
    Ok(rows)
}

/// Run the markers workflow: graph, partitions, sets, tree views, grid.
///
/// `vcs` is `git` or `hg`; `marker_type` selects branches or bookmarks;
/// `include_remote` (the `--all` extension) also heads remote-tracking
/// markers. Without a conduit client the workflow fails like PHP
/// (T1153 LAND-23).
pub fn run_workflow(
    dir: &Path,
    vcs: &str,
    marker_type: &str,
    include_remote: bool,
    client: Option<&crate::conduit::Client>,
    terminal: Option<usize>,
) -> Result<String, String> {
    run_workflow_ansi(
        dir,
        vcs,
        marker_type,
        include_remote,
        client,
        terminal,
        terminal::ansi_enabled(),
    )
}

/// Run the markers workflow with an explicit ANSI setting
/// (deterministic under test).
pub fn run_workflow_ansi(
    dir: &Path,
    vcs: &str,
    marker_type: &str,
    include_remote: bool,
    client: Option<&crate::conduit::Client>,
    terminal: Option<usize>,
    ansi: bool,
) -> Result<String, String> {
    run_workflow_with_ansi(
        dir,
        &hg::HgCommand::system(),
        vcs,
        marker_type,
        include_remote,
        client,
        terminal,
        ansi,
    )
}

/// Workflow markers: local markers of `marker_type`, plus
/// remote-tracking branches when `include_remote` applies.
fn query_workflow_markers(
    dir: &Path,
    hg_command: &hg::HgCommand,
    vcs: &str,
    marker_type: &str,
    include_remote: bool,
) -> Result<Vec<MarkerRef>, String> {
    let mut markers = match vcs {
        "git" => query_git_markers(dir, marker_type, false)?,
        "hg" => query_hg_markers_with(dir, hg_command, marker_type)?,
        other => return Err(support_error(other, marker_type)),
    };
    if include_remote && vcs == "git" && marker_type == MARKER_TYPE_BRANCH {
        markers.extend(query_git_markers(dir, marker_type, true)?);
    }
    Ok(markers)
}

/// Published-commit tails bounding the unpublished walk.
fn query_workflow_tails(
    dir: &Path,
    hg_command: &hg::HgCommand,
    vcs: &str,
    client: Option<&crate::conduit::Client>,
) -> Result<Vec<String>, String> {
    match vcs {
        "git" => published_git_hashes(dir, client),
        _ => published_hg_hashes_with(dir, hg_command),
    }
}

/// Walk unpublished commits from `heads` down to `tails`, enforcing the
/// unpublished-commit guard.
fn run_head_query(
    graph: &mut commitgraph::CommitGraph,
    dir: &Path,
    hg_command: &hg::HgCommand,
    vcs: &str,
    heads: &[String],
    tails: &[String],
) -> Result<Vec<String>, String> {
    let tails_option = if tails.is_empty() { None } else { Some(tails) };
    let limited = commitgraph::QueryBounds::limit(UNPUBLISHED_LIMIT + 1);
    let seen = match vcs {
        "git" => commitgraph::run_git_query(graph, dir, Some(heads), tails_option, None, &limited)?,
        _ => commitgraph::run_hg_query(
            graph,
            dir,
            hg_command,
            Some(heads),
            tails_option,
            None,
            &limited,
        )?,
    };
    check_limit(seen.len())?;
    Ok(seen)
}

/// Load marker heads missing from the walk exactly: markers pointing at
/// already-published commits never surface in the walk, and must not
/// vanish from the output.
fn load_disjoint_heads(
    graph: &mut commitgraph::CommitGraph,
    dir: &Path,
    hg_command: &hg::HgCommand,
    vcs: &str,
    heads: &[String],
    nodes: &mut Vec<String>,
) -> Result<(), String> {
    let mut disjoint: Vec<String> = Vec::new();
    for head in heads {
        if !nodes.contains(head) {
            disjoint.push(head.clone());
        }
    }
    if disjoint.is_empty() {
        return Ok(());
    }
    let unbounded = commitgraph::QueryBounds::unbounded();
    let extra = match vcs {
        "git" => commitgraph::run_git_query(graph, dir, None, None, Some(&disjoint), &unbounded)?,
        _ => commitgraph::run_hg_query(
            graph,
            dir,
            hg_command,
            None,
            None,
            Some(&disjoint),
            &unbounded,
        )?,
    };
    for hash in extra {
        if !nodes.contains(&hash) {
            nodes.push(hash);
        }
    }
    Ok(())
}

/// Working-copy states for the walked nodes, as a map and in graph order.
fn collect_workflow_states(
    graph: &commitgraph::CommitGraph,
    nodes: &[String],
) -> (HashMap<String, String>, Vec<StateCommit>) {
    let mut states: HashMap<String, String> = HashMap::new();
    let mut ordered_states = Vec::new();
    for hash in nodes {
        let mut message = "";
        if let Some(node) = graph.node(hash) {
            message = node.message().unwrap_or("");
        }
        states.insert(hash.clone(), message.to_string());
        ordered_states.push(StateCommit {
            hash: hash.clone(),
            message: message.to_string(),
        });
    }
    (states, ordered_states)
}

/// Markers grouped by the commit hash they point at.
fn group_markers_by_hash(markers: &[MarkerRef]) -> HashMap<String, Vec<MarkerRef>> {
    let mut groups: HashMap<String, Vec<MarkerRef>> = HashMap::new();
    for marker in markers {
        groups
            .entry(marker.commit_hash.clone())
            .or_default()
            .push(marker.clone());
    }
    groups
}

/// Deduplicated revision ids per state hash for the set query.
fn revision_waypoint_map(
    revisions: &HashMap<String, Vec<revision::RevisionRef>>,
) -> HashMap<String, Vec<u32>> {
    let mut waypoint_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (hash, refs) in revisions {
        let mut ids = Vec::new();
        for reference in refs {
            if !ids.contains(&reference.id()) {
                ids.push(reference.id());
            }
        }
        waypoint_map.insert(hash.clone(), ids);
    }
    waypoint_map
}

/// Sorted sets and sort vectors for every partition.
fn build_partition_sets(
    graph: &commitgraph::CommitGraph,
    partitions: &[commitgraph::Partition],
    revisions: &HashMap<String, Vec<revision::RevisionRef>>,
    groups: &HashMap<String, Vec<MarkerRef>>,
) -> Result<
    (
        Vec<Vec<commitgraph::CommitSet>>,
        Vec<commitgraph::SortVector>,
    ),
    String,
> {
    let waypoint_map = revision_waypoint_map(revisions);
    let mut partition_sets = Vec::new();
    let mut partition_vectors = Vec::new();
    let mut set_counter = 1;
    for partition in partitions {
        let mut sets = commitgraph::set_query(graph, partition, &waypoint_map, &mut set_counter)?;
        let vector = sort_sets(graph, &mut sets, groups);
        partition_sets.push(sets);
        partition_vectors.push(vector);
    }
    Ok((partition_sets, partition_vectors))
}

/// Compare partition order entries by vector bytes, ties by position.
fn partition_order_cmp(left: &(String, usize), right: &(String, usize)) -> std::cmp::Ordering {
    left.0
        .as_bytes()
        .cmp(right.0.as_bytes())
        .then(left.1.cmp(&right.1))
}

/// Partition indexes ordered by their vectors, byte-compared and stable.
fn order_partitions(vectors: &[commitgraph::SortVector]) -> Vec<usize> {
    let mut order: Vec<(String, usize)> = Vec::new();
    for (index, vector) in vectors.iter().enumerate() {
        order.push((vector.sort_string(), index));
    }
    order.sort_by(partition_order_cmp);
    let mut keys = Vec::new();
    for (_, index) in &order {
        keys.push(*index);
    }
    keys
}

/// Render every partition's rows into the markers grid.
#[allow(clippy::too_many_arguments)]
fn render_marker_grid(
    partitions: &[commitgraph::Partition],
    partition_sets: &[Vec<commitgraph::CommitSet>],
    order: &[usize],
    states: &HashMap<String, String>,
    revisions: &HashMap<String, Vec<revision::RevisionRef>>,
    groups: &HashMap<String, Vec<MarkerRef>>,
    terminal: Option<usize>,
    ansi: bool,
) -> Result<String, String> {
    let mut grid = grid::GridView::new();
    grid.new_column("marker");
    grid.new_column("commits");
    grid.new_column("status");
    grid.new_column("revisions");
    grid.new_column("messages").set_minimum_width(12);
    for index in order {
        partitions.get(*index).expect("partition order covers all");
        for row in render_partition(
            &partition_sets[*index],
            states,
            revisions,
            groups,
            terminal,
            ansi,
        )? {
            grid.new_row(row);
        }
    }
    Ok(grid.draw(terminal))
}

/// Run the markers workflow invoking a custom `hg` program (test seam).
pub fn run_workflow_with(
    dir: &Path,
    hg_command: &hg::HgCommand,
    vcs: &str,
    marker_type: &str,
    include_remote: bool,
    client: Option<&crate::conduit::Client>,
    terminal: Option<usize>,
) -> Result<String, String> {
    run_workflow_with_ansi(
        dir,
        hg_command,
        vcs,
        marker_type,
        include_remote,
        client,
        terminal,
        terminal::ansi_enabled(),
    )
}

/// Run the markers workflow with a custom `hg` program and an explicit
/// ANSI setting (deterministic under test).
#[allow(clippy::too_many_arguments)]
pub fn run_workflow_with_ansi(
    dir: &Path,
    hg_command: &hg::HgCommand,
    vcs: &str,
    marker_type: &str,
    include_remote: bool,
    client: Option<&crate::conduit::Client>,
    terminal: Option<usize>,
    ansi: bool,
) -> Result<String, String> {
    let markers = query_workflow_markers(dir, hg_command, vcs, marker_type, include_remote)?;
    let tails = query_workflow_tails(dir, hg_command, vcs, client)?;
    let mut heads: Vec<String> = Vec::new();
    for marker in &markers {
        heads.push(marker.commit_hash.clone());
    }
    let mut graph = commitgraph::CommitGraph::new();
    let mut nodes = run_head_query(&mut graph, dir, hg_command, vcs, &heads, &tails)?;
    load_disjoint_heads(&mut graph, dir, hg_command, vcs, &heads, &mut nodes)?;
    let (states, ordered_states) = collect_workflow_states(&graph, &nodes);
    let revisions = load_revision_refs_for_vcs(client, &ordered_states, vcs)?;
    let partitions = commitgraph::partition_query(&graph, &heads, Some(&nodes))?;
    let groups = group_markers_by_hash(&markers);
    let (partition_sets, partition_vectors) =
        build_partition_sets(&graph, &partitions, &revisions, &groups)?;
    let order = order_partitions(&partition_vectors);
    render_marker_grid(
        &partitions,
        &partition_sets,
        &order,
        &states,
        &revisions,
        &groups,
        terminal,
        ansi,
    )
}

/// List local git branches as plain markers (the `--json` shape).
pub fn list_git_branches(dir: &Path) -> Result<Vec<Marker>, String> {
    if !crate::git::is_inside_work_tree(dir) {
        return Err("not inside a git repository".to_string());
    }
    let refs = query_git_markers(dir, MARKER_TYPE_BRANCH, false)?;
    let mut markers: Vec<Marker> = refs
        .iter()
        .map(|marker| Marker {
            name: marker.name.clone(),
            marker_type: marker.marker_type.clone(),
            active: marker.active,
            hash: marker.commit_hash.clone(),
            epoch: marker.epoch.max(0) as u64,
            subject: marker.summary.clone(),
        })
        .collect();
    sort_markers(&mut markers);
    Ok(markers)
}

/// List markers of `marker_type` under `dir`, dispatching on the VCS.
pub fn list(dir: &Path, marker_type: &str) -> Result<Vec<Marker>, String> {
    list_with(dir, marker_type, &hg::HgCommand::system())
}

/// List markers invoking a custom `hg` program (test seam).
pub fn list_with(
    dir: &Path,
    marker_type: &str,
    hg_command: &hg::HgCommand,
) -> Result<Vec<Marker>, String> {
    let resolved = resolve_type(dir, Some(marker_type))?;
    if resolved == MARKER_TYPE_BOOKMARK {
        let refs = query_hg_markers_with(dir, hg_command, &resolved)?;
        let mut markers: Vec<Marker> = refs
            .iter()
            .map(|marker| Marker {
                name: marker.name.clone(),
                marker_type: marker.marker_type.clone(),
                active: marker.active,
                hash: marker.commit_hash.clone(),
                epoch: marker.epoch.max(0) as u64,
                subject: marker.summary.clone(),
            })
            .collect();
        sort_markers(&mut markers);
        return Ok(markers);
    }
    if crate::git::is_inside_work_tree(dir) {
        list_git_branches(dir)
    } else {
        let refs = query_hg_markers_with(dir, hg_command, &resolved)?;
        let mut markers: Vec<Marker> = refs
            .iter()
            .map(|marker| Marker {
                name: marker.name.clone(),
                marker_type: marker.marker_type.clone(),
                active: marker.active,
                hash: marker.commit_hash.clone(),
                epoch: marker.epoch.max(0) as u64,
                subject: marker.summary.clone(),
            })
            .collect();
        sort_markers(&mut markers);
        Ok(markers)
    }
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

/// Detect the working-copy flavor: `git`, `hg`, or `svn`.
fn detect_vcs(dir: &Path) -> Option<&'static str> {
    if crate::git::is_inside_work_tree(dir) {
        Some("git")
    } else if find_hg_root(dir).is_some() {
        Some("hg")
    } else if find_svn_root(dir).is_some() {
        Some("svn")
    } else {
        None
    }
}

/// True for the marker errors PHP raises as
/// `PhutilArgumentUsageException` (exit 1 under the `USAGE EXCEPTION`
/// label): outside a working copy (which fails the marker-type
/// support gate), unsupported VCS/type pairs
/// (`ArcanistBranchesWorkflow.php:31-38`), and the 1000-unpublished
/// guard (`ArcanistMarkersWorkflow.php:37-43`). Query and render
/// failures escape into a fatal. Shared with `branches`, which runs
/// the same workflow.
#[must_use]
pub fn is_toolset_usage_message(message: &str) -> bool {
    message.starts_with("not inside a working copy")
        || message == "not inside a git repository"
        || (message.starts_with("The version control system (\"")
            && message.contains("does not support "))
        || (message.starts_with("Found more than ")
            && message.ends_with(" unpublished commits which are ancestors of heads."))
        || message.starts_with("unknown marker type '")
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
/// Reject working copies that cannot list `marker_type`, returning the VCS.
///
/// Shared by the non-JSON tails of [`execute`] (ahead of conduit
/// setup, per PHP's `getRepositoryAPI` order) and `execute_core`.
fn check_vcs_support(dir: &Path, marker_type: &str) -> Result<&'static str, String> {
    let vcs = detect_vcs(dir).ok_or_else(|| {
        "not inside a working copy (no git repository or Mercurial '.hg' directory found)"
            .to_string()
    })?;
    if !supports_marker_type(vcs, marker_type) {
        return Err(support_error(vcs, marker_type));
    }
    Ok(vcs)
}

/// Resolve the marker type and run the working-copy/support gate.
fn check_markers_support(dir: &Path, requested: Option<&str>) -> Result<String, String> {
    let marker_type = resolve_type(dir, requested)?;
    check_vcs_support(dir, &marker_type)?;
    Ok(marker_type)
}

pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        // T1153 RB-D4 (R4): PHP renders usage errors with no dump.
        Err(message) => return Err(message),
    };
    if options.json {
        // The `--json` extension never calls conduit; it stays offline.
        return execute_core(
            dir,
            &options,
            grid::terminal_width(),
            None,
            terminal::ansi_enabled(),
        );
    }
    // T1153 LAND-28: the working-copy/support gate runs before
    // conduit setup — PHP's getRepositoryAPI (and its usage error)
    // precedes the marker query.
    check_markers_support(dir, options.marker_type.as_deref())?;
    // T1153 LAND-23: the engine needs conduit (tokenless when only
    // credentials are missing — calls then fail mapped); without a
    // URI the setup block replaces the grid.
    let owned_client = match config::client_for_tokenless_with(
        options.conduit_uri.as_deref(),
        dir,
        config::arcrc_path().as_deref(),
        &get_config::LoadOptions::default(),
    ) {
        Ok(client) => client,
        Err(message) => return Err(conduit::toolset_setup_message(&message)),
    };
    execute_core(
        dir,
        &options,
        grid::terminal_width(),
        Some(&owned_client),
        terminal::ansi_enabled(),
    )
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
        // T1153 RB-D4 (R4): PHP renders usage errors with no dump.
        Err(message) => return Err(message),
    };
    execute_core(dir, &options, terminal, client, terminal::ansi_enabled())
}

fn execute_core(
    dir: &Path,
    options: &Options,
    terminal: Option<usize>,
    client: Option<&crate::conduit::Client>,
    ansi: bool,
) -> Result<String, String> {
    let marker_type = resolve_type(dir, options.marker_type.as_deref())?;
    if options.json {
        let markers = list(dir, &marker_type)?;
        let mut text = serde_json::to_string_pretty(&format_json(&markers))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        return Ok(text);
    }
    let vcs = check_vcs_support(dir, &marker_type)?;
    run_workflow_ansi(dir, vcs, &marker_type, false, client, terminal, ansi)
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 CORE-5: auth-coded query failures render login
            // guidance (the toolset runtime's auth arm). The calls
            // went out on a strict client, re-resolved here for the
            // mapping context (config reads only, no I/O).
            let client = parse_options(argv)
                .ok()
                .flatten()
                .and_then(|options| config::client_for(options.conduit_uri.as_deref()).ok());
            if let Some(client) = client
                && let Some(code) = conduit::auth_report_for(&client, &message)
            {
                return code;
            }
            // T1153 RB-D4 (R4): `markers`/`branches` run in the
            // toolset runtime (`ArcanistMarkersWorkflow extends
            // ArcanistArcWorkflow`). Argument, support-gate, and
            // limit errors render the red `USAGE EXCEPTION` label
            // (exit 1); query and render failures escape into a
            // fatal (255).
            let kind = if parse_options(argv).is_err() || is_toolset_usage_message(&message) {
                EnvelopeKind::ToolsetUsage
            } else {
                EnvelopeKind::Fatal
            };
            conduit::report(kind, &message)
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
