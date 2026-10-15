//! `arc-rs browse`: open files and objects in a web browser.
//!
//! PHP references: `src/browse/workflow/ArcanistBrowseWorkflow.php` and
//! `src/browse/query/ArcanistBrowse{Commit,CommitURI,ObjectNameURI,PathURI,
//! RevisionURI}HardpointQuery.php`. Each positional target is resolved to
//! URIs of four kinds (commit, object, path, revision); the URIs are
//! listed or opened in a browser (see [`crate::browser`]).
//!
//! Statuses render through the log engine (`writeWarn` yellow,
//! `writeInfo` blue); the pick table has bold titles and escaped values;
//! the pick prompt is wrapped like `phutil_console_prompt`.
//!
//! Deviations: prompts read plain lines (no readline); end of input during
//! the pick prompt aborts instead of looping forever like PHP; drive
//! letters in absolute paths split into `:lines` exactly like PHP; the
//! `<default>` token display is decided per ref while PHP reuses the last
//! warning-loop token; statuses use the log-engine pattern while PHP's
//! `writeWarn`/`writeInfo` nest bold and background in the other order
//! (identical once markup is stripped).

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{
    browser, config, diff_submit, get_config, git,
    terminal::{self, PhtArg, TVal},
    which,
};

pub const NAME: &str = "browse";

/// Browse kinds, mirroring the `BROWSETYPE_*` constants.
pub const BROWSE_COMMIT: &str = "commit";
pub const BROWSE_OBJECT: &str = "object";
pub const BROWSE_PATH: &str = "path";
pub const BROWSE_REVISION: &str = "revision";

/// Abort text for end of input during the pick prompt.
pub const ABORT: &str = "User aborted the workflow.";

const USAGE: &str = "usage: arc-rs browse [--branch BRANCH] [--types TYPES] [--force] [--] [TARGET ...]\n\
\n\
Open a file, commit, object, or revision in a web browser. With no\n\
target, opens the current working copy. TYPES is a comma- or\n\
space-separated list of commit, object, path, and revision.\n\
\n\
Options:\n\
  --branch BRANCH  browse paths on BRANCH instead of the default branch\n\
  --types TYPES    only resolve these browse kinds\n\
  --force          deprecated alias for --types path\n\
  -h, --help       Show this help.\n";

/// Parsed command line.
pub struct BrowseOptions {
    pub branch: Option<String>,
    pub types: Vec<String>,
    pub force: bool,
    pub targets: Vec<String>,
}

enum Parsed {
    Help,
    Options(BrowseOptions),
    Error(String),
}

fn take_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, Parsed> {
    match iter.next() {
        Some(value) => Ok(value.clone()),
        None => Err(Parsed::Error(format!("{flag} requires a value"))),
    }
}

fn parse_args(argv: &[String]) -> Parsed {
    let mut branch = None;
    let mut types = Vec::new();
    let mut force = false;
    let mut targets = Vec::new();
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if !end_of_flags && arg == "--" {
            end_of_flags = true;
        } else if !end_of_flags && arg == "--branch" {
            match take_value(&mut iter, "--branch") {
                Ok(value) => branch = Some(value),
                Err(outcome) => return outcome,
            }
        } else if !end_of_flags && arg.starts_with("--branch=") {
            branch = Some(arg["--branch=".len()..].to_string());
        } else if !end_of_flags && arg == "--types" {
            match take_value(&mut iter, "--types") {
                Ok(value) => types = split_types(&value),
                Err(outcome) => return outcome,
            }
        } else if !end_of_flags && arg.starts_with("--types=") {
            types = split_types(&arg["--types=".len()..]);
        } else if !end_of_flags && arg == "--force" {
            force = true;
        } else if !end_of_flags && (arg == "-h" || arg == "--help") {
            return Parsed::Help;
        } else if !end_of_flags && arg.starts_with('-') {
            return Parsed::Error(format!("unknown flag '{arg}'"));
        } else {
            targets.push(arg.clone());
        }
    }
    Parsed::Options(BrowseOptions {
        branch,
        types,
        force,
        targets,
    })
}

/// Split a `--types` value on whitespace/comma runs, keeping empty parts
/// like `preg_split` without `PREG_SPLIT_NO_EMPTY`.
pub fn split_types(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    while index < chars.len() {
        let (offset, ch) = chars[index];
        if ch.is_whitespace() || ch == ',' {
            parts.push(value[start..offset].to_string());
            index += 1;
            while index < chars.len() {
                let (_, next) = chars[index];
                if next.is_whitespace() || next == ',' {
                    index += 1;
                } else {
                    break;
                }
            }
            start = chars.get(index).map_or(value.len(), |(off, _)| *off);
        } else {
            index += 1;
        }
    }
    parts.push(value[start..].to_string());
    parts
}

