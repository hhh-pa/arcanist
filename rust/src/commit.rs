//! `arc commit`: commit an accepted revision, or create a git commit.
//!
//! PHP reference: `src/workflow/ArcanistCommitWorkflow.php`, which commits
//! an accepted Differential revision via `svn commit`. Under Subversion
//! this ports that flow: the revision is guessed from the working copy
//! unless `--revision` is given, sanity checks prompt, `svn commit` runs
//! over the revision's paths, and the revision is finalized through
//! `close-revision --finalize`. Under git (an arc-rs extension, since PHP
//! supports svn only) this creates a local commit instead: the message
//! comes from `--message`, or from Differential via `--revision`.
//! `--show` prints the message without committing. Nothing is pushed.
//!
//! Divergences from PHP (all deliberate):
//!
//! - Banners and prompts render as plain text through `land::console_confirm`
//!   (the `phutil_console_confirm` port); prompt queries and error
//!   sentences keep the PHP wording.
//! - Git support plus `-m`/`--all`/`--allow-empty` are pure arc-rs
//!   extensions (T1153 CORE-64): PHP commits svn only
//!   (`ArcanistCommitWorkflow.php:282-284` at ceb).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::conduit::Client;
use crate::{amend, close_revision, config, git, land, svn};

pub const NAME: &str = "commit";

/// T1153 B21/B23: `--help` renders the generated workflow help
/// (`src/workflow/ArcanistCommitWorkflow.php`,
/// `getCommandSynopses()`/`getCommandHelp()`), not a short custom
/// usage string. T1153 RB-D5: the renderer is the shared `crate::help`
/// module; this file keeps only the data. The `-m`/`--message`/
/// `--conduit-uri`/`-a`/`--all`/`--allow-empty` extensions stay
/// functional but undocumented in `--help`, like PHP. Classic
/// workflows never require `--` (B21 exempt: PHP runs them via the
/// `scripts/arcanist.php` re-exec).
const HELP_EXAMPLES: &str = "**commit**";
const HELP_SYNOPSIS: &str = "      **commit** [--revision __revision_id__] [--show]";
const HELP_TEXT: &str =
    "          Supports: svn\n          Commit a revision which has been accepted by a reviewer.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "show",
        None,
        None,
        "Show the command which would be issued, but do not actually commit anything.",
    ),
    (
        "revision",
        None,
        Some("revision_id"),
        "Commit a specific revision. If you do not specify a revision, arc will look for committable revisions.",
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
        false,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(crate::terminal::ansi_enabled())
}

/// Parsed `commit` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub messages: Vec<String>,
    pub all: bool,
    pub allow_empty: bool,
    pub show: bool,
    pub revision: Option<String>,
    pub conduit_uri: Option<String>,
}

