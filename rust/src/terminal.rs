//! Console formatting port: `tsprintf`, the console formatter, `pht`, and
//! ref views (`src/xsprintf/tsprintf.php`, `src/console/format.php`,
//! `src/console/PhutilConsoleFormatter.php`,
//! `src/xsprintf/PhutilTerminalString.php`,
//! `src/internationalization/`, `src/ref/ArcanistRefView.php`,
//! `src/log/ArcanistLogEngine.php`).
//!
//! Non-TTY output strips markup (`**bold**`, `__underline__`, `##invert##`,
//! `<fg:color>`/`<bg:color>`); TTY output renders ANSI. Values substituted
//! into patterns are escaped per conversion and never markup-interpreted.
//!
//! Deviations: `tsprintf` values are strings, integers, or nested terminal
//! strings (PHP arrays, booleans, and `null` are not supported); `pht`
//! numbers are integers (no float decimals); invalid UTF-8 can not occur
//! (`str` is always valid); terminal width is cached without `WINCH`
//! resets; the `STDOUT`-handle check in ANSI detection is skipped (a CLI
//! always has stdout).

use std::fmt;

/// An interpreted pattern with substituted values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalString {
    text: String,
}

impl TerminalString {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self { text }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for TerminalString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A `tsprintf` conversion value.
#[derive(Debug, Clone)]
pub enum TVal<'a> {
    Text(&'a str),
    Owned(String),
    Int(i64),
    Terminal(&'a TerminalString),
}

impl<'a> TVal<'a> {
    fn as_text(&self) -> String {
        match self {
            TVal::Text(text) => (*text).to_string(),
            TVal::Owned(text) => text.clone(),
            TVal::Int(number) => number.to_string(),
            TVal::Terminal(terminal) => terminal.as_str().to_string(),
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, TVal::Terminal(_))
    }
}

fn find_marker(text: &[u8], marker: u8, from: usize) -> Option<usize> {
    let mut index = from;
    while index < text.len() {
        if text.get(index) == Some(&marker) && text.get(index + 1) == Some(&marker) {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// One `**`/`__`/`##` pass over `text`, stripped or ANSI.
fn interpret_pair(text: &str, marker: u8, open: &str, close: &str, ansi: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let Some(start) = find_marker(bytes, marker, index) else {
            out.push_str(&text[index..]);
            break;
        };
        // An escaped opener fails; scanning resumes at the next byte.
        if start > 0 && bytes[start - 1] == b'\\' {
            out.push_str(&text[index..=start]);
            index = start + 1;
            continue;
        }
        match find_marker(bytes, marker, start + 2) {
            Some(end) => {
                out.push_str(&text[index..start]);
                if ansi {
                    out.push_str(open);
                    out.push_str(&text[start + 2..end]);
                    out.push_str(close);
                } else {
                    out.push_str(&text[start + 2..end]);
                }
                index = end + 2;
            }
            None => {
                out.push_str(&text[index..start + 2]);
                index = start + 2;
            }
        }
    }
    out
}

fn color_number(name: &str) -> Option<u8> {
    match name {
        "black" => Some(0),
        "red" => Some(1),
        "green" => Some(2),
        "yellow" => Some(3),
        "blue" => Some(4),
        "magenta" => Some(5),
        "cyan" => Some(6),
        "white" => Some(7),
        "default" => Some(9),
        _ => None,
    }
}

/// The next `<` at or after `from`.
fn find_tag_open(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Match a `<fg:color>body</color>` tag at `open`: (color number, is
/// foreground, body start, body end, tag end). Unknown colors, missing
/// closers, and non-color tags do not match.
fn match_color_tag(text: &str, open: usize) -> Option<(u8, bool, usize, usize, usize)> {
    let rest = text.get(open..)?;
    let is_fg = if rest.starts_with("<fg:") {
        true
    } else if rest.starts_with("<bg:") {
        false
    } else {
        return None;
    };
    let after = rest.get(4..)?;
    let end = after.find('>')?;
    let number = color_number(&after[..end])?;
    let closer = if is_fg { "</fg>" } else { "</bg>" };
    let body_start = open + 4 + end + 1;
    let tail = text.get(body_start..)?;
    let close_at = tail.find(closer)?;
    Some((
        number,
        is_fg,
        body_start,
        body_start + close_at,
        body_start + close_at + closer.len(),
    ))
}

/// Push a color-tag body, ANSI-wrapped or bare.
fn push_colored(
    out: &mut String,
    text: &str,
    body: (usize, usize),
    number: u8,
    is_fg: bool,
    ansi: bool,
) {
    if ansi {
        let (code, default) = if is_fg {
            (30 + number, 39)
        } else {
            (40 + number, 49)
        };
        out.push_str(&format!(
            "\x1b[{code}m{}\x1b[{default}m",
            &text[body.0..body.1]
        ));
    } else {
        out.push_str(&text[body.0..body.1]);
    }
}

/// One `<fg:color>`/`<bg:color>` pass over `text`, stripped or ANSI.
fn interpret_colors(text: &str, ansi: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let Some(open) = find_tag_open(bytes, index) else {
            out.push_str(&text[index..]);
            break;
        };
        match match_color_tag(text, open) {
            Some((number, is_fg, body_start, body_end, end)) => {
                out.push_str(&text[index..open]);
                push_colored(&mut out, text, (body_start, body_end), number, is_fg, ansi);
                index = end;
            }
            None => {
                out.push_str(&text[index..=open]);
                index = open + 1;
            }
        }
    }
    out
}

/// Drop backslash escapes before `**`/`__`/`##` groups.
fn interpret_unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut flushed = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        let marker = bytes.get(index + 1).and_then(|&first| {
            let second = bytes.get(index + 2)?;
            if first == *second && matches!(first, b'*' | b'_' | b'#') {
                Some(first)
            } else {
                None
            }
        });
        let end = marker.and_then(|mark| find_marker(bytes, mark, index + 3));
        match end {
            Some(end) => {
                out.push_str(&text[flushed..index]);
                out.push_str(&text[index + 1..end + 2]);
                flushed = end + 2;
                index = end + 2;
            }
            None => {
                out.push_str(&text[flushed..=index]);
                flushed = index + 1;
                index += 1;
            }
        }
    }
    out.push_str(&text[flushed..]);
    out
}

/// Interpret (or strip) console markup in a pattern.
#[must_use]
pub fn interpret(pattern: &str, ansi: bool) -> String {
    let esc = "\x1b";
    let text = interpret_pair(
        pattern,
        b'*',
        &format!("{esc}[1m"),
        &format!("{esc}[m"),
        ansi,
    );
    let text = interpret_pair(&text, b'_', &format!("{esc}[4m"), &format!("{esc}[m"), ansi);
    let text = interpret_pair(&text, b'#', &format!("{esc}[7m"), &format!("{esc}[m"), ansi);
    let text = interpret_colors(&text, ansi);
    interpret_unescape(&text)
}

fn escape_byte(byte: u8, out: &mut Vec<u8>) {
    match byte {
        0x00 => out.extend_from_slice(b"<NUL>"),
        0x07 => out.extend_from_slice(b"<BEL>"),
        0x08 => out.extend_from_slice(b"<BS>"),
        0x09 => out.extend_from_slice(b"<TAB>"),
        0x0A => out.extend_from_slice(b"<LF>"),
        0x0D => out.extend_from_slice(b"<CR>"),
        0x1B => out.extend_from_slice(b"<ESC>"),
        0x7F => out.extend_from_slice(b"<DEL>"),
        _ => {
            if byte < 0x20 {
                out.extend_from_slice(format!("<0x{byte:02X}>").as_bytes());
            } else {
                out.push(byte);
            }
        }
    }
}

/// Escape terminal control characters, preserving whitespace when asked.
#[must_use]
pub fn escape_value(value: &str, allow_whitespace: bool) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if allow_whitespace && matches!(byte, b'\r' | b'\n' | b'\t') {
            if byte == b'\r' && bytes.get(index + 1) != Some(&b'\n') {
                out.extend_from_slice(b"<CR>");
            } else {
                out.push(byte);
            }
            index += 1;
            continue;
        }
        escape_byte(byte, &mut out);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Copy an ANSI escape (through `m`) into `line`; the next index after.
fn copy_escape(chars: &[char], mut index: usize, line: &mut Vec<char>) -> usize {
    while index < chars.len() {
        line.push(chars[index]);
        if chars[index] == 'm' {
            break;
        }
        index += 1;
    }
    index + 1
}

/// Incremental `console_wrap` line state.
#[derive(Default)]
struct WrapState {
    lines: Vec<Vec<char>>,
    line: Vec<char>,
    line_len: i64,
    len_after_break: i64,
    break_pos: Option<usize>,
}

impl WrapState {
    /// Break the line at the last space when past `width`.
    fn wrap_if_needed(&mut self, width: i64) {
        if self.line_len <= width {
            return;
        }
        let Some(at) = self.break_pos else {
            return;
        };
        let mut slice = self.line[..at].to_vec();
        while slice.last() == Some(&' ') {
            slice.pop();
        }
        slice.push('\n');
        self.lines.push(slice);
        self.line = self.line[at..].to_vec();
        self.line_len = self.len_after_break;
        self.len_after_break = 0;
        self.break_pos = None;
    }

    /// Push one character, wrapping and tracking breaks.
    fn push_char(&mut self, current: char, width: i64) {
        self.line.push(current);
        self.line_len += 1;
        self.len_after_break += 1;
        self.wrap_if_needed(width);
        if current == ' ' {
            self.break_pos = Some(self.line.len());
            self.len_after_break = 0;
        }
        if current == '\n' {
            self.lines.push(std::mem::take(&mut self.line));
            self.len_after_break = 0;
            self.line_len = 0;
            self.break_pos = None;
        }
    }

    /// Render wrapped lines with the indent prefix.
    fn finish(self, indent: usize, with_prefix: bool) -> String {
        let mut lines = self.lines;
        if !self.line.is_empty() {
            lines.push(self.line);
        }
        let prefix: String = if indent > 0 {
            " ".repeat(indent)
        } else {
            String::new()
        };
        let mut out = String::new();
        for (number, wrapped) in lines.iter().enumerate() {
            if number > 0 || with_prefix {
                out.push_str(&prefix);
            }
            out.extend(wrapped.iter());
        }
        out
    }
}

/// Soft-wrap text at column 78 minus `indent`.
#[must_use]
pub fn console_wrap(text: &str, indent: usize, with_prefix: bool) -> String {
    let width = 78i64 - indent as i64;
    let chars: Vec<char> = text.chars().collect();
    let mut state = WrapState::default();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\x1b' {
            index = copy_escape(&chars, index, &mut state.line);
            continue;
        }
        state.push_char(chars[index], width);
        index += 1;
    }
    state.finish(indent, with_prefix)
}

/// Join an argv like PHP `csprintf('%Ls', ...)`: every word
/// `escapeshellarg()`-quoted (single quotes, always), space-separated.
/// Used for the `--trace` ARGV echo and shell-alias displays.
#[must_use]
pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|word| format!("'{}'", word.replace('\'', "'\\''")))
        .collect::<Vec<String>>()
        .join(" ")
}

