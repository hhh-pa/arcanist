//! `arc tasks`: view assigned Maniphest tasks.
//!
//! Port of `src/workflow/ArcanistTasksWorkflow.php`: resolves the owner (you
//! by default, `--owner`, `--unassigned`, or `@all` for everyone) and lists
//! matching tasks from `maniphest.query` as a plain-text table or `--json`.
//!
//! Known limits (see T1128): output is plain text without the PHP terminal
//! colors; like the PHP reference, an unknown `--owner` username resolves to
//! no owner filter and therefore lists everyone's tasks.

use std::process::ExitCode;

use crate::{
    conduit::{self, EnvelopeKind},
    config, get_config, terminal,
};

/// Subcommand name, matching `arc tasks`.
pub const NAME: &str = "tasks";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistTasksWorkflow.php:14-64`).
const HELP_SYNOPSIS: &str = "      **tasks** [__options__]";
const HELP_TEXT: &str = "        View all assigned tasks.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "status",
        None,
        Some("task_status"),
        "Show tasks that are open or closed, default is open.",
    ),
    (
        "owner",
        None,
        Some("username"),
        "Only show tasks assigned to the given username, also accepts @all to show all, default is you.",
    ),
    (
        "order",
        None,
        Some("task_order"),
        "Arrange tasks based on priority, created, or modified, default is priority.",
    ),
    (
        "limit",
        None,
        Some("n"),
        "Limit the amount of tasks outputted, default is all.",
    ),
    (
        "unassigned",
        None,
        None,
        "Only show tasks that are not assigned (upforgrabs).",
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
        None,
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        false,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(terminal::ansi_enabled())
}

/// Parsed `tasks` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Raw `--status` value (`open`, `closed`, or `all`).
    pub status: Option<String>,
    /// Raw `--owner` value (a username, or `@all` for everyone).
    pub owner: Option<String>,
    /// Raw `--order` value (`priority`, `created`, or `modified`).
    pub order: Option<String>,
    /// Raw `--limit` value, sent to conduit verbatim (T1153 RB-D28:
    /// PHP strips the `int` paramtype, so `--limit abc` is sent raw).
    pub limit: Option<String>,
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

/// One argument's parse step: consumed, `--help`, or a fatal error.
enum Step {
    Handled,
    Help,
    Error(String),
}

struct Parser {
    status: Option<String>,
    owner: Option<String>,
    order: Option<String>,
    limit: Option<String>,
    unassigned: bool,
    json: bool,
    conduit_uri: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            status: None,
            owner: None,
            order: None,
            limit: None,
            unassigned: false,
            json: false,
            conduit_uri: None,
            end_of_flags: false,
        }
    }

    fn flag_status(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--status" || arg.starts_with("--status=") {
            match flag_value(iter, arg, "--status") {
                Ok(value) => self.status = Some(value),
                Err(message) => return Some(Step::Error(message)),
            }
            return Some(Step::Handled);
        }
        None
    }

    fn flag_owner(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--owner" || arg.starts_with("--owner=") {
            match flag_value(iter, arg, "--owner") {
                Ok(value) => self.owner = Some(value),
                Err(message) => return Some(Step::Error(message)),
            }
            return Some(Step::Handled);
        }
        None
    }

    fn flag_order(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--order" || arg.starts_with("--order=") {
            match flag_value(iter, arg, "--order") {
                Ok(value) => self.order = Some(value),
                Err(message) => return Some(Step::Error(message)),
            }
            return Some(Step::Handled);
        }
        None
    }

    fn flag_limit(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--limit" || arg.starts_with("--limit=") {
            // T1153 RB-D28: no integer validation — PHP strips the
            // `int` paramtype (`newLegacyPhutilWorkflow` unsets it),
            // so any value rides through to conduit verbatim.
            match flag_value(iter, arg, "--limit") {
                Ok(raw) => self.limit = Some(raw),
                Err(message) => return Some(Step::Error(message)),
            }
            return Some(Step::Handled);
        }
        None
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--conduit-uri" || arg.starts_with("--conduit-uri=") {
            match flag_value(iter, arg, "--conduit-uri") {
                Ok(value) => self.conduit_uri = Some(value),
                Err(message) => return Some(Step::Error(message)),
            }
            return Some(Step::Handled);
        }
        None
    }

    fn tail(&mut self, arg: &str) -> Step {
        if arg == "--" {
            self.end_of_flags = true;
            return Step::Handled;
        }
        if arg == "--unassigned" {
            self.unassigned = true;
            return Step::Handled;
        }
        if arg == "--json" {
            self.json = true;
            return Step::Handled;
        }
        if arg == "--help" || arg == "-h" {
            return Step::Help;
        }
        if arg.starts_with('-') {
            return Step::Error(format!("unknown flag '{arg}'"));
        }
        Step::Error(format!("unexpected argument '{arg}'"))
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            return Step::Error(format!("unexpected argument '{arg}'"));
        }
        if let Some(step) = self.flag_status(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_owner(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_order(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_limit(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        self.tail(arg)
    }

    fn finish(self) -> Result<Option<Options>, String> {
        // T1153 RB-D28: no owner/unassigned conflict — PHP strips the
        // `conflict` spec (`newLegacyPhutilWorkflow` unsets it), so
        // both flags are accepted and `--owner` wins in `execute`
        // (`if ($owner) ... else if ($unassigned)`).
        Ok(Some(Options {
            status: self.status,
            owner: self.owner,
            order: self.order,
            limit: self.limit,
            unassigned: self.unassigned,
            json: self.json,
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

/// One task row from `maniphest.query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Numeric id without the `T` prefix.
    pub id: String,
    pub title: String,
    pub priority: String,
    pub priority_color: Option<String>,
    pub status_name: String,
    pub is_closed: Option<bool>,
}

