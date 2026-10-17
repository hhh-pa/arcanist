//! `arc close-revision`: explicitly close an accepted revision.
//!
//! Port of `src/workflow/ArcanistCloseRevisionWorkflow.php`: resolves one
//! revision with `differential.query` and closes it with `differential.close`.
//! `--finalize` is the soft variant used by other workflows: a revision which
//! is missing or not accepted is left open with a warning instead of failing.
//!
//! Known limits (see T1128): `--force` is a port affordance for
//! non-interactive closes of revisions owned by someone else (PHP has no
//! `--force` flag; it skips only the not-the-author prompt). `--help` is a
//! port affordance too: PHP has no workflow `--help` flag (help goes
//! through `arc help`), so `--help`/`-h` print the usage text instead of
//! failing as unknown arguments.

use std::path::Path;
use std::process::ExitCode;

use crate::{conduit, config, get_config, git, hg, prompt, svn, terminal};

/// Subcommand name, matching `arc close-revision`.
pub const NAME: &str = "close-revision";

/// Differential "Accepted" status, closable via the API.
pub const STATUS_ACCEPTED: i64 = 2;
/// Differential "Closed" status.
pub const STATUS_CLOSED: i64 = 3;

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not the short error-dump usage. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistCloseRevisionWorkflow.php:12-51`).
const HELP_SYNOPSIS: &str = "      **close-revision** [__options__] __revision__";
const HELP_TEXT: &str = "          Supports: git, hg, svn\n          Close a revision which has been committed (svn) or pushed (git, hg).\n          You should not normally need to do this: arc commit (svn), arc amend\n          (git, hg), arc land (git, hg), or repository tracking on the master\n          remote repository should do it for you. However, if these mechanisms\n          have failed for some reason you can use this command to manually\n          change a revision status from \"Accepted\" to \"Closed\".";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "finalize",
        None,
        None,
        "Close only if the repository is untracked and the revision is accepted. Continue even if the close can't happen. This is a soft version of '' used by other workflows.",
    ),
    ("quiet", None, None, "Do not print a success message."),
];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
        NAME,
        None,
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

/// Parsed `close-revision` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Revision argument as given (`123` or `D123`).
    pub revision: String,
    /// Soft close: leave missing or unaccepted revisions open with a warning.
    pub finalize: bool,
    /// Skip the final message.
    pub quiet: bool,
    /// Emit a JSON result document instead of progress lines.
    pub json: bool,
    /// Close without asking, even when owned by someone else.
    pub force: bool,
    /// Explicit conduit URI override.
    pub conduit_uri: Option<String>,
    /// Working copy's repository PHID, when identified. `--finalize`
    /// skips the close for tracked (identified) repositories, mirroring
    /// `getRepositoryPHID()`. Resolved by [`run`]; tests set it directly.
    pub repository_phid: Option<String>,
}

/// One argument's parse step: consumed, `--help`, or a fatal error.
enum Step {
    Handled,
    Help,
    Error(String),
}

struct Parser {
    finalize: bool,
    quiet: bool,
    json: bool,
    force: bool,
    conduit_uri: Option<String>,
    revision: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            finalize: false,
            quiet: false,
            json: false,
            force: false,
            conduit_uri: None,
            revision: None,
            end_of_flags: false,
        }
    }

    fn positional(&mut self, arg: &str) -> Step {
        if self.revision.is_some() {
            Step::Error("close-revision requires exactly one revision.".to_string())
        } else {
            self.revision = Some(arg.to_string());
            Step::Handled
        }
    }

    fn flag_simple(&mut self, arg: &str) -> Option<Step> {
        if arg == "--finalize" {
            self.finalize = true;
        } else if arg == "--quiet" {
            self.quiet = true;
        } else if arg == "--json" {
            self.json = true;
        } else if arg == "--force" {
            self.force = true;
        } else {
            return None;
        }
        Some(Step::Handled)
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--conduit-uri" {
            let Some(value) = iter.next() else {
                return Some(Step::Error("--conduit-uri requires a value".to_string()));
            };
            self.conduit_uri = Some(value.clone());
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            if value.is_empty() {
                return Some(Step::Error("--conduit-uri requires a value".to_string()));
            }
            self.conduit_uri = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn tail(&mut self, arg: &str) -> Step {
        if arg == "--" {
            self.end_of_flags = true;
            Step::Handled
        } else if arg == "--help" || arg == "-h" {
            Step::Help
        } else if arg.starts_with('-') {
            // `Unknown argument '%s'. Try 'arc help'.` over the key
            // without dashes (`ArcanistWorkflow.php:838-860`).
            let key = arg
                .strip_prefix("--")
                .or_else(|| arg.strip_prefix('-'))
                .unwrap_or(arg)
                .split('=')
                .next()
                .unwrap_or(arg);
            Step::Error(format!("Unknown argument '{key}'. Try 'arc help'."))
        } else {
            self.positional(arg)
        }
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            return self.positional(arg);
        }
        if let Some(step) = self.flag_simple(arg) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        self.tail(arg)
    }

    fn finish(self) -> Result<Option<Options>, String> {
        let Some(revision) = self.revision else {
            return Err("close-revision requires a revision number.".to_string());
        };
        Ok(Some(Options {
            revision,
            finalize: self.finalize,
            quiet: self.quiet,
            json: self.json,
            force: self.force,
            conduit_uri: self.conduit_uri,
            repository_phid: None,
        }))
    }
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut parser = Parser::new();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        match parser.consume(arg, &mut iter) {
            Step::Handled => {}
            Step::Help => return Ok(None),
            Step::Error(message) => return Err(message),
        }
    }
    parser.finish()
}

/// Strip one leading `D`, like `ArcanistWorkflow::normalizeRevisionID`
/// (no format validation upstream).
#[must_use]
pub fn normalize_revision_id(raw: &str) -> String {
    raw.strip_prefix('D')
        .or_else(|| raw.strip_prefix('d'))
        .unwrap_or(raw)
        .to_string()
}

/// Revision fields `close-revision` needs from `differential.query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub id: String,
    pub title: String,
    pub status: i64,
    pub status_name: String,
    pub author_phid: String,
    pub commits: Vec<serde_json::Value>,
}

