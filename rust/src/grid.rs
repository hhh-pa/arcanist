//! Console grid rendering and console-string helpers, mirroring the PHP
//! console and UTF-8 utilities used by the markers workflows.
//!
//! PHP reference: `src/console/grid/ArcanistGridView.php`,
//! `src/console/grid/ArcanistGridColumn.php`,
//! `src/console/grid/ArcanistGridRow.php`,
//! `src/console/grid/ArcanistGridCell.php` (layout),
//! `src/utils/PhutilUTF8StringTruncator.php` (truncation),
//! `src/utils/utf8.php` (`phutil_utf8_console_strlen`,
//! `phutil_utf8v_combine_characters`, `phutil_utf8_is_combining_character`),
//! `src/console/PhutilConsoleMetrics.php` (terminal width),
//! `src/utils/utils.php` (`phutil_split_lines`),
//! `src/internationalization/PhutilTranslator.php` (`formatNumber`).
//!
//! Divergences:
//! - PHP renders `tsprintf()` markup (bold, colors) when stdout is a TTY;
//!   cells here carry plain text only, matching the non-TTY rendering.
//! - `phutil_utf8_strtolower()` uses `mb_convert_case()` when available;
//!   [`utf8_lower`] uses Unicode full lowercase mapping, which only differs
//!   in locale-sensitive edge cases (Turkish/Azeri dotted I).

use std::collections::HashMap;

/// Split `text` on `\r\n` or `\n`, discarding one trailing empty piece.
///
/// Mirrors `phutil_split_lines($corpus, false)`: an empty corpus yields one
/// empty line.
#[must_use]
pub fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![""];
    }
    let mut lines: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find('\n') {
        let mut line = &rest[..index];
        line = line.strip_suffix('\r').unwrap_or(line);
        lines.push(line);
        rest = &rest[index + 1..];
    }
    lines.push(rest);
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// True for the combining codepoint ranges in
/// `phutil_utf8_is_combining_character()`.
fn is_combining(codepoint: u32) -> bool {
    (0x0300..=0x036F).contains(&codepoint)
        || (0x1DC0..=0x1DFF).contains(&codepoint)
        || (0x20D0..=0x20FF).contains(&codepoint)
        || (0xFE20..=0xFE2F).contains(&codepoint)
}

/// Group `chars` into glyphs, merging combining characters.
///
/// Mirrors `phutil_utf8v_combine_characters()`: when the string starts with
/// a combining character, the first glyph starts with a space.
fn combine_glyphs(chars: &[char]) -> Vec<Vec<char>> {
    if chars.is_empty() {
        return Vec::new();
    }
    let mut glyphs: Vec<Vec<char>> = Vec::new();
    let mut buf: Option<Vec<char>> = None;
    if is_combining(chars[0] as u32) {
        buf = Some(vec![' ']);
    }
    for &char in chars {
        let combining = char.len_utf8() > 1 && is_combining(char as u32);
        if combining {
            buf.get_or_insert_with(Vec::new).push(char);
        } else {
            if let Some(ready) = buf.take() {
                glyphs.push(ready);
            }
            buf = Some(vec![char]);
        }
    }
    if let Some(ready) = buf.take() {
        glyphs.push(ready);
    }
    glyphs
}

/// Wide-character ranges from `phutil_utf8_console_strlen()`, as
/// inclusive (lo, hi) pairs. `0x303F` is excluded separately.
const WIDE_RANGES: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x2329, 0x2329),
    (0x232A, 0x232A),
    (0x2E80, 0xA4CF),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE6F),
    (0xFF00, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x20000, 0x2FFFD),
    (0x30000, 0x3FFFD),
];

/// True when `codepoint` takes two console columns.
fn is_wide_codepoint(codepoint: u32) -> bool {
    if codepoint == 0x303F {
        return false;
    }
    for &(lo, hi) in WIDE_RANGES {
        if (lo..=hi).contains(&codepoint) {
            return true;
        }
    }
    false
}