/// One token to resolve, mirroring `ArcanistBrowseRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseRef {
    pub token: Option<String>,
    pub types: Vec<String>,
    pub branch: Option<String>,
}

impl BrowseRef {
    /// True when no `--types` filter applies (all kinds resolve).
    #[must_use]
    pub fn is_untyped(&self) -> bool {
        self.types.is_empty()
    }

    /// True when `kind` is in the `--types` filter.
    #[must_use]
    pub fn has_type(&self, kind: &str) -> bool {
        self.types.iter().any(|t| t == kind)
    }
}

/// One resolved URI plus its browse kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseUri {
    pub uri: String,
    pub kind: String,
}

/// An identified repository: PHID, browse base, and default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub phid: String,
    pub browse_uri: String,
    pub default_branch: String,
}

/// Resolution context: working directory, Conduit caller, API base URI
/// (for absolutizing), and the lazily resolved repository.
pub struct BrowseCtx<'a> {
    pub dir: &'a Path,
    pub call: &'a diff_submit::ConduitCall<'a>,
    pub conduit_uri: &'a str,
    repo: Option<Option<RepoInfo>>,
}

impl<'a> BrowseCtx<'a> {
    /// Build a context with an unresolved repository.
    #[must_use]
    pub fn new(
        dir: &'a Path,
        call: &'a diff_submit::ConduitCall<'a>,
        conduit_uri: &'a str,
    ) -> Self {
        Self {
            dir,
            call,
            conduit_uri,
            repo: None,
        }
    }

    /// The working-copy repository, resolved once and cached.
    pub fn repo(&mut self) -> Result<Option<&RepoInfo>, String> {
        if self.repo.is_none() {
            self.repo = Some(resolve_repo(self.dir, self.call)?);
        }
        Ok(self.repo.as_ref().and_then(|repo| repo.as_ref()))
    }
}

/// Rows of a legacy Conduit response (list or id-keyed map, in order).
fn query_rows(result: &serde_json::Value) -> Vec<&serde_json::Value> {
    match result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    }
}