/// Status names, mirroring `ArcanistDifferentialRevisionStatus`.
#[must_use]
pub fn status_name_for(status: i64) -> &'static str {
    match status {
        0 => "Needs Review",
        1 => "Needs Revision",
        2 => "Accepted",
        3 => "Closed",
        4 => "Abandoned",
        5 => "Changes Planned",
        6 => "In Preparation",
        _ => "Unknown",
    }
}

fn id_string(value: &serde_json::Value) -> Option<String> {
    if let Some(n) = value.as_i64() {
        Some(n.to_string())
    } else if let Some(n) = value.as_u64() {
        Some(n.to_string())
    } else {
        value.as_str().map(str::to_string)
    }
}

/// Parse one `differential.query` row; `None` when it has no id.
#[must_use]
pub fn parse_revision(value: &serde_json::Value) -> Option<Revision> {
    let id = id_string(value.get("id")?)?;
    let status = value.get("status").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    });
    let status = status.unwrap_or(-1);
    let status_name = value
        .get("statusName")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| status_name_for(status).to_string());
    Some(Revision {
        id,
        title: value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status,
        status_name,
        author_phid: value
            .get("authorPHID")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        commits: value
            .get("commits")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    })
}

fn first_row(result: &serde_json::Value) -> Option<&serde_json::Value> {
    match result {
        serde_json::Value::Array(items) => items.first(),
        serde_json::Value::Object(map) => map.values().next(),
        _ => None,
    }
}

/// Revision ID parameter: always the normalized string, as PHP sends
/// `normalizeRevisionID()` verbatim (`'ids' => array($revision_id)`,
/// `'revisionID' => $revision_id`).
fn id_param(id: &str) -> serde_json::Value {
    serde_json::Value::String(id.to_string())
}

/// Look one revision up with `differential.query`.
pub fn query_revision(client: &conduit::Client, id: &str) -> Result<Option<Revision>, String> {
    let result = client
        .call(
            "differential.query",
            &serde_json::json!({"ids": [id_param(id)]}),
        )
        .map_err(|e| e.to_string())?;
    Ok(first_row(&result).and_then(parse_revision))
}

