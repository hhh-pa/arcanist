//! `arc-rs diff`: local change inspection, plus opt-in Differential submit.
//!
//! Read-only half of `src/workflow/ArcanistDiffWorkflow.php` by default:
//! shows the working copy diff against a base revision as text or JSON.
//! Message-field parsing (`--message-file`, `--reviewers`, `--cc`),
//! commit-range selection (positional `[commit]`, `--head`), and the submit
//! flows (`--create`, `--update`, `--only`) reuse `diff_local` and
//! `diff_submit`. Flag names and Conduit payload keys are verbatim from PHP.
//!
//! Divergences from `ArcanistDiffWorkflow::run()` (`:345-577`):
//! - PHP always submits to Differential; this port stays read-only unless
//!   `--create`, `--update`, or `--only` is given.
//! - Submitted changes carry file metadata only (no hunk corpora): the
//!   unified-diff parser (`generateChanges()`, `:802-1026`) is not ported.
//! - `--raw`, `--raw-command`, `--edit`, `--verbatim`, `--draft`, lint/unit
//!   execution, commit amend, and browser opening are not ported.

extern crate self as arc_rs;

use std::path::Path;
use std::process::{Command, ExitCode};

use arc_rs::{config, diff_local, diff_submit};

/// Subcommand name, matching `arc diff`.
pub const NAME: &str = "diff";

const USAGE: &str = "usage: arc-rs diff [--base REV] [--head REV] [--json] [--message MSG] [--message-file FILE] [--reviewers NAMES] [--cc NAMES] [--create|--update REV|--only] [--conduit-uri URI] [commit]\n";

/// Parsed `diff` options.
///
/// Flag names are verbatim from
/// `ArcanistDiffWorkflow::getArguments()` (`:86-339`).
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    /// Explicit `--base` revision, if given (`base`, `:297-304`).
    pub base: Option<String>,
    /// Whether `--json` output was requested (`json`, `:244-247`).
    pub json: bool,
    /// End of the commit range (`head`, `:325-335`).
    pub head: Option<String>,
    /// Positional base commit (`arc diff [commit]`, `:38-44`; at most one,
    /// via `parseBaseCommitArgument()`,
    /// `src/workflow/ArcanistWorkflow.php:1825-1846`).
    pub commit: Option<String>,
    /// Update message (`message`, `:88-94`).
    pub message: Option<String>,
    /// Revision message file (`message-file`, `:95-102`).
    pub message_file: Option<String>,
    /// Reviewers for created revisions (`reviewers`, `:275-282`).
    pub reviewers: Option<String>,
    /// CCs for created revisions (`cc`, `:283-290`).
    pub cc: Option<String>,
    /// Revision to update (`update`, `:164-167`).
    pub update: Option<String>,
    /// Always create a new revision (`create`, `:147-163`).
    pub create: bool,
    /// Only create a diff, no revision (`only`, `:190-198`).
    pub only: bool,
    /// Conduit URI override (global `--conduit-uri`,
    /// `src/workflow/ArcanistWorkflow.php:304-310`).
    pub conduit_uri: Option<String>,
}

/// One file's numstat entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path as reported by git (renames report the new path).
    pub path: String,
    /// Added lines (0 for binary files).
    pub added: u64,
    /// Deleted lines (0 for binary files).
    pub deleted: u64,
}

/// Read the space-separated value of `flag` from `iter`.
fn take_value(
    iter: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
    flag: &str,
) -> Result<String, String> {
    let Some(value) = iter.next() else {
        return Err(format!("{flag} requires a value"));
    };
    if value.is_empty() {
        return Err(format!("{flag} requires a non-empty value"));
    }
    Ok(value.clone())
}

/// Read the `--flag=value` form of `flag` from the stripped `value`.
fn take_equals(value: &str, flag: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err(format!("{flag} requires a non-empty value"));
    }
    Ok(value.to_string())
}

/// A value-taking `diff` flag (`--base REV`, `--base=REV`, `-m MSG`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueFlag {
    Base,
    Head,
    Message,
    MessageFile,
    Reviewers,
    Cc,
    Update,
    ConduitUri,
}

