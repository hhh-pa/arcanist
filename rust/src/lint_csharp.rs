//! `arc lint` C# linter (T1128).
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
//! Known limits (see T1128): only one file is passed per invocation
//! (PHP batches paths up to the command-line limit); a nonzero tool exit
//! or invalid JSON yields no messages (the PHP linter raises; unfiled,
//! unlike the load failures below); reported paths are the display paths
//! passed to [`lint_bytes`](arc_rs::lint::Linter::lint_bytes), never the
//! staged temp-file paths. A missing tool, a missing Mono runtime, or a
//! cslint version other than 1 fails loudly through
//! [`Linter::take_error`](arc_rs::lint::Linter::take_error) for the
//! engine's aggregate, like PHP's `loadEnvironment()` (T1153 LINT-32).
//! The `binary` hint resolves against the project root (PHP resolves
//! against the process working directory). Discovery-map keys serialize
//! in sorted order (the JSON map preserves no insertion order here).

extern crate self as arc_rs;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use arc_rs::lint::{LintMessage, Linter, OptionContext, SEVERITY_AUTOFIX, SEVERITY_ERROR};

/// Companion severities mirroring `ArcanistLintSeverity`; `lint.rs` only
/// defines the severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";
const SEVERITY_DISABLED: &str = "disabled";

/// C# linter: `cslint.exe` findings via an external binary.
#[derive(Debug, Default)]
pub struct CsharpLinter {
    binary_hint: Option<PathBuf>,
    discovery: Option<BTreeMap<String, Vec<String>>>,
    error: RefCell<Option<String>>,
}

impl CsharpLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the tool honoring the configured `binary` hint and
    /// discovery map, mirroring `loadEnvironment()` (T1153 LINT-32):
    /// the runtime comes first, then the tool, then the version probe,
    /// and every failure reports PHP's text for the aggregate.
    fn resolve(&self) -> Result<(PathBuf, Vec<String>), String> {
        self.resolve_in(&std::env::var_os("PATH").unwrap_or_default())
    }

    /// [`resolve`](Self::resolve) against an explicit `PATH` value, so
    /// tests need not depend on the ambient environment.
    fn resolve_in(&self, path_env: &std::ffi::OsStr) -> Result<(PathBuf, Vec<String>), String> {
        let path = path_env.to_os_string();
        #[cfg(not(windows))]
        let mono = find_on_path(&path, runtime_name())
            .ok_or_else(|| "Unable to find Mono and you are not on Windows!".to_string())?;
        let settings = settings_base64(self.discovery.as_ref());
        let cslint = self
            .binary_hint
            .as_deref()
            .filter(|hint| hint.is_file())
            .map(Path::to_path_buf)
            .or_else(|| find_on_path(&path, binary_name()))
            .ok_or_else(|| "Unable to locate cslint.".to_string())?;
        // The only mutation is the non-Windows `insert` below; without
        // this the `mut` is unused on Windows (T1170).
        #[cfg_attr(windows, allow(unused_mut))]
        let mut prefix = command_flags_for(&settings);
        #[cfg(not(windows))]
        prefix.insert(0, cslint.to_string_lossy().into_owned());
        #[cfg(not(windows))]
        let program = mono;
        #[cfg(windows)]
        let program = cslint;
        check_version(&program, &prefix)?;
        Ok((program, prefix))
    }
}

/// `SUPPORTED_VERSION`: only cslint 1 is accepted, like PHP.
const SUPPORTED_VERSION: i64 = 1;

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
    command_flags_for(default_settings_base64())
}

/// Same, for an explicit `--settings-base64` payload.
#[must_use]
pub fn command_flags_for(settings: &str) -> Vec<String> {
    vec![format!("--settings-base64={settings}"), "-r=.".to_string()]
}

/// `--settings-base64` payload: base64 of the discovery-map JSON, or
/// base64(`null`) without a configured map, like PHP.
#[must_use]
pub fn settings_base64(discovery: Option<&BTreeMap<String, Vec<String>>>) -> String {
    match discovery {
        None => default_settings_base64().to_string(),
        Some(map) => base64_encode(discovery_json(map).as_bytes()),
    }
}

