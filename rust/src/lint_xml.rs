//! `xml` linter: XML files must be well-formed.
//!
//! PHP reference: `src/lint/linter/ArcanistXMLLinter.php` (`XML`), which
//! reports libxml diagnostics. This port implements a dependency-free
//! well-formedness checker covering the XML declaration, comments,
//! processing instructions, CDATA sections, `DOCTYPE` (skipped, never
//! validated), elements, attributes, and entity references.
//!
//! Self-filter: only `*.xml` paths (case-insensitive) are checked; anything
//! else passes silently. Binary (NUL) data is skipped, mirroring the PHP
//! engine's treatment of linters with `shouldLintBinaryFiles() === false`.
//!
//! Known limits: no encoding/transcoding validation (non-ASCII bytes are
//! accepted structurally), no DTD validation, no namespace processing,
//! general entity references are rejected (only the five predefined
//! entities and character references are accepted), and element nesting is
//! capped at [`MAX_DEPTH`] as a recursion guard.
//! Diagnostics carry a synthetic `XML1` code: libxml error codes and warning
//! levels have no counterpart here, so every finding is severity `error`
//! (matching PHP's treatment of `LIBXML_ERR_ERROR`/`LIBXML_ERR_FATAL`).

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};

const CODE_XML_ERROR: &str = "XML1";
const NAME_XML_ERROR: &str = "LibXML Error";

/// Maximum element nesting before the checker bails out (recursion guard,
/// mirroring libxml2's 256-level parser limit).
pub const MAX_DEPTH: usize = 256;

/// Linter reporting XML well-formedness errors (PHP `XML`).
#[derive(Debug, Default)]
pub struct XmlLinter;

impl XmlLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// True when `path` names a `.xml` file (ASCII case-insensitive).
#[must_use]
pub fn is_xml_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".xml")
}

/// A well-formedness failure at a 1-based (line, column) byte position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// Check `data` for XML well-formedness, returning the first error found.
pub fn check_well_formed(data: &[u8]) -> Result<(), XmlError> {
    let mut cursor = Cursor::new(data);
    cursor.skip_bom();
    if cursor.at_xml_decl() {
        cursor.parse_xml_decl()?;
    }
    cursor.parse_misc()?;
    if cursor.at_doctype() {
        cursor.parse_doctype()?;
        cursor.parse_misc()?;
    }
    if cursor.eof() {
        let (line, col) = cursor.position();
        return Err(cursor.error_at(line, col, "no root element"));
    }
    if !cursor.at_start_tag() {
        let (line, col) = cursor.position();
        return Err(cursor.error_at(line, col, "expected root element"));
    }
    let mut stack = Vec::new();
    cursor.parse_element(&mut stack, 0)?;
    cursor.parse_misc()?;
    if !cursor.eof() {
        let (line, col) = cursor.position();
        let message = if cursor.at_doctype() {
            "DOCTYPE is only allowed before the root element"
        } else if cursor.starts_with(b"<") {
            "only one root element is allowed"
        } else {
            "content after root element"
        };
        return Err(cursor.error_at(line, col, message));
    }
    Ok(())
}

impl Linter for XmlLinter {
    fn name(&self) -> &'static str {
        "XML"
    }

    fn applies_custom_severities(&self) -> bool {
        // The PHP linter sets severities directly and never consults
        // custom severity maps.
        false
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_xml_path(path) || data.contains(&0) || is_skippable_filesystem_object(path) {
            return Vec::new();
        }
        match check_well_formed(data) {
            Ok(()) => Vec::new(),
            Err(error) => vec![LintMessage {
                path: path.to_string(),
                line: error.line,
                col: error.col,
                code: CODE_XML_ERROR.to_string(),
                severity: arc_rs::lint::SEVERITY_ERROR.to_string(),
                name: NAME_XML_ERROR.to_string(),
                description: error.message,
                original: None,
                replacement: None,
                granularity: None,
                locations: Vec::new(),
                bypass_changed_line_filtering: None,
                original_bytes: None,
                replacement_bytes: None,
            }],
        }
    }
}

