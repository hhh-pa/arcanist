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
//! Known gaps: filemode properties and C-escaped paths are dropped by
//! the diff parser; JSON escapes differ from PHP's `json_encode` (same
//! parsed value).

use std::path::Path;
use std::process::ExitCode;

use crate::conduit::{self, EnvelopeKind};
use crate::diff_local::TYPE_ADD;

pub const NAME: &str = "export";

/// T1153 BROWSE-B23: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`
/// over `newLegacyPhutilWorkflow()`), not a short custom usage string.
/// `export` is a classic workflow, so the examples fall back to the
/// `PhutilArgumentWorkflow` default while the synopsis and help come
/// from `getCommandSynopses()`/`getCommandHelp()`. T1153 RB-D5: the
/// renderer is the shared `crate::help` module; this file keeps only
/// the data.
const HELP_EXAMPLES: &str = "**export**";
const HELP_SYNOPSIS: &str = "\
     \x20     **export** [__paths__] __format__ (svn)\n\
     \x20     **export** [__commit_range__] __format__ (git, hg)\n\
     \x20     **export** __--revision__ __revision_id__ __format__\n\
     \x20     **export** __--diff__ __diff_id__ __format__";
const HELP_TEXT: &str = "\
     \x20         Supports: svn, git, hg\n\
     \x20         Export the local changeset (or a Differential changeset) to a file,\n\
     \x20         in some __format__: git diff (__--git__), unified diff\n\
     \x20         (__--unified__), or arc bundle (__--arcbundle__ __path__) format.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "arcbundle",
        None,
        Some("file"),
        "Export change as an arc bundle. This format can represent all changes. These bundles can be applied with 'arc patch'.",
    ),
    (
        "diff",
        None,
        Some("diff_id"),
        "Instead of exporting changes from the working copy, export them from a Differential diff.",
    ),
    (
        "encoding",
        None,
        Some("encoding"),
        "Attempt to convert non UTF-8 patch into specified encoding.",
    ),
    (
        "git",
        None,
        None,
        "Export change as a git patch. This format is more complete than unified, but less complete than arc bundles. These patches can be applied with 'git apply' or 'arc patch'.",
    ),
    (
        "revision",
        None,
        Some("revision_id"),
        "Instead of exporting changes from the working copy, export them from a Differential revision.",
    ),
    (
        "unified",
        None,
        None,
        "Export change as a unified patch. This format is less complete than git patches or arc bundles. These patches can be applied with 'patch' or 'arc patch'.",
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
        false,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(crate::terminal::ansi_enabled())
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
    Local {
        base: Option<String>,
    },
    Revision(u64),
    Diff(u64),
    /// Non-numeric `--revision` (one `D`/`d` stripped): sent raw, the
    /// server errors (T1153 EXPORT-E25).
    RevisionRaw(String),
    /// Non-numeric `--diff`: sent raw, the server errors (T1153 EXPORT-E25).
    DiffRaw(String),
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
    positionals: Vec<String>,
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
            positionals: Vec::new(),
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
        // T1153 EXPORT-E27: positionals are collected, not validated here:
        // conduit sources ignore them entirely (PHP only calls
        // `parseBaseCommitArgument()` for local sources), and the
        // multi-positional error applies to local sources only.
        self.positionals.push(arg.to_string());
        Step::Handled
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

/// Strip one leading `D`/`d`, like `normalizeRevisionID()`
/// (`preg_replace('/^D/i', '', $id)`).
fn strip_revision_prefix(value: &str) -> &str {
    value
        .strip_prefix('D')
        .or_else(|| value.strip_prefix('d'))
        .unwrap_or(value)
}

fn resolve_source(
    revision: Option<String>,
    diff: Option<String>,
    positionals: Vec<String>,
) -> Result<Source, ParseOutcome> {
    // T1153 EXPORT-E27: positionals with `--revision`/`--diff` are
    // silently ignored (PHP never reads `paths` for conduit sources).
    if let Some(value) = revision {
        // T1153 EXPORT-E25: one `D` stripped, then sent raw (numeric or
        // not); the server errors on garbage.
        let stripped = strip_revision_prefix(&value);
        if let Ok(id) = normalize_revision_id(&value) {
            return Ok(Source::Revision(id));
        }
        return Ok(Source::RevisionRaw(stripped.to_string()));
    }
    if let Some(value) = diff {
        if let Ok(id) = parse_diff_id(&value) {
            return Ok(Source::Diff(id));
        }
        return Ok(Source::DiffRaw(value));
    }
    // T1153 R4 (EXPORT-E28): PHP's `parseBaseCommitArgument()`
    // (`ArcanistWorkflow.php:1834-1840`) throws `ArcanistUsageException`,
    // rendered `Usage Exception:` with exit 1 (the "exit" half of E28
    // already matches; the "255" reading does not reproduce — `export`
    // is a subprocess workflow). Local sources only (E27).
    if positionals.len() > 1 {
        return Err(ParseOutcome::Error(
            "Specify exactly one base commit. The end of the commit range is always the working copy state."
                .to_string(),
        ));
    }
    Ok(Source::Local {
        base: positionals.into_iter().next(),
    })
}

fn finish(parser: Parser) -> ParseOutcome {
    // T1153 EXPORT-E29: PHP's `didParseArguments()` checks the
    // source conflict before the format selection.
    if parser.revision.is_some() && parser.diff.is_some() {
        return ParseOutcome::Error(
            "Options '--revision' and '--diff' are not compatible. Choose exactly one change source."
                .to_string(),
        );
    }
    let format = match resolve_format(parser.git, parser.unified, parser.arcbundle) {
        Ok(format) => format,
        Err(outcome) => return outcome,
    };
    let source = match resolve_source(parser.revision, parser.diff, parser.positionals) {
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

/// The `loadBundleFromConduit()` not-found text, wrapped like PHP's
/// `phutil_console_wrap()` with no indent (T1153 EXPORT-E31; the "255"
/// half does not reproduce — `export` is a subprocess workflow, exit 1).
fn not_found_message() -> String {
    crate::terminal::console_wrap(
        "The diff or revision you specified is either invalid or you don't have permission to view it.",
        0,
        true,
    )
}

/// First diff ID in a `differential.querydiffs` result (a dict keyed by diff
/// ID, or a list).
fn first_diff_id(result: &serde_json::Value) -> Result<String, String> {
    let missing = not_found_message;
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
            // T1153 EXPORT-E26: PHP sends string IDs on the wire.
            &serde_json::json!({"revisionIDs": [revision.to_string()]}),
        )
        .map_err(|e| e.to_string())?;
    let diff_id = first_diff_id(&result)?;
    fetch_raw_diff_for_diff(client, &diff_id)
}

/// The `differential.querydiffs` params for a conduit [`Source`]: PHP sends
/// the raw (string) IDs (T1153 EXPORT-E25/E26).
fn querydiffs_params(source: &Source) -> serde_json::Value {
    match source {
        Source::Revision(id) => serde_json::json!({"revisionIDs": [id.to_string()]}),
        Source::RevisionRaw(raw) => serde_json::json!({"revisionIDs": [raw]}),
        Source::Diff(id) => serde_json::json!({"ids": [id.to_string()]}),
        Source::DiffRaw(raw) => serde_json::json!({"ids": [raw]}),
        Source::Local { .. } => serde_json::json!({}),
    }
}

/// Local `git diff` of the working tree against `base` in `format`, as bytes.
/// Resolve `HEAD`, blaming it with `resolveCommit()`'s text when missing
/// (empty repositories).
fn resolve_export_head(dir: &Path) -> Result<String, String> {
    crate::git::stdout_text(dir, &["rev-parse", "HEAD"])
        .map_err(|_| "Unable to find any git commit named 'HEAD' in this repository.".to_string())
}

std::thread_local! {
    /// Set when the base-commit prompt is reached without an interactive
    /// stdin: PHP throws `PhutilConsoleStdinNotInteractiveException` there,
    /// before any fallback.
    static EXPORT_PROMPT_TTY_FAILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The interactive base-commit prompt for [`crate::which::resolve_base`]:
/// echo the prompt, then read one answer line on a TTY.
fn export_prompt_line(prompt: &str) -> Option<String> {
    print!("{}", crate::prompt::prompt_text(prompt));
    use std::io::Write;
    std::io::stdout().flush().ok();
    if !crate::prompt::stdin_is_terminal() {
        EXPORT_PROMPT_TTY_FAILED.set(true);
        return None;
    }
    crate::prompt::read_answer(&mut std::io::stdin().lock())
}

/// Resolve the local base commit: an explicit base goes through
/// `merge-base(base, HEAD)` (T1153 EXPORT-E3), otherwise the full default
/// chain (rules, `base` config, `default-relative-commit`, upstream,
/// scratch, interactive prompt; T1153 EXPORT-E1), via
/// [`crate::which::resolve_base`].
fn resolve_export_base(dir: &Path, explicit: Option<&str>) -> Result<String, String> {
    let user_file = crate::config::arcrc_path();
    resolve_export_base_with_prompt(dir, explicit, user_file.as_deref(), &export_prompt_line)
}

fn resolve_export_base_with_prompt(
    dir: &Path,
    explicit: Option<&str>,
    user_file: Option<&Path>,
    prompt: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    // PHP finds the repository API before parsing the base
    // (`getRepositoryAPI()` precedes `parseBaseCommitArgument()`).
    if !crate::git::is_inside_work_tree(dir) {
        return Err("not inside a git working copy".to_string());
    }
    // PHP resolves the head before the merge-base (`getHeadCommit()`
    // inside `buildBaseCommit()`), so empty repositories blame `HEAD`.
    resolve_export_head(dir)?;
    // T1153 RB-D38: `resolve_base` takes the merged config so the
    // `base` gate sees `--config` runtime values like
    // `getConfigFromAnySource()` does.
    let loaded =
        crate::get_config::load_merged(dir, user_file, &crate::get_config::LoadOptions::default())?;
    let inputs = crate::which::BaseInputs {
        runtime_rules: "",
        prompt,
    };
    let outcome = crate::which::resolve_base(dir, explicit, "HEAD", false, &loaded, &inputs);
    // A prompt reached without a TTY throws before any fallback.
    if EXPORT_PROMPT_TTY_FAILED.take() {
        return Err(crate::prompt::NOT_INTERACTIVE.to_string());
    }
    outcome.map(|(base, _explanation)| base)
}

pub fn local_diff_bytes(dir: &Path, format: Format, base: &str) -> Result<Vec<u8>, String> {
    if !crate::git::is_inside_work_tree(dir) {
        return Err("not inside a git working copy".to_string());
    }
    // Both text formats diff in git format now: the committed `base HEAD`
    // range (`getFullGitDiff($base, $head)`, T1153 EXPORT-E2) with the
    // pinned config and diff options (T1153 EXPORT-E36). `--unified`
    // renders from the bundle (T1153 EXPORT-E5).
    let _ = format;
    let head = resolve_export_head(dir)?;
    let out = crate::git::output(
        dir,
        &[
            "-c",
            "diff.suppressBlankEmpty=false",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--submodule=short",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U32767",
            "-M",
            "-C",
            base,
            &head,
            "--",
        ],
    )?;

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

/// Local `git diff` of the committed `base HEAD` range.
pub fn local_diff(dir: &Path, format: Format, base: &str) -> Result<String, String> {
    crate::patch::decode_bytes(&local_diff_bytes(dir, format, base)?, None)
}

/// Reject an empty local diff like `ArcanistDiffParser::parseDiff()`,
/// which throws on `!strlen(trim($diff))`
/// (`src/parser/ArcanistDiffParser.php:189-192`). Conduit sources never
/// pass through `parseDiff`, so only local call sites check.
fn reject_empty_diff(bytes: &[u8]) -> Result<(), String> {
    let blank = bytes
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\0' | 0x0B));
    if blank {
        // T1153 R4 (EXPORT-E7): PHP throws a generic `Exception`,
        // rendered under the red banner with exit 1 — the "255"
        // reading does not reproduce (`export` is a subprocess
        // workflow), but erroring (vs printing `''`/an empty bundle)
        // does.
        return Err("Can't parse an empty diff!".to_string());
    }
    Ok(())
}

/// Decode patch bytes with `--encoding` (input direction: foreign bytes
/// into UTF-8).
pub fn decode_patch(bytes: &[u8], encoding: Option<&str>) -> Result<String, String> {
    crate::patch::decode_bytes(bytes, encoding)
}

/// Normalize a Conduit raw diff (trailing newline). Conduit text arrives
/// UTF-8; `--encoding` output conversion happens at the stdout sink
/// ([`execute_bytes`]), like `convertNonUTF8Diff()` at render.
#[must_use]
pub fn decode_conduit_text(text: &str) -> String {
    ensure_trailing_newline(text.to_string())
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Bundle metadata version, mirroring `writeToDisk()`.
pub const BUNDLE_VERSION: u64 = 5;

/// One hunk length in `toDictionary()` shape (T1153 EXPORT-E11): the
/// spelled string when the header carried `,length`, else the integer
/// the parser substitutes (`ArcanistDiffParser.php:922-933`).
fn hunk_len_dict(length: u64, explicit: bool) -> serde_json::Value {
    if explicit {
        serde_json::Value::String(length.to_string())
    } else {
        serde_json::json!(length)
    }
}

/// One hunk in `toDictionary()` shape (corpus still inline). Offsets are
/// strings and lengths strings-when-explicit (T1153 EXPORT-E11:
/// `ArcanistDiffParser.php:919-933`); counts and missing-newline flags
/// come from the shared length-driven loop (T1153 EXPORT-E15).
fn hunk_dict(
    hunk: &crate::diff_local::ParsedHunk,
    explicit: &crate::diff_local::HunkLenExplicit,
) -> serde_json::Value {
    // Counts honor the synthetic pre-rewrite override (T1153 EXPORT-E20).
    let (add, del) = crate::diff_local::hunk_counts(hunk);
    let (_, _, missing_old, missing_new) =
        crate::diff_local::hunk_stats(&hunk.corpus, hunk.old_len, hunk.new_len);
    serde_json::json!({
        "oldOffset": hunk.old_start.to_string(),
        "newOffset": hunk.new_start.to_string(),
        "oldLength": hunk_len_dict(hunk.old_len, explicit.old_explicit),
        "newLength": hunk_len_dict(hunk.new_len, explicit.new_explicit),
        "addLines": add,
        "delLines": del,
        "isMissingOldNewline": missing_old,
        "isMissingNewNewline": missing_new,
        "corpus": hunk.corpus,
    })
}

/// One `unix:filemode` property dict (T1153 EXPORT-E13): PHP sets the
/// property from the diff's mode lines
/// (`ArcanistDiffParser.php:622-627`) and `toDictionary()` passes it
/// through; absent modes stay `[]`
/// (empty PHP arrays encode as `[]`).
fn filemode_dict(mode: Option<&str>) -> serde_json::Value {
    match mode {
        Some(mode) => serde_json::json!({"unix:filemode": mode}),
        None => serde_json::json!([]),
    }
}

/// One change in `toDictionary()` shape. Adds report `/dev/null` like the
/// PHP parser (which overwrites the seeded path from the `---` line);
/// move/copy-away companions report a null old path (PHP never sets it).
fn change_dict(change: &crate::diff_local::ParsedChange) -> serde_json::Value {
    let old_path = if change.change_type == TYPE_ADD {
        serde_json::Value::String("/dev/null".to_string())
    } else if crate::diff_local::is_companion_type(change.change_type) {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(change.old_path.clone())
    };
    // `hunk_len_explicit` runs parallel to `hunks` (EXPORT-E11).
    let hunks: Vec<serde_json::Value> = change
        .hunks
        .iter()
        .zip(change.hunk_len_explicit.iter())
        .map(|(hunk, explicit)| hunk_dict(hunk, explicit))
        .collect();
    // Empty PHP arrays encode as `[]`, not `{}`.
    serde_json::json!({
        "metadata": [],
        "oldPath": old_path,
        "currentPath": change.current_path,
        "awayPaths": change.away_paths,
        "oldProperties": filemode_dict(change.old_mode.as_deref()),
        "newProperties": filemode_dict(change.new_mode.as_deref()),
        "type": change.change_type,
        "fileType": change.file_type,
        "commitHash": change.commit_hash,
        "hunks": hunks,
    })
}

/// Local changes in `toDictionary()` shape, with folded rename
/// companions (T1153 EXPORT-E14: shared [`with_companions`], so
/// repeats from one old path fold to one away/multicopy change like
/// PHP's keyed `buildChange($oldPath)`).
pub fn changes_json(changes: &[crate::diff_local::ParsedChange]) -> Vec<serde_json::Value> {
    crate::diff_local::with_companions(changes)
        .iter()
        .map(change_dict)
        .collect()
}

/// PHP `nonempty($value, array())` for change-dict lists: missing and
/// empty-ish values (`null`, `""`, `"0"`, `false`, `0`, `[]`, `{}`) become
/// `[]`, like `ArcanistDiffChange::newFromDictionary()`.
fn nonempty_or_empty_array(value: Option<&serde_json::Value>) -> serde_json::Value {
    let empty = match value {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::Bool(flag)) => !flag,
        Some(serde_json::Value::Number(number)) => {
            number.as_i64() == Some(0) || number.as_u64() == Some(0) || number.as_f64() == Some(0.0)
        }
        Some(serde_json::Value::String(text)) => text.is_empty() || text == "0",
        Some(serde_json::Value::Array(items)) => items.is_empty(),
        Some(serde_json::Value::Object(map)) => map.is_empty(),
    };
    if empty {
        serde_json::json!([])
    } else {
        value.cloned().unwrap_or(serde_json::Value::Null)
    }
}

/// One conduit hunk dict in `toDictionary()` shape: the 9 keys, corpus
/// stringified (`(string)$corpus`).
fn normalize_conduit_hunk(hunk: &serde_json::Value) -> serde_json::Value {
    let corpus = match hunk.get("corpus") {
        Some(serde_json::Value::String(text)) => text.clone(),
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::Number(_)) | Some(serde_json::Value::Bool(_)) => hunk
            .get("corpus")
            .map(|v| v.to_string())
            .unwrap_or_default(),
        Some(_) => String::new(),
    };
    let field = |key: &str| hunk.get(key).cloned().unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "oldOffset": field("oldOffset"),
        "newOffset": field("newOffset"),
        "oldLength": field("oldLength"),
        "newLength": field("newLength"),
        "addLines": field("addLines"),
        "delLines": field("delLines"),
        "isMissingOldNewline": field("isMissingOldNewline"),
        "isMissingNewNewline": field("isMissingNewNewline"),
        "corpus": corpus,
    })
}

/// Escape every string in a conduit value tree (T1153 EXPORT-E9),
/// so `write_bundle`'s un-escaping restores them byte-exactly.
/// Marker-free values pass through unchanged.
fn escape_json_strings(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(crate::patch::escape_transport_markers(text))
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(escape_json_strings).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, val)| (key.clone(), escape_json_strings(val)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Restore every string in a `changes.json` value tree (T1153
/// EXPORT-E9): local entries ride as transport and conduit entries
/// escaped at normalize, so all restore here. Byte-exact for
/// escape-free trees; undecodable paths render lossily (PHP's
/// `json_encode` fails on those instead — exotic, Linux-only
/// filenames).
fn untransport_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(crate::patch::untransport_lossy(text))
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(untransport_json_value).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, val)| (key.clone(), untransport_json_value(val)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Normalize one conduit `changes[]` dict through
/// `newFromDictionary()` + `toDictionary()` (T1153 EXPORT-E17): exactly
/// the 10 keys (extras like `id` dropped), values passing through.
/// (Key byte order stays `BTreeMap`-sorted like the local path; PHP's
/// insertion order needs the R7 `preserve_order` change, out of scope.)
fn normalize_conduit_change(change: &serde_json::Value) -> serde_json::Value {
    let field = |key: &str| change.get(key).cloned().unwrap_or(serde_json::Value::Null);
    let hunks: Vec<serde_json::Value> = change
        .get("hunks")
        .and_then(|h| h.as_array())
        .map(|items| items.iter().map(normalize_conduit_hunk).collect())
        .unwrap_or_default();
    escape_json_strings(&serde_json::json!({
        "metadata": field("metadata"),
        "oldPath": field("oldPath"),
        "currentPath": field("currentPath"),
        "awayPaths": nonempty_or_empty_array(change.get("awayPaths")),
        "oldProperties": nonempty_or_empty_array(change.get("oldProperties")),
        "newProperties": nonempty_or_empty_array(change.get("newProperties")),
        "type": field("type"),
        "fileType": field("fileType"),
        "commitHash": field("commitHash"),
        "hunks": hunks,
    }))
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

/// Lax base64 decode, like PHP `base64_decode()` without `$strict`
/// (T1153 EXPORT-E35): non-alphabet bytes are skipped and padding repaired;
/// undecodable input yields empty bytes (PHP returns false there, which
/// `writeFile()` stores as `""`).
fn lax_base64_decode(input: &str) -> Vec<u8> {
    use base64::Engine;
    let clean: String = input
        .bytes()
        .filter(|byte| {
            matches!(
                byte,
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/'
            )
        })
        .map(|byte| byte as char)
        .collect();
    let mut padded = clean;
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(&padded)
        .unwrap_or_default()
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
    Ok(lax_base64_decode(encoded))
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

/// Split `Name <addr>` into display name + address, mirroring
/// `PhutilEmailAddress` (T1153 EXPORT-E32): a trailing `<...>` (greedy
/// display, quote/space-trimmed; address split at the first `@`) wins,
/// else a bare `local@domain` split at the last `@` (no display name),
/// else the whole string as the local part.
fn split_email_author(author: &str) -> (Option<String>, String) {
    let trimmed = author.trim();
    if let Some(inner) = trimmed.strip_suffix('>')
        && let Some(at) = inner.rfind('<')
    {
        let display = inner[..at].trim_matches(['\'', '"', ' ']).to_string();
        let addr = &inner[at + 1..];
        if let Some((local, domain)) = addr.split_once('@') {
            let mut address = local.to_string();
            if !domain.is_empty() {
                address.push('@');
                address.push_str(domain);
            }
            return (Some(display), address);
        }
        return (Some(display), addr.to_string());
    }
    if let Some((local, domain)) = trimmed.rsplit_once('@') {
        let mut address = local.to_string();
        if !domain.is_empty() {
            address.push('@');
            address.push_str(domain);
        }
        return (None, address);
    }
    (None, trimmed.to_string())
}

/// `meta.json` for a local export.
fn local_meta(
    base_revision: &str,
    encoding: Option<&str>,
    author_name: Option<&str>,
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
    first_row(&result).cloned().ok_or_else(not_found_message)
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
    let output = crate::proc_env::command("tar")
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
/// The JSON files use PHP `json_encode()` escaping (`\/` + `\uXXXX`,
/// T1153 EXPORT-E18).
fn write_bundle(
    out: &Path,
    changes: &[serde_json::Value],
    meta: &serde_json::Value,
    hunks: &[String],
    blobs: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let stage = stage_dir()?;
    let result = (|| {
        let plain: Vec<serde_json::Value> = changes.iter().map(untransport_json_value).collect();
        let changes_text =
            serde_json::to_string(&plain).map_err(|e| format!("failed to encode bundle: {e}"))?;
        write_stage_file(
            &stage,
            "changes.json",
            crate::upload::php_escape_json_text(&changes_text, true).as_bytes(),
        )?;
        let meta_text =
            serde_json::to_string(meta).map_err(|e| format!("failed to encode bundle: {e}"))?;
        write_stage_file(
            &stage,
            "meta.json",
            crate::upload::php_escape_json_text(&meta_text, true).as_bytes(),
        )?;
        for (index, corpus) in hunks.iter().enumerate() {
            // Corpora ride as transport; members carry raw bytes, like PHP.
            let raw = crate::patch::untransport_bytes(corpus);
            write_stage_file(&stage, &format!("hunks/{index}"), &raw)?;
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

/// Local working-copy changes as bundle parts. `base` is the resolved
/// base commit.
fn local_bundle_parts(
    dir: &Path,
    base: &str,
    encoding: Option<&str>,
    client: &crate::conduit::Client,
) -> Result<(Vec<serde_json::Value>, serde_json::Value), String> {
    let bytes = local_diff_bytes(dir, Format::Git, base)?;
    reject_empty_diff(&bytes)?;
    let parsed =
        crate::diff_local::parse_unified_diff_transport(&crate::patch::transport_bytes(&bytes))?;
    // T1153 EXPORT-E20: PHP's local export parses with a repository API
    // (`ArcanistExportWorkflow.php:187`), so `loadSyntheticData()`
    // fabricates pure-move hunks for local bundles too.
    let mut full = crate::diff_local::with_companions(&parsed);
    let raw_for = |path: &str| raw_diff_transport(dir, base, path);
    crate::diff_local::attach_synthetic_git_hunks_transport(&mut full, &raw_for)?;
    let changes = changes_json(&full);
    let base_revision = crate::git::stdout_text(dir, &["rev-parse", base])?;
    // PHP builds `sprintf('%s <%s>', $realName, $email)` and parses it back
    // with `PhutilEmailAddress` (T1153 EXPORT-E32).
    let author_name = viewer_real_name(client)?;
    let author_email = crate::git::stdout_text(dir, &["config", "user.email"])?;
    let (display, address) = split_email_author(&format!("{author_name} <{author_email}>"));
    let meta = local_meta(&base_revision, encoding, display.as_deref(), &address);
    Ok((changes, meta))
}

/// Re-diff one path as transport (T1153 EXPORT-E9): the path arrives
/// as transport (view it lossily for the CLI) and the output rides as
/// transport.
fn raw_diff_transport(dir: &Path, base: &str, path: &str) -> Result<String, String> {
    let plain = crate::patch::untransport_lossy(path);
    let bytes = crate::diff_local::get_raw_diff_bytes(dir, base, &plain)?;
    Ok(crate::patch::transport_bytes(&bytes))
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
        .map(|items| items.iter().map(normalize_conduit_change).collect())
        .unwrap_or_default();
    let meta = conduit_meta(&diff, encoding);
    Ok((changes, meta))
}

/// File bytes of `path` at `revision` (`getFileDataAtRevision()`),
/// `None` when missing (empty path or no blob).
fn file_data_at_revision(dir: &Path, revision: &str, path: &str) -> Option<Vec<u8>> {
    if path.is_empty() {
        return None;
    }
    let spec = format!("{revision}:{path}");
    let out = crate::git::output(dir, &["show", &spec]).ok()?;
    out.status.success().then_some(out.stdout)
}

/// Batch `git ls-tree` mode lookup (`-z`, unquoted names): path to
/// `unix:filemode`. Missing paths (added/deleted sides) are absent.
/// Failures yield no modes (the render falls back to `100644`).
fn local_file_modes(
    dir: &Path,
    revision: &str,
    paths: &[String],
) -> std::collections::BTreeMap<String, String> {
    let mut modes = std::collections::BTreeMap::new();
    // Chunked: huge diffs would overflow the command line otherwise.
    for chunk in paths.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let mut args: Vec<&str> = vec!["ls-tree", "-z", revision, "--"];
        args.extend(chunk.iter().map(String::as_str));
        let Ok(out) = crate::git::output(dir, &args) else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        for entry in out.stdout.split(|byte| *byte == 0) {
            // `100644 blob <sha>\t<name>`.
            let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
                continue;
            };
            let (meta, name) = entry.split_at(tab);
            let mut meta = meta.split(|byte| *byte == b' ');
            let (Some(mode), Some(_kind)) = (meta.next(), meta.next()) else {
                continue;
            };
            if let (Ok(mode), Ok(name)) =
                (std::str::from_utf8(mode), std::str::from_utf8(&name[1..]))
            {
                modes.insert(name.to_string(), mode.to_string());
            }
        }
    }
    modes
}

/// Convert parsed local changes for rendering. Modes come from `ls-tree`
/// at the base/head (what PHP's parser reads off git's own mode lines):
/// without them, deletes render without `deleted file mode` and no
/// longer apply. Binary changes additionally attach base/head file data
/// like `loadSyntheticData()` (T1153 EXPORT-E19).
fn local_render_changes(
    dir: &Path,
    base: &str,
    head: &str,
    parsed: &[crate::diff_local::ParsedChange],
) -> Vec<crate::patch::BundleChange> {
    // T1153 OLD-PATH: ADD old paths are `/dev/null` like the parser;
    // they carry no mode and never hit the filesystem.
    // Parsed paths ride as transport (T1153 EXPORT-E9); the
    // filesystem, CLI, and mode-map lookups below need the lossy view.
    // Render fields keep transport (verbatim into the output).
    let old_paths: Vec<String> = parsed
        .iter()
        .map(|change| crate::patch::untransport_lossy(&change.old_path))
        .filter(|path| !path.is_empty() && path != "/dev/null")
        .collect();
    let new_paths: Vec<String> = parsed
        .iter()
        .map(|change| crate::patch::untransport_lossy(&change.current_path))
        .filter(|path| !path.is_empty())
        .collect();
    let old_modes = local_file_modes(dir, base, &old_paths);
    let new_modes = local_file_modes(dir, head, &new_paths);
    parsed
        .iter()
        .map(|parsed| {
            let old_plain = crate::patch::untransport_lossy(&parsed.old_path);
            let current_plain = crate::patch::untransport_lossy(&parsed.current_path);
            let mut change = crate::patch::BundleChange {
                change_type: parsed.change_type,
                file_type: parsed.file_type,
                // T1153 OLD-PATH: `/dev/null` (ADD) maps to no old
                // path for rendering, like empty.
                old_path: if parsed.old_path.is_empty() || parsed.old_path == "/dev/null" {
                    None
                } else {
                    Some(parsed.old_path.clone())
                },
                current_path: if parsed.current_path.is_empty() {
                    None
                } else {
                    Some(parsed.current_path.clone())
                },
                old_mode: old_modes.get(&old_plain).cloned(),
                new_mode: new_modes.get(&current_plain).cloned(),
                hunks: parsed.hunks.clone(),
                old_binary_phid: None,
                new_binary_phid: None,
                away_paths: parsed.away_paths.clone(),
                original_file_data: None,
                current_file_data: None,
            };
            if parsed.file_type == crate::diff_local::FILE_BINARY {
                // T1153 OLD-PATH: no old bytes for `/dev/null` (ADD).
                if parsed.old_path != "/dev/null" {
                    change.original_file_data = file_data_at_revision(dir, base, &old_plain);
                }
                change.current_file_data = file_data_at_revision(dir, head, &current_plain);
            }
            change
        })
        .collect()
}

/// Convert a conduit `changes[]` dict for rendering (mirrors
/// `ArcanistDiffChange::newFromDictionary()` for the rendered fields).
fn conduit_render_change(change: &serde_json::Value) -> crate::patch::BundleChange {
    let mode = |key: &str| {
        change
            .get(key)
            .and_then(|props| props.get("unix:filemode"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    crate::patch::BundleChange {
        change_type: change
            .get("type")
            .and_then(crate::diff_submit::loose_u64)
            .unwrap_or(u64::from(crate::diff_local::TYPE_CHANGE)) as u32,
        file_type: change
            .get("fileType")
            .and_then(crate::diff_submit::loose_u64)
            .unwrap_or(u64::from(crate::diff_local::FILE_TEXT)) as u32,
        old_path: change
            .get("oldPath")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        current_path: change
            .get("currentPath")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        old_mode: mode("oldProperties"),
        new_mode: mode("newProperties"),
        hunks: crate::patch::conduit_hunks_from_dict(change),
        old_binary_phid: change
            .get("metadata")
            .and_then(|m| m.get("old:binary-phid"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        new_binary_phid: change
            .get("metadata")
            .and_then(|m| m.get("new:binary-phid"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        away_paths: change
            .get("awayPaths")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        original_file_data: None,
        current_file_data: None,
    }
}

/// `isGitBinaryChange()`: `FILE_BINARY` or `FILE_IMAGE` (2).
fn is_export_binary(change: &crate::patch::BundleChange) -> bool {
    change.file_type == crate::diff_local::FILE_BINARY || change.file_type == 2
}

/// `getOldPath()`: null when empty or an addition.
fn export_old_path(change: &crate::patch::BundleChange) -> Option<&str> {
    if change.change_type == crate::diff_local::TYPE_ADD {
        return None;
    }
    change.old_path.as_deref().filter(|path| !path.is_empty())
}

/// `getCurrentPath()`: null when empty, a deletion, or a multicopy.
fn export_current_path(change: &crate::patch::BundleChange) -> Option<&str> {
    if change.change_type == crate::diff_local::TYPE_DELETE
        || change.change_type == crate::diff_local::TYPE_MULTICOPY
    {
        return None;
    }
    change
        .current_path
        .as_deref()
        .filter(|path| !path.is_empty())
}

/// `encodeGitTargetPath()`: targets containing spaces get a trailing tab.
fn export_git_target(target: &str) -> String {
    if target.contains(' ') {
        format!("{target}\t")
    } else {
        target.to_string()
    }
}

/// Basename for the blob-download message (`basename()`).
fn export_basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// Decompose `MULTICOPY` changes into one `MOVE_HERE` plus the surviving
/// `COPY_HERE`s (`toGitPatch()`, T419), or fail like PHP.
fn decompose_export_multicopies(
    changes: &mut Vec<crate::patch::BundleChange>,
) -> Result<(), String> {
    let keys: Vec<usize> = (0..changes.len()).collect();
    let mut removed: Vec<usize> = Vec::new();
    for key in keys {
        if changes[key].change_type != crate::diff_local::TYPE_MULTICOPY {
            continue;
        }
        let current = changes[key].current_path.clone();
        let mut found = None;
        for (index, change) in changes.iter().enumerate() {
            if removed.contains(&index) {
                continue;
            }
            if change.change_type == crate::diff_local::TYPE_COPY_HERE && change.old_path == current
            {
                found = Some(index);
                break;
            }
        }
        let Some(index) = found else {
            return Err(
                "Failed to decompose multicopy changeset in order to generate diff.".to_string(),
            );
        };
        changes[index].change_type = crate::diff_local::TYPE_MOVE_HERE;
        removed.push(key);
    }
    removed.sort_unstable();
    for key in removed.into_iter().rev() {
        changes.remove(key);
    }
    Ok(())
}

/// One `GIT binary patch` literal body: zlib-compress (PHP `gzcompress()`,
/// default level) then base85-encode. (Compression bytes may differ from
/// libzlib's; both inflate identically.)
fn export_binary_body(data: &[u8]) -> Result<String, String> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| format!("failed to compress binary data: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("failed to compress binary data: {e}"))?;
    Ok(crate::patch::base85_encode(&compressed))
}

/// `buildBinaryChange()` without the loader: index line, `GIT binary
/// patch`, and both literal bodies. PHP never emits a `Binary files ...
/// differ` line here. Missing data renders 40 zeros (present-but-empty
/// data hashes as the empty blob).
fn render_export_binary_body(
    old_data: Option<&[u8]>,
    new_data: Option<&[u8]>,
) -> Result<String, String> {
    let id_of = |data: Option<&[u8]>| match data {
        Some(bytes) => crate::patch::git_blob_sha1(bytes),
        None => "0".repeat(40),
    };
    let mut body = format!(
        "index {}..{}\nGIT binary patch\n",
        id_of(old_data),
        id_of(new_data)
    );
    let new_bytes = new_data.unwrap_or(b"");
    body.push_str(&format!("literal {}\n", new_bytes.len()));
    body.push_str(&export_binary_body(new_bytes)?);
    body.push('\n');
    let old_bytes = old_data.unwrap_or(b"");
    body.push_str(&format!("literal {}\n", old_bytes.len()));
    body.push_str(&export_binary_body(old_bytes)?);
    body.push('\n');
    Ok(body)
}

/// Lazy blob loader: `file.download` by PHID, with the basename for the
/// stderr message when known.
type BlobLoader<'a> = dyn Fn(&str, Option<&str>) -> Result<Vec<u8>, String> + 'a;

/// `buildBinaryChange()`: resolve old/new bytes (inline data, else the
/// blob loader) and render. `old_binary` is the away change when the
/// current path was claimed by one.
fn build_export_binary_body(
    change: &crate::patch::BundleChange,
    old_binary: Option<&crate::patch::BundleChange>,
    load_blob: &BlobLoader<'_>,
) -> Result<String, String> {
    let (old_data, old_phid) = match old_binary {
        Some(away) => match away.original_file_data.clone() {
            Some(data) => (Some(data), None),
            None => (None, away.old_binary_phid.as_deref()),
        },
        None => (
            change.original_file_data.clone(),
            change.old_binary_phid.as_deref(),
        ),
    };
    let old_data = match (old_data, old_phid) {
        (Some(data), _) => Some(data),
        (None, Some(phid)) => Some(load_blob(
            phid,
            export_old_path(change).map(export_basename),
        )?),
        (None, None) => None,
    };
    let mut new_data = change.current_file_data.clone();
    if new_data.is_none()
        && let Some(phid) = change.new_binary_phid.as_deref()
    {
        new_data = Some(load_blob(
            phid,
            export_current_path(change).map(export_basename),
        )?);
    }
    render_export_binary_body(old_data.as_deref(), new_data.as_deref())
}

/// First byte of a hunk corpus line, or NUL for empty lines.
fn export_hunk_first_byte(line: &str) -> u8 {
    line.as_bytes().first().copied().unwrap_or(0)
}

/// Scan forward from `from` for the first added/removed corpus line.
fn next_export_change_line(lines: &[&str], from: usize) -> Option<usize> {
    let mut jj = from;
    while jj < lines.len() {
        let byte = export_hunk_first_byte(lines[jj]);
        if byte == b'-' || byte == b'+' {
            return Some(jj);
        }
        jj += 1;
    }
    None
}

/// One scanned hunk span: end offset plus the line counts the header
/// rebuild needs.
struct ExportHunkScan {
    end: usize,
    old_lines: u64,
    new_lines: u64,
    adjust: u64,
    break_at: Option<usize>,
}

/// Scan one hunk span from its first change line, counting added /
/// removed lines and stopping after `context * 2 + 1` trailing
/// context lines (PHP: `>= ($context * 2 + 1)`).
fn scan_export_hunk_end(lines: &[&str], from: usize, context: usize) -> ExportHunkScan {
    let n = lines.len();
    let mut jj = from;
    let mut old_lines: u64 = 0;
    let mut new_lines: u64 = 0;
    let mut adjust: u64 = 0;
    let mut last_change = jj;
    let mut break_here: Option<usize> = None;
    while jj < n {
        if export_hunk_first_byte(lines[jj]) == b' ' {
            if jj - last_change > context && break_here.is_none() {
                break_here = Some(jj);
            }
            if jj - last_change > context * 2 {
                break;
            }
        } else {
            break_here = None;
            last_change = jj;
            match export_hunk_first_byte(lines[jj]) {
                b'\\' => adjust += 1,
                b'-' => old_lines += 1,
                b'+' => new_lines += 1,
                _ => {}
            }
        }
        jj += 1;
    }
    ExportHunkScan {
        end: jj,
        old_lines,
        new_lines,
        adjust,
        break_at: break_here,
    }
}

/// Break one hunk into small hunks with 3 lines of context
/// (`breakHunkIntoSmallHunks()`; same algorithm as the verified
/// `arc patch` port, which lives in a read-only module for this task).
fn break_export_hunk(base: &crate::diff_local::ParsedHunk) -> Vec<crate::diff_local::ParsedHunk> {
    const CONTEXT: usize = 3;
    let lines: Vec<&str> = base.corpus.split_inclusive('\n').collect();
    let n = lines.len();
    let mut old_offset = base.old_start;
    let mut new_offset = base.new_start;
    let mut results = Vec::new();
    let mut ii = 0;
    while ii < n {
        let Some(found) = next_export_change_line(&lines, ii) else {
            break;
        };
        let hunk_start = found.saturating_sub(CONTEXT);
        let scan = scan_export_hunk_end(&lines, found, CONTEXT);
        let jj = scan.break_at.unwrap_or(scan.end);
        let hunk_length = jj.min(n) - hunk_start;
        let count_length = hunk_length as u64 - scan.adjust;
        let corpus: String = lines[hunk_start..hunk_start + hunk_length].concat();
        // Adjacent hunks share context, so `hunk_start` may precede `ii`.
        let back = ii as i64 - hunk_start as i64;
        results.push(crate::diff_local::ParsedHunk {
            old_start: (old_offset as i64 - back).max(0) as u64,
            old_len: count_length - scan.new_lines,
            new_start: (new_offset as i64 - back).max(0) as u64,
            new_len: count_length - scan.old_lines,
            corpus,
            // Splitter hunks count from the corpus (T1153 EXPORT-E20).
            synthetic_counts: None,
        });
        old_offset += (jj - ii) as u64 - scan.new_lines;
        new_offset += (jj - ii) as u64 - scan.old_lines;
        ii = jj;
    }
    results
}

/// `buildHunkChanges()`: rebuilt `@@` headers (length 1 omitted, like git)
/// plus corpora, with a newline forced after unterminated bodies.
fn render_export_hunks(hunks: &[crate::diff_local::ParsedHunk]) -> String {
    fn head(offset: u64, len: u64) -> String {
        if len == 1 {
            format!("{offset}")
        } else {
            format!("{offset},{len}")
        }
    }
    let mut out = String::new();
    for hunk in hunks {
        for small in break_export_hunk(hunk) {
            out.push_str(&format!(
                "@@ -{} +{} @@\n",
                head(small.old_start, small.old_len),
                head(small.new_start, small.new_len)
            ));
            out.push_str(&small.corpus);
            if !small.corpus.is_empty()
                && !small.corpus.ends_with('\n')
                && !small.corpus.ends_with('\r')
            {
                out.push('\n');
            }
        }
    }
    out
}

/// Away-path to change index for binary move/copy sources.
fn collect_export_binary_sources(
    changes: &[crate::patch::BundleChange],
) -> std::collections::BTreeMap<String, usize> {
    use crate::diff_local as types;
    let mut binary_sources: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for (index, change) in changes.iter().enumerate() {
        if !is_export_binary(change) {
            continue;
        }
        let change_type = change.change_type;
        if change_type == types::TYPE_MOVE_AWAY
            || change_type == types::TYPE_COPY_AWAY
            || change_type == types::TYPE_MULTICOPY
        {
            for path in &change.away_paths {
                binary_sources.insert(path.clone(), index);
            }
        }
    }
    binary_sources
}

/// Away change feeding a binary body, if the current path was claimed
/// by a move/copy source.
fn lookup_export_old_binary<'a>(
    changes: &'a [crate::patch::BundleChange],
    binary_sources: &std::collections::BTreeMap<String, usize>,
    change: &crate::patch::BundleChange,
) -> Option<&'a crate::patch::BundleChange> {
    let cur = export_current_path(change)?;
    let away = binary_sources.get(cur).copied()?;
    Some(&changes[away])
}

/// True for changes with no patch output at all: directories and
/// move-away halves.
fn export_git_has_no_output(change: &crate::patch::BundleChange) -> bool {
    use crate::diff_local as types;
    change.file_type == types::FILE_DIRECTORY || change.change_type == types::TYPE_MOVE_AWAY
}

/// Render one change body: binary literal blocks or rebuilt hunks.
fn render_export_change_body(
    change: &crate::patch::BundleChange,
    old_binary: Option<&crate::patch::BundleChange>,
    load_blob: &BlobLoader<'_>,
) -> Result<String, String> {
    if is_export_binary(change) {
        build_export_binary_body(change, old_binary, load_blob)
    } else {
        Ok(render_export_hunks(&change.hunks))
    }
}

/// True for `COPY_AWAY` halves with no body and no mode change, which
/// carry no output.
fn is_empty_copy_away(
    change: &crate::patch::BundleChange,
    body: &str,
    old_mode: &str,
    new_mode: &str,
) -> bool {
    use crate::diff_local as types;
    change.change_type == types::TYPE_COPY_AWAY && body.is_empty() && old_mode == new_mode
}

/// `diff --git` index lines plus tab-suffixed `---`/`+++` targets
/// (additions track the new path, deletions the old path).
fn export_git_index_targets(
    old_path: Option<&str>,
    cur_path: Option<&str>,
) -> (String, String, String, String) {
    let (old_index, old_target) = match old_path {
        // `getOldPath()` null (additions): index tracks the new path.
        None => (
            format!("a/{}", cur_path.unwrap_or_default()),
            "/dev/null".to_string(),
        ),
        Some(path) => (format!("a/{path}"), format!("a/{path}")),
    };
    let (cur_index, cur_target) = match cur_path {
        None => (
            format!("b/{}", old_path.unwrap_or_default()),
            "/dev/null".to_string(),
        ),
        Some(path) => (format!("b/{path}"), format!("b/{path}")),
    };
    (
        old_index,
        cur_index,
        export_git_target(&old_target),
        export_git_target(&cur_target),
    )
}

/// `new file mode` / `old mode` / `new mode` header lines.
fn export_git_mode_lines(
    change: &crate::patch::BundleChange,
    old_mode: &str,
    new_mode: &str,
    out: &mut String,
) {
    use crate::diff_local as types;
    if change.change_type == types::TYPE_ADD {
        out.push_str(&format!("new file mode {new_mode}\n"));
    }
    let change_type = change.change_type;
    if (change_type == types::TYPE_COPY_HERE
        || change_type == types::TYPE_MOVE_HERE
        || change_type == types::TYPE_COPY_AWAY
        || change_type == types::TYPE_CHANGE)
        && old_mode != new_mode
    {
        out.push_str(&format!("old mode {old_mode}\n"));
        out.push_str(&format!("new mode {new_mode}\n"));
    }
}

/// `copy from/to`, `rename from/to`, and `deleted file mode` lines.
fn export_git_copy_lines(
    change: &crate::patch::BundleChange,
    old_path: Option<&str>,
    cur_path: Option<&str>,
    out: &mut String,
) {
    use crate::diff_local as types;
    if change.change_type == types::TYPE_COPY_HERE {
        out.push_str(&format!(
            "copy from {}\ncopy to {}\n",
            old_path.unwrap_or_default(),
            cur_path.unwrap_or_default()
        ));
    } else if change.change_type == types::TYPE_MOVE_HERE {
        out.push_str(&format!(
            "rename from {}\nrename to {}\n",
            old_path.unwrap_or_default(),
            cur_path.unwrap_or_default()
        ));
    } else if change.change_type == types::TYPE_DELETE
        || change.change_type == types::TYPE_MULTICOPY
    {
        // No `100644` default here: the line only prints when the old
        // properties actually carry a mode.
        if let Some(mode) = change.old_mode.as_deref().filter(|mode| !mode.is_empty()) {
            out.push_str(&format!("deleted file mode {mode}\n"));
        }
    }
}

/// Render one change of a git patch, or `None` when it carries no
/// output. Blob downloads happen before the `COPY_AWAY` skip check,
/// like PHP.
fn render_export_git_change(
    change: &crate::patch::BundleChange,
    old_binary: Option<&crate::patch::BundleChange>,
    load_blob: &BlobLoader<'_>,
) -> Result<Option<String>, String> {
    if export_git_has_no_output(change) {
        return Ok(None);
    }
    let old_mode = change.old_mode.as_deref().unwrap_or("100644");
    let new_mode = change.new_mode.as_deref().unwrap_or("100644");
    let is_binary = is_export_binary(change);
    let old_path = export_old_path(change);
    let cur_path = export_current_path(change);
    let change_body = render_export_change_body(change, old_binary, load_blob)?;
    if is_empty_copy_away(change, &change_body, old_mode, new_mode) {
        return Ok(None);
    }
    let (old_index, cur_index, old_target, cur_target) =
        export_git_index_targets(old_path, cur_path);
    let mut out = format!("diff --git {old_index} {cur_index}\n");
    export_git_mode_lines(change, old_mode, new_mode, &mut out);
    export_git_copy_lines(change, old_path, cur_path, &mut out);
    if !change_body.is_empty() {
        if !is_binary {
            out.push_str(&format!("--- {old_target}\n"));
            out.push_str(&format!("+++ {cur_target}\n"));
        }
        out.push_str(&change_body);
    }
    Ok(Some(out))
}

/// Render bundle changes as a git patch (`ArcanistBundle::toGitPatch()`):
/// rebuilt headers from offsets, 3-line context, tab-suffixed targets, no
/// `index`/`similarity`/`Binary files` lines (T1153 EXPORT-E6/E8). The
/// blob loader downloads conduit blobs lazily (`file.download`).
fn render_export_git_patch(
    changes: &mut Vec<crate::patch::BundleChange>,
    load_blob: &BlobLoader<'_>,
) -> Result<String, String> {
    decompose_export_multicopies(changes)?;
    let binary_sources = collect_export_binary_sources(changes);
    let mut result = String::new();
    for index in 0..changes.len() {
        let change = &changes[index];
        let old_binary = lookup_export_old_binary(changes, &binary_sources, change);
        if let Some(text) = render_export_git_change(change, old_binary, load_blob)? {
            result.push_str(&text);
        }
    }
    result.push('\n');
    Ok(result)
}

/// Render bundle changes as a unified diff
/// (`ArcanistBundle::toUnifiedDiff()`, T1153 EXPORT-E4/E5): `Index:` +
/// `===` + `---`/`+++` (new path wins for moves) + rebuilt hunks. Empty
/// bundles render `""`.
fn render_export_unified(changes: &[crate::patch::BundleChange]) -> String {
    let mut result = String::new();
    for change in changes {
        let hunk_changes = render_export_hunks(&change.hunks);
        if hunk_changes.is_empty() {
            continue;
        }
        let old_path = export_old_path(change);
        let cur_path = export_current_path(change);
        let index_path = cur_path.or(old_path).unwrap_or_default();
        result.push_str(&format!("Index: {index_path}\n"));
        result.push_str(&format!("{}\n", "=".repeat(67)));
        let mut old_target = old_path.unwrap_or("/dev/null").to_string();
        let cur_target = cur_path.unwrap_or("/dev/null").to_string();
        if cur_target != "/dev/null" && old_target != "/dev/null" {
            old_target = cur_target.clone();
        }
        result.push_str(&format!("--- {old_target}\n"));
        result.push_str(&format!("+++ {cur_target}\n"));
        result.push_str(&hunk_changes);
    }
    result
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
            let base = resolve_export_base(dir, base.as_deref())?;
            local_bundle_parts(dir, &base, encoding, client)?
        }
        other => conduit_bundle_parts(&querydiffs_params(other), encoding, client)?,
    };
    let hunks = strip_corpora(&mut changes);
    // T1153 EXPORT-E33: PHP echoes the progress line before `writeToDisk()`
    // (blob downloads included), so failures leave partial output behind.
    println!("Writing bundle to '{path}'...");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut blobs = Vec::new();
    for phid in blob_phids(&changes) {
        blobs.push((phid.clone(), download_blob(client, &phid)?));
    }
    let out = bundle_out_path(dir, path);
    write_bundle(&out, &changes, &meta, &hunks, &blobs)?;
    Ok("Done.\n".to_string())
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
        // T1153 R4 (EXPORT-E28): PHP renders usage errors with no dump.
        ParseOutcome::Error(message) => return Err(message),
    };
    execute_with_opts(dir, &opts)
}

/// Render text output from bundle changes: a git patch or a unified
/// diff (T1153 EXPORT-E4/E5/E6/E8).
fn render_export_text(
    changes: &mut Vec<crate::patch::BundleChange>,
    git: bool,
    load_blob: &BlobLoader<'_>,
) -> Result<String, String> {
    if git {
        render_export_git_patch(changes, load_blob)
    } else {
        Ok(render_export_unified(changes))
    }
}

/// Local workcopy diff as transport text (T1153 EXPORT-E9): raw bytes
/// ride through parse/render, and `--encoding` converts only at the
/// stdout sink, like `convertNonUTF8Diff()`.
fn decode_local_export_diff(dir: &Path, base: &str) -> Result<String, String> {
    let bytes = local_diff_bytes(dir, Format::Git, base)?;
    reject_empty_diff(&bytes)?;
    Ok(crate::patch::transport_bytes(&bytes))
}

/// Text export over a local workcopy (T1153 EXPORT-E30: PHP requires
/// conduit (and queries the author) for ALL local exports, even
/// `--git`/`--unified`).
fn execute_local_export_text(
    dir: &Path,
    opts: &Options,
    base: &Option<String>,
    git: bool,
) -> Result<String, String> {
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
    let base = resolve_export_base(dir, base.as_deref())?;
    let text = decode_local_export_diff(dir, &base)?;
    let parsed = crate::diff_local::parse_unified_diff_transport(&text)?;
    let head = resolve_export_head(dir)?;
    // The author is discarded for text formats, but PHP issues
    // the query before rendering.
    viewer_real_name(&client)?;
    let mut changes = local_render_changes(dir, &base, &head, &parsed);
    render_export_text(&mut changes, git, &|phid, _| {
        Err(format!("Nowhere to load blob '{phid}' from!"))
    })
}

/// `getBlob($phid, $name)`: download one conduit blob, naming the
/// file on stderr when known.
fn download_export_blob(
    client: &crate::conduit::Client,
    phid: &str,
    name: Option<&str>,
) -> Result<Vec<u8>, String> {
    match name {
        Some(text) => eprintln!("Downloading binary data for '{text}'..."),
        None => eprintln!("Downloading binary data..."),
    }
    let result = client
        .call("file.download", &serde_json::json!({"phid": phid}))
        .map_err(|e| e.to_string())?;
    let encoded = result
        .as_str()
        .ok_or_else(|| "file.download returned an unexpected response".to_string())?;
    Ok(lax_base64_decode(encoded))
}

/// Text export over a conduit diff.
fn execute_conduit_export_text(
    opts: &Options,
    source: &Source,
    git: bool,
) -> Result<String, String> {
    let client = crate::config::client_for(opts.conduit_uri.as_deref())?;
    let diff = first_diff_dict(&client, &querydiffs_params(source))?;
    let items = diff
        .get("changes")
        .and_then(|changes| changes.as_array())
        .cloned()
        .unwrap_or_default();
    let mut changes: Vec<crate::patch::BundleChange> =
        items.iter().map(conduit_render_change).collect();
    // `getBlob($phid, $name)`: the render path passes the file
    // basename, so the stderr message names the file.
    render_export_text(&mut changes, git, &|phid, name| {
        download_export_blob(&client, phid, name)
    })
}

/// [`execute`] over parsed options.
fn execute_with_opts(dir: &Path, opts: &Options) -> Result<String, String> {
    if let Format::Bundle { path } = &opts.format {
        return export_bundle(dir, opts, path);
    }
    let git = matches!(opts.format, Format::Git);
    match &opts.source {
        Source::Local { base } => execute_local_export_text(dir, opts, base, git),
        other => execute_conduit_export_text(opts, other, git),
    }
}

/// Run the workflow rooted at `dir`, returning stdout bytes or an error
/// message: like [`execute`], then UTF-8 to `--encoding` conversion at
/// the sink (`convertNonUTF8Diff()`). Bundle writes keep UTF-8 stdout
/// (the file carries its own encoding record).
pub fn execute_bytes(dir: &Path, argv: &[String]) -> Result<Vec<u8>, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage().into_bytes()),
        // T1153 R4 (EXPORT-E28): PHP renders usage errors with no dump.
        ParseOutcome::Error(message) => return Err(message),
    };
    let text = execute_with_opts(dir, &opts)?;
    if matches!(opts.format, Format::Bundle { .. }) {
        return Ok(text.into_bytes());
    }
    // Local text rides as transport (T1153 EXPORT-E9); conduit text is
    // already UTF-8. Both convert at the sink, like
    // `convertNonUTF8Diff()`.
    if matches!(opts.source, Source::Local { .. }) {
        crate::patch::encode_transport_text(&text, opts.encoding.as_deref())
    } else {
        crate::patch::encode_bytes(&text, opts.encoding.as_deref())
    }
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    use std::io::Write;
    match execute_bytes(dir, argv) {
        Ok(bytes) => {
            if std::io::stdout().write_all(&bytes).is_err() {
                eprintln!("arc {NAME}: failed to write output");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 R4 (EXPORT-E7, EXPORT-E28, T1153 RB-D4): `export` is a
            // subprocess workflow. Argument errors render `Usage
            // Exception:` (exit 1); empty diffs, conduit failures,
            // and not-found errors are generic exceptions under the
            // red banner (exit 1, never 255).
            let kind = if matches!(parse_args(argv), ParseOutcome::Error(_)) {
                EnvelopeKind::Usage
            } else {
                EnvelopeKind::Exception
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
#[path = "export_tests.rs"]
mod export_tests;