impl Options {
    fn empty() -> Self {
        Self {
            messages: Vec::new(),
            all: false,
            allow_empty: false,
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

fn flag_simple(options: &mut Options, arg: &str) -> Option<Step> {
    if arg == "-a" || arg == "--all" {
        options.all = true;
    } else if arg == "--allow-empty" {
        options.allow_empty = true;
    } else if arg == "--show" {
        options.show = true;
    } else {
        return None;
    }
    Some(Step::Handled)
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
        Step::Error(
            "unexpected argument '--': commit takes no paths (stage changes with 'git add' first)"
                .to_string(),
        )
    } else if arg.starts_with('-') {
        Step::Error(format!("unknown flag '{arg}'"))
    } else {
        Step::Error(format!(
            "unexpected argument '{arg}': commit takes no paths (stage changes with 'git add' first)"
        ))
    }
}

fn consume_arg(options: &mut Options, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
    if let Some(step) = flag_simple(options, arg) {
        return step;
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
    let mut options = Options::empty();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        match consume_arg(&mut options, arg, &mut iter) {
            Step::Handled => {}
            Step::Help => return Ok(None),
            Step::Error(message) => return Err(message),
        }
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

fn resolve_message(options: &Options) -> Result<String, String> {
    if !options.messages.is_empty() && options.revision.is_some() {
        return Err("pass only one of --message and --revision".to_string());
    }
    if !options.messages.is_empty() {
        return normalize_message(&options.messages.join("\n\n"));
    }
    if let Some(revision) = &options.revision {
        let id = parse_revision_id(revision)?;
        let client = config::client_for(options.conduit_uri.as_deref())?;
        return normalize_message(&fetch_commit_message(&client, id)?);
    }
    Err("commit requires a message: pass --message <text> or --revision <id>".to_string())
}

/// Monotonic message-file disambiguator: process-unique names without
/// relying on clock granularity (parallel tests share one clock tick).
static MESSAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_message_file(tag: &str, message: &str) -> Result<std::path::PathBuf, String> {
    for _ in 0..100 {
        let path = std::env::temp_dir().join(format!(
            "arc-rs-{tag}-{}-{}.msg",
            std::process::id(),
            MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, message.as_bytes())
                    .map_err(|e| format!("failed to write commit message file: {e}"))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("failed to write commit message file: {error}")),
        }
    }
    Err("failed to allocate a unique commit message file".to_string())
}

/// Git flow (arc-rs extension): create a local commit from an explicit
/// message. Unchanged behavior; prompting lives in the svn flow only.
fn flow_git(dir: &Path, options: &Options) -> Result<String, String> {
    let message = resolve_message(options)?;
    if options.show {
        return Ok(message);
    }
    let file = write_message_file("commit", &message)?;
    let path = file.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["commit", "-F", path.as_str()];
    if options.all {
        args.push("--all");
    }
    if options.allow_empty {
        args.push("--allow-empty");
    }
    let output = git::output(dir, &args);
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
            return Err("git commit failed".to_string());
        }
        return Err(format!("git commit failed: {detail}"));
    }
    let summary = String::from_utf8_lossy(&output.stdout);
    let summary = summary.trim();
    if summary.is_empty() {
        Ok("committed.\n".to_string())
    } else {
        Ok(format!("{summary}\n"))
    }
}

/// Working copy root of the Subversion checkout holding `dir`.
fn svn_root(dir: &Path) -> Result<PathBuf, String> {
    let metadata = svn::find_metadata_path(dir)
        .ok_or_else(|| "not inside a Subversion working copy".to_string())?;
    metadata
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "not inside a Subversion working copy".to_string())
}

/// Reject git-only options under Subversion, where the message always
/// comes from Differential.
fn reject_git_only_options(options: &Options) -> Result<(), String> {
    if !options.messages.is_empty() {
        return Err("the --message option is only supported for git working copies (under Subversion the message comes from Differential)"
            .to_string());
    }
    if options.all {
        return Err("the --all option is only supported for git working copies".to_string());
    }
    if options.allow_empty {
        return Err(
            "the --allow-empty option is only supported for git working copies".to_string(),
        );
    }
    Ok(())
}

/// True when a `sourcePath` row belongs to `root`: exact match, or match
/// after resolving symlinks (macOS temp dirs, Windows `\\?\` prefixes).
fn root_matches_source(root: &Path, source: &str) -> bool {
    let raw = root.to_string_lossy().into_owned();
    if source == raw {
        return true;
    }
    let Ok(canonical) = std::fs::canonicalize(root) else {
        return false;
    };
    let canonical = canonical.to_string_lossy().into_owned();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    source == canonical
}

/// Guess the committable revision: accepted revisions by the viewer whose
/// `sourcePath` is this working copy (`:69-91`,
/// `ArcanistSubversionAPI.php:643-660`).
fn svn_guessed_revision(
    client: &Client,
    user_phid: &str,
    root: &Path,
) -> Result<(serde_json::Value, close_revision::Revision), String> {
    let rows = amend::query_revision_rows(
        client,
        &serde_json::json!({"authors": [user_phid], "status": "status-accepted"}),
    )?;
    let mut matched = Vec::new();
    for row in &rows {
        let source = row
            .get("sourcePath")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if !root_matches_source(root, source) {
            continue;
        }
        if let Some(revision) = close_revision::parse_revision(row) {
            matched.push((row.clone(), revision));
        }
    }
    if matched.is_empty() {
        return Err("Unable to identify the revision in the working copy. Use '--revision <revision_id>' to select a revision."
            .to_string());
    }
    if matched.len() > 1 {
        let mut list = String::new();
        for (_, revision) in &matched {
            list.push_str(&format!("     - D{}: {}\n", revision.id, revision.title));
        }
        return Err(format!(
            "More than one revision exists in the working copy:\n\n{list}\nUse '--revision <revision_id>' to select a revision."
        ));
    }
    Ok(matched.swap_remove(0))
}

