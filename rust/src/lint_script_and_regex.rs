//! `script-and-regex` linter: run a script, parse output (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistScriptAndRegexLinter.php`.
//!
//! Scope: the named-group to message semantics are ported exactly
//! ([`parse_output_with`]): `message`/`name`/`code`/`file` defaults,
//! the severity mapping, `line`/`char`/`offset` handling, and the
//! `ignore`/`stop`/`halt`/`throw` control groups. Configuration (the script
//! command and the output parser) is constructor-provided; the default
//! linter is unconfigured and stays silent until engine config plumbing
//! lands.
//!
//! Out of scope / divergences:
//!
//! - PCRE matching itself is the caller's job ([`OutputParser`]): the
//!   dependency set has no regex engine, so callers port their
//!   `script-and-regex.regex` to a matcher function returning
//!   [`ScriptRegexMatch`] values.
//! - `halt` (stop every linter on this file) behaves like `stop` (stop this
//!   linter on this file): `lint_bytes` has no engine handle.
//! - Script failures, a missing script binary, and `throw` matches make
//!   `lint_bytes` return no messages (PHP raises); the fallible
//!   [`parse_output_with`] surfaces `throw`, and [`run_script_for_path`] is
//!   available to direct callers.
//! - `original`/`replacement` groups are dropped ([`LintMessage`] has no
//!   such fields). Whole-file messages use line/column 1 (the Rust message
//!   has no null line).

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter, SEVERITY_AUTOFIX, SEVERITY_ERROR};

/// Default message code (the PHP linter name) when no `code` group matches.
pub const CODE_DEFAULT: &str = "S&RX";
/// Default message name when no `name` group matches.
pub const NAME_DEFAULT: &str = "Lint";
/// Default description when no `message` group matches.
pub const MESSAGE_DEFAULT: &str = "Undefined Lint Message";
/// Extra severities beyond [`SEVERITY_ERROR`] and [`SEVERITY_AUTOFIX`].
pub const SEVERITY_WARNING: &str = "warning";
/// Advice severity, mirroring `ArcanistLintSeverity`.
pub const SEVERITY_ADVICE: &str = "advice";
/// Disabled severity, mirroring `ArcanistLintSeverity`.
pub const SEVERITY_DISABLED: &str = "disabled";

/// Named groups captured from one script-output match.
///
/// Each field mirrors a `(?P<name>...)` group from the PHP docs. `None` (or
/// an empty string) means the group did not participate, matching PHP's
/// `empty($match[...])` / `idx(...)` checks.
#[derive(Debug, Clone, Default)]
pub struct ScriptRegexMatch {
    /// `message` group: text describing the finding.
    pub message: Option<String>,
    /// `name` group: one-line summary.
    pub name: Option<String>,
    /// `severity` group: error/warning/autofix/advice/disabled.
    pub severity: Option<String>,
    /// `error` group: nonempty means error severity.
    pub error: Option<String>,
    /// `warning` group: nonempty means warning severity.
    pub warning: Option<String>,
    /// `advice` group: nonempty means advice severity.
    pub advice: Option<String>,
    /// `autofix` group: nonempty means autofix severity.
    pub autofix: Option<String>,
    /// `disabled` group: nonempty means disabled severity.
    pub disabled: Option<String>,
    /// `file` group: file to raise the message in (defaults to the file).
    pub file: Option<String>,
    /// `line` group: 1-based line number.
    pub line: Option<String>,
    /// `char` group: 1-based column.
    pub char: Option<String>,
    /// `offset` group: byte offset (supersedes `line` and `char`).
    pub offset: Option<String>,
    /// `code` group: short error type identifier.
    pub code: Option<String>,
    /// `ignore` group: nonempty skips this match.
    pub ignore: Option<String>,
    /// `stop` group: nonempty stops processing this file.
    pub stop: Option<String>,
    /// `halt` group: nonempty stops processing this file (see module docs).
    pub halt: Option<String>,
    /// `throw` group: nonempty aborts with an error.
    pub throw: Option<String>,
}

