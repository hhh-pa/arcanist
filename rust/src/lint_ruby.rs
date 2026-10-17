//! `RubyLinter`: Ruby syntax checks via the external `ruby` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistRubyLinter.php`. Concrete:
//! `getLinterName()` returns `'RUBY'`, default binary `ruby`, with
//! mandatory flags `-w -c` (warnings on, syntax check only).
//!
//! Behavior: paths that are not Ruby files (`.rb`/`.rake`/`.gemspec`/
//! `.ru` extensions, or `Rakefile`/`Gemfile` basenames), inputs
//! containing NUL bytes, and runs where `ruby` is absent from `PATH`
//! yield no messages. Otherwise the input bytes are written to a
//! scratch `target.rb` file and `ruby -w -c <scratch>` stderr lines of
//! the form `path:line: message` are parsed (the match is unanchored,
//! like the PHP regex, so the leftmost `:digits: ` split wins).
//! Messages carry code `RUBY`, name `Syntax Error`, and error severity
//! (the PHP linter uses the base-class default severity); the column
//! is always 1 since the tool reports none.
//!
//! The `bin`, `flags`, and `version` linter options apply (N4).

extern crate self as arc_rs;

use arc_rs::arclint::SeverityOverrides;
use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default binary name, mirroring PHP `getDefaultBinary()`.
pub const BINARY: &str = "ruby";

const SEVERITY_ERROR: &str = "error";

/// Mandatory command flags, mirroring PHP `getMandatoryFlags()`.
#[must_use]
pub fn mandatory_flags() -> Vec<&'static str> {
    vec!["-w", "-c"]
}

/// Default flags, mirroring `getDefaultFlags()`: none.
fn default_flags() -> Vec<String> {
    Vec::new()
}

/// Mandatory flags as owned strings for flag composition.
fn mandatory_flag_strings() -> Vec<String> {
    mandatory_flags().into_iter().map(str::to_string).collect()
}

/// True for paths this linter applies to: Ruby files.
#[must_use]
pub fn is_supported_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if base == "Rakefile" || base == "Gemfile" {
        return true;
    }
    let Some((_, ext)) = base.rsplit_once('.') else {
        return false;
    };
    ext.eq_ignore_ascii_case("rb")
        || ext.eq_ignore_ascii_case("rake")
        || ext.eq_ignore_ascii_case("gemspec")
        || ext.eq_ignore_ascii_case("ru")
}

/// Ruby syntax linter (external `ruby -w -c` wrapper).
#[derive(Debug, Default)]
pub struct RubyLinter {
    options: ToolOptions,
    severity: SeverityOverrides,
}

impl RubyLinter {
    /// Create the linter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Linter for RubyLinter {
    fn name(&self) -> &'static str {
        "RUBY"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported_path(path) || data.contains(&0) {
            return Vec::new();
        }
        let tool = self.options.bin.clone().or_else(|| binary_on_path(BINARY));
        let Some(ref tool) = tool else {
            return Vec::new();
        };
        if let Some(requirement) = &self.options.version
            && !version_requirement_satisfied(
                version_pattern_for("ruby"),
                tool,
                requirement,
                "ArcanistRubyLinter",
            )
        {
            return Vec::new();
        }
        let flags = self
            .options
            .command_flags(&mandatory_flag_strings(), &default_flags());
        lint_with_binary_and_flags(path, data, Some(tool), &flags, &self.severity)
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }

    fn applies_custom_severities(&self) -> bool {
        // The PHP linter looks severities up by the message head (text
        // before the first comma), not the message code, so overrides
        // apply internally during parsing instead of in the engine.
        false
    }

    fn set_custom_severity_map(&mut self, map: &HashMap<String, String>) {
        self.severity.map.clone_from(map);
    }

    fn set_custom_severity_rules(&mut self, rules: &[(String, String)]) {
        self.severity.rules = rules.to_vec();
    }
}

/// Lint with an explicit tool binary (test seam mirroring bundle E);
/// `None` means the tool is not installed and reports nothing.
pub fn lint_with_binary(path: &str, data: &[u8], binary: Option<&Path>) -> Vec<LintMessage> {
    lint_with_binary_and_flags(
        path,
        data,
        binary,
        &mandatory_flag_strings(),
        &SeverityOverrides::default(),
    )
}

