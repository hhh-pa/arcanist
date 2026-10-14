//! `arc-rs lint`: run lint rules on changes.
//!
//! Full-state port of `src/workflow/ArcanistLintWorkflow.php` (`run()`,
//! `:147-387`), `src/lint/engine/ArcanistLintEngine.php` (severity filter,
//! changed-line relevance, stops, patch marking), the six renderers under
//! `src/lint/renderer/`, `src/lint/ArcanistLintPatcher.php`, and
//! `src/lint/linter/ArcanistTextLinter.php` (all nine `TXT` rules).
//!
//! Flag parsing mirrors `ArcanistWorkflow::parseArguments()`
//! (`src/workflow/ArcanistWorkflow.php:788-926`) exactly, including error
//! strings, conflict handling (`:899-919`, with the `': 1'` quirk for
//! `true` reasons), and `willRunWorkflow()` VCS gating (`:1494-1517`).
//! Exit codes are the PHP result codes: 0 okay, 1 warnings, 2 errors.
//! Usage errors and runtime failures exit 1, like the toolset runtime
//! (`src/runtime/ArcanistRuntime.php:36-49`).
//!
//! Divergences from PHP (see also [`crate::arclint`]):
//! - VCS discovery is innermost-wins (nearest `.git`/`.hg`/`.svn` upward),
//!   matching what the VCS tools themselves resolve; PHP walks from the
//!   filesystem root and takes the outermost match
//!   (`ArcanistWorkingCopyIdentity.php:60-109`). Nested working copies
//!   resolve to the inner one here.
//! - Only project `.arcconfig` is read (`lint.engine`,
//!   `lint.engine.single.linter`, `history.immutable`,
//!   `git.default-relative-commit`); user/system/local config layers,
//!   `arc:base` rules, and the `base` config are not consulted.
//! - Include/exclude/`severity.rules` routing needs a regex engine the
//!   dependency set lacks, so per-linter path routing is not applied and
//!   `severity.rules` never match; exact `severity` maps are applied.
//! - Per-linter tool options (`bin`, `flags`, `version`, `interpreter`,
//!   per-tool config keys) parse but are not applied, except
//!   `text.max-line-length`. External tools run with default binaries.
//! - `severity.rules` patterns are not validated as regular expressions.
//! - Console markup (colors, `##inverse##` highlights) is stripped: output
//!   matches piped (non-TTY) `arc` exactly, verified against the golden
//!   files in `src/lint/renderer/__tests__/data/`.
//! - `phutil_console_wrap()` is approximated as word-wrap at 80 columns;
//!   only long descriptions are affected.
//! - The `OKAY` line, `LINT NOTICE` line, confirm-prompt decorations, and
//!   the XHP rules table are reconstructed (phutil is not vendored):
//!   `   OKAY  No lint messages.`, `** LINT NOTICE ** ...`, `{prompt}
//!   [Y/n] `, and a bordered ID/Class/Name table.
//! - Descriptions containing `/` are emitted as `\/` in `--output json`
//!   to match PHP `json_encode()` escaping.
//! - `--engine ArcanistUnitTestableLintEngine` and unknown engine classes
//!   are rejected with the not-a-subclass error; PHP could instantiate
//!   other custom engines.
//! - `script-and-regex` `halt` groups and `original`/`replacement` groups
//!   behave like `stop` / are dropped (see `lint_script_and_regex`).
//! - Obsolete-message filtering and lint caching are not ported; no
//!   ported linter emits obsolete messages or reads caches.

extern crate self as arc_rs;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{arclint, git, hg, svn};

pub const NAME: &str = "lint";

/// PHP `ArcanistLintWorkflow::RESULT_OKAY` (`:8`).
pub const RESULT_OKAY: i32 = 0;
/// PHP `ArcanistLintWorkflow::RESULT_WARNINGS` (`:9`).
pub const RESULT_WARNINGS: i32 = 1;
/// PHP `ArcanistLintWorkflow::RESULT_ERRORS` (`:10`).
pub const RESULT_ERRORS: i32 = 2;
/// PHP `ArcanistLintWorkflow::RESULT_SKIP` (`:11`).
pub const RESULT_SKIP: i32 = 3;

pub const SEVERITY_ERROR: &str = "error";
pub const SEVERITY_WARNING: &str = "warning";
pub const SEVERITY_AUTOFIX: &str = "autofix";
pub const SEVERITY_ADVICE: &str = "advice";
pub const SEVERITY_DISABLED: &str = "disabled";

/// Default minimum severity, mirroring
/// `ArcanistLintWorkflow::DEFAULT_SEVERITY`
/// (`src/workflow/ArcanistLintWorkflow.php:13`).
pub const DEFAULT_SEVERITY: &str = SEVERITY_ADVICE;

/// Renderer keys, mirroring the `RENDERERKEY` constants.
pub const RENDERER_CONSOLE: &str = "console";
pub const RENDERER_JSON: &str = "json";
pub const RENDERER_SUMMARY: &str = "summary";
pub const RENDERER_COMPILER: &str = "compiler";
pub const RENDERER_XML: &str = "xml";
pub const RENDERER_NONE: &str = "none";

/// Supported renderer keys, sorted. PHP lists them in class-map order
/// (`ArcanistLintRenderer::getAllRenderers()`), which is not reproducible
/// here; the sorted order is deterministic.
#[must_use]
pub fn all_renderers() -> Vec<&'static str> {
    vec![
        RENDERER_COMPILER,
        RENDERER_CONSOLE,
        RENDERER_JSON,
        RENDERER_NONE,
        RENDERER_SUMMARY,
        RENDERER_XML,
    ]
}

/// Engine classes accepted by `--engine` / `lint.engine`, verbatim.
pub const ENGINE_CONFIGURATION_DRIVEN: &str = "ArcanistConfigurationDrivenLintEngine";
pub const ENGINE_COMPREHENSIVE: &str = "ArcanistComprehensiveLintEngine";
pub const ENGINE_SINGLE: &str = "ArcanistSingleLintEngine";
pub const ENGINE_BASE_CLASS: &str = "ArcanistLintEngine";

const CODE_DOS_NEWLINE: &str = "TXT1";
const CODE_TAB: &str = "TXT2";
const CODE_LINE_WRAP: &str = "TXT3";
const CODE_EOF_NEWLINE: &str = "TXT4";
const CODE_BAD_CHARSET: &str = "TXT5";
const CODE_TRAILING: &str = "TXT6";
const CODE_BOF_WHITESPACE: &str = "TXT8";
const CODE_EOF_WHITESPACE: &str = "TXT9";
const CODE_EMPTY_FILE: &str = "TXT10";

const NAME_DOS_NEWLINE: &str = "DOS Newlines";
const NAME_TAB: &str = "Tab Literal";
const NAME_LINE_WRAP: &str = "Line Too Long";
const NAME_EOF_NEWLINE: &str = "File Does Not End in Newline";
const NAME_BAD_CHARSET: &str = "Bad Charset";
const NAME_TRAILING: &str = "Trailing Whitespace";
const NAME_BOF_WHITESPACE: &str = "Leading Whitespace at BOF";
const NAME_EOF_WHITESPACE: &str = "Trailing Whitespace at EOF";
const NAME_EMPTY_FILE: &str = "Empty File";

const DESC_TAB: &str = "Configure your editor to use spaces for indentation.";
const DESC_EOF_NEWLINE: &str = "Files must end in a newline.";
const DESC_TRAILING: &str = "This line contains trailing whitespace. Consider setting up your editor to automatically remove trailing whitespace.";
const DESC_DOS_NEWLINE: &str = "You must use ONLY Unix linebreaks (\"\n\") in source code.";
const DESC_BAD_CHARSET: &str = "Source code should contain only ASCII bytes with ordinal decimal values between 32 and 126 inclusive, plus linefeed. Do not use UTF-8 or other multibyte charsets.";
const DESC_BOF_WHITESPACE: &str = "This file contains leading whitespace at the beginning of the file. This is unnecessary and should be avoided when possible.";
const DESC_EOF_WHITESPACE: &str = "This file contains unnecessary trailing whitespace.";
const DESC_EMPTY_FILE: &str = "Empty files usually don't serve any useful purpose.";

/// Default `text.max-line-length`
/// (`ArcanistTextLinter.php:18`).
pub const DEFAULT_MAX_LINE_LENGTH: usize = 80;

/// Context lines for the JSON renderer
/// (`ArcanistJSONLintRenderer.php:7`).
pub const JSON_LINES_OF_CONTEXT: usize = 3;

/// Diff context lines for changed-line queries. PHP uses
/// `getDiffLinesOfContext()` (default 3); only `+` lines are collected,
/// so the value does not affect the result.
pub const DIFF_CONTEXT_LINES: u32 = 3;

fn usage() -> String {
    "usage: arc-rs lint [--lintall] [--rev REV] [--output FORMAT] [--outfile FILE] [--engine CLASS] [--apply-patches | --never-apply-patches] [--amend-all] [--amend-autofixes] [--everything] [--severity LEVEL] [PATHS...]\n\
     \n\
     Run static analysis on changes to check for mistakes. If no files\n\
     are specified, lint will be run on all files which have been modified.\n\
     \n\
     Options:\n\
     \x20 --lintall             Show all lint warnings, not just those on\n\
     \x20                       changed lines. When paths are specified, this\n\
     \x20                       is the default behavior.\n\
     \x20 --rev REV             Lint changes since a specific revision.\n\
     \x20 --output FORMAT       Select an output format: console, json,\n\
     \x20                       summary, compiler, xml, none.\n\
     \x20 --outfile FILE        Output the linter results to a file.\n\
     \x20                       Defaults to stdout.\n\
     \x20 --engine CLASS        Override configured lint engine for this\n\
     \x20                       project.\n\
     \x20 --apply-patches       Apply patches suggested by lint to the\n\
     \x20                       working copy without prompting.\n\
     \x20 --never-apply-patches Never apply patches suggested by lint.\n\
     \x20 --amend-all           When linting git repositories, amend HEAD\n\
     \x20                       with all patches suggested by lint without\n\
     \x20                       prompting.\n\
     \x20 --amend-autofixes     When linting git repositories, amend HEAD\n\
     \x20                       with autofix patches suggested by lint\n\
     \x20                       without prompting.\n\
     \x20 --everything          Lint all tracked files in the working copy.\n\
     \x20                       Ignored files and untracked files will not\n\
     \x20                       be linted.\n\
     \x20 --severity LEVEL      Set minimum message severity. One of:\n\
     \x20                       'advice', 'autofix', 'warning', 'error',\n\
     \x20                       'disabled'. Defaults to 'advice'.\n\
     \x20 -h, --help            Show this help.\n"
        .to_string()
}

/// Rank of a severity level, mirroring
/// `ArcanistLintSeverity::isAtLeastAsSevere()`
/// (`src/lint/ArcanistLintSeverity.php:34-48`): disabled 10, advice 20,
/// autofix 25, warning 30, error 40.
fn severity_rank(severity: &str) -> Option<u32> {
    match severity {
        SEVERITY_DISABLED => Some(10),
        SEVERITY_ADVICE => Some(20),
        SEVERITY_AUTOFIX => Some(25),
        SEVERITY_WARNING => Some(30),
        SEVERITY_ERROR => Some(40),
        _ => None,
    }
}

/// True when a message with `message_severity` passes a `minimum` filter,
/// mirroring `ArcanistLintSeverity::isAtLeastAsSevere()`: unknown message
/// severities pass, and an unknown minimum passes everything.
#[must_use]
pub fn severity_allows(message_severity: &str, minimum: &str) -> bool {
    let Some(rank) = severity_rank(message_severity) else {
        return true;
    };
    rank >= severity_rank(minimum).unwrap_or(0)
}

/// Drop messages below `minimum`, mirroring the engine-side
/// `isSeverityEnabled()` filter (`src/lint/engine/ArcanistLintEngine.php:272-275`).
#[must_use]
pub fn filter_by_severity(messages: Vec<LintMessage>, minimum: &str) -> Vec<LintMessage> {
    messages
        .into_iter()
        .filter(|message| severity_allows(&message.severity, minimum))
        .collect()
}

/// Display name for a severity, mirroring
/// `ArcanistLintSeverity::getStringForSeverity()`
/// (`src/lint/ArcanistLintSeverity.php:24-32`).
pub fn severity_display_name(severity: &str) -> Result<&'static str, String> {
    match severity {
        SEVERITY_ADVICE => Ok("Advice"),
        SEVERITY_AUTOFIX => Ok("Auto-Fix"),
        SEVERITY_WARNING => Ok("Warning"),
        SEVERITY_ERROR => Ok("Error"),
        SEVERITY_DISABLED => Ok("Disabled"),
        _ => Err(format!("Unknown lint severity '{severity}'!")),
    }
}

/// Checkstyle severity word, mirroring
/// `ArcanistCheckstyleXMLLintRenderer::getStringForSeverity()`.
fn xml_severity(severity: &str) -> &'static str {
    match severity {
        SEVERITY_ADVICE | SEVERITY_AUTOFIX => "info",
        SEVERITY_WARNING => "warning",
        SEVERITY_ERROR => "error",
        SEVERITY_DISABLED => "ignore",
        _ => "",
    }
}

