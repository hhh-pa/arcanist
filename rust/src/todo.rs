//! `arc-rs todo`: quickly create a Maniphest task for yourself.
//!
//! Port of `src/workflow/ArcanistTodoWorkflow.php`: resolves `--cc` users
//! with `user.query` and `--project` slugs with `project.query`, then creates
//! the task with `maniphest.createtask` owned by the authenticated user.
//!
//! Known limits (see T1128): `--browse` is not yet ported and fails before
//! any task is created; output is plain text without the PHP colors.

use std::process::ExitCode;

use crate::{conduit, config};

/// Subcommand name, matching `arc todo`.
pub const NAME: &str = "todo";

const USAGE: &str =
    "usage: arc-rs todo SUMMARY [--cc USER] [--project SLUG] [--json] [--conduit-uri URI]\n";

/// Parsed `todo` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Summary words, joined with spaces.
    pub summary: Vec<String>,
    /// Extra users to CC (`--cc`, repeatable).
    pub cc: Vec<String>,
    /// Project slugs to attach (`--project`, repeatable).
    pub projects: Vec<String>,
    /// Open the new task in a browser (not yet ported).
    pub browse: bool,
    /// Emit the created task as JSON.
    pub json: bool,
    /// Explicit conduit URI override.
    pub conduit_uri: Option<String>,
}

fn flag_value(
    iter: &mut std::slice::Iter<String>,
    arg: &str,
    flag: &str,
) -> Result<String, String> {
    if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
        if value.is_empty() {
            return Err(format!("{flag} requires a value"));
        }
        return Ok(value.to_string());
    }
    let Some(value) = iter.next() else {
        return Err(format!("{flag} requires a value"));
    };
    Ok(value.clone())
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut summary = Vec::new();
    let mut cc = Vec::new();
    let mut projects = Vec::new();
    let mut browse = false;
    let mut json = false;
    let mut conduit_uri = None;
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            summary.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--cc" || arg == "-C" || arg.starts_with("--cc=") {
            cc.push(flag_value(&mut iter, arg, "--cc").map_err(|_| {
                if arg == "-C" {
                    "-C requires a value".to_string()
                } else {
                    "--cc requires a value".to_string()
                }
            })?);
        } else if arg == "--project" || arg.starts_with("--project=") {
            projects.push(flag_value(&mut iter, arg, "--project")?);
        } else if arg == "--browse" {
            browse = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--conduit-uri" || arg.starts_with("--conduit-uri=") {
            conduit_uri = Some(flag_value(&mut iter, arg, "--conduit-uri")?);
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            summary.push(arg.clone());
        }
    }
    Ok(Some(Options {
        summary,
        cc,
        projects,
        browse,
        json,
        conduit_uri,
    }))
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

/// Resolve CC usernames to PHIDs with `user.query`, skipping unknown users
/// like the PHP reference.
pub fn resolve_cc_phids(client: &conduit::Client, ccs: &[String]) -> Result<Vec<String>, String> {
    let users = client
        .call("user.query", &serde_json::json!({"usernames": ccs}))
        .map_err(|e| e.to_string())?;
    let items: Vec<&serde_json::Value> = match &users {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    };
    Ok(items
        .into_iter()
        .filter_map(|u| u.get("phid").and_then(|v| v.as_str()).map(str::to_string))
        .collect())
}

/// Resolve project slugs to PHIDs with `project.query`.
pub fn resolve_project_phids(
    client: &conduit::Client,
    slugs: &[String],
) -> Result<Vec<String>, String> {
    let result = client
        .call("project.query", &serde_json::json!({"slugs": slugs}))
        .map_err(|e| e.to_string())?;
    let slug_map = result.get("slugMap").and_then(|v| v.as_object());
    let mut phids = Vec::with_capacity(slugs.len());
    for slug in slugs {
        let phid = slug_map
            .and_then(|map| map.get(slug))
            .and_then(|v| v.as_str());
        let Some(phid) = phid else {
            return Err(format!("No such project: \"{slug}\""));
        };
        phids.push(phid.to_string());
    }
    Ok(phids)
}

/// A newly created task from `maniphest.createtask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedTask {
    /// Numeric id without the `T` prefix.
    pub id: String,
    pub title: String,
    pub uri: String,
}

/// Parse a `maniphest.createtask` result; `None` when it has no id.
#[must_use]
pub fn parse_created(value: &serde_json::Value) -> Option<CreatedTask> {
    let id_value = value.get("id")?;
    let id = if let Some(n) = id_value.as_i64() {
        n.to_string()
    } else if let Some(n) = id_value.as_u64() {
        n.to_string()
    } else {
        id_value.as_str().map(str::to_string)?
    };
    Some(CreatedTask {
        id,
        title: value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        uri: value
            .get("uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Render the creation confirmation line.
#[must_use]
pub fn render_created(task: &CreatedTask) -> String {
    format!(
        "Created task T{}: '{}' at {}\n",
        task.id, task.title, task.uri
    )
}

/// Build the `--json` document for a created task.
#[must_use]
pub fn created_json(task: &CreatedTask) -> serde_json::Value {
    serde_json::json!({
        "id": format!("T{}", task.id),
        "title": task.title,
        "uri": task.uri,
    })
}

/// Create the task per `options`, returning the stdout text.
///
/// Like the PHP reference, a blank summary prints a message and creates
/// nothing. `--browse` fails before any conduit call.
pub fn execute(client: &conduit::Client, options: &Options) -> Result<String, String> {
    if options.browse {
        return Err("--browse is not yet ported (see T1128)".to_string());
    }
    let summary = options.summary.join(" ");
    if summary.trim().is_empty() {
        return Ok("Please provide a summary.\n".to_string());
    }

    let mut params = serde_json::Map::from_iter([
        ("title".to_string(), serde_json::Value::String(summary)),
        (
            "ownerPHID".to_string(),
            serde_json::Value::String(user_phid(client)?),
        ),
    ]);
    if !options.cc.is_empty() {
        let phids = resolve_cc_phids(client, &options.cc)?;
        params.insert(
            "ccPHIDs".to_string(),
            serde_json::Value::Array(phids.into_iter().map(serde_json::Value::String).collect()),
        );
    }
    if !options.projects.is_empty() {
        let phids = resolve_project_phids(client, &options.projects)?;
        params.insert(
            "projectPHIDs".to_string(),
            serde_json::Value::Array(phids.into_iter().map(serde_json::Value::String).collect()),
        );
    }

    let result = client
        .call("maniphest.createtask", &serde_json::Value::Object(params))
        .map_err(|e| e.to_string())?;
    let task = parse_created(&result)
        .ok_or_else(|| "conduit maniphest.createtask response has no id".to_string())?;
    if options.json {
        let mut doc = serde_json::to_string_pretty(&created_json(&task))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        doc.push('\n');
        return Ok(doc);
    }
    Ok(render_created(&task))
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
#[path = "todo_tests.rs"]
mod todo_tests;