impl ScriptRegexMatch {
    /// Match carrying only a message.
    #[must_use]
    pub fn message_only(text: &str) -> Self {
        Self {
            message: Some(text.to_string()),
            ..Self::default()
        }
    }
}

/// Parses script stdout into regex matches (the caller's PCRE equivalent).
pub type OutputParser = fn(&str) -> Vec<ScriptRegexMatch>;

/// Config-driven script-and-regex linter. Unconfigured by default (silent).
#[derive(Debug, Clone, Default)]
pub struct ScriptAndRegexLinter {
    script: Option<String>,
    parser: Option<OutputParser>,
}

impl ScriptAndRegexLinter {
    /// Unconfigured linter: reports nothing until configured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Linter running `script` with the linted path appended, parsing
    /// stdout with `parser`. The script runs from the process working
    /// directory (PHP runs it from the project root).
    #[must_use]
    pub fn with_script_and_parser(script: &str, parser: OutputParser) -> Self {
        Self {
            script: Some(script.to_string()),
            parser: Some(parser),
        }
    }
}

impl Linter for ScriptAndRegexLinter {
    fn name(&self) -> &'static str {
        "S&RX"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        let (Some(script), Some(parser)) = (self.script.as_deref(), self.parser) else {
            return Vec::new();
        };
        if data.contains(&0) || script.trim().is_empty() {
            return Vec::new();
        }
        let argv0 = script.split_whitespace().next().unwrap_or("");
        if !binary_exists(argv0) {
            return Vec::new();
        }
        let Ok(output) = run_script_for_path(script, path) else {
            return Vec::new();
        };
        parse_output_with(parser, &output, path, data).unwrap_or_default()
    }
}

/// Parse script `output` into messages, applying the PHP named-group rules.
///
/// `path` is the linted file (the default message file) and `data` its
/// bytes (for `offset` resolution). Returns `Err` when a match captures a
/// nonempty `throw` group, mirroring the PHP exception.
pub fn parse_output_with(
    parser: OutputParser,
    output: &str,
    path: &str,
    data: &[u8],
) -> Result<Vec<LintMessage>, String> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for captured in parser(output) {
        if let Some(thrown) = nonempty(&captured.throw) {
            return Err(format!(
                "ArcanistScriptAndRegexLinter: configuration captured a 'throw' \
                 named capturing group, '{thrown}'. Script output:\n{output}"
            ));
        }
        if nonempty(&captured.halt).is_some() || nonempty(&captured.stop).is_some() {
            break;
        }
        if nonempty(&captured.ignore).is_some() {
            continue;
        }
        let (line, col) = match_line_and_char(&captured, data);
        out.push(LintMessage {
            path: nonempty(&captured.file).unwrap_or(path).to_string(),
            line,
            col,
            code: nonempty(&captured.code).unwrap_or(CODE_DEFAULT).to_string(),
            severity: match_severity(&captured).to_string(),
            name: nonempty(&captured.name).unwrap_or(NAME_DEFAULT).to_string(),
            description: nonempty(&captured.message)
                .unwrap_or(MESSAGE_DEFAULT)
                .to_string(),
        });
    }
    Ok(out)
}

/// Severity for one match, ported exactly from `getMatchSeverity`.
///
/// Note the PHP quirks this preserves: when no `severity` group matched,
/// the result is `error` without consulting the `error`/`warning`/...
/// groups (despite the PHP docblock suggesting otherwise), and named
/// groups beat the `severity` value only when they come earlier in map
/// order (`error`, `warning`, `autofix`, `advice`, `disabled`).
pub fn match_severity(captured: &ScriptRegexMatch) -> &'static str {
    let severity_name = nonempty(&captured.severity).unwrap_or("");
    if severity_name.is_empty() {
        return SEVERITY_ERROR;
    }
    let severity_name = severity_name.to_ascii_lowercase();
    let groups = [
        (&captured.error, "error", SEVERITY_ERROR),
        (&captured.warning, "warning", SEVERITY_WARNING),
        (&captured.autofix, "autofix", SEVERITY_AUTOFIX),
        (&captured.advice, "advice", SEVERITY_ADVICE),
        (&captured.disabled, "disabled", SEVERITY_DISABLED),
    ];
    for (group, name, severity) in groups {
        if nonempty(group).is_some() {
            return severity;
        }
        if severity_name == name {
            return severity;
        }
    }
    SEVERITY_ERROR
}

