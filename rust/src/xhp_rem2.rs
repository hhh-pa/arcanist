//! XHP remainder rules, set 2: switches, declarations, methods (T1128, dv-c).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`).
//!
//! Assigned rules (ID: PHP class):
//! - 50: ArcanistDuplicateSwitchCaseXHPASTLinterRule
//! - 52: ArcanistImplicitVisibilityXHPASTLinterRule
//! - 55: ArcanistUnnecessaryFinalModifierXHPASTLinterRule
//! - 57: ArcanistSelfMemberReferenceXHPASTLinterRule
//! - 59: ArcanistInnerFunctionXHPASTLinterRule
//! - 60: ArcanistDefaultParametersXHPASTLinterRule
//! - 63: ArcanistUselessOverridingMethodXHPASTLinterRule
//! - 64: ArcanistNoParentScopeXHPASTLinterRule
//! - 67: ArcanistToStringExceptionXHPASTLinterRule
//! - 69: ArcanistInstanceOfOperatorXHPASTLinterRule
//! - 70: ArcanistInvalidDefaultParameterXHPASTLinterRule
//! - 72: ArcanistInvalidModifiersXHPASTLinterRule
//! - 76: ArcanistArrayValueXHPASTLinterRule
//! - 77: ArcanistListAssignmentXHPASTLinterRule
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (token scans,
//! `Tree::gap_text`, `Tree::calls`, `Tree::blocks`). Emit
//! [`super::xhp_ast::RawFinding`] with the PHP rule ID and the EXACT
//! PHP message text (`pht()` templates with `%s`/`%d` substituted
//! exactly as the PHP does, including quirks). Do not invent
//! messages. If a (sub-)check cannot be ported meaningfully, leave
//! it out and document the gap in the module docs + final report;
//! never silently diverge.
//!
//! Constraints: no new dependencies; every function cy<=15 / co<=20
//! (named private fns, same file; closures count into parents);
//! `cargo clippy --offline --all-targets -- -D warnings` clean;
//! `cargo fmt --check` clean.
//!
//! Port notes:
//! - XHP50 compares case expressions by concatenated semantic-token
//!   text (mirrors `getSemanticString()`); only the last duplicate
//!   arm is flagged (PHP's `array_pop` choice). PHP's secondary
//!   `setOtherLocations()` markers are not emitted: this port never
//!   attaches secondary locations.
//! - XHP52/72 read property declarations from class-body top-level
//!   statements (modifiers plus `$var`); class constants carry no
//!   modifier list in the PHP AST and are skipped. Property hooks
//!   (PHP 8.4) can merge adjacent statements; accepted.
//! - XHP57 treats an unset `xhpast.php-version` as modern PHP
//!   (always flag); PHP falls back to the runtime `phpversion()`,
//!   which is always >= 5.4 today. Arrow functions count as
//!   closures. Only `class` declarations are scanned, like PHP.
//! - XHP59 only counts non-method function declarations as parents:
//!   a named function inside a method is not an inner function in
//!   PHP (`n_METHOD_DECLARATION` is not `n_FUNCTION_DECLARATION`).
//! - XHP60 emits one finding per parameter list (PHP raises once per
//!   offending parameter at the same list node). Variadic parameters
//!   are exempt: they postdate the PHP rule and are always legal.
//! - XHP63 keeps the PHP quirk that fewer call arguments than
//!   declared parameters still flags (leftover expectations are
//!   ignored); `parent` and the method name match case-sensitively.
//! - XHP64 scans the whole method span (parameter defaults
//!   included), matching PHP's whole-method descendant search, and
//!   matches `parent` case-sensitively. Only `class` declarations
//!   are scanned, like PHP.
//! - XHP69 classifies the LHS by single-token shape plus a
//!   precedence-aware boundary scan: `instanceof` binds tighter
//!   than every binary operator but looser than `~`, `@`, `**`,
//!   casts, `new`, and postfix access, so only the latter (plus
//!   operand tokens) suppress the finding. Unary `-`/`+` before a
//!   scalar LHS is treated as a boundary (flagged).
//! - XHP70 predates scalar, nullable, and union hints. Leading `?`,
//!   union/intersection separators, and constructor-promotion
//!   modifiers are stripped before dispatch; scalar/builtin members
//!   (`int`, `string`, `mixed`, ...) exempt the parameter;
//!   `iterable` dispatches with `array` (same `{array, null}`
//!   domain, same message); first-class-callable defaults (`...`)
//!   are accepted for `callable`.
//! - XHP72 does not port the `Multiple "static" modifiers` check:
//!   it is dead in PHP (`$is_static` is never set). Modifier names
//!   match case-sensitively, like PHP.
//! - XHP76 covers `array(...)` and `[...]` literals; `[...] =` list
//!   targets and `foreach (... as [...])` destructuring are not
//!   array literals and are excluded.
//! - XHP77 covers `list(...)` plus short-list `[...] =` targets
//!   (postdate the PHP rule; same trigger).
//! - No autofix replacements are produced.

use std::cmp::Ordering;

use super::xhp_ast::{
    self, Block, BlockKind, BodySpan, CaseBlock, Param, RawFinding, RuleCtx, Token, Tree,
    is_semantic, kinds,
};

/// Run all remainder-2 rules, pushing findings into `out`.
pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    rule_duplicate_case(tree, &blocks, out);
    rule_implicit_visibility(tree, &blocks, out);
    rule_unnecessary_final(tree, &blocks, out);
    rule_self_reference(ctx, &blocks, out);
    rule_inner_function(tree, &blocks, out);
    rule_default_parameters(tree, &blocks, out);
    rule_useless_override(tree, &blocks, out);
    rule_no_parent(tree, &blocks, out);
    rule_tostring_throw(tree, &blocks, out);
    rule_instanceof(tree, out);
    rule_invalid_default(tree, &blocks, out);
    rule_invalid_modifiers(tree, &blocks, out);
    rule_array_value(tree, out);
    rule_list_assignment(tree, out);
}

