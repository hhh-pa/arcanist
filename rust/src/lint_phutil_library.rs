//! `phutil-library` linter: libphutil symbol rules (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistPhutilLibraryLinter.php`.
//!
//! Scope: the one-class-per-file rule (`PHL3`) runs per file in
//! `lint_bytes` over a lightweight PHP declaration scan (top-level
//! `class`/`interface`/`function` declarations, skipping comments,
//! strings, heredocs, and non-PHP regions). The workspace-global rules
//! are ported as pure functions over caller-supplied symbol maps:
//! [`check_duplicates`] (`PHL2`), [`check_unknown_symbols`] (`PHL1` and
//! `PHL4`), and [`unknown_symbol_details`]. Severities are all `error`
//! (the PHP linter defines no severity map, so the default applies).
//!
//! Out of scope / divergences:
//!
//! - PHP builds its maps with `PhutilLibraryMapBuilder` over XHPAST for
//!   every library in the workspace; there is no such global analysis in
//!   the Rust engine, so `PHL1`/`PHL2`/`PHL4` fire only through the
//!   map-based helpers, and `lint_bytes` covers `PHL3` alone.
//! - The scan is syntactic, not XHPAST: bracketed `namespace { ... }`
//!   blocks and conditional declarations hide top-level symbols (false
//!   negatives), and `trait`/`enum` declarations are ignored exactly like
//!   the PHP rule (which only looks at class/interface/function).
//! - PHP keys builtin `interface` symbols inconsistently so they never
//!   collide; this port keys them by class type like every other
//!   interface (documented; the PHP behavior looks like a bug).
//! - Findings from the helpers carry byte offsets; mapping them to lines
//!   needs the defining file's bytes (see [`LibraryFinding::to_message`]).

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter, SEVERITY_ERROR};

/// Full code for unknown symbols (`PHL` + `LINT_UNKNOWN_SYMBOL`).
pub const CODE_UNKNOWN_SYMBOL: &str = "PHL1";
/// Full code for duplicate symbols (`PHL` + `LINT_DUPLICATE_SYMBOL`).
pub const CODE_DUPLICATE_SYMBOL: &str = "PHL2";
/// Full code for one-class-per-file (`PHL` + `LINT_ONE_CLASS_PER_FILE`).
pub const CODE_ONE_CLASS_PER_FILE: &str = "PHL3";
/// Full code for noncanonical symbols (`PHL` + `LINT_NONCANONICAL_SYMBOL`).
pub const CODE_NONCANONICAL_SYMBOL: &str = "PHL4";
/// Display names per code, mirroring `getLintNameMap`.
pub const NAME_UNKNOWN_SYMBOL: &str = "Unknown Symbol";
/// Display name for duplicate symbols.
pub const NAME_DUPLICATE_SYMBOL: &str = "Duplicate Symbol";
/// Display name for one-class-per-file findings.
pub const NAME_ONE_CLASS_PER_FILE: &str = "One Class Per File";
/// Display name for noncanonical symbols.
pub const NAME_NONCANONICAL_SYMBOL: &str = "Noncanonical Symbol";

/// Phutil library symbol linter.
#[derive(Debug, Default)]
pub struct PhutilLibraryLinter;

impl PhutilLibraryLinter {
    /// New phutil-library linter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Linter for PhutilLibraryLinter {
    fn name(&self) -> &'static str {
        "PHL"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_php_file(path) || data.contains(&0) {
            return Vec::new();
        }
        check_one_class_per_file(path, data)
    }
}

/// True for `.php` files (ASCII case-insensitive).
#[must_use]
pub fn is_php_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| {
            extension
                .to_str()
                .is_some_and(|text| text.eq_ignore_ascii_case("php"))
        })
}

/// Declaration kind relevant to the PHP rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolType {
    /// `class` declarations.
    Class,
    /// Named `function` declarations.
    Function,
    /// `interface` declarations.
    Interface,
}