/// Parse one `maniphest.query` row; `None` when it has no id.
#[must_use]
/// Task id as digits: numbers stringify, `"T42"`/`"t42"` shed the prefix.
fn task_id(id_value: &serde_json::Value) -> Option<String> {
    if let Some(n) = id_value.as_i64() {
        return Some(n.to_string());
    }
    if let Some(n) = id_value.as_u64() {
        return Some(n.to_string());
    }
    let raw = id_value.as_str()?;
    Some(
        raw.strip_prefix('T')
            .or_else(|| raw.strip_prefix('t'))
            .unwrap_or(raw)
            .to_string(),
    )
}

/// T1153 RB-D39: PHP reads `isClosed` with loose truthiness, so string
/// forms (`"0"`/`""` false, anything else true) apply like numbers do.
fn task_is_closed(value: Option<&serde_json::Value>) -> Option<bool> {
    value.and_then(|v| {
        v.as_bool()
            .or_else(|| v.as_i64().map(|n| n != 0))
            .or_else(|| v.as_u64().map(|n| n != 0))
            .or_else(|| v.as_str().map(|s| !s.is_empty() && s != "0"))
    })
}

pub fn parse_task(value: &serde_json::Value) -> Option<Task> {
    let id = task_id(value.get("id")?)?;
    Some(Task {
        id,
        title: value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_end_matches([' ', '\t', '\n', '\r', '\0', '\u{b}'])
            .to_string(),
        priority: value
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        priority_color: value
            .get("priorityColor")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        status_name: value
            .get("statusName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        is_closed: task_is_closed(value.get("isClosed")),
    })
}

/// Rows of a `maniphest.query` result, whether it is a list or a map
/// (T1153 RB-D26: server order, not sorted — `preserve_order` keeps
/// the map's insertion order, matching PHP's `foreach`).
#[must_use]
pub fn parse_tasks(result: &serde_json::Value) -> Vec<Task> {
    let items: Vec<&serde_json::Value> = match result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    };
    items.into_iter().filter_map(parse_task).collect()
}

/// True for the argument values PHP treats as empty in
/// `loadManiphestTasks` (`($order ? ... : ...)`): missing, `""`, or
/// `"0"` (the only falsy strings).
fn is_blank_arg(value: Option<&str>) -> bool {
    matches!(value, None | Some("") | Some("0"))
}

