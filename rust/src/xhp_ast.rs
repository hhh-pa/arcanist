//! XHPAST tree model, canned-output parser, PHP tokenizer, and rule
//! context for the XHP linter (T1128, divergence dv-c).
//!
//! PHP reference: `src/lint/linter/ArcanistXHPASTLinter.php`,
//! `ArcanistBaseXHPASTLinter.php`, and the rules under
//! `src/lint/linter/xhpast/rules/`. The XHPAST engine itself lives in
//! libphutil (not vendored), so the real `xhpast` wire format is
//! undocumented in-repo; this module defines a JSON equivalent and
//! derives rule structure from the token stream.
//!
//! Canned `xhpast` output (parsed by [`tree_from_canned`]) is JSON:
//! `{"source": "<php>", "tokens": [...], "nodes": [...]}`. `source` is
//! required. `tokens` is optional; each entry is either an object
//! `{"t": kind, "v": value, "o": offset}` or a compact triple
//! `[kind, value, offset]`. When `tokens` is absent the source is
//! tokenized locally with [`tokenize`]. `nodes` is an optional nested
//! AST (`{"t": kind, "s": start, "e": end, "c": [...]}`), exposed via
//! [`Tree::nodes_of_type`]; the ported rule tree derives structure
//! from tokens (see [`Tree::calls`] and [`Tree::blocks`]), so nodes
//! are informational.
//!
//! Tokenizer normalizations (vs PHP `token_get_all()`), which every
//! rule below is implemented against:
//! - `T_OPEN_TAG` is exactly `<?php` / `<?`; trailing whitespace is a
//!   separate `T_WHITESPACE` token (PHP attaches one whitespace byte).
//! - `T_CLOSE_TAG` is `?>` plus one immediately following newline
//!   when present (mirroring PHP, which consumes it).
//! - Each string literal is one token: `T_CONSTANT_ENCAPSED_STRING`
//!   for single-quoted and non-interpolated double-quoted strings,
//!   `T_INTERPOLATED_STRING` for double-quoted strings containing
//!   interpolation, `T_HEREDOC` / `T_NOWDOC` for heredocs, `T_BACKTICK`
//!   for backtick literals. Use [`string_variables`] for the
//!   interpolated variables (mirrors `getStringVariables()`).
//! - Numbers use maximal munch (`08`, `0b102` lex as one `T_LNUMBER`)
//!   so invalid literals stay detectable (XHP125).
//! - `self` / `parent` are `T_STRING`; `static` is `T_STATIC`;
//!   `true` / `false` / `null` are `T_STRING`; `exit` and `die` are
//!   `T_EXIT`.
//!
//! The rule-tree contract (shared with `xhp_calls`, `xhp_style`,
//! `xhp_decl`, `xhp_flow`): each group exposes
//! `pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>)`.
//! [`RawFinding`] carries the PHP rule ID, a byte offset, and the
//! exact PHP message text. Rule names/severities attach at emit time
//! in `lint_xhpast` from the metadata table. Findings sort by
//! (offset, rule) at emit; no autofix replacements are produced.

use std::cmp::Ordering;
use std::collections::HashMap;

/// Token kind constants mirroring PHP / XHPAST token names.
pub mod kinds {
    pub const T_OPEN_TAG: &str = "T_OPEN_TAG";
    pub const T_OPEN_TAG_WITH_ECHO: &str = "T_OPEN_TAG_WITH_ECHO";
    pub const T_CLOSE_TAG: &str = "T_CLOSE_TAG";
    pub const T_INLINE_HTML: &str = "T_INLINE_HTML";
    pub const T_WHITESPACE: &str = "T_WHITESPACE";
    pub const T_COMMENT: &str = "T_COMMENT";
    pub const T_DOC_COMMENT: &str = "T_DOC_COMMENT";
    pub const T_STRING: &str = "T_STRING";
    pub const T_VARIABLE: &str = "T_VARIABLE";
    pub const T_LNUMBER: &str = "T_LNUMBER";
    pub const T_DNUMBER: &str = "T_DNUMBER";
    pub const T_CONSTANT_ENCAPSED_STRING: &str = "T_CONSTANT_ENCAPSED_STRING";
    pub const T_INTERPOLATED_STRING: &str = "T_INTERPOLATED_STRING";
    pub const T_HEREDOC: &str = "T_HEREDOC";
    pub const T_NOWDOC: &str = "T_NOWDOC";
    pub const T_BACKTICK: &str = "T_BACKTICK";
    pub const T_DOUBLE_ARROW: &str = "T_DOUBLE_ARROW";
    pub const T_OBJECT_OPERATOR: &str = "T_OBJECT_OPERATOR";
    pub const T_NULLSAFE_OBJECT_OPERATOR: &str = "T_NULLSAFE_OBJECT_OPERATOR";
    pub const T_PAAMAYIM_NEKUDOTAYIM: &str = "T_PAAMAYIM_NEKUDOTAYIM";
    pub const T_ELLIPSIS: &str = "T_ELLIPSIS";
    pub const T_COALESCE: &str = "T_COALESCE";
    pub const T_SPACESHIP: &str = "T_SPACESHIP";
    pub const T_BOOLEAN_AND: &str = "T_BOOLEAN_AND";
    pub const T_BOOLEAN_OR: &str = "T_BOOLEAN_OR";
    pub const T_LOGICAL_AND: &str = "T_LOGICAL_AND";
    pub const T_LOGICAL_OR: &str = "T_LOGICAL_OR";
    pub const T_LOGICAL_XOR: &str = "T_LOGICAL_XOR";
    pub const T_IS_EQUAL: &str = "T_IS_EQUAL";
    pub const T_IS_NOT_EQUAL: &str = "T_IS_NOT_EQUAL";
    pub const T_IS_IDENTICAL: &str = "T_IS_IDENTICAL";
    pub const T_IS_NOT_IDENTICAL: &str = "T_IS_NOT_IDENTICAL";
    pub const T_INC: &str = "T_INC";
    pub const T_DEC: &str = "T_DEC";
    pub const T_POW: &str = "T_POW";
    pub const T_SL: &str = "T_SL";
    pub const T_SR: &str = "T_SR";
    pub const T_IF: &str = "T_IF";
    pub const T_ELSEIF: &str = "T_ELSEIF";
    pub const T_ELSE: &str = "T_ELSE";
    pub const T_ENDIF: &str = "T_ENDIF";
    pub const T_WHILE: &str = "T_WHILE";
    pub const T_ENDWHILE: &str = "T_ENDWHILE";
    pub const T_DO: &str = "T_DO";
    pub const T_FOR: &str = "T_FOR";
    pub const T_ENDFOR: &str = "T_ENDFOR";
    pub const T_FOREACH: &str = "T_FOREACH";
    pub const T_ENDFOREACH: &str = "T_ENDFOREACH";
    pub const T_SWITCH: &str = "T_SWITCH";
    pub const T_ENDSWITCH: &str = "T_ENDSWITCH";
    pub const T_CASE: &str = "T_CASE";
    pub const T_DEFAULT: &str = "T_DEFAULT";
    pub const T_BREAK: &str = "T_BREAK";
    pub const T_CONTINUE: &str = "T_CONTINUE";
    pub const T_RETURN: &str = "T_RETURN";
    pub const T_THROW: &str = "T_THROW";
    pub const T_TRY: &str = "T_TRY";
    pub const T_CATCH: &str = "T_CATCH";
    pub const T_FINALLY: &str = "T_FINALLY";
    pub const T_FUNCTION: &str = "T_FUNCTION";
    pub const T_FN: &str = "T_FN";
    pub const T_CLASS: &str = "T_CLASS";
    pub const T_INTERFACE: &str = "T_INTERFACE";
    pub const T_TRAIT: &str = "T_TRAIT";
    pub const T_ENUM: &str = "T_ENUM";
    pub const T_EXTENDS: &str = "T_EXTENDS";
    pub const T_IMPLEMENTS: &str = "T_IMPLEMENTS";
    pub const T_ABSTRACT: &str = "T_ABSTRACT";
    pub const T_FINAL: &str = "T_FINAL";
    pub const T_PUBLIC: &str = "T_PUBLIC";
    pub const T_PRIVATE: &str = "T_PRIVATE";
    pub const T_PROTECTED: &str = "T_PROTECTED";
    pub const T_STATIC: &str = "T_STATIC";
    pub const T_VAR: &str = "T_VAR";
    pub const T_CONST: &str = "T_CONST";
    pub const T_GLOBAL: &str = "T_GLOBAL";
    pub const T_NEW: &str = "T_NEW";
    pub const T_CLONE: &str = "T_CLONE";
    pub const T_INSTANCEOF: &str = "T_INSTANCEOF";
    pub const T_AS: &str = "T_AS";
    pub const T_USE: &str = "T_USE";
    pub const T_NAMESPACE: &str = "T_NAMESPACE";
    pub const T_ECHO: &str = "T_ECHO";
    pub const T_PRINT: &str = "T_PRINT";
    pub const T_EXIT: &str = "T_EXIT";
    pub const T_ISSET: &str = "T_ISSET";
    pub const T_EMPTY: &str = "T_EMPTY";
    pub const T_EVAL: &str = "T_EVAL";
    pub const T_INCLUDE: &str = "T_INCLUDE";
    pub const T_INCLUDE_ONCE: &str = "T_INCLUDE_ONCE";
    pub const T_REQUIRE: &str = "T_REQUIRE";
    pub const T_REQUIRE_ONCE: &str = "T_REQUIRE_ONCE";
    pub const T_LIST: &str = "T_LIST";
    pub const T_ARRAY: &str = "T_ARRAY";
    pub const T_DECLARE: &str = "T_DECLARE";
    pub const T_ENDDECLARE: &str = "T_ENDDECLARE";
    pub const T_GOTO: &str = "T_GOTO";
    pub const T_MATCH: &str = "T_MATCH";
    pub const T_YIELD: &str = "T_YIELD";
    pub const T_HALT_COMPILER: &str = "T_HALT_COMPILER";
    pub const T_CLASS_C: &str = "T_CLASS_C";
    pub const T_FUNC_C: &str = "T_FUNC_C";
    pub const T_METHOD_C: &str = "T_METHOD_C";
    pub const T_FILE: &str = "T_FILE";
    pub const T_DIR: &str = "T_DIR";
    pub const T_LINE: &str = "T_LINE";
    pub const T_NS_C: &str = "T_NS_C";
    pub const T_TRAIT_C: &str = "T_TRAIT_C";
    pub const T_ATTRIBUTE: &str = "T_ATTRIBUTE";
    pub const T_READONLY: &str = "T_READONLY";
    pub const T_CALLABLE: &str = "T_CALLABLE";
    pub const T_INSTEADOF: &str = "T_INSTEADOF";
    pub const T_UNSET: &str = "T_UNSET";
}

