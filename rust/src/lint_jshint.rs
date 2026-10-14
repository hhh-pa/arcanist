//! `JSHint` linter: JavaScript error checking via the `jshint` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistJSHintLinter.php` with the JSON
//! reporter at `src/lint/linter/reporter.js` (embedded below as
//! [`REPORTER_JS`]).
//!
//! Runs `jshint --reporter=<embedded-reporter> <file>` when `jshint` is on
//! `PATH` and parses the JSON report array (`line`, `col`, `reason`,
//! `code`). Codes starting with `W` are warnings, `E043` is disabled, and
//! everything else is an error. When the binary is absent the linter is
//! inactive and reports nothing.
//!
//! Only `.js` files are examined; binary data (NUL byte) is skipped.
//! Directories and symlinks are skipped by `Engine::lint_file` before bytes
//! reach here. Linter configuration (`jshint.jshintrc`,
//! `jshint.jshintignore`, custom `bin`/`flags`) is not supported. The tool
//! lints a temp copy of the input, so config discovery relative to the
//! source file does not apply.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter};

/// Registry name, matching PHP `getLinterName()`.
pub const LINTER_NAME: &str = "JSHint";
/// Default binary, matching PHP `getDefaultBinary()`.
pub const BINARY: &str = "jshint";
/// File extensions this linter applies to.
const EXTENSIONS: &[&str] = &["js"];
/// Temp-file slug for this linter.
const SLUG: &str = "jshint";

/// Embedded copy of `src/lint/linter/reporter.js`: writes a JSON array of
/// `{file, line, col, reason, code}` objects to stdout.
const REPORTER_JS: &str = "module.exports = {
  reporter: function (results) {
    var report = [];

    results.forEach(function (result) {
      var error = result.error;
      report.push({
        'file'    : result.file,
        'line'    : error.line,
        'col'     : error.character,
        'reason'  : error.reason,
        'code'    : error.code,
      });
    });

    process.stdout.write(JSON.stringify(report));
  }
};
";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `JSHint` external linter.
#[derive(Debug, Default)]
pub struct JshintLinter {
    binary: Option<PathBuf>,
}

impl JshintLinter {
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

impl Linter for JshintLinter {
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
        let Some(reporter) = write_temp_file(SLUG, "reporter.js", "js", REPORTER_JS.as_bytes())
        else {
            std::fs::remove_file(&input).ok();
            return Vec::new();
        };
        let args = vec![
            format!("--reporter={}", reporter.display()),
            input.display().to_string(),
        ];
        let found =
            run_tool(&binary, &args).map_or_else(Vec::new, |out| {
                match parse_reporter_output(path, &out.stdout) {
                    Ok(parsed) => finish(path, &out, parsed),
                    Err(description) if out.success => vec![failure_message(path, &description)],
                    Err(_) => vec![tool_failure_message(path, &out)],
                }
            });
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&reporter).ok();
        found
    }
}

/// Parse `jshint --reporter` JSON output into lint messages.
pub fn parse_reporter_output(path: &str, stdout: &str) -> Result<Vec<LintMessage>, String> {
    let value: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|err| format!("{LINTER_NAME} returned unparseable output: {err}"))?;
    let items = value.as_array().ok_or_else(|| {
        format!("{LINTER_NAME} returned unparseable output: expected a JSON array")
    })?;
    let mut messages = Vec::with_capacity(items.len());
    for item in items {
        let code = item
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let line = item.get("line").and_then(json_usize).unwrap_or(1).max(1);
        let col = item.get("col").and_then(json_usize).unwrap_or(1).max(1);
        let reason = item
            .get("reason")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let severity = if code.starts_with('W') {
            "warning"
        } else if code == "E043" {
            "disabled"
        } else {
            arc_rs::lint::SEVERITY_ERROR
        };
        messages.push(LintMessage {
            path: path.to_string(),
            line,
            col,
            code: code.to_string(),
            severity: severity.to_string(),
            name: format!("{LINTER_NAME}{code}"),
            description: reason.to_string(),
            original: None,
            replacement: None,
        });
    }
    Ok(messages)
}

fn json_usize(value: &serde_json::Value) -> Option<usize> {
    value.as_u64().and_then(|n| usize::try_from(n).ok())
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
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Mirror PHP `resolveFuture`: a nonzero exit with no parsed messages means
/// the tool itself failed, which is reported as one error message.
fn finish(path: &str, out: &ToolOutput, parsed: Vec<LintMessage>) -> Vec<LintMessage> {
    if parsed.is_empty() && !out.success {
        vec![tool_failure_message(path, out)]
    } else {
        parsed
    }
}

fn tool_failure_message(path: &str, out: &ToolOutput) -> LintMessage {
    let code = out.code;
    let detail = first_line(&out.stderr);
    failure_message(path, &format!("{BINARY} failed (exit {code}): {detail}"))
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
        original: None,
        replacement: None,
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
#[path = "lint_jshint_tests.rs"]
mod lint_jshint_tests;
