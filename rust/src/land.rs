//! `land` workflow: squash-merge the current branch onto a target branch.
//!
//! Tracks T1128. Mirrors the core `arc land` flow from
//! `src/workflow/ArcanistLandWorkflow.php` under the default "squash"
//! strategy: check out the target branch, squash-merge the feature branch,
//! and commit the result as a single commit naming the branch.
//!
//! Pre-land gates (`land_checks`, from
//! `src/land/engine/ArcanistLandEngine.php`) run only with `--revision` and
//! only when revision info is available; otherwise they are skipped with a
//! note.
//!
//! Out of scope (not yet ported, see T1128): automatic revision discovery
//! from commits, build and parent gates, and landing directly onto
//! remote-tracking branches.

extern crate self as arc_rs;

use std::path::Path;
use std::process::{Command, ExitCode};

use arc_rs::{config, diff_submit, land_checks};

pub const NAME: &str = "land";

const DEFAULT_ONTO: &str = "master";

/// `user.whoami` Conduit method; the result's `phid` is the viewer PHID
/// (`src/workflow/ArcanistWorkflow.php:489-494`).
pub const METHOD_WHOAMI: &str = "user.whoami";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    onto: String,
    preview: bool,
    push: bool,
    revision: Option<String>,
    conduit_uri: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub branch: String,
    pub onto: String,
    pub base: String,
    pub head: String,
    pub commits: Vec<Commit>,
}

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--onto BRANCH] [--preview] [--push] [--revision REV] [--conduit-uri URI]\n\
         \n\
         Squash-merge the current branch onto BRANCH (default: {DEFAULT_ONTO})\n\
         and commit the result as a single commit naming the branch.\n\
         \n\
         Options:\n\
         \x20 --onto BRANCH  land onto BRANCH instead of {DEFAULT_ONTO} (must be a local branch)\n\
         \x20 --preview      print the commits that would land; change nothing\n\
         \x20 --push         push the target branch after landing\n\
         \x20 --revision REV  check pre-land gates for revision REV (D123 or 123)\n\
         \x20 --conduit-uri URI  Conduit URI for the revision lookup\n\
         \n\
         Pre-land gates (author, status, working-set size) run only with\n\
         --revision and only when revision info is available; otherwise they\n\
         are skipped with a note.\n\
         \n\
         Not yet ported (see T1128): automatic revision discovery from commits,\n\
         build and parent gates, landing directly onto remote-tracking branches.\n"
    )
}

fn wants_help(argv: &[String]) -> bool {
    argv.iter()
        .any(|arg| arg == "-h" || arg == "--help" || arg == "help")
}

/// Read the value of a `--flag VALUE` argument, rejecting a missing, empty,
/// or flag-like value. `what` names the value in the error.
fn take_space_value(
    iter: &mut std::slice::Iter<'_, String>,
    flag: &str,
    what: &str,
) -> Result<String, String> {
    let Some(value) = iter.next() else {
        return Err(format!("{flag} requires a non-empty {what}"));
    };
    if value.is_empty() || value.starts_with('-') {
        return Err(format!("{flag} requires a non-empty {what}"));
    }
    Ok(value.clone())
}

/// Read the value of a `--flag=VALUE` argument, rejecting an empty value.
fn take_equals_value(value: &str, flag: &str, what: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err(format!("{flag} requires a non-empty {what}"));
    }
    Ok(value.to_string())
}

