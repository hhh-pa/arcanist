//! `arc-rs close-revision`: explicitly close an accepted revision.
//!
//! Port of `src/workflow/ArcanistCloseRevisionWorkflow.php`: resolves one
//! revision with `differential.query` and closes it with `differential.close`.
//! `--finalize` is the soft variant used by other workflows: a revision which
//! is missing or not accepted is left open with a warning instead of failing.
//!
//! Known limits (see T1128): the tracked-repository check in `--finalize`
//! mode is skipped (arc-rs cannot resolve the repository PHID yet), so
//! finalize always attempts the close for accepted revisions; only the git
//! finalized message is reproduced; `--force` is a port affordance for
//! non-interactive closes of revisions owned by someone else.

use std::process::ExitCode;

use crate::{conduit, config};

/// Subcommand name, matching `arc close-revision`.
pub const NAME: &str = "close-revision";

/// Differential "Accepted" status, closable via the API.
pub const STATUS_ACCEPTED: i64 = 2;
/// Differential "Closed" status.
pub const STATUS_CLOSED: i64 = 3;

const USAGE: &str = "usage: arc-rs close-revision [--finalize] [--quiet] [--json] [--force] [--conduit-uri URI] REVISION\n";

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
            Step::Error(format!("unknown flag '{arg}'"))
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

/// Strip a leading `D` (like `ArcanistWorkflow::normalizeRevisionID`) and
/// require a numeric id.
pub fn normalize_revision_id(raw: &str) -> Result<String, String> {
    let stripped = raw
        .strip_prefix('D')
        .or_else(|| raw.strip_prefix('d'))
        .unwrap_or(raw);
    if stripped.is_empty() || !stripped.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid revision '{raw}'"));
    }
    Ok(stripped.to_string())
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

fn id_param(id: &str) -> serde_json::Value {
    id.parse::<u64>().map_or_else(
        |_| serde_json::Value::String(id.to_string()),
        serde_json::Value::from,
    )
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

/// Authenticated user's PHID via `user.whoami`.
pub fn user_phid(client: &conduit::Client) -> Result<String, String> {
    let who = client
        .call("user.whoami", &serde_json::json!({}))
        .map_err(|e| e.to_string())?;
    who.get("phid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "conduit user.whoami response has no phid".to_string())
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
    confirm: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    if !options.finalize && rev.author_phid != user_phid(client)? && !options.force {
        let prompt = format!(
            "You are not the author of revision \"{object_name}\", are you sure you want to close it?"
        );
        if !confirm(&prompt) {
            return Err("Aborted.".to_string());
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
) -> Result<CloseStep, String> {
    // See T13458: the server rejects a close transition unless the
    // revision is accepted, so finalize mode skips the attempt.
    if options.finalize && rev.status != STATUS_ACCEPTED {
        return Ok(CloseStep {
            progress: String::new(),
            warnings: format!(
                "OPEN REVISION: Revision \"{object_name}\" is not in state \"Accepted\", so it will be left open.\n"
            ),
            closed: false,
        });
    }
    let display = format!("{object_name} {}", rev.title);
    let mut progress = format!("Closing revision \"{display}\"...\n");
    client
        .call(
            "differential.close",
            &serde_json::json!({"revisionID": id_param(id)}),
        )
        .map_err(|e| e.to_string())?;
    progress += "Done, closed revision.\n";
    Ok(CloseStep {
        progress,
        warnings: String::new(),
        closed: true,
    })
}

/// The final message: empty with `--quiet`, else the push hint for a
/// finalized revision or a plain `Done.`.
fn closing_message(revision: &Option<Revision>, quiet: bool) -> String {
    if quiet {
        return String::new();
    }
    let finalized = matches!(&revision, Some(rev)
        if (rev.status == STATUS_ACCEPTED || rev.status == STATUS_CLOSED)
            && rev.commits.is_empty());
    if finalized {
        "You may now push this commit upstream, as appropriate (e.g. with 'git push', or 'git svn dcommit', or by printing and faxing it).\n"
            .to_string()
    } else {
        "Done.\n".to_string()
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
            stderr: format!("{progress}{closing}{warnings}"),
        })
    } else {
        Ok(Report {
            stdout: format!("{progress}{closing}"),
            stderr: warnings.to_string(),
        })
    }
}

/// Close `options.revision`, asking `confirm` before closing a revision owned
/// by someone else (skipped with `--finalize`, which never prompts, or
/// `--force`).
pub fn execute(
    client: &conduit::Client,
    options: &Options,
    confirm: &dyn Fn(&str) -> bool,
) -> Result<Report, String> {
    let id = normalize_revision_id(&options.revision)?;
    let object_name = format!("D{id}");
    let revision = query_revision(client, &id)?;
    check_closable(&revision, options, &object_name)?;

    let mut progress = String::new();
    let mut warnings = String::new();
    let mut closed = false;
    if let Some(rev) = &revision {
        confirm_ownership(client, options, rev, &object_name, confirm)?;
        let step = close_if_accepted(client, options, rev, &id, &object_name)?;
        progress += &step.progress;
        warnings += &step.warnings;
        closed = step.closed;
    }

    let closing = closing_message(&revision, options.quiet);
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

fn confirm_interactive(prompt: &str) -> bool {
    use std::io::BufRead;
    eprint!("{prompt} [y/N] ");
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(_) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let client = match config::client_for(options.conduit_uri.as_deref()) {
        Ok(client) => client,
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}");
            return ExitCode::FAILURE;
        }
    };
    match execute(&client, &options, &confirm_interactive) {
        Ok(report) => {
            print!("{}", report.stdout);
            eprint!("{}", report.stderr);
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "close_revision_tests.rs"]
mod close_revision_tests;
