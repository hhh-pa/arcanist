//! External-linter base mechanics (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistExternalLinter.php`.
//!
//! IMPORTANT: the PHP class is `abstract` and defines NO linter name (it
//! has neither `getLinterName` nor `getLinterConfigurationName`, while the
//! base class declares `abstract public function getLinterName()` in
//! `src/lint/linter/ArcanistLinter.php`). It is a base class for concrete
//! tool linters, never a registered linter itself. Per the bundle rules,
//! this module therefore ports only the reusable base mechanics (binary
//! resolution, flag composition, version requirements, output-resolution
//! semantics) plus a generic config-driven runner. The `"external"` name
//! below is a local identifier for that runner: DO NOT register it in
//! `all_linters()`.
//!
//! Scope: [`ExternalToolConfig`] (binary/interpreter candidates, flags,
//! version requirement), [`check_binary_configuration`],
//! [`check_version_requirement`] (with an approximate [`version_compare`]),
//! and [`resolve_output`] (the `resolveFuture` semantics). The default
//! runner is unconfigured and stays silent; a missing binary also stays
//! silent (inactive, mirroring PHP).
//!
//! Out of scope / divergences: CWD is the process working directory (PHP
//! uses the project root); lookup failures and parse failures inside
//! `lint_bytes` stay silent (PHP raises; the fallible helpers surface
//! them); version comparison approximates PHP `version_compare` (numeric
//! segments compare numerically, known pre-release forms keep PHP order,
//! anything else falls back to byte comparison).

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};
use std::cmp::Ordering;

/// Local identifier for the generic runner. There is no exact arc linter
/// name for this module (the PHP class is abstract and nameless), so this
/// must never appear in the linter registry.
pub const NAME_LOCAL: &str = "external";

/// Configured `bin` / `flags` / `version` options shared by every
/// external linter (N4). Linters store one of these, apply `.arclint`
/// values through [`apply_tool_option`], and read it back when
/// composing argv.
#[derive(Debug, Clone, Default)]
pub struct ToolOptions {
    /// Configured binary (resolved at config time), else the default.
    pub bin: Option<std::path::PathBuf>,
    /// Configured flags, replacing defaults when non-empty (PHP
    /// `nonempty($this->flags, $this->getDefaultFlags())`: an
    /// explicitly empty list keeps the defaults).
    pub flags: Option<Vec<String>>,
    /// Trimmed version requirement (`setVersionRequirement()`).
    pub version: Option<String>,
}

impl ToolOptions {
    /// Effective flags: mandatory plus configured-or-default, mirroring
    /// `getCommandFlags()`.
    #[must_use]
    pub fn command_flags(&self, mandatory: &[String], defaults: &[String]) -> Vec<String> {
        let mut out = mandatory.to_vec();
        match &self.flags {
            Some(flags) if !flags.is_empty() => out.extend(flags.iter().cloned()),
            _ => out.extend(defaults.iter().cloned()),
        }
        out
    }
}

/// Apply one tool option (`bin`, `flags`, `version`) to `options`,
/// mirroring `ArcanistExternalLinter::setLinterConfigurationValue()`.
/// Values are already type-checked by the engine. `project_root`
/// resolves relative `bin` candidates, like PHP.
pub fn apply_tool_option(
    options: &mut ToolOptions,
    key: &str,
    value: &serde_json::Value,
    project_root: &std::path::Path,
) -> Result<(), String> {
    match key {
        "bin" => {
            options.bin = Some(resolve_configured_bin(value, project_root)?);
            Ok(())
        }
        "flags" => {
            let serde_json::Value::Array(flags) = value else {
                return Err("'flags' must be a list of strings".to_string());
            };
            let mut out = Vec::new();
            for flag in flags {
                let serde_json::Value::String(flag) = flag else {
                    return Err("'flags' must be a list of strings".to_string());
                };
                out.push(flag.clone());
            }
            options.flags = Some(out);
            Ok(())
        }
        "version" => {
            let serde_json::Value::String(version) = value else {
                return Err("'version' must be a string".to_string());
            };
            options.version = Some(version.trim().to_string());
            Ok(())
        }
        _ => Err(format!("unknown option '{key}'.")),
    }
}

