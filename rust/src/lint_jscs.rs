//! `JSCS` linter: JavaScript code style via the `jscs` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistJscsLinter.php`.
//!
//! Runs `jscs --reporter=checkstyle --no-colors <file>` when `jscs` is on
//! `PATH` and parses the checkstyle XML (`error` elements with `line`,
//! `column`, `severity`, and `message` attributes). Tool severities map to
//! `error`/`warning`, anything else to `advice`. When the binary is absent
//! the linter is inactive and reports nothing.
//!
//! Only `.js` files are examined; binary data (NUL byte) is skipped.
//! Directories and symlinks are skipped by `Engine::lint_file` before bytes
//! reach here. The `bin`, `flags`, `version`, `jscs.config`, and
//! `jscs.preset` linter options apply (N4). The tool lints a temp copy
//! of the input, so config discovery relative to the source file does
//! not apply.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// Registry name, matching PHP `getLinterName()`.
pub const LINTER_NAME: &str = "JSCS";
/// Default binary, matching PHP `getDefaultBinary()`.
pub const BINARY: &str = "jscs";
/// File extensions this linter applies to.
const EXTENSIONS: &[&str] = &["js"];
/// Temp-file slug for this linter.
const SLUG: &str = "jscs";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `JSCS` external linter.
#[derive(Debug, Default)]
pub struct JscsLinter {
    binary: Option<PathBuf>,
    options: ToolOptions,
    config: Option<String>,
    preset: Option<String>,
}

impl JscsLinter {
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
            preset: None,
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
        let mut mandatory = vec![
            "--reporter=checkstyle".to_string(),
            "--no-colors".to_string(),
        ];
        if let Some(config) = &self.config {
            mandatory.push(format!("--config={config}"));
        }
        if let Some(preset) = &self.preset {
            mandatory.push(format!("--preset={preset}"));
        }
        self.options.command_flags(&mandatory, &[])
    }
}

impl Linter for JscsLinter {
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
                version_pattern_for("jscs"),
                &binary,
                requirement,
                "ArcanistJscsLinter",
            )
        {
            return Vec::new();
        }
        let Some(input) = write_temp_file(SLUG, path, "js", data) else {
            return Vec::new();
        };
        let mut args = self.tool_flags();
        args.push(input.display().to_string());
        let found = run_tool(&binary, &args).map_or_else(Vec::new, |out| {
            match parse_checkstyle_output(path, &out.stdout) {
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
        if key == "jscs.config" || key == "jscs.preset" {
            let Some(text) = value.as_str() else {
                return Err(format!("'{key}' must be a string"));
            };
            if key == "jscs.config" {
                self.config = Some(text.to_string());
            } else {
                self.preset = Some(text.to_string());
            }
            return Ok(());
        }
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }

    fn validate_severity_code(&self, _code: &str) -> Result<String, String> {
        // Rules can't be identified from messages, like PHP.
        Err(
            "JSCS does not currently support custom severity levels, because rules can't be \
             identified from messages in output."
                .to_string(),
        )
    }
}

/// Parse `jscs --reporter=checkstyle` XML output into lint messages.
pub fn parse_checkstyle_output(path: &str, stdout: &str) -> Result<Vec<LintMessage>, String> {
    if !stdout.contains("<checkstyle") {
        return Err(format!("{LINTER_NAME} returned unparseable output."));
    }
    let mut messages = Vec::new();
    for attrs in element_tags(stdout, "error") {
        let severity = match attr(&attrs, "severity") {
            "error" => arc_rs::lint::SEVERITY_ERROR,
            "warning" => "warning",
            _ => "advice",
        };
        messages.push(LintMessage {
            path: path.to_string(),
            line: attr_usize(&attrs, "line"),
            col: attr_usize(&attrs, "column"),
            code: LINTER_NAME.to_string(),
            severity: severity.to_string(),
            name: LINTER_NAME.to_string(),
            description: attr(&attrs, "message").to_string(),
            original: None,
            replacement: None,
        });
    }
    Ok(messages)
}

/// Collect the attribute lists of every `<tag ...>` element occurrence.
fn element_tags(xml: &str, tag: &str) -> Vec<Vec<(String, String)>> {
    let open = format!("<{tag}");
    let mut tags = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open.as_str()) {
        rest = &rest[start + open.len()..];
        match rest.chars().next() {
            Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('/') | Some('>') => {}
            _ => continue,
        }
        let Some(end) = tag_end(rest) else {
            break;
        };
        tags.push(parse_attrs(&rest[..end]));
        rest = &rest[end + 1..];
    }
    tags
}