/// One lexical token: a kind (`T_*` or single character), its concrete
/// text, and its byte offset in [`Tree::source`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: String,
    pub value: String,
    pub offset: usize,
}

/// An AST node from canned `nodes` output (informational; rules
/// derive structure from tokens).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XhpNode {
    pub kind: String,
    pub start: usize,
    pub end: usize,
    pub children: Vec<XhpNode>,
}

/// A parsed file: source text, authoritative token stream, line
/// index, and optional canned AST nodes.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub source: String,
    pub tokens: Vec<Token>,
    pub line_starts: Vec<usize>,
    pub nodes: Vec<XhpNode>,
}

/// Linter options fanned out to the XHP rule tree, mirroring the
/// `xhpast.*` configuration keys.
#[derive(Debug, Clone, Default)]
pub struct XhpOptions {
    pub php_version: Option<String>,
    pub php_version_windows: Option<String>,
    pub blacklisted: Vec<(String, String)>,
    pub deprecated: Vec<(String, String)>,
    pub printf: Vec<(String, i64)>,
    pub dynamic_classes: Vec<(String, i64)>,
    pub dynamic_functions: Vec<(String, i64)>,
    /// Accepted but inert: custom PHP hook classes cannot run here.
    pub switchhook: Option<String>,
    /// Accepted but inert: custom PHP hook classes cannot run here.
    pub naminghook: Option<String>,
}

/// Rule evaluation context: the parsed tree, the display path (for
/// filename-based rules), and the active options.
#[derive(Debug)]
pub struct RuleCtx<'a> {
    pub tree: &'a Tree,
    pub path: &'a str,
    pub options: &'a XhpOptions,
}

/// Offset sentinel for path-level findings (mirrors PHP
/// `raiseLintAtPath()`): emitted as line 0, column 0.
pub const PATH_LEVEL: usize = usize::MAX;

/// One rule hit: PHP rule ID, byte offset, exact PHP message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    pub rule: u32,
    pub offset: usize,
    pub message: String,
}

impl RawFinding {
    #[must_use]
    pub fn at(rule: u32, offset: usize, message: String) -> Self {
        Self {
            rule,
            offset,
            message,
        }
    }
}

fn string_map(value: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(map) = value.as_object() {
        for (key, val) in map {
            if let Some(text) = val.as_str() {
                out.push((key.clone(), text.to_string()));
            }
        }
    }
    out
}

fn int_map(value: &serde_json::Value) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    if let Some(map) = value.as_object() {
        for (key, val) in map {
            if let Some(num) = val.as_i64() {
                out.push((key.clone(), num));
            } else if let Some(text) = val.as_str()
                && let Ok(num) = text.parse::<i64>()
            {
                out.push((key.clone(), num));
            }
        }
    }
    out
}

fn opt_string(value: Option<&serde_json::Value>) -> Option<String> {
    value?.as_str().map(str::to_string)
}

impl XhpOptions {
    /// Build options from the linter's raw option map. Wrong-typed
    /// values fall back to defaults (mirrors PHP's optional keys).
    #[must_use]
    pub fn from_map(map: &HashMap<String, serde_json::Value>) -> Self {
        Self {
            php_version: opt_string(map.get("xhpast.php-version")),
            php_version_windows: opt_string(map.get("xhpast.php-version.windows")),
            blacklisted: map
                .get("xhpast.blacklisted.function")
                .map(string_map)
                .unwrap_or_default(),
            deprecated: map
                .get("xhpast.deprecated.functions")
                .map(string_map)
                .unwrap_or_default(),
            printf: map
                .get("xhpast.printf-functions")
                .map(int_map)
                .unwrap_or_default(),
            dynamic_classes: map
                .get("xhpast.dynamic-string.classes")
                .map(int_map)
                .unwrap_or_default(),
            dynamic_functions: map
                .get("xhpast.dynamic-string.functions")
                .map(int_map)
                .unwrap_or_default(),
            switchhook: opt_string(map.get("xhpast.switchhook")),
            naminghook: opt_string(map.get("xhpast.naminghook")),
        }
    }
}

/// True for `T_WHITESPACE` tokens.
#[must_use]
pub fn is_whitespace(token: &Token) -> bool {
    token.kind == kinds::T_WHITESPACE
}

/// True for `T_COMMENT` / `T_DOC_COMMENT` tokens.
#[must_use]
pub fn is_comment(token: &Token) -> bool {
    token.kind == kinds::T_COMMENT || token.kind == kinds::T_DOC_COMMENT
}

/// True for tokens carrying code (everything except whitespace and
/// comments), mirroring XHPAST `isSemantic()`.
#[must_use]
pub fn is_semantic(token: &Token) -> bool {
    !is_whitespace(token) && !is_comment(token)
}

fn build_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

impl Tree {
    /// 1-based (line, col) for a byte offset, counting bytes like the
    /// PHP engine (mirrors `lint.rs` conventions).
    #[must_use]
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.source.len());
        let mut line = 1;
        for (index, start) in self.line_starts.iter().enumerate() {
            if *start <= offset {
                line = index + 1;
            } else {
                break;
            }
        }
        let col = offset - self.line_starts[line - 1] + 1;
        (line, col)
    }

    /// 1-based line number for a byte offset.
    #[must_use]
    pub fn line_of(&self, offset: usize) -> usize {
        self.line_col(offset).0
    }

    /// Source text of a 1-based line without its terminator.
    #[must_use]
    pub fn source_line(&self, lineno: usize) -> &str {
        if lineno == 0 || lineno > self.line_starts.len() {
            return "";
        }
        let start = self.line_starts[lineno - 1];
        let end = if lineno < self.line_starts.len() {
            self.line_starts[lineno] - 1
        } else {
            self.source.len()
        };
        self.source.get(start..end).unwrap_or("")
    }

    /// Next code-token index after `idx`, skipping whitespace/comments.
    #[must_use]
    pub fn next_semantic(&self, idx: usize) -> Option<usize> {
        let mut i = idx + 1;
        while i < self.tokens.len() {
            if is_semantic(&self.tokens[i]) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// Previous code-token index before `idx`, skipping trivia.
    #[must_use]
    pub fn prev_semantic(&self, idx: usize) -> Option<usize> {
        let mut i = idx;
        while i > 0 {
            i -= 1;
            if is_semantic(&self.tokens[i]) {
                return Some(i);
            }
        }
        None
    }

    /// Raw source text strictly between tokens `a` and `b`.
    #[must_use]
    pub fn gap_text(&self, a: usize, b: usize) -> String {
        if a >= self.tokens.len() || b >= self.tokens.len() || a >= b {
            return String::new();
        }
        let start = self.tokens[a].offset + self.tokens[a].value.len();
        let end = self.tokens[b].offset.min(self.source.len());
        if start >= end {
            return String::new();
        }
        self.source
            .get(start..end.min(self.source.len()))
            .unwrap_or("")
            .to_string()
    }

    /// Source slice spanning tokens `first..=last` (inclusive).
    #[must_use]
    pub fn span_text(&self, first: usize, last: usize) -> &str {
        if first >= self.tokens.len() || last >= self.tokens.len() || first > last {
            return "";
        }
        let start = self.tokens[first].offset.min(self.source.len());
        let end = (self.tokens[last].offset + self.tokens[last].value.len()).min(self.source.len());
        self.source.get(start..end).unwrap_or("")
    }

    /// Token index containing `offset`, if any.
    #[must_use]
    pub fn token_at_offset(&self, offset: usize) -> Option<usize> {
        let mut lo = 0;
        let mut hi = self.tokens.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let token = &self.tokens[mid];
            if offset < token.offset {
                hi = mid;
            } else if offset >= token.offset + token.value.len() {
                lo = mid + 1;
            } else {
                return Some(mid);
            }
        }
        None
    }
}

fn collect_nodes<'a>(node: &'a XhpNode, kind: &str, out: &mut Vec<&'a XhpNode>) {
    if node.kind == kind {
        out.push(node);
    }
    for child in &node.children {
        collect_nodes(child, kind, out);
    }
}

fn is_open(kind: &str) -> bool {
    kind == "(" || kind == "{" || kind == "["
}

fn is_close(kind: &str) -> bool {
    kind == ")" || kind == "}" || kind == "]"
}

fn matches(open: &str, close: &str) -> bool {
    matches!((open, close), ("(", ")") | ("{", "}") | ("[", "]"))
}

impl Tree {
    /// Index of the bracket matching the opener at `open_idx`.
    #[must_use]
    pub fn match_close(&self, open_idx: usize) -> Option<usize> {
        if open_idx >= self.tokens.len() || !is_open(&self.tokens[open_idx].kind) {
            return None;
        }
        let open = self.tokens[open_idx].kind.clone();
        let mut depth = 0;
        let mut i = open_idx;
        while i < self.tokens.len() {
            let kind = self.tokens[i].kind.as_str();
            if is_open(kind) {
                depth += 1;
            } else if is_close(kind) {
                depth -= 1;
                if depth == 0 {
                    return if matches(&open, kind) { Some(i) } else { None };
                }
                if depth < 0 {
                    return None;
                }
            }
            i += 1;
        }
        None
    }

    /// All canned AST nodes of type `kind` (depth-first).
    #[must_use]
    pub fn nodes_of_type(&self, kind: &str) -> Vec<&XhpNode> {
        let mut out = Vec::new();
        for node in &self.nodes {
            collect_nodes(node, kind, &mut out);
        }
        out
    }

    /// Index of the bracket matching the closer at `close_idx`.
    #[must_use]
    pub fn match_open(&self, close_idx: usize) -> Option<usize> {
        if close_idx >= self.tokens.len() || !is_close(&self.tokens[close_idx].kind) {
            return None;
        }
        let close = self.tokens[close_idx].kind.clone();
        let mut depth = 0;
        let mut i = close_idx + 1;
        while i > 0 {
            i -= 1;
            let kind = self.tokens[i].kind.as_str();
            if is_close(kind) {
                depth += 1;
            } else if is_open(kind) {
                depth -= 1;
                if depth == 0 {
                    return if matches(kind, &close) { Some(i) } else { None };
                }
            }
        }
        None
    }
}

fn is_label_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_label_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn is_space_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x0B)
}