/// Canonical `--flag` spelling of a value flag, for error messages.
fn flag_name(flag: ValueFlag) -> &'static str {
    match flag {
        ValueFlag::Base => "--base",
        ValueFlag::Head => "--head",
        ValueFlag::Message => "--message",
        ValueFlag::MessageFile => "--message-file",
        ValueFlag::Reviewers => "--reviewers",
        ValueFlag::Cc => "--cc",
        ValueFlag::Update => "--update",
        ValueFlag::ConduitUri => "--conduit-uri",
    }
}

/// Classify a space-separated value flag (`--base`, `-m`, ...).
fn classify_space_flag(arg: &str) -> Option<ValueFlag> {
    if arg == "--base" {
        Some(ValueFlag::Base)
    } else if arg == "--head" {
        Some(ValueFlag::Head)
    } else if arg == "--message" || arg == "-m" {
        Some(ValueFlag::Message)
    } else if arg == "--message-file" || arg == "-F" {
        Some(ValueFlag::MessageFile)
    } else if arg == "--reviewers" {
        Some(ValueFlag::Reviewers)
    } else if arg == "--cc" {
        Some(ValueFlag::Cc)
    } else if arg == "--update" {
        Some(ValueFlag::Update)
    } else if arg == "--conduit-uri" {
        Some(ValueFlag::ConduitUri)
    } else {
        None
    }
}

/// Classify a `--flag=value` argument, returning the flag and the value.
fn classify_equals_flag(arg: &str) -> Option<(ValueFlag, &str)> {
    if let Some(value) = arg.strip_prefix("--base=") {
        Some((ValueFlag::Base, value))
    } else if let Some(value) = arg.strip_prefix("--head=") {
        Some((ValueFlag::Head, value))
    } else if let Some(value) = arg.strip_prefix("--message=") {
        Some((ValueFlag::Message, value))
    } else if let Some(value) = arg.strip_prefix("--message-file=") {
        Some((ValueFlag::MessageFile, value))
    } else if let Some(value) = arg.strip_prefix("--reviewers=") {
        Some((ValueFlag::Reviewers, value))
    } else if let Some(value) = arg.strip_prefix("--cc=") {
        Some((ValueFlag::Cc, value))
    } else if let Some(value) = arg.strip_prefix("--update=") {
        Some((ValueFlag::Update, value))
    } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
        Some((ValueFlag::ConduitUri, value))
    } else {
        None
    }
}

/// Store a parsed value-flag value in `options`.
fn store_value_flag(options: &mut Options, flag: ValueFlag, value: String) {
    match flag {
        ValueFlag::Base => options.base = Some(value),
        ValueFlag::Head => options.head = Some(value),
        ValueFlag::Message => options.message = Some(value),
        ValueFlag::MessageFile => options.message_file = Some(value),
        ValueFlag::Reviewers => options.reviewers = Some(value),
        ValueFlag::Cc => options.cc = Some(value),
        ValueFlag::Update => options.update = Some(value),
        ValueFlag::ConduitUri => options.conduit_uri = Some(value),
    }
}

/// Apply a boolean `diff` flag (`--json`, `--create`, `--only`).
///
/// Returns true when `arg` was consumed.
fn apply_simple_flag(options: &mut Options, arg: &str) -> bool {
    if arg == "--json" {
        options.json = true;
    } else if arg == "--create" {
        options.create = true;
    } else if arg == "--only" {
        options.only = true;
    } else {
        return false;
    }
    true
}

/// Apply one value-taking `diff` flag in either `--flag VALUE` or
/// `--flag=VALUE` form.
///
/// Returns true when `arg` was consumed.
fn apply_value_flag(
    options: &mut Options,
    iter: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
    arg: &str,
) -> Result<bool, String> {
    if let Some(flag) = classify_space_flag(arg) {
        store_value_flag(options, flag, take_value(iter, arg)?);
        return Ok(true);
    }
    if let Some((flag, value)) = classify_equals_flag(arg) {
        store_value_flag(options, flag, take_equals(value, flag_name(flag))?);
        return Ok(true);
    }
    Ok(false)
}