fn row_string(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `git show -s --format=%H`, mirroring `getCanonicalRevisionName()`
/// (errors resolve to nothing).
fn canonical_commit(dir: &Path, token: &str) -> Option<String> {
    git::stdout_text(dir, &["show", "-s", "--format=%H", token, "--"])
        .ok()
        .filter(|hash| !hash.is_empty())
}

/// Canonical commits for one token: `.` resolves to nothing, a missing
/// token means `HEAD`.
pub fn commit_hashes(dir: &Path, token: &Option<String>) -> Vec<String> {
    match token {
        Some(text) if text == "." => Vec::new(),
        Some(text) => canonical_commit(dir, text).into_iter().collect(),
        None => canonical_commit(dir, "HEAD").into_iter().collect(),
    }
}

/// Upstream remote of the current branch (longest matching remote name).
fn upstream_remote(dir: &Path) -> Option<String> {
    let full = git::stdout_text(dir, &["rev-parse", "--symbolic-full-name", "@{upstream}"]).ok()?;
    let rest = full.strip_prefix("refs/remotes/")?;
    let remotes = git::stdout_text(dir, &["remote"]).ok()?;
    remotes
        .lines()
        .filter(|name| *rest == **name || rest.starts_with(&format!("{name}/")))
        .max_by_key(|name| name.len())
        .map(str::to_string)
}

/// Fetch URI of the upstream remote (else `origin`), like `getRemoteURI()`.
fn remote_uri(dir: &Path) -> Option<String> {
    let remote = upstream_remote(dir).unwrap_or_else(|| "origin".to_string());
    git::stdout_text(dir, &["config", &format!("remote.{remote}.url")])
        .ok()
        .filter(|uri| !uri.is_empty())
}

/// Identify the working-copy repository via `repository.query`
/// (callsign config, else remote URI). Zero or several matches, a
/// missing row, or an old server mean "no repository".
pub fn resolve_repo(
    dir: &Path,
    call: &diff_submit::ConduitCall,
) -> Result<Option<RepoInfo>, String> {
    if !git::is_inside_work_tree(dir) {
        return Ok(None);
    }
    let sources = get_config::load_sources(dir, config::arcrc_path().as_deref());
    let callsign = sources
        .ok()
        .and_then(|loaded| get_config::current_value("repository.callsign", &loaded))
        .and_then(|(_, value)| value.as_str().map(str::to_string))
        .filter(|callsign| !callsign.is_empty());
    let query = if let Some(callsign) = callsign {
        serde_json::json!({"callsigns": [callsign]})
    } else if let Some(uri) = remote_uri(dir) {
        serde_json::json!({"remoteURIs": [uri]})
    } else {
        return Ok(None);
    };
    let result = match call("repository.query", &query) {
        Ok(result) => result,
        Err(message) if message.contains("ERR-CONDUIT-CALL") => return Ok(None),
        Err(message) => return Err(message),
    };
    let rows = query_rows(&result);
    if rows.len() != 1 {
        return Ok(None);
    }
    let row = rows[0];
    let phid = row_string(row, "phid");
    let browse_uri = row_string(row, "uri");
    if phid.is_empty() || browse_uri.is_empty() {
        return Ok(None);
    }
    let default_branch = row
        .get("fields")
        .and_then(|fields| fields.get("defaultBranch"))
        .and_then(|v| v.as_str())
        .or_else(|| row.get("defaultBranch").and_then(|v| v.as_str()))
        .unwrap_or("master")
        .to_string();
    Ok(Some(RepoInfo {
        phid,
        browse_uri,
        default_branch,
    }))
}

/// Commit URIs via `diffusion.querycommits` (batched over hashes).
fn commit_uris(ctx: &mut BrowseCtx, hashes: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut uris = BTreeMap::new();
    if hashes.is_empty() {
        return Ok(uris);
    }
    let Some(repo) = ctx.repo()? else {
        return Ok(uris);
    };
    let repo_phid = repo.phid.clone();
    let result = (ctx.call)(
        "diffusion.querycommits",
        &serde_json::json!({"repositoryPHID": repo_phid, "names": hashes}),
    )?;
    let empty = serde_json::json!({});
    let identifiers = result.get("identifierMap").unwrap_or(&empty);
    let data = result.get("data").unwrap_or(&empty);
    for hash in hashes {
        let uri = identifiers
            .get(hash)
            .and_then(|phid| phid.as_str())
            .and_then(|phid| data.get(phid))
            .and_then(|commit| commit.get("uri"))
            .and_then(|uri| uri.as_str());
        if let Some(uri) = uri.filter(|uri| !uri.is_empty()) {
            uris.insert(hash.clone(), uri.to_string());
        }
    }
    Ok(uris)
}

/// Object URIs via `phid.lookup` (batched over tokens).
fn object_uris(ctx: &mut BrowseCtx, tokens: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut uris = BTreeMap::new();
    let names: BTreeMap<&str, &str> = tokens
        .iter()
        .filter(|token| !token.is_empty())
        .map(|token| (token.as_str(), token.as_str()))
        .collect();
    if names.is_empty() {
        return Ok(uris);
    }
    let result = (ctx.call)("phid.lookup", &serde_json::json!({"names": names}))?;
    for token in names.keys() {
        let uri = result
            .get(token)
            .and_then(|object| object.get("uri"))
            .and_then(|uri| uri.as_str());
        if let Some(uri) = uri.filter(|uri| !uri.is_empty()) {
            uris.insert((*token).to_string(), uri.to_string());
        }
    }
    Ok(uris)
}

/// Join `dir` with `path` when relative; clean `.`/`..` lexically.
fn resolve_path(dir: &Path, path: &str) -> PathBuf {
    let joined = Path::new(path);
    let full = if joined.is_absolute() {
        joined.to_path_buf()
    } else {
        dir.join(joined)
    };
    let mut clean = PathBuf::new();
    for part in full.components() {
        use std::path::Component::ParentDir;
        if part == ParentDir {
            clean.pop();
        } else {
            clean.push(part);
        }
    }
    clean
}

/// Canonicalize, resolving missing files through the nearest existing
/// ancestor (else the lexical path).
fn canonicalize_lax(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            missing.push(name.to_os_string());
        }
        if let Ok(canonical) = parent.canonicalize() {
            let mut out = canonical;
            for name in missing.iter().rev() {
                out.push(name);
            }
            return out;
        }
        current = parent;
    }
    path.to_path_buf()
}

/// Path relative to `root` when inside, else the full path.
fn readable_path(full: &Path, root: &Path) -> String {
    full.strip_prefix(root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| full.to_string_lossy().replace('\\', "/"))
}