/// Sorted (keyword, token kind) table for [`keyword_kind`].
const KEYWORDS: &[(&str, &str)] = &[
    ("__class__", kinds::T_CLASS_C),
    ("__dir__", kinds::T_DIR),
    ("__file__", kinds::T_FILE),
    ("__function__", kinds::T_FUNC_C),
    ("__halt_compiler", kinds::T_HALT_COMPILER),
    ("__line__", kinds::T_LINE),
    ("__method__", kinds::T_METHOD_C),
    ("__namespace__", kinds::T_NS_C),
    ("__trait__", kinds::T_TRAIT_C),
    ("abstract", kinds::T_ABSTRACT),
    ("and", kinds::T_LOGICAL_AND),
    ("array", kinds::T_ARRAY),
    ("as", kinds::T_AS),
    ("break", kinds::T_BREAK),
    ("callable", kinds::T_CALLABLE),
    ("case", kinds::T_CASE),
    ("catch", kinds::T_CATCH),
    ("class", kinds::T_CLASS),
    ("clone", kinds::T_CLONE),
    ("const", kinds::T_CONST),
    ("continue", kinds::T_CONTINUE),
    ("declare", kinds::T_DECLARE),
    ("default", kinds::T_DEFAULT),
    ("die", kinds::T_EXIT),
    ("do", kinds::T_DO),
    ("echo", kinds::T_ECHO),
    ("else", kinds::T_ELSE),
    ("elseif", kinds::T_ELSEIF),
    ("empty", kinds::T_EMPTY),
    ("enddeclare", kinds::T_ENDDECLARE),
    ("endfor", kinds::T_ENDFOR),
    ("endforeach", kinds::T_ENDFOREACH),
    ("endif", kinds::T_ENDIF),
    ("endswitch", kinds::T_ENDSWITCH),
    ("endwhile", kinds::T_ENDWHILE),
    ("enum", kinds::T_ENUM),
    ("eval", kinds::T_EVAL),
    ("exit", kinds::T_EXIT),
    ("extends", kinds::T_EXTENDS),
    ("final", kinds::T_FINAL),
    ("finally", kinds::T_FINALLY),
    ("fn", kinds::T_FN),
    ("for", kinds::T_FOR),
    ("foreach", kinds::T_FOREACH),
    ("function", kinds::T_FUNCTION),
    ("global", kinds::T_GLOBAL),
    ("goto", kinds::T_GOTO),
    ("if", kinds::T_IF),
    ("implements", kinds::T_IMPLEMENTS),
    ("include", kinds::T_INCLUDE),
    ("include_once", kinds::T_INCLUDE_ONCE),
    ("instanceof", kinds::T_INSTANCEOF),
    ("insteadof", kinds::T_INSTEADOF),
    ("interface", kinds::T_INTERFACE),
    ("isset", kinds::T_ISSET),
    ("list", kinds::T_LIST),
    ("match", kinds::T_MATCH),
    ("namespace", kinds::T_NAMESPACE),
    ("new", kinds::T_NEW),
    ("or", kinds::T_LOGICAL_OR),
    ("print", kinds::T_PRINT),
    ("private", kinds::T_PRIVATE),
    ("protected", kinds::T_PROTECTED),
    ("public", kinds::T_PUBLIC),
    ("readonly", kinds::T_READONLY),
    ("require", kinds::T_REQUIRE),
    ("require_once", kinds::T_REQUIRE_ONCE),
    ("return", kinds::T_RETURN),
    ("static", kinds::T_STATIC),
    ("switch", kinds::T_SWITCH),
    ("throw", kinds::T_THROW),
    ("trait", kinds::T_TRAIT),
    ("try", kinds::T_TRY),
    ("unset", kinds::T_UNSET),
    ("use", kinds::T_USE),
    ("var", kinds::T_VAR),
    ("while", kinds::T_WHILE),
    ("xor", kinds::T_LOGICAL_XOR),
    ("yield", kinds::T_YIELD),
];

/// Map a lowercase label to its keyword token kind, if any.
fn keyword_kind(lower: &str) -> Option<&'static str> {
    KEYWORDS
        .binary_search_by(|(word, _)| word.cmp(&lower))
        .ok()
        .map(|index| KEYWORDS[index].1)
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    in_php: bool,
    tokens: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            pos: 0,
            in_php: false,
            tokens: Vec::new(),
        }
    }

    fn rest(&self) -> &'a [u8] {
        &self.bytes[self.pos..]
    }

    fn starts_with(&self, text: &[u8]) -> bool {
        self.rest().starts_with(text)
    }

    fn push(&mut self, kind: &str, start: usize, end: usize) {
        let value = String::from_utf8_lossy(&self.bytes[start..end]).into_owned();
        self.tokens.push(Token {
            kind: kind.to_string(),
            value,
            offset: start,
        });
        self.pos = end;
    }

    fn run(&mut self) {
        while self.pos < self.bytes.len() {
            if self.in_php {
                self.lex_php();
            } else {
                self.lex_html();
            }
        }
    }

    fn lex_html(&mut self) {
        let start = self.pos;
        let mut i = start;
        while i < self.bytes.len() {
            if self.bytes[i] == b'<' && i + 1 < self.bytes.len() && self.bytes[i + 1] == b'?' {
                break;
            }
            i += 1;
        }
        if i > start {
            self.push(kinds::T_INLINE_HTML, start, i);
            return;
        }
        self.lex_open_tag();
    }

    fn lex_open_tag(&mut self) {
        let start = self.pos;
        let rest = self.rest();
        if rest.starts_with(b"<?=") {
            self.in_php = true;
            self.push(kinds::T_OPEN_TAG_WITH_ECHO, start, start + 3);
        } else if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"<?php") {
            self.in_php = true;
            self.push(kinds::T_OPEN_TAG, start, start + 5);
        } else {
            self.in_php = true;
            self.push(kinds::T_OPEN_TAG, start, start + 2);
        }
    }

    fn lex_php(&mut self) {
        let byte = self.bytes[self.pos];
        if self.starts_with(b"?>") {
            self.lex_close_tag();
        } else if is_space_byte(byte) {
            self.lex_whitespace();
        } else if self.starts_with(b"//") || self.starts_with(b"#") || self.starts_with(b"/*") {
            self.lex_comment();
        } else if byte == b'\'' {
            self.lex_single_string();
        } else if byte == b'"' {
            self.lex_double_string();
        } else if byte == b'`' {
            self.lex_backtick();
        } else if self.starts_with(b"<<<") {
            self.lex_heredoc_or_shl();
        } else if byte == b'$' {
            self.lex_variable();
        } else if self.at_number_start(byte) {
            self.lex_number();
        } else if is_label_start(byte) {
            self.lex_label();
        } else {
            self.lex_operator();
        }
    }

    fn lex_close_tag(&mut self) {
        let start = self.pos;
        self.in_php = false;
        let mut end = start + 2;
        if self.bytes.get(end..end + 2) == Some(b"\r\n") {
            end += 2;
        } else if self.bytes.get(end) == Some(&b'\n') {
            end += 1;
        }
        self.push(kinds::T_CLOSE_TAG, start, end);
    }

    fn at_number_start(&self, byte: u8) -> bool {
        byte.is_ascii_digit() || self.at_fraction_start(byte)
    }

    fn at_fraction_start(&self, byte: u8) -> bool {
        byte == b'.' && self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1].is_ascii_digit()
    }

    fn lex_whitespace(&mut self) {
        let start = self.pos;
        let mut i = start;
        while i < self.bytes.len() && is_space_byte(self.bytes[i]) {
            i += 1;
        }
        self.push(kinds::T_WHITESPACE, start, i);
    }

    fn lex_comment(&mut self) {
        let start = self.pos;
        if self.starts_with(b"/*") {
            let doc = self.rest().starts_with(b"/**") && self.rest().get(3) != Some(&b'/');
            let mut i = start + 2;
            while i < self.bytes.len() {
                if self.bytes[i] == b'*' && i + 1 < self.bytes.len() && self.bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            let kind = if doc {
                kinds::T_DOC_COMMENT
            } else {
                kinds::T_COMMENT
            };
            self.push(kind, start, i);
        } else if self.starts_with(b"#[") {
            self.push("T_ATTRIBUTE", start, start + 2);
        } else {
            let mut i = start;
            while i < self.bytes.len() && self.bytes[i] != b'\n' {
                i += 1;
            }
            self.push(kinds::T_COMMENT, start, i);
        }
    }

    fn lex_single_string(&mut self) {
        let start = self.pos;
        let end = scan_quoted(self.bytes, start);
        self.push(kinds::T_CONSTANT_ENCAPSED_STRING, start, end);
    }

    fn lex_double_string(&mut self) {
        let start = self.pos;
        let end = scan_quoted(self.bytes, start);
        let kind = if has_interpolation(&self.bytes[start..end]) {
            kinds::T_INTERPOLATED_STRING
        } else {
            kinds::T_CONSTANT_ENCAPSED_STRING
        };
        self.push(kind, start, end);
    }

    fn lex_backtick(&mut self) {
        let start = self.pos;
        let end = scan_quoted(self.bytes, start);
        self.push(kinds::T_BACKTICK, start, end);
    }

    fn lex_heredoc_or_shl(&mut self) {
        let start = self.pos;
        if let Some((end, nowdoc)) = scan_heredoc(self.bytes, start) {
            let kind = if nowdoc {
                kinds::T_NOWDOC
            } else {
                kinds::T_HEREDOC
            };
            self.push(kind, start, end);
        } else {
            self.lex_operator();
        }
    }

    fn lex_variable(&mut self) {
        let start = self.pos;
        if start + 1 < self.bytes.len() && is_label_start(self.bytes[start + 1]) {
            let mut i = start + 2;
            while i < self.bytes.len() && is_label_char(self.bytes[i]) {
                i += 1;
            }
            self.push(kinds::T_VARIABLE, start, i);
        } else {
            self.push("$", start, start + 1);
        }
    }

    fn lex_number(&mut self) {
        let start = self.pos;
        let end = scan_number(self.bytes, start);
        let text = &self.bytes[start..end];
        let prefixed = text.len() > 2
            && text[0] == b'0'
            && (text[1] == b'x' || text[1] == b'X' || text[1] == b'b' || text[1] == b'B');
        let kind = if !prefixed
            && (text.contains(&b'.') || text.contains(&b'e') || text.contains(&b'E'))
        {
            kinds::T_DNUMBER
        } else {
            kinds::T_LNUMBER
        };
        self.push(kind, start, end);
    }

    fn lex_label(&mut self) {
        let start = self.pos;
        let mut i = start;
        while i < self.bytes.len() && is_label_char(self.bytes[i]) {
            i += 1;
        }
        let word = String::from_utf8_lossy(&self.bytes[start..i]).into_owned();
        let kind = keyword_kind(&word.to_ascii_lowercase()).unwrap_or(kinds::T_STRING);
        self.push(kind, start, i);
    }

    fn lex_operator(&mut self) {
        let start = self.pos;
        let (kind, len) = match_operator(self.rest());
        self.push(kind, start, start + len);
    }
}

