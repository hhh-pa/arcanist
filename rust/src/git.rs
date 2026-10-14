//! Shared git subprocess helpers for workflow modules.
//!
//! New code must use these instead of spawning `git` directly, so flags,
//! error shapes, and directory handling stay uniform.
//!
//! Port of the git-relevant parts of `src/repository/api/ArcanistGitAPI.php`,
//! `src/repository/api/ArcanistGitUpstreamPath.php`,
//! `src/repository/api/ArcanistRepositoryAPI.php` (flag constants, display
//! hashes), and `src/repository/raw/ArcanistGitRawCommit.php` (task T1128).
//!
//! Naming follows the PHP methods verbatim, transliterated to Rust
//! snake_case (`getBranchName` becomes [`branch_name`]); struct fields that
//! collide with Rust keywords keep the PHP name via raw identifiers
//! (`r#type`, `r#ref`). Every public item cites its PHP source line.
//!
//! The PHP API is stateful (cached version, metadata path, base/head
//! commits, explanations). This port is stateless: every function takes its
//! inputs explicitly and re-queries git on each call. Divergences from PHP
//! behavior are documented on the affected items.

// Integration harnesses path-include this file (see `rust/tests/*.rs`),
// where helpers a binary never calls would trip `dead_code` under
// `-D warnings`. Sibling harnesses are owned by other bundles and
// disagree on the outer `mod` allow, so the allow lives here, scoped
// to test builds; the library build keeps the lint. The second allow
// covers harnesses that already allow `dead_code` on the `mod`, where
// clippy would otherwise flag this inner allow as duplicated.
#![cfg_attr(test, allow(dead_code))]
#![cfg_attr(test, allow(clippy::duplicated_attributes))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Run git in `dir`, returning the raw output. Spawn failures become
/// errors; non-zero exit statuses are left for the caller to interpret.
pub fn output(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// Run git in `dir`, returning trimmed stdout, or the combined trimmed
/// output when git exits non-zero.
pub fn stdout_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = output(dir, args)?;
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
            Err(format!("git {} failed", args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

/// True when git exits zero in `dir` for `args`.
#[must_use]
pub fn ok(dir: &Path, args: &[&str]) -> bool {
    output(dir, args).is_ok_and(|out| out.status.success())
}

/// True when `dir` is inside a git working tree.
#[must_use]
pub fn is_inside_work_tree(dir: &Path) -> bool {
    let Ok(out) = output(dir, &["rev-parse", "--is-inside-work-tree"]) else {
        return false;
    };
    out.status.success() && out.stdout.trim_ascii() == b"true"
}

/// Hash of the empty tree, used as the base commit for repositories with
/// zero or one commits.
///
/// PHP: `ArcanistGitAPI::GIT_MAGIC_ROOT_COMMIT`
/// (`src/repository/api/ArcanistGitAPI.php:15`).
pub const GIT_MAGIC_ROOT_COMMIT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// How many parents back `git_history_log` looks for revision numbers.
///
/// PHP: `ArcanistGitAPI::SEARCH_LENGTH_FOR_PARENT_REVISIONS`
/// (`src/repository/api/ArcanistGitAPI.php:9`).
pub const SEARCH_LENGTH_FOR_PARENT_REVISIONS: u32 = 16;

/// Hash of the empty tree for newly built raw commits.
///
/// PHP: `ArcanistGitRawCommit::GIT_EMPTY_TREE_HASH`
/// (`src/repository/raw/ArcanistGitRawCommit.php:12`).
/// Same value as [`GIT_MAGIC_ROOT_COMMIT`]; kept as a separate constant
/// because PHP declares it separately.
pub const GIT_EMPTY_TREE_HASH: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Path changed in a way covered by no more specific flag.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_MODIFIED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:10`).
pub const FLAG_MODIFIED: u32 = 1;
/// Path was added.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_ADDED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:11`).
pub const FLAG_ADDED: u32 = 2;
/// Path was deleted.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_DELETED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:12`).
pub const FLAG_DELETED: u32 = 4;
/// Path is not tracked by version control.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_UNTRACKED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:13`).
pub const FLAG_UNTRACKED: u32 = 8;
/// Path has a merge conflict.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_CONFLICT`
/// (`src/repository/api/ArcanistRepositoryAPI.php:14`).
pub const FLAG_CONFLICT: u32 = 16;
/// Path is missing from disk.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_MISSING`
/// (`src/repository/api/ArcanistRepositoryAPI.php:15`).
pub const FLAG_MISSING: u32 = 32;
/// Path has changes not staged for commit.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_UNSTAGED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:16`).
pub const FLAG_UNSTAGED: u32 = 64;
/// Path differs from HEAD.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_UNCOMMITTED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:17`).
pub const FLAG_UNCOMMITTED: u32 = 128;
/// Submodule with uncommitted or untracked changes inside it.
///
/// PHP: `ArcanistRepositoryAPI::FLAG_EXTERNALS`
/// (`src/repository/api/ArcanistRepositoryAPI.php:21`).
pub const FLAG_EXTERNALS: u32 = 256;
/// Path replaced by an unmanaged directory (Subversion only).
///
/// PHP: `ArcanistRepositoryAPI::FLAG_OBSTRUCTED`
/// (`src/repository/api/ArcanistRepositoryAPI.php:25`).
pub const FLAG_OBSTRUCTED: u32 = 512;
/// Update was interrupted (Subversion only).
///
/// PHP: `ArcanistRepositoryAPI::FLAG_INCOMPLETE`
/// (`src/repository/api/ArcanistRepositoryAPI.php:28`).
pub const FLAG_INCOMPLETE: u32 = 1024;

/// Prefix of local-branch upstream refs (`refs/heads/...`).
///
/// PHP: matched inline by `ArcanistGitAPI::getPathToUpstream`
/// (`src/repository/api/ArcanistGitAPI.php:1473`).
pub const UPSTREAM_LOCAL_PREFIX: &str = "refs/heads/";
/// Prefix of remote-tracking upstream refs (`refs/remotes/...`).
///
/// PHP: matched inline by `ArcanistGitAPI::getPathToUpstream`
/// (`src/repository/api/ArcanistGitAPI.php:1496`).
pub const UPSTREAM_REMOTE_PREFIX: &str = "refs/remotes/";

/// `git` version-control system name.
///
/// PHP: `ArcanistGitAPI::getSourceControlSystemName`
/// (`src/repository/api/ArcanistGitAPI.php:49`).
#[must_use]
pub fn source_control_system_name() -> &'static str {
    "git"
}

/// Symbol for the current commit.
///
/// PHP: `ArcanistGitAPI::newCurrentCommitSymbol`
/// (`src/repository/api/ArcanistGitAPI.php:1662`).
#[must_use]
pub fn current_commit_symbol() -> &'static str {
    "HEAD"
}

/// Git has no source-control path concept; always `None`.
///
/// PHP: `ArcanistGitAPI::getSourceControlPath`
/// (`src/repository/api/ArcanistGitAPI.php:559`).
#[must_use]
pub fn source_control_path() -> Option<String> {
    None
}

/// Git history is mutable.
///
/// PHP: `ArcanistGitAPI::isHistoryDefaultImmutable`
/// (`src/repository/api/ArcanistGitAPI.php:1128`).
#[must_use]
pub fn is_history_default_immutable() -> bool {
    false
}

/// Git supports amend.
///
/// PHP: `ArcanistGitAPI::supportsAmend`
/// (`src/repository/api/ArcanistGitAPI.php:1132`).
#[must_use]
pub fn supports_amend() -> bool {
    true
}

/// Git supports commit ranges.
///
/// PHP: `ArcanistGitAPI::supportsCommitRanges`
/// (`src/repository/api/ArcanistGitAPI.php:1136`).
#[must_use]
pub fn supports_commit_ranges() -> bool {
    true
}

/// Git supports local commits.
///
/// PHP: `ArcanistGitAPI::supportsLocalCommits`
/// (`src/repository/api/ArcanistGitAPI.php:1140`).
#[must_use]
pub fn supports_local_commits() -> bool {
    true
}

/// Git supports stashing.
///
/// PHP: `ArcanistGitAPI::canStashChanges`
/// (`src/repository/api/ArcanistGitAPI.php:1433`).
#[must_use]
pub fn can_stash_changes() -> bool {
    true
}

/// Message printed after a revision is finalized.
///
/// PHP: `ArcanistGitAPI::getFinalizedRevisionMessage`
/// (`src/repository/api/ArcanistGitAPI.php:1164`).
#[must_use]
pub fn finalized_revision_message() -> String {
    "You may now push this commit upstream, as appropriate (e.g. with \
     'git push', or 'git svn dcommit', or by printing and faxing it)."
        .to_string()
}

/// First 12 characters of a hash for display.
///
/// PHP: `ArcanistRepositoryAPI::getDisplayHash`
/// (`src/repository/api/ArcanistRepositoryAPI.php:806`).
///
/// Divergence: PHP `substr` is byte-based; the cut snaps back to a
/// character boundary so non-ASCII input cannot panic. Hashes are hex in
/// practice, where both agree.
#[must_use]
pub fn display_hash(hash: &str) -> &str {
    let end = hash.len().min(12);
    let end = (0..=end)
        .rev()
        .find(|&i| hash.is_char_boundary(i))
        .unwrap_or(0);
    &hash[..end]
}

/// Kind of one hop in an [`UpstreamPath`].
///
/// PHP: `ArcanistGitUpstreamPath::TYPE_LOCAL` / `TYPE_REMOTE` string values
/// (`src/repository/api/ArcanistGitUpstreamPath.php:7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamType {
    /// Hop to another local branch.
    Local,
    /// Hop to a branch on a remote.
    Remote,
}

impl UpstreamType {
    /// PHP string value of this hop kind (`local` or `remote`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UpstreamType::Local => UpstreamPath::TYPE_LOCAL,
            UpstreamType::Remote => UpstreamPath::TYPE_REMOTE,
        }
    }
}

