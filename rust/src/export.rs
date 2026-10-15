//! `export` workflow: print local or Differential changes as a patch.
//!
//! PHP reference: `src/workflow/ArcanistExportWorkflow.php`.
//!
//! Local sources run `git diff` against a base commit (default `HEAD`) and
//! print the result: `--git` prints a full git diff, `--unified` a plain
//! unified diff. `--revision`/`--diff` download the raw diff over Conduit
//! (`differential.querydiffs`, then `differential.getrawdiff`).
//!
//! `--encoding` reinterprets non-UTF-8 patch bytes before printing,
//! mirroring `ArcanistBundle::convertNonUTF8Diff()`. `--arcbundle <path>`
//! writes a version-5 bundle tarball (`changes.json`, `meta.json`,
//! `hunks/`, `blobs/`), mirroring `ArcanistBundle::writeToDisk()`.
//!
//! Known gaps: hunk corpora are parsed as UTF-8 (non-UTF-8 bytes become
//! U+FFFD instead of staying raw); filemode properties and C-escaped
//! paths are dropped by the diff parser; JSON escapes differ from PHP's
//! `json_encode` (same parsed value).

use std::path::Path;
use std::process::ExitCode;

use crate::diff_local::{
    FILE_TEXT, TYPE_ADD, TYPE_COPY_AWAY, TYPE_COPY_HERE, TYPE_MOVE_AWAY, TYPE_MOVE_HERE,
};

pub const NAME: &str = "export";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} (--git | --unified | --arcbundle PATH) [BASE]\n\
         \x20      arc-rs {NAME} (--git | --unified | --arcbundle PATH) --revision ID\n\
         \x20      arc-rs {NAME} (--git | --unified | --arcbundle PATH) --diff ID\n\
         \n\
         Export the working copy changes against BASE (default: HEAD), or a\n\
         Differential revision/diff, to stdout as a patch, or to PATH as an\n\
         arc bundle.\n\
         \n\
         Options:\n\
         \x20 --git          export as a git patch (applies with `git apply`)\n\
         \x20 --unified      export as a plain unified patch (applies with `patch`)\n\
         \x20 --arcbundle PATH  export as an arc bundle written to PATH\n\
         \x20 --revision ID  export a Differential revision (D123 or 123)\n\
         \x20 --diff ID      export one Differential diff by numeric ID\n\
         \x20 --encoding ENCODING  convert a non-UTF-8 patch from ENCODING\n\
         \x20 --conduit-uri URI  conduit server (or ARC_CONDUIT_URI / .arcconfig)\n\
         \x20 -h, --help     show this help\n"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Git,
    Unified,
    Bundle { path: String },
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
    encoding: Option<String>,
    conduit_uri: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// `D12345` or `12345` to a numeric revision ID, mirroring PHP
