//! `export` workflow: print local or Differential changes as a patch.
//!
//! PHP reference: `src/workflow/ArcanistExportWorkflow.php`.
//!
//! Local sources run `git diff` against a base commit (default `HEAD`) and
//! print the result: `--git` prints a full git diff, `--unified` a plain
//! unified diff. `--revision`/`--diff` download the raw diff over Conduit
//! (`differential.querydiffs`, then `differential.getrawdiff`).
//!
//! Out of scope (rejected with a not-yet-ported message): `--arcbundle` and
//! `--encoding`.

use std::path::Path;
use std::process::ExitCode;

pub const NAME: &str = "export";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} (--git | --unified) [BASE]\n\
         \x20      arc-rs {NAME} (--git | --unified) --revision ID\n\
         \x20      arc-rs {NAME} (--git | --unified) --diff ID\n\
         \n\
         Export the working copy changes against BASE (default: HEAD), or a\n\
         Differential revision/diff, to stdout as a patch.\n\
         \n\
         Options:\n\
         \x20 --git          export as a git patch (applies with `git apply`)\n\
         \x20 --unified      export as a plain unified patch (applies with `patch`)\n\
         \x20 --revision ID  export a Differential revision (D123 or 123)\n\
         \x20 --diff ID      export one Differential diff by numeric ID\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI / .arcconfig)\n\
         \x20 -h, --help     show this help\n\
         \n\
         Not yet ported (see T1128): --arcbundle, --encoding.\n"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Git,
    Unified,
}

/// Where the exported change comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local { base: Option<String> },
    Revision(u64),
    Diff(u64),
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    format: Format,
    source: Source,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    NotYetPorted(String),
    Error(String),
}

fn not_yet_ported(flag: &str) -> ParseOutcome {
    ParseOutcome::NotYetPorted(format!(
        "arc-rs {NAME}: {flag} is not yet ported (see T1128)"
    ))
}

/// `D12345` or `12345` to a numeric revision ID.
pub fn normalize_revision_id(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    let digits = trimmed
        .strip_prefix('D')
        .or_else(|| trimmed.strip_prefix('d'))
        .unwrap_or(trimmed);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits
            .parse::<u64>()
            .map_err(|_| format!("invalid revision '{text}': expected D12345 or a revision ID"))
    } else {
        Err(format!(
            "invalid revision '{text}': expected D12345 or a revision ID"
        ))
    }
}

fn parse_diff_id(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        trimmed
            .parse::<u64>()
            .map_err(|_| format!("invalid diff ID '{text}': expected a numeric diff ID"))
    } else {
        Err(format!(
            "invalid diff ID '{text}': expected a numeric diff ID"
        ))
    }
}

fn take_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, ParseOutcome> {
    match iter.next() {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        _ => Err(ParseOutcome::Error(format!("{flag} requires a value"))),
    }
}

/// One argument's parse step: consumed, or a terminal outcome.
enum Step {
    Handled,
    Done(ParseOutcome),
}