/// `rawurlencode()`, mirroring PHP (unreserved bytes pass through).
fn rawurlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `phutil_escape_uri()`: encoded, but `/` stays a slash.
fn escape_uri(input: &str) -> String {
    rawurlencode(input).replace("%2F", "/")
}

/// `phutil_escape_uri_path_component()`: double-encoded.
fn escape_component(input: &str) -> String {
    rawurlencode(&rawurlencode(input))
}

/// A Diffusion browse URI, mirroring `newBrowseURI()`.
#[must_use]
pub fn new_browse_uri(
    repo: &RepoInfo,
    path: &str,
    lines: Option<&str>,
    branch: Option<&str>,
) -> String {
    let base = repo.browse_uri.trim_end_matches('/');
    let branch = branch
        .filter(|name| !name.is_empty())
        .unwrap_or(repo.default_branch.as_str());
    let uri_path = escape_uri(path.trim_start_matches('/'));
    let uri_lines = lines
        .filter(|text| !text.is_empty())
        .map(|text| format!("${}", escape_uri(text)))
        .unwrap_or_default();
    format!(
        "{base}/browse/{}/{uri_path}{uri_lines}",
        escape_component(branch)
    )
}

/// Split `path:lines` on the last colon (drive letters split too, like PHP).
fn split_lines(token: &str) -> (String, Option<String>) {
    match token.rsplit_once(':') {
        Some((path, lines)) => (path.to_string(), Some(lines.to_string())),
        None => (token.to_string(), None),
    }
}

/// A path URI for one ref (missing files resolve only when path-typed).
fn path_uri(ctx: &mut BrowseCtx, rf: &BrowseRef, repo: &RepoInfo) -> Option<BrowseUri> {
    let is_path = rf.has_type(BROWSE_PATH);
    let token = match &rf.token {
        Some(token) => token.clone(),
        None if is_path => ".".to_string(),
        None => return None,
    };
    let (path, lines) = split_lines(&token);
    let full = resolve_path(ctx.dir, &path);
    if !full.exists() && !is_path {
        return None;
    }
    let root = git::stdout_text(ctx.dir, &["rev-parse", "--show-toplevel"]).ok()?;
    let root = PathBuf::from(root);
    // Canonicalize for the comparison (short vs long paths, symlinks),
    // resolving missing files through existing ancestors.
    let full_canon = canonicalize_lax(&full);
    let root_canon = canonicalize_lax(&root);
    let rel = if full_canon == root_canon {
        String::new()
    } else {
        readable_path(&full_canon, &root_canon)
    };
    Some(BrowseUri {
        uri: new_browse_uri(repo, &rel, lines.as_deref(), rf.branch.as_deref()),
        kind: BROWSE_PATH.to_string(),
    })
}

/// Legacy `differential.query` rows mentioning `hash`: (uri, closed).
fn hash_revision_hits(
    call: &diff_submit::ConduitCall,
    hashes: &[Vec<String>],
) -> Result<BTreeMap<String, Vec<(String, bool)>>, String> {
    let mut hits: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    if hashes.is_empty() {
        return Ok(hits);
    }
    let flat: Vec<serde_json::Value> = hashes.iter().map(|pair| serde_json::json!(pair)).collect();
    let result = call(
        "differential.query",
        &serde_json::json!({"commitHashes": flat}),
    )?;
    for row in query_rows(&result) {
        let status = row.get("statusName").and_then(|v| v.as_str()).unwrap_or("");
        let closed = matches!(status, "Abandoned" | "Closed");
        let uri = row_string(row, "uri");
        if uri.is_empty() {
            continue;
        }
        let empty = Vec::new();
        let row_hashes = row
            .get("hashes")
            .and_then(|hashes| hashes.as_array())
            .unwrap_or(&empty);
        for pair in hashes {
            let wanted = pair.get(1).cloned().unwrap_or_default();
            let found = row_hashes.iter().any(|entry| {
                entry.as_array().is_some_and(|items| {
                    items.get(1).and_then(|v| v.as_str()) == Some(wanted.as_str())
                })
            });
            if found {
                hits.entry(wanted).or_default().push((uri.clone(), closed));
            }
        }
    }
    Ok(hits)
}

/// Footer ids across commit messages (malformed footers skipped).
fn footer_ids(dir: &Path, hashes: &[String]) -> BTreeMap<String, Vec<usize>> {
    let mut by_id: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, hash) in hashes.iter().enumerate() {
        let Ok(message) = git::stdout_text(dir, &["show", "-s", "--format=%B", hash, "--"]) else {
            continue;
        };
        if let Ok(Some(id)) = which::revision_id_from_corpus(&message) {
            by_id.entry(id).or_default().push(index);
        }
    }
    by_id
}

