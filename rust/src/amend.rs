//! `arc amend`: amend the working copy, synchronizing the local commit
//! message from Differential.
//!
//! PHP reference: `src/workflow/ArcanistAmendWorkflow.php`. Under git the
//! HEAD message is rewritten via `git commit --amend`, under Mercurial via
//! `hg commit --amend`. The message comes from `--revision` (or is guessed
//! from the working copy) through Differential
//! (`differential.getcommitmessage`); `--show` prints it without amending.
//! Revisions you do not own, revisions unrelated to the working copy, and
//! histories marked immutable prompt before anything is mutated.
//!
//! `-m/--message` is an arc-rs extension for message-only amends without
//! Differential. Divergences from PHP (all deliberate):
//!
//! - Only staged changes are refused under git (PHP refuses any uncommitted
//!   change, but its own TODO calls that overly broad:
//!   `ArcanistAmendWorkflow.php:170-171`). Under Mercurial, dirty tracked
//!   files are refused while untracked files are allowed.
//! - Banners render as plain text through `land::push_banner` (that module
//!   owns TTY styling); ref views colorize on TTYs via `terminal::RefView`
//!   and keep the legacy `**`-marked plain text otherwise. Prompt queries,
//!   error sentences, and ref lines keep the PHP wording.
//! - Guessing scans the HEAD commit only (explicit `Differential Revision:`
//!   ID, else the HEAD commit and tree hashes), where PHP resolves the full
//!   working-copy state through the symbol engine; the observable outcomes
//!   (no revision, several revisions, one revision) match.
//! - The `?` help of the prompts below needs three description lines in
//!   `land_checks::prompt_description` (not wired: that module belongs to
//!   another gap group); prompting itself works without them.
//!
//! The `differential.query` / `user.search` helpers here are shared with
//! the other VCS workflows (`commit`, `bookmarks`, `branches`).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use crate::conduit::Client;
use crate::{
    close_revision, config, diff_local, get_config, git, hg, land, revision, svn, terminal,
};

pub const NAME: &str = "amend";