/// PHP `(int)` cast of a string: leading whitespace, an optional sign,
/// and leading ASCII digits, clamped to 64 bits.
#[must_use]
pub fn php_int_cast(text: &str) -> i64 {
    let trimmed = text.trim_start_matches([' ', '\t', '\n', '\r', '\0', '\u{b}']);
    let (digits, negative) = match trimmed.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (trimmed.strip_prefix('+').unwrap_or(trimmed), false),
    };
    let mut value: i64 = 0;
    let mut any = false;
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            break;
        }
        any = true;
        let digit = i64::from(byte - b'0');
        value = value
            .saturating_mul(10)
            .saturating_add(if negative { -digit } else { digit });
    }
    if any { value } else { 0 }
}

/// Escape a conversion value unless it is already terminal text.
fn escaped_text(value: &TVal, text: &str, allow_whitespace: bool) -> String {
    if value.is_terminal() {
        text.to_string()
    } else {
        escape_value(text, allow_whitespace)
    }
}

/// Format a `%d` conversion (integers directly, else a PHP int cast).
fn convert_int(value: &TVal, text: &str) -> String {
    match value {
        TVal::Int(number) => number.to_string(),
        _ => php_int_cast(text).to_string(),
    }
}

/// Format a markup-building conversion (`!`, `?`, `>`); anything else is
/// an unsupported escape. `pattern_so_far` mirrors the partially
/// rewritten pattern for error messages.
fn convert_markup(
    conversion: u8,
    value: &TVal,
    pattern_so_far: &mut [u8],
    pos: usize,
    ansi: bool,
) -> Result<String, String> {
    match conversion {
        b'!' => {
            pattern_so_far[pos] = b's';
            tsprintf_ansi(
                "<bg:yellow>** <!> %s **</bg>",
                std::slice::from_ref(value),
                ansi,
            )
            .map(|t| t.to_string())
        }
        b'?' => {
            pattern_so_far[pos] = b's';
            let nested = tsprintf_ansi(
                "<bg:green>**  ?  **</bg> %s",
                std::slice::from_ref(value),
                ansi,
            )?; // LCOV_EXCL_LINE // Reason: nested fixed-pattern format is infallible; the `?` never propagates
            Ok(console_wrap(nested.as_str(), 6, false))
        }
        b'>' => {
            pattern_so_far[pos] = b's';
            tsprintf_ansi("    **$ %s**\n", std::slice::from_ref(value), ansi)
                .map(|t| t.to_string())
        }
        _ => Err(format!(
            "Unsupported escape sequence \"{}\" found in pattern: {}",
            conversion as char,
            String::from_utf8_lossy(pattern_so_far)
        )),
    }
}