/// Linter body with explicit flags (configured flags replace defaults).
fn lint_with_binary_and_flags(
    path: &str,
    data: &[u8],
    binary: Option<&Path>,
    flags: &[String],
    overrides: &SeverityOverrides,
) -> Vec<LintMessage> {
    let Some(tool) = binary else {
        // Inactive when the tool is not installed, mirroring PHP.
        return Vec::new();
    };
    let Some(scratch) = write_scratch("ruby", ".rb", path, data) else {
        return Vec::new();
    };
    let mut command = crate::proc_env::command(tool);
    command
        .args(flags)
        .arg(&scratch.file)
        .current_dir(&scratch.cwd);
    let output = arc_rs::lint::spawn_with_retry(&mut command);
    let mut messages = Vec::new();
    if let Ok(output) = output {
        messages = parse_output_with(path, &String::from_utf8_lossy(&output.stderr), overrides);
    }
    scratch.cleanup();
    messages
}

/// Parse `ruby -w -c` stderr into messages for `path`, mirroring PHP
/// `parseLinterOutput()`. Unparseable lines are skipped.
#[must_use]
pub fn parse_output(path: &str, stderr: &str) -> Vec<LintMessage> {
    parse_output_with(path, stderr, &SeverityOverrides::default())
}

/// Parse with custom severity overrides applied by message head (text
/// before the first comma), like PHP
/// `getLintMessageSeverity(head(explode(',', $message)))`.
#[must_use]
pub fn parse_output_with(
    path: &str,
    stderr: &str,
    overrides: &SeverityOverrides,
) -> Vec<LintMessage> {
    let mut out = Vec::new();
    for raw in stderr.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(message) = parse_line(path, line, overrides) {
            out.push(message);
        }
    }
    out
}

/// Try each `:` left to right (the PHP regex is unanchored with a lazy
/// path group, so the leftmost viable split wins).
fn parse_line(path: &str, line: &str, overrides: &SeverityOverrides) -> Option<LintMessage> {
    for (index, _) in line.char_indices().filter(|(_, c)| *c == ':') {
        if let Some(message) = parse_at(path, line, index, overrides) {
            return Some(message);
        }
    }
    None
}

fn parse_at(
    path: &str,
    line: &str,
    colon: usize,
    overrides: &SeverityOverrides,
) -> Option<LintMessage> {
    let after_path = line.get(colon + 1..)?;
    let (line_no, rest) = take_digits(after_path)?;
    let description = rest.strip_prefix(": ")?.trim().to_string();
    let head = description.split(',').next().unwrap_or(&description);
    Some(LintMessage {
        path: path.to_string(),
        line: line_no,
        col: 1,
        code: "RUBY".to_string(),
        severity: overrides.severity_for(head, SEVERITY_ERROR),
        name: "Syntax Error".to_string(),
        description,
        original: None,
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    })
}

/// Split leading ASCII digits into a number plus the remainder.
fn take_digits(text: &str) -> Option<(usize, &str)> {
    let end = text.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    let value = text[..end].parse::<usize>().ok()?;
    Some((value, &text[end..]))
}

/// Look `name` up on `PATH`, returning the tool path when present.
fn binary_on_path(name: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(name);
    if direct.components().count() > 1 {
        if direct.is_file() {
            return Some(direct);
        }
        return None;
    }
    std::env::var_os("PATH").and_then(|path| binary_in(&path))
}

/// Look the tool up in `path_env` (test seam; production passes `$PATH`).
fn binary_in(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join(BINARY);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let pathext = std::env::var_os("PATHEXT")
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".EXE;.BAT;.CMD;.COM".to_string());
            for ext in pathext.split(';') {
                let ext = ext.trim();
                if ext.is_empty() {
                    continue;
                }
                let mut probed = candidate.clone();
                probed.as_mut_os_string().push(ext);
                if probed.is_file() {
                    return Some(probed);
                }
            }
        }
    }
    None
}

struct Scratch {
    dir: PathBuf,
    file: PathBuf,
    cwd: PathBuf,
}

impl Scratch {
    fn cleanup(self) {
        std::fs::remove_file(&self.file).ok();
        std::fs::remove_dir(&self.dir).ok();
    }
}

/// Write `data` to a unique temp file with the tool's expected suffix.
/// The tool runs with the linted file's directory as its working
/// directory when that directory exists (so tool config files apply,
/// mirroring PHP running from the project root), else the temp dir.
fn write_scratch(slug: &str, suffix: &str, display: &str, data: &[u8]) -> Option<Scratch> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("arc-rs-lint-{slug}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("target{suffix}"));
    std::fs::write(&file, data).ok()?;
    let cwd = Path::new(display)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty() && parent.is_dir())
        .map_or_else(|| dir.clone(), Path::to_path_buf);
    Some(Scratch { dir, file, cwd })
}

#[cfg(test)]
#[path = "lint_ruby_tests.rs"]
mod lint_ruby_tests;