/// One hop in an [`UpstreamPath`]: the spec array PHP stores per branch.
///
/// PHP: spec arrays built by `ArcanistGitAPI::getPathToUpstream`
/// (`src/repository/api/ArcanistGitAPI.php:1478` for local hops,
/// `src/repository/api/ArcanistGitAPI.php:1500` for remote hops).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamHop {
    /// Hop kind (`type` key in PHP).
    pub r#type: UpstreamType,
    /// Local branch name, or remote branch name (`name` key in PHP).
    pub name: String,
    /// Remote name (`remote` key; only set on remote hops).
    pub remote: Option<String>,
    /// Whether following this hop revisits a branch (`cycle` key on
    /// local hops; always false on remote hops).
    pub cycle: bool,
}

impl UpstreamHop {
    /// Hop to the local branch `name`; `cycle` when `name` was visited.
    #[must_use]
    pub fn local(name: String, cycle: bool) -> Self {
        Self {
            r#type: UpstreamType::Local,
            name,
            remote: None,
            cycle,
        }
    }

    /// Hop to `branch` on `remote`.
    #[must_use]
    pub fn remote(branch: String, remote: String) -> Self {
        Self {
            r#type: UpstreamType::Remote,
            name: branch,
            remote: Some(remote),
            cycle: false,
        }
    }
}

/// Chain of tracking branches from a local branch up to a remote.
///
/// PHP: `ArcanistGitUpstreamPath`
/// (`src/repository/api/ArcanistGitUpstreamPath.php:3`).
/// Insert-ordered like the PHP array; keys are branch cursors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpstreamPath {
    hops: Vec<(String, UpstreamHop)>,
}

impl UpstreamPath {
    /// Hop-kind value for local branches.
    ///
    /// PHP: `ArcanistGitUpstreamPath::TYPE_LOCAL`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:7`).
    pub const TYPE_LOCAL: &'static str = "local";
    /// Hop-kind value for remote branches.
    ///
    /// PHP: `ArcanistGitUpstreamPath::TYPE_REMOTE`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:8`).
    pub const TYPE_REMOTE: &'static str = "remote";

    /// Empty path.
    #[must_use]
    pub fn new() -> Self {
        Self { hops: Vec::new() }
    }

    /// Record that `key` tracks `spec`, replacing any older hop for `key`.
    ///
    /// PHP: `ArcanistGitUpstreamPath::addUpstream`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:11`).
    pub fn add_upstream(&mut self, key: String, spec: UpstreamHop) -> &mut Self {
        if let Some(slot) = self.hops.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = spec;
        } else {
            self.hops.push((key, spec));
        }
        self
    }

    /// Drop the hop keyed by `key`, if present.
    ///
    /// PHP: `ArcanistGitUpstreamPath::removeUpstream`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:16`).
    pub fn remove_upstream(&mut self, key: &str) -> &mut Self {
        self.hops.retain(|(k, _)| k != key);
        self
    }

    /// Hop keyed by `key`, if present.
    ///
    /// PHP: `ArcanistGitUpstreamPath::getUpstream`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:21`).
    #[must_use]
    pub fn get_upstream(&self, key: &str) -> Option<&UpstreamHop> {
        self.hops.iter().find(|(k, _)| k == key).map(|(_, hop)| hop)
    }

    /// Number of hops.
    ///
    /// PHP: `ArcanistGitUpstreamPath::getLength`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:25`).
    #[must_use]
    pub fn get_length(&self) -> usize {
        self.hops.len()
    }

    /// True when the last hop reaches a remote.
    ///
    /// PHP: `ArcanistGitUpstreamPath::isConnectedToRemote`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:34`).
    #[must_use]
    pub fn is_connected_to_remote(&self) -> bool {
        self.hops
            .last()
            .is_some_and(|(_, hop)| hop.r#type == UpstreamType::Remote)
    }

    /// Branch cursors in visit order.
    ///
    /// PHP: `ArcanistGitUpstreamPath::getLocalBranches`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:44`).
    #[must_use]
    pub fn get_local_branches(&self) -> Vec<String> {
        self.hops.iter().map(|(k, _)| k.clone()).collect()
    }

    /// Remote branch name when connected to a remote.
    ///
    /// PHP: `ArcanistGitUpstreamPath::getRemoteBranchName`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:48`).
    #[must_use]
    pub fn get_remote_branch_name(&self) -> Option<&str> {
        if !self.is_connected_to_remote() {
            return None;
        }
        self.hops.last().map(|(_, hop)| hop.name.as_str())
    }

    /// Remote name when connected to a remote.
    ///
    /// PHP: `ArcanistGitUpstreamPath::getRemoteRemoteName`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:56`).
    #[must_use]
    pub fn get_remote_remote_name(&self) -> Option<&str> {
        if !self.is_connected_to_remote() {
            return None;
        }
        self.hops.last().and_then(|(_, hop)| hop.remote.as_deref())
    }

    /// Description of the local-branch cycle, if the path ends in one.
    ///
    /// PHP: `ArcanistGitUpstreamPath::getCycle`
    /// (`src/repository/api/ArcanistGitUpstreamPath.php:70`).
    #[must_use]
    pub fn get_cycle(&self) -> Option<Vec<String>> {
        let (_, last) = self.hops.last()?;
        if !last.cycle {
            return None;
        }
        let mut parts = self.get_local_branches();
        parts.push(last.name.clone());
        parts.push("...".to_string());
        Some(parts)
    }
}

/// One `@{upstream}` reference classified by shape.
///
/// PHP: classified inline by `ArcanistGitAPI::getPathToUpstream`
/// (`src/repository/api/ArcanistGitAPI.php:1473`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamRef {
    /// `refs/heads/<name>` hop target.
    LocalBranch(String),
    /// `refs/remotes/<remote>/<branch>` hop target.
    RemoteBranch {
        /// Remote name (before the first `/`).
        remote: String,
        /// Branch name (after the first `/`).
        branch: String,
    },
}

/// Classify trimmed `rev-parse --symbolic-full-name X@{upstream}` output.
///
/// PHP: `ArcanistGitAPI::getPathToUpstream`
/// (`src/repository/api/ArcanistGitAPI.php:1471`).
///
/// Divergence: a `refs/remotes/` value without `/` (a remote with no
/// branch part, which git never emits) is rejected with the
/// unrecognized-format error; PHP would store a null branch name.
pub fn classify_upstream_ref(upstream: &str) -> Result<UpstreamRef, String> {
    if let Some(name) = upstream.strip_prefix(UPSTREAM_LOCAL_PREFIX) {
        return Ok(UpstreamRef::LocalBranch(name.to_string()));
    }
    if let Some(rest) = upstream.strip_prefix(UPSTREAM_REMOTE_PREFIX)
        && let Some((remote, branch)) = rest.split_once('/')
    {
        return Ok(UpstreamRef::RemoteBranch {
            remote: remote.to_string(),
            branch: branch.to_string(),
        });
    }
    Err(format!(
        "Got unrecognized upstream format (\"{upstream}\") from Git, expected \
         \"refs/heads/...\" or \"refs/remotes/...\"."
    ))
}

/// One row of `getLocalCommitInformation` output.
///
/// PHP: rows built by `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:193`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCommit {
    /// Commit hash (`%H`).
    pub commit: String,
    /// Tree hash (`%T`).
    pub tree: String,
    /// Parent hashes (`%P`, space-separated).
    pub parents: Vec<String>,
    /// Author timestamp (`%at`).
    pub time: String,
    /// Author name (`%an`).
    pub author: String,
    /// Subject line (`%s`).
    pub summary: String,
    /// Full message (`%s%n%n%b`, trailing whitespace trimmed).
    pub message: String,
    /// Author email (`%aE`).
    pub author_email: String,
}

/// One row of `getAllBranches` output.
///
/// PHP: rows built by `ArcanistGitAPI::getAllBranches`
/// (`src/repository/api/ArcanistGitAPI.php:1091`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// Whether this is the checked-out branch.
    pub current: bool,
    /// Short branch name.
    pub name: String,
    /// Full ref (`refs/heads/...`).
    pub r#ref: String,
    /// Commit hash.
    pub hash: String,
    /// Tree hash.
    pub tree: String,
    /// Committer epoch (leading integer of `committerdate:raw`).
    pub epoch: i64,
    /// Subject line.
    pub desc: String,
    /// Subject plus body.
    pub text: String,
}

/// One row of `parseGitRawDiff` full-mode output.
///
/// PHP: `ArcanistGitAPI::parseGitRawDiff` with `$full = true`
/// (`src/repository/api/ArcanistGitAPI.php:919`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDiffEntry {
    /// Status bitmask (`FLAG_*`).
    pub mask: u32,
    /// New blob hash (40 zeros for a rename source).
    pub r#ref: String,
}

/// One row of `parseGitTree` output.
///
/// PHP: rows built by `ArcanistGitAPI::parseGitTree`
/// (`src/repository/api/ArcanistGitAPI.php:1013`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Six-digit mode.
    pub mode: String,
    /// `blob`, `tree`, or `commit`.
    pub r#type: String,
    /// Object hash.
    pub r#ref: String,
}

/// One blamed line: author name plus commit hash.
///
/// PHP: rows built by `ArcanistGitAPI::getBlame`
/// (`src/repository/api/ArcanistGitAPI.php:980`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// Author name.
    pub author: String,
    /// Commit hash.
    pub revision: String,
}

/// Parsed raw commit object.
///
/// PHP: `ArcanistGitRawCommit`
/// (`src/repository/raw/ArcanistGitRawCommit.php:3`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawCommit {
    tree_hash: Option<String>,
    parents: Vec<String>,
    raw_author: Option<String>,
    raw_committer: Option<String>,
    message: Option<String>,
}

impl RawCommit {
    /// Commit with the empty tree and no other fields.
    ///
    /// PHP: `ArcanistGitRawCommit::newEmptyCommit`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:14`).
    #[must_use]
    pub fn new_empty_commit() -> Self {
        Self {
            tree_hash: Some(GIT_EMPTY_TREE_HASH.to_string()),
            ..Self::default()
        }
    }

