//! `arc work`: begin or resume work on a branch.
//!
//! PHP references: `src/workflow/ArcanistWorkWorkflow.php` (exactly one
//! symbol, `--start`), `src/work/ArcanistWorkEngine.php` (local-state
//! dance, revision/task symbols), `src/work/ArcanistGitWorkEngine.php`
//! (check out an existing branch, else `checkout -b` from the startpoint),
//! `src/work/ArcanistMercurialWorkEngine.php` (T1153 WORK-HG: bookmarks
//! plus branch checkout), and
//! `src/repository/state/ArcanistRepositoryLocalState.php` (stash
//! prompts).
//!
//! Deviations: `--track` is passed only when the startpoint is itself a
//! branch (PHP always passes it, so PHP fails for SHA/tag startpoints);
//! prompt answers are never persisted and saved responses are not read,
//! and prompts read stdin without a TTY gate (see `prompt`); merge
//! conflicts can not occur for git upstream (only
//! Subversion sets the flag), so that gate is omitted; `@NNN` SVN
//! startpoints are not resolved; non-commit startpoints resolve like any
//! other object. Monogram symbols resolve to ID `0`, exactly like
//! upstream `(int)$matches[1]`, so `D123`/`T123` never match.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::{config, diff_submit, git, hg, prompt, terminal, which};

/// Abort text for declined prompts (same as `prompt::ABORT`).
pub const ABORT: &str = "User aborted the workflow.";

/// Subcommand name, matching `arc work`.
pub const NAME: &str = "work";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `arc_rs::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistWorkWorkflow.php:11-50`).
const HELP_EXAMPLES: &str = "**work** [--start __start__] __symbol__";
const HELP_SYNOPSIS: &str = "Begin or resume work.";
const HELP_TEXT: &str = "Begin or resume work on a branch, bookmark, task, or revision.\n\
\n\
The __symbol__ may be a branch or bookmark name, a revision name (like \"D123\"),\n\
a task name (like \"T123\"), or a new symbol.\n\
\n\
If you provide a symbol which currently does not identify any ongoing work,\n\
Arcanist will create a new branch or bookmark with the name you provide.\n\
\n\
If you provide the name of an existing branch or bookmark, Arcanist will switch\n\
to that branch or bookmark.\n\
\n\
If you provide the name of a revision or task, Arcanist will look for a related\n\
branch or bookmark that exists in the working copy. If it finds one, it will\n\
switch to it. If it does not find one, it will attempt to create a new branch\n\
or bookmark.\n\
\n\
When \"arc work\" creates a branch or bookmark, it will use **--start** as the\n\
branchpoint if it is provided. Otherwise, the current working copy state will\n\
serve as the starting point.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[(
    "start",
    None,
    Some("symbol"),
    "When creating a new branch or bookmark, use this as the branch point.",
)];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<arc_rs::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    arc_rs::help::usage_text(
        NAME,
        Some(HELP_EXAMPLES),
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        true,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(terminal::ansi_enabled())
}

/// Parsed `work` options.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub start: Option<String>,
    pub symbol: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
    /// T1153 B21/B23: stdin is piped and argv carries no `--`.
    MissingTerminator,
}

/// Parse `argv` (excluding the subcommand name).
pub fn parse_args(argv: &[String]) -> ParseOutcome {
    parse_args_with_tty(argv, arc_rs::prompt::stdin_is_terminal())
}

/// [`parse_args`] with an injected TTY flag (deterministic tests).
/// T1153 B21/B23: piped runs without `--` fail with
/// `MissingTerminator` (checked after unknown-flag scanning, like
/// PHP's `parseFull()`, but before symbol-count validation). `--help`
/// without `--` when piped reports the terminator. Vs ceb
/// `ArcanistRuntime.php:80-86,172-188` and
/// `PhutilArgumentParser.php:331-336`.
fn parse_args_with_tty(argv: &[String], stdin_tty: bool) -> ParseOutcome {
    let missing = arc_rs::help::terminator_missing(argv, stdin_tty);
    let mut start = None;
    let mut symbols = Vec::new();
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            symbols.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--start" {
            let Some(value) = iter.next() else {
                return ParseOutcome::Error("--start requires a value".to_string());
            };
            start = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--start=") {
            start = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            if missing {
                return ParseOutcome::MissingTerminator;
            }
            return ParseOutcome::Help;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            symbols.push(arg.clone());
        }
    }
    if missing {
        return ParseOutcome::MissingTerminator;
    }
    if symbols.is_empty() {
        return ParseOutcome::Error(
            "Provide a branch, bookmark, task, or revision name to begin or resume work on."
                .to_string(),
        );
    }
    if symbols.len() > 1 {
        return ParseOutcome::Error(
            "Too many arguments: provide exactly one argument.".to_string(),
        );
    }
    let symbol = symbols.pop().unwrap_or_default();
    if symbol.is_empty() {
        return ParseOutcome::Error(
            "Provide a nonempty symbol to begin or resume work on.".to_string(),
        );
    }
    ParseOutcome::Args(Options { start, symbol })
}

/// A revision or task object symbol: a bare/monogram ID or a PHID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectSymbol {
    Id(String),
    Phid(String),
}

/// True when `rest` is `[1-9][0-9]*`.
fn is_bare_id(rest: &str) -> bool {
    let mut chars = rest.chars();
    if !matches!(chars.next(), Some(first) if ('1'..='9').contains(&first)) {
        return false;
    }
    rest.bytes().all(|b| b.is_ascii_digit())
}

/// `[Xx]?[1-9][0-9]*`, mirroring `ArcanistSimpleSymbolRef`: a prefixed
/// monogram resolves to ID `0` (upstream casts the whole match, prefix
/// included), a bare ID to its digits.
fn prefixed_id(symbol: &str, prefix: char) -> Option<String> {
    let lower = prefix.to_ascii_lowercase();
    for candidate in [prefix, lower] {
        if let Some(rest) = symbol.strip_prefix(candidate) {
            return is_bare_id(rest).then(|| "0".to_string());
        }
    }
    is_bare_id(symbol).then(|| symbol.to_string())
}

/// `PHID-<TYPE>-<non-blank>`, mirroring `ArcanistSimpleSymbolRef`.
fn typed_phid(symbol: &str, phid_type: &str) -> Option<String> {
    let prefix = format!("PHID-{phid_type}-");
    let rest = symbol.strip_prefix(&prefix)?;
    if rest.is_empty() {
        return None;
    }
    // PCRE `\S`: anything but space, \t, \n, \r, \f, \v.
    let blank = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x0b);
    if rest.bytes().any(blank) {
        None
    } else {
        Some(symbol.to_string())
    }
}

/// Parse a revision symbol (`D123`, `123`, or `PHID-DREV-...`).
#[must_use]
pub fn revision_symbol(symbol: &str) -> Option<ObjectSymbol> {
    if let Some(id) = prefixed_id(symbol, 'D') {
        return Some(ObjectSymbol::Id(id));
    }
    typed_phid(symbol, "DREV").map(ObjectSymbol::Phid)
}

/// Parse a task symbol (`T123`, `123`, or `PHID-TASK-...`).
#[must_use]
pub fn task_symbol(symbol: &str) -> Option<ObjectSymbol> {
    if let Some(id) = prefixed_id(symbol, 'T') {
        return Some(ObjectSymbol::Id(id));
    }
    typed_phid(symbol, "TASK").map(ObjectSymbol::Phid)
}

/// True when `symbol` parses as a revision symbol.
#[must_use]
pub fn looks_like_revision(symbol: &str) -> bool {
    revision_symbol(symbol).is_some()
}

/// True when `symbol` parses as a task symbol.
#[must_use]
pub fn looks_like_task(symbol: &str) -> bool {
    task_symbol(symbol).is_some()
}

/// True when `symbol` needs a Conduit client (a revision or task symbol).
#[must_use]
pub fn symbol_needs_conduit(symbol: &str) -> bool {
    looks_like_revision(symbol) || looks_like_task(symbol)
}