/// Prompt confirming an unrelated revision (`newPrompts()`, `:45-49`).
pub const PROMPT_UNRELATED: &str = "arc.amend.unrelated";
/// Prompt confirming a revision owned by someone else (`:50-54`).
pub const PROMPT_AUTHOR: &str = "arc.amend.author";
/// Prompt confirming history mutation (`:55-59`).
pub const PROMPT_IMMUTABLE: &str = "arc.amend.immutable";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistAmendWorkflow.php:10-43`).
const HELP_EXAMPLES: &str = "**amend** [options] -- ";
const HELP_SYNOPSIS: &str = "Amend the working copy, synchronizing the local commit message.";
const HELP_TEXT: &str = "Amend the working copy, synchronizing the local commit message from\n\
Differential.\n\
\n\
Supported in Mercurial 2.2 and newer.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "show",
        None,
        None,
        "Show the amended commit message, without modifying the working copy.",
    ),
    (
        "revision",
        None,
        Some("id"),
        "Use the message from a specific revision. If you do not specify a revision, arc will guess which revision is in the working copy.",
    ),
];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
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
    usage_text(crate::terminal::ansi_enabled())
}

/// Parsed `amend` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub messages: Vec<String>,
    pub show: bool,
    pub revision: Option<String>,
    pub conduit_uri: Option<String>,
}

impl Options {
    fn empty() -> Self {
        Self {
            messages: Vec::new(),
            show: false,
            revision: None,
            conduit_uri: None,
        }
    }
}

fn take_value(flag: &str, iter: &mut std::slice::Iter<'_, String>) -> Result<String, String> {
    match iter.next() {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        _ => Err(format!("{flag} requires a non-empty value")),
    }
}

/// One argument's parse step: consumed, `--help`, or a fatal error.
enum Step {
    Handled,
    Help,
    Error(String),
}

fn flag_message(
    options: &mut Options,
    arg: &str,
    iter: &mut std::slice::Iter<'_, String>,
) -> Option<Step> {
    if arg == "-m" || arg == "--message" {
        match take_value(arg, iter) {
            Ok(value) => options.messages.push(value),
            Err(message) => return Some(Step::Error(message)),
        }
        return Some(Step::Handled);
    }
    if let Some(value) = arg.strip_prefix("--message=") {
        if value.is_empty() {
            return Some(Step::Error(
                "--message requires a non-empty value".to_string(),
            ));
        }
        options.messages.push(value.to_string());
        return Some(Step::Handled);
    }
    None
}

fn flag_message_short(options: &mut Options, arg: &str) -> Option<Step> {
    if arg.len() > 2 && arg.starts_with("-m") && !arg.starts_with("--") {
        options.messages.push(arg[2..].to_string());
        Some(Step::Handled)
    } else {
        None
    }
}

fn flag_revision(
    options: &mut Options,
    arg: &str,
    iter: &mut std::slice::Iter<'_, String>,
) -> Option<Step> {
    if arg == "--revision" {
        match take_value(arg, iter) {
            Ok(value) => options.revision = Some(value),
            Err(message) => return Some(Step::Error(message)),
        }
        return Some(Step::Handled);
    }
    if let Some(value) = arg.strip_prefix("--revision=") {
        if value.is_empty() {
            return Some(Step::Error(
                "--revision requires a non-empty value".to_string(),
            ));
        }
        options.revision = Some(value.to_string());
        return Some(Step::Handled);
    }
    None
}

fn flag_conduit(
    options: &mut Options,
    arg: &str,
    iter: &mut std::slice::Iter<'_, String>,
) -> Option<Step> {
    if arg == "--conduit-uri" {
        match take_value(arg, iter) {
            Ok(value) => options.conduit_uri = Some(value),
            Err(message) => return Some(Step::Error(message)),
        }
        return Some(Step::Handled);
    }
    if let Some(value) = arg.strip_prefix("--conduit-uri=") {
        if value.is_empty() {
            return Some(Step::Error(
                "--conduit-uri requires a non-empty value".to_string(),
            ));
        }
        options.conduit_uri = Some(value.to_string());
        return Some(Step::Handled);
    }
    None
}

fn flag_tail(arg: &str) -> Step {
    if arg == "-h" || arg == "--help" {
        Step::Help
    } else if arg == "--" {
        Step::Error("unexpected argument '--': amend takes no paths".to_string())
    } else if arg.starts_with('-') {
        Step::Error(format!("unknown flag '{arg}'"))
    } else {
        Step::Error(format!("unexpected argument '{arg}': amend takes no paths"))
    }
}

fn consume_arg(options: &mut Options, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
    if arg == "--show" {
        options.show = true;
        return Step::Handled;
    }
    if let Some(step) = flag_message(options, arg, iter) {
        return step;
    }
    if let Some(step) = flag_message_short(options, arg) {
        return step;
    }
    if let Some(step) = flag_revision(options, arg, iter) {
        return step;
    }
    if let Some(step) = flag_conduit(options, arg, iter) {
        return step;
    }
    flag_tail(arg)
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    parse_options_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_options`] with an injected TTY flag (deterministic tests).
/// T1153 B21/B23: piped runs without `--` fail with
/// `MISSING_TERMINATOR` (checked after unknown-flag scanning, like
/// PHP's `parseFull()`, but before success). A lone `--` is the
/// terminator (accepted); `--help` after `--` is a positional
/// (rejected, like PHP). Vs ceb `ArcanistRuntime.php:80-86,172-188`
/// and `PhutilArgumentParser.php:331-336`.
fn parse_options_with_tty(argv: &[String], stdin_tty: bool) -> Result<Option<Options>, String> {
    let missing = crate::help::terminator_missing(argv, stdin_tty);
    let mut options = Options::empty();
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            return Err(format!("unexpected argument '{arg}': amend takes no paths"));
        }
        if arg == "--" {
            end_of_flags = true;
            continue;
        }
        match consume_arg(&mut options, arg, &mut iter) {
            Step::Handled => {}
            Step::Help => {
                if missing {
                    return Err(crate::help::MISSING_TERMINATOR.to_string());
                }
                return Ok(None);
            }
            Step::Error(message) => return Err(message),
        }
    }
    if missing {
        return Err(crate::help::MISSING_TERMINATOR.to_string());
    }
    Ok(Some(options))
}

/// Accept `123` or `D123` revision symbols, mirroring
/// `ArcanistWorkflow::normalizeRevisionID`.
pub fn parse_revision_id(text: &str) -> Result<u64, String> {
    let digits = text
        .strip_prefix('D')
        .or_else(|| text.strip_prefix('d'))
        .unwrap_or(text);
    if !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(id) = digits.parse::<u64>()
        && id > 0
    {
        return Ok(id);
    }
    Err(format!(
        "invalid revision id '{text}' (expected '123' or 'D123')"
    ))
}

/// Apply arc message conventions: the message must be non-empty and ends
/// with exactly one newline.
pub fn normalize_message(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("commit message is empty".to_string());
    }
    Ok(format!("{trimmed}\n"))
}