/// Apply one `diff` flag (boolean or value-taking).
///
/// Returns true when `arg` was consumed.
fn apply_flag(
    options: &mut Options,
    iter: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
    arg: &str,
) -> Result<bool, String> {
    if apply_simple_flag(options, arg) {
        return Ok(true);
    }
    apply_value_flag(options, iter, arg)
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        base: None,
        json: false,
        head: None,
        commit: None,
        message: None,
        message_file: None,
        reviewers: None,
        cc: None,
        update: None,
        create: false,
        only: false,
        conduit_uri: None,
    };
    let mut positionals: Vec<String> = Vec::new();
    let mut end_of_flags = false;
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            positionals.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if apply_flag(&mut options, &mut iter, arg)? {
            // Flag consumed.
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            positionals.push(arg.clone());
        }
    }
    options.commit = diff_local::parse_base_commit_argument(&positionals)?;
    check_conflicts(&options)?;
    Ok(Some(options))
}

/// Reject flag combinations PHP declares conflicting.
///
/// Conflict text follows the `'%s can not be used with %s.'` shape
/// (`ArcanistDiffWorkflow.php:150-161`). `--update` plus `--only` is allowed:
/// `shouldOnlyCreateDiff()` (`:724-738`) lets `--update` win.
fn check_conflicts(options: &Options) -> Result<(), String> {
    if options.create && options.update.is_some() {
        return Err("'--create' can not be used with '--update'.".to_string());
    }
    if options.create && options.only {
        return Err("'--create' can not be used with '--only'.".to_string());
    }
    if options.only {
        // `only` conflicts (`:194-197`); `reviewers`/`cc` conflicts (`:278-289`).
        if options.message.is_some() {
            return Err(
                "'--message' can not be used with '--only': '--only' does not update any revision."
                    .to_string(),
            );
        }
        if options.reviewers.is_some() {
            return Err("'--reviewers' can not be used with '--only'.".to_string());
        }
        if options.cc.is_some() {
            return Err("'--cc' can not be used with '--only'.".to_string());
        }
    }
    if options.update.is_some() {
        // `reviewers`/`cc` conflicts (`:278-289`).
        if options.reviewers.is_some() {
            return Err("'--reviewers' can not be used with '--update'.".to_string());
        }
        if options.cc.is_some() {
            return Err("'--cc' can not be used with '--update'.".to_string());
        }
    }
    // `--base` is a revision here (not PHP's `rules`, `:297-304`), so it and
    // the positional base commit both select the base; refuse the ambiguity.
    if options.base.is_some() && options.commit.is_some() {
        return Err("'--base' can not be used with a base commit.".to_string());
    }
    Ok(())
}

/// Run `git` with `args` in `cwd`, capturing output.
fn git_output(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// True when `cwd` is inside a git working tree.
pub fn is_inside_git_repo(cwd: &Path) -> bool {
    let Ok(out) = git_output(cwd, &["rev-parse", "--is-inside-work-tree"]) else {
        return false;
    };
    out.status.success() && out.stdout.trim_ascii() == b"true"
}

/// Resolve the base revision to a commit SHA.
///
/// Uses the explicit revision when given, else the merge-base of `HEAD`
/// with `@{upstream}` when an upstream exists, else `HEAD`.
pub fn resolve_base(cwd: &Path, explicit: Option<&str>) -> Result<String, String> {
    if let Some(rev) = explicit {
        return rev_to_commit(cwd, rev).ok_or_else(|| format!("invalid base revision '{rev}'"));
    }
    if upstream_exists(cwd)
        && let Ok(out) = git_output(cwd, &["merge-base", "HEAD", "@{upstream}"])
        && out.status.success()
    {
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            return Ok(sha);
        }
    }
    rev_to_commit(cwd, "HEAD")
        .ok_or_else(|| "cannot resolve base: HEAD does not exist (empty repository?)".to_string())
}

/// Resolve `rev` to a commit SHA, or `None` when it does not resolve.
fn rev_to_commit(cwd: &Path, rev: &str) -> Option<String> {
    let arg = format!("{rev}^{{commit}}");
    let out = git_output(cwd, &["rev-parse", "--verify", &arg]).ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// True when the current branch has an upstream configured.
fn upstream_exists(cwd: &Path) -> bool {
    git_output(cwd, &["rev-parse", "--verify", "@{upstream}"]).is_ok_and(|out| out.status.success())
}

/// Parse raw `git diff --numstat -z` output.
pub fn parse_numstat_z(raw: &[u8]) -> Result<Vec<FileDiff>, String> {
    let mut files = Vec::new();
    let mut fields: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    if fields.last() == Some(&b"".as_slice()) {
        fields.pop();
    }
    let mut i = 0;
    while i < fields.len() {
        let record = String::from_utf8_lossy(fields[i]);
        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(format!("malformed numstat record {}", i + 1));
        };
        let added = parse_count(added)?;
        let deleted = parse_count(deleted)?;
        i += 1;
        let path = if path.is_empty() {
            // Rename/copied entry: next two fields are old and new paths.
            let (Some(_old), Some(new)) = (fields.get(i), fields.get(i + 1)) else {
                return Err(format!("truncated rename record {}", files.len() + 1));
            };
            i += 2;
            String::from_utf8_lossy(new).into_owned()
        } else {
            path.to_string()
        };
        files.push(FileDiff {
            path,
            added,
            deleted,
        });
    }
    Ok(files)
}

