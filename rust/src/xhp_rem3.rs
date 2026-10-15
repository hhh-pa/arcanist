//! XHP declaration / class-tree remainder rules (T1128, dv-c).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`).
//!
//! Assigned rules (ID: PHP class):
//! - 87: ArcanistRaggedClassTreeEdgeXHPASTLinterRule
//! - 88: ArcanistClassExtendsObjectXHPASTLinterRule
//! - 91: ArcanistThisReassignmentXHPASTLinterRule (`$this = ...`)
//! - 92: ArcanistUnexpectedReturnValueXHPASTLinterRule
//! - 95: ArcanistSelfClassReferenceXHPASTLinterRule (`new self`)
//! - 107: ArcanistAbstractPrivateMethodXHPASTLinterRule
//! - 108: ArcanistAbstractMethodBodyXHPASTLinterRule
//! - 113: ArcanistClassMustBeDeclaredAbstractXHPASTLinterRule
//! - 114: ArcanistInterfaceMethodBodyXHPASTLinterRule
//! - 118: ArcanistInterfaceAbstractMethodXHPASTLinterRule
//! - 128: ArcanistContinueInsideSwitchXHPASTLinterRule
//! - 130: ArcanistPublicPropertyXHPASTLinterRule (`public` props)
//! - 132: ArcanistPartialCatchXHPASTLinterRule (`catch (Exception`)
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (token scans,
//! `Tree::blocks`, `Tree::is_method`, `Tree::enclosing_class`). Emit
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
//! `cargo fmt --check` clean. Touch ONLY this file,
//! `xhp_rem3_tests.rs`, and the `lint_xhpast.rs` wiring.
//!
//! Port notes:
//! - Stock XHPAST predates PHP 7, so anonymous classes (`new class`)
//!   never reach the PHP rules (they fail to parse there). Rules keyed
//!   on the class declaration itself (87 needs the class-name node;
//!   88 and 113 raise on the class) skip anonymous classes; rules
//!   keyed on member/method nodes (92, 107, 108, 130) still apply
//!   inside them. Rule 95 skips anonymous classes explicitly,
//!   mirroring the PHP `n_EMPTY` name check.
//! - XHP87 compares modifier spellings case-sensitively (uppercase
//!   `FINAL` does not count), exactly like the PHP `== 'final'`
//!   checks. Everywhere else modifiers match case-insensitively,
//!   mirroring `getModifiers()` / the explicit `strtolower()` in
//!   XHP107.
//! - XHP87 exempts classes whose adjacent docblock declares
//!   `@concrete-extensible` (presence counts, even valueless).
//! - XHP128 reproduces the PHP quirks: nested switches raise once per
//!   enclosing `switch`, and the ancestor walk crosses function
//!   boundaries (only `while`/`for`/`foreach`/`do-while` stop it).
//! - XHP132 collects every class name in a `catch` header, so union
//!   catches (`catch (A | B $e)`, PHP 7.1+) count `Throwable` in any
//!   position; stock XHPAST predates unions, so the PHP only ever saw
//!   single names. A leading `\` (as in `catch (\Exception $e)`)
//!   keeps the PHP behavior of not matching.
//! - No sub-check was omitted: every assigned rule is fully ported.

use super::xhp_ast::{Block, BlockKind, BodySpan, RawFinding, RuleCtx, Tree, is_semantic, kinds};

/// Run all rem3 rules, pushing findings into `out`.
pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    rule_ragged_edge(tree, &blocks, out);
    rule_extends_object(tree, &blocks, out);
    rule_this_reassignment(tree, out);
    rule_unexpected_return(tree, &blocks, out);
    rule_self_reference(tree, &blocks, out);
    rule_abstract_private(tree, &blocks, out);
    rule_abstract_body(tree, &blocks, out);
    rule_must_be_abstract(tree, &blocks, out);
    rule_interface_body(tree, &blocks, out);
    rule_interface_abstract(tree, &blocks, out);
    rule_continue_switch(tree, &blocks, out);
    rule_public_property(tree, &blocks, out);
    rule_partial_catch(tree, &blocks, out);
}

fn push(out: &mut Vec<RawFinding>, rule: u32, offset: usize, message: &str) {
    out.push(RawFinding::at(rule, offset, message.to_string()));
}