/// Spawn `cmd`, retrying briefly on ETXTBSY ("Text file busy"). A freshly
/// written executable can still be settling when it is spawned (parallel
/// tests, an editor saving a hook); the error is transient by nature, so
/// retry bounded, then return the last error unchanged. Every external
/// tool runner must spawn through here.
pub fn spawn_with_retry(cmd: &mut Command) -> Result<std::process::Output, std::io::Error> {
    let mut attempt = 0;
    loop {
        match cmd.output() {
            Err(error) if is_text_file_busy(&error) && attempt < 50 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

/// True for ETXTBSY (errno 26 on Linux and the BSDs, including macOS).
/// Windows has no such spawn error.
#[cfg(unix)]
fn is_text_file_busy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

/// True for ETXTBSY (errno 26 on Linux and the BSDs, including macOS).
/// Windows has no such spawn error.
#[cfg(not(unix))]
fn is_text_file_busy(_error: &std::io::Error) -> bool {
    false
}

/// A single lint finding, mirroring `ArcanistLintMessage`.
///
/// `line` and `col` are 1-based; 0 means "no line/char" (PHP `null` from
/// `raiseLintAtPath()`). `original`/`replacement` carry patch data;
/// a message is patchable when a replacement differs from the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintMessage {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub code: String,
    pub severity: String,
    pub name: String,
    pub description: String,
    pub original: Option<String>,
    pub replacement: Option<String>,
}

impl LintMessage {
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == SEVERITY_ERROR
    }

    #[must_use]
    pub fn is_warning(&self) -> bool {
        self.severity == SEVERITY_WARNING
    }

    #[must_use]
    pub fn is_autofix(&self) -> bool {
        self.severity == SEVERITY_AUTOFIX
    }

    /// True when the message targets a file position (PHP
    /// `hasFileContext()`: line is not null).
    #[must_use]
    pub fn has_file_context(&self) -> bool {
        self.line != 0
    }

    /// True when the message carries an applicable patch (PHP
    /// `isPatchable()`: replacement is not null and strictly differs
    /// from the original, so a missing original with any replacement
    /// counts as patchable).
    #[must_use]
    pub fn is_patchable(&self) -> bool {
        match (&self.original, &self.replacement) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(original), Some(replacement)) => original != replacement,
        }
    }

    #[must_use]
    pub fn to_line(&self) -> String {
        format!(
            "{}:{}:{}: {}: {}",
            self.path, self.line, self.col, self.severity, self.description
        )
    }

    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path,
            "line": self.line,
            "char": self.col,
            "col": self.col,
            "code": self.code,
            "severity": self.severity,
            "name": self.name,
            "description": self.description,
            "message": self.description,
        })
    }
}

/// A lint rule set. More linters can be added behind this trait.
///
/// The defaulted methods mirror `ArcanistLinter` predicates:
/// [`Linter::priority`] is `getLinterPriority()` (default 1.0) and the
/// `should_lint_*` methods are the `shouldLint*()` path filters.
pub trait Linter {
    fn name(&self) -> &'static str;
    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage>;
    /// Execution priority, lowest first (`getLinterPriority()`).
    fn priority(&self) -> f64 {
        1.0
    }
    /// True when the linter runs on binary files.
    fn should_lint_binary_files(&self) -> bool {
        false
    }
    /// True when the linter runs on directories.
    fn should_lint_directories(&self) -> bool {
        false
    }
    /// True when the linter runs on symbolic links.
    fn should_lint_symbolic_links(&self) -> bool {
        false
    }
}

/// Basic text linter: all nine `ArcanistTextLinter` rules
/// (`src/lint/linter/ArcanistTextLinter.php:93-125`).
#[derive(Debug)]
pub struct TextLinter {
    max_line_length: usize,
    /// Exact-code severity map: DOS-newline and bad-charset stops
    /// only fire for enabled codes (`isMessageEnabled()`).
    severity_map: HashMap<String, String>,
}

impl TextLinter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_line_length: DEFAULT_MAX_LINE_LENGTH,
            severity_map: HashMap::new(),
        }
    }

    /// A text linter with a custom `text.max-line-length`.
    #[must_use]
    pub fn with_max_line_length(max_line_length: usize) -> Self {
        Self {
            max_line_length,
            severity_map: HashMap::new(),
        }
    }

    /// A text linter with an exact-code severity map.
    #[must_use]
    pub fn with_severity_map(severity_map: HashMap<String, String>) -> Self {
        Self {
            max_line_length: DEFAULT_MAX_LINE_LENGTH,
            severity_map,
        }
    }

    #[must_use]
    pub fn max_line_length(&self) -> usize {
        self.max_line_length
    }

    /// True when a stop code is not custom-disabled, keyed by full
    /// code then short code (`TXT1` or `1`).
    fn stop_enabled(&self, full_code: &str) -> bool {
        if let Some(severity) = self.severity_map.get(full_code) {
            return severity != SEVERITY_DISABLED;
        }
        if let Some(short) = full_code.strip_prefix("TXT")
            && let Some(severity) = self.severity_map.get(short)
        {
            return severity != SEVERITY_DISABLED;
        }
        true
    }
}

impl Default for TextLinter {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a count like `PhutilNumber` (thousands separators).
fn format_number(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(byte as char);
    }
    out
}

/// True for PHP `ctype`/regex `\s` bytes (space, tab, LF, CR, FF, VT).
/// Rust `is_ascii_whitespace` omits VT (`0x0B`), which PHP includes.
fn is_php_space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == 0x0B
}

/// Nine positional fields mirror the PHP message constructor; a
/// struct would only rename the same call sites.
#[allow(clippy::too_many_arguments)]
fn text_message(
    path: &str,
    line: usize,
    col: usize,
    code: &str,
    severity: &str,
    name: &str,
    description: String,
    original: Option<String>,
    replacement: Option<String>,
) -> LintMessage {
    LintMessage {
        path: path.to_string(),
        line,
        col,
        code: code.to_string(),
        severity: severity.to_string(),
        name: name.to_string(),
        description,
        original,
        replacement,
    }
}

impl Linter for TextLinter {
    fn name(&self) -> &'static str {
        "TXT"
    }

    fn priority(&self) -> f64 {
        0.5
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        let mut out = Vec::new();

        // `lintEmptyFile()` runs first and stops every linter (including
        // the rest of this one) when it fires.
        if lint_empty_file(path, data, &mut out) {
            return out;
        }
        if data.is_empty() {
            // Empty files which survived the empty-file rule (dotfiles
            // and `__init__.py`) need no trailing newline.
            return Vec::new();
        }

        // `lintNewlines()` then `lintTabs()`; an enabled DOS-newline
        // stop skips the charset phase but not the tabs already raised.
        let dos_stop =
            lint_dos_newlines(path, data, &mut out) && self.stop_enabled(CODE_DOS_NEWLINE);
        lint_tabs(path, data, &mut out);
        if dos_stop {
            return out;
        }

        if lint_charset(path, data, &mut out) && self.stop_enabled(CODE_BAD_CHARSET) {
            return out;
        }

        lint_line_length(path, data, self.max_line_length, &mut out);
        lint_eof_newline(path, data, &mut out);
        lint_trailing_whitespace(path, data, &mut out);
        lint_bof_whitespace(path, data, &mut out);
        lint_eof_whitespace(path, data, &mut out);

        out.sort_by(|a, b| (a.line, a.col, &a.code).cmp(&(b.line, b.col, &b.code)));
        out
    }
}

/// `lintEmptyFile()` (`ArcanistTextLinter.php:127-148`): whitespace-only
/// files (except dotfiles and `__init__.py`) raise `TXT10` and stop all
/// linters. Returns true when it stopped.
fn lint_empty_file(path: &str, data: &[u8], out: &mut Vec<LintMessage>) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if base == "__init__.py" || base.starts_with('.') {
        return false;
    }
    // `/^\s*$/`: without `D`, `$` also matches before a trailing `\n`,
    // so any all-whitespace byte string matches.
    if data.iter().all(|byte| is_php_space(*byte)) {
        out.push(text_message(
            path,
            0,
            0,
            CODE_EMPTY_FILE,
            SEVERITY_ERROR,
            NAME_EMPTY_FILE,
            DESC_EMPTY_FILE.to_string(),
            None,
            None,
        ));
        return true;
    }
    false
}

/// `lintNewlines()` (`ArcanistTextLinter.php:150-166`): any CR byte raises
/// `TXT1` at offset 0 with a whole-file patch, and stops all linters
/// (subject to the caller's severity map; the engine re-checks). Returns
/// true when the message fired.
fn lint_dos_newlines(path: &str, data: &[u8], out: &mut Vec<LintMessage>) -> bool {
    if !data.contains(&b'\r') {
        return false;
    }
    let original = String::from_utf8_lossy(data).into_owned();
    let replacement = original.replace("\r\n", "\n");
    out.push(text_message(
        path,
        1,
        1,
        CODE_DOS_NEWLINE,
        SEVERITY_ERROR,
        NAME_DOS_NEWLINE,
        DESC_DOS_NEWLINE.to_string(),
        Some(original),
        Some(replacement),
    ));
    true
}

/// `lintTabs()` (`ArcanistTextLinter.php:168-177`): only the FIRST tab is
/// reported, with the tab as non-patchable context.
fn lint_tabs(path: &str, data: &[u8], out: &mut Vec<LintMessage>) {
    let Some(offset) = data.iter().position(|byte| *byte == b'\t') else {
        return;
    };
    let (line, col) = line_col_for_offset(data, offset);
    out.push(text_message(
        path,
        line,
        col,
        CODE_TAB,
        SEVERITY_ERROR,
        NAME_TAB,
        DESC_TAB.to_string(),
        Some("\t".to_string()),
        None,
    ));
}

/// `lintLineLength()` (`ArcanistTextLinter.php:179-197`): lines longer
/// than `width` bytes raise `TXT3` warnings.
fn lint_line_length(path: &str, data: &[u8], width: usize, out: &mut Vec<LintMessage>) {
    for (index, line) in split_lines(data).iter().enumerate() {
        if line.len() > width {
            out.push(text_message(
                path,
                index + 1,
                1,
                CODE_LINE_WRAP,
                SEVERITY_WARNING,
                NAME_LINE_WRAP,
                format!(
                    "This line is {} characters long, but the convention is {} characters.",
                    format_number(line.len()),
                    format_number(width),
                ),
                Some(String::from_utf8_lossy(line).into_owned()),
                None,
            ));
        }
    }
}

/// `lintEOFNewline()` (`ArcanistTextLinter.php:199-209`): a missing final
/// newline raises patchable `TXT4` at end of file.
fn lint_eof_newline(path: &str, data: &[u8], out: &mut Vec<LintMessage>) {
    if data.last() == Some(&b'\n') {
        return;
    }
    let (line, col) = line_col_for_offset(data, data.len());
    out.push(text_message(
        path,
        line,
        col,
        CODE_EOF_NEWLINE,
        SEVERITY_ERROR,
        NAME_EOF_NEWLINE,
        DESC_EOF_NEWLINE.to_string(),
        Some(String::new()),
        Some("\n".to_string()),
    ));
}

/// True for bytes outside `[\x09\x0A\x20-\x7E]`
/// (`ArcanistTextLinter.php:215`).
fn is_bad_charset_byte(byte: u8) -> bool {
    !matches!(byte, 0x09 | 0x0A | 0x20..=0x7E)
}

/// `lintCharset()` (`ArcanistTextLinter.php:211-241`): the greedy
/// `/bad(.*bad)?/` match spans first-to-last bad byte per line, one
/// `TXT5` message each, and stops all linters. Returns true on fire.
fn lint_charset(path: &str, data: &[u8], out: &mut Vec<LintMessage>) -> bool {
    let mut fired = false;
    let mut offset = 0;
    for line in split_lines(data) {
        let mut first = None;
        let mut last = 0;
        for (index, byte) in line.iter().enumerate() {
            if is_bad_charset_byte(*byte) {
                if first.is_none() {
                    first = Some(index);
                }
                last = index;
            }
        }
        if let Some(start) = first {
            let span = &line[start..=last];
            let (line_no, col) = line_col_for_offset(data, offset + start);
            out.push(text_message(
                path,
                line_no,
                col,
                CODE_BAD_CHARSET,
                SEVERITY_ERROR,
                NAME_BAD_CHARSET,
                DESC_BAD_CHARSET.to_string(),
                Some(String::from_utf8_lossy(span).into_owned()),
                None,
            ));
            fired = true;
        }
        offset += line.len() + 1;
    }
    fired
}

/// `lintTrailingWhitespace()` (`ArcanistTextLinter.php:243-268`):
/// `/[[:blank:]]+$/m` — trailing spaces/tabs immediately before `\n` or
/// end of string. A `\r` breaks the match (no CR stripping).
fn lint_trailing_whitespace(path: &str, data: &[u8], out: &mut Vec<LintMessage>) {
    for (offset, len) in trailing_spans(data) {
        let (line, col) = line_col_for_offset(data, offset);
        out.push(text_message(
            path,
            line,
            col,
            CODE_TRAILING,
            SEVERITY_AUTOFIX,
            NAME_TRAILING,
            DESC_TRAILING.to_string(),
            Some(String::from_utf8_lossy(&data[offset..offset + len]).into_owned()),
            Some(String::new()),
        ));
    }
}

/// `lintBOFWhitespace()` (`ArcanistTextLinter.php:270-293`):
/// `/^\s*\n/` — leading whitespace ending in a newline raises `TXT8`.
fn lint_bof_whitespace(path: &str, data: &[u8], out: &mut Vec<LintMessage>) {
    let mut end = 0;
    while end < data.len() && is_php_space(data[end]) {
        end += 1;
    }
    // The match must end with `\n`: back the greedy prefix off to the
    // last newline (a prefix with no newline matches nothing).
    let mut match_end = end;
    while match_end > 0 && data[match_end - 1] != b'\n' {
        match_end -= 1;
    }
    if match_end == 0 {
        return;
    }
    out.push(text_message(
        path,
        1,
        1,
        CODE_BOF_WHITESPACE,
        SEVERITY_AUTOFIX,
        NAME_BOF_WHITESPACE,
        DESC_BOF_WHITESPACE.to_string(),
        Some(String::from_utf8_lossy(&data[..match_end]).into_owned()),
        Some(String::new()),
    ));
}