/// Fetch the commit message for `revision_id` from Differential.
pub fn fetch_commit_message(client: &Client, revision_id: u64) -> Result<String, String> {
    let params = serde_json::json!({"revision_id": revision_id, "edit": false});
    let result = client
        .call("differential.getcommitmessage", &params)
        .map_err(|e| e.to_string())?;
    result.as_str().map(str::to_string).ok_or_else(|| {
        "conduit differential.getcommitmessage returned a non-string result".to_string()
    })
}

/// Working-copy hints for revision guessing: the explicit ID in HEAD's
/// message, if any, plus hashes for the `commitHashes` fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hints {
    /// Explicit `Differential Revision:` ID from HEAD's message.
    pub message_id: Option<u64>,
    /// HEAD commit hash.
    pub hash: String,
    /// HEAD tree hash (git only; `None` under Mercurial).
    pub tree: Option<String>,
}

/// Parse the explicit revision ID from one commit message.
///
/// PHP: `ArcanistDifferentialCommitMessage::newFromRawCorpus` inside
/// `loadWorkingCopyDifferentialRevisions`; like PHP, an invalid field is
/// an error, not a silent skip.
pub fn revision_id_in_message(message: &str) -> Result<Option<u64>, String> {
    diff_local::parse_revision_id_from_raw_corpus(message)
}

/// Guessing hints for a git working copy: HEAD's message plus the HEAD
/// commit and tree hashes.
pub fn git_hints(dir: &Path) -> Result<Hints, String> {
    let message = git::commit_message(dir, "HEAD")?;
    let hash = git::working_copy_revision(dir)?;
    let tree = git::stdout_text(dir, &["show", "-s", "--format=%T", "HEAD", "--"])?;
    Ok(Hints {
        message_id: revision_id_in_message(&message)?,
        hash,
        tree: Some(tree),
    })
}

/// Guessing hints for a Mercurial working copy: the current commit's
/// message plus its node hash.
pub fn hg_hints(dir: &Path, hg: &hg::HgCommand) -> Result<Hints, String> {
    let message = hg::get_commit_message_with(dir, hg, ".")?;
    let hash = hg::get_canonical_revision_name_with(dir, hg, ".")?;
    Ok(Hints {
        message_id: revision_id_in_message(&message)?,
        hash,
        tree: None,
    })
}

/// Rows of a `differential.query`-style result: arrays in order, `data`
/// lists for search methods, other objects in sorted-key order so output
/// never depends on map iteration order.
pub fn rows_of(result: &serde_json::Value) -> Vec<serde_json::Value> {
    match result {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(items)) = map.get("data") {
                return items.clone();
            }
            object_rows(map)
        }
        _ => Vec::new(),
    }
}

/// Object rows in sorted-key order.
fn object_rows(map: &serde_json::Map<String, serde_json::Value>) -> Vec<serde_json::Value> {
    let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .map(|(_, value)| value.clone())
        .collect()
}