struct Parser {
    git: bool,
    unified: bool,
    revision: Option<String>,
    diff: Option<String>,
    positional: Option<String>,
    conduit_uri: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            git: false,
            unified: false,
            revision: None,
            diff: None,
            positional: None,
            conduit_uri: None,
            end_of_flags: false,
        }
    }

    fn flag_revision(
        &mut self,
        arg: &str,
        iter: &mut std::slice::Iter<'_, String>,
    ) -> Option<Step> {
        if arg == "--revision" {
            match take_value(iter, "--revision") {
                Ok(value) => self.revision = Some(value),
                Err(outcome) => return Some(Step::Done(outcome)),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--revision=") {
            self.revision = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn flag_diff(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--diff" {
            match take_value(iter, "--diff") {
                Ok(value) => self.diff = Some(value),
                Err(outcome) => return Some(Step::Done(outcome)),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--diff=") {
            self.diff = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--conduit-uri" {
            match take_value(iter, "--conduit-uri") {
                Ok(value) => self.conduit_uri = Some(value),
                Err(outcome) => return Some(Step::Done(outcome)),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            self.conduit_uri = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn positional(&mut self, arg: &str) -> Step {
        if self.positional.is_some() {
            Step::Done(ParseOutcome::Error("unexpected extra argument".to_string()))
        } else {
            self.positional = Some(arg.to_string());
            Step::Handled
        }
    }

    fn tail(&mut self, arg: &str) -> Step {
        if arg == "--" {
            self.end_of_flags = true;
            return Step::Handled;
        }
        if arg == "--git" {
            self.git = true;
            return Step::Handled;
        }
        if arg == "--unified" {
            self.unified = true;
            return Step::Handled;
        }
        if arg == "-h" || arg == "--help" {
            return Step::Done(ParseOutcome::Help);
        }
        if arg == "--arcbundle" || arg.starts_with("--arcbundle=") {
            return Step::Done(not_yet_ported("--arcbundle"));
        }
        if arg == "--encoding" || arg.starts_with("--encoding=") {
            return Step::Done(not_yet_ported("--encoding"));
        }
        if arg.starts_with('-') {
            return Step::Done(ParseOutcome::Error(format!("unknown flag '{arg}'")));
        }
        self.positional(arg)
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            return self.positional(arg);
        }
        if let Some(step) = self.flag_revision(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_diff(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        self.tail(arg)
    }
}

fn resolve_format(git: bool, unified: bool) -> Result<Format, ParseOutcome> {
    match (git, unified) {
        (true, false) => Ok(Format::Git),
        (false, true) => Ok(Format::Unified),
        (false, false) => Err(ParseOutcome::Error(
            "Specify one of '--git' or '--unified' to choose an export format.".to_string(),
        )),
        (true, true) => Err(ParseOutcome::Error(
            "Options '--git' and '--unified' are not compatible. Choose exactly one export format."
                .to_string(),
        )),
    }
}

fn resolve_source(
    revision: Option<String>,
    diff: Option<String>,
    positional: Option<String>,
) -> Result<Source, ParseOutcome> {
    if let Some(value) = revision {
        if positional.is_some() {
            return Err(ParseOutcome::Error(
                "unexpected argument: --revision takes no base commit".to_string(),
            ));
        }
        return match normalize_revision_id(&value) {
            Ok(id) => Ok(Source::Revision(id)),
            Err(message) => Err(ParseOutcome::Error(message)),
        };
    }
    if let Some(value) = diff {
        if positional.is_some() {
            return Err(ParseOutcome::Error(
                "unexpected argument: --diff takes no base commit".to_string(),
            ));
        }
        return match parse_diff_id(&value) {
            Ok(id) => Ok(Source::Diff(id)),
            Err(message) => Err(ParseOutcome::Error(message)),
        };
    }
    Ok(Source::Local { base: positional })
}

fn finish(parser: Parser) -> ParseOutcome {
    let format = match resolve_format(parser.git, parser.unified) {
        Ok(format) => format,
        Err(outcome) => return outcome,
    };
    if parser.revision.is_some() && parser.diff.is_some() {
        return ParseOutcome::Error(
            "Options '--revision' and '--diff' are not compatible. Choose exactly one change source."
                .to_string(),
        );
    }
    let source = match resolve_source(parser.revision, parser.diff, parser.positional) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    ParseOutcome::Args(Options {
        format,
        source,
        conduit_uri: parser.conduit_uri,
    })
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut parser = Parser::new();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        match parser.consume(arg, &mut iter) {
            Step::Handled => {}
            Step::Done(outcome) => return outcome,
        }
    }
    finish(parser)
}

/// First diff ID in a `differential.querydiffs` result (a dict keyed by diff
/// ID, or a list).
fn first_diff_id(result: &serde_json::Value) -> Result<String, String> {
    let missing = || {
        "The diff or revision you specified is either invalid or you don't \
         have permission to view it."
            .to_string()
    };
    let diff = match result {
        serde_json::Value::Object(map) => map.values().next().ok_or_else(missing)?,
        serde_json::Value::Array(items) => items.first().ok_or_else(missing)?,
        _ => return Err(missing()),
    };
    match diff.get("id") {
        Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        _ => Err(missing()),
    }
}

fn raw_diff_text(result: &serde_json::Value) -> Option<String> {
    if let Some(text) = result.as_str() {
        return Some(text.to_string());
    }
    result
        .get("rawDiff")
        .or_else(|| result.get("diff"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Download one raw diff by numeric diff ID.
pub fn fetch_raw_diff_for_diff(
    client: &crate::conduit::Client,
    diff_id: &str,
) -> Result<String, String> {
    let result = client
        .call(
            "differential.getrawdiff",
            &serde_json::json!({"diffID": diff_id}),
        )
        .map_err(|e| e.to_string())?;
    raw_diff_text(&result)
        .ok_or_else(|| "differential.getrawdiff returned an unexpected response".to_string())
}

/// Download the most recent raw diff attached to a revision.
pub fn fetch_raw_diff_for_revision(
    client: &crate::conduit::Client,
    revision: u64,
) -> Result<String, String> {
    let result = client
        .call(
            "differential.querydiffs",
            &serde_json::json!({"revisionIDs": [revision]}),
        )
        .map_err(|e| e.to_string())?;
    let diff_id = first_diff_id(&result)?;
    fetch_raw_diff_for_diff(client, &diff_id)
}

/// Local `git diff` of the working tree against `base` in `format`.
pub fn local_diff(dir: &Path, format: Format, base: &str) -> Result<String, String> {
    if !crate::git::is_inside_work_tree(dir) {
        return Err("not inside a git working copy".to_string());
    }
    let args: &[&str] = match format {
        Format::Git => &["diff", "--no-color", base, "--"],
        Format::Unified => &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-prefix",
            base,
            "--",
        ],
    };
    let out = crate::git::output(dir, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let trimmed = detail.trim();
        if trimmed.is_empty() {
            Err("git diff failed".to_string())
        } else {
            Err(trimmed.to_string())
        }
    }
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::NotYetPorted(message) => return Err(message),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    match opts.source {
        Source::Local { base } => {
            let base = base.as_deref().unwrap_or("HEAD");
            local_diff(dir, opts.format, base)
        }
        Source::Revision(id) => {
            let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
            fetch_raw_diff_for_revision(&client, id).map(ensure_trailing_newline)
        }
        Source::Diff(id) => {
            let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
            fetch_raw_diff_for_diff(&client, &id.to_string()).map(ensure_trailing_newline)
        }
    }
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod export_tests;