fn push(out: &mut Vec<RawFinding>, rule: u32, offset: usize, message: &str) {
    out.push(RawFinding::at(rule, offset, message.to_string()));
}

fn is_class_like(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
    )
}

fn is_modifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        kinds::T_PUBLIC
            | kinds::T_PRIVATE
            | kinds::T_PROTECTED
            | kinds::T_STATIC
            | kinds::T_ABSTRACT
            | kinds::T_FINAL
            | kinds::T_VAR
            | kinds::T_READONLY
    )
}

/// Modifier token indices directly before `kw` (trivia and attribute
/// spans skipped, mirroring `Tree::modifiers_before`).
fn modifier_tokens(tree: &Tree, kw: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = kw;
    while let Some(prev) = i.checked_sub(1) {
        let kind = tree.tokens[prev].kind.as_str();
        if is_modifier_kind(kind) {
            out.push(prev);
            i = prev;
        } else if kind == "]" {
            let Some(attr) = attribute_open_before(tree, prev) else {
                break;
            };
            i = attr;
        } else if !is_semantic(&tree.tokens[prev]) {
            i = prev;
        } else {
            break;
        }
    }
    out.reverse();
    out
}

/// Index of the `#[` opening the attribute closed at `close`, if the
/// closer really ends an attribute span.
fn attribute_open_before(tree: &Tree, close: usize) -> Option<usize> {
    let mut depth: u32 = 0;
    let mut j = close;
    while j > 0 {
        j -= 1;
        let kind = tree.tokens[j].kind.as_str();
        if kind == "]" || kind == ")" || kind == "}" {
            depth += 1;
        } else if kind == "[" || kind == "(" || kind == "{" {
            if depth == 0 {
                return None;
            }
            depth -= 1;
        } else if depth == 0 && kind == kinds::T_ATTRIBUTE {
            return Some(j);
        } else if depth == 0 && attribute_scan_stops(kind) {
            return None;
        }
    }
    None
}

fn attribute_scan_stops(kind: &str) -> bool {
    matches!(
        kind,
        ";" | "{"
            | "}"
            | kinds::T_OPEN_TAG
            | kinds::T_OPEN_TAG_WITH_ECHO
            | kinds::T_CLOSE_TAG
            | kinds::T_INLINE_HTML
            | kinds::T_FUNCTION
            | kinds::T_CLASS
            | kinds::T_INTERFACE
            | kinds::T_TRAIT
            | kinds::T_ENUM
    )
}

/// Start offset of a declaration: its first modifier, else its keyword.
fn decl_offset(tree: &Tree, kw: usize) -> usize {
    modifier_tokens(tree, kw)
        .first()
        .map_or(tree.tokens[kw].offset, |m| tree.tokens[*m].offset)
}

/// Inclusive token range covering a block from its keyword.
fn block_span(tree: &Tree, block: &Block) -> (usize, usize) {
    let end = tree
        .body_last(block)
        .unwrap_or_else(|| tree.tokens.len().saturating_sub(1));
    (block.kw_idx, end.max(block.kw_idx))
}

/// Token range strictly inside a brace body: `(open + 1, close)`.
fn brace_inner(tree: &Tree, block: &Block) -> Option<(usize, usize)> {
    if let BodySpan::Brace { open, close } = block.body {
        let end = close.unwrap_or(tree.tokens.len()).min(tree.tokens.len());
        if open < end {
            return Some((open + 1, end));
        }
    }
    None
}

/// True when block `idx` descends (transitively) from `ancestor`.
fn descends_from(blocks: &[Block], mut cur: usize, ancestor: usize) -> bool {
    while let Some(parent) = blocks[cur].parent {
        if parent == ancestor {
            return true;
        }
        cur = parent;
    }
    false
}

/// True when block `idx` has an ancestor satisfying `pred`.
fn has_ancestor(blocks: &[Block], idx: usize, pred: impl Fn(usize) -> bool) -> bool {
    let mut cur = blocks[idx].parent;
    while let Some(parent) = cur {
        if pred(parent) {
            return true;
        }
        cur = blocks[parent].parent;
    }
    false
}

/// XHP50: duplicate `case` expressions within one `switch`.
fn rule_duplicate_case(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if block.kind != BlockKind::Switch {
            continue;
        }
        duplicate_case_in(tree, block, out);
    }
}

fn duplicate_case_in(tree: &Tree, block: &Block, out: &mut Vec<RawFinding>) {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for case in block.cases.iter().filter(|c| !c.is_default) {
        let key = case_semantic(tree, block, case);
        match groups.iter_mut().find(|(seen, _)| *seen == key) {
            Some((_, kws)) => kws.push(case.kw_idx),
            None => groups.push((key, vec![case.kw_idx])),
        }
    }
    for (_, kws) in groups {
        if kws.len() > 1
            && let Some(last) = kws.last()
        {
            push(
                out,
                50,
                tree.tokens[*last].offset,
                "Duplicate case in switch statement. PHP will ignore all \
                 but the first case.",
            );
        }
    }
}