/// Resolve a `bin` value (string or list) to the first locatable
/// binary: each candidate as given, else resolved against
/// `project_root` (`ArcanistExternalLinter.php:524-544`). No
/// candidate locatable is a configuration error.
pub fn resolve_configured_bin(
    value: &serde_json::Value,
    project_root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let mut candidates = Vec::new();
    match value {
        serde_json::Value::String(candidate) => candidates.push(candidate.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                let serde_json::Value::String(candidate) = item else {
                    return Err("'bin' must be a string or a list of strings".to_string());
                };
                candidates.push(candidate.clone());
            }
        }
        _ => return Err("'bin' must be a string or a list of strings".to_string()),
    }
    for candidate in &candidates {
        if binary_exists(candidate) {
            return Ok(std::path::PathBuf::from(candidate));
        }
        let resolved = project_root.join(candidate);
        if binary_exists(resolved.to_string_lossy().as_ref()) {
            return Ok(resolved);
        }
    }
    Err("None of the configured binaries can be located.".to_string())
}

/// Enforce a configured `version` requirement against `binary`,
/// probing `<binary> --version` like `getVersion()`. Returns `false`
/// when the linter must stay inactive (unknown version or mismatch);
/// PHP raises `ArcanistMissingLinterException` there, which fails the
/// run, but the port stays silent for unusable tools, exactly like a
/// missing default binary.
pub fn version_requirement_satisfied(
    pattern: Option<crate::linters::VersionPattern>,
    binary: &std::path::Path,
    requirement: &str,
    linter_class: &str,
) -> bool {
    let version = pattern.and_then(|pattern| crate::linters::probe_binary_version(pattern, binary));
    check_version_requirement(
        linter_class,
        &binary.to_string_lossy(),
        version.as_deref(),
        requirement,
        None,
    )
    .is_ok()
}

/// Configuration for one external tool invocation.
#[derive(Debug, Clone)]
pub struct ExternalToolConfig {
    /// Binary candidates; the first one found wins (mirrors `bin` lists).
    pub binaries: Vec<String>,
    /// Interpreter candidates (`shouldUseInterpreter` mode); empty means
    /// the binary runs directly.
    pub interpreters: Vec<String>,
    /// Flags passed on every invocation.
    pub flags: Vec<String>,
    /// Version requirement, e.g. `">= 1.2"` (see `version` option).
    pub version_requirement: Option<String>,
    /// Installed version, when known (feeds the requirement check).
    pub known_version: Option<String>,
    /// Install instructions for missing-binary errors.
    pub install_instructions: String,
    /// Upgrade instructions for version-mismatch errors.
    pub upgrade_instructions: Option<String>,
    /// Keep parsing when the tool exits nonzero (PHP default `true`).
    pub expect_command_errors: bool,
}

impl Default for ExternalToolConfig {
    fn default() -> Self {
        Self {
            binaries: Vec::new(),
            interpreters: Vec::new(),
            flags: Vec::new(),
            version_requirement: None,
            known_version: None,
            install_instructions: String::new(),
            upgrade_instructions: None,
            expect_command_errors: true,
        }
    }
}

/// Binary plus optional interpreter after successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// Interpreter to run the binary under, if any.
    pub interpreter: Option<String>,
    /// Binary (or script, in interpreter mode) to execute.
    pub binary: String,
}

/// Parses tool output into messages; `None` means parse failure (PHP
/// returns `false` from `parseLinterOutput`).
pub type ExternalParser =
    fn(path: &str, exit_code: i32, stdout: &str, stderr: &str) -> Option<Vec<LintMessage>>;

/// Generic config-driven external-tool runner. Unconfigured by default
/// (silent). NOT a registry linter; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct ExternalLinter {
    config: ExternalToolConfig,
    parser: Option<ExternalParser>,
}

impl ExternalLinter {
    /// Unconfigured runner: reports nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runner for `config`, parsing tool output with `parser`.
    #[must_use]
    pub fn with_config(config: ExternalToolConfig, parser: ExternalParser) -> Self {
        Self {
            config,
            parser: Some(parser),
        }
    }

    /// The runner configuration.
    #[must_use]
    pub fn config(&self) -> &ExternalToolConfig {
        &self.config
    }
}

