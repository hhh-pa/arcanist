//! `paste` workflow: share and grab text with the Paste application.
//!
//! PHP reference: `src/workflow/ArcanistPasteWorkflow.php`.
//!
//! With a paste ID argument, the paste content is printed (`paste.search`
//! with the `content` attachment). Otherwise a paste is created from
//! `--input` or stdin via `paste.edit` transactions, like PHP
//! (`paste.create` is frozen server-side).
//!
//! When reading, `--lang`, `--title`, `--browse`, and `--input` are usage
//! errors, mirroring the PHP workflow. When creating, `--browse` opens the
//! new paste in a browser (see [`crate::browser`]), after printing.
//!
//! Creates report through the log engine (`DONE` on stderr) plus a
//! [`crate::terminal::RefView`] on stdout, like PHP.

use std::path::Path;
use std::process::ExitCode;

pub const NAME: &str = "paste";

const DEFAULT_TITLE: &str = "Command-Line Input";

fn usage() -> String {
    format!(
        "usage: arc {NAME} [--title TITLE] [--lang LANG] [--input PATH] [--browse]\n\
         \x20      arc {NAME} P123\n\
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

/// The `ArcanistSimpleSymbolRef::resolveSymbol()` rejection
/// (`src/ref/simple/ArcanistSimpleSymbolRef.php:46-51`).
fn unrecognized_symbol(text: &str) -> String {
    format!(
        "The format of symbol \"{text}\" is unrecognized. Expected a monogram like \
         \"X123\", or an ID like \"123\", or a PHID."
    )
}

/// `P123`, `123`, or a paste PHID to search constraints, mirroring
/// `ArcanistSimpleSymbolRef::resolveSymbol()` with the paste prefix
/// (`[Pp]?`) and PHID type (`PSTE`):
///
/// - no whitespace trimming (T1153 PASTE-P6);
/// - IDs are `[1-9][0-9]*`: `P0`, `0`, and leading zeros are rejected
///   (T1153 PASTE-P7);
/// - overflowing digits saturate to `PHP_INT_MAX` and search numerically
///   (T1153 PASTE-P8);
/// - only `PHID-PSTE-` PHIDs search; anything else is unrecognized
///   (T1153 PASTE-P9), with PHP's exact text (T1153 PASTE-P10).
pub fn parse_paste_symbol(text: &str) -> Result<PasteSymbol, String> {
    if let Some(rest) = text.strip_prefix("PHID-PSTE-") {
        if !rest.is_empty() && rest.bytes().all(|b| !b.is_ascii_whitespace()) {
            return Ok(PasteSymbol::Phids(vec![text.to_string()]));
        }
        return Err(unrecognized_symbol(text));
    }
    if text.starts_with("PHID-") {
        return Err(unrecognized_symbol(text));
    }
    let digits = text
        .strip_prefix('P')
        .or_else(|| text.strip_prefix('p'))
        .unwrap_or(text);
    let mut bytes = digits.bytes();
    match bytes.next() {
        Some(b'1'..=b'9') => {}
        _ => return Err(unrecognized_symbol(text)),
    }
    if !bytes.all(|b| b.is_ascii_digit()) {
        return Err(unrecognized_symbol(text));
    }
    // `(int)` saturates past `PHP_INT_MAX` (64-bit).
    let id = digits
        .parse::<u64>()
        .unwrap_or(u64::MAX)
        .min(i64::MAX as u64);
    Ok(PasteSymbol::Ids(vec![id]))
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

/// True when `uri` already carries a domain: absolute (`scheme://`) or
/// protocol-relative (`//host/...`), mirroring `PhutilURI::getDomain()`.
fn uri_has_domain(uri: &str) -> bool {
    if uri.starts_with("//") {
        return true;
    }
    let Some(scheme) = uri.split_once("://").map(|(scheme, _)| scheme) else {
        return false;
    };
    let mut bytes = scheme.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

/// Make a possibly-relative display URI absolute against the conduit root
/// (`ArcanistWorkflow::getAbsoluteURI()`): absolute URIs pass through
/// untouched, while relative ones join the conduit root by path only —
/// any query or fragment is dropped (T1153 PASTE-P17), like PHP's
/// `setPath($raw_uri->getPath())`.
pub fn absolute_uri(conduit_uri: Option<&str>, uri: &str) -> String {
    if uri_has_domain(uri) {
        return uri.to_string();
    }
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    match conduit_uri {
        Some(base) => format!("{}{}", conduit_root(base), path),
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
    read_paste_classified(client, symbol_text).map_err(|err| err.message)
}

/// A created paste's identity, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedPaste {
    pub id: u64,
    pub uri: String,
}

/// A created paste's identity plus its searched title, for the RefView.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedPasteDetail {
    pub id: u64,
    pub uri: String,
    pub title: Option<String>,
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn detail_from_search_hit(
    hit: &serde_json::Value,
    conduit_uri: Option<&str>,
) -> CreatedPasteDetail {
    // `ArcanistPasteRef`: monogram from `id`, title + URI from `fields`,
    // URI falling back to `/P{id}`.
    let id = hit.get("id").and_then(json_u64).unwrap_or(0);
    let fields = hit.get("fields");
    let uri = fields
        .and_then(|f| f.get("uri"))
        .and_then(|v| v.as_str())
        .map_or_else(|| format!("/P{id}"), str::to_string);
    let title = fields
        .and_then(|f| f.get("title"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    CreatedPasteDetail {
        id,
        uri: absolute_uri(conduit_uri, &uri),
        title,
    }
}

/// Resolve a freshly created paste by PHID via `paste.search`
/// (`ArcanistPasteWorkflow.php:147-148`). PHP then calls `getURI()` on the
/// result unconditionally; a missing hit is a fatal crash there, so a clean
/// error stays here.
fn detail_from_phid(
    client: &crate::conduit::Client,
    phid: &str,
    conduit_uri: Option<&str>,
) -> Result<CreatedPasteDetail, String> {
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
        .ok_or_else(|| "paste.edit succeeded but the new paste could not be found".to_string())?;
    Ok(detail_from_search_hit(hit, conduit_uri))
}

/// Create a paste via `paste.edit` transactions ONLY
/// (`ArcanistPasteWorkflow.php:137-145`; T1153 PASTE-P1 — `paste.create` is
/// frozen server-side: "New code should use paste.edit"). The new paste is
/// always re-searched by PHID (T1153 PASTE-P16), like PHP's
/// `loadPasteForSymbol($paste_phid)`.
pub fn create_paste_detail(
    client: &crate::conduit::Client,
    title: &str,
    lang: Option<&str>,
    content: &str,
    conduit_uri: Option<&str>,
) -> Result<CreatedPasteDetail, String> {
    let mut transactions = vec![serde_json::json!({"type": "title", "value": title})];
    if let Some(lang) = lang {
        transactions.push(serde_json::json!({"type": "language", "value": lang}));
    }
    transactions.push(serde_json::json!({"type": "text", "value": content}));
    edit_and_resolve(
        client,
        &serde_json::json!({"transactions": transactions}),
        conduit_uri,
    )
}

/// Create a paste via `paste.edit` transactions (see [`create_paste_detail`]).
pub fn create_paste(
    client: &crate::conduit::Client,
    title: &str,
    lang: Option<&str>,
    content: &str,
    conduit_uri: Option<&str>,
) -> Result<CreatedPaste, String> {
    create_paste_detail(client, title, lang, content, conduit_uri).map(|detail| CreatedPaste {
        id: detail.id,
        uri: detail.uri,
    })
}

/// `ArcanistWorkflow::readStdin()`: the `INPUT` hint on interactive stdin
/// (T1153 PASTE-P15), then all input bytes.
fn read_stdin_bytes() -> Result<Vec<u8>, String> {
    use std::io::Read;
    if crate::prompt::stdin_is_terminal() {
        eprint!(
            "{}",
            crate::terminal::format_log(
                "blue",
                "INPUT",
                "Waiting for input on stdin...",
                crate::terminal::ansi_enabled()
            )
        );
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    Ok(bytes)
}

/// Resolve `--input` for messages, mirroring `Filesystem::resolvePath()`:
/// absolute against `dir`, symlink parents resolved, missing paths resolved
/// through the nearest existing ancestor.
fn resolve_input_path(dir: &Path, path: &str) -> String {
    let joined = dir.join(path);
    if joined.is_symlink() {
        if let (Some(parent), Some(name)) = (joined.parent(), joined.file_name())
            && let Ok(canonical) = std::fs::canonicalize(parent)
        {
            return canonical.join(name).to_string_lossy().to_string();
        }
        return joined.to_string_lossy().to_string();
    }
    if let Ok(canonical) = std::fs::canonicalize(&joined) {
        return canonical.to_string_lossy().to_string();
    }
    // Missing: resolve through the nearest existing ancestor, then re-append
    // the remainder lexically.
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    let mut current: &Path = &joined;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            remainder.push(name.to_os_string());
        }
        if let Ok(canonical) = std::fs::canonicalize(parent) {
            let mut out = canonical;
            for part in remainder.iter().rev() {
                out.push(part);
            }
            return out.to_string_lossy().to_string();
        }
        current = parent;
    }
    joined.to_string_lossy().to_string()
}

/// Read `--input` bytes, mirroring `Filesystem::readFile()` (T1153 PASTE-P13):
/// existence (with the Unix parent-without-`+x` variant), is-file, and
/// readability checks against the resolved path.
fn read_input_file(dir: &Path, path: &str) -> Result<Vec<u8>, String> {
    let disk = dir.join(path);
    let display = resolve_input_path(dir, path);
    if std::fs::symlink_metadata(&disk).is_err() {
        #[cfg(unix)]
        for parent in disk.ancestors().skip(1) {
            use std::os::unix::fs::PermissionsExt;
            let blocked = std::fs::symlink_metadata(parent)
                .map(|meta| meta.is_dir() && meta.permissions().mode() & 0o111 == 0)
                .unwrap_or(false);
            if blocked {
                return Err(format!(
                    "Filesystem path \"{display}\" can not be accessed because a parent \
                     directory (\"{}\") is not executable (the current process does not have \
                     \"+x\" permission).",
                    parent.to_string_lossy()
                ));
            }
        }
        return Err(format!("Filesystem path \"{display}\" does not exist."));
    }
    if !std::fs::metadata(&disk).is_ok_and(|meta| meta.is_file()) {
        return Err(format!("Requested path '{display}' is not a file."));
    }
    match std::fs::read(&disk) {
        Ok(data) => Ok(data),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(format!("Path '{display}' is not readable."))
        }
        Err(_) => Err(format!("Failed to read file '{display}'.")),
    }
}

/// Read create content as bytes: `--input` file or stdin
/// (`stdin_text` overrides real stdin).
fn read_create_bytes(
    dir: &Path,
    opts: &Options,
    stdin_text: Option<&str>,
) -> Result<Vec<u8>, crate::conduit::ClassifiedError> {
    match opts.input.as_deref() {
        None | Some("-") => match stdin_text {
            Some(text) => Ok(text.as_bytes().to_vec()),
            None => read_stdin_bytes().map_err(crate::conduit::ClassifiedError::fatal),
        },
        Some(path) => read_input_file(dir, path).map_err(crate::conduit::ClassifiedError::fatal),
    }
}

/// `paste.edit` with `params`, then the mandatory re-search by PHID.
fn edit_and_resolve(
    client: &crate::conduit::Client,
    params: &serde_json::Value,
    conduit_uri: Option<&str>,
) -> Result<CreatedPasteDetail, String> {
    let result = client
        .call("paste.edit", params)
        .map_err(|e| e.to_string())?;
    let phid = result
        .get("object")
        .and_then(|o| o.get("phid"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "paste.edit returned an unexpected response".to_string())?;
    detail_from_phid(client, phid, conduit_uri)
}

/// `writeSuccess('DONE', 'Created a new paste.')` on stderr (T1153 PASTE-P2).
fn log_paste_created() {
    eprint!(
        "{}",
        crate::terminal::format_log(
            "green",
            "DONE",
            "Created a new paste.",
            crate::terminal::ansi_enabled()
        )
    );
}

/// Render a created paste with an explicit console width and ANSI setting:
/// `ArcanistPasteRef::buildRefView()` (monogram + title) plus the URI
/// (T1153 PASTE-P2).
pub fn format_created_detail(created: &CreatedPasteDetail, width: usize, ansi: bool) -> String {
    // `ArcanistPasteRef::buildRefView()`: monogram + title.
    let view = crate::terminal::RefView {
        object_name: Some(format!("P{}", created.id)),
        title: created.title.clone(),
        uri: Some(created.uri.clone()),
        ..Default::default()
    };
    view.render(0, width, ansi)
}

/// Create output plus an optional browse URI, without browsing: lets `run_in`
/// print before browsing, like PHP (T1153 PASTE-P3).
#[derive(Debug)]
struct PasteParts {
    text: String,
    browse_uri: Option<String>,
}

fn create_paste_parts(
    dir: &Path,
    client: &crate::conduit::Client,
    opts: &Options,
    stdin_text: Option<&str>,
) -> Result<PasteParts, crate::conduit::ClassifiedError> {
    use crate::conduit::ClassifiedError;
    let bytes = read_create_bytes(dir, opts, stdin_text)?;
    let title = opts.title.as_deref().unwrap_or(DEFAULT_TITLE);
    let base = resolve_conduit_uri(opts.conduit_uri.as_deref());
    // T1153 PASTE-P5: empty content is sent (`text:''`); the server decides.
    // T1153 PASTE-P14: non-UTF-8 input fails PHP's `json_encode()`, so the
    // server sees an empty request; send empty params and surface its verdict.
    let detail = match String::from_utf8(bytes) {
        Ok(content) => create_paste_detail(
            client,
            title,
            opts.lang.as_deref(),
            &content,
            base.as_deref(),
        )
        .map_err(ClassifiedError::fatal)?,
        Err(_) => edit_and_resolve(client, &serde_json::json!({}), base.as_deref())
            .map_err(ClassifiedError::fatal)?,
    };
    log_paste_created();
    let text = format_created_detail(
        &detail,
        crate::terminal::terminal_width(),
        crate::terminal::ansi_enabled(),
    );
    Ok(PasteParts {
        text,
        browse_uri: opts.browse.then(|| detail.uri.clone()),
    })
}

/// `read_paste` with the R4 envelope classification (T1153 PASTE-P18):
/// symbol and not-found failures are toolset usage errors, like PHP's
/// `PhutilArgumentUsageException`s; conduit failures are fatal.
fn read_paste_classified(
    client: &crate::conduit::Client,
    symbol_text: &str,
) -> Result<String, crate::conduit::ClassifiedError> {
    use crate::conduit::ClassifiedError;
    let symbol = parse_paste_symbol(symbol_text).map_err(ClassifiedError::toolset_usage)?;
    let result = client
        .call(
            "paste.search",
            &serde_json::json!({
                "constraints": search_constraints(&symbol),
                "attachments": {"content": true},
            }),
        )
        .map_err(|e| ClassifiedError::fatal(e.to_string()))?;
    let hit = result
        .get("data")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .ok_or_else(|| {
            ClassifiedError::toolset_usage(format!(
                "Paste \"{symbol_text}\" does not exist, or you do not have access to see it."
            ))
        })?;
    // T1153 PASTE-P11: missing attachments echo `''`.
    Ok(hit
        .get("attachments")
        .and_then(|a| a.get("content"))
        .and_then(|c| c.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

fn execute_parts(
    dir: &Path,
    argv: &[String],
    stdin_text: Option<&str>,
) -> Result<PasteParts, crate::conduit::ClassifiedError> {
    use crate::conduit::ClassifiedError;
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => {
            return Ok(PasteParts {
                text: usage(),
                browse_uri: None,
            });
        }
        ParseOutcome::Error(message) => return Err(ClassifiedError::toolset_usage(message)),
    };
    let client =
        crate::config::client_for(opts.conduit_uri.as_deref()).map_err(ClassifiedError::fatal)?;

    if let Some(symbol) = opts.symbol {
        // T1153 PASTE-P4: PHP `echo`s the content raw (no `\n` appended).
        return Ok(PasteParts {
            text: read_paste_classified(&client, &symbol)?,
            browse_uri: None,
        });
    }
    create_paste_parts(dir, &client, &opts, stdin_text)
}

/// Run the workflow; `stdin_text` overrides stdin when creating from stdin
/// (`None` reads real stdin).
pub fn execute_with_stdin(
    dir: &Path,
    argv: &[String],
    stdin_text: Option<&str>,
) -> Result<String, String> {
    let parts = execute_parts(dir, argv, stdin_text).map_err(|err| err.message)?;
    if let Some(uri) = parts.browse_uri {
        let configured = crate::browser::configured_browser(dir);
        crate::browser::open_uris_in_browser(std::slice::from_ref(&uri), configured.as_deref())?;
    }
    Ok(parts.text)
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

/// [`run_in`] with a stdout sink (deterministic under test): the text is
/// emitted before any browsing, so a browser failure still leaves the
/// printed paste behind (T1153 PASTE-P3). Errors report through the R4
/// toolset envelope (T1153 PASTE-P18).
pub fn run_in_with_sink(dir: &Path, argv: &[String], sink: &mut dyn FnMut(&str)) -> ExitCode {
    match execute_parts(dir, argv, None) {
        Ok(parts) => {
            sink(&parts.text);
            use std::io::Write;
            std::io::stdout().flush().ok();
            if let Some(uri) = parts.browse_uri {
                let configured = crate::browser::configured_browser(dir);
                if let Err(message) = crate::browser::open_uris_in_browser(
                    std::slice::from_ref(&uri),
                    configured.as_deref(),
                ) {
                    return crate::conduit::ClassifiedError::fatal(message).report();
                }
            }
            ExitCode::SUCCESS
        }
        Err(err) => err.report(),
    }
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    run_in_with_sink(dir, argv, &mut |text| print!("{text}"))
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "paste_tests.rs"]
mod paste_tests;
