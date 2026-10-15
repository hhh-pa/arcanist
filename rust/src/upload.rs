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
//! The per-file lines carry no console markup: PHP renders each upload
//! as a (styled) ref view, while this port keeps plain `F123 name` lines,
//! so both ANSI modes share the exact bytes. Chunk progress already goes
//! to stderr unstyled, like PHP.

use std::path::Path;
use std::process::ExitCode;

pub const NAME: &str = "upload";

/// Temporary files expire after 24 hours, like the PHP workflow.
const TEMPORARY_TTL_SECS: u64 = 24 * 60 * 60;

fn usage() -> String {
    format!(
        "usage: arc {NAME} [--json] [--temporary] [--browse] [--conduit-uri URI] -- FILE [FILE ...]\n\
         \n\
         Upload one or more files from local disk.\n\
         \n\
         Options:\n\
         \x20 --json         output upload information in JSON format\n\
         \x20 --temporary    mark files temporary (deleted after 24 hours)\n\
         \x20 --browse       after the upload completes, open the files in a web browser\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI / .arcconfig)\n\
         \x20 -h, --help     show this help\n"
    )
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
}

fn take_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, ParseOutcome> {
    match iter.next() {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        _ => Err(ParseOutcome::Error(format!("{flag} requires a value"))),
    }
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut paths: Vec<String> = Vec::new();
    let mut json = false;
    let mut temporary = false;
    let mut browse = false;
    let mut conduit_uri: Option<String> = None;
    let mut end_of_flags = false;

    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            paths.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--temporary" {
            temporary = true;
        } else if arg == "--conduit-uri" {
            match take_value(&mut iter, "--conduit-uri") {
                Ok(value) => conduit_uri = Some(value),
                Err(outcome) => return outcome,
            }
        } else if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            conduit_uri = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg == "--browse" {
            browse = true;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            paths.push(arg.clone());
        }
    }

    if paths.is_empty() {
        return ParseOutcome::Error(
            "Specify one or more paths to files you want to upload.".to_string(),
        );
    }
    ParseOutcome::Args(Options {
        paths,
        json,
        temporary,
        browse,
        conduit_uri,
    })
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
    for chunk in &chunks {
        if conduit_truthy(chunk.get("complete")) {
            continue;
        }
        upload_one_chunk(client, data, file_phid, chunk, display)?;
    }
    Ok(())
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
                return Err(format!(
                    "file.allocate for \"{name}\" asked for an upload without a file PHID"
                ));
            }
            upload_missing_chunks(client, data, &phid, name)?;
            Ok(phid)
        }
        Err(_) => {
            // Old servers have no `file.allocate`; send the whole file at once.
            let result = client
                .call(
                    "file.upload",
                    &serde_json::json!({
                        "name": name,
                        "data_base64": base64_encode(data),
                    }),
                )
                .map_err(|e| format!("Unable to upload file data: {e}"))?;
            phid_of(&result).ok_or_else(|| {
                format!("file.upload for \"{name}\" returned an unexpected response")
            })
        }
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

/// Resolve uploaded PHIDs to display refs via `file.search`.
pub fn resolve_refs(
    client: &crate::conduit::Client,
    uploads: &[(String, String)],
    conduit_uri: Option<&str>,
) -> Result<Vec<UploadedRef>, String> {
    let phids: Vec<&str> = uploads.iter().map(|(_, phid)| phid.as_str()).collect();
    let result = client
        .call(
            "file.search",
            &serde_json::json!({"constraints": {"phids": phids}}),
        )
        .map_err(|e| e.to_string())?;
    let empty = Vec::new();
    let data = result
        .get("data")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut refs = Vec::with_capacity(uploads.len());
    for (argument, phid) in uploads {
        let hit = data
            .iter()
            .find(|h| h.get("phid").and_then(|v| v.as_str()) == Some(phid.as_str()))
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

fn upload_single_path(
    client: &crate::conduit::Client,
    dir: &Path,
    path: &str,
    temporary: bool,
) -> Result<(String, String), String> {
    let disk = dir.join(path);
    let data = match std::fs::read(&disk) {
        Ok(data) => data,
        Err(error) => return Err(format!("cannot read \"{path}\": {error}")),
    };
    let name = match disk.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => path.to_string(),
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

fn render_uploaded_refs(refs: &[UploadedRef], json: bool) -> Result<String, String> {
    if json {
        let list: Vec<serde_json::Value> = refs.iter().map(UploadedRef::to_json).collect();
        let mut text = match serde_json::to_string_pretty(&list) {
            Ok(text) => text,
            Err(error) => return Err(format!("failed to encode JSON: {error}")),
        };
        text.push('\n');
        return Ok(text);
    }
    let mut out = String::new();
    for item in refs {
        out.push_str(&format!(
            "F{id} {name}\n{uri}\n",
            id = item.id,
            name = item.name,
            uri = item.uri
        ));
    }
    Ok(out)
}

/// Upload `paths`, opening the uploaded files in a browser when `browse`.
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
    if browse {
        browse_uploaded_refs(dir, &refs)?;
    }
    render_uploaded_refs(&refs, json)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
    let base = resolve_conduit_uri(opts.conduit_uri.as_deref());
    upload_paths_with_browse(
        &client,
        dir,
        &opts.paths,
        opts.temporary,
        opts.json,
        opts.browse,
        base.as_deref(),
    )
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod upload_tests;