impl SymbolType {
    /// PHP type word used in messages (`class`/`function`/`interface`).
    #[must_use]
    pub fn as_word(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Function => "function",
            Self::Interface => "interface",
        }
    }

    /// Library type: classes and interfaces share one namespace.
    #[must_use]
    pub fn libtype(self) -> LibType {
        match self {
            Self::Class | Self::Interface => LibType::Class,
            Self::Function => LibType::Function,
        }
    }
}

/// Symbol namespace: class-like versus function symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibType {
    /// Classes and interfaces.
    Class,
    /// Functions.
    Function,
}

/// How a symbol is used (mirrors the map builder `need` types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseType {
    /// Used as a class.
    Class,
    /// Used as a function.
    Function,
    /// Used as an interface.
    Interface,
    /// Used as a class or interface.
    ClassOrInterface,
}

impl UseType {
    #[must_use]
    fn libtype(self) -> LibType {
        match self {
            Self::Class | Self::Interface | Self::ClassOrInterface => LibType::Class,
            Self::Function => LibType::Function,
        }
    }
}

/// One symbol definition in a library file.
#[derive(Debug, Clone)]
pub struct SymbolDef {
    /// Symbol name as defined.
    pub symbol: String,
    /// Declaration kind.
    pub symbol_type: SymbolType,
    /// Owning library name.
    pub library: String,
    /// Library-relative file path.
    pub file: String,
    /// Byte offset of the definition.
    pub offset: usize,
}

/// One symbol use in a library file.
#[derive(Debug, Clone)]
pub struct SymbolUse {
    /// Symbol name as written.
    pub symbol: String,
    /// How the symbol is used.
    pub use_type: UseType,
    /// Owning library name.
    pub library: String,
    /// Library-relative file path.
    pub file: String,
    /// Byte offset of the use.
    pub offset: usize,
}

/// One builtin symbol (provided by PHP itself, no library or file).
#[derive(Debug, Clone)]
pub struct BuiltinSymbol {
    /// Symbol name.
    pub symbol: String,
    /// Declaration kind.
    pub symbol_type: SymbolType,
}

/// A workspace-global finding, located by library/file/byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFinding {
    /// Owning library name.
    pub library: String,
    /// Library-relative file path.
    pub file: String,
    /// Byte offset of the finding.
    pub offset: usize,
    /// Full finding code (`PHL1`..`PHL4`).
    pub code: String,
    /// Display name.
    pub name: String,
    /// Finding description.
    pub description: String,
}