/// Run `differential.query` with `params`, returning every result row.
pub fn query_revision_rows(
    client: &Client,
    params: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, String> {
    let result = client
        .call("differential.query", params)
        .map_err(|e| e.to_string())?;
    Ok(rows_of(&result))
}

/// Parse every row into a revision, skipping rows without an id.
pub fn parse_revisions(rows: &[serde_json::Value]) -> Vec<close_revision::Revision> {
    rows.iter()
        .filter_map(close_revision::parse_revision)
        .collect()
}

/// Guess working-copy revisions: the explicit message ID first, else the
/// HEAD hashes — mirroring `loadWorkingCopyDifferentialRevisions`
/// (`ArcanistGitAPI.php:1180-1241`, `ArcanistMercurialAPI.php:551-615`).
/// `hash_kind` is `gtcm` (git) or `hgcm` (Mercurial).
pub fn guess_revisions(
    client: &Client,
    hints: &Hints,
    hash_kind: &str,
) -> Result<Vec<close_revision::Revision>, String> {
    if let Some(id) = hints.message_id {
        let rows = query_revision_rows(client, &serde_json::json!({"ids": [id]}))?;
        return Ok(parse_revisions(&rows));
    }
    let mut hashes = vec![vec![hash_kind, hints.hash.as_str()]];
    if let Some(tree) = &hints.tree {
        hashes.push(vec!["gttr", tree.as_str()]);
    }
    let rows = query_revision_rows(client, &serde_json::json!({"commitHashes": hashes}))?;
    Ok(parse_revisions(&rows))
}

/// IDs of the revisions `guess_revisions` finds: the local set for the
/// unrelated-revision check.
pub fn local_revision_ids(
    client: &Client,
    hints: &Hints,
    hash_kind: &str,
) -> Result<Vec<String>, String> {
    Ok(guess_revisions(client, hints, hash_kind)?
        .iter()
        .map(|revision| revision.id.clone())
        .collect())
}

/// Pick the single guessed revision with the process ANSI setting
/// (`selectRevisionRef`, `:180-203`).
pub fn select_guessed(
    io: &mut land::Io,
    revisions: Vec<close_revision::Revision>,
) -> Result<close_revision::Revision, String> {
    select_guessed_ansi(io, revisions, terminal::ansi_enabled())
}

/// Pick the single guessed revision with an explicit ANSI setting
/// (deterministic under test): none and multiple are usage errors; the
/// multiple case lists the candidates first, like PHP. The banner stays
/// plain (rendered by `land`); the ref-view list colorizes on TTYs.
pub fn select_guessed_ansi(
    io: &mut land::Io,
    revisions: Vec<close_revision::Revision>,
    ansi: bool,
) -> Result<close_revision::Revision, String> {
    if revisions.is_empty() {
        return Err("No revision specified with \"--revision\", and no revisions found that match the current working copy state. Use \"--revision <id>\" to specify which revision you want to amend."
            .to_string());
    }
    if revisions.len() > 1 {
        land::push_banner(
            io,
            "MULTIPLE REVISIONS IN WORKING COPY",
            "More than one revision was found in the working copy:",
        );
        for revision in &revisions {
            io.push_out(&ref_view_line_ansi(
                &format!("D{}", revision.id),
                &revision.title,
                ansi,
            ));
        }
        return Err(
            "Use \"--revision <id>\" to specify which revision you want to amend.".to_string(),
        );
    }
    let mut revisions = revisions;
    Ok(revisions.swap_remove(0))
}

/// Load one revision for a `123`/`D123` symbol, like the symbol engine's
/// `loadRevisionForSymbol` (`:97-105`).
pub fn load_revision_for_symbol(
    client: &Client,
    symbol: &str,
) -> Result<close_revision::Revision, String> {
    let id = parse_revision_id(symbol)?;
    load_revision_by_id(client, id, symbol)
}

/// Load one revision by numeric id, reporting `symbol` (as given) when
/// the server knows no such revision.
fn load_revision_by_id(
    client: &Client,
    id: u64,
    symbol: &str,
) -> Result<close_revision::Revision, String> {
    let revision = close_revision::query_revision(client, &id.to_string())?;
    revision.ok_or_else(|| {
        format!("Revision \"{symbol}\" does not exist, or you do not have permission to see it.")
    })
}

/// Numeric id of a looked-up revision, for `differential.getcommitmessage`.
pub fn revision_numeric_id(revision: &close_revision::Revision) -> Result<u64, String> {
    revision
        .id
        .parse::<u64>()
        .map_err(|_| format!("revision D{} has a non-numeric id", revision.id))
}

/// One `ArcanistRefView` line (`src/ref/ArcanistRefView.php`) with the
/// process ANSI setting.
#[must_use]
pub fn ref_view_line(object: &str, title: &str) -> String {
    ref_view_line_ansi(object, title, terminal::ansi_enabled())
}

/// One `ArcanistRefView` line with an explicit ANSI setting (deterministic
/// under test): the legacy `**`-marked plain text without ANSI, else the
/// cyan-marker/bold-object TTY rendering through [`terminal::RefView`]
/// (matching PHP `phutil_console_format` byte-for-byte, title truncation
/// included).
#[must_use]
pub fn ref_view_line_ansi(object: &str, title: &str, ansi: bool) -> String {
    if !ansi {
        return ref_view_plain(object, title);
    }
    terminal::RefView {
        object_name: Some(object.to_string()),
        title: Some(title.to_string()),
        ..terminal::RefView::default()
    }
    .render(0, land::REF_VIEW_TERMINAL_WIDTH, true)
}

/// The legacy plain-text ref-view line: `**  *  ** D123 Title`.
fn ref_view_plain(object: &str, title: &str) -> String {
    if title.is_empty() {
        format!("**  *  ** {object}\n")
    } else {
        format!("**  *  ** {object} {title}\n")
    }
}

/// Look one user up by PHID: `(username, real_name)`.
///
/// PHP: `ArcanistUserSymbolHardpointQuery` (`user.search` with `phids`).
/// `None` when the server knows no such user.
pub fn query_user(client: &Client, phid: &str) -> Result<Option<(String, String)>, String> {
    let result = client
        .call(
            "user.search",
            &revision::conduit_search_params(&serde_json::json!({"phids": [phid]}), None),
        )
        .map_err(|e| e.to_string())?;
    Ok(rows_of(&result).iter().filter_map(user_name_of).next())
}

/// `(username, real_name)` of one `user.search` row, if it names a user.
fn user_name_of(row: &serde_json::Value) -> Option<(String, String)> {
    let fields = row.get("fields")?;
    let username = fields.get("username")?.as_str()?.to_string();
    let real = fields
        .get("realName")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    Some((username, real))
}

/// One Differential revision annotating a bookmark or branch head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedRevision {
    /// `D123` monogram.
    pub monogram: String,
    /// Review status display name (`Accepted`, ...).
    pub status: String,
    /// Revision title.
    pub title: String,
}

