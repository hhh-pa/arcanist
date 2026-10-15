//! `paste` workflow: share and grab text with the Paste application.
//!
//! PHP reference: `src/workflow/ArcanistPasteWorkflow.php`.
//!
//! With a paste ID argument, the paste content is printed (`paste.search`
//! with the `content` attachment). Otherwise a paste is created from
//! `--input` or stdin via `paste.create`, falling back to `paste.edit`
//! transactions on servers without it.
//!
//! When reading, `--lang`, `--title`, `--browse`, and `--input` are usage
//! errors, mirroring the PHP workflow. When creating, `--browse` opens the
//! new paste in a browser (see [`crate::browser`]).
//!
//! The creation line carries no console markup: PHP reports creates with
//! a log-engine line plus a ref view (both styled), while this port keeps
//! its own one-line summary, so both ANSI modes share the exact bytes.

use std::path::Path;
use std::process::ExitCode;

pub const NAME: &str = "paste";

const DEFAULT_TITLE: &str = "Command-Line Input";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--title TITLE] [--lang LANG] [--input PATH] [--browse]\n\
         \x20      arc-rs {NAME} P123\n\
         \n\
         Share and grab text using the Paste application. To create a paste,\n\
         use --input or provide the text on stdin; to retrieve one, give\n\
         its paste ID.\n\
         \n\
         Options:\n\
         \x20 --title TITLE  title for the paste\n\
         \x20 --lang LANG    language for the paste\n\
         \x20 --input PATH   create a paste from a file ('-' reads stdin)\n\
         \x20 --browse       after creating a paste, open it in a web browser\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI / .arcconfig)\n\
         \x20 -h, --help     show this help\n"
    )
}

/// A paste symbol: numeric ID(s) or PHID(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteSymbol {
    Ids(Vec<u64>),
    Phids(Vec<String>),
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    title: Option<String>,
    lang: Option<String>,
    input: Option<String>,
    browse: bool,
    symbol: Option<String>,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

fn take_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    match iter.next() {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        _ => Err(format!("{flag} requires a value")),
    }
}

/// One argument's parse step: consumed, or a terminal outcome.
enum Step {
    Handled,
    Done(ParseOutcome),
}

