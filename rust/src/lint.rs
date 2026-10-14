//! `lint` workflow: run ported linters (T1128).
//!
//! PHP references: `src/lint/linter/ArcanistTextLinter.php` (text rules and
//! messages), `src/lint/ArcanistLintSeverity.php` (severities),
//! `src/workflow/ArcanistLintWorkflow.php` (CLI flags, including
//! `--severity`), `src/workflow/ArcanistWorkflow.php` (`newLintEngine()`),
//! and `src/lint/engine/ArcanistConfigurationDrivenLintEngine.php`
//! (`.arclint` selection).
//!
//! Scope: a small [`Linter`] trait plus an [`Engine`], with [`TextLinter`]
//! covering trailing whitespace, tab literals, and a missing newline at
//! end of file. Severities mirror the PHP linter: trailing whitespace is
//! `autofix`, tab literals and a missing EOF newline are `error`.
//!
//! Run wiring: [`discover_arclint`] finds the nearest `.arclint` at or
//! above the working directory; when one exists the engine runs only the
//! configured linters ([`Engine::from_config`], exact PHP configuration
//! names via [`crate::arclint`]). Without a config every ported linter
//! runs, exactly as before. `--severity` filters messages by minimum
//! severity, defaulting to `advice` like the PHP workflow.
//!
//! Out of scope (rejected with a not-yet-ported message): other engines
//! (`--engine`), autofix application (`--apply-patches`, `--amend-all`,
//! `--amend-autofixes`), and linting committed ranges (`--rev`).
//! Per-linter `.arclint` keys (include/exclude routing, `severity` maps,
//! per-tool binary paths) parse but are not applied yet: routing and
//! `severity.rules` need a regex engine the dependency set lacks.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub const NAME: &str = "lint";

pub const SEVERITY_ERROR: &str = "error";
pub const SEVERITY_WARNING: &str = "warning";
pub const SEVERITY_AUTOFIX: &str = "autofix";
pub const SEVERITY_ADVICE: &str = "advice";
pub const SEVERITY_DISABLED: &str = "disabled";

/// Default minimum severity, mirroring
/// `ArcanistLintWorkflow::DEFAULT_SEVERITY`
/// (`src/workflow/ArcanistLintWorkflow.php:13`).
pub const DEFAULT_SEVERITY: &str = SEVERITY_ADVICE;

const CODE_TAB: &str = "TXT2";
const CODE_EOF_NEWLINE: &str = "TXT4";
const CODE_TRAILING: &str = "TXT6";

const NAME_TAB: &str = "Tab Literal";
const NAME_EOF_NEWLINE: &str = "File Does Not End in Newline";
const NAME_TRAILING: &str = "Trailing Whitespace";

const DESC_TAB: &str = "Configure your editor to use spaces for indentation.";
const DESC_EOF_NEWLINE: &str = "Files must end in a newline.";
const DESC_TRAILING: &str = "This line contains trailing whitespace. Consider setting up your editor to automatically remove trailing whitespace.";

fn usage() -> String {
    "usage: arc-rs lint [PATHS...] [--json] [--severity LEVEL]\n\
     \n\
     Run text lint rules (trailing whitespace, tab literals, missing EOF\n\
     newline).\n\
     \n\
     If no PATHS are given, lint the tracked files changed in the working\n\
     tree versus HEAD (staged plus unstaged). Explicit paths must exist;\n\
     directories are walked recursively.\n\
     \n\
     When an .arclint file exists at or above the working directory, only\n\
     its configured linters run; otherwise every ported linter runs.\n\
     \n\
     Output is one line per message as path:line:col: severity: message,\n\
     or a JSON array of message objects with --json. Exits non-zero when\n\
     any error-severity message is found.\n\
     \n\
     Options:\n\
     \x20 --json              Emit a JSON array of message objects.\n\
     \x20 --severity LEVEL    Only show messages at or above LEVEL: advice,\n\
     \x20                     autofix, warning, error, disabled. Defaults\n\
     \x20                     to 'advice'.\n\
     \x20 -h, --help          Show this help.\n\
     \n\
     Not yet ported (see T1128): other linters (--engine), autofix\n\
     (--apply-patches, --amend-all, --amend-autofixes), linting committed\n\
     ranges (--rev), and --everything/--lintall/--output/--outfile/\n\
     --never-apply-patches.\n"
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

/// A single lint finding, mirroring `ArcanistLintMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintMessage {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub code: String,
    pub severity: String,
    pub name: String,
    pub description: String,
}

