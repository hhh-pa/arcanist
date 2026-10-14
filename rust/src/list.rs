//! `arc-rs list`: list your open Differential revisions.
//!
//! Port of `src/workflow/ArcanistListWorkflow.php`: queries
//! `differential.query` for open revisions authored by the authenticated
//! user, marks with `*` the ones whose source path is the current working
//! copy, and prints them sorted like the PHP table (or as `--json`).
//!
//! Known limits (see T1128): output is plain text without the PHP status
//! colors; outside a git repository the command still works with no `*`
//! markers instead of failing like the PHP reference.

use std::path::Path;
use std::process::ExitCode;

use crate::{conduit, config, git};

/// Subcommand name, matching `arc list`.
pub const NAME: &str = "list";

const USAGE: &str = "usage: arc-rs list [--json] [--conduit-uri URI]\n";

/// Parsed `list` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Emit a JSON array instead of text rows.
    pub json: bool,
    /// Explicit conduit URI override.
    pub conduit_uri: Option<String>,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut json = false;
    let mut conduit_uri = None;
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            return Err(format!("unexpected argument '{arg}'"));
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--json" {
            json = true;
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
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(Some(Options { json, conduit_uri }))
}

/// One open revision from `differential.query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionInfo {
    /// Numeric id without the `D` prefix.
    pub id: String,
    pub title: String,
    pub status: i64,
    pub status_name: String,
    pub source_path: String,
    /// True when the revision's source path is the current working copy.
    pub exists: bool,
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

/// Parse one `differential.query` row; `None` when it has no id.
#[must_use]
pub fn parse_revision(value: &serde_json::Value) -> Option<RevisionInfo> {
    let id_value = value.get("id")?;
    let id = if let Some(n) = id_value.as_i64() {
        n.to_string()
    } else if let Some(n) = id_value.as_u64() {
        n.to_string()
    } else {
        let raw = id_value.as_str()?;
        raw.strip_prefix('D')
            .or_else(|| raw.strip_prefix('d'))
            .unwrap_or(raw)
            .to_string()
    };
    let status = value.get("status").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    });
    let status = status.unwrap_or(0);
    let status_name = value
        .get("statusName")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| status_name_for(status).to_string());
    Some(RevisionInfo {
        id,
        title: value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status,
        status_name,
        source_path: value
            .get("sourcePath")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        exists: false,
    })
}

/// Rows of a `differential.query` result, whether it is a list or a map.
#[must_use]
pub fn parse_revisions(result: &serde_json::Value) -> Vec<RevisionInfo> {
    let items: Vec<&serde_json::Value> = match result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    };
    items.into_iter().filter_map(parse_revision).collect()
}

/// Sort like the PHP table key (`exists`, `status`, `id` ascending).
pub fn sort_revisions(revisions: &mut [RevisionInfo]) {
    revisions.sort_by(|a, b| {
        let key = |r: &RevisionInfo| {
            (
                r.exists,
                r.status,
                r.id.parse::<u64>().unwrap_or(u64::MAX),
                r.id.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
}

/// True when two paths name the same location (symlinks resolved), mirroring
/// the PHP `Filesystem::resolvePath` comparison.
#[must_use]
pub fn same_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
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

/// Render one revision row, `*`-marked when it lives in this working copy.
#[must_use]
pub fn render_row(revision: &RevisionInfo) -> String {
    let marker = if revision.exists { "*" } else { " " };
    format!(
        "{marker} {} D{}: {}",
        revision.status_name, revision.id, revision.title
    )
}

/// Build the `--json` document.
#[must_use]
pub fn revisions_json(revisions: &[RevisionInfo]) -> serde_json::Value {
    serde_json::Value::Array(
        revisions
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": format!("D{}", r.id),
                    "title": r.title,
                    "status": r.status,
                    "statusName": r.status_name,
                    "exists": r.exists,
                })
            })
            .collect(),
    )
}

/// Query and render open revisions, resolving `*` markers against `cwd`.
pub fn execute_in(
    client: &conduit::Client,
    options: &Options,
    cwd: &Path,
) -> Result<String, String> {
    let phid = user_phid(client)?;
    let result = client
        .call(
            "differential.query",
            &serde_json::json!({"authors": [phid], "status": "status-open"}),
        )
        .map_err(|e| e.to_string())?;
    let mut revisions = parse_revisions(&result);
    if revisions.is_empty() {
        return Ok("You have no open Differential revisions.\n".to_string());
    }
    let toplevel = git::stdout_text(cwd, &["rev-parse", "--show-toplevel"]).ok();
    if let Some(top) = toplevel {
        for revision in &mut revisions {
            revision.exists = same_path(&revision.source_path, &top);
        }
    }
    sort_revisions(&mut revisions);
    if options.json {
        let mut doc = serde_json::to_string_pretty(&revisions_json(&revisions))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        doc.push('\n');
        return Ok(doc);
    }
    let mut out = String::new();
    for revision in &revisions {
        out += &render_row(revision);
        out.push('\n');
    }
    Ok(out)
}

/// Query and render open revisions against the current directory.
pub fn execute(client: &conduit::Client, options: &Options) -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    execute_in(client, options, &cwd)
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
    match execute(&client, &options) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod list_tests;