/// True when `symbol` names an existing local branch.
#[must_use]
pub fn branch_exists(dir: &Path, symbol: &str) -> bool {
    let target = format!("refs/heads/{symbol}");
    git::ok(dir, &["show-ref", "--verify", "--quiet", &target])
}

/// Resolve `rev` to a SHA via `show -s --format=%H`, mirroring
/// `ArcanistGitAPI::getCanonicalRevisionName()`, or `None` when it does
/// not resolve.
#[must_use]
pub fn canonical_revision(dir: &Path, rev: &str) -> Option<String> {
    git::output(dir, &["show", "-s", "--format=%H", rev, "--"])
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim_end_matches([' ', '\t', '\n', '\r', '\0', '\u{b}'])
                .to_string()
        })
        .filter(|sha| !sha.is_empty())
}

/// Default branch point: the current branch name, else the `HEAD` SHA.
#[must_use]
pub fn default_start(dir: &Path) -> Option<String> {
    if let Ok(name) = git::stdout_text(dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        && !name.is_empty()
    {
        return Some(name);
    }
    canonical_revision(dir, "HEAD")
}

/// True when `start` resolves to a local or remote-tracking branch, in which
/// case the new branch is created with `--track` like the PHP engine.
/// (`--track` is rejected by git for SHA or tag startpoints, so those
/// create a plain branch.)
fn start_is_branch(dir: &Path, start: &str) -> bool {
    let Ok(full) = git::stdout_text(
        dir,
        &["rev-parse", "--verify", "--symbolic-full-name", start],
    ) else {
        return false;
    };
    full.starts_with("refs/heads/") || full.starts_with("refs/remotes/")
}

/// One log line on stderr like PHP's log engine (`writeStatus`).
fn push_status(label: &str, message: &str) {
    eprint!(
        "{}",
        terminal::format_log("blue", label, message, terminal::ansi_enabled())
    );
}

/// First 12 hex characters, mirroring `getDisplayHash()`.
fn display_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

fn id_number(id: &str) -> serde_json::Value {
    id.parse::<u64>().map_or_else(
        |_| serde_json::Value::String(id.to_string()),
        serde_json::Value::from,
    )
}

/// Search params for one object symbol (`constraints` + `ids`/`phids`).
fn search_params(symbol: &ObjectSymbol) -> serde_json::Value {
    match symbol {
        ObjectSymbol::Id(id) => serde_json::json!({"constraints": {"ids": [id_number(id)]}}),
        ObjectSymbol::Phid(phid) => serde_json::json!({"constraints": {"phids": [phid]}}),
    }
}

/// Rows of a Conduit search response (`data` list).
fn search_rows(result: &serde_json::Value) -> Vec<&serde_json::Value> {
    result
        .get("data")
        .and_then(|data| data.as_array())
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

/// Rows of a legacy `differential.query` response (list or id-keyed map).
fn query_rows(result: &serde_json::Value) -> Vec<&serde_json::Value> {
    match result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.into_iter().filter_map(|k| map.get(k)).collect()
        }
        _ => Vec::new(),
    }
}

fn row_string(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| {
            v.as_str().map(str::to_string).or_else(|| {
                v.as_i64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            })
        })
        .unwrap_or_default()
}

fn row_title(row: &serde_json::Value) -> String {
    row.get("fields")
        .and_then(|fields| fields.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// A revision loaded from Conduit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub id: String,
    pub phid: String,
    pub monogram: String,
    pub title: String,
}

/// True when a search row is the object `symbol` asked for.
fn row_matches_symbol(row: &serde_json::Value, symbol: &ObjectSymbol) -> bool {
    match symbol {
        ObjectSymbol::Id(id) => row_string(row, "id") == *id,
        ObjectSymbol::Phid(phid) => row_string(row, "phid") == *phid,
    }
}

/// Load the revision for `symbol` via `differential.revision.search`.
pub fn load_revision(
    call: &diff_submit::ConduitCall,
    symbol: &ObjectSymbol,
) -> Result<Option<Revision>, String> {
    let result = call("differential.revision.search", &search_params(symbol))?;
    Ok(search_rows(&result)
        .into_iter()
        .find(|row| row_matches_symbol(row, symbol))
        .map(|row| {
            let id = row_string(row, "id");
            Revision {
                monogram: format!("D{id}"),
                title: row_title(row),
                phid: row_string(row, "phid"),
                id,
            }
        }))
}

/// A task loaded from Conduit (existence only; task branches are unported).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub phid: String,
    pub monogram: String,
}

/// Load the task for `symbol` via `maniphest.search`.
pub fn load_task(
    call: &diff_submit::ConduitCall,
    symbol: &ObjectSymbol,
) -> Result<Option<Task>, String> {
    let result = call("maniphest.search", &search_params(symbol))?;
    Ok(search_rows(&result)
        .into_iter()
        .find(|row| row_matches_symbol(row, symbol))
        .map(|row| {
            let id = row_string(row, "id");
            Task {
                monogram: format!("T{id}"),
                phid: row_string(row, "phid"),
                id,
            }
        }))
}

/// A local branch tip, mirroring a git marker ref (or an hg
/// branch/bookmark marker, T1153 WORK-HG).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchTip {
    pub name: String,
    pub commit: String,
    pub tree: String,
    pub epoch: i64,
    pub summary: String,
    pub message: String,
    /// `branch` or `bookmark` (T1153 WORK-HG: PHP renders
    /// `Bookmark "x"` vs `Branch "x"`,
    /// `ArcanistMarkerRef.php:27-36`).
    pub marker_type: String,
}

fn parse_tip_record(record: &str) -> Option<BranchTip> {
    let record = record.trim_matches([' ', '\n', '\u{2}']);
    if record.is_empty() {
        return None;
    }
    let mut fields = record.splitn(6, '\u{1}');
    let (Some(name), Some(commit), Some(tree), Some(epoch), Some(summary), Some(message)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    Some(BranchTip {
        name: name.to_string(),
        commit: commit.to_string(),
        tree: tree.to_string(),
        epoch: epoch.parse().unwrap_or(0),
        summary: summary.to_string(),
        message: message.to_string(),
        // T1153 WORK-HG: git tips are always branches.
        marker_type: "branch".to_string(),
    })
}

/// Local branch tips via `for-each-ref` (refname order, like PHP).
pub fn local_branch_tips(dir: &Path) -> Result<Vec<BranchTip>, String> {
    let format = "%(refname:short)%01%(objectname)%01%(tree)%01%(committerdate:unix)%01%(subject)%01%(body)%02";
    let flag = format!("--format={format}");
    let out = git::output(dir, &["for-each-ref", &flag, "--", "refs/heads"])
        .map_err(|e| format!("failed to list branches: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git for-each-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\u{2}')
        .filter_map(parse_tip_record)
        .collect())
}

/// True when a `differential.query` hash entry matches a branch tip.
fn hash_matches(entry: &serde_json::Value, tip: &BranchTip) -> bool {
    let pair: Vec<&str> = entry
        .as_array()
        .map(|items| items.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    match pair.as_slice() {
        ["gtcm", hash] => *hash == tip.commit,
        ["gttr", hash] => *hash == tip.tree,
        _ => false,
    }
}

/// Mark branches whose tip commit/tree is attached to the revision
/// (`differential.query` with `commitHashes`).
fn mark_hash_associated(
    call: &diff_submit::ConduitCall,
    tips: &[BranchTip],
    phid: &str,
    hits: &mut [bool],
) -> Result<(), String> {
    let mut hashes = Vec::new();
    for tip in tips {
        hashes.push(serde_json::json!(["gtcm", tip.commit]));
        if !tip.tree.is_empty() {
            hashes.push(serde_json::json!(["gttr", tip.tree]));
        }
    }
    let result = call(
        "differential.query",
        &serde_json::json!({"commitHashes": hashes}),
    )?;
    let empty = Vec::new();
    for row in query_rows(&result) {
        if row_string(row, "phid") != phid {
            continue;
        }
        let revision_hashes = row
            .get("hashes")
            .and_then(|hashes| hashes.as_array())
            .unwrap_or(&empty);
        for (tip, hit) in tips.iter().zip(hits.iter_mut()) {
            if revision_hashes.iter().any(|entry| hash_matches(entry, tip)) {
                *hit = true;
            }
        }
    }
    Ok(())
}

/// Mark branches whose tip message names the revision in a
/// `Differential Revision:` footer (malformed footers are skipped).
fn mark_footer_associated(
    call: &diff_submit::ConduitCall,
    tips: &[BranchTip],
    phid: &str,
    hits: &mut [bool],
) -> Result<(), String> {
    let mut by_id: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, tip) in tips.iter().enumerate() {
        if let Ok(Some(id)) = which::revision_id_from_corpus(&tip.message) {
            by_id.entry(id).or_default().push(index);
        }
    }
    if by_id.is_empty() {
        return Ok(());
    }
    let ids: Vec<serde_json::Value> = by_id.keys().map(|id| id_number(id)).collect();
    let result = call(
        "differential.revision.search",
        &serde_json::json!({"constraints": {"ids": ids}}),
    )?;
    for row in search_rows(&result) {
        if row_string(row, "phid") != phid {
            continue;
        }
        if let Some(indices) = by_id.get(&row_string(row, "id")) {
            for index in indices {
                hits[*index] = true;
            }
        }
    }
    Ok(())
}

/// Local branches associated with `revision`, in refname order.
pub fn branches_for_revision(
    call: &diff_submit::ConduitCall,
    dir: &Path,
    revision: &Revision,
) -> Result<Vec<BranchTip>, String> {
    let tips = local_branch_tips(dir)?;
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = vec![false; tips.len()];
    mark_hash_associated(call, &tips, &revision.phid, &mut hits)?;
    mark_footer_associated(call, &tips, &revision.phid, &mut hits)?;
    Ok(tips
        .into_iter()
        .zip(hits)
        .filter(|(_, hit)| *hit)
        .map(|(tip, _)| tip)
        .collect())
}

/// One hg bookmark entry: name plus node (T1153 WORK-HG).
fn parse_hg_bookmark_nodes(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line
                .trim_start()
                .strip_prefix([' ', '*'])
                .unwrap_or(line.trim_start());
            let mut parts = trimmed.split_whitespace();
            let name = parts.next().unwrap_or("");
            let rev_node = parts.next().unwrap_or("");
            let node = rev_node.split(':').nth(1).unwrap_or("");
            if name.is_empty() || node.is_empty() {
                None
            } else {
                Some((name.to_string(), node.to_string()))
            }
        })
        .collect()
}

