//! `download` workflow: download a file to local disk.
//!
//! PHP reference: `src/workflow/ArcanistDownloadWorkflow.php`.
//!
//! The file is resolved over Conduit (`file.search` by ID), then fetched
//! from its data URI and written to disk (or stdout with `--as -`). The
//! downloaded size is verified against the expected size.
//!
//! Status lines render through the log engine (blue `DATA`/`DONE`
//! labels on stderr); large downloads draw a console progress sink.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::{
    conduit::{self, EnvelopeKind},
    terminal,
};

pub const NAME: &str = "download";

/// T1153 DL-D8: the stdout path never overrides the timeout, so it
/// keeps the `BaseHTTPFuture` default of 300 seconds (file downloads
/// use `download_timeout()` instead).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// T1153 DL-D9: files larger than this draw a progress sink
/// (`$minimum_bar_bytes`).
const PROGRESS_MIN_BYTES: u64 = 4 * 1024 * 1024;

/// T1153 BROWSE-B23: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string.
const HELP_EXAMPLES: &str = "**download** [__options__] -- __file__";
const HELP_SYNOPSIS: &str = "Download a file to local disk.";
const HELP_TEXT: &str = "Download a file to local disk.";
/// Non-wildcard argument specs in `msort` order: (name, param, help).
const HELP_SPECS: &[(&str, Option<&str>, &str)] =
    &[("as", Some("path"), "Save the file to a specific location.")];

/// `PhutilArgumentParser::indent()`: interpret markup, then wrap.
fn help_indent(level: usize, text: &str, ansi: bool) -> String {
    terminal::console_wrap(&terminal::console_format(text, &[], ansi), level, true)
}

/// `renderArgumentSpecs()` for the workflow arguments.
fn help_specs(ansi: bool) -> String {
    let mut out: Vec<String> = Vec::new();
    for (name, param, help) in HELP_SPECS {
        let mut first = help_indent(6, &format!("__--{name}__"), ansi);
        if let Some(param) = param {
            first.push_str(&terminal::console_format(
                &format!(" __{param}__"),
                &[],
                ansi,
            ));
        }
        out.push(first);
        out.push(help_indent(10, help, ansi));
        out.push(String::new());
    }
    out.join("\n")
}

/// The full `--help` text: `**DOWNLOAD WORKFLOW**` header plus the
/// detailed workflow help and a trailing blank line.
fn usage_text(ansi: bool) -> String {
    let header = terminal::console_format("**%s**\n\n", &["DOWNLOAD WORKFLOW"], ansi);
    let body = [
        help_indent(0, HELP_EXAMPLES, ansi),
        help_indent(0, HELP_SYNOPSIS, ansi),
        String::new(),
        help_indent(0, HELP_TEXT, ansi),
        String::new(),
        help_specs(ansi),
        String::new(),
    ]
    .join("\n");
    format!("{header}{body}\n")
}

fn usage() -> String {
    usage_text(terminal::ansi_enabled())
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
    /// T1153 BROWSE-B21: stdin is piped and argv carries no `--`.
    MissingTerminator,
}

/// `F123` or `123` to a numeric file ID, mirroring `/^F?\d+$/` (uppercase
/// `F` only, no trimming) plus the symbol engine's `[1-9]\d*` rule.
pub fn parse_file_symbol(text: &str) -> Result<u64, String> {
    let digits = text.strip_prefix('F').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Specify file by ID, e.g. F123.".to_string());
    }
    // T1153 DL-D3: `didParseArguments` accepts `F0`/`0`/leading zeros,
    // but the symbol engine rejects them (`ArcanistSimpleSymbolRef.php`
    // `([1-9]\d*)`, with the file `[Ff]?` prefix after the uppercase-`F`
    // gate above).
    if digits.starts_with('0') {
        return Err(format!(
            "The format of symbol \"{text}\" is unrecognized. Expected a monogram like \"X123\", or an ID like \"123\", or a PHID."
        ));
    }
    // PHP `(int)` casts saturate at `PHP_INT_MAX`.
    Ok(digits.parse::<u64>().unwrap_or(i64::MAX as u64))
}

/// T1153 BROWSE-B21: noninteractive runs must carry `--`
/// (`ArcanistRuntime.php:80-86`, enforced by
/// `PhutilArgumentParser::parseFull()`). Any standalone `--` token
/// satisfies PHP: the standard-arguments scan that records it never
/// consumes workflow flag values.
fn terminator_missing(argv: &[String], stdin_tty: bool) -> bool {
    !stdin_tty && !argv.iter().any(|arg| arg == "--")
}

