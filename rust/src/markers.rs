//! `arc-rs markers`: list branch/bookmark markers with commit context.
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
//! Divergences:
//! - No ANSI styling: the active marker shows as ` *  name` and revision
//!   statuses render as plain text instead of the TTY-only bold and color
//!   labels (`ArcanistCommitGraphSetView.php:336-345,509-524`).
//! - Conduit is opportunistic: without credentials the revision and
//!   published-ref annotation is empty and the grid still renders. PHP
//!   fails the whole workflow when conduit calls fail.
//! - Empty `git remote --verbose` output yields no remotes. PHP throws
//!   `Failed to match remote pattern against line "".` here, but the
//!   workflow's own `!$remotes` guard shows an empty list was intended.
//! - `regexp(...)` permanent-ref rules use the matcher's supported subset
//!   (see `revision`); exact-name rules behave identically.
//! - A `refs/remotes/` ref without a `/` (which git never emits) is
//!   skipped instead of producing a null-named marker.
//! - `hg arc-ls-markers` resolves the `arc-hg` extension script from
//!   `ARC_HG_EXTENSION` or an upward `support/hg/arc-hg.py` search, else
//!   runs bare and relies on the user's Mercurial configuration. PHP
//!   always passes its bundled script path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{commitgraph, config, grid, hg, revision};

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
        "usage: arc-rs {NAME} [--type branch|bookmark] [--json] [--conduit-uri URI]\n\
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

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut marker_type = None;
    let mut json = false;
    let mut conduit_uri = None;
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
            return Err("unexpected argument '--': markers takes no paths".to_string());
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(Some(Options {
        marker_type,
        json,
        conduit_uri,
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
pub fn query_git_remotes(dir: &Path) -> Result<Vec<RemoteRef>, String> {
    let text = crate::git::stdout_text(dir, &["remote", "--verbose"])?;
    if text.trim().is_empty() {
        // PHP throws on the empty line here; an empty list was intended.
        return Ok(Vec::new());
    }
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

/// Locate the `arc-hg` extension script: `ARC_HG_EXTENSION` first, then an
/// upward `support/hg/arc-hg.py` search from `dir`.
#[must_use]
pub fn arc_hg_script(dir: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ARC_HG_EXTENSION") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    dir.ancestors()
        .map(|dir| dir.join("support").join("hg").join("arc-hg.py"))
        .find(|path| path.is_file())
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
        let description = item
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let summary = grid::split_lines(description)
            .first()
            .unwrap_or(&"")
            .to_string();
        out.push(MarkerRef {
            name: name.to_string(),
            marker_type: marker_type.to_string(),
            epoch: 0,
            marker_hash: None,
            commit_hash: node.to_string(),
            summary,
            message: Some(description.to_string()),
            active: php_truthy(item.get("isActive")),
            remote_name: None,
        });
    }
    Ok(out)
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
    if !output.status.success() {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = detail.trim();
        if detail.is_empty() {
            return Err("hg arc-ls-markers failed".to_string());
        }
        return Err(detail.to_string());
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
/// Without a conduit client no repository ref can resolve, so no hash is
/// permanent and the result is empty.
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
    let Some(client) = client else {
        return Ok(Vec::new());
    };
    let mut uris = Vec::new();
    for remote in &remotes {
        if let Some(uri) = &remote.fetch_uri {
            uris.push(uri.clone());
        }
        if let Some(uri) = &remote.push_uri {
            uris.push(uri.clone());
        }
    }
    uris.sort();
    uris.dedup();
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
fn is_permanent_git_ref(
    repositories: &[revision::RepositoryRef],
    remote: &RemoteRef,
    ref_name: &str,
) -> bool {
    let Some(push_uri) = &remote.push_uri else {
        return false;
    };
    let push_uri = revision::normalize_git_uri(push_uri);
    for repository in repositories {
        let mut matches = false;
        for uri in repository.uris() {
            if revision::normalize_git_uri(&uri) == push_uri {
                matches = true;
                break;
            }
        }
        if matches {
            return repository.is_permanent_ref(ref_name);
        }
    }
    false
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

/// Load revision refs for every state hash: hash-query refs first, then
/// message-query refs, deduplicated by PHID.
///
/// `states` runs in graph order, which also orders the conduit request
/// parameters like the PHP hardpoint queries. Without a conduit client
/// every hash maps to an empty list.
pub fn load_revision_refs(
    client: Option<&crate::conduit::Client>,
    states: &[StateCommit],
) -> Result<HashMap<String, Vec<revision::RevisionRef>>, String> {
    let mut refs: HashMap<String, Vec<revision::RevisionRef>> = HashMap::new();
    for state in states {
        refs.insert(state.hash.clone(), Vec::new());
    }
    let Some(client) = client else {
        return Ok(refs);
    };
    // State commit refs carry no tree hash, so only `gtcm` pairs apply.
    let pairs: Vec<(String, String)> = states
        .iter()
        .map(|state| ("gtcm".to_string(), state.hash.clone()))
        .collect();
    let mut merged = refs;
    for (key, reference) in revision::load_by_commit_hashes(client, &pairs)? {
        if let Some(hash) = key.strip_prefix("gtcm:")
            && let Some(list) = merged.get_mut(hash)
        {
            list.push(reference);
        }
    }
    let mut id_map: HashMap<u32, Vec<String>> = HashMap::new();
    let mut id_order: Vec<u32> = Vec::new();
    for state in states {
        // Usage errors (invalid fields) skip the commit, like the PHP
        // message query's `catch (ArcanistUsageException)`.
        let Ok(id) = revision::revision_id_from_message(&state.message) else {
            continue;
        };
        if let Some(id) = id {
            id_map
                .entry(id)
                .or_insert_with(|| {
                    id_order.push(id);
                    Vec::new()
                })
                .push(state.hash.clone());
        }
    }
    let ids = id_order;
    for reference in revision::load_by_ids(client, &ids)? {
        let id = reference.id();
        if let Some(hashes) = id_map.get(&id) {
            for hash in hashes {
                if let Some(list) = merged.get_mut(hash) {
                    let duplicate = reference.phid().is_some()
                        && list.iter().any(|item| item.phid() == reference.phid());
                    if !duplicate {
                        list.push(reference.clone());
                    }
                }
            }
        }
    }
    Ok(merged)
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
    let mut active: HashMap<u64, ()> = HashMap::new();
    for set in sets.iter() {
        let has_active = set.hashes().iter().any(|hash| {
            groups
                .get(hash)
                .is_some_and(|markers| markers.iter().any(|marker| marker.active))
        });
        if has_active {
            active.insert(set.set_id(), ());
        }
    }
    let mut stack: Vec<u64> = sets
        .iter()
        .filter(|set| active.contains_key(&set.set_id()))
        .map(|set| set.set_id())
        .collect();
    let parents_of = |id: u64| -> Vec<u64> {
        sets.iter()
            .find(|set| set.set_id() == id)
            .map(|set| set.parent_sets().to_vec())
            .unwrap_or_default()
    };
    while let Some(id) = stack.pop() {
        for parent in parents_of(id) {
            if active.contains_key(&parent) {
                continue;
            }
            active.insert(parent, ());
            stack.push(parent);
        }
    }
    let mut vectors: Vec<(u64, commitgraph::SortVector)> = Vec::new();
    let mut partition_epoch = 0;
    let mut partition_names: Vec<String> = Vec::new();
    for set in sets.iter() {
        let has_active = active.contains_key(&set.set_id());
        let mut max_epoch = 0;
        let mut marker_names: Vec<String> = Vec::new();
        for hash in set.hashes() {
            if let Some(node) = graph.node(hash) {
                max_epoch = max_epoch.max(node.epoch().unwrap_or(0));
            }
            if let Some(markers) = groups.get(hash) {
                marker_names.extend(markers.iter().map(|marker| marker.name.clone()));
            }
        }
        partition_epoch = partition_epoch.max(max_epoch);
        let (has_markers, max_name) = if marker_names.is_empty() {
            (0, String::new())
        } else {
            let borrowed: Vec<&str> = marker_names.iter().map(String::as_str).collect();
            let max = commitgraph::natural_max(&borrowed)
                .unwrap_or("")
                .to_string();
            partition_names.push(max.clone());
            (1, max)
        };
        let mut vector = commitgraph::SortVector::new();
        vector.add_int(i64::from(has_active));
        vector.add_int(max_epoch);
        vector.add_int(has_markers);
        vector
            .add_string(&max_name)
            .expect("marker names lack NUL bytes");
        vectors.push((set.set_id(), vector));
    }
    let mut order: Vec<(String, usize)> = vectors
        .iter()
        .enumerate()
        .map(|(index, (_, vector))| (vector.sort_string(), index))
        .collect();
    order.sort_by(|left, right| {
        commitgraph::natural_cmp(left.0.as_bytes(), right.0.as_bytes(), true)
            .then(left.1.cmp(&right.1))
    });
    let vector_keys: Vec<u64> = order.iter().map(|(_, index)| vectors[*index].0).collect();
    for set in sets.iter_mut() {
        set.reorder_display_children(&vector_keys);
    }
    let mut ordered: Vec<commitgraph::CommitSet> = Vec::with_capacity(sets.len());
    for id in &vector_keys {
        let position = sets
            .iter()
            .position(|set| set.set_id() == *id)
            .expect("vector keys cover every set");
        ordered.push(sets[position].clone());
    }
    *sets = ordered;
    let any_active = !active.is_empty();
    let (has_markers, partition_name) = if partition_names.is_empty() {
        (0, String::new())
    } else {
        let borrowed: Vec<&str> = partition_names.iter().map(String::as_str).collect();
        (
            1,
            commitgraph::natural_max(&borrowed)
                .unwrap_or("")
                .to_string(),
        )
    };
    let mut vector = commitgraph::SortVector::new();
    vector.add_int(i64::from(any_active));
    vector.add_int(partition_epoch);
    vector.add_int(has_markers);
    vector
        .add_string(&partition_name)
        .expect("marker names lack NUL bytes");
    vector
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

/// Assign each view to a group (`getGroupForView()`).
///
/// `revisions` maps a set id to whether the set's head commit carries an
/// unpublished revision; `marked` maps a set id to whether any commit in
/// the set carries a marker.
fn group_views(views: &mut [SetView], revisions: &HashMap<u64, bool>, marked: &HashMap<u64, bool>) {
    let index_of = |views: &[SetView], id: u64| -> usize {
        views
            .iter()
            .position(|view| view.set_id == id)
            .expect("view ids resolve")
    };
    // Views run preorder, so parents group before their children.
    for position in 0..views.len() {
        let id = views[position].set_id;
        let grouped = if revisions.get(&id).copied().unwrap_or(false)
            || marked.get(&id).copied().unwrap_or(false)
            || views[position].children.is_empty()
        {
            id
        } else {
            match views[position].parent {
                None => id,
                Some(parent) => {
                    let siblings = views[index_of(views, parent)].children.len();
                    if siblings != 1 {
                        id
                    } else {
                        views[index_of(views, parent)].group
                    }
                }
            }
        };
        views[position].group = grouped;
    }
    for position in 0..views.len() {
        let group = views[position].group;
        let member = views[position].set_id;
        let target = index_of(views, group);
        views[target].members.push(member);
    }
    // Break small groups apart.
    for position in 0..views.len() {
        let count = views[position].members.len();
        if count > 1 && count < 4 {
            let members = views[position].members.clone();
            for member in members {
                let target = index_of(views, member);
                views[target].group = member;
                views[target].members = vec![member];
            }
        }
    }
    // Assign depths from parent groups.
    for position in 0..views.len() {
        let depth = match views[position].parent {
            None => 0,
            Some(parent) => {
                let group = views[index_of(views, parent)].group;
                let holder = index_of(views, group);
                let base = views[holder].depth;
                if views[holder].members.len() > 1 {
                    base + 2
                } else {
                    base + 1
                }
            }
        };
        views[position].depth = depth;
    }
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

/// Lay out a view's commits, revisions, and markers into items.
fn build_items(
    commits: &[(String, String)],
    revisions: &[revision::RevisionRef],
    markers: &HashMap<String, Vec<MarkerRef>>,
) -> Vec<Item> {
    let mut merge_strings: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, reference) in revisions.iter().enumerate() {
        merge_strings
            .entry(merge_key(reference.name().unwrap_or("")))
            .or_default()
            .push(index);
    }
    let mut merge_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (commit_index, (_, message)) in commits.iter().enumerate() {
        let merge_with = if revisions.len() == 1 {
            Some(0)
        } else {
            let key = merge_key(&commitgraph::commit_summary(message));
            match merge_strings.get(&key) {
                Some(refs) if refs.len() == 1 => Some(refs[0]),
                _ => None,
            }
        };
        if let Some(index) = merge_with
            && let Some(phid) = revisions[index].phid()
        {
            merge_map
                .entry(phid.to_string())
                .or_default()
                .push(commit_index);
        }
    }
    // `result_map`: commit hash to revision, only for 1:1 merges.
    let mut result_map: HashMap<String, usize> = HashMap::new();
    let mut merged_phids: HashMap<String, ()> = HashMap::new();
    for (phid, commit_list) in &merge_map {
        if commit_list.len() != 1 {
            continue;
        }
        let position = revisions
            .iter()
            .position(|reference| reference.phid() == Some(phid.as_str()));
        if let Some(index) = position {
            result_map.insert(commits[commit_list[0]].0.clone(), index);
            merged_phids.insert(phid.clone(), ());
        }
    }
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
    for (commit_index, (hash, _)) in commits.iter().enumerate() {
        items.push(Item {
            commit: Some(commit_index),
            revision: result_map.get(hash).copied(),
            ..Item::default()
        });
    }
    // Attach markers: the head marker joins the commit item, extras get
    // their own marker-only items.
    let mut with_markers = Vec::new();
    for mut item in items {
        let hash = item.commit.map(|index| commits[index].0.clone());
        let marker_list = hash.as_ref().and_then(|hash| markers.get(hash));
        match marker_list {
            Some(list) if !list.is_empty() => {
                let mut list = list.clone();
                item.marker = Some(list.remove(0));
                with_markers.push(item);
                for marker in list {
                    with_markers.push(Item {
                        marker: Some(marker),
                        ..Item::default()
                    });
                }
            }
            _ => with_markers.push(item),
        }
    }
    collapse_items(with_markers)
}

/// Collapse long leading runs of bare commits, keeping 3 of context on
/// each side (`collapseItems()`).
///
/// PHP breaks out of the partition loop at the first non-collapsible
/// run, so only a leading collapsible run ever collapses; the
/// placeholder carries the full run count, not the hidden count.
fn collapse_items(mut items: Vec<Item>) -> Vec<Item> {
    const SHOW_CONTEXT: usize = 3;
    let collapsible: Vec<bool> = items
        .iter()
        .map(|item| {
            item.commit.is_some()
                && item.revision.is_none()
                && item.marker.is_none()
                && !item.group
                && item.collapse.is_none()
        })
        .collect();
    // Partition into runs of equal values, keys preserved.
    let mut runs: Vec<(bool, Vec<usize>)> = Vec::new();
    for (index, value) in collapsible.iter().enumerate() {
        match runs.last_mut() {
            Some((head, run)) if head == value => run.push(index),
            _ => runs.push((*value, vec![index])),
        }
    }
    for (value, run) in &runs {
        if !value {
            break;
        }
        if run.len() < SHOW_CONTEXT * 2 + 3 {
            continue;
        }
        let middle = &run[SHOW_CONTEXT..run.len() - SHOW_CONTEXT];
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
    let commit_label = if data.commits.len() == 1 {
        short_hash(&data.commits[0].0).to_string()
    } else if data.commits.is_empty() {
        String::new()
    } else {
        format!(
            "{}..{}",
            short_hash(
                data.commits
                    .first()
                    .map(|commit| commit.0.as_str())
                    .unwrap_or("")
            ),
            short_hash(
                data.commits
                    .last()
                    .map(|commit| commit.0.as_str())
                    .unwrap_or("")
            )
        )
    };
    let width = data.terminal.unwrap_or(0);
    let max_depth = 3 + width.saturating_sub(72) / 6;
    let (display_depth, _squished) = if data.depth <= max_depth {
        (data.depth * 2, false)
    } else {
        (max_depth * 2, true)
    };
    let max_width = max_depth * 2 + 16;
    let available = max_width.saturating_sub(display_depth);
    let last_key = items.len().saturating_sub(1);
    let mut cell = Vec::new();
    for (key, item) in items.iter().enumerate() {
        let is_first = key == 0;
        let is_last = key == last_key;
        let label = if let Some(marker) = &item.marker {
            let width = if marker.active {
                available.saturating_sub(4)
            } else {
                available
            };
            let name = grid::truncate_glyphs(&marker.name, width);
            if marker.active {
                format!(" *  {name}")
            } else {
                name
            }
        } else if item.group {
            format!(
                "(... {} more revisions ...)",
                grid::group_number(data.members.saturating_sub(1) as u64)
            )
        } else if is_first {
            commit_label.clone()
        } else {
            String::new()
        };
        let indent = if display_depth > 2 {
            " ".repeat(display_depth - 2)
        } else {
            String::new()
        };
        let path = if is_first {
            if display_depth == 0 {
                format!("{MARK_BULLET} ")
            } else if data.has_children {
                format!("{MARK_NE}{MARK_EW}{MARK_ESW} ")
            } else if !is_last {
                format!("{MARK_NE}{MARK_EW}{MARK_ESW_LIGHT} ")
            } else {
                format!("{MARK_NE}{MARK_EW}{MARK_EW} ")
            }
        } else if item.group {
            format!("{MARK_NE}/{MARK_SW} ")
        } else {
            let mut path = if is_last && !data.has_children {
                format!("{MARK_NE_LIGHT} ")
            } else {
                format!("{MARK_NS_LIGHT} ")
            };
            if display_depth > 0 {
                path = format!("  {path}");
            }
            path
        };
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
                let reference = &data.revisions[index];
                let status = if narrow {
                    reference.status_short_name()
                } else {
                    reference.status_display_name()
                };
                format!("{}\n", status.unwrap_or(""))
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
fn draw_view_row(
    commits: &[(String, String)],
    revisions: &[revision::RevisionRef],
    items: Vec<Item>,
    members: usize,
    depth: usize,
    has_children: bool,
    terminal: Option<usize>,
) -> Vec<grid::GridCell> {
    let data = RowData {
        commits,
        revisions,
        items,
        members,
        depth,
        has_children,
        terminal,
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
            ));
        }
    }
    Ok(rows)
}

/// Run the markers workflow: graph, partitions, sets, tree views, grid.
///
/// `vcs` is `git` or `hg`; `marker_type` selects branches or bookmarks;
/// `include_remote` (the `--all` extension) also heads remote-tracking
/// markers. Without a conduit client, revision annotation stays empty.
pub fn run_workflow(
    dir: &Path,
    vcs: &str,
    marker_type: &str,
    include_remote: bool,
    client: Option<&crate::conduit::Client>,
    terminal: Option<usize>,
) -> Result<String, String> {
    run_workflow_with(
        dir,
        &hg::HgCommand::system(),
        vcs,
        marker_type,
        include_remote,
        client,
        terminal,
    )
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
    let mut markers = match vcs {
        "git" => query_git_markers(dir, marker_type, false)?,
        "hg" => query_hg_markers_with(dir, hg_command, marker_type)?,
        other => return Err(support_error(other, marker_type)),
    };
    if include_remote && vcs == "git" && marker_type == MARKER_TYPE_BRANCH {
        markers.extend(query_git_markers(dir, marker_type, true)?);
    }
    let tails = match vcs {
        "git" => published_git_hashes(dir, client)?,
        _ => published_hg_hashes_with(dir, hg_command)?,
    };
    let heads: Vec<String> = markers
        .iter()
        .map(|marker| marker.commit_hash.clone())
        .collect();
    let tails_option = if tails.is_empty() {
        None
    } else {
        Some(tails.as_slice())
    };
    let mut graph = commitgraph::CommitGraph::new();
    let limited = commitgraph::QueryBounds::limit(UNPUBLISHED_LIMIT + 1);
    let seen = match vcs {
        "git" => {
            commitgraph::run_git_query(&mut graph, dir, Some(&heads), tails_option, None, &limited)?
        }
        _ => commitgraph::run_hg_query(
            &mut graph,
            dir,
            hg_command,
            Some(&heads),
            tails_option,
            None,
            &limited,
        )?,
    };
    check_limit(seen.len())?;
    let mut nodes = seen;
    // Markers pointing at already-published commits never surface in the
    // walk; load them exactly so they don't vanish from the output.
    let disjoint: Vec<String> = heads
        .iter()
        .filter(|head| !nodes.contains(head))
        .cloned()
        .collect();
    if !disjoint.is_empty() {
        let unbounded = commitgraph::QueryBounds::unbounded();
        let extra = match vcs {
            "git" => commitgraph::run_git_query(
                &mut graph,
                dir,
                None,
                None,
                Some(&disjoint),
                &unbounded,
            )?,
            _ => commitgraph::run_hg_query(
                &mut graph,
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
    }
    let mut states: HashMap<String, String> = HashMap::new();
    let mut ordered_states = Vec::new();
    for hash in &nodes {
        let message = graph
            .node(hash)
            .and_then(|node| node.message())
            .unwrap_or("")
            .to_string();
        states.insert(hash.clone(), message.clone());
        ordered_states.push(StateCommit {
            hash: hash.clone(),
            message,
        });
    }
    let revisions = load_revision_refs(client, &ordered_states)?;
    let partitions = commitgraph::partition_query(&graph, &heads, Some(&nodes))?;
    let mut groups: HashMap<String, Vec<MarkerRef>> = HashMap::new();
    for marker in &markers {
        groups
            .entry(marker.commit_hash.clone())
            .or_default()
            .push(marker.clone());
    }
    let mut partition_sets = Vec::new();
    let mut partition_vectors = Vec::new();
    let mut set_counter = 1;
    for partition in &partitions {
        let mut waypoint_map: HashMap<String, Vec<u32>> = HashMap::new();
        for (hash, refs) in &revisions {
            let mut ids = Vec::new();
            for reference in refs {
                if !ids.contains(&reference.id()) {
                    ids.push(reference.id());
                }
            }
            waypoint_map.insert(hash.clone(), ids);
        }
        let mut sets = commitgraph::set_query(&graph, partition, &waypoint_map, &mut set_counter)?;
        let vector = sort_sets(&graph, &mut sets, &groups);
        partition_sets.push(sets);
        partition_vectors.push(vector);
    }
    // Order partitions by their vectors, byte-compared and stable.
    let mut order: Vec<(String, usize)> = partition_vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| (vector.sort_string(), index))
        .collect();
    order.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then(left.1.cmp(&right.1))
    });
    let mut grid = grid::GridView::new();
    grid.new_column("marker");
    grid.new_column("commits");
    grid.new_column("status");
    grid.new_column("revisions");
    grid.new_column("messages").set_minimum_width(12);
    for (_, index) in &order {
        partitions.get(*index).expect("partition order covers all");
        for row in render_partition(
            &partition_sets[*index],
            &states,
            &revisions,
            &groups,
            terminal,
        )? {
            grid.new_row(row);
        }
    }
    Ok(grid.draw(terminal))
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
    let marker_type = resolve_type(dir, options.marker_type.as_deref())?;
    if options.json {
        let markers = list(dir, &marker_type)?;
        let mut text = serde_json::to_string_pretty(&format_json(&markers))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        return Ok(text);
    }
    let vcs = detect_vcs(dir).ok_or_else(|| {
        "not inside a working copy (no git repository or Mercurial '.hg' directory found)"
            .to_string()
    })?;
    if !supports_marker_type(vcs, &marker_type) {
        return Err(support_error(vcs, &marker_type));
    }
    run_workflow(dir, vcs, &marker_type, false, client, terminal)
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