impl Linter for ExternalLinter {
    fn name(&self) -> &'static str {
        NAME_LOCAL
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        let Some(parser) = self.parser else {
            return Vec::new();
        };
        if data.contains(&0) {
            return Vec::new();
        }
        let resolved = match check_binary_configuration(&self.config) {
            Ok(resolved) => resolved,
            Err(_) => return Vec::new(),
        };
        if let Some(requirement) = self.config.version_requirement.as_deref() {
            let ok = check_version_requirement(
                "ExternalLinter",
                &resolved.binary,
                self.config.known_version.as_deref(),
                requirement,
                self.config.upgrade_instructions.as_deref(),
            )
            .is_ok();
            if !ok {
                return Vec::new();
            }
        }
        let mut command = if let Some(interpreter) = resolved.interpreter.as_deref() {
            let mut command = crate::proc_env::command(interpreter);
            command.arg(&resolved.binary);
            command
        } else {
            crate::proc_env::command(&resolved.binary)
        };
        command.args(&self.config.flags);
        command.arg(path);
        let output = match arc_rs::lint::spawn_with_retry(&mut command) {
            Ok(output) => output,
            Err(_) => return Vec::new(),
        };
        let exit_code = output.status.code().unwrap_or(1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        resolve_output(
            parser,
            path,
            exit_code,
            &stdout,
            &stderr,
            self.config.expect_command_errors,
        )
        .unwrap_or_default()
    }
}

/// Resolve the configured binary (and interpreter), mirroring
/// `checkBinaryConfiguration`. With an interpreter the binary only needs
/// to exist; otherwise it must be an executable file.
pub fn check_binary_configuration(config: &ExternalToolConfig) -> Result<ResolvedTool, String> {
    let interpreter = if config.interpreters.is_empty() {
        None
    } else {
        let found = config
            .interpreters
            .iter()
            .find(|candidate| binary_exists(candidate));
        match found {
            Some(interpreter) => Some(interpreter.clone()),
            None => {
                return Err(format!(
                    "Unable to locate interpreter \"{}\" to run linter ExternalLinter. \
                     You may need to install the interpreter, or adjust your linter configuration.",
                    config.interpreters.join(", ")
                ));
            }
        }
    };
    let found = if interpreter.is_some() {
        config
            .binaries
            .iter()
            .find(|candidate| std::path::Path::new(candidate.as_str()).exists())
    } else {
        config
            .binaries
            .iter()
            .find(|candidate| binary_exists(candidate))
    };
    match found {
        Some(binary) => Ok(ResolvedTool {
            interpreter,
            binary: binary.clone(),
        }),
        None => {
            let (what, verb) = if interpreter.is_some() {
                ("script", "install the script")
            } else {
                ("binary", "install the binary")
            };
            let candidates = config.binaries.join(", ");
            Err(format!(
                "Unable to locate {what} \"{candidates}\" to run linter ExternalLinter. \
                 You may need to {verb}, or adjust your linter configuration.\n\
                 TO INSTALL: {}",
                config.install_instructions
            ))
        }
    }
}

/// Apply `resolveFuture` semantics to one finished invocation.
pub fn resolve_output(
    parser: ExternalParser,
    path: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
    expect_command_errors: bool,
) -> Result<Vec<LintMessage>, String> {
    if exit_code != 0 && !expect_command_errors {
        return Err(command_failure(exit_code, stdout, stderr));
    }
    let Some(messages) = parser(path, exit_code, stdout, stderr) else {
        return Err(parse_failure(exit_code, stdout, stderr));
    };
    if exit_code != 0 && expect_command_errors && messages.is_empty() {
        return Err(parse_failure(exit_code, stdout, stderr));
    }
    Ok(messages)
}

fn command_failure(exit_code: i32, stdout: &str, stderr: &str) -> String {
    format!(
        "linter command failed with exit code {exit_code}\n\nSTDOUT\n{stdout}\n\nSTDERR\n{stderr}"
    )
}

fn parse_failure(exit_code: i32, stdout: &str, stderr: &str) -> String {
    if exit_code != 0 {
        command_failure(exit_code, stdout, stderr)
    } else {
        format!("Linter failed to parse output!\n\nSTDOUT\n{stdout}\n\nSTDERR\n{stderr}")
    }
}