/// Parse one numstat count (`-` for binary files means 0).
fn parse_count(text: &str) -> Result<u64, String> {
    if text == "-" {
        return Ok(0);
    }
    text.parse::<u64>()
        .map_err(|_| format!("malformed numstat count '{text}'"))
}

/// Collect numstat entries for the worktree diff against `base`.
pub fn numstat(cwd: &Path, base: &str) -> Result<Vec<FileDiff>, String> {
    numstat_range(cwd, base, None)
}

/// Collect numstat entries for `base` against `head` (or the worktree).
pub fn numstat_range(cwd: &Path, base: &str, head: Option<&str>) -> Result<Vec<FileDiff>, String> {
    let mut args = vec!["diff", "--numstat", "-z", base];
    if let Some(rev) = head {
        args.push(rev);
    }
    args.push("--");
    let out = git_output(cwd, &args)?;
    if !out.status.success() {
        return Err(format!(
            "git diff --numstat failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_numstat_z(&out.stdout)
}

/// Total insertions and deletions across `files`.
pub fn summarize(files: &[FileDiff]) -> (u64, u64) {
    files
        .iter()
        .fold((0, 0), |(ins, del), f| (ins + f.added, del + f.deleted))
}

/// Build the `--json` document.
pub fn diff_json(base: &str, files: &[FileDiff]) -> serde_json::Value {
    let (insertions, deletions) = summarize(files);
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "added": f.added,
                "deleted": f.deleted,
            })
        })
        .collect();
    serde_json::json!({
        "base": base,
        "files": files,
        "insertions": insertions,
        "deletions": deletions,
    })
}

/// Render merged commit-message fields as a JSON object.
pub fn fields_json(fields: &diff_local::CommitFields) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in fields {
        let rendered = match value {
            diff_local::FieldValue::Text(text) => serde_json::Value::String(text.clone()),
            diff_local::FieldValue::List(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(|item| serde_json::Value::String(item.clone()))
                    .collect(),
            ),
        };
        map.insert(key.clone(), rendered);
    }
    serde_json::Value::Object(map)
}

/// Stat plus full unified diff against `base`, via the git binary.
pub fn default_diff_output(cwd: &Path, base: &str) -> Result<Vec<u8>, String> {
    default_diff_output_range(cwd, base, None)
}

/// Stat plus full unified diff for `base` against `head` (or the worktree).
pub fn default_diff_output_range(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut stat_args = vec!["--no-pager", "diff", "--no-color", "--stat", base];
    if let Some(rev) = head {
        stat_args.push(rev);
    }
    stat_args.push("--");
    let mut patch_args = vec!["--no-pager", "diff", "--no-color", base];
    if let Some(rev) = head {
        patch_args.push(rev);
    }
    patch_args.push("--");
    let mut combined = Vec::new();
    for args in [stat_args, patch_args] {
        let out = git_output(cwd, &args)?;
        if !out.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        combined.extend_from_slice(&out.stdout);
    }
    Ok(combined)
}

/// Resolved diff range: base commit plus optional head commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// Resolved base commit SHA.
    pub base: String,
    /// Symbolic head revision (`--head`), if given.
    pub head: Option<String>,
}