/// Authenticated user's PHID, reusing the handshake identity (T1153
/// LAND-22: PHP's close/amend/commit ownership checks read the cached
/// `userPHID` from `authenticateConduit()`
/// (`src/workflow/ArcanistWorkflow.php:494,572`, via `getUserPHID()`),
/// never a fresh `user.whoami` per close). On a cache miss the identity
/// is resolved once and cached for later callers.
pub fn user_phid(client: &conduit::Client) -> Result<String, String> {
    client.cached_user_phid()
}

/// Output of [`execute`]: warnings and progress go to stderr in `--json`
/// mode so stdout stays a pure JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub stdout: String,
    pub stderr: String,
}

/// Build the `--json` result document.
#[must_use]
pub fn result_json(
    object_name: &str,
    revision: Option<&Revision>,
    closed: bool,
) -> serde_json::Value {
    serde_json::json!({
        "revision": object_name,
        "closed": closed,
        "status": revision.map(|r| r.status),
        "statusName": revision.map(|r| r.status_name.clone()),
    })
}

/// Reject a missing revision, or one that is not accepted outside
/// `--finalize` mode.
fn check_closable(
    revision: &Option<Revision>,
    options: &Options,
    object_name: &str,
) -> Result<(), String> {
    if revision.is_none() && !options.finalize {
        return Err(format!("Revision {object_name} does not exist."));
    }
    if let Some(rev) = revision
        && !options.finalize
        && rev.status != STATUS_ACCEPTED
    {
        return Err(format!(
            "Revision {object_name} can not be closed. You can only close revisions which have been 'accepted'."
        ));
    }
    Ok(())
}

/// Ask `confirm` before closing a revision owned by someone else (skipped
/// with `--finalize`, which never prompts, or `--force`).
fn confirm_ownership(
    client: &conduit::Client,
    options: &Options,
    rev: &Revision,
    object_name: &str,
    confirm: &dyn Fn(&str) -> Result<bool, String>,
) -> Result<(), String> {
    if !options.finalize && rev.author_phid != user_phid(client)? && !options.force {
        let prompt = format!(
            "You are not the author of revision \"{object_name}\", are you sure you want to close it?"
        );
        if !confirm(&prompt)? {
            // `ArcanistUserAbortException`: 'User aborted the workflow.'
            return Err("User aborted the workflow.".to_string());
        }
    }
    Ok(())
}

/// Output of one close attempt: progress and warning text plus whether the
/// `differential.close` call ran.
struct CloseStep {
    progress: String,
    warnings: String,
    closed: bool,
}

/// Close one revision unless `--finalize` mode leaves it open.
fn close_if_accepted(
    client: &conduit::Client,
    options: &Options,
    rev: &Revision,
    id: &str,
    object_name: &str,
    ansi: bool,
) -> Result<CloseStep, String> {
    // `--finalize` closes only untracked repositories: a tracked working
    // copy is left alone silently, mirroring `getRepositoryPHID()`.
    if options.finalize && options.repository_phid.is_some() {
        return Ok(CloseStep {
            progress: String::new(),
            warnings: String::new(),
            closed: false,
        });
    }
    // See T13458: the server rejects a close transition unless the
    // revision is accepted, so finalize mode skips the attempt.
    if options.finalize && rev.status != STATUS_ACCEPTED {
        let warnings = terminal::format_status_message(
            "yellow",
            "OPEN REVISION",
            &format!(
                "Revision \"{object_name}\" is not in state \"Accepted\", so it will be left open."
            ),
            ansi,
        );
        return Ok(CloseStep {
            progress: String::new(),
            warnings,
            closed: false,
        });
    }
    let display = format!("{object_name} {}", rev.title);
    let mut progress = terminal::format_status_message(
        "blue",
        "CLOSE",
        &format!("Closing revision \"{display}\"..."),
        ansi,
    );
    client
        .call(
            "differential.close",
            &serde_json::json!({"revisionID": id_param(id)}),
        )
        .map_err(|e| e.to_string())?;
    progress += &terminal::format_status_message("green", "CLOSE", "Done, closed revision.", ansi);
    Ok(CloseStep {
        progress,
        warnings: String::new(),
        closed: true,
    })
}