/// Scan a quoted literal starting at the quote byte; returns the
/// one-past-end offset (unterminated runs to end of input).
fn scan_quoted(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// True when a double-quoted literal contains `$` interpolation.
fn has_interpolation(literal: &[u8]) -> bool {
    let mut i = 1;
    while i < literal.len() {
        if literal[i] == b'\\' {
            i += 2;
            continue;
        }
        if literal[i] == b'$' && i + 1 < literal.len() {
            let next = literal[i + 1];
            if is_label_start(next) || next == b'{' {
                return true;
            }
        }
        if literal[i] == b'{' && i + 1 < literal.len() && literal[i + 1] == b'$' {
            return true;
        }
        i += 1;
    }
    false
}

/// Scan a heredoc/nowdoc at `start` (which points at `<<<`).
/// Returns the one-past-end offset and whether it is a nowdoc.
fn scan_heredoc(bytes: &[u8], start: usize) -> Option<(usize, bool)> {
    let (label, body_start, quoted) = scan_heredoc_opener(bytes, start)?;
    let end = scan_heredoc_body(bytes, body_start, label);
    Some((end, quoted == Some(b'\'')))
}

fn scan_heredoc_opener(bytes: &[u8], start: usize) -> Option<(&[u8], usize, Option<u8>)> {
    let mut i = start + 3;
    let mut quoted: Option<u8> = None;
    if is_heredoc_quote(bytes, i) {
        quoted = Some(bytes[i]);
        i += 1;
    }
    let label_start = i;
    if i >= bytes.len() || !is_label_start(bytes[i]) {
        return None;
    }
    while i < bytes.len() && is_label_char(bytes[i]) {
        i += 1;
    }
    let label = &bytes[label_start..i];
    let after_quote = skip_heredoc_quote(bytes, i, quoted)?;
    if after_quote < bytes.len() && !is_line_break(bytes[after_quote]) {
        return None;
    }
    let body_start = skip_line_breaks(bytes, after_quote);
    Some((label, body_start, quoted))
}

fn is_heredoc_quote(bytes: &[u8], i: usize) -> bool {
    i < bytes.len() && (bytes[i] == b'\'' || bytes[i] == b'"')
}

fn skip_heredoc_quote(bytes: &[u8], i: usize, quoted: Option<u8>) -> Option<usize> {
    let Some(q) = quoted else {
        return Some(i);
    };
    if i >= bytes.len() || bytes[i] != q {
        return None;
    }
    Some(i + 1)
}

fn is_line_break(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r'
}

fn skip_line_breaks(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && is_line_break(bytes[i]) {
        i += 1;
    }
    i
}

fn scan_heredoc_body(bytes: &[u8], body_start: usize, label: &[u8]) -> usize {
    let mut line_start = body_start;
    let mut i = body_start;
    while i <= bytes.len() {
        if heredoc_line_end(bytes, i) {
            if let Some(end) = heredoc_close_at(bytes, line_start, i, label) {
                return end;
            }
            if i == bytes.len() {
                break;
            }
            i = skip_line_breaks(bytes, i);
            line_start = i;
            continue;
        }
        i += 1;
    }
    bytes.len()
}

fn heredoc_line_end(bytes: &[u8], i: usize) -> bool {
    i == bytes.len() || is_line_break(bytes[i])
}

fn heredoc_close_at(bytes: &[u8], line_start: usize, i: usize, label: &[u8]) -> Option<usize> {
    let mut j = line_start;
    while j < i && (bytes[j] == b' ' || bytes[j] == b'\t') {
        j += 1;
    }
    if !bytes[j..i.min(bytes.len())].starts_with(label) {
        return None;
    }
    let after = j + label.len();
    if heredoc_close_ok(bytes, after, i) {
        return Some(after);
    }
    None
}

fn heredoc_close_ok(bytes: &[u8], after: usize, i: usize) -> bool {
    let ok = after >= bytes.len() || (!is_label_char(bytes[after]) && bytes[after] != b'\x80');
    ok && after <= i
}

/// Scan a numeric literal; maximal munch keeps invalid literals
/// (like `08`) whole for XHP125.
fn scan_number(bytes: &[u8], start: usize) -> usize {
    if let Some(end) = scan_radix_int(bytes, start) {
        return end;
    }
    let after_int = scan_digit_run(bytes, start);
    let after_fraction = scan_fraction(bytes, after_int);
    scan_exponent(bytes, after_fraction).max(start + 1)
}

fn scan_radix_int(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes[start] != b'0' || start + 1 >= bytes.len() {
        return None;
    }
    let marker = bytes[start + 1];
    if marker == b'x' || marker == b'X' {
        Some(scan_prefixed_run(bytes, start, true))
    } else if marker == b'b' || marker == b'B' {
        Some(scan_prefixed_run(bytes, start, false))
    } else {
        None
    }
}

fn scan_prefixed_run(bytes: &[u8], start: usize, hex: bool) -> usize {
    let mut i = start + 2;
    let digits = i;
    while i < bytes.len() && is_radix_digit(bytes[i], hex) {
        i += 1;
    }
    if i == digits { start + 1 } else { i }
}

fn is_radix_digit(byte: u8, hex: bool) -> bool {
    if hex {
        byte.is_ascii_hexdigit() || byte == b'_'
    } else {
        byte == b'0' || byte == b'1' || byte == b'_'
    }
}

fn scan_digit_run(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    i
}

fn scan_fraction(bytes: &[u8], i: usize) -> usize {
    if at_fraction_dot(bytes, i) {
        scan_digit_run(bytes, i + 1)
    } else if at_concat_dot(bytes, i) {
        i + 1
    } else {
        i
    }
}

fn at_fraction_dot(bytes: &[u8], i: usize) -> bool {
    i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit()
}

fn at_concat_dot(bytes: &[u8], i: usize) -> bool {
    i + 1 < bytes.len() && bytes[i] == b'.' && is_label_start(bytes[i + 1])
}

fn scan_exponent(bytes: &[u8], i: usize) -> usize {
    if i >= bytes.len() || (bytes[i] != b'e' && bytes[i] != b'E') {
        return i;
    }
    scan_exponent_digits(bytes, i + 1).unwrap_or(i)
}

fn scan_exponent_digits(bytes: &[u8], mut j: usize) -> Option<usize> {
    if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
        j += 1;
    }
    if j < bytes.len() && bytes[j].is_ascii_digit() {
        return Some(scan_digit_run(bytes, j + 1));
    }
    None
}

fn match_operator(rest: &[u8]) -> (&'static str, usize) {
    if let Some(hit) = match_three(rest) {
        return hit;
    }
    if let Some(hit) = match_two(rest) {
        return hit;
    }
    (match_single(rest[0], rest), 1)
}

fn match_three(rest: &[u8]) -> Option<(&'static str, usize)> {
    use kinds::*;
    const THREE: &[(&[u8], &str)] = &[
        (b"===", T_IS_IDENTICAL),
        (b"!==", T_IS_NOT_IDENTICAL),
        (b"<=>", T_SPACESHIP),
        (b"**=", "**="),
        (b"...", T_ELLIPSIS),
        (b"??=", "??="),
        (b"<<=", "<<="),
        (b">>=", ">>="),
        (b"?->", T_NULLSAFE_OBJECT_OPERATOR),
    ];
    match_prefix(rest, THREE)
}

fn match_two(rest: &[u8]) -> Option<(&'static str, usize)> {
    use kinds::*;
    const TWO: &[(&[u8], &str)] = &[
        (b"==", T_IS_EQUAL),
        (b"!=", T_IS_NOT_EQUAL),
        (b"<>", T_IS_NOT_EQUAL),
        (b"<=", "<="),
        (b">=", ">="),
        (b"&&", T_BOOLEAN_AND),
        (b"||", T_BOOLEAN_OR),
        (b"++", T_INC),
        (b"--", T_DEC),
        (b"**", T_POW),
        (b"<<", T_SL),
        (b">>", T_SR),
        (b"=>", T_DOUBLE_ARROW),
        (b"->", T_OBJECT_OPERATOR),
        (b"::", T_PAAMAYIM_NEKUDOTAYIM),
        (b"+=", "+="),
        (b"-=", "-="),
        (b"*=", "*="),
        (b"/=", "/="),
        (b"%=", "%="),
        (b".=", ".="),
        (b"&=", "&="),
        (b"|=", "|="),
        (b"^=", "^="),
        (b"??", T_COALESCE),
        (b"?->", T_NULLSAFE_OBJECT_OPERATOR),
    ];
    match_prefix(rest, TWO)
}

fn match_prefix(rest: &[u8], table: &[(&[u8], &'static str)]) -> Option<(&'static str, usize)> {
    for (text, kind) in table {
        if rest.starts_with(text) {
            return Some((kind, text.len()));
        }
    }
    None
}

/// Single-byte operators use the byte itself as the kind.
fn match_single(byte: u8, rest: &[u8]) -> &'static str {
    const SINGLE: &[(&[u8], &str)] = &[
        (b";", ";"),
        (b"(", "("),
        (b")", ")"),
        (b"{", "{"),
        (b"}", "}"),
        (b"[", "["),
        (b"]", "]"),
        (b",", ","),
        (b".", "."),
        (b"=", "="),
        (b"+", "+"),
        (b"-", "-"),
        (b"*", "*"),
        (b"/", "/"),
        (b"%", "%"),
        (b"!", "!"),
        (b"<", "<"),
        (b">", ">"),
        (b"&", "&"),
        (b"|", "|"),
        (b"^", "^"),
        (b"~", "~"),
        (b"?", "?"),
        (b":", ":"),
        (b"@", "@"),
        (b"\\", "\\"),
        (b"#", "#"),
        (b"$", "$"),
    ];
    let _ = byte;
    match_prefix(rest, SINGLE).map_or("?", |(kind, _)| kind)
}

/// Tokenize PHP source into the XHPAST-shaped token stream.
#[must_use]
pub fn tokenize(source: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(source);
    lexer.run();
    lexer.tokens
}

/// Build a [`Tree`] from source text using the local tokenizer.
#[must_use]
pub fn tree_from_source(source: &str) -> Tree {
    Tree {
        tokens: tokenize(source),
        line_starts: build_line_starts(source),
        source: source.to_string(),
        nodes: Vec::new(),
    }
}

/// Unquote a plain string literal (single-quoted, or double-quoted
/// without interpolation). Returns `None` for anything dynamic,
/// mirroring `isConstantString()` + `evalStatic()`.
#[must_use]
pub fn unquote_plain_string(literal: &str) -> Option<String> {
    let bytes = literal.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let (quote, single) = match bytes[0] {
        b'\'' => (b'\'', true),
        b'"' => (b'"', false),
        _ => return None,
    };
    if bytes[bytes.len() - 1] != quote {
        return None;
    }
    let inner = literal.get(1..literal.len() - 1)?;
    unquote_inner(inner, single)
}

fn unquote_inner(inner: &str, single: bool) -> Option<String> {
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            push_unquote_escape(&mut chars, &mut out, single);
            continue;
        }
        if !single && ch == '$' {
            return None;
        }
        out.push(ch);
    }
    Some(out)
}