/// Concatenated semantic-token text of a `case` expression.
fn case_semantic(tree: &Tree, block: &Block, case: &CaseBlock) -> String {
    let end = case
        .colon
        .unwrap_or_else(|| case_boundary(tree, block, case.kw_idx));
    let mut text = String::new();
    let mut i = case.kw_idx + 1;
    while i < end && i < tree.tokens.len() {
        if is_semantic(&tree.tokens[i]) {
            text.push_str(&tree.tokens[i].value);
        }
        i += 1;
    }
    text
}

/// End bound of a colon-less `case` head: the next arm or body end.
fn case_boundary(tree: &Tree, block: &Block, kw: usize) -> usize {
    block
        .cases
        .iter()
        .map(|c| c.kw_idx)
        .filter(|k| *k > kw)
        .min()
        .unwrap_or_else(|| {
            tree.body_last(block)
                .unwrap_or(tree.tokens.len())
                .min(tree.tokens.len())
        })
}

/// XHP52: methods and properties without an explicit visibility.
fn rule_implicit_visibility(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind == BlockKind::Function && tree.is_method(blocks, idx) {
            method_visibility(tree, block, out);
        }
        if is_class_like(block.kind) {
            for prop in class_properties(tree, block) {
                property_visibility(tree, &prop, out);
            }
        }
    }
}

fn method_visibility(tree: &Tree, block: &Block, out: &mut Vec<RawFinding>) {
    let mods = modifier_tokens(tree, block.kw_idx);
    for m in &mods {
        if is_visibility_value(&tree.tokens[*m].value) {
            return;
        }
    }
    let offset = mods
        .first()
        .map_or(tree.tokens[block.kw_idx].offset, |f| tree.tokens[*f].offset);
    push(
        out,
        52,
        offset,
        "Methods should have their visibility declared explicitly.",
    );
}

fn is_visibility_value(value: &str) -> bool {
    matches!(value, "public" | "protected" | "private")
}

fn property_visibility(tree: &Tree, prop: &PropertyDecl, out: &mut Vec<RawFinding>) {
    for m in &prop.modifiers {
        let value = tree.tokens[*m].value.as_str();
        if value == "var" {
            push(
                out,
                52,
                tree.tokens[*m].offset,
                "Use `public` instead of `var` to indicate public visibility.",
            );
            return;
        }
        if is_visibility_value(value) {
            return;
        }
    }
    let offset = prop
        .modifiers
        .first()
        .map_or(tree.tokens[prop.var_idx].offset, |f| tree.tokens[*f].offset);
    push(
        out,
        52,
        offset,
        "Properties should have their visibility declared explicitly.",
    );
}

/// A property declaration: leading modifiers plus the `$var` token.
struct PropertyDecl {
    modifiers: Vec<usize>,
    var_idx: usize,
}

