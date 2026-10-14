//! `download` workflow: download a file to local disk.
//!
//! PHP reference: `src/workflow/ArcanistDownloadWorkflow.php`.
//!
//! The file is resolved over Conduit (`file.search` by ID), then fetched
//! from its data URI and written to disk (or stdout with `--as -`). The
//! downloaded size is verified against the expected size.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

pub const NAME: &str = "download";

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--as PATH] [--conduit-uri URI] -- F123\n\
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
    save_as: Option<String>,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// `F123` or `123` to a numeric file ID.
pub fn parse_file_symbol(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    let digits = trimmed
        .strip_prefix('F')
        .or_else(|| trimmed.strip_prefix('f'))
        .unwrap_or(trimmed);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits
            .parse::<u64>()
            .map_err(|_| format!("Specify file by ID, e.g. F123 (got '{text}')."))
    } else {
        Err(format!("Specify file by ID, e.g. F123 (got '{text}')."))
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

fn first_search_hit(result: &serde_json::Value) -> Option<&serde_json::Value> {
    result.get("data")?.as_array()?.first()
}

/// Resolve a file by ID via `file.search`.
pub fn fetch_file_ref(client: &crate::conduit::Client, file_id: u64) -> Result<FileRef, String> {
    let result = client
        .call(
            "file.search",
            &serde_json::json!({"constraints": {"ids": [file_id]}}),
        )
        .map_err(|e| e.to_string())?;
    let hit = first_search_hit(&result).ok_or_else(|| {
        format!("File \"F{file_id}\" does not exist, or you do not have permission to view it.")
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

/// Fetch raw bytes from a data URI.
pub fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let http = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
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
    let file = fetch_file_ref(client, file_id)?;
    let display_name = file.display_name();

    if save_as == Some("-") {
        let data = download_bytes(&file.data_uri)?;
        if data.len() as u64 != file.size {
            return Err(format!(
                "Downloaded file size ({} bytes) does not match expected file size ({} bytes). \
                 This download may be incomplete or corrupt.",
                data.len(),
                file.size
            ));
        }
        std::io::stdout()
            .write_all(&data)
            .map_err(|e| format!("failed to write to stdout: {e}"))?;
        return Ok(String::new());
    }

    let target = resolve_target(dir, save_as, &file)?;
    let display_path = target.display().to_string();
    let mut out = format!(
        "Downloading \"{display_name}\" ({} byte(s)) to \"{display_path}\"...\n",
        file.size
    );
    let data = download_bytes(&file.data_uri)?;
    if data.len() as u64 != file.size {
        return Err(format!(
            "Downloaded file size ({} bytes) does not match expected file size ({} bytes). \
             This download may be incomplete or corrupt.",
            data.len(),
            file.size
        ));
    }
    if let Err(e) = std::fs::write(&target, &data) {
        std::fs::remove_file(&target).ok();
        return Err(format!("failed to write \"{display_path}\": {e}"));
    }
    out.push_str(&format!(
        "Saved \"{display_name}\" as \"{display_path}\".\n"
    ));
    Ok(out)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
    download_file(&client, dir, opts.file_id, opts.save_as.as_deref())
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
#[path = "download_tests.rs"]
mod download_tests;