fn is_modifier_word(kind: &str) -> bool {
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

/// First modifier token before `kw_idx` (the PHP node start), else
/// `kw_idx` itself. Mirrors `Tree::modifiers_before` traversal.
fn node_start(tree: &Tree, kw_idx: usize) -> usize {
    let mut start = kw_idx;
    let mut i = kw_idx;
    while let Some(prev) = i.checked_sub(1) {
        let kind = tree.tokens[prev].kind.as_str();
        if is_modifier_word(kind) {
            start = prev;
            i = prev;
        } else if kind == "]" {
            let Some(open) = tree.match_open(prev) else {
                break;
            };
            i = open;
        } else if !is_semantic(&tree.tokens[prev]) {
            i = prev;
        } else {
            break;
        }
    }
    start
}

/// Case-insensitive modifier probe (mirrors `getModifiers()`).
fn has_modifier(block: &Block, name: &str) -> bool {
    block.modifiers.iter().any(|m| m.eq_ignore_ascii_case(name))
}

/// Case-sensitive modifier probe (XHP87's `==` checks).
fn has_modifier_exact(block: &Block, name: &str) -> bool {
    block.modifiers.iter().any(|m| m == name)
}

/// Named function blocks declared directly inside a class-like body.
fn method_blocks<'a>(tree: &Tree, blocks: &'a [Block]) -> Vec<(usize, &'a Block)> {
    blocks
        .iter()
        .enumerate()
        .filter(|(idx, block)| {
            block.kind == BlockKind::Function && !block.is_anonymous && tree.is_method(blocks, *idx)
        })
        .collect()
}

/// XHP87: classes must be `final`, `abstract`, or `@concrete-extensible`.
fn rule_ragged_edge(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if block.kind != BlockKind::Class || block.is_anonymous {
            continue;
        }
        let Some(name_idx) = block.name_idx else {
            continue;
        };
        if has_modifier_exact(block, "final") || has_modifier_exact(block, "abstract") {
            continue;
        }
        if class_docblock_exempts(tree, block) {
            continue;
        }
        push(
            out,
            87,
            tree.tokens[name_idx].offset,
            "This class is neither `final` nor `abstract`, and does not have a docblock marking \
             it `@concrete-extensible`.",
        );
    }
}

/// True when a `T_DOC_COMMENT` directly precedes the class node and
/// declares `@concrete-extensible`.
fn class_docblock_exempts(tree: &Tree, block: &Block) -> bool {
    let mut i = node_start(tree, block.kw_idx);
    while i > 0 && tree.tokens[i - 1].kind == kinds::T_WHITESPACE {
        i -= 1;
    }
    let Some(doc) = i.checked_sub(1) else {
        return false;
    };
    tree.tokens[doc].kind == kinds::T_DOC_COMMENT
        && has_concrete_extensible(&tree.tokens[doc].value)
}

fn has_concrete_extensible(doc: &str) -> bool {
    let bytes = doc.as_bytes();
    let needle = b"@concrete-extensible";
    let mut from = 0;
    while from + needle.len() <= bytes.len() {
        let Some(rel) = find_bytes(&bytes[from..], needle) else {
            return false;
        };
        let hit = from + rel;
        let before_ok = hit == 0 || !is_doc_key_char(bytes[hit - 1]);
        let after = hit + needle.len();
        let after_ok = after >= bytes.len() || !is_doc_key_char(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        from = hit + 1;
    }
    false
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn is_doc_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// XHP88: every class except `Phobject` must have a base class.
fn rule_extends_object(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if block.kind != BlockKind::Class || block.is_anonymous {
            continue;
        }
        if block.name.as_deref() == Some("Phobject") {
            continue;
        }
        if block.extends.is_none() {
            push(
                out,
                88,
                tree.tokens[node_start(tree, block.kw_idx)].offset,
                "Classes should extend from `Phobject` or from some other class. All classes \
                 (except for `Phobject` itself) should have a base class.",
            );
        }
    }
}

/// XHP91: `$this` cannot be reassigned.
fn rule_this_reassignment(tree: &Tree, out: &mut Vec<RawFinding>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_VARIABLE || token.value != "$this" {
            continue;
        }
        if tree
            .prev_semantic(i)
            .is_some_and(|p| tree.tokens[p].kind == "$")
        {
            continue;
        }
        if tree
            .next_semantic(i)
            .is_some_and(|n| tree.tokens[n].kind == "=")
        {
            push(
                out,
                91,
                token.offset,
                "`$this` cannot be re-assigned. This construct will cause a PHP fatal error.",
            );
        }
    }
}