/// 1-based (line, col) for one match, ported from `getMatchLineAndChar`.
///
/// A nonempty `offset` group wins (parsed like a PHP `(int)` cast, so
/// garbage means byte 0); otherwise `line` parses as an integer with
/// non-positive values clamped to 1 (PHP maps 0 to 1) and missing values
/// defaulting to 1; `char` follows PHP truthiness (`"0"` counts as absent)
/// with the same 1-based clamping.
pub fn match_line_and_char(captured: &ScriptRegexMatch, data: &[u8]) -> (usize, usize) {
    if let Some(offset) = nonempty(&captured.offset) {
        let byte = php_int(offset).max(0) as usize;
        return line_col_for_offset(data, byte);
    }
    let line = match nonempty(&captured.line) {
        Some(text) => php_int(text).max(1) as usize,
        None => 1,
    };
    let col = match captured.char.as_deref() {
        Some(text) if !text.is_empty() && text != "0" => php_int(text).max(1) as usize,
        _ => 1,
    };
    (line, col)
}

/// True when a binary name or path resolves (PATH lookup for bare names).
pub fn binary_exists(candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let path = std::path::Path::new(candidate);
    if candidate.contains('/') || candidate.contains('\\') {
        return is_executable_file(path);
    }
    let Some(dirs) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&dirs) {
        let full = dir.join(candidate);
        if is_executable_file(&full) {
            return true;
        }
        #[cfg(windows)]
        for extension in ["exe", "bat", "cmd", "com"] {
            if is_executable_file(&dir.join(format!("{candidate}.{extension}"))) {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Shell command running `script` with the linted `path` appended.
#[must_use]
pub fn script_command(script: &str, path: &str) -> String {
    format!("{script} {}", shell_quote(path))
}

/// Run `script` with the linted path appended, returning stdout.
///
/// Mirrors `ExecFuture('%C %s', $script, $path)`: stderr is ignored and a
/// nonzero exit is an error.
pub fn run_script_for_path(script: &str, path: &str) -> Result<String, String> {
    let command = script_command(script, path);
    #[cfg(unix)]
    let mut spawned_command = std::process::Command::new("sh");
    #[cfg(windows)]
    let mut spawned_command = std::process::Command::new("cmd");
    #[cfg(unix)]
    spawned_command.arg("-c").arg(&command);
    #[cfg(windows)]
    spawned_command.arg("/C").arg(&command);
    let spawned = arc_rs::lint::spawn_with_retry(&mut spawned_command);
    let output = spawned.map_err(|error| format!("failed to run script: {error}"))?;
    if !output.status.success() {
        return Err(format!("script exited with status {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Quote one shell argument.
fn shell_quote(argument: &str) -> String {
    #[cfg(unix)]
    {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
    #[cfg(windows)]
    {
        format!("\"{}\"", argument.replace('"', "\"\""))
    }
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|text| !text.is_empty())
}

/// PHP `(int)` cast: leading whitespace, optional sign, digit prefix.
fn php_int(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (digits, negative) = match trimmed.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (trimmed.strip_prefix('+').unwrap_or(trimmed), false),
    };
    let mut value: i64 = 0;
    let mut any = false;
    for digit in digits.chars().take_while(char::is_ascii_digit) {
        any = true;
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(digit as u8 - b'0'));
    }
    if !any {
        return 0;
    }
    if negative { -value } else { value }
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

#[cfg(test)]
#[path = "lint_script_and_regex_tests.rs"]
mod lint_script_and_regex_tests;
