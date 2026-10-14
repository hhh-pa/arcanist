//! `arc-rs lint` PHPCS linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistPhpcsLinter.php` (an
//! `ArcanistExternalLinter` wrapping PHP_CodeSniffer).
//!
//! Behavior: runs `phpcs --report=xml <file>` on PHP files and parses
//! the stdout XML report. Each `<error>`/`<warning>` inside a `<file>`
//! becomes one message with code `PHPCS.E.<source>` or
//! `PHPCS.W.<source>`, name `<source>`, and the element text as the
//! description; warnings map to `warning` and errors to `error`.
//! A zero tool exit means no findings and yields no messages.
//!
//! Known limits (see T1128): the `phpcs.standard` linter option is not
//! supported (the tool default standard applies); invalid XML yields no
//! messages (the PHP linter raises, but the
//! [`Linter`](arc_rs::lint::Linter) trait has no error channel).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter, SEVERITY_ERROR};

/// Severity mirroring `ArcanistLintSeverity`; `lint.rs` only defines the
/// severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";

/// PHPCS linter: PHP_CodeSniffer findings via an external binary.
#[derive(Debug, Default)]
pub struct PhpcsLinter;

impl PhpcsLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "phpcs"
}

/// Exact tool argv flags, mirroring `getMandatoryFlags()` without a
/// configured standard.
#[must_use]
pub fn command_flags() -> Vec<String> {
    vec!["--report=xml".to_string()]
}

fn is_supported(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["php", "phtml", "inc"];
    let Some(ext) = extension_of(path) else {
        return false;
    };
    EXTENSIONS
        .iter()
        .any(|known| ext.eq_ignore_ascii_case(known))
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

fn binary_in(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    find_on_path(path_env, binary_name())
}

/// Locate the tool on the real `$PATH`; `None` means the linter is
/// inactive and reports no messages.
#[must_use]
pub fn binary_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    binary_in(&path)
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
            "arc-rs-lint-phpcs-{}-{nanos}-{n}",
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
        _ => "src.php".to_string(),
    }
}

