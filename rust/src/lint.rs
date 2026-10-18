//! `arc lint`: run lint rules on changes.
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
//! - Per-linter tool options (`bin`, `flags`, `version`, per-tool
//!   config keys) parse and apply (T1153 LINT-45); no in-tree linter
//!   declares `interpreter` (`shouldUseInterpreter()` is never
//!   overridden at ceb).
//! - Console markup (colors, `##inverse##` highlights) renders on TTYs
//!   via the shared terminal styling helpers (see the `*_with_ansi`
//!   seams) and is stripped otherwise: output
//!   matches piped (non-TTY) `arc` exactly, verified against the golden
//!   files in `src/lint/renderer/__tests__/data/`.
//! - `phutil_console_wrap()` is ported exactly (T1153 LINT-24): content
//!   wraps at `78 - indent` characters with ANSI escapes ignored.
//! - The `OKAY` line and `LINT NOTICE` line render through the shared
//!   formatter (T1153 LINT-5/LINT-6): ` OKAY  No lint messages.` and
//!   ` LINT NOTICE  ...` on non-TTYs. Confirm-prompt decorations and
//!   the XHP rules table are reconstructed (phutil is not vendored):
//!   `{prompt} [Y/n] ` and a bordered ID/Class/Name table.
//! - Descriptions containing `/` are emitted as `\/` in `--output json`
//!   to match PHP `json_encode()` escaping.
//! - `--engine ArcanistUnitTestableLintEngine` and unknown engine classes
//!   are rejected with the not-a-subclass error; PHP could instantiate
//!   other custom engines.
//! - Obsolete-message filtering is not ported: `setObsolete()` has no
//!   caller in the lint surface (producers live in other flows), so no
//!   ported message is ever obsolete. Cached results
//!   (`setCachedResults()`) are honored with per-linter granularity;
//!   cache versions are port-local (PHP mixes linter-file md5s).

extern crate self as arc_rs;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{arclint, git, hg, svn};

use crate::conduit::{self, ClassifiedError, EnvelopeKind};
use crate::terminal::{self, TVal};

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
/// PHP `ArcanistLinter::GRANULARITY_FILE` (`:12`).
pub const GRANULARITY_FILE: u8 = 1;
/// PHP `ArcanistLinter::GRANULARITY_DIRECTORY` (`:13`).
pub const GRANULARITY_DIRECTORY: u8 = 2;
/// PHP `ArcanistLinter::GRANULARITY_REPOSITORY` (`:14`).
pub const GRANULARITY_REPOSITORY: u8 = 3;
/// PHP `ArcanistLinter::GRANULARITY_GLOBAL` (`:15`).
pub const GRANULARITY_GLOBAL: u8 = 4;

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

