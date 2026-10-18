//! `arc todo`: quickly create a Maniphest task for yourself.
//!
//! Port of `src/workflow/ArcanistTodoWorkflow.php`: resolves `--cc` users
//! with `user.query` and `--project` slugs with `project.query`, then creates
//! the task with `maniphest.createtask` owned by the authenticated user.
//!
//! The creation line keeps the PHP console colors (green title, blue URI)
//! on TTYs, gated on the process ANSI setting like `tsprintf()`.
//!
//! No `--check` flag exists at ceb (`ArcanistTodoWorkflow.php` is 121
//! lines; the audit's `--check` subsystem and browse success text
//! describe a newer baseline, T1153 RB-D29 Rule 4); `--browse` stays
//! silent on success like `:115-117`.

use std::process::ExitCode;

use crate::{
    conduit::{self, EnvelopeKind},
    config, get_config, terminal,
};

/// Subcommand name, matching `arc todo`.
pub const NAME: &str = "todo";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistTodoWorkflow.php:12-50`).
const HELP_SYNOPSIS: &str = "      **todo** __summary__ [__options__]";
const HELP_TEXT: &str = "        Quickly create a task for yourself.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "cc",
        Some("C"),
        Some("cc"),
        "Other users to CC on the new task.",
    ),
    (
        "project",
        None,
        Some("project"),
        "Projects to assign to the task.",
    ),
    (
        "browse",
        None,
        None,
        "After creating the task, open it in a web browser.",
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

/// A created task plus its stdout text: the text prints first and the
/// task feeds `--browse` after it (T1153 RB-D29b: PHP echoes the
/// `Created task ...` line, then opens the browser).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedOutcome {
    pub text: String,
    pub task: Option<CreatedTask>,
}

fn render_created_task(
    task: &CreatedTask,
    options: &Options,
    ansi: bool,
) -> Result<String, String> {
    if options.json {
        let mut doc =
            serde_json::to_string_pretty(&created_json(task)).map_err(json_encode_error)?;
        doc.push('\n');
        return Ok(doc);
    }
    Ok(render_created(task, ansi))
}

/// Open the created task in a browser, like PHP's
/// `openURIsInBrowser` after the creation line. Failures are usage
/// errors (PHP throws `ArcanistUsageException`).
pub fn open_created_in_browser(dir: &std::path::Path, task: &CreatedTask) -> Result<(), String> {
    let configured = crate::browser::configured_browser(dir);
    crate::browser::open_uris_in_browser(std::slice::from_ref(&task.uri), configured.as_deref())
}

/// Create the task per `options` in `dir`, returning the stdout text
/// and the created task (none when the summary is blank). Browsing is
/// the caller's job, after printing the text.
pub fn create_in_ansi(
    _dir: &std::path::Path,
    client: &conduit::Client,
    options: &Options,
    ansi: bool,
) -> Result<CreatedOutcome, String> {
    let summary = options.summary.join(" ");
    if is_blank_summary(&summary) {
        return Ok(CreatedOutcome {
            text: "Please provide a summary.\n".to_string(),
            task: None,
        });
    }
    let params = task_create_params(client, options, summary)?;
    let result = client
        .call("maniphest.createtask", &serde_json::Value::Object(params))
        .map_err(conduit_error_text)?;
    let task = parse_created(&result).ok_or_else(missing_task_id)?;
    let text = render_created_task(&task, options, ansi)?;
    Ok(CreatedOutcome {
        text,
        task: Some(task),
    })
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
        Err(_) => std::path::PathBuf::from("."), // LCOV_EXCL_LINE // Reason: current_dir is infallible in practice; deleting the cwd under parallel tests is unsafe
    };
    execute_in_ansi(&dir, client, options, ansi)
}

/// Create the task per `options` with `dir` pinning config discovery.
///
/// Only the stdout text is returned: `--browse` is the caller's job,
/// after printing (see [`create_in_ansi`] + [`open_created_in_browser`]).
pub fn execute_in(
    dir: &std::path::Path,
    client: &conduit::Client,
    options: &Options,
) -> Result<String, String> {
    execute_in_ansi(dir, client, options, terminal::ansi_enabled())
}

/// Create the task per `options` in `dir` with an explicit ANSI setting
/// (deterministic under test). `--browse` is the caller's job, after
/// printing the returned text (T1153 RB-D29b).
pub fn execute_in_ansi(
    dir: &std::path::Path,
    client: &conduit::Client,
    options: &Options,
    ansi: bool,
) -> Result<String, String> {
    create_in_ansi(dir, client, options, ansi).map(|outcome| outcome.text)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    run_in_with_user_file(&cwd, argv, config::arcrc_path().as_deref())
}

/// [`run`] with an explicit working copy and credential file (test
/// seam: hermetic tests stage a fake arcrc instead of touching HOME,
/// and a fixture cwd instead of the ambient one).
fn run_in_with_user_file(
    cwd: &std::path::Path,
    argv: &[String],
    user_file: Option<&std::path::Path>,
) -> ExitCode {
    let options = match parse_args(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        // T1153 RB-D4 (R4): `todo` is a subprocess workflow:
        // usage errors render `Usage Exception:` with no usage dump
        // (`scripts/arcanist.php:432-444`).
        Err(msg) => return conduit::report(EnvelopeKind::Usage, &msg),
    };
    // T1153 CORE-4/T1153 CORE-9: classic `requiresAuthentication` startup
    // (tokenless engine, credential gate, `user.whoami` /
    // `conduit.connect` verification).
    let client = match config::authed_client_for(
        options.conduit_uri.as_deref(),
        cwd,
        user_file,
        &get_config::LoadOptions::default(),
        &config::command_description(),
    ) {
        Ok(client) => client,
        Err(error) => return error.report(),
    };
    match create_in_ansi(cwd, &client, &options, terminal::ansi_enabled()) {
        Ok(outcome) => {
            print!("{}", outcome.text);
            // T1153 RB-D29b: the creation line prints first; `--browse`
            // opens the task after it (usage errors, like PHP's
            // `openURIsInBrowser` throw).
            if options.browse
                && let Some(task) = outcome.task
                && let Err(msg) = open_created_in_browser(cwd, &task)
            {
                return conduit::report(EnvelopeKind::Usage, &msg);
            }
            ExitCode::SUCCESS
        }
        Err(msg) => conduit::report(EnvelopeKind::Exception, &msg),
    }
}

#[cfg(test)]
#[path = "todo_tests.rs"]
mod todo_tests;