    /// Parse a `git cat-file commit` blob.
    ///
    /// PHP: `ArcanistGitRawCommit::newFromRawBlob`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:20`).
    ///
    /// Headers end at the first blank line; `parent` may repeat; unknown
    /// labels and malformed lines are errors. Error texts mirror the PHP
    /// messages (shown lines are trimmed of the newline here).
    pub fn new_from_raw_blob(blob: &str) -> Result<Self, String> {
        const PATTERN: &str = "(^(\\w+) ([^\\n]+)\\n?\\z)";
        let mut raw = Self::default();
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        let mut rest = blob;
        let mut consumed = 0usize;
        loop {
            let (line, tail) = match rest.find('\n') {
                Some(pos) => (&rest[..pos + 1], &rest[pos + 1..]),
                None => (rest, ""),
            };
            if line.is_empty() {
                break;
            }
            consumed += line.len();
            rest = tail;
            if line == "\n" {
                break;
            }
            let body = line.strip_suffix('\n').unwrap_or(line);
            let (label, value) = body
                .split_once(' ')
                .filter(|(label, value)| {
                    !label.is_empty()
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        && !value.is_empty()
                })
                .ok_or_else(|| {
                    format!(
                        "Expected to match pattern \"{PATTERN}\" against line \"{body}\" \
                         in raw commit blob: {blob}"
                    )
                })?;
            if seen.contains_key(label) && label != "parent" {
                let first = seen[label].trim_end_matches('\n');
                return Err(format!(
                    "Encountered two \"{label}\" lines (\"{first}\", \"{body}\") while \
                     parsing raw commit blob, expected at most one: {blob}"
                ));
            }
            seen.insert(label, line);
            match label {
                "tree" => raw.tree_hash = Some(value.to_string()),
                "parent" => raw.parents.push(value.to_string()),
                "author" => raw.raw_author = Some(value.to_string()),
                "committer" => raw.raw_committer = Some(value.to_string()),
                _ => {
                    return Err(format!(
                        "Unknown attribute label \"{label}\" in line \"{body}\" while \
                         parsing raw commit blob: {blob}"
                    ));
                }
            }
            if tail.is_empty() && !line.ends_with('\n') {
                break;
            }
        }
        raw.message = Some(blob[consumed..].to_string());
        Ok(raw)
    }

    /// Serialize back to a `git hash-object --stdin` blob.
    ///
    /// PHP: `ArcanistGitRawCommit::getRawBlob`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:100`).
    #[must_use]
    pub fn get_raw_blob(&self) -> String {
        let mut out = String::new();
        if let Some(tree) = &self.tree_hash {
            out.push_str(&format!("tree {tree}\n"));
        }
        for parent in &self.parents {
            out.push_str(&format!("parent {parent}\n"));
        }
        if let Some(author) = &self.raw_author {
            out.push_str(&format!("author {author}\n"));
        }
        if let Some(committer) = &self.raw_committer {
            out.push_str(&format!("committer {committer}\n"));
        }
        out.push('\n');
        if let Some(message) = &self.message {
            out.push_str(message);
        }
        out
    }

    /// Set the tree hash.
    ///
    /// PHP: `ArcanistGitRawCommit::setTreeHash`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:133`).
    pub fn set_tree_hash(&mut self, tree_hash: Option<String>) -> &mut Self {
        self.tree_hash = tree_hash;
        self
    }

    /// Tree hash, if set.
    ///
    /// PHP: `ArcanistGitRawCommit::getTreeHash`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:138`).
    #[must_use]
    pub fn get_tree_hash(&self) -> Option<&str> {
        self.tree_hash.as_deref()
    }

    /// Set the raw `author` line value.
    ///
    /// PHP: `ArcanistGitRawCommit::setRawAuthor`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:142`).
    pub fn set_raw_author(&mut self, raw_author: Option<String>) -> &mut Self {
        self.raw_author = raw_author;
        self
    }

    /// Raw `author` line value, if set.
    ///
    /// PHP: `ArcanistGitRawCommit::getRawAuthor`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:146`).
    #[must_use]
    pub fn get_raw_author(&self) -> Option<&str> {
        self.raw_author.as_deref()
    }

    /// Set the raw `committer` line value.
    ///
    /// PHP: `ArcanistGitRawCommit::setRawCommitter`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:150`).
    pub fn set_raw_committer(&mut self, raw_committer: Option<String>) -> &mut Self {
        self.raw_committer = raw_committer;
        self
    }

    /// Raw `committer` line value, if set.
    ///
    /// PHP: `ArcanistGitRawCommit::getRawCommitter`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:154`).
    #[must_use]
    pub fn get_raw_committer(&self) -> Option<&str> {
        self.raw_committer.as_deref()
    }

    /// Replace the parent list.
    ///
    /// PHP: `ArcanistGitRawCommit::setParents`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:160`).
    pub fn set_parents(&mut self, parents: Vec<String>) -> &mut Self {
        self.parents = parents;
        self
    }

    /// Parent hashes.
    ///
    /// PHP: `ArcanistGitRawCommit::getParents`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:165`).
    #[must_use]
    pub fn get_parents(&self) -> &[String] {
        &self.parents
    }

    /// Append one parent hash.
    ///
    /// PHP: `ArcanistGitRawCommit::addParent`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:169`).
    pub fn add_parent(&mut self, hash: String) -> &mut Self {
        self.parents.push(hash);
        self
    }

    /// Set the commit message.
    ///
    /// PHP: `ArcanistGitRawCommit::setMessage`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:174`).
    pub fn set_message(&mut self, message: Option<String>) -> &mut Self {
        self.message = message;
        self
    }