/// `lintEOFWhitespace()` (`ArcanistTextLinter.php:295-316`):
/// `/(?<=\n)\s+$/` — the earliest position after a newline with only
/// whitespace to the end raises `TXT9`.
fn lint_eof_whitespace(path: &str, data: &[u8], out: &mut Vec<LintMessage>) {
    let mut start = None;
    for candidate in 1..data.len() {
        if data[candidate - 1] == b'\n' && data[candidate..].iter().all(|b| is_php_space(*b)) {
            start = Some(candidate);
            break;
        }
    }
    let Some(start) = start else {
        return;
    };
    let (line, col) = line_col_for_offset(data, start);
    out.push(text_message(
        path,
        line,
        col,
        CODE_EOF_WHITESPACE,
        SEVERITY_AUTOFIX,
        NAME_EOF_WHITESPACE,
        DESC_EOF_WHITESPACE.to_string(),
        Some(String::from_utf8_lossy(&data[start..]).into_owned()),
        Some(String::new()),
    ));
}

/// Split bytes on `\n`, dropping the terminators (PHP `explode()`).
fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(&data[start..index]);
            start = index + 1;
        }
    }
    lines.push(&data[start..]);
    lines
}

/// 1-based (line, col) for a byte offset, counting bytes like the PHP engine.
fn line_col_for_offset(data: &[u8], offset: usize) -> (usize, usize) {
    let end = offset.min(data.len());
    let mut line = 1;
    let mut col = 1;
    for &byte in &data[..end] {
        if byte == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Byte spans of trailing `[ \t]+` runs, one per affected line, mirroring
/// `/[[:blank:]]+$/m`: the run must sit immediately before `\n` or end of
/// string. A `\r` breaks the match.
fn trailing_spans(data: &[u8]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, &byte) in data.iter().enumerate() {
        if byte == b'\n' {
            if let Some(span) = trailing_in_line(data, start, index) {
                spans.push(span);
            }
            start = index + 1;
        }
    }
    if start < data.len()
        && let Some(span) = trailing_in_line(data, start, data.len())
    {
        spans.push(span);
    }
    spans
}

fn trailing_in_line(data: &[u8], start: usize, end: usize) -> Option<(usize, usize)> {
    if start >= end {
        return None;
    }
    let mut len = 0;
    for &byte in data[start..end].iter().rev() {
        if byte == b' ' || byte == b'\t' {
            len += 1;
        } else {
            break;
        }
    }
    if len == 0 {
        None
    } else {
        Some((end - len, len))
    }
}

/// Runs a set of linters over files.
pub struct Engine {
    linters: Vec<Box<dyn Linter>>,
    /// Exact-code severity overrides per display name, merged first-wins
    /// from every config section selecting that linter.
    severity_maps: HashMap<String, HashMap<String, String>>,
    /// Route linters per path like [`ENGINE_COMPREHENSIVE`].
    comprehensive: bool,
}

/// Every ported linter, in registration order. `lint_external` stays out:
/// the PHP base class is abstract and defines no linter of its own.
#[must_use]
pub fn all_linters() -> Vec<Box<dyn Linter>> {
    all_linters_with_text_options(DEFAULT_MAX_LINE_LENGTH, HashMap::new())
}

/// Every ported linter with custom text options.
fn all_linters_with_text_options(
    max_line_length: usize,
    text_severity_map: HashMap<String, String>,
) -> Vec<Box<dyn Linter>> {
    let mut text = TextLinter::with_max_line_length(max_line_length);
    text.severity_map = text_severity_map;
    vec![
        Box::new(text),
        Box::new(crate::lint_chmod::ChmodLinter::new()),
        Box::new(crate::lint_closure::ClosureLinter::new()),
        Box::new(crate::lint_coffeelint::CoffeelintLinter::new()),
        Box::new(crate::lint_composer::ComposerLinter::new()),
        Box::new(crate::lint_cppcheck::CppcheckLinter::new()),
        Box::new(crate::lint_cpplint::CpplintLinter::new()),
        Box::new(crate::lint_csharp::CsharpLinter::new()),
        Box::new(crate::lint_csslint::CsslintLinter::new()),
        Box::new(crate::lint_filename::FilenameLinter::new()),
        Box::new(crate::lint_flake8::Flake8Linter::new()),
        Box::new(crate::lint_generated::GeneratedLinter::new()),
        Box::new(crate::lint_golint::GolintLinter::new()),
        Box::new(crate::lint_hlint::HlintLinter::new()),
        Box::new(crate::lint_jscs::JscsLinter::new()),
        Box::new(crate::lint_jshint::JshintLinter::new()),
        Box::new(crate::lint_json::JsonLinter::new()),
        Box::new(crate::lint_jsonlint::JsonlintLinter::new()),
        Box::new(crate::lint_lessc::LesscLinter::new()),
        Box::new(crate::lint_merge_conflict::MergeConflictLinter::new()),
        Box::new(crate::lint_nolint::NoLintLinter::new()),
        Box::new(crate::lint_pep8::Pep8Linter::new()),
        Box::new(crate::lint_php::PhpLinter::new()),
        Box::new(crate::lint_phpcs::PhpcsLinter::new()),
        Box::new(crate::lint_phutil_library::PhutilLibraryLinter::new()),
        Box::new(crate::lint_puppetlint::PuppetlintLinter::new()),
        Box::new(crate::lint_pyflakes::PyFlakesLinter::new()),
        Box::new(crate::lint_pylint::PyLintLinter::new()),
        Box::new(crate::lint_rubocop::RuboCopLinter::new()),
        Box::new(crate::lint_ruby::RubyLinter::new()),
        Box::new(crate::lint_script_and_regex::ScriptAndRegexLinter::new()),
        Box::new(crate::lint_spelling::SpellingLinter::new()),
        Box::new(crate::lint_xhpast::XhpastLinter::new()),
        Box::new(crate::lint_xml::XmlLinter::new()),
    ]
}

/// File extensions routed to the text linters by
/// `ArcanistComprehensiveLintEngine` (`/\.(php|css|hpp|cpp|l|y|py|pl)$/`).
const COMPREHENSIVE_TEXT_EXTENSIONS: &[&str] = &["php", "css", "hpp", "cpp", "l", "y", "py", "pl"];

/// Comprehensive-engine routing (`ArcanistComprehensiveLintEngine.php`),
/// as the display names running on `path`, or `None` when `path` is
/// excluded (`^externals/`).
fn comprehensive_linters_for(path: &str) -> Option<Vec<&'static str>> {
    if path.starts_with("externals/") {
        return None;
    }
    let ext = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let mut out = vec!["NAME"];
    if COMPREHENSIVE_TEXT_EXTENSIONS.contains(&ext) {
        out.extend(["GEN", "NOLINT", "TXT"]);
    }
    if ext == "php" {
        out.push("XHP");
    }
    if ext == "py" {
        out.extend(["PYFLAKES", "PEP8"]);
    }
    if ext == "rb" {
        out.push("RUBY");
    }
    if ext == "js" {
        out.push("JSHint");
    }
    Some(out)
}

/// Find the nearest `.arclint` file at or above `start`. A file `start`
/// resolves from its parent directory.
#[must_use]
pub fn discover_arclint(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent().map(Path::to_path_buf)?
    } else {
        start.to_path_buf()
    };
    loop {
        let candidate = dir.join(crate::arclint::CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// Read `text.max-line-length` from the first `text` section defining it.
/// PHP validates `optional int` per section; a non-integer value is a
/// config error.
fn text_length_from_config(config: &arclint::ArclintConfig) -> Result<usize, String> {
    for spec in &config.linters {
        if spec.display != "TXT" {
            continue;
        }
        let Some(value) = spec.extra.get("text.max-line-length") else {
            continue;
        };
        let Some(length) = value.as_u64() else {
            return Err(format!(
                "Error in parsing '.arclint' file, for linter '{}': 'text.max-line-length' must be an integer",
                spec.name,
            ));
        };
        let Ok(length) = usize::try_from(length) else {
            return Err(format!(
                "Error in parsing '.arclint' file, for linter '{}': 'text.max-line-length' is too large",
                spec.name,
            ));
        };
        return Ok(length);
    }
    Ok(DEFAULT_MAX_LINE_LENGTH)
}

/// Merge exact-code severity maps per display name, first config section
/// wins per code.
fn severity_maps_from_config(
    config: &arclint::ArclintConfig,
) -> HashMap<String, HashMap<String, String>> {
    let mut maps: HashMap<String, HashMap<String, String>> = HashMap::new();
    // Iterate in reverse so the first section wins via `extend`.
    for spec in config.linters.iter().rev() {
        maps.entry(spec.display.clone())
            .or_default()
            .extend(spec.severity.map.clone());
    }
    maps
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            linters: all_linters(),
            severity_maps: HashMap::new(),
            comprehensive: false,
        }
    }

    /// An engine running only the linters `config` selects, matched by
    /// exact PHP display name against [`all_linters`], with exact-code
    /// severity maps and `text.max-line-length` applied.
    #[must_use]
    pub fn from_config(config: &arclint::ArclintConfig) -> Self {
        let max_line_length = text_length_from_config(config).unwrap_or(DEFAULT_MAX_LINE_LENGTH);
        let severity_maps = severity_maps_from_config(config);
        let text_map = severity_maps.get("TXT").cloned().unwrap_or_default();
        let mut linters = all_linters_with_text_options(max_line_length, text_map);
        arclint::retain_selected(&mut linters, |linter| linter.name(), config);
        Self {
            linters,
            severity_maps,
            comprehensive: false,
        }
    }

    /// A comprehensive engine: every ported linter, routed per path by
    /// [`comprehensive_linters_for`].
    #[must_use]
    pub fn comprehensive() -> Self {
        Self {
            linters: all_linters(),
            severity_maps: HashMap::new(),
            comprehensive: true,
        }
    }

    /// Keep only the linter with display name `display`.
    pub fn retain_display(&mut self, display: &str) {
        self.linters.retain(|linter| linter.name() == display);
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            linters: Vec::new(),
            severity_maps: HashMap::new(),
            comprehensive: false,
        }
    }

    pub fn add_linter(&mut self, linter: Box<dyn Linter>) {
        self.linters.push(linter);
    }

    /// True when the engine runs no linters (`'No linters to run.'`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.linters.is_empty()
    }

    /// True for [`Engine::comprehensive`].
    #[must_use]
    pub fn is_comprehensive(&self) -> bool {
        self.comprehensive
    }

    /// Resolve the effective severity for a message, mirroring
    /// `getLintMessageSeverity()`
    /// (`src/lint/linter/ArcanistLinter.php:350-368`): the exact custom
    /// map first (keyed by full code, then by the short code with the
    /// display-name prefix stripped), else the linter's own severity.
    /// `severity.rules` patterns need a regex engine and never match.
    #[must_use]
    pub fn severity_for(&self, display: &str, full_code: &str, default: &str) -> String {
        let Some(map) = self.severity_maps.get(display) else {
            return default.to_string();
        };
        if let Some(severity) = map.get(full_code) {
            return severity.clone();
        }
        if let Some(short) = full_code.strip_prefix(display)
            && let Some(severity) = map.get(short)
        {
            return severity.clone();
        }
        default.to_string()
    }

    /// Linters in execution order: stable by ascending
    /// [`Linter::priority`] (`msort($linters, 'getLinterPriority')`,
    /// `ArcanistLintEngine.php:165`).
    fn ordered_linters(&self) -> Vec<&dyn Linter> {
        let mut linters: Vec<&dyn Linter> = self.linters.iter().map(AsRef::as_ref).collect();
        linters.sort_by(|a, b| {
            a.priority()
                .partial_cmp(&b.priority())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        linters
    }

    fn apply_severity_maps(&self, display: &str, messages: &mut [LintMessage]) {
        if !self.severity_maps.contains_key(display) {
            return;
        }
        for message in messages.iter_mut() {
            message.severity = self.severity_for(display, &message.code.clone(), &message.severity);
        }
    }

    /// True when a text stop code fired and is not custom-disabled,
    /// mirroring `isMessageEnabled()`
    /// (`ArcanistLinter.php:374-377`): only the exact custom map can
    /// disable it here.
    fn text_stop_enabled(&self, full_code: &str) -> bool {
        self.severity_for("TXT", full_code, SEVERITY_ERROR) != SEVERITY_DISABLED
    }

    #[must_use]
    pub fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        let routed = self.comprehensive.then(|| comprehensive_linters_for(path));
        if let Some(None) = routed {
            return Vec::new();
        }
        let allowed = routed.flatten();
        let runs = |name: &str| {
            self.linters.iter().any(|linter| linter.name() == name)
                && allowed.as_ref().is_none_or(|names| names.contains(&name))
        };

        // PHP `shouldStopAllLinters`: generated and `@nolint` files skip
        // every linter, but only when the stopping linter (`GEN`,
        // `NOLINT`) runs on this path.
        let generated = runs("GEN") && crate::lint_generated::is_generated(data);
        let nolint = runs("NOLINT") && crate::lint_nolint::is_nolint(data);
        if generated || nolint {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut stopped = false;
        for linter in self.ordered_linters() {
            if stopped {
                break;
            }
            if let Some(names) = &allowed
                && !names.contains(&linter.name())
            {
                continue;
            }
            let mut messages = linter.lint_bytes(path, data);
            self.apply_severity_maps(linter.name(), &mut messages);
            // Text stops (`lintEmptyFile`, `lintNewlines`, `lintCharset`):
            // an empty file always stops; DOS newlines and bad charsets
            // stop unless custom-disabled.
            if linter.name() == "TXT" {
                if messages.iter().any(|m| m.code == CODE_EMPTY_FILE) {
                    stopped = true;
                } else {
                    if messages.iter().any(|m| m.code == CODE_DOS_NEWLINE)
                        && self.text_stop_enabled(CODE_DOS_NEWLINE)
                    {
                        stopped = true;
                    }
                    if messages.iter().any(|m| m.code == CODE_BAD_CHARSET)
                        && self.text_stop_enabled(CODE_BAD_CHARSET)
                    {
                        stopped = true;
                    }
                }
            }
            out.extend(messages);
        }
        out.sort_by(|a, b| {
            (&a.path, a.line, a.col, &a.code).cmp(&(&b.path, b.line, b.col, &b.code))
        });
        out
    }

    /// Lint one file from disk, applying each linter's `shouldLint*()`
    /// path filters like the PHP engine (`ArcanistLinter.php:294-319`):
    /// missing files lint nowhere; directories and symlinks only reach
    /// linters opting in (the filename linter); binary files (NUL byte)
    /// only reach linters opting in (filename, chmod).
    pub fn lint_file(&self, disk: &Path, display: &str) -> Result<Vec<LintMessage>, String> {
        if self.comprehensive && comprehensive_linters_for(display).is_none() {
            return Ok(Vec::new());
        }
        let meta = match std::fs::symlink_metadata(disk) {
            Err(_) => return Ok(Vec::new()),
            Ok(meta) => meta,
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            return Ok(self
                .lint_bytes_filtered(display, b"", |linter| linter.should_lint_symbolic_links()));
        }
        if file_type.is_dir() {
            return Ok(
                self.lint_bytes_filtered(display, b"", |linter| linter.should_lint_directories())
            );
        }
        let data = std::fs::read(disk).map_err(|e| format!("cannot read: {e}"))?;
        if data.contains(&0) {
            return Ok(self
                .lint_bytes_filtered(display, &data, |linter| linter.should_lint_binary_files()));
        }
        Ok(self.lint_bytes(display, &data))
    }

    /// Run the linters accepting `filter` over one file's bytes. Stops
    /// cannot trigger here: no stopping linter opts into filtered paths.
    fn lint_bytes_filtered(
        &self,
        path: &str,
        data: &[u8],
        filter: impl Fn(&dyn Linter) -> bool,
    ) -> Vec<LintMessage> {
        let mut linters = self.ordered_linters();
        linters.retain(|linter| filter(*linter));
        let allowed = self
            .comprehensive
            .then(|| comprehensive_linters_for(path))
            .flatten();
        let mut out = Vec::new();
        for linter in linters {
            if let Some(names) = &allowed
                && !names.contains(&linter.name())
            {
                continue;
            }
            let mut messages = linter.lint_bytes(path, data);
            self.apply_severity_maps(linter.name(), &mut messages);
            out.extend(messages);
        }
        out.sort_by(|a, b| {
            (&a.path, a.line, a.col, &a.code).cmp(&(&b.path, b.line, b.col, &b.code))
        });
        out
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/* -(  Working copies  )----------------------------------------------------- */

/// Version control systems `arc lint` supports
/// (`ArcanistLintWorkflow.php:50`: git, svn, hg).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vcs {
    Git,
    Hg,
    Svn,
}

impl Vcs {
    /// `getSourceControlSystemName()`: `git`, `hg`, `svn`.
    #[must_use]
    pub fn system_name(self) -> &'static str {
        match self {
            Vcs::Git => "git",
            Vcs::Hg => "hg",
            Vcs::Svn => "svn",
        }
    }

    #[must_use]
    pub fn supports_commit_ranges(self) -> bool {
        match self {
            Vcs::Git => git::supports_commit_ranges(),
            Vcs::Hg => hg::supports_commit_ranges(),
            Vcs::Svn => svn::supports_commit_ranges(),
        }
    }

    #[must_use]
    pub fn is_history_default_immutable(self) -> bool {
        match self {
            Vcs::Git => git::is_history_default_immutable(),
            Vcs::Hg => hg::is_history_default_immutable(),
            Vcs::Svn => svn::is_history_default_immutable(),
        }
    }

    /// `supportsAmend()`. Mercurial support is probed per working copy.
    pub fn supports_amend(self, root: &Path) -> bool {
        match self {
            Vcs::Git => git::supports_amend(),
            Vcs::Hg => hg::supports_amend(root).unwrap_or(false),
            Vcs::Svn => svn::supports_amend(),
        }
    }
}

/// A discovered working copy: VCS kind, VCS root, and project root. For
/// git/hg the project root is the VCS root; for svn it is the nearest
/// `.arcconfig` above the start directory, else the VCS root
/// (`ArcanistWorkingCopyIdentity.php:87-106`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCopy {
    pub vcs: Vcs,
    pub vcs_root: PathBuf,
    pub project_root: PathBuf,
}

