//! `COFFEE` linter: CoffeeScript style via the `coffeelint` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistCoffeeLintLinter.php`.
//!
//! Runs `coffeelint --reporter=raw <file>` when `coffeelint` is on `PATH`
//! and parses the raw JSON report: an object with one entry mapping the
//! file to its report list (`lineNumber`, `name`, `message`, `level`).
//! Levels map to `warning` (`warn`) and `error` (`error`), anything else
//! to `advice`. Report names are `snake_case` converted to words. When the
//! binary is absent the linter is inactive and reports nothing.
//!
//! Only `.coffee` files are examined; binary data (NUL byte) is skipped.
//! Directories and symlinks are skipped by `Engine::lint_file` before bytes
//! reach here. The `bin`, `flags`, `version`, and `coffeelint.config`
//! linter options apply (N4). The tool lints a temp copy of the input,
//! so config discovery relative to the source file does not apply.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// Registry name, matching PHP `getLinterName()`.
pub const LINTER_NAME: &str = "COFFEE";
/// Default binary, matching PHP `getDefaultBinary()`.
pub const BINARY: &str = "coffeelint";
/// File extensions this linter applies to.
const EXTENSIONS: &[&str] = &["coffee"];
/// Temp-file slug for this linter.
const SLUG: &str = "coffeelint";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `COFFEE` external linter.
#[derive(Debug, Default)]
pub struct CoffeelintLinter {
    binary: Option<PathBuf>,
    options: ToolOptions,
    config: Option<String>,
}

impl CoffeelintLinter {
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
            config: None,
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
    fn tool_flags(&self) -> Vec<String> {
        let mut mandatory = vec!["--reporter=raw".to_string()];
        if let Some(config) = &self.config {
            mandatory.push(format!("--file={config}"));
        }
        self.options.command_flags(&mandatory, &[])
    }
}

impl Linter for CoffeelintLinter {
    fn name(&self) -> &'static str {
        LINTER_NAME
    }

    fn applies_custom_severities(&self) -> bool {
        // The PHP linter never consults custom severity maps or rules.
        false
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
                version_pattern_for("coffeelint"),
                &binary,
                requirement,
                "ArcanistCoffeeLintLinter",
            )
        {
            return Vec::new();
        }
        let Some(input) = write_temp_file(SLUG, path, "coffee", data) else {
            return Vec::new();
        };
        let mut args = self.tool_flags();
        args.push(input.display().to_string());
        let found = run_tool(&binary, &args).map_or_else(Vec::new, |out| {
            match parse_raw_output(path, &out.stdout) {
                Ok(parsed) => finish(path, &out, parsed),
                Err(description) if out.success => vec![failure_message(path, &description)],
                Err(_) => vec![tool_failure_message(path, &out)],
            }
        });
        std::fs::remove_file(&input).ok();
        found
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        if key == "coffeelint.config" {
            let Some(config) = value.as_str() else {
                return Err("'coffeelint.config' must be a string".to_string());
            };
            self.config = Some(config.to_string());
            return Ok(());
        }
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }

    fn validate_severity_code(&self, _code: &str) -> Result<String, String> {
        // Rules can't be identified from messages, like PHP.
        Err(
            "CoffeeLint does not currently support custom severity levels, because rules can't \
             be identified from messages in output."
                .to_string(),
        )
    }
}

/// Parse `coffeelint --reporter=raw` JSON output into lint messages.
pub fn parse_raw_output(path: &str, stdout: &str) -> Result<Vec<LintMessage>, String> {
    let value: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|err| format!("{LINTER_NAME} returned unparseable output: {err}"))?;
    let report = value.as_object().and_then(|object| {
        if object.len() == 1 {
            object.values().next().and_then(|entry| entry.as_array())
        } else {
            None
        }
    });
    let Some(reports) = report else {
        return Err(format!(
            "{LINTER_NAME} returned unparseable output: expected a single file report"
        ));
    };
    let mut messages = Vec::with_capacity(reports.len());
    for item in reports {
        let line = item
            .get("lineNumber")
            .and_then(json_usize)
            .unwrap_or(1)
            .max(1);
        let raw_name = item
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let message = item
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let severity = match item.get("level").and_then(|value| value.as_str()) {
            Some("warn") => "warning",
            Some("error") => arc_rs::lint::SEVERITY_ERROR,
            _ => "advice",
        };
        messages.push(LintMessage {
            path: path.to_string(),
            line,
            col: 1,
            code: LINTER_NAME.to_string(),
            severity: severity.to_string(),
            name: ucwords(&raw_name.replace('_', " ")),
            description: message.to_string(),
            original: item
                .get("line")
                .and_then(|value| value.as_str())
                .map(str::to_string),
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

/// Uppercase ASCII initials after whitespace, mirroring PHP `ucwords()`
/// (byte-based: only `a-z` change, so non-ASCII bytes pass through).
fn ucwords(text: &str) -> String {
    let mut out = text.as_bytes().to_vec();
    let mut start = true;
    for byte in &mut out {
        if start {
            byte.make_ascii_uppercase();
        }
        start = matches!(*byte, b' ' | b'\t' | b'\r' | b'\n' | b'\x0C' | b'\x0B');
    }
    // ASCII case mapping preserves UTF-8 validity.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
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
#[path = "lint_coffeelint_tests.rs"]
mod lint_coffeelint_tests;
