//! `download` workflow: download a file to local disk.
//!
//! PHP reference: `src/workflow/ArcanistDownloadWorkflow.php`.
//!
//! The file is resolved over Conduit (`file.search` by ID), then fetched
//! from its data URI and written to disk (or stdout with `--as -`). The
//! downloaded size is verified against the expected size.
//!
//! The status lines carry no console markup: PHP styles only the log
//! labels (`DATA`/`DONE`, blue on stderr) and keeps these message words
//! plain, so both ANSI modes share the exact bytes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::conduit::{self, EnvelopeKind};

pub const NAME: &str = "download";

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

fn usage() -> String {
    format!(
        "usage: arc {NAME} [--as PATH] [--conduit-uri URI] -- F123\n\
         \n\
         Download a file to local disk. Without --as, the file is saved\n\
         under its own name; `--as -` writes it to stdout.\n\
         \n\
         Options:\n\
         \x20 --as PATH        save the file to PATH ('-' writes to stdout)\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI / .arcconfig)\n\
         \x20 -h, --help       show this help\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    file_id: u64,
    symbol: String,
    save_as: Option<String>,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// `F123` or `123` to a numeric file ID, mirroring `/^F?\d+$/` (uppercase
/// `F` only, no trimming).
pub fn parse_file_symbol(text: &str) -> Result<u64, String> {
    let digits = text.strip_prefix('F').unwrap_or(text);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        // PHP `(int)` casts saturate at `PHP_INT_MAX`.
        Ok(digits.parse::<u64>().unwrap_or(i64::MAX as u64))
    } else {
        Err("Specify file by ID, e.g. F123.".to_string())
    }
}

/// Thousands separators, mirroring `PhutilNumber` for counts.
pub fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, byte) in digits.bytes().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(byte as char);
    }
    out
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
    save_as: Option<String>,
    conduit_uri: Option<String>,
    positionals: Vec<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            save_as: None,
            conduit_uri: None,
            positionals: Vec::new(),
            end_of_flags: false,
        }
    }

    fn flag_save_as(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Option<Step> {
        if arg == "--as" {
            match take_value(iter, "--as") {
                Ok(value) => self.save_as = Some(value),
                Err(outcome) => return Some(Step::Done(outcome)),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--as=") {
            self.save_as = Some(value.to_string());
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

    fn tail(&mut self, arg: &str) -> Step {
        if arg == "--" {
            self.end_of_flags = true;
            Step::Handled
        } else if arg == "-h" || arg == "--help" {
            Step::Done(ParseOutcome::Help)
        } else if arg.starts_with('-') {
            Step::Done(ParseOutcome::Error(format!("unknown flag '{arg}'")))
        } else {
            self.positionals.push(arg.to_string());
            Step::Handled
        }
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            self.positionals.push(arg.to_string());
            return Step::Handled;
        }
        if let Some(step) = self.flag_save_as(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        self.tail(arg)
    }

    fn finish(self) -> ParseOutcome {
        if self.positionals.is_empty() {
            return ParseOutcome::Error("Specify a file to download.".to_string());
        }
        if self.positionals.len() > 1 {
            return ParseOutcome::Error("Specify exactly one file to download.".to_string());
        }
        let file_id = match parse_file_symbol(&self.positionals[0]) {
            Ok(id) => id,
            Err(message) => return ParseOutcome::Error(message),
        };
        ParseOutcome::Args(Options {
            file_id,
            symbol: self.positionals[0].clone(),
            save_as: self.save_as,
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

/// A file resolved over Conduit, mirroring `ArcanistFileRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub data_uri: String,
}

impl FileRef {
    #[must_use]
    pub fn monogram(&self) -> String {
        format!("F{}", self.id)
    }

    #[must_use]
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.monogram()
        } else {
            self.name.clone()
        }
    }
}

/// Pick the `file.search` hit whose `id` equals `file_id`, like PHP's
/// `ipull($results, null, 'id')` lookup (`ArcanistSimpleSymbolHardpointQuery.php:96-102`):
/// the last duplicate wins, and numeric-string ids match like PHP's
/// loose array keys.
fn select_file_hit(hits: &[serde_json::Value], file_id: u64) -> Option<&serde_json::Value> {
    hits.iter().rfind(|hit| {
        hit.get("id")
            .and_then(crate::diff_submit::loose_u64)
            .is_some_and(|id| id == file_id)
    })
}

/// Resolve a file by ID via `file.search`.
pub fn fetch_file_ref(client: &crate::conduit::Client, file_id: u64) -> Result<FileRef, String> {
    fetch_file_ref_for_symbol(client, file_id, &format!("F{file_id}"))
}

/// Resolve a file, echoing the user's original symbol in the not-found
/// error like PHP (`loadFileForSymbol($file_symbol)`).
pub fn fetch_file_ref_for_symbol(
    client: &crate::conduit::Client,
    file_id: u64,
    symbol: &str,
) -> Result<FileRef, String> {
    let hits = crate::revision::conduit_search(
        client,
        "file.search",
        &serde_json::json!({"ids": [file_id]}),
        None,
    )?;
    let hit = select_file_hit(&hits, file_id).ok_or_else(|| {
        format!("File \"{symbol}\" does not exist, or you do not have permission to view it.")
    })?;
    let id = hit
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(file_id);
    let fields = hit.get("fields");
    let name = fields
        .and_then(|f| f.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let size = fields
        .and_then(|f| f.get("size"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let data_uri = fields
        .and_then(|f| f.get("dataURI"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("File \"F{file_id}\" has no download URI."))?
        .to_string();
    Ok(FileRef {
        id,
        name,
        size,
        data_uri,
    })
}

/// Fetch raw bytes from a data URI with the default timeout.
pub fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    download_bytes_with_timeout(url, DOWNLOAD_TIMEOUT)
}

/// Timeout for file downloads, mirroring PHP: 120 seconds plus one second
/// per 32 KiB of expected size.
pub fn download_timeout(expected_bytes: u64) -> Duration {
    Duration::from_secs(120 + expected_bytes / (32 * 1024))
}

/// Fetch raw bytes from a data URI with an explicit timeout.
pub fn download_bytes_with_timeout(url: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let http = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let response = http
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("download failed: HTTP {}", response.status()));
    }
    response
        .bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("download failed: {e}"))
}

fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Append `.1`, `.2`, ... until the path is free.
fn resolve_unique(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let base = path.to_string_lossy().to_string();
    let mut counter = 1u64;
    loop {
        let candidate = PathBuf::from(format!("{base}.{counter}"));
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// Resolve where to save the download: `--as` wins (and must not exist);
/// otherwise the file's own name, uniquified when taken.
pub fn resolve_target(
    dir: &Path,
    save_as: Option<&str>,
    file: &FileRef,
) -> Result<PathBuf, String> {
    match save_as {
        Some(path) => {
            let target = dir.join(path);
            if target.exists() {
                return Err(format!("File \"{}\" already exists.", target.display()));
            }
            Ok(target)
        }
        None => {
            let display = file.display_name();
            let target = dir.join(basename(&display));
            Ok(resolve_unique(&target))
        }
    }
}

fn size_mismatch_error(got: u64, expected: u64) -> String {
    format!(
        "Downloaded file size ({} bytes) does not match expected file size ({} bytes). \
         This download may be incomplete or corrupt.",
        format_count(got),
        format_count(expected)
    )
}

/// Mirror `Filesystem::readablePath()`: paths under `dir` show relative.
fn readable_path(dir: &Path, target: &Path) -> String {
    target
        .strip_prefix(dir)
        .unwrap_or(target)
        .display()
        .to_string()
}

/// Download one file with an existing client.
///
/// With `save_as == Some("-")`, the file bytes are written to stdout
/// directly and the returned text is empty.
pub fn download_file(
    client: &crate::conduit::Client,
    dir: &Path,
    file_id: u64,
    save_as: Option<&str>,
) -> Result<String, String> {
    download_file_for_symbol(client, dir, file_id, &format!("F{file_id}"), save_as)
}

/// Download one file, echoing the user's original symbol in errors.
pub fn download_file_for_symbol(
    client: &crate::conduit::Client,
    dir: &Path,
    file_id: u64,
    symbol: &str,
    save_as: Option<&str>,
) -> Result<String, String> {
    let file = fetch_file_ref_for_symbol(client, file_id, symbol)?;
    let display_name = file.display_name();

    if save_as == Some("-") {
        let data = download_bytes(&file.data_uri)?;
        if data.len() as u64 != file.size {
            return Err(size_mismatch_error(data.len() as u64, file.size));
        }
        std::io::stdout()
            .write_all(&data)
            .map_err(|e| format!("failed to write to stdout: {e}"))?;
        return Ok(String::new());
    }

    let target = resolve_target(dir, save_as, &file)?;
    let display_path = readable_path(dir, &target);
    let mut out = format_downloading(&display_name, file.size, &display_path);
    let data = download_bytes_with_timeout(&file.data_uri, download_timeout(file.size))?;
    if data.len() as u64 != file.size {
        return Err(size_mismatch_error(data.len() as u64, file.size));
    }
    if let Err(e) = std::fs::write(&target, &data) {
        std::fs::remove_file(&target).ok();
        return Err(format!("failed to write \"{display_path}\": {e}"));
    }
    out.push_str(&format_saved(&display_name, &display_path));
    Ok(out)
}

/// `Downloading "name" (N byte(s)) to "path"...`: the words PHP passes
/// to `writeStatus` unstyled.
fn format_downloading(display_name: &str, size: u64, display_path: &str) -> String {
    format!(
        "Downloading \"{display_name}\" ({} byte(s)) to \"{display_path}\"...\n",
        format_count(size)
    )
}

/// `Saved "name" as "path".`: likewise unstyled in PHP.
fn format_saved(display_name: &str, display_path: &str) -> String {
    format!("Saved \"{display_name}\" as \"{display_path}\".\n")
}

/// True for the resolution errors PHP raises as
/// `PhutilArgumentUsageException` in `runWorkflow()` (exit 1 under the
/// `USAGE EXCEPTION` label): unknown files and occupied `--as` paths
/// (`ArcanistDownloadWorkflow.php:70-109`). HTTP, size-mismatch, and
/// write failures are uncaught `Exception`s: fatal (255).
fn is_toolset_usage_message(message: &str) -> bool {
    message.starts_with("File \"")
        && (message.ends_with("\" does not exist, or you do not have permission to view it.")
            || message.ends_with("\" already exists."))
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        // T1153 R4 (DL-D2): PHP renders usage errors with no usage dump.
        ParseOutcome::Error(message) => return Err(message),
    };
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
    download_file_for_symbol(
        &client,
        dir,
        opts.file_id,
        &opts.symbol,
        opts.save_as.as_deref(),
    )
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 R4 (DL-D2, DL-D10): `download` is a toolset
            // workflow. Argument and resolution errors render the red
            // `USAGE EXCEPTION` label (exit 1); HTTP and size failures
            // escape into a PHP fatal (255). (`didParseArguments()`
            // raising `ArcanistUsageException` is dead in the toolset
            // path: the live `runWorkflow()` raises
            // `PhutilArgumentUsageException`, caught at
            // `ArcanistRuntime.php:38-39` — so DL-D2's "fatal 255" for
            // arg errors does not reproduce; only the presentation
            // changes there.)
            let kind = if matches!(parse_args(argv), ParseOutcome::Error(_))
                || is_toolset_usage_message(&message)
            {
                EnvelopeKind::ToolsetUsage
            } else {
                EnvelopeKind::Fatal
            };
            conduit::report(kind, &message)
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod download_tests;
