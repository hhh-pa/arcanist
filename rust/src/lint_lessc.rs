//! `LESSC` linter: LESS errors via the `lessc` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistLesscLinter.php`.
//!
//! Runs `lessc --lint --no-color --strict-math=off --strict-units=off
//! <file>` when `lessc` is on `PATH` and parses stderr lines shaped
//! `Name: description in path on line L, column C:`. Every finding is an
//! error with code `LESSC<Name>` and name `Unknown lint message!` (the
//! PHP name-map miss). When the binary is absent the linter is inactive
//! and reports nothing.
//!
//! Only `.less` files are examined; binary data (NUL byte) is skipped.
//! Directories and symlinks are skipped by `Engine::lint_file` before bytes
//! reach here. The `bin`, `flags`, `version`, `lessc.strict-math`, and
//! `lessc.strict-units` linter options apply (N4); strict modes default
//! to `off` like the PHP. The tool lints a temp copy of the input, so
//! config discovery relative to the source file does not apply.
//!
//! Quirk preserved: the PHP looks up the message name with the string
//! error name against an integer-keyed map, so every finding reports
//! `Unknown lint message!`.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// Registry name, matching PHP `getLinterName()`.
pub const LINTER_NAME: &str = "LESSC";
/// Default binary, matching PHP `getDefaultBinary()`.
pub const BINARY: &str = "lessc";
/// File extensions this linter applies to.
const EXTENSIONS: &[&str] = &["less"];
/// Temp-file slug for this linter.
const SLUG: &str = "lessc";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `LESSC` external linter.
#[derive(Debug, Default)]
pub struct LesscLinter {
    binary: Option<PathBuf>,
    options: ToolOptions,
    strict_math: bool,
    strict_units: bool,
}

impl LesscLinter {
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
            strict_math: false,
            strict_units: false,
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
        let on_off = |on: bool| if on { "on" } else { "off" };
        let mandatory = vec![
            "--lint".to_string(),
            "--no-color".to_string(),
            format!("--strict-math={}", on_off(self.strict_math)),
            format!("--strict-units={}", on_off(self.strict_units)),
        ];
        self.options.command_flags(&mandatory, &[])
    }
}

impl Linter for LesscLinter {
    fn name(&self) -> &'static str {
        LINTER_NAME
    }

    fn severity_lookup_code<'a>(&self, full_code: &'a str) -> &'a str {
        // PHP looks severities up by the string error name (the code
        // with the `LESSC` prefix stripped).
        full_code.strip_prefix(LINTER_NAME).unwrap_or(full_code)
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
                version_pattern_for("lessc"),
                &binary,
                requirement,
                "ArcanistLesscLinter",
            )
        {
            return Vec::new();
        }
        let Some(input) = write_temp_file(SLUG, path, "less", data) else {
            return Vec::new(); // LCOV_EXCL_LINE // Reason: temp-dir write failure; hermetic tests cannot fail temp writes (TMPDIR is process-global)
        };
        let mut args = self.tool_flags();
        args.push(input.display().to_string());
        let found = run_tool(&binary, &args).map_or_else(Vec::new, |out| {
            let parsed = parse_lint_output(path, &out.stderr);
            finish(path, &out, parsed)
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
        if key == "lessc.strict-math" || key == "lessc.strict-units" {
            let Some(on) = value.as_bool() else {
                return Err(format!("'{key}' must be a boolean"));
            };
            if key == "lessc.strict-math" {
                self.strict_math = on;
            } else {
                self.strict_units = on;
            }
            return Ok(());
        }
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }
}

/// PHP's `getLintMessageName()` fallback: the lessc name map is keyed by
/// integer codes but looked up with the string error name, so every
/// finding reports this name.
const NAME_UNKNOWN: &str = "Unknown lint message!";

/// Parse `lessc --lint` stderr into lint messages. Lines that do not match
/// `Name: description in path on line L, column C:` are skipped, like the
/// PHP. A matching line with an unrecognized error name aborts the whole
/// parse with a single failure message (the PHP throws on those,
/// discarding every finding).
pub fn parse_lint_output(path: &str, stderr: &str) -> Vec<LintMessage> {
    let mut messages = Vec::new();
    for line in stderr.lines() {
        let Some((name, description, line_no, col_no)) = parse_lint_line(line) else {
            continue;
        };
        if !is_known_error(&name) {
            return vec![failure_message(
                path,
                &format!("Unrecognized lessc error {name:?}."),
                line_no,
                col_no,
            )];
        }
        messages.push(LintMessage {
            path: path.to_string(),
            line: line_no,
            col: col_no,
            code: format!("{LINTER_NAME}{name}"),
            severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
            name: NAME_UNKNOWN.to_string(),
            description: ucfirst(&description),
            original: None,
            replacement: None,
            granularity: None,
            locations: Vec::new(),
            bypass_changed_line_filtering: None,
            original_bytes: None,
            replacement_bytes: None,
        });
    }
    messages
}

/// Match one stderr line against
/// `Name: description in path on line L, column C:`.
fn parse_lint_line(line: &str) -> Option<(String, String, usize, usize)> {
    let without_colon = line.strip_suffix(':')?;
    let (head, location) = without_colon.rsplit_once(" on line ")?;
    let (line_no, col_no) = location.split_once(", column ")?;
    let line_no = parse_digits(line_no)?.max(1);
    let col_no = parse_digits(col_no)?.max(1);
    let (name, rest) = head.split_once(": ")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return None;
    }
    let (description, _path) = rest.rsplit_once(" in ")?;
    if description.is_empty() {
        return None;
    }
    Some((name.to_string(), description.to_string(), line_no, col_no))
}

/// Error names from the PHP `switch ($matches['name'])`; anything else
/// makes the PHP throw `Unrecognized lint message code`.
fn is_known_error(name: &str) -> bool {
    matches!(
        name,
        "RuntimeError"
            | "ArgumentError"
            | "FileError"
            | "NameError"
            | "OperationError"
            | "ParseError"
            | "SyntaxError"
    )
}

fn parse_digits(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok()
}

/// Uppercase a leading ASCII lowercase byte, mirroring PHP `ucfirst()`
/// (byte-based: non-ASCII initials pass through unchanged).
fn ucfirst(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    if let Some(first) = bytes.first_mut() {
        first.make_ascii_uppercase();
    }
    // ASCII case mapping preserves UTF-8 validity.
    String::from_utf8(bytes).unwrap_or_else(|_| text.to_string())
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
            1,
            1,
        )]
    } else {
        parsed
    }
}

fn failure_message(path: &str, description: &str, line: usize, col: usize) -> LintMessage {
    LintMessage {
        path: path.to_string(),
        line,
        col,
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
#[path = "lint_lessc_tests.rs"]
mod lint_lessc_tests;