/// Annotate revision ids with monogram + status + title in one batched
/// `differential.query` call, keyed by id string.
pub fn annotate_ids(
    client: &Client,
    ids: &[u64],
) -> Result<BTreeMap<String, AnnotatedRevision>, String> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = query_revision_rows(client, &serde_json::json!({"ids": ids}))?;
    let mut map = BTreeMap::new();
    for revision in parse_revisions(&rows) {
        map.insert(
            revision.id.clone(),
            AnnotatedRevision {
                monogram: format!("D{}", revision.id),
                status: revision.status_name.clone(),
                title: revision.title.clone(),
            },
        );
    }
    Ok(map)
}

/// Per-head revision ids plus the batched annotation map.
pub type MessageAnnotations = (Vec<Option<u64>>, BTreeMap<String, AnnotatedRevision>);

/// Parse head messages into per-head ids plus the batched annotation map:
/// the shared `bookmarks`/`branches` review-status lookup. Like guessing,
/// an invalid `Differential Revision:` field is an error.
pub fn annotations_for_messages(
    client: &Client,
    messages: &[String],
) -> Result<MessageAnnotations, String> {
    let mut ids = Vec::new();
    let mut head_ids = Vec::new();
    for message in messages {
        let id = diff_local::parse_revision_id_from_raw_corpus(message)?;
        if let Some(id) = id
            && !ids.contains(&id)
        {
            ids.push(id);
        }
        head_ids.push(id);
    }
    let annotations = annotate_ids(client, &ids)?;
    Ok((head_ids, annotations))
}

/// `confirmAmendAuthor` (`:205-243`): warn and prompt when the viewer did
/// not author the revision. Unknown authors skip the warning, like PHP.
fn confirm_author(
    io: &mut land::Io,
    client: &Client,
    revision: &close_revision::Revision,
    ansi: bool,
) -> Result<(), String> {
    let viewer = close_revision::user_phid(client)?;
    if viewer == revision.author_phid || revision.author_phid.is_empty() {
        return Ok(());
    }
    let Some((username, real)) = query_user(client, &revision.author_phid)? else {
        return Ok(());
    };
    let monogram = format!("D{}", revision.id);
    land::push_banner(
        io,
        "NOT REVISION AUTHOR",
        &format!(
            "You are amending the working copy using information from a revision you are not the author of.\n\nThe author of this revision ({monogram}) is:"
        ),
    );
    let title = if real.is_empty() {
        String::new()
    } else {
        format!("({real})")
    };
    io.push_out(&ref_view_line_ansi(&format!("@{username}"), &title, ansi));
    land::prompt_for_key(
        io,
        PROMPT_AUTHOR,
        &format!("Amend working copy using revision owned by @{username}?"),
    )
}

/// `confirmAmendNotFound` (`:245-274`): warn and prompt when the revision
/// is absent from the working copy's local revision ids.
fn confirm_related(
    io: &mut land::Io,
    local_ids: &[String],
    revision: &close_revision::Revision,
) -> Result<(), String> {
    if local_ids.iter().any(|id| id == &revision.id) {
        return Ok(());
    }
    land::push_banner(
        io,
        "UNRELATED REVISION",
        "You are amending the working copy using information from a revision that does not appear to be associated with the current state of the working copy.",
    );
    land::prompt_for_key(
        io,
        PROMPT_UNRELATED,
        &format!(
            "Amend working copy using unrelated revision D{}?",
            revision.id
        ),
    )
}

/// `isHistoryImmutable` (`ArcanistWorkflow.php`) over loaded sources:
/// the `history.immutable` config value wins, else the VCS default.
/// Non-boolean values are ignored: only booleans are unambiguous in JSON.
pub fn history_is_immutable_in(sources: &get_config::Sources, default: bool) -> bool {
    if let Some((_, value)) = get_config::current_value("history.immutable", sources)
        && let Some(flag) = value.as_bool()
    {
        return flag;
    }
    default
}