fn run_tool(binary: &Path, file: &Path) -> Option<(i32, String, String)> {
    let mut command = Command::new(binary);
    command.args(command_flags()).arg(file).stdin(Stdio::null());
    let output = arc_rs::lint::spawn_with_retry(&mut command).ok()?;
    let code = output.status.code().unwrap_or(1);
    Some((
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Linter body with an explicit tool path (mirrors the PHP `bin` option).
/// `None` (tool absent) reports no messages.
pub fn lint_with_binary(path: &str, data: &[u8], binary: Option<&Path>) -> Vec<LintMessage> {
    let Some(binary) = binary else {
        return Vec::new();
    };
    let Ok(staged) = StagedFile::stage(path, data) else {
        return Vec::new();
    };
    let Some((code, stdout, stderr)) = run_tool(binary, staged.path()) else {
        return Vec::new();
    };
    parse_output(path, code, &stdout, &stderr)
}

/// Parse a PHPCS `--report=xml` stdout report, mirroring
/// `parseLinterOutput()`. A zero exit means no findings; invalid XML
/// yields no messages.
#[must_use]
pub fn parse_output(path: &str, exit_code: i32, stdout: &str, _stderr: &str) -> Vec<LintMessage> {
    if exit_code == 0 {
        return Vec::new();
    }
    if !looks_like_phpcs_xml(stdout) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (_, inner) in find_elements(stdout, "file") {
        for (kind, open, text) in find_findings(&inner) {
            let source = tag_attr(&open, "source").unwrap_or_default();
            let prefix = if kind == "error" { 'E' } else { 'W' };
            let severity = if kind == "error" {
                SEVERITY_ERROR
            } else {
                SEVERITY_WARNING
            };
            out.push(LintMessage {
                path: path.to_string(),
                line: tag_attr(&open, "line").map_or(1, |v| parse_num(&v)),
                col: tag_attr(&open, "column").map_or(1, |v| parse_num(&v)),
                code: format!("PHPCS.{prefix}.{source}"),
                severity: severity.to_string(),
                name: source,
                description: xml_unescape(&text),
                original: None,
                replacement: None,
            });
        }
    }
    out
}

/// Reject obvious non-XML output the way `DOMDocument::loadXML()` would
/// fail to parse it.
fn looks_like_phpcs_xml(stdout: &str) -> bool {
    stdout.contains("<phpcs") && stdout.contains("</phpcs>")
}

/// `<error>`/`<warning>` findings in document order as
/// `(kind, open_tag, inner_text)` triples.
fn find_findings(xml: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < xml.len() {
        let rest = &xml[pos..];
        // Next `<error` or `<warning` tag, whichever comes first.
        let mut next: Option<(&str, usize)> = None;
        for kind in ["error", "warning"] {
            let needle = format!("<{kind}");
            if let Some(found) = rest.find(&needle)
                && next.is_none_or(|(_, best)| found < best)
            {
                next = Some((kind, found));
            }
        }
        let Some((kind, found)) = next else {
            break;
        };
        let needle = format!("<{kind}");
        let tag_start = pos + found;
        let after_name = xml[tag_start + needle.len()..].chars().next();
        if !after_name.is_none_or(|c| c == '>' || c == '/' || c.is_whitespace()) {
            pos = tag_start + 1;
            continue;
        }
        let Some(tag_end) = find_tag_end(&xml[tag_start..]) else {
            break;
        };
        let open_tag = xml[tag_start..tag_start + tag_end].to_string();
        let after = tag_start + tag_end;
        let close = format!("</{kind}>");
        if open_tag.ends_with("/>") {
            out.push((kind.to_string(), open_tag, String::new()));
            pos = after;
        } else if let Some(end) = xml[after..].find(&close) {
            out.push((
                kind.to_string(),
                open_tag,
                xml[after..after + end].to_string(),
            ));
            pos = after + end + close.len();
        } else {
            break;
        }
    }
    out
}

/// 1-based position component; missing, unparsable, or zero values map
/// to 1.
fn parse_num(text: &str) -> usize {
    text.trim().parse::<usize>().unwrap_or(1).max(1)
}

/// Decode the five predefined XML entities plus decimal/hex char refs.
/// Malformed sequences are kept literal.
fn xml_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            break;
        };
        if semi > 12 {
            out.push('&');
            rest = &rest[1..];
            continue;
        }
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" => Some("'".to_string()),
            _ if entity.starts_with('#') => char_ref(entity),
            _ => None,
        };
        if let Some(decoded) = decoded {
            out.push_str(&decoded);
        } else {
            out.push_str(&rest[..=semi]);
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn char_ref(entity: &str) -> Option<String> {
    let digits = entity.strip_prefix('#')?;
    let code = if let Some(hex) = digits
        .strip_prefix('x')
        .or_else(|| digits.strip_prefix('X'))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse::<u32>().ok()?
    };
    char::from_u32(code).map(|c| c.to_string())
}

/// Value of `name="..."` (single- or double-quoted) inside an open tag.
fn tag_attr(tag: &str, name: &str) -> Option<String> {
    let mut search = 0;
    while let Some(found) = tag[search..].find(name) {
        let start = search + found;
        let before = tag[..start].chars().next_back();
        let after = tag[start + name.len()..].chars().next();
        search = start + name.len();
        let boundary_before =
            before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':'));
        if !boundary_before || after != Some('=') {
            continue;
        }
        let mut value = tag[start + name.len() + 1..].trim_start();
        let quote = value.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        value = &value[1..];
        let end = value.find(quote)?;
        return Some(xml_unescape(&value[..end]));
    }
    None
}

/// All `<tag ...>...</tag>` (or self-closing `<tag .../>`) elements as
/// `(open_tag, inner_xml)` pairs. This is a tolerant scanner for
/// machine-generated tool output, not a general XML parser.
fn find_elements(xml: &str, tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pos = 0;
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    while let Some(found) = xml[pos..].find(&open) {
        let tag_start = pos + found;
        let after_name = xml[tag_start + open.len()..].chars().next();
        if !after_name.is_none_or(|c| c == '>' || c == '/' || c.is_whitespace()) {
            pos = tag_start + 1;
            continue;
        }
        let Some(tag_end) = find_tag_end(&xml[tag_start..]) else {
            break;
        };
        let open_tag = xml[tag_start..tag_start + tag_end].to_string();
        let after = tag_start + tag_end;
        if open_tag.ends_with("/>") {
            out.push((open_tag, String::new()));
            pos = after;
        } else if let Some(end) = xml[after..].find(&close) {
            out.push((open_tag, xml[after..after + end].to_string()));
            pos = after + end + close.len();
        } else {
            break;
        }
    }
    out
}

/// Offset just past the `>` closing the tag at the start of `text`,
/// skipping `>` characters inside quoted attribute values.
fn find_tag_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' | b'\'' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
            }
            b'>' => return Some(index + 1),
            _ => {}
        }
        index += 1;
    }
    None
}

impl Linter for PhpcsLinter {
    fn name(&self) -> &'static str {
        "PHPCS"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported(path) {
            return Vec::new();
        }
        if data.contains(&0) {
            return Vec::new();
        }
        lint_with_binary(path, data, binary_on_path().as_deref())
    }
}

#[cfg(test)]
#[path = "lint_phpcs_tests.rs"]
mod lint_phpcs_tests;