/// Format one `tsprintf` conversion; `pattern_so_far` mirrors the
/// partially rewritten pattern for error messages.
fn convert(
    conversion: u8,
    value: &TVal,
    pattern_so_far: &mut [u8],
    pos: usize,
    ansi: bool,
) -> Result<String, String> {
    let text = value.as_text();
    match conversion {
        b's' => {
            pattern_so_far[pos] = b's';
            Ok(escaped_text(value, &text, false))
        }
        b'B' => {
            pattern_so_far[pos] = b's';
            Ok(escaped_text(value, &text, true))
        }
        b'R' => {
            pattern_so_far[pos] = b's';
            Ok(text)
        }
        b'W' => {
            pattern_so_far[pos] = b's';
            Ok(console_wrap(&escaped_text(value, &text, true), 0, true))
        }
        b'd' => {
            pattern_so_far[pos] = b'd';
            Ok(convert_int(value, &text))
        }
        other => convert_markup(other, value, pattern_so_far, pos, ansi),
    }
}

/// Incremental `tsprintf_ansi` formatting state.
struct SprintfState {
    pattern_so_far: Vec<u8>,
    out: Vec<u8>,
    arg: usize,
    conv: bool,
}

impl SprintfState {
    fn new(bytes: &[u8]) -> Self {
        Self {
            pattern_so_far: bytes.to_vec(),
            out: Vec::new(),
            arg: 0,
            conv: false,
        }
    }