/// Resolve the base commit and validate the head of the range.
///
/// Explicit `--base` resolves as before; the positional commit resolves to
/// its merge-base with the head (`ArcanistGitAPI::buildBaseCommit()`,
/// `src/repository/api/ArcanistGitAPI.php:208-243`); otherwise the default
/// base applies. `--head` must descend from the base
/// (`ArcanistGitAPI.php:132-157`).
pub fn resolve_range(cwd: &Path, options: &Options) -> Result<Range, String> {
    if options.base.is_some() && options.commit.is_some() {
        return Err("'--base' can not be used with a base commit.".to_string());
    }
    let head_symbolic = options.head.as_deref().unwrap_or("HEAD");
    let base = if let Some(rev) = options.base.as_deref() {
        rev_to_commit(cwd, rev).ok_or_else(|| format!("invalid base revision '{rev}'"))?
    } else if let Some(commit) = options.commit.as_deref() {
        diff_local::resolve_explicit_base(cwd, commit, head_symbolic)?
    } else {
        resolve_base(cwd, None)?
    };
    if let Some(head) = options.head.as_deref() {
        diff_local::require_head_descends_base(cwd, &base, head)?;
    }
    Ok(Range {
        base,
        head: options.head.clone(),
    })
}

/// Merge commit messages (oldest-first) plus the `--reviewers`/`--cc` faux
/// tags into one field dictionary.
///
/// Ports the merge in `parseCommitMessagesIntoFields()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1890-1969`): the faux flags
/// message parses last, like PHP's prepend-then-reverse (`:1839-1904`).
pub fn merge_message_fields(
    oldest_first: &[String],
    reviewers: Option<&str>,
    cc: Option<&str>,
) -> diff_local::CommitFields {
    let mut parsed: Vec<diff_local::CommitFields> = oldest_first
        .iter()
        .map(|text| diff_local::parse_field_tags(text))
        .collect();
    if let Some(faux) = diff_local::faux_flag_message(reviewers, cc) {
        parsed.push(diff_local::parse_field_tags(&faux));
    }
    diff_local::merge_fields_oldest_first(&parsed)
}

/// Collect the merged revision fields for `options`.
///
/// `--message-file` wins alone (`buildCommitMessage()`,
/// `src/workflow/ArcanistDiffWorkflow.php:1331-1338`); otherwise the local
/// commits in the range merge with the `--reviewers`/`--cc` faux message.
pub fn collect_fields(
    cwd: &Path,
    options: &Options,
    base: &str,
    head: Option<&str>,
) -> Result<diff_local::CommitFields, String> {
    if let Some(path) = options.message_file.as_deref() {
        let message = diff_local::commit_message_from_file(Path::new(path))?;
        return Ok(diff_local::parse_field_tags(&message.raw_corpus));
    }
    let infos = diff_local::get_local_commit_information(cwd, base, head.unwrap_or("HEAD"))?;
    let oldest_first: Vec<String> = infos
        .iter()
        .rev()
        .map(|info| info.message.clone())
        .collect();
    Ok(merge_message_fields(
        &oldest_first,
        options.reviewers.as_deref(),
        options.cc.as_deref(),
    ))
}

/// How a submit reaches Differential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitMode {
    /// `--only`: create a diff, no revision
    /// (`src/workflow/ArcanistDiffWorkflow.php:724-738`).
    Only,
    /// `--create`: create a new revision (`:147-163`).
    Create,
    /// `--update`: update a revision (`:164-167`).
    Update(u64),
}

/// Derive the submit mode from `options`, or `None` to stay read-only.
///
/// `--update` wins over `--only`, like `shouldOnlyCreateDiff()`
/// (`src/workflow/ArcanistDiffWorkflow.php:724-738`).
pub fn submit_mode(options: &Options) -> Result<Option<SubmitMode>, String> {
    if options.create {
        return Ok(Some(SubmitMode::Create));
    }
    if let Some(target) = options.update.as_deref() {
        return Ok(Some(SubmitMode::Update(
            diff_local::parse_update_revision_id(target)?,
        )));
    }
    if options.only {
        return Ok(Some(SubmitMode::Only));
    }
    Ok(None)
}

/// Everything a submit sends, collected without touching Conduit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitInput {
    /// Submit mode.
    pub mode: SubmitMode,
    /// Update message (`--message`), update mode only.
    pub message: Option<String>,
    /// Revision fields (create mode) or `{}` (update mode).
    pub fields: serde_json::Value,
    /// Full `differential.creatediff` params.
    pub spec: serde_json::Value,
    /// `differential.setdiffproperty` writes after the diff exists.
    pub properties: Vec<diff_submit::DiffProperty>,
    /// `Included changes:` preview block.
    pub included_changes: String,
    /// Whether `--json` output was requested.
    pub json: bool,
}