    /// Commit message, if set.
    ///
    /// PHP: `ArcanistGitRawCommit::getMessage`
    /// (`src/repository/raw/ArcanistGitRawCommit.php:179`).
    #[must_use]
    pub fn get_message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// Parse `git --version` stdout into a bare version number.
///
/// PHP: `ArcanistGitAPI::getGitVersion`
/// (`src/repository/api/ArcanistGitAPI.php:53`).
///
/// Divergence: PHP caches the version per process; callers here re-query.
#[must_use]
pub fn parse_git_version(stdout: &str) -> String {
    stdout.replace("git version ", "").trim_end().to_string()
}

/// True when dotted `version` is at least `minimum`.
///
/// PHP: `version_compare($version, $min, '>=')` at
/// `src/repository/api/ArcanistGitAPI.php:709`,
/// `src/repository/api/ArcanistGitAPI.php:1319`,
/// `src/repository/api/ArcanistGitAPI.php:1605`, and
/// `src/repository/api/ArcanistGitAPI.php:1641`.
///
/// Divergence: only leading-digit numeric segments are compared;
/// PHP's pre-release ordering (`dev` < `alpha` < `beta` < `RC` < `pl`)
/// and `_`/`-`/`+` separators are not replicated. Git versions are
/// plain dotted numbers with optional trailing text, where both agree.
#[must_use]
pub fn version_at_least(version: &str, minimum: &str) -> bool {
    fn segments(text: &str) -> Vec<u64> {
        text.split('.')
            .map(|part| {
                let digits: String = part
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .map(char::from)
                    .collect();
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect()
    }
    let have = segments(version);
    let want = segments(minimum);
    for i in 0..have.len().max(want.len()) {
        let a = have.get(i).copied().unwrap_or(0);
        let b = want.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    true
}

/// Parse `git rev-parse --git-dir` stdout into a metadata directory.
///
/// PHP: `ArcanistGitAPI::getMetadataPath`
/// (`src/repository/api/ArcanistGitAPI.php:62`).
///
/// Divergence: PHP caches the path per process; callers here re-query.
#[must_use]
pub fn parse_metadata_path(stdout: &str, repo_path: &Path) -> PathBuf {
    let path = stdout.trim_end_matches('\n');
    if path == ".git" {
        repo_path.join(".git")
    } else {
        PathBuf::from(path)
    }
}

/// Strip the `refs/heads/` prefix from a branch ref.
///
/// PHP: `ArcanistGitAPI::getBranchNameFromRef`
/// (`src/repository/api/ArcanistGitAPI.php:508`).
#[must_use]
pub fn parse_branch_name_from_ref(ref_name: &str) -> Option<String> {
    let branch = ref_name.strip_prefix(UPSTREAM_LOCAL_PREFIX)?;
    if branch.is_empty() {
        return None;
    }
    Some(branch.to_string())
}

/// Interpret `git symbolic-ref --quiet HEAD` exit status plus output.
///
/// PHP: `ArcanistGitAPI::getBranchName`
/// (`src/repository/api/ArcanistGitAPI.php:522`): status 0 parses the
/// ref, status 1 means detached HEAD, anything else is an error.
pub fn branch_name_from_symbolic_ref(
    status: i32,
    stdout: &str,
    stderr: &str,
) -> Result<Option<String>, String> {
    if status == 0 {
        let branch = parse_branch_name_from_ref(stdout.trim_end());
        return branch
            .map(Some)
            .ok_or_else(|| "Failed to parse git symbolic-ref output!".to_string());
    }
    if status == 1 {
        return Ok(None);
    }
    Err(format!("Command git symbolic-ref failed: {stderr}"))
}

/// True when `child` strictly descends from `parent`.
///
/// PHP: `ArcanistGitAPI::isDescendant`
/// (`src/repository/api/ArcanistGitAPI.php:89`).
/// `merge_base_stdout` is the raw `git merge-base child parent` output.
#[must_use]
pub fn is_descendant_output(merge_base_stdout: &str, child: &str, parent: &str) -> bool {
    let common = merge_base_stdout.trim();
    common == parent && common != child
}

/// True when the commit range runs backward or is empty.
///
/// PHP: the `!isDescendant` check in
/// `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:139`).
#[must_use]
pub fn commit_range_is_reversed(
    merge_base_stdout: &str,
    resolved_head: &str,
    resolved_base: &str,
) -> bool {
    !is_descendant_output(merge_base_stdout, resolved_head, resolved_base)
}

/// Error for an empty, backward, or invalid commit range.
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:149`).
#[must_use]
pub fn empty_commit_range_message(base_commit: &str, head_commit: &str) -> String {
    format!(
        "The specified commit range is empty, backward or invalid: the base ({base_commit}) \
         is not an ancestor of the head ({head_commit}). You can not diff an empty or \
         reversed commit range."
    )
}

/// Error when no commit matches a symbolic name.
///
/// PHP: `ArcanistGitAPI::resolveCommit`
/// (`src/repository/api/ArcanistGitAPI.php:416`) and
/// `ArcanistGitAPI::buildBaseCommit`
/// (`src/repository/api/ArcanistGitAPI.php:221`).
#[must_use]
pub fn unknown_commit_message(symbolic_commit: &str) -> String {
    format!("Unable to find any git commit named '{symbolic_commit}' in this repository.")
}

/// Error when a relative commit names a non-commit object.
///
/// PHP: `ArcanistGitAPI::buildBaseCommit`
/// (`src/repository/api/ArcanistGitAPI.php:366`).
#[must_use]
pub fn relative_commit_not_commit_message(relative: &str) -> String {
    format!("Relative commit '{relative}' is not the name of a commit!")
}

/// Error when `git svn find-rev` finds nothing.
///
/// PHP: `ArcanistGitAPI::executeSVNFindRev`
/// (`src/repository/api/ArcanistGitAPI.php:620`).
#[must_use]
pub fn svn_find_rev_missing_message(input: &str, vcs: &str) -> String {
    format!("Cannot find the {vcs} equivalent of {input}.")
}

/// `git log` revision argument showing `head` excluding `base`.
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:159`).
#[must_use]
pub fn commit_range_against(head: &str, base: &str) -> String {
    format!("{head} --not {base}")
}

/// `--format` value listing local commits with `\x01` / `\x02` separators.
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:178`).
///
/// Divergence: the Windows `%C` escaping variant is not ported; arguments
/// never pass through a shell here, so `%` needs no special handling.
#[must_use]
pub fn log_local_commit_format() -> &'static str {
    "%H%x01%T%x01%P%x01%at%x01%an%x01%aE%x01%s%x01%s%n%n%b%x02"
}

/// Parse `git log --format=<log_local_commit_format>` stdout.
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:180`).
///
/// Divergence: records without exactly 8 fields are skipped; PHP would
/// read null fields. A literal `"0"` parent is kept, while PHP
/// `array_filter` would drop it; neither occurs in real git output.
#[must_use]
pub fn parse_local_commit_information(stdout: &str) -> Vec<LocalCommit> {
    let info = stdout.trim_matches([' ', '\n', '\u{2}']);
    if info.is_empty() {
        return Vec::new();
    }
    let mut commits = Vec::new();
    for record in info.split('\u{2}') {
        let fields: Vec<&str> = record.trim().splitn(8, '\u{1}').collect();
        if fields.len() != 8 {
            continue;
        }
        commits.push(LocalCommit {
            commit: fields[0].to_string(),
            tree: fields[1].to_string(),
            parents: fields[2]
                .split(' ')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
            time: fields[3].to_string(),
            author: fields[4].to_string(),
            summary: fields[6].to_string(),
            message: fields[7].trim_end().to_string(),
            author_email: fields[5].to_string(),
        });
    }
    commits
}

/// Diff flags shared by every git diff invocation.
///
/// PHP: `ArcanistGitAPI::getDiffBaseOptions`
/// (`src/repository/api/ArcanistGitAPI.php:442`).
#[must_use]
pub fn diff_base_options() -> Vec<&'static str> {
    vec!["--no-ext-diff", "--no-textconv", "--submodule=short"]
}

/// Full diff flags: base options plus color, prefixes, context lines,
/// and optionally rename/copy detection.
///
/// PHP: `ArcanistGitAPI::getDiffFullOptions`
/// (`src/repository/api/ArcanistGitAPI.php:425`).
#[must_use]
pub fn diff_full_options(context_lines: u32, detect_moves_and_renames: bool) -> Vec<String> {
    let mut options: Vec<String> = diff_base_options()
        .into_iter()
        .map(str::to_string)
        .collect();
    options.push("--no-color".to_string());
    options.push("--src-prefix=a/".to_string());
    options.push("--dst-prefix=b/".to_string());
    options.push(format!("-U{context_lines}"));
    if detect_moves_and_renames {
        options.push("-M".to_string());
        options.push("-C".to_string());
    }
    options
}

/// Parse `git diff --raw` stdout into path-to-mask flags.
///
/// PHP: `ArcanistGitAPI::parseGitRawDiff`
/// (`src/repository/api/ArcanistGitAPI.php:844`).
///
/// Divergence: lines with fewer than 6 fields, and rename/copy lines
/// without a tab-separated second path, are skipped; PHP would read
/// null fields. Neither occurs in real git output.
#[must_use]
pub fn parse_git_raw_diff(status: &str) -> BTreeMap<String, u32> {
    parse_git_raw_diff_full(status)
        .into_iter()
        .map(|(path, entry)| (path, entry.mask))
        .collect()
}

/// Parse `git diff --raw` stdout into path-to-mask-plus-ref rows.
///
/// PHP: `ArcanistGitAPI::parseGitRawDiff` with `$full = true`
/// (`src/repository/api/ArcanistGitAPI.php:844`).
/// Same skip Divergences as [`parse_git_raw_diff`].
#[must_use]
pub fn parse_git_raw_diff_full(status: &str) -> BTreeMap<String, RawDiffEntry> {
    const ZERO_REF: &str = "0000000000000000000000000000000000000000";
    let mut files: BTreeMap<String, RawDiffEntry> = BTreeMap::new();
    for line in status.trim().split('\n') {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.splitn(6, [' ', '\t']).collect();
        if fields.len() != 6 {
            continue;
        }
        let old_mode = fields[0].trim_start_matches(':');
        let new_mode = fields[1];
        let old_hash = fields[2].trim_end_matches('.');
        let new_hash = fields[3].trim_end_matches('.');
        let flag = fields[4];
        let file = fields[5];
        let new_value = u32::from_str_radix(new_mode, 8).unwrap_or(0);
        let is_submodule = new_value & 0o160000 == 0o160000;
        let mut mask = 0;
        if is_submodule && flag == "M" && old_hash == new_hash && old_mode == new_mode {
            // T9455: submodule reports modified, but nothing changed from
            // the parent view, so the change is inside the submodule.
            mask |= FLAG_EXTERNALS;
        } else if flag == "A" {
            mask |= FLAG_ADDED;
        } else if flag == "M" {
            mask |= FLAG_MODIFIED;
        } else if flag == "D" {
            mask |= FLAG_DELETED;
        } else if flag.starts_with('R') {
            let Some((old_path, new_path)) = file.split_once('\t') else {
                continue;
            };
            files.insert(
                old_path.to_string(),
                RawDiffEntry {
                    mask: mask | FLAG_DELETED,
                    r#ref: ZERO_REF.to_string(),
                },
            );
            files.insert(
                new_path.to_string(),
                RawDiffEntry {
                    mask: mask | FLAG_ADDED,
                    r#ref: new_hash.to_string(),
                },
            );
            continue;
        } else if flag.starts_with('C') {
            let Some((_, new_path)) = file.split_once('\t') else {
                continue;
            };
            files.insert(
                new_path.to_string(),
                RawDiffEntry {
                    mask: mask | FLAG_ADDED,
                    r#ref: new_hash.to_string(),
                },
            );
            continue;
        }
        files.insert(
            file.to_string(),
            RawDiffEntry {
                mask,
                r#ref: new_hash.to_string(),
            },
        );
    }
    files
}

/// Parse `git status --porcelain=2 -z` stdout into path-to-mask flags.
///
/// PHP: `ArcanistGitAPI::buildUncommittedStatusViaStatus`
/// (`src/repository/api/ArcanistGitAPI.php:645`).
/// Results are key-sorted, matching the `ksort` in
/// `ArcanistRepositoryAPI::getUncommittedStatus`
/// (`src/repository/api/ArcanistRepositoryAPI.php:148`).
///
/// Divergence: entries with an unknown ord (`1`, `2`, `u`, `?` are
/// known), truncated field lists, or a rename without its recorded
/// source path are skipped; PHP would read unset fields. Paths are
/// lossily decoded; non-UTF-8 bytes become U+FFFD.
#[must_use]
pub fn parse_porcelain_v2_status(stdout: &[u8]) -> BTreeMap<String, u32> {
    let parts: Vec<&[u8]> = stdout.split(|b| *b == 0).collect();
    let mut result: BTreeMap<String, u32> = BTreeMap::new();
    let mut rest = parts.as_slice();
    let ord_field = |entry: &[u8], count: usize| -> Option<Vec<String>> {
        let text = String::from_utf8_lossy(entry).into_owned();
        let fields: Vec<String> = text.splitn(count, ' ').map(str::to_string).collect();
        if fields.len() != count {
            return None;
        }
        Some(fields)
    };
    while rest.len() > 1 {
        let entry = rest[0];
        rest = &rest[1..];
        let head_text = String::from_utf8_lossy(entry).into_owned();
        let head: Vec<&str> = head_text.splitn(2, ' ').collect();
        if head.len() != 2 {
            continue;
        }
        if head[0] == "?" {
            result.insert(head[1].to_string(), FLAG_UNTRACKED);
            continue;
        }
        let (fields, path) = match head[0] {
            "1" => {
                let Some(fields) = ord_field(entry, 9) else {
                    continue;
                };
                let path = fields[8].clone();
                (fields, path)
            }
            "2" => {
                let Some(fields) = ord_field(entry, 10) else {
                    continue;
                };
                let path = fields[9].clone();
                (fields, path)
            }
            "u" => {
                let Some(fields) = ord_field(entry, 11) else {
                    continue;
                };
                let path = fields[10].clone();
                (fields, path)
            }
            _ => continue,
        };
        let xy = fields[1].as_bytes();
        let index_state = xy.first().copied().unwrap_or(b'.');
        let working_state = xy.get(1).copied().unwrap_or(b'.');
        let mut mask = FLAG_UNCOMMITTED;
        if index_state == b'A' {
            mask |= FLAG_ADDED;
        } else if index_state == b'M' {
            mask |= FLAG_MODIFIED;
        } else if index_state == b'D' {
            mask |= FLAG_DELETED;
        }
        if working_state != b'.' {
            mask |= FLAG_UNSTAGED;
            if index_state == b'.' {
                if working_state == b'A' {
                    mask |= FLAG_ADDED;
                } else if working_state == b'M' {
                    mask |= FLAG_MODIFIED;
                } else if working_state == b'D' {
                    mask |= FLAG_DELETED;
                }
            }
        }
        let submodule = fields[2].as_bytes();
        if submodule.get(2) == Some(&b'M') || submodule.get(3) == Some(&b'U') {
            mask |= FLAG_EXTERNALS;
        }
        if head[0] == "2" {
            if rest.is_empty() {
                continue;
            }
            let orig = String::from_utf8_lossy(rest[0]).into_owned();
            rest = &rest[1..];
            let current = result.get(&path).copied().unwrap_or(0) | mask;
            result.insert(path.clone(), current);
            result.insert(orig, current | FLAG_DELETED);
            result.insert(path, current | FLAG_ADDED);
        } else {
            *result.entry(path).or_default() |= mask;
        }
    }
    result
}

/// Parse `git ls-tree` stdout into path-to-entry rows.
///
/// PHP: `ArcanistGitAPI::parseGitTree`
/// (`src/repository/api/ArcanistGitAPI.php:995`).
pub fn parse_git_tree(stdout: &str) -> Result<BTreeMap<String, TreeEntry>, String> {
    let mut result = BTreeMap::new();
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(result);
    }
    for line in trimmed.split('\n') {
        let valid: Option<(String, TreeEntry)> = (|| {
            let (meta, name) = line.split_once('\t')?;
            let mut meta_parts = meta.split(' ');
            let (mode, kind, hash) = (meta_parts.next()?, meta_parts.next()?, meta_parts.next()?);
            if meta_parts.next().is_some() {
                return None;
            }
            if mode.len() != 6 || !mode.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            if kind != "blob" && kind != "tree" && kind != "commit" {
                return None;
            }
            if hash.len() != 40
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            {
                return None;
            }
            Some((
                name.to_string(),
                TreeEntry {
                    mode: mode.to_string(),
                    r#type: kind.to_string(),
                    r#ref: hash.to_string(),
                },
            ))
        })();
        let Some((name, entry)) = valid else {
            return Err("Failed to parse git ls-tree output!".to_string());
        };
        result.insert(name, entry);
    }
    Ok(result)
}