/// Discovery map rendered exactly like PHP `json_encode()` (compact,
/// `/` escaped, non-ASCII as `\uXXXX`), keys in sorted order.
fn discovery_json(map: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::from("{");
    for (index, (key, values)) in map.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&php_json_escape(key));
        out.push_str("\":[");
        for (item_index, value) in values.iter().enumerate() {
            if item_index > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(&php_json_escape(value));
            out.push('"');
        }
        out.push(']');
    }
    out.push('}');
    out
}

/// PHP `json_encode()` string escaping for valid UTF-8 input.
fn php_json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        push_json_char(&mut out, ch);
    }
    out
}

/// Push one char with PHP `json_encode()` escaping: structural
/// punctuation here, controls and non-ASCII below.
fn push_json_char(out: &mut String, ch: char) {
    match ch {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '/' => out.push_str("\\/"),
        _ => push_json_escape_tail(out, ch),
    }
}

/// Push one char that is not structural JSON punctuation: short
/// escapes, `\u00XX` controls, ASCII passthrough, or UTF-16
/// `\uXXXX` pairs.
fn push_json_escape_tail(out: &mut String, ch: char) {
    match ch {
        '\u{08}' => out.push_str("\\b"),
        '\u{0C}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
            out.push_str(&format!("\\u{:04x}", ch as u32));
        }
        ch if (ch as u32) < 0x80 => out.push(ch),
        ch => {
            let mut encoded = [0u16; 2];
            for unit in ch.encode_utf16(&mut encoded) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
}

/// Standard base64 with padding, byte-for-byte like PHP
/// `base64_encode()`.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut word: u32 = 0;
        for (index, byte) in chunk.iter().enumerate() {
            word |= u32::from(*byte) << (8 * (2 - index));
        }
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((word >> (6 * (3 - index))) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Resolved `(program, prefix_argv)` for a `$PATH`-style search string:
/// `cslint.exe` directly on Windows, `mono cslint.exe` elsewhere. A
/// `binary` hint pointing at an existing file wins over the `PATH`
/// lookup, like PHP. `None` means the linter is inactive. This is the
/// pure lookup half of `loadEnvironment()`; the version probe runs
/// separately in [`check_version`].
fn resolve_command_in(
    path_env: &std::ffi::OsStr,
    hint: Option<&Path>,
    settings: &str,
) -> Option<(PathBuf, Vec<String>)> {
    let cslint = hint
        .filter(|path| path.is_file())
        .map(Path::to_path_buf)
        .or_else(|| find_on_path(path_env, binary_name()))?;
    #[cfg(windows)]
    let out = {
        let prefix = command_flags_for(settings);
        Some((cslint, prefix))
    };
    #[cfg(not(windows))]
    let out = {
        let mut prefix = command_flags_for(settings);
        let mono = find_on_path(path_env, runtime_name())?;
        prefix.insert(0, cslint.to_string_lossy().into_owned());
        Some((mono, prefix))
    };
    out
}

/// Resolve the tool on the real `$PATH` with default settings; `None`
/// means the linter is inactive and reports no messages. Test seam;
/// linting goes through the version-gated `CsharpLinter::resolve`.
#[must_use]
pub fn resolve_command() -> Option<(PathBuf, Vec<String>)> {
    let path = std::env::var_os("PATH")?;
    resolve_command_in(&path, None, default_settings_base64())
}

/// Probe `<program> -v` (plus the cslint path under Mono, which is
/// `prefix[0]` there by construction), mirroring the `loadEnvironment()`
/// gate (T1153 LINT-32): a failing probe or an older version reports
/// PHP's upgrade text, a newer version the Arcanist-upgrade text.
fn check_version(program: &Path, prefix: &[String]) -> Result<(), String> {
    let mut command = crate::proc_env::command(program);
    if !cfg!(windows) {
        let Some(cslint) = prefix.first() else {
            return Err(old_version_error());
        };
        command.arg(cslint);
    }
    command.arg("-v").stdin(Stdio::null());
    let Ok(output) = arc_rs::lint::spawn_with_retry(&mut command) else {
        return Err(old_version_error());
    };
    let code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if version_output_supported(code, &stdout) {
        return Ok(());
    }
    // PHP reports newer versions with a distinct text.
    if code == 0 && php_int_cast(&stdout) > SUPPORTED_VERSION {
        return Err(
            "Arcanist does not support this version of cslint (it is newer). \
             You can try upgrading Arcanist with `arc upgrade`."
                .to_string(),
        );
    }
    Err(old_version_error())
}

/// PHP's old-cslint text (`loadEnvironment()`).
fn old_version_error() -> String {
    format!(
        "You are running an old version of cslint. Please upgrade to version {SUPPORTED_VERSION}."
    )
}

/// Pure half of the version gate: the probe must exit 0 and its stdout
/// must cast to [`SUPPORTED_VERSION`], like `(int)$stdout`.
fn version_output_supported(exit_code: i32, stdout: &str) -> bool {
    if exit_code != 0 {
        return false;
    }
    php_int_cast(stdout) == SUPPORTED_VERSION
}

/// PHP `(int)` cast: leading whitespace, optional sign, digit run.
fn php_int_cast(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (negative, rest) = match trimmed.strip_prefix(['+', '-']) {
        Some(rest) => (trimmed.starts_with('-'), rest),
        None => (false, trimmed),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return 0;
    }
    let value = digits.parse::<i64>().unwrap_or(i64::MAX);
    if negative { -value } else { value }
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
    let mut command = crate::proc_env::command(program);
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
        return Vec::new(); // LCOV_EXCL_LINE // Reason: staging failure; hermetic tests cannot fail temp-dir writes
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
        original: issue
            .get("OriginalText")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        replacement: issue
            .get("ReplacementText")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
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
        *self.error.borrow_mut() = None;
        if !is_supported(path) {
            return Vec::new();
        }
        if data.contains(&0) {
            return Vec::new();
        }
        let command = match self.resolve() {
            Ok(command) => command, // LCOV_EXCL_LINE // Reason: resolve-Ok arm; needs cslint plus mono, absent on ubuntu CI
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                return Vec::new();
            }
        };
        let borrowed = Some((command.0.as_path(), command.1.as_slice())); // LCOV_EXCL_START // Reason: resolve-Ok arm; needs cslint plus mono, absent on ubuntu CI
        lint_with_command(path, data, borrowed) // LCOV_EXCL_STOP
    }

    fn take_error(&self) -> Option<String> {
        self.error.borrow_mut().take()
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        match key {
            "binary" => {
                let Some(hint) = value.as_str() else {
                    return Err("'binary' must be a string".to_string());
                };
                self.binary_hint = Some(ctx.project_root.join(hint));
                Ok(())
            }
            "discovery" => {
                self.discovery = Some(read_discovery_map(value));
                Ok(())
            }
            _ => Err(format!("unknown option '{key}'.")),
        }
    }

    fn validate_severity_entry(&self, code: &str, severity: &str) -> Result<String, String> {
        // StyleCop rules may not be disabled, like PHP's
        // `setCustomSeverityMap()` veto (ceb wording, T1153 LINT-31).
        if code.starts_with("SA") && severity == "disabled" {
            return Err(
                "In order to keep StyleCop integration with IDEs and other tools consistent \
                 with Arcanist results, you aren't permitted to disable StyleCop rules within \
                 '.arclint'. Instead configure the severity using the StyleCop settings dialog \
                 (usually accessible from within your IDE). StyleCop settings for your project \
                 will be used when linting for Arcanist."
                    .to_string(),
            );
        }
        self.validate_severity_code(code)
    }
}

/// Read a `map<string, list<string>>` discovery map, skipping malformed
/// entries (the engine type-checks the shape first).
fn read_discovery_map(value: &serde_json::Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Some(entries) = value.as_object() else {
        return out;
    };
    for (key, items) in entries {
        let Some(items) = items.as_array() else {
            continue;
        };
        let values: Vec<String> = items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect();
        out.insert(key.clone(), values);
    }
    out
}

#[cfg(test)]
#[path = "lint_csharp_tests.rs"]
mod lint_csharp_tests;