/// Modern `differential.revision.search` rows: (id, uri, closed).
fn search_revision_hits(
    call: &diff_submit::ConduitCall,
    ids: &[String],
) -> Result<Vec<(String, String, bool)>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| {
            id.parse::<u64>().map_or_else(
                |_| serde_json::Value::String(id.clone()),
                serde_json::Value::from,
            )
        })
        .collect();
    let result = call(
        "differential.revision.search",
        &serde_json::json!({"constraints": {"ids": ids}}),
    )?;
    let mut hits = Vec::new();
    if let Some(rows) = result.get("data").and_then(|data| data.as_array()) {
        for row in rows {
            let id = row
                .get("id")
                .and_then(|v| {
                    v.as_str().map(str::to_string).or_else(|| {
                        v.as_u64()
                            .map(|n| n.to_string())
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                })
                .unwrap_or_default();
            let uri = row
                .get("fields")
                .and_then(|fields| fields.get("uri"))
                .and_then(|uri| uri.as_str())
                .map_or_else(|| format!("/D{id}"), str::to_string);
            let closed = row
                .get("fields")
                .and_then(|fields| fields.get("status"))
                .and_then(|status| status.get("closed"))
                .and_then(|closed| closed.as_bool())
                .unwrap_or(false);
            hits.push((id, uri, closed));
        }
    }
    Ok(hits)
}

/// Revision URIs for commits: hash association plus message footers,
/// skipping closed revisions.
fn revision_uris(
    ctx: &mut BrowseCtx,
    hashes: &[String],
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut uris: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if hashes.is_empty() {
        return Ok(uris);
    }
    let pairs: Vec<Vec<String>> = hashes
        .iter()
        .map(|hash| vec!["gtcm".to_string(), hash.clone()])
        .collect();
    for (hash, hits) in hash_revision_hits(ctx.call, &pairs)? {
        for (uri, closed) in hits {
            if !closed {
                uris.entry(hash.clone()).or_default().push(uri);
            }
        }
    }
    let by_id = footer_ids(ctx.dir, hashes);
    let ids: Vec<String> = by_id.keys().cloned().collect();
    for (id, uri, closed) in search_revision_hits(ctx.call, &ids)? {
        if closed {
            continue;
        }
        if let Some(indices) = by_id.get(&id) {
            for index in indices {
                uris.entry(hashes[*index].clone())
                    .or_default()
                    .push(uri.clone());
            }
        }
    }
    Ok(uris)
}

/// Commits plus URIs resolved for one ref.
pub struct ResolvedRef {
    pub commits: Vec<String>,
    pub uris: Vec<BrowseUri>,
}

/// Resolve every ref, batching Conduit calls per query kind like PHP.
pub fn resolve_refs(ctx: &mut BrowseCtx, refs: &[BrowseRef]) -> Result<Vec<ResolvedRef>, String> {
    let commits: Vec<Vec<String>> = refs
        .iter()
        .map(|rf| {
            if rf.is_untyped() || rf.has_type(BROWSE_COMMIT) || rf.has_type(BROWSE_REVISION) {
                commit_hashes(ctx.dir, &rf.token)
            } else {
                Vec::new()
            }
        })
        .collect();

    let mut commit_hits: BTreeMap<String, String> = BTreeMap::new();
    let mut revision_hits: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let wanted_commits: Vec<String> = refs
        .iter()
        .zip(&commits)
        .flat_map(|(rf, hashes)| {
            if rf.is_untyped() || rf.has_type(BROWSE_COMMIT) {
                hashes.clone()
            } else {
                Vec::new()
            }
        })
        .collect();
    let wanted_revisions: Vec<String> = refs
        .iter()
        .zip(&commits)
        .flat_map(|(rf, hashes)| {
            if rf.is_untyped() || rf.has_type(BROWSE_REVISION) {
                hashes.clone()
            } else {
                Vec::new()
            }
        })
        .collect();
    commit_hits.extend(commit_uris(ctx, &wanted_commits)?);
    for (hash, uris) in revision_uris(ctx, &wanted_revisions)? {
        revision_hits.insert(hash, uris);
    }

    let wanted_objects: Vec<String> = refs
        .iter()
        .filter(|rf| rf.is_untyped() || rf.has_type(BROWSE_OBJECT))
        .filter_map(|rf| rf.token.clone())
        .collect();
    let object_hits = object_uris(ctx, &wanted_objects)?;

    let wants_path = refs
        .iter()
        .any(|rf| rf.is_untyped() || rf.has_type(BROWSE_PATH));
    let repo = if wants_path {
        ctx.repo()?.cloned()
    } else {
        None
    };

    let mut resolved = Vec::new();
    for (rf, hashes) in refs.iter().zip(&commits) {
        let mut uris = Vec::new();
        if rf.is_untyped() || rf.has_type(BROWSE_COMMIT) {
            for hash in hashes {
                if let Some(uri) = commit_hits.get(hash) {
                    uris.push(BrowseUri {
                        uri: uri.clone(),
                        kind: BROWSE_COMMIT.to_string(),
                    });
                }
            }
        }
        if (rf.is_untyped() || rf.has_type(BROWSE_OBJECT))
            && let Some(token) = &rf.token
            && let Some(uri) = object_hits.get(token)
        {
            uris.push(BrowseUri {
                uri: uri.clone(),
                kind: BROWSE_OBJECT.to_string(),
            });
        }
        if (rf.is_untyped() || rf.has_type(BROWSE_PATH))
            && let Some(repo) = &repo
            && let Some(found) = path_uri(ctx, rf, repo)
        {
            uris.push(found);
        }
        if rf.is_untyped() || rf.has_type(BROWSE_REVISION) {
            for hash in hashes {
                if let Some(found) = revision_hits.get(hash) {
                    for uri in found {
                        uris.push(BrowseUri {
                            uri: uri.clone(),
                            kind: BROWSE_REVISION.to_string(),
                        });
                    }
                }
            }
        }
        resolved.push(ResolvedRef {
            commits: hashes.clone(),
            uris,
        });
    }
    Ok(resolved)
}

/// True when any ref resolved to several URIs.
fn any_many(resolved: &[ResolvedRef]) -> bool {
    resolved.iter().any(|rf| rf.uris.len() > 1)
}

/// Format one pick-table row.
fn table_row(cells: &[String]) -> String {
    cells.join(" ")
}

/// Raw display widths spanning titles and cells, like `addRow` plus
/// `getWidth` without the padding.
fn table_widths(columns: &[&str], rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    widths
}

/// A borderless table (`Pick Type URI`), mirroring `PhutilConsoleTable`
/// spacing without borders (padding 1, last column unpadded).
#[must_use]
pub fn render_table(columns: &[&str], rows: &[Vec<String>]) -> String {
    let widths = table_widths(columns, rows);
    let mut out = String::new();
    out.push_str(&table_row(&pad_row(columns, &widths)));
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = cells.iter().map(String::as_str).collect();
        out.push_str(&table_row(&pad_row(&refs, &widths)));
        out.push('\n');
    }
    out
}