/// Display width of the first codepoint of a glyph.
///
/// Mirrors the wide-character ranges in `phutil_utf8_console_strlen()`; a
/// NUL codepoint contributes nothing.
fn glyph_width(codepoint: u32) -> usize {
    if codepoint == 0 {
        return 0;
    }
    if is_wide_codepoint(codepoint) { 2 } else { 1 }
}

/// Strip ANSI color sequences (`ESC [ digits m`), like
/// `phutil_utf8_console_strlen()` does before measuring.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        let mut end = index + 2;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if bytes[index] == 0x1B
            && bytes.get(index + 1) == Some(&b'[')
            && bytes.get(end) == Some(&b'm')
        {
            index = end + 1;
            continue;
        }
        // Copy one whole UTF-8 sequence so multibyte text survives.
        let lead = bytes[index];
        let width = if lead < 0x80 {
            1
        } else if lead < 0xE0 {
            2
        } else if lead < 0xF0 {
            3
        } else {
            4
        };
        out.push_str(&text[index..index + width]);
        index += width;
    }
    out
}

/// Console display width of `text`.
///
/// Mirrors `phutil_utf8_console_strlen()`: ANSI color sequences contribute
/// nothing, ASCII counts one column per byte, other glyphs count one or two
/// columns by the wide-character table.
#[must_use]
pub fn console_width(text: &str) -> usize {
    let stripped = strip_ansi(text);
    if stripped.bytes().all(|byte| (0x01..=0x7F).contains(&byte)) {
        return stripped.len();
    }
    let chars: Vec<char> = stripped.chars().collect();
    combine_glyphs(&chars)
        .iter()
        .map(|glyph| glyph_width(glyph[0] as u32))
        .sum()
}

/// Lowercase `text`, mirroring `phutil_utf8_strtolower()`.
#[must_use]
pub fn utf8_lower(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Format an integer with thousands separators, like `PhutilNumber` with the
/// default locale (`number_format($n, 0, '.', ',')`).
#[must_use]
pub fn group_number(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Detect the terminal width in columns.
///
/// Mirrors `PhutilConsoleMetrics::computeTerminalWidth()`: always `None` on
/// Windows, otherwise the `tput cols` result when it parses to a positive
/// integer.
#[must_use]
pub fn terminal_width() -> Option<usize> {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(windows))]
    {
        terminal_width_from_tput()
    }
}

#[cfg(not(windows))]
fn terminal_width_from_tput() -> Option<usize> {
    let output = std::process::Command::new("tput")
        .arg("cols")
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let width: usize = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    if width > 0 { Some(width) } else { None }
}

/// The default truncation terminator (`…`), like the truncator default.
pub const TRUNCATOR_TERMINATOR: &str = "…";

fn is_break_char(glyph: &[char]) -> bool {
    matches!(
        glyph,
        [' '] | ['\n'] | [';'] | [':'] | ['['] | ['('] | [','] | ['-']
    )
}

fn is_stop_char(glyph: &[char]) -> bool {
    matches!(glyph, ['.'] | ['!'] | ['?'])
}

