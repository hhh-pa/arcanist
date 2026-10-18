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
//! reach here. The `bin`, `flags`, `version`, `jshint.jshintrc`, and
//! `jshint.jshintignore` linter options apply (N4). The tool lints a
//! temp copy of the input, so config discovery relative to the source
//! file does not apply.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

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
    options: ToolOptions,
    jshintrc: Option<String>,
    jshintignore: Option<String>,
}

impl JshintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the tool binary (mirrors PHP `setBinary()`); used by tests
    /// to point the linter at a fake tool.
    #[must_use]
    pub fn with_binary(path: &Path) -> Self {
        Self {
            binary: Some(path.to_path_buf()),
            options: ToolOptions::default(),
            jshintrc: None,
            jshintignore: None,
        }
    }

    fn resolve_binary(&self) -> Option<PathBuf> {
        if let Some(path) = &self.binary {
            return path.is_file().then(|| path.clone());
        }
        if let Some(path) = &self.options.bin {
            return Some(path.clone());
        }
        find_on_path(BINARY)
    }

    /// Effective tool flags (without the file): mandatory plus
    /// configured-or-default, mirroring `getCommandFlags()`.
    fn tool_flags(&self, reporter: &str) -> Vec<String> {
        let mut mandatory = vec![format!("--reporter={reporter}")];
        if let Some(jshintrc) = &self.jshintrc {
            mandatory.push(format!("--config={jshintrc}"));
        }
        if let Some(jshintignore) = &self.jshintignore {
            mandatory.push(format!("--exclude-path={jshintignore}"));
        }
        self.options.command_flags(&mandatory, &[])
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
        if let Some(requirement) = &self.options.version
            && !version_requirement_satisfied(
                version_pattern_for("jshint"),
                &binary,
                requirement,
                "ArcanistJSHintLinter",
            )
        {
            return Vec::new();
        }
        let Some(input) = write_temp_file(SLUG, path, "js", data) else {
            return Vec::new(); // LCOV_EXCL_LINE // Reason: temp-dir write failure; hermetic tests cannot fail temp writes (TMPDIR is process-global)
        };
        let Some(reporter) = write_temp_file(SLUG, "reporter.js", "js", REPORTER_JS.as_bytes())
        else {
            std::fs::remove_file(&input).ok(); // LCOV_EXCL_START // Reason: temp-dir write failure; hermetic tests cannot fail temp writes (TMPDIR is process-global)
            return Vec::new(); // LCOV_EXCL_STOP
        };
        let mut args = self.tool_flags(&reporter.display().to_string());
        args.push(input.display().to_string());
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

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        if key == "jshint.jshintrc" || key == "jshint.jshintignore" {
            let Some(text) = value.as_str() else {
                return Err(format!("'{key}' must be a string"));
            };
            if key == "jshint.jshintrc" {
                self.jshintrc = Some(text.to_string());
            } else {
                self.jshintignore = Some(text.to_string());
            }
            return Ok(());
        }
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }

    fn validate_severity_code(&self, code: &str) -> Result<String, String> {
        // `/^(E|W)\d+$/`, like PHP: raw codes, no brackets.
        let bytes = code.as_bytes();
        let ok = bytes.len() > 1
            && matches!(bytes.first(), Some(b'E' | b'W'))
            && bytes[1..].iter().all(|b| b.is_ascii_digit());
        if ok {
            return Ok(code.to_string());
        }
        Err(format!(
            "Unrecognized lint message code \"{code}\". Expected a valid JSHint lint code like \
             \"E033\" or \"W093\"."
        ))
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
            granularity: None,
            locations: Vec::new(),
            bypass_changed_line_filtering: None,
            original_bytes: None,
            replacement_bytes: None,
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
        // LCOV_EXCL_START // Reason: windows-only PATH probe; cfg!(windows) is false on ubuntu CI
        for ext in ["exe", "cmd", "bat"] {
            candidates.push(format!("{name}.{ext}"));
        } // LCOV_EXCL_STOP
    }
    for dir in std::env::split_paths(&path_var) {
        for candidate in &candidates {
            let full = dir.join(candidate);
            if full.is_file() {
                return Some(full); // LCOV_EXCL_LINE // Reason: find hit; jshint is absent from PATH on ubuntu CI
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
        let mut command = crate::proc_env::command("cmd");
        command.arg("/C").arg(binary).args(args);
        command
    } else {
        let mut command = crate::proc_env::command(binary);
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
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
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