/// Discover the working copy around `start`, innermost VCS wins: walk
/// upward to the nearest directory containing `.git`, `.hg`, or `.svn`
/// (checked in that order per directory). PHP instead walks from the
/// filesystem root and takes the outermost match; see the module docs.
pub fn discover_working_copy(start: &Path) -> Result<WorkCopy, String> {
    let mut dir = if start.is_file() {
        start
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| not_in_working_copy(start))?
    } else {
        start.to_path_buf()
    };
    loop {
        for (vcs, marker) in [(Vcs::Git, ".git"), (Vcs::Hg, ".hg"), (Vcs::Svn, ".svn")] {
            if dir.join(marker).exists() {
                let project_root = project_root_for(dir.clone(), start, vcs);
                return Ok(WorkCopy {
                    vcs,
                    vcs_root: dir,
                    project_root,
                });
            }
        }
        dir = dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| not_in_working_copy(start))?;
    }
}

fn not_in_working_copy(start: &Path) -> String {
    format!("Path \"{}\" is not inside a working copy.", start.display())
}

/// Project root for a VCS root: nearest `.arcconfig` at or below the
/// start for svn, else the VCS root itself.
fn project_root_for(vcs_root: PathBuf, start: &Path, vcs: Vcs) -> PathBuf {
    if vcs != Vcs::Svn {
        return vcs_root;
    }
    let mut dir = if start.is_file() {
        start.parent().map(Path::to_path_buf)
    } else {
        Some(start.to_path_buf())
    };
    while let Some(current) = dir {
        if current.join(".arcconfig").is_file() {
            return current;
        }
        if current == vcs_root {
            break;
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    vcs_root
}

/// Read one key from the project `.arcconfig` (`getConfigFromAnySource()`
/// restricted to the project layer). A missing file yields `None`; an
/// invalid one is an error.
pub fn arcconfig_value(
    project_root: &Path,
    key: &str,
) -> Result<Option<serde_json::Value>, String> {
    let path = project_root.join(".arcconfig");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| format!("Unable to parse '.arcconfig' file '{}'.", path.display()))?;
    Ok(parsed.get(key).cloned())
}

fn arcconfig_string(project_root: &Path, key: &str) -> Result<Option<String>, String> {
    match arcconfig_value(project_root, key)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) if text.is_empty() => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text)),
        Some(_) => Ok(None),
    }
}

fn arcconfig_bool(project_root: &Path, key: &str) -> Result<Option<bool>, String> {
    match arcconfig_value(project_root, key)? {
        Some(serde_json::Value::Bool(value)) => Ok(Some(value)),
        _ => Ok(None),
    }
}

/* -(  Base commits  )-------------------------------------------------------- */

/// Resolve an explicit `--rev` symbol to the diff base, mirroring the
/// explicit branch of `buildBaseCommit()`: the merge-base of the symbol
/// and `HEAD`.
fn git_base_for_rev(work: &WorkCopy, rev: &str) -> Result<String, String> {
    git::base_commit_for_explicit_symbol(&work.vcs_root, rev, git::current_commit_symbol())
}

/// Zero/one-commit detection (`ArcanistGitAPI.php:245-263`): a repository
/// whose `HEAD^` does not verify uses the empty tree as base.
fn git_repository_has_two_commits(root: &Path) -> bool {
    git::output(root, &["rev-parse", "--verify", "HEAD^"]).is_ok_and(|out| out.status.success())
}

/// Default git base commit (`ArcanistGitAPI.php:245-388`):
/// `.arcconfig` `git.default-relative-commit`, else `@{upstream}`, else
/// the `default-relative-commit` scratch file, else an interactive
/// prompt (default `origin/master`, written back to scratch).
fn git_default_base(
    work: &WorkCopy,
    input: &mut dyn BufRead,
    output: &mut dyn FnMut(&str),
) -> Result<String, String> {
    if !git_repository_has_two_commits(&work.vcs_root) {
        return Ok(git::GIT_MAGIC_ROOT_COMMIT.to_string());
    }
    if let Some(relative) = arcconfig_string(&work.project_root, "git.default-relative-commit")? {
        return git_merge_base_with_head(&work.vcs_root, &relative);
    }
    if let Ok(out) = git::output(
        &work.vcs_root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    ) && out.status.success()
    {
        let upstream = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !upstream.is_empty() {
            return git_merge_base_with_head(&work.vcs_root, &upstream);
        }
    }
    if let Some(relative) = read_default_relative_commit(&work.vcs_root) {
        return git_merge_base_with_head(&work.vcs_root, &relative);
    }
    let relative = prompt_default_relative_commit(input, output);
    git_check_relative_commit(&work.vcs_root, &relative)?;
    write_default_relative_commit(&work.vcs_root, &relative);
    git_merge_base_with_head(&work.vcs_root, &relative)
}