/// Truncate `text` to at most `maximum` glyphs.
///
/// Mirrors `PhutilUTF8StringTruncator` with only `setMaximumGlyphs()`:
/// strings within the limit return unmodified, longer strings are cut with
/// room for the `…` terminator, preferring word and sentence boundaries.
#[must_use]
pub fn truncate_glyphs(text: &str, maximum: usize) -> String {
    if text.len() <= maximum {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let glyphs = combine_glyphs(&chars);
    let glyph_len = glyphs.len();
    if glyph_len <= maximum {
        return text.to_string();
    }
    let cutoff = maximum.saturating_sub(1);
    if cutoff == 0 {
        return TRUNCATOR_TERMINATOR.to_string();
    }
    // T1153 RB-D35: PHP divides by two as a float (`$cutoff / 2`,
    // `src/utils/PhutilUTF8StringTruncator.php:204-209`), so odd cutoffs
    // round the search floor up (like `terminal`'s truncator).
    let search = cutoff.div_ceil(2).max(cutoff.saturating_sub(256));
    let mut word_boundary: Option<usize> = None;
    let mut stop_boundary: Option<usize> = None;
    let mut any_nonboundary = false;
    let mut index = cutoff.min(glyph_len.saturating_sub(1)) as isize;
    while index >= search as isize {
        let glyph = &glyphs[index as usize];
        if is_break_char(glyph) {
            word_boundary = Some(index as usize);
        } else if is_stop_char(glyph) {
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
        return glyphs[..stop.min(glyph_len)]
            .iter()
            .flat_map(|glyph| glyph.iter())
            .collect();
    }
    let boundary = match word_boundary {
        Some(found) if any_nonboundary => found,
        _ => cutoff,
    };
    let mut out: String = glyphs[..boundary.min(glyph_len)]
        .iter()
        .flat_map(|glyph| glyph.iter())
        .collect();
    out.push_str(TRUNCATOR_TERMINATOR);
    out
}

/// Column alignment (`ArcanistGridColumn::ALIGNMENT_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Left,
    Center,
    Right,
}

/// One grid column (`ArcanistGridColumn`); alignment defaults to left.
#[derive(Debug, Clone)]
pub struct GridColumn {
    key: String,
    alignment: Alignment,
    display_width: Option<usize>,
    minimum_width: Option<usize>,
}

impl GridColumn {
    #[must_use]
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            alignment: Alignment::Left,
            display_width: None,
            minimum_width: None,
        }
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn set_alignment(&mut self, alignment: Alignment) {
        self.alignment = alignment;
    }

    #[must_use]
    pub fn alignment(&self) -> Alignment {
        self.alignment
    }

    pub fn set_display_width(&mut self, width: usize) {
        self.display_width = Some(width);
    }

    pub fn set_minimum_width(&mut self, width: usize) {
        self.minimum_width = Some(width);
    }

    #[must_use]
    pub fn minimum_width(&self) -> Option<usize> {
        self.minimum_width
    }
}

/// One grid cell (`ArcanistGridCell`): content pieces joined, then split
/// into display lines.
#[derive(Debug, Clone)]
pub struct GridCell {
    key: String,
    content: Vec<String>,
}

impl GridCell {
    #[must_use]
    pub fn new(key: &str, content: Vec<String>) -> Self {
        Self {
            key: key.to_string(),
            content,
        }
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Display lines: pieces glued together, then split without endings.
    #[must_use]
    pub fn display_lines(&self) -> Vec<String> {
        let joined = self.content.concat();
        split_lines(&joined)
            .iter()
            .map(|line| (*line).to_string())
            .collect()
    }

    #[must_use]
    pub fn content_width(&self) -> usize {
        self.display_lines()
            .iter()
            .map(|line| console_width(line))
            .max()
            .unwrap_or(0)
    }
}

/// One grid row (`ArcanistGridRow`): cells keyed by column key.
#[derive(Debug, Clone, Default)]
pub struct GridRow {
    cells: HashMap<String, GridCell>,
}

impl GridRow {
    #[must_use]
    pub fn new(cells: Vec<GridCell>) -> Self {
        let mut map = HashMap::new();
        for cell in cells {
            map.insert(cell.key().to_string(), cell);
        }
        Self { cells: map }
    }

    #[must_use]
    pub fn has_cell(&self, key: &str) -> bool {
        self.cells.contains_key(key)
    }