struct Parser {
    title: Option<String>,
    lang: Option<String>,
    input: Option<String>,
    browse: bool,
    symbol: Option<String>,
    conduit_uri: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            title: None,
            lang: None,
            input: None,
            browse: false,
            symbol: None,
            conduit_uri: None,
            end_of_flags: false,
        }
    }

    fn flag_title(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--title" {
            match take_value(iter, "--title") {
                Ok(value) => self.title = Some(value),
                Err(message) => return Some(Step::Done(ParseOutcome::Error(message))),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--title=") {
            self.title = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn flag_lang(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--lang" {
            match take_value(iter, "--lang") {
                Ok(value) => self.lang = Some(value),
                Err(message) => return Some(Step::Done(ParseOutcome::Error(message))),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--lang=") {
            self.lang = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn flag_input(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--input" {
            match take_value(iter, "--input") {
                Ok(value) => self.input = Some(value),
                Err(message) => return Some(Step::Done(ParseOutcome::Error(message))),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--input=") {
            self.input = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--conduit-uri" {
            match take_value(iter, "--conduit-uri") {
                Ok(value) => self.conduit_uri = Some(value),
                Err(message) => return Some(Step::Done(ParseOutcome::Error(message))),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            self.conduit_uri = Some(value.to_string());
            return Some(Step::Handled);
        }
        None
    }

    fn symbol(&mut self, arg: &str) -> Step {
        if self.symbol.is_some() {
            Step::Done(ParseOutcome::Error(
                "Specify only one paste to retrieve.".to_string(),
            ))
        } else {
            self.symbol = Some(arg.to_string());
            Step::Handled
        }
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
        if arg == "-h" || arg == "--help" {
            return Step::Done(ParseOutcome::Help);
        }
        if arg.starts_with('-') {
            return Step::Done(ParseOutcome::Error(format!("unknown flag '{arg}'")));
        }
        self.symbol(arg)
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            return self.symbol(arg);
        }
        if let Some(step) = self.flag_title(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_lang(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_input(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        self.tail(arg)
    }

    fn finish(self) -> ParseOutcome {
        if self.symbol.is_some() {
            if self.lang.is_some() {
                return ParseOutcome::Error(
                    "Flag \"--lang\" is not supported when reading pastes.".to_string(),
                );
            }
            if self.title.is_some() {
                return ParseOutcome::Error(
                    "Flag \"--title\" is not supported when reading pastes.".to_string(),
                );
            }
            if self.browse {
                return ParseOutcome::Error(
                    "Flag \"--browse\" is not supported when reading pastes. Use \"arc browse\" to browse known objects."
                        .to_string(),
                );
            }
            if self.input.is_some() {
                return ParseOutcome::Error(
                    "Flag \"--input\" is not supported when reading pastes.".to_string(),
                );
            }
        }
        ParseOutcome::Args(Options {
            title: self.title,
            lang: self.lang,
            input: self.input,
            browse: self.browse,
            symbol: self.symbol,
            conduit_uri: self.conduit_uri,
        })
    }
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
    parser.finish()
}

/// `P123`, `123`, or a paste PHID to search constraints.
pub fn parse_paste_symbol(text: &str) -> Result<PasteSymbol, String> {
    let trimmed = text.trim();
    if trimmed.starts_with("PHID-") {
        return Ok(PasteSymbol::Phids(vec![trimmed.to_string()]));
    }
    let digits = trimmed
        .strip_prefix('P')
        .or_else(|| trimmed.strip_prefix('p'))
        .unwrap_or(trimmed);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return digits.parse::<u64>().map_or_else(
            |_| Err(format!("Specify paste by ID, e.g. P123 (got '{text}').")),
            |id| Ok(PasteSymbol::Ids(vec![id])),
        );
    }
    Err(format!("Specify paste by ID, e.g. P123 (got '{text}')."))
}

/// Server root of a conduit URI (`https://host[/api...]` -> `https://host`).
pub fn conduit_root(conduit_uri: &str) -> String {
    let trimmed = conduit_uri.trim_end_matches('/');
    trimmed.strip_suffix("/api").unwrap_or(trimmed).to_string()
}

/// Resolve conduit URI precedence, mirroring `config::client_for`.
pub fn resolve_conduit_uri(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(crate::config::discover_conduit_uri)
}

/// Make a possibly-relative display URI absolute against the conduit root.
pub fn absolute_uri(conduit_uri: Option<&str>, uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return uri.to_string();
    }
    match conduit_uri {
        Some(base) => format!("{}{}", conduit_root(base), uri),
        None => uri.to_string(),
    }
}

fn search_constraints(symbol: &PasteSymbol) -> serde_json::Value {
    match symbol {
        PasteSymbol::Ids(ids) => serde_json::json!({"ids": ids}),
        PasteSymbol::Phids(phids) => serde_json::json!({"phids": phids}),
    }
}

/// Print-ready paste content for a symbol via `paste.search`.
pub fn read_paste(client: &crate::conduit::Client, symbol_text: &str) -> Result<String, String> {
    let symbol = parse_paste_symbol(symbol_text)?;
    let result = client
        .call(
            "paste.search",
            &serde_json::json!({
                "constraints": search_constraints(&symbol),
                "attachments": {"content": true},
            }),
        )
        .map_err(|e| e.to_string())?;
    let hit = result
        .get("data")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .ok_or_else(|| {
            format!("Paste \"{symbol_text}\" does not exist, or you do not have access to see it.")
        })?;
    hit.get("attachments")
        .and_then(|a| a.get("content"))
        .and_then(|c| c.get("content"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("Paste \"{symbol_text}\" has no content."))
}

/// A created paste's identity, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedPaste {
    pub id: u64,
    pub uri: String,
}

fn created_id_of(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse::<u64>().ok(),
        serde_json::Value::Object(map) => map
            .get("id")
            .and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_u64(),
                serde_json::Value::String(s) => s.parse::<u64>().ok(),
                _ => None,
            })
            .or_else(|| {
                map.get("object")
                    .and_then(|o| o.get("id"))
                    .and_then(|v| match v {
                        serde_json::Value::Number(n) => n.as_u64(),
                        serde_json::Value::String(s) => s.parse::<u64>().ok(),
                        _ => None,
                    })
            }),
        _ => None,
    }
}

fn created_phid_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map
            .get("phid")
            .or_else(|| map.get("object").and_then(|o| o.get("phid")))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

fn created_uri_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map
            .get("uri")
            .or_else(|| map.get("object").and_then(|o| o.get("uri")))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

fn fetch_paste_uri(
    client: &crate::conduit::Client,
    symbol: &PasteSymbol,
    conduit_uri: Option<&str>,
    fallback_id: u64,
) -> String {
    let fallback = absolute_uri(conduit_uri, &format!("/P{fallback_id}"));
    let Ok(result) = client.call(
        "paste.search",
        &serde_json::json!({"constraints": search_constraints(symbol)}),
    ) else {
        return fallback;
    };
    result
        .get("data")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|hit| hit.get("fields"))
        .and_then(|f| f.get("uri"))
        .and_then(|v| v.as_str())
        .map(|uri| absolute_uri(conduit_uri, uri))
        .unwrap_or(fallback)
}