fn git_merge_base_with_head(root: &Path, relative: &str) -> Result<String, String> {
    let out = git::output(root, &["merge-base", "--", relative, "HEAD"])?;
    if !out.status.success() {
        return Err(git::unknown_commit_message(relative));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `cat-file -t` must report a commit
/// (`ArcanistGitAPI.php:362-371`).
fn git_check_relative_commit(root: &Path, relative: &str) -> Result<(), String> {
    let out = git::output(root, &["cat-file", "-t", relative])?;
    if !out.status.success() || String::from_utf8_lossy(&out.stdout).trim() != "commit" {
        return Err(git::relative_commit_not_commit_message(relative));
    }
    Ok(())
}

fn git_scratch_path(root: &Path) -> Option<PathBuf> {
    let meta = git::metadata_path(root).ok()?;
    Some(meta.join("arc").join("default-relative-commit"))
}

fn read_default_relative_commit(root: &Path) -> Option<String> {
    let path = git_scratch_path(root)?;
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn write_default_relative_commit(root: &Path, relative: &str) {
    let Some(path) = git_scratch_path(root) else {
        return;
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, relative).ok();
}

/// The "Select a Default Commit Range" prompt
/// (`ArcanistGitAPI.php:320-360`), markup stripped. Returns the chosen
/// relative commit (`origin/master` when the answer is blank).
fn prompt_default_relative_commit(input: &mut dyn BufRead, output: &mut dyn FnMut(&str)) -> String {
    output("** Select a Default Commit Range **\n\n");
    output(&word_wrap(
        "You're running a command which operates on a range of revisions (usually, from some revision to HEAD) but have not specified the revision that should determine the start of the range.\n\nPreviously, arc assumed you meant 'HEAD^' when you did not specify a start revision, but this behavior does not make much sense in most workflows outside of Facebook's historic git-svn workflow.\n\narc no longer assumes 'HEAD^'. You must specify a relative commit explicitly when you invoke a command (e.g., `arc diff HEAD^`, not just `arc diff`) or select a default for this working copy.\n\nIn most cases, the best default is 'origin/master'. You can also select 'HEAD^' to preserve the old behavior, or some other remote or branch. But you almost certainly want to select 'origin/master'.\n\n(Technically: the merge-base of the selected revision and HEAD is used to determine the start of the commit range.)",
        0,
    ));
    output("\n");
    output("What default do you want to use? [origin/master] ");
    let mut line = String::new();
    input.read_line(&mut line).ok();
    let trimmed = line.trim();
    if trimmed.is_empty() {
        "origin/master".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Mercurial base commit: explicit `--rev` resolves against the working
/// copy (`resolve_explicit_base()`), else the automatic base.
fn hg_base_for(work: &WorkCopy, rev: Option<&str>) -> Result<String, String> {
    if let Some(rev) = rev {
        return hg::resolve_explicit_base(&work.vcs_root, rev).map(|(base, _)| base);
    }
    let branch = hg::get_branch_name(&work.vcs_root)?;
    hg::auto_base_commit(&work.vcs_root, &branch, ".").map(|(base, _)| base)
}

/* -(  Path selection  )------------------------------------------------------ */

/// Display path for lint output: relative to the project root when
/// contained (PHP `Filesystem::readablePath()`), else as given.
fn display_for(disk: &Path, project_root: &Path) -> String {
    if let Ok(rel) = disk.strip_prefix(project_root)
        && !rel.as_os_str().is_empty()
    {
        return rel.to_string_lossy().to_string();
    }
    disk.to_string_lossy().to_string()
}

/// Expand explicit CLI paths, mirroring `selectPathsForWorkflow()`
/// (`ArcanistWorkflow.php:1622-1663`): every path must exist (files,
/// directories, and symlinks alike; directories are NOT walked), and
/// display paths are project-relative. Relative paths resolve from
/// `cwd`.
pub fn collect_explicit(
    paths: &[String],
    cwd: &Path,
    project_root: &Path,
) -> Result<Vec<(PathBuf, String)>, String> {
    let mut out = Vec::new();
    for path in paths {
        let disk = PathBuf::from(path);
        let probe = if disk.is_absolute() {
            disk.clone()
        } else {
            cwd.join(&disk)
        };
        if std::fs::symlink_metadata(&probe).is_err() {
            return Err(format!("Path '{path}' does not exist!"));
        }
        let absolute = if disk.is_absolute() {
            disk
        } else {
            cwd.join(&disk)
        };
        let display = display_for(&absolute, project_root);
        out.push((absolute, display));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

/// Tracked working-copy status paths (range plus uncommitted, minus
/// untracked), mirroring `getWorkingCopyStatus()` with the default omit
/// mask (`FLAG_UNTRACKED`). `base` is the resolved commit-range base;
/// `None` skips the range (svn, or callers wanting uncommitted only).
fn status_paths(work: &WorkCopy, base: Option<&str>) -> Result<Vec<String>, String> {
    let mut merged: BTreeMap<String, u32> = BTreeMap::new();
    match work.vcs {
        Vcs::Git => {
            let root = &work.vcs_root;
            if let Some(base) = base {
                for (path, mask) in git::commit_range_status(root, base)? {
                    *merged.entry(path).or_default() |= mask;
                }
            }
            for (path, mask) in git::uncommitted_status(root)? {
                *merged.entry(path).or_default() |= mask;
            }
            merged.retain(|_, mask| *mask & git::FLAG_UNTRACKED == 0);
        }
        Vcs::Hg => {
            let root = &work.vcs_root;
            if let Some(base) = base {
                for (path, mask) in hg::get_commit_range_status(root, base)? {
                    *merged.entry(path).or_default() |= mask;
                }
            }
            for (path, mask) in hg::get_uncommitted_status(root)? {
                *merged.entry(path).or_default() |= mask;
            }
            merged.retain(|_, mask| *mask & hg::FLAG_UNTRACKED == 0);
        }
        Vcs::Svn => {
            let status = svn::get_uncommitted_status(&work.vcs_root, &[])?;
            for (path, mask) in status {
                if mask & svn::FLAG_UNTRACKED == 0 {
                    merged.insert(path, mask);
                }
            }
        }
    }
    Ok(merged.keys().cloned().collect())
}

/// All tracked files (`--everything`), mirroring `getAllFiles()`.
fn everything_paths(work: &WorkCopy) -> Result<Vec<String>, String> {
    match work.vcs {
        Vcs::Git => git::all_files(&work.vcs_root),
        Vcs::Hg => hg::get_all_files(&work.vcs_root),
        Vcs::Svn => svn::get_all_files(&work.vcs_root),
    }
}

/// Tracked files changed in the working tree (staged plus unstaged,
/// plus the commit range when `base` is set), as `(disk_path,
/// display_path)` with repo-relative display paths.
pub fn changed_files_in(dir: &Path, base: Option<&str>) -> Result<Vec<(PathBuf, String)>, String> {
    let work = discover_working_copy(dir)?;
    let names = status_paths(&work, base)?;
    Ok(names
        .into_iter()
        .map(|rel| (work.vcs_root.join(&rel), rel))
        .collect())
}

/// Parse unified-diff hunks, collecting new-side `+` line numbers like
/// `ArcanistDiffHunk::getChangedLines('new')`
/// (`src/parser/diff/ArcanistDiffHunk.php:48-80`): only added lines
/// count; `\ No newline` and empty lines advance like context.
pub fn parse_changed_lines(diff: &str) -> BTreeSet<usize> {
    let mut changed = BTreeSet::new();
    let mut new_line = 0;
    let mut in_hunk = false;
    for line in diff.split('\n') {
        if let Some(rest) = line.strip_prefix("@@ ") {
            if let Some(new_start) = parse_hunk_new_start(rest) {
                new_line = new_start;
                in_hunk = true;
            } else {
                in_hunk = false;
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => {
                changed.insert(new_line);
                new_line += 1;
            }
            Some(b'-') => {}
            _ => {
                new_line += 1;
            }
        }
    }
    changed
}

/// Parse the `+start[,len]` half of a hunk header (`@@ -a,b +c,d @@`).
/// A zero-length hunk (`+c,0`) starts *after* line `c`.
fn parse_hunk_new_start(header: &str) -> Option<usize> {
    let plus = header.find('+')?;
    let after = &header[plus + 1..];
    let end = after.find(|c: char| !c.is_ascii_digit())?;
    let start: usize = after[..end].parse().ok()?;
    let rest = &after[end..];
    if let Some(len_text) = rest.strip_prefix(',') {
        let len_end = len_text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(len_text.len());
        if len_text[..len_end].parse::<usize>().ok() == Some(0) {
            return Some(start + 1);
        }
    }
    Some(start)
}

/// New-side changed lines for `path`, or `None` when the path is a
/// directory, missing, or binary (PHP `getChangedLines()` returns null
/// there; `ArcanistWorkflow.php:1423-1442`).
pub fn changed_lines_for(
    work: &WorkCopy,
    base: &str,
    path: &str,
) -> Result<Option<BTreeSet<usize>>, String> {
    let disk = work.vcs_root.join(path);
    if disk.is_dir() || std::fs::symlink_metadata(&disk).is_err() {
        return Ok(None);
    }
    // Deleted paths vanish from the worktree; added-then-deleted churn
    // yields an empty change (`ArcanistWorkflow.php:1475-1482`).
    let diff = match work.vcs {
        Vcs::Git => git::raw_diff_text(&work.vcs_root, base, path, DIFF_CONTEXT_LINES, true)?,
        Vcs::Hg => hg::get_raw_diff_text(&work.vcs_root, base, path, DIFF_CONTEXT_LINES)?,
        Vcs::Svn => {
            let status = svn::get_uncommitted_status(&work.vcs_root, &[path])?;
            match svn::get_raw_diff_text(&work.vcs_root, path, &status, DIFF_CONTEXT_LINES)? {
                Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                None => return Ok(Some(BTreeSet::new())),
            }
        }
    };
    if diff.contains("Binary files ") && diff.contains("differ") {
        return Ok(None);
    }
    Ok(Some(parse_changed_lines(&diff)))
}

/* -(  Arguments  )------------------------------------------------------------ */

/// Parsed `lint` options. Flag names are verbatim from
/// `ArcanistLintWorkflow::getArguments()` (`:57-133`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Show all warnings, not just changed-line ones (`lintall`, `:59-63`).
    pub lintall: bool,
    /// Lint changes since a revision (`rev`, `:64-74`).
    pub rev: Option<String>,
    /// Output format (`output`, `:75-78`).
    pub output: Option<String>,
    /// Output file (`outfile`, `:79-83`).
    pub outfile: Option<String>,
    /// Engine class override (`engine`, `:84-87`).
    pub engine: Option<String>,
    /// Apply patches without prompting (`apply-patches`, `:88-95`).
    pub apply_patches: bool,
    /// Never apply patches (`never-apply-patches`, `:96-101`).
    pub never_apply_patches: bool,
    /// Amend HEAD with all patches (`amend-all`, `:102-106`).
    pub amend_all: bool,
    /// Amend HEAD with autofix patches (`amend-autofixes`, `:107-111`).
    pub amend_autofixes: bool,
    /// Lint all tracked files (`everything`, `:112-119`).
    pub everything: bool,
    /// Minimum severity (`severity`, `:120-130`).
    pub severity: Option<String>,
    /// Positional paths (the `*` wildcard, `:131`).
    pub paths: Vec<String>,
    /// Canonical long flag names in first-provision order (PHP `$dict`
    /// insertion order). Empty for hand-built options.
    pub provided: Vec<&'static str>,
}

/// A value-taking `lint` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueFlag {
    Rev,
    Output,
    Outfile,
    Engine,
    Severity,
}

/// A boolean `lint` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolFlag {
    Lintall,
    ApplyPatches,
    NeverApplyPatches,
    AmendAll,
    AmendAutofixes,
    Everything,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Value(ValueFlag),
    Bool(BoolFlag),
}

impl Flag {
    fn takes_param(self) -> bool {
        matches!(self, Flag::Value(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlagSpec {
    long: &'static str,
    flag: Flag,
}

/// `lint` flags in `getArguments()` order
/// (`src/workflow/ArcanistLintWorkflow.php:57-133`). No short aliases.
const FLAG_SPECS: &[FlagSpec] = &[
    FlagSpec {
        long: "lintall",
        flag: Flag::Bool(BoolFlag::Lintall),
    },
    FlagSpec {
        long: "rev",
        flag: Flag::Value(ValueFlag::Rev),
    },
    FlagSpec {
        long: "output",
        flag: Flag::Value(ValueFlag::Output),
    },
    FlagSpec {
        long: "outfile",
        flag: Flag::Value(ValueFlag::Outfile),
    },
    FlagSpec {
        long: "engine",
        flag: Flag::Value(ValueFlag::Engine),
    },
    FlagSpec {
        long: "apply-patches",
        flag: Flag::Bool(BoolFlag::ApplyPatches),
    },
    FlagSpec {
        long: "never-apply-patches",
        flag: Flag::Bool(BoolFlag::NeverApplyPatches),
    },
    FlagSpec {
        long: "amend-all",
        flag: Flag::Bool(BoolFlag::AmendAll),
    },
    FlagSpec {
        long: "amend-autofixes",
        flag: Flag::Bool(BoolFlag::AmendAutofixes),
    },
    FlagSpec {
        long: "everything",
        flag: Flag::Bool(BoolFlag::Everything),
    },
    FlagSpec {
        long: "severity",
        flag: Flag::Value(ValueFlag::Severity),
    },
];

fn lookup_long(name: &str) -> Option<FlagSpec> {
    FLAG_SPECS.iter().find(|spec| spec.long == name).copied()
}

fn store_value_flag(options: &mut Options, flag: ValueFlag, value: String) {
    match flag {
        ValueFlag::Rev => options.rev = Some(value),
        ValueFlag::Output => options.output = Some(value),
        ValueFlag::Outfile => options.outfile = Some(value),
        ValueFlag::Engine => options.engine = Some(value),
        ValueFlag::Severity => options.severity = Some(value),
    }
}

fn store_bool_flag(options: &mut Options, flag: BoolFlag) {
    match flag {
        BoolFlag::Lintall => options.lintall = true,
        BoolFlag::ApplyPatches => options.apply_patches = true,
        BoolFlag::NeverApplyPatches => options.never_apply_patches = true,
        BoolFlag::AmendAll => options.amend_all = true,
        BoolFlag::AmendAutofixes => options.amend_autofixes = true,
        BoolFlag::Everything => options.everything = true,
    }
}

/// Record one parsed flag in `options` (`$dict[$arg_key] = ...`,
/// `ArcanistWorkflow.php:867-883`), tracking first-provision order.
fn store_flag(options: &mut Options, spec: FlagSpec, value: Option<String>) {
    if !options.provided.contains(&spec.long) {
        options.provided.push(spec.long);
    }
    match spec.flag {
        Flag::Value(value_flag) => {
            store_value_flag(options, value_flag, value.unwrap_or_default());
        }
        Flag::Bool(bool_flag) => store_bool_flag(options, bool_flag),
    }
}

/// Parse one `--name` or `--name=value` token
/// (`ArcanistWorkflow.php:824-851`). Values may be empty and may look
/// like flags; only a trailing `param` flag errors (`:871-876`). No
/// `lint` flag name has a British-variant pair, so unknown names always
/// error (the spelling corrector, `:834-843`, can never fire).
fn parse_long_flag(
    options: &mut Options,
    queue: &mut std::collections::VecDeque<String>,
    typed: &str,
    name: &str,
) -> Result<(), String> {
    let (key, inline) = match name.split_once('=') {
        Some((key, value)) => (key, Some(value)),
        None => (name, None),
    };
    let Some(spec) = lookup_long(key) else {
        return Err(format!("Unknown argument '{key}'. Try 'arc help'."));
    };
    if !spec.flag.takes_param() {
        store_flag(options, spec, None);
        if let Some(value) = inline {
            // `--bool=value`: PHP splices the value back as the next
            // token (`:826-832`), so it parses as a positional.
            queue.push_front(value.to_string());
        }
        return Ok(());
    }
    let value = match inline {
        Some(value) => value.to_string(),
        None => queue
            .pop_front()
            .ok_or_else(|| format!("Option '{typed}' requires a parameter."))?,
    };
    store_flag(options, spec, Some(value));
    Ok(())
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// Parse `argv` (excluding the subcommand name) like
/// `ArcanistWorkflow::parseArguments()`; `-h`/`--help` anywhere shows
/// help (the toolset handles help before parsing).
fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut options = Options::default();
    let mut queue: std::collections::VecDeque<String> = argv.iter().cloned().collect();
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = queue.pop_front() {
        if arg == "--" {
            positional.extend(queue.drain(..));
            break;
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if let Some(stripped) = arg.strip_prefix("--") {
            let typed = arg.clone();
            if let Err(message) = parse_long_flag(&mut options, &mut queue, &typed, stripped) {
                return ParseOutcome::Error(message);
            }
        } else if arg.starts_with('-') && arg.len() > 1 {
            // `lint` declares no short flags
            // (`ArcanistWorkflow.php:852-862`).
            let spelling = &arg[1..];
            return ParseOutcome::Error(format!("Unknown argument '{spelling}'. Try 'arc help'."));
        } else {
            positional.push(arg);
        }
    }
    options.paths = positional;
    if let Err(message) = check_conflicts(&options.provided) {
        return ParseOutcome::Error(message);
    }
    ParseOutcome::Args(options)
}

/// How a `conflicts` reason renders (`ArcanistWorkflow.php:905-909`):
/// a `true` reason stringifies to `': 1'`.
enum ConflictReason {
    One,
    Text(&'static str),
}

struct ConflictDecl {
    flag: &'static str,
    conflicts: &'static [(&'static str, ConflictReason)],
}

/// `lint` conflict declarations, verbatim from `getArguments()`.
const CONFLICTS: &[ConflictDecl] = &[
    ConflictDecl {
        flag: "apply-patches",
        conflicts: &[("never-apply-patches", ConflictReason::One)],
    },
    ConflictDecl {
        flag: "never-apply-patches",
        conflicts: &[("apply-patches", ConflictReason::One)],
    },
    ConflictDecl {
        flag: "everything",
        conflicts: &[("rev", ConflictReason::Text("--everything lints all files"))],
    },
];

/// Reject flag combinations PHP declares conflicting
/// (`ArcanistWorkflow.php:899-919`), in first-provision order.
fn check_conflicts(provided: &[&'static str]) -> Result<(), String> {
    for key in provided {
        let Some(decl) = CONFLICTS.iter().find(|decl| decl.flag == *key) else {
            continue;
        };
        for (conflict, reason) in decl.conflicts {
            if provided.contains(conflict) {
                let suffix = match reason {
                    ConflictReason::One => ": 1".to_string(),
                    ConflictReason::Text(text) => format!(": {text}"),
                };
                return Err(format!(
                    "Arguments '--{key}' and '--{conflict}' are mutually exclusive{suffix}"
                ));
            }
        }
    }
    Ok(())
}

/// `willRunWorkflow()` VCS gating (`ArcanistWorkflow.php:1494-1517`):
/// `--rev` needs git or hg.
fn check_supports(options: &Options, vcs: Vcs) -> Result<(), String> {
    if options.rev.is_some() && !matches!(vcs, Vcs::Git | Vcs::Hg) {
        return Err(format!(
            "Option '--rev' is not supported under {}. Lint does not currently support {} in SVN.",
            vcs.system_name(),
            "--rev"
        ));
    }
    Ok(())
}

/* -(  Engines  )-------------------------------------------------------------- */

/// Resolve the lint engine (`ArcanistWorkflow::newLintEngine()`,
/// `:2056-2095`): `--engine`, else `.arcconfig` `lint.engine`, else the
/// configuration-driven engine when `.arclint` exists at the project
/// root, else the no-engine error.
pub fn engine_for_workcopy(work: &WorkCopy, cli_engine: Option<&str>) -> Result<Engine, String> {
    let mut class = cli_engine
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    if class.is_none() {
        class = arcconfig_string(&work.project_root, "lint.engine")?;
    }
    if class.is_none() && work.project_root.join(arclint::CONFIG_FILE_NAME).is_file() {
        class = Some(ENGINE_CONFIGURATION_DRIVEN.to_string());
    }
    let Some(class) = class else {
        return Err("No lint engine is configured for this project. Create an '.arclint' file, or configure an advanced engine with 'lint.engine' in '.arcconfig'."
            .to_string());
    };
    match class.as_str() {
        ENGINE_CONFIGURATION_DRIVEN => {
            let config = arclint::load_arclint(&work.project_root)?;
            Ok(Engine::from_config(&config))
        }
        ENGINE_COMPREHENSIVE => Ok(Engine::comprehensive()),
        ENGINE_SINGLE => single_engine(work),
        _ => Err(format!(
            "Configured lint engine \"{class}\" is not a subclass of \"{ENGINE_BASE_CLASS}\", but must be."
        )),
    }
}

/// `ArcanistSingleLintEngine`: run the linter named by
/// `lint.engine.single.linter` on every path.
fn single_engine(work: &WorkCopy) -> Result<Engine, String> {
    const KEY: &str = "lint.engine.single.linter";
    let name = arcconfig_string(&work.project_root, KEY)?;
    let Some(name) = name else {
        return Err(format!(
            "You must configure '{KEY}' with the name of a linter in order to use {ENGINE_SINGLE}."
        ));
    };
    let Some(display) = single_linter_display(&name) else {
        return Err(format!(
            "Linter '{name}' configured in '{KEY}' does not exist!"
        ));
    };
    let mut engine = Engine::new();
    engine.retain_display(display);
    Ok(engine)
}

/// Map a single-engine linter name (PHP class name or display name) to
/// a display name.
fn single_linter_display(name: &str) -> Option<&'static str> {
    // Accept PHP class names (`ArcanistTextLinter`) as well as display
    // names (`TXT`).
    for (class, display) in single_linter_classes() {
        if name == class || name == display {
            return Some(display);
        }
    }
    None
}

fn single_linter_classes() -> Vec<(&'static str, &'static str)> {
    vec![
        ("ArcanistTextLinter", "TXT"),
        ("ArcanistFilenameLinter", "NAME"),
        ("ArcanistJSONLinter", "JSON"),
        ("ArcanistJSONLintLinter", "JSON"),
        ("ArcanistXMLLinter", "XML"),
        ("ArcanistMergeConflictLinter", "MERGECONFLICT"),
        ("ArcanistGeneratedLinter", "GEN"),
        ("ArcanistNoLintLinter", "NOLINT"),
        ("ArcanistChmodLinter", "CHMOD"),
        ("ArcanistSpellingLinter", "SPELL"),
        ("ArcanistScriptAndRegexLinter", "S&RX"),
        ("ArcanistComposerLinter", "COMPOSER"),
        ("ArcanistPhutilLibraryLinter", "PHL"),
        ("ArcanistJSHintLinter", "JSHint"),
        ("ArcanistJscsLinter", "JSCS"),
        ("ArcanistCSSLintLinter", "CSSLint"),
        ("ArcanistLesscLinter", "LESSC"),
        ("ArcanistClosureLinter", "GJSLINT"),
        ("ArcanistCoffeeLintLinter", "COFFEE"),
        ("ArcanistFlake8Linter", "flake8"),
        ("ArcanistPyFlakesLinter", "PYFLAKES"),
        ("ArcanistPyLintLinter", "PyLint"),
        ("ArcanistPEP8Linter", "PEP8"),
        ("ArcanistRuboCopLinter", "RuboCop"),
        ("ArcanistRubyLinter", "RUBY"),
        ("ArcanistCppcheckLinter", "cppcheck"),
        ("ArcanistCpplintLinter", "C++ Google's Styleguide"),
        ("ArcanistGoLintLinter", "GOLINT"),
        ("ArcanistHLintLinter", "HLINT"),
        ("ArcanistCSharpLinter", "C#"),
        ("ArcanistPhpcsLinter", "PHPCS"),
        ("ArcanistPhpLinter", "PHP"),
        ("ArcanistPuppetLintLinter", "PUPPETLINT"),
        ("ArcanistXHPASTLinter", "XHP"),
    ]
}

/// Resolve the engine for a directory: discover the working copy, then
/// resolve like [`engine_for_workcopy`]. Returns the engine plus the
/// config path used, if any.
pub fn engine_for_dir(dir: &Path) -> Result<(Engine, Option<PathBuf>), String> {
    let work = discover_working_copy(dir)?;
    let engine = engine_for_workcopy(&work, None)?;
    let config = work.project_root.join(arclint::CONFIG_FILE_NAME);
    Ok((engine, config.is_file().then_some(config)))
}

/* -(  Running linters  )------------------------------------------------------ */

/// Lint explicit paths with `engine`, returning sorted messages or a
/// combined error. `cwd` resolves relative paths; display paths are
/// relative to `project_root`.
pub fn lint_explicit_paths_with_engine(
    engine: &Engine,
    cwd: &Path,
    project_root: &Path,
    paths: &[String],
) -> Result<Vec<LintMessage>, String> {
    let files = collect_explicit(paths, cwd, project_root)?;
    let mut messages = Vec::new();
    let mut errors = Vec::new();
    for (disk, display) in &files {
        match engine.lint_file(disk, display) {
            Ok(mut found) => messages.append(&mut found),
            Err(error) => errors.push(format!("{display}: {error}")),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    messages
        .sort_by(|a, b| (&a.path, a.line, a.col, &a.code).cmp(&(&b.path, b.line, b.col, &b.code)));
    Ok(messages)
}

/// Lint explicit paths with every ported linter. The working copy is
/// discovered from `cwd` for project-relative display paths.
pub fn lint_explicit_paths(cwd: &Path, paths: &[String]) -> Result<Vec<LintMessage>, String> {
    let work = discover_working_copy(cwd)?;
    lint_explicit_paths_with_engine(&Engine::new(), cwd, &work.project_root, paths)
}

/// Lint the working-tree selection under `dir` (range `base` plus
/// uncommitted, minus untracked) with `engine`, skipping deleted files.
pub fn lint_changed_in_with_engine(
    engine: &Engine,
    dir: &Path,
    base: Option<&str>,
) -> Result<Vec<LintMessage>, String> {
    let files = changed_files_in(dir, base)?;
    let mut messages = Vec::new();
    for (disk, display) in &files {
        if std::fs::symlink_metadata(disk).is_err() {
            continue;
        }
        match engine.lint_file(disk, display) {
            Ok(mut found) => messages.append(&mut found),
            Err(error) => return Err(format!("{display}: {error}")),
        }
    }
    messages
        .sort_by(|a, b| (&a.path, a.line, a.col, &a.code).cmp(&(&b.path, b.line, b.col, &b.code)));
    Ok(messages)
}

/// Lint the working-tree selection under `dir` with every ported
/// linter, skipping deleted files.
pub fn lint_changed_in(dir: &Path, base: Option<&str>) -> Result<Vec<LintMessage>, String> {
    lint_changed_in_with_engine(&Engine::new(), dir, base)
}

/* -(  Results  )---------------------------------------------------------------- */

/// Messages for one path, mirroring `ArcanistLintResult`. `applied[i]`
/// marks patch-applied messages (excluded from the result code).
#[derive(Debug, Clone)]
pub struct LintResult {
    pub path: String,
    pub disk_path: PathBuf,
    pub data: Vec<u8>,
    pub messages: Vec<LintMessage>,
    pub applied: Vec<bool>,
}

impl LintResult {
    pub fn new(path: String, disk_path: PathBuf, data: Vec<u8>) -> Self {
        Self {
            path,
            disk_path,
            data,
            messages: Vec::new(),
            applied: Vec::new(),
        }
    }

    pub fn add_message(&mut self, message: LintMessage) {
        self.messages.push(message);
        self.applied.push(false);
    }

    /// Sort messages by position (`sortAndFilterMessages()`:
    /// `line * 8192 + char`, with a code tiebreak for determinism).
    /// Messages without a line sort first, like PHP's null key 0.
    pub fn sort_messages(&mut self) {
        let mut order: Vec<usize> = (0..self.messages.len()).collect();
        order.sort_by_key(|&i| {
            let message = &self.messages[i];
            (message.line * 8192 + message.col, message.code.clone())
        });
        let mut messages = Vec::with_capacity(self.messages.len());
        let mut applied = Vec::with_capacity(self.applied.len());
        for i in order {
            messages.push(self.messages[i].clone());
            applied.push(self.applied[i]);
        }
        self.messages = messages;
        self.applied = applied;
    }

    #[must_use]
    pub fn is_patchable(&self) -> bool {
        self.messages.iter().any(LintMessage::is_patchable)
    }

    #[must_use]
    pub fn is_all_autofix(&self) -> bool {
        self.messages.iter().all(LintMessage::is_autofix)
    }
}

/// Group messages by path (sorted), loading file data for context
/// rendering. Missing/unreadable files get empty data.
pub fn group_results(
    messages: Vec<LintMessage>,
    disk_for: &dyn Fn(&str) -> PathBuf,
) -> Vec<LintResult> {
    let mut paths: Vec<String> = messages.iter().map(|m| m.path.clone()).collect();
    paths.sort();
    paths.dedup();
    let mut results = Vec::new();
    for path in paths {
        let disk_path = disk_for(&path);
        let data = std::fs::read(&disk_path).unwrap_or_default();
        let mut result = LintResult::new(path.clone(), disk_path, data);
        for message in messages.iter().filter(|m| m.path == path).cloned() {
            result.add_message(message);
        }
        result.sort_messages();
        results.push(result);
    }
    results
}

/// Drop non-error messages outside the changed lines, mirroring
/// `isRelevantMessage()` (`ArcanistLintEngine.php:316-369`): errors and
/// line-less messages are always relevant; a `None` line set (binary,
/// directory, missing) keeps everything.
pub fn filter_changed_lines(
    messages: Vec<LintMessage>,
    changed: &HashMap<String, Option<BTreeSet<usize>>>,
) -> Vec<LintMessage> {
    messages
        .into_iter()
        .filter(|message| {
            if message.is_error() || !message.has_file_context() {
                return true;
            }
            match changed.get(&message.path) {
                None => false,
                Some(None) => true,
                Some(Some(lines)) => {
                    let mut last = message.line;
                    if let Some(original) = &message.original {
                        last += original.bytes().filter(|b| *b == b'\n').count();
                    }
                    (message.line..=last).any(|line| lines.contains(&line))
                }
            }
        })
        .collect()
}

/// Compute the workflow result code from unresolved (non-applied)
/// messages (`ArcanistLintWorkflow.php:356-386`).
#[must_use]
pub fn result_code(results: &[LintResult]) -> i32 {
    let mut code = RESULT_OKAY;
    for result in results {
        for (message, applied) in result.messages.iter().zip(result.applied.iter()) {
            if *applied {
                continue;
            }
            if message.is_error() {
                return RESULT_ERRORS;
            }
            if message.is_warning() {
                code = RESULT_WARNINGS;
            }
        }
    }
    code
}

/* -(  Renderers  )--------------------------------------------------------------- */

/// Word-wrap `text` like `phutil_console_wrap()`: each paragraph is
/// wrapped to 80 columns and every line is prefixed with `indent`
/// spaces. Approximation; see the module docs.
pub fn word_wrap(text: &str, indent: usize) -> String {
    const WIDTH: usize = 80;
    let prefix = " ".repeat(indent);
    let mut out = String::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            out.push_str(&prefix);
            out.push('\n');
            continue;
        }
        let mut line_len = 0;
        for word in paragraph.split(' ') {
            if word.is_empty() {
                continue;
            }
            let width = indent + line_len + if line_len > 0 { 1 } else { 0 } + word.len();
            if line_len > 0 && width > WIDTH {
                out.push('\n');
                line_len = 0;
            }
            if line_len == 0 {
                out.push_str(&prefix);
            } else {
                out.push(' ');
            }
            out.push_str(word);
            line_len += if line_len > 0 {
                word.len() + 1
            } else {
                word.len()
            };
        }
        out.push('\n');
    }
    out
}

/// Render one result in console format
/// (`ArcanistConsoleLintRenderer::renderLintResult()`, `:50-103`),
/// markup stripped (non-TTY).
pub fn render_console_result(result: &LintResult) -> Result<String, String> {
    let mut entries = Vec::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        entries.push(format!(
            "   {}  ({}) {}\n{}",
            severity,
            message.code,
            message.name,
            word_wrap(&message.description, 4),
        ));
        if message.has_file_context() {
            entries.push(render_context(message, &result.data));
        }
    }
    if entries.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        ">>> Lint for {}:\n\n\n{}",
        result.path,
        entries.join("\n")
    ))
}

/// Render one message's file context
/// (`ArcanistConsoleLintRenderer::renderContext()`, `:105-336`).
/// Highlights are stripped (non-TTY `##..##`); see the module docs.
/// Split like `phutil_split_lines()`: lines keep their endings; a
/// trailing newline leaves no empty piece.
fn split_keep_ends(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(text[start..=index].to_string());
            start = index + 1;
        }
    }
    if start < text.len() {
        lines.push(text[start..].to_string());
    }
    lines
}

fn render_context(message: &LintMessage, data: &[u8]) -> String {
    const CONTEXT: usize = 3;
    let data_text = String::from_utf8_lossy(data).into_owned();
    let old_lines: Vec<String> = split_keep_ends(&data_text);

    let (line, char, original, replacement) = trimmed_patch(message);
    let old_impact = original.bytes().filter(|b| *b == b'\n').count() + 1;
    let mut start = line;
    let max_old = old_lines.len();

    // PHI1782: messages past the end of the file render a warning.
    let invalid = if start > max_old + 1 {
        true
    } else if start > max_old {
        !original.is_empty() || (char != 0 && char != 1)
    } else {
        false
    };
    if invalid {
        return format!(
            "{}\n\n",
            render_line(
                Some(start),
                &format!(
                    "(This message was raised at line {}, but the file only has {} line(s).)",
                    format_number(start),
                    format_number(max_old),
                ),
                false,
                None,
            )
        );
    }

    let patchable = message.is_patchable();
    let mut new_lines: Vec<String> = Vec::new();
    let (old_impact, new_impact) = if patchable {
        let line_map = offset_map(&data_text);
        let patch_offset = line_map.get(&line).copied().unwrap_or(0) + char.saturating_sub(1);
        let replacement_text = replacement.clone().unwrap_or_default();
        let mut new_data = data_text.as_bytes().to_vec();
        let end = (patch_offset + original.len()).min(new_data.len());
        new_data.splice(
            patch_offset.min(new_data.len())..end,
            replacement_text.bytes(),
        );
        let new_text = String::from_utf8_lossy(&new_data).into_owned();
        new_lines = split_keep_ends(&new_text);
        let new_impact = replacement_text.bytes().filter(|b| *b == b'\n').count() + 1;
        shrink_ranges(&old_lines, &new_lines, &mut start, old_impact, new_impact)
    } else {
        (0, 0)
    };

    struct Spec {
        text: String,
        number: Option<usize>,
        diff: Option<char>,
        chevron: bool,
    }
    let mut specs: Vec<Spec> = Vec::new();
    let head = start.saturating_sub(CONTEXT).max(1);
    for number in head..start {
        specs.push(Spec {
            text: old_lines.get(number - 1).cloned().unwrap_or_default(),
            number: Some(number),
            diff: None,
            chevron: false,
        });
    }
    for number in start..start + old_impact {
        specs.push(Spec {
            text: old_lines.get(number - 1).cloned().unwrap_or_default(),
            number: Some(number),
            diff: Some('-'),
            chevron: number == start,
        });
    }
    for number in start..start + new_impact {
        specs.push(Spec {
            text: new_lines.get(number - 1).cloned().unwrap_or_default(),
            number: None,
            diff: Some('+'),
            chevron: number == start,
        });
    }
    let cursor = start + old_impact;
    let foot = old_lines.len().min(cursor + CONTEXT);
    for number in cursor..=foot {
        if number == 0 {
            continue;
        }
        specs.push(Spec {
            text: old_lines.get(number - 1).cloned().unwrap_or_default(),
            number: Some(number),
            diff: None,
            chevron: number == cursor,
        });
    }

    let mut out = String::new();
    let mut seen_chevron = false;
    for spec in &specs {
        let chevron = if seen_chevron {
            false
        } else {
            if spec.chevron {
                seen_chevron = true;
            }
            spec.chevron
        };
        let mut text = spec.text.clone();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        out.push_str(&render_line(spec.number, &text, chevron, spec.diff));
        if chevron && !patchable && original.is_empty() {
            out.push_str(&render_caret(char));
            out.push('\n');
        }
    }
    out
}

/// Trim a shared prefix/suffix off a patchable message's texts
/// (`ArcanistLintMessage::newTrimmedMessage()`, `:299-376`). Returns
/// (line, char, original, replacement); char 0 means null. Trimming
/// runs on bytes (PHP `substr` is byte-based); results are lossy
/// decoded for rendering.
fn trimmed_patch(message: &LintMessage) -> (usize, usize, String, Option<String>) {
    let line = message.line;
    let char = message.col;
    if !message.is_patchable() {
        return (
            line,
            char,
            message.original.clone().unwrap_or_default(),
            None,
        );
    }
    let original_bytes = message.original.clone().unwrap_or_default().into_bytes();
    let replacement_bytes = message.replacement.clone().unwrap_or_default().into_bytes();
    let minimum = original_bytes.len().min(replacement_bytes.len());

    let mut prefix = 0;
    while prefix < minimum && original_bytes[prefix] == replacement_bytes[prefix] {
        prefix += 1;
    }
    let max_suffix = minimum - prefix;
    let mut suffix = 0;
    while suffix < max_suffix
        && original_bytes[original_bytes.len() - 1 - suffix]
            == replacement_bytes[replacement_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut original = original_bytes.clone();
    let mut replacement = replacement_bytes.clone();
    if suffix > 0 {
        original.truncate(original.len() - suffix);
        replacement.truncate(replacement.len() - suffix);
    }
    let mut line = line;
    let mut char = char;
    if prefix > 0 {
        let dropped = original_bytes[..prefix].to_vec();
        original = original[prefix.min(original.len())..].to_vec();
        replacement = replacement[prefix.min(replacement.len())..].to_vec();
        for byte in dropped {
            char += 1;
            if byte == b'\n' {
                line += 1;
                char = 1;
            }
        }
    }
    (
        line,
        char,
        String::from_utf8_lossy(&original).into_owned(),
        Some(String::from_utf8_lossy(&replacement).into_owned()),
    )
}

/// Shrink identical head/tail lines off a patch range (T13543-aware).
fn shrink_ranges(
    old_lines: &[String],
    new_lines: &[String],
    start: &mut usize,
    mut old_impact: usize,
    mut new_impact: usize,
) -> (usize, usize) {
    loop {
        if old_lines.get(*start - 1) != new_lines.get(*start - 1) {
            break;
        }
        *start += 1;
        old_impact = old_impact.saturating_sub(1);
        new_impact = new_impact.saturating_sub(1);
        if old_impact == 0 || new_impact == 0 {
            break;
        }
    }
    if old_impact > 0 && new_impact > 0 {
        loop {
            let old_suffix = old_lines.get(*start + old_impact - 2);
            let new_suffix = new_lines.get(*start + new_impact - 2);
            if old_suffix != new_suffix {
                break;
            }
            old_impact -= 1;
            new_impact -= 1;
            if old_impact == 0 || new_impact == 0 {
                break;
            }
        }
    }
    (old_impact, new_impact)
}

/// Byte offsets where each 1-based line starts, plus a virtual final
/// offset when the data ends in a newline (`newOffsetMap()`, `:352-375`).
/// `phutil_split_lines()` keeps line endings.
fn offset_map(data: &str) -> HashMap<usize, usize> {
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0;
    for (index, byte) in data.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&data[start..=index]);
            start = index + 1;
        }
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    let mut map = HashMap::new();
    let mut number = 1;
    let mut offset = 0;
    for line in &lines {
        map.insert(number, offset);
        number += 1;
        offset += line.len();
    }
    if !lines.is_empty() && data.ends_with('\n') {
        map.insert(number, offset);
    }
    map
}

fn render_caret(pos: usize) -> String {
    format!("{}^", " ".repeat(16 + pos))
}

/// `renderLine()` (`:342-350`): `    %3s %1s %6s %s`. Note `%1s`
/// pads an empty diff marker to one space.
fn render_line(number: Option<usize>, data: &str, chevron: bool, diff: Option<char>) -> String {
    let chevron_text = if chevron { ">>>" } else { "" };
    let diff_text = diff.map(String::from).unwrap_or_default();
    let number_text = number.map(|n| n.to_string()).unwrap_or_default();
    format!("    {chevron_text:>3} {diff_text:1} {number_text:>6} {data}")
}

/// `renderResultCode()` for console and summary: the `OKAY` line on
/// success, else nothing.
fn render_result_code_okay(result_code: i32) -> String {
    if result_code == RESULT_OKAY {
        "   OKAY  No lint messages.\n".to_string()
    } else {
        String::new()
    }
}

/// Render one result in summary format
/// (`ArcanistSummaryLintRenderer.php:7-22`).
pub fn render_summary_result(result: &LintResult) -> Result<String, String> {
    let mut out = String::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        let line = if message.line == 0 {
            String::new()
        } else {
            message.line.to_string()
        };
        out.push_str(&format!(
            "{}:{line}:{severity}: {}\n",
            result.path, message.name
        ));
    }
    Ok(out)
}

/// Render one result in compiler format
/// (`ArcanistCompilerLintRenderer.php:7-28`).
pub fn render_compiler_result(result: &LintResult) -> Result<String, String> {
    let mut out = String::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        out.push_str(&format!(
            "{}:{}:{} ({}) {}\n",
            result.path, message.line, severity, message.code, message.description,
        ));
    }
    Ok(out)
}

/// One message as a JSON-renderer dictionary: `toDictionary()` plus
/// `context`, minus `path`.
fn message_json_dict(message: &LintMessage, data: &[u8]) -> serde_json::Value {
    let text = String::from_utf8_lossy(data);
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines.insert(0, "");
    let start = (message.line as i64 - JSON_LINES_OF_CONTEXT as i64).max(1) as usize;
    let context: Vec<&str> = lines
        .iter()
        .skip(start)
        .take(JSON_LINES_OF_CONTEXT * 2 + 1)
        .copied()
        .collect();
    serde_json::json!({
        "line": if message.line == 0 { serde_json::Value::Null } else { serde_json::json!(message.line) },
        "char": if message.col == 0 { serde_json::Value::Null } else { serde_json::json!(message.col) },
        "code": message.code,
        "severity": message.severity,
        "name": message.name,
        "description": message.description,
        "original": message.original,
        "replacement": message.replacement,
        "granularity": serde_json::Value::Null,
        "locations": Vec::<serde_json::Value>::new(),
        "bypassChangedLineFiltering": serde_json::Value::Null,
        "context": context.join("\n"),
    })
}

/// Render one result in JSON format
/// (`ArcanistJSONLintRenderer.php:9-28`): one compact object per path.
pub fn render_json_result(result: &LintResult) -> String {
    let dicts: Vec<serde_json::Value> = result
        .messages
        .iter()
        .map(|message| message_json_dict(message, &result.data))
        .collect();
    let mut map = serde_json::Map::new();
    map.insert(result.path.clone(), serde_json::Value::Array(dicts));
    // PHP `json_encode()` escapes `/` as `\/`.
    serde_json::to_string(&map)
        .unwrap_or_else(|_| "{}".to_string())
        .replace('/', "\\/")
}

/// Escape XML attribute text like `XMLWriter`.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Open the checkstyle document (`willRenderResults()`).
fn render_xml_header() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<checkstyle version=\"4.3\">\n".to_string()
}