/// The per-VCS finalized message, wrapped like
/// `echo phutil_console_wrap($message)."\n"`.
///
/// Outside a repository PHP throws
/// `ArcanistCapabilityNotSupportedException` from
/// `ArcanistFilesystemAPI::getFinalizedRevisionMessage()`.
fn finalized_message(vcs: Option<&WorkingCopy>) -> Result<String, String> {
    let message = match vcs {
        Some(WorkingCopy::Hg) => {
            "You may now push this commit upstream, as appropriate (e.g. with \
             'hg push' or by printing and faxing it)."
                .to_string()
        }
        Some(WorkingCopy::Svn) => "Done.".to_string(),
        Some(WorkingCopy::Git) => "You may now push this commit upstream, as appropriate \
             (e.g. with 'git push', or 'git svn dcommit', or by printing and faxing it)."
            .to_string(),
        None => {
            return Err(
                "This repository API ('filesystem') does not support the requested capability."
                    .to_string(),
            );
        }
    };
    Ok(format!("{}\n", terminal::console_wrap(&message, 0, true)))
}

/// The final message: empty with `--quiet`, else the push hint for a
/// finalized revision or a plain `Done.`.
fn closing_message(revision: &Option<Revision>, quiet: bool, dir: &Path) -> Result<String, String> {
    if quiet {
        return Ok(String::new());
    }
    let finalized = matches!(&revision, Some(rev)
        if (rev.status == STATUS_ACCEPTED || rev.status == STATUS_CLOSED)
            && rev.commits.is_empty());
    if finalized {
        finalized_message(detect_working_copy(dir).as_ref())
    } else {
        Ok("Done.\n".to_string())
    }
}

fn render_report(
    options: &Options,
    object_name: &str,
    revision: Option<&Revision>,
    progress: &str,
    closing: &str,
    warnings: &str,
    closed: bool,
) -> Result<Report, String> {
    if options.json {
        let mut doc = serde_json::to_string_pretty(&result_json(object_name, revision, closed))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        doc.push('\n');
        Ok(Report {
            stdout: doc,
            stderr: format!("{progress}{warnings}{closing}"),
        })
    } else {
        Ok(Report {
            stdout: closing.to_string(),
            stderr: format!("{progress}{warnings}"),
        })
    }
}

/// Close `options.revision`, asking `confirm` before closing a revision owned
/// by someone else (skipped with `--finalize`, which never prompts, or
/// `--force`).
pub fn execute(
    client: &conduit::Client,
    options: &Options,
    confirm: &dyn Fn(&str) -> Result<bool, String>,
) -> Result<Report, String> {
    let dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    execute_in(&dir, client, options, confirm)
}

/// [`execute`] with `dir` pinning working-copy detection.
pub fn execute_in(
    dir: &Path,
    client: &conduit::Client,
    options: &Options,
    confirm: &dyn Fn(&str) -> Result<bool, String>,
) -> Result<Report, String> {
    let id = normalize_revision_id(&options.revision);
    let object_name = format!("D{id}");
    let revision = query_revision(client, &id)?;
    check_closable(&revision, options, &object_name)?;

    let ansi = terminal::ansi_enabled();
    let mut progress = String::new();
    let mut warnings = String::new();
    let mut closed = false;
    if let Some(rev) = &revision {
        confirm_ownership(client, options, rev, &object_name, confirm)?;
        let step = close_if_accepted(client, options, rev, &id, &object_name, ansi)?;
        progress += &step.progress;
        warnings += &step.warnings;
        closed = step.closed;
    }

    let closing = closing_message(&revision, options.quiet, dir)?;
    render_report(
        options,
        &object_name,
        revision.as_ref(),
        &progress,
        &closing,
        &warnings,
        closed,
    )
}

/// `repository.callsign` from any config source, when set and non-empty.
fn configured_callsign(sources: &get_config::Sources) -> Option<String> {
    let (_, value) = get_config::current_value("repository.callsign", sources)?;
    let callsign = value.as_str()?.trim().to_string();
    (!callsign.is_empty()).then_some(callsign)
}

