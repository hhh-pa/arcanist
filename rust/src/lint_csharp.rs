//! `arc-rs lint` C# linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistCSharpLinter.php`.
//!
//! Behavior: runs the `cslint.exe` analyzer on C# files and parses the
//! stdout JSON array of per-file results. Exact argv mirrors
//! `willLintPaths()`: `cslint.exe --settings-base64=<base64 of the
//! discovery map> -r=. <file>`, executed directly on Windows and via
//! `mono` elsewhere. With no configured discovery map the settings
//! payload is base64(`null`), i.e. `bnVsbA==`. Each issue becomes one
//! message with code/name from the issue index, a `vsprintf()`-rendered
//! description, and severity 0=advice, 1=autofix, 2=warning, 3=error,
//! 4=disabled.
//!
//! Known limits (see T1128): the `discovery` and `binary` linter options
//! are not supported (empty discovery map, PATH lookup); only one file is
//! passed per invocation (PHP batches paths up to the command-line
//! limit); a nonzero tool exit or invalid JSON yields no messages (the
//! PHP linter raises, but the [`Linter`](arc_rs::lint::Linter) trait has
//! no error channel); reported paths are the display paths passed to
//! [`lint_bytes`](arc_rs::lint::Linter::lint_bytes), never the staged
//! temp-file paths.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter, SEVERITY_AUTOFIX, SEVERITY_ERROR};

/// Companion severities mirroring `ArcanistLintSeverity`; `lint.rs` only
/// defines the severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";
const SEVERITY_DISABLED: &str = "disabled";

/// C# linter: `cslint.exe` findings via an external binary.
#[derive(Debug, Default)]
pub struct CsharpLinter;

impl CsharpLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Analyzer binary name, mirroring the `cslint.exe` PATH lookup.
#[must_use]
pub fn binary_name() -> &'static str {
    "cslint.exe"
}

/// Runtime engine binary used off Windows, mirroring `loadEnvironment()`.
#[must_use]
pub fn runtime_name() -> &'static str {
    "mono"
}

/// Base64 of the default (empty) discovery map JSON `null`.
#[must_use]
pub fn default_settings_base64() -> &'static str {
    "bnVsbA=="
}

/// Exact tool argv flags after the program, mirroring `willLintPaths()`:
/// `--settings-base64=<...>` then `-r=.`.
#[must_use]
pub fn command_flags() -> Vec<String> {
    vec![
        format!("--settings-base64={}", default_settings_base64()),
        "-r=.".to_string(),
    ]
}

fn is_supported(path: &str) -> bool {
    extension_of(path).is_some_and(|ext| ext.eq_ignore_ascii_case("cs"))
}

fn extension_of(path: &str) -> Option<&str> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let dot = base.rfind('.')?;
    if dot + 1 >= base.len() {
        return None;
    }
    Some(&base[dot + 1..])
}

