//! `GJSLINT` linter: JavaScript checks via the `gjslint` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistClosureLinter.php`.
//!
//! Runs `gjslint <file>` (no mandatory flags) when `gjslint` is on `PATH`
//! and parses stdout lines shaped `Line L, E:NNN: description`. Every
//! finding is an error with code and name `GJSLINT<NNN>`. When the binary
//! is absent the linter is inactive and reports nothing.
//!
//! Only `.js` files are examined; binary data (NUL byte) is skipped.
//! Directories and symlinks are skipped by `Engine::lint_file` before bytes
//! reach here. Custom `bin`/`flags` configuration is not supported. The
//! tool lints a temp copy of the input, so config discovery relative to
//! the source file does not apply.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter};

/// Registry name, matching PHP `getLinterName()`.
pub const LINTER_NAME: &str = "GJSLINT";
/// Default binary, matching PHP `getDefaultBinary()`.
pub const BINARY: &str = "gjslint";
/// File extensions this linter applies to.
const EXTENSIONS: &[&str] = &["js"];
/// Temp-file slug for this linter.
const SLUG: &str = "closure";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `GJSLINT` external linter.
#[derive(Debug, Default)]
pub struct ClosureLinter {
    binary: Option<PathBuf>,
}

impl ClosureLinter {
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

impl Linter for ClosureLinter {
    fn name(&self) -> &'static str {
        LINTER_NAME
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !should_lint(path) || data.contains(&0) {
            return Vec::new();
        }
        let Some(binary) = self.resolve_binary() else {
            return Vec::new();
        };
        let Some(input) = write_temp_file(SLUG, path, "js", data) else {
            return Vec::new();
        };
        let args = vec![input.display().to_string()];
        let found = run_tool(&binary, &args).map_or_else(Vec::new, |out| {
            let parsed = parse_gjslint_output(path, &out.stdout);
            finish(path, &out, parsed)
        });
        std::fs::remove_file(&input).ok();
        found
    }
}

/// Parse `gjslint` stdout into lint messages. Lines that do not match
/// `Line L, E:NNN: description` are skipped, like the PHP.
pub fn parse_gjslint_output(path: &str, stdout: &str) -> Vec<LintMessage> {
    let mut messages = Vec::new();
    for line in stdout.lines() {
        if let Some((line_no, number, description)) = parse_gjslint_line(line) {
            messages.push(LintMessage {
                path: path.to_string(),
                line: line_no,
                col: 1,
                code: format!("{LINTER_NAME}{number}"),
                severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
                name: format!("{LINTER_NAME}{number}"),
                description,
            });
        }
    }
    messages
}

/// Match one stdout line against `Line L, E:NNN: description`.
fn parse_gjslint_line(line: &str) -> Option<(usize, String, String)> {
    let rest = line.strip_prefix("Line ")?;
    let (line_no, rest) = rest.split_once(", E:")?;
    let line_no = parse_digits(line_no)?.max(1);
    let (number, description) = rest.split_once(": ")?;
    parse_digits(number)?;
    Some((line_no, number.to_string(), description.to_string()))
}

fn parse_digits(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok()
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
    stdout: String,
    stderr: String,
}

fn run_tool(binary: &Path, args: &[String]) -> Option<ToolOutput> {
    // npm ships Windows shims as `.cmd`/`.bat`, which need `cmd /C`.
    let scripted = matches!(
        binary.extension().and_then(|ext| ext.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd")
    );
    let output = if scripted {
        Command::new("cmd")
            .arg("/C")
            .arg(binary)
            .args(args)
            .output()
    } else {
        Command::new(binary).args(args).output()
    };
    let output = output.ok()?;
    Some(ToolOutput {
        success: output.status.success(),
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
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
#[path = "lint_closure_tests.rs"]
mod lint_closure_tests;