/// Pad one cell to its width unless it is the last column.
fn pad_cell(cell: &str, width: usize, last: bool) -> String {
    if last {
        return cell.to_string();
    }
    let pad = width.saturating_sub(cell.chars().count());
    format!("{cell}{}", " ".repeat(pad))
}

/// Pad every column but the last to its width.
fn pad_row(cells: &[&str], widths: &[usize]) -> Vec<String> {
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let width = widths.get(index).copied().unwrap_or_default();
            pad_cell(cell, width, index + 1 == cells.len())
        })
        .collect()
}

/// Pad a rendered title: spaces follow the markup, sized by the raw
/// title width (ANSI codes add no display width).
fn pad_title(rendered: &str, raw_chars: usize, width: usize, last: bool) -> String {
    if last {
        return rendered.to_string();
    }
    let pad = width.saturating_sub(raw_chars);
    format!("{rendered}{}", " ".repeat(pad))
}

/// The token display for multi-ref tables: `pht('<default>')` for empty
/// tokens, else the token itself.
fn default_token_display(token: Option<&str>) -> String {
    match token.filter(|text| !text.is_empty()) {
        Some(text) => text.to_string(),
        None => terminal::pht("<default>", &[]),
    }
}

/// The multi-match pick table (stdout text), mirroring
/// `PhutilConsoleTable` without borders: `pht` titles render bold
/// (`tsprintf('**%s**', ...)`), cells pad on raw widths and are escaped
/// afterwards (`drawConsoleString`).
#[must_use]
pub fn render_pick_table(
    refs: &[BrowseRef],
    resolved: &[ResolvedRef],
    single: bool,
    ansi: bool,
) -> String {
    let columns: &[&str] = if single {
        &["Pick", "Type", "URI"]
    } else {
        &["Argument", "Type", "URI"]
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut pick_id = 0;
    for (rf, hit) in refs.iter().zip(resolved) {
        if hit.uris.len() <= 1 {
            continue;
        }
        for uri in &hit.uris {
            pick_id += 1;
            let first = if single {
                pick_id.to_string()
            } else {
                default_token_display(rf.token.as_deref())
            };
            rows.push(vec![first, uri.kind.clone(), uri.uri.clone()]);
        }
    }
    let widths = table_widths(columns, &rows);
    let mut out = String::new();
    let header: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let text = terminal::pht(title, &[]);
            let bold = terminal::tsprintf_ansi("**%s**", &[TVal::Text(&text)], ansi)
                .expect("fixed tsprintf pattern");
            let width = widths.get(index).copied().unwrap_or_default();
            pad_title(
                bold.as_str(),
                text.chars().count(),
                width,
                index + 1 == columns.len(),
            )
        })
        .collect();
    out.push_str(&table_row(&header));
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let width = widths.get(index).copied().unwrap_or_default();
                let padded = pad_cell(cell, width, index + 1 == row.len());
                terminal::escape_value(&padded, true)
            })
            .collect();
        out.push_str(&table_row(&cells));
        out.push('\n');
    }
    out
}