/// Render one result in checkstyle XML format
/// (`ArcanistCheckstyleXMLLintRenderer.php:43-64`).
pub fn render_xml_result(result: &LintResult) -> String {
    let mut out = format!("  <file name=\"{}\">\n", xml_escape(&result.path));
    for message in &result.messages {
        let line = if message.line == 0 {
            String::new()
        } else {
            message.line.to_string()
        };
        let col = if message.col == 0 {
            String::new()
        } else {
            message.col.to_string()
        };
        out.push_str(&format!(
            "    <error line=\"{}\" column=\"{}\" severity=\"{}\" message=\"{}\" source=\"{}\"/>\n",
            xml_escape(&line),
            xml_escape(&col),
            xml_severity(&message.severity),
            xml_escape(&message.description),
            xml_escape(&message.code),
        ));
    }
    out.push_str("  </file>\n");
    out
}

/// Close the checkstyle document (`didRenderResults()`).
fn render_xml_footer() -> String {
    "</checkstyle>\n".to_string()
}

/* -(  Patches  )----------------------------------------------------------------- */

/// Byte offsets where each line starts (`getCharacterOffset()` line
/// map, `ArcanistLintPatcher.php:108-124`). Index `line - 1`.
fn patch_line_offsets(data: &[u8]) -> Vec<usize> {
    let mut offsets = vec![0];
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            offsets.push(index + 1);
        }
    }
    offsets
}