fn create_params(title: &str, lang: Option<&str>, content: &str) -> serde_json::Value {
    let mut params = serde_json::json!({
        "content": content,
        "title": title,
    });
    if let Some(lang) = lang {
        params["language"] = serde_json::json!(lang);
    }
    params
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn created_from_search_hit(hit: &serde_json::Value, conduit_uri: Option<&str>) -> CreatedPaste {
    let id = hit
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let uri = hit
        .get("fields")
        .and_then(|f| f.get("uri"))
        .and_then(|v| v.as_str())
        .map_or_else(|| format!("/P{id}"), str::to_string);
    CreatedPaste {
        id,
        uri: absolute_uri(conduit_uri, &uri),
    }
}

/// Resolve a freshly created paste known only by PHID via `paste.search`.
fn created_from_phid(
    client: &crate::conduit::Client,
    phid: &str,
    conduit_uri: Option<&str>,
    verb: &str,
) -> Result<CreatedPaste, String> {
    let symbol = PasteSymbol::Phids(vec![phid.to_string()]);
    let search = client
        .call(
            "paste.search",
            &serde_json::json!({"constraints": search_constraints(&symbol)}),
        )
        .map_err(|e| e.to_string())?;
    let hit = search
        .get("data")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .ok_or_else(|| format!("{verb} succeeded but the new paste could not be found"))?;
    Ok(created_from_search_hit(hit, conduit_uri))
}

fn created_from_create_result(
    client: &crate::conduit::Client,
    result: &serde_json::Value,
    conduit_uri: Option<&str>,
) -> Result<CreatedPaste, String> {
    if let Some(id) = created_id_of(result) {
        let uri = created_uri_of(result).map_or_else(
            || fetch_paste_uri(client, &PasteSymbol::Ids(vec![id]), conduit_uri, id),
            |uri| absolute_uri(conduit_uri, &uri),
        );
        return Ok(CreatedPaste { id, uri });
    }
    if let Some(phid) = created_phid_of(result) {
        return created_from_phid(client, &phid, conduit_uri, "paste.create");
    }
    Err("paste.create returned an unexpected response".to_string())
}

fn create_via_edit(
    client: &crate::conduit::Client,
    title: &str,
    lang: Option<&str>,
    content: &str,
    conduit_uri: Option<&str>,
) -> Result<CreatedPaste, String> {
    let mut transactions = vec![serde_json::json!({"type": "title", "value": title})];
    if let Some(lang) = lang {
        transactions.push(serde_json::json!({"type": "language", "value": lang}));
    }
    transactions.push(serde_json::json!({"type": "text", "value": content}));
    let result = client
        .call(
            "paste.edit",
            &serde_json::json!({"transactions": transactions}),
        )
        .map_err(|e| e.to_string())?;
    let object = result
        .get("object")
        .ok_or_else(|| "paste.edit returned an unexpected response".to_string())?;
    let id = object.get("id").and_then(json_u64);
    let phid = object
        .get("phid")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match (id, phid) {
        (Some(id), _) => Ok(CreatedPaste {
            uri: fetch_paste_uri(client, &PasteSymbol::Ids(vec![id]), conduit_uri, id),
            id,
        }),
        (None, Some(phid)) => created_from_phid(client, &phid, conduit_uri, "paste.edit"),
        (None, None) => Err("paste.edit returned an unexpected response".to_string()),
    }
}

/// Create a paste via `paste.create`, falling back to `paste.edit`.
pub fn create_paste(
    client: &crate::conduit::Client,
    title: &str,
    lang: Option<&str>,
    content: &str,
    conduit_uri: Option<&str>,
) -> Result<CreatedPaste, String> {
    let params = create_params(title, lang, content);
    if let Ok(result) = client.call("paste.create", &params) {
        return created_from_create_result(client, &result, conduit_uri);
    }
    // Older or newer servers may only offer `paste.edit` transactions.
    create_via_edit(client, title, lang, content, conduit_uri)
}

fn read_stdin_text() -> Result<String, String> {
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    Ok(text)
}

fn read_paste_output(client: &crate::conduit::Client, symbol: &str) -> Result<String, String> {
    let content = read_paste(client, symbol)?;
    if content.ends_with('\n') || content.is_empty() {
        return Ok(content);
    }
    Ok(format!("{content}\n"))
}

fn create_paste_output(
    dir: &Path,
    client: &crate::conduit::Client,
    opts: &Options,
    stdin_text: Option<&str>,
) -> Result<String, String> {
    let content = match opts.input.as_deref() {
        None | Some("-") => match stdin_text {
            Some(text) => text.to_string(),
            None => read_stdin_text()?,
        },
        Some(path) => std::fs::read_to_string(dir.join(path))
            .map_err(|e| format!("cannot read \"{path}\": {e}"))?,
    };
    if content.is_empty() {
        return Err("no paste content provided (stdin was empty)".to_string());
    }
    let title = opts.title.as_deref().unwrap_or(DEFAULT_TITLE);
    let base = resolve_conduit_uri(opts.conduit_uri.as_deref());
    let created = create_paste(
        client,
        title,
        opts.lang.as_deref(),
        &content,
        base.as_deref(),
    )?;
    if opts.browse {
        let configured = crate::browser::configured_browser(dir);
        crate::browser::open_uris_in_browser(
            std::slice::from_ref(&created.uri),
            configured.as_deref(),
        )?;
    }
    Ok(format_created(&created))
}

/// Run the workflow; `stdin_text` overrides stdin when creating from stdin
/// (`None` reads real stdin).
pub fn execute_with_stdin(
    dir: &Path,
    argv: &[String],
    stdin_text: Option<&str>,
) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;

    if let Some(symbol) = opts.symbol {
        return read_paste_output(&client, &symbol);
    }
    create_paste_output(dir, &client, &opts, stdin_text)
}

/// One-line summary of a created paste.
#[must_use]
pub fn format_created(created: &CreatedPaste) -> String {
    format!(
        "Created paste P{id}: {uri}\n",
        id = created.id,
        uri = created.uri
    )
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    execute_with_stdin(dir, argv, None)
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
#[path = "paste_tests.rs"]
mod paste_tests;