/// Convert a local change entry to a submit payload change.
///
/// Divergence: hunks are empty and away paths unpaired. Faithful hunks need
/// the unified-diff parser (`generateChanges()`,
/// `src/workflow/ArcanistDiffWorkflow.php:802-1026`), which is not ported.
fn submit_change(entry: &diff_local::ChangeEntry) -> diff_submit::DiffChange {
    let old_path = match entry.change_type {
        diff_local::TYPE_MOVE_HERE | diff_local::TYPE_COPY_HERE => {
            entry.old_path.clone().unwrap_or_default()
        }
        _ => entry.current.path.clone(),
    };
    let mut change = diff_submit::DiffChange::new_text(&old_path, &entry.current.path);
    change.change_type = i64::from(entry.change_type);
    change.file_type = i64::from(entry.file_type);
    change
}

/// Require a `--message` update message.
///
/// PHP prompts interactively when `--message` is absent
/// (`src/workflow/ArcanistDiffWorkflow.php:701-716`); this port has no
/// interactive editor, so the flag is required.
fn update_message(message: Option<&str>) -> Result<String, String> {
    match message {
        None => Err(
            "Updating a revision requires --message (interactive update prompts are not supported)."
                .to_string(),
        ),
        Some(text) if text.trim().is_empty() => Err("Update message must not be empty.".to_string()),
        Some(text) => Ok(text.to_string()),
    }
}