/// Supported renderer keys in PHP class-map order
/// (`ArcanistLintRenderer::getAllRenderers()`, T1153 LINT-16):
/// alphabetical by class name, so `xml` (Checkstyle) first.
#[must_use]
pub fn all_renderers() -> Vec<&'static str> {
    vec![
        RENDERER_XML,
        RENDERER_COMPILER,
        RENDERER_CONSOLE,
        RENDERER_JSON,
        RENDERER_NONE,
        RENDERER_SUMMARY,
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

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistLintWorkflow.php:42-130`).
const HELP_SYNOPSIS: &str =
    "      **lint** [__options__] [__paths__]\n      **lint** [__options__] --rev [__rev__]";
const HELP_TEXT: &str = "          Supports: git, svn, hg\n          Run static analysis on changes to check for mistakes. If no files\n          are specified, lint will be run on all files which have been modified.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "lintall",
        None,
        None,
        "Show all lint warnings, not just those on changed lines. When paths are specified, this is the default behavior.",
    ),
    (
        "rev",
        None,
        Some("revision"),
        "Lint changes since a specific revision.",
    ),
    ("output", None, Some("format"), "Select an output format."),
    (
        "outfile",
        None,
        Some("path"),
        "Output the linter results to a file. Defaults to stdout.",
    ),
    (
        "engine",
        None,
        Some("classname"),
        "Override configured lint engine for this project.",
    ),
    (
        "apply-patches",
        None,
        None,
        "Apply patches suggested by lint to the working copy without prompting.",
    ),
    (
        "never-apply-patches",
        None,
        None,
        "Never apply patches suggested by lint.",
    ),
    (
        "amend-all",
        None,
        None,
        "When linting git repositories, amend HEAD with all patches suggested by lint without prompting.",
    ),
    (
        "amend-autofixes",
        None,
        None,
        "When linting git repositories, amend HEAD with autofix patches suggested by lint without prompting.",
    ),
    (
        "everything",
        None,
        None,
        "Lint all tracked files in the working copy. Ignored files and untracked files will not be linted.",
    ),
    (
        "severity",
        None,
        Some("string"),
        "Set minimum message severity. One of: 'advice', 'autofix', 'warning', 'error', 'disabled'. Defaults to 'advice'.",
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
        None,
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        false,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(terminal::ansi_enabled())
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
    /// Cache granularity stamped by the engine
    /// (`$message->setGranularity($linter->getCacheGranularity())`,
    /// `ArcanistLintEngine.php:218`); `None` before stamping (PHP
    /// `null`), rendered as JSON `null` (T1153 LINT-11).
    pub granularity: Option<u8>,
    /// Secondary locations (`getOtherLocations()`), rendered as the
    /// console "Other locations" suffix and honored by relevance
    /// filtering (T1153 LINT-13/LINT-14).
    pub locations: Vec<OtherLocation>,
    /// Bypass for changed-line filtering
    /// (`shouldBypassChangedLineFiltering()`); `None` is PHP `null`
    /// (T1153 LINT-14).
    pub bypass_changed_line_filtering: Option<bool>,
    /// Raw-byte patch spans (T1153 LINT-43): when present, the patcher
    /// splices these instead of `original`/`replacement`, so non-UTF-8
    /// files patch byte-exact like PHP's `str_replace()`. Only the
    /// text linter's newline rule sets them; display rendering keeps
    /// the lossy `String` fields.
    pub original_bytes: Option<Vec<u8>>,
    pub replacement_bytes: Option<Vec<u8>>,
}

/// One secondary message location (`getOtherLocation()` dicts:
/// `path`/`line`/`char`, plus `original` for relevance checks).
/// `line`/`char` are PHP's values with 0 meaning unset (message lines
/// are 1-based; location chars are 0-based byte offsets).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtherLocation {
    pub path: Option<String>,
    pub line: usize,
    pub char: usize,
    pub original: Option<String>,
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

    /// The message as an `ArcanistLintMessage::toDictionary()` dict
    /// (T1153 DIFF-D22: `src/lint/ArcanistLintMessage.php:56-71`): the 12
    /// keys in PHP order. Unset line/char render null (0 means unset).
    #[must_use]
    pub fn to_dictionary(&self) -> serde_json::Value {
        let locations: Vec<serde_json::Value> =
            self.locations.iter().map(location_json_dict).collect();
        serde_json::json!({
            "path": self.path,
            "line": if self.line == 0 { serde_json::Value::Null } else { serde_json::json!(self.line) },
            "char": if self.col == 0 { serde_json::Value::Null } else { serde_json::json!(self.col) },
            "code": self.code,
            "severity": self.severity,
            "name": self.name,
            "description": self.description,
            "original": self.original,
            "replacement": self.replacement,
            "granularity": self.granularity,
            "locations": locations,
            "bypassChangedLineFiltering": self.bypass_changed_line_filtering,
        })
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
/// `should_lint_*` methods are the `shouldLint*()` path filters. The
/// configuration methods mirror `getLinterConfigurationOptions()` /
/// `setLinterConfigurationValue()` plus the severity hooks; the engine
/// only calls [`Linter::set_option`] for keys the linter declares.
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
    /// The code form severity maps and rules match against: `raiseLintAt*`
    /// linters look up the short code (display prefix stripped),
    /// direct-message linters the full code (`getLintMessageSeverity()`).
    fn severity_lookup_code<'a>(&self, full_code: &'a str) -> &'a str {
        full_code
    }
    /// False for linters that reject `severity`/`severity.rules`/`standard`
    /// keys (`canCustomizeLintSeverities()`).
    fn can_customize_severities(&self) -> bool {
        true
    }
    /// False for linters whose messages never consult custom severity
    /// maps: PHP only applies `severity`/`severity.rules` inside
    /// `getLintMessageSeverity()`, so linters setting severities
    /// directly (or never calling it) silently ignore configured maps.
    fn applies_custom_severities(&self) -> bool {
        true
    }
    /// Cache granularity (`getCacheGranularity()`): file by default;
    /// only the phutil-library linter overrides it (global).
    fn cache_granularity(&self) -> u8 {
        GRANULARITY_FILE
    }
    /// Linter cache version (`getCacheVersion()`): `0` everywhere in
    /// the port (PHP's XHPAST rule/binary mix is unreproducible
    /// without rules, and reduces to `0` here anyway).
    fn cache_version(&self) -> &'static str {
        "0"
    }
    /// Validate and normalize one `severity` map key
    /// (`getLintCodeFromLinterConfigurationKey()`); the default accepts
    /// any code unchanged.
    fn validate_severity_code(&self, code: &str) -> Result<String, String> {
        Ok(code.to_string())
    }
    /// Validate one `severity` map entry, defaulting to code-only
    /// validation. Overridden where the severity value matters (C#
    /// forbids disabling `SA` rules).
    fn validate_severity_entry(&self, code: &str, _severity: &str) -> Result<String, String> {
        self.validate_severity_code(code)
    }
    /// Apply one declared configuration value
    /// (`setLinterConfigurationValue()`). The engine type-checks
    /// values against the linter's option table first and only calls
    /// this for declared keys; errors abort configuration with a
    /// config error, like PHP.
    fn set_option(
        &mut self,
        key: &str,
        _value: &serde_json::Value,
        _ctx: &OptionContext,
    ) -> Result<(), String> {
        Err(format!("unknown option '{key}'."))
    }
    /// Take a pending halt-everything request (`stopAllLinters()`):
    /// true once when this linter halted all other linters on the
    /// current path (script-and-regex `halt` groups).
    fn take_halt(&self) -> bool {
        false
    }
    /// Take a pending linter failure (T1153 LINT-4/LINT-28/LINT-32):
    /// `Some` once when the last `lint_bytes()` call failed the way
    /// PHP's `lintPath()` throws (script failures, `throw` groups,
    /// missing tools). The engine polls this after every section and
    /// folds failures into the `Some linters failed:` aggregate instead
    /// of aborting or staying silent. The text only surfaces through
    /// direct callers; the workflow aggregate swallows it like PHP.
    fn take_error(&self) -> Option<String> {
        None
    }
    /// Receive the on-disk path of the file being linted (T1153
    /// LINT-26): linters needing filesystem access beyond `data`
    /// (chmod's stat) resolve it against this path, mirroring
    /// `getFilePathOnDisk()` (project root, not the process working
    /// directory). The engine calls this from `lint_file()` before
    /// running the sections; direct `lint_bytes()` callers leave it
    /// unset and linters fall back to the given path as-is.
    fn set_active_disk_path(&self, _disk: &Path) {}
    /// Receive the section's validated custom severity map, for
    /// linters whose own phase gates consult `isMessageEnabled()`
    /// (the text linter's `didStopAllLinters()` gates). The default
    /// ignores it.
    fn set_custom_severity_map(&mut self, _map: &HashMap<String, String>) {}
    /// Receive the section's `severity.rules` in config order, for
    /// linters applying custom severities internally because their
    /// lookup key is not the message code (RuboCop, ruby). The
    /// default ignores it.
    fn set_custom_severity_rules(&mut self, _rules: &[(String, String)]) {}
}

/// Context for [`Linter::set_option`]: the project root resolves
/// relative `bin` candidates and tool config paths, like PHP.
#[derive(Debug, Clone, Copy)]
pub struct OptionContext<'a> {
    /// Project root (`.arclint` directory).
    pub project_root: &'a Path,
}

/// Basic text linter: all nine `ArcanistTextLinter` rules
/// (`src/lint/linter/ArcanistTextLinter.php:93-125`).
#[derive(Debug)]
pub struct TextLinter {
    /// Signed like PHP's `$maxLineLength`: a negative width flags every
    /// line (`strlen($line) > $width`), T1153 LINT-41.
    max_line_length: i64,
    /// Custom severity overrides: DOS-newline and bad-charset stops
    /// only fire for enabled codes (`isMessageEnabled()`, exact map
    /// plus `severity.rules`).
    severity: arclint::SeverityOverrides,
}

impl TextLinter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_line_length: DEFAULT_MAX_LINE_LENGTH as i64,
            severity: arclint::SeverityOverrides::default(),
        }
    }

    /// A text linter with a custom `text.max-line-length`.
    #[must_use]
    pub fn with_max_line_length(max_line_length: usize) -> Self {
        Self {
            max_line_length: i64::try_from(max_line_length).unwrap_or(i64::MAX),
            severity: arclint::SeverityOverrides::default(),
        }
    }

    /// A text linter with an exact-code severity map.
    #[must_use]
    pub fn with_severity_map(severity_map: HashMap<String, String>) -> Self {
        Self {
            max_line_length: DEFAULT_MAX_LINE_LENGTH as i64,
            severity: arclint::SeverityOverrides {
                map: severity_map,
                rules: Vec::new(),
            },
        }
    }

    #[must_use]
    pub fn max_line_length(&self) -> usize {
        usize::try_from(self.max_line_length.max(0)).unwrap_or(usize::MAX)
    }

    /// True when a stop code is not custom-disabled
    /// (`isMessageEnabled()`): exact map plus `severity.rules` by
    /// short code.
    fn stop_enabled(&self, full_code: &str) -> bool {
        let lookup = self.severity_lookup_code(full_code);
        self.severity.severity_for(lookup, SEVERITY_ERROR) != SEVERITY_DISABLED
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

/// Parameter bundle for [`text_message`]: the nine PHP message fields.
struct TextMessageSpec<'a> {
    path: &'a str,
    line: usize,
    col: usize,
    code: &'a str,
    severity: &'a str,
    name: &'a str,
    description: String,
    original: Option<String>,
    replacement: Option<String>,
}

fn text_message(spec: TextMessageSpec<'_>) -> LintMessage {
    LintMessage {
        path: spec.path.to_string(),
        line: spec.line,
        col: spec.col,
        code: spec.code.to_string(),
        severity: spec.severity.to_string(),
        name: spec.name.to_string(),
        description: spec.description,
        original: spec.original,
        replacement: spec.replacement,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    }
}

impl Linter for TextLinter {
    fn name(&self) -> &'static str {
        "TXT"
    }

    fn priority(&self) -> f64 {
        0.5
    }

    fn severity_lookup_code<'a>(&self, full_code: &'a str) -> &'a str {
        full_code.strip_prefix("TXT").unwrap_or(full_code)
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        _ctx: &OptionContext,
    ) -> Result<(), String> {
        match key {
            "text.max-line-length" => {
                self.max_line_length = value.as_i64().unwrap_or(DEFAULT_MAX_LINE_LENGTH as i64);
                Ok(())
            }
            _ => Err(format!("unknown option '{key}'.")),
        }
    }

    fn set_custom_severity_map(&mut self, map: &HashMap<String, String>) {
        self.severity.map.clone_from(map);
    }

    fn set_custom_severity_rules(&mut self, rules: &[(String, String)]) {
        self.severity.rules = rules.to_vec();
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
    // PHP `basename()` splits `/` only (T1153 LINT-44): `foo\.hidden`
    // is not a dotfile there.
    let base = path.rsplit('/').next().unwrap_or(path);
    if base == "__init__.py" || base.starts_with('.') {
        return false;
    }
    // `/^\s*$/`: without `D`, `$` also matches before a trailing `\n`,
    // so any all-whitespace byte string matches.
    if data.iter().all(|byte| is_php_space(*byte)) {
        out.push(text_message(TextMessageSpec {
            path,
            line: 0,
            col: 0,
            code: CODE_EMPTY_FILE,
            severity: SEVERITY_ERROR,
            name: NAME_EMPTY_FILE,
            description: DESC_EMPTY_FILE.to_string(),
            original: None,
            replacement: None,
        }));
        return true;
    }
    false
}

/// `lintNewlines()` (`ArcanistTextLinter.php:150-166`): any CR byte raises
/// `TXT1` at offset 0 with a whole-file patch, and stops all linters
/// (subject to the caller's severity map; the engine re-checks). Returns
/// true when the message fired. The patch spans are raw bytes
/// (`str_replace("\r\n", "\n", $data)`, T1153 LINT-43); the display
/// strings stay lossy.
fn lint_dos_newlines(path: &str, data: &[u8], out: &mut Vec<LintMessage>) -> bool {
    if !data.contains(&b'\r') {
        return false;
    }
    let replacement_bytes = strip_crlf(data);
    let original = String::from_utf8_lossy(data).into_owned();
    let replacement = String::from_utf8_lossy(&replacement_bytes).into_owned();
    let mut message = text_message(TextMessageSpec {
        path,
        line: 1,
        col: 1,
        code: CODE_DOS_NEWLINE,
        severity: SEVERITY_ERROR,
        name: NAME_DOS_NEWLINE,
        description: DESC_DOS_NEWLINE.to_string(),
        original: Some(original),
        replacement: Some(replacement),
    });
    message.original_bytes = Some(data.to_vec());
    message.replacement_bytes = Some(replacement_bytes);
    out.push(message);
    true
}

/// Byte-level `str_replace("\r\n", "\n", $data)`: collapse CR-LF pairs,
/// leaving lone CR bytes in place.
fn strip_crlf(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == b'\r' && data.get(index + 1) == Some(&b'\n') {
            index += 1;
        }
        out.push(data[index]);
        index += 1;
    }
    out
}

/// `lintTabs()` (`ArcanistTextLinter.php:168-177`): only the FIRST tab is
/// reported, with the tab as non-patchable context.
fn lint_tabs(path: &str, data: &[u8], out: &mut Vec<LintMessage>) {
    let Some(offset) = data.iter().position(|byte| *byte == b'\t') else {
        return;
    };
    let (line, col) = line_col_for_offset(data, offset);
    out.push(text_message(TextMessageSpec {
        path,
        line,
        col,
        code: CODE_TAB,
        severity: SEVERITY_ERROR,
        name: NAME_TAB,
        description: DESC_TAB.to_string(),
        original: Some("\t".to_string()),
        replacement: None,
    }));
}

/// `lintLineLength()` (`ArcanistTextLinter.php:179-197`): lines longer
/// than `width` bytes raise `TXT3` warnings (signed: a negative width
/// flags every line, T1153 LINT-41). Only the line length is
/// comma-formatted (`PhutilNumber`); the width prints raw (T1153
/// LINT-36).
fn lint_line_length(path: &str, data: &[u8], width: i64, out: &mut Vec<LintMessage>) {
    for (index, line) in split_lines(data).iter().enumerate() {
        if line.len() as i64 > width {
            out.push(text_message(TextMessageSpec {
                path,
                line: index + 1,
                col: 1,
                code: CODE_LINE_WRAP,
                severity: SEVERITY_WARNING,
                name: NAME_LINE_WRAP,
                description: format!(
                    "This line is {} characters long, but the convention is {width} characters.",
                    format_number(line.len()),
                ),
                original: Some(String::from_utf8_lossy(line).into_owned()),
                replacement: None,
            }));
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
    out.push(text_message(TextMessageSpec {
        path,
        line,
        col,
        code: CODE_EOF_NEWLINE,
        severity: SEVERITY_ERROR,
        name: NAME_EOF_NEWLINE,
        description: DESC_EOF_NEWLINE.to_string(),
        original: Some(String::new()),
        replacement: Some("\n".to_string()),
    }));
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
            out.push(text_message(TextMessageSpec {
                path,
                line: line_no,
                col,
                code: CODE_BAD_CHARSET,
                severity: SEVERITY_ERROR,
                name: NAME_BAD_CHARSET,
                description: DESC_BAD_CHARSET.to_string(),
                original: Some(String::from_utf8_lossy(span).into_owned()),
                replacement: None,
            }));
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
        out.push(text_message(TextMessageSpec {
            path,
            line,
            col,
            code: CODE_TRAILING,
            severity: SEVERITY_AUTOFIX,
            name: NAME_TRAILING,
            description: DESC_TRAILING.to_string(),
            original: Some(String::from_utf8_lossy(&data[offset..offset + len]).into_owned()),
            replacement: Some(String::new()),
        }));
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
    out.push(text_message(TextMessageSpec {
        path,
        line: 1,
        col: 1,
        code: CODE_BOF_WHITESPACE,
        severity: SEVERITY_AUTOFIX,
        name: NAME_BOF_WHITESPACE,
        description: DESC_BOF_WHITESPACE.to_string(),
        original: Some(String::from_utf8_lossy(&data[..match_end]).into_owned()),
        replacement: Some(String::new()),
    }));
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
    out.push(text_message(TextMessageSpec {
        path,
        line,
        col,
        code: CODE_EOF_WHITESPACE,
        severity: SEVERITY_AUTOFIX,
        name: NAME_EOF_WHITESPACE,
        description: DESC_EOF_WHITESPACE.to_string(),
        original: Some(String::from_utf8_lossy(&data[start..]).into_owned()),
        replacement: Some(String::new()),
    }));
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

/// Runs a set of linters over files: one section per configured
/// linter, each with its own routed paths, severity overrides, and
/// tool options, like PHP's `buildLinters()`.
pub struct Engine {
    sections: Vec<Section>,
    /// Route linters per path like [`ENGINE_COMPREHENSIVE`].
    comprehensive: bool,
    /// Single-linter glue engine ([`ENGINE_SINGLE`]): directories never
    /// reach linters, like `ArcanistSingleLintEngine::buildLinters()`
    /// stripping them from the paths (T1153 LINT-22).
    single: bool,
    /// Injected prior results by path and cache version
    /// (`setCachedResults()`).
    cached: CachedResults,
    /// Current repository version for cache granularity checks.
    repository_version: Option<String>,
}

/// One configured linter section: a linter instance plus the routing,
/// severity overrides, and applied options from its `.arclint`
/// section (`ArcanistConfigurationDrivenLintEngine::buildLinters()`).
struct Section {
    linter: Box<dyn Linter>,
    /// PHP class name, for cache-version computation.
    class: &'static str,
    routing: arclint::SectionRouting,
    severity: arclint::SeverityOverrides,
}

impl Section {
    /// An unconfigured section running `linter` on every path.
    fn open(class: &'static str, linter: Box<dyn Linter>) -> Self {
        Self {
            linter,
            class,
            routing: arclint::SectionRouting::default(),
            severity: arclint::SeverityOverrides::default(),
        }
    }
}

/// Cached lint results by path and cache version
/// (`setCachedResults()`): path -> version -> results.
pub type CachedResults = HashMap<String, HashMap<u32, CachedPathResults>>;

/// One path's cached results at one cache version.
#[derive(Debug, Clone, Default)]
pub struct CachedPathResults {
    /// `repository_version` recorded with the results.
    pub repository_version: Option<String>,
    /// Section index that stopped the path, if any (`stopped`).
    pub stopped: Option<usize>,
    /// Cached messages with their recorded granularity.
    pub messages: Vec<CachedMessage>,
}

/// One cached message (`newFromDictionary()` shape): the message plus
/// its recorded granularity (`None` when unknown, which never reuses
/// the cache, like PHP's `default` branch).
#[derive(Debug, Clone)]
pub struct CachedMessage {
    /// Reconstructed message.
    pub message: LintMessage,
    /// Recorded granularity.
    pub granularity: Option<u8>,
}

impl CachedMessage {
    /// Rebuild a cached message from a `toDictionary()` map
    /// (`newFromDictionary()`); returns `None` when required keys
    /// are missing or mistyped. `granularity`, `locations`, and
    /// `bypassChangedLineFiltering` restore onto the message (T1153
    /// LINT-11/LINT-13/LINT-14) as well as the cache entry.
    #[must_use]
    pub fn from_dict(dict: &HashMap<String, serde_json::Value>) -> Option<Self> {
        let text = |key: &str| dict.get(key).and_then(|value| value.as_str());
        let granularity = dict
            .get("granularity")
            .and_then(serde_json::Value::as_u64)
            .and_then(|granularity| u8::try_from(granularity).ok());
        let message = LintMessage {
            path: text("path")?.to_string(),
            line: dict
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
                .unwrap_or(0),
            col: dict
                .get("char")
                .and_then(serde_json::Value::as_u64)
                .and_then(|col| usize::try_from(col).ok())
                .unwrap_or(0),
            code: text("code")?.to_string(),
            severity: text("severity")?.to_string(),
            name: text("name")?.to_string(),
            description: text("description")?.to_string(),
            original: text("original").map(str::to_string),
            replacement: text("replacement").map(str::to_string),
            granularity,
            locations: dict
                .get("locations")
                .and_then(serde_json::Value::as_array)
                .map(|items| items.iter().filter_map(location_from_dict).collect())
                .unwrap_or_default(),
            bypass_changed_line_filtering: dict.get("bypassChangedLineFiltering").and_then(
                |value| {
                    if value.is_null() {
                        None
                    } else {
                        Some(value.as_bool().unwrap_or(false))
                    }
                },
            ),
            original_bytes: None,
            replacement_bytes: None,
        };
        Some(Self {
            message,
            granularity,
        })
    }
}

/// Rebuild one `locations` entry (`path`/`line`/`char`/`original` keys;
/// unknown keys only arise from custom linters and are dropped).
fn location_from_dict(value: &serde_json::Value) -> Option<OtherLocation> {
    let entry = value.as_object()?;
    let number = |key: &str| {
        entry
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
    };
    Some(OtherLocation {
        path: entry
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        line: number("line"),
        char: number("char"),
        original: entry
            .get("original")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

/// Every ported linter, in registration order. `lint_external` stays out:
/// the PHP base class is abstract and defines no linter of its own.
#[must_use]
pub fn all_linters() -> Vec<Box<dyn Linter>> {
    all_linter_pairs()
        .into_iter()
        .map(|(_, linter)| linter)
        .collect()
}

/// Every ported linter with its canonical `.arclint` configuration
/// name (for cache-version class lookup).
fn all_linter_pairs() -> Vec<(&'static str, Box<dyn Linter>)> {
    vec![
        ("text", Box::new(TextLinter::new())),
        ("chmod", Box::new(crate::lint_chmod::ChmodLinter::new())),
        (
            "closure",
            Box::new(crate::lint_closure::ClosureLinter::new()),
        ),
        (
            "coffeelint",
            Box::new(crate::lint_coffeelint::CoffeelintLinter::new()),
        ),
        (
            "composer",
            Box::new(crate::lint_composer::ComposerLinter::new()),
        ),
        (
            "cppcheck",
            Box::new(crate::lint_cppcheck::CppcheckLinter::new()),
        ),
        (
            "cpplint",
            Box::new(crate::lint_cpplint::CpplintLinter::new()),
        ),
        ("csharp", Box::new(crate::lint_csharp::CsharpLinter::new())),
        (
            "csslint",
            Box::new(crate::lint_csslint::CsslintLinter::new()),
        ),
        (
            "filename",
            Box::new(crate::lint_filename::FilenameLinter::new()),
        ),
        ("flake8", Box::new(crate::lint_flake8::Flake8Linter::new())),
        (
            "generated",
            Box::new(crate::lint_generated::GeneratedLinter::new()),
        ),
        ("golint", Box::new(crate::lint_golint::GolintLinter::new())),
        ("hlint", Box::new(crate::lint_hlint::HlintLinter::new())),
        ("jscs", Box::new(crate::lint_jscs::JscsLinter::new())),
        ("jshint", Box::new(crate::lint_jshint::JshintLinter::new())),
        ("json", Box::new(crate::lint_json::JsonLinter::new())),
        (
            "jsonlint",
            Box::new(crate::lint_jsonlint::JsonlintLinter::new()),
        ),
        ("lessc", Box::new(crate::lint_lessc::LesscLinter::new())),
        (
            "merge-conflict",
            Box::new(crate::lint_merge_conflict::MergeConflictLinter::new()),
        ),
        ("nolint", Box::new(crate::lint_nolint::NoLintLinter::new())),
        ("pep8", Box::new(crate::lint_pep8::Pep8Linter::new())),
        ("php", Box::new(crate::lint_php::PhpLinter::new())),
        ("phpcs", Box::new(crate::lint_phpcs::PhpcsLinter::new())),
        (
            "phutil-library",
            Box::new(crate::lint_phutil_library::PhutilLibraryLinter::new()),
        ),
        (
            "puppet-lint",
            Box::new(crate::lint_puppetlint::PuppetlintLinter::new()),
        ),
        (
            "pyflakes",
            Box::new(crate::lint_pyflakes::PyFlakesLinter::new()),
        ),
        ("pylint", Box::new(crate::lint_pylint::PyLintLinter::new())),
        (
            "rubocop",
            Box::new(crate::lint_rubocop::RuboCopLinter::new()),
        ),
        ("ruby", Box::new(crate::lint_ruby::RubyLinter::new())),
        (
            "script-and-regex",
            Box::new(crate::lint_script_and_regex::ScriptAndRegexLinter::new()),
        ),
        (
            "spelling",
            Box::new(crate::lint_spelling::SpellingLinter::new()),
        ),
        ("xhpast", Box::new(crate::lint_xhpast::XhpastLinter::new())),
        ("xml", Box::new(crate::lint_xml::XmlLinter::new())),
    ]
}

/// Open sections from canonical-name pairs, resolving PHP classes.
fn sections_from_pairs(pairs: Vec<(&'static str, Box<dyn Linter>)>) -> Vec<Section> {
    pairs
        .into_iter()
        .map(|(canonical, linter)| {
            let class =
                crate::linters::class_for_config(canonical).unwrap_or("ArcanistUnknownLinter");
            Section::open(class, linter)
        })
        .collect()
}

/// IEEE CRC32 (`crc32()`), bitwise (no table): polynomial
/// `0xEDB88320`, init/xor `0xFFFFFFFF`.
fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = crc & 1;
            crc >>= 1;
            if mask != 0 {
                crc ^= 0xEDB8_8320;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// True when cached results may be reused (`shouldUseCache()`):
/// file granularity always, directory/repository only when the
/// repository version matches, anything else never.
fn should_use_cache(
    granularity: Option<u8>,
    cached_repository: Option<&str>,
    current_repository: Option<&str>,
) -> bool {
    match granularity {
        Some(GRANULARITY_FILE) => true,
        Some(GRANULARITY_DIRECTORY | GRANULARITY_REPOSITORY) => {
            current_repository == cached_repository
        }
        _ => false,
    }
}

/// Build the linter for a `.arclint` `type`: exact configuration
/// names only (display names never resolve, T1153 LINT-38), mirroring
/// [`arclint::canonical_config_name`].
fn linter_for_type(linter_type: &str) -> Option<Box<dyn Linter>> {
    let canonical = arclint::canonical_config_name(linter_type)?;
    linter_for_text_to_flake8(canonical)
        .or_else(|| linter_for_generated_to_pep8(canonical))
        .or_else(|| linter_for_php_to_xml(canonical))
}

/// Construct a linter for a canonical config name in `text..=flake8`.
fn linter_for_text_to_flake8(canonical: &str) -> Option<Box<dyn Linter>> {
    let linter: Box<dyn Linter> = match canonical {
        "text" => Box::new(TextLinter::new()),
        "chmod" => Box::new(crate::lint_chmod::ChmodLinter::new()),
        "closure" | "gjslint" => Box::new(crate::lint_closure::ClosureLinter::new()),
        "coffeelint" => Box::new(crate::lint_coffeelint::CoffeelintLinter::new()),
        "composer" => Box::new(crate::lint_composer::ComposerLinter::new()),
        "cppcheck" => Box::new(crate::lint_cppcheck::CppcheckLinter::new()),
        "cpplint" => Box::new(crate::lint_cpplint::CpplintLinter::new()),
        "csharp" => Box::new(crate::lint_csharp::CsharpLinter::new()),
        "csslint" => Box::new(crate::lint_csslint::CsslintLinter::new()),
        "filename" => Box::new(crate::lint_filename::FilenameLinter::new()),
        "flake8" => Box::new(crate::lint_flake8::Flake8Linter::new()),
        _ => return None,
    };
    Some(linter)
}

/// Construct a linter for a canonical config name in
/// `generated..=pep8`.
fn linter_for_generated_to_pep8(canonical: &str) -> Option<Box<dyn Linter>> {
    let linter: Box<dyn Linter> = match canonical {
        "generated" => Box::new(crate::lint_generated::GeneratedLinter::new()),
        "golint" => Box::new(crate::lint_golint::GolintLinter::new()),
        "hlint" => Box::new(crate::lint_hlint::HlintLinter::new()),
        "jscs" => Box::new(crate::lint_jscs::JscsLinter::new()),
        "jshint" => Box::new(crate::lint_jshint::JshintLinter::new()),
        "json" => Box::new(crate::lint_json::JsonLinter::new()),
        "jsonlint" => Box::new(crate::lint_jsonlint::JsonlintLinter::new()),
        "lessc" => Box::new(crate::lint_lessc::LesscLinter::new()),
        "merge-conflict" => Box::new(crate::lint_merge_conflict::MergeConflictLinter::new()),
        "nolint" => Box::new(crate::lint_nolint::NoLintLinter::new()),
        "pep8" => Box::new(crate::lint_pep8::Pep8Linter::new()),
        _ => return None,
    };
    Some(linter)
}

/// Construct a linter for a canonical config name in `php..=xml`.
fn linter_for_php_to_xml(canonical: &str) -> Option<Box<dyn Linter>> {
    let linter: Box<dyn Linter> = match canonical {
        "php" => Box::new(crate::lint_php::PhpLinter::new()),
        "phpcs" => Box::new(crate::lint_phpcs::PhpcsLinter::new()),
        "phutil-library" => Box::new(crate::lint_phutil_library::PhutilLibraryLinter::new()),
        "puppet-lint" => Box::new(crate::lint_puppetlint::PuppetlintLinter::new()),
        "pyflakes" => Box::new(crate::lint_pyflakes::PyFlakesLinter::new()),
        "pylint" => Box::new(crate::lint_pylint::PyLintLinter::new()),
        "rubocop" => Box::new(crate::lint_rubocop::RuboCopLinter::new()),
        "ruby" => Box::new(crate::lint_ruby::RubyLinter::new()),
        "script-and-regex" => Box::new(crate::lint_script_and_regex::ScriptAndRegexLinter::new()),
        "spelling" => Box::new(crate::lint_spelling::SpellingLinter::new()),
        "xhpast" => Box::new(crate::lint_xhpast::XhpastLinter::new()),
        "xml" => Box::new(crate::lint_xml::XmlLinter::new()),
        _ => return None,
    };
    Some(linter)
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
/// Expected value shape for a linter-specific option, mirroring the
/// PhutilTypeSpec types in `getLinterConfigurationOptions()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionType {
    /// `string`
    Str,
    /// `bool`
    Bool,
    /// `int`
    Int,
    /// `list<string>`
    StrList,
    /// `string | list<string>`
    StrOrStrList,
    /// `regex` (a string compiling as PCRE)
    Regex,
    /// `map<string, list<string>>`
    MapStrList,
    /// `map<string, string>`
    MapStrStr,
    /// `map<string, int>`
    MapStrInt,
}

/// Linter-specific option keys in PHP table order (without `type`,
/// `include`, `exclude`, `severity`, `severity.rules`, `standard`,
/// which the engine handles), plus whether custom keys apply after
/// the base options (lessc, csharp, and xhpast append the parent
/// table instead of prepending it).
fn linter_option_table(config_name: &str) -> (&'static [(&'static str, OptionType)], bool) {
    const TOOL: &[(&str, OptionType)] = &[
        ("bin", OptionType::StrOrStrList),
        ("flags", OptionType::StrList),
        ("version", OptionType::Str),
    ];
    match config_name {
        "text" => (&[("text.max-line-length", OptionType::Int)], false),
        "coffeelint" => (
            &[
                ("coffeelint.config", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "jscs" => (
            &[
                ("jscs.config", OptionType::Str),
                ("jscs.preset", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "jshint" => (
            &[
                ("jshint.jshintignore", OptionType::Str),
                ("jshint.jshintrc", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "phpcs" => (
            &[
                ("phpcs.standard", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "puppet-lint" => (
            &[
                ("puppet-lint.config", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "pylint" => (
            &[
                ("pylint.config", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "rubocop" => (
            &[
                ("rubocop.config", OptionType::Str),
                ("bin", OptionType::StrOrStrList),
                ("flags", OptionType::StrList),
                ("version", OptionType::Str),
            ],
            false,
        ),
        "csharp" => (
            &[
                ("discovery", OptionType::MapStrList),
                ("binary", OptionType::Str),
            ],
            true,
        ),
        "spelling" => (&[("spelling.dictionaries", OptionType::StrList)], false),
        "script-and-regex" => (
            &[
                ("script-and-regex.script", OptionType::Str),
                ("script-and-regex.regex", OptionType::Regex),
            ],
            false,
        ),
        "xhpast" => (
            &[
                ("xhpast.php-version", OptionType::Str),
                ("xhpast.php-version.windows", OptionType::Str),
                ("xhpast.blacklisted.function", OptionType::MapStrStr),
                ("xhpast.deprecated.functions", OptionType::MapStrStr),
                ("xhpast.printf-functions", OptionType::MapStrInt),
                ("xhpast.switchhook", OptionType::Str),
                ("xhpast.naminghook", OptionType::Str),
                ("xhpast.dynamic-string.classes", OptionType::MapStrStr),
                ("xhpast.dynamic-string.functions", OptionType::MapStrStr),
            ],
            true,
        ),
        other => linter_tool_option_table(other, TOOL),
    }
}

/// Option table for linters sharing the `bin`/`flags`/`version` tool
/// keys without custom options (lessc appends its custom keys after
/// the base options, hence `true`).
fn linter_tool_option_table(
    config_name: &str,
    tool: &'static [(&'static str, OptionType)],
) -> (&'static [(&'static str, OptionType)], bool) {
    match config_name {
        "lessc" => (tool, true),
        "closure" | "gjslint" | "cppcheck" | "cpplint" | "csslint" | "flake8" | "golint"
        | "hlint" | "jsonlint" | "pep8" | "php" | "pyflakes" | "ruby" => (tool, false),
        _ => (&[], false),
    }
}

/// Lessc appends its custom keys after the base options; they live
/// outside [`linter_option_table`] so the table stays a plain slice.
const LESSC_CUSTOM_OPTIONS: &[(&str, OptionType)] = &[
    ("lessc.strict-math", OptionType::Bool),
    ("lessc.strict-units", OptionType::Bool),
];

/// Type-check one option value, mirroring the `checkMap()` type
/// validation (inner phutil messages are not reproducible here; the
/// shape follows the established local config-error text).
fn check_option_type(
    key: &str,
    expected: OptionType,
    value: &serde_json::Value,
) -> Result<(), String> {
    if option_value_matches(expected, value) {
        return Ok(());
    }
    Err(format!(
        "'{key}' must be {}",
        option_type_expectation(expected)
    ))
}

/// True when `value` has the JSON shape of `expected`. `Int` is a
/// signed 64-bit integer like PHP's `int` (negatives pass, T1153
/// LINT-41).
fn option_value_matches(expected: OptionType, value: &serde_json::Value) -> bool {
    match expected {
        OptionType::Str => value.is_string(),
        OptionType::Bool => value.is_boolean(),
        OptionType::Int => value.as_i64().is_some(),
        OptionType::StrList => is_str_list_value(value),
        OptionType::StrOrStrList => is_str_or_str_list_value(value),
        OptionType::Regex => is_valid_regex_value(value),
        OptionType::MapStrList => is_map_str_list_value(value),
        OptionType::MapStrStr => is_map_str_str_value(value),
        OptionType::MapStrInt => is_map_str_int_value(value),
    }
}

/// Human-readable expectation for an [`OptionType`], used in the
/// config-error text.
fn option_type_expectation(expected: OptionType) -> &'static str {
    match expected {
        OptionType::Str => "a string",
        OptionType::Bool => "a boolean",
        OptionType::Int => "an integer",
        OptionType::StrList => "a list of strings",
        OptionType::StrOrStrList => "a string or a list of strings",
        OptionType::Regex => "a valid regular expression",
        OptionType::MapStrList => "a map from strings to lists of strings",
        OptionType::MapStrStr => "a map from strings to strings",
        OptionType::MapStrInt => "a map from strings to integers",
    }
}

/// True for a JSON array of strings.
fn is_str_list_value(value: &serde_json::Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|item| item.is_string()))
}

/// True for a JSON string or an array of strings.
fn is_str_or_str_list_value(value: &serde_json::Value) -> bool {
    value.is_string() || is_str_list_value(value)
}

/// True for a string compiling as PCRE.
fn is_valid_regex_value(value: &serde_json::Value) -> bool {
    value
        .as_str()
        .is_some_and(|pattern| arclint::compile_pcre(pattern).is_ok())
}

/// True for a JSON object whose values are arrays of strings.
fn is_map_str_list_value(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.values().all(is_str_list_value))
}

/// True for a JSON object whose values are strings.
fn is_map_str_str_value(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.values().all(|entry| entry.is_string()))
}

/// True for a JSON object whose values are integers.
fn is_map_str_int_value(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.values().all(|entry| entry.as_u64().is_some()))
}

/// The `phutil.xhpast` standard's severity map (rule ID to severity),
/// applied to XHP sections naming the standard.
const PHUTIL_XHPAST_STANDARD_SEVERITY: &[(&str, &str)] =
    &[("16", "advice"), ("34", "error"), ("87", "warning")];

/// The `phutil.xhpast` standard's rule configuration, applied through
/// the XHP linter like PHP's `setLinterConfigurationValue()` loop
/// (verbatim from `ArcanistPhutilXHPASTLinterStandard.php:25-56`,
/// T1153 LINT-30).
fn phutil_xhpast_standard_config() -> serde_json::Value {
    serde_json::json!({
        "xhpast.blacklisted.function": {
            "eval": "The `eval` function should be avoided. It is potentially unsafe and makes debugging more difficult.",
        },
        "xhpast.php-version": "5.2.3",
        "xhpast.php-version.windows": "5.3.0",
        "xhpast.dynamic-string.classes": {
            "ExecFuture": 0,
        },
        "xhpast.dynamic-string.functions": {
            "pht": 0,
            "hsprintf": 0,
            "jsprintf": 0,
            "hgsprintf": 0,
            "csprintf": 0,
            "vcsprintf": 0,
            "execx": 0,
            "exec_manual": 0,
            "phutil_passthru": 0,
            "qsprintf": 1,
            "vqsprintf": 1,
            "queryfx": 1,
            "queryfx_all": 1,
            "queryfx_one": 1,
        },
    })
}

/// Prefix `.arclint` option errors with the linter name, as a proxy
/// chain link (T1153 LINT-40): `checkMap()` failures use the `for
/// linter` context, `setLinterConfigurationValue()` failures the `in
/// key` context, each followed by the cause on the next line.
fn arclint_error(name: &str, detail: String) -> String {
    format!("Error in parsing '.arclint' file, for linter '{name}'.\n{detail}")
}

/// Prefix `.arclint` option-setter errors with the linter name and key
/// (`Error in parsing ... in key ...`, T1153 LINT-40).
fn arclint_key_error(name: &str, key: &str, detail: String) -> String {
    format!("Error in parsing '.arclint' file, in key '{key}' for linter '{name}'.\n{detail}")
}

/// Apply base/custom/lessc options in PHP table order, then validate the
/// severity map; each stage wraps its own errors with the linter name.
fn apply_section_options(
    linter: &mut Box<dyn Linter>,
    spec: &arclint::LinterSpec,
    canonical: &str,
    table: &[(&str, OptionType)],
    ctx: &OptionContext,
    custom_after_base: bool,
) -> Result<arclint::SeverityOverrides, String> {
    let mut severity;
    if custom_after_base {
        severity = apply_base_options(linter, spec, canonical, ctx)?;
        apply_custom_options(linter, spec, canonical, table, ctx)?;
        apply_lessc_options(linter, spec, canonical, ctx)?;
    } else {
        apply_custom_options(linter, spec, canonical, table, ctx)?;
        severity = apply_base_options(linter, spec, canonical, ctx)?;
    }
    severity.map = validate_severity_map(linter.as_ref(), &severity.map)
        .map_err(|detail| arclint_key_error(&spec.name, "severity", detail))?;
    Ok(severity)
}

/// Build one configured section: instantiate the linter by type,
/// validate and apply options in PHP table order, validate severity
/// codes, resolve `standard`, and compile routing.
fn build_section(
    spec: &arclint::LinterSpec,
    global_exclude: &[String],
    project_root: &Path,
) -> Result<Section, String> {
    let mut linter = linter_for_type(&spec.linter_type).ok_or_else(|| {
        format!(
            "Error in parsing '.arclint' file: unknown linter type '{}'.",
            spec.linter_type
        )
    })?;
    let canonical = arclint::canonical_config_name(&spec.linter_type).unwrap_or("text");
    let (table, custom_after_base) = linter_option_table(canonical);
    reject_unknown_options(
        &spec.extra,
        table,
        canonical,
        linter.can_customize_severities(),
    )
    .map_err(|detail| arclint_error(&spec.name, detail))?;
    let ctx = OptionContext { project_root };
    let severity =
        apply_section_options(&mut linter, spec, canonical, table, &ctx, custom_after_base)?;
    linter.set_custom_severity_map(&severity.map);
    linter.set_custom_severity_rules(&severity.rules);
    Ok(Section {
        linter,
        class: crate::linters::class_for_config(canonical).unwrap_or("ArcanistUnknownLinter"),
        routing: arclint::SectionRouting::new(&spec.include, &spec.exclude, global_exclude),
        severity,
    })
}

/// Reject `.arclint` keys no linter table declares, in sorted order
/// for deterministic errors (PHP's `checkMap()` rejects surplus keys
/// before applying any value).
fn reject_unknown_options(
    extra: &HashMap<String, serde_json::Value>,
    table: &[(&str, OptionType)],
    canonical: &str,
    can_customize: bool,
) -> Result<(), String> {
    let mut keys: Vec<&str> = extra.keys().map(String::as_str).collect();
    keys.sort_unstable();
    for key in keys {
        if key == "standard" {
            if can_customize {
                continue;
            }
            return Err("unknown option 'standard'.".to_string());
        }
        if table.iter().any(|(known, _)| *known == key) {
            continue;
        }
        if canonical == "lessc" && LESSC_CUSTOM_OPTIONS.iter().any(|(known, _)| *known == key) {
            continue;
        }
        return Err(format!("unknown option '{key}'."));
    }
    Ok(())
}

/// Apply linter-specific options in PHP table order (type-check, then
/// `set_option`, like `checkMap()` feeding
/// `setLinterConfigurationValue()`): type failures use the `for
/// linter` context, setter failures the `in key` context (T1153
/// LINT-40).
fn apply_custom_options(
    linter: &mut Box<dyn Linter>,
    spec: &arclint::LinterSpec,
    canonical: &str,
    table: &[(&str, OptionType)],
    ctx: &OptionContext,
) -> Result<(), String> {
    for (key, expected) in table {
        let Some(value) = spec.extra.get(*key) else {
            continue;
        };
        if canonical == "text" && *key == "text.max-line-length" {
            apply_max_line_length(linter, spec, value, ctx)?;
            continue;
        }
        check_option_type(key, *expected, value)
            .map_err(|detail| arclint_error(&spec.name, detail))?;
        linter
            .set_option(key, value, ctx)
            .map_err(|detail| arclint_key_error(&spec.name, key, detail))?;
    }
    Ok(())
}

/// Lessc's custom keys apply after the base options.
fn apply_lessc_options(
    linter: &mut Box<dyn Linter>,
    spec: &arclint::LinterSpec,
    canonical: &str,
    ctx: &OptionContext,
) -> Result<(), String> {
    if canonical != "lessc" {
        return Ok(());
    }
    for (key, expected) in LESSC_CUSTOM_OPTIONS {
        let Some(value) = spec.extra.get(*key) else {
            continue;
        };
        check_option_type(key, *expected, value)
            .map_err(|detail| arclint_error(&spec.name, detail))?;
        linter
            .set_option(key, value, ctx)
            .map_err(|detail| arclint_key_error(&spec.name, key, detail))?;
    }
    Ok(())
}

/// Apply `text.max-line-length`, preserving the long-standing error
/// text for non-integer values. Negatives pass through like PHP's
/// `'optional int'` (T1153 LINT-41).
fn apply_max_line_length(
    linter: &mut Box<dyn Linter>,
    spec: &arclint::LinterSpec,
    value: &serde_json::Value,
    ctx: &OptionContext,
) -> Result<(), String> {
    check_option_type("text.max-line-length", OptionType::Int, value).map_err(|_| {
        arclint_error(
            &spec.name,
            "'text.max-line-length' must be an integer".to_string(),
        )
    })?;
    linter
        .set_option("text.max-line-length", value, ctx)
        .map_err(|detail| arclint_key_error(&spec.name, "text.max-line-length", detail))
}

/// Apply the base options: reject `severity` keys for linters that
/// cannot customize severities, and resolve `standard` names. Shape
/// rejections use the `for linter` context (`checkMap()`); `standard`
/// resolution failures the `in key 'standard'` context (T1153 LINT-40).
fn apply_base_options(
    linter: &mut Box<dyn Linter>,
    spec: &arclint::LinterSpec,
    canonical: &str,
    ctx: &OptionContext,
) -> Result<arclint::SeverityOverrides, String> {
    let mut severity = spec.severity.clone();
    if !linter.can_customize_severities() {
        if !severity.map.is_empty() {
            return Err(arclint_error(
                &spec.name,
                "unknown option 'severity'.".to_string(),
            ));
        }
        if !severity.rules.is_empty() {
            return Err(arclint_error(
                &spec.name,
                "unknown option 'severity.rules'.".to_string(),
            ));
        }
        if spec.extra.contains_key("standard") {
            return Err(arclint_error(
                &spec.name,
                "unknown option 'standard'.".to_string(),
            ));
        }
        return Ok(severity);
    }
    let Some(standard) = spec.extra.get("standard") else {
        return Ok(severity);
    };
    check_option_type("standard", OptionType::StrOrStrList, standard)
        .map_err(|detail| arclint_error(&spec.name, detail))?;
    let mut names = Vec::new();
    if let Some(name) = standard.as_str() {
        names.push(name.to_string());
    } else if let Some(items) = standard.as_array() {
        for item in items {
            names.push(item.as_str().unwrap_or_default().to_string());
        }
    } // LCOV_EXCL_LINE // Reason: dead fall-through; the StrOrStrList check guarantees str or array so one branch always takes
    for name in names {
        apply_standard(&mut severity, linter, canonical, &name, ctx)
            .map_err(|detail| arclint_key_error(&spec.name, "standard", detail))?;
    }
    Ok(severity)
}

/// Resolve one `standard` name: only `phutil.xhpast` on the XHP linter
/// exists (`ArcanistLinterStandard::getStandard()`); anything else is
/// a configuration error.
fn apply_standard(
    severity: &mut arclint::SeverityOverrides,
    linter: &mut Box<dyn Linter>,
    canonical: &str,
    name: &str,
    ctx: &OptionContext,
) -> Result<(), String> {
    if name != "phutil.xhpast" || canonical != "xhpast" {
        let available = if canonical == "xhpast" {
            "phutil.xhpast"
        } else {
            ""
        };
        return Err(format!(
            "No such linter standard. Available standards are: {available}."
        ));
    }
    // `addCustomSeverityMap()`: existing entries win over the standard.
    for (code, standard_severity) in PHUTIL_XHPAST_STANDARD_SEVERITY {
        severity
            .map
            .entry((*code).to_string())
            .or_insert_with(|| (*standard_severity).to_string());
    }
    // The standard's rule configuration feeds the linter like PHP's
    // `setLinterConfigurationValue()` loop (explicit keys apply after
    // and overwrite these).
    let config = phutil_xhpast_standard_config();
    let serde_json::Value::Object(entries) = &config else {
        return Ok(()); // LCOV_EXCL_LINE // Reason: dead arm; the standard config is a literal object
    };
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    for key in keys {
        linter.set_option(key, &entries[key], ctx)?;
    }
    Ok(())
}

/// Validate `severity` map codes through the linter
/// (`getLintCodeFromLinterConfigurationKey()`), normalizing keys.
fn validate_severity_map(
    linter: &dyn Linter,
    map: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    let mut codes: Vec<&String> = map.keys().collect();
    codes.sort();
    for code in codes {
        let normalized = linter.validate_severity_entry(code, &map[code])?;
        out.insert(normalized, map[code].clone());
    }
    Ok(out)
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sections: sections_from_pairs(all_linter_pairs()),
            comprehensive: false,
            single: false,
            cached: CachedResults::new(),
            repository_version: None,
        }
    }

    /// An engine running one section per configured linter, each with
    /// its own routed paths, severity overrides, and tool options
    /// (`buildLinters()`). `project_root` resolves relative `bin`
    /// candidates and tool config paths. Configuration failures
    /// (unknown options, bad values, unlocatable binaries, unknown
    /// standards) are errors, like PHP.
    pub fn from_config(
        config: &arclint::ArclintConfig,
        project_root: &Path,
    ) -> Result<Self, String> {
        let mut sections = Vec::new();
        for spec in &config.linters {
            sections.push(build_section(spec, &config.global_exclude, project_root)?);
        }
        Ok(Self {
            sections,
            comprehensive: false,
            single: false,
            cached: CachedResults::new(),
            repository_version: None,
        })
    }

    /// A comprehensive engine: every ported linter, routed per path by
    /// [`comprehensive_linters_for`]. The PEP8 section carries the
    /// comprehensive `--ignore` flags, like PHP.
    #[must_use]
    pub fn comprehensive() -> Self {
        let pairs: Vec<(&'static str, Box<dyn Linter>)> = all_linter_pairs()
            .into_iter()
            .map(|(canonical, linter)| {
                if linter.name() == "PEP8" {
                    (
                        canonical,
                        Box::new(crate::lint_pep8::Pep8Linter::comprehensive()) as Box<dyn Linter>,
                    )
                } else {
                    (canonical, linter)
                }
            })
            .collect();
        Self {
            sections: sections_from_pairs(pairs),
            comprehensive: true,
            single: false,
            cached: CachedResults::new(),
            repository_version: None,
        }
    }

    /// Keep only sections running the linter with display name `display`.
    pub fn retain_display(&mut self, display: &str) {
        self.sections
            .retain(|section| section.linter.name() == display);
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            sections: Vec::new(),
            comprehensive: false,
            single: false,
            cached: CachedResults::new(),
            repository_version: None,
        }
    }

    pub fn add_linter(&mut self, linter: Box<dyn Linter>) {
        self.sections
            .push(Section::open("ArcanistUnknownLinter", linter));
    }

    /// Inject prior results by path and cache version
    /// (`setCachedResults()`).
    pub fn set_cached_results(&mut self, cached: CachedResults) {
        self.cached = cached;
    }

    /// Set the current repository version for cache granularity
    /// checks.
    pub fn set_repository_version(&mut self, version: Option<String>) {
        self.repository_version = version;
    }

    /// Engine cache version (`crc32(implode("\n", $versions))`): the
    /// engine base (`1`) plus `Class:version` per section, in section
    /// order. PHP also mixes each linter file's md5, which has no
    /// port equivalent, so values are port-local (deterministic, but
    /// never equal to PHP's).
    #[must_use]
    pub fn cache_version(&self) -> u32 {
        let mut parts = vec!["1".to_string()];
        for section in &self.sections {
            parts.push(format!(
                "{}:{}",
                section.class,
                section.linter.cache_version()
            ));
        }
        crc32_ieee(parts.join("\n").as_bytes())
    }

    /// True when the engine runs no linters (`'No linters to run.'`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// True for [`Engine::comprehensive`].
    #[must_use]
    pub fn is_comprehensive(&self) -> bool {
        self.comprehensive
    }

    /// True for the single-linter glue engine (T1153 LINT-22).
    #[must_use]
    pub fn is_single(&self) -> bool {
        self.single
    }

    /// Resolve the effective severity for a message through the first
    /// section running `display`, mirroring `getLintMessageSeverity()`
    /// (`src/lint/linter/ArcanistLinter.php:350-360`): the exact custom
    /// map first (keyed by the lookup code: short codes for
    /// `raiseLintAt*` linters, full codes for direct-message
    /// linters), then the first matching `severity.rules` pattern,
    /// else the linter's own severity.
    #[must_use]
    pub fn severity_for(&self, display: &str, full_code: &str, default: &str) -> String {
        let Some(section) = self
            .sections
            .iter()
            .find(|section| section.linter.name() == display)
        else {
            return default.to_string();
        };
        let lookup = section.linter.severity_lookup_code(full_code);
        section.severity.severity_for(lookup, default)
    }

    /// Sections in execution order: stable by ascending
    /// [`Linter::priority`] (`msort($linters, 'getLinterPriority')`,
    /// `ArcanistLintEngine.php:165`), with build-order indexes (for
    /// cached `stopped` markers, which record build-order linter IDs).
    fn ordered_sections(&self) -> Vec<(usize, &Section)> {
        let mut sections: Vec<(usize, &Section)> = self.sections.iter().enumerate().collect();
        sections.sort_by(|a, b| {
            a.1.linter
                .priority()
                .partial_cmp(&b.1.linter.priority())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sections
    }

    /// This path's cached results at the current cache version, if any.
    fn cache_entry_for(&self, path: &str) -> Option<&CachedPathResults> {
        self.cached
            .get(path)
            .and_then(|by_version| by_version.get(&self.cache_version()))
    }

    /// Per-section prelude for [`Engine::lint_bytes`]: comprehensive
    /// allow-listing, path routing, and the cache short-circuit
    /// (`ArcanistLintEngine.php:511-526`). Restores a recorded stop
    /// and reports whether the caller should skip execution.
    fn section_preskip(
        &self,
        section: &Section,
        path: &str,
        allowed: &Option<Vec<&'static str>>,
        entry: Option<&CachedPathResults>,
        index: usize,
        stopped: &mut bool,
    ) -> bool {
        if let Some(names) = allowed
            && !names.contains(&section.linter.name())
        {
            return true;
        }
        if !section.routing.allows(path) {
            return true;
        }
        let Some(entry) = entry else {
            return false;
        };
        if !should_use_cache(
            Some(section.linter.cache_granularity()),
            entry.repository_version.as_deref(),
            self.repository_version.as_deref(),
        ) {
            return false;
        }
        if entry.stopped == Some(index) {
            *stopped = true;
        }
        true
    }

    /// Merge reusable cached messages for `path`
    /// (`ArcanistLintEngine.php:224-240`): file granularity always,
    /// directory/repository only on repository-version match.
    fn merge_cached_messages(&self, path: &str, out: &mut Vec<LintMessage>) {
        let Some(entry) = self.cache_entry_for(path) else {
            return;
        };
        let cached_repository = entry.repository_version.as_deref();
        for cached in &entry.messages {
            if should_use_cache(
                cached.granularity,
                cached_repository,
                self.repository_version.as_deref(),
            ) {
                out.push(cached.message.clone());
            }
        }
    }

    /// Apply one section's severity overrides to its messages.
    fn apply_section_severity(section: &Section, messages: &mut [LintMessage]) {
        if !section.linter.applies_custom_severities() {
            return;
        }
        for message in messages.iter_mut() {
            let lookup = section.linter.severity_lookup_code(&message.code);
            let lookup = lookup.to_string();
            message.severity = section
                .severity
                .severity_for(&lookup, &message.severity.clone());
        }
    }

    /// Text stop rules for [`Engine::lint_bytes`]: an empty file
    /// always stops the engine; DOS newlines and bad charsets stop
    /// unless custom-disabled for the section.
    fn apply_text_stops(section: &Section, messages: &[LintMessage], stopped: &mut bool) {
        if messages.iter().any(|m| m.code == CODE_EMPTY_FILE) {
            *stopped = true;
        } else {
            if messages.iter().any(|m| m.code == CODE_DOS_NEWLINE)
                && Self::text_stop_enabled(section, CODE_DOS_NEWLINE)
            {
                *stopped = true;
            }
            if messages.iter().any(|m| m.code == CODE_BAD_CHARSET)
                && Self::text_stop_enabled(section, CODE_BAD_CHARSET)
            {
                *stopped = true;
            }
        }
    }

    /// True when a text stop code fired and is not custom-disabled,
    /// mirroring `isMessageEnabled()`
    /// (`ArcanistLinter.php:374-377`): the exact custom map and
    /// `severity.rules` both apply, like `getLintMessageSeverity()`.
    fn text_stop_enabled(section: &Section, full_code: &str) -> bool {
        let lookup = section.linter.severity_lookup_code(full_code);
        section.severity.severity_for(lookup, SEVERITY_ERROR) != SEVERITY_DISABLED
    }

    /// Comprehensive allow-list for `path`: `None` when the path is
    /// excluded (`externals/`), else the permitted display names (or
    /// `None` when routing is off).
    fn bytes_allowed(&self, path: &str) -> Option<Option<Vec<&'static str>>> {
        if !self.comprehensive {
            return Some(None);
        }
        comprehensive_linters_for(path).map(Some)
    }

    /// True when a section running the linter `name` covers `path`
    /// under the comprehensive allow-list.
    fn section_runs_on(&self, name: &str, path: &str, allowed: &Option<Vec<&'static str>>) -> bool {
        let routed = self
            .sections
            .iter()
            .any(|section| section.linter.name() == name && section.routing.allows(path));
        routed && allowed.as_ref().is_none_or(|names| names.contains(&name))
    }

    /// True when generated/`@nolint` markers skip every linter on this
    /// path (PHP `shouldStopAllLinters`): only when the stopping
    /// linter (`GEN`, `NOLINT`) runs on this path.
    fn bytes_globally_skipped(
        &self,
        path: &str,
        data: &[u8],
        allowed: &Option<Vec<&'static str>>,
    ) -> bool {
        let generated =
            self.section_runs_on("GEN", path, allowed) && crate::lint_generated::is_generated(data);
        let nolint =
            self.section_runs_on("NOLINT", path, allowed) && crate::lint_nolint::is_nolint(data);
        generated || nolint
    }

    #[must_use]
    pub fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        self.lint_bytes_with_errors(path, data).0
    }

    /// [`lint_bytes`](Self::lint_bytes) plus per-section failures
    /// (T1153 LINT-4): messages come back in section-execution order
    /// (PHP keeps linter emission order until `sortAndFilterMessages()`;
    /// result grouping sorts by position), each stamped with its
    /// section's cache granularity (`ArcanistLintEngine.php:218`).
    /// Section failures are collected, never aborting the remaining
    /// sections, like `executeLintersOnChunk()`; callers fold them
    /// into the `Some linters failed:` aggregate.
    #[must_use]
    pub fn lint_bytes_with_errors(
        &self,
        path: &str,
        data: &[u8],
    ) -> (Vec<LintMessage>, Vec<String>) {
        let Some(allowed) = self.bytes_allowed(path) else {
            return (Vec::new(), Vec::new());
        };
        if self.bytes_globally_skipped(path, data, &allowed) {
            return (Vec::new(), Vec::new());
        }

        let mut out = Vec::new();
        let mut errors = Vec::new();
        let mut stopped = false;
        let entry = self.cache_entry_for(path);
        for (index, section) in self.ordered_sections() {
            if stopped {
                break;
            }
            if self.section_preskip(section, path, &allowed, entry, index, &mut stopped) {
                continue;
            }
            let mut messages = section.linter.lint_bytes(path, data);
            Self::apply_section_severity(section, &mut messages);
            let granularity = section.linter.cache_granularity();
            for message in &mut messages {
                message.granularity = Some(granularity);
            }
            stopped |= section.linter.take_halt();
            if let Some(error) = section.linter.take_error() {
                errors.push(error);
            }
            // Text stops (`lintEmptyFile`, `lintNewlines`, `lintCharset`):
            // an empty file always stops; DOS newlines and bad charsets
            // stop unless custom-disabled.
            if section.linter.name() == "TXT" {
                Self::apply_text_stops(section, &messages, &mut stopped);
            }
            out.extend(messages);
        }
        self.merge_cached_messages(path, &mut out);
        (out, errors)
    }

    /// Lint one file from disk, applying each linter's `shouldLint*()`
    /// path filters like the PHP engine (`ArcanistLinter.php:294-319`):
    /// missing files lint nowhere; directories and symlinks only reach
    /// linters opting in (the filename linter); binary files (NUL byte)
    /// only reach linters opting in (filename, chmod). Under the single
    /// engine, directories lint nowhere at all (T1153 LINT-22).
    pub fn lint_file(&self, disk: &Path, display: &str) -> Result<Vec<LintMessage>, String> {
        let (messages, errors) = self.lint_file_with_errors(disk, display);
        if errors.is_empty() {
            Ok(messages)
        } else {
            Err(errors.join("; "))
        }
    }

    /// [`lint_file`](Self::lint_file) with section failures collected
    /// instead of aborting (T1153 LINT-4/LINT-20): an unreadable file
    /// yields no messages plus one error, like PHP folding the
    /// `loadData()` failure into the aggregate while other paths still
    /// lint. Callers render the `Some linters failed:` aggregate when
    /// errors are non-empty. Every section sees the on-disk path first
    /// (`set_active_disk_path()`, T1153 LINT-26).
    #[must_use]
    pub fn lint_file_with_errors(
        &self,
        disk: &Path,
        display: &str,
    ) -> (Vec<LintMessage>, Vec<String>) {
        if self.comprehensive && comprehensive_linters_for(display).is_none() {
            return (Vec::new(), Vec::new());
        }
        let meta = match std::fs::symlink_metadata(disk) {
            Err(_) => return (Vec::new(), Vec::new()),
            Ok(meta) => meta,
        };
        for section in &self.sections {
            section.linter.set_active_disk_path(disk);
        }
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            return self
                .lint_bytes_filtered(display, b"", |linter| linter.should_lint_symbolic_links());
        }
        if file_type.is_dir() {
            if self.single {
                return (Vec::new(), Vec::new());
            }
            return self
                .lint_bytes_filtered(display, b"", |linter| linter.should_lint_directories());
        }
        let data = match std::fs::read(disk) {
            Ok(data) => data,
            Err(error) => return (Vec::new(), vec![format!("cannot read: {error}")]),
        };
        if data.contains(&0) {
            return self
                .lint_bytes_filtered(display, &data, |linter| linter.should_lint_binary_files());
        }
        self.lint_bytes_with_errors(display, &data)
    }

    /// Run the linters accepting `filter` over one file's bytes. Stops
    /// cannot trigger here: no stopping linter opts into filtered paths.
    /// Section failures are collected like [`lint_bytes`](Self::lint_bytes).
    fn lint_bytes_filtered(
        &self,
        path: &str,
        data: &[u8],
        filter: impl Fn(&dyn Linter) -> bool,
    ) -> (Vec<LintMessage>, Vec<String>) {
        let mut sections = self.ordered_sections();
        sections.retain(|(_, section)| filter(section.linter.as_ref()));
        let allowed = self
            .comprehensive
            .then(|| comprehensive_linters_for(path))
            .flatten();
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let entry = self.cache_entry_for(path);
        for (_, section) in sections {
            if let Some(names) = &allowed
                && !names.contains(&section.linter.name())
            {
                continue;
            }
            if !section.routing.allows(path) {
                continue;
            }
            // Cached results skip execution, like `lint_bytes()`.
            if let Some(entry) = entry
                && should_use_cache(
                    Some(section.linter.cache_granularity()),
                    entry.repository_version.as_deref(),
                    self.repository_version.as_deref(),
                )
            {
                continue;
            }
            let mut messages = section.linter.lint_bytes(path, data);
            Self::apply_section_severity(section, &mut messages);
            let granularity = section.linter.cache_granularity();
            for message in &mut messages {
                message.granularity = Some(granularity);
            }
            if let Some(error) = section.linter.take_error() {
                errors.push(error);
            }
            out.extend(messages);
        }
        self.merge_cached_messages(path, &mut out);
        (out, errors)
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

/// Read one key from the project `.arcconfig` only (the project layer;
/// [`config_value`] reads the full local > project > user > system
/// cascade). A missing file yields `None`; an invalid one is an error.
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

/// Read one key through the local > project > user > system cascade
/// (`getConfigFromAnySource()`), with `willReadValue()` type checks.
/// Every non-null layer value is type-checked in precedence order
/// (PHP builds the full map before picking the winner); the first
/// non-null value wins. Layer paths are explicit for testability.
fn config_value_in(
    project_root: &Path,
    local_path: Option<&Path>,
    user_path: Option<&Path>,
    system_path: Option<&Path>,
    key: &str,
) -> Result<Option<serde_json::Value>, String> {
    let project_file = project_root.join(".arcconfig");
    let mut layers: Vec<(&Path, bool)> = Vec::new();
    if let Some(path) = local_path {
        layers.push((path, false));
    }
    layers.push((&project_file, false));
    if let Some(path) = user_path {
        layers.push((path, true));
    }
    if let Some(path) = system_path {
        layers.push((path, false));
    }
    let mut winner: Option<serde_json::Value> = None;
    for (path, user_layer) in layers {
        let Some(found) = read_config_layer(path, key, user_layer)? else {
            continue;
        };
        if found.is_null() {
            continue;
        }
        check_config_type(key, &found)?;
        if winner.is_none() {
            winner = Some(found);
        }
    }
    Ok(winner)
}

/// Read one key through the configuration cascade with default user
/// and system locations from the environment (`HOME`/`APPDATA`,
/// `/etc/arcconfig`/`ProgramData`). Skips the interactive
/// `~/.arcrc` permission prompt (always reads).
fn config_value(work: &WorkCopy, key: &str) -> Result<Option<serde_json::Value>, String> {
    let local = local_config_path(work);
    config_value_in(
        &work.project_root,
        local.as_deref(),
        default_user_config_path().as_deref(),
        default_system_config_path().as_deref(),
        key,
    )
}

/// Cascade string reader: empty strings read as missing (PHP falsy);
/// non-strings read as missing for `wild` keys (PHP passes those
/// through, but no lint caller accepts them).
fn config_string(work: &WorkCopy, key: &str) -> Result<Option<String>, String> {
    match config_value(work, key)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) if text.is_empty() => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text)),
        Some(_) => Ok(None),
    }
}

/// Cascade boolean reader.
fn config_bool(work: &WorkCopy, key: &str) -> Result<Option<bool>, String> {
    match config_value(work, key)? {
        Some(serde_json::Value::Bool(value)) => Ok(Some(value)),
        _ => Ok(None),
    }
}

/// Read `key` from one layer file: missing files and non-object JSON
/// contribute nothing; invalid JSON is an error. User files read the
/// nested `config` map when present, else the top level.
fn read_config_layer(
    path: &Path,
    key: &str,
    user_layer: bool,
) -> Result<Option<serde_json::Value>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| invalid_config_text(path, user_layer))?;
    let Some(object) = parsed.as_object() else {
        return Ok(None);
    };
    if user_layer {
        match object.get("config") {
            None | Some(serde_json::Value::Null) => Ok(object.get(key).cloned()),
            Some(nested) => Ok(nested.as_object().and_then(|map| map.get(key)).cloned()),
        }
    } else {
        Ok(object.get(key).cloned())
    }
}

/// Invalid-JSON error text per layer.
fn invalid_config_text(path: &Path, user_layer: bool) -> String {
    if user_layer {
        "Your '~/.arcrc' file is not a valid JSON file.".to_string()
    } else if path.ends_with(".arcconfig") {
        format!("Unable to parse '.arcconfig' file '{}'.", path.display())
    } else {
        format!("Your '{}' file is not a valid JSON file.", path.display())
    }
}

/// `willReadValue()` type checks for the keys lint reads
/// (`lint.engine` is a string, `history.immutable` is a bool,
/// everything else is `wild`).
fn check_config_type(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let want = match key {
        "lint.engine" if !value.is_string() => "string",
        "history.immutable" if !value.is_boolean() => "boolean",
        _ => return Ok(()),
    };
    Err(format!("Type of setting '{key}' must be {want}."))
}

/// Local layer path: `<metadata>/arc/config` for the working copy's
/// VCS, following gitfile pointers (linked worktrees, submodules).
fn local_config_path(work: &WorkCopy) -> Option<PathBuf> {
    let meta = match work.vcs {
        Vcs::Git => ".git",
        Vcs::Hg => ".hg",
        Vcs::Svn => ".svn",
    };
    let dir = work.vcs_root.join(meta);
    if dir.is_dir() {
        return Some(dir.join("arc/config"));
    }
    if matches!(work.vcs, Vcs::Git)
        && dir.is_file()
        && let Ok(text) = std::fs::read_to_string(&dir)
        && let Some(target) = text.strip_prefix("gitdir:").map(str::trim)
    {
        let target = Path::new(target);
        let base = if target.is_absolute() {
            target.to_path_buf()
        } else {
            work.vcs_root.join(target)
        };
        return Some(base.join("arc/config"));
    }
    None
}

/// Default user layer path: `$HOME/.arcrc`, or `%APPDATA%/.arcrc` on
/// Windows.
fn default_user_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|dir| Path::new(&dir).join(".arcrc")) // LCOV_EXCL_LINE // Reason: windows arm; cfg!(windows) is false on ubuntu CI
    } else {
        std::env::var_os("HOME").map(|dir| Path::new(&dir).join(".arcrc"))
    }
}

