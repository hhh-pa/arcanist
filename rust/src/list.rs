//! `arc list`: list your open Differential revisions.
//!
//! Port of `src/workflow/ArcanistListWorkflow.php`: queries
//! `differential.query` for open revisions authored by the authenticated
//! user, marks with `*` the ones whose source path is the current working
//! copy, and prints them sorted like the PHP table (or as `--json`).
//!
//! Known limits (see T1128): output is plain text without the PHP status
//! colors.
//!
//! Like the PHP runtime for `requiresRepositoryAPI()` workflows, [`run`]
//! fails outside a repository before any Conduit call; [`execute_in`]
//! stays lenient (no `*` markers) for direct callers.

use std::path::Path;
use std::process::ExitCode;

use crate::{
    conduit::{self, EnvelopeKind},
    config, get_config, git, svn, terminal,
};

/// Subcommand name, matching `arc list`.
pub const NAME: &str = "list";

const USAGE: &str = "usage: arc list [--json] [--conduit-uri URI]\n";

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

/// Sort like the PHP table key: `sprintf('%d%04d%08d', exists, status,
/// id)` ascending.
pub fn sort_revisions(revisions: &mut [RevisionInfo]) {
    revisions.sort_by(|a, b| {
        let key = |r: &RevisionInfo| {
            format!(
                "{}{:04}{:08}",
                r.exists as u8,
                r.status,
                terminal::php_int_cast(&r.id)
            )
        };
        key(a).cmp(&key(b))
    });
}

/// Map a revision status name to a terminal color, defaulting like PHP.
#[must_use]
pub fn status_color(status_name: &str) -> &'static str {
    match status_name {
        "Closed" => "cyan",
        "Needs Review" => "magenta",
        "Needs Revision" | "Changes Planned" => "red",
        "Accepted" => "green",
        "No Revision" => "blue",
        _ => "default",
    }
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

/// Walk toward the filesystem root for a Mercurial `.hg` dir.
#[must_use]
pub fn find_hg_root(start: &Path) -> Option<std::path::PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".hg").exists())
        .map(Path::to_path_buf)
}

/// Working-copy root for `*` markers: git first, then Mercurial, then
/// Subversion, mirroring the PHP repository API detection.
#[must_use]
pub fn working_copy_root(cwd: &Path) -> Option<String> {
    if let Ok(top) = git::stdout_text(cwd, &["rev-parse", "--show-toplevel"]) {
        return Some(top);
    }
    if let Some(root) = find_hg_root(cwd) {
        return Some(root.to_string_lossy().to_string());
    }
    svn::find_metadata_path(cwd)
        .and_then(|meta| meta.parent().map(Path::to_path_buf))
        .map(|root| root.to_string_lossy().to_string())
}

/// Authenticated user's PHID: the identity cached at startup auth,
/// else one `user.whoami` call (T1153 RB-D8: PHP resolves whoami once
/// first, then reuses the cached PHID).
pub fn user_phid(client: &conduit::Client) -> Result<String, String> {
    if let Some((_, phid)) = client.authenticated_user() {
        return Ok(phid);
    }
    let who = client
        .call("user.whoami", &serde_json::json!({}))
        .map_err(|e| e.to_string())?;
    who.get("phid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "conduit user.whoami response has no phid".to_string())
}

/// Render revisions as a console table, `*`-marking rows that live in
/// this working copy.
#[must_use]
pub fn render_table(revisions: &[RevisionInfo], ansi: bool) -> String {
    let rows: Vec<Vec<terminal::TableCell>> = revisions
        .iter()
        .map(|r| {
            let exists = if r.exists {
                terminal::TableCell::Terminal(
                    terminal::tsprintf_ansi("**%s**", &[terminal::TVal::Text("*")], ansi)
                        .unwrap_or_default(),
                )
            } else {
                terminal::TableCell::Text(String::new())
            };
            let status = terminal::tsprintf_ansi(
                &format!("<fg:{}>%s</fg>", status_color(&r.status_name)),
                &[terminal::TVal::Text(&r.status_name)],
                ansi,
            )
            .unwrap_or_default();
            let title = terminal::tsprintf_ansi(
                "**D%d:** %s",
                &[
                    terminal::TVal::Int(terminal::php_int_cast(&r.id)),
                    terminal::TVal::Text(&r.title),
                ],
                ansi,
            )
            .unwrap_or_default();
            vec![
                exists,
                terminal::TableCell::Terminal(status),
                terminal::TableCell::Terminal(title),
            ]
        })
        .collect();
    terminal::console_table(&["", "Status", "Title"], &rows)
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
///
/// Outside a repository no row is marked; the [`run`] entry point rejects
/// that case with the exact PHP error instead (the runtime working-copy
/// check for `requiresRepositoryAPI()` workflows).
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
    if let Some(top) = working_copy_root(cwd) {
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
    Ok(render_table(&revisions, terminal::ansi_enabled()))
}

/// Query and render open revisions against the current directory.
pub fn execute(client: &conduit::Client, options: &Options) -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    execute_in(client, options, &cwd)
}

/// Reject `cwd` outside a repository with the exact PHP error.
///
/// Mirrors the `arc` runtime working-copy check for workflows with
/// `requiresRepositoryAPI()`: `arc list` fails before any Conduit call.
fn require_working_copy(cwd: &Path) -> Result<(), String> {
    if working_copy_root(cwd).is_some() {
        Ok(())
    } else {
        Err(terminal::pht(
            "This command must be run in a Git, Mercurial or Subversion working copy.",
            &[],
        ))
    }
}

/// Entry point rooted at `cwd`: `argv` excludes the subcommand name.
pub fn run_in(cwd: &Path, argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        // T1153 RB-D4 (R4): `list` is a subprocess workflow:
        // usage errors render `Usage Exception:` with no usage dump
        // (`scripts/arcanist.php:432-444`).
        Err(msg) => return conduit::report(EnvelopeKind::Usage, &msg),
    };
    if let Err(msg) = require_working_copy(cwd) {
        // The working-copy gate is a usage error (same envelope).
        return conduit::report(EnvelopeKind::Usage, &msg);
    }
    // T1153 CORE-4/T1153 CORE-9: classic `requiresAuthentication` startup
    // (tokenless engine, credential gate, `user.whoami` /
    // `conduit.connect` verification).
    let user_file = config::arcrc_path();
    let client = match config::authed_client_for(
        options.conduit_uri.as_deref(),
        cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
        &config::command_description(),
    ) {
        Ok(client) => client,
        Err(error) => return error.report(),
    };
    match execute_in(&client, &options, cwd) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(msg) => conduit::report(EnvelopeKind::Exception, &msg),
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod list_tests;