impl LibraryFinding {
    /// Render as a [`LintMessage`] once the offset is mapped to a 1-based
    /// line and column with the defining file's bytes.
    #[must_use]
    pub fn to_message(&self, display_path: &str, line: usize, col: usize) -> LintMessage {
        LintMessage {
            path: display_path.to_string(),
            line,
            col,
            code: self.code.clone(),
            severity: SEVERITY_ERROR.to_string(),
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

/// Lowercase a symbol for comparison, mirroring `normalizeSymbol`.
#[must_use]
pub fn normalize_symbol(symbol: &str) -> String {
    symbol.to_lowercase()
}

/// Find duplicate definitions, mirroring the PHP duplicate pass.
///
/// Definitions are processed in order; the first definition of a symbol
/// (compared case-insensitively within its namespace) wins, and later
/// ones raise `PHL2`.
pub fn check_duplicates(defs: &[SymbolDef], builtins: &[BuiltinSymbol]) -> Vec<LibraryFinding> {
    use std::collections::HashMap;
    // (namespace, normalized) -> original symbol, plus definition sites.
    let mut canonical: HashMap<(LibType, String), String> = HashMap::new();
    let mut sites: HashMap<(LibType, String), (Option<String>, Option<String>)> = HashMap::new();
    for builtin in builtins {
        let libtype = builtin.symbol_type.libtype();
        canonical.insert(
            (libtype, normalize_symbol(&builtin.symbol)),
            builtin.symbol.clone(),
        );
        sites.insert((libtype, builtin.symbol.clone()), (None, None));
    }
    let mut out = Vec::new();
    for def in defs {
        let libtype = def.symbol_type.libtype();
        let normal = normalize_symbol(&def.symbol);
        if let Some(old) = canonical.get(&(libtype, normal.clone())) {
            let (old_file, old_lib) = sites
                .get(&(libtype, old.clone()))
                .cloned()
                .unwrap_or((None, None));
            let description = match (old_file, old_lib) {
                (Some(file), Some(library)) => format!(
                    "Definition of symbol \"{}\" (of type \"{}\") in file \"{}\" in \
                     library \"{}\" duplicates prior definition in file \"{file}\" in \
                     library \"{library}\".",
                    def.symbol,
                    def.symbol_type.as_word(),
                    def.file,
                    def.library,
                ),
                _ => format!(
                    "Definition of symbol \"{}\" (of type \"{}\") in file \"{}\" in \
                     library \"{}\" duplicates builtin definition of the same symbol.",
                    def.symbol,
                    def.symbol_type.as_word(),
                    def.file,
                    def.library,
                ),
            };
            out.push(LibraryFinding {
                library: def.library.clone(),
                file: def.file.clone(),
                offset: def.offset,
                code: CODE_DUPLICATE_SYMBOL.to_string(),
                name: NAME_DUPLICATE_SYMBOL.to_string(),
                description,
            });
            continue;
        }
        canonical.insert((libtype, normal), def.symbol.clone());
        sites.insert(
            (libtype, def.symbol.clone()),
            (Some(def.file.clone()), Some(def.library.clone())),
        );
    }
    out
}

/// Find unknown and noncanonical symbol uses, mirroring the PHP unknown
/// pass. `client_name` and `arcanist_root` feed the "common causes" text.
pub fn check_unknown_symbols(
    uses: &[SymbolUse],
    defs: &[SymbolDef],
    builtins: &[BuiltinSymbol],
    client_name: &str,
    arcanist_root: &str,
) -> Vec<LibraryFinding> {
    use std::collections::{HashMap, HashSet};
    let mut defined: HashSet<(LibType, &str)> = HashSet::new();
    let mut canonical: HashMap<(LibType, String), &str> = HashMap::new();
    for builtin in builtins {
        let libtype = builtin.symbol_type.libtype();
        defined.insert((libtype, builtin.symbol.as_str()));
        canonical.insert(
            (libtype, normalize_symbol(&builtin.symbol)),
            builtin.symbol.as_str(),
        );
    }
    for def in defs {
        let libtype = def.symbol_type.libtype();
        defined.insert((libtype, def.symbol.as_str()));
        canonical
            .entry((libtype, normalize_symbol(&def.symbol)))
            .or_insert(def.symbol.as_str());
    }
    let mut out = Vec::new();
    for use_ in uses {
        let libtype = use_.use_type.libtype();
        if defined.contains(&(libtype, use_.symbol.as_str())) {
            continue;
        }
        if let Some(proper) = canonical.get(&(libtype, normalize_symbol(&use_.symbol))) {
            let description = match use_.use_type {
                UseType::Class => {
                    format!(
                        "Class symbol \"{}\" should be written as \"{proper}\".",
                        use_.symbol
                    )
                }
                UseType::Function => format!(
                    "Function symbol \"{}\" should be written as \"{proper}\".",
                    use_.symbol
                ),
                UseType::Interface => format!(
                    "Interface symbol \"{}\" should be written as \"{proper}\".",
                    use_.symbol
                ),
                UseType::ClassOrInterface => format!(
                    "Class or interface symbol \"{}\" should be written as \"{proper}\".",
                    use_.symbol
                ),
            };
            out.push(LibraryFinding {
                library: use_.library.clone(),
                file: use_.file.clone(),
                offset: use_.offset,
                code: CODE_NONCANONICAL_SYMBOL.to_string(),
                name: NAME_NONCANONICAL_SYMBOL.to_string(),
                description,
            });
            continue;
        }
        let summary = match use_.use_type {
            UseType::Class => format!("Use of unknown class symbol \"{}\".", use_.symbol),
            UseType::Function => {
                format!("Use of unknown function symbol \"{}\".", use_.symbol)
            }
            UseType::Interface => {
                format!("Use of unknown interface symbol \"{}\".", use_.symbol)
            }
            UseType::ClassOrInterface => format!(
                "Use of unknown class or interface symbol \"{}\".",
                use_.symbol
            ),
        };
        out.push(LibraryFinding {
            library: use_.library.clone(),
            file: use_.file.clone(),
            offset: use_.offset,
            code: CODE_UNKNOWN_SYMBOL.to_string(),
            name: NAME_UNKNOWN_SYMBOL.to_string(),
            description: format!(
                "{summary}\n\n{}",
                unknown_symbol_details(client_name, arcanist_root)
            ),
        });
    }
    out
}

/// The "common causes" detail appended to unknown-symbol findings.
#[must_use]
pub fn unknown_symbol_details(client_name: &str, arcanist_root: &str) -> String {
    format!(
        "Common causes are:\n\
         \n\
         \x20 - Your copy of {client_name} is out of date.\n\
         \x20   This is the most common cause.\n\
         \x20   Update this copy of {client_name}:\n\
         \n\
         \x20     {arcanist_root}\n\
         \n\
         \x20 - Some other library is out of date.\n\
         \x20   Update the library this symbol appears in.\n\
         \n\
         \x20 - The symbol is misspelled.\n\
         \x20   Spell the symbol name correctly.\n\
         \n\
         \x20 - You added the symbol recently, but have not updated\n\
         \x20   the symbol map for the library.\n\
         \x20   Run \"arc liberate\" in the library where the symbol is\n\
         \x20   defined.\n\
         \n\
         \x20 - This symbol is defined in an external library.\n\
         \x20   Use \"@phutil-external-symbol\" to annotate it.\n\
         \x20   Use \"grep\" to find examples of usage."
    )
}

/// Apply the one-class-per-file rule to one PHP file, mirroring the PHP
/// file pass (raised at the last function offset for mixes, else at the
/// last class offset).
pub fn check_one_class_per_file(path: &str, data: &[u8]) -> Vec<LintMessage> {
    let decls = scan_php_declarations(data);
    let mut classes: Vec<&PhpDecl> = Vec::new();
    let mut functions: Vec<&PhpDecl> = Vec::new();
    for decl in &decls {
        match decl.kind {
            DeclKind::ClassOrInterface => classes.push(decl),
            DeclKind::Function => functions.push(decl),
        }
    }
    if !functions.is_empty() && !classes.is_empty() {
        let function_list = join_names(&functions);
        let class_list = join_names(&classes);
        let offset = functions.last().map(|decl| decl.offset).unwrap_or(0);
        let (line, col) = line_col_for_offset(data, offset);
        return vec![LintMessage {
            path: path.to_string(),
            line,
            col,
            code: CODE_ONE_CLASS_PER_FILE.to_string(),
            severity: SEVERITY_ERROR.to_string(),
            name: NAME_ONE_CLASS_PER_FILE.to_string(),
            description: format!(
                "File '{path}' mixes function ({function_list}) and class/interface \
                 ({class_list}) definitions in the same file. A file which declares a class \
                 or an interface MUST declare nothing else."
            ),
        }];
    }
    if classes.len() > 1 {
        let class_list = join_names(&classes);
        let offset = classes.last().map(|decl| decl.offset).unwrap_or(0);
        let (line, col) = line_col_for_offset(data, offset);
        return vec![LintMessage {
            path: path.to_string(),
            line,
            col,
            code: CODE_ONE_CLASS_PER_FILE.to_string(),
            severity: SEVERITY_ERROR.to_string(),
            name: NAME_ONE_CLASS_PER_FILE.to_string(),
            description: format!(
                "File '{path}' declares more than one class or interface ({class_list}). \
                 A file which declares a class or interface MUST declare nothing else."
            ),
        }];
    }
    Vec::new()
}

fn join_names(decls: &[&PhpDecl]) -> String {
    decls
        .iter()
        .map(|decl| decl.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Top-level declaration found by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PhpDecl {
    kind: DeclKind,
    name: String,
    /// Byte offset of the declaring keyword.
    offset: usize,
}

/// Declaration namespace relevant to the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclKind {
    ClassOrInterface,
    Function,
}

/// Scan top-level `class`/`interface`/named-`function` declarations.
///
/// Tracks `<?php ... ?>` regions, brace depth, comments, strings, and
/// heredocs so keywords in non-code do not count. Anonymous classes,
/// closures, methods, `::class` constants, and `<?xml` prologs are
/// excluded.
fn scan_php_declarations(data: &[u8]) -> Vec<PhpDecl> {
    let mut decls = Vec::new();
    let len = data.len();
    let mut index = 0;
    let mut in_php = false;
    let mut depth: usize = 0;
    while index < len {
        if !in_php {
            index = step_outside_php(data, index, &mut in_php);
            continue;
        }
        if let Some((after, closed)) = skip_php_trivia(data, index) {
            index = after;
            if closed {
                in_php = false;
            }
            continue;
        }
        index = step_php_code(data, index, &mut depth, &mut decls);
    }
    decls
}

/// Advance one step outside `<?php` code: honor `<?` open tags, else skip a
/// byte. `index` is below `data.len()`.
fn step_outside_php(data: &[u8], index: usize, in_php: &mut bool) -> usize {
    if is_open_tag(data, index) {
        *in_php = true;
        index + 2
    } else {
        index + 1
    }
}

/// Skip comments, strings, heredocs, and `?>` inside PHP code. Returns the
/// resume offset and whether PHP mode closed, or `None` for code bytes.
/// `index` is below `data.len()`.
fn skip_php_trivia(data: &[u8], index: usize) -> Option<(usize, bool)> {
    let byte = data[index];
    let next = data.get(index + 1).copied().unwrap_or(0);
    if (byte == b'/' && next == b'/') || byte == b'#' {
        let (after, closed) = skip_line_rest(data, index);
        return Some((after, closed));
    }
    if byte == b'/' && next == b'*' {
        return Some((skip_block_comment(data, index), false));
    }
    if byte == b'\'' || byte == b'"' || byte == b'`' {
        return Some((skip_quoted(data, index), false));
    }
    if let Some(after) = skip_heredoc(data, index) {
        return Some((after, false));
    }
    if byte == b'?' && next == b'>' {
        return Some((index + 2, true));
    }
    None
}

/// Advance one step over PHP code bytes: track braces, match top-level
/// declarations, else skip a byte. `index` is below `data.len()`.
fn step_php_code(data: &[u8], index: usize, depth: &mut usize, decls: &mut Vec<PhpDecl>) -> usize {
    let byte = data[index];
    if byte == b'{' {
        *depth += 1;
        return index + 1;
    }
    if byte == b'}' {
        *depth = depth.saturating_sub(1);
        return index + 1;
    }
    if *depth == 0
        && let Some((decl, end)) = match_declaration(data, index)
    {
        if let Some(decl) = decl {
            decls.push(decl);
        }
        return end;
    }
    index + 1
}

/// Match a declaration keyword at `index`: the declaration (absent for
/// anonymous classes, closures, and `::class` fetches) plus the offset just
/// past the keyword. `index` is below `data.len()`.
fn match_declaration(data: &[u8], index: usize) -> Option<(Option<PhpDecl>, usize)> {
    let byte = data[index];
    if !is_identifier_start(byte) || !is_left_boundary(data, index) {
        return None;
    }
    let (kind, end) = match_keyword(data, index)?;
    let excluded = kind == DeclKind::ClassOrInterface && is_double_colon_before(data, index);
    let decl = if excluded {
        None
    } else {
        parse_declaration(data, index, kind, end)
    };
    Some((decl, end))
}

/// `<?` that is not a `<?xml` prolog.
fn is_open_tag(data: &[u8], index: usize) -> bool {
    if data[index] != b'<' || data.get(index + 1) != Some(&b'?') {
        return false;
    }
    let rest = &data[index + 2..];
    if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"xml") {
        let after = rest.get(3).copied().unwrap_or(0);
        if !is_identifier_char(after) {
            return false;
        }
    }
    true
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_identifier_char(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

/// Keywords must not follow identifier characters, `$`, or `\`.
fn is_left_boundary(data: &[u8], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let prev = data[index - 1];
    !(is_identifier_char(prev) || prev == b'$' || prev == b'\\')
}

/// Match `class`/`interface`/`function` (any case) with a right boundary.
fn match_keyword(data: &[u8], index: usize) -> Option<(DeclKind, usize)> {
    for (word, kind) in [
        ("class", DeclKind::ClassOrInterface),
        ("interface", DeclKind::ClassOrInterface),
        ("function", DeclKind::Function),
    ] {
        let end = index + word.len();
        let after = data.get(end).copied();
        let boundary =
            after.is_none_or(|byte| !is_identifier_char(byte) && byte != b'$' && byte != b'\\');
        if data.len() >= end && data[index..end].eq_ignore_ascii_case(word.as_bytes()) && boundary {
            return Some((kind, end));
        }
    }
    None
}

/// True when `class` is really a `::class` constant fetch.
fn is_double_colon_before(data: &[u8], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 && data[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    if cursor < 2 || data[cursor - 1] != b':' {
        return false;
    }
    let mut before = cursor - 1;
    while before > 0 && data[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    before > 0 && data[before - 1] == b':'
}

/// Parse the declaration led by the keyword at `start` (`end` is just past
/// the keyword). Returns `None` for anonymous classes and closures.
fn parse_declaration(data: &[u8], start: usize, kind: DeclKind, end: usize) -> Option<PhpDecl> {
    let mut cursor = skip_trivia(data, end);
    if kind == DeclKind::Function && data.get(cursor) == Some(&b'&') {
        cursor = skip_trivia(data, cursor + 1);
    }
    let byte = data.get(cursor).copied()?;
    if byte == b'(' || byte == b'{' || byte == b';' {
        return None;
    }
    if !is_identifier_start(byte) {
        return None;
    }
    let name_start = cursor;
    while cursor < data.len() && is_identifier_char(data[cursor]) {
        cursor += 1;
    }
    let name = String::from_utf8_lossy(&data[name_start..cursor]).to_string();
    // `new class extends Foo` / `new class implements Bar`: anonymous.
    if kind == DeclKind::ClassOrInterface
        && (name.eq_ignore_ascii_case("extends") || name.eq_ignore_ascii_case("implements"))
    {
        return None;
    }
    Some(PhpDecl {
        kind,
        name,
        offset: start,
    })
}

/// Skip whitespace and comments (not close tags or strings).
fn skip_trivia(data: &[u8], mut index: usize) -> usize {
    loop {
        while index < data.len() && data[index].is_ascii_whitespace() {
            index += 1;
        }
        let next = data.get(index + 1).copied().unwrap_or(0);
        let line_comment =
            (data.get(index) == Some(&b'/') && next == b'/') || data.get(index) == Some(&b'#');
        if line_comment {
            index = skip_line_rest(data, index).0;
        } else if data.get(index) == Some(&b'/') && next == b'*' {
            index = skip_block_comment(data, index);
        } else {
            return index;
        }
    }
}

/// Skip to the newline; reports `?>` (which ends PHP mode in real PHP).
fn skip_line_rest(data: &[u8], index: usize) -> (usize, bool) {
    let mut cursor = index;
    if data.get(cursor) == Some(&b'#') {
        cursor += 1;
    } else {
        cursor += 2;
    }
    while cursor < data.len() && data[cursor] != b'\n' {
        if data[cursor] == b'?' && data.get(cursor + 1) == Some(&b'>') {
            return (cursor + 2, true);
        }
        cursor += 1;
    }
    (cursor, false)
}

fn skip_block_comment(data: &[u8], index: usize) -> usize {
    let mut cursor = index + 2;
    while cursor < data.len() {
        if data[cursor] == b'*' && data.get(cursor + 1) == Some(&b'/') {
            return cursor + 2;
        }
        cursor += 1;
    }
    cursor
}

fn skip_quoted(data: &[u8], index: usize) -> usize {
    let quote = data[index];
    let mut cursor = index + 1;
    while cursor < data.len() {
        if data[cursor] == b'\\' {
            cursor += 2;
            continue;
        }
        if data[cursor] == quote {
            return cursor + 1;
        }
        cursor += 1;
    }
    cursor
}

/// Skip a heredoc/nowdoc starting at `index`, if one starts there.
fn skip_heredoc(data: &[u8], index: usize) -> Option<usize> {
    let (label, body) = parse_heredoc_opener(data, index)?;
    Some(find_heredoc_end(data, label, body))
}

/// Parse a heredoc/nowdoc opener at `index`: the label plus the offset of
/// the first body line. `None` when no opener starts there.
fn parse_heredoc_opener(data: &[u8], index: usize) -> Option<(&[u8], usize)> {
    if !is_heredoc_opener(data, index) {
        return None;
    }
    let mut cursor = skip_opener_gap(data, index + 3);
    let quote = data.get(cursor).copied().unwrap_or(0);
    let quoted = quote == b'\'' || quote == b'"';
    if quoted {
        cursor += 1;
    }
    let (label, after) = scan_heredoc_label(data, cursor)?;
    cursor = after;
    if quoted {
        if data.get(cursor) != Some(&quote) {
            return None;
        }
        cursor += 1;
    }
    let body = opener_body_start(data, cursor)?;
    Some((label, body))
}

/// True when `<<<` (but not `<<<=` or a `<<` shift) starts at `index`.
fn is_heredoc_opener(data: &[u8], index: usize) -> bool {
    if data.get(index) != Some(&b'<')
        || data.get(index + 1) != Some(&b'<')
        || data.get(index + 2) != Some(&b'<')
    {
        return false;
    }
    data.get(index + 3) != Some(&b'<') && data.get(index + 3) != Some(&b'=')
}

/// Offset of the first body line: the label must end the opener line
/// (modulo whitespace), else `None`.
fn opener_body_start(data: &[u8], cursor: usize) -> Option<usize> {
    let mut end = cursor;
    while end < data.len() && data[end] != b'\n' {
        if data[end] != b' ' && data[end] != b'\t' && data[end] != b'\r' {
            return None;
        }
        end += 1;
    }
    if end >= data.len() {
        Some(end)
    } else {
        Some(end + 1)
    }
}

/// Skip spaces/tabs between `<<<` and the heredoc label.
fn skip_opener_gap(data: &[u8], mut cursor: usize) -> usize {
    while cursor < data.len() && (data[cursor] == b' ' || data[cursor] == b'\t') {
        cursor += 1;
    }
    cursor
}

/// Scan a heredoc label at `cursor`: the label plus the offset past it.
/// The label must be a valid identifier start.
fn scan_heredoc_label(data: &[u8], cursor: usize) -> Option<(&[u8], usize)> {
    let mut end = cursor;
    while end < data.len() && (data[end].is_ascii_alphanumeric() || data[end] == b'_') {
        end += 1;
    }
    if end == cursor || !is_identifier_start(data[cursor]) {
        return None;
    }
    Some((&data[cursor..end], end))
}

/// Consume heredoc body lines from `body` until the terminator line for
/// `label`; unterminated bodies run to the end of input.
fn find_heredoc_end(data: &[u8], label: &[u8], mut cursor: usize) -> usize {
    while cursor < data.len() {
        if let Some(probe) = heredoc_terminator_probe(data, label, cursor) {
            return end_of_line(data, probe);
        }
        cursor = end_of_line(data, cursor);
    }
    cursor
}

/// Offset of the label when the line at `cursor` terminates the heredoc:
/// optional indent, the label, then a non-identifier byte.
fn heredoc_terminator_probe(data: &[u8], label: &[u8], cursor: usize) -> Option<usize> {
    let mut probe = cursor;
    while probe < data.len() && (data[probe] == b' ' || data[probe] == b'\t') {
        probe += 1;
    }
    if !data[probe..].starts_with(label) {
        return None;
    }
    let next = data.get(probe + label.len()).copied().unwrap_or(b';');
    if next != b'_' && !next.is_ascii_alphanumeric() {
        Some(probe)
    } else {
        None
    }
}

fn end_of_line(data: &[u8], index: usize) -> usize {
    let mut cursor = index;
    while cursor < data.len() && data[cursor] != b'\n' {
        cursor += 1;
    }
    if cursor < data.len() {
        cursor += 1;
    }
    cursor
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
#[path = "lint_phutil_library_tests.rs"]
mod lint_phutil_library_tests;