    /// Format one conversion after `%` (modifiers are unsupported); the
    /// next index after.
    fn step_conversion(
        &mut self,
        bytes: &[u8],
        args: &[TVal],
        index: usize,
        ansi: bool,
    ) -> Result<usize, String> {
        let byte = bytes[index];
        if matches!(byte, b'\'' | b'-' | b'0'..=b'9' | b'.' | b'$' | b'+') {
            return Err(format!(
                "xsprintf() does not support the \"%{}\" modifier.",
                byte as char
            ));
        }
        self.conv = false;
        if self.arg >= args.len() {
            return Err("Too few arguments to xsprintf().".to_string());
        }
        let replacement = convert(byte, &args[self.arg], &mut self.pattern_so_far, index, ansi)?;
        self.arg += 1;
        self.out.extend_from_slice(replacement.as_bytes());
        Ok(index + 1)
    }

    /// Process the byte at `index`; the next index after.
    fn step(
        &mut self,
        bytes: &[u8],
        args: &[TVal],
        index: usize,
        ansi: bool,
    ) -> Result<usize, String> {
        let byte = bytes[index];
        if self.conv && byte != b'%' {
            return self.step_conversion(bytes, args, index, ansi);
        }
        if byte == b'%' {
            if self.conv {
                self.out.push(b'%');
            }
            self.conv = !self.conv;
        } else {
            self.out.push(byte);
        }
        Ok(index + 1)
    }

    /// Finish formatting, checking the argument count.
    fn finish(self, expected: usize) -> Result<TerminalString, String> {
        let mut out = self.out;
        if self.conv {
            out.push(b'%');
        }
        if self.arg != expected {
            return Err("Too many arguments to xsprintf().".to_string());
        }
        Ok(TerminalString::new(
            String::from_utf8(out).unwrap_or_default(),
        ))
    }
}

/// Format with an explicit ANSI setting (deterministic under test).
pub fn tsprintf_ansi(pattern: &str, args: &[TVal], ansi: bool) -> Result<TerminalString, String> {
    let interpreted = interpret(pattern, ansi);
    let bytes = interpreted.as_bytes();
    let mut state = SprintfState::new(bytes);
    let mut index = 0;
    while index < bytes.len() {
        index = state.step(bytes, args, index, ansi)?;
    }
    state.finish(args.len())
}

/// Format with the process ANSI setting.
pub fn tsprintf(pattern: &str, args: &[TVal]) -> Result<TerminalString, String> {
    tsprintf_ansi(pattern, args, ansi_enabled())
}

#[cfg(windows)]
fn tput_columns() -> usize {
    0
}

#[cfg(not(windows))]
fn tput_columns() -> usize {
    let Ok(output) = crate::proc_env::command("tput").arg("cols").output() else {
        return 0; // LCOV_EXCL_LINE // Reason: tput is present on ubuntu CI; the spawn-failure arm never runs
    };
    if !output.status.success() {
        return 0; // LCOV_EXCL_LINE // Reason: tput cols succeeds with a terminal; without one (gate container) this arm takes instead
    } // LCOV_EXCL_START // Reason: tput fails where gates measure (no terminal in the gate container), so the success-parse body never executes on CI; covered on developer runs
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim_matches([' ', '\t', '\n', '\r', '\0', '\u{b}']);
    if trimmed.starts_with('-') {
        return 0; // LCOV_EXCL_LINE // Reason: tput prints plain widths; negative output never occurs
    }
    let digits = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let mut value: usize = 0;
    let mut any = false;
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            break; // LCOV_EXCL_LINE // Reason: tput prints plain digits; the non-digit break never trips
        }
        any = true;
        value = value
            .saturating_mul(10)
            .saturating_add(usize::from(byte - b'0'));
    }
    if any && value > 0 { value } else { 0 }
    // LCOV_EXCL_STOP
}

/// Console display width, mirroring `phutil_console_get_terminal_width()`.
/// Failures (including Windows) yield 0, like PHP's `null`.
#[must_use]
pub fn terminal_width() -> usize {
    static WIDTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *WIDTH.get_or_init(tput_columns)
}

fn php_truthy(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(text) => !text.is_empty() && text != "0",
    }
}

/// True when `--trace` mode is on (T1153 CORE-15): magenta trace
/// labels print, and classic error arms dump backtraces. Set by the
/// dispatcher from the global `--trace` flag.
#[must_use]
pub fn trace_enabled() -> bool {
    crate::globals::trace_enabled()
}