/// Load an explicit `--revision` (`:92-105`).
fn svn_explicit_revision(
    client: &Client,
    id: u64,
) -> Result<(serde_json::Value, close_revision::Revision), String> {
    let rows = amend::query_revision_rows(client, &serde_json::json!({"ids": [id]}))?;
    let found = rows.iter().find_map(|row| {
        close_revision::parse_revision(row).map(|revision| (row.clone(), revision))
    });
    found.ok_or_else(|| format!("Revision 'D{id}' does not exist."))
}

/// Resolve the revision to commit: explicit `--revision`, or a guess
/// from the working copy.
fn resolve_svn_revision(
    client: &Client,
    options: &Options,
    user_phid: Option<&str>,
    root: &Path,
) -> Result<(serde_json::Value, close_revision::Revision), String> {
    if let Some(symbol) = &options.revision {
        let id = parse_revision_id(symbol)?;
        return svn_explicit_revision(client, id);
    }
    svn_guessed_revision(client, user_phid.unwrap_or(""), root)
}

/// Confirm every prompt in order, aborting on the first decline
/// (`runSanityChecks`, `:345-349`).
fn confirm_all(io: &mut land::Io, prompts: &[String]) -> Result<(), String> {
    for prompt in prompts {
        if !land::console_confirm(io, prompt, true)? {
            return Err(land::ABORT.to_string());
        }
    }
    Ok(())
}

/// `runSanityChecks` (`:312-350`): confirm revisions which are not
/// accepted, not owned by the viewer, or generated from another branch.
fn svn_sanity_checks(
    io: &mut land::Io,
    svn: &svn::SvnCommand,
    root: &Path,
    row: &serde_json::Value,
    revision: &close_revision::Revision,
    user_phid: &str,
) -> Result<(), String> {
    let mut prompts = Vec::new();
    if revision.status != close_revision::STATUS_ACCEPTED {
        prompts.push(format!(
            "Revision 'D{}: {}' has not been accepted. Commit this revision anyway?",
            revision.id, revision.title
        ));
    }
    if revision.author_phid != user_phid {
        prompts.push(format!(
            "You are not the author of 'D{}: {}'. Commit this revision anyway?",
            revision.id, revision.title
        ));
    }
    let current = svn::get_branch_name_with(root, svn)?;
    let source = row.get("branch").and_then(|value| value.as_str());
    if source != Some(current.as_str()) {
        prompts.push(format!(
            "Revision 'D{}: {}' was generated from '{}', but current working copy root is '{}'. Commit this revision anyway?",
            revision.id,
            revision.title,
            source.unwrap_or(""),
            current
        ));
    }
    confirm_all(io, &prompts)
}

/// `differential.getcommitpaths` for one revision.
fn svn_commit_paths(client: &Client, revision_id: u64) -> Result<Vec<String>, String> {
    let result = client
        .call(
            "differential.getcommitpaths",
            &serde_json::json!({"revision_id": revision_id}),
        )
        .map_err(|e| e.to_string())?;
    Ok(amend::rows_of(&result)
        .iter()
        .filter_map(|row| row.as_str().map(str::to_string))
        .collect())
}

/// Parent directory of a `/`-separated path, `.` at the top.
fn parent_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => ".".to_string(),
    }
}

/// Commit paths plus every parent directory (`:188-196`).
fn expand_commit_dirs(paths: &[String]) -> BTreeSet<String> {
    let mut commit = BTreeSet::new();
    for path in paths {
        commit.insert(path.clone());
        let mut dir = parent_dir(path);
        while dir != "." {
            commit.insert(dir.clone());
            dir = parent_dir(&dir);
        }
    }
    commit
}

/// True when `path` sits strictly under the committed `candidate`
/// directory (`Filesystem::isDescendant`): both must exist on disk, and
/// the candidate must be a directory.
fn is_path_descendant(root: &Path, path: &str, candidate: &str) -> bool {
    if candidate == "." {
        return path != "." && !path.starts_with("..") && svn::join_path(root, Some(path)).exists();
    }
    if !path.starts_with(&format!("{candidate}/")) {
        return false;
    }
    svn::join_path(root, Some(path)).exists() && svn::join_path(root, Some(candidate)).is_dir()
}