/// Byte index of the `>` closing a tag, ignoring quoted `>` characters.
fn tag_end(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in text.char_indices() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
        } else {
            match ch {
                '"' | '\'' => quote = Some(ch),
                '>' => return Some(index),
                _ => {}
            }
        }
    }
    None
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-' | b'.')
}

/// Parse `name="value"` pairs from inside a tag; values are XML-unescaped.
fn parse_attrs(tag: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match scan_attr(tag, bytes, i) {
            AttrScan::Parsed(key, value, next) => {
                attrs.push((key, value));
                i = next;
            }
            AttrScan::Skipped(next) => i = next,
            AttrScan::Halt => break,
        }
    }
    attrs
}

/// Outcome of scanning one attribute at an offset: parsed, skipped (resume
/// at the offset), or halted (a trailing `=` with nothing after it).
enum AttrScan {
    Parsed(String, String, usize),
    Skipped(usize),
    Halt,
}

/// Scan one attribute starting at `start` (past leading padding).
fn scan_attr(tag: &str, bytes: &[u8], start: usize) -> AttrScan {
    let mut i = skip_attr_gap(bytes, start);
    if i >= bytes.len() {
        return AttrScan::Halt;
    }
    let name_start = i;
    while i < bytes.len() && is_name_byte(bytes[i]) {
        i += 1;
    }
    if name_start == i {
        return AttrScan::Skipped(skip_attr_char(tag, i));
    }
    let key = tag[name_start..i].to_string();
    scan_attr_value(tag, bytes, key, i)
}

/// Scan the `= "value"` half of an attribute whose name ends at `start`.
fn scan_attr_value(tag: &str, bytes: &[u8], key: String, start: usize) -> AttrScan {
    let mut i = skip_attr_ws(bytes, start);
    if i >= bytes.len() || bytes[i] != b'=' {
        return AttrScan::Skipped(i);
    }
    i += 1;
    i = skip_attr_ws(bytes, i);
    if i >= bytes.len() {
        return AttrScan::Halt;
    }
    let quote = bytes[i];
    if quote != b'"' && quote != b'\'' {
        return AttrScan::Skipped(i);
    }
    i += 1;
    let value_start = i;
    while i < bytes.len() && bytes[i] != quote {
        i += 1;
    }
    let raw = &tag[value_start..i.min(bytes.len())];
    let value = unescape_xml(raw);
    if i < bytes.len() {
        i += 1;
    }
    AttrScan::Parsed(key, value, i)
}

/// Skip inter-attribute padding (whitespace and `/`).
fn skip_attr_gap(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
        i += 1;
    }
    i
}

/// Skip whitespace between an attribute name, `=`, and its value.
fn skip_attr_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Skip one non-name character (a full UTF-8 scalar, never a bare byte).
fn skip_attr_char(tag: &str, i: usize) -> usize {
    i + tag[i..].chars().next().map_or(1, |ch| ch.len_utf8())
}

fn unescape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let entity = &rest[amp..];
        if let Some(semi) = entity.find(';') {
            let body = &entity[1..semi];
            if let Some(decoded) = decode_entity(body) {
                out.push_str(&decoded);
                rest = &entity[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[amp + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_entity(body: &str) -> Option<String> {
    match body {
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" => Some("'".to_string()),
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = if let Some(hex) = digits
                .strip_prefix('x')
                .or_else(|| digits.strip_prefix('X'))
            {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                digits.parse::<u32>().ok()?
            };
            char::from_u32(code).map(|ch| ch.to_string())
        }
    }
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> &'a str {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map_or("", |(_, value)| value.as_str())
}

fn attr_usize(attrs: &[(String, String)], name: &str) -> usize {
    attr(attrs, name).parse::<usize>().unwrap_or(1).max(1)
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
#[path = "lint_jscs_tests.rs"]
mod lint_jscs_tests;