impl LintMessage {
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
pub trait Linter {
    fn name(&self) -> &'static str;
    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage>;
}

/// Basic text linter: trailing whitespace, tabs, missing EOF newline.
#[derive(Debug, Default)]
pub struct TextLinter;

impl TextLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Linter for TextLinter {
    fn name(&self) -> &'static str {
        "TXT"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if data.is_empty() {
            // Mirror the PHP linter: an empty file needs no trailing newline.
            return Vec::new();
        }
        let mut out = Vec::new();
        for (offset, _len) in trailing_spans(data) {
            let (line, col) = line_col_for_offset(data, offset);
            out.push(LintMessage {
                path: path.to_string(),
                line,
                col,
                code: CODE_TRAILING.to_string(),
                severity: SEVERITY_AUTOFIX.to_string(),
                name: NAME_TRAILING.to_string(),
                description: DESC_TRAILING.to_string(),
            });
        }
        // PHP reports only the first tab per file; we report every tab so no
        // occurrence is hidden.
        for (offset, &byte) in data.iter().enumerate() {
            if byte == b'\t' {
                let (line, col) = line_col_for_offset(data, offset);
                out.push(LintMessage {
                    path: path.to_string(),
                    line,
                    col,
                    code: CODE_TAB.to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    name: NAME_TAB.to_string(),
                    description: DESC_TAB.to_string(),
                });
            }
        }
        if data.last() != Some(&b'\n') {
            let (line, col) = line_col_for_offset(data, data.len());
            out.push(LintMessage {
                path: path.to_string(),
                line,
                col,
                code: CODE_EOF_NEWLINE.to_string(),
                severity: SEVERITY_ERROR.to_string(),
                name: NAME_EOF_NEWLINE.to_string(),
                description: DESC_EOF_NEWLINE.to_string(),
            });
        }
        out.sort_by(|a, b| (a.line, a.col, &a.code).cmp(&(b.line, b.col, &b.code)));
        out
    }
}

/// Runs a set of linters over files.
pub struct Engine {
    linters: Vec<Box<dyn Linter>>,
}