/// Check uncommitted status against the commit set (`:200-224`): paths
/// outside the set come back as `modified_but_not_included`, unless one
/// sits under a committed directory, which throws.
fn partition_uncommitted(
    root: &Path,
    status: &svn::StatusMap,
    commit: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let mut skipped = Vec::new();
    for path in status.keys() {
        if commit.contains(path) {
            continue;
        }
        for candidate in commit.iter() {
            if is_path_descendant(root, path, candidate) {
                return Err(format!(
                    "This commit includes the directory '{candidate}', but it contains a modified path ('{path}') which is NOT included in the commit. Subversion can not handle this operation and will commit the path anyway. You need to sort out the working copy changes to '{path}' before you may proceed with the commit."
                ));
            }
        }
        skipped.push(path.clone());
    }
    Ok(skipped)
}

/// Drop commit paths missing from disk (`:235-249`), returning them.
/// Deleted paths stay: Subversion commits the deletion.
fn remove_missing_paths(
    root: &Path,
    status: &svn::StatusMap,
    commit: &mut BTreeSet<String>,
) -> Vec<String> {
    let mut missing = Vec::new();
    let paths: Vec<String> = commit.iter().cloned().collect();
    for path in paths {
        let disk = svn::join_path(root, Some(&path));
        if disk.exists() || disk.is_symlink() {
            continue;
        }
        if status
            .get(&path)
            .is_some_and(|mask| mask & svn::FLAG_DELETED != 0)
        {
            continue;
        }
        missing.push(path.clone());
        commit.remove(&path);
    }
    missing
}

/// `promptFileWarning` (`:272-280`): list the paths, confirm, abort on
/// decline.
fn prompt_file_warning(
    io: &mut land::Io,
    prefix: &str,
    prompt: &str,
    paths: &[String],
) -> Result<(), String> {
    io.push_out(&format!("{prefix}\n\n"));
    for path in paths {
        io.push_out(&format!("    {path}\n"));
    }
    if !land::console_confirm(io, prompt, true)? {
        return Err(land::ABORT.to_string());
    }
    Ok(())
}

/// `getCommitFileList` (`:179-270`): the revision's paths, reconciled with
/// the working copy, escaped for svn.
fn svn_commit_file_list(
    io: &mut land::Io,
    client: &Client,
    svn: &svn::SvnCommand,
    root: &Path,
    revision_id: u64,
) -> Result<Vec<String>, String> {
    let paths = svn_commit_paths(client, revision_id)?;
    let mut commit = expand_commit_dirs(&paths);
    let status = svn::get_svn_status_with(root, svn, &[], false)?;
    let skipped = partition_uncommitted(root, &status.files, &commit)?;
    if !skipped.is_empty() {
        prompt_file_warning(
            io,
            &format!(
                "{} locally modified path(s) are not included in this revision:",
                skipped.len()
            ),
            &format!(
                "These {} path(s) will NOT be committed. Commit this revision anyway?",
                skipped.len()
            ),
            &skipped,
        )?;
    }
    let missing = remove_missing_paths(root, &status.files, &mut commit);
    if !missing.is_empty() {
        prompt_file_warning(
            io,
            &format!(
                "Revision includes changes to {} path(s) that do not exist:",
                missing.len()
            ),
            "Commit this revision anyway?",
            &missing,
        )?;
    }
    let files: Vec<&str> = commit.iter().map(String::as_str).collect();
    let files = svn::escape_file_names_for_svn(&files);
    if files.is_empty() {
        return Err(
            "There is nothing left to commit. None of the modified paths exist.".to_string(),
        );
    }
    Ok(files)
}

/// Ask Phabricator to pick up the push soon (T1153 COMMIT-C3):
/// `askForRepositoryUpdate()` (`ArcanistWorkflow.php:2218-2235`) sends
/// an advisory `diffusion.looksoon` for the working copy's repository.
/// Fully best-effort: repository-resolution failures are swallowed
/// like the nudge itself (the commit already landed, so failing here
/// would report failure for a success), matching
/// [`finalize_revision`]'s untracked swallowing.
fn ask_for_repository_update(client: &Client, dir: &Path) {
    let Ok(Some(phid)) = close_revision::resolve_repository_phid(client, dir) else {
        return;
    };
    let _ = client.call(
        "diffusion.looksoon",
        &serde_json::json!({"repositories": [phid]}),
    );
}