/// XHP92: `__construct` / `__destruct` must not return values.
fn rule_unexpected_return(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in method_blocks(tree, blocks) {
        let Some(name) = block.name.as_deref() else {
            continue;
        };
        if !name.eq_ignore_ascii_case("__construct") && !name.eq_ignore_ascii_case("__destruct") {
            continue;
        }
        let (Some(first), Some(last)) = (tree.body_first(block), tree.body_last(block)) else {
            continue;
        };
        unexpected_return_in_body(tree, blocks, idx, name, first, last, out);
    }
}

fn unexpected_return_in_body(
    tree: &Tree,
    blocks: &[Block],
    method: usize,
    name: &str,
    first: usize,
    last: usize,
    out: &mut Vec<RawFinding>,
) {
    let closures = inner_closure_spans(tree, blocks, method, last);
    for r in first..=last {
        if tree.tokens[r].kind != kinds::T_RETURN {
            continue;
        }
        if closures.iter().any(|(s, e)| *s <= r && r <= *e) {
            continue;
        }
        if is_bare_keyword(tree, r) {
            continue;
        }
        push(
            out,
            92,
            tree.tokens[r].offset,
            &format!("Unexpected `return` value in `{name}` method."),
        );
    }
}

/// Anonymous-function spans nested inside a method body.
fn inner_closure_spans(
    tree: &Tree,
    blocks: &[Block],
    method: usize,
    last: usize,
) -> Vec<(usize, usize)> {
    let kw = blocks[method].kw_idx;
    let mut spans = Vec::new();
    for block in blocks {
        if block.kind != BlockKind::Function || !block.is_anonymous {
            continue;
        }
        if block.kw_idx <= kw || block.kw_idx > last {
            continue;
        }
        if let Some(end) = tree.body_last(block) {
            spans.push((block.kw_idx, end));
        }
    }
    spans
}

/// True when `return` / `continue` carries no expression: `;`, `}`,
/// `?>`, or EOF.
fn is_bare_keyword(tree: &Tree, idx: usize) -> bool {
    match tree.next_semantic(idx) {
        None => true,
        Some(n) => matches!(tree.tokens[n].kind.as_str(), ";" | "}" | kinds::T_CLOSE_TAG),
    }
}

/// XHP95: instantiate the current class via `self`.
fn rule_self_reference(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if block.kind != BlockKind::Class || block.is_anonymous {
            continue;
        }
        let (Some(name), Some(last)) = (block.name.as_deref(), tree.body_last(block)) else {
            continue;
        };
        self_reference_in_span(tree, block.kw_idx, last, name, out);
    }
}

fn self_reference_in_span(
    tree: &Tree,
    kw: usize,
    last: usize,
    name: &str,
    out: &mut Vec<RawFinding>,
) {
    let end = last.min(tree.tokens.len().saturating_sub(1));
    for i in kw..=end {
        if tree.tokens[i].kind == kinds::T_NEW
            && let Some(hit) = self_reference_target(tree, i, name)
        {
            push(
                out,
                95,
                tree.tokens[hit].offset,
                "Use `self` to instantiate the current class.",
            );
        }
    }
}

/// The class-name token when `new` instantiates exactly `name`.
fn self_reference_target(tree: &Tree, new_idx: usize, name: &str) -> Option<usize> {
    let n = tree.next_semantic(new_idx)?;
    if tree.tokens[n].kind != kinds::T_STRING || !tree.tokens[n].value.eq_ignore_ascii_case(name) {
        return None;
    }
    if tree.prev_semantic(n) != Some(new_idx) {
        return None;
    }
    if tree
        .next_semantic(n)
        .is_some_and(|a| tree.tokens[a].kind == "\\")
    {
        return None;
    }
    Some(n)
}

/// XHP107: `abstract` methods cannot be `private`.
fn rule_abstract_private(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (_, block) in method_blocks(tree, blocks) {
        if has_modifier(block, "abstract") && has_modifier(block, "private") {
            push(
                out,
                107,
                tree.tokens[node_start(tree, block.kw_idx)].offset,
                "`abstract` method cannot be declared `private`. This construct will cause a \
                 fatal error.",
            );
        }
    }
}

