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

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut finalize = false;
    let mut quiet = false;
    let mut json = false;
    let mut force = false;
    let mut conduit_uri = None;
    let mut revision: Option<String> = None;
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            if revision.is_some() {
                return Err("close-revision requires exactly one revision.".to_string());
            }
            revision = Some(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--finalize" {
            finalize = true;
        } else if arg == "--quiet" {
            quiet = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--force" {
            force = true;
        } else if arg == "--conduit-uri" {
            let Some(value) = iter.next() else {
                return Err("--conduit-uri requires a value".to_string());
            };
            conduit_uri = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            if value.is_empty() {
                return Err("--conduit-uri requires a value".to_string());
            }
            conduit_uri = Some(value.to_string());
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            if revision.is_some() {
                return Err("close-revision requires exactly one revision.".to_string());
            }
            revision = Some(arg.clone());
        }
    }
    let Some(revision) = revision else {
        return Err("close-revision requires a revision number.".to_string());
    };
    Ok(Some(Options {
        revision,
        finalize,
        quiet,
        json,
        force,
        conduit_uri,
    }))
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

    if revision.is_none() && !options.finalize {
        return Err(format!("Revision {object_name} does not exist."));
    }
    if let Some(rev) = &revision
        && !options.finalize
        && rev.status != STATUS_ACCEPTED
    {
        return Err(format!(
            "Revision {object_name} can not be closed. You can only close revisions which have been 'accepted'."
        ));
    }

    let mut progress = String::new();
    let mut warnings = String::new();
    let mut closed = false;

    if let Some(rev) = &revision {
        let display = format!("{object_name} {}", rev.title);
        if !options.finalize && rev.author_phid != user_phid(client)? && !options.force {
            let prompt = format!(
                "You are not the author of revision \"{object_name}\", are you sure you want to close it?"
            );
            if !confirm(&prompt) {
                return Err("Aborted.".to_string());
            }
        }

        // See T13458: the server rejects a close transition unless the
        // revision is accepted, so finalize mode skips the attempt.
        let mut actually_close = true;
        if options.finalize && rev.status != STATUS_ACCEPTED {
            warnings += &format!(
                "OPEN REVISION: Revision \"{object_name}\" is not in state \"Accepted\", so it will be left open.\n"
            );
            actually_close = false;
        }

        if actually_close {
            progress += &format!("Closing revision \"{display}\"...\n");
            client
                .call(
                    "differential.close",
                    &serde_json::json!({"revisionID": id_param(&id)}),
                )
                .map_err(|e| e.to_string())?;
            closed = true;
            progress += "Done, closed revision.\n";
        }
    }

    let is_finalized = matches!(&revision, Some(rev)
        if (rev.status == STATUS_ACCEPTED || rev.status == STATUS_CLOSED)
            && rev.commits.is_empty());
    let mut closing = String::new();
    if !options.quiet {
        if is_finalized {
            closing += "You may now push this commit upstream, as appropriate (e.g. with 'git push', or 'git svn dcommit', or by printing and faxing it).\n";
        } else {
            closing += "Done.\n";
        }
    }

    if options.json {
        let mut doc =
            serde_json::to_string_pretty(&result_json(&object_name, revision.as_ref(), closed))
                .map_err(|e| format!("failed to encode JSON: {e}"))?;
        doc.push('\n');
        warnings = progress + &closing + &warnings;
        Ok(Report {
            stdout: doc,
            stderr: warnings,
        })
    } else {
        Ok(Report {
            stdout: progress + &closing,
            stderr: warnings,
        })
    }
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