/// Close the committed revision softly, like the `close-revision
/// --finalize` child workflow (`:168-174`). `--finalize` never prompts.
fn finalize_revision(
    client: &Client,
    options: &Options,
    dir: &Path,
    revision_id: u64,
) -> Result<String, String> {
    let close_options = close_revision::Options {
        revision: revision_id.to_string(),
        finalize: true,
        quiet: false,
        json: false,
        force: false,
        conduit_uri: options.conduit_uri.clone(),
        // Unresolvable (e.g. fake backends in tests) means untracked: close.
        repository_phid: close_revision::resolve_repository_phid(client, dir)
            .ok()
            .flatten(),
    };
    let report = close_revision::execute(client, &close_options, &|_| Ok(false))?;
    Ok(format!("{}{}", report.stdout, report.stderr))
}

/// The resolved Subversion commit target: root, client, revision, and
/// Differential message.
struct SvnCommitTarget {
    root: PathBuf,
    client: Client,
    revision: close_revision::Revision,
    id: u64,
    message: String,
}

/// Subversion flow (`run`, `:65-177`): resolve, check, commit, finalize.
fn flow_svn(
    dir: &Path,
    options: &Options,
    io: &mut land::Io,
    svn: &svn::SvnCommand,
) -> Result<String, String> {
    let target = resolve_svn_commit_target(dir, options, io, svn)?;
    if options.show {
        return Ok(target.message);
    }
    io.push_out(&format!(
        "Committing 'D{}: {}'...\n",
        target.revision.id, target.revision.title
    ));
    let files = svn_commit_file_list(io, &target.client, svn, &target.root, target.id)?;
    let file_refs: Vec<&str> = files.iter().map(String::as_str).collect();
    // T1153 CORE-65: the commit streams live like `phutil_passthru`,
    // so only the finalize text is returned here.
    svn::commit_with(&target.root, svn, &file_refs, &target.message)?;
    // T1153 COMMIT-C3: the advisory nudge runs after the commit and
    // before the finalize child (`:165-174`).
    ask_for_repository_update(&target.client, dir);
    finalize_revision(&target.client, options, dir, target.id)
}

/// Resolve the revision to commit, run sanity checks (unless `--show`),
/// and fetch its Differential message.
fn resolve_svn_commit_target(
    dir: &Path,
    options: &Options,
    io: &mut land::Io,
    svn: &svn::SvnCommand,
) -> Result<SvnCommitTarget, String> {
    let root = svn_root(dir)?;
    reject_git_only_options(options)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let user_phid = resolve_svn_user_phid(&client, options)?;
    let (row, revision) = resolve_svn_revision(&client, options, user_phid.as_deref(), &root)?;
    if !options.show {
        svn_sanity_checks(
            io,
            svn,
            &root,
            &row,
            &revision,
            user_phid.as_deref().unwrap_or(""),
        )?;
    }
    let (id, message) = fetch_svn_commit_message(&client, &revision)?;
    Ok(SvnCommitTarget {
        root,
        client,
        revision,
        id,
        message,
    })
}

/// The viewer PHID, needed to guess the revision (no `--revision`) or to
/// run sanity checks (no `--show`).
fn resolve_svn_user_phid(client: &Client, options: &Options) -> Result<Option<String>, String> {
    if options.revision.is_none() || !options.show {
        Ok(Some(close_revision::user_phid(client)?))
    } else {
        Ok(None)
    }
}

/// The numeric id plus the normalized Differential message for `revision`.
fn fetch_svn_commit_message(
    client: &Client,
    revision: &close_revision::Revision,
) -> Result<(u64, String), String> {
    let id = amend::revision_numeric_id(revision)?;
    let message = normalize_message(&fetch_commit_message(client, id)?)?;
    Ok((id, message))
}

/// Run the workflow rooted at `dir` with `svn` as the Subversion backend,
/// returning stdout text or an error message. Prompts go to `io`; with a
/// live `io` they echo as the workflow runs.
pub fn execute_with_io_backends(
    dir: &Path,
    argv: &[String],
    io: &mut land::Io,
    svn: &svn::SvnCommand,
) -> Result<String, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage()),
        Err(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    if git::is_inside_work_tree(dir) {
        return flow_git(dir, &options);
    }
    if svn::find_metadata_path(dir).is_some() {
        return flow_svn(dir, &options, io, svn);
    }
    Err("not inside a git or Subversion working copy".to_string())
}

/// Run the workflow rooted at `dir` with the system `svn` backend.
pub fn execute_with_io(dir: &Path, argv: &[String], io: &mut land::Io) -> Result<String, String> {
    execute_with_io_backends(dir, argv, io, &svn::SvnCommand::system())
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
#[path = "commit_tests.rs"]
mod commit_tests;