/// `isHistoryImmutable` (`ArcanistWorkflow.php`) for a working copy.
pub fn history_is_immutable(dir: &Path, default: bool) -> Result<bool, String> {
    let user_file = config::arcrc_path();
    let sources = get_config::load_sources(dir, user_file.as_deref())?;
    Ok(history_is_immutable_in(&sources, default))
}

/// Prompt before mutating an immutable history (`requireAmendSupport`,
/// `:151-165`).
fn confirm_mutable(io: &mut land::Io, dir: &Path, default_immutable: bool) -> Result<(), String> {
    if !history_is_immutable(dir, default_immutable)? {
        return Ok(());
    }
    land::push_banner(
        io,
        "IMMUTABLE WORKING COPY",
        "This working copy is configured to have an immutable local history, using the \"history.immutable\" configuration option. Amending the working copy will mutate local history.",
    );
    land::prompt_for_key(
        io,
        PROMPT_IMMUTABLE,
        "Are you sure you want to mutate history?",
    )
}

/// Refuse when staged changes exist, so `amend` only ever rewrites the
/// message and never folds staged content into HEAD silently.
fn ensure_nothing_staged(dir: &Path) -> Result<(), String> {
    let output = git::output(dir, &["diff", "--cached", "--quiet"])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(
            "you have staged changes; commit or unstage them before amending (refusing to amend content silently)"
                .to_string(),
        )
    }
}

/// Refuse Mercurial amends with dirty tracked files: `hg commit --amend`
/// would fold them into the commit. Untracked files can never be folded,
/// so they are allowed, matching the git rule above.
fn ensure_hg_clean(dir: &Path, hg: &hg::HgCommand) -> Result<(), String> {
    let status = hg::get_uncommitted_status_with(dir, hg)?;
    let dirty = status
        .values()
        .any(|flags| flags & !hg::FLAG_UNTRACKED != 0);
    if dirty {
        return Err(
            "You have uncommitted changes in this working copy. Commit or revert them before proceeding."
                .to_string(),
        );
    }
    Ok(())
}

/// `requireAmendSupport` for git (`:134-178`): git always supports amend;
/// immutable histories prompt, staged changes fail.
fn require_git_amend_support(dir: &Path, io: &mut land::Io) -> Result<(), String> {
    confirm_mutable(io, dir, git::is_history_default_immutable())?;
    ensure_nothing_staged(dir)
}

/// `requireAmendSupport` for Mercurial (`:134-178`): old servers fail,
/// immutable histories prompt, dirty files fail.
fn require_hg_amend_support(
    dir: &Path,
    hg: &hg::HgCommand,
    io: &mut land::Io,
) -> Result<(), String> {
    if !hg::supports_amend_with(dir, hg)? {
        return Err("\"arc amend\" is only supported under Mercurial 2.2 or newer. Older versions of Mercurial do not support the \"--amend\" flag to \"hg commit ...\", which this workflow requires."
            .to_string());
    }
    confirm_mutable(io, dir, hg::is_history_default_immutable())?;
    ensure_hg_clean(dir, hg)
}

/// `-m` message, if given. The `--message`/`--revision` conflict is
/// rejected up front in [`execute_with_io_backends`].
fn message_from_options(options: &Options) -> Result<Option<String>, String> {
    if options.messages.is_empty() {
        return Ok(None);
    }
    normalize_message(&options.messages.join("\n\n")).map(Some)
}

/// Conduit client for revision resolution: explicit revisions report
/// client errors raw, guessing hints at the explicit sources.
fn revision_client(options: &Options) -> Result<Client, String> {
    if options.revision.is_some() {
        config::client_for(options.conduit_uri.as_deref())
    } else {
        config::client_for(options.conduit_uri.as_deref()).map_err(|error| {
            format!(
                "unable to guess the revision from the working copy ({error}); pass --message <text> or --revision <id> instead"
            )
        })
    }
}

/// Resolve the revision to amend from: explicit `--revision`, or a guess
/// from the working copy (`:94-105`). Symbols validate before any client
/// is built, so bad symbols report as such without Conduit.
fn resolve_revision(
    options: &Options,
    io: &mut land::Io,
    hints: &Hints,
    hash_kind: &str,
    ansi: bool,
) -> Result<(Client, close_revision::Revision), String> {
    if let Some(symbol) = &options.revision {
        let id = parse_revision_id(symbol)?;
        let client = config::client_for(options.conduit_uri.as_deref())?;
        let revision = load_revision_by_id(&client, id, symbol)?;
        return Ok((client, revision));
    }
    let client = revision_client(options)?;
    let revision = select_guessed_ansi(io, guess_revisions(&client, hints, hash_kind)?, ansi)?;
    Ok((client, revision))
}