/// `--format` value listing every local branch for [`parse_all_branches`].
///
/// PHP: field list built by `ArcanistGitAPI::getAllBranches`
/// (`src/repository/api/ArcanistGitAPI.php:1062`).
#[must_use]
pub fn all_branches_format() -> String {
    [
        "%(refname)",
        "%(objectname)",
        "%(committerdate:raw)",
        "%(tree)",
        "%(subject)",
        "%(subject)%0a%0a%(body)",
        "%02",
    ]
    .join("%01")
}

/// Parse `git for-each-ref --format=<all_branches_format> -- refs/heads`.
///
/// PHP: `ArcanistGitAPI::getAllBranches`
/// (`src/repository/api/ArcanistGitAPI.php:1072`).
/// `current` is the checked-out branch name, if any.
///
/// Divergence: records without exactly 6 fields are skipped; PHP would
/// read null fields. Epochs use the leading integer only, matching the
/// PHP `(int)` cast on `committerdate:raw`.
#[must_use]
pub fn parse_all_branches(stdout: &str, current: Option<&str>) -> Vec<BranchInfo> {
    let mut branches = Vec::new();
    for record in stdout.split('\u{2}') {
        let line = record.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.splitn(6, '\u{1}').collect();
        if fields.len() != 6 {
            continue;
        }
        let branch = parse_branch_name_from_ref(fields[0]);
        let Some(branch) = branch else {
            continue;
        };
        let epoch = fields[2]
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<i64>().ok())
            .unwrap_or(0);
        branches.push(BranchInfo {
            current: Some(branch.as_str()) == current,
            name: branch,
            r#ref: fields[0].to_string(),
            hash: fields[1].to_string(),
            tree: fields[3].to_string(),
            epoch,
            desc: fields[4].to_string(),
            text: fields[5].to_string(),
        });
    }
    branches
}

/// Parse `git blame --porcelain -w -M <base> -- <path>` stdout.
///
/// PHP: `ArcanistGitAPI::getBlame`
/// (`src/repository/api/ArcanistGitAPI.php:945`).
/// Lines from the boundary commit are dropped; commit metadata is
/// cached per hash like the PHP `$revision_data` map.
pub fn parse_blame(stdout: &str) -> Result<Vec<BlameLine>, String> {
    let text = stdout.trim_end();
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut rest = text;
    while let Some(pos) = rest.find('\n') {
        let line = &rest[..pos + 1];
        rest = &rest[pos + 1..];
        if line.starts_with('\t') {
            chunks.push(std::mem::take(&mut current));
        } else {
            current.push_str(line);
        }
    }
    current.push_str(rest);
    chunks.push(current);

    let mut cache: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut blame = Vec::new();
    for chunk in &chunks {
        let revision =
            String::from_utf8_lossy(chunk.as_bytes().get(..40).unwrap_or(&[])).into_owned();
        let data = match cache.get(&revision) {
            Some(data) => data.clone(),
            None => {
                let Some(author) = chunk.lines().find_map(|line| line.strip_prefix("author "))
                else {
                    return Err(format!(
                        "Unexpected output from git blame: no author for commit {revision}"
                    ));
                };
                let boundary = chunk.lines().any(|line| line == "boundary");
                cache.insert(revision.clone(), (author.to_string(), boundary));
                (author.to_string(), boundary)
            }
        };
        if !data.1 {
            blame.push(BlameLine {
                author: data.0,
                revision,
            });
        }
    }
    Ok(blame)
}

/// Interpret `git config <key>` exit status plus output.
///
/// PHP: `ArcanistGitAPI::getGitConfig`
/// (`src/repository/api/ArcanistGitAPI.php:792`).
/// Returns `None` on failure; the caller substitutes the default.
#[must_use]
pub fn config_value_from_output(success: bool, stdout: &str) -> Option<String> {
    if !success {
        return None;
    }
    Some(stdout.trim_end().to_string())
}

/// Parse `git var GIT_AUTHOR_IDENT` stdout into an author name.
///
/// PHP: `ArcanistGitAPI::getAuthor`
/// (`src/repository/api/ArcanistGitAPI.php:800`):
/// trailing newlines are trimmed, then `/\s+<.*/` is stripped.
#[must_use]
pub fn parse_author(stdout: &str) -> String {
    let text = stdout.trim_end_matches('\n');
    let mut cut = text.len();
    let mut prev_is_space = false;
    for (i, c) in text.char_indices() {
        if c == '<' && prev_is_space {
            cut = text[..i].trim_end().len();
            break;
        }
        prev_is_space = c.is_whitespace();
    }
    text[..cut].to_string()
}

/// Commit summary for `git log -n 1 --format=%s` stdout.
///
/// PHP: `ArcanistGitAPI::getCommitSummary`
/// (`src/repository/api/ArcanistGitAPI.php:1248`): the empty tree
/// reports `(The Empty Tree)`; anything else trims the log output.
#[must_use]
pub fn parse_commit_summary(commit: &str, log_stdout: &str) -> String {
    if commit == GIT_MAGIC_ROOT_COMMIT {
        return "(The Empty Tree)".to_string();
    }
    log_stdout.trim().to_string()
}

/// Svn revision number when `name` ends in `@<digits>`.
///
/// PHP: `ArcanistGitAPI::getCanonicalRevisionName`
/// (`src/repository/api/ArcanistGitAPI.php:603`): `/@([0-9]+)$/`.
#[must_use]
pub fn svn_revision_suffix(name: &str) -> Option<String> {
    let (_, suffix) = name.rsplit_once('@')?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(suffix.to_string())
}

/// Parse `git svn find-rev` stdout into the rebuilt final hash.
///
/// PHP: `ArcanistGitAPI::executeSVNFindRev`
/// (`src/repository/api/ArcanistGitAPI.php:615`): git may print a
/// partial rebuild, so the second-to-last line wins. Empty output and
/// single-line output yield `None` (PHP throws); the caller maps that
/// to [`svn_find_rev_missing_message`].
#[must_use]
pub fn parse_svn_find_rev(stdout: &str) -> Option<String> {
    if stdout.is_empty() {
        return None;
    }
    let lines: Vec<&str> = stdout.split('\n').collect();
    if lines.len() < 2 {
        return None;
    }
    Some(lines[lines.len() - 2].trim_end().to_string())
}

/// Split a base-commit rule into `type:name`.
///
/// PHP: `ArcanistGitAPI::resolveBaseCommitRule`
/// (`src/repository/api/ArcanistGitAPI.php:1266`): `explode(':', $rule, 2)`.
/// Rules without `:` fall through to the PHP `default` case and resolve
/// to nothing, so this returns `None` for them.
#[must_use]
pub fn parse_base_commit_rule(rule: &str) -> Option<(String, String)> {
    let (kind, name) = rule.split_once(':')?;
    Some((kind.to_string(), name.to_string()))
}

/// Normalize one `git branch --contains` line to a known branch.
///
/// PHP: the filter loop in `ArcanistGitAPI::resolveBaseCommitRule`
/// (`src/repository/api/ArcanistGitAPI.php:1332`): strip the `*`
/// current marker and surrounding spaces, then drop anything that is
/// not a known branch (detached-HEAD and `no branch` lines).
#[must_use]
pub fn filter_branch_contains_line(line: &str, known_branches: &[&str]) -> Option<String> {
    let branch = line.trim_matches([' ', '*']);
    if known_branches.contains(&branch) {
        Some(branch.to_string())
    } else {
        None
    }
}

/// Normalize one raw remote-URI command output.
///
/// PHP: `ArcanistGitAPI::getGitRemoteURI`
/// (`src/repository/api/ArcanistGitAPI.php:1560`):
/// trailing whitespace is trimmed and empty output means no URI.
#[must_use]
pub fn normalize_remote_uri(stdout: &str) -> Option<String> {
    let uri = stdout.trim_end();
    if uri.is_empty() {
        None
    } else {
        Some(uri.to_string())
    }
}

