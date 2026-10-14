//! `JSON` linter: JSON syntax errors via the `jsonlint` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistJSONLintLinter.php`.
//!
//! Runs `jsonlint --compact <file>` when `jsonlint` is on `PATH` and parses
//! stderr lines shaped `[path: ]line L, col C, description`. Every finding
//! is an error with code and name `JSON`.
//!
//! When the binary is absent the linter falls back to native `serde_json`
//! validation (ported from `ArcanistJSONLinter.php`, code `JSON1`, name
//! `Parse Error`) instead of going fully silent, so `.json` files are still
//! checked. Only `.json` files are examined; binary data (NUL byte) is
//! skipped. Directories and symlinks are skipped by `Engine::lint_file`
//! before bytes reach here.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter};

/// Registry name, matching PHP `getLinterName()`.
pub const LINTER_NAME: &str = "JSON";
/// Default binary, matching PHP `getDefaultBinary()`.
pub const BINARY: &str = "jsonlint";
/// File extensions this linter applies to.
const EXTENSIONS: &[&str] = &["json"];
/// Temp-file slug for this linter.
const SLUG: &str = "jsonlint";
/// Native-fallback code, matching the sibling `JsonLinter` port.
const NATIVE_CODE: &str = "JSON1";
/// Native-fallback name, matching PHP `ArcanistJSONLinter`.
const NATIVE_NAME: &str = "Parse Error";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `JSON` external linter.
#[derive(Debug, Default)]
pub struct JsonlintLinter {
    binary: Option<PathBuf>,
}

impl JsonlintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self { binary: None }
    }

    /// Override the tool binary (mirrors PHP `setBinary()`); used by tests
    /// to point the linter at a fake tool.
    #[must_use]
    pub fn with_binary(path: &Path) -> Self {
        Self {
            binary: Some(path.to_path_buf()),
        }
    }

    fn resolve_binary(&self) -> Option<PathBuf> {
        if let Some(path) = &self.binary {
            return path.is_file().then(|| path.clone());
        }
        find_on_path(BINARY)
    }
}

impl Linter for JsonlintLinter {
    fn name(&self) -> &'static str {
        LINTER_NAME
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !should_lint(path) || data.contains(&0) {
            return Vec::new();
        }
        let Some(binary) = self.resolve_binary() else {
            return native_json_check(path, data);
        };
        let Some(input) = write_temp_file(SLUG, path, "json", data) else {
            return Vec::new();
        };
        let args = vec!["--compact".to_string(), input.display().to_string()];
        let found = run_tool(&binary, &args).map_or_else(Vec::new, |out| {
            let parsed = parse_compact_output(path, &out.stderr);
            finish(path, &out, parsed)
        });
        std::fs::remove_file(&input).ok();
        found
    }
}

/// Parse `jsonlint --compact` stderr into lint messages. Lines that do not
/// match `[path: ]line L, col C, description` are skipped, like the PHP.
pub fn parse_compact_output(path: &str, stderr: &str) -> Vec<LintMessage> {
    let mut messages = Vec::new();
    for line in stderr.lines() {
        if let Some((line_no, col_no, description)) = parse_compact_line(line) {
            messages.push(LintMessage {
                path: path.to_string(),
                line: line_no,
                col: col_no,
                code: LINTER_NAME.to_string(),
                severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
                name: LINTER_NAME.to_string(),
                description,
            });
        }
    }
    messages
}

/// Match one stderr line against `[path: ]line L, col C, description`,
/// trying `: line ` splits rightmost-first like the greedy PHP regex.
fn parse_compact_line(line: &str) -> Option<(usize, usize, String)> {
    let mut splits = Vec::new();
    let mut search = line;
    while let Some(index) = search.rfind(": line ") {
        if index == 0 {
            break;
        }
        splits.push(index + ": ".len());
        search = &search[..index];
    }
    for start in splits {
        if let Some(found) = parse_compact_tail(&line[start..]) {
            return Some(found);
        }
    }
    if line.starts_with("line ") {
        return parse_compact_tail(line);
    }
    None
}