/// Every ported linter, in registration order. `lint_external` stays out:
/// the PHP base class is abstract and defines no linter of its own.
#[must_use]
pub fn all_linters() -> Vec<Box<dyn Linter>> {
    vec![
        Box::new(TextLinter::new()),
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

/// Find the nearest `.arclint` file at or above `start`. PHP looks only
/// at the project root (`ArcanistWorkflow.php:2065` `newLintEngine()` via
/// `ArcanistWorkingCopyIdentity::getProjectPath()`); the upward walk
/// generalizes that so nested working directories resolve their project
/// config. A file `start` resolves from its parent directory.
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

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            linters: all_linters(),
        }
    }

    /// An engine running only the linters `config` selects, matched by
    /// exact PHP display name against [`all_linters`].
    #[must_use]
    pub fn from_config(config: &crate::arclint::ArclintConfig) -> Self {
        let mut linters = all_linters();
        crate::arclint::retain_selected(&mut linters, |linter| linter.name(), config);
        Self { linters }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            linters: Vec::new(),
        }
    }

    pub fn add_linter(&mut self, linter: Box<dyn Linter>) {
        self.linters.push(linter);
    }

    #[must_use]
    pub fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        // PHP `shouldStopAllLinters`: generated and `@nolint` files skip
        // every linter, but only when the stopping linter (`GEN`,
        // `NOLINT`) is part of the engine.
        let generated = self.linters.iter().any(|linter| linter.name() == "GEN")
            && crate::lint_generated::is_generated(data);
        let nolint = self.linters.iter().any(|linter| linter.name() == "NOLINT")
            && crate::lint_nolint::is_nolint(data);
        if generated || nolint {
            return Vec::new();
        }
        let mut out = Vec::new();
        for linter in &self.linters {
            out.extend(linter.lint_bytes(path, data));
        }
        out.sort_by(|a, b| {
            (&a.path, a.line, a.col, &a.code).cmp(&(&b.path, b.line, b.col, &b.code))
        });
        out
    }

    /// Lint one file from disk, skipping directories, symlinks, and binary
    /// files (NUL byte) like the PHP engine does.
    pub fn lint_file(&self, disk: &Path, display: &str) -> Result<Vec<LintMessage>, String> {
        let meta = std::fs::symlink_metadata(disk).map_err(|e| format!("cannot stat: {e}"))?;
        if meta.file_type().is_symlink() || meta.file_type().is_dir() {
            return Ok(Vec::new());
        }
        let data = std::fs::read(disk).map_err(|e| format!("cannot read: {e}"))?;
        if data.contains(&0) {
            return Ok(Vec::new());
        }
        Ok(self.lint_bytes(display, &data))
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
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

/// Byte spans of trailing `[ \t]+` runs, one per affected line.
fn trailing_spans(data: &[u8]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, &byte) in data.iter().enumerate() {
        if byte == b'\n' {
            let mut end = index;
            if end > start && data[end - 1] == b'\r' {
                end -= 1;
            }
            if let Some(span) = trailing_in_line(data, start, end) {
                spans.push(span);
            }
            start = index + 1;
        }
    }
    if start < data.len() {
        // Last line without a newline: no CR stripping (a trailing CR here is
        // content, matching the PHP `[[:blank:]]+$` behavior).
        if let Some(span) = trailing_in_line(data, start, data.len()) {
            spans.push(span);
        }
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

/// Resolve the engine for a working directory: the nearest `.arclint`
/// at or above `dir` selects the linters, or every ported linter runs
/// when no config is found. Returns the engine plus the config path used,
/// if any. An unreadable or invalid config is an error, mirroring the PHP
/// engine which throws (`ArcanistConfigurationDrivenLintEngine.php:9-30`).
pub fn engine_for_dir(dir: &Path) -> Result<(Engine, Option<PathBuf>), String> {
    let Some(config_path) = discover_arclint(dir) else {
        return Ok((Engine::new(), None));
    };
    let root = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.to_path_buf());
    let config = crate::arclint::load_arclint(&root)?;
    Ok((Engine::from_config(&config), Some(config_path)))
}

#[derive(Debug, PartialEq, Eq)]
struct LintArgs {
    paths: Vec<String>,
    json: bool,
    severity: String,
}

enum ParseOutcome {
    Args(LintArgs),
    Help,
    NotYetPorted(String),
    Error(String),
}

fn flag_name(arg: &str) -> &str {
    arg.split('=').next().unwrap_or(arg)
}

fn not_yet_ported_reason(flag: &str) -> Option<&'static str> {
    match flag {
        "--rev" => Some("linting committed ranges"),
        "--apply-patches" | "--amend-all" | "--amend-autofixes" => Some("autofix"),
        "--engine" => Some("other linters"),
        "--everything" | "--lintall" | "--output" | "--outfile" | "--never-apply-patches" => {
            Some("extended lint options")
        }
        _ => None,
    }
}

