//! `RuboCopLinter`: Ruby checks via the external `rubocop` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistRuboCopLinter.php`.
//! Concrete: `getLinterName()` returns `'RuboCop'`, default binary
//! `rubocop`, with the mandatory flag `--format=json`.
//!
//! Behavior: paths that are not Ruby files (`.rb`/`.rake`/`.gemspec`/
//! `.ru` extensions, or `Rakefile`/`Gemfile` basenames), inputs
//! containing NUL bytes, and runs where `rubocop` is absent from `PATH`
//! yield no messages. Otherwise the input bytes are written to a
//! scratch `target.rb` file and `rubocop --format=json <scratch>`
//! stdout is parsed: each JSON offense becomes a message with the
//! offense's line/column, `cop_name` as code, and severity mapped from
//! the offense severity (`convention`/`refactor`/`warning` warning,
//! `error`/`fatal` error, anything else advice). Unlike PHP, which
//! reports the tool-echoed path, the reported path is always the linted
//! path (the tool only ever sees the scratch file). Invalid JSON or
//! malformed offenses yield no messages.
//!
//! The `bin`, `flags`, `version`, and `rubocop.config` linter options
//! apply (N4).

extern crate self as arc_rs;

use arc_rs::arclint::SeverityOverrides;
use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default binary name, mirroring PHP `getDefaultBinary()`.
pub const BINARY: &str = "rubocop";

const SEVERITY_ERROR: &str = "error";
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";

/// Mandatory command flags, mirroring PHP `getMandatoryFlags()`.
#[must_use]
pub fn mandatory_flags() -> Vec<&'static str> {
    vec!["--format=json"]
}

/// Default flags, mirroring `getDefaultFlags()`: none.
fn default_flags() -> Vec<String> {
    Vec::new()
}

/// Mandatory flags as owned strings for flag composition, plus
/// `--config=` when a config file is configured.
fn mandatory_flag_strings(config: Option<&str>) -> Vec<String> {
    let mut flags: Vec<String> = mandatory_flags().into_iter().map(str::to_string).collect();
    if let Some(config) = config {
        flags.push(format!("--config={config}"));
    }
    flags
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

/// Ruby static code analyzer (external `rubocop` wrapper).
#[derive(Debug, Default)]
pub struct RuboCopLinter {
    options: ToolOptions,
    config: Option<String>,
    severity: SeverityOverrides,
    /// Test seam: base directory for the staged temp input (`None` uses
    /// `std::env::temp_dir()`, the production behavior).
    temp_dir: Option<PathBuf>,
}

impl RuboCopLinter {
    /// Create the linter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Linter for RuboCopLinter {
    fn name(&self) -> &'static str {
        "RuboCop"
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
                version_pattern_for("rubocop"),
                tool,
                requirement,
                "ArcanistRuboCopLinter",
            )
        {
            return Vec::new();
        }
        let flags = self.options.command_flags(
            &mandatory_flag_strings(self.config.as_deref()),
            &default_flags(),
        );
        let dir = self.temp_dir.clone().unwrap_or_else(std::env::temp_dir);
        lint_with_binary_and_flags(&dir, path, data, Some(tool), &flags, &self.severity)
    }

    fn applies_custom_severities(&self) -> bool {
        // The PHP linter looks severities up by the offense severity
        // word, not the message code, so overrides apply internally
        // during parsing instead of in the engine.
        false
    }

    fn set_custom_severity_map(&mut self, map: &HashMap<String, String>) {
        self.severity.map.clone_from(map);
    }

    fn set_custom_severity_rules(&mut self, rules: &[(String, String)]) {
        self.severity.rules = rules.to_vec();
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        if key == "rubocop.config" {
            let Some(config) = value.as_str() else {
                return Err("'rubocop.config' must be a string".to_string());
            };
            self.config = Some(config.to_string());
            return Ok(());
        }
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }
}

/// Lint with an explicit tool binary (test seam mirroring bundle E);
/// `None` means the tool is not installed and reports nothing.
pub fn lint_with_binary(path: &str, data: &[u8], binary: Option<&Path>) -> Vec<LintMessage> {
    let dir = std::env::temp_dir();
    lint_with_binary_and_flags(
        &dir,
        path,
        data,
        binary,
        &mandatory_flag_strings(None),
        &SeverityOverrides::default(),
    )
}

/// Linter body with explicit flags (configured flags replace defaults).
fn lint_with_binary_and_flags(
    dir: &Path,
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
    let Some(scratch) = write_scratch(dir, "rubocop", ".rb", path, data) else {
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
        messages = parse_output_with(path, &String::from_utf8_lossy(&output.stdout), overrides);
    }
    scratch.cleanup();
    messages
}

/// Parse `rubocop --format=json` stdout into messages for `path`,
/// mirroring PHP `parseLinterOutput()`. Invalid JSON yields no
/// messages; malformed offenses are skipped.
#[must_use]
pub fn parse_output(path: &str, stdout: &str) -> Vec<LintMessage> {
    parse_output_with(path, stdout, &SeverityOverrides::default())
}

/// Parse with custom severity overrides applied by offense severity
/// word, like PHP `getLintMessageSeverity($offense['severity'])`.
#[must_use]
pub fn parse_output_with(
    path: &str,
    stdout: &str,
    overrides: &SeverityOverrides,
) -> Vec<LintMessage> {
    let mut out = Vec::new();
    let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return Vec::new();
    };
    let Some(files) = json.get("files").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    for file in files {
        let Some(offenses) = file.get("offenses").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for offense in offenses {
            if let Some(message) = parse_offense(path, offense, overrides) {
                out.push(message);
            }
        }
    }
    out
}

fn parse_offense(
    path: &str,
    offense: &serde_json::Value,
    overrides: &SeverityOverrides,
) -> Option<LintMessage> {
    let description = offense.get("message")?.as_str()?.to_string();
    let location = offense.get("location")?;
    let line = location.get("line")?.as_u64()? as usize;
    let col = location.get("column")?.as_u64()? as usize;
    let severity_word = offense.get("severity")?.as_str()?;
    let code = offense.get("cop_name")?.as_str()?;
    Some(LintMessage {
        path: path.to_string(),
        line,
        col,
        code: code.to_string(),
        severity: overrides.severity_for(severity_word, severity_for(severity_word)),
        name: "RuboCop".to_string(),
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

fn severity_for(word: &str) -> &'static str {
    match word {
        "convention" | "refactor" | "warning" => SEVERITY_WARNING,
        "error" | "fatal" => SEVERITY_ERROR,
        _ => SEVERITY_ADVICE,
    }
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
fn write_scratch(
    dir: &Path,
    slug: &str,
    suffix: &str,
    display: &str,
    data: &[u8],
) -> Option<Scratch> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = dir.join(format!("arc-rs-lint-{slug}-{}-{n}", std::process::id()));
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
#[path = "lint_rubocop_tests.rs"]
mod lint_rubocop_tests;