const MISSING_TERMINATOR: &str = "Missing required \"--\" in argument list.";

/// The three cyan `USAGE` hints PHP prints before the terminator
/// error (`ArcanistRuntime.php:172-188`), on stderr like PHP.
fn print_terminator_hints(ansi: bool) {
    use crate::terminal::{PhtArg, TVal};
    let usage = terminal::pht("USAGE", &[]);
    for hint in [
        terminal::pht(
            "\"%s\" is being run noninteractively, but the argument list is missing \"--\" to indicate end of flags.",
            &[PhtArg::Text("arc")],
        ),
        terminal::pht(
            "When running noninteractively, you MUST provide \"--\" to all commands (even if they take no arguments).",
            &[],
        ),
    ] {
        eprint!("{}", terminal::format_log("cyan", &usage, &hint, ansi));
    }
    // `tsprintf('%s <__%s__>', ...)` nested like PHP: the inner line
    // passes through the outer `%s` unescaped (`TVal::Terminal`).
    let inner = terminal::tsprintf_ansi(
        "%s <__%s__>",
        &[
            TVal::Text(&terminal::pht("Learn More:", &[])),
            TVal::Text("https://phurl.io/u/noninteractive"),
        ],
        ansi,
    )
    .expect("fixed tsprintf pattern");
    let line = terminal::tsprintf_ansi(
        "**<bg:cyan> %s </bg>** %s\n",
        &[TVal::Text(&usage), TVal::Terminal(&inner)],
        ansi,
    )
    .expect("fixed tsprintf pattern");
    eprint!("{}", line.as_str());
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

/// T1153 DL-D14: a separate `--as ''` is an empty value, not a
/// missing one: PHP resolves it to the working directory and reports
/// `already exists` (like `--as=` already does here).
fn take_value_or_empty(
    iter: &mut std::slice::Iter<'_, String>,
    flag: &str,
) -> Result<String, ParseOutcome> {
    match iter.next() {
        Some(value) => Ok(value.clone()),
        None => Err(ParseOutcome::Error(format!("{flag} requires a value"))),
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
            match take_value_or_empty(iter, "--as") {
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

    fn finish(self, missing_terminator: bool) -> ParseOutcome {
        // T1153 BROWSE-B21: the terminator check runs after flag
        // scanning (unknown flags report first, like PHP's
        // `parseFull()`), but before workflow validation (PHP's
        // `didParseArguments` runs after it).
        if missing_terminator {
            return ParseOutcome::MissingTerminator;
        }
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
    parse_args_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_args`] with an injected TTY flag (deterministic tests).
fn parse_args_with_tty(argv: &[String], stdin_tty: bool) -> ParseOutcome {
    let missing = terminator_missing(argv, stdin_tty);
    let mut parser = Parser::new();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        match parser.consume(arg, &mut iter) {
            Step::Handled => {}
            // `--help` reroutes through the help workflow in PHP,
            // which still requires the terminator.
            Step::Done(ParseOutcome::Help) if missing => {
                return ParseOutcome::MissingTerminator;
            }
            Step::Done(outcome) => return outcome,
        }
    }
    parser.finish(missing)
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

/// T1153 DL-D4: PHP `basename()` only strips `/` on Linux.
fn basename(name: &str) -> &str {
    if cfg!(windows) {
        name.rsplit(['/', '\\']).next().unwrap_or(name)
    } else {
        name.rsplit('/').next().unwrap_or(name)
    }
}

/// `Filesystem::pathExists()`: true for symlinks, even broken ones
/// (`file_exists() || is_link()`).
fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// `Filesystem::resolvePath()`: absolute, parent symlinks resolved, a
/// trailing symlink itself kept, missing leaves resolved through the
/// nearest existing ancestor (else lexical).
///
/// T1153 DL-D17: PHP downloads to (and displays) this canonical path,
/// not the lexical join.
fn canonical_target(dir: &Path, path: &str) -> PathBuf {
    let joined = dir.join(path);
    if joined.is_symlink()
        && let Some(parent) = joined.parent()
        && let Some(name) = joined.file_name()
        && let Ok(canonical) = parent.canonicalize()
    {
        return canonical.join(name);
    }
    if let Ok(canonical) = joined.canonicalize() {
        return canonical;
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = joined.as_path();
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            missing.push(name.to_os_string());
        }
        if let Ok(canonical) = parent.canonicalize() {
            let mut out = canonical;
            for name in missing.iter().rev() {
                out.push(name);
            }
            return out;
        }
        current = parent;
    }
    joined
}

/// Append `.1`, `.2`, ... until the path is free.
///
/// T1153 DL-D6: like `writeUniqueFile()`'s `fopen('x')`, a broken
/// symlink occupies its name, so the download uniquifies past it.
fn resolve_unique(path: &Path) -> PathBuf {
    if !path_exists(path) {
        return path.to_path_buf();
    }
    let base = path.to_string_lossy().to_string();
    let mut counter = 1u64;
    loop {
        let candidate = PathBuf::from(format!("{base}.{counter}"));
        if !path_exists(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// Resolve where to save the download: `--as` wins (and must not exist);
/// otherwise the file's own name, uniquified when taken.
///
/// T1153 DL-D5: `--as` naming a broken symlink is `already exists`.
pub fn resolve_target(
    dir: &Path,
    save_as: Option<&str>,
    file: &FileRef,
) -> Result<PathBuf, String> {
    match save_as {
        Some(path) => {
            let target = canonical_target(dir, path);
            if path_exists(&target) {
                return Err(format!("File \"{}\" already exists.", target.display()));
            }
            Ok(target)
        }
        None => {
            let display = file.display_name();
            let target = canonical_target(dir, basename(&display));
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
///
/// T1153 DL-D17: only a `dir/` prefix strips (against `dir` and its
/// canonical form, like PHP); the directory itself shows absolute.
fn readable_path(dir: &Path, target: &Path) -> String {
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    for base in [dir, canonical_dir.as_path()] {
        if let Ok(relative) = target.strip_prefix(base)
            && !relative.as_os_str().is_empty()
        {
            return relative.display().to_string();
        }
    }
    target.display().to_string()
}

/// Download one file with an existing client.
///
/// Statuses go to stderr; the returned text is always empty.
pub fn download_file(
    client: &crate::conduit::Client,
    dir: &Path,
    file_id: u64,
    save_as: Option<&str>,
) -> Result<String, String> {
    download_file_for_symbol(client, dir, file_id, &format!("F{file_id}"), save_as)
}

/// `PhutilConsoleProgressSink` for large downloads: `[ 12.3%]
/// Working...` on stderr, throttled (TTY: 5/s with `\r` overwrite;
/// piped: every 2s as new lines), plus a trailing newline on
/// completion or failure.
struct ProgressSink {
    total: u64,
    completed: u64,
    last_update: Option<std::time::Instant>,
    tty: bool,
}

impl ProgressSink {
    fn new(total: u64, tty: bool) -> Self {
        Self {
            total,
            completed: 0,
            last_update: None,
            tty,
        }
    }

    /// `sprintf('% 3.1f%%', 100 * $percent)` (clamped, half away from zero).
    fn marker(&self) -> String {
        let fraction = (self.completed as f64 / self.total.max(1) as f64).clamp(0.0, 1.0);
        let tenths = (1000.0 * fraction).round() as u64;
        let body = format!("{}.{}", tenths / 10, tenths % 10);
        format!(" {body:>3}%")
    }

    fn message(&self) -> String {
        format!("[{}] Working...", self.marker())
    }

    fn should_publish(&self) -> bool {
        let Some(last) = self.last_update else {
            return true;
        };
        let min_gap = if self.tty { 0.2 } else { 2.0 };
        last.elapsed().as_secs_f64() >= min_gap
    }

    fn made_progress(&mut self, bytes: u64) {
        self.completed += bytes;
        if !self.should_publish() {
            return;
        }
        self.last_update = Some(std::time::Instant::now());
        let message = self.message();
        if self.tty {
            // PHP's pad-to-`lineWidth` arm never runs (`lineWidth`
            // stays null), so overwrites carry no padding.
            eprint!("\r{message}");
        } else {
            eprintln!("{message}");
        }
    }

    fn complete(self) {
        eprintln!();
    }

    fn fail(self) {
        eprintln!();
    }
}

/// Open the download target: `--as` paths refuse overwrites (like
/// `setDownloadPath()`); unique paths are fresh by construction.
fn create_target(target: &Path, unique: bool) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if unique {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    options.open(target).map_err(|e| {
        // T1153 DL-D16: the ceb race message carries a `%s` with no
        // argument (TOCTOU-only), reproduced byte for byte.
        if !unique && e.kind() == std::io::ErrorKind::AlreadyExists {
            "Specified download path \"%s\" already exists, refusing to overwrite.".to_string()
        } else {
            format!("failed to open \"{}\": {e}", target.display())
        }
    })
}

/// Stream one data URI straight to `target` (`setDownloadPath`),
/// returning the bytes written.
///
/// T1153 DL-D15: bytes stream to disk in 32 KiB chunks instead of
/// buffering the whole file in RAM.
fn download_to_path(
    url: &str,
    timeout: Duration,
    target: &Path,
    unique: bool,
    expected_bytes: u64,
    stderr_tty: bool,
) -> Result<u64, String> {
    // T1153 DL-D9: files over 4 MiB draw a progress sink.
    let mut sink = (expected_bytes > PROGRESS_MIN_BYTES)
        .then(|| ProgressSink::new(expected_bytes, stderr_tty));
    let fail = |sink: Option<ProgressSink>, target: &Path, message: String| {
        if let Some(sink) = sink {
            sink.fail();
        }
        std::fs::remove_file(target).ok();
        message
    };
    let http = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let mut response = http
        .get(url)
        .send()
        .map_err(|e| fail(sink.take(), target, format!("download failed: {e}")))?;
    if !response.status().is_success() {
        let status = response.status();
        return Err(fail(
            sink.take(),
            target,
            format!("download failed: HTTP {status}"),
        ));
    }
    let mut out = match create_target(target, unique) {
        Ok(file) => file,
        // `setDownloadPath()` throws outside the `removePath` catch,
        // so a racy file stays (only the sink fails).
        Err(message) => {
            if let Some(sink) = sink.take() {
                sink.fail();
            }
            return Err(message);
        }
    };
    let mut written: u64 = 0;
    let mut chunk = [0u8; 32 * 1024];
    loop {
        match response.read(&mut chunk) {
            Ok(0) => break,
            Ok(got) => {
                if let Err(e) = out.write_all(&chunk[..got]) {
                    return Err(fail(
                        sink.take(),
                        target,
                        format!("failed to write \"{}\": {e}", target.display()),
                    ));
                }
                written += got as u64;
                if let Some(sink) = sink.as_mut() {
                    sink.made_progress(got as u64);
                }
            }
            Err(e) => {
                return Err(fail(sink.take(), target, format!("download failed: {e}")));
            }
        }
    }
    if let Some(sink) = sink.take() {
        sink.complete();
    }
    Ok(written)
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
    // T1153 DL-D1: blue-labeled statuses on stderr via `writeStatus`,
    // never stdout.
    let ansi = terminal::ansi_enabled();
    eprint!(
        "{}",
        terminal::format_log(
            "blue",
            &terminal::pht("DATA", &[]),
            &format_downloading(&display_name, file.size, &display_path),
            ansi,
        )
    );
    let written = download_to_path(
        &file.data_uri,
        download_timeout(file.size),
        &target,
        save_as.is_none(),
        file.size,
        {
            use std::io::IsTerminal;
            std::io::stderr().is_terminal()
        },
    )?;
    // T1153 DL-D7: the size check runs after the bytes land, so a
    // mismatch leaves the corrupt file behind like PHP.
    if written != file.size {
        return Err(size_mismatch_error(written, file.size));
    }
    eprint!(
        "{}",
        terminal::format_log(
            "blue",
            &terminal::pht("DONE", &[]),
            &format_saved(&display_name, &display_path),
            ansi,
        )
    );
    Ok(String::new())
}

/// `Downloading "name" (N byte(s)) to "path"...`: the `DATA` message
/// PHP passes to `writeStatus`.
fn format_downloading(display_name: &str, size: u64, display_path: &str) -> String {
    format!(
        "Downloading \"{display_name}\" ({} byte(s)) to \"{display_path}\"...",
        format_count(size)
    )
}

/// `Saved "name" as "path".`: the `DONE` message.
fn format_saved(display_name: &str, display_path: &str) -> String {
    format!("Saved \"{display_name}\" as \"{display_path}\".")
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
        ParseOutcome::MissingTerminator => return Err(MISSING_TERMINATOR.to_string()),
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
            if message == MISSING_TERMINATOR {
                print_terminator_hints(terminal::ansi_enabled());
            }
            let kind = if matches!(
                parse_args(argv),
                ParseOutcome::Error(_) | ParseOutcome::MissingTerminator
            ) || is_toolset_usage_message(&message)
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