fn check_severity(value: &str) -> Result<(), String> {
    if crate::arclint::is_valid_severity(value) {
        Ok(())
    } else {
        Err(format!(
            "unknown severity '{value}' (expected one of: {})",
            crate::arclint::SEVERITIES.join(", ")
        ))
    }
}

/// Read the value of a `--severity VALUE` argument, advancing `index` past
/// it. Validates the severity like the `--severity=VALUE` form.
fn take_severity_space(argv: &[String], index: &mut usize) -> Result<String, String> {
    let Some(value) = argv.get(*index) else {
        return Err("--severity requires a value".to_string());
    };
    *index += 1;
    check_severity(value)?;
    Ok(value.clone())
}

/// Outcome for an unrecognized `-` flag: either not-yet-ported or unknown.
fn unknown_flag_outcome(arg: &str) -> ParseOutcome {
    let name = flag_name(arg);
    if let Some(reason) = not_yet_ported_reason(name) {
        return ParseOutcome::NotYetPorted(format!(
            "arc-rs {NAME}: {name} is not yet ported ({reason}; see T1128)"
        ));
    }
    ParseOutcome::Error(format!("unknown flag '{arg}'"))
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut paths = Vec::new();
    let mut json = false;
    let mut severity = DEFAULT_SEVERITY.to_string();
    let mut end_of_flags = false;
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        index += 1;
        if end_of_flags {
            paths.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg == "-" {
            paths.push(arg.clone());
        } else if arg == "--severity" {
            match take_severity_space(argv, &mut index) {
                Ok(value) => severity = value,
                Err(message) => return ParseOutcome::Error(message),
            }
        } else if let Some(value) = arg.strip_prefix("--severity=") {
            if let Err(message) = check_severity(value) {
                return ParseOutcome::Error(message);
            }
            severity = value.to_string();
        } else if arg.starts_with('-') {
            return unknown_flag_outcome(arg);
        } else {
            paths.push(arg.clone());
        }
    }
    ParseOutcome::Args(LintArgs {
        paths,
        json,
        severity,
    })
}