fn parse_compact_tail(rest: &str) -> Option<(usize, usize, String)> {
    let rest = rest.strip_prefix("line ")?;
    let (line_no, rest) = rest.split_once(", col ")?;
    let (col_no, description) = rest.split_once(", ")?;
    Some((
        parse_digits(line_no)?.max(1),
        parse_digits(col_no)?.max(1),
        ucfirst(description),
    ))
}

fn parse_digits(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok()
}

fn ucfirst(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Native JSON validation used when the `jsonlint` binary is absent,
/// ported from `ArcanistJSONLinter::lintPath`.
pub fn native_json_check(path: &str, data: &[u8]) -> Vec<LintMessage> {
    match serde_json::from_slice::<serde_json::Value>(data) {
        Ok(_) => Vec::new(),
        Err(error) => vec![LintMessage {
            path: path.to_string(),
            line: error.line().max(1),
            col: error.column().max(1),
            code: NATIVE_CODE.to_string(),
            severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
            name: NATIVE_NAME.to_string(),
            description: short_message(&error.to_string()),
        }],
    }
}

/// Strip `serde_json`'s trailing " at line L column C": positions travel in
/// dedicated fields.
fn short_message(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(index) => message[..index].to_string(),
        None => message.to_string(),
    }
}

fn should_lint(path: &str) -> bool {
    has_extension(path, EXTENSIONS)
}

fn has_extension(path: &str, allowed: &[&str]) -> bool {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some(dot) = file.rfind('.') else {
        return false;
    };
    let ext = &file[dot + 1..];
    !ext.is_empty() && allowed.iter().any(|want| ext.eq_ignore_ascii_case(want))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let mut candidates = vec![name.to_string()];
    if cfg!(windows) {
        for ext in ["exe", "cmd", "bat"] {
            candidates.push(format!("{name}.{ext}"));
        }
    }
    for dir in std::env::split_paths(&path_var) {
        for candidate in &candidates {
            let full = dir.join(candidate);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn temp_extension(path: &str, fallback: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match file.rfind('.') {
        Some(dot) if dot + 1 < file.len() => file[dot + 1..].to_string(),
        _ => fallback.to_string(),
    }
}

fn write_temp_file(slug: &str, path: &str, fallback_ext: &str, data: &[u8]) -> Option<PathBuf> {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let ext = temp_extension(path, fallback_ext);
    let file = std::env::temp_dir().join(format!("arc-rs-lint-{slug}-{pid}-{n}.{ext}"));
    std::fs::write(&file, data).ok()?;
    Some(file)
}

struct ToolOutput {
    success: bool,
    code: i32,
    stderr: String,
}

fn run_tool(binary: &Path, args: &[String]) -> Option<ToolOutput> {
    // npm ships Windows shims as `.cmd`/`.bat`, which need `cmd /C`.
    let scripted = matches!(
        binary.extension().and_then(|ext| ext.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd")
    );
    let mut command = if scripted {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(binary).args(args);
        command
    } else {
        let mut command = Command::new(binary);
        command.args(args);
        command
    };
    let output = arc_rs::lint::spawn_with_retry(&mut command).ok()?;
    Some(ToolOutput {
        success: output.status.success(),
        code: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Mirror PHP `resolveFuture`: a nonzero exit with no parsed messages means
/// the tool itself failed, which is reported as one error message.
fn finish(path: &str, out: &ToolOutput, parsed: Vec<LintMessage>) -> Vec<LintMessage> {
    if parsed.is_empty() && !out.success {
        let code = out.code;
        let detail = first_line(&out.stderr);
        vec![failure_message(
            path,
            &format!("{BINARY} failed (exit {code}): {detail}"),
        )]
    } else {
        parsed
    }
}

fn failure_message(path: &str, description: &str) -> LintMessage {
    LintMessage {
        path: path.to_string(),
        line: 1,
        col: 1,
        code: LINTER_NAME.to_string(),
        severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
        name: LINTER_NAME.to_string(),
        description: description.to_string(),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return "no output".to_string();
    }
    truncate(line, 300)
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

#[cfg(test)]
#[path = "lint_jsonlint_tests.rs"]
mod lint_jsonlint_tests;