/// Interpret `git ls-remote --get-url <remote>` exit status plus output.
///
/// PHP: `ArcanistGitAPI::loadGitRemoteURI`
/// (`src/repository/api/ArcanistGitAPI.php:1629`): the command echoes
/// the remote name when no URI exists, which counts as failure.
#[must_use]
pub fn remote_uri_from_ls_remote(success: bool, stdout: &str, remote: &str) -> Option<String> {
    if !success {
        return None;
    }
    if stdout.trim_end() == remote {
        return None;
    }
    normalize_remote_uri(stdout)
}

/// True when `git ls-files -z -- ':(attr:filter=lfs)'` output proves an
/// LFS working copy: success plus any output.
///
/// PHP: `ArcanistGitAPI::isGitLFSWorkingCopy`
/// (`src/repository/api/ArcanistGitAPI.php:1666`).
#[must_use]
pub fn lfs_output_indicates_lfs(success: bool, stdout: &[u8]) -> bool {
    success && !stdout.is_empty()
}

/// Bare git version number for `dir`.
///
/// PHP: `ArcanistGitAPI::getGitVersion`
/// (`src/repository/api/ArcanistGitAPI.php:53`).
pub fn git_version(dir: &Path) -> Result<String, String> {
    stdout_text(dir, &["--version"]).map(|text| parse_git_version(&text))
}

/// Metadata (`.git`) directory for `dir`.
///
/// PHP: `ArcanistGitAPI::getMetadataPath`
/// (`src/repository/api/ArcanistGitAPI.php:62`).
pub fn metadata_path(dir: &Path) -> Result<PathBuf, String> {
    let out = output(dir, &["rev-parse", "--git-dir"])?;
    if !out.status.success() {
        return Err("git rev-parse --git-dir failed".to_string());
    }
    Ok(parse_metadata_path(
        &String::from_utf8_lossy(&out.stdout),
        dir,
    ))
}