/// Apply a result's patches to `data`
/// (`ArcanistLintPatcher::buildModifiedFile()`, `:66-106`). Messages
/// apply in position order; overlapping messages are skipped via the
/// dirty cursor. Returns the patched data plus per-message applied
/// flags. A message past the end of the file is an error (PHP's
/// message names an undefined variable, rendering as 0).
pub fn build_patched_data(
    data: &[u8],
    messages: &[LintMessage],
) -> Result<(Vec<u8>, Vec<bool>), String> {
    let offsets = patch_line_offsets(data);
    let mut patched = data.to_vec();
    let mut delta: i64 = 0;
    let mut dirty_until: usize = 0;
    let mut applied = vec![false; messages.len()];
    for (index, message) in messages.iter().enumerate() {
        if !message.is_patchable() {
            continue;
        }
        if message.line == 0 || message.line > offsets.len() {
            return Err("Data has fewer than 0 lines.".to_string());
        }
        let orig_offset = offsets[message.line - 1] + message.col.saturating_sub(1);
        if dirty_until > orig_offset {
            continue;
        }
        let working_offset = (orig_offset as i64 + delta).max(0) as usize;
        let old = message.original.clone().unwrap_or_default();
        let new = message.replacement.clone().unwrap_or_default();
        if working_offset == patched.len() {
            // Temporary hack to work around a destructive hphpi issue.
            patched.extend_from_slice(new.as_bytes());
        } else {
            let start = working_offset.min(patched.len());
            let end = (working_offset + old.len()).min(patched.len());
            patched.splice(start..end, new.bytes());
        }
        delta += new.len() as i64 - old.len() as i64;
        dirty_until = orig_offset + old.len();
        applied[index] = true;
    }
    Ok((patched, applied))
}