/// Top-level `modifiers $var ...;` statements in a class-like body.
fn class_properties(tree: &Tree, block: &Block) -> Vec<PropertyDecl> {
    let Some((mut i, end)) = brace_inner(tree, block) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth: u32 = 0;
    let mut stmt_start = i;
    while i < end {
        match tree.tokens[i].kind.as_str() {
            "(" | "{" | "[" => depth += 1,
            ")" | "}" | "]" => depth = depth.saturating_sub(1),
            ";" if depth == 0 => {
                if let Some(prop) = property_in_statement(tree, stmt_start, i) {
                    out.push(prop);
                }
                stmt_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// A property declaration inside one `;`-terminated statement, if the
/// statement is a modifier run (attributes skipped) plus `$var`.
fn property_in_statement(tree: &Tree, start: usize, end: usize) -> Option<PropertyDecl> {
    let mut mods = Vec::new();
    let mut i = start;
    while i < end {
        if !is_semantic(&tree.tokens[i]) {
            i += 1;
            continue;
        }
        let kind = tree.tokens[i].kind.as_str();
        if kind == kinds::T_ATTRIBUTE {
            i = attribute_close(tree, i, end)? + 1;
            continue;
        }
        if !is_modifier_kind(kind) {
            break;
        }
        mods.push(i);
        i += 1;
    }
    while i < end && !is_semantic(&tree.tokens[i]) {
        i += 1;
    }
    if i < end && tree.tokens[i].kind == kinds::T_VARIABLE {
        Some(PropertyDecl {
            modifiers: mods,
            var_idx: i,
        })
    } else {
        None
    }
}

/// Index of the `]` closing the attribute opened at `attr`.
fn attribute_close(tree: &Tree, attr: usize, end: usize) -> Option<usize> {
    let mut depth: u32 = 0;
    let mut i = attr + 1;
    while i < end {
        match tree.tokens[i].kind.as_str() {
            "[" | "(" | "{" => depth += 1,
            "]" if depth == 0 => return Some(i),
            "]" | ")" | "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    None
}

/// XHP55: `final` methods inside `final` classes.
fn rule_unnecessary_final(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Class || !class_is_final(tree, block) {
            continue;
        }
        unnecessary_final_in(tree, blocks, idx, out);
    }
}

fn unnecessary_final_in(tree: &Tree, blocks: &[Block], class: usize, out: &mut Vec<RawFinding>) {
    for (midx, method) in blocks.iter().enumerate() {
        if method.kind != BlockKind::Function || !tree.is_method(blocks, midx) {
            continue;
        }
        if !descends_from(blocks, midx, class) {
            continue;
        }
        for m in modifier_tokens(tree, method.kw_idx) {
            if tree.tokens[m].value == "final" {
                push(
                    out,
                    55,
                    tree.tokens[m].offset,
                    "Unnecessary `final` modifier in `final` class.",
                );
            }
        }
    }
}

fn class_is_final(tree: &Tree, block: &Block) -> bool {
    modifier_tokens(tree, block.kw_idx)
        .iter()
        .any(|m| tree.tokens[*m].value == "final")
}

/// XHP57: `ClassName::` references inside `ClassName` itself.
fn rule_self_reference(ctx: &RuleCtx, blocks: &[Block], out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let modern = version_is_modern(ctx.options.php_version.as_deref());
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Class {
            continue;
        }
        let Some(class_name) = block.name.as_deref() else {
            continue;
        };
        let (start, end) = block_span(tree, block);
        let closures = closure_spans(tree, blocks, idx);
        self_reference_in(tree, class_name, start, end, &closures, modern, out);
    }
}

/// True for PHP >= 5.4 (or an unset version, which PHP resolves to the
/// always-modern runtime version).
fn version_is_modern(version: Option<&str>) -> bool {
    let Some(target) = version else {
        return true;
    };
    xhp_ast::version_compare(target, "5.4.0") != Ordering::Less
}

/// Token spans of closures (anonymous functions and arrow functions)
/// declared inside class block `class`.
fn closure_spans(tree: &Tree, blocks: &[Block], class: usize) -> Vec<(usize, usize)> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, block)| *i != class && is_closure(block) && descends_from(blocks, *i, class))
        .map(|(_, block)| block_span(tree, block))
        .collect()
}

fn is_closure(block: &Block) -> bool {
    block.kind == BlockKind::ArrowFn || (block.kind == BlockKind::Function && block.is_anonymous)
}

fn self_reference_in(
    tree: &Tree,
    class_name: &str,
    start: usize,
    end: usize,
    closures: &[(usize, usize)],
    modern: bool,
    out: &mut Vec<RawFinding>,
) {
    let mut i = start;
    while i <= end && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_PAAMAYIM_NEKUDOTAYIM
            && let Some(ref_at) = self_class_ref(tree, class_name, i)
            && (modern || !in_any_span(closures, ref_at))
        {
            push(
                out,
                57,
                tree.tokens[ref_at].offset,
                "Use `self::` for local static member references.",
            );
        }
        i += 1;
    }
}

fn in_any_span(spans: &[(usize, usize)], idx: usize) -> bool {
    spans.iter().any(|(s, e)| *s <= idx && idx <= *e)
}

/// Start index of the `::` class-name operand when it names
/// `class_name` (case-insensitively, like PHP).
fn self_class_ref(tree: &Tree, class_name: &str, paamayim: usize) -> Option<usize> {
    let mut start = tree.prev_semantic(paamayim)?;
    if tree.tokens[start].kind != kinds::T_STRING {
        return None;
    }
    start = qualified_name_start(tree, start);
    if tree
        .prev_semantic(start)
        .is_some_and(|p| tree.tokens[p].kind == "\\")
    {
        start = tree.prev_semantic(start).unwrap_or(start);
    }
    let mut text = String::new();
    let mut i = start;
    while i < paamayim {
        if is_semantic(&tree.tokens[i]) {
            text.push_str(&tree.tokens[i].value);
        }
        i += 1;
    }
    if text.eq_ignore_ascii_case(class_name) {
        Some(start)
    } else {
        None
    }
}

/// Start of the `Name\Name...` run ending at `end`.
fn qualified_name_start(tree: &Tree, mut start: usize) -> usize {
    while let Some(prev) = tree.prev_semantic(start) {
        if tree.tokens[prev].kind != "\\" {
            break;
        }
        let Some(name) = tree.prev_semantic(prev) else {
            break;
        };
        if tree.tokens[name].kind != kinds::T_STRING {
            break;
        }
        start = name;
    }
    start
}

/// XHP59: named functions declared inside a (non-method) function.
fn rule_inner_function(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function || block.name.is_none() || block.is_anonymous {
            continue;
        }
        let nested = has_ancestor(blocks, idx, |a| {
            blocks[a].kind == BlockKind::Function && !tree.is_method(blocks, a)
        });
        if nested {
            push(
                out,
                59,
                tree.tokens[block.kw_idx].offset,
                "Avoid the use of inner functions.",
            );
        }
    }
}

/// XHP60: parameters with defaults must end the parameter list.
fn rule_default_parameters(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if !matches!(block.kind, BlockKind::Function | BlockKind::ArrowFn) {
            continue;
        }
        let Some(lparen) = block.lparen else {
            continue;
        };
        if params_default_misordered(&block.params) {
            push(
                out,
                60,
                tree.tokens[lparen].offset,
                "Arguments with default values must be at the end of the argument list.",
            );
        }
    }
}

fn params_default_misordered(params: &[Param]) -> bool {
    let mut default_found = false;
    for param in params {
        if param.default.is_some() {
            default_found = true;
        } else if default_found && !param.variadic {
            return true;
        }
    }
    false
}

