//! `upload` workflow: upload files from local disk.
//!
//! PHP references: `src/workflow/ArcanistUploadWorkflow.php` and
//! `src/upload/ArcanistFileUploader.php`.
//!
//! Each file is offered to the server with `file.allocate` (name, byte
//! size, SHA-256 content hash); when the server asks for data, missing
//! chunks are sent with `file.querychunks` + `file.uploadchunk`. Servers
//! without `file.allocate` fall back to a single `file.upload` call.
//! Uploaded files are resolved with `file.search` for display. `--browse`
//! opens the uploaded files in a browser (see [`crate::browser`]).
//!
//! The per-file lines render through [`crate::terminal::RefView`] like
//! PHP (`ArcanistUploadWorkflow.php:117-125` + `ArcanistFileRef::buildRefView`):
//! object name `F123`, title, and URI, with title truncation to the
//! terminal width. Chunk progress goes to stderr with a TTY progress bar,
//! like PHP.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::conduit::{self, EnvelopeKind};

pub const NAME: &str = "upload";

/// Temporary files expire after 24 hours, like the PHP workflow.
const TEMPORARY_TTL_SECS: u64 = 24 * 60 * 60;

/// T1153 BROWSE-B23: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. T1153 RB-D5: the renderer is the
/// shared `crate::help` module; this file keeps only the data.
const HELP_EXAMPLES: &str = "**upload** [__options__] -- __file__ [__file__ ...]";
const HELP_SYNOPSIS: &str = "Upload files.";
const HELP_TEXT: &str = "Upload one or more files from local disk to Phabricator.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "browse",
        None,
        None,
        "After the upload completes, open the files in a web browser.",
    ),
    (
        "json",
        None,
        None,
        "Output upload information in JSON format.",
    ),
    (
        "temporary",
        None,
        None,
        "Mark the file as temporary. Temporary files will be deleted after 24 hours.",
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
        Some(HELP_EXAMPLES),
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        true,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(crate::terminal::ansi_enabled())
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    paths: Vec<String>,
    json: bool,
    temporary: bool,
    browse: bool,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
    /// T1153 BROWSE-B21: stdin is piped and argv carries no `--`.
    MissingTerminator,
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
    paths: Vec<String>,
    json: bool,
    temporary: bool,
    browse: bool,
    conduit_uri: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            json: false,
            temporary: false,
            browse: false,
            conduit_uri: None,
            end_of_flags: false,
        }
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
        } else if arg == "--json" {
            self.json = true;
            Step::Handled
        } else if arg == "--temporary" {
            self.temporary = true;
            Step::Handled
        } else if arg == "--browse" {
            self.browse = true;
            Step::Handled
        } else if arg == "-h" || arg == "--help" {
            Step::Done(ParseOutcome::Help)
        } else if arg.starts_with('-') {
            Step::Done(ParseOutcome::Error(format!("unknown flag '{arg}'")))
        } else {
            self.paths.push(arg.to_string());
            Step::Handled
        }
    }

    fn consume(&mut self, arg: &str, iter: &mut std::slice::Iter<'_, String>) -> Step {
        if self.end_of_flags {
            self.paths.push(arg.to_string());
            return Step::Handled;
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
        if self.paths.is_empty() {
            return ParseOutcome::Error(
                "Specify one or more paths to files you want to upload.".to_string(),
            );
        }
        ParseOutcome::Args(Options {
            paths: self.paths,
            json: self.json,
            temporary: self.temporary,
            browse: self.browse,
            conduit_uri: self.conduit_uri,
        })
    }
}

// T1153 RB-D5: terminator gate lives in the shared `crate::help`
// renderer; this module calls it directly.