/// XHP108: `abstract` methods cannot have a body.
fn rule_abstract_body(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (_, block) in method_blocks(tree, blocks) {
        if !has_modifier(block, "abstract") {
            continue;
        }
        if let BodySpan::Brace { open, .. } = block.body {
            push(
                out,
                108,
                tree.tokens[open].offset,
                "`abstract` methods cannot contain a body. This construct will cause a fatal \
                 error.",
            );
        }
    }
}

/// XHP113: classes with `abstract` methods must be `abstract`.
fn rule_must_be_abstract(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if block.kind != BlockKind::Class || block.is_anonymous {
            continue;
        }
        if has_modifier(block, "abstract") {
            continue;
        }
        let Some(last) = tree.body_last(block) else {
            continue;
        };
        let count = abstract_method_count(tree, blocks, block.kw_idx, last);
        if count > 0 {
            push(
                out,
                113,
                tree.tokens[node_start(tree, block.kw_idx)].offset,
                &format!(
                    "Class contains {count} abstract method(s) and must therefore be declared \
                     `abstract`."
                ),
            );
        }
    }
}

fn abstract_method_count(tree: &Tree, blocks: &[Block], kw: usize, last: usize) -> usize {
    method_blocks(tree, blocks)
        .iter()
        .filter(|(_, m)| m.kw_idx > kw && m.kw_idx <= last && has_modifier(m, "abstract"))
        .count()
}

/// XHP114: `interface` methods cannot have a body.
fn rule_interface_body(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in method_blocks(tree, blocks) {
        if !is_interface_method(tree, blocks, idx) {
            continue;
        }
        if let BodySpan::Brace { open, .. } = block.body {
            push(
                out,
                114,
                tree.tokens[open].offset,
                "`interface` methods cannot contain a body. This construct will cause a fatal \
                 error.",
            );
        }
    }
}

/// XHP118: `interface` methods cannot be `abstract`.
fn rule_interface_abstract(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in method_blocks(tree, blocks) {
        if !is_interface_method(tree, blocks, idx) {
            continue;
        }
        if has_modifier(block, "abstract") {
            push(
                out,
                118,
                tree.tokens[node_start(tree, block.kw_idx)].offset,
                "`interface` methods cannot be marked as `abstract`. This construct will cause \
                 a fatal error.",
            );
        }
    }
}

fn is_interface_method(tree: &Tree, blocks: &[Block], idx: usize) -> bool {
    tree.enclosing_class(blocks, idx)
        .is_some_and(|c| blocks[c].kind == BlockKind::Interface)
}

/// XHP128: bare `continue;` inside `switch` (acts like `break;`).
fn rule_continue_switch(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    let lasts: Vec<Option<usize>> = blocks.iter().map(|b| tree.body_last(b)).collect();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_CONTINUE || !is_bare_keyword(tree, i) {
            continue;
        }
        if let Some(inner) = innermost_block(blocks, &lasts, i) {
            continue_switch_walk(blocks, inner, token.offset, out);
        }
    }
}

fn innermost_block(blocks: &[Block], lasts: &[Option<usize>], idx: usize) -> Option<usize> {
    let mut best = None;
    for (n, block) in blocks.iter().enumerate() {
        if block.kw_idx < idx && lasts[n].is_some_and(|last| last >= idx) {
            let deeper = best.is_none_or(|b: usize| blocks[n].depth > blocks[b].depth);
            if deeper {
                best = Some(n);
            }
        }
    }
    best
}

/// Walk ancestors mirroring the PHP parent loop: loops stop it,
/// every enclosing `switch` raises, anything else is transparent.
fn continue_switch_walk(
    blocks: &[Block],
    mut cur: usize,
    offset: usize,
    out: &mut Vec<RawFinding>,
) {
    loop {
        match blocks[cur].kind {
            BlockKind::While | BlockKind::DoWhile | BlockKind::For | BlockKind::Foreach => break,
            BlockKind::Switch => push(
                out,
                128,
                offset,
                "In a \"switch\" statement, \"continue;\" is equivalent to \"break;\" but causes \
                 compile errors beginning with PHP 7.0.0.",
            ),
            _ => {}
        }
        let Some(parent) = blocks[cur].parent else {
            break;
        };
        cur = parent;
    }
}

/// XHP130: `public` properties should use getters/setters.
fn rule_public_property(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if !matches!(block.kind, BlockKind::Class | BlockKind::Trait) {
            continue;
        }
        if let BodySpan::Brace { open, close } = block.body {
            public_property_in_body(tree, open, close.unwrap_or(tree.tokens.len()), out);
        }
    }
}