/// XHP63: methods that only forward to `parent::same(...)`.
fn rule_useless_override(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function || !tree.is_method(blocks, idx) {
            continue;
        }
        if block.is_anonymous {
            continue;
        }
        let Some(name) = block.name.as_deref() else {
            continue;
        };
        if useless_override_body(tree, block, name) {
            push(
                out,
                63,
                decl_offset(tree, block.kw_idx),
                "Useless overriding method.",
            );
        }
    }
}

fn useless_override_body(tree: &Tree, block: &Block, name: &str) -> bool {
    let mut expected: Vec<&str> = Vec::new();
    for param in &block.params {
        if param.default.is_some() {
            return false;
        }
        expected.push(param.name.as_str());
    }
    let Some((first, last)) = single_statement(tree, block) else {
        return false;
    };
    useless_override_call(tree, first, last, name, &expected)
}

/// Inclusive token range of a brace body's single statement, if the
/// body holds exactly one (empty `;` segments count, like PHP).
fn single_statement(tree: &Tree, block: &Block) -> Option<(usize, usize)> {
    let (start, end) = brace_inner(tree, block)?;
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut depth: u32 = 0;
    let mut seg_start = start;
    let mut i = start;
    while i < end {
        match tree.tokens[i].kind.as_str() {
            "(" | "{" | "[" => depth += 1,
            ")" | "}" | "]" => depth = depth.saturating_sub(1),
            ";" if depth == 0 => {
                chunks.push((seg_start, i));
                seg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    chunks.push((seg_start, end));
    if chunks.len() > 1 && chunks.last().is_some_and(|c| chunk_is_empty(tree, *c)) {
        chunks.pop();
    }
    if chunks.len() != 1 {
        return None;
    }
    trim_chunk(tree, chunks[0])
}

fn chunk_is_empty(tree: &Tree, chunk: (usize, usize)) -> bool {
    let (start, end) = chunk;
    (start..end).all(|i| i >= tree.tokens.len() || !is_semantic(&tree.tokens[i]))
}

/// Trim trivia from an exclusive-end chunk into inclusive bounds.
fn trim_chunk(tree: &Tree, chunk: (usize, usize)) -> Option<(usize, usize)> {
    let (mut start, end) = chunk;
    let mut stop = end.min(tree.tokens.len());
    while start < stop && !is_semantic(&tree.tokens[start]) {
        start += 1;
    }
    while stop > start && !is_semantic(&tree.tokens[stop - 1]) {
        stop -= 1;
    }
    if start < stop {
        Some((start, stop - 1))
    } else {
        None
    }
}

/// True for `[return] parent::name(args)` covering `first..=last`.
fn useless_override_call(
    tree: &Tree,
    first: usize,
    last: usize,
    name: &str,
    expected: &[&str],
) -> bool {
    let mut cur = first;
    if tree.tokens[cur].kind == kinds::T_RETURN {
        let Some(next) = tree.next_semantic(cur).filter(|n| *n <= last) else {
            return false;
        };
        cur = next;
    }
    if tree.tokens[cur].kind != kinds::T_STRING || tree.tokens[cur].value != "parent" {
        return false;
    }
    let Some(paamayim) = tree.next_semantic(cur).filter(|n| *n <= last) else {
        return false;
    };
    if tree.tokens[paamayim].kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
        return false;
    }
    let Some(method) = tree.next_semantic(paamayim).filter(|n| *n <= last) else {
        return false;
    };
    if tree.tokens[method].kind != kinds::T_STRING || tree.tokens[method].value != name {
        return false;
    }
    let Some(lparen) = tree.next_semantic(method).filter(|n| *n <= last) else {
        return false;
    };
    if tree.tokens[lparen].kind != "(" {
        return false;
    }
    let Some(rparen) = tree.match_close(lparen) else {
        return false;
    };
    if rparen != last {
        return false;
    }
    override_args_match(tree, lparen, rparen, expected)
}

/// True when every call argument is the next expected `$variable` in
/// order (trailing declared parameters may be unpassed, like PHP).
fn override_args_match(tree: &Tree, lparen: usize, rparen: usize, expected: &[&str]) -> bool {
    let args = tree.split_top_level(lparen, rparen);
    if args.len() > expected.len() {
        return false;
    }
    for (n, (first, last)) in args.iter().enumerate() {
        if first != last {
            return false;
        }
        let token = &tree.tokens[*first];
        if token.kind != kinds::T_VARIABLE || token.value != expected[n] {
            return false;
        }
    }
    true
}

/// XHP64: `parent::` inside a class without a parent.
fn rule_no_parent(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Class || block.extends.is_some() {
            continue;
        }
        for (midx, method) in blocks.iter().enumerate() {
            if method.kind != BlockKind::Function || !tree.is_method(blocks, midx) {
                continue;
            }
            if !descends_from(blocks, midx, idx) {
                continue;
            }
            let (start, end) = block_span(tree, method);
            parent_access_in(tree, start, end, out);
        }
    }
}

fn parent_access_in(tree: &Tree, start: usize, end: usize, out: &mut Vec<RawFinding>) {
    let mut i = start;
    while i <= end && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_STRING
            && tree.tokens[i].value == "parent"
            && tree
                .next_semantic(i)
                .is_some_and(|n| n <= end && tree.tokens[n].kind == kinds::T_PAAMAYIM_NEKUDOTAYIM)
        {
            push(
                out,
                64,
                tree.tokens[i].offset,
                "Cannot access `parent::` when current class scope has no parent.",
            );
        }
        i += 1;
    }
}