/// Locate `binary` on a `$PATH`-style search string (mirrors
/// `anoid::find_on_path` so this module stays standalone-includeable).
fn find_on_path(path_env: &std::ffi::OsStr, binary: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Resolved `(program, prefix_argv)` for a `$PATH`-style search string:
/// `cslint.exe` directly on Windows, `mono cslint.exe` elsewhere. `None`
/// means the linter is inactive.
fn resolve_command_in(path_env: &std::ffi::OsStr) -> Option<(PathBuf, Vec<String>)> {
    let cslint = find_on_path(path_env, binary_name())?;
    let mut prefix = command_flags();
    if cfg!(windows) {
        Some((cslint, prefix))
    } else {
        let mono = find_on_path(path_env, runtime_name())?;
        prefix.insert(0, cslint.to_string_lossy().into_owned());
        Some((mono, prefix))
    }
}

/// Resolve the tool on the real `$PATH`; `None` means the linter is
/// inactive and reports no messages.
#[must_use]
pub fn resolve_command() -> Option<(PathBuf, Vec<String>)> {
    let path = std::env::var_os("PATH")?;
    resolve_command_in(&path)
}

/// Staged copy of the linted bytes: the external tool needs a real file.
struct StagedFile {
    dir: PathBuf,
    file: PathBuf,
}

impl StagedFile {
    fn stage(display_path: &str, data: &[u8]) -> Result<Self, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-lint-csharp-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create temp dir: {e}"))?;
        let file = dir.join(staged_name(display_path));
        std::fs::write(&file, data).map_err(|e| format!("cannot stage lint input: {e}"))?;
        Ok(Self { dir, file })
    }

    fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Temp file name preserving a safe source extension so the tool sniffs
/// the file type correctly.
fn staged_name(display_path: &str) -> String {
    match extension_of(display_path) {
        Some(ext)
            if ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+') =>
        {
            format!("src.{ext}")
        }
        _ => "src.cs".to_string(),
    }
}

fn run_tool(program: &Path, prefix: &[String], file: &Path) -> Option<(i32, String, String)> {
    let mut command = Command::new(program);
    command.args(prefix).arg(file).stdin(Stdio::null());
    let output = arc_rs::lint::spawn_with_retry(&mut command).ok()?;
    let code = output.status.code().unwrap_or(1);
    Some((
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Linter body with an explicit tool command (mirrors the PHP `binary`
/// option). `None` (tool absent) reports no messages.
pub fn lint_with_command(
    path: &str,
    data: &[u8],
    command: Option<(&Path, &[String])>,
) -> Vec<LintMessage> {
    let Some((program, prefix)) = command else {
        return Vec::new();
    };
    let Ok(staged) = StagedFile::stage(path, data) else {
        return Vec::new();
    };
    let Some((code, stdout, stderr)) = run_tool(program, prefix, staged.path()) else {
        return Vec::new();
    };
    parse_output(path, code, &stdout, &stderr)
}

/// Parse cslint stdout JSON, mirroring `resolveFuture()`: an array of
/// per-file `{FileName, Issues[]}` results. A nonzero exit (PHP raises
/// via `resolvex()`) or invalid JSON yields no messages. A result with a
/// null body or null `Issues` stops parsing but keeps messages collected
/// so far, mirroring the PHP early `return`.
#[must_use]
pub fn parse_output(path: &str, exit_code: i32, stdout: &str, _stderr: &str) -> Vec<LintMessage> {
    if exit_code != 0 {
        return Vec::new();
    }
    let Ok(serde_json::Value::Array(results)) = serde_json::from_str::<serde_json::Value>(stdout)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for result in &results {
        let Some(issues) = result.get("Issues") else {
            break;
        };
        let serde_json::Value::Array(issues) = issues else {
            break;
        };
        for issue in issues {
            let Some(message) = parse_issue(path, issue) else {
                continue;
            };
            out.push(message);
        }
    }
    out
}

fn parse_issue(path: &str, issue: &serde_json::Value) -> Option<LintMessage> {
    let index = issue.get("Index")?;
    if !index.is_object() {
        return None;
    }
    let text = |key: &str| {
        index
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
    let params: Vec<String> = issue
        .get("Parameters")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().map(json_scalar).collect())
        .unwrap_or_default();
    let format = text("Message");
    let description = vsprintf(format, &params).unwrap_or_else(|| format.to_string());
    Some(LintMessage {
        path: path.to_string(),
        line: json_position(issue.get("LineNumber")),
        col: json_position(issue.get("Column")),
        code: text("Code").to_string(),
        severity: cslint_severity(index.get("Severity")).to_string(),
        name: text("Name").to_string(),
        description,
    })
}

fn cslint_severity(value: Option<&serde_json::Value>) -> &'static str {
    match value.and_then(serde_json::Value::as_u64) {
        Some(0) => SEVERITY_ADVICE,
        Some(1) => SEVERITY_AUTOFIX,
        Some(2) => SEVERITY_WARNING,
        Some(3) => SEVERITY_ERROR,
        Some(4) => SEVERITY_DISABLED,
        _ => SEVERITY_ADVICE,
    }
}

/// 1-based position from a JSON number or string; missing, unparsable,
/// or zero values map to 1.
fn json_position(value: Option<&serde_json::Value>) -> usize {
    let raw = match value {
        Some(serde_json::Value::Number(number)) => {
            if let Some(n) = number.as_u64() {
                return usize::try_from(n).unwrap_or(1).max(1);
            }
            if let Some(n) = number.as_i64() {
                return usize::try_from(n).unwrap_or(1).max(1);
            }
            number.as_f64().unwrap_or(1.0).to_string()
        }
        Some(serde_json::Value::String(text)) => text.to_string(),
        _ => return 1,
    };
    raw.trim().parse::<usize>().unwrap_or(1).max(1)
}

/// PHP string cast of a JSON scalar for `vsprintf()` substitution.
fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(true) => "1".to_string(),
        serde_json::Value::Bool(false) | serde_json::Value::Null => String::new(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => String::new(),
    }
}

/// Minimal `vsprintf()` for `%s %d %i %u %f %F %c %x %X %o %b %%` with
/// optional positional (`%2$s`) selectors and float precision (`%.2f`).
/// Returns `None` when the format needs a missing parameter, uses an
/// unsupported conversion, or carries flags/width/`*`/non-float precision,
/// in which case the caller falls back to the raw message exactly like the
/// PHP `@vsprintf(...) === false` branch.
fn vsprintf(format: &str, params: &[String]) -> Option<String> {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    let mut sequential = 0;
    while let Some(percent) = rest.find('%') {
        out.push_str(&rest[..percent]);
        rest = &rest[percent + 1..];
        if let Some(stripped) = rest.strip_prefix('%') {
            out.push('%');
            rest = stripped;
            continue;
        }
        if rest.is_empty() {
            // Trailing lone `%` stays literal, like PHP.
            out.push('%');
            break;
        }
        let (positional, after_pos) = take_positional(rest);
        let (spec, precision, after_spec) = take_conversion(after_pos)?;
        if !matches!(
            spec,
            's' | 'd' | 'i' | 'u' | 'f' | 'F' | 'c' | 'x' | 'X' | 'o' | 'b'
        ) {
            return None;
        }
        if precision.is_some() && !matches!(spec, 'f' | 'F') {
            return None;
        }
        let index = positional.unwrap_or_else(|| {
            sequential += 1;
            sequential
        });
        if positional.is_some() {
            sequential = sequential.max(index);
        }
        let param = params.get(index.wrapping_sub(1))?;
        push_conversion(&mut out, spec, param, precision)?;
        rest = after_spec;
    }
    out.push_str(rest);
    Some(out)
}

/// Leading `<n>$` positional marker, returning the 1-based index and the
/// remaining format text.
fn take_positional(text: &str) -> (Option<usize>, &str) {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !digits.starts_with(|c: char| c != '0' && c.is_ascii_digit()) {
        return (None, text);
    }
    if text[digits.len()..].starts_with('$') {
        let index = digits.parse::<usize>().unwrap_or(0);
        if index > 0 {
            return (Some(index), &text[digits.len() + 1..]);
        }
    }
    (None, text)
}

/// Parse one conversion after `%` (and any positional selector),
/// returning `(specifier, precision, rest)`. Flags, widths, and `*`
/// parts decline with `None` so the caller falls back to the raw message.
fn take_conversion(text: &str) -> Option<(char, Option<usize>, &str)> {
    let first = text.chars().next()?;
    // A leading flag or width character means an unsupported padded
    // conversion (positional selectors were consumed earlier), so decline.
    if matches!(first, '-' | '+' | ' ' | '#' | '*') || first.is_ascii_digit() {
        return None;
    }
    let mut head = text;
    let mut precision = None;
    if let Some(after_dot) = head.strip_prefix('.') {
        if after_dot.starts_with('*') {
            return None;
        }
        let digits: String = after_dot.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        precision = digits.parse::<usize>().ok();
        head = &after_dot[digits.len()..];
    }
    let spec = head.chars().next()?;
    Some((spec, precision, &head[spec.len_utf8()..]))
}

fn push_conversion(
    out: &mut String,
    spec: char,
    param: &str,
    precision: Option<usize>,
) -> Option<()> {
    match spec {
        's' => out.push_str(param),
        'd' | 'i' => out.push_str(&format_int(param)),
        'u' => out.push_str(&format_uint(param)),
        'f' | 'F' => out.push_str(&format_float(param, precision.unwrap_or(6))),
        'c' => {
            let code = format_int(param).parse::<u32>().ok()?;
            out.push(char::from_u32(code)?);
        }
        'x' => out.push_str(&format!(
            "{:x}",
            format_int(param).parse::<i64>().unwrap_or(0) as u64
        )),
        'X' => out.push_str(&format!(
            "{:X}",
            format_int(param).parse::<i64>().unwrap_or(0) as u64
        )),
        'o' => out.push_str(&format!(
            "{:o}",
            format_int(param).parse::<i64>().unwrap_or(0) as u64
        )),
        'b' => out.push_str(&format!(
            "{:b}",
            format_int(param).parse::<i64>().unwrap_or(0) as u64
        )),
        _ => return None,
    }
    Some(())
}

/// PHP integer cast: leading-numeric prefix, else float truncation, else 0.
fn php_int(param: &str) -> i64 {
    let trimmed = param.trim_start();
    let (sign, digits_start) = match trimmed.strip_prefix(['+', '-']) {
        Some(rest) => (trimmed.starts_with('-'), rest),
        None => (false, trimmed),
    };
    let digits: String = digits_start
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if !digits.is_empty() {
        let value = digits.parse::<i64>().unwrap_or(i64::MAX);
        return if sign { -value } else { value };
    }
    param.trim().parse::<f64>().unwrap_or(0.0).trunc() as i64
}

fn format_int(param: &str) -> String {
    php_int(param).to_string()
}

fn format_uint(param: &str) -> String {
    (php_int(param) as u64).to_string()
}

fn format_float(param: &str, precision: usize) -> String {
    let value = param.trim().parse::<f64>().unwrap_or(0.0);
    format!("{value:.precision$}")
}

impl Linter for CsharpLinter {
    fn name(&self) -> &'static str {
        "C#"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported(path) {
            return Vec::new();
        }
        if data.contains(&0) {
            return Vec::new();
        }
        let command = resolve_command();
        let borrowed = command
            .as_ref()
            .map(|(program, prefix)| (program.as_path(), prefix.as_slice()));
        lint_with_command(path, data, borrowed)
    }
}

#[cfg(test)]
#[path = "lint_csharp_tests.rs"]
mod lint_csharp_tests;