    #[must_use]
    pub fn cell(&self, key: &str) -> Option<&GridCell> {
        self.cells.get(key)
    }
}

fn pad_line(line: &str, src_width: usize, dst_width: usize, alignment: Alignment) -> String {
    let delta = dst_width - src_width;
    let padding = " ".repeat(delta);
    match alignment {
        Alignment::Left => format!("{line}{padding}"),
        Alignment::Right => format!("{padding}{line}"),
        Alignment::Center => {
            let head = " ".repeat(delta / 2);
            let tail = " ".repeat(delta.div_ceil(2));
            format!("{head}{line}{tail}")
        }
    }
}

/// A console grid (`ArcanistGridView`).
#[derive(Debug, Default)]
pub struct GridView {
    columns: Vec<GridColumn>,
    rows: Vec<GridRow>,
}

impl GridView {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a column, returning a handle for width options.
    pub fn new_column(&mut self, key: &str) -> &mut GridColumn {
        self.columns.push(GridColumn::new(key));
        self.columns.last_mut().expect("just pushed")
    }

    pub fn new_row(&mut self, cells: Vec<GridCell>) {
        self.rows.push(GridRow::new(cells));
    }

    fn column_widths(&self, terminal: Option<usize>) -> Vec<usize> {
        let mut widths = Vec::new();
        let mut flexible: Vec<usize> = Vec::new();
        for (index, column) in self.columns.iter().enumerate() {
            let mut width = column.display_width.unwrap_or(1);
            if column.display_width.is_none() {
                for row in &self.rows {
                    if let Some(cell) = row.cell(column.key()) {
                        width = width.max(cell.content_width());
                    }
                }
            }
            if column.minimum_width.is_some() {
                flexible.push(index);
            }
            widths.push(width);
        }
        let available =
            terminal.unwrap_or(0) as i64 - 2 * (self.columns.len().saturating_sub(1) as i64);
        loop {
            let total: usize = widths.iter().sum();
            if total as i64 <= available {
                break;
            }
            if flexible.is_empty() {
                break;
            }
            // PHP shrinks only the first flexible column, once.
            let key = flexible[0];
            let need = total as i64 - available;
            let minimum = self.columns[key].minimum_width.unwrap_or(0);
            let shrunk = (widths[key] as i64 - need).max(minimum as i64).max(0) as usize;
            widths[key] = shrunk;
            flexible.clear();
        }
        widths
    }

    fn draw_row(&self, row: &GridRow, widths: &[usize]) -> Vec<String> {
        let mut cells: Vec<Vec<String>> = Vec::new();
        for (index, column) in self.columns.iter().enumerate() {
            let width = widths[index];
            let mut lines = match row.cell(column.key()) {
                Some(cell) => cell.display_lines(),
                None => vec![String::new()],
            };
            for line in &mut lines {
                let line_width = console_width(line);
                if line_width < width {
                    *line = pad_line(line, line_width, width, column.alignment());
                } else if line_width > width {
                    *line = truncate_glyphs(line, width);
                }
            }
            cells.push(lines);
        }
        let line_count = cells.iter().map(Vec::len).max().unwrap_or(0);
        let mut out = Vec::new();
        for line_index in 0..line_count {
            let mut parts = Vec::new();
            for (cell_index, cell) in cells.iter().enumerate() {
                match cell.get(line_index) {
                    Some(line) => parts.push(line.clone()),
                    None => parts.push(" ".repeat(widths[cell_index])),
                }
            }
            out.push(parts.join("  "));
        }
        out
    }

    /// Draw the grid: rows joined with newlines plus one trailing newline.
    ///
    /// T1153 RB-D36: a grid with no rows renders `"\n"` — `phutil_glue`
    /// returns `[]` for no rows and `%s` of it is `""`
    /// (`src/utils/utils.php:1955-1958`, `src/xsprintf/tsprintf.php` via
    /// `PhutilTerminalString::escapeStringValue`).
    #[must_use]
    pub fn draw(&self, terminal: Option<usize>) -> String {
        if self.columns.is_empty() {
            panic!("Can not draw a grid with no columns!");
        }
        if self.rows.is_empty() {
            return "\n".to_string();
        }
        let widths = self.column_widths(terminal);
        let mut out = String::new();
        for row in &self.rows {
            for line in self.draw_row(row, &widths) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "grid_tests.rs"]
mod grid_tests;