fn push_unquote_escape(chars: &mut std::str::Chars<'_>, out: &mut String, single: bool) {
    let Some(next) = chars.next() else {
        out.push('\\');
        return;
    };
    if single {
        push_single_escape(out, next);
    } else {
        push_double_escape(out, next);
    }
}

fn push_single_escape(out: &mut String, next: char) {
    if next == '\\' || next == '\'' {
        out.push(next);
    } else {
        out.push('\\');
        out.push(next);
    }
}

fn push_double_escape(out: &mut String, next: char) {
    if next == '"' || next == '\\' || next == '$' {
        out.push(next);
    } else {
        out.push('\\');
        out.push(next);
    }
}

/// Interpolated variables in a double-quoted / heredoc / backtick
/// literal: (byte offset within `literal`, name without `$`).
/// Mirrors `XHPASTNode::getStringVariables()`.
#[must_use]
pub fn string_variables(literal: &str) -> Vec<(usize, String)> {
    let bytes = literal.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'$'
            && let Some((end, name)) = scan_string_var(bytes, i)
        {
            out.push((i, name));
            i = end;
            continue;
        }
        if bytes[i] == b'{'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'$'
            && let Some((end, name)) = scan_string_var(bytes, i + 1)
        {
            out.push((i + 1, name));
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

fn scan_string_var(bytes: &[u8], dollar: usize) -> Option<(usize, String)> {
    let mut i = dollar + 1;
    if i < bytes.len() && bytes[i] == b'{' {
        i += 1;
        let start = i;
        if i < bytes.len() && is_label_start(bytes[i]) {
            while i < bytes.len() && is_label_char(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'}' {
                let name = String::from_utf8_lossy(&bytes[start..i]).into_owned();
                return Some((i + 1, name));
            }
        }
        return None;
    }
    if i >= bytes.len() || !is_label_start(bytes[i]) {
        return None;
    }
    let start = i;
    while i < bytes.len() && is_label_char(bytes[i]) {
        i += 1;
    }
    let name = String::from_utf8_lossy(&bytes[start..i]).into_owned();
    Some((i, name))
}

fn parse_token_entry(entry: &serde_json::Value) -> Option<Token> {
    if let Some(parts) = entry.as_array() {
        if parts.len() != 3 {
            return None;
        }
        let kind = parts[0].as_str()?;
        let value = parts[1].as_str()?;
        let offset = parts[2].as_u64()?;
        return Some(Token {
            kind: kind.to_string(),
            value: value.to_string(),
            offset: offset as usize,
        });
    }
    let obj = entry.as_object()?;
    let kind = obj.get("t")?.as_str()?;
    let value = obj.get("v")?.as_str()?;
    let offset = obj.get("o")?.as_u64()?;
    Some(Token {
        kind: kind.to_string(),
        value: value.to_string(),
        offset: offset as usize,
    })
}

fn parse_node_entry(entry: &serde_json::Value) -> Option<XhpNode> {
    let obj = entry.as_object()?;
    let kind = obj.get("t")?.as_str()?;
    let start = obj.get("s")?.as_u64()? as usize;
    let end = obj.get("e")?.as_u64()? as usize;
    let mut children = Vec::new();
    if let Some(list) = obj.get("c").and_then(serde_json::Value::as_array) {
        for child in list {
            children.push(parse_node_entry(child)?);
        }
    }
    Some(XhpNode {
        kind: kind.to_string(),
        start,
        end,
        children,
    })
}

/// Parse canned `xhpast` JSON output (`{"source", "tokens"?, "nodes"?}`).
/// `tokens`, when present, is authoritative; otherwise the embedded
/// source is tokenized locally.
pub fn tree_from_canned(json: &str) -> Result<Tree, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid xhpast output: {e}"))?;
    let obj = parsed
        .as_object()
        .ok_or("invalid xhpast output: not an object")?;
    let source = obj
        .get("source")
        .and_then(serde_json::Value::as_str)
        .ok_or("invalid xhpast output: missing source")?;
    tree_from_canned_with_source(source, json)
}

/// Parse canned `xhpast` output against known source text (the real
/// file bytes). An embedded `tokens` array is authoritative; without
/// one the source is tokenized locally. An embedded `source` is
/// ignored in favor of the argument.
pub fn tree_from_canned_with_source(source: &str, stdout: &str) -> Result<Tree, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("invalid xhpast output: {e}"))?;
    let obj = parsed
        .as_object()
        .ok_or("invalid xhpast output: not an object")?;
    let tokens = match obj.get("tokens") {
        None => tokenize(source),
        Some(list) => {
            let entries = list.as_array().ok_or("invalid xhpast output: bad tokens")?;
            let mut out = Vec::with_capacity(entries.len());
            for entry in entries {
                out.push(parse_token_entry(entry).ok_or("invalid xhpast output: bad token")?);
            }
            out
        }
    };
    let mut nodes = Vec::new();
    if let Some(list) = obj.get("nodes") {
        let entries = list.as_array().ok_or("invalid xhpast output: bad nodes")?;
        for entry in entries {
            nodes.push(parse_node_entry(entry).ok_or("invalid xhpast output: bad node")?);
        }
    }
    Ok(Tree {
        line_starts: build_line_starts(source),
        source: source.to_string(),
        tokens,
        nodes,
    })
}

/// Serialize a token stream back to canned JSON (used by tests to
/// build canned `xhpast` output from fixtures).
#[must_use]
pub fn tokens_to_canned(source: &str, tokens: &[Token]) -> String {
    let mut parts = Vec::with_capacity(tokens.len());
    for token in tokens {
        parts.push(format!(
            "[{}, {}, {}]",
            serde_json::to_string(&token.kind).unwrap_or_default(),
            serde_json::to_string(&token.value).unwrap_or_default(),
            token.offset
        ));
    }
    format!(
        "{{\"source\": {}, \"tokens\": [{}]}}",
        serde_json::to_string(source).unwrap_or_default(),
        parts.join(", ")
    )
}

fn version_part(part: &str) -> (i64, &str) {
    let digits: String = part
        .bytes()
        .take_while(u8::is_ascii_digit)
        .map(|b| b as char)
        .collect();
    let num = digits.parse::<i64>().unwrap_or(0);
    (num, &part[digits.len()..])
}

/// A function call site: `name(...)` with matched parens and
/// top-level comma-split argument ranges (inclusive token indices,
/// edge trivia trimmed; empty segments skipped).
#[derive(Debug, Clone)]
pub struct CallSite {
    pub name: String,
    pub name_idx: usize,
    pub lparen: usize,
    pub rparen: Option<usize>,
    pub args: Vec<(usize, usize)>,
    /// True for `isset` / `empty` / `eval` / `exit` constructs.
    pub is_construct: bool,
}

impl Tree {
    /// Source slice of call argument `i`.
    #[must_use]
    pub fn arg_text(&self, call: &CallSite, i: usize) -> &str {
        if i >= call.args.len() {
            return "";
        }
        let (first, last) = call.args[i];
        self.span_text(first, last)
    }

    /// Unquoted value when argument `i` is a single plain string
    /// literal (mirrors `isConstantString()` + `evalStatic()`).
    #[must_use]
    pub fn arg_plain_string(&self, call: &CallSite, i: usize) -> Option<String> {
        if i >= call.args.len() {
            return None;
        }
        let (first, last) = call.args[i];
        if first != last {
            return None;
        }
        let token = &self.tokens[first];
        if token.kind != kinds::T_CONSTANT_ENCAPSED_STRING {
            return None;
        }
        unquote_plain_string(&token.value)
    }

    /// Static scalar text for a token slice (plain string, number,
    /// `true` / `false` / `null`), mirroring `isStaticScalar()`.
    #[must_use]
    pub fn static_scalar_text(&self, first: usize, last: usize) -> Option<String> {
        if first != last || first >= self.tokens.len() {
            return None;
        }
        let token = &self.tokens[first];
        if token.kind == kinds::T_CONSTANT_ENCAPSED_STRING {
            return unquote_plain_string(&token.value);
        }
        if token.kind == kinds::T_LNUMBER || token.kind == kinds::T_DNUMBER {
            return Some(token.value.clone());
        }
        if token.kind == kinds::T_STRING {
            let lower = token.value.to_ascii_lowercase();
            if lower == "true" || lower == "false" || lower == "null" {
                return Some(token.value.clone());
            }
        }
        None
    }

    /// All function call sites: `T_STRING` names (excluding method,
    /// static, declaration, attribute, and qualified-name positions)
    /// plus the `isset` / `empty` / `eval` / `exit` constructs.
    #[must_use]
    pub fn calls(&self) -> Vec<CallSite> {
        let mut out = Vec::new();
        for i in 0..self.tokens.len() {
            if let Some(call) = self.call_at(i) {
                out.push(call);
            }
        }
        out
    }

    fn call_at(&self, i: usize) -> Option<CallSite> {
        let kind = self.tokens[i].kind.as_str();
        let is_construct = matches!(
            kind,
            kinds::T_ISSET | kinds::T_EMPTY | kinds::T_EVAL | kinds::T_EXIT
        );
        if kind != kinds::T_STRING && !is_construct {
            return None;
        }
        let lparen = self.next_semantic(i)?;
        if self.tokens[lparen].kind != "(" {
            return None;
        }
        if !is_construct && self.call_name_excluded(i) {
            return None;
        }
        let rparen = self.match_close(lparen);
        let end = rparen.unwrap_or(self.tokens.len().saturating_sub(1));
        let args = split_args(self, lparen, end);
        Some(CallSite {
            name: self.tokens[i].value.clone(),
            name_idx: i,
            lparen,
            rparen,
            args,
            is_construct,
        })
    }

    /// True when a `T_STRING` before `(` is not a plain function call.
    fn call_name_excluded(&self, i: usize) -> bool {
        let Some(prev) = self.prev_semantic(i) else {
            return false;
        };
        match self.tokens[prev].kind.as_str() {
            kinds::T_OBJECT_OPERATOR
            | kinds::T_NULLSAFE_OBJECT_OPERATOR
            | kinds::T_PAAMAYIM_NEKUDOTAYIM
            | kinds::T_FUNCTION
            | kinds::T_NEW
            | kinds::T_ATTRIBUTE
            | "#" => true,
            "&" => self
                .prev_semantic(prev)
                .is_some_and(|p| self.tokens[p].kind == kinds::T_FUNCTION),
            "\\" => self.prev_semantic(prev).is_some_and(|p| {
                matches!(
                    self.tokens[p].kind.as_str(),
                    kinds::T_STRING | kinds::T_VARIABLE | "]" | ")"
                )
            }),
            _ => false,
        }
    }
}