/// `normalizeRevisionID()` (one leading `D`, either case, no trimming).
pub fn normalize_revision_id(text: &str) -> Result<u64, String> {
    let digits = text
        .strip_prefix('D')
        .or_else(|| text.strip_prefix('d'))
        .unwrap_or(text);
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
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse::<u64>()
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
    arcbundle: Option<String>,
    revision: Option<String>,
    diff: Option<String>,
    encoding: Option<String>,
    positional: Option<String>,
    conduit_uri: Option<String>,
    end_of_flags: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            git: false,
            unified: false,
            arcbundle: None,
            revision: None,
            diff: None,
            encoding: None,
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

    fn flag_encoding(
        &mut self,
        arg: &str,
        iter: &mut std::slice::Iter<'_, String>,
    ) -> Option<Step> {
        if arg == "--encoding" {
            match take_value(iter, "--encoding") {
                Ok(value) => self.encoding = Some(value),
                Err(outcome) => return Some(Step::Done(outcome)),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--encoding=") {
            self.encoding = Some(value.to_string());
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

    fn flag_arcbundle(
        &mut self,
        arg: &str,
        iter: &mut std::slice::Iter<'_, String>,
    ) -> Option<Step> {
        if arg == "--arcbundle" {
            match take_value(iter, "--arcbundle") {
                Ok(value) => self.arcbundle = Some(value),
                Err(outcome) => return Some(Step::Done(outcome)),
            }
            return Some(Step::Handled);
        }
        if let Some(value) = arg.strip_prefix("--arcbundle=") {
            self.arcbundle = Some(value.to_string());
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
        if let Some(step) = self.flag_encoding(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_arcbundle(arg, iter) {
            return step;
        }
        self.tail(arg)
    }
}

fn resolve_format(
    git: bool,
    unified: bool,
    arcbundle: Option<String>,
) -> Result<Format, ParseOutcome> {
    let requested = u32::from(git) + u32::from(unified) + u32::from(arcbundle.is_some());
    if requested == 0 {
        return Err(ParseOutcome::Error(
            "Specify one of '--git', '--unified' or '--arcbundle <path>' to choose an export format."
                .to_string(),
        ));
    }
    if requested > 1 {
        return Err(ParseOutcome::Error(
            "Options '--git', '--unified' and '--arcbundle' are not compatible. Choose exactly one export format."
                .to_string(),
        ));
    }
    if let Some(path) = arcbundle {
        return Ok(Format::Bundle { path });
    }
    if git {
        return Ok(Format::Git);
    }
    Ok(Format::Unified)
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
    let format = match resolve_format(parser.git, parser.unified, parser.arcbundle) {
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
        encoding: parser.encoding,
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

/// Local `git diff` of the working tree against `base` in `format`, as bytes.
pub fn local_diff_bytes(dir: &Path, format: Format, base: &str) -> Result<Vec<u8>, String> {
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
        // Bundles always diff in git format; `execute` never passes this.
        Format::Bundle { .. } => unreachable!("bundle output diffs in git format"),
    };
    let out = crate::git::output(dir, args)?;
    if out.status.success() {
        Ok(out.stdout)
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

/// Local `git diff` of the working tree against `base` in `format`.
pub fn local_diff(dir: &Path, format: Format, base: &str) -> Result<String, String> {
    Ok(crate::patch::decode_bytes(
        &local_diff_bytes(dir, format, base)?,
        None,
    ))
}

/// Decode patch bytes with `--encoding`, mirroring
/// `ArcanistBundle::convertNonUTF8Diff()`.
#[must_use]
pub fn decode_patch(bytes: &[u8], encoding: Option<&str>) -> String {
    crate::patch::decode_bytes(bytes, encoding)
}

/// Decode a Conduit raw diff with `--encoding` (a no-op without one).
#[must_use]
pub fn decode_conduit_text(text: &str, encoding: Option<&str>) -> String {
    match encoding {
        None => ensure_trailing_newline(text.to_string()),
        Some(name) => ensure_trailing_newline(decode_patch(text.as_bytes(), Some(name))),
    }
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Bundle metadata version, mirroring `writeToDisk()`.
pub const BUNDLE_VERSION: u64 = 5;

/// Added/del counts plus missing-newline flags for one hunk corpus,
/// simulating the PHP parser's length-driven loop (the corpus starts
/// with the `@@` header line).
fn hunk_stats(corpus: &str, old_len: u64, new_len: u64) -> (u64, u64, bool, bool) {
    let mut add = 0;
    let mut del = 0;
    let mut missing_old = false;
    let mut missing_new = false;
    let mut old_left = old_len;
    let mut new_left = new_len;
    let mut lines = corpus.lines();
    lines.next(); // @@ header
    for line in lines {
        if line.starts_with("\\ No newline at end of file") {
            if new_left > 0 {
                missing_old = true;
            } else {
                missing_new = true;
            }
            if new_left == 0 {
                break;
            }
            continue;
        }
        match line.as_bytes().first().copied().unwrap_or(b' ') {
            b'+' => {
                add += 1;
                new_left = new_left.saturating_sub(1);
            }
            b'-' => {
                if old_left == 0 {
                    break;
                }
                del += 1;
                old_left -= 1;
            }
            b' ' => {
                if old_left == 0 && new_left == 0 {
                    break;
                }
                old_left = old_left.saturating_sub(1);
                new_left = new_left.saturating_sub(1);
            }
            _ => break,
        }
    }
    (add, del, missing_old, missing_new)
}

/// One hunk in `toDictionary()` shape (corpus still inline).
fn hunk_dict(hunk: &crate::diff_local::ParsedHunk) -> serde_json::Value {
    let (add, del, missing_old, missing_new) = hunk_stats(&hunk.corpus, hunk.old_len, hunk.new_len);
    serde_json::json!({
        "oldOffset": hunk.old_start,
        "newOffset": hunk.new_start,
        "oldLength": hunk.old_len,
        "newLength": hunk.new_len,
        "addLines": add,
        "delLines": del,
        "isMissingOldNewline": missing_old,
        "isMissingNewNewline": missing_new,
        "corpus": hunk.corpus,
    })
}

/// One change in `toDictionary()` shape. Adds report `/dev/null` like the
/// PHP parser (which overwrites the seeded path from the `---` line);
/// properties stay empty (the diff parser drops filemodes).
fn change_dict(change: &crate::diff_local::ParsedChange) -> serde_json::Value {
    let old_path = if change.change_type == TYPE_ADD {
        "/dev/null"
    } else {
        change.old_path.as_str()
    };
    // Empty PHP arrays encode as `[]`, not `{}`.
    serde_json::json!({
        "metadata": [],
        "oldPath": old_path,
        "currentPath": change.current_path,
        "awayPaths": [],
        "oldProperties": [],
        "newProperties": [],
        "type": change.change_type,
        "fileType": change.file_type,
        "commitHash": change.commit_hash,
        "hunks": change.hunks.iter().map(hunk_dict).collect::<Vec<_>>(),
    })
}

/// The `MOVE_AWAY`/`COPY_AWAY` companion the PHP parser builds for
/// renames and copies (old path, no hunks, away path set).
fn move_companion(change: &crate::diff_local::ParsedChange) -> Option<serde_json::Value> {
    let away_type = match change.change_type {
        TYPE_MOVE_HERE => TYPE_MOVE_AWAY,
        TYPE_COPY_HERE => TYPE_COPY_AWAY,
        _ => return None,
    };
    if change.old_path.is_empty() || change.old_path == change.current_path {
        return None;
    }
    Some(serde_json::json!({
        "metadata": [],
        "oldPath": null,
        "currentPath": change.old_path,
        "awayPaths": [change.current_path],
        "oldProperties": [],
        "newProperties": [],
        "type": away_type,
        "fileType": FILE_TEXT,
        "commitHash": null,
        "hunks": [],
    }))
}

/// Local changes in `toDictionary()` shape, with rename companions.
pub fn changes_json(changes: &[crate::diff_local::ParsedChange]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for change in changes {
        out.push(change_dict(change));
        if let Some(companion) = move_companion(change) {
            out.push(companion);
        }
    }
    out
}

/// Replace inline hunk corpora with their numeric file IDs, collecting
/// the corpora in order, mirroring `writeToDisk()`.
pub fn strip_corpora(changes: &mut [serde_json::Value]) -> Vec<String> {
    let mut hunks = Vec::new();
    for change in changes {
        let Some(list) = change.get_mut("hunks").and_then(|h| h.as_array_mut()) else {
            continue;
        };
        for hunk in list {
            let corpus = hunk
                .get("corpus")
                .and_then(|corpus| corpus.as_str())
                .unwrap_or("")
                .to_string();
            hunks.push(corpus);
            hunk["corpus"] = serde_json::json!(hunks.len() - 1);
        }
    }
    hunks
}

/// Unique binary blob PHIDs referenced by change metadata.
pub fn blob_phids(changes: &[serde_json::Value]) -> Vec<String> {
    let mut phids: Vec<String> = Vec::new();
    for change in changes {
        let Some(meta) = change.get("metadata") else {
            continue;
        };
        for key in ["old:binary-phid", "new:binary-phid"] {
            let Some(phid) = meta.get(key).and_then(|v| v.as_str()) else {
                continue;
            };
            // PHP `empty()`: "" and "0" do not reference a blob.
            if !phid.is_empty() && phid != "0" && !phids.iter().any(|p| p == phid) {
                phids.push(phid.to_string());
            }
        }
    }
    phids
}

/// Download one blob via `file.download` (base64), mirroring `getBlob()`.
fn download_blob(client: &crate::conduit::Client, phid: &str) -> Result<Vec<u8>, String> {
    eprintln!("Downloading binary data...");
    let result = client
        .call("file.download", &serde_json::json!({"phid": phid}))
        .map_err(|e| e.to_string())?;
    let encoded = result
        .as_str()
        .ok_or_else(|| "file.download returned an unexpected response".to_string())?;
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("file.download returned invalid data: {e}"))
}

/// First row of a legacy Conduit response (list or id-keyed map).
fn first_row(result: &serde_json::Value) -> Option<&serde_json::Value> {
    match result {
        serde_json::Value::Array(items) => items.first(),
        serde_json::Value::Object(map) => map.values().next(),
        _ => None,
    }
}

/// `user.query` real name for the authenticated user.
fn viewer_real_name(client: &crate::conduit::Client) -> Result<String, String> {
    let who = client
        .call("user.whoami", &serde_json::json!({}))
        .map_err(|e| e.to_string())?;
    let phid = who
        .get("phid")
        .and_then(|phid| phid.as_str())
        .ok_or_else(|| "conduit user.whoami response has no phid".to_string())?;
    let users = client
        .call("user.query", &serde_json::json!({"phids": [phid]}))
        .map_err(|e| e.to_string())?;
    Ok(first_row(&users)
        .and_then(|row| row.get("realName"))
        .and_then(|name| name.as_str())
        .unwrap_or("")
        .to_string())
}

/// `meta.json` for a local export.
fn local_meta(
    base_revision: &str,
    encoding: Option<&str>,
    author_name: &str,
    author_email: &str,
) -> serde_json::Value {
    serde_json::json!({
        "version": BUNDLE_VERSION,
        "baseRevision": base_revision,
        "revisionID": null,
        "encoding": encoding,
        "authorName": author_name,
        "authorEmail": author_email,
    })
}

/// `meta.json` for a Conduit export (fields pass through untouched).
fn conduit_meta(diff: &serde_json::Value, encoding: Option<&str>) -> serde_json::Value {
    let field = |key: &str| diff.get(key).cloned().unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "version": BUNDLE_VERSION,
        "baseRevision": field("sourceControlBaseRevision"),
        "revisionID": field("revisionID"),
        "encoding": encoding,
        "authorName": field("authorName"),
        "authorEmail": field("authorEmail"),
    })
}

/// First diff dict of a `differential.querydiffs` response.
fn first_diff_dict(
    client: &crate::conduit::Client,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let result = client
        .call("differential.querydiffs", params)
        .map_err(|e| e.to_string())?;
    first_row(&result).cloned().ok_or_else(|| {
        "The diff or revision you specified is either invalid or you don't have permission to view it."
            .to_string()
    })
}

/// Fresh staging directory with `hunks/` and `blobs/` subdirectories.
fn stage_dir() -> Result<std::path::PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("arc-rs-export-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("hunks"))
        .and_then(|()| std::fs::create_dir_all(dir.join("blobs")))
        .map_err(|e| format!("failed to stage bundle files: {e}"))?;
    Ok(dir)
}

fn write_stage_file(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(dir.join(name), bytes).map_err(|e| format!("failed to stage bundle files: {e}"))
}

/// `tar -czf <out>` over the staged files (alphabetical, like `*`).
fn create_tar(stage: &Path, out: &Path) -> Result<(), String> {
    let output = std::process::Command::new("tar")
        .current_dir(stage)
        .args([
            "-czf",
            &out.to_string_lossy(),
            "blobs",
            "changes.json",
            "hunks",
            "meta.json",
        ])
        .output()
        .map_err(|e| format!("failed to run tar: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let trimmed = detail.trim();
        if trimmed.is_empty() {
            Err("tar -czf failed".to_string())
        } else {
            Err(trimmed.to_string())
        }
    }
}

/// Stage `changes.json`, `meta.json`, hunks, and blobs, then tar them.
fn write_bundle(
    out: &Path,
    changes: &[serde_json::Value],
    meta: &serde_json::Value,
    hunks: &[String],
    blobs: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let stage = stage_dir()?;
    let result = (|| {
        write_stage_file(
            &stage,
            "changes.json",
            serde_json::to_string(changes)
                .map_err(|e| format!("failed to encode bundle: {e}"))?
                .as_bytes(),
        )?;
        write_stage_file(
            &stage,
            "meta.json",
            serde_json::to_string(meta)
                .map_err(|e| format!("failed to encode bundle: {e}"))?
                .as_bytes(),
        )?;
        for (index, corpus) in hunks.iter().enumerate() {
            write_stage_file(&stage, &format!("hunks/{index}"), corpus.as_bytes())?;
        }
        for (phid, blob) in blobs {
            write_stage_file(&stage, &format!("blobs/{phid}"), blob)?;
        }
        create_tar(&stage, out)
    })();
    std::fs::remove_dir_all(&stage).ok();
    result
}

/// Resolve the bundle output path against `dir` when relative.
fn bundle_out_path(dir: &Path, path: &str) -> std::path::PathBuf {
    let out = Path::new(path);
    if out.is_absolute() {
        out.to_path_buf()
    } else {
        dir.join(out)
    }
}

/// Local working-copy changes as bundle parts.
fn local_bundle_parts(
    dir: &Path,
    base: &str,
    encoding: Option<&str>,
    client: &crate::conduit::Client,
) -> Result<(Vec<serde_json::Value>, serde_json::Value), String> {
    let bytes = local_diff_bytes(dir, Format::Git, base)?;
    let parsed = crate::diff_local::parse_unified_diff(&String::from_utf8_lossy(&bytes));
    let changes = changes_json(&parsed);
    let base_revision = crate::git::stdout_text(dir, &["rev-parse", base])?;
    let author_name = viewer_real_name(client)?;
    let author_email = crate::git::stdout_text(dir, &["config", "user.email"])?;
    let meta = local_meta(&base_revision, encoding, &author_name, &author_email);
    Ok((changes, meta))
}

/// Conduit revision/diff changes as bundle parts.
fn conduit_bundle_parts(
    params: &serde_json::Value,
    encoding: Option<&str>,
    client: &crate::conduit::Client,
) -> Result<(Vec<serde_json::Value>, serde_json::Value), String> {
    let diff = first_diff_dict(client, params)?;
    let changes: Vec<serde_json::Value> = diff
        .get("changes")
        .and_then(|changes| changes.as_array())
        .cloned()
        .unwrap_or_default();
    let meta = conduit_meta(&diff, encoding);
    Ok((changes, meta))
}

/// Write an `--arcbundle` file with an existing client.
fn export_bundle_with_client(
    dir: &Path,
    opts: &Options,
    path: &str,
    client: &crate::conduit::Client,
) -> Result<String, String> {
    let encoding = opts.encoding.as_deref().filter(|e| !e.is_empty());
    let (mut changes, meta) = match &opts.source {
        Source::Local { base } => {
            let base = base.as_deref().unwrap_or("HEAD");
            local_bundle_parts(dir, base, encoding, client)?
        }
        Source::Revision(id) => {
            conduit_bundle_parts(&serde_json::json!({"revisionIDs": [id]}), encoding, client)?
        }
        Source::Diff(id) => {
            conduit_bundle_parts(&serde_json::json!({"ids": [id]}), encoding, client)?
        }
    };
    let hunks = strip_corpora(&mut changes);
    let mut blobs = Vec::new();
    for phid in blob_phids(&changes) {
        blobs.push((phid.clone(), download_blob(client, &phid)?));
    }
    let out = bundle_out_path(dir, path);
    write_bundle(&out, &changes, &meta, &hunks, &blobs)?;
    Ok(format!("Writing bundle to '{path}'...\nDone.\n"))
}

/// Write an `--arcbundle` file, returning the stdout text.
fn export_bundle(dir: &Path, opts: &Options, path: &str) -> Result<String, String> {
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
    export_bundle_with_client(dir, opts, path, &client)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    if let Format::Bundle { path } = &opts.format {
        return export_bundle(dir, &opts, path);
    }
    match opts.source {
        Source::Local { base } => {
            let base = base.as_deref().unwrap_or("HEAD");
            let bytes = local_diff_bytes(dir, opts.format, base)?;
            Ok(decode_patch(&bytes, opts.encoding.as_deref()))
        }
        Source::Revision(id) => {
            let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
            fetch_raw_diff_for_revision(&client, id)
                .map(|text| decode_conduit_text(&text, opts.encoding.as_deref()))
        }
        Source::Diff(id) => {
            let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
            fetch_raw_diff_for_diff(&client, &id.to_string())
                .map(|text| decode_conduit_text(&text, opts.encoding.as_deref()))
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