/// True when console output should carry ANSI codes.
///
/// `ARC_ANSI=0`/`ARC_ANSI=1` (the global `--no-ansi`/`--ansi` channel,
/// T1153 RB-D3) force the answer, mirroring
/// `PhutilConsoleFormatter::disableANSI()`; otherwise the TERM +
/// stdout-TTY probe decides.
#[must_use]
pub fn ansi_enabled() -> bool {
    if let Some(forced) = crate::globals::ansi_override() {
        return forced;
    }
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    let term = if !php_truthy(Some(&term)) && php_truthy(std::env::var("ANSICON").ok().as_deref()) {
        "ansi".to_string() // LCOV_EXCL_LINE // Reason: needs TERM unset with ANSICON set; env is process-global
    } else {
        term
    };
    #[cfg(windows)]
    if term != "cygwin" && term != "ansi" {
        return false;
    }
    // `term` only gates the Windows early return above; keep it observed
    // off Windows so the binding stays live on every target.
    #[cfg(not(windows))]
    let _ = &term;
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// A `pht` substitution value.
#[derive(Debug, Clone)]
pub enum PhtArg<'a> {
    Text(&'a str),
    Int(i64),
    Number(i64, u32),
}

enum Variant {
    Pair(&'static str, &'static str),
    Nested([[&'static str; 2]; 2]),
}

/// Arcanist-side `en_US` translations (first half of the map).
fn lookup_translation_arcanist(pattern: &str) -> Option<Variant> {
    match pattern {
        "%s locally modified path(s) are not included in this revision:" => Some(Variant::Pair(
            "A locally modified path is not included in this revision:",
            "Locally modified paths are not included in this revision:",
        )),
        "These %s path(s) will NOT be committed. Commit this revision anyway?" => {
            Some(Variant::Pair(
                "This path will NOT be committed. Commit this revision anyway?",
                "These paths will NOT be committed. Commit this revision anyway?",
            ))
        }
        "Revision includes changes to %s path(s) that do not exist:" => Some(Variant::Pair(
            "Revision includes changes to a path that does not exist:",
            "Revision includes changes to paths that do not exist:",
        )),
        "This diff includes %s file(s) which are not valid UTF-8 (they contain invalid byte \
         sequences). You can either stop this workflow and fix these files, or continue. If you \
         continue, these files will be marked as binary." => Some(Variant::Pair(
            "This diff includes a file which is not valid UTF-8 (it has invalid byte sequences). \
             You can either stop this workflow and fix it, or continue. If you continue, this \
             file will be marked as binary.",
            "This diff includes files which are not valid UTF-8 (they contain invalid byte \
             sequences). You can either stop this workflow and fix these files, or continue. If \
             you continue, these files will be marked as binary.",
        )),
        "%s AFFECTED FILE(S)" => Some(Variant::Pair("AFFECTED FILE", "AFFECTED FILES")),
        "Do you want to mark these %s file(s) as binary and continue?" => Some(Variant::Pair(
            "Do you want to mark this file as binary and continue?",
            "Do you want to mark these files as binary and continue?",
        )),
        "Do you want to amend these %s change(s) to the current commit?" => Some(Variant::Pair(
            "Do you want to amend this change to the current commit?",
            "Do you want to amend these changes to the current commit?",
        )),
        "Do you want to create a new commit with these %s change(s)?" => Some(Variant::Pair(
            "Do you want to create a new commit with this change?",
            "Do you want to create a new commit with these changes?",
        )),
        "(To ignore these %s change(s), add them to \"%s\".)" => Some(Variant::Pair(
            "(To ignore this change, add it to \"%2$s\".)",
            "(To ignore these changes, add them to \"%2$s\".)",
        )),
        "%s line(s)" => Some(Variant::Pair("line", "lines")),
        _ => None,
    }
}

/// libphutil-side `en_US` translations (second half of the map).
fn lookup_translation_libphutil(pattern: &str) -> Option<Variant> {
    match pattern {
        "%s assertion(s) passed." => Some(Variant::Pair(
            "%s assertion passed.",
            "%s assertions passed.",
        )),
        "Ignore these %s untracked file(s) and continue?" => Some(Variant::Pair(
            "Ignore this untracked file and continue?",
            "Ignore these untracked files and continue?",
        )),
        "%s submodule(s) have uncommitted or untracked changes:" => Some(Variant::Pair(
            "A submodule has uncommitted or untracked changes:",
            "Submodules have uncommitted or untracked changes:",
        )),
        "Ignore the changes to these %s submodule(s) and continue?" => Some(Variant::Pair(
            "Ignore the changes to this submodule and continue?",
            "Ignore the changes to these submodules and continue?",
        )),
        "These %s commit(s) will be landed:" => Some(Variant::Pair(
            "This commit will be landed:",
            "These commits will be landed:",
        )),
        "Updated %s librarie(s)." => {
            Some(Variant::Pair("Updated library.", "Updated %s libraries."))
        }
        "Daemon was idle for more than %s second(s), scaling pool down." => Some(Variant::Pair(
            "Daemon was idle for more than one second, scaling pool down.",
            "Daemon was idle for more than %s seconds, scaling pool down.",
        )),
        "Analyzing %s file(s) with %s subprocess(es)..." => Some(Variant::Nested([
            [
                "Analyzing one file with a single subprocess...",
                "Analyzing %s file with %s subprocesses...",
            ],
            [
                "Analyzing %s files with a single subprocess...",
                "Analyzing %s files with %s subprocesses...",
            ],
        ])),
        "... (%s more byte(s)) ..." => Some(Variant::Pair(
            "... (%s more byte) ...",
            "... (%s more bytes) ...",
        )),
        _ => None,
    }
}

/// The merged `en_US` translation map (Arcanist + libphutil).
fn lookup_translation(pattern: &str) -> Option<Variant> {
    if let Some(variant) = lookup_translation_arcanist(pattern) {
        return Some(variant);
    }
    lookup_translation_libphutil(pattern)
}

fn invalid_variant(pattern: &str, pos: &str, kind: &str) -> String {
    format!(
        "[Invalid Translation!] The \"en_US\" language data offers variant translations for the \
         plurality or gender of argument {pos}, but the value for that argument is not an \
         integer, PhutilNumber, or PhutilPerson (it is a value of type \"{kind}\"). Raw input: \
         <{pattern}>."
    )
}

/// The variant-selecting number of one argument (positions are 1-based,
/// like PHP's argument array).
fn plural_number(args: &[PhtArg], index: usize, pattern: &str) -> Result<i64, String> {
    match args.get(index) {
        Some(PhtArg::Int(number)) | Some(PhtArg::Number(number, _)) => Ok(*number),
        Some(PhtArg::Text(_)) => Err(invalid_variant(pattern, &(index + 1).to_string(), "string")),
        None => Err(invalid_variant(pattern, "", "boolean")),
    }
}

/// `number_format()` with `en_US` separators (`,` thousands, `.` point).
fn format_number(value: i64, decimals: u32) -> String {
    let mut digits: Vec<char> = value.unsigned_abs().to_string().chars().collect();
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped = format!(",{}{grouped}", tail.into_iter().collect::<String>());
    }
    grouped = format!("{}{grouped}", digits.into_iter().collect::<String>());
    let mut out = if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    };
    if decimals > 0 {
        out.push('.');
        out.extend(std::iter::repeat_n('0', decimals as usize));
    }
    out
}

fn pht_arg_text(arg: &PhtArg) -> String {
    match arg {
        PhtArg::Text(text) => (*text).to_string(),
        PhtArg::Int(number) => number.to_string(),
        PhtArg::Number(value, decimals) => format_number(*value, *decimals),
    }
}

/// Scan `%` digits and an optional `$`: (positional index, digits, cursor).
fn scan_placement(bytes: &[u8], index: usize) -> (usize, String, usize) {
    let mut cursor = index + 1;
    let mut digits = String::new();
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        digits.push(bytes[cursor] as char);
        cursor += 1;
    }
    let positional = if !digits.is_empty() && cursor < bytes.len() && bytes[cursor] == b'$' {
        cursor += 1;
        digits.parse::<usize>().unwrap_or(0)
    } else {
        0
    };
    (positional, digits, cursor)
}

/// Push one `%s`/`%d` argument (`%d` renders integers, casting text).
fn push_pht_arg(out: &mut Vec<u8>, conversion: u8, arg: &PhtArg) {
    if conversion == b'd' {
        let text = match arg {
            PhtArg::Int(number) => number.to_string(),
            PhtArg::Number(value, _) => value.to_string(),
            PhtArg::Text(text) => php_int_cast(text).to_string(),
        };
        out.extend_from_slice(text.as_bytes());
    } else {
        out.extend_from_slice(pht_arg_text(arg).as_bytes());
    }
}

/// Process one `%` placeholder at `index`; the next index after, or
/// `None` for an invalid translation.
fn vsprintf_step(
    bytes: &[u8],
    args: &[PhtArg],
    index: usize,
    sequential: &mut usize,
    out: &mut Vec<u8>,
) -> Option<usize> {
    let (positional, digits, cursor) = scan_placement(bytes, index);
    if cursor >= bytes.len() {
        out.push(b'%');
        out.extend_from_slice(digits.as_bytes());
        return Some(bytes.len());
    }
    let conversion = bytes[cursor];
    if conversion == b'%' && positional == 0 && digits.is_empty() {
        out.push(b'%');
        return Some(cursor + 1);
    }
    if !matches!(conversion, b's' | b'd') {
        return None;
    }
    let which = if positional > 0 {
        positional - 1
    } else {
        let next = *sequential;
        *sequential += 1;
        next
    };
    let arg = args.get(which)?;
    push_pht_arg(out, conversion, arg);
    Some(cursor + 1)
}

/// Minimal `vsprintf`: `%%`, `%s`, `%d`, and `%N$s`/`%N$d`.
fn vsprintf(translation: &str, args: &[PhtArg]) -> String {
    let bytes = translation.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut sequential = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        match vsprintf_step(bytes, args, index, &mut sequential, &mut out) {
            Some(next) => index = next,
            None => return format!("[Invalid Translation!] {translation}"),
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Translate (US English) and substitute, with plural variants.
#[must_use]
pub fn pht(pattern: &str, args: &[PhtArg]) -> String {
    let translation = match lookup_translation(pattern) {
        None => pattern.to_string(),
        Some(Variant::Pair(singular, plural)) => {
            let number = match plural_number(args, 0, pattern) {
                Ok(number) => number,
                Err(invalid) => return invalid,
            };
            (if number == 1 { singular } else { plural }).to_string()
        }
        Some(Variant::Nested(rows)) => {
            let first = match plural_number(args, 0, pattern) {
                Ok(number) => number,
                Err(invalid) => return invalid,
            };
            let second = match plural_number(args, 1, pattern) {
                Ok(number) => number,
                Err(invalid) => return invalid,
            };
            let row = if first == 1 { &rows[0] } else { &rows[1] };
            (if second == 1 { row[0] } else { row[1] }).to_string()
        }
    };
    vsprintf(&translation, args)
}

fn is_combining(codepoint: u32) -> bool {
    (0x0300..=0x036F).contains(&codepoint)
        || (0x1DC0..=0x1DFF).contains(&codepoint)
        || (0x20D0..=0x20FF).contains(&codepoint)
        || (0xFE20..=0xFE2F).contains(&codepoint)
}

/// Split into glyphs (base characters with combiners attached).
fn combine_glyphs(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut buf: Option<String> = if is_combining(chars[0] as u32) {
        Some(" ".to_string())
    } else {
        None
    };
    for current in chars {
        if current < '\u{80}' || !is_combining(current as u32) {
            if let Some(done) = buf.take() {
                parts.push(done);
            }
            buf = Some(current.to_string());
        } else if let Some(active) = buf.as_mut() {
            active.push(current);
        }
    }
    if let Some(done) = buf {
        parts.push(done);
    }
    parts
}

fn strip_ansi_for_width(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1B && bytes.get(index + 1) == Some(&b'[') {
            let mut end = index + 2;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if bytes.get(end) == Some(&b'm') {
                index = end + 1;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn is_wide(codepoint: u32) -> bool {
    codepoint >= 0x1100
        && (codepoint <= 0x115F
            || codepoint == 0x2329
            || codepoint == 0x232A
            || ((0x2E80..=0xA4CF).contains(&codepoint) && codepoint != 0x303F)
            || (0xAC00..=0xD7A3).contains(&codepoint)
            || (0xF900..=0xFAFF).contains(&codepoint)
            || (0xFE10..=0xFE19).contains(&codepoint)
            || (0xFE30..=0xFE6F).contains(&codepoint)
            || (0xFF00..=0xFF60).contains(&codepoint)
            || (0xFFE0..=0xFFE6).contains(&codepoint)
            || (0x20000..=0x2FFFD).contains(&codepoint)
            || (0x30000..=0x3FFFD).contains(&codepoint))
}

/// Console display width, mirroring `phutil_utf8_console_strlen()`.
fn console_strlen(text: &str) -> usize {
    let stripped = strip_ansi_for_width(text);
    if stripped.bytes().all(|b| (0x01..=0x7F).contains(&b)) {
        return stripped.len();
    }
    let mut len = 0;
    for glyph in combine_glyphs(&stripped) {
        let first = glyph
            .chars()
            .next()
            .expect("combine_glyphs only pushes nonempty buffers");
        let codepoint = first as u32;
        if codepoint == 0 {
            continue;
        }
        len += 1;
        if is_wide(codepoint) {
            len += 1;
        }
    }
    len
}

/// Truncate to `max` glyphs with word-boundary backoff, mirroring
/// `PhutilUTF8StringTruncator` in glyph mode (terminator `…`).
fn truncate_glyphs(text: &str, max: usize) -> String {
    const TERMINATOR: &str = "…";
    if text.len() <= max {
        return text.to_string();
    }
    let glyphs = combine_glyphs(text);
    if glyphs.len() <= max {
        return text.to_string();
    }
    let cutoff = max.saturating_sub(1);
    if cutoff == 0 {
        return TERMINATOR.to_string();
    }
    let search = cutoff.div_ceil(2).max(cutoff.saturating_sub(256));
    let mut word_boundary: Option<usize> = None;
    let mut stop_boundary: Option<usize> = None;
    let mut any_nonboundary = false;
    let mut index = cutoff.min(glyphs.len().saturating_sub(1)) as i64;
    while index >= search as i64 {
        let glyph = &glyphs[index as usize];
        if matches!(
            glyph.as_str(),
            " " | "\n" | ";" | ":" | "[" | "(" | "," | "-"
        ) {
            word_boundary = Some(index as usize);
        } else if matches!(glyph.as_str(), "." | "!" | "?") {
            stop_boundary = Some(index as usize + 1);
            break;
        } else {
            any_nonboundary = true;
            if word_boundary.is_some() {
                break;
            }
        }
        index -= 1;
    }
    if let Some(stop) = stop_boundary {
        return glyphs[..stop].concat();
    }
    let end = match word_boundary {
        Some(at) if any_nonboundary => at,
        _ => cutoff,
    };
    format!("{}{TERMINATOR}", glyphs[..end].concat())
}

/// A ref view (`ArcanistRefView`): object name, title, URI, lines, children.
#[derive(Debug, Clone, Default)]
pub struct RefView {
    pub object_name: Option<String>,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub lines: Vec<String>,
    pub children: Vec<RefView>,
}

impl RefView {
    /// Render at `indent` levels for a `width`-column console.
    #[must_use]
    pub fn render(&self, indent: usize, width: usize, ansi: bool) -> String {
        let reserve = self
            .object_name
            .as_ref()
            .map(|name| console_strlen(name) + 1)
            .unwrap_or(0);
        let indent_text = "  ".repeat(indent);
        let usable =
            (width as i64 - 6 - reserve as i64 - indent_text.len() as i64).max(16) as usize;
        let title = self.title.as_ref().map(|title| {
            if title.is_empty() {
                title.clone()
            } else {
                truncate_glyphs(title, usable)
            }
        });
        let display = match (&self.object_name, &title) {
            (Some(name), Some(title)) if !title.is_empty() => {
                tsprintf_ansi("**%s** %s", &[TVal::Text(name), TVal::Text(title)], ansi)
                    .unwrap_or_default()
            }
            (Some(name), _) => {
                tsprintf_ansi("**%s**", &[TVal::Text(name)], ansi).unwrap_or_default()
            }
            (None, title) => {
                TerminalString::new(escape_value(title.as_deref().unwrap_or(""), false))
            }
        };
        let mut out = tsprintf_ansi(
            "<bg:cyan>**  *  **</bg> %s%s\n",
            &[TVal::Text(&indent_text), TVal::Terminal(&display)],
            ansi,
        )
        .map(|t| t.to_string())
        .unwrap_or_default();
        if let Some(uri) = &self.uri {
            out.push_str(
                &tsprintf_ansi(
                    "<bg:cyan>** :// **</bg>   %s__%s__\n",
                    &[TVal::Text(&indent_text), TVal::Text(uri)],
                    ansi,
                )
                .map(|t| t.to_string())
                .unwrap_or_default(),
            );
        }
        for line in &self.lines {
            out.push_str(
                &tsprintf_ansi(
                    "        %s%s\n",
                    &[TVal::Text(&indent_text), TVal::Text(line)],
                    ansi,
                )
                .map(|t| t.to_string())
                .unwrap_or_default(),
            );
        }
        for child in &self.children {
            out.push_str(&child.render(indent + 1, width, ansi));
        }
        out
    }
}

/// A console-table cell: preformatted terminal text (raw) or plain text
/// (escaped with whitespace allowed, as `%B`).
#[derive(Debug, Clone)]
pub enum TableCell {
    Terminal(TerminalString),
    Text(String),
}

impl TableCell {
    fn display(&self) -> String {
        match self {
            TableCell::Terminal(text) => text.to_string(),
            TableCell::Text(text) => escape_value(text, true),
        }
    }
}

/// A borderless left-aligned console table, mirroring `PhutilConsoleTable`
/// with padding 1, no borders, and no header row. `titles` still size the
/// columns. Every row ends with `\n`; the last column is never padded.
#[must_use]
pub fn console_table(titles: &[&str], rows: &[Vec<TableCell>]) -> String {
    let mut widths: Vec<usize> = titles.iter().map(|title| console_strlen(title)).collect();
    let rendered: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            titles
                .iter()
                .enumerate()
                .map(|(index, _)| row.get(index).map(TableCell::display).unwrap_or_default())
                .collect()
        })
        .collect();
    for row in &rendered {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(console_strlen(cell));
        }
    }
    let mut out = String::new();
    for row in rendered {
        let last = row.len().saturating_sub(1);
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            out.push_str(cell);
            if index != last {
                let pad = widths[index].saturating_sub(console_strlen(cell));
                out.push_str(&" ".repeat(pad));
            }
        }
        out.push('\n');
    }
    out
}

/// One log-engine line for `color` (`yellow`/`red`/`green`/`blue`/
/// `magenta`/`cyan`), on stderr like PHP.
#[must_use]
pub fn format_log(color: &str, label: &str, message: &str, ansi: bool) -> String {
    tsprintf_ansi(
        &format!("**<bg:{color}> %s </bg>** %s\n"),
        &[TVal::Text(label), TVal::Text(message)],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default()
}

/// `phutil_console_format()` with `%s` values: interpret the pattern,
/// then substitute raw (no escaping).
#[must_use]
pub fn console_format(pattern: &str, values: &[&str], ansi: bool) -> String {
    let interpreted = interpret(pattern, ansi);
    let mut out = String::new();
    let mut rest = interpreted.as_str();
    let mut index = 0;
    while let Some(at) = rest.find("%s") {
        out.push_str(&rest[..at]);
        out.push_str(values.get(index).copied().unwrap_or(""));
        rest = &rest[at + 2..];
        index += 1;
    }
    out.push_str(rest);
    out
}

/// One `writeStatusMessage` line (`writeInfo`/`writeWarn`/`writeOkay`),
/// which strips like a log line but nests the ANSI codes in the
/// opposite order on TTYs.
#[must_use]
pub fn format_status_message(color: &str, label: &str, message: &str, ansi: bool) -> String {
    console_format(
        &format!("<bg:{color}>** %s **</bg> %s\n"),
        &[label, message],
        ansi,
    )
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod terminal_tests;