impl Tree {
    /// Top-level comma split of `(open, close)`: inclusive
    /// `(first, last)` token ranges, edge trivia trimmed.
    #[must_use]
    pub fn split_top_level(&self, open: usize, close: usize) -> Vec<(usize, usize)> {
        split_args(self, open, close)
    }
}

fn trim_trivia(tree: &Tree, first: usize, last: usize) -> Option<(usize, usize)> {
    let mut a = first;
    let mut b = last;
    while a <= b && !is_semantic(&tree.tokens[a]) {
        a += 1;
    }
    while b > a && !is_semantic(&tree.tokens[b]) {
        b -= 1;
    }
    if a <= b && is_semantic(&tree.tokens[a]) {
        Some((a, b))
    } else {
        None
    }
}

/// Top-level comma split of `(lparen, end)`.
fn split_args(tree: &Tree, lparen: usize, end: usize) -> Vec<(usize, usize)> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = lparen + 1;
    let mut i = start;
    while i <= end && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if is_open(kind) {
            depth += 1;
        } else if is_close(kind) {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if kind == "," && depth == 0 {
            if i > start
                && let Some(range) = trim_trivia(tree, start, i - 1)
            {
                args.push(range);
            }
            start = i + 1;
        }
        i += 1;
    }
    if i > start
        && start <= end
        && let Some(range) = trim_trivia(tree, start, end.min(i - 1))
    {
        args.push(range);
    }
    args
}

/// Compare dotted version strings like PHP `version_compare()`
/// (numeric parts numerically, the rest lexically).
#[must_use]
pub fn version_compare(left: &str, right: &str) -> Ordering {
    let lparts = split_version(left);
    let rparts = split_version(right);
    let len = lparts.len().max(rparts.len());
    for i in 0..len {
        let l = lparts.get(i).map(String::as_str).unwrap_or("");
        let r = rparts.get(i).map(String::as_str).unwrap_or("");
        let (ln, ls) = version_part(l);
        let (rn, rs) = version_part(r);
        match ln.cmp(&rn).then_with(|| ls.cmp(rs)) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

fn split_version(value: &str) -> Vec<String> {
    value
        .split(['.', '-', '_', '+'])
        .map(str::to_string)
        .collect()
}

/// Kind of a control / declaration block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    If,
    ElseIf,
    Else,
    While,
    DoWhile,
    For,
    Foreach,
    Switch,
    Try,
    Catch,
    Finally,
    Function,
    ArrowFn,
    Class,
    Interface,
    Trait,
    Enum,
    Declare,
    Namespace,
}

/// Shape of a block body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodySpan {
    Brace { open: usize, close: Option<usize> },
    Colon { colon: usize, end: Option<usize> },
    Single { first: usize, last: usize },
    Empty { semi: usize },
    Missing,
}

/// One declared parameter: `$name`, optional type/default ranges
/// (inclusive token indices).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub name_idx: usize,
    pub ty: Option<(usize, usize)>,
    pub default: Option<(usize, usize)>,
    pub by_ref: bool,
    pub variadic: bool,
}

/// One `case` / `default` arm: keyword, `:`, and inclusive body
/// token bounds (`None` when empty).
#[derive(Debug, Clone)]
pub struct CaseBlock {
    pub kw_idx: usize,
    pub is_default: bool,
    pub colon: Option<usize>,
    pub body_first: Option<usize>,
    pub body_last: Option<usize>,
}

/// A control-structure or declaration block: keyword, header,
/// params, modifiers, body, and nesting links.
#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    pub kw_idx: usize,
    pub name: Option<String>,
    pub name_idx: Option<usize>,
    pub lparen: Option<usize>,
    pub rparen: Option<usize>,
    pub params: Vec<Param>,
    pub modifiers: Vec<String>,
    pub body: BodySpan,
    pub cases: Vec<CaseBlock>,
    pub linked: Vec<usize>,
    pub tail_while: Option<usize>,
    pub tail_rparen: Option<usize>,
    pub closure_use: Option<(usize, usize)>,
    pub extends: Option<(String, usize)>,
    pub implements: Vec<(String, usize)>,
    pub is_anonymous: bool,
    pub parent: Option<usize>,
    pub depth: usize,
}

impl Block {
    fn new(kind: BlockKind, kw_idx: usize) -> Self {
        Self {
            kind,
            kw_idx,
            name: None,
            name_idx: None,
            lparen: None,
            rparen: None,
            params: Vec::new(),
            modifiers: Vec::new(),
            body: BodySpan::Missing,
            cases: Vec::new(),
            linked: Vec::new(),
            tail_while: None,
            tail_rparen: None,
            closure_use: None,
            extends: None,
            implements: Vec::new(),
            is_anonymous: false,
            parent: None,
            depth: 0,
        }
    }
}

fn is_modifier_kind(kind: &str) -> bool {
    use kinds::*;
    matches!(
        kind,
        T_PUBLIC | T_PRIVATE | T_PROTECTED | T_STATIC | T_ABSTRACT | T_FINAL | T_VAR | T_READONLY
    )
}

impl Tree {
    /// First token index of a block body, if any.
    #[must_use]
    pub fn body_first(&self, block: &Block) -> Option<usize> {
        match block.body {
            BodySpan::Brace { open, .. } => self.next_semantic(open),
            BodySpan::Colon { colon, .. } => self.next_semantic(colon),
            BodySpan::Single { first, .. } => Some(first),
            BodySpan::Empty { semi } => Some(semi),
            BodySpan::Missing => None,
        }
    }

    /// Last token index of a block body (inclusive), if any.
    #[must_use]
    pub fn body_last(&self, block: &Block) -> Option<usize> {
        match block.body {
            BodySpan::Brace { close, .. } => close.or_else(|| self.tokens.len().checked_sub(1)),
            BodySpan::Colon { end, .. } => end
                .and_then(|e| self.prev_semantic(e))
                .or_else(|| self.tokens.len().checked_sub(1)),
            BodySpan::Single { last, .. } => Some(last),
            BodySpan::Empty { semi } => Some(semi),
            BodySpan::Missing => None,
        }
    }

    /// True when a function block is a method (directly inside a
    /// class-like body).
    #[must_use]
    pub fn is_method(&self, blocks: &[Block], idx: usize) -> bool {
        if blocks[idx].kind != BlockKind::Function {
            return false;
        }
        blocks[idx].parent.is_some_and(|p| {
            matches!(
                blocks[p].kind,
                BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
            )
        })
    }

    /// Enclosing class-like block index, if any.
    #[must_use]
    pub fn enclosing_class(&self, blocks: &[Block], idx: usize) -> Option<usize> {
        let mut cur = blocks[idx].parent;
        while let Some(p) = cur {
            if matches!(
                blocks[p].kind,
                BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
            ) {
                return Some(p);
            }
            cur = blocks[p].parent;
        }
        None
    }

    /// Parse all control-structure and declaration blocks.
    #[must_use]
    pub fn blocks(&self) -> Vec<Block> {
        let tails = self.do_while_tails();
        let mut out = Vec::new();
        for i in 0..self.tokens.len() {
            if let Some(block) = self.block_at(i, &tails) {
                out.push(block);
            }
        }
        link_blocks(self, &mut out);
        out
    }

    fn do_while_tails(&self) -> Vec<usize> {
        let mut tails = Vec::new();
        for i in 0..self.tokens.len() {
            if self.tokens[i].kind != kinds::T_DO {
                continue;
            }
            let Some(open) = self.next_semantic(i) else {
                continue;
            };
            if self.tokens[open].kind != "{" {
                continue;
            }
            let Some(close) = self.match_close(open) else {
                continue;
            };
            if self
                .next_semantic(close)
                .is_some_and(|w| self.tokens[w].kind == kinds::T_WHILE && !tails.contains(&w))
            {
                tails.push(self.next_semantic(close).unwrap_or(close));
            }
        }
        tails
    }

    fn block_at(&self, i: usize, tails: &[usize]) -> Option<Block> {
        if let Some(block) = self.control_block_at(i, tails) {
            return Some(block);
        }
        self.decl_block_at(i)
    }

    fn control_block_at(&self, i: usize, tails: &[usize]) -> Option<Block> {
        use kinds::*;
        match self.tokens[i].kind.as_str() {
            T_IF => Some(self.parse_paren_block(BlockKind::If, i)),
            T_ELSEIF => Some(self.parse_paren_block(BlockKind::ElseIf, i)),
            T_WHILE if !tails.contains(&i) => Some(self.parse_paren_block(BlockKind::While, i)),
            T_FOR => Some(self.parse_paren_block(BlockKind::For, i)),
            T_FOREACH => Some(self.parse_paren_block(BlockKind::Foreach, i)),
            T_SWITCH => Some(self.parse_switch(i)),
            T_ELSE => Some(self.parse_else(i)),
            T_DO => Some(self.parse_do(i)),
            T_TRY => Some(self.parse_brace_only(BlockKind::Try, i)),
            T_CATCH => Some(self.parse_paren_block(BlockKind::Catch, i)),
            T_FINALLY => Some(self.parse_brace_only(BlockKind::Finally, i)),
            T_DECLARE => Some(self.parse_paren_block(BlockKind::Declare, i)),
            _ => None,
        }
    }

    fn decl_block_at(&self, i: usize) -> Option<Block> {
        use kinds::*;
        match self.tokens[i].kind.as_str() {
            T_FUNCTION => self.parse_function(i),
            T_FN => Some(self.parse_arrow(i)),
            T_CLASS if self.class_is_declaration(i) => {
                Some(self.parse_class_like(BlockKind::Class, i))
            }
            T_INTERFACE => Some(self.parse_class_like(BlockKind::Interface, i)),
            T_TRAIT => Some(self.parse_class_like(BlockKind::Trait, i)),
            T_ENUM => Some(self.parse_class_like(BlockKind::Enum, i)),
            T_NAMESPACE => Some(self.parse_namespace(i)),
            _ => None,
        }
    }

    fn class_is_declaration(&self, i: usize) -> bool {
        if let Some(prev) = self.prev_semantic(i) {
            if self.tokens[prev].kind == kinds::T_PAAMAYIM_NEKUDOTAYIM {
                return false;
            }
            if self.tokens[prev].kind == kinds::T_NEW {
                return true;
            }
        }
        true
    }

    fn parse_paren_block(&self, kind: BlockKind, kw: usize) -> Block {
        let mut block = Block::new(kind, kw);
        let Some(lparen) = self.next_semantic(kw) else {
            return block;
        };
        if self.tokens[lparen].kind != "(" {
            block.body = self.body_from(lparen);
            return block;
        }
        block.lparen = Some(lparen);
        block.rparen = self.match_close(lparen);
        let after = block.rparen.unwrap_or(lparen);
        block.body = self.parse_body_after(after, kind);
        block
    }