/// Default system layer path: `/etc/arcconfig`, or
/// `%ProgramData%/Phabricator/Arcanist/config` on Windows.
fn default_system_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("ProgramData") // LCOV_EXCL_START // Reason: windows arm; cfg!(windows) is false on ubuntu CI
            .map(|dir| Path::new(&dir).join("Phabricator/Arcanist/config")) // LCOV_EXCL_STOP
    } else {
        Some(PathBuf::from("/etc/arcconfig"))
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
    ansi: bool,
) -> Result<String, String> {
    if !git_repository_has_two_commits(&work.vcs_root) {
        return Ok(git::GIT_MAGIC_ROOT_COMMIT.to_string());
    }
    if let Some(relative) = config_string(work, "git.default-relative-commit")? {
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
        } // LCOV_EXCL_LINE // Reason: rev-parse prints a name on success; empty output with success status is not stageable
    }
    if let Some(relative) = read_default_relative_commit(&work.vcs_root) {
        return git_merge_base_with_head(&work.vcs_root, &relative);
    }
    let relative = prompt_default_relative_commit(input, output, ansi);
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

/// Body of the "Select a Default Commit Range" prompt
/// (`ArcanistGitAPI.php:320-360`).
const DEFAULT_COMMIT_RANGE_TEXT: &str = "You're running a command which operates on a range of revisions (usually, from some revision to HEAD) but have not specified the revision that should determine the start of the range.\n\nPreviously, arc assumed you meant 'HEAD^' when you did not specify a start revision, but this behavior does not make much sense in most workflows outside of Facebook's historic git-svn workflow.\n\narc no longer assumes 'HEAD^'. You must specify a relative commit explicitly when you invoke a command (e.g., `arc diff HEAD^`, not just `arc diff`) or select a default for this working copy.\n\nIn most cases, the best default is 'origin/master'. You can also select 'HEAD^' to preserve the old behavior, or some other remote or branch. But you almost certainly want to select 'origin/master'.\n\n(Technically: the merge-base of the selected revision and HEAD is used to determine the start of the commit range.)";