/// Build the `maniphest.query` parameters, mirroring the PHP defaults
/// (`status-open`, `order-priority`); `all` maps to `any` like the PHP.
/// Empty `--order`/`--status` fall back to the defaults (T1153 RB-D28:
/// PHP's `?:` treats them as missing), and `--limit` rides through
/// verbatim as the argv string PHP sends.
#[must_use]
pub fn query_params(
    owner_phid: Option<&str>,
    status: Option<&str>,
    order: Option<&str>,
    limit: Option<&str>,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(phid) = owner_phid {
        params.insert("ownerPHIDs".to_string(), serde_json::json!([phid]));
    }
    if let Some(raw) = limit {
        params.insert(
            "limit".to_string(),
            serde_json::Value::String(raw.to_string()),
        );
    }
    let order = if is_blank_arg(order) {
        "priority"
    } else {
        order.unwrap_or("priority")
    };
    params.insert(
        "order".to_string(),
        serde_json::Value::String(format!("order-{order}")),
    );
    let status = match status {
        Some("all") => "any",
        Some(s) if !is_blank_arg(Some(s)) => s,
        _ => "open",
    };
    params.insert(
        "status".to_string(),
        serde_json::Value::String(format!("status-{status}")),
    );
    serde_json::Value::Object(params)
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

/// Map a Maniphest priority color to a terminal color, defaulting to
/// white like the PHP reference.
#[must_use]
pub fn priority_color(web_color: Option<&str>) -> &'static str {
    match web_color {
        Some("violet" | "indigo" | "lightindigo" | "lightviolet") => "magenta",
        Some("orange" | "red" | "lightred" | "lightorange") => "red",
        Some("sky" | "cyan") => "cyan",
        Some("yellow" | "lightyellow") => "yellow",
        Some("green" | "lightgreen") => "green",
        Some("blue" | "lightblue" | "lightsky") => "blue",
        Some("magenta") => "magenta",
        _ => "white",
    }
}

/// Render tasks as a console table (ID, Title, Priority, Status),
/// mirroring the PHP `PhutilConsoleTable` cells.
#[must_use]
pub fn render_table(tasks: &[Task], ansi: bool) -> String {
    let rows: Vec<Vec<terminal::TableCell>> = tasks
        .iter()
        .map(|t| {
            let id = terminal::tsprintf_ansi(
                "**%s**",
                &[terminal::TVal::Text(&format!("T{}", t.id))],
                ansi,
            )
            .unwrap_or_default();
            let priority = terminal::tsprintf_ansi(
                &format!(
                    "<bg:{}> </bg> %s",
                    priority_color(t.priority_color.as_deref())
                ),
                &[terminal::TVal::Text(&t.priority)],
                ansi,
            )
            .unwrap_or_default();
            let status = match t.is_closed {
                // PHP leaves the status column blank when `isClosed` is
                // missing from the row.
                None => terminal::TableCell::Text(String::new()),
                Some(closed) => {
                    let color = if closed { "red" } else { "green" };
                    terminal::TableCell::Terminal(
                        terminal::tsprintf_ansi(
                            &format!("<bg:{color}> </bg> %s"),
                            &[terminal::TVal::Text(&t.status_name)],
                            ansi,
                        )
                        .unwrap_or_default(),
                    )
                }
            };
            vec![
                terminal::TableCell::Terminal(id),
                terminal::TableCell::Text(t.title.clone()),
                terminal::TableCell::Terminal(priority),
                status,
            ]
        })
        .collect();
    terminal::console_table(&["ID", "Title", "Priority", "Status"], &rows)
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
        options.limit.as_deref(),
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
    Ok(render_table(&tasks, terminal::ansi_enabled()))
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        // T1153 RB-D4 (R4): `tasks` is a subprocess workflow:
        // usage errors render `Usage Exception:` with no usage dump
        // (`scripts/arcanist.php:432-444`).
        Err(msg) => return conduit::report(EnvelopeKind::Usage, &msg),
    };
    // T1153 CORE-4/T1153 CORE-9: classic `requiresAuthentication` startup
    // (tokenless engine, credential gate, `user.whoami` /
    // `conduit.connect` verification).
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let user_file = config::arcrc_path();
    let client = match config::authed_client_for(
        options.conduit_uri.as_deref(),
        &cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
        &config::command_description(),
    ) {
        Ok(client) => client,
        Err(error) => return error.report(),
    };
    match execute(&client, &options) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(msg) => conduit::report(EnvelopeKind::Exception, &msg),
    }
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tasks_tests;