    fn parse_body_after(&self, after: usize, kind: BlockKind) -> BodySpan {
        let Some(first) = self.next_semantic(after) else {
            return BodySpan::Missing;
        };
        if self.tokens[first].kind == ":" && takes_colon(kind) {
            return BodySpan::Colon {
                colon: first,
                end: self.find_ender(kind, first),
            };
        }
        self.body_from(first)
    }

    fn body_from(&self, first: usize) -> BodySpan {
        match self.tokens[first].kind.as_str() {
            "{" => BodySpan::Brace {
                open: first,
                close: self.match_close(first),
            },
            ";" => BodySpan::Empty { semi: first },
            "}" | kinds::T_CLOSE_TAG => BodySpan::Missing,
            _ => BodySpan::Single {
                first,
                last: self.single_end(first),
            },
        }
    }

    fn single_end(&self, first: usize) -> usize {
        let mut depth = 0;
        let mut i = first;
        let mut last = first;
        while i < self.tokens.len() {
            let kind = self.tokens[i].kind.as_str();
            if is_open(kind) {
                depth += 1;
            } else if is_close(kind) {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            } else if kind == ";" && depth == 0 {
                return i;
            } else if kind == kinds::T_CLOSE_TAG && depth == 0 {
                break;
            }
            if is_semantic(&self.tokens[i]) {
                last = i;
            }
            i += 1;
        }
        last
    }