/// XHP67: `throw` inside `__toString`.
fn rule_tostring_throw(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function || !tree.is_method(blocks, idx) {
            continue;
        }
        if block.name.as_deref() != Some("__toString") {
            continue;
        }
        let Some((start, end)) = brace_inner(tree, block) else {
            continue;
        };
        let mut i = start;
        while i < end && i < tree.tokens.len() {
            if tree.tokens[i].kind == kinds::T_THROW {
                push(
                    out,
                    67,
                    tree.tokens[i].offset,
                    "It is not possible to throw an `Exception` from within the `__toString` method.",
                );
            }
            i += 1;
        }
    }
}

/// XHP69: static scalar or constant as the `instanceof` left operand.
fn rule_instanceof(tree: &Tree, out: &mut Vec<RawFinding>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_INSTANCEOF {
            continue;
        }
        if tree.next_semantic(i).is_none() {
            continue;
        }
        if let Some(start) = instanceof_const_lhs(tree, i) {
            push(
                out,
                69,
                tree.tokens[start].offset,
                "`instanceof` expects an object instance, constant given.",
            );
        }
    }
}

/// Start of the LHS operand when it is a single static scalar or bare
/// constant name (mirrors `isStaticScalar()` / `n_SYMBOL_NAME`).
fn instanceof_const_lhs(tree: &Tree, inst: usize) -> Option<usize> {
    let end = tree.prev_semantic(inst)?;
    if !is_scalar_or_name(&tree.tokens[end]) {
        return None;
    }
    let start = instanceof_name_start(tree, end);
    match tree.prev_semantic(start) {
        None => Some(start),
        Some(before) if instanceof_lhs_boundary(&tree.tokens[before].kind) => Some(start),
        _ => None,
    }
}

/// True for literal, magic-constant, and bare-name tokens.
fn is_scalar_or_name(token: &Token) -> bool {
    matches!(
        token.kind.as_str(),
        kinds::T_LNUMBER
            | kinds::T_DNUMBER
            | kinds::T_CONSTANT_ENCAPSED_STRING
            | kinds::T_CLASS_C
            | kinds::T_FUNC_C
            | kinds::T_METHOD_C
            | kinds::T_FILE
            | kinds::T_DIR
            | kinds::T_LINE
            | kinds::T_NS_C
            | kinds::T_TRAIT_C
            | kinds::T_STRING
    )
}

/// Start of a (possibly qualified) constant name ending at `end`.
fn instanceof_name_start(tree: &Tree, end: usize) -> usize {
    if tree.tokens[end].kind != kinds::T_STRING {
        return end;
    }
    let mut start = qualified_name_start(tree, end);
    if let Some(prev) = tree.prev_semantic(start)
        && tree.tokens[prev].kind == "\\"
    {
        start = prev;
    }
    start
}

/// True when the token before an `instanceof` LHS scalar starts a new
/// operand (rather than continuing a larger expression or access).
/// `instanceof` binds tighter than every binary operator but looser
/// than `~`, `@`, `**`, casts, `new`, and postfix access.
fn instanceof_lhs_boundary(kind: &str) -> bool {
    !matches!(
        kind,
        ")" | "]"
            | "}"
            | kinds::T_OBJECT_OPERATOR
            | kinds::T_NULLSAFE_OBJECT_OPERATOR
            | kinds::T_PAAMAYIM_NEKUDOTAYIM
            | kinds::T_NEW
            | kinds::T_INC
            | kinds::T_DEC
            | "~"
            | "@"
            | kinds::T_POW
            | kinds::T_VARIABLE
            | kinds::T_STRING
            | kinds::T_STATIC
            | "\\"
            | "$"
            | kinds::T_LNUMBER
            | kinds::T_DNUMBER
            | kinds::T_CONSTANT_ENCAPSED_STRING
            | kinds::T_INTERPOLATED_STRING
            | kinds::T_HEREDOC
            | kinds::T_NOWDOC
            | kinds::T_BACKTICK
            | kinds::T_CLASS_C
            | kinds::T_FUNC_C
            | kinds::T_METHOD_C
            | kinds::T_FILE
            | kinds::T_DIR
            | kinds::T_LINE
            | kinds::T_NS_C
            | kinds::T_TRAIT_C
    )
}

/// XHP70: defaults for hinted parameters must be `array`/`null`-shaped.
fn rule_invalid_default(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if !matches!(block.kind, BlockKind::Function | BlockKind::ArrowFn) {
            continue;
        }
        for param in &block.params {
            invalid_default_param(tree, param, out);
        }
    }
}

fn invalid_default_param(tree: &Tree, param: &Param, out: &mut Vec<RawFinding>) {
    let (Some(ty), Some(default)) = (param.ty, param.default) else {
        return;
    };
    let members = hint_members(tree, ty);
    if members.iter().any(|m| is_scalar_hint(m)) {
        return;
    }
    if default_is_null(tree, default) {
        return;
    }
    let has_array = members.iter().any(|m| m == "array" || m == "iterable");
    let has_callable = members.iter().any(|m| m == "callable");
    if has_array && default_is_array(tree, default) {
        return;
    }
    if has_callable && default_has_ellipsis(tree, default) {
        return;
    }
    let message = invalid_default_message(&members, has_array, has_callable);
    push(out, 70, tree.tokens[default.0].offset, message);
}