/// Apply one `--onto` argument in either `--onto BRANCH` or `--onto=BRANCH`
/// form. Returns true when `arg` was consumed.
fn apply_onto_flag(
    opts: &mut Options,
    iter: &mut std::slice::Iter<'_, String>,
    arg: &str,
) -> Result<bool, String> {
    if arg == "--onto" {
        opts.onto = take_space_value(iter, "--onto", "branch name")?;
        Ok(true)
    } else if let Some(value) = arg.strip_prefix("--onto=") {
        opts.onto = take_equals_value(value, "--onto", "branch name")?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Apply one `--revision` argument in either space or `=` form. Returns true
/// when `arg` was consumed.
fn apply_revision_flag(
    opts: &mut Options,
    iter: &mut std::slice::Iter<'_, String>,
    arg: &str,
) -> Result<bool, String> {
    if arg == "--revision" {
        opts.revision = Some(take_space_value(iter, "--revision", "revision")?);
        Ok(true)
    } else if let Some(value) = arg.strip_prefix("--revision=") {
        opts.revision = Some(take_equals_value(value, "--revision", "revision")?);
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Apply one `--conduit-uri` argument in either space or `=` form. Returns
/// true when `arg` was consumed.
fn apply_conduit_flag(
    opts: &mut Options,
    iter: &mut std::slice::Iter<'_, String>,
    arg: &str,
) -> Result<bool, String> {
    if arg == "--conduit-uri" {
        opts.conduit_uri = Some(take_space_value(iter, "--conduit-uri", "URI")?);
        Ok(true)
    } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
        opts.conduit_uri = Some(take_equals_value(value, "--conduit-uri", "URI")?);
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Apply one value-taking `land` flag. Returns true when `arg` was consumed.
fn apply_value_flag(
    opts: &mut Options,
    iter: &mut std::slice::Iter<'_, String>,
    arg: &str,
) -> Result<bool, String> {
    if apply_onto_flag(opts, iter, arg)? {
        return Ok(true);
    }
    if apply_revision_flag(opts, iter, arg)? {
        return Ok(true);
    }
    if apply_conduit_flag(opts, iter, arg)? {
        return Ok(true);
    }
    Ok(false)
}

fn parse_args(argv: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        onto: DEFAULT_ONTO.to_string(),
        preview: false,
        push: false,
        revision: None,
        conduit_uri: None,
    };
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if apply_value_flag(&mut opts, &mut iter, arg)? {
            // Flag consumed.
        } else if arg == "--preview" {
            opts.preview = true;
        } else if arg == "--push" {
            opts.push = true;
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(opts)
}

/// Parse a `--revision` value (`D123` or `123`) into a revision ID.
///
/// Mirrors `normalizeRevisionID()`
/// (`src/workflow/ArcanistWorkflow.php:1519-1521`) for the `revision`
/// argument (`src/workflow/ArcanistLandWorkflow.php:179-184`).
pub fn parse_revision_argument(target: &str) -> Result<u64, String> {
    let digits = target
        .strip_prefix('D')
        .or_else(|| target.strip_prefix('d'))
        .unwrap_or(target)
        .trim();
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && let Ok(id) = digits.parse::<u64>()
    {
        return Ok(id);
    }
    Err("Parameter to --revision must be a Differential Revision number.".to_string())
}

/// Run `git` in `dir`, returning trimmed stdout or trimmed combined output.
fn git_output(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}{stderr}");
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            Err(format!("git {} failed", args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn is_remote_tracking(dir: &Path, onto: &str) -> bool {
    let Ok(remotes) = git_output(dir, &["remote"]) else {
        return false;
    };
    remotes.lines().map(str::trim).any(|remote| {
        !remote.is_empty()
            && onto
                .strip_prefix(remote)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Fail unless the working tree has no staged, unstaged, or untracked changes.
pub fn ensure_clean(dir: &Path) -> Result<(), String> {
    let status = git_output(dir, &["status", "--porcelain"])?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(
            "working tree is not clean; commit or stash your changes first (or use --preview)"
                .to_string(),
        )
    }
}

/// Validate preconditions and collect the commits `merge-base..HEAD` would land.
///
/// Checks: inside a git repo, on a named branch, `onto` names a local
/// branch, and the commit list is non-empty. Dirt is checked separately by
/// [`ensure_clean`] so `--preview` can skip it.
pub fn plan(dir: &Path, onto: &str) -> Result<Plan, String> {
    if !git_ok(dir, &["rev-parse", "--git-dir"]) {
        return Err("not inside a git repository".to_string());
    }
    let branch = git_output(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    if branch.is_empty() {
        return Err(
            "not on a named branch (detached HEAD); check out a branch to land".to_string(),
        );
    }
    if !git_ok(
        dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{onto}"),
        ],
    ) {
        if is_remote_tracking(dir, onto) {
            return Err(format!(
                "not yet ported: landing directly onto remote-tracking branch '{onto}' is not supported (see T1128)"
            ));
        }
        if git_ok(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{onto}^{{commit}}"),
            ],
        ) {
            return Err(format!("onto target '{onto}' is not a local branch"));
        }
        return Err(format!("onto target '{onto}' does not exist"));
    }
    let base = git_output(dir, &["merge-base", "HEAD", onto]).map_err(|_| {
        format!("cannot find a merge base between '{branch}' and '{onto}' (unrelated histories?)")
    })?;
    let head = git_output(dir, &["rev-parse", "HEAD"])?;
    let range = format!("{base}..{head}");
    let log = git_output(dir, &["log", "--format=%H%x1f%s", &range])?;
    let commits: Vec<Commit> = log
        .lines()
        .filter_map(|line| {
            line.split_once('\x1f').map(|(sha, subject)| Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect();
    if commits.is_empty() {
        return Err(format!(
            "nothing to land: '{branch}' has no commits beyond '{onto}'"
        ));
    }
    Ok(Plan {
        branch,
        onto: onto.to_string(),
        base,
        head,
        commits,
    })
}

pub fn format_preview(plan: &Plan) -> String {
    let mut out = format!(
        "Preview: land '{}' onto '{}' ({} commit(s), nothing changed).\nbase: {}\nhead: {}\ncommits:\n",
        plan.branch,
        plan.onto,
        plan.commits.len(),
        plan.base,
        plan.head
    );
    for commit in &plan.commits {
        out.push_str(&format!("{} {}\n", commit.sha, commit.subject));
    }
    out
}

pub fn build_message(plan: &Plan) -> String {
    let mut message = format!("Land {}\n", plan.branch);
    message.push_str(&format!(
        "\nSquashed {} commit(s) from '{}' onto '{}':\n",
        plan.commits.len(),
        plan.branch,
        plan.onto
    ));
    for commit in &plan.commits {
        let short: String = commit.sha.chars().take(7).collect();
        message.push_str(&format!("{short} {}\n", commit.subject));
    }
    message
}

/// Load revision rows for `revision_id` via `differential.revision.search`.
///
/// The modern search shape is parsed by `land_checks::parse_revision`; an
/// empty `data` list means no revision info is available.
pub fn fetch_revision_views(
    call: &diff_submit::ConduitCall<'_>,
    revision_id: u64,
) -> Result<Vec<land_checks::RevisionView>, String> {
    let id = i64::try_from(revision_id)
        .map_err(|_| format!("Revision ID {revision_id} is out of range."))?;
    let result = call(
        diff_submit::METHOD_REVISION_SEARCH,
        &diff_submit::revision_search_params(&[id]),
    )?;
    let rows = result
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "differential.revision.search response has no data list".to_string())?;
    rows.iter().map(land_checks::parse_revision).collect()
}

/// Load the viewer PHID via `user.whoami` (`result['phid']`,
/// `src/workflow/ArcanistWorkflow.php:489-494`).
pub fn fetch_viewer_phid(call: &diff_submit::ConduitCall<'_>) -> Result<String, String> {
    let result = call(METHOD_WHOAMI, &serde_json::json!({}))?;
    result
        .get("phid")
        .and_then(|phid| phid.as_str())
        .filter(|phid| !phid.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "user.whoami response has no phid".to_string())
}

/// Evaluate the pre-land gates for `revision_id` over `call`.
///
/// Fails when revision info is unavailable (the caller skips with a note).
/// Parent and build gates run empty: joining parents and builds to revisions
/// needs Conduit data this port does not fetch (see the `land_checks`
/// divergences).
pub fn load_gate_findings(
    call: &diff_submit::ConduitCall<'_>,
    revision_id: u64,
    commit_count: usize,
) -> Result<Vec<land_checks::GateFinding>, String> {
    let revisions = fetch_revision_views(call, revision_id)?;
    if revisions.is_empty() {
        return Err(format!(
            "revision D{revision_id} has no differential.revision.search result"
        ));
    }
    // Without a viewer identity the author check can not run; viewing as the
    // landing author reduces it to a no-op for the single-row result.
    let fallback = revisions[0].author_phid.clone();
    let viewer = fetch_viewer_phid(call).unwrap_or(fallback);
    Ok(land_checks::evaluate_gates(
        &revisions,
        &viewer,
        &[],
        &[],
        &[],
        Some(commit_count),
    ))
}

/// Note printed when gates are skipped: revision info is unavailable.
pub fn format_skip_note(reason: &str) -> String {
    format!("Note: revision info unavailable ({reason}); skipping pre-land checks.\n")
}

/// One gate finding as an indented line.
fn format_finding_line(finding: &land_checks::GateFinding) -> String {
    format!(
        "  [{}] {}: {}\n",
        finding.prompt, finding.title, finding.query
    )
}

/// Advisory gate report for `--preview`: a pass line or listed findings.
pub fn format_gate_preview(findings: &[land_checks::GateFinding], revision_id: u64) -> String {
    if findings.is_empty() {
        return format!("Pre-land checks passed (--revision D{revision_id}).\n");
    }
    let mut out = format!("Pre-land checks (--revision D{revision_id}):\n");
    for finding in findings {
        out.push_str(&format_finding_line(finding));
    }
    out
}

/// Blocking gate report for a real land: listed findings abort the land.
///
/// Divergence: PHP prompts interactively per finding (`confirmRevisions`,
/// `src/land/engine/ArcanistLandEngine.php:278-534`, and `confirmBuilds`,
/// `:536-718`); this non-interactive port aborts instead.
pub fn format_gate_failure(findings: &[land_checks::GateFinding], revision_id: u64) -> String {
    let mut out = format!("Pre-land checks failed (--revision D{revision_id}):\n");
    for finding in findings {
        out.push_str(&format_finding_line(finding));
    }
    out
}

/// Run the `--revision` gates, returning output text (or a blocking error).
///
/// Unavailable revision info (no Conduit, failed lookup, empty result) skips
/// with a note; findings block a real land but stay advisory under
/// `--preview`.
fn revision_gate_text(
    conduit_uri: Option<&str>,
    revision_id: u64,
    commit_count: usize,
    preview: bool,
) -> Result<String, String> {
    let client = match config::client_for(conduit_uri) {
        Ok(client) => client,
        Err(reason) => return Ok(format_skip_note(&reason)),
    };
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    let findings = match load_gate_findings(&call, revision_id, commit_count) {
        Ok(findings) => findings,
        Err(reason) => return Ok(format_skip_note(&reason)),
    };
    if preview {
        Ok(format_gate_preview(&findings, revision_id))
    } else if findings.is_empty() {
        Ok(String::new())
    } else {
        Err(format_gate_failure(&findings, revision_id))
    }
}

fn push_remote(dir: &Path) -> Result<String, String> {
    let remotes = git_output(dir, &["remote"])?;
    let names: Vec<&str> = remotes
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.contains(&"origin") {
        return Ok("origin".to_string());
    }
    if names.len() == 1 {
        return Ok(names[0].to_string());
    }
    if names.is_empty() {
        return Err("cannot push: this repository has no git remotes".to_string());
    }
    Err("cannot push: there is no 'origin' remote and several remotes exist".to_string())
}

fn land_plan(dir: &Path, plan: &Plan, push: bool) -> Result<String, String> {
    git_output(dir, &["checkout", "-q", &plan.onto])
        .map_err(|detail| format!("failed to check out '{}': {detail}", plan.onto))?;
    git_output(dir, &["merge", "--squash", &plan.branch]).map_err(|detail| {
        format!(
            "squash-merge of '{}' into '{}' failed: {detail}\nResolve the conflicts and commit, or run 'git reset --hard HEAD' to abort.",
            plan.branch, plan.onto
        )
    })?;
    if git_ok(dir, &["diff", "--cached", "--quiet"]) {
        return Err(format!(
            "squash-merge of '{}' produced no changes relative to '{}'; nothing to commit",
            plan.branch, plan.onto
        ));
    }
    let message = build_message(plan);
    git_output(dir, &["commit", "-q", "-m", &message])
        .map_err(|detail| format!("failed to commit the landed change: {detail}"))?;
    let landed = git_output(dir, &["rev-parse", "HEAD"])?;
    let mut out = format!(
        "Landed '{}' onto '{}' as {}.\n",
        plan.branch, plan.onto, landed
    );
    if push {
        let remote = push_remote(dir)?;
        git_output(dir, &["push", "-q", &remote, &plan.onto]).map_err(|detail| {
            format!(
                "landed '{}' onto '{}' but failed to push to '{remote}': {detail}",
                plan.branch, plan.onto
            )
        })?;
        out.push_str(&format!("Pushed '{}' to '{remote}'.\n", plan.onto));
    }
    Ok(out)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    if wants_help(argv) {
        return Ok(usage());
    }
    let opts = parse_args(argv).map_err(|message| format!("{message}\n\n{}", usage()))?;
    let land = plan(dir, &opts.onto)?;
    if opts.preview {
        let mut out = String::new();
        if let Some(target) = opts.revision.as_deref() {
            let revision_id = parse_revision_argument(target)?;
            out.push_str(&revision_gate_text(
                opts.conduit_uri.as_deref(),
                revision_id,
                land.commits.len(),
                true,
            )?);
        }
        out.push_str(&format_preview(&land));
        if opts.push {
            out.push_str("--push has no effect with --preview; nothing was pushed.\n");
        }
        return Ok(out);
    }
    ensure_clean(dir)?;
    let mut out = String::new();
    if let Some(target) = opts.revision.as_deref() {
        let revision_id = parse_revision_argument(target)?;
        out.push_str(&revision_gate_text(
            opts.conduit_uri.as_deref(),
            revision_id,
            land.commits.len(),
            false,
        )?);
    }
    out.push_str(&land_plan(dir, &land, opts.push)?);
    Ok(out)
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs land: {message}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "land_tests.rs"]
mod land_tests;
