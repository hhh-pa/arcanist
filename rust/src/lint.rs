//! `lint` workflow: text lint rules (T1128).
//!
//! PHP references: `src/lint/linter/ArcanistTextLinter.php` (rules and
//! messages), `src/lint/ArcanistLintSeverity.php` (severities), and
//! `src/workflow/ArcanistLintWorkflow.php` (CLI flags).
//!
//! Scope: a small [`Linter`] trait plus an [`Engine`], with [`TextLinter`]
//! covering trailing whitespace, tab literals, and a missing newline at
//! end of file. Severities mirror the PHP linter: trailing whitespace is
//! `autofix`, tab literals and a missing EOF newline are `error`.
//!
//! Out of scope (rejected with a not-yet-ported message): other linters
//! (`--engine`), autofix application (`--apply-patches`, `--amend-all`,
//! `--amend-autofixes`), and linting committed ranges (`--rev`).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub const NAME: &str = "lint";

pub const SEVERITY_ERROR: &str = "error";
pub const SEVERITY_AUTOFIX: &str = "autofix";

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
    "usage: arc-rs lint [PATHS...] [--json]\n\
     \n\
     Run text lint rules (trailing whitespace, tab literals, missing EOF\n\
     newline).\n\
     \n\
     If no PATHS are given, lint the tracked files changed in the working\n\
     tree versus HEAD (staged plus unstaged). Explicit paths must exist;\n\
     directories are walked recursively.\n\
     \n\
     Output is one line per message as path:line:col: severity: message,\n\
     or a JSON array of message objects with --json. Exits non-zero when\n\
     any error-severity message is found.\n\
     \n\
     Options:\n\
     \x20 --json        Emit a JSON array of message objects.\n\
     \x20 -h, --help    Show this help.\n\
     \n\
     Not yet ported (see T1128): other linters (--engine), autofix\n\
     (--apply-patches, --amend-all, --amend-autofixes), linting committed\n\
     ranges (--rev), and --everything/--lintall/--severity/--output/\n\
     --outfile/--never-apply-patches.\n"
        .to_string()
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
        "text"
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

/// Every ported linter, in registration order. Extended as linters land.
#[must_use]
pub fn all_linters() -> Vec<Box<dyn Linter>> {
    vec![Box::new(TextLinter::new())]
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            linters: all_linters(),
        }
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

#[derive(Debug, PartialEq, Eq)]
struct LintArgs {
    paths: Vec<String>,
    json: bool,
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
        "--everything"
        | "--lintall"
        | "--severity"
        | "--output"
        | "--outfile"
        | "--never-apply-patches" => Some("extended lint options"),
        _ => None,
    }
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut paths = Vec::new();
    let mut json = false;
    let mut end_of_flags = false;
    for arg in argv {
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
        } else if arg.starts_with('-') {
            let name = flag_name(arg);
            if let Some(reason) = not_yet_ported_reason(name) {
                return ParseOutcome::NotYetPorted(format!(
                    "arc-rs {NAME}: {name} is not yet ported ({reason}; see T1128)"
                ));
            }
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            paths.push(arg.clone());
        }
    }
    ParseOutcome::Args(LintArgs { paths, json })
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
            out.push((path.clone(), path.to_string_lossy().to_string()));
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
            out.push((disk, path.clone()));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

/// Lint explicit paths, returning sorted messages or a combined error.
pub fn lint_explicit_paths(paths: &[String]) -> Result<Vec<LintMessage>, String> {
    let engine = Engine::new();
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

/// Lint the working-tree changes under `dir`, skipping deleted files.
pub fn lint_changed_in(dir: &Path) -> Result<Vec<LintMessage>, String> {
    let engine = Engine::new();
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

pub fn run(argv: &[String]) -> ExitCode {
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

    let messages = if args.paths.is_empty() {
        let cwd = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(error) => {
                eprintln!("arc-rs {NAME}: cannot determine current directory: {error}");
                return ExitCode::FAILURE;
            }
        };
        match lint_changed_in(&cwd) {
            Ok(messages) => messages,
            Err(error) => {
                eprintln!("arc-rs {NAME}: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match lint_explicit_paths(&args.paths) {
            Ok(messages) => messages,
            Err(error) => {
                eprintln!("arc-rs {NAME}: {error}");
                return ExitCode::FAILURE;
            }
        }
    };

    if args.json {
        let array: Vec<serde_json::Value> = messages.iter().map(LintMessage::to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&array).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for message in &messages {
            println!("{}", message.to_line());
        }
    }

    if messages.iter().any(|m| m.severity == SEVERITY_ERROR) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod lint_tests;