/// The `repository.query` constraint, mirroring `getRepositoryQuery()`:
/// configured callsign first, then the VCS UUID, then the remote URI.
fn select_repository_query(
    sources: &get_config::Sources,
    uuid: Option<&str>,
    remote: Option<&str>,
) -> Option<serde_json::Value> {
    if let Some(callsign) = configured_callsign(sources) {
        return Some(serde_json::json!({"callsigns": [callsign]}));
    }
    if let Some(uuid) = uuid {
        return Some(serde_json::json!({"uuids": [uuid]}));
    }
    remote.map(|remote| serde_json::json!({"remoteURIs": [remote]}))
}

enum WorkingCopy {
    Git,
    Hg,
    Svn,
}

fn detect_working_copy(dir: &Path) -> Option<WorkingCopy> {
    if git::is_inside_work_tree(dir) {
        return Some(WorkingCopy::Git);
    }
    if crate::list::find_hg_root(dir).is_some() {
        return Some(WorkingCopy::Hg);
    }
    if svn::find_metadata_path(dir).is_some() {
        return Some(WorkingCopy::Svn);
    }
    None
}

/// The `repository.query` constraint for `dir`, mirroring
/// `getRepositoryQuery()`: configured callsign first, then the VCS UUID
/// (Subversion only), then the remote URI.
fn repository_query(dir: &Path) -> Result<Option<serde_json::Value>, String> {
    let user_file = config::arcrc_path();
    let sources = get_config::load_sources(dir, user_file.as_deref())?;
    if configured_callsign(&sources).is_some() {
        return Ok(select_repository_query(&sources, None, None));
    }
    let Some(vcs) = detect_working_copy(dir) else {
        return Ok(None);
    };
    let (uuid, remote) = match vcs {
        WorkingCopy::Git => (None, git::remote_uri(dir)?),
        WorkingCopy::Hg => (hg::get_repository_uuid(), hg::get_remote_uri(dir)?),
        WorkingCopy::Svn => (svn::get_repository_uuid(dir)?, svn::get_remote_uri(dir)?),
    };
    Ok(select_repository_query(
        &sources,
        uuid.as_deref(),
        remote.as_deref(),
    ))
}

/// PHID of the Phabricator repository for `dir`, or `None` when no
/// repository can be identified, mirroring `getRepositoryPHID()`: only a
/// unique `repository.query` match counts, and an outdated server
/// (`ERR-CONDUIT-CALL`) means untracked.
pub fn resolve_repository_phid(
    client: &conduit::Client,
    dir: &Path,
) -> Result<Option<String>, String> {
    let Some(query) = repository_query(dir)? else {
        return Ok(None);
    };
    let results = match client.call("repository.query", &query) {
        Ok(results) => results,
        Err(conduit::Error::Api { code, .. }) if code == "ERR-CONDUIT-CALL" => {
            return Ok(None);
        }
        Err(e) => return Err(e.to_string()),
    };
    Ok(select_unique_phid(&results))
}

/// The identified repository PHID: only a unique match counts, mirroring
/// `loadRepositoryInformation()`.
fn select_unique_phid(results: &serde_json::Value) -> Option<String> {
    let matches = results.as_array()?;
    if matches.len() != 1 {
        return None;
    }
    matches[0]
        .get("phid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Ownership confirm: PHP `phutil_console_confirm($prompt)` defaults to
/// no (`[y/N]`), so Enter aborts the close
/// (`ArcanistCloseRevisionWorkflow.php:121`).
fn confirm_interactive(query: &str) -> Result<bool, String> {
    prompt::console_confirm(&mut std::io::stdin().lock(), query, true)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let mut options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc {NAME}: {msg}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
    };
    let client = match config::client_for(options.conduit_uri.as_deref()) {
        Ok(client) => client,
        Err(msg) => {
            eprintln!("arc {NAME}: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    if options.finalize {
        match resolve_repository_phid(&client, &cwd) {
            Ok(phid) => options.repository_phid = phid,
            Err(msg) => {
                eprintln!("arc {NAME}: {msg}");
                return ExitCode::FAILURE;
            }
        }
    }
    match execute_in(&cwd, &client, &options, &confirm_interactive) {
        Ok(report) => {
            print!("{}", report.stdout);
            eprint!("{}", report.stderr);
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc {NAME}: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "close_revision_tests.rs"]
mod close_revision_tests;