/// Read one answer line (`None` on EOF/error).
fn read_answer(input: &mut dyn BufRead) -> Option<String> {
    std::io::stdout().flush().ok();
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
    }
}

/// The wrapped select prompt, like `phutil_console_prompt()`: the
/// prompt plus ` [{min} - {max}] ` wrapped at indent 4.
#[must_use]
pub fn select_prompt(prompt: &str, min: u64, max: u64) -> String {
    terminal::console_wrap(&format!("{prompt} [{min} - {max}] "), 4, true)
}

/// `phutil_console_select()`: `{prompt} [{min} - {max}]` until a number
/// in range; end of input aborts.
pub fn console_select(
    input: &mut dyn BufRead,
    prompt: &str,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    loop {
        print!("\n\n{}", select_prompt(prompt, min, max));
        let Some(raw) = read_answer(input) else {
            return Err(ABORT.to_string());
        };
        let trimmed = raw.trim();
        if !trimmed.is_empty()
            && trimmed.bytes().all(|b| b.is_ascii_digit())
            && let Ok(pick) = trimmed.parse::<u64>()
            && pick >= min
            && pick <= max
        {
            return Ok(pick);
        }
    }
}

/// One warning/info line on stderr, through the log engine
/// (`writeWarn` yellow, `writeInfo` blue).
fn push_status(color: &str, label: &str, message: &str) {
    eprint!(
        "{}",
        terminal::format_log(color, label, message, terminal::ansi_enabled())
    );
}