/// Checked-out branch name, or `None` on a detached HEAD.
///
/// PHP: `ArcanistGitAPI::getBranchName`
/// (`src/repository/api/ArcanistGitAPI.php:522`).
pub fn branch_name(dir: &Path) -> Result<Option<String>, String> {
    let out = output(dir, &["symbolic-ref", "--quiet", "HEAD"])?;
    let status = out.status.code().unwrap_or(-1);
    branch_name_from_symbolic_ref(
        status,
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// Merge-base of two commits.
///
/// PHP: `git merge-base -- %s %s` as used by
/// `ArcanistGitAPI::buildBaseCommit`
/// (`src/repository/api/ArcanistGitAPI.php:216`).
pub fn merge_base(dir: &Path, a: &str, b: &str) -> Result<String, String> {
    stdout_text(dir, &["merge-base", "--", a, b])
}

/// True when `child` strictly descends from `parent`.
///
/// PHP: `ArcanistGitAPI::isDescendant`
/// (`src/repository/api/ArcanistGitAPI.php:89`).
pub fn is_descendant(dir: &Path, child: &str, parent: &str) -> Result<bool, String> {
    let out = output(dir, &["merge-base", "--", child, parent])?;
    if !out.status.success() {
        return Err(unknown_commit_message(&format!("{child} or {parent}")));
    }
    Ok(is_descendant_output(
        &String::from_utf8_lossy(&out.stdout),
        child,
        parent,
    ))
}

/// Translate a symbolic commit (like `HEAD^`) to a commit hash.
///
/// PHP: `ArcanistGitAPI::resolveCommit`
/// (`src/repository/api/ArcanistGitAPI.php:410`).
pub fn resolve_commit(dir: &Path, symbolic_commit: &str) -> Result<String, String> {
    let out = output(dir, &["rev-parse", symbolic_commit])?;
    if !out.status.success() {
        return Err(unknown_commit_message(symbolic_commit));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Hash of `HEAD`.
///
/// PHP: `ArcanistGitAPI::getHeadCommit` with no symbolic head set
/// (`src/repository/api/ArcanistGitAPI.php:390`).
///
/// Divergence: the PHP symbolic-head override (`setHeadCommit`) is
/// instance state and is not ported; pass another revision to
/// [`resolve_commit`] instead.
pub fn head_commit(dir: &Path) -> Result<String, String> {
    resolve_commit(dir, current_commit_symbol())
}

/// Resolve an explicitly specified base commit to a hash.
///
/// PHP: the explicit-symbol branch of `ArcanistGitAPI::buildBaseCommit`
/// (`src/repository/api/ArcanistGitAPI.php:208`): the empty tree passes
/// through, anything else resolves to its merge-base with `head`.
///
/// Divergence: base-commit explanations are instance state and are not
/// ported. The zero/one-commit detection, `base` configuration rules,
/// upstream default, scratch file, and interactive prompt
/// (`src/repository/api/ArcanistGitAPI.php:245`) are not ported: the
/// caller supplies the explicit symbol (or the empty tree) instead.
pub fn base_commit_for_explicit_symbol(
    dir: &Path,
    symbolic_commit: &str,
    head: &str,
) -> Result<String, String> {
    if symbolic_commit == GIT_MAGIC_ROOT_COMMIT {
        return Ok(GIT_MAGIC_ROOT_COMMIT.to_string());
    }
    let out = output(dir, &["merge-base", "--", symbolic_commit, head])?;
    if !out.status.success() {
        return Err(unknown_commit_message(symbolic_commit));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Follow tracking branches from `start` up to a remote or a cycle.
///
/// PHP: `ArcanistGitAPI::getPathToUpstream`
/// (`src/repository/api/ArcanistGitAPI.php:1458`).
pub fn get_path_to_upstream(dir: &Path, start: &str) -> Result<UpstreamPath, String> {
    let mut path = UpstreamPath::new();
    let mut cursor = start.to_string();
    loop {
        let query = format!("{cursor}@{{upstream}}");
        let out = output(dir, &["rev-parse", "--symbolic-full-name", &query])?;
        if !out.status.success() {
            break;
        }
        let upstream = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match classify_upstream_ref(&upstream)? {
            UpstreamRef::LocalBranch(name) => {
                let is_cycle = path.get_upstream(&name).is_some();
                path.add_upstream(cursor, UpstreamHop::local(name.clone(), is_cycle));
                if is_cycle {
                    break;
                }
                cursor = name;
            }
            UpstreamRef::RemoteBranch { remote, branch } => {
                path.add_upstream(cursor, UpstreamHop::remote(branch, remote));
                break;
            }
        }
    }
    Ok(path)
}

/// Load one remote URI with the PHP version-gated fallback chain.
///
/// PHP: `ArcanistGitAPI::loadGitRemoteURI`
/// (`src/repository/api/ArcanistGitAPI.php:1573`): `remote get-url`
/// first, then (below git 2.7) `ls-remote --get-url`, then (below git
/// 1.7.5) `config remote.<name>.url`. Returns the raw stdout; the
/// caller normalizes it.
fn load_git_remote_uri(dir: &Path, remote_name: &str, for_push: bool) -> Option<String> {
    let mut args: Vec<&str> = vec!["remote", "get-url"];
    if for_push {
        args.push("--push");
    }
    args.push("--");
    args.push(remote_name);
    if let Ok(out) = output(dir, &args)
        && out.status.success()
    {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let version = git_version(dir).unwrap_or_default();
    if version_at_least(&version, "2.7") {
        return None;
    }
    if let Ok(out) = output(dir, &["ls-remote", "--get-url", remote_name]) {
        let success = out.status.success();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if let Some(uri) = remote_uri_from_ls_remote(success, &text, remote_name) {
            return Some(uri);
        }
    }
    if version_at_least(&version, "1.7.5") {
        return None;
    }
    let key = format!("remote.{remote_name}.url");
    if let Ok(out) = output(dir, &["config", "--", &key])
        && out.status.success()
    {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    None
}

/// Fetch URI for `remote_name`, if the remote exists.
///
/// PHP: `ArcanistGitAPI::getGitRemoteFetchURI`
/// (`src/repository/api/ArcanistGitAPI.php:1552`).
#[must_use]
pub fn remote_fetch_uri(dir: &Path, remote_name: &str) -> Option<String> {
    load_git_remote_uri(dir, remote_name, false).and_then(|raw| normalize_remote_uri(&raw))
}

/// Push URI for `remote_name`, if the remote exists.
///
/// PHP: `ArcanistGitAPI::getGitRemotePushURI`
/// (`src/repository/api/ArcanistGitAPI.php:1556`).
#[must_use]
pub fn remote_push_uri(dir: &Path, remote_name: &str) -> Option<String> {
    load_git_remote_uri(dir, remote_name, true).and_then(|raw| normalize_remote_uri(&raw))
}

/// True when `remote_name` has a push URI.
///
/// PHP: `ArcanistGitAPI::isPushableRemote`
/// (`src/repository/api/ArcanistGitAPI.php:1542`).
#[must_use]
pub fn is_pushable_remote(dir: &Path, remote_name: &str) -> bool {
    remote_push_uri(dir, remote_name).is_some()
}

/// True when `remote_name` has a fetch URI.
///
/// PHP: `ArcanistGitAPI::isFetchableRemote`
/// (`src/repository/api/ArcanistGitAPI.php:1547`).
#[must_use]
pub fn is_fetchable_remote(dir: &Path, remote_name: &str) -> bool {
    remote_fetch_uri(dir, remote_name).is_some()
}

/// Fetch URI of the remote the current branch tracks (`origin` by default).
///
/// PHP: `ArcanistGitAPI::getRemoteURI`
/// (`src/repository/api/ArcanistGitAPI.php:545`).
pub fn remote_uri(dir: &Path) -> Result<Option<String>, String> {
    let mut remote = "origin".to_string();
    if let Some(branch) = branch_name(dir)? {
        let path = get_path_to_upstream(dir, &branch)?;
        if path.is_connected_to_remote()
            && let Some(name) = path.get_remote_remote_name()
        {
            remote = name.to_string();
        }
    }
    Ok(remote_fetch_uri(dir, &remote))
}

/// Git config value for `key`, or `default` when unset.
///
/// PHP: `ArcanistGitAPI::getGitConfig`
/// (`src/repository/api/ArcanistGitAPI.php:792`).
#[must_use]
pub fn git_config_or(dir: &Path, key: &str, default: Option<&str>) -> Option<String> {
    git_config(dir, key).or_else(|| default.map(str::to_string))
}

/// Git config value for `key`, or `None` when unset.
///
/// PHP: `ArcanistGitAPI::getGitConfig`
/// (`src/repository/api/ArcanistGitAPI.php:792`).
#[must_use]
pub fn git_config(dir: &Path, key: &str) -> Option<String> {
    let out = output(dir, &["config", key]).ok()?;
    config_value_from_output(out.status.success(), &String::from_utf8_lossy(&out.stdout))
}

/// Author name from the git identity.
///
/// PHP: `ArcanistGitAPI::getAuthor`
/// (`src/repository/api/ArcanistGitAPI.php:800`).
pub fn author(dir: &Path) -> Result<String, String> {
    let out = output(dir, &["var", "GIT_AUTHOR_IDENT"])?;
    if !out.status.success() {
        return Err("git var GIT_AUTHOR_IDENT failed".to_string());
    }
    Ok(parse_author(&String::from_utf8_lossy(&out.stdout)))
}

/// Full message (`subject + body`) of `commit`.
///
/// PHP: `ArcanistGitAPI::getCommitMessage`
/// (`src/repository/api/ArcanistGitAPI.php:1172`).
pub fn commit_message(dir: &Path, commit: &str) -> Result<String, String> {
    stdout_text(dir, &["log", "-n1", "--format=%s%n%n%b", commit, "--"])
}

/// Subject line of `commit`.
///
/// PHP: `ArcanistGitAPI::getCommitSummary`
/// (`src/repository/api/ArcanistGitAPI.php:1248`).
pub fn commit_summary(dir: &Path, commit: &str) -> Result<String, String> {
    if commit == GIT_MAGIC_ROOT_COMMIT {
        return Ok(parse_commit_summary(commit, ""));
    }
    let text = stdout_text(dir, &["log", "-n", "1", "--format=%s", commit, "--"])?;
    Ok(parse_commit_summary(commit, &text))
}

/// Hash of `HEAD`.
///
/// PHP: `ArcanistGitAPI::getWorkingCopyRevision`
/// (`src/repository/api/ArcanistGitAPI.php:1123`).
pub fn working_copy_revision(dir: &Path) -> Result<String, String> {
    stdout_text(dir, &["rev-parse", "HEAD"])
}

/// Canonical hash for a revision name, resolving `@<svn rev>` via git-svn.
///
/// PHP: `ArcanistGitAPI::getCanonicalRevisionName`
/// (`src/repository/api/ArcanistGitAPI.php:600`).
pub fn canonical_revision_name(dir: &Path, name: &str) -> Result<String, String> {
    if let Some(number) = svn_revision_suffix(name) {
        return hash_from_svn_revision_number(dir, &number);
    }
    stdout_text(dir, &["show", "-s", "--format=%H", name, "--"])
}

/// Run `git svn find-rev` and parse the rebuilt final hash.
///
/// PHP: `ArcanistGitAPI::executeSVNFindRev`
/// (`src/repository/api/ArcanistGitAPI.php:615`).
pub fn execute_svn_find_rev(dir: &Path, input: &str, vcs: &str) -> Result<String, String> {
    let out = output(dir, &["svn", "find-rev", input])?;
    if !out.status.success() {
        return Err(svn_find_rev_missing_message(input, vcs));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    parse_svn_find_rev(&text).ok_or_else(|| svn_find_rev_missing_message(input, vcs))
}

/// Git hash for Subversion revision `revision_id`.
///
/// PHP: `ArcanistGitAPI::getHashFromFromSVNRevisionNumber`
/// (`src/repository/api/ArcanistGitAPI.php:635`).
pub fn hash_from_svn_revision_number(dir: &Path, revision_id: &str) -> Result<String, String> {
    execute_svn_find_rev(dir, &format!("r{revision_id}"), "Git")
}

/// Subversion revision number for git hash `hash`.
///
/// PHP: `ArcanistGitAPI::getSVNRevisionNumberFromHash`
/// (`src/repository/api/ArcanistGitAPI.php:641`).
pub fn svn_revision_number_from_hash(dir: &Path, hash: &str) -> Result<String, String> {
    execute_svn_find_rev(dir, hash, "SVN")
}

/// True when `commit` resolves locally.
///
/// PHP: `ArcanistGitAPI::hasLocalCommit`
/// (`src/repository/api/ArcanistGitAPI.php:1144`).
#[must_use]
pub fn has_local_commit(dir: &Path, commit: Option<&str>) -> bool {
    let Some(commit) = commit else {
        return false;
    };
    canonical_revision_name(dir, commit).is_ok_and(|hash| !hash.is_empty())
}

/// Uncommitted working-copy status: path-to-mask flags.
///
/// PHP: `ArcanistGitAPI::buildUncommittedStatusViaStatus`
/// (`src/repository/api/ArcanistGitAPI.php:645`).
///
/// Divergence: the pre-2.11 fallback (`diff --raw` plus `ls-files`
/// plus `diff-files`, `src/repository/api/ArcanistGitAPI.php:708`)
/// is not ported; git 2.11 is from 2016 and porcelain v2 is assumed.
pub fn uncommitted_status(dir: &Path) -> Result<BTreeMap<String, u32>, String> {
    let out = output(dir, &["status", "--porcelain=2", "-z"])?;
    if !out.status.success() {
        return Err("git status --porcelain=2 failed".to_string());
    }
    Ok(parse_porcelain_v2_status(&out.stdout))
}

/// Status of the commit range: files changed between `base` and HEAD.
///
/// PHP: `ArcanistGitAPI::buildCommitRangeStatus`
/// (`src/repository/api/ArcanistGitAPI.php:783`).
pub fn commit_range_status(dir: &Path, base: &str) -> Result<BTreeMap<String, u32>, String> {
    let out = output(
        dir,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--submodule=short",
            "--raw",
            base,
            "HEAD",
            "--",
        ],
    )?;
    if !out.status.success() {
        return Err("git diff --raw failed".to_string());
    }
    Ok(parse_git_raw_diff(&String::from_utf8_lossy(&out.stdout)))
}

/// Files changed since `since_commit`: path-to-mask flags.
///
/// PHP: `ArcanistGitAPI::getChangedFiles`
/// (`src/repository/api/ArcanistGitAPI.php:938`).
pub fn changed_files(dir: &Path, since_commit: &str) -> Result<BTreeMap<String, u32>, String> {
    let out = output(dir, &["diff", "--raw", since_commit, "--"])?;
    if !out.status.success() {
        return Err("git diff --raw failed".to_string());
    }
    Ok(parse_git_raw_diff(&String::from_utf8_lossy(&out.stdout)))
}

/// Full unified diff of `base` against `head` (working copy when `None`).
///
/// PHP: `ArcanistGitAPI::getFullGitDiff`
/// (`src/repository/api/ArcanistGitAPI.php:464`), including the
/// `diff.suppressBlankEmpty=false` override (T13432).
/// `context_lines` is the PHP `getDiffLinesOfContext` value.
pub fn full_git_diff(
    dir: &Path,
    base: &str,
    head: Option<&str>,
    context_lines: u32,
) -> Result<String, String> {
    let mut args: Vec<String> = vec![
        "-c".to_string(),
        "diff.suppressBlankEmpty=false".to_string(),
        "diff".to_string(),
    ];
    args.extend(diff_full_options(context_lines, true));
    args.push(base.to_string());
    if let Some(head) = head {
        args.push(head.to_string());
    }
    args.push("--".to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    stdout_text(dir, &refs)
}

/// Unified diff of `base` against the working copy for one `path`.
///
/// PHP: `ArcanistGitAPI::getRawDiffText`
/// (`src/repository/api/ArcanistGitAPI.php:499`).
pub fn raw_diff_text(
    dir: &Path,
    base: &str,
    path: &str,
    context_lines: u32,
    detect_moves_and_renames: bool,
) -> Result<String, String> {
    let mut args: Vec<String> = vec!["diff".to_string()];
    args.extend(diff_full_options(context_lines, detect_moves_and_renames));
    args.push(base.to_string());
    args.push("--".to_string());
    args.push(path.to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    stdout_text(dir, &refs)
}

/// Medium-format log of the commit range.
///
/// PHP: `ArcanistGitAPI::getGitCommitLog`
/// (`src/repository/api/ArcanistGitAPI.php:564`):
/// empty when the repository has no commits, `HEAD` when the base is
/// the empty tree, else first-parent `base..head`.
pub fn git_commit_log(
    dir: &Path,
    has_commits: bool,
    base: &str,
    head: &str,
) -> Result<String, String> {
    if !has_commits {
        return Ok(String::new());
    }
    if base == GIT_MAGIC_ROOT_COMMIT {
        return stdout_text(dir, &["log", "--format=medium", "HEAD", "--"]);
    }
    let range = format!("{base}..{head}");
    stdout_text(
        dir,
        &["log", "--first-parent", "--format=medium", &range, "--"],
    )
}

/// Medium-format log of the last parents before `base`.
///
/// PHP: `ArcanistGitAPI::getGitHistoryLog`
/// (`src/repository/api/ArcanistGitAPI.php:585`).
pub fn git_history_log(dir: &Path, base: &str) -> Result<String, String> {
    let count = format!("-n{}", SEARCH_LENGTH_FOR_PARENT_REVISIONS);
    stdout_text(dir, &["log", "--format=medium", &count, base, "--"])
}

/// Blame of `path` at `base`: one row per line.
///
/// PHP: `ArcanistGitAPI::getBlame`
/// (`src/repository/api/ArcanistGitAPI.php:945`).
pub fn blame(dir: &Path, base: &str, path: &str) -> Result<Vec<BlameLine>, String> {
    let text = stdout_text(dir, &["blame", "--porcelain", "-w", "-M", base, "--", path])?;
    parse_blame(&text)
}

/// File bytes at `revision`, or `None` when the path is missing, empty,
/// a directory, or not a blob.
///
/// PHP: `ArcanistGitAPI::getFileDataAtRevision`
/// (`src/repository/api/ArcanistGitAPI.php:1022`).
pub fn file_data_at_revision(
    dir: &Path,
    path: &str,
    revision: &str,
) -> Result<Option<Vec<u8>>, String> {
    if path.is_empty() {
        return Ok(None);
    }
    let listing = stdout_text(dir, &["ls-tree", revision, "--", path])?;
    let tree = parse_git_tree(&listing)?;
    let Some(entry) = tree.get(path) else {
        return Ok(None);
    };
    if entry.r#type != "blob" {
        return Ok(None);
    }
    let out = output(dir, &["cat-file", "blob", "--", &entry.r#ref])?;
    if !out.status.success() {
        return Err("git cat-file blob failed".to_string());
    }
    Ok(Some(out.stdout))
}

/// File bytes at the base commit.
///
/// PHP: `ArcanistGitAPI::getOriginalFileData`
/// (`src/repository/api/ArcanistGitAPI.php:987`).
pub fn original_file_data(dir: &Path, path: &str, base: &str) -> Result<Option<Vec<u8>>, String> {
    file_data_at_revision(dir, path, base)
}

/// File bytes at HEAD.
///
/// PHP: `ArcanistGitAPI::getCurrentFileData`
/// (`src/repository/api/ArcanistGitAPI.php:991`).
pub fn current_file_data(dir: &Path, path: &str) -> Result<Option<Vec<u8>>, String> {
    file_data_at_revision(dir, path, current_commit_symbol())
}

/// Every local branch with hash, tree, epoch, and subject.
///
/// PHP: `ArcanistGitAPI::getAllBranches`
/// (`src/repository/api/ArcanistGitAPI.php:1061`).
pub fn all_branches(dir: &Path) -> Result<Vec<BranchInfo>, String> {
    let format = all_branches_format();
    let current = branch_name(dir)?;
    let out = output(
        dir,
        &["for-each-ref", "--format", &format, "--", "refs/heads"],
    )?;
    if !out.status.success() {
        return Err("git for-each-ref failed".to_string());
    }
    Ok(parse_all_branches(
        &String::from_utf8_lossy(&out.stdout),
        current.as_deref(),
    ))
}

/// Every tracked path in the working copy.
///
/// PHP: `ArcanistGitAPI::getAllFiles`
/// (`src/repository/api/ArcanistGitAPI.php:932`).
///
/// Divergence: PHP streams paths via `LinesOfALargeExecFuture`; this
/// reads them all into memory.
pub fn all_files(dir: &Path) -> Result<Vec<String>, String> {
    let out = output(dir, &["ls-files", "-z"])?;
    if !out.status.success() {
        return Err("git ls-files failed".to_string());
    }
    let mut files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    if files.last().is_some_and(String::is_empty) {
        files.pop();
    }
    Ok(files)
}

/// Local commits in the `git log <against>` range.
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation`
/// (`src/repository/api/ArcanistGitAPI.php:99`).
/// `against` is `HEAD` for a single-commit repository, else
/// [`commit_range_against`] of head and base; the caller resolves the
/// range because base/head tracking is PHP instance state.
///
/// Divergence: the empty-range direction check and its exception
/// (`src/repository/api/ArcanistGitAPI.php:139`) are the caller's job
/// via [`commit_range_is_reversed`] plus
/// [`empty_commit_range_message`]; the no-commits exception is not
/// raised because the caller passes the range explicitly.
pub fn local_commit_information(dir: &Path, against: &str) -> Result<Vec<LocalCommit>, String> {
    let format = log_local_commit_format();
    let mut args: Vec<&str> = vec!["log"];
    args.extend(against.split_whitespace());
    args.extend(["--format", format, "--"]);
    let out = output(dir, &args)?;
    if !out.status.success() {
        return Err("git log failed".to_string());
    }
    Ok(parse_local_commit_information(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// Parse the raw commit object `hash`.
///
/// PHP: `ArcanistGitAPI::readRawCommit`
/// (`src/repository/api/ArcanistGitAPI.php:1740`).
///
/// Divergence: the blob is lossily decoded; non-UTF-8 message bytes
/// become U+FFFD.
pub fn read_raw_commit(dir: &Path, hash: &str) -> Result<RawCommit, String> {
    let out = output(dir, &["cat-file", "commit", "--", hash])?;
    if !out.status.success() {
        return Err(unknown_commit_message(hash));
    }
    RawCommit::new_from_raw_blob(&String::from_utf8_lossy(&out.stdout))
}

/// Write a raw commit object, returning its hash.
///
/// PHP: `ArcanistGitAPI::writeRawCommit`
/// (`src/repository/api/ArcanistGitAPI.php:1748`).
pub fn write_raw_commit(dir: &Path, commit: &RawCommit) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new("git")
        .current_dir(dir)
        .args(["hash-object", "-t", "commit", "--stdin", "-w"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        stdin
            .write_all(commit.get_raw_blob().as_bytes())
            .map_err(|e| format!("failed to write raw commit: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err("git hash-object failed".to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Stage `paths` for commit.
///
/// PHP: `ArcanistGitAPI::addToCommit`
/// (`src/repository/api/ArcanistGitAPI.php:805`).
///
/// Divergence: the PHP working-copy cache reload is instance state and
/// has no equivalent in this stateless port.
pub fn add_to_commit(dir: &Path, paths: &[&str]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["add", "-A", "--"];
    args.extend(paths.iter().copied());
    stdout_text(dir, &args).map(|_| ())
}

/// Commit staged changes with `message`.
///
/// PHP: `ArcanistGitAPI::doCommit`
/// (`src/repository/api/ArcanistGitAPI.php:813`):
/// the message goes through a temporary file. The file is removed
/// afterwards on a best-effort basis.
pub fn do_commit(dir: &Path, message: &str) -> Result<(), String> {
    let path = commit_message_file(message)?;
    let name = path.to_string_lossy().into_owned();
    let result = stdout_text(dir, &["commit", "-F", &name]).map(|_| ());
    std::fs::remove_file(&path).ok();
    result
}

/// Amend HEAD, replacing the message unless `message` is `None`.
///
/// PHP: `ArcanistGitAPI::amendCommit`
/// (`src/repository/api/ArcanistGitAPI.php:829`).
pub fn amend_commit(dir: &Path, message: Option<&str>) -> Result<(), String> {
    if let Some(message) = message {
        let path = commit_message_file(message)?;
        let name = path.to_string_lossy().into_owned();
        let result =
            stdout_text(dir, &["commit", "--amend", "--allow-empty", "-F", &name]).map(|_| ());
        std::fs::remove_file(&path).ok();
        result
    } else {
        stdout_text(dir, &["commit", "--amend", "--allow-empty", "-C", "HEAD"]).map(|_| ())
    }
}

/// Write `message` to a unique temporary file for `commit -F`.
fn commit_message_file(message: &str) -> Result<PathBuf, String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "arc-rs-commit-{}-{nanos}-{id}.txt",
        std::process::id()
    ));
    std::fs::write(&path, message).map_err(|e| format!("failed to write commit message: {e}"))?;
    Ok(path)
}

/// Stash working-copy changes.
///
/// PHP: `ArcanistGitAPI::stashChanges`
/// (`src/repository/api/ArcanistGitAPI.php:1437`).
pub fn stash_changes(dir: &Path) -> Result<(), String> {
    stdout_text(dir, &["stash"]).map(|_| ())
}

/// Restore stashed working-copy changes.
///
/// PHP: `ArcanistGitAPI::unstashChanges`
/// (`src/repository/api/ArcanistGitAPI.php:1442`).
pub fn unstash_changes(dir: &Path) -> Result<(), String> {
    stdout_text(dir, &["stash", "pop"]).map(|_| ())
}

/// Pull the upstream into the working copy.
///
/// PHP: `ArcanistGitAPI::updateWorkingCopy`
/// (`src/repository/api/ArcanistGitAPI.php:1243`).
pub fn update_working_copy(dir: &Path) -> Result<(), String> {
    stdout_text(dir, &["pull"]).map(|_| ())
}

/// True when the repository has git-svn metadata.
///
/// PHP: `ArcanistGitAPI::isGitSubversionRepo`
/// (`src/repository/api/ArcanistGitAPI.php:1261`).
#[must_use]
pub fn is_git_subversion_repo(dir: &Path) -> bool {
    metadata_path(dir).is_ok_and(|meta| meta.join("svn").exists())
}

/// True when `remote_name` is the Perforce pseudo-remote: named `p4`
/// with refs under `.git/refs/remotes/p4` (T13434).
///
/// PHP: `ArcanistGitAPI::isPerforceRemote`
/// (`src/repository/api/ArcanistGitAPI.php:1522`).
#[must_use]
pub fn is_perforce_remote(dir: &Path, remote_name: &str) -> bool {
    if remote_name != "p4" {
        return false;
    }
    metadata_path(dir).is_ok_and(|meta| meta.join("refs").join("remotes").join("p4").exists())
}

/// True when any tracked file has the LFS filter attribute.
///
/// PHP: `ArcanistGitAPI::isGitLFSWorkingCopy`
/// (`src/repository/api/ArcanistGitAPI.php:1666`).
///
/// Divergence: only the modern `ls-files ':(attr:filter=lfs)'` probe
/// is ported; on error this returns false, which is also what PHP
/// does after its fallback discussion
/// (`src/repository/api/ArcanistGitAPI.php:1724`).
#[must_use]
pub fn is_git_lfs_working_copy(dir: &Path) -> bool {
    let Ok(out) = output(dir, &["ls-files", "-z", "--", ":(attr:filter=lfs)"]) else {
        return false;
    };
    lfs_output_indicates_lfs(out.status.success(), &out.stdout)
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod git_tests;
