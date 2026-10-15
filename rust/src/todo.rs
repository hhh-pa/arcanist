//! `arc todo`: quickly create a Maniphest task for yourself.
//!
//! Port of `src/workflow/ArcanistTodoWorkflow.php`: resolves `--cc` users
//! with `user.query` and `--project` slugs with `project.query`, then creates
//! the task with `maniphest.createtask` owned by the authenticated user.
//!
//! The creation line keeps the PHP console colors (green title, blue URI)
//! on TTYs, gated on the process ANSI setting like `tsprintf()`.

use std::process::ExitCode;

use crate::{conduit, config, terminal};

/// Subcommand name, matching `arc todo`.
pub const NAME: &str = "todo";

const USAGE: &str = "usage: arc todo SUMMARY [--cc USER] [--project SLUG] [--json] [--browse] [--conduit-uri URI]\n";

/// Parsed `todo` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Summary words, joined with spaces.
    pub summary: Vec<String>,
    /// Extra users to CC (`--cc`, repeatable).
    pub cc: Vec<String>,
    /// Project slugs to attach (`--project`, repeatable).
    pub projects: Vec<String>,
    /// Open the new task in a browser.
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

/// One argument's parse step: consumed, `--help`, or a fatal error.
enum Step {
    Handled,
    Help,
    Error(String),
}

struct Parser {
    summary: Vec<String>,
    cc: Vec<String>,
    projects: Vec<String>,
    browse: bool,
    json: bool,
    conduit_uri: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            summary: Vec::new(),
            cc: Vec::new(),
            projects: Vec::new(),
            browse: false,
            json: false,
            conduit_uri: None,
            end_of_flags: false,
        }
    }

    fn cc_error(arg: &str) -> String {
        if arg == "-C" {
            "-C requires a value".to_string()
        } else {
            "--cc requires a value".to_string()
        }
    }

    fn flag_cc(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--cc" || arg == "-C" || arg.starts_with("--cc=") {
            match flag_value(iter, arg, "--cc") {
                Ok(value) => self.cc.push(value),
                Err(_) => return Some(Step::Error(Self::cc_error(arg))),
            }
            return Some(Step::Handled);
        }
        None
    }

    fn flag_project(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--project" || arg.starts_with("--project=") {
            match flag_value(iter, arg, "--project") {
                Ok(value) => self.projects.push(value),
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
        if arg == "--browse" {
            self.browse = true;
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
        self.summary.push(arg.to_string());
        Step::Handled
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            self.summary.push(arg.to_string());
            return Step::Handled;
        }
        if let Some(step) = self.flag_cc(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_project(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        self.tail(arg)
    }

    fn finish(self) -> Options {
        Options {
            summary: self.summary,
            cc: self.cc,
            projects: self.projects,
            browse: self.browse,
            json: self.json,
            conduit_uri: self.conduit_uri,
        }
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
    Ok(Some(parser.finish()))
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
            .and_then(|v| v.as_str())
            .filter(|phid| !phid.is_empty() && *phid != "0");
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

/// Render the creation confirmation line: the title and URI keep console
/// colors (TTY) but substitute raw, like `phutil_console_format()`.
#[must_use]
pub fn render_created(task: &CreatedTask, ansi: bool) -> String {
    let title = terminal::console_format("<fg:green>**%s**</fg>", &[&task.title], ansi);
    let uri = terminal::console_format("<fg:blue>**%s**</fg>", &[&task.uri], ansi);
    terminal::pht(
        "Created task %s: '%s' at %s\n",
        &[
            terminal::PhtArg::Text(&format!("T{}", task.id)),
            terminal::PhtArg::Text(&title),
            terminal::PhtArg::Text(&uri),
        ],
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

fn conduit_error_text(error: conduit::Error) -> String {
    error.to_string()
}

fn missing_task_id() -> String {
    "conduit maniphest.createtask response has no id".to_string()
}

fn json_encode_error(error: serde_json::Error) -> String {
    format!("failed to encode JSON: {error}")
}

fn is_blank_summary(summary: &str) -> bool {
    summary
        .trim_matches([' ', '\t', '\n', '\r', '\0', '\u{b}'])
        .is_empty()
}

fn phid_array(phids: Vec<String>) -> serde_json::Value {
    serde_json::Value::Array(phids.into_iter().map(serde_json::Value::String).collect())
}

fn task_create_params(
    client: &conduit::Client,
    options: &Options,
    summary: String,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut params = serde_json::Map::from_iter([
        ("title".to_string(), serde_json::Value::String(summary)),
        (
            "ownerPHID".to_string(),
            serde_json::Value::String(user_phid(client)?),
        ),
    ]);
    if !options.cc.is_empty() {
        let phids = resolve_cc_phids(client, &options.cc)?;
        params.insert("ccPHIDs".to_string(), phid_array(phids));
    }
    if !options.projects.is_empty() {
        let phids = resolve_project_phids(client, &options.projects)?;
        params.insert("projectPHIDs".to_string(), phid_array(phids));
    }
    Ok(params)
}

fn finish_created_task(
    dir: &std::path::Path,
    task: &CreatedTask,
    options: &Options,
    ansi: bool,
) -> Result<String, String> {
    if options.browse {
        let configured = crate::browser::configured_browser(dir);
        crate::browser::open_uris_in_browser(
            std::slice::from_ref(&task.uri),
            configured.as_deref(),
        )?;
    }
    if options.json {
        let mut doc =
            serde_json::to_string_pretty(&created_json(task)).map_err(json_encode_error)?;
        doc.push('\n');
        return Ok(doc);
    }
    Ok(render_created(task, ansi))
}

/// Create the task per `options`, returning the stdout text.
///
/// Like the PHP reference, a blank summary prints a message and creates
/// nothing. `--browse` opens the new task, resolving the browser command
/// from the config for the current directory.
pub fn execute(client: &conduit::Client, options: &Options) -> Result<String, String> {
    execute_ansi(client, options, terminal::ansi_enabled())
}

/// Create the task per `options` with an explicit ANSI setting
/// (deterministic under test).
pub fn execute_ansi(
    client: &conduit::Client,
    options: &Options,
    ansi: bool,
) -> Result<String, String> {
    let dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(_) => std::path::PathBuf::from("."),
    };
    execute_in_ansi(&dir, client, options, ansi)
}

/// Create the task per `options` with `dir` pinning config discovery.
pub fn execute_in(
    dir: &std::path::Path,
    client: &conduit::Client,
    options: &Options,
) -> Result<String, String> {
    execute_in_ansi(dir, client, options, terminal::ansi_enabled())
}

/// Create the task per `options` in `dir` with an explicit ANSI setting
/// (deterministic under test).
pub fn execute_in_ansi(
    dir: &std::path::Path,
    client: &conduit::Client,
    options: &Options,
    ansi: bool,
) -> Result<String, String> {
    let summary = options.summary.join(" ");
    if is_blank_summary(&summary) {
        return Ok("Please provide a summary.\n".to_string());
    }
    let params = task_create_params(client, options, summary)?;
    let result = client
        .call("maniphest.createtask", &serde_json::Value::Object(params))
        .map_err(conduit_error_text)?;
    let task = parse_created(&result).ok_or_else(missing_task_id)?;
    finish_created_task(dir, &task, options, ansi)
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
            eprintln!("arc {NAME}: {msg}\n\n{USAGE}");
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
    match execute_ansi(&client, &options, terminal::ansi_enabled()) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc {NAME}: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "todo_tests.rs"]
mod todo_tests;