/// The "Select a Default Commit Range" prompt
/// (`ArcanistGitAPI.php:320-360`). TTYs render the green header
/// (`phutil_console_format("<bg:green>** %s **</bg>\n\n", ...)`) and
/// wrap the body at 78 columns; non-TTYs keep the stripped rendering
/// byte-identical. Returns the chosen relative commit (`origin/master`
/// when the answer is blank).
fn prompt_default_relative_commit(
    input: &mut dyn BufRead,
    output: &mut dyn FnMut(&str),
    ansi: bool,
) -> String {
    if ansi {
        output(&terminal::console_format(
            "<bg:green>** %s **</bg>\n\n",
            &["Select a Default Commit Range"],
            true,
        ));
        output(&terminal::console_wrap(DEFAULT_COMMIT_RANGE_TEXT, 0, true));
    } else {
        output("** Select a Default Commit Range **\n\n");
        output(&word_wrap(DEFAULT_COMMIT_RANGE_TEXT, 0));
    }
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
    hg::auto_base_commit(&work.vcs_root, &branch, ".").map(|(base, _)| base) // LCOV_EXCL_LINE // Reason: needs a working hg binary; the gates container ships none
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
/// `cwd`. CLI order is kept verbatim, duplicates included (T1153
/// LINT-9: PHP never sorts or dedups), and `.`/`..` segments are
/// normalized lexically like `Filesystem::resolvePath()` (T1153
/// LINT-10), so `sub/../file.php` displays as `file.php`.
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
        let absolute = normalize_lexical(&absolute);
        let display = display_for(&absolute, project_root);
        out.push((absolute, display));
    }
    Ok(out)
}