/// Display path for lint output: relative to the working directory when the
/// file is under it (like the PHP engine's repo-relative paths), otherwise
/// the path as given.
fn display_for(disk: &Path) -> String {
    if disk.is_relative() {
        return disk.to_string_lossy().to_string();
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = disk.strip_prefix(&cwd)
    {
        return rel.to_string_lossy().to_string();
    }
    disk.to_string_lossy().to_string()
}

fn collect_from_dir(dir: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<(), String> {
    let read =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list '{}': {e}", dir.display()))?;
    let mut entries = Vec::new();
    for entry in read {
        entries.push(entry.map_err(|e| format!("cannot list '{}': {e}", dir.display()))?);
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|e| format!("cannot stat '{}': {e}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            collect_from_dir(&path, out)?;
        } else {
            out.push((path.clone(), display_for(&path)));
        }
    }
    Ok(())
}

/// Expand explicit CLI paths (files plus recursive directories).
pub fn collect_explicit(paths: &[String]) -> Result<Vec<(PathBuf, String)>, String> {
    let mut out = Vec::new();
    for path in paths {
        let disk = PathBuf::from(path);
        let meta =
            std::fs::symlink_metadata(&disk).map_err(|_| format!("path does not exist: {path}"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.file_type().is_dir() {
            collect_from_dir(&disk, &mut out)?;
        } else {
            let display = display_for(&disk);
            out.push((disk, display));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

/// Lint explicit paths with `engine`, returning sorted messages or a
/// combined error.
pub fn lint_explicit_paths_with_engine(
    engine: &Engine,
    paths: &[String],
) -> Result<Vec<LintMessage>, String> {
    let files = collect_explicit(paths)?;
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

/// Lint explicit paths with every ported linter, returning sorted
/// messages or a combined error.
pub fn lint_explicit_paths(paths: &[String]) -> Result<Vec<LintMessage>, String> {
    lint_explicit_paths_with_engine(&Engine::new(), paths)
}

fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

fn parse_nul_names(output: &[u8]) -> Vec<String> {
    output
        .split(|&byte| byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect()
}

/// Tracked files changed in the working tree versus HEAD (staged plus
/// unstaged), as `(disk_path, display_path)` with git's repo-relative
/// display form. Untracked files are excluded.
pub fn changed_files_in(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let top = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Err("not a git repository (git rev-parse failed)".to_string());
    }
    let toplevel = String::from_utf8_lossy(&top.stdout).trim().to_string();
    if toplevel.is_empty() {
        return Err("not a git repository".to_string());
    }
    let root = PathBuf::from(&toplevel);

    let diff = run_git(dir, &["diff", "--name-only", "-z", "HEAD", "--"])?;
    let mut names = if diff.status.success() {
        parse_nul_names(&diff.stdout)
    } else {
        let head = run_git(dir, &["rev-parse", "--verify", "HEAD"])?;
        if head.status.success() {
            let stderr = String::from_utf8_lossy(&diff.stderr).trim().to_string();
            if stderr.is_empty() {
                return Err("git diff failed".to_string());
            }
            return Err(format!("git diff failed: {stderr}"));
        }
        let staged = run_git(dir, &["diff", "--cached", "--name-only", "-z", "--"])?;
        let unstaged = run_git(dir, &["diff", "--name-only", "-z", "--"])?;
        if !staged.status.success() || !unstaged.status.success() {
            return Err("git diff failed (no HEAD)".to_string());
        }
        let mut names = parse_nul_names(&staged.stdout);
        names.extend(parse_nul_names(&unstaged.stdout));
        names.sort();
        names.dedup();
        names
    };
    names.sort();
    names.dedup();

    let mut out = Vec::new();
    for rel in names {
        if rel.is_empty() {
            continue;
        }
        out.push((root.join(&rel), rel));
    }
    Ok(out)
}

/// Lint the working-tree changes under `dir` with `engine`, skipping
/// deleted files.
pub fn lint_changed_in_with_engine(
    engine: &Engine,
    dir: &Path,
) -> Result<Vec<LintMessage>, String> {
    let files = changed_files_in(dir)?;
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

/// Lint the working-tree changes under `dir` with every ported linter,
/// skipping deleted files.
pub fn lint_changed_in(dir: &Path) -> Result<Vec<LintMessage>, String> {
    lint_changed_in_with_engine(&Engine::new(), dir)
}

/// Collect `run_in` messages: working-tree changes when no paths were given,
/// else the explicit paths.
fn collect_run_messages(
    engine: &Engine,
    cwd: &Path,
    paths: &[String],
) -> Result<Vec<LintMessage>, String> {
    if paths.is_empty() {
        lint_changed_in_with_engine(engine, cwd)
    } else {
        lint_explicit_paths_with_engine(engine, paths)
    }
}

/// Print collected lint messages as JSON or one-per-line text.
fn emit_messages(messages: &[LintMessage], json: bool) {
    if json {
        let array: Vec<serde_json::Value> = messages.iter().map(LintMessage::to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&array).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for message in messages {
            println!("{}", message.to_line());
        }
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

/// Entry point with an explicit working directory (the discovery root and
/// default lint target), so tests can run without mutating the process
/// directory.
pub fn run_in(cwd: &Path, argv: &[String]) -> ExitCode {
    let args = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::NotYetPorted(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(args) => args,
    };

    let (engine, _config) = match engine_for_dir(cwd) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("arc-rs {NAME}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let messages = match collect_run_messages(&engine, cwd, &args.paths) {
        Ok(messages) => messages,
        Err(error) => {
            eprintln!("arc-rs {NAME}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let messages = filter_by_severity(messages, &args.severity);
    emit_messages(&messages, args.json);

    if messages.iter().any(|m| m.severity == SEVERITY_ERROR) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod lint_tests;