/// True only when `path` positively resolves to a directory or symlink (the
/// PHP linter never sees those). Missing paths return false so content
/// checks still run for display-only paths.
fn is_skippable_filesystem_object(path: &str) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.file_type().is_symlink() || meta.file_type().is_dir(),
        Err(_) => false,
    }
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':') || byte >= 0x80
}

fn is_name_char(byte: u8) -> bool {
    is_name_start(byte) || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
}

fn hex_val(byte: u8) -> u32 {
    match byte {
        b'0'..=b'9' => u32::from(byte - b'0'),
        b'a'..=b'f' => u32::from(byte - b'a') + 10,
        b'A'..=b'F' => u32::from(byte - b'A') + 10,
        _ => 0,
    }
}

/// The five predefined XML entities.
fn is_predefined_entity(body: &[u8]) -> bool {
    body == b"amp" || body == b"lt" || body == b"gt" || body == b"apos" || body == b"quot"
}

/// Value of a `#...` character reference body: an optional `x` radix marker
/// plus up to 8 well-formed digits. `None` when malformed.
fn char_ref_value(digits: &[u8]) -> Option<u32> {
    let (digits, radix) = match digits.first() {
        Some(b'x' | b'X') => (&digits[1..], 16),
        _ => (digits, 10),
    };
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let well_formed = if radix == 16 {
        digits.iter().all(|byte| byte.is_ascii_hexdigit())
    } else {
        digits.iter().all(|byte| byte.is_ascii_digit())
    };
    if !well_formed {
        return None;
    }
    Some(digits.iter().fold(0u32, |acc, byte| {
        acc.wrapping_mul(radix).wrapping_add(hex_val(*byte))
    }))
}