fn public_property_in_body(tree: &Tree, open: usize, close: usize, out: &mut Vec<RawFinding>) {
    let end = close.min(tree.tokens.len());
    let mut seg_start = open + 1;
    let mut depth: u32 = 0;
    let mut i = seg_start;
    while i < end {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            ";" if depth == 0 => {
                check_member_segment(tree, seg_start, i, out);
                seg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
}

fn check_member_segment(tree: &Tree, first: usize, semi: usize, out: &mut Vec<RawFinding>) {
    let mut depth: u32 = 0;
    let mut i = first;
    let mut has_public = false;
    let mut has_variable = false;
    let mut first_modifier = None;
    while i < semi {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            _ if depth == 0 => {
                if member_segment_halts(tree, i) {
                    return;
                }
                member_segment_flags(
                    tree,
                    i,
                    &mut has_public,
                    &mut has_variable,
                    &mut first_modifier,
                );
            }
            _ => {}
        }
        i += 1;
    }
    if has_public
        && has_variable
        && let Some(start) = first_modifier
    {
        push(
            out,
            130,
            tree.tokens[start].offset,
            "`public` properties should be avoided. Instead of exposing the property value \
             directly, consider using getter and setter methods.",
        );
    }
}

/// Methods (and closures) are not member declarations.
fn member_segment_halts(tree: &Tree, i: usize) -> bool {
    matches!(
        tree.tokens[i].kind.as_str(),
        kinds::T_FUNCTION | kinds::T_FN
    )
}

fn member_segment_flags(
    tree: &Tree,
    i: usize,
    has_public: &mut bool,
    has_variable: &mut bool,
    first_modifier: &mut Option<usize>,
) {
    let kind = tree.tokens[i].kind.as_str();
    if is_modifier_word(kind) {
        if first_modifier.is_none() {
            *first_modifier = Some(i);
        }
        if kind == kinds::T_PUBLIC {
            *has_public = true;
        }
    } else if kind == kinds::T_VARIABLE {
        *has_variable = true;
    }
}

/// XHP132: `catch (Exception` without `catch (Throwable`.
fn rule_partial_catch(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for block in blocks {
        if block.kind != BlockKind::Try {
            continue;
        }
        let mut exception_at = None;
        let mut has_throwable = false;
        for linked in &block.linked {
            if blocks[*linked].kind != BlockKind::Catch {
                continue;
            }
            catch_names(
                tree,
                &blocks[*linked],
                &mut exception_at,
                &mut has_throwable,
            );
        }
        if let Some(at) = exception_at
            && !has_throwable
        {
            push(
                out,
                132,
                tree.tokens[at].offset,
                "Try/catch block catches \"Exception\", but does not catch \"Throwable\". In \
                 PHP7 and newer, some runtime exceptions will escape this block.",
            );
        }
    }
}

/// Collect class names from one `catch` header (last `Exception`
/// wins, mirroring the PHP map overwrite).
fn catch_names(
    tree: &Tree,
    catch: &Block,
    exception_at: &mut Option<usize>,
    has_throwable: &mut bool,
) {
    let Some(lparen) = catch.lparen else {
        return;
    };
    let end = catch.rparen.unwrap_or(tree.tokens.len());
    let mut i = lparen + 1;
    while i < end && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if kind == kinds::T_STRING || kind == "\\" {
            let (name, next) = catch_name_run(tree, i, end);
            if name.eq_ignore_ascii_case("exception") {
                *exception_at = Some(i);
            } else if name.eq_ignore_ascii_case("throwable") {
                *has_throwable = true;
            }
            i = next;
        } else {
            i += 1;
        }
    }
}

/// One (possibly qualified) class name starting at `start`.
fn catch_name_run(tree: &Tree, start: usize, end: usize) -> (String, usize) {
    let mut name = String::new();
    let mut i = start;
    while i < end && i < tree.tokens.len() {
        let token = &tree.tokens[i];
        if token.kind == kinds::T_STRING || token.kind == "\\" {
            name.push_str(&token.value);
            i += 1;
        } else if is_semantic(token) {
            break;
        } else {
            i += 1;
        }
    }
    (name, i)
}

#[cfg(test)]
#[path = "xhp_rem3_tests.rs"]
mod xhp_rem3_tests;