fn parse_args(argv: &[String]) -> ParseOutcome {
    parse_args_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_args`] with an injected TTY flag (deterministic tests).
fn parse_args_with_tty(argv: &[String], stdin_tty: bool) -> ParseOutcome {
    let missing = crate::help::terminator_missing(argv, stdin_tty);
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

// --- base64 (RFC 4648, standard alphabet) -----------------------------------

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes as standard base64.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let mut n: u32 = 0;
        for &byte in chunk {
            n = (n << 8) | u32::from(byte);
        }
        n <<= 8 * (3 - chunk.len());
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64_ALPHABET[(n >> 18) as usize & 63] as char);
            } else {
                out.push('=');
            }
            n <<= 6;
        }
    }
    out
}

// --- SHA-256 (FIPS 180-4) ----------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_block(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (i, word) in w.iter_mut().enumerate().take(16) {
        *word = u32::from_be_bytes([
            block[4 * i],
            block[4 * i + 1],
            block[4 * i + 2],
            block[4 * i + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let (blocks, rem) = data.as_chunks::<64>();
    for block in blocks {
        sha256_block(&mut state, block);
    }
    let mut tail = [0u8; 64];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    if rem.len() + 1 > 56 {
        sha256_block(&mut state, &tail);
        tail = [0u8; 64];
    }
    tail[56..].copy_from_slice(&bit_len.to_be_bytes());
    sha256_block(&mut state, &tail);
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Lowercase hex SHA-256 of `data`, matching PHP `hash('sha256', ...)`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in sha256_digest(data) {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

// --- upload ---------------------------------------------------------------

/// One uploaded file's resolved identity, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedRef {
    pub argument: String,
    pub id: u64,
    pub phid: String,
    pub name: String,
    pub uri: String,
}

impl UploadedRef {
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "argument": self.argument,
            "id": self.id,
            "phid": self.phid,
            "name": self.name,
            "uri": self.uri,
        })
    }
}

/// Escape already-serialized JSON the way PHP `json_encode()` does
/// (T1153 UPLOAD-U9, EXPORT-E18): serde emits raw UTF-8, while PHP emits
/// `\uXXXX` (with UTF-16 surrogates past the BMP, lowercase hex) and,
/// without `JSON_UNESCAPED_SLASHES`, `\/` for `/`. Everything else serde
/// emits (short escapes, `\u00XX` controls, structural whitespace)
/// already matches, so only string contents are rewritten.
/// `escape_slash` selects the `writeToDisk()` form (`json_encode()` plain)
/// versus the pretty-print form (`JSON_UNESCAPED_SLASHES`).
#[must_use]
pub fn php_escape_json_text(text: &str, escape_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if !in_string {
            out.push(ch);
            if ch == '"' {
                in_string = true;
            }
            continue;
        }
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                out.push(ch);
                escaped = true;
            }
            '"' => {
                out.push(ch);
                in_string = false;
            }
            '/' if escape_slash => out.push_str("\\/"),
            ch if (ch as u32) < 0x80 => out.push(ch),
            ch => {
                let n = ch as u32;
                if n > 0xFFFF {
                    let v = n - 0x10000;
                    let high = 0xD800 + (v >> 10);
                    let low = 0xDC00 + (v & 0x3FF);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                } else {
                    out.push_str(&format!("\\u{n:04x}"));
                }
            }
        }
    }
    out
}

fn phid_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Object(map) => map
            .get("phid")
            .or_else(|| map.get("filePHID"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// PHP truthiness for boolean-ish Conduit fields (`upload`, `complete`).
fn conduit_truthy(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::Number(number)) => {
            number.as_i64().is_none_or(|n| n != 0)
                && number.as_u64().is_none_or(|n| n != 0)
                && number.as_f64().is_none_or(|n| n != 0.0)
        }
        Some(serde_json::Value::String(text)) => !text.is_empty() && text != "0",
        Some(serde_json::Value::Array(items)) => !items.is_empty(),
        Some(serde_json::Value::Object(map)) => !map.is_empty(),
    }
}

fn conduit_error_text(error: crate::conduit::Error) -> String {
    error.to_string()
}

/// TTY chunk progress bar (`PhutilConsoleProgressBar`, T1153 UPLOAD-U10).
/// Draws only when stderr is a TTY, at most every 100ms, `min(errcols, 78)`
/// columns wide; the bar is erased (no `Done.`) when the upload finishes,
/// because PHP never calls `done()` here (only the destructor runs).
struct ChunkProgress {
    total: usize,
    done: usize,
    drawn: bool,
    last_update: Option<std::time::Instant>,
}

impl ChunkProgress {
    fn new() -> Self {
        Self {
            total: 0,
            done: 0,
            drawn: false,
            last_update: None,
        }
    }

    /// Bar width: `min(errcols, 78)` (`getWidth()`).
    fn width() -> usize {
        let width = crate::terminal::terminal_width();
        if width == 0 { 78 } else { width.min(78) }
    }

    fn stderr_is_tty() -> bool {
        use std::io::IsTerminal;
        std::io::stderr().is_terminal()
    }

    /// `setTotal()`: store the total, then a throttled redraw.
    fn set_total(&mut self, total: usize) {
        self.total = total;
        self.redraw();
    }

    /// `update()`: advance, then a throttled redraw.
    fn update(&mut self, work: usize) {
        self.done += work;
        self.redraw();
    }

    fn redraw(&mut self) {
        if let Some(last) = self.last_update
            && last.elapsed() < std::time::Duration::from_millis(100)
        {
            return;
        }
        self.draw();
    }

    /// `draw()`: erase the line and repaint the bar on stderr.
    fn draw(&mut self) {
        if self.total == 0 || !Self::stderr_is_tty() {
            return;
        } // LCOV_EXCL_START // Reason: needs stderr to be a TTY; cargo-test stderr is piped
        let width = Self::width();
        let chrome_width = "[] 100.0%   ".len();
        if width < chrome_width {
            return;
        }
        self.last_update = Some(std::time::Instant::now());
        self.drawn = true;
        Self::erase_line(width);
        eprint!("{}", render_chunk_bar(self.done, self.total, width)); // LCOV_EXCL_STOP
    }

    /// Destructor-equivalent (`done($clean_exit = false)`): erase the bar.
    fn finish(&self) {
        if self.drawn {
            Self::erase_line(Self::width());
        }
    }

    fn erase_line(width: usize) {
        eprint!("\r{}\r", " ".repeat(width));
    }
}

/// Pure bar rendering (`draw()`): `[====...]  12.3%`, bar field padded to
/// the console width minus the 12-column chrome, percent `%5.1f`, with the
/// fractional `-`/`~` tip character.
fn render_chunk_bar(done: usize, total: usize, width: usize) -> String {
    let max_width = width.saturating_sub("[] 100.0%   ".len());
    let percent = done as f64 / total as f64;
    let bar_width = percent * max_width as f64;
    let bar_int = bar_width.floor() as usize;
    let bar_frac = bar_width - bar_int as f64;
    const FRAC_MAP: [&str; 3] = ["", "-", "~"];
    let frac_char = FRAC_MAP[(bar_frac * FRAC_MAP.len() as f64).floor() as usize];
    let field = format!("{}{}", "=".repeat(bar_int.min(max_width)), frac_char);
    format!("[{field:<max_width$}] {:>5.1}%", 100.0 * percent)
}

fn query_file_chunks(
    client: &crate::conduit::Client,
    file_phid: &str,
    display: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let chunks = client
        .call(
            "file.querychunks",
            &serde_json::json!({"filePHID": file_phid}),
        )
        .map_err(conduit_error_text)?;
    match chunks {
        serde_json::Value::Array(chunks) => Ok(chunks),
        _ => Err(format!(
            "file.querychunks for \"{display}\" returned an unexpected response"
        )),
    }
}

fn announce_chunk_progress(chunks: &[serde_json::Value]) {
    let mut remaining = 0;
    for chunk in chunks {
        if !conduit_truthy(chunk.get("complete")) {
            remaining += 1;
        }
    }
    // PHP `writeStatus()` goes to stderr.
    if remaining == chunks.len() {
        eprintln!(
            "Uploading chunks ({} chunks to upload).",
            crate::download::format_count(remaining as u64)
        );
    } else {
        eprintln!(
            "Resuming upload ({} of {} chunks remain).",
            crate::download::format_count(remaining as u64),
            crate::download::format_count(chunks.len() as u64)
        );
    }
}

fn chunk_byte_range(chunk: &serde_json::Value, display: &str) -> Result<(usize, usize), String> {
    let start = match chunk.get("byteStart").and_then(serde_json::Value::as_u64) {
        Some(start) => start as usize,
        None => {
            return Err(format!(
                "file.querychunks for \"{display}\" returned a chunk without byteStart"
            ));
        }
    };
    let end = match chunk.get("byteEnd").and_then(serde_json::Value::as_u64) {
        Some(end) => end as usize,
        None => {
            return Err(format!(
                "file.querychunks for \"{display}\" returned a chunk without byteEnd"
            ));
        }
    };
    Ok((start, end))
}

fn upload_one_chunk(
    client: &crate::conduit::Client,
    data: &[u8],
    file_phid: &str,
    chunk: &serde_json::Value,
    display: &str,
) -> Result<(), String> {
    let (start, end) = chunk_byte_range(chunk, display)?;
    if start > end || end > data.len() {
        return Err(format!(
            "file.querychunks for \"{display}\" returned an out-of-range chunk ({start}..{end})"
        ));
    }
    match client.call(
        "file.uploadchunk",
        &serde_json::json!({
            "filePHID": file_phid,
            "byteStart": start,
            "dataEncoding": "base64",
            "data": base64_encode(&data[start..end]),
        }),
    ) {
        Ok(_) => Ok(()),
        Err(error) => Err(format!("Unable to upload file chunks: {error}")),
    }
}

fn upload_missing_chunks(
    client: &crate::conduit::Client,
    data: &[u8],
    file_phid: &str,
    display: &str,
) -> Result<(), String> {
    let chunks = query_file_chunks(client, file_phid, display)?;
    announce_chunk_progress(&chunks);
    // `uploadChunks()`: pre-fill finished chunks, force one draw (otherwise
    // the bar may sit at 0% until the first chunk completes), then update
    // per chunk.
    let mut bar = ChunkProgress::new();
    bar.set_total(chunks.len());
    let done = chunks
        .iter()
        .filter(|chunk| conduit_truthy(chunk.get("complete")))
        .count();
    for _ in 0..done {
        bar.update(1);
    }
    bar.draw();
    for chunk in &chunks {
        if conduit_truthy(chunk.get("complete")) {
            continue;
        }
        upload_one_chunk(client, data, file_phid, chunk, display)?;
        bar.update(1);
    }
    bar.finish();
    Ok(())
}

/// Legacy whole-file upload (`uploadData()`): one `file.upload` call.
fn upload_legacy_file(
    client: &crate::conduit::Client,
    name: &str,
    data: &[u8],
) -> Result<String, String> {
    let result = client
        .call(
            "file.upload",
            &serde_json::json!({
                "name": name,
                "data_base64": base64_encode(data),
            }),
        )
        .map_err(|e| format!("Unable to upload file data: {e}"))?;
    phid_of(&result)
        .ok_or_else(|| format!("file.upload for \"{name}\" returned an unexpected response"))
}

/// Upload one file's bytes; returns the file PHID.
pub fn upload_data(
    client: &crate::conduit::Client,
    name: &str,
    data: &[u8],
    temporary: bool,
) -> Result<String, String> {
    let mut params = serde_json::json!({
        "name": name,
        "contentLength": data.len(),
        "contentHash": sha256_hex(data),
    });
    if temporary {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        params["deleteAfterEpoch"] = serde_json::json!(now + TEMPORARY_TTL_SECS);
    }
    match client.call("file.allocate", &params) {
        Ok(allocated) => {
            let wants_upload = conduit_truthy(allocated.get("upload"));
            let phid = allocated
                .get("filePHID")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !wants_upload {
                if phid.is_empty() {
                    return Err(format!(
                        "Unable to upload file: the server refused to accept file \"{name}\". \
                         This usually means it is too large."
                    ));
                }
                return Ok(phid);
            }
            if phid.is_empty() {
                // T1153 UPLOAD-U5: `upload=true` with an empty PHID stays in
                // `$files` and falls through to the legacy `file.upload`
                // loop (`ArcanistFileUploader.php:159-166,183-194`).
                return upload_legacy_file(client, name, data);
            }
            upload_missing_chunks(client, data, &phid, name)?;
            Ok(phid)
        }
        // Old servers have no `file.allocate`; send the whole file at once.
        Err(_) => upload_legacy_file(client, name, data),
    }
}

/// Resolve conduit URI precedence for absolutizing display URIs, mirroring
/// `config::client_for` (explicit flag, `ARC_CONDUIT_URI`, `.arcconfig`).
pub fn resolve_conduit_uri(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(crate::config::discover_conduit_uri)
}

/// Server root of a conduit URI (`https://host[/api...]` -> `https://host`).
pub fn conduit_root(conduit_uri: &str) -> String {
    let trimmed = conduit_uri.trim_end_matches('/');
    trimmed.strip_suffix("/api").unwrap_or(trimmed).to_string()
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
/// any query or fragment is dropped (T1153 UPLOAD-U14), like PHP's
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

/// Resolve uploaded PHIDs to display refs via `file.search`,
/// following `cursor.after` pagination like `ConduitSearchFuture`.
pub fn resolve_refs(
    client: &crate::conduit::Client,
    uploads: &[(String, String)],
    conduit_uri: Option<&str>,
) -> Result<Vec<UploadedRef>, String> {
    let phids: Vec<&str> = uploads.iter().map(|(_, phid)| phid.as_str()).collect();
    let data = crate::revision::conduit_search(
        client,
        "file.search",
        &serde_json::json!({"phids": phids}),
        None,
    )?;
    let mut refs = Vec::with_capacity(uploads.len());
    for (argument, phid) in uploads {
        // Last duplicate wins, like PHP's `ipull($results, null, 'phid')`.
        let hit = data
            .iter()
            .rfind(|h| h.get("phid").and_then(|v| v.as_str()) == Some(phid.as_str()))
            .ok_or_else(|| format!("Failed to resolve symbol ref \"{phid}\"."))?;
        let id = hit
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let fields = hit.get("fields");
        let name = fields
            .and_then(|f| f.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let uri = fields
            .and_then(|f| f.get("uri"))
            .and_then(|v| v.as_str())
            .map_or_else(|| format!("/F{id}"), str::to_string);
        refs.push(UploadedRef {
            argument: argument.clone(),
            id,
            phid: phid.clone(),
            name,
            uri: absolute_uri(conduit_uri, &uri),
        });
    }
    Ok(refs)
}

/// Upload `paths` with an existing client; `conduit_uri` absolutizes URIs.
pub fn upload_paths(
    client: &crate::conduit::Client,
    dir: &Path,
    paths: &[String],
    temporary: bool,
    json: bool,
    conduit_uri: Option<&str>,
) -> Result<String, String> {
    upload_paths_with_browse(client, dir, paths, temporary, json, false, conduit_uri)
}

/// Read one path's bytes, mirroring `ArcanistFileDataRef::willUpload()`
/// (T1153 UPLOAD-U4): missing, not-a-file, and not-readable paths fail
/// with the PHP texts (against the as-typed path); a file which still
/// fails to read reports the `readBytes()` open failure.
fn read_upload_bytes(dir: &Path, path: &str) -> Result<Vec<u8>, String> {
    let disk = dir.join(path);
    // `Filesystem::pathExists()`: `file_exists() || is_link()`.
    if std::fs::symlink_metadata(&disk).is_err() {
        return Err(format!(
            "Unable to upload file: path \"{path}\" does not exist."
        ));
    }
    // `Filesystem::assertIsFile()`: `is_file()` follows links, so broken
    // links and directories land here.
    if !std::fs::metadata(&disk).is_ok_and(|meta| meta.is_file()) {
        return Err(format!(
            "Unable to upload file: path \"{path}\" is not a file."
        ));
    }
    match std::fs::read(&disk) {
        Ok(data) => Ok(data),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(format!(
            "Unable to upload file: path \"{path}\" is not readable."
        )),
        Err(_) => Err(format!(
            "Unable to upload file: failed to open path \"{path}\" for reading."
        )),
    }
}

fn upload_single_path(
    client: &crate::conduit::Client,
    dir: &Path,
    path: &str,
    temporary: bool,
) -> Result<(String, String), String> {
    let data = read_upload_bytes(dir, path)?;
    let name = match dir.join(path).file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => path.to_string(), // LCOV_EXCL_LINE // Reason: dead arm; a path that read successfully always has a file_name
    };
    let phid = upload_data(client, &name, &data, temporary)?;
    Ok((path.to_string(), phid))
}

fn collect_uploads(
    client: &crate::conduit::Client,
    dir: &Path,
    paths: &[String],
    temporary: bool,
) -> Result<Vec<(String, String)>, String> {
    // PHP uploads every file first, then throws the first file's errors.
    let mut uploads: Vec<(String, String)> = Vec::with_capacity(paths.len());
    let mut first_error: Option<String> = None;
    for path in paths {
        match upload_single_path(client, dir, path, temporary) {
            Ok(pair) => uploads.push(pair),
            Err(message) => {
                if first_error.is_none() {
                    first_error = Some(message);
                }
            }
        }
    }
    match first_error {
        Some(message) => Err(message),
        None => Ok(uploads),
    }
}

fn browse_uploaded_refs(dir: &Path, refs: &[UploadedRef]) -> Result<(), String> {
    let mut uris: Vec<String> = Vec::with_capacity(refs.len());
    for item in refs {
        uris.push(item.uri.clone());
    }
    let configured = crate::browser::configured_browser(dir);
    crate::browser::open_uris_in_browser(&uris, configured.as_deref())
}

/// Render resolved uploads with an explicit console width and ANSI
/// setting (deterministic under test): JSON pretty-prints with PHP's
/// `\uXXXX` escapes (T1153 UPLOAD-U9); otherwise one
/// [`crate::terminal::RefView`] per file (T1153 UPLOAD-U1), titles truncated
/// to the width (T1153 UPLOAD-U2).
pub fn render_uploaded_refs_ansi(
    refs: &[UploadedRef],
    json: bool,
    width: usize,
    ansi: bool,
) -> Result<String, String> {
    if json {
        let list: Vec<serde_json::Value> = refs.iter().map(UploadedRef::to_json).collect();
        let mut text = match serde_json::to_string_pretty(&list) {
            Ok(text) => text,
            Err(error) => return Err(format!("failed to encode JSON: {error}")), // LCOV_EXCL_LINE // Reason: dead arm; to_string_pretty on a Vec<Value> cannot fail
        };
        text = php_escape_json_text(&text, false);
        text.push('\n');
        return Ok(text);
    }
    let mut out = String::new();
    for item in refs {
        // `ArcanistFileRef::buildRefView()`: monogram + name.
        let view = crate::terminal::RefView {
            object_name: Some(format!("F{}", item.id)),
            title: Some(item.name.clone()),
            uri: Some(item.uri.clone()),
            ..Default::default()
        };
        out.push_str(&view.render(0, width, ansi));
    }
    Ok(out)
}

fn render_uploaded_refs(refs: &[UploadedRef], json: bool) -> Result<String, String> {
    render_uploaded_refs_ansi(
        refs,
        json,
        crate::terminal::terminal_width(),
        crate::terminal::ansi_enabled(),
    )
}

/// Upload `paths`, opening the uploaded files in a browser when `browse`.
/// Renders before browsing (T1153 UPLOAD-U3): PHP echoes the refs, then
/// opens the browser.
pub fn upload_paths_with_browse(
    client: &crate::conduit::Client,
    dir: &Path,
    paths: &[String],
    temporary: bool,
    json: bool,
    browse: bool,
    conduit_uri: Option<&str>,
) -> Result<String, String> {
    let uploads = collect_uploads(client, dir, paths, temporary)?;
    let refs = resolve_refs(client, &uploads, conduit_uri)?;
    let text = render_uploaded_refs(&refs, json)?;
    if browse {
        browse_uploaded_refs(dir, &refs)?;
    }
    Ok(text)
}

/// Uploaded output plus the display URIs, without browsing: lets `run_in`
/// print before browsing, like PHP.
struct UploadParts {
    text: String,
    uris: Vec<String>,
    browse: bool,
}

fn execute_parts(dir: &Path, argv: &[String]) -> Result<UploadParts, String> {
    execute_parts_with(dir, argv, crate::config::arcrc_path().as_deref())
}

/// [`execute_parts`] with an explicit credential file (test seam:
/// hermetic tests stage a fake arcrc instead of touching HOME).
fn execute_parts_with(
    dir: &Path,
    argv: &[String],
    user_file: Option<&Path>,
) -> Result<UploadParts, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => {
            return Ok(UploadParts {
                text: usage(),
                uris: Vec::new(),
                browse: false,
            });
        }
        ParseOutcome::MissingTerminator => {
            return Err(crate::help::MISSING_TERMINATOR.to_string());
        }
        // T1153 R4 (UPLOAD-U15): PHP renders usage errors with no dump.
        ParseOutcome::Error(message) => return Err(message),
    };
    // Byte-identical to `config::client_for(..)` when the wrapper
    // passes the ambient user file (same cwd fallback).
    let config_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let client = crate::config::client_for_with(
        opts.conduit_uri.as_deref(),
        &config_dir,
        user_file,
        &crate::get_config::LoadOptions::default(),
    )?;
    let base = resolve_conduit_uri(opts.conduit_uri.as_deref());
    let uploads = collect_uploads(&client, dir, &opts.paths, opts.temporary)?;
    let refs = resolve_refs(&client, &uploads, base.as_deref())?;
    let text = render_uploaded_refs(&refs, opts.json)?;
    let uris = refs.iter().map(|item| item.uri.clone()).collect();
    Ok(UploadParts {
        text,
        uris,
        browse: opts.browse,
    })
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    execute_with_user_file(dir, argv, crate::config::arcrc_path().as_deref())
}