/// Code points no character reference may denote.
fn is_forbidden_char_ref(value: u32) -> bool {
    value == 0
        || value > 0x10FFFF
        || (0xD800..0xE000).contains(&value)
        || value == 0xFFFE
        || value == 0xFFFF
        || (value < 0x20 && !matches!(value, 0x09 | 0x0A | 0x0D))
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn position(&self) -> (usize, usize) {
        (self.line, self.col)
    }

    fn error_at(&self, line: usize, col: usize, message: impl Into<String>) -> XmlError {
        XmlError {
            line,
            col,
            message: message.into(),
        }
    }

    fn error_here(&self, message: impl Into<String>) -> XmlError {
        self.error_at(self.line, self.col, message)
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.data[self.pos..].starts_with(prefix)
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos)?;
        self.pos += 1;
        if byte == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(byte)
    }

    fn bump_n(&mut self, n: usize) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn skip_bom(&mut self) {
        if self.starts_with(b"\xef\xbb\xbf") {
            self.pos += 3;
            self.col += 3;
        }
    }

    fn skip_whitespace(&mut self) -> bool {
        let mut any = false;
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.bump();
            any = true;
        }
        any
    }

    /// Reject bytes that are never legal XML 1.0 characters.
    fn check_text_char(&self) -> Result<(), XmlError> {
        match self.peek() {
            Some(b'\t' | b'\n' | b'\r') | None => Ok(()),
            Some(byte) if byte < 0x20 || byte == 0x7f => {
                Err(self.error_here("invalid character in content"))
            }
            Some(_) => Ok(()),
        }
    }

    fn at_xml_decl(&self) -> bool {
        if !self.starts_with(b"<?xml") {
            return false;
        }
        matches!(
            self.data.get(self.pos + 5).copied(),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        )
    }

    fn at_doctype(&self) -> bool {
        if !self.starts_with(b"<!DOCTYPE") {
            return false;
        }
        matches!(
            self.data.get(self.pos + 9).copied(),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        )
    }

    fn at_start_tag(&self) -> bool {
        self.starts_with(b"<")
            && !self.starts_with(b"</")
            && !self.starts_with(b"<?")
            && !self.starts_with(b"<!")
    }

    fn parse_name(&mut self) -> Result<Vec<u8>, XmlError> {
        let start = self.pos;
        match self.peek() {
            Some(byte) if is_name_start(byte) => {
                self.bump();
            }
            _ => return Err(self.error_here("expected a name")),
        }
        while let Some(byte) = self.peek() {
            if is_name_char(byte) {
                self.bump();
            } else {
                break;
            }
        }
        Ok(self.data[start..self.pos].to_vec())
    }

    fn parse_eq(&mut self) -> Result<(), XmlError> {
        self.skip_whitespace();
        if self.peek() == Some(b'=') {
            self.bump();
        } else {
            return Err(self.error_here("expected '='"));
        }
        self.skip_whitespace();
        Ok(())
    }

    /// Attribute value: entity references are validated, `<` is rejected.
    fn parse_att_value(&mut self) -> Result<(), XmlError> {
        let quote = match self.peek() {
            Some(found @ (b'\'' | b'"')) => {
                self.bump();
                found
            }
            _ => return Err(self.error_here("expected a quoted attribute value")),
        };
        loop {
            match self.peek() {
                None => return Err(self.error_here("unterminated attribute value")),
                Some(byte) if byte == quote => {
                    self.bump();
                    return Ok(());
                }
                Some(b'<') => return Err(self.error_here("invalid '<' in attribute value")),
                Some(b'&') => self.parse_reference()?,
                Some(_) => {
                    self.check_text_char()?;
                    self.bump();
                }
            }
        }
    }

    /// Raw quoted string for declarations: consume through the matching
    /// quote without interpreting references.
    fn parse_quoted_raw(&mut self) -> Result<Vec<u8>, XmlError> {
        let quote = match self.peek() {
            Some(found @ (b'\'' | b'"')) => {
                self.bump();
                found
            }
            _ => return Err(self.error_here("expected a quoted string")),
        };
        let start = self.pos;
        loop {
            match self.peek() {
                None => return Err(self.error_here("unterminated quoted string")),
                Some(byte) if byte == quote => {
                    let out = self.data[start..self.pos].to_vec();
                    self.bump();
                    return Ok(out);
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    fn parse_reference(&mut self) -> Result<(), XmlError> {
        // Precondition: the current byte is '&'.
        let (line, col) = self.position();
        let body = self.scan_reference_body(line, col)?;
        self.check_reference_body(&body, line, col)
    }

    /// Consume `&...;`, returning the raw body between them.
    fn scan_reference_body(&mut self, line: usize, col: usize) -> Result<Vec<u8>, XmlError> {
        self.bump(); // '&'
        let start = self.pos;
        loop {
            match self.peek() {
                None => return Err(self.error_at(line, col, "unterminated entity reference")),
                Some(b';') => break,
                Some(byte) if byte.is_ascii_alphanumeric() || byte == b'#' => {
                    self.bump();
                }
                _ => return Err(self.error_at(line, col, "invalid entity reference")),
            }
        }
        let body = self.data[start..self.pos].to_vec();
        self.bump(); // Consume ';'.
        Ok(body)
    }

    /// Validate a scanned reference body: a predefined entity or a
    /// well-formed, in-range character reference.
    fn check_reference_body(&self, body: &[u8], line: usize, col: usize) -> Result<(), XmlError> {
        if is_predefined_entity(body) {
            return Ok(());
        }
        let Some(digits) = body.strip_prefix(b"#") else {
            return Err(self.error_at(line, col, "unknown entity"));
        };
        let Some(value) = char_ref_value(digits) else {
            return Err(self.error_at(line, col, "invalid character reference"));
        };
        if is_forbidden_char_ref(value) {
            return Err(self.error_at(line, col, "character reference out of range"));
        }
        Ok(())
    }

    fn parse_comment(&mut self) -> Result<(), XmlError> {
        // Precondition: starts with "<!--".
        let (line, col) = self.position();
        self.bump_n(4);
        loop {
            if self.eof() {
                return Err(self.error_at(line, col, "unterminated comment"));
            }
            if self.starts_with(b"-->") {
                self.bump_n(3);
                return Ok(());
            }
            if self.starts_with(b"--") {
                return Err(self.error_here("'--' is not allowed inside a comment"));
            }
            self.bump();
        }
    }

    fn parse_pi(&mut self) -> Result<(), XmlError> {
        // Precondition: starts with "<?".
        let (line, col) = self.position();
        self.bump_n(2);
        let target = match self.parse_name() {
            Ok(name) => name,
            Err(_) => return Err(self.error_at(line, col, "invalid processing instruction")),
        };
        if target.len() == 3
            && target[0].eq_ignore_ascii_case(&b'x')
            && target[1].eq_ignore_ascii_case(&b'm')
            && target[2].eq_ignore_ascii_case(&b'l')
        {
            return Err(self.error_at(line, col, "reserved processing instruction target"));
        }
        if self.starts_with(b"?>") {
            self.bump_n(2);
            return Ok(());
        }
        if !self.skip_whitespace() {
            return Err(self.error_here("expected whitespace in processing instruction"));
        }
        loop {
            if self.eof() {
                return Err(self.error_at(line, col, "unterminated processing instruction"));
            }
            if self.starts_with(b"?>") {
                self.bump_n(2);
                return Ok(());
            }
            self.bump();
        }
    }

    fn parse_cdata(&mut self) -> Result<(), XmlError> {
        // Precondition: starts with "<![CDATA[".
        let (line, col) = self.position();
        self.bump_n(9);
        loop {
            if self.eof() {
                return Err(self.error_at(line, col, "unterminated CDATA section"));
            }
            if self.starts_with(b"]]>") {
                self.bump_n(3);
                return Ok(());
            }
            self.bump();
        }
    }

    fn parse_xml_decl(&mut self) -> Result<(), XmlError> {
        // Precondition: `at_xml_decl()`.
        let (line, col) = self.position();
        self.bump_n(5); // "<?xml"
        self.skip_whitespace();
        self.parse_xml_decl_version(line, col)?;
        self.parse_xml_decl_attrs(line, col)
    }

    /// The mandatory `version="1.0"|"1.1"` opening of an XML declaration.
    fn parse_xml_decl_version(&mut self, line: usize, col: usize) -> Result<(), XmlError> {
        let version_name = match self.parse_name() {
            Ok(name) => name,
            Err(_) => return Err(self.error_at(line, col, "invalid XML declaration")),
        };
        if version_name != b"version" {
            return Err(self.error_at(line, col, "XML declaration must start with version"));
        }
        self.parse_eq()
            .map_err(|_| self.error_at(line, col, "invalid XML declaration"))?;
        let version = self
            .parse_quoted_raw()
            .map_err(|_| self.error_at(line, col, "invalid XML declaration"))?;
        if version != b"1.0" && version != b"1.1" {
            return Err(self.error_at(line, col, "unsupported XML version"));
        }
        Ok(())
    }

    /// Optional `encoding`/`standalone` attributes plus the closing `?>`.
    fn parse_xml_decl_attrs(&mut self, line: usize, col: usize) -> Result<(), XmlError> {
        let mut saw_encoding = false;
        let mut saw_standalone = false;
        loop {
            if self.starts_with(b"?>") {
                self.bump_n(2);
                return Ok(());
            }
            if self.eof() {
                return Err(self.error_at(line, col, "unterminated XML declaration"));
            }
            if !self.skip_whitespace() {
                return Err(self.error_here("expected whitespace in XML declaration"));
            }
            if self.starts_with(b"?>") {
                self.bump_n(2);
                return Ok(());
            }
            let attr = match self.parse_name() {
                Ok(name) => name,
                Err(_) => return Err(self.error_at(line, col, "invalid XML declaration")),
            };
            self.parse_eq()
                .map_err(|_| self.error_at(line, col, "invalid XML declaration"))?;
            let value = self
                .parse_quoted_raw()
                .map_err(|_| self.error_at(line, col, "invalid XML declaration"))?;
            self.check_xml_decl_attr(
                &attr,
                &value,
                &mut saw_encoding,
                &mut saw_standalone,
                line,
                col,
            )?;
        }
    }

    /// Validate one `encoding`/`standalone` declaration attribute in order:
    /// `encoding` first and non-empty, `standalone` once as `yes`/`no`.
    fn check_xml_decl_attr(
        &self,
        attr: &[u8],
        value: &[u8],
        saw_encoding: &mut bool,
        saw_standalone: &mut bool,
        line: usize,
        col: usize,
    ) -> Result<(), XmlError> {
        if attr == b"encoding" {
            if *saw_encoding || *saw_standalone || value.is_empty() {
                return Err(self.error_at(line, col, "invalid encoding declaration"));
            }
            *saw_encoding = true;
        } else if attr == b"standalone" {
            if *saw_standalone || (value != b"yes" && value != b"no") {
                return Err(self.error_at(line, col, "invalid standalone declaration"));
            }
            *saw_standalone = true;
        } else {
            return Err(self.error_at(line, col, "unknown XML declaration attribute"));
        }
        Ok(())
    }

    fn parse_doctype(&mut self) -> Result<(), XmlError> {
        // Precondition: `at_doctype()`. The declaration is skipped, never
        // validated (matching "well-formedness only" scope).
        let (line, col) = self.position();
        self.bump_n(9); // "<!DOCTYPE"
        self.skip_whitespace();
        if self.parse_name().is_err() {
            return Err(self.error_at(line, col, "invalid DOCTYPE"));
        }
        self.skip_whitespace();
        if self.starts_with(b"[") {
            self.bump();
            self.skip_internal_subset()?;
            self.skip_whitespace();
        } else {
            loop {
                if self.starts_with(b">") {
                    break;
                }
                if self.eof() {
                    return Err(self.error_at(line, col, "unterminated DOCTYPE"));
                }
                match self.peek() {
                    Some(b'\'' | b'"') => {
                        self.parse_quoted_raw()?;
                    }
                    Some(byte) if is_name_start(byte) => {
                        self.parse_name()?;
                    }
                    _ => return Err(self.error_here("invalid DOCTYPE")),
                }
                self.skip_whitespace();
            }
        }
        if self.peek() == Some(b'>') {
            self.bump();
            Ok(())
        } else {
            Err(self.error_at(line, col, "unterminated DOCTYPE"))
        }
    }

    fn skip_internal_subset(&mut self) -> Result<(), XmlError> {
        // Precondition: the opening "[" was consumed.
        let (line, col) = self.position();
        loop {
            if self.eof() {
                return Err(self.error_at(line, col, "unterminated DOCTYPE"));
            }
            if self.starts_with(b"<!--") {
                self.parse_comment()?;
            } else if self.starts_with(b"<?") {
                self.parse_pi()?;
            } else if matches!(self.peek(), Some(b'\'' | b'"')) {
                self.parse_quoted_raw()?;
            } else if self.starts_with(b"]") {
                self.bump();
                return Ok(());
            } else {
                self.bump();
            }
        }
    }

    fn parse_misc(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_whitespace();
            if self.starts_with(b"<!--") {
                self.parse_comment()?;
            } else if self.starts_with(b"<?") {
                self.parse_pi()?;
            } else {
                return Ok(());
            }
        }
    }

    fn parse_attribute(&mut self, seen: &mut Vec<Vec<u8>>) -> Result<(), XmlError> {
        let name = self.parse_name()?;
        if seen.contains(&name) {
            return Err(self.error_here("duplicate attribute"));
        }
        seen.push(name);
        self.parse_eq()?;
        self.parse_att_value()?;
        Ok(())
    }

    fn parse_element(&mut self, stack: &mut Vec<Vec<u8>>, depth: usize) -> Result<(), XmlError> {
        // Precondition: `at_start_tag()`.
        if depth > MAX_DEPTH {
            return Err(self.error_here("element nesting too deep"));
        }
        let (line, col) = self.position();
        let (name, empty) = self.parse_start_tag(line, col)?;
        if empty {
            return Ok(());
        }
        stack.push(name);
        self.parse_element_content(stack, depth)
    }

    /// The `<name ...>` opener: the element name plus whether it self-closes
    /// (`/>`). Attribute duplicates are rejected.
    fn parse_start_tag(&mut self, line: usize, col: usize) -> Result<(Vec<u8>, bool), XmlError> {
        self.bump(); // '<'
        let name = match self.parse_name() {
            Ok(name) => name,
            Err(_) => return Err(self.error_at(line, col, "invalid start tag")),
        };
        let mut attrs = Vec::new();
        loop {
            let saw_ws = self.skip_whitespace();
            match self.peek() {
                Some(b'/') => {
                    self.bump();
                    if self.peek() == Some(b'>') {
                        self.bump();
                        return Ok((name, true));
                    }
                    return Err(self.error_here("expected '>'"));
                }
                Some(b'>') => {
                    self.bump();
                    return Ok((name, false));
                }
                Some(_) if saw_ws => self.parse_attribute(&mut attrs)?,
                Some(_) => return Err(self.error_here("expected whitespace before attribute")),
                None => return Err(self.error_at(line, col, "unterminated start tag")),
            }
        }
    }

    /// Element content up to the matching end tag: nested markup,
    /// references, and text.
    fn parse_element_content(
        &mut self,
        stack: &mut Vec<Vec<u8>>,
        depth: usize,
    ) -> Result<(), XmlError> {
        loop {
            match self.peek() {
                None => {
                    let open = stack.last().cloned().unwrap_or_default();
                    let open = String::from_utf8_lossy(&open);
                    return Err(self.error_here(format!("unclosed element <{open}>")));
                }
                Some(b'<') => {
                    if self.parse_content_markup(stack, depth)? {
                        return Ok(());
                    }
                }
                Some(b'&') => self.parse_reference()?,
                Some(_) => {
                    self.check_text_char()?;
                    self.bump();
                }
            }
        }
    }

    /// One `<...` construct in content. Returns true when the element's own
    /// end tag closed it. Precondition: the current byte is `<`.
    fn parse_content_markup(
        &mut self,
        stack: &mut Vec<Vec<u8>>,
        depth: usize,
    ) -> Result<bool, XmlError> {
        if self.starts_with(b"<!--") {
            self.parse_comment()?;
            Ok(false)
        } else if self.starts_with(b"<![CDATA[") {
            self.parse_cdata()?;
            Ok(false)
        } else if self.starts_with(b"<?") {
            self.parse_pi()?;
            Ok(false)
        } else if self.starts_with(b"</") {
            self.parse_end_tag(stack)?;
            Ok(true)
        } else if self.starts_with(b"<!") {
            Err(self.error_here("markup declaration is not allowed here"))
        } else {
            self.parse_element(stack, depth + 1)?;
            Ok(false)
        }
    }

    /// The `</name>` closer: pop the open element and match names.
    /// Precondition: starts with `</`.
    fn parse_end_tag(&mut self, stack: &mut Vec<Vec<u8>>) -> Result<(), XmlError> {
        let (end_line, end_col) = self.position();
        self.bump_n(2);
        let end = match self.parse_name() {
            Ok(name) => name,
            Err(_) => return Err(self.error_at(end_line, end_col, "invalid end tag")),
        };
        self.skip_whitespace();
        if self.peek() != Some(b'>') {
            return Err(self.error_here("expected '>'"));
        }
        self.bump();
        let open = stack.pop().unwrap_or_default();
        if open != end {
            let open = String::from_utf8_lossy(&open);
            return Err(self.error_at(
                end_line,
                end_col,
                format!("mismatched end tag: expected </{open}>"),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "lint_xml_tests.rs"]
mod lint_xml_tests;