/// Hg branch/bookmark tips (T1153 WORK-HG: plain `hg` lists, like
/// `land`; messages come from `hg log` per node for footer search).
pub fn hg_branch_tips_with(dir: &Path, hg_cmd: &hg::HgCommand) -> Result<Vec<BranchTip>, String> {
    let bookmarks = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    let branches = hg::stdout_text_with(dir, hg_cmd, &["branches"]).unwrap_or_default();
    let mut tips = Vec::new();
    for (name, short) in parse_hg_bookmark_nodes(&bookmarks) {
        // `hg bookmarks` gives a short hash; resolve to full node.
        let node = hg_canonical_revision_with(dir, hg_cmd, &short).unwrap_or(short);
        let message = hg::stdout_text_with(
            dir,
            hg_cmd,
            &["log", "-l", "1", "--template", "{desc}", "-r", &node, "--"],
        )
        .unwrap_or_default();
        let summary = message.lines().next().unwrap_or("").to_string();
        tips.push(BranchTip {
            name,
            commit: node,
            tree: String::new(),
            epoch: 0,
            summary,
            message,
            marker_type: "bookmark".to_string(),
        });
    }
    let branch_map = hg::parse_mercurial_branches(&branches).unwrap_or_default();
    for (name, entry) in branch_map {
        // `hg branches` gives a short hash; resolve to full node.
        let node = hg_canonical_revision_with(dir, hg_cmd, &name).unwrap_or(entry.rev);
        let message = hg::stdout_text_with(
            dir,
            hg_cmd,
            &["log", "-l", "1", "--template", "{desc}", "-r", &node, "--"],
        )
        .unwrap_or_default();
        let summary = message.lines().next().unwrap_or("").to_string();
        tips.push(BranchTip {
            name,
            commit: node,
            tree: String::new(),
            epoch: 0,
            summary,
            message,
            marker_type: "branch".to_string(),
        });
    }
    Ok(tips)
}

/// Hg tips with the system binary (T1153 WORK-HG).
pub fn hg_branch_tips(dir: &Path) -> Result<Vec<BranchTip>, String> {
    hg_branch_tips_with(dir, &hg::HgCommand::system())
}