/// [`execute`] with an explicit credential file (test seam: hermetic
/// tests stage a fake arcrc instead of touching HOME).
fn execute_with_user_file(
    dir: &Path,
    argv: &[String],
    user_file: Option<&Path>,
) -> Result<String, String> {
    let parts = execute_parts_with(dir, argv, user_file)?;
    if parts.browse {
        let configured = crate::browser::configured_browser(dir);
        crate::browser::open_uris_in_browser(&parts.uris, configured.as_deref())?;
    }
    Ok(parts.text)
}

/// [`run_in`] with a stdout sink (deterministic under test): the text is
/// emitted before any browsing, so a browser failure still leaves the
/// printed refs behind (T1153 UPLOAD-U3).
pub fn run_in_with_sink(dir: &Path, argv: &[String], sink: &mut dyn FnMut(&str)) -> ExitCode {
    match execute_parts(dir, argv) {
        Ok(parts) => {
            sink(&parts.text);
            use std::io::Write;
            std::io::stdout().flush().ok();
            if parts.browse {
                let configured = crate::browser::configured_browser(dir);
                if let Err(message) =
                    crate::browser::open_uris_in_browser(&parts.uris, configured.as_deref())
                {
                    // Browse failures are uncaught `ArcanistUsageException`s
                    // (a plain `Exception` subclass): fatal (255).
                    return conduit::report(EnvelopeKind::Fatal, &message);
                }
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 R4 (UPLOAD-U15): `upload` is a toolset workflow.
            // Argument errors render the red `USAGE
            // EXCEPTION` label (exit 1); aggregate and resolve errors
            // are uncaught `Exception`s: fatal (255)
            // (`ArcanistUploadWorkflow.php:43,77-93`).
            if message == crate::help::MISSING_TERMINATOR {
                crate::help::print_terminator_hints(crate::terminal::ansi_enabled());
            }
            let kind = if matches!(
                parse_args(argv),
                ParseOutcome::Error(_) | ParseOutcome::MissingTerminator
            ) {
                EnvelopeKind::ToolsetUsage
            } else {
                EnvelopeKind::Fatal
            };
            conduit::report(kind, &message)
        }
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
#[path = "upload_tests.rs"]
mod upload_tests;