/// Collect the diff properties to store: local commits plus onto targets.
///
/// Lint/unit properties are skipped: no engines ran, so there are no results
/// (`should_update_lint_property` / `should_update_unit_property`,
/// `src/workflow/ArcanistDiffWorkflow.php:2296-2320`).
fn diff_properties(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffProperty>, String> {
    let mut properties = Vec::new();
    let infos = diff_local::get_local_commit_information(cwd, base, head.unwrap_or("HEAD"))?;
    if diff_local::should_update_local_property(false, !infos.is_empty()) {
        let commits: Vec<serde_json::Value> = infos
            .iter()
            .map(diff_local::LocalCommitInfo::to_json)
            .collect();
        let data = serde_json::to_string(&commits)
            .map_err(|e| format!("failed to encode local commits: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_LOCAL_COMMITS.to_string(),
            data,
        });
    }
    let upstream = diff_local::resolve_upstream_branch(cwd);
    let targets = diff_local::diff_onto_targets(upstream.as_deref(), None);
    if diff_local::should_update_onto_property(!targets.is_empty()) {
        let rendered: Vec<serde_json::Value> = targets
            .iter()
            .map(diff_local::OntoTarget::to_json)
            .collect();
        let data = serde_json::to_string(&rendered)
            .map_err(|e| format!("failed to encode onto targets: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_ONTO.to_string(),
            data,
        });
    }
    Ok(properties)
}

/// Collect everything a submit sends, without touching Conduit.
pub fn prepare_submit(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
) -> Result<SubmitInput, String> {
    let range = resolve_range(cwd, options)?;
    let head = range.head.as_deref();
    let entries = diff_local::collect_change_entries(cwd, &range.base, head)?;
    diff_submit::require_changes(entries.len())?;
    let changes: Vec<diff_submit::DiffChange> = entries.iter().map(submit_change).collect();
    let local = diff_local::load_diff_specification(
        cwd,
        &range.base,
        &diff_local::default_source_machine_name(),
        None,
        false,
    )?;
    let spec_input = diff_submit::DiffSpecInput {
        source_machine: local.source_machine,
        source_path: local.source_path,
        branch: local.branch,
        bookmark: local.bookmark,
        source_control_system: local.source_control_system,
        source_control_path: local.source_control_path,
        source_control_base_revision: local.source_control_base_revision,
        repository_phid: None,
    };
    let spec = diff_submit::diff_spec_json(
        &changes,
        diff_local::lint_status(None),
        diff_local::unit_status(None),
        &spec_input,
    );
    let fields = match mode {
        SubmitMode::Create => fields_json(&collect_fields(cwd, options, &range.base, head)?),
        // Without `--verbatim` PHP sends no local fields on update (`:648-652`).
        SubmitMode::Update(_) => serde_json::json!({}),
        // Diff-only mode builds no commit message (`:1292-1295`).
        SubmitMode::Only => serde_json::Value::Null,
    };
    let message = match mode {
        SubmitMode::Update(_) => Some(update_message(options.message.as_deref())?),
        _ => None,
    };
    Ok(SubmitInput {
        mode: mode.clone(),
        message,
        fields,
        spec,
        properties: diff_properties(cwd, &range.base, head)?,
        included_changes: diff_local::render_included_changes(&entries),
        json: options.json,
    })
}

/// Run a submit over `call`, returning the stdout text.
///
/// Call order mirrors `run()` (`src/workflow/ArcanistDiffWorkflow.php:382-410,
/// 463-465,525-527`): `differential.creatediff`, the
/// `differential.setdiffproperty` writes, then the revision call. Output
/// follows `:414-420` (diff-only), `:476,554-558` (update),
/// `:551-558` (create), plus `Included changes:` (`:565-568`).
///
/// Divergence: console markup (`**bold**`, `__underline__`) renders as plain
/// text, and diff-only `--json` reports `"human": ""` (this port buffers no
/// human output, `:421-429`).
pub fn submit_over(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
) -> Result<String, String> {
    let created = diff_submit::create_diff(call, &input.spec)?;
    diff_submit::set_diff_properties(call, created.diff_id, &input.properties)?;
    match &input.mode {
        SubmitMode::Only => {
            if input.json {
                let doc = serde_json::json!({
                    "diffURI": created.uri,
                    "diffID": created.diff_id,
                    "human": "",
                });
                let mut text = serde_json::to_string(&doc)
                    .map_err(|e| format!("failed to encode JSON: {e}"))?;
                text.push('\n');
                return Ok(text);
            }
            Ok(format!(
                "Created a new Differential diff:\n        Diff URI: {}\n\n{}",
                created.uri, input.included_changes
            ))
        }
        SubmitMode::Update(id) => {
            let revision_id =
                i64::try_from(*id).map_err(|_| format!("Revision ID {id} is out of range."))?;
            let params = diff_submit::update_revision_params(
                revision_id,
                input.fields.clone(),
                created.diff_id,
                input.message.as_deref().unwrap_or(""),
            );
            let updated = diff_submit::update_revision(call, &params)?;
            Ok(format!(
                "Updated an existing Differential revision:\n        Revision URI: {}\n\n{}",
                updated.uri, input.included_changes
            ))
        }
        SubmitMode::Create => {
            // No server transactions are available locally, so the legacy
            // `differential.createrevision` path applies (`:478-531`).
            let params = diff_submit::create_revision_params(input.fields.clone(), created.diff_id);
            let revision = diff_submit::create_revision_legacy(call, &params)?;
            Ok(format!(
                "Created a new Differential revision:\n        Revision URI: {}\n\n{}",
                revision.uri, input.included_changes
            ))
        }
    }
}

/// Collect a submit locally, then run it over a real Conduit client.
fn submit_via_client(cwd: &Path, options: &Options, mode: &SubmitMode) -> Result<String, String> {
    let input = prepare_submit(cwd, options, mode)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    submit_over(&call, &input)
}

/// Inspect `cwd` per `options`, returning the bytes for stdout.
///
/// Without a submit mode this is the read-only inspection: `--json`
/// gains a `fields` object only when field flags are given, so default
/// output is byte-identical to before. With `--create`, `--update`, or
/// `--only`, the diff is submitted over Conduit instead.
pub fn inspect(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    if !is_inside_git_repo(cwd) {
        return Err("not inside a git repository".to_string());
    }
    if let Some(mode) = submit_mode(options)? {
        return submit_via_client(cwd, options, &mode).map(String::into_bytes);
    }
    let range = resolve_range(cwd, options)?;
    if options.json {
        let files = numstat_range(cwd, &range.base, range.head.as_deref())?;
        let mut doc = diff_json(&range.base, &files);
        if options.message_file.is_some() || options.reviewers.is_some() || options.cc.is_some() {
            let fields = collect_fields(cwd, options, &range.base, range.head.as_deref())?;
            doc["fields"] = fields_json(&fields);
        }
        let mut text = serde_json::to_string_pretty(&doc)
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text.into_bytes())
    } else {
        default_diff_output_range(cwd, &range.base, range.head.as_deref())
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc-rs diff: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    match inspect(&cwd, &options) {
        Ok(out) => {
            use std::io::Write;
            if std::io::stdout().write_all(&out).is_err() {
                eprintln!("arc-rs diff: failed to write output");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc-rs diff: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod diff_tests;