/// Normalize `.`/`..` segments lexically, like PHP
/// `Filesystem::resolvePath()` (no symlink resolution, no filesystem
/// access): `..` pops the previous segment and clamps at the root, so
/// joined CLI paths reduce to the canonical absolute form PHP displays.
fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    // Fast path with zero separator churn: without `.`/`..` segments
    // there is nothing to resolve, so return the path byte-identical.
    // (A `PathBuf` rebuild would convert `/` to `\` on Windows and
    // break the `/`-anchored `^externals/` routing and displays.)
    if !path
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()), // LCOV_EXCL_LINE // Reason: dead on ubuntu; Component::Prefix only occurs in Windows paths
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            // `PathBuf::pop()` clamps at the root, like PHP.
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

/// Tracked working-copy status paths (range plus uncommitted, minus
/// untracked), mirroring `getWorkingCopyStatus()` with the default omit
/// mask (`FLAG_UNTRACKED`). `base` is the resolved commit-range base;
/// `None` skips the range (svn, or callers wanting uncommitted only).
fn status_paths(work: &WorkCopy, base: Option<&str>) -> Result<Vec<String>, String> {
    let mut merged: BTreeMap<String, u32> = BTreeMap::new();
    match work.vcs {
        Vcs::Git => merge_git_status(&work.vcs_root, base, &mut merged)?,
        Vcs::Hg => merge_hg_status(&work.vcs_root, base, &mut merged)?,
        Vcs::Svn => merge_svn_status(&work.vcs_root, &mut merged)?,
    }
    Ok(merged.keys().cloned().collect())
}

/// Merge the git commit range (when `base` is set) plus uncommitted
/// status into `merged`, minus untracked paths.
fn merge_git_status(
    root: &Path,
    base: Option<&str>,
    merged: &mut BTreeMap<String, u32>,
) -> Result<(), String> {
    if let Some(base) = base {
        for (path, mask) in git::commit_range_status(root, base)? {
            *merged.entry(path).or_default() |= mask;
        }
    }
    for (path, mask) in git::uncommitted_status(root)? {
        *merged.entry(path).or_default() |= mask;
    }
    merged.retain(|_, mask| *mask & git::FLAG_UNTRACKED == 0);
    Ok(())
}

/// Merge the Mercurial commit range (when `base` is set) plus
/// uncommitted status into `merged`, minus untracked paths.
fn merge_hg_status(
    root: &Path,
    base: Option<&str>,
    merged: &mut BTreeMap<String, u32>,
) -> Result<(), String> {
    if let Some(base) = base {
        for (path, mask) in hg::get_commit_range_status(root, base)? {
            *merged.entry(path).or_default() |= mask; // LCOV_EXCL_START // Reason: needs system hg; the gates container ships none
        } // LCOV_EXCL_STOP
    }
    for (path, mask) in hg::get_uncommitted_status(root)? {
        *merged.entry(path).or_default() |= mask; // LCOV_EXCL_START // Reason: needs system hg; the gates container ships none
    }
    merged.retain(|_, mask| *mask & hg::FLAG_UNTRACKED == 0);
    Ok(()) // LCOV_EXCL_STOP
}

/// Merge the Subversion worktree status into `merged`, minus untracked
/// paths.
fn merge_svn_status(root: &Path, merged: &mut BTreeMap<String, u32>) -> Result<(), String> {
    let status = svn::get_uncommitted_status(root, &[])?;
    // LCOV_EXCL_START // Reason: needs system svn; the gates container ships none
    for (path, mask) in status {
        if mask & svn::FLAG_UNTRACKED == 0 {
            merged.insert(path, mask);
        }
    }
    Ok(()) // LCOV_EXCL_STOP
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
            // LCOV_EXCL_START // Reason: needs system svn; the gates container ships none
            match svn::get_raw_diff_text(&work.vcs_root, path, &status, DIFF_CONTEXT_LINES)? {
                Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                None => return Ok(Some(BTreeSet::new())), // LCOV_EXCL_STOP
            }
        }
    };
    if diff.lines().any(is_binary_diff_marker) {
        return Ok(None);
    }
    Ok(Some(parse_changed_lines(&diff)))
}