/// Check a version requirement, mirroring `checkBinaryVersion`.
///
/// `requirement` may prefix the version with `<`, `<=`, `>`, `>=`, or `=`
/// (default `=`). Error texts mirror the PHP messages.
pub fn check_version_requirement(
    linter: &str,
    binary: &str,
    version: Option<&str>,
    requirement: &str,
    upgrade_instructions: Option<&str>,
) -> Result<(), String> {
    let requirement = requirement.trim();
    if requirement.is_empty() {
        return Ok(());
    }
    let Some(version) = version else {
        let mut message = format!(
            "Linter {linter} requires {binary} version {requirement}. \
             Unable to determine the version that you have installed."
        );
        if let Some(instructions) = upgrade_instructions {
            message.push_str(&format!("\nTO UPGRADE: {instructions}"));
        }
        return Err(message);
    };
    let (operator, compare_to) = split_requirement(requirement);
    let ordering = version_compare(version, compare_to);
    let ok = match operator {
        "<" => ordering == Ordering::Less,
        "<=" => ordering != Ordering::Greater,
        ">" => ordering == Ordering::Greater,
        ">=" => ordering != Ordering::Less,
        _ => ordering == Ordering::Equal,
    };
    if ok {
        return Ok(());
    }
    let mut message = format!(
        "Linter {linter} requires {binary} version {requirement}. You have version {version}."
    );
    if let Some(instructions) = upgrade_instructions {
        message.push_str(&format!("\nTO UPGRADE: {instructions}"));
    }
    Err(message)
}

/// Split a requirement into `(operator, version)`, mirroring the PHP
/// `/^([<>]=?|=)\s*(.*)$/` match (`=` normalizes to `==`).
fn split_requirement(requirement: &str) -> (&str, &str) {
    for prefix in [">=", "<=", ">", "<", "="] {
        if let Some(rest) = requirement.strip_prefix(prefix) {
            let operator = if prefix == "=" { "==" } else { prefix };
            return (operator, rest.trim_start());
        }
    }
    ("==", requirement)
}

/// Approximate PHP `version_compare`.
///
/// `-`, `_`, and `+` split segments like `.`; numeric segments compare
/// numerically; known pre-release forms keep PHP order (`dev` < `alpha`/`a`
/// < `beta`/`b` < `RC`/`rc` < number < `pl`/`p`, matched case-sensitively
/// like PHP); anything else falls back to byte comparison; extra trailing
/// segments make a version newer, except pre-release forms
/// (`dev`/`alpha`/`beta`/`RC`), which keep it older than the bare prefix.
#[must_use]
pub fn version_compare(left: &str, right: &str) -> Ordering {
    let left = split_version(left);
    let right = split_version(right);
    let mut left = left.iter();
    let mut right = right.iter();
    loop {
        match (left.next(), right.next()) {
            (Some(a), Some(b)) => {
                let ordering = compare_segment(a, b);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(extra), None) => {
                return if is_prerelease(extra) {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (None, Some(extra)) => {
                return if is_prerelease(extra) {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (None, None) => break,
        }
    }
    Ordering::Equal
}

/// True for `dev`/`alpha`/`beta`/`RC` forms: a version carrying only such
/// an extra segment is still older (e.g. `1.0-RC` < `1.0`).
fn is_prerelease(segment: &str) -> bool {
    matches!(form_rank(segment), Some(rank) if rank < 4)
}

fn split_version(version: &str) -> Vec<&str> {
    version.split(['.', '-', '_', '+']).collect()
}

fn compare_segment(left: &str, right: &str) -> Ordering {
    let left_number = !left.is_empty() && left.bytes().all(|byte| byte.is_ascii_digit());
    let right_number = !right.is_empty() && right.bytes().all(|byte| byte.is_ascii_digit());
    if left_number && right_number {
        let left = left.trim_start_matches('0');
        let right = right.trim_start_matches('0');
        return left.len().cmp(&right.len()).then_with(|| left.cmp(right));
    }
    match (form_rank(left), form_rank(right)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => left.cmp(right),
    }
}

/// Rank of a known version form; numbers rank 4, between `rc` and `pl`.
fn form_rank(segment: &str) -> Option<i32> {
    if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(4);
    }
    match segment {
        "dev" => Some(0),
        "alpha" | "a" => Some(1),
        "beta" | "b" => Some(2),
        "RC" | "rc" => Some(3),
        "pl" | "p" => Some(5),
        _ => None,
    }
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
        // Staged by the `env_hermetic` subprocess test (child without
        // `PATH`); unsetting `PATH` in-process would mutate ambient state.
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

#[cfg(test)]
#[path = "lint_external_tests.rs"]
mod lint_external_tests;