/// `echo tsprintf("%s\n\n%s\n", ...)` (`:107-112`): the announce line plus
/// the revision ref view (a terminal value, substituted raw).
fn announce_text(revision: &close_revision::Revision, ansi: bool) -> String {
    let line = ref_view_line_ansi(&format!("D{}", revision.id), &revision.title, ansi);
    let view = terminal::TerminalString::new(line);
    terminal::tsprintf_ansi(
        "%s\n\n%s\n",
        &[
            terminal::TVal::Text("Amending commit message to reflect revision:"),
            terminal::TVal::Terminal(&view),
        ],
        ansi,
    )
    .map(|rendered| rendered.to_string())
    .unwrap_or_default()
}

/// Print the announce text, then run the author and unrelated-revision
/// confirmations (`:107-115`). Guessed revisions skip the unrelated
/// check: they are local by construction.
fn announce_and_confirm(
    io: &mut land::Io,
    client: &Client,
    options: &Options,
    revision: &close_revision::Revision,
    hints: &Hints,
    hash_kind: &str,
    ansi: bool,
) -> Result<(), String> {
    io.push_out(&announce_text(revision, ansi));
    confirm_author(io, client, revision, ansi)?;
    if options.revision.is_some() {
        let local = local_revision_ids(client, hints, hash_kind)?;
        confirm_related(io, &local, revision)?;
    }
    Ok(())
}

/// `--show` with an explicit source: no working copy needed (`:80-92`).
fn show_without_state(options: &Options) -> Result<String, String> {
    if let Some(message) = message_from_options(options)? {
        return Ok(message);
    }
    let Some(symbol) = options.revision.as_deref() else {
        return Err("amend --show requires --message <text> or --revision <id>".to_string());
    };
    let id = parse_revision_id(symbol)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let revision = load_revision_by_id(&client, id, symbol)?;
    let numeric = revision_numeric_id(&revision)?;
    normalize_message(&fetch_commit_message(&client, numeric)?)
}

fn write_message_file(tag: &str, message: &str) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "arc-rs-{tag}-{}-{}.msg",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, message)
        .map_err(|e| format!("failed to write commit message file: {e}"))?;
    Ok(path)
}

/// Rewrite the HEAD message via `git commit --amend`, returning git's
/// summary.
fn amend_git(dir: &Path, message: &str) -> Result<String, String> {
    amend_git_with(dir, message, None)
}

/// [`amend_git`] with the `git` program override injected (test seam;
/// production passes `None`, which spawns `git`).
fn amend_git_with(dir: &Path, message: &str, git_program: Option<&Path>) -> Result<String, String> {
    let file = write_message_file("amend", message)?;
    let path = file.to_string_lossy().to_string();
    let args: Vec<&str> = vec!["commit", "--amend", "--allow-empty", "-F", path.as_str()];
    let output = git::output_with_git(dir, &args, git_program);
    std::fs::remove_file(&file).ok();
    let output = output?;
    if !output.status.success() {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = detail.trim();
        if detail.is_empty() {
            return Err("git commit --amend failed".to_string());
        }
        return Err(format!("git commit --amend failed: {detail}"));
    }
    let summary = String::from_utf8_lossy(&output.stdout);
    let summary = summary.trim();
    if summary.is_empty() {
        Ok("amended.\n".to_string())
    } else {
        Ok(format!("{summary}\n"))
    }
}

/// Rewrite the current commit message via `hg commit --amend`.
fn amend_hg(dir: &Path, hg: &hg::HgCommand, message: &str) -> Result<String, String> {
    hg::amend_commit_with(dir, hg, Some(message), None)?;
    Ok("amended.\n".to_string())
}

/// Print (`--show`) or amend the resolved message under git.
fn finish_git(dir: &Path, options: &Options, message: &str) -> Result<String, String> {
    if options.show {
        return Ok(message.to_string());
    }
    amend_git(dir, message)
}

/// Print (`--show`) or amend the resolved message under Mercurial.
fn finish_hg(
    dir: &Path,
    options: &Options,
    hg: &hg::HgCommand,
    message: &str,
) -> Result<String, String> {
    if options.show {
        return Ok(message.to_string());
    }
    amend_hg(dir, hg, message)
}

/// Message for a resolved revision, via `differential.getcommitmessage`.
fn message_for_revision(
    client: &Client,
    revision: &close_revision::Revision,
) -> Result<String, String> {
    let id = revision_numeric_id(revision)?;
    normalize_message(&fetch_commit_message(client, id)?)
}