fn invalid_default_message(
    members: &[String],
    has_array: bool,
    has_callable: bool,
) -> &'static str {
    let only_array = has_array && members.iter().all(|m| m == "array" || m == "iterable");
    let only_callable = has_callable && members.iter().all(|m| m == "callable");
    if only_array {
        "Default value for parameters with `array` type hint can only be an `array` or `null`."
    } else if only_callable {
        "Default value for parameters with `callable` type hint can only be `null`."
    } else {
        "Default value for parameters with a class type hint can only be `null`."
    }
}

/// Lowercased hint members: `?`, union/intersection separators, and
/// constructor-promotion modifiers stripped.
fn hint_members(tree: &Tree, range: (usize, usize)) -> Vec<String> {
    let mut members = Vec::new();
    let mut current = String::new();
    let mut i = range.0;
    while i <= range.1 && i < tree.tokens.len() {
        let token = &tree.tokens[i];
        if !is_semantic(token) {
            i += 1;
            continue;
        }
        match token.kind.as_str() {
            "|" | "&" | "(" | ")" => {
                if !current.is_empty() {
                    members.push(std::mem::take(&mut current));
                }
            }
            "?" => {}
            kinds::T_PUBLIC
            | kinds::T_PRIVATE
            | kinds::T_PROTECTED
            | kinds::T_READONLY
            | kinds::T_VAR
            | kinds::T_STATIC => {}
            _ => current.push_str(&token.value.to_ascii_lowercase()),
        }
        i += 1;
    }
    if !current.is_empty() {
        members.push(current);
    }
    members
}

/// Scalar/builtin hints (unknown to the PHP5-era rule): any default
/// is the compiler's business, not this lint's.
fn is_scalar_hint(member: &str) -> bool {
    matches!(
        member,
        "int"
            | "integer"
            | "float"
            | "double"
            | "string"
            | "bool"
            | "boolean"
            | "mixed"
            | "object"
            | "void"
            | "never"
            | "null"
            | "true"
            | "false"
    )
}

/// True for a lone `null` (any case) default.
fn default_is_null(tree: &Tree, range: (usize, usize)) -> bool {
    let mut semantics = Vec::new();
    let mut i = range.0;
    while i <= range.1 && i < tree.tokens.len() {
        if is_semantic(&tree.tokens[i]) {
            semantics.push(i);
        }
        i += 1;
    }
    semantics.len() == 1
        && tree.tokens[semantics[0]].kind == kinds::T_STRING
        && tree.tokens[semantics[0]].value.eq_ignore_ascii_case("null")
}

/// True for an `array(...)` / `[...]` default covering the range.
fn default_is_array(tree: &Tree, range: (usize, usize)) -> bool {
    let (Some(first), Some(last)) = (
        first_semantic_in(tree, range),
        last_semantic_in(tree, range),
    ) else {
        return false;
    };
    if tree.tokens[first].kind == "[" {
        return tree.match_close(first) == Some(last);
    }
    if tree.tokens[first].kind == kinds::T_ARRAY {
        return tree
            .next_semantic(first)
            .is_some_and(|lp| tree.tokens[lp].kind == "(" && tree.match_close(lp) == Some(last));
    }
    false
}

fn first_semantic_in(tree: &Tree, range: (usize, usize)) -> Option<usize> {
    (range.0..=range.1).find(|i| *i < tree.tokens.len() && is_semantic(&tree.tokens[*i]))
}

fn last_semantic_in(tree: &Tree, range: (usize, usize)) -> Option<usize> {
    (range.0..=range.1)
        .rev()
        .find(|i| *i < tree.tokens.len() && is_semantic(&tree.tokens[*i]))
}

/// True when the default uses first-class-callable syntax (`...`).
fn default_has_ellipsis(tree: &Tree, range: (usize, usize)) -> bool {
    (range.0..=range.1).any(|i| i < tree.tokens.len() && tree.tokens[i].kind == kinds::T_ELLIPSIS)
}

/// XHP72: contradictory or repeated member modifiers.
fn rule_invalid_modifiers(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind == BlockKind::Function && tree.is_method(blocks, idx) {
            check_modifier_list(tree, &modifier_tokens(tree, block.kw_idx), false, out);
        }
        if is_class_like(block.kind) {
            for prop in class_properties(tree, block) {
                check_modifier_list(tree, &prop.modifiers, true, out);
            }
        }
    }
}

#[derive(Debug, Default)]
struct ModifierState {
    is_abstract: bool,
    is_final: bool,
    has_visibility: bool,
    is_private: bool,
    final_idx: Option<usize>,
}

fn check_modifier_list(tree: &Tree, mods: &[usize], is_property: bool, out: &mut Vec<RawFinding>) {
    let mut state = ModifierState::default();
    for m in mods {
        check_one_modifier(tree, *m, is_property, &mut state, out);
    }
    check_modifier_combos(tree, &state, is_property, out);
}