/// True when a diff line marks the file binary, mirroring the
/// `ArcanistDiffParser` markers (`ArcanistDiffParser.php:733-787`,
/// T1153 LINT-17): the svn `Cannot display` notice, the
/// git/svn `Binary files ... differ` / `Files ... differ` line, the hg
/// `Binary file ... has changed` line, and `GIT binary patch` headers.
/// Lines are `rtrim()`ed and matched anchored, like PHP.
fn is_binary_diff_marker(line: &str) -> bool {
    let line = line.trim_end_matches([' ', '\t', '\n', '\r', '\0', '\x0B']);
    if line == "Cannot display: file marked as a binary type." || line == "GIT binary patch" {
        return true;
    }
    if line.starts_with("Binary file ") && line.ends_with(" has changed") {
        return true;
    }
    for prefix in ["Binary files ", "Files "] {
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some(middle) = rest.strip_suffix(" differ")
            && middle.contains(" and ")
        {
            return true;
        }
    }
    false
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
    /// Bare amend intent from `diff`'s `shouldAmend()` (T1153 DIFF-D22:
    /// `setShouldAmendChanges()` has no argv form). Prompts like a
    /// default-yes confirm once patches reach disk.
    child_amend: bool,
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
/// `:2056-2095`): `--engine`, else `lint.engine` from any config
/// source (local > project > user > system), else the
/// configuration-driven engine when `.arclint` exists at the project
/// root, else the no-engine error.
pub fn engine_for_workcopy(work: &WorkCopy, cli_engine: Option<&str>) -> Result<Engine, String> {
    let mut class = cli_engine
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    if class.is_none() {
        class = config_string(work, "lint.engine")?;
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
            Engine::from_config(&config, &work.project_root)
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
    let name = config_string(work, KEY)?;
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
    engine.single = true;
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

    /// Sort messages by position (`sortAndFilterMessages()`,
    /// `ArcanistLintResult.php:93-98`: `line * 8192 + char`, stable so
    /// ties keep linter emission order like PHP's `asort()` key order,
    /// T1153 LINT-15). Messages without a line sort first, like PHP's
    /// null key 0.
    pub fn sort_messages(&mut self) {
        let mut order: Vec<usize> = (0..self.messages.len()).collect();
        order.sort_by_key(|&i| {
            let message = &self.messages[i];
            message.line * 8192 + message.col
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

/// Group messages by path in first-seen order (T1153 LINT-9: PHP
/// renders result sections in engine path order, never sorted),
/// loading file data for context rendering. Missing/unreadable files
/// get empty data.
pub fn group_results(
    messages: Vec<LintMessage>,
    disk_for: &dyn Fn(&str) -> PathBuf,
) -> Vec<LintResult> {
    let mut paths: Vec<String> = Vec::new();
    for message in &messages {
        if !paths.contains(&message.path) {
            paths.push(message.path.clone());
        }
    }
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
/// `isRelevantMessage()` (`ArcanistLintEngine.php:316-369`, T1153
/// LINT-14).
pub fn filter_changed_lines(
    messages: Vec<LintMessage>,
    changed: &HashMap<String, Option<BTreeSet<usize>>>,
) -> Vec<LintMessage> {
    messages
        .into_iter()
        .filter(|message| is_relevant_message(message, changed))
        .collect()
}

/// True when `message` survives changed-line filtering
/// (`isRelevantMessage()`): errors, bypass-flagged messages, and an
/// empty change map are always relevant; otherwise the message's own
/// location plus every `otherLocations` entry is tried (PHP's Windows
/// path juggling omitted), where a `None` line set (binary,
/// directory, missing) or a line-less location keeps the message, and
/// a location with `original` text spans its newline count.
fn is_relevant_message(
    message: &LintMessage,
    changed: &HashMap<String, Option<BTreeSet<usize>>>,
) -> bool {
    if changed.is_empty()
        || message.is_error()
        || message.bypass_changed_line_filtering == Some(true)
    {
        return true;
    }
    let own_path = message.path.clone();
    let mut locations: Vec<(String, usize, Option<&str>)> = message
        .locations
        .iter()
        .map(|location| {
            (
                location.path.clone().unwrap_or_else(|| own_path.clone()),
                location.line,
                location.original.as_deref(),
            )
        })
        .collect();
    locations.push((own_path, message.line, message.original.as_deref()));
    for (path, line, original) in &locations {
        let Some(entry) = changed.get(path) else {
            continue;
        };
        let Some(lines) = entry else {
            return true;
        };
        if *line == 0 {
            return true;
        }
        let mut last = *line;
        if let Some(original) = original {
            last += original.bytes().filter(|b| *b == b'\n').count();
        }
        if (*line..=last).any(|line| lines.contains(&line)) {
            return true;
        }
    }
    false
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

/// Word-wrap `text` like `phutil_console_wrap()` (T1153 LINT-24):
/// content wraps at `78 - indent` characters (not 80 total), counted
/// in characters with ANSI escapes ignored; lines break after the
/// last space (the break space is dropped, interior spacing kept),
/// embedded newlines flush the line as-is, and every emitted line is
/// prefixed with `indent` spaces. No trailing newline is synthesized.
/// Mutable line state for [`word_wrap`].
#[derive(Default)]
struct WrapState {
    lines: Vec<Vec<char>>,
    line: Vec<char>,
    line_len: usize,
    break_pos: Option<usize>,
    len_after_break: usize,
}

impl WrapState {
    /// Copy one ANSI escape (`ESC ... m`) onto the line verbatim,
    /// returning the index past the terminator.
    fn push_escape(&mut self, chars: &[char], mut index: usize) -> usize {
        while index < chars.len() {
            self.line.push(chars[index]);
            let done = chars[index] == 'm';
            index += 1;
            if done {
                break;
            }
        }
        index
    }

    /// Append one display char, wrapping at the last blank when the
    /// line overflows and flushing at newlines.
    fn push_char(&mut self, chr: char, width: usize) {
        self.line.push(chr);
        self.line_len += 1;
        self.len_after_break += 1;
        if self.line_len > width
            && let Some(at) = self.break_pos
        {
            let mut slice: Vec<char> = self.line[..at].to_vec();
            while slice.last() == Some(&' ') {
                slice.pop();
            }
            slice.push('\n');
            self.lines.push(slice);
            self.line = self.line[at..].to_vec();
            self.line_len = self.len_after_break;
            self.len_after_break = 0;
            self.break_pos = None;
        }
        if chr == ' ' {
            self.break_pos = Some(self.line.len());
            self.len_after_break = 0;
        }
        if chr == '\n' {
            self.lines.push(std::mem::take(&mut self.line));
            self.len_after_break = 0;
            self.line_len = 0;
            self.break_pos = None;
        }
    }

    /// Flush the tail line and indent every line.
    fn finish(mut self, indent: usize) -> String {
        if !self.line.is_empty() {
            self.lines.push(std::mem::take(&mut self.line));
        }
        let prefix = " ".repeat(indent);
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(&prefix);
            out.extend(line.iter());
        }
        out
    }
}

pub fn word_wrap(text: &str, indent: usize) -> String {
    let width = 78usize.saturating_sub(indent);
    let chars: Vec<char> = text.chars().collect();
    let mut state = WrapState::default();
    let mut index = 0;
    while index < chars.len() {
        // ANSI escapes ride along without counting toward the width.
        if chars[index] == '\x1b' {
            index = state.push_escape(&chars, index);
            continue;
        }
        let chr = chars[index];
        index += 1;
        state.push_char(chr, width);
    }
    state.finish(indent)
}

/// Render one result in console format
/// (`ArcanistConsoleLintRenderer::renderLintResult()`, `:50-103`),
/// markup stripped (non-TTY).
pub fn render_console_result(result: &LintResult) -> Result<String, String> {
    render_console_result_with_ansi(result, terminal::ansi_enabled())
}

/// Render one result in console format with an explicit ANSI flag:
/// TTYs render the severity badge, bold `>>>`, and underlines like
/// PHP; non-TTYs keep the stripped rendering byte-identical.
pub fn render_console_result_with_ansi(result: &LintResult, ansi: bool) -> Result<String, String> {
    if ansi {
        return render_console_result_tty(result);
    }
    let mut entries = Vec::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        let description = description_with_other_locations(message, &result.path);
        entries.push(format!(
            "   {}  ({}) {}\n{}\n",
            severity,
            message.code,
            message.name,
            word_wrap(&description, 4),
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

/// A message's description plus the `Other locations: ...` suffix
/// (`ArcanistConsoleLintRenderer.php:71-81`, T1153 LINT-13): each
/// secondary location renders as `path` plus `:line` when the line is
/// set, appended after a newline and wrapped with the description.
fn description_with_other_locations(message: &LintMessage, default_path: &str) -> String {
    if message.locations.is_empty() {
        return message.description.clone();
    }
    let mut locations = Vec::new();
    for location in &message.locations {
        let path = location.path.as_deref().unwrap_or(default_path);
        if location.line == 0 {
            locations.push(path.to_string());
        } else {
            locations.push(format!("{path}:{}", location.line));
        }
    }
    format!(
        "{}\nOther locations: {}",
        message.description,
        locations.join(", ")
    )
}

/// TTY rendering for [`render_console_result_with_ansi`]:
/// `phutil_console_format("  **<bg:COLOR> %s </bg>** (%s) __%s__\n%s\n",
/// ...)` per message (`:83-88`, red for errors, yellow otherwise) plus
/// the bold `**>>>**` prefix with the underlined path (`:95-99`).
fn render_console_result_tty(result: &LintResult) -> Result<String, String> {
    let mut entries = Vec::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        let color = if message.is_error() { "red" } else { "yellow" };
        let pattern = format!("  **<bg:{color}> %s </bg>** (%s) __%s__\n%s\n");
        let description = description_with_other_locations(message, &result.path);
        let description = terminal::console_wrap(&description, 4, true);
        entries.push(terminal::console_format(
            &pattern,
            &[
                severity,
                message.code.as_str(),
                message.name.as_str(),
                description.as_str(),
            ],
            true,
        ));
        if message.has_file_context() {
            entries.push(render_context_with_ansi(message, &result.data, true));
        }
    }
    if entries.is_empty() {
        return Ok(String::new());
    }
    let path = terminal::console_format("__%s__", &[&result.path], true);
    let inner = format!("Lint for {path}:");
    let prefix = terminal::console_format("**>>>** %s\n\n\n", &[&inner], true);
    Ok(prefix + &entries.join("\n"))
}

/// Render one message's file context
/// (`ArcanistConsoleLintRenderer::renderContext()`, `:105-336`).
/// Highlights are stripped (non-TTY `##..##`); see the module docs.
/// Split like `phutil_split_lines()`: lines keep their endings; a
/// trailing newline leaves no empty piece; empty text is one empty
/// line (T1153 LINT-25).
fn split_keep_ends(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
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
    render_context_with_ansi(message, data, false)
}

/// Render one message's file context with an explicit ANSI flag.
/// TTYs additionally wrap the changed ranges in `##` invert markers
/// (`highlightText()`, `:377-383`, non-testable `tsprintf('##%s##',
/// ...)`), applied before shrinking like PHP; non-TTYs render the
/// stripped context byte-identical.
fn render_context_with_ansi(message: &LintMessage, data: &[u8], ansi: bool) -> String {
    let data_text = String::from_utf8_lossy(data).into_owned();
    let mut old_lines: Vec<String> = split_keep_ends(&data_text);

    let (line, char, original, replacement) = trimmed_patch(message);
    let old_impact = original.bytes().filter(|b| *b == b'\n').count() + 1;
    let mut start = line;

    // PHI1782: messages past the end of the file render a warning.
    if let Some(invalid) = context_past_end_message(start, old_lines.len(), &original, char) {
        return invalid;
    }

    let patchable = message.is_patchable();
    let mut new_lines: Vec<String> = Vec::new();
    let (old_impact, new_impact) = if patchable {
        let patch = ContextPatch {
            line,
            char,
            original: &original,
            replacement: replacement.as_deref(),
            old_impact,
        };
        context_patch_impacts(
            &data_text,
            &mut old_lines,
            &mut new_lines,
            &mut start,
            &patch,
            ansi,
        )
    } else {
        if ansi {
            highlight_single_line_old(&mut old_lines, start, char, &original, old_impact);
        }
        (0, 0)
    };

    let specs = context_build_specs(&old_lines, &new_lines, start, old_impact, new_impact);
    context_render_specs(&specs, patchable, original.is_empty(), char)
}

/// The TTY form of `highlightText()` (`:377-383`): the text wrapped in
/// `##` invert markers (non-testable `tsprintf('##%s##', ...)`).
fn invert_highlight(text: &str) -> String {
    terminal::tsprintf_ansi("##%s##", &[TVal::Text(text)], true)
        .expect("fixed tsprintf pattern")
        .as_str()
        .to_string()
}

/// Splice `highlight` over `len` bytes at byte offset `at` in `line`
/// (PHP `substr_replace()`, byte-based), lossy-decoding the result
/// like the surrounding rendering.
fn splice_highlight(line: &str, at: usize, len: usize, highlight: &str) -> String {
    let bytes = line.as_bytes();
    let at = at.min(bytes.len());
    let end = at.saturating_add(len).min(bytes.len());
    let mut out = Vec::with_capacity(bytes.len() + highlight.len());
    out.extend_from_slice(&bytes[..at]);
    out.extend_from_slice(highlight.as_bytes());
    out.extend_from_slice(&bytes[end..]);
    String::from_utf8_lossy(&out).into_owned()
}

/// Splice an invert highlight over the changed range of line `start`
/// (1-based, `char` a 1-based byte offset). `create` mirrors PHP's
/// auto-vivification for the boundary line past the end of the file;
/// the new side (T13543) highlights only existing lines. Like the
/// patch offset above, an unset (`0`) char highlights at the line
/// start (PHP's `$char - 1` quirk for null chars is not replicated).
fn highlight_line_at(lines: &mut Vec<String>, start: usize, char: usize, text: &str, create: bool) {
    let highlight = invert_highlight(text);
    let offset = char.saturating_sub(1);
    if let Some(line) = lines.get_mut(start.wrapping_sub(1)) {
        let spliced = splice_highlight(line, offset, text.len(), &highlight);
        *line = spliced;
    } else if create && start == lines.len() + 1 {
        lines.push(splice_highlight("", offset, text.len(), &highlight));
    }
}

/// Patchable single-line highlights on both sides (`:176-199`),
/// before shrinking (T13543: the new side only when the line exists).
/// Runs before [`shrink_ranges`], so `patch.line` is still the range
/// start.
fn highlight_patchable_lines(
    old_lines: &mut Vec<String>,
    new_lines: &mut Vec<String>,
    patch: &ContextPatch,
    new_impact: usize,
) {
    if patch.old_impact != 1 || new_impact != 1 {
        return;
    }
    highlight_line_at(old_lines, patch.line, patch.char, patch.original, true);
    highlight_line_at(
        new_lines,
        patch.line,
        patch.char,
        patch.replacement.unwrap_or_default(),
        false,
    );
}

/// Non-patchable single-line highlight on the old side (`:232-240`).
fn highlight_single_line_old(
    old_lines: &mut Vec<String>,
    start: usize,
    char: usize,
    original: &str,
    old_impact: usize,
) {
    if old_impact != 1 || original.is_empty() {
        return;
    }
    highlight_line_at(old_lines, start, char, original, true);
}

/// PHI1782 warning when a message sits past the end of the file.
fn context_past_end_message(
    start: usize,
    max_old: usize,
    original: &str,
    char: usize,
) -> Option<String> {
    let invalid = if start > max_old + 1 {
        true
    } else if start > max_old {
        !original.is_empty() || (char != 0 && char != 1)
    } else {
        false
    };
    if !invalid {
        return None;
    }
    Some(format!(
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
    ))
}

/// Trimmed patch inputs for [`context_patch_impacts`].
struct ContextPatch<'a> {
    line: usize,
    char: usize,
    original: &'a str,
    replacement: Option<&'a str>,
    old_impact: usize,
}

/// Apply a patchable message's patch to `data_text`, fill `new_lines`,
/// and return the shrunk old/new impacts (adjusting `start`).
fn context_patch_impacts(
    data_text: &str,
    old_lines: &mut Vec<String>,
    new_lines: &mut Vec<String>,
    start: &mut usize,
    patch: &ContextPatch,
    ansi: bool,
) -> (usize, usize) {
    let line_map = offset_map(data_text);
    let patch_offset =
        line_map.get(&patch.line).copied().unwrap_or(0) + patch.char.saturating_sub(1);
    let replacement_text = patch.replacement.unwrap_or_default();
    let mut new_data = data_text.as_bytes().to_vec();
    let end = (patch_offset + patch.original.len()).min(new_data.len());
    new_data.splice(
        patch_offset.min(new_data.len())..end,
        replacement_text.bytes(),
    );
    let new_text = String::from_utf8_lossy(&new_data).into_owned();
    *new_lines = split_keep_ends(&new_text);
    let new_impact = replacement_text.bytes().filter(|b| *b == b'\n').count() + 1;
    if ansi {
        highlight_patchable_lines(old_lines, new_lines, patch, new_impact);
    }
    shrink_ranges(old_lines, new_lines, start, patch.old_impact, new_impact)
}

/// One context line to render.
struct ContextSpec {
    text: String,
    number: Option<usize>,
    diff: Option<char>,
    chevron: bool,
}

/// Context lines: head context, the `-`/`+` diff, then foot context.
fn context_build_specs(
    old_lines: &[String],
    new_lines: &[String],
    start: usize,
    old_impact: usize,
    new_impact: usize,
) -> Vec<ContextSpec> {
    const CONTEXT: usize = 3;
    let mut specs: Vec<ContextSpec> = Vec::new();
    let head = start.saturating_sub(CONTEXT).max(1);
    for number in head..start {
        specs.push(ContextSpec {
            text: old_lines.get(number - 1).cloned().unwrap_or_default(),
            number: Some(number),
            diff: None,
            chevron: false,
        });
    }
    context_push_diff_specs(
        &mut specs, old_lines, new_lines, start, old_impact, new_impact,
    );
    let cursor = start + old_impact;
    let foot = old_lines.len().min(cursor + CONTEXT);
    for number in cursor..=foot {
        if number == 0 {
            continue;
        }
        specs.push(ContextSpec {
            text: old_lines.get(number - 1).cloned().unwrap_or_default(),
            number: Some(number),
            diff: None,
            chevron: number == cursor,
        });
    }
    specs
}

/// The `-` old lines and `+` new lines of a patchable message.
fn context_push_diff_specs(
    specs: &mut Vec<ContextSpec>,
    old_lines: &[String],
    new_lines: &[String],
    start: usize,
    old_impact: usize,
    new_impact: usize,
) {
    for number in start..start + old_impact {
        specs.push(ContextSpec {
            text: old_lines.get(number - 1).cloned().unwrap_or_default(),
            number: Some(number),
            diff: Some('-'),
            chevron: number == start,
        });
    }
    for number in start..start + new_impact {
        specs.push(ContextSpec {
            text: new_lines.get(number - 1).cloned().unwrap_or_default(),
            number: None,
            diff: Some('+'),
            chevron: number == start,
        });
    }
}

/// Render context specs, marking the first chevron line (plus a caret
/// for non-patchable messages without original text).
fn context_render_specs(
    specs: &[ContextSpec],
    patchable: bool,
    original_empty: bool,
    char: usize,
) -> String {
    let mut out = String::new();
    let mut seen_chevron = false;
    for spec in specs {
        let chevron = context_take_chevron(spec.chevron, &mut seen_chevron);
        let mut text = spec.text.clone();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        out.push_str(&render_line(spec.number, &text, chevron, spec.diff));
        if chevron && !patchable && original_empty {
            out.push_str(&render_caret(char));
            out.push('\n');
        }
    }
    out
}

/// Take a spec's chevron marker unless one was already shown.
fn context_take_chevron(chevron: bool, seen_chevron: &mut bool) -> bool {
    if *seen_chevron {
        return false;
    }
    if chevron {
        *seen_chevron = true;
    }
    chevron
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
/// success, else nothing (`PhutilConsoleLogLine::drawView()`,
/// `tsprintf("<bg:green>** %s **</bg> %s\n", ...)` through the shared
/// formatter, so non-TTYs render the stripped ` OKAY  No lint
/// messages.` with one leading space, T1153 LINT-5).
fn render_result_code_okay(result_code: i32, ansi: bool) -> String {
    if result_code != RESULT_OKAY {
        return String::new();
    }
    terminal::tsprintf_ansi(
        "<bg:green>** %s **</bg> %s\n",
        &[TVal::Text("OKAY"), TVal::Text("No lint messages.")],
        ansi,
    )
    .expect("fixed tsprintf pattern")
    .as_str()
    .to_string()
}

/// Render one result in summary format
/// (`ArcanistSummaryLintRenderer.php:7-22`).
pub fn render_summary_result(result: &LintResult) -> Result<String, String> {
    render_summary_result_with_ansi(result, terminal::ansi_enabled())
}

/// Render one result in summary format with an explicit ANSI flag.
/// The format carries no markup, so both modes render byte-identical
/// text through the console formatter.
pub fn render_summary_result_with_ansi(result: &LintResult, ansi: bool) -> Result<String, String> {
    let mut out = String::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        let line = if message.line == 0 {
            String::new()
        } else {
            message.line.to_string()
        };
        out.push_str(&terminal::console_format(
            "%s:%s:%s: %s\n",
            &[&result.path, &line, severity, &message.name],
            ansi,
        ));
    }
    Ok(out)
}

/// Render one result in compiler format
/// (`ArcanistCompilerLintRenderer.php:7-28`).
pub fn render_compiler_result(result: &LintResult) -> Result<String, String> {
    render_compiler_result_with_ansi(result, terminal::ansi_enabled())
}

/// Render one result in compiler format with an explicit ANSI flag.
/// The format carries no markup, so both modes render byte-identical
/// text through the console formatter.
pub fn render_compiler_result_with_ansi(result: &LintResult, ansi: bool) -> Result<String, String> {
    let mut out = String::new();
    for message in &result.messages {
        let severity = severity_display_name(&message.severity)?;
        let line = message.line.to_string();
        out.push_str(&terminal::console_format(
            "%s:%s:%s (%s) %s\n",
            &[
                &result.path,
                &line,
                severity,
                &message.code,
                &message.description,
            ],
            ansi,
        ));
    }
    Ok(out)
}

/// One message as a JSON-renderer dictionary: `toDictionary()` plus
/// `context`, minus `path` (T1153 DIFF-D22: built from the shared
/// dict so both agree). Key order matches PHP's insertion order
/// (`line` through `bypassChangedLineFiltering`, then `context`).
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
    let mut dict = message.to_dictionary();
    if let Some(map) = dict.as_object_mut() {
        map.shift_remove("path");
        map.insert(
            "context".to_string(),
            serde_json::Value::String(context.join("\n")),
        );
    }
    dict
}

/// One `locations` entry as JSON: the `getOtherLocation()` dict shape
/// (`path`/`line`/`char`), plus `original` when set.
fn location_json_dict(location: &OtherLocation) -> serde_json::Value {
    let mut dict = serde_json::Map::new();
    dict.insert(
        "path".to_string(),
        location
            .path
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    dict.insert("line".to_string(), serde_json::json!(location.line));
    dict.insert("char".to_string(), serde_json::json!(location.char));
    if let Some(original) = &location.original {
        dict.insert(
            "original".to_string(),
            serde_json::Value::String(original.clone()),
        );
    }
    serde_json::Value::Object(dict)
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
    // PHP `json_encode()` (default flags) escapes `/` as `\/` and every
    // non-ASCII char as `\uXXXX`.
    let rendered = serde_json::to_string(&map)
        .unwrap_or_else(|_| "{}".to_string())
        .replace('/', "\\/");
    json_escape_nonascii(&rendered)
}

/// Escape non-ASCII text like PHP `json_encode()`: every char at or
/// above U+0080 becomes `\uXXXX` (surrogate pairs above the BMP).
/// DEL (0x7F) passes through raw like PHP (T1153 LINT-11).
/// Serialized JSON syntax is ASCII-only, so only string contents (and
/// raw UTF-8 in them) change; existing escapes pass through.
fn json_escape_nonascii(rendered: &str) -> String {
    if rendered.chars().all(|ch| (ch as u32) < 0x80) {
        return rendered.to_string();
    }
    let mut out = String::with_capacity(rendered.len());
    for ch in rendered.chars() {
        if (ch as u32) < 0x80 {
            out.push(ch);
        } else {
            let mut encoded = [0u16; 2];
            for unit in ch.encode_utf16(&mut encoded) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// Escape XML attribute text like `XMLWriter::writeAttribute()`
/// (T1153 LINT-12): `& < > "` plus tab/newline/CR as character
/// references (inserted after `&` so the reference `&` survives).
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\t', "&#9;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
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
/// flags. Targets up to one past the last line are allowed
/// (`getCharacterOffset()` holds `nlines + 1` entries, T1153 LINT-18);
/// anything further is an error (PHP's message names an undefined
/// variable, rendering as 0). An unset char offsets one byte before
/// the line start (PHP's null-char `- 1`, T1153 LINT-18). Byte-exact
/// spans splice raw when the message carries them (T1153 LINT-43).
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
        if message.line == 0 || message.line > offsets.len() + 1 {
            return Err("Data has fewer than 0 lines.".to_string());
        }
        // The virtual line past the end starts one byte past the data
        // (`lineOffsets[nlines]`, the trailing `strlen + 1` entry).
        let line_start = offsets
            .get(message.line - 1)
            .copied()
            .unwrap_or(data.len() + 1) as i64;
        let orig_offset = line_start + message.col as i64 - 1;
        if dirty_until as i64 > orig_offset {
            continue;
        }
        let working_offset = (orig_offset + delta).max(0) as usize;
        let old = patch_span(
            message.original_bytes.as_deref(),
            message.original.as_deref(),
        );
        let new = patch_span(
            message.replacement_bytes.as_deref(),
            message.replacement.as_deref(),
        );
        if working_offset == patched.len() {
            // Temporary hack to work around a destructive hphpi issue.
            patched.extend_from_slice(new);
        } else {
            let start = working_offset.min(patched.len());
            let end = (working_offset + old.len()).min(patched.len());
            patched.splice(start..end, new.iter().copied());
        }
        delta += new.len() as i64 - old.len() as i64;
        dirty_until = orig_offset.max(0) as usize + old.len();
        applied[index] = true;
    }
    Ok((patched, applied))
}

/// One patch span as bytes: the raw-byte span when the message carries
/// it, else the lossy text span, else empty.
fn patch_span<'a>(bytes: Option<&'a [u8]>, text: Option<&'a str>) -> &'a [u8] {
    if let Some(bytes) = bytes {
        return bytes;
    }
    text.map(str::as_bytes).unwrap_or_default()
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
    if rename_over(&temp, path).is_err() {
        // PHP names no reason (`ArcanistLintPatcher.php:52-57`, T1153
        // LINT-19).
        return Err(format!(
            "Unable to overwrite path '{}', patched version was left at '{}'.",
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

/// Ask a yes/no question (`phutil_console_confirm()`): two blank lines
/// plus the query and `[Y/n]` (or `[y/N]`) wrapped with a 4-space
/// indent, looping until `y`/`n`/empty, then a blank line. Blank
/// answers and EOF take the default (`default_yes`).
///
/// The TTY is injected: scripted sessions pass `true`, production
/// passes the real stdin state, and piped input echoes the prompt
/// once, then fails with the `phutil_console_require_tty()` error.
pub fn confirm_with_tty(
    prompt: &str,
    default_yes: bool,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<bool, String> {
    let default_no = !default_yes;
    let options = if default_no { "[y/N]" } else { "[Y/n]" };
    crate::prompt::console_confirm_with_tty(input, prompt, options, default_no, stdin_tty)
}

/// [`confirm_with_tty`] for scripted sessions (stdin reads as a TTY).
pub fn confirm(prompt: &str, default_yes: bool, input: &mut dyn BufRead) -> bool {
    confirm_with_tty(prompt, default_yes, input, true).unwrap_or(default_yes)
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
        let output = crate::proc_env::command("diff")
            .arg("-u")
            .arg(&old_path)
            .arg(new_path)
            .output();
        if let Ok(output) = output {
            diff_text = String::from_utf8_lossy(&output.stdout).into_owned();
        }
        std::fs::remove_file(new_path).ok();
    } // LCOV_EXCL_LINE // Reason: temp-file staging fails only if the temp dir is unwritable; never the case in tests
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
        // LCOV_EXCL_START // Reason: needs getcwd to fail on the test runner's cwd; not stageable
        Err(error) => {
            return conduit::report(
                EnvelopeKind::Exception,
                &format!("cannot determine current directory: {error}"), // LCOV_EXCL_STOP
            );
        }
    };
    run_in(&cwd, argv)
}

/// Entry point with an explicit working directory, so tests can run
/// without mutating the process directory. Prompts read stdin.
pub fn run_in(cwd: &Path, argv: &[String]) -> ExitCode {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    run_in_with_input_and_tty(cwd, argv, &mut input, crate::prompt::stdin_is_terminal())
}

/// Entry point with an explicit prompt-input reader. Injected input
/// models a TTY session; use [`run_in_with_input_and_tty`] to gate
/// prompts on the real stdin state.
pub fn run_in_with_input(cwd: &Path, argv: &[String], input: &mut dyn BufRead) -> ExitCode {
    run_in_with_input_and_tty(cwd, argv, input, true)
}

/// [`run_in_with_input`] with an injected TTY flag: piped input
/// echoes each prompt once, then fails with the
/// `phutil_console_require_tty()` error.
pub fn run_in_with_input_and_tty(
    cwd: &Path,
    argv: &[String],
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        // T1153 R4 (LINT-2): `lint` is a subprocess workflow:
        // usage errors render `Usage Exception:` with no usage dump
        // (`scripts/arcanist.php:432-444`).
        ParseOutcome::Error(message) => return conduit::report(EnvelopeKind::Usage, &message),
        ParseOutcome::Args(options) => options,
    };
    let ansi = terminal::ansi_enabled();
    match run_workflow_classified(cwd, &options, input, ansi, stdin_tty) {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => error.report(),
    }
}

/// Classify a gather-stage error: "no paths" is `NoEffect`
/// (`ArcanistLintEngine.php:179`), everything else (base resolution,
/// git failures) is a generic exception.
fn classify_gather_error(message: String) -> ClassifiedError {
    if message == "No paths are lintable." {
        ClassifiedError::no_effect(message)
    } else {
        ClassifiedError::exception(message)
    }
}

/// Classify an emit-stage error: declining the amend is a usage error
/// (`ArcanistLintWorkflow.php:349`), everything else (bad renderer,
/// patch and IO failures) is a generic exception.
fn classify_emit_error(message: String) -> ClassifiedError {
    if message == "Sort out the lint changes that were applied to the working copy and relint." {
        ClassifiedError::usage(message)
    } else {
        ClassifiedError::exception(message)
    }
}

/// [`run_workflow`] with errors classified for the R4 envelope.
fn run_workflow_classified(
    cwd: &Path,
    options: &Options,
    input: &mut dyn BufRead,
    ansi: bool,
    stdin_tty: bool,
) -> Result<i32, ClassifiedError> {
    run_workflow_collecting(cwd, options, input, ansi, stdin_tty).map(|(code, _)| code)
}

/// [`run_workflow_classified`] that also returns the per-path results,
/// so `diff` can collect unresolved message dicts (T1153 DIFF-D22).
fn run_workflow_collecting(
    cwd: &Path,
    options: &Options,
    input: &mut dyn BufRead,
    ansi: bool,
    stdin_tty: bool,
) -> Result<(i32, Vec<LintResult>), ClassifiedError> {
    let work = discover_working_copy(cwd).map_err(ClassifiedError::usage)?;
    check_supports(options, work.vcs).map_err(ClassifiedError::usage)?;
    reject_everything_with_paths(options).map_err(ClassifiedError::usage)?;
    let engine =
        engine_for_workcopy(&work, options.engine.as_deref()).map_err(ClassifiedError::usage)?;
    if engine.is_empty() {
        // T1153 R4 (LINT-1): `ArcanistNoEffectException` prints on
        // stdout with exit 0 (`scripts/arcanist.php:428-430`).
        return Err(ClassifiedError::no_effect("No linters to run.".to_string()));
    }
    let mut results = workflow_gather(&work, cwd, options, &engine, input, ansi)
        .map_err(classify_gather_error)?;
    let code = workflow_emit(&work, options, input, &mut results, ansi, stdin_tty)
        .map_err(classify_emit_error)?;
    Ok((code, results))
}

/// Unresolved message dicts: messages whose patches were not applied
/// (T1153 DIFF-D22: `ArcanistLintWorkflow.php:360-375` feeds
/// `getUnresolvedMessages()` into `diff`'s `unresolvedLint`).
#[must_use]
pub fn unresolved_dicts(results: &[LintResult]) -> Vec<serde_json::Value> {
    let mut dicts = Vec::new();
    for result in results {
        for (message, applied) in result.messages.iter().zip(result.applied.iter()) {
            if !applied {
                dicts.push(message.to_dictionary());
            }
        }
    }
    dicts
}

/// Report of a collecting child lint run (T1153 DIFF-D22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildLintReport {
    /// Workflow result code.
    pub code: i32,
    /// Unresolved message dicts (`unresolvedLint`).
    pub unresolved: Vec<serde_json::Value>,
}

/// Run lint as `diff`'s child, collecting unresolved message dicts
/// (T1153 DIFF-D22: `runLint()`,
/// `src/workflow/ArcanistDiffWorkflow.php:1146-1207`). `amend` is
/// diff's `shouldAmend()` (bare `setShouldAmendChanges()`, no argv
/// form). Renders and prompts like the standalone run; errors return
/// unreported for the caller to map (`NoEffect` prints with exit 0,
/// everything else aborts the diff, like PHP's child catches).
pub fn run_child_collecting(
    cwd: &Path,
    argv: &[String],
    amend: bool,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<ChildLintReport, ClassifiedError> {
    let mut options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return Err(ClassifiedError::usage(
                "unexpected --help in child lint argv".to_string(),
            ));
        }
        // T1153 R4 (LINT-2): usage errors abort the diff unreported
        // here (the caller renders them); the standalone entry reports
        // `Usage Exception:` with no usage dump instead.
        ParseOutcome::Error(message) => return Err(ClassifiedError::usage(message)),
        ParseOutcome::Args(options) => options,
    };
    options.child_amend = amend;
    let ansi = terminal::ansi_enabled();
    let (code, results) = run_workflow_collecting(cwd, &options, input, ansi, stdin_tty)?;
    Ok(ChildLintReport {
        code,
        unresolved: unresolved_dicts(&results),
    })
}

/// String-error entry kept for API stability; production uses
/// [`run_workflow_classified`].
#[allow(dead_code)]
fn run_workflow(
    cwd: &Path,
    options: &Options,
    input: &mut dyn BufRead,
    ansi: bool,
    stdin_tty: bool,
) -> Result<i32, String> {
    run_workflow_classified(cwd, options, input, ansi, stdin_tty).map_err(|error| error.message)
}

/// Reject `--everything` combined with explicit paths.
fn reject_everything_with_paths(options: &Options) -> Result<(), String> {
    if options.everything && !options.paths.is_empty() {
        return Err("You can not specify paths with --everything. The --everything flag lints every tracked file in the working copy."
            .to_string());
    }
    Ok(())
}

/// Lint the selection and group the filtered messages by path.
fn workflow_gather(
    work: &WorkCopy,
    cwd: &Path,
    options: &Options,
    engine: &Engine,
    input: &mut dyn BufRead,
    ansi: bool,
) -> Result<Vec<LintResult>, String> {
    let base = workflow_base(work, options, input, ansi)?;
    let selection = workflow_selection(work, cwd, options, base.as_deref(), engine)?;
    let messages = workflow_lint_selection(engine, work, &selection)?;
    let minimum = options.severity.as_deref().unwrap_or(DEFAULT_SEVERITY);
    let mut messages = filter_by_severity(messages, minimum);
    if !lint_all(options) {
        messages = workflow_filter_changed(work, base.as_deref(), &selection, messages)?;
    }
    Ok(workflow_group_results(work, messages))
}

/// Resolve the commit-range base: explicit `--rev`, else the default
/// base when linting the whole selection (`parseBaseCommitArgument()`
/// plus `buildBaseCommit()`; svn has no ranges).
fn workflow_base(
    work: &WorkCopy,
    options: &Options,
    input: &mut dyn BufRead,
    ansi: bool,
) -> Result<Option<String>, String> {
    let needs_base = options.rev.is_some() || (options.paths.is_empty() && !options.everything);
    if !needs_base || work.vcs == Vcs::Svn {
        return Ok(None);
    }
    if let Some(rev) = &options.rev {
        return workflow_explicit_base(work, rev).map(Some);
    }
    workflow_default_base(work, input, ansi)
}

/// Base for an explicit `--rev` symbol.
fn workflow_explicit_base(work: &WorkCopy, rev: &str) -> Result<String, String> {
    match work.vcs {
        Vcs::Git => git_base_for_rev(work, rev),
        Vcs::Hg => hg_base_for(work, Some(rev)),
        Vcs::Svn => unreachable!("svn --rev rejected by check_supports"),
    }
}

/// Default base commit (svn has no ranges).
fn workflow_default_base(
    work: &WorkCopy,
    input: &mut dyn BufRead,
    ansi: bool,
) -> Result<Option<String>, String> {
    match work.vcs {
        Vcs::Git => git_default_base(work, input, &mut |text| print!("{text}"), ansi).map(Some),
        Vcs::Hg => hg_base_for(work, None).map(Some),
        Vcs::Svn => Ok(None),
    }
}

/// Resolve lint targets: `--everything`, explicit paths, or worktree
/// status. Errors when nothing is lintable.
fn workflow_selection(
    work: &WorkCopy,
    cwd: &Path,
    options: &Options,
    base: Option<&str>,
    engine: &Engine,
) -> Result<Vec<String>, String> {
    let selection: Vec<String> = if options.everything {
        let mut paths = everything_paths(work)?;
        paths.sort();
        paths
    } else if !options.paths.is_empty() {
        collect_explicit(&options.paths, cwd, &work.project_root)?
            .into_iter()
            .map(|(_, display)| display)
            .collect()
    } else {
        status_paths(work, base)?
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
    Ok(selection)
}

/// Lint every selected path, collecting messages. Per-path and
/// per-section failures never abort the run: every path still lints
/// (T1153 LINT-4/LINT-20), and any failure becomes the
/// `PhutilAggregateException('Some linters failed:')` aggregate
/// (`ArcanistLintEngine.php:263-267`), which the renderer rethrows
/// (`ArcanistLintRenderer.php:45-47`) so nothing renders.
fn workflow_lint_selection(
    engine: &Engine,
    work: &WorkCopy,
    selection: &[String],
) -> Result<Vec<LintMessage>, String> {
    let mut messages = Vec::new();
    let mut failed = false;
    for path in selection {
        let disk = work.vcs_root.join(path);
        let (mut found, errors) = engine.lint_file_with_errors(&disk, path);
        messages.append(&mut found);
        if !errors.is_empty() {
            failed = true;
        }
    }
    if failed {
        return Err("Some linters failed:".to_string());
    }
    Ok(messages)
}

/// Filter messages to changed lines (the base is always resolved when
/// filtering under git/hg; svn diffs the worktree).
fn workflow_filter_changed(
    work: &WorkCopy,
    base: Option<&str>,
    selection: &[String],
    messages: Vec<LintMessage>,
) -> Result<Vec<LintMessage>, String> {
    let mut changed: HashMap<String, Option<BTreeSet<usize>>> = HashMap::new();
    for path in selection {
        let lines = changed_lines_for(work, base.unwrap_or("HEAD"), path)?;
        changed.insert(path.clone(), lines);
    }
    Ok(filter_changed_lines(messages, &changed))
}

/// Group messages by path, loading file data for context rendering.
fn workflow_group_results(work: &WorkCopy, messages: Vec<LintMessage>) -> Vec<LintResult> {
    group_results(messages, &|path: &str| work.vcs_root.join(path))
}

/// Validate the `--output` renderer key.
fn workflow_renderer(options: &Options) -> Result<&str, String> {
    let renderer_key = options.output.as_deref().unwrap_or(RENDERER_CONSOLE);
    if !all_renderers().contains(&renderer_key) {
        return Err(format!(
            "Lint renderer \"{}\" is unknown. Supported renderers are: {}.",
            renderer_key,
            all_renderers().join(", ")
        ));
    }
    Ok(renderer_key)
}

/// Render results, apply patches, flush `--outfile`, maybe amend, and
/// compute the result code.
fn workflow_emit(
    work: &WorkCopy,
    options: &Options,
    input: &mut dyn BufRead,
    results: &mut [LintResult],
    ansi: bool,
    stdin_tty: bool,
) -> Result<i32, String> {
    let renderer_key = workflow_renderer(options)?;
    let mut sink = if options.outfile.is_some() {
        Sink::Buffer(String::new())
    } else {
        Sink::Stdout
    };
    let (wrote_to_disk, all_autofix) = workflow_render(
        results,
        renderer_key,
        options,
        input,
        &mut sink,
        ansi,
        stdin_tty,
    )?;
    // `--outfile` lands via temp file plus rename (PHP `TempFile`
    // plus `Filesystem::rename()`), before the amend prompt.
    if let Sink::Buffer(buffer) = &sink {
        flush_outfile(options.outfile.as_deref().unwrap_or_default(), buffer)?;
    }
    maybe_amend(
        work,
        options,
        wrote_to_disk,
        all_autofix,
        input,
        ansi,
        stdin_tty,
    )?;
    let code = result_code(results);
    workflow_render_okay(renderer_key, options.outfile.as_deref(), code, ansi)?;
    Ok(code)
}

/// Render every result and apply console patches. Returns
/// `(wrote_to_disk, all_autofix)`.
fn workflow_render(
    results: &mut [LintResult],
    renderer_key: &str,
    options: &Options,
    input: &mut dyn BufRead,
    sink: &mut Sink,
    ansi: bool,
    stdin_tty: bool,
) -> Result<(bool, bool), String> {
    if renderer_key == RENDERER_XML {
        sink.write(&render_xml_header());
    }
    let apply_patches = !options.never_apply_patches;
    let prompt_patches = !options.apply_patches;
    let should_patch = apply_patches && renderer_key == RENDERER_CONSOLE;
    let mut wrote_to_disk = false;
    let mut all_autofix = true;
    for result in results.iter_mut() {
        if result.messages.is_empty() {
            continue;
        }
        if !result.is_all_autofix() {
            all_autofix = false;
        }
        render_workflow_result(result, renderer_key, sink, ansi)?;
        if should_patch && result.is_patchable() {
            wrote_to_disk |=
                apply_result_patch(result, prompt_patches, sink, input, ansi, stdin_tty)?;
        }
    }
    if renderer_key == RENDERER_XML {
        sink.write(&render_xml_footer());
    }
    Ok((wrote_to_disk, all_autofix))
}

/// Render one result through the selected renderer.
fn render_workflow_result(
    result: &LintResult,
    renderer_key: &str,
    sink: &mut Sink,
    ansi: bool,
) -> Result<(), String> {
    match renderer_key {
        RENDERER_CONSOLE => sink.write(&render_console_result_with_ansi(result, ansi)?),
        RENDERER_JSON => sink.write(&format!("{}\n", render_json_result(result))),
        RENDERER_SUMMARY => sink.write(&render_summary_result_with_ansi(result, ansi)?),
        RENDERER_COMPILER => sink.write(&render_compiler_result_with_ansi(result, ansi)?),
        RENDERER_XML => sink.write(&render_xml_result(result)),
        RENDERER_NONE => {}
        _ => unreachable!("renderer validated above"),
    }
    Ok(())
}

/// Apply one result's patch (prompting unless autofix-only or
/// `--apply-patches`). Returns true when patches reached disk.
/// The patch-apply prompt (`promptForPatch()`, `:31-48`): the path
/// is `__underlined__` on TTYs (`tsprintf('__%s__', ...)`), else
/// plain.
fn patch_prompt_with_ansi(path: &str, ansi: bool) -> String {
    if ansi {
        let underlined = terminal::tsprintf_ansi("__%s__", &[TVal::Text(path)], true)
            .expect("fixed tsprintf pattern");
        format!("Apply this patch to {}?", underlined.as_str())
    } else {
        format!("Apply this patch to {path}?")
    }
}

fn apply_result_patch(
    result: &mut LintResult,
    prompt_patches: bool,
    sink: &mut Sink,
    input: &mut dyn BufRead,
    ansi: bool,
    stdin_tty: bool,
) -> Result<bool, String> {
    let (patched, _) = build_patched_data(&result.data, &result.messages)?;
    let result_all_autofix = result.is_all_autofix();
    let mut apply = true;
    if prompt_patches && !result_all_autofix {
        prompt_for_patch(result, &patched, sink);
        apply = confirm_with_tty(
            &patch_prompt_with_ansi(&result.path, ansi),
            true,
            input,
            stdin_tty,
        )?;
    }
    if !apply {
        return Ok(false);
    }
    let applied = write_patch_to_disk(result)?;
    for (slot, flag) in result.applied.iter_mut().zip(applied.iter()) {
        if *flag {
            *slot = true;
        }
    }
    Ok(true)
}

/// Render the trailing `OKAY` line for console/summary renderers.
/// With `--outfile`, PHP renders the code after renaming the temp file
/// away, so the line lands in a deleted temp file and is lost
/// (`ArcanistLintWorkflow.php:323-326,384`: T1153 LINT-7); without an
/// outfile it prints to stdout.
fn workflow_render_okay(
    renderer_key: &str,
    outfile: Option<&str>,
    code: i32,
    ansi: bool,
) -> Result<(), String> {
    if !matches!(renderer_key, RENDERER_CONSOLE | RENDERER_SUMMARY) {
        return Ok(());
    }
    if outfile.is_some() {
        return Ok(());
    }
    print!("{}", render_result_code_okay(code, ansi));
    Ok(())
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

/// Amend HEAD with applied patches
/// (`ArcanistLintWorkflow.php:238-354`).
/// The `LINT NOTICE` amend line (`ArcanistLintWorkflow.php`, the
/// `writeOut("<bg:yellow>** %s **</bg> %s\n", ...)` call) through the
/// shared formatter, so non-TTYs render the stripped ` LINT NOTICE  ...`
/// line with the markup removed (T1153 LINT-6). Includes the trailing
/// newline.
fn lint_notice_line(ansi: bool) -> String {
    terminal::format_status_message(
        "yellow",
        "LINT NOTICE",
        "Automatically amending HEAD with lint patches.",
        ansi,
    )
}

fn maybe_amend(
    work: &WorkCopy,
    options: &Options,
    wrote_to_disk: bool,
    all_autofix: bool,
    input: &mut dyn BufRead,
    ansi: bool,
    stdin_tty: bool,
) -> Result<(), String> {
    if !wrote_to_disk || !amend_enabled(work, options)? {
        return Ok(());
    }
    let automatic = options.amend_all || (options.amend_autofixes && all_autofix);
    let amend = if automatic {
        print!("{}", lint_notice_line(ansi));
        true
    } else {
        confirm_with_tty("Amend HEAD with lint patches?", true, input, stdin_tty)?
    };
    if !amend {
        return Err(
            "Sort out the lint changes that were applied to the working copy and relint."
                .to_string(),
        );
    }
    amend_head(work)
}

/// True when the amend flags request an amend the VCS allows: the VCS
/// must support amend and history must be mutable. A bare child amend
/// (T1153 DIFF-D22) gates the same way (`:248-252`).
fn amend_enabled(work: &WorkCopy, options: &Options) -> Result<bool, String> {
    if !(options.amend_all || options.amend_autofixes || options.child_amend) {
        return Ok(false);
    }
    let immutable = config_bool(work, "history.immutable")?
        .unwrap_or_else(|| work.vcs.is_history_default_immutable());
    Ok(work.vcs.supports_amend(&work.vcs_root) && !immutable)
}

/// Amend HEAD with the applied patches.
fn amend_head(work: &WorkCopy) -> Result<(), String> {
    match work.vcs {
        Vcs::Git => amend_git_head(&work.vcs_root),
        Vcs::Hg => hg::amend_commit(&work.vcs_root, None, None),
        Vcs::Svn => Ok(()),
    }
}

/// Stage the worktree (`git add -u`) and amend HEAD.
fn amend_git_head(root: &Path) -> Result<(), String> {
    let out = git::output(root, &["add", "-u"])?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err("git add -u failed".to_string()); // LCOV_EXCL_LINE // Reason: git failures explain on stderr; empty stderr needs a silent kill, not stageable
        }
        return Err(format!("git add -u failed: {stderr}"));
    }
    git::amend_commit(root, None)
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod lint_tests;