    fn find_ender(&self, kind: BlockKind, colon: usize) -> Option<usize> {
        let (opener, ender) = ender_pair(kind)?;
        let mut depth = 0;
        let mut i = colon + 1;
        while i < self.tokens.len() {
            let kind_name = self.tokens[i].kind.as_str();
            if kind_name == opener && self.is_colon_form(i) {
                depth += 1;
            } else if kind_name == ender {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            } else if depth == 0 && ender_breaks_on_else(kind, kind_name) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// True when the opener at `idx` uses `: ... end*` form.
    fn is_colon_form(&self, idx: usize) -> bool {
        let Some(lparen) = self.next_semantic(idx) else {
            return false;
        };
        if self.tokens[lparen].kind != "(" {
            return self.tokens[lparen].kind == ":";
        }
        self.match_close(lparen)
            .and_then(|r| self.next_semantic(r))
            .is_some_and(|n| self.tokens[n].kind == ":")
    }

    fn parse_switch(&self, kw: usize) -> Block {
        let mut block = self.parse_paren_block(BlockKind::Switch, kw);
        block.cases = self.parse_cases(&block);
        block
    }

    fn parse_cases(&self, block: &Block) -> Vec<CaseBlock> {
        let (start, end) = match block.body {
            BodySpan::Brace { open, close } => (open + 1, close.unwrap_or(self.tokens.len())),
            BodySpan::Colon { colon, end } => (colon + 1, end.unwrap_or(self.tokens.len())),
            _ => return Vec::new(),
        };
        let mut heads = Vec::new();
        let mut depth: u32 = 0;
        let mut i = start;
        while i < end && i < self.tokens.len() {
            let kind = self.tokens[i].kind.as_str();
            if is_open(kind) {
                depth += 1;
            } else if is_close(kind) {
                depth = depth.saturating_sub(1);
            } else if depth == 0 && (kind == kinds::T_CASE || kind == kinds::T_DEFAULT) {
                heads.push(i);
            }
            i += 1;
        }
        let mut cases = Vec::new();
        for (n, head) in heads.iter().enumerate() {
            let boundary = heads
                .get(n + 1)
                .copied()
                .unwrap_or(end)
                .min(self.tokens.len());
            cases.push(self.parse_case(*head, boundary));
        }
        cases
    }

    fn parse_case(&self, head: usize, boundary: usize) -> CaseBlock {
        let is_default = self.tokens[head].kind == kinds::T_DEFAULT;
        let mut colon = None;
        let mut depth: u32 = 0;
        let mut i = head + 1;
        while i < boundary {
            let kind = self.tokens[i].kind.as_str();
            if is_open(kind) {
                depth += 1;
            } else if is_close(kind) {
                depth = depth.saturating_sub(1);
            } else if depth == 0 && (kind == ":" || kind == ";") {
                colon = Some(i);
                break;
            }
            i += 1;
        }
        let (body_first, body_last) = match colon {
            None => (None, None),
            Some(c) => {
                let first = self.next_semantic(c).filter(|f| *f < boundary);
                let last = first.and_then(|_| {
                    let mut l = boundary;
                    while l > c + 1 {
                        l -= 1;
                        if is_semantic(&self.tokens[l]) {
                            return Some(l);
                        }
                    }
                    None
                });
                (first, last)
            }
        };
        CaseBlock {
            kw_idx: head,
            is_default,
            colon,
            body_first,
            body_last,
        }
    }

    fn parse_else(&self, kw: usize) -> Block {
        let mut block = Block::new(BlockKind::Else, kw);
        let Some(first) = self.next_semantic(kw) else {
            return block;
        };
        if self.tokens[first].kind == ":" {
            block.body = BodySpan::Colon {
                colon: first,
                end: self.find_ender(BlockKind::Else, first),
            };
        } else {
            block.body = self.body_from(first);
        }
        block
    }

    fn parse_do(&self, kw: usize) -> Block {
        let mut block = Block::new(BlockKind::DoWhile, kw);
        let Some(open) = self.next_semantic(kw) else {
            return block;
        };
        if self.tokens[open].kind != "{" {
            block.body = self.body_from(open);
            return block;
        }
        let close = self.match_close(open);
        block.body = BodySpan::Brace { open, close };
        let Some(close_idx) = close else {
            return block;
        };
        let tail = self
            .next_semantic(close_idx)
            .filter(|w| self.tokens[*w].kind == kinds::T_WHILE);
        let Some(w) = tail else {
            return block;
        };
        block.tail_while = Some(w);
        if let Some(lp) = self.next_semantic(w)
            && self.tokens[lp].kind == "("
        {
            block.tail_rparen = self.match_close(lp);
        }
        block
    }

    fn parse_brace_only(&self, kind: BlockKind, kw: usize) -> Block {
        let mut block = Block::new(kind, kw);
        let Some(first) = self.next_semantic(kw) else {
            return block;
        };
        block.body = self.body_from(first);
        block
    }

    fn parse_function(&self, kw: usize) -> Option<Block> {
        let mut block = Block::new(BlockKind::Function, kw);
        block.modifiers = self.modifiers_before(kw);
        let cur = self.function_name_end(kw, &mut block)?;
        if self.tokens[cur].kind != "(" {
            return None;
        }
        block.lparen = Some(cur);
        block.rparen = self.match_close(cur);
        let rparen = block.rparen.unwrap_or(cur);
        block.params = self.parse_params(cur, rparen);
        let after_use = self.closure_use_end(rparen, &mut block);
        let after = self.skip_return_type(after_use);
        let Some(body_at) = after else {
            return Some(block);
        };
        block.body = self.body_from(body_at);
        Some(block)
    }

    fn function_name_end(&self, kw: usize, block: &mut Block) -> Option<usize> {
        let mut cur = self.next_semantic(kw)?;
        if self.tokens[cur].kind == "&" {
            cur = self.next_semantic(cur)?;
        }
        if self.tokens[cur].kind == kinds::T_STRING {
            block.name = Some(self.tokens[cur].value.clone());
            block.name_idx = Some(cur);
            cur = self.next_semantic(cur)?;
        } else if self.tokens[cur].kind == "(" {
            block.is_anonymous = true;
        } else {
            return None;
        }
        Some(cur)
    }

    fn closure_use_end(&self, rparen: usize, block: &mut Block) -> Option<usize> {
        let after = self.next_semantic(rparen);
        if after.is_none_or(|a| self.tokens[a].kind != kinds::T_USE) {
            return after;
        }
        let use_kw = after.unwrap_or(rparen);
        if let Some(lp) = self.next_semantic(use_kw)
            && self.tokens[lp].kind == "("
            && let Some(rp) = self.match_close(lp)
        {
            block.closure_use = Some((lp, rp));
            return self.next_semantic(rp);
        }
        after
    }

    fn skip_return_type(&self, after: Option<usize>) -> Option<usize> {
        let Some(colon) = after else {
            return after;
        };
        if self.tokens[colon].kind != ":" {
            return after;
        }
        let first = self.next_semantic(colon)?;
        self.union_type_end(first)
    }

    fn union_type_end(&self, first: usize) -> Option<usize> {
        let mut next = self.next_semantic(first);
        while let Some(a) = next {
            if !self.is_union_member(a) {
                break;
            }
            next = self.next_semantic(a);
        }
        next
    }

    fn is_union_member(&self, a: usize) -> bool {
        matches!(
            self.tokens[a].kind.as_str(),
            "|" | "&" | kinds::T_STRING | "\\"
        )
    }

    fn parse_arrow(&self, kw: usize) -> Block {
        let mut block = Block::new(BlockKind::ArrowFn, kw);
        block.modifiers = self.modifiers_before(kw);
        block.is_anonymous = true;
        let Some(lp) = self.next_semantic(kw) else {
            return block;
        };
        if self.tokens[lp].kind != "(" {
            return block;
        }
        block.lparen = Some(lp);
        block.rparen = self.match_close(lp);
        let rparen = block.rparen.unwrap_or(lp);
        block.params = self.parse_params(lp, rparen);
        let Some(arrow) = self.next_semantic(rparen) else {
            return block;
        };
        if self.tokens[arrow].kind != kinds::T_DOUBLE_ARROW {
            return block;
        }
        let Some(first) = self.next_semantic(arrow) else {
            return block;
        };
        block.body = BodySpan::Single {
            first,
            last: self.arrow_end(first),
        };
        block
    }

    fn arrow_end(&self, first: usize) -> usize {
        let mut depth = 0;
        let mut i = first;
        let mut last = first;
        while i < self.tokens.len() {
            let kind = self.tokens[i].kind.as_str();
            if is_open(kind) {
                depth += 1;
            } else if is_close(kind) {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            } else if depth == 0 && (kind == ";" || kind == ",") {
                break;
            }
            if is_semantic(&self.tokens[i]) {
                last = i;
            }
            i += 1;
        }
        last
    }

    fn parse_class_like(&self, kind: BlockKind, kw: usize) -> Block {
        let mut block = Block::new(kind, kw);
        block.modifiers = self.modifiers_before(kw);
        let Some(first) = self.next_semantic(kw) else {
            return block;
        };
        let mut cur = self.class_name_end(first, kind, &mut block);
        while cur < self.tokens.len() {
            let Some(next) = self.class_header_step(&mut block, cur) else {
                break;
            };
            if next == cur {
                break;
            }
            cur = next;
        }
        block
    }

    fn class_name_end(&self, first: usize, kind: BlockKind, block: &mut Block) -> usize {
        if self.tokens[first].kind == kinds::T_STRING {
            block.name = Some(self.tokens[first].value.clone());
            block.name_idx = Some(first);
            self.next_semantic(first).unwrap_or(first)
        } else {
            block.is_anonymous = kind == BlockKind::Class;
            first
        }
    }

    /// Process one header token; returns the next cursor, or `None`
    /// to stop scanning the header.
    fn class_header_step(&self, block: &mut Block, cur: usize) -> Option<usize> {
        let kind_name = self.tokens[cur].kind.as_str();
        if kind_name == kinds::T_EXTENDS {
            return self.class_extends_end(block, cur);
        }
        if kind_name == kinds::T_IMPLEMENTS {
            let after = self.parse_implements(block, cur);
            return self.next_semantic(after);
        }
        if kind_name == "{" || kind_name == ";" {
            block.body = self.body_from(cur);
            return None;
        }
        if kind_name == "(" {
            return self.class_paren_end(cur);
        }
        if Self::class_header_breaks(&self.tokens[cur], kind_name) {
            return None;
        }
        self.next_semantic(cur)
    }

    fn class_header_breaks(token: &Token, kind_name: &str) -> bool {
        is_semantic(token) && kind_name != kinds::T_STRING && kind_name != "\\"
    }

    fn class_extends_end(&self, block: &mut Block, cur: usize) -> Option<usize> {
        let mut end = cur;
        if let Some(name_idx) = self.next_semantic(cur) {
            block.extends = Some((self.qualified_name(name_idx), name_idx));
            end = name_idx;
        }
        self.next_semantic(end)
    }

    fn class_paren_end(&self, cur: usize) -> Option<usize> {
        let mut end = cur;
        if let Some(close) = self.match_close(cur) {
            end = close;
        }
        self.next_semantic(end)
    }

    fn qualified_name(&self, mut idx: usize) -> String {
        let mut name = String::new();
        while self.tokens[idx].kind == kinds::T_STRING || self.tokens[idx].kind == "\\" {
            name.push_str(&self.tokens[idx].value);
            let Some(next) = self.next_semantic(idx) else {
                break;
            };
            if self.tokens[next].kind != kinds::T_STRING && self.tokens[next].kind != "\\" {
                break;
            }
            idx = next;
        }
        name
    }

    fn parse_implements(&self, block: &mut Block, kw: usize) -> usize {
        let mut cur = kw;
        while let Some(next) = self.next_semantic(cur) {
            let kind_name = self.tokens[next].kind.as_str();
            if kind_name == kinds::T_STRING || kind_name == "\\" {
                let name = self.qualified_name(next);
                block.implements.push((name, next));
                cur = self.qualified_end(next);
            } else if kind_name == "," {
                cur = next;
            } else {
                break;
            }
        }
        cur
    }

    fn qualified_end(&self, mut idx: usize) -> usize {
        while let Some(next) = self.next_semantic(idx) {
            if self.tokens[next].kind != kinds::T_STRING && self.tokens[next].kind != "\\" {
                break;
            }
            idx = next;
        }
        idx
    }

    fn parse_namespace(&self, kw: usize) -> Block {
        let mut block = Block::new(BlockKind::Namespace, kw);
        let Some(cur) = self.next_semantic(kw) else {
            return block;
        };
        let kind_name = self.tokens[cur].kind.as_str();
        if kind_name == kinds::T_STRING || kind_name == "\\" {
            block.name = Some(self.qualified_name(cur));
            block.name_idx = Some(cur);
            let end = self.qualified_end(cur);
            let Some(after) = self.next_semantic(end) else {
                return block;
            };
            block.body = self.body_from(after);
        } else {
            block.body = self.body_from(cur);
        }
        block
    }

    fn modifiers_before(&self, kw: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = kw;
        while let Some(prev) = self.prev_token(i) {
            let kind_name = self.tokens[prev].kind.as_str();
            if is_modifier_kind(kind_name) {
                out.push(self.tokens[prev].value.clone());
                i = prev;
            } else if kind_name == "]" {
                let Some(open) = self.match_open(prev) else {
                    break;
                };
                i = open;
            } else if !is_semantic(&self.tokens[prev]) {
                i = prev;
            } else {
                break;
            }
        }
        out.reverse();
        out
    }

    fn prev_token(&self, idx: usize) -> Option<usize> {
        idx.checked_sub(1)
    }

    fn parse_params(&self, lparen: usize, rparen: usize) -> Vec<Param> {
        let mut params = Vec::new();
        for (first, last) in split_args(self, lparen, rparen) {
            if let Some(param) = self.parse_param(first, last) {
                params.push(param);
            }
        }
        params
    }

    fn parse_param(&self, first: usize, last: usize) -> Option<Param> {
        let (name_idx, eq_at) = scan_param_name(self, first, last);
        let name_idx = name_idx?;
        let name = self.tokens[name_idx].value.clone();
        let (by_ref, variadic) = scan_param_flags(self, first, name_idx);
        let ty = param_ty_range(self, first, name_idx);
        let default = param_default_range(self, eq_at, last);
        Some(Param {
            name,
            name_idx,
            ty,
            default,
            by_ref,
            variadic,
        })
    }
}

fn scan_param_name(tree: &Tree, first: usize, last: usize) -> (Option<usize>, Option<usize>) {
    let mut name_idx = None;
    let mut eq_at = None;
    let mut depth: u32 = 0;
    let mut i = first;
    while i <= last {
        let kind_name = tree.tokens[i].kind.as_str();
        if is_open(kind_name) {
            depth += 1;
        } else if is_close(kind_name) {
            depth = depth.saturating_sub(1);
        } else if param_name_eq(kind_name, depth, eq_at) {
            eq_at = Some(i);
        } else if param_name_var(kind_name, depth, name_idx) {
            name_idx = Some(i);
        }
        i += 1;
    }
    (name_idx, eq_at)
}

fn param_name_eq(kind_name: &str, depth: u32, eq_at: Option<usize>) -> bool {
    depth == 0 && kind_name == "=" && eq_at.is_none()
}

fn param_name_var(kind_name: &str, depth: u32, name_idx: Option<usize>) -> bool {
    depth == 0 && kind_name == kinds::T_VARIABLE && name_idx.is_none()
}

fn scan_param_flags(tree: &Tree, first: usize, name_idx: usize) -> (bool, bool) {
    let mut by_ref = false;
    let mut variadic = false;
    let mut j = first;
    while j < name_idx {
        match tree.tokens[j].kind.as_str() {
            "&" => by_ref = true,
            kinds::T_ELLIPSIS => variadic = true,
            _ => {}
        }
        j += 1;
    }
    (by_ref, variadic)
}

fn param_ty_range(tree: &Tree, first: usize, name_idx: usize) -> Option<(usize, usize)> {
    let end = walker_before_name(tree, first, name_idx)?;
    let mut start = first;
    while start <= end && !is_semantic(&tree.tokens[start]) {
        start += 1;
    }
    if start <= end {
        Some((start, end))
    } else {
        None
    }
}

fn param_default_range(tree: &Tree, eq_at: Option<usize>, last: usize) -> Option<(usize, usize)> {
    let eq = eq_at?;
    let start = eq + 1;
    if start <= last {
        trim_trivia(tree, start, last)
    } else {
        None
    }
}

fn ender_pair(kind: BlockKind) -> Option<(&'static str, &'static str)> {
    use kinds::*;
    match kind {
        BlockKind::If | BlockKind::ElseIf | BlockKind::Else => Some((T_IF, T_ENDIF)),
        BlockKind::While => Some((T_WHILE, T_ENDWHILE)),
        BlockKind::For => Some((T_FOR, T_ENDFOR)),
        BlockKind::Foreach => Some((T_FOREACH, T_ENDFOREACH)),
        BlockKind::Switch => Some((T_SWITCH, T_ENDSWITCH)),
        BlockKind::Declare => Some((T_DECLARE, T_ENDDECLARE)),
        _ => None,
    }
}

fn ender_breaks_on_else(kind: BlockKind, kind_name: &str) -> bool {
    use kinds::*;
    matches!(kind, BlockKind::If | BlockKind::ElseIf | BlockKind::Else)
        && matches!(kind_name, T_ELSE | T_ELSEIF)
}

#[cfg(test)]
#[path = "xhp_ast_tests.rs"]
mod xhp_ast_tests;

/// Last type-token index before a parameter name (skipping `&`,
/// `...` and trivia), if any type is present.
fn walker_before_name(tree: &Tree, first: usize, name_idx: usize) -> Option<usize> {
    if name_idx == 0 || first >= name_idx {
        return None;
    }
    let mut end = name_idx - 1;
    loop {
        let kind_name = tree.tokens[end].kind.as_str();
        if kind_name == "&" || kind_name == kinds::T_ELLIPSIS || !is_semantic(&tree.tokens[end]) {
            if end == first {
                return None;
            }
            end -= 1;
            continue;
        }
        break;
    }
    if end < first {
        return None;
    }
    Some(end)
}

fn takes_colon(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::If
            | BlockKind::ElseIf
            | BlockKind::Else
            | BlockKind::While
            | BlockKind::For
            | BlockKind::Foreach
            | BlockKind::Switch
            | BlockKind::Declare
    )
}

fn link_blocks(tree: &Tree, blocks: &mut [Block]) {
    let lasts: Vec<Option<usize>> = blocks.iter().map(|b| tree.body_last(b)).collect();
    for i in 0..blocks.len() {
        let mut best: Option<usize> = None;
        for j in 0..blocks.len() {
            if i == j {
                continue;
            }
            if blocks[j].kw_idx < blocks[i].kw_idx
                && lasts[j].is_some_and(|last| last > blocks[i].kw_idx)
            {
                let better = best.is_none_or(|b: usize| {
                    blocks[j].kw_idx > blocks[b].kw_idx
                        && lasts[j].unwrap_or(usize::MAX) < lasts[b].unwrap_or(usize::MAX)
                });
                if better {
                    best = Some(j);
                }
            }
        }
        blocks[i].parent = best;
    }
    for i in 0..blocks.len() {
        let mut depth = 0;
        let mut cur = blocks[i].parent;
        while let Some(p) = cur {
            depth += 1;
            cur = blocks[p].parent;
        }
        blocks[i].depth = depth;
    }
    link_try_blocks(blocks, &lasts);
}

fn link_try_blocks(blocks: &mut [Block], lasts: &[Option<usize>]) {
    for i in 0..blocks.len() {
        if blocks[i].kind != BlockKind::Try {
            continue;
        }
        let Some(try_last) = lasts[i] else {
            continue;
        };
        for cand in (i + 1)..blocks.len() {
            if blocks[cand].kw_idx <= try_last {
                continue;
            }
            if matches!(blocks[cand].kind, BlockKind::Catch | BlockKind::Finally) {
                blocks[i].linked.push(cand);
            } else {
                break;
            }
        }
    }
}
