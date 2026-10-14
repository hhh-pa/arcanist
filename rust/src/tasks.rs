//! `arc-rs tasks`: view assigned Maniphest tasks.
//!
//! Port of `src/workflow/ArcanistTasksWorkflow.php`: resolves the owner (you
//! by default, `--owner`, `--unassigned`, or `@all` for everyone) and lists
//! matching tasks from `maniphest.query` as a plain-text table or `--json`.
//!
//! Known limits (see T1128): output is plain text without the PHP terminal
//! colors; like the PHP reference, an unknown `--owner` username resolves to
//! no owner filter and therefore lists everyone's tasks.

use std::process::ExitCode;

use crate::{conduit, config};

/// Subcommand name, matching `arc tasks`.
pub const NAME: &str = "tasks";

const USAGE: &str = "usage: arc-rs tasks [--status STATUS] [--owner USER] [--order ORDER] [--limit N] [--unassigned] [--json] [--conduit-uri URI]\n";

/// Parsed `tasks` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Raw `--status` value (`open`, `closed`, or `all`).
    pub status: Option<String>,
    /// Raw `--owner` value (a username, or `@all` for everyone).
    pub owner: Option<String>,
    /// Raw `--order` value (`priority`, `created`, or `modified`).
    pub order: Option<String>,
    /// Maximum tasks to list.
    pub limit: Option<u32>,
    /// Only show unassigned tasks.
    pub unassigned: bool,
    /// Emit a JSON array instead of a table.
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
    let mut status = None;
    let mut owner = None;
    let mut order = None;
    let mut limit = None;
    let mut unassigned = false;
    let mut json = false;
    let mut conduit_uri = None;
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            return Err(format!("unexpected argument '{arg}'"));
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--status" || arg.starts_with("--status=") {
            status = Some(flag_value(&mut iter, arg, "--status")?);
        } else if arg == "--owner" || arg.starts_with("--owner=") {
            owner = Some(flag_value(&mut iter, arg, "--owner")?);
        } else if arg == "--order" || arg.starts_with("--order=") {
            order = Some(flag_value(&mut iter, arg, "--order")?);
        } else if arg == "--limit" || arg.starts_with("--limit=") {
            let raw = flag_value(&mut iter, arg, "--limit")?;
            match raw.parse::<u32>() {
                Ok(n) => limit = Some(n),
                Err(_) => return Err("--limit requires an integer value".to_string()),
            }
        } else if arg == "--unassigned" {
            unassigned = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--conduit-uri" || arg.starts_with("--conduit-uri=") {
            conduit_uri = Some(flag_value(&mut iter, arg, "--conduit-uri")?);
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    if owner.is_some() && unassigned {
        return Err(
            "--owner and --unassigned conflict (--owner suppresses unassigned)".to_string(),
        );
    }
    Ok(Some(Options {
        status,
        owner,
        order,
        limit,
        unassigned,
        json,
        conduit_uri,
    }))
}

/// One task row from `maniphest.query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Numeric id without the `T` prefix.
    pub id: String,
    pub title: String,
    pub priority: String,
    pub status_name: String,
    pub is_closed: Option<bool>,
}

/// Parse one `maniphest.query` row; `None` when it has no id.
#[must_use]
pub fn parse_task(value: &serde_json::Value) -> Option<Task> {
    let id_value = value.get("id")?;
    let id = if let Some(n) = id_value.as_i64() {
        n.to_string()
    } else if let Some(n) = id_value.as_u64() {
        n.to_string()
    } else {
        let raw = id_value.as_str()?;
        raw.strip_prefix('T')
            .or_else(|| raw.strip_prefix('t'))
            .unwrap_or(raw)
            .to_string()
    };
    Some(Task {
        id,
        title: value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_end()
            .to_string(),
        priority: value
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status_name: value
            .get("statusName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        is_closed: value.get("isClosed").and_then(|v| v.as_bool()),
    })
}

/// Rows of a `maniphest.query` result, whether it is a list or a map.
#[must_use]
pub fn parse_tasks(result: &serde_json::Value) -> Vec<Task> {
    let items: Vec<&serde_json::Value> = match result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    };
    items.into_iter().filter_map(parse_task).collect()
}

/// Build the `maniphest.query` parameters, mirroring the PHP defaults
/// (`status-open`, `order-priority`); `all` maps to `any` like the PHP.
#[must_use]
pub fn query_params(
    owner_phid: Option<&str>,
    status: Option<&str>,
    order: Option<&str>,
    limit: Option<u32>,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(phid) = owner_phid {
        params.insert("ownerPHIDs".to_string(), serde_json::json!([phid]));
    }
    if let Some(n) = limit {
        params.insert("limit".to_string(), serde_json::Value::from(n));
    }
    params.insert(
        "order".to_string(),
        serde_json::Value::String(format!("order-{}", order.unwrap_or("priority"))),
    );
    let status = match status {
        Some("all") => "any",
        Some(s) => s,
        None => "open",
    };
    params.insert(
        "status".to_string(),
        serde_json::Value::String(format!("status-{status}")),
    );
    serde_json::Value::Object(params)
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

/// Resolve `--owner` to a PHID: `@all` and unknown users yield `None` (no
/// owner filter), mirroring the PHP reference.
pub fn find_owner_phid(client: &conduit::Client, owner: &str) -> Result<Option<String>, String> {
    if owner == "@all" {
        return Ok(None);
    }
    let users = client
        .call("user.query", &serde_json::json!({"usernames": [owner]}))
        .map_err(|e| e.to_string())?;
    let first = match &users {
        serde_json::Value::Array(items) => items.first(),
        serde_json::Value::Object(map) => map.values().next(),
        _ => None,
    };
    Ok(first
        .and_then(|u| u.get("phid"))
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// Render tasks as an aligned plain-text table (ID, Title, Priority, Status).
#[must_use]
pub fn render_table(tasks: &[Task]) -> String {
    let mut widths = [2usize, 5, 8, 6];
    let rows: Vec<[String; 4]> = tasks
        .iter()
        .map(|t| {
            [
                format!("T{}", t.id),
                t.title.clone(),
                t.priority.clone(),
                t.status_name.clone(),
            ]
        })
        .collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let mut out = String::new();
    for row in rows {
        let line = format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        );
        out += line.trim_end();
        out.push('\n');
    }
    out
}

/// Build the `--json` document.
#[must_use]
pub fn tasks_json(tasks: &[Task]) -> serde_json::Value {
    serde_json::Value::Array(
        tasks
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": format!("T{}", t.id),
                    "title": t.title,
                    "priority": t.priority,
                    "status": t.status_name,
                    "isClosed": t.is_closed,
                })
            })
            .collect(),
    )
}

/// Query and render tasks per `options`, returning the stdout text.
pub fn execute(client: &conduit::Client, options: &Options) -> Result<String, String> {
    let owner_phid = if let Some(owner) = &options.owner {
        find_owner_phid(client, owner)?
    } else if options.unassigned {
        None
    } else {
        Some(user_phid(client)?)
    };
    let params = query_params(
        owner_phid.as_deref(),
        options.status.as_deref(),
        options.order.as_deref(),
        options.limit,
    );
    let result = client
        .call("maniphest.query", &params)
        .map_err(|e| e.to_string())?;
    let tasks = parse_tasks(&result);
    if tasks.is_empty() {
        return Ok("No tasks found.\n".to_string());
    }
    if options.json {
        let mut doc = serde_json::to_string_pretty(&tasks_json(&tasks))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        doc.push('\n');
        return Ok(doc);
    }
    Ok(render_table(&tasks))
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
#[path = "tasks_tests.rs"]
mod tasks_tests;