/// Mark hg tips whose node is attached to the revision (T1153 WORK-HG:
/// `hgcm` hashes, `ArcanistMercurialAPI.php:578-592`).
fn mark_hg_hash_associated(
    call: &diff_submit::ConduitCall,
    tips: &[BranchTip],
    phid: &str,
    hits: &mut [bool],
) -> Result<(), String> {
    let mut hashes = Vec::new();
    for tip in tips {
        hashes.push(serde_json::json!(["hgcm", tip.commit]));
    }
    let result = call(
        "differential.query",
        &serde_json::json!({"commitHashes": hashes}),
    )?;
    let empty = Vec::new();
    for row in query_rows(&result) {
        if row_string(row, "phid") != phid {
            continue;
        }
        let revision_hashes = row
            .get("hashes")
            .and_then(|hashes| hashes.as_array())
            .unwrap_or(&empty);
        for (tip, hit) in tips.iter().zip(hits.iter_mut()) {
            let attached = revision_hashes.iter().any(|entry| {
                let pair: Vec<&str> = entry
                    .as_array()
                    .map(|items| items.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                matches!(pair.as_slice(), ["hgcm", hash] if *hash == tip.commit)
            });
            if attached {
                *hit = true;
            }
        }
    }
    Ok(())
}

/// Hg markers associated with `revision` (T1153 WORK-HG).
pub fn hg_branches_for_revision_with(
    call: &diff_submit::ConduitCall,
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    revision: &Revision,
) -> Result<Vec<BranchTip>, String> {
    let tips = hg_branch_tips_with(dir, hg_cmd)?;
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = vec![false; tips.len()];
    mark_hg_hash_associated(call, &tips, &revision.phid, &mut hits)?;
    mark_footer_associated(call, &tips, &revision.phid, &mut hits)?;
    Ok(tips
        .into_iter()
        .zip(hits)
        .filter(|(_, hit)| *hit)
        .map(|(tip, _)| tip)
        .collect())
}

/// Hg association with the system binary (T1153 WORK-HG).
pub fn hg_branches_for_revision(
    call: &diff_submit::ConduitCall,
    dir: &Path,
    revision: &Revision,
) -> Result<Vec<BranchTip>, String> {
    hg_branches_for_revision_with(call, dir, &hg::HgCommand::system(), revision)
}

/// A revision ref view (stdout), mirroring `ArcanistRefView`.
#[must_use]
pub fn revision_ref_view(revision: &Revision, width: usize, ansi: bool) -> String {
    terminal::RefView {
        object_name: Some(revision.monogram.clone()),
        title: Some(revision.title.clone()),
        ..terminal::RefView::default()
    }
    .render(0, width, ansi)
}

/// A marker ref view (stdout): `Branch "name"` (or `Bookmark "x"`
/// for hg bookmarks, T1153 WORK-HG), title `hash12 summary`.
#[must_use]
pub fn marker_ref_view(tip: &BranchTip, width: usize, ansi: bool) -> String {
    // T1153 WORK-HG: PHP `getRefDisplayName()` branches on marker
    // type (`ArcanistMarkerRef.php:27-36`).
    let label = if tip.marker_type == "bookmark" {
        "Bookmark"
    } else {
        "Branch"
    };
    terminal::RefView {
        object_name: Some(format!("{label} \"{}\"", tip.name)),
        title: Some(format!("{} {}", display_hash(&tip.commit), tip.summary)),
        ..terminal::RefView::default()
    }
    .render(0, width, ansi)
}

/// The multi-match banner plus marker views (stdout).
#[must_use]
pub fn ambiguous_marker_block(
    monogram: &str,
    tips: &[BranchTip],
    width: usize,
    ansi: bool,
) -> String {
    let message = terminal::pht(
        "More than one marker in the local working copy is associated with the revision \"%s\", \
         using the most recent one.",
        &[terminal::PhtArg::Text(monogram)],
    );
    let mut out = terminal::tsprintf_ansi(
        "\n%!\n%W\n\n",
        &[
            terminal::TVal::Text("AMBIGUOUS MARKER"),
            terminal::TVal::Text(&message),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    for tip in tips {
        out.push_str(&marker_ref_view(tip, width, ansi));
    }
    out.push('\n');
    out
}

fn work_outside_repo() -> ExitCode {
    // T1153 WORK-HG: hg is ported now; this fires only outside any
    // working copy (`ArcanistWorkWorkflow.php:56-61`).
    eprintln!("arc {NAME}: \"arc work\" must be run in a Git or Mercurial working copy.");
    ExitCode::FAILURE
}

/// True inside a Mercurial working copy (T1153 WORK-HG).
fn is_hg_repo(dir: &Path) -> bool {
    hg::find_root(dir).is_some()
}

/// Saved position for restore-on-failure, mirroring `ArcanistGitLocalState`.
pub struct WorkState {
    pub ref_name: Option<String>,
    pub commit: String,
}

/// Record the current ref (or detached commit) for a later restore.
pub fn save_work_state(dir: &Path) -> Result<WorkState, String> {
    let commit = git::stdout_text(dir, &["rev-parse", "--verify", "HEAD"])?;
    let abbrev = git::stdout_text(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "HEAD".to_string());
    let ref_name = if abbrev == "HEAD" { None } else { Some(abbrev) };
    Ok(WorkState { ref_name, commit })
}

/// Check the saved position back out and update submodules.
pub fn restore_work_state(dir: &Path, state: &WorkState) -> Result<(), String> {
    match &state.ref_name {
        Some(name) => {
            push_status(
                "LOAD STATE",
                &format!(
                    "Restoring local state (to ref \"{name}\" at commit \"{}\").",
                    display_hash(&state.commit)
                ),
            );
            git::stdout_text(dir, &["checkout", "-B", name, &state.commit, "--"])?;
        }
        None => {
            push_status(
                "LOAD STATE",
                &format!(
                    "Restoring local state (to detached commit \"{}\").",
                    display_hash(&state.commit)
                ),
            );
            git::stdout_text(dir, &["checkout", &state.commit, "--"])?;
        }
    }
    git::stdout_text(dir, &["submodule", "update", "--init", "--recursive"])?;
    Ok(())
}

/// Saved hg position, mirroring `ArcanistMercurialLocalState`
/// (T1153 WORK-HG: `src/repository/state/ArcanistMercurialLocalState.php:6-12`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgWorkState {
    pub commit: String,
    pub branch: String,
    pub bookmark: Option<String>,
}

/// Record the hg commit, branch, and active bookmark (T1153 WORK-HG).
pub fn save_hg_state_with(dir: &Path, hg_cmd: &hg::HgCommand) -> Result<HgWorkState, String> {
    let commit = hg::stdout_text_with(
        dir,
        hg_cmd,
        &["log", "-l", "1", "--template", "{node}", "-r", "."],
    )?;
    let branch = hg::stdout_text_with(dir, hg_cmd, &["branch"])?;
    let bookmarks = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    let bookmark = parse_hg_bookmarks_active(&bookmarks);
    Ok(HgWorkState {
        commit: commit.trim().to_string(),
        branch: branch.trim().to_string(),
        bookmark,
    })
}

/// Record the hg position with the system binary (T1153 WORK-HG).
pub fn save_hg_state(dir: &Path) -> Result<HgWorkState, String> {
    save_hg_state_with(dir, &hg::HgCommand::system())
}

/// Active bookmark name from `hg bookmarks` output, if any (T1153
/// WORK-HG: `*` marks the active entry, like `land::parse_hg_bookmarks`).
fn parse_hg_bookmarks_active(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('*') {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Restore the saved hg position (T1153 WORK-HG:
/// `ArcanistMercurialLocalState.php:82-107`).
pub fn restore_hg_state_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    state: &HgWorkState,
) -> Result<(), String> {
    match &state.bookmark {
        Some(bookmark) => push_status(
            "LOAD STATE",
            &format!(
                "Restoring local state (at \"{}\" on branch \"{}\", bookmarked as \"{bookmark}\").",
                display_hash(&state.commit),
                state.branch
            ),
        ),
        None => push_status(
            "LOAD STATE",
            &format!(
                "Restoring local state (at \"{}\" on branch \"{}\").",
                display_hash(&state.commit),
                state.branch
            ),
        ),
    }
    hg::stdout_text_with(dir, hg_cmd, &["update", "--", &state.commit])?;
    hg::stdout_text_with(dir, hg_cmd, &["branch", "--force", "--", &state.branch])?;
    if let Some(bookmark) = &state.bookmark {
        hg::stdout_text_with(dir, hg_cmd, &["bookmark", "--force", "--", bookmark])?;
    }
    Ok(())
}

/// Restore the hg position with the system binary (T1153 WORK-HG).
pub fn restore_hg_state(dir: &Path, state: &HgWorkState) -> Result<(), String> {
    restore_hg_state_with(dir, &hg::HgCommand::system(), state)
}

/// One path's status flags from `status --porcelain=2 -z`.
#[derive(Debug, Default, Clone, Copy)]
struct EntryFlags {
    uncommitted: bool,
    unstaged: bool,
    untracked: bool,
    externals: bool,
}

/// Changed paths grouped like `saveLocalState()`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DirtyLists {
    pub untracked: Vec<String>,
    pub unstaged: Vec<String>,
    pub uncommitted: Vec<String>,
    pub externals: Vec<String>,
}

fn subm_is_dirty(subm: &str) -> bool {
    let bytes = subm.as_bytes();
    bytes.get(2) == Some(&b'M') || bytes.get(3) == Some(&b'U')
}

fn flags_for_entry(entry: &str) -> Option<(&str, EntryFlags, bool)> {
    let kind = entry.split(' ').next()?;
    match kind {
        // Limited splits keep paths with spaces intact, like PHP's
        // `explode(' ', $entry, N)`.
        "?" => {
            let path = entry.split_once(' ')?.1;
            Some((
                path,
                EntryFlags {
                    untracked: true,
                    ..EntryFlags::default()
                },
                false,
            ))
        }
        "1" | "2" | "u" => {
            let mut head = entry.splitn(4, ' ');
            head.next()?;
            let xy = head.next()?;
            let subm = head.next()?;
            let path = match kind {
                "1" => entry.splitn(9, ' ').nth(8)?,
                "2" => entry.splitn(10, ' ').nth(9)?,
                _ => entry.splitn(11, ' ').nth(10)?,
            };
            let working = xy.as_bytes().get(1).copied().unwrap_or(b'.');
            Some((
                path,
                EntryFlags {
                    uncommitted: true,
                    unstaged: working != b'.',
                    externals: subm_is_dirty(subm),
                    ..EntryFlags::default()
                },
                kind == "2",
            ))
        }
        _ => None,
    }
}

/// Parse `status --porcelain=2 -z` into per-path flags (rename origins
/// inherit the entry flags; paths stay sorted via the map).
fn status_flags(dir: &Path) -> Result<BTreeMap<String, EntryFlags>, String> {
    let out = git::output(dir, &["status", "--porcelain=2", "-z"])
        .map_err(|e| format!("failed to read working copy status: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.split('\0');
    let mut flags: BTreeMap<String, EntryFlags> = BTreeMap::new();
    while let Some(entry) = fields.next() {
        if entry.is_empty() {
            continue;
        }
        let Some((path, entry_flags, is_rename)) = flags_for_entry(entry) else {
            continue; // LCOV_EXCL_LINE // Reason: porcelain=2 without --branch emits only 1/2/u/? entries; no real output reaches this arm
        };
        flags.insert(path.to_string(), entry_flags);
        if is_rename
            && let Some(orig) = fields.next()
            && !orig.is_empty()
        {
            flags.insert(orig.to_string(), entry_flags);
        }
    }
    Ok(flags)
}

/// Group changed paths into untracked/unstaged/uncommitted/externals.
pub fn dirty_lists(dir: &Path) -> Result<DirtyLists, String> {
    let flags = status_flags(dir)?;
    let mut lists = DirtyLists::default();
    for (path, entry) in &flags {
        if entry.untracked {
            lists.untracked.push(path.clone());
        }
        if entry.externals {
            lists.externals.push(path.clone());
        }
    }
    for (path, entry) in &flags {
        if entry.unstaged && !lists.externals.iter().any(|e| e == path) {
            lists.unstaged.push(path.clone());
        }
    }
    for (path, entry) in &flags {
        if entry.uncommitted
            && !lists.unstaged.iter().any(|e| e == path)
            && !lists.externals.iter().any(|e| e == path)
        {
            lists.uncommitted.push(path.clone());
        }
    }
    Ok(lists)
}

/// Group hg changed paths (T1153 WORK-HG:
/// `ArcanistMercurialAPI.php:307-323` marks tracked files uncommitted;
/// hg has no index, so unstaged stays empty).
pub fn hg_dirty_lists_with(dir: &Path, hg_cmd: &hg::HgCommand) -> Result<DirtyLists, String> {
    let status = hg::get_uncommitted_status_with(dir, hg_cmd)?;
    let mut lists = DirtyLists::default();
    for (path, mask) in &status {
        if mask & hg::FLAG_UNTRACKED != 0 {
            lists.untracked.push(path.clone());
        }
    }
    for (path, mask) in &status {
        if mask & hg::FLAG_UNCOMMITTED != 0
            && !lists.unstaged.iter().any(|e| e == path)
            && !lists.externals.iter().any(|e| e == path)
        {
            lists.uncommitted.push(path.clone());
        }
    }
    Ok(lists)
}

/// Group hg changed paths with the system binary (T1153 WORK-HG).
pub fn hg_dirty_lists(dir: &Path) -> Result<DirtyLists, String> {
    hg_dirty_lists_with(dir, &hg::HgCommand::system())
}

/// `newDisplayFileList` + `printFileLists` rendering (stdout): each
/// nonempty list is a title plus indented paths, lists are separated by
/// blank lines, and one trailing newline always prints.
fn render_file_lists(lists: &[(&str, &[String])], ansi: bool) -> String {
    let rendered: Vec<String> = lists
        .iter()
        .filter(|(_, files)| !files.is_empty())
        .map(|(title, files)| {
            let mut out = terminal::tsprintf_ansi("%s\n\n", &[terminal::TVal::Text(title)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default();
            for file in files.iter() {
                out.push_str(
                    &terminal::tsprintf_ansi("    %s\n", &[terminal::TVal::Text(file)], ansi)
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                );
            }
            out
        })
        .collect();
    format!("{}\n", rendered.join("\n\n"))
}

/// The repository root with a trailing separator, mirroring
/// `ArcanistRepositoryAPI::getPath()`.
#[must_use]
pub fn repo_display_path(dir: &Path) -> String {
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])
        .unwrap_or_else(|_| dir.to_string_lossy().into_owned());
    let root = root.trim_end().to_string();
    #[cfg(windows)]
    let root = root.replace('/', "\\");
    let sep = std::path::MAIN_SEPARATOR;
    format!("{root}{sep}")
}

/// The hg repository root with a trailing separator (T1153 WORK-HG).
#[must_use]
pub fn hg_repo_display_path(dir: &Path) -> String {
    let root = hg::find_root(dir)
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned());
    let root = root.trim_end().to_string();
    #[cfg(windows)]
    let root = root.replace('/', "\\");
    let sep = std::path::MAIN_SEPARATOR;
    format!("{root}{sep}")
}

/// The UNCOMMITTED CHANGES block (stdout).
#[must_use]
pub fn render_uncommitted_block(dir: &Path, lists: &DirtyLists, ansi: bool) -> String {
    let working_copy = terminal::tsprintf_ansi(
        "    %s: %s\n",
        &[
            terminal::TVal::Text("Working Copy"),
            terminal::TVal::Text(&repo_display_path(dir)),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    let working_copy = terminal::TerminalString::new(working_copy);
    let mut out = terminal::tsprintf_ansi(
        "\n%!\n%W\n\n%s\n",
        &[
            terminal::TVal::Text("UNCOMMITTED CHANGES"),
            terminal::TVal::Text("You have uncommitted changes in this working copy."),
            terminal::TVal::Terminal(&working_copy),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    out.push_str(&render_file_lists(
        &[
            (
                "Untracked changes in working copy:",
                lists.untracked.as_slice(),
            ),
            (
                "Unstaged changes in working copy:",
                lists.unstaged.as_slice(),
            ),
            (
                "Uncommitted changes in working copy:",
                lists.uncommitted.as_slice(),
            ),
        ],
        ansi,
    ));
    if !lists.untracked.is_empty() {
        let hint = terminal::pht(
            "To configure Git to ignore certain files in this working copy, add the file paths \
             to \"%s\".",
            &[terminal::PhtArg::Text(".git/info/exclude")],
        );
        out.push_str(
            &terminal::tsprintf_ansi("%?\n", &[terminal::TVal::Text(&hint)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default(),
        );
    }
    out
}

/// The hg UNCOMMITTED CHANGES block (T1153 WORK-HG: `.hgignore`
/// hint, `ArcanistMercurialLocalState.php:118-124`).
#[must_use]
pub fn render_hg_uncommitted_block(dir: &Path, lists: &DirtyLists, ansi: bool) -> String {
    let working_copy = terminal::tsprintf_ansi(
        "    %s: %s\n",
        &[
            terminal::TVal::Text("Working Copy"),
            terminal::TVal::Text(&hg_repo_display_path(dir)),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    let working_copy = terminal::TerminalString::new(working_copy);
    let mut out = terminal::tsprintf_ansi(
        "\n%!\n%W\n\n%s\n",
        &[
            terminal::TVal::Text("UNCOMMITTED CHANGES"),
            terminal::TVal::Text("You have uncommitted changes in this working copy."),
            terminal::TVal::Terminal(&working_copy),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    out.push_str(&render_file_lists(
        &[
            (
                "Untracked changes in working copy:",
                lists.untracked.as_slice(),
            ),
            (
                "Unstaged changes in working copy:",
                lists.unstaged.as_slice(),
            ),
            (
                "Uncommitted changes in working copy:",
                lists.uncommitted.as_slice(),
            ),
        ],
        ansi,
    ));
    if !lists.untracked.is_empty() {
        let hint = terminal::pht(
            "To configure Mercurial to ignore certain files in the working copy, add them to \
             \".hgignore\".",
            &[],
        );
        out.push_str(
            &terminal::tsprintf_ansi("%?\n", &[terminal::TVal::Text(&hint)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default(),
        );
    }
    out
}

/// The dirty-submodule message with plural variants.
#[must_use]
pub fn externals_message(count: usize) -> String {
    terminal::pht(
        "%s submodule(s) have uncommitted or untracked changes:",
        &[terminal::PhtArg::Number(count as i64, 0)],
    )
}

/// The dirty-submodule confirm query with plural variants.
#[must_use]
pub fn externals_query(count: usize) -> String {
    terminal::pht(
        "Ignore the changes to these %s submodule(s) and continue?",
        &[terminal::PhtArg::Number(count as i64, 0)],
    )
}

/// The dirty-submodule block (stdout): a wrapped paragraph, a blank
/// line, an indented `-` list, and a trailing blank line.
#[must_use]
pub fn render_externals_block(externals: &[String], ansi: bool) -> String {
    let paragraph = terminal::tsprintf_ansi(
        "%s",
        &[terminal::TVal::Text(&externals_message(externals.len()))],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    let mut out = format!("{}\n\n", terminal::console_wrap(&paragraph, 0, true));
    for path in externals {
        out.push_str(&format!("      - {}\n", terminal::escape_value(path, true)));
    }
    out.push('\n');
    out
}

/// Ask `query` until `y`/`n`/empty, mirroring
/// `phutil_console_confirm()`; see [`prompt::console_confirm`]. Like the
/// other commands, input is always read (no TTY gate).
pub fn console_confirm(
    input: &mut dyn BufRead,
    query: &str,
    default_no: bool,
) -> Result<bool, String> {
    let options = if default_no { "[y/N]" } else { "[Y/n]" };
    prompt::console_confirm_with_tty(input, query, options, default_no, true)
}

/// The `arc.state.stash` prompt; see [`prompt::execute_prompt`].
/// Declining aborts with [`ABORT`]. Input is always read (no TTY gate).
pub fn stash_prompt(input: &mut dyn BufRead, query: &str) -> Result<(), String> {
    prompt::execute_prompt(
        input,
        prompt::STASH_PROMPT_KEY,
        prompt::STASH_DESCRIPTION,
        query,
        true,
        None,
    )
}

/// Stash everything (including untracked files) for the duration of `work`.
pub fn save_stash(dir: &Path) -> Result<(), String> {
    git::stdout_text(dir, &["stash", "push", "--include-untracked", "--"])?;
    push_status("SAVE STASH", "Saved uncommitted changes from working copy.");
    Ok(())
}

/// Re-apply a stash saved by [`save_stash`] and drop it.
pub fn apply_stash(dir: &Path) -> Result<(), String> {
    push_status(
        "LOAD STASH",
        "Restoring uncommitted changes to working copy.",
    );
    // NOTE: `stash apply` and `stash drop` do not accept `--` (git 2.21.1).
    git::stdout_text(dir, &["stash", "apply"])?;
    git::stdout_text(dir, &["stash", "drop"])?;
    Ok(())
}

/// `arc-` plus 12 random `[a-z2-7]` characters, like
/// `Filesystem::readRandomCharacters(12)` (T1153 WORK-HG:
/// `src/filesystem/Filesystem.php:538-558`, same as `land`).
fn new_hg_shelve_name() -> String {
    const MAP: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut state = seed | 1;
    let mut name = String::from("arc-");
    for _ in 0..12 {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let byte = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        name.push(MAP[(byte >> 59) as usize] as char);
    }
    name
}

/// Shelve hg changes, returning the shelf name (T1153 WORK-HG:
/// `ArcanistMercurialLocalState.php:147-164`).
pub fn save_hg_shelve_with(dir: &Path, hg_cmd: &hg::HgCommand) -> Result<String, String> {
    let name = new_hg_shelve_name();
    hg::exec_with_extension_with(
        dir,
        hg_cmd,
        "shelve",
        &["shelve", "--unknown", "--name", &name, "--"],
        None,
    )?;
    push_status("SHELVE", "Shelving uncommitted changes from working copy.");
    Ok(name)
}

/// Shelve with the system binary (T1153 WORK-HG).
pub fn save_hg_shelve(dir: &Path) -> Result<String, String> {
    save_hg_shelve_with(dir, &hg::HgCommand::system())
}

/// Unshelve and delete the shelf (T1153 WORK-HG:
/// `ArcanistMercurialLocalState.php:166-185`).
pub fn apply_hg_shelve_with(dir: &Path, hg_cmd: &hg::HgCommand, name: &str) -> Result<(), String> {
    push_status("UNSHELVE", "Restoring uncommitted changes to working copy.");
    hg::exec_with_extension_with(
        dir,
        hg_cmd,
        "shelve",
        &["unshelve", "--keep", "--name", name, "--"],
        None,
    )?;
    hg::exec_with_extension_with(
        dir,
        hg_cmd,
        "shelve",
        &["shelve", "--delete", name, "--"],
        None,
    )?;
    Ok(())
}

/// Unshelve with the system binary (T1153 WORK-HG).
pub fn apply_hg_shelve(dir: &Path, name: &str) -> Result<(), String> {
    apply_hg_shelve_with(dir, &hg::HgCommand::system(), name)
}

fn checkout_branch(dir: &Path, symbol: &str) -> Result<(), String> {
    push_status("BRANCH", &format!("Checking out branch \"{symbol}\"."));
    git::stdout_text(dir, &["checkout", symbol, "--"])
        .map(|_| ())
        .map_err(|message| format!("git checkout failed: {message}"))
}

fn resolve_start_value(dir: &Path, start: Option<String>) -> Result<String, String> {
    match start {
        Some(start) => {
            if canonical_revision(dir, &start).is_none() {
                return Err(format!("Unable to resolve startpoint \"{start}\"."));
            }
            Ok(start)
        }
        None => match default_start(dir) {
            Some(start) if canonical_revision(dir, &start).is_some() => Ok(start),
            Some(start) => Err(format!("Unable to resolve startpoint \"{start}\".")),
            None => Err("Unable to determine a startpoint; pass --start <commit>.".to_string()),
        },
    }
}

fn create_new_branch(dir: &Path, symbol: &str, start: &str) -> Result<(), String> {
    push_status(
        "NEW BRANCH",
        &format!("Creating new branch \"{symbol}\" from \"{start}\"."),
    );
    let mut args: Vec<&str> = vec!["checkout"];
    if start_is_branch(dir, start) {
        args.push("--track");
    }
    args.extend(["-b", symbol, start, "--"]);
    git::stdout_text(dir, &args)
        .map(|_| ())
        .map_err(|message| format!("git checkout failed: {message}"))
}

/// Default hg startpoint: `.` (T1153 WORK-HG:
/// `ArcanistMercurialAPI.php:451-453` returns `.`).
#[must_use]
pub fn hg_default_start() -> String {
    hg::get_working_copy_revision().to_string()
}

/// Resolve an hg rev to a node, or `None` (T1153 WORK-HG:
/// `ArcanistMercurialAPI.php:51-58`).
#[must_use]
pub fn hg_canonical_revision_with(dir: &Path, hg_cmd: &hg::HgCommand, rev: &str) -> Option<String> {
    hg::get_canonical_revision_name_with(dir, hg_cmd, rev).ok()
}

/// Resolve with the system binary (T1153 WORK-HG).
#[must_use]
pub fn hg_canonical_revision(dir: &Path, rev: &str) -> Option<String> {
    hg_canonical_revision_with(dir, &hg::HgCommand::system(), rev)
}

/// Bookmark names from `hg bookmarks` output (T1153 WORK-HG).
fn parse_hg_bookmark_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line
                .trim_start()
                .strip_prefix([' ', '*'])
                .unwrap_or(line.trim_start());
            let name = trimmed.split_whitespace().next().unwrap_or("");
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

/// Branch names from `hg branches` output (T1153 WORK-HG).
fn parse_hg_branch_names(output: &str) -> Vec<String> {
    hg::parse_mercurial_branches(output)
        .map(|map| map.into_keys().collect())
        .unwrap_or_default()
}

/// Markers matching `symbol`: bookmarks plus branches (T1153 WORK-HG:
/// `ArcanistWorkEngine.php:37-57` queries `withNames`; plain `hg`
/// carries the same names as `arc-hg`, like `land`).
fn hg_markers_for_symbol_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    symbol: &str,
) -> Vec<(String, String)> {
    let bookmarks = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    let branches = hg::stdout_text_with(dir, hg_cmd, &["branches"]).unwrap_or_default();
    let mut out = Vec::new();
    for name in parse_hg_bookmark_names(&bookmarks) {
        if name == symbol {
            out.push((name, "bookmark".to_string()));
        }
    }
    for name in parse_hg_branch_names(&branches) {
        if name == symbol {
            out.push((name, "branch".to_string()));
        }
    }
    out
}

/// Check out an hg bookmark or branch (T1153 WORK-HG:
/// `ArcanistMercurialWorkEngine.php:31-54`).
fn hg_checkout_marker_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    name: &str,
    marker_type: &str,
) -> Result<(), String> {
    if marker_type == "bookmark" {
        push_status("BOOKMARK", &format!("Checking out bookmark \"{name}\"."));
    } else {
        push_status("BRANCH", &format!("Checking out branch \"{name}\"."));
    }
    hg::stdout_text_with(dir, hg_cmd, &["checkout", name, "--"])
        .map(|_| ())
        .map_err(|message| format!("hg checkout failed: {message}"))
}

/// Create a new hg bookmark (T1153 WORK-HG:
/// `ArcanistMercurialWorkEngine.php:11-29`).
fn hg_create_bookmark_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    symbol: &str,
    start: &str,
) -> Result<(), String> {
    push_status(
        "NEW BOOKMARK",
        &format!("Creating new bookmark \"{symbol}\" from \"{start}\"."),
    );
    if start != hg_default_start() {
        hg::stdout_text_with(dir, hg_cmd, &["update", "--", start])
            .map_err(|message| format!("hg update failed: {message}"))?;
    }
    hg::stdout_text_with(dir, hg_cmd, &["bookmark", symbol, "--"])
        .map(|_| ())
        .map_err(|message| format!("hg bookmark failed: {message}"))
}

fn hg_resolve_start_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    start: Option<String>,
) -> Result<String, String> {
    match start {
        Some(start) => {
            if hg_canonical_revision_with(dir, hg_cmd, &start).is_none() {
                return Err(format!("Unable to resolve startpoint \"{start}\"."));
            }
            Ok(start)
        }
        None => Ok(hg_default_start()),
    }
}

/// Switch to the branch associated with a revision symbol.
fn work_on_revision(
    dir: &Path,
    symbol: &str,
    parsed: &ObjectSymbol,
    call: &diff_submit::ConduitCall,
) -> Result<(), String> {
    let revision = load_revision(call, parsed)?.ok_or_else(|| {
        format!("No revision \"{symbol}\" exists, or you do not have permission to view it.")
    })?;
    let mut selected = branches_for_revision(call, dir, &revision)?;
    if selected.is_empty() {
        return Err(format!(
            "Revision \"{}\" was not found anywhere in this working copy.",
            revision.monogram
        ));
    }
    selected.sort_by_key(|tip| tip.epoch);
    let target = selected[selected.len() - 1].clone();
    let width = terminal::terminal_width();
    let ansi = terminal::ansi_enabled();
    if selected.len() > 1 {
        print!(
            "{}",
            ambiguous_marker_block(&revision.monogram, &selected, width, ansi)
        );
    }
    push_status("REVISION", "Resuming work on revision:");
    println!("{}", revision_ref_view(&revision, width, ansi));
    checkout_branch(dir, &target.name)
}

/// Tasks resolve through Conduit, but upstream throws here.
fn work_on_task(
    symbol: &str,
    parsed: &ObjectSymbol,
    call: &diff_submit::ConduitCall,
) -> Result<(), String> {
    let found = load_task(call, parsed)?.is_some();
    if !found {
        return Err(format!(
            "No task \"{symbol}\" exists, or you do not have permission to view it."
        ));
    }
    Err("TODO: Implement this workflow.".to_string())
}

fn dispatch_symbol(
    dir: &Path,
    symbol: &str,
    start: Option<String>,
    call: &diff_submit::ConduitCall,
) -> Result<(), String> {
    if branch_exists(dir, symbol) {
        return checkout_branch(dir, symbol);
    }
    if let Some(parsed) = revision_symbol(symbol) {
        return work_on_revision(dir, symbol, &parsed, call);
    }
    if let Some(parsed) = task_symbol(symbol) {
        return work_on_task(symbol, &parsed, call);
    }
    let start = resolve_start_value(dir, start)?;
    create_new_branch(dir, symbol, &start)
}

/// Switch to the hg marker associated with a revision (T1153 WORK-HG).
fn hg_work_on_revision_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    symbol: &str,
    parsed: &ObjectSymbol,
    call: &diff_submit::ConduitCall,
) -> Result<(), String> {
    let revision = load_revision(call, parsed)?.ok_or_else(|| {
        format!("No revision \"{symbol}\" exists, or you do not have permission to view it.")
    })?;
    let mut selected = hg_branches_for_revision_with(call, dir, hg_cmd, &revision)?;
    if selected.is_empty() {
        return Err(format!(
            "Revision \"{}\" was not found anywhere in this working copy.",
            revision.monogram
        ));
    }
    selected.sort_by_key(|tip| tip.epoch);
    let target = selected[selected.len() - 1].clone();
    let width = terminal::terminal_width();
    let ansi = terminal::ansi_enabled();
    if selected.len() > 1 {
        print!(
            "{}",
            ambiguous_marker_block(&revision.monogram, &selected, width, ansi)
        );
    }
    push_status("REVISION", "Resuming work on revision:");
    println!("{}", revision_ref_view(&revision, width, ansi));
    hg_checkout_marker_with(dir, hg_cmd, &target.name, &target.marker_type)
}

fn hg_dispatch_symbol_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    symbol: &str,
    start: Option<String>,
    call: &diff_submit::ConduitCall,
) -> Result<(), String> {
    // T1153 WORK-HG: `ArcanistWorkEngine.php:41-57` moves to the
    // marker, failing on ambiguity.
    let markers = hg_markers_for_symbol_with(dir, hg_cmd, symbol);
    if markers.len() > 1 {
        return Err(format!("Symbol \"{symbol}\" is ambiguous."));
    }
    if let Some((name, marker_type)) = markers.into_iter().next() {
        return hg_checkout_marker_with(dir, hg_cmd, &name, &marker_type);
    }
    if let Some(parsed) = revision_symbol(symbol) {
        return hg_work_on_revision_with(dir, hg_cmd, symbol, &parsed, call);
    }
    if let Some(parsed) = task_symbol(symbol) {
        return work_on_task(symbol, &parsed, call);
    }
    let start = hg_resolve_start_with(dir, hg_cmd, start)?;
    hg_create_bookmark_with(dir, hg_cmd, symbol, &start)
}

/// The externals gate: list dirty submodules and confirm ignoring them.
fn confirm_externals(input: &mut dyn BufRead, lists: &DirtyLists) -> Result<(), String> {
    if lists.externals.is_empty() {
        return Ok(());
    }
    print!(
        "{}",
        render_externals_block(&lists.externals, terminal::ansi_enabled())
    );
    let query = externals_query(lists.externals.len());
    let confirmed = console_confirm(input, &query, false)
        .expect("console_confirm with tty=true never errors (read failures read as empty)");
    if confirmed {
        Ok(())
    } else {
        Err(ABORT.to_string())
    }
}

/// The uncommitted gate: show the block, prompt, and stash. Returns
/// whether a stash was saved (and must be re-applied later).
fn maybe_stash(dir: &Path, input: &mut dyn BufRead, lists: &DirtyLists) -> Result<bool, String> {
    if lists.untracked.is_empty() && lists.unstaged.is_empty() && lists.uncommitted.is_empty() {
        return Ok(false);
    }
    print!(
        "{}",
        render_uncommitted_block(dir, lists, terminal::ansi_enabled())
    );
    stash_prompt(input, "Stash these changes and continue?")?;
    save_stash(dir)?;
    Ok(true)
}

/// The hg uncommitted gate (T1153 WORK-HG): without shelve support
/// hg fails like PHP (`canStashChanges()`,
/// `ArcanistRepositoryLocalState.php:124-148`). Returns the shelf
/// name when shelved.
fn hg_maybe_shelve_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    input: &mut dyn BufRead,
    lists: &DirtyLists,
) -> Result<Option<String>, String> {
    if lists.untracked.is_empty() && lists.unstaged.is_empty() && lists.uncommitted.is_empty() {
        return Ok(None);
    }
    print!(
        "{}",
        render_hg_uncommitted_block(dir, lists, terminal::ansi_enabled())
    );
    if !hg::mercurial_feature_supported_with(dir, hg_cmd, "shelve").unwrap_or(false) {
        return Err(
            "You can not continue with uncommitted changes. Commit or discard them before \
             proceeding."
                .to_string(),
        );
    }
    stash_prompt(input, "Stash these changes and continue?")?;
    save_hg_shelve_with(dir, hg_cmd).map(Some)
}

fn fail_with_restore(dir: &Path, state: &WorkState, stashed: bool, message: String) -> ExitCode {
    // Mirror the destructor restore: check the saved position back out,
    // re-apply the stash, then report the original error.
    if let Err(restore) = restore_work_state(dir, state) {
        eprintln!("arc {NAME}: {restore}");
        return ExitCode::FAILURE;
    }
    if stashed && let Err(message) = apply_stash(dir) {
        eprintln!("arc {NAME}: {message}");
        return ExitCode::FAILURE;
    }
    eprintln!("arc {NAME}: {message}");
    ExitCode::FAILURE
}

fn hg_fail_with_restore_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    state: &HgWorkState,
    shelved: Option<&str>,
    message: String,
) -> ExitCode {
    // T1153 WORK-HG: mirror the git restore (state, then shelf).
    if let Err(restore) = restore_hg_state_with(dir, hg_cmd, state) {
        eprintln!("arc {NAME}: {restore}");
        return ExitCode::FAILURE;
    }
    if let Some(name) = shelved
        && let Err(message) = apply_hg_shelve_with(dir, hg_cmd, name)
    {
        eprintln!("arc {NAME}: {message}");
        return ExitCode::FAILURE;
    }
    eprintln!("arc {NAME}: {message}");
    ExitCode::FAILURE
}

fn execute_parsed(
    dir: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall,
    input: &mut dyn BufRead,
) -> ExitCode {
    let state = match save_work_state(dir) {
        Ok(state) => state,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let lists = match dirty_lists(dir) {
        Ok(lists) => lists,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(message) = confirm_externals(input, &lists) {
        eprintln!("arc {NAME}: {message}");
        return ExitCode::FAILURE;
    }
    let stashed = match maybe_stash(dir, input, &lists) {
        Ok(stashed) => stashed,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    match dispatch_symbol(dir, &options.symbol, options.start.clone(), call) {
        Ok(()) => {
            if stashed && let Err(message) = apply_stash(dir) {
                eprintln!("arc {NAME}: {message}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(message) => fail_with_restore(dir, &state, stashed, message),
    }
}

fn hg_execute_parsed_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall,
    input: &mut dyn BufRead,
) -> ExitCode {
    // T1153 WORK-HG: hg mirrors the git dance (save, gates, dispatch,
    // restore on failure).
    let state = match save_hg_state_with(dir, hg_cmd) {
        Ok(state) => state,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let lists = match hg_dirty_lists_with(dir, hg_cmd) {
        Ok(lists) => lists,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(message) = confirm_externals(input, &lists) {
        eprintln!("arc {NAME}: {message}"); // LCOV_EXCL_START // Reason: hg lists never carry externals, so confirm_externals early-Oks here; this Err arm is dead
        return ExitCode::FAILURE; // LCOV_EXCL_STOP
    }
    let shelved = match hg_maybe_shelve_with(dir, hg_cmd, input, &lists) {
        Ok(shelved) => shelved,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    match hg_dispatch_symbol_with(dir, hg_cmd, &options.symbol, options.start.clone(), call) {
        Ok(()) => {
            if let Some(name) = &shelved
                && let Err(message) = apply_hg_shelve_with(dir, hg_cmd, name)
            {
                eprintln!("arc {NAME}: {message}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(message) => hg_fail_with_restore_with(dir, hg_cmd, &state, shelved.as_deref(), message),
    }
}

fn parse_or_report(argv: &[String]) -> Result<Options, ExitCode> {
    match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            Err(ExitCode::SUCCESS)
        }
        // T1153 B21/B23: the terminator failure prints the three cyan
        // hints first (like `ArcanistRuntime.php:172-188`) and reports
        // through the toolset usage arm (exit 1).
        ParseOutcome::MissingTerminator => {
            arc_rs::help::print_terminator_hints(arc_rs::terminal::ansi_enabled());
            Err(arc_rs::conduit::report(
                arc_rs::conduit::EnvelopeKind::ToolsetUsage,
                arc_rs::help::MISSING_TERMINATOR,
            ))
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc {NAME}: {message}\n\n{}", usage());
            Err(ExitCode::FAILURE)
        }
        ParseOutcome::Args(options) => Ok(options),
    }
}

/// Begin or resume work in `dir` per `argv`, with an injected Conduit
/// caller (`None` builds one from config only when the symbol needs it)
/// and prompt input.
pub fn run_in_with(
    dir: &Path,
    argv: &[String],
    call: Option<&diff_submit::ConduitCall>,
    input: &mut dyn BufRead,
) -> ExitCode {
    let options = match parse_or_report(argv) {
        Ok(options) => options,
        Err(code) => return code,
    };
    // T1153 WORK-HG: hg dispatches to the Mercurial engine.
    if is_hg_repo(dir) {
        return run_hg_in_with(dir, &options, call, input);
    }
    if !git::is_inside_work_tree(dir) {
        return work_outside_repo();
    }
    if let Some(call) = call {
        return execute_parsed(dir, &options, call, input);
    }
    if !branch_exists(dir, &options.symbol) && symbol_needs_conduit(&options.symbol) {
        // LCOV_EXCL_START // Reason: live-client path; needs real creds or fingerprints ambient discovery, neither hermetic
        let client = match config::client_for(None) {
            Ok(client) => client,
            Err(message) => {
                eprintln!("arc {NAME}: {message}");
                return ExitCode::FAILURE;
            }
        };
        let live = |method: &str, params: &serde_json::Value| {
            client.call(method, params).map_err(|e| e.to_string())
        };
        execute_parsed(dir, &options, &live, input) // LCOV_EXCL_STOP
    } else {
        execute_parsed(dir, &options, &conduit_unavailable_pill, input)
    }
}

/// Hg entry with an injected Conduit caller (T1153 WORK-HG).
/// Poison-pill conduit caller for the no-conduit dispatch paths: the gates
/// above guarantee dispatch never calls conduit here, so any call fails
/// closed with the byte-identical message the callers propagate.
fn conduit_unavailable_pill(_: &str, _: &serde_json::Value) -> Result<serde_json::Value, String> {
    Err("conduit unavailable".to_string())
}

fn run_hg_in_with(
    dir: &Path,
    options: &Options,
    call: Option<&diff_submit::ConduitCall>,
    input: &mut dyn BufRead,
) -> ExitCode {
    let hg_cmd = hg::HgCommand::system();
    run_hg_in_with_cmd(dir, &hg_cmd, options, call, input)
}

/// Hg entry driving `hg` through `hg_cmd` (T1153 WORK-HG: test seam).
pub fn run_hg_in_with_cmd(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: Option<&diff_submit::ConduitCall>,
    input: &mut dyn BufRead,
) -> ExitCode {
    if let Some(call) = call {
        return hg_execute_parsed_with(dir, hg_cmd, options, call, input);
    }
    let markers = hg_markers_for_symbol_with(dir, hg_cmd, &options.symbol);
    if markers.is_empty() && symbol_needs_conduit(&options.symbol) {
        // LCOV_EXCL_START // Reason: live-client path; needs real creds or fingerprints ambient discovery, neither hermetic
        let client = match config::client_for(None) {
            Ok(client) => client,
            Err(message) => {
                eprintln!("arc {NAME}: {message}");
                return ExitCode::FAILURE;
            }
        };
        let live = |method: &str, params: &serde_json::Value| {
            client.call(method, params).map_err(|e| e.to_string())
        };
        hg_execute_parsed_with(dir, hg_cmd, options, &live, input) // LCOV_EXCL_STOP
    } else {
        hg_execute_parsed_with(dir, hg_cmd, options, &conduit_unavailable_pill, input)
    }
}

/// Begin or resume work in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    run_in_with(dir, argv, None, &mut input)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "work_tests.rs"]
mod work_tests;