/// Git flow: support checks, revision resolution, confirmations, then the
/// message is printed (`--show`) or amended into HEAD.
fn flow_git(
    dir: &Path,
    options: &Options,
    io: &mut land::Io,
    ansi: bool,
) -> Result<String, String> {
    if !options.show {
        require_git_amend_support(dir, io)?;
    }
    if let Some(message) = message_from_options(options)? {
        return finish_git(dir, options, &message);
    }
    let hints = git_hints(dir)?;
    let (client, revision) = resolve_revision(options, io, &hints, "gtcm", ansi)?;
    if !options.show {
        announce_and_confirm(io, &client, options, &revision, &hints, "gtcm", ansi)?;
    }
    let message = message_for_revision(&client, &revision)?;
    finish_git(dir, options, &message)
}

/// Mercurial flow: same shape as [`flow_git`].
fn flow_hg(
    dir: &Path,
    options: &Options,
    io: &mut land::Io,
    hg: &hg::HgCommand,
    ansi: bool,
) -> Result<String, String> {
    if !options.show {
        require_hg_amend_support(dir, hg, io)?;
    }
    if let Some(message) = message_from_options(options)? {
        return finish_hg(dir, options, hg, &message);
    }
    let hints = hg_hints(dir, hg)?;
    let (client, revision) = resolve_revision(options, io, &hints, "hgcm", ansi)?;
    if !options.show {
        announce_and_confirm(io, &client, options, &revision, &hints, "hgcm", ansi)?;
    }
    let message = message_for_revision(&client, &revision)?;
    finish_hg(dir, options, hg, &message)
}

/// Run the workflow rooted at `dir` with `hg` as the Mercurial backend and
/// the process ANSI setting, returning stdout text or an error message.
/// Prompts and banners go to `io`; with a live `io` they echo as the
/// workflow runs.
pub fn execute_with_io_backends(
    dir: &Path,
    argv: &[String],
    io: &mut land::Io,
    hg: &hg::HgCommand,
) -> Result<String, String> {
    execute_with_io_backends_ansi(dir, argv, io, hg, terminal::ansi_enabled())
}

/// [`execute_with_io_backends`] with an explicit ANSI setting
/// (deterministic under test).
pub fn execute_with_io_backends_ansi(
    dir: &Path,
    argv: &[String],
    io: &mut land::Io,
    hg: &hg::HgCommand,
    ansi: bool,
) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        // T1153 B21/B23: the terminator failure carries no usage dump
        // (toolset `USAGE EXCEPTION`, like PHP).
        Err(message) if message == crate::help::MISSING_TERMINATOR => return Err(message),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    if !options.messages.is_empty() && options.revision.is_some() {
        return Err("pass only one of --message and --revision".to_string());
    }
    // PHP `need_state`: `--show` with an explicit source touches neither
    // the working copy nor the VCS (`:80-92`).
    if options.show && (!options.messages.is_empty() || options.revision.is_some()) {
        return show_without_state(&options);
    }
    if git::is_inside_work_tree(dir) {
        return flow_git(dir, &options, io, ansi);
    }
    if hg::find_root(dir).is_some() {
        return flow_hg(dir, &options, io, hg, ansi);
    }
    if svn::find_metadata_path(dir).is_some() {
        return Err("\"arc amend\" must be run from inside a working copy of a repository using a version control system that supports amending commits, like Git or Mercurial."
            .to_string());
    }
    Err("not inside a git or Mercurial working copy".to_string())
}

/// Run the workflow rooted at `dir` with the system `hg` backend.
pub fn execute_with_io(dir: &Path, argv: &[String], io: &mut land::Io) -> Result<String, String> {
    execute_with_io_backends(dir, argv, io, &hg::HgCommand::system())
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let mut io = land::Io::buffered();
    match execute_with_io(dir, argv, &mut io) {
        Ok(main) => Ok(format!("{}{}", io.out.stdout, main)),
        Err(message) => Err(format!("{}{}", io.out.stdout, message)),
    }
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute_with_io(dir, argv, &mut land::Io::live()) {
        Ok(main) => {
            print!("{main}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 B21/B23: the terminator failure prints the three
            // cyan hints first (like `ArcanistRuntime.php:172-188`)
            // and reports through the toolset usage arm (exit 1).
            if message == crate::help::MISSING_TERMINATOR {
                crate::help::print_terminator_hints(crate::terminal::ansi_enabled());
                return crate::conduit::report(
                    crate::conduit::EnvelopeKind::ToolsetUsage,
                    &message,
                );
            }
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
#[path = "amend_tests.rs"]
mod amend_tests;