fn check_one_modifier(
    tree: &Tree,
    m: usize,
    is_property: bool,
    state: &mut ModifierState,
    out: &mut Vec<RawFinding>,
) {
    let value = tree.tokens[m].value.as_str();
    let offset = tree.tokens[m].offset;
    match value {
        "abstract" => {
            if is_property {
                push(
                    out,
                    72,
                    offset,
                    "Properties cannot be declared \"abstract\".",
                );
            }
            if state.is_abstract {
                push(
                    out,
                    72,
                    offset,
                    "Multiple \"abstract\" modifiers are not allowed.",
                );
            }
            state.is_abstract = true;
        }
        "final" => {
            if is_property {
                push(out, 72, offset, "Properties can not be declared \"final\".");
            }
            if state.is_final {
                push(
                    out,
                    72,
                    offset,
                    "Multiple \"final\" modifiers are not allowed.",
                );
            }
            state.is_final = true;
            state.final_idx = Some(m);
        }
        "public" | "protected" | "private" => {
            if state.has_visibility {
                push(
                    out,
                    72,
                    offset,
                    "Multiple access type modifiers are not allowed.",
                );
            }
            state.has_visibility = true;
            state.is_private = value == "private";
        }
        _ => {}
    }
}

fn check_modifier_combos(
    tree: &Tree,
    state: &ModifierState,
    is_property: bool,
    out: &mut Vec<RawFinding>,
) {
    if is_property {
        return;
    }
    let Some(final_idx) = state.final_idx else {
        return;
    };
    let offset = tree.tokens[final_idx].offset;
    if state.is_abstract {
        push(
            out,
            72,
            offset,
            "Methods may not be both \"abstract\" and \"final\".",
        );
    }
    if state.is_private {
        push(
            out,
            72,
            offset,
            "Methods may not be both \"private\" and \"final\".",
        );
    }
}

/// XHP76: multiline array literals need one element per line.
fn rule_array_value(tree: &Tree, out: &mut Vec<RawFinding>) {
    for (open, close) in array_literals(tree) {
        if tree.line_of(tree.tokens[open].offset) == tree.line_of(tree.tokens[close].offset) {
            continue;
        }
        for (first, _) in tree.split_top_level(open, close) {
            let prev = tree.prev_semantic(first).unwrap_or(open);
            if !tree.gap_text(prev, first).contains('\n') {
                push(
                    out,
                    76,
                    tree.tokens[first].offset,
                    "Array elements should each occupy a single line.",
                );
            }
        }
    }
}

/// `(open, close)` of every `array(...)` / `[...]` literal.
fn array_literals(tree: &Tree) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind == kinds::T_ARRAY
            && let Some(lp) = tree.next_semantic(i)
            && tree.tokens[lp].kind == "("
            && let Some(rp) = tree.match_close(lp)
        {
            out.push((lp, rp));
        } else if token.kind == "["
            && is_array_open(tree, i)
            && let Some(close) = tree.match_close(i)
        {
            out.push((i, close));
        }
    }
    out
}

/// True for `[` opening a literal (not an index, a `[...] =` list
/// target, or a `foreach (... as [...])` shape).
fn is_array_open(tree: &Tree, open: usize) -> bool {
    if short_list_target(tree, open) {
        return false;
    }
    !bracket_prev_is_operand(tree, open)
}

/// True when the `[` at `open` is followed by `=` after its match.
fn short_list_target(tree: &Tree, open: usize) -> bool {
    tree.match_close(open).is_some_and(|close| {
        tree.next_semantic(close)
            .is_some_and(|n| tree.tokens[n].kind == "=")
    })
}

/// True when `[` follows an operand-ish token (index access) or `as`.
fn bracket_prev_is_operand(tree: &Tree, open: usize) -> bool {
    tree.prev_semantic(open).is_some_and(|p| {
        matches!(
            tree.tokens[p].kind.as_str(),
            kinds::T_VARIABLE
                | kinds::T_STRING
                | kinds::T_STATIC
                | ")"
                | "]"
                | "}"
                | kinds::T_CONSTANT_ENCAPSED_STRING
                | kinds::T_INTERPOLATED_STRING
                | kinds::T_LNUMBER
                | kinds::T_DNUMBER
                | kinds::T_AS
                | "$"
                | kinds::T_INC
                | kinds::T_DEC
                | "@"
        )
    })
}

/// XHP77: trailing commas in `list(...)` / `[...] =` targets.
fn rule_list_assignment(tree: &Tree, out: &mut Vec<RawFinding>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind == kinds::T_LIST
            && let Some(lp) = tree.next_semantic(i)
            && tree.tokens[lp].kind == "("
            && let Some(rp) = tree.match_close(lp)
        {
            trailing_commas_in(tree, lp, rp, out);
        } else if token.kind == "["
            && is_short_list(tree, i)
            && let Some(close) = tree.match_close(i)
        {
            trailing_commas_in(tree, i, close, out);
        }
    }
}

fn is_short_list(tree: &Tree, open: usize) -> bool {
    short_list_target(tree, open) && !bracket_prev_is_operand(tree, open)
}

/// Flag commas after the last semantic token inside `(open, close)`.
fn trailing_commas_in(tree: &Tree, open: usize, close: usize, out: &mut Vec<RawFinding>) {
    let mut i = close;
    while i > open + 1 {
        i -= 1;
        let token = &tree.tokens[i];
        if token.kind == "," {
            push(
                out,
                77,
                token.offset,
                "Unnecessary comma in list assignment.",
            );
        } else if is_semantic(token) {
            break;
        }
    }
}

#[cfg(test)]
#[path = "xhp_rem2_tests.rs"]
mod xhp_rem2_tests;