/// Write a patch to disk via a `<path>.linted[N]` temp file plus rename
/// (`ArcanistLintPatcher::writePatchToDisk()`, `:32-62`): the existing
/// file is copied first to preserve permissions. Returns the applied
/// flags for the result's messages.
pub fn write_patch_to_disk(result: &LintResult) -> Result<Vec<bool>, String> {
    let (patched, applied) = build_patched_data(&result.data, &result.messages)?;
    let path = &result.disk_path;
    let mut suffix: Option<usize> = None;
    let temp = loop {
        let name = match suffix {
            None => format!("{}.linted", path.display()),
            Some(n) => format!("{}.linted{n}", path.display()),
        };
        let candidate = PathBuf::from(&name);
        if !candidate.exists() {
            break candidate;
        }
        suffix = Some(suffix.map_or(1, |n| n + 1));
    };
    if path.exists() {
        std::fs::copy(path, &temp)
            .map_err(|e| format!("Unable to copy '{}': {e}", path.display()))?;
    }
    std::fs::write(&temp, &patched)
        .map_err(|e| format!("Unable to write '{}': {e}", temp.display()))?;
    if let Err(rename_error) = rename_over(&temp, path) {
        return Err(format!(
            "Unable to overwrite path '{}', patched version was left at '{}'. ({rename_error})",
            path.display(),
            temp.display(),
        ));
    }
    Ok(applied)
}

/// Rename `from` onto `to`, replacing an existing destination (PHP
/// `Filesystem::rename()` overwrites).
fn rename_over(from: &Path, to: &Path) -> Result<(), String> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) if to.exists() => {
            std::fs::remove_file(to).map_err(|e| e.to_string())?;
            std::fs::rename(from, to).map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Ask a yes/no question (`phutil_console_confirm()`): `{prompt} [Y/n] `
/// for a yes default, `{prompt} [y/N] ` for a no default. Blank answers
/// and EOF take the default; invalid answers re-prompt.
pub fn confirm(prompt: &str, default_yes: bool, input: &mut dyn BufRead) -> bool {
    use std::io::Write;
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    loop {
        print!("{prompt} {suffix} ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) => return default_yes,
            Err(_) => return default_yes,
            Ok(_) => {}
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "" => return default_yes,
            "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => {}
        }
    }
}

/// Show the `diff -u` of a patch
/// (`ArcanistConsoleLintRenderer::promptForPatch()`, `:31-48`); the
/// diff goes to the renderer `sink` and the caller asks to apply it.
/// A missing `diff` binary yields empty output, like PHP's ignored
/// `exec_manual()` failure.
fn prompt_for_patch(result: &LintResult, patched: &[u8], sink: &mut Sink) {
    let old_path = if result.disk_path.exists() {
        result.disk_path.to_string_lossy().to_string()
    } else {
        "/dev/null".to_string()
    };
    let new_path = write_temp_bytes("arc-rs-lint-patch", patched);
    let mut diff_text = String::new();
    if let Some(new_path) = &new_path {
        let output = Command::new("diff")
            .arg("-u")
            .arg(&old_path)
            .arg(new_path)
            .output();
        if let Ok(output) = output {
            diff_text = String::from_utf8_lossy(&output.stdout).into_owned();
        }
        std::fs::remove_file(new_path).ok();
    }
    sink.write(&diff_text);
}

/// Write `data` to a unique temp file, returning its path.
fn write_temp_bytes(tag: &str, data: &[u8]) -> Option<PathBuf> {
    write_temp_bytes_in(&std::env::temp_dir(), tag, data)
}

/// Write `data` to a unique temp file inside `dir`, returning its path.
fn write_temp_bytes_in(dir: &Path, tag: &str, data: &[u8]) -> Option<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = dir.join(format!("{tag}-{}-{n}", std::process::id()));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

/// Renderer output: stdout streams immediately (preserving prompt
/// interleaving); `--outfile` buffers for a temp-file rename.
enum Sink {
    Stdout,
    Buffer(String),
}

impl Sink {
    fn write(&mut self, text: &str) {
        match self {
            Sink::Stdout => print!("{text}"),
            Sink::Buffer(buffer) => buffer.push_str(text),
        }
    }
}

/* -(  Entry points  )------------------------------------------------------------ */

/// Whether to show every warning (`ArcanistLintWorkflow.php:182-190`).
fn lint_all(options: &Options) -> bool {
    if options.lintall {
        true
    } else if options.rev.is_some() {
        false
    } else {
        !options.paths.is_empty() || options.everything
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("arc-rs {NAME}: cannot determine current directory: {error}");
            return ExitCode::FAILURE;
        }
    };
    run_in(&cwd, argv)
}

/// Entry point with an explicit working directory, so tests can run
/// without mutating the process directory. Prompts read stdin.
pub fn run_in(cwd: &Path, argv: &[String]) -> ExitCode {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    run_in_with_input(cwd, argv, &mut input)
}

/// Entry point with an explicit prompt-input reader.
pub fn run_in_with_input(cwd: &Path, argv: &[String], input: &mut dyn BufRead) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(options) => options,
    };
    match run_workflow(cwd, &options, input) {
        Ok(code) => ExitCode::from(code as u8),
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_workflow(cwd: &Path, options: &Options, input: &mut dyn BufRead) -> Result<i32, String> {
    let work = discover_working_copy(cwd)?;
    check_supports(options, work.vcs)?;

    if options.everything && !options.paths.is_empty() {
        return Err("You can not specify paths with --everything. The --everything flag lints every tracked file in the working copy."
            .to_string());
    }

    let engine = engine_for_workcopy(&work, options.engine.as_deref())?;
    if engine.is_empty() {
        return Err("No linters to run.".to_string());
    }

    // Resolve the commit-range base: explicit `--rev`, else the default
    // base when linting the whole selection (`parseBaseCommitArgument()`
    // plus `buildBaseCommit()`; svn has no ranges).
    let needs_base = options.rev.is_some() || (options.paths.is_empty() && !options.everything);
    let base: Option<String> = if !needs_base || work.vcs == Vcs::Svn {
        None
    } else if let Some(rev) = &options.rev {
        Some(match work.vcs {
            Vcs::Git => git_base_for_rev(&work, rev)?,
            Vcs::Hg => hg_base_for(&work, Some(rev))?,
            Vcs::Svn => unreachable!("svn --rev rejected by check_supports"),
        })
    } else {
        match work.vcs {
            Vcs::Git => Some(git_default_base(&work, input, &mut |text| {
                print!("{text}")
            })?),
            Vcs::Hg => Some(hg_base_for(&work, None)?),
            Vcs::Svn => None,
        }
    };

    let lint_all = lint_all(options);
    let selection: Vec<String> = if options.everything {
        let mut paths = everything_paths(&work)?;
        paths.sort();
        paths
    } else if !options.paths.is_empty() {
        collect_explicit(&options.paths, cwd, &work.project_root)?
            .into_iter()
            .map(|(_, display)| display)
            .collect()
    } else {
        status_paths(&work, base.as_deref())?
    };
    if selection.is_empty() {
        return Err("No paths are lintable.".to_string());
    }
    if engine.is_comprehensive()
        && selection
            .iter()
            .all(|path| comprehensive_linters_for(path).is_none())
    {
        return Err("No paths are lintable.".to_string());
    }

    let disk_for = |path: &str| work.vcs_root.join(path);
    let mut messages = Vec::new();
    for path in &selection {
        let disk = disk_for(path);
        match engine.lint_file(&disk, path) {
            Ok(mut found) => messages.append(&mut found),
            Err(error) => return Err(format!("{path}: {error}")),
        }
    }

    let minimum = options.severity.as_deref().unwrap_or(DEFAULT_SEVERITY);
    let mut messages = filter_by_severity(messages, minimum);
    if !lint_all {
        let mut changed: HashMap<String, Option<BTreeSet<usize>>> = HashMap::new();
        for path in &selection {
            // The base is always resolved when filtering under git/hg;
            // svn diffs the worktree.
            let lines = changed_lines_for(&work, base.as_deref().unwrap_or("HEAD"), path)?;
            changed.insert(path.clone(), lines);
        }
        messages = filter_changed_lines(messages, &changed);
    }

    let mut results = group_results(messages, &disk_for);

    let renderer_key = options.output.as_deref().unwrap_or(RENDERER_CONSOLE);
    if !all_renderers().contains(&renderer_key) {
        return Err(format!(
            "Lint renderer \"{}\" is unknown. Supported renderers are: {}.",
            renderer_key,
            all_renderers().join(", ")
        ));
    }

    let mut sink = if options.outfile.is_some() {
        Sink::Buffer(String::new())
    } else {
        Sink::Stdout
    };
    if renderer_key == RENDERER_XML {
        sink.write(&render_xml_header());
    }

    let apply_patches = !options.never_apply_patches;
    let prompt_patches = !options.apply_patches;
    let should_patch = apply_patches && renderer_key == RENDERER_CONSOLE;

    let mut wrote_to_disk = false;
    let mut all_autofix = true;
    for result in &mut results {
        if result.messages.is_empty() {
            continue;
        }
        if !result.is_all_autofix() {
            all_autofix = false;
        }
        match renderer_key {
            RENDERER_CONSOLE => sink.write(&render_console_result(result)?),
            RENDERER_JSON => sink.write(&format!("{}\n", render_json_result(result))),
            RENDERER_SUMMARY => sink.write(&render_summary_result(result)?),
            RENDERER_COMPILER => sink.write(&render_compiler_result(result)?),
            RENDERER_XML => sink.write(&render_xml_result(result)),
            RENDERER_NONE => {}
            _ => unreachable!("renderer validated above"),
        }

        if should_patch && result.is_patchable() {
            let (patched, _) = build_patched_data(&result.data, &result.messages)?;
            let result_all_autofix = result.is_all_autofix();
            let mut apply = true;
            if prompt_patches && !result_all_autofix {
                prompt_for_patch(result, &patched, &mut sink);
                apply = confirm(
                    &format!("Apply this patch to {}?", result.path),
                    true,
                    input,
                );
            }
            if apply {
                let applied = write_patch_to_disk(result)?;
                for (slot, flag) in result.applied.iter_mut().zip(applied.iter()) {
                    if *flag {
                        *slot = true;
                    }
                }
                wrote_to_disk = true;
            }
        }
    }
    if renderer_key == RENDERER_XML {
        sink.write(&render_xml_footer());
    }

    // `--outfile` lands via temp file plus rename (PHP `TempFile`
    // plus `Filesystem::rename()`), before the amend prompt.
    if let Sink::Buffer(buffer) = &sink {
        flush_outfile(options.outfile.as_deref().unwrap_or_default(), buffer)?;
    }

    maybe_amend(&work, options, wrote_to_disk, all_autofix, input)?;

    let code = result_code(&results);
    if matches!(renderer_key, RENDERER_CONSOLE | RENDERER_SUMMARY) {
        let okay = render_result_code_okay(code);
        if let Some(out_path) = options.outfile.as_deref() {
            // `renderResultCode()` appends through the same sink.
            append_outfile(out_path, &okay)?;
        } else {
            print!("{okay}");
        }
    }
    Ok(code)
}

/// Write renderer output to `--outfile` via temp file plus rename.
fn flush_outfile(out_path: &str, buffer: &str) -> Result<(), String> {
    // Stage next to the destination (same filesystem): system-temp plus
    // rename fails with EXDEV when the workspace lives on another mount
    // (the gates container mounts /work apart from /tmp).
    let staging: PathBuf = match Path::new(out_path).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
    };
    let temp = write_temp_bytes_in(&staging, "arc-rs-lint-out", buffer.as_bytes())
        .ok_or_else(|| "cannot write output file".to_string())?;
    rename_over(&temp, Path::new(out_path)).map_err(|e| format!("cannot write '{out_path}': {e}"))
}

/// Append to `--outfile` (PHP `Filesystem::appendFile()`).
fn append_outfile(out_path: &str, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_path)
        .map_err(|e| format!("cannot write '{out_path}': {e}"))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("cannot write '{out_path}': {e}"))
}

/// Amend HEAD with applied patches
/// (`ArcanistLintWorkflow.php:238-354`).
fn maybe_amend(
    work: &WorkCopy,
    options: &Options,
    wrote_to_disk: bool,
    all_autofix: bool,
    input: &mut dyn BufRead,
) -> Result<(), String> {
    let mut should_amend = options.amend_all || options.amend_autofixes;
    if should_amend {
        let immutable = arcconfig_bool(&work.project_root, "history.immutable")?
            .unwrap_or_else(|| work.vcs.is_history_default_immutable());
        should_amend = work.vcs.supports_amend(&work.vcs_root) && !immutable;
    }
    if !wrote_to_disk || !should_amend {
        return Ok(());
    }
    let automatic = options.amend_all || (options.amend_autofixes && all_autofix);
    let amend = if automatic {
        println!("** LINT NOTICE ** Automatically amending HEAD with lint patches.");
        true
    } else {
        confirm("Amend HEAD with lint patches?", true, input)
    };
    if !amend {
        return Err(
            "Sort out the lint changes that were applied to the working copy and relint."
                .to_string(),
        );
    }
    match work.vcs {
        Vcs::Git => {
            let out = git::output(&work.vcs_root, &["add", "-u"])?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if stderr.is_empty() {
                    return Err("git add -u failed".to_string());
                }
                return Err(format!("git add -u failed: {stderr}"));
            }
            git::amend_commit(&work.vcs_root, None)?;
        }
        Vcs::Hg => {
            hg::amend_commit(&work.vcs_root, None, None)?;
        }
        Vcs::Svn => {}
    }
    Ok(())
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod lint_tests;