/// Make `raw` absolute against the Conduit base when it has no domain.
#[must_use]
pub fn absolute_uri(conduit_uri: &str, raw: &str) -> String {
    if raw.contains("://") {
        return raw.to_string();
    }
    let base = conduit_uri.trim_end_matches('/');
    let path = raw.strip_prefix('/').unwrap_or(raw);
    if let Some(scheme_end) = base.find("://") {
        let after = &base[scheme_end + 3..];
        let host = after.split('/').next().unwrap_or(after);
        let scheme = &base[..scheme_end];
        format!("{scheme}://{host}/{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// Report ambiguity, list the table, and pick one URI for single refs.
fn disambiguate(
    refs: &[BrowseRef],
    resolved: &[ResolvedRef],
    input: &mut dyn BufRead,
    ansi: bool,
) -> Result<Vec<BrowseUri>, String> {
    let ambiguous = terminal::pht("AMBIGUOUS", &[]);
    for (rf, hit) in refs.iter().zip(resolved) {
        if hit.uris.len() > 1 {
            let message = match &rf.token {
                Some(token) => {
                    terminal::pht("Argument \"%s\" is ambiguous.", &[PhtArg::Text(token)])
                }
                None => terminal::pht("Default behavior is ambiguous.", &[]),
            };
            push_status("yellow", &ambiguous, &message);
        }
    }
    let single = refs.len() == 1;
    print!("{}", render_pick_table(refs, resolved, single, ansi));
    if !single {
        push_status(
            "blue",
            &terminal::pht("CHOOSE", &[]),
            &terminal::pht("Use \"--types\" to select between alternatives.", &[]),
        );
        return Ok(Vec::new());
    }
    let max = resolved
        .iter()
        .map(|hit| hit.uris.len() as u64)
        .sum::<u64>();
    let prompt = terminal::pht("Which URI do you want to open?", &[]);
    let pick = console_select(input, &prompt, 1, max)?;
    let mut ordered: Vec<BrowseUri> = Vec::new();
    for hit in resolved {
        if hit.uris.len() > 1 {
            ordered.extend(hit.uris.clone());
        }
    }
    Ok(ordered
        .into_iter()
        .nth((pick - 1) as usize)
        .into_iter()
        .collect())
}

/// Report unresolvable refs (stdout text): `echo tsprintf("%s\n",
/// pht(...))` per ref.
fn render_unresolved(refs: &[BrowseRef], resolved: &[ResolvedRef], ansi: bool) -> String {
    let mut out = String::new();
    for (rf, hit) in refs.iter().zip(resolved) {
        if hit.uris.is_empty() {
            let message = match &rf.token {
                Some(token) => {
                    terminal::pht("Unable to resolve argument \"%s\".", &[PhtArg::Text(token)])
                }
                None => terminal::pht("Unable to resolve default browse target.", &[]),
            };
            let line = terminal::tsprintf_ansi("%s\n", &[TVal::Text(&message)], ansi)
                .expect("fixed tsprintf pattern");
            out.push_str(line.as_str());
        }
    }
    out
}

/// Open one ref set, returning the URIs to open (stdout text included).
fn execute_refs(
    ctx: &mut BrowseCtx,
    refs: &[BrowseRef],
    input: &mut dyn BufRead,
    ansi: bool,
) -> Result<Vec<String>, String> {
    let resolved = resolve_refs(ctx, refs)?;
    let mut open: Vec<BrowseUri> = Vec::new();
    for hit in &resolved {
        if hit.uris.len() == 1 {
            open.extend(hit.uris.clone());
        }
    }
    if any_many(&resolved) {
        open.extend(disambiguate(refs, &resolved, input, ansi)?);
    }
    print!("{}", render_unresolved(refs, &resolved, ansi));
    Ok(open
        .iter()
        .map(|uri| absolute_uri(ctx.conduit_uri, &uri.uri))
        .collect())
}

/// Build refs from targets (deduped, order kept), applying types/branch.
fn build_refs(options: &BrowseOptions) -> Vec<BrowseRef> {
    let mut seen = Vec::new();
    for target in &options.targets {
        if !seen.contains(target) {
            seen.push(target.clone());
        }
    }
    if seen.is_empty() {
        return vec![BrowseRef {
            token: None,
            types: options.types.clone(),
            branch: options.branch.clone(),
        }];
    }
    seen.into_iter()
        .map(|token| BrowseRef {
            token: Some(token),
            types: options.types.clone(),
            branch: options.branch.clone(),
        })
        .collect()
}

/// Run with an injected Conduit caller and prompt input.
pub fn run_in_with(
    dir: &Path,
    argv: &[String],
    call: Option<&diff_submit::ConduitCall>,
    input: &mut dyn BufRead,
) -> ExitCode {
    let mut options = match parse_args(argv) {
        Parsed::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Parsed::Options(options) => options,
    };
    if options.force {
        push_status(
            "yellow",
            &terminal::pht("DEPRECATED", &[]),
            &terminal::pht(
                "Argument \"--force\" for \"arc browse\" is deprecated. Use \"--type %s\" instead.",
                &[PhtArg::Text(BROWSE_PATH)],
            ),
        );
        options.types = vec![BROWSE_PATH.to_string()];
    }
    // Injected callers resolve against "" (see `run_prepared` for an
    // explicit base URI); otherwise build a real client.
    if let Some(call) = call {
        return run_prepared(dir, &options, call, "", input);
    }
    let client = match config::client_for(None) {
        Ok(client) => client,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let live = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    run_prepared(dir, &options, &live, client.base_uri(), input)
}

/// Run with a resolved Conduit base URI (used by `run_in` and tests).
pub fn run_prepared(
    dir: &Path,
    options: &BrowseOptions,
    call: &diff_submit::ConduitCall,
    conduit_uri: &str,
    input: &mut dyn BufRead,
) -> ExitCode {
    let mut ctx = BrowseCtx::new(dir, call, conduit_uri);
    let refs = build_refs(options);
    let uris = match execute_refs(&mut ctx, &refs, input, terminal::ansi_enabled()) {
        Ok(uris) => uris,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let configured = browser::configured_browser(dir);
    if let Err(message) = browser::open_uris_in_browser(&uris, configured.as_deref()) {
        eprintln!("arc-rs {NAME}: {message}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Begin browsing in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    run_in_with(dir, argv, None, &mut input)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "browse_tests.rs"]
mod browse_tests;
