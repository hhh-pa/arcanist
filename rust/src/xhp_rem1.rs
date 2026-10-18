//! XHP remaining rules, batch 1 (T1128).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`).
//!
//! Assigned rules (ID: PHP class):
//! - 5: ArcanistUndeclaredVariableXHPASTLinterRule
//! - 9: ArcanistNamingConventionsXHPASTLinterRule
//! - 10: ArcanistImplicitConstructorXHPASTLinterRule
//! - 13: ArcanistStaticThisXHPASTLinterRule
//! - 17: ArcanistExitExpressionXHPASTLinterRule
//! - 19: ArcanistClassFilenameMismatchXHPASTLinterRule
//! - 20: ArcanistTautologicalExpressionXHPASTLinterRule
//! - 22: ArcanistDuplicateKeysInArrayXHPASTLinterRule
//! - 23: ArcanistReusedIteratorXHPASTLinterRule
//! - 24: ArcanistBraceFormattingXHPASTLinterRule
//! - 30: ArcanistImplicitFallthroughXHPASTLinterRule
//! - 32: ArcanistReusedAsIteratorXHPASTLinterRule
//! - 39: ArcanistReusedIteratorReferenceXHPASTLinterRule
//! - 45: ArcanistPHPCompatibilityXHPASTLinterRule
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (token scans,
//! `Tree::gap_text`, `Tree::calls`, `Tree::blocks`). Emit
//! [`super::xhp_ast::RawFinding`] with the PHP rule ID and the EXACT
//! PHP message text (`pht()` templates with `%s`/`%d` substituted
//! exactly as the PHP does, including quirks). Do not invent
//! messages.
//!
//! Port notes (sub-checks that cannot be ported meaningfully, plus
//! fixture-derived quirks):
//! - XHP45: the symbol-table checks (functions, classes,
//!   constants, parameter versions, Windows availability, and the
//!   `class_exists`/`function_exists`/`defined` conditional-usage
//!   whitelist) are ported against the vendored
//!   `rust/resources/php/symbol-information.json` (embedded via
//!   `include_str!`, byte-identical to the PHP-side copy), plus the
//!   `xhpast.php-version` gating and the PHP 5.3/5.4
//!   syntax-feature checks. Prerelease table versions
//!   (`5.4.0RC3`, `5.6.0beta1`) compare in PHP
//!   `version_compare()` order, which the shared dotted comparator
//!   does not implement. `interface_exists()` guards are dropped:
//!   PHP files them under an `interface` bucket no check reads.
//!   Member names after `->`/`?->`/`::`, declaration names, `as`
//!   aliases, `goto` targets, and statement-start labels are never
//!   scanned (all `n_STRING`, never `n_SYMBOL_NAME`, per the parser
//!   fixtures); each `use`/`namespace` name gets one lookup of its
//!   full concrete string, exactly like its single `n_SYMBOL_NAME`
//!   node, and so does every other qualified run (namespaced
//!   constants live in the table); runs with interior whitespace
//!   never look up (PHP's concrete string carries the spaces and
//!   misses). Deliberate approximations: `use function`/`use const`
//!   and group-`use` names are skipped (the stock grammar cannot
//!   parse those files at all); a label directly after `:` still
//!   scans; class-like property types are not scanned.
//! - XHP9/XHP30: the `xhpast.naminghook` / `xhpast.switchhook`
//!   overrides name custom PHP classes and cannot run here; default
//!   determinations are enforced.
//! - XHP9: single-declarator class `const` statements raise only
//!   `Constants should be uppercase.` (no `LIST` node exists for
//!   them, so the class-constants convention check cannot fire);
//!   multi-declarator statements raise both. Derived from the
//!   `naming-conventions` fixtures.
//! - XHP17: only `exit` is flagged, never `die` (PHP compares the
//!   operator against `'exit'`).
//! - XHP20: `<>` is excluded from the identical-operand check (PHP
//!   lists `!=` only); `and`/`or` are excluded from the static-logic
//!   check (PHP lists `&&`/`||` only).
//! - XHP24: the `n_DO_WHILE` leg of the third check is dead in PHP
//!   (a do-while node never ends with `}`) and is omitted.
//! - XHP45 method-call indexing (`$o->m()[0]`) is flagged per the
//!   rule code (`n_METHOD_CALL` is listed); the fixture comment cites
//!   T4334 for engine behavior here.
//! - Modern syntax stock XHPAST cannot parse (arrow functions,
//!   `?->`, short array destructuring): arrow-function bodies are
//!   excluded from XHP5 (they capture the enclosing scope, so uses
//!   cannot be judged undeclared); XHP45 closure checks skip arrow
//!   functions (they bind `$this`, unlike 5.3 closures); `?->` is
//!   treated like `->` where the check is about the operand rather
//!   than the operator.
//!
//! Constraints: no new dependencies; every function cy<=15 / co<=20
//! (named private fns, same file; closures count into parents);
//! `cargo clippy --offline --all-targets -- -D warnings` clean;
//! `cargo fmt --check` clean. Touch ONLY this file and
//! `xhp_rem1_tests.rs` (plus the `lint_xhpast.rs` wiring and the
//! vendored `resources/php/symbol-information.json` data).

use std::collections::{HashMap, HashSet};

use super::xhp_ast::{
    Block, BlockKind, BodySpan, CallSite, RawFinding, RuleCtx, Token, Tree, is_semantic, kinds,
    string_variables, unquote_plain_string, version_compare,
};

/// Run all batch-1 remaining rules, pushing findings into `out`.
pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    rule_undeclared_variable(ctx, out);
    rule_naming_conventions(ctx, out);
    rule_implicit_constructor(ctx, out);
    rule_static_this(ctx, out);
    rule_exit_expression(ctx, out);
    rule_class_filename_mismatch(ctx, out);
    rule_tautological_expression(ctx, out);
    rule_duplicate_keys(ctx, out);
    rule_reused_iterator(ctx, out);
    rule_brace_formatting(ctx, out);
    rule_implicit_fallthrough(ctx, out);
    rule_reused_as_iterator(ctx, out);
    rule_reused_iterator_reference(ctx, out);
    rule_php_compatibility(ctx, out);
}

fn push(out: &mut Vec<RawFinding>, rule: u32, offset: usize, message: &str) {
    out.push(RawFinding::at(rule, offset, message.to_string()));
}

/// PHP superglobals, predeclared in every function scope (XHP5/32).
const SUPERGLOBALS: &[&str] = &[
    "$GLOBALS",
    "$_SERVER",
    "$_GET",
    "$_POST",
    "$_FILES",
    "$_COOKIE",
    "$_SESSION",
    "$_REQUEST",
    "$_ENV",
];

fn is_superglobal(name: &str) -> bool {
    SUPERGLOBALS.contains(&name)
}

/// Inclusive token index where a block's span ends.
fn span_end_idx(tree: &Tree, block: &Block) -> usize {
    tree.body_last(block)
        .or(block.rparen)
        .unwrap_or(block.kw_idx)
}

/// Inclusive (first, last) token range of a block body, if any.
fn body_range(tree: &Tree, block: &Block) -> Option<(usize, usize)> {
    let first = tree.body_first(block)?;
    let last = tree.body_last(block)?;
    Some((first.min(last), first.max(last)))
}

/// True when token `idx` lies inside `[start, end]`.
fn in_range(idx: usize, start: usize, end: usize) -> bool {
    idx >= start && idx <= end
}

/// Indices of `kind` blocks whose keyword lies in `range`.
fn blocks_in_range(blocks: &[Block], range: (usize, usize), kind: BlockKind) -> Vec<usize> {
    blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.kind == kind && in_range(b.kw_idx, range.0, range.1))
        .map(|(i, _)| i)
        .collect()
}

/// A plain `foreach` iterator: variable token plus by-ref flag.
#[derive(Debug, Clone, Copy)]
struct IteratorVar {
    idx: usize,
    by_ref: bool,
}

/// Plain key/value iterators of a `foreach` header.
#[derive(Debug, Default)]
struct ForeachIters {
    key: Option<IteratorVar>,
    value: Option<IteratorVar>,
}

/// Parse the iterators of a `foreach` header. Non-plain shapes
/// (`list()`, `$$x`) yield `None` parts.
fn foreach_iters(tree: &Tree, block: &Block) -> ForeachIters {
    let mut out = ForeachIters::default();
    let (Some(lparen), Some(rparen)) = (block.lparen, block.rparen) else {
        return out;
    };
    let Some(as_idx) = scan_top_level(tree, lparen, rparen, kinds::T_AS) else {
        return out;
    };
    let val_start = match scan_top_level(tree, as_idx, rparen, kinds::T_DOUBLE_ARROW) {
        Some(arrow) => {
            out.key = plain_iterator(tree, as_idx + 1, arrow);
            arrow + 1
        }
        None => as_idx + 1,
    };
    out.value = plain_iterator(tree, val_start, rparen);
    out
}

/// Depth-0 scan for one token kind strictly inside `(open, close)`.
fn scan_top_level(tree: &Tree, open: usize, close: usize, want: &str) -> Option<usize> {
    let mut depth = 0;
    let mut i = open + 1;
    while i < close && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if kind == "(" || kind == "[" || kind == "{" {
            depth += 1;
        } else if kind == ")" || kind == "]" || kind == "}" {
            depth -= 1;
        } else if depth == 0 && kind == want {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A plain iterator part: `[$var]` or `[&, $var]`, else `None`.
fn plain_iterator(tree: &Tree, start: usize, end: usize) -> Option<IteratorVar> {
    let mut parts = Vec::new();
    let mut i = start;
    while i < end && i < tree.tokens.len() {
        if is_semantic(&tree.tokens[i]) {
            parts.push(i);
        }
        i += 1;
    }
    match parts.as_slice() {
        [var] if tree.tokens[*var].kind == kinds::T_VARIABLE => Some(IteratorVar {
            idx: *var,
            by_ref: false,
        }),
        [amp, var]
            if tree.tokens[*amp].kind == "&" && tree.tokens[*var].kind == kinds::T_VARIABLE =>
        {
            Some(IteratorVar {
                idx: *var,
                by_ref: true,
            })
        }
        _ => None,
    }
}

/// Variables assigned with plain `=` in a `for` header (XHP23).
fn for_assigned_vars(tree: &Tree, block: &Block) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let (Some(lparen), Some(rparen)) = (block.lparen, block.rparen) else {
        return out;
    };
    let mut i = lparen + 1;
    while i < rparen {
        if tree.tokens[i].kind == "="
            && let Some(var) = tree.prev_semantic(i)
            && var > lparen
            && tree.tokens[var].kind == kinds::T_VARIABLE
            && !is_dynamic_var(tree, var)
        {
            out.push((tree.tokens[var].value.clone(), var));
        }
        i += 1;
    }
    out
}

/// True when a `T_VARIABLE` is really `$$x` (a `$` sits before it).
fn is_dynamic_var(tree: &Tree, var: usize) -> bool {
    tree.prev_semantic(var)
        .is_some_and(|p| tree.tokens[p].kind == "$")
}

/// Collect `static` / `global` declarators: `T_VARIABLE` indices from
/// `first` to the depth-0 `;`, skipping `= value` parts.
fn collect_decl_list(tree: &Tree, first: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut i = first;
    while i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if kind == "(" || kind == "[" || kind == "{" {
            depth += 1;
        } else if kind == ")" || kind == "]" || kind == "}" {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if depth == 0 && kind == ";" {
            break;
        } else if depth == 0 && kind == kinds::T_VARIABLE {
            out.push(i);
        } else if depth == 0 && kind == "=" {
            i = skip_assign_value(tree, i);
        }
        i += 1;
    }
    out
}

/// Index of the depth-0 `,` / `;` ending an `= value` in a decl list.
fn skip_assign_value(tree: &Tree, eq: usize) -> usize {
    let mut depth = 0;
    let mut i = eq + 1;
    while i < tree.tokens.len() {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" if depth > 0 => depth -= 1,
            "," | ";" if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    i
}

/// Consume one RHS chain link; false ends the chain.
fn static_chain_step(tree: &Tree, next: usize, out: &mut HashSet<usize>) -> bool {
    match tree.tokens[next].kind.as_str() {
        kinds::T_STRING | kinds::T_CLASS | "$" => true,
        kinds::T_VARIABLE => {
            out.insert(next);
            true
        }
        kinds::T_OBJECT_OPERATOR | kinds::T_NULLSAFE_OBJECT_OPERATOR => true,
        "(" | "[" | "{" => {
            bracket_vars(tree, next, out);
            true
        }
        _ => false,
    }
}

/// Advance past a consumed link (skipping bracketed groups).
fn static_chain_next(tree: &Tree, next: usize) -> usize {
    match tree.tokens[next].kind.as_str() {
        "(" | "[" | "{" => tree.match_close(next).unwrap_or(next),
        _ => next,
    }
}

/// Insert `T_VARIABLE` indices strictly inside a bracket pair.
fn bracket_vars(tree: &Tree, open: usize, out: &mut HashSet<usize>) {
    let end = tree.match_close(open).unwrap_or(tree.tokens.len());
    let mut i = open + 1;
    while i < end {
        if tree.tokens[i].kind == kinds::T_VARIABLE {
            out.insert(i);
        }
        i += 1;
    }
}

/// All `T_VARIABLE` indices excluded as static-access parts in range.
fn static_excluded_in(tree: &Tree, start: usize, end: usize) -> HashSet<usize> {
    let mut out = HashSet::new();
    let mut i = start;
    while i <= end && i < tree.tokens.len() {
        if tree.tokens[i].kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
            i += 1;
            continue;
        }
        if let Some(lhs) = tree.prev_semantic(i)
            && tree.tokens[lhs].kind == kinds::T_VARIABLE
        {
            out.insert(lhs);
        }
        let mut j = i;
        while let Some(next) = tree.next_semantic(j) {
            if !static_chain_step(tree, next, &mut out) {
                break;
            }
            j = static_chain_next(tree, next);
        }
        i += 1;
    }
    out
}

/// Array literals as `(open, close)` token pairs: `array(...)` plus
/// short `[...]` (index and attribute brackets excluded).
fn array_literals(tree: &Tree) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind == kinds::T_ARRAY {
            if let Some(pair) = array_call(tree, i) {
                out.push(pair);
            }
        } else if token.kind == "["
            && !is_index_bracket(tree, i)
            && !is_attribute_bracket(tree, i)
            && let Some(close) = tree.match_close(i)
        {
            out.push((i, close));
        }
    }
    out
}

fn array_call(tree: &Tree, array_kw: usize) -> Option<(usize, usize)> {
    let lparen = tree.next_semantic(array_kw)?;
    if tree.tokens[lparen].kind != "(" {
        return None;
    }
    tree.match_close(lparen).map(|close| (lparen, close))
}

fn is_attribute_bracket(tree: &Tree, bracket: usize) -> bool {
    tree.prev_semantic(bracket)
        .is_some_and(|p| tree.tokens[p].kind == kinds::T_ATTRIBUTE)
}

fn is_index_bracket(tree: &Tree, bracket: usize) -> bool {
    // `[` never leads the file (outside PHP mode it lexes as inline
    // HTML), so a previous token always exists; token 0 falls through
    // to `_` below exactly like the old `None` arm.
    let prev = tree.prev_semantic(bracket).unwrap_or(0);
    match tree.tokens[prev].kind.as_str() {
        kinds::T_VARIABLE
        | kinds::T_STRING
        | "]"
        | kinds::T_CONSTANT_ENCAPSED_STRING
        | kinds::T_INTERPOLATED_STRING => true,
        ")" => is_call_or_group_rparen(tree, prev),
        "}" => is_deref_close(tree, prev),
        _ => false,
    }
}

/// A `)` before `[` indexes unless it closes a control header (in
/// which case `[` starts a fresh statement).
fn is_call_or_group_rparen(tree: &Tree, rparen: usize) -> bool {
    if tree.calls().iter().any(|call| call.rparen == Some(rparen)) {
        return true;
    }
    !tree.blocks().iter().any(|block| {
        block.rparen == Some(rparen)
            && [
                BlockKind::If,
                BlockKind::ElseIf,
                BlockKind::While,
                BlockKind::For,
                BlockKind::Foreach,
                BlockKind::Switch,
                BlockKind::Catch,
                BlockKind::Declare,
            ]
            .contains(&block.kind)
    })
}

/// A `}` before `[` indexes unless it closes a block body.
fn is_deref_close(tree: &Tree, close: usize) -> bool {
    let Some(open) = tree.match_open(close) else {
        return true;
    };
    !tree.blocks().iter().any(|block| match block.body {
        BodySpan::Brace { open: body, .. } => body == open,
        _ => false,
    })
}

/// True for `(`/`[`/`{`.
fn is_open_kind(kind: &str) -> bool {
    kind == "(" || kind == "[" || kind == "{"
}

/// True for `)`/`]`/`}`.
fn is_close_kind(kind: &str) -> bool {
    kind == ")" || kind == "]" || kind == "}"
}

/// Boundary operators splitting XHP20 operands (both check kinds).
fn is_split_boundary(kind: &str) -> bool {
    matches!(
        kind,
        ";" | ","
            | "?"
            | ":"
            | "="
            | "+="
            | "-="
            | "*="
            | "/="
            | ".="
            | "%="
            | "&="
            | "|="
            | "^="
            | "<<="
            | ">>="
            | "**="
            | "??="
            | kinds::T_DOUBLE_ARROW
            | kinds::T_BOOLEAN_AND
            | kinds::T_BOOLEAN_OR
            | kinds::T_LOGICAL_AND
            | kinds::T_LOGICAL_OR
            | kinds::T_LOGICAL_XOR
            | kinds::T_SPACESHIP
            | kinds::T_COALESCE
    ) || is_split_keyword(kind)
}

/// Statement keywords splitting XHP20 operands (expression keywords
/// like `new` / `instanceof` / `isset` stay inside operands).
fn is_split_keyword(kind: &str) -> bool {
    matches!(
        kind,
        kinds::T_RETURN
            | kinds::T_ECHO
            | kinds::T_PRINT
            | kinds::T_INCLUDE
            | kinds::T_INCLUDE_ONCE
            | kinds::T_REQUIRE
            | kinds::T_REQUIRE_ONCE
            | kinds::T_THROW
            | kinds::T_YIELD
            | kinds::T_CASE
            | kinds::T_DEFAULT
            | kinds::T_BREAK
            | kinds::T_CONTINUE
            | kinds::T_GOTO
            | kinds::T_GLOBAL
            | kinds::T_STATIC
            | kinds::T_VAR
            | kinds::T_CONST
            | kinds::T_UNSET
            | kinds::T_IF
            | kinds::T_ELSEIF
            | kinds::T_ELSE
            | kinds::T_WHILE
            | kinds::T_DO
            | kinds::T_FOR
            | kinds::T_FOREACH
            | kinds::T_SWITCH
            | kinds::T_TRY
            | kinds::T_CATCH
            | kinds::T_FINALLY
            | kinds::T_FUNCTION
            | kinds::T_FN
            | kinds::T_CLASS
            | kinds::T_INTERFACE
            | kinds::T_TRAIT
            | kinds::T_ENUM
            | kinds::T_DECLARE
            | kinds::T_NAMESPACE
            | kinds::T_USE
            | kinds::T_AS
            | kinds::T_MATCH
            | kinds::T_HALT_COMPILER
            | kinds::T_ATTRIBUTE
            | kinds::T_OPEN_TAG
            | kinds::T_OPEN_TAG_WITH_ECHO
            | kinds::T_CLOSE_TAG
            | kinds::T_INLINE_HTML
    )
}

/// Comparison operators: boundaries for the identical-operand check
/// (operands of `&&` / `||` may contain them instead).
fn is_comparison_kind(kind: &str) -> bool {
    matches!(
        kind,
        "<" | ">"
            | "<="
            | ">="
            | kinds::T_IS_EQUAL
            | kinds::T_IS_IDENTICAL
            | kinds::T_IS_NOT_EQUAL
            | kinds::T_IS_NOT_IDENTICAL
    )
}

/// Inclusive semantic token range left of `op`, or `None` when empty.
fn scan_operand_left(tree: &Tree, op: usize, stop_comparison: bool) -> Option<(usize, usize)> {
    let mut depth = 0;
    let mut j = op;
    let mut stopped = false;
    while j > 0 {
        j -= 1;
        let kind = tree.tokens[j].kind.as_str();
        if is_close_kind(kind) {
            depth += 1;
        } else if is_open_kind(kind) {
            if depth == 0 {
                stopped = true;
                break;
            }
            depth -= 1;
        } else if depth == 0 && is_operand_stop(kind, stop_comparison) {
            stopped = true;
            break;
        }
    }
    let start = if stopped { j + 1 } else { j };
    trim_semantic(tree, start, op.wrapping_sub(1))
}

/// Inclusive semantic token range right of `op`, or `None` when empty.
fn scan_operand_right(tree: &Tree, op: usize, stop_comparison: bool) -> Option<(usize, usize)> {
    let mut depth = 0;
    let mut j = op + 1;
    while j < tree.tokens.len() {
        let kind = tree.tokens[j].kind.as_str();
        if is_open_kind(kind) {
            depth += 1;
        } else if is_close_kind(kind) {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if depth == 0 && is_operand_stop(kind, stop_comparison) {
            break;
        }
        j += 1;
    }
    trim_semantic(tree, op + 1, j.wrapping_sub(1))
}

fn is_operand_stop(kind: &str, stop_comparison: bool) -> bool {
    is_split_boundary(kind) || (stop_comparison && is_comparison_kind(kind))
}

/// Trim a raw range to its semantic endpoints.
fn trim_semantic(tree: &Tree, first: usize, last: usize) -> Option<(usize, usize)> {
    if first > last || last >= tree.tokens.len() {
        return None;
    }
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

/// Semantic text of a token range (mirrors `getSemanticString()`).
fn semantic_text(tree: &Tree, first: usize, last: usize) -> String {
    let mut out = String::new();
    let mut i = first;
    while i <= last && i < tree.tokens.len() {
        if is_semantic(&tree.tokens[i]) {
            out.push_str(&tree.tokens[i].value);
        }
        i += 1;
    }
    out
}

/// Variable declarations of one function scope (XHP5/XHP32): first
/// offset per name plus the declaring token indices.
#[derive(Debug, Default)]
struct ScopeDecls {
    first: HashMap<String, usize>,
    idxs: HashSet<usize>,
}

fn declare(decls: &mut ScopeDecls, tree: &Tree, idx: usize) {
    let name = tree.tokens[idx].value.clone();
    let offset = tree.tokens[idx].offset;
    decls
        .first
        .entry(name)
        .and_modify(|seen| *seen = (*seen).min(offset))
        .or_insert(offset);
    decls.idxs.insert(idx);
}

fn predeclare(decls: &mut ScopeDecls) {
    decls.first.insert("$this".to_string(), 0);
    for global in SUPERGLOBALS {
        decls.first.insert((*global).to_string(), 0);
    }
}

/// Collect params, closure-`use`, `static`, `global`, `catch`, and
/// `=`-lval declarations of one function body (not `foreach`).
fn collect_scope_decls(tree: &Tree, block: &Block, body: (usize, usize)) -> ScopeDecls {
    let mut decls = ScopeDecls::default();
    predeclare(&mut decls);
    for param in &block.params {
        declare(&mut decls, tree, param.name_idx);
    }
    if let Some((lp, rp)) = block.closure_use {
        for var in vars_between(tree, lp, rp) {
            declare(&mut decls, tree, var);
        }
    }
    decl_static_global(tree, body, &mut decls);
    decl_catch_vars(tree, body, &mut decls);
    decl_assign_lvals(tree, body, &mut decls);
    decls
}

/// `T_VARIABLE` indices strictly inside `(open, close)`.
fn vars_between(tree: &Tree, open: usize, close: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < close && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE {
            out.push(i);
        }
        i += 1;
    }
    out
}

fn decl_static_global(tree: &Tree, body: (usize, usize), decls: &mut ScopeDecls) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if kind != kinds::T_STATIC && kind != kinds::T_GLOBAL {
            i += 1;
            continue;
        }
        let Some(next) = tree.next_semantic(i) else {
            break;
        };
        if tree.tokens[next].kind == kinds::T_VARIABLE {
            for var in collect_decl_list(tree, next) {
                declare(decls, tree, var);
            }
        }
        i += 1;
    }
}

fn decl_catch_vars(tree: &Tree, body: (usize, usize), decls: &mut ScopeDecls) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind != kinds::T_CATCH {
            i += 1;
            continue;
        }
        if let Some(lparen) = tree.next_semantic(i)
            && tree.tokens[lparen].kind == "("
            && let Some(rparen) = tree.match_close(lparen)
        {
            for var in vars_between(tree, lparen, rparen) {
                declare(decls, tree, var);
            }
        }
        i += 1;
    }
}

fn decl_assign_lvals(tree: &Tree, body: (usize, usize), decls: &mut ScopeDecls) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == "="
            && let Some(lval) = tree.prev_semantic(i)
            && lval >= body.0
        {
            decl_assign_lval(tree, lval, decls);
        }
        i += 1;
    }
}

fn decl_assign_lval(tree: &Tree, lval: usize, decls: &mut ScopeDecls) {
    if tree.tokens[lval].kind == kinds::T_VARIABLE {
        if !is_dynamic_var(tree, lval) {
            declare(decls, tree, lval);
        }
        return;
    }
    if tree.tokens[lval].kind == ")"
        && let Some(open) = tree.match_open(lval)
        && tree
            .prev_semantic(open)
            .is_some_and(|p| tree.tokens[p].kind == kinds::T_LIST)
    {
        for var in vars_between(tree, open, lval) {
            declare(decls, tree, var);
        }
    }
    if tree.tokens[lval].kind == "]"
        && let Some(open) = tree.match_open(lval)
        && !is_index_bracket(tree, open)
    {
        for var in vars_between(tree, open, lval) {
            declare(decls, tree, var);
        }
    }
}

/// Per-`foreach` data for one function body (XHP5/XHP32).
struct ForeachScope {
    iters: Vec<usize>,
    end_offset: usize,
}

fn foreach_scopes(tree: &Tree, blocks: &[Block], body: (usize, usize)) -> Vec<ForeachScope> {
    let mut out = Vec::new();
    for idx in blocks_in_range(blocks, body, BlockKind::Foreach) {
        let block = &blocks[idx];
        let iters = foreach_iters(tree, block);
        let mut vars = Vec::new();
        if let Some(key) = iters.key {
            vars.push(key.idx);
        }
        if let Some(value) = iters.value {
            vars.push(value.idx);
        }
        let end = span_end_idx(tree, block);
        out.push(ForeachScope {
            iters: vars,
            end_offset: tree.tokens[end].offset,
        });
    }
    out
}

/// XHP5: uses before declaration in function/method bodies (one
/// finding per variable, at its first bad use).
fn rule_undeclared_variable(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function {
            continue;
        }
        let Some(body) = body_range(tree, block) else {
            continue;
        };
        if matches!(block.body, BodySpan::Empty { .. }) {
            continue;
        }
        undeclared_in_def(tree, &blocks, idx, body, out);
    }
}

fn undeclared_in_def(
    tree: &Tree,
    blocks: &[Block],
    def: usize,
    body: (usize, usize),
    out: &mut Vec<RawFinding>,
) {
    let mut decls = collect_scope_decls(tree, &blocks[def], body);
    let loops = foreach_scopes(tree, blocks, body);
    for scope in &loops {
        for iter in &scope.iters {
            declare(&mut decls, tree, *iter);
        }
    }
    let excluded = undeclared_excluded(tree, blocks, def, body);
    let destroyed = scope_destroyed_at(tree, body);
    let uses_token = undeclared_uses(tree, body, &decls, &excluded);
    let mut uses = uses_token;
    undeclared_string_uses(tree, blocks, def, body, &mut uses);
    emit_undeclared(&decls, destroyed, &uses, out);
}

/// Excluded `T_VARIABLE` indices: `isset`/`empty` args, static-access
/// parts, nested-closure and arrow-function bodies.
fn undeclared_excluded(
    tree: &Tree,
    blocks: &[Block],
    def: usize,
    body: (usize, usize),
) -> HashSet<usize> {
    let mut excluded = static_excluded_in(tree, body.0, body.1);
    for call in tree.calls() {
        let check =
            call.name.eq_ignore_ascii_case("isset") || call.name.eq_ignore_ascii_case("empty");
        if check && in_range(call.name_idx, body.0, body.1) {
            let end = call.rparen.unwrap_or(tree.tokens.len());
            excluded.extend(vars_between(tree, call.lparen, end));
        }
    }
    for range in nested_fn_ranges(tree, blocks, def, body, true) {
        excluded.extend(vars_between(tree, range.0, range.1));
    }
    excluded
}

/// Token spans of nested anonymous functions (and, when asked, arrow
/// functions) strictly inside a function body.
fn nested_fn_ranges(
    tree: &Tree,
    blocks: &[Block],
    def: usize,
    body: (usize, usize),
    arrows: bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (idx, block) in blocks.iter().enumerate() {
        if idx == def || !in_range(block.kw_idx, body.0, body.1) {
            continue;
        }
        let nested_closure = block.kind == BlockKind::Function && block.is_anonymous;
        let nested_arrow = arrows && block.kind == BlockKind::ArrowFn;
        if nested_closure || nested_arrow {
            out.push((block.kw_idx, span_end_idx(tree, block)));
        }
    }
    out
}

/// First offset where scope becomes unknowable (`extract()`, `$$x`
/// assignment, dynamic `global`), or `usize::MAX`.
fn scope_destroyed_at(tree: &Tree, body: (usize, usize)) -> usize {
    let mut destroyed = usize::MAX;
    for call in tree.calls() {
        if call.name.eq_ignore_ascii_case("extract") && in_range(call.name_idx, body.0, body.1) {
            destroyed = destroyed.min(tree.tokens[call.name_idx].offset);
        }
    }
    destroyed = destroyed.min(dynamic_global_at(tree, body));
    destroyed = destroyed.min(dynamic_assign_at(tree, body));
    destroyed
}

fn dynamic_global_at(tree: &Tree, body: (usize, usize)) -> usize {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_GLOBAL
            && let Some(next) = tree.next_semantic(i)
            && tree.tokens[next].kind != kinds::T_VARIABLE
        {
            return tree.tokens[next].offset;
        }
        i += 1;
    }
    usize::MAX
}

fn dynamic_assign_at(tree: &Tree, body: (usize, usize)) -> usize {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == "="
            && let Some(lval) = tree.prev_semantic(i)
            && lval >= body.0
            && tree.tokens[lval].kind == kinds::T_VARIABLE
            && is_dynamic_var(tree, lval)
        {
            return tree.tokens[lval].offset;
        }
        i += 1;
    }
    usize::MAX
}

/// Ordered `(offset, name)` token uses (declarations and exclusions
/// skipped).
fn undeclared_uses(
    tree: &Tree,
    body: (usize, usize),
    decls: &ScopeDecls,
    excluded: &HashSet<usize>,
) -> Vec<(usize, String)> {
    let mut uses = Vec::new();
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE
            && !decls.idxs.contains(&i)
            && !excluded.contains(&i)
        {
            uses.push((tree.tokens[i].offset, tree.tokens[i].value.clone()));
        }
        i += 1;
    }
    uses
}

/// Append interpolated-string variable uses (outside nested closures).
fn undeclared_string_uses(
    tree: &Tree,
    blocks: &[Block],
    def: usize,
    body: (usize, usize),
    uses: &mut Vec<(usize, String)>,
) {
    let nested = nested_fn_ranges(tree, blocks, def, body, true);
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        let interpolated = kind == kinds::T_INTERPOLATED_STRING || kind == kinds::T_HEREDOC;
        if interpolated && !nested.iter().any(|r| in_range(i, r.0, r.1)) {
            for (rel, name) in string_variables(&tree.tokens[i].value) {
                uses.push((tree.tokens[i].offset + rel, format!("${name}")));
            }
        }
        i += 1;
    }
}

fn emit_undeclared(
    decls: &ScopeDecls,
    destroyed: usize,
    uses: &[(usize, String)],
    out: &mut Vec<RawFinding>,
) {
    let mut warned = HashSet::new();
    for (offset, name) in uses {
        if *offset >= destroyed {
            continue;
        }
        let declared_at = decls
            .first
            .get(name.as_str())
            .copied()
            .unwrap_or(usize::MAX);
        if *offset >= declared_at {
            continue;
        }
        if !warned.insert(name.clone()) {
            continue;
        }
        push(out, 5, *offset, undeclared_message());
    }
}

fn undeclared_message() -> &'static str {
    "Declare variables prior to use (even if you are passing them \
     as reference parameters). You may have misspelled this \
     variable name."
}

/// XHP32: `foreach` iterators that reuse a previously declared local
/// which is also used after the loop.
fn rule_reused_as_iterator(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for block in &blocks {
        if block.kind != BlockKind::Function {
            continue;
        }
        let Some(body) = body_range(tree, block) else {
            continue;
        };
        if matches!(block.body, BodySpan::Empty { .. }) {
            continue;
        }
        reused_as_in_def(tree, &blocks, block, body, out);
    }
}

fn reused_as_in_def(
    tree: &Tree,
    blocks: &[Block],
    def: &Block,
    body: (usize, usize),
    out: &mut Vec<RawFinding>,
) {
    let decls = collect_scope_decls(tree, def, body);
    let loops = foreach_scopes(tree, blocks, body);
    let mut uses = all_var_uses(tree, body);
    for scope in &loops {
        remove_loop_uses(tree, scope, &mut uses);
    }
    for scope in &loops {
        for iter in &scope.iters {
            check_reused_iterator(tree, &decls, &uses, *iter, out);
        }
    }
}

/// Every `T_VARIABLE` occurrence in range, by name (declarations and
/// exclusions included, mirroring PHP's `$uses` map).
fn all_var_uses(tree: &Tree, body: (usize, usize)) -> HashMap<String, Vec<usize>> {
    let mut uses: HashMap<String, Vec<usize>> = HashMap::new();
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE {
            uses.entry(tree.tokens[i].value.clone())
                .or_default()
                .push(tree.tokens[i].offset);
        }
        i += 1;
    }
    uses
}

/// Drop uses of a loop's iterators inside `[iter_offset, loop_end)`.
fn remove_loop_uses(tree: &Tree, scope: &ForeachScope, uses: &mut HashMap<String, Vec<usize>>) {
    for iter in &scope.iters {
        let name = &tree.tokens[*iter].value;
        let start = tree.tokens[*iter].offset;
        if let Some(list) = uses.get_mut(name) {
            list.retain(|use_offset| *use_offset < start || *use_offset >= scope.end_offset);
        } // LCOV_EXCL_LINE // Reason: brace carries no llvm count though the block executes
    }
}

fn check_reused_iterator(
    tree: &Tree,
    decls: &ScopeDecls,
    uses: &HashMap<String, Vec<usize>>,
    iter: usize,
    out: &mut Vec<RawFinding>,
) {
    let name = &tree.tokens[iter].value;
    let offset = tree.tokens[iter].offset;
    let Some(declared) = decls.first.get(name.as_str()).copied() else {
        return;
    };
    if declared >= offset {
        return;
    }
    let used_after = uses
        .get(name.as_str())
        .is_some_and(|list| list.iter().any(|use_offset| *use_offset > offset));
    if used_after {
        push(out, 32, offset, reused_as_iterator_message());
    }
}

fn reused_as_iterator_message() -> &'static str {
    "This iterator variable is a previously declared local \
     variable. To avoid overwriting locals, do not reuse them \
     as iterator variables."
}

/// XHP9: naming conventions for classes, functions, constants,
/// properties, parameters, and variables (default determinations;
/// the naming-hook override cannot run here).
fn rule_naming_conventions(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    naming_declarations(tree, &blocks, out);
    let params = naming_params(tree, &blocks, out);
    let consts = collect_consts(tree, &blocks);
    naming_class_consts(tree, &consts, out);
    let members = naming_members(tree, &blocks, out);
    naming_def_vars(tree, &blocks, &params, &members, out);
    naming_defines(tree, &consts, out);
}

fn naming_declarations(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    for (idx, block) in blocks.iter().enumerate() {
        match block.kind {
            BlockKind::Class => naming_symbol(tree, block, "classes", "UpperCamelCase", out),
            BlockKind::Interface => {
                naming_symbol(tree, block, "interfaces", "UpperCamelCase", out);
            }
            BlockKind::Function => naming_function(tree, blocks, idx, out),
            _ => {}
        }
    }
}

fn naming_symbol(
    tree: &Tree,
    block: &Block,
    what: &str,
    convention: &str,
    out: &mut Vec<RawFinding>,
) {
    let (Some(name), Some(name_idx)) = (block.name.as_ref(), block.name_idx) else {
        return;
    };
    if !is_upper_camel(name) {
        let message =
            format!("Follow naming conventions: {what} should be named using `{convention}`.");
        push(out, 9, tree.tokens[name_idx].offset, &message);
    }
}

fn naming_function(tree: &Tree, blocks: &[Block], idx: usize, out: &mut Vec<RawFinding>) {
    let block = &blocks[idx];
    let (Some(name), Some(name_idx)) = (block.name.as_ref(), block.name_idx) else {
        return;
    };
    let stripped = strip_php_function(name);
    if tree.is_method(blocks, idx) {
        if !is_lower_camel(&stripped) {
            push(
                out,
                9,
                tree.tokens[name_idx].offset,
                naming_method_message(),
            );
        }
    } else if !is_lower_under(&stripped) {
        push(
            out,
            9,
            tree.tokens[name_idx].offset,
            naming_function_message(),
        );
    }
}

fn naming_method_message() -> &'static str {
    "Follow naming conventions: methods should be named using `lowerCamelCase`."
}

fn naming_function_message() -> &'static str {
    "Follow naming conventions: functions should be named using \
     `lowercase_with_underscores`."
}

/// Check every parameter; return the parameter token indices.
fn naming_params(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) -> HashSet<usize> {
    let mut idxs = HashSet::new();
    for block in blocks {
        if block.kind != BlockKind::Function && block.kind != BlockKind::ArrowFn {
            continue;
        }
        for param in &block.params {
            idxs.insert(param.name_idx);
            if !is_lower_under(&strip_php_variable(&param.name)) {
                push(
                    out,
                    9,
                    tree.tokens[param.name_idx].offset,
                    naming_param_message(),
                );
            }
        }
    }
    idxs
}

fn naming_param_message() -> &'static str {
    "Follow naming conventions: parameters should be named using \
     `lowercase_with_underscores`"
}

/// One `const` declarator: name token, name, multi-ness, class scope.
struct ConstDecl {
    idx: usize,
    name: String,
    multi: bool,
    in_class: bool,
}

/// All `const` declarators outside function bodies.
fn collect_consts(tree: &Tree, blocks: &[Block]) -> Vec<ConstDecl> {
    let fns = fn_spans(tree, blocks);
    let classes = class_body_spans(tree, blocks);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tree.tokens.len() {
        if tree.tokens[i].kind != kinds::T_CONST || inside_any(i, &fns) {
            i += 1;
            continue;
        }
        let names = parse_const_stmt(tree, i);
        let in_class = inside_any(i, &classes);
        let multi = names.len() > 1;
        for (idx, name) in names {
            out.push(ConstDecl {
                idx,
                name,
                multi,
                in_class,
            });
        }
        i += 1;
    }
    out
}

/// `(name token, name)` declarators of one `const` statement.
fn parse_const_stmt(tree: &Tree, const_kw: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut scan = ConstScan {
        expect_name: true,
        ..ConstScan::default()
    };
    let mut i = const_kw + 1;
    while i < tree.tokens.len() && !scan.done {
        const_scan_step(tree, i, &mut scan, &mut out);
        i += 1;
    }
    out
}

#[derive(Debug, Default)]
struct ConstScan {
    depth: u32,
    expect_name: bool,
    done: bool,
}

fn const_scan_step(tree: &Tree, i: usize, scan: &mut ConstScan, out: &mut Vec<(usize, String)>) {
    let kind = tree.tokens[i].kind.as_str();
    if is_open_kind(kind) {
        scan.depth += 1;
        return;
    }
    if is_close_kind(kind) {
        const_scan_close(scan);
        return;
    }
    if scan.depth == 0 {
        const_scan_top(tree, i, kind, scan, out);
    }
}

fn const_scan_close(scan: &mut ConstScan) {
    if scan.depth == 0 {
        scan.done = true;
    } else {
        scan.depth -= 1;
    }
}

fn const_scan_top(
    tree: &Tree,
    i: usize,
    kind: &str,
    scan: &mut ConstScan,
    out: &mut Vec<(usize, String)>,
) {
    if kind == ";" {
        scan.done = true;
    } else if kind == "," {
        scan.expect_name = true;
    } else if scan.expect_name {
        const_scan_name(tree, i, kind, scan, out);
    }
}

fn const_scan_name(
    tree: &Tree,
    i: usize,
    kind: &str,
    scan: &mut ConstScan,
    out: &mut Vec<(usize, String)>,
) {
    if kind == kinds::T_STRING {
        out.push((i, tree.tokens[i].value.clone()));
        scan.expect_name = false;
    } else if is_semantic(&tree.tokens[i]) {
        scan.done = true;
    }
}

/// Multi-declarator class consts get the convention check.
fn naming_class_consts(tree: &Tree, consts: &[ConstDecl], out: &mut Vec<RawFinding>) {
    for decl in consts {
        if decl.in_class && decl.multi && !is_upper_under(&decl.name) {
            push(out, 9, tree.tokens[decl.idx].offset, naming_const_message());
        }
    }
}

fn naming_const_message() -> &'static str {
    "Follow naming conventions: class constants should be named using \
     `UPPERCASE_WITH_UNDERSCORES`"
}

/// Check class properties; return the property token indices.
fn naming_members(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) -> HashSet<usize> {
    let fns = fn_spans(tree, blocks);
    let classes = class_body_spans(tree, blocks);
    let mut idxs = HashSet::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_VARIABLE {
            continue;
        }
        if !inside_any(i, &classes) || inside_any(i, &fns) {
            continue;
        }
        idxs.insert(i);
        if !is_lower_camel(&strip_php_variable(&token.value)) {
            push(out, 9, token.offset, naming_member_message());
        }
    }
    idxs
}

fn naming_member_message() -> &'static str {
    "Follow naming conventions: class properties should be named using \
     `lowerCamelCase`."
}

/// Brace-body spans of class-like blocks.
fn class_body_spans(tree: &Tree, blocks: &[Block]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for block in blocks {
        let class_like = matches!(
            block.kind,
            BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
        );
        if class_like && let BodySpan::Brace { open, close } = block.body {
            out.push((open, close.unwrap_or(tree.tokens.len())));
        }
    }
    out
}

/// Full spans of function and arrow-function blocks.
fn fn_spans(tree: &Tree, blocks: &[Block]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for block in blocks {
        if block.kind == BlockKind::Function || block.kind == BlockKind::ArrowFn {
            out.push((block.kw_idx, span_end_idx(tree, block)));
        }
    }
    out
}

fn inside_any(idx: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|span| in_range(idx, span.0, span.1))
}

/// Per-definition variable checks (globals exempt by name).
fn naming_def_vars(
    tree: &Tree,
    blocks: &[Block],
    params: &HashSet<usize>,
    members: &HashSet<usize>,
    out: &mut Vec<RawFinding>,
) {
    for block in blocks {
        if block.kind != BlockKind::Function {
            continue;
        }
        let Some(body) = body_range(tree, block) else {
            continue;
        };
        let globals = def_global_names(tree, body);
        let statics = static_excluded_in(tree, body.0, body.1);
        naming_def_vars_in(tree, body, &globals, params, members, &statics, out);
    }
}

fn naming_def_vars_in(
    tree: &Tree,
    body: (usize, usize),
    globals: &HashSet<String>,
    params: &HashSet<usize>,
    members: &HashSet<usize>,
    statics: &HashSet<usize>,
    out: &mut Vec<RawFinding>,
) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE
            && !params.contains(&i)
            && !members.contains(&i)
            && !statics.contains(&i)
        {
            naming_variable(tree, i, globals, out);
        }
        i += 1;
    }
}

fn naming_variable(tree: &Tree, idx: usize, globals: &HashSet<String>, out: &mut Vec<RawFinding>) {
    let name = tree.tokens[idx].value.trim_matches(['{', '}']);
    if is_superglobal(name) || globals.contains(name) {
        return;
    }
    if !is_lower_under(&strip_php_variable(name)) {
        push(out, 9, tree.tokens[idx].offset, naming_variable_message());
    }
}

fn naming_variable_message() -> &'static str {
    "Follow naming conventions: variables should be named using \
     `lowercase_with_underscores`."
}

/// Names declared via `global` in one body.
fn def_global_names(tree: &Tree, body: (usize, usize)) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_GLOBAL
            && let Some(next) = tree.next_semantic(i)
            && tree.tokens[next].kind == kinds::T_VARIABLE
        {
            for var in collect_decl_list(tree, next) {
                out.insert(tree.tokens[var].value.clone());
            }
        }
        i += 1;
    }
    out
}

/// `define()` names plus every `const` declarator must be uppercase.
fn naming_defines(tree: &Tree, consts: &[ConstDecl], out: &mut Vec<RawFinding>) {
    for call in tree.calls() {
        if !call.name.eq_ignore_ascii_case("define") || call.args.is_empty() {
            continue;
        }
        let (first, last) = call.args[0];
        if first == last && tree.tokens[first].kind == kinds::T_CONSTANT_ENCAPSED_STRING {
            let name = &tree.tokens[first].value;
            if *name != ascii_upper(name) {
                push(
                    out,
                    9,
                    tree.tokens[first].offset,
                    naming_uppercase_message(),
                );
            }
        }
    }
    for decl in consts {
        if decl.name != ascii_upper(&decl.name) {
            push(
                out,
                9,
                tree.tokens[decl.idx].offset,
                naming_uppercase_message(),
            );
        }
    }
}

fn naming_uppercase_message() -> &'static str {
    "Constants should be uppercase."
}

/// ASCII-only uppercase (mirrors PHP `strtoupper()`).
fn ascii_upper(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

fn is_upper_camel(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

fn is_lower_camel(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

fn is_upper_under(symbol: &str) -> bool {
    !symbol.is_empty()
        && symbol
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn is_lower_under(symbol: &str) -> bool {
    !symbol.is_empty()
        && symbol
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Strip the `__` magic-method signifier for known magic names.
fn strip_php_function(symbol: &str) -> String {
    const MAGIC: &[&str] = &[
        "__assign_concat",
        "__call",
        "__callStatic",
        "__clone",
        "__concat",
        "__construct",
        "__debugInfo",
        "__destruct",
        "__get",
        "__invoke",
        "__isset",
        "__set",
        "__set_state",
        "__sleep",
        "__toString",
        "__unset",
        "__wakeup",
    ];
    if MAGIC.contains(&symbol) {
        symbol.trim_start_matches('_').to_string()
    } else {
        symbol.to_string()
    }
}

/// Strip the leading `$` from a variable name.
fn strip_php_variable(symbol: &str) -> String {
    symbol.strip_prefix('$').unwrap_or(symbol).to_string()
}

/// XHP10: methods named like their class are implicit constructors.
fn rule_implicit_constructor(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for class in &blocks {
        if class.kind != BlockKind::Class {
            continue;
        }
        let Some(class_name) = class.name.as_ref() else {
            continue;
        };
        let end = span_end_idx(tree, class);
        for method in &blocks {
            if method.kind != BlockKind::Function {
                continue;
            }
            check_implicit_ctor(tree, class_name, class.kw_idx, end, method, out);
        }
    }
}

fn check_implicit_ctor(
    tree: &Tree,
    class_name: &str,
    class_kw: usize,
    class_end: usize,
    method: &Block,
    out: &mut Vec<RawFinding>,
) {
    if !in_range(method.kw_idx, class_kw, class_end) || method.kw_idx == class_kw {
        return;
    }
    let (Some(name), Some(name_idx)) = (method.name.as_ref(), method.name_idx) else {
        return;
    };
    if name.eq_ignore_ascii_case(class_name) {
        push(
            out,
            10,
            tree.tokens[name_idx].offset,
            implicit_ctor_message(),
        );
    }
}

fn implicit_ctor_message() -> &'static str {
    "Name constructors `__construct` explicitly. This method is a \
     constructor because it has the same name as the class \
     it is defined in."
}

/// XHP13: `$this` inside static methods.
fn rule_static_this(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function || !tree.is_method(&blocks, idx) {
            continue;
        }
        if !method_in_class(tree, &blocks, idx) {
            continue;
        }
        if !is_static_concrete(block) {
            continue;
        }
        let Some(body) = body_range(tree, block) else {
            continue;
        };
        static_this_in(tree, body, out);
    }
}

/// True when a method's nearest class-like ancestor is a class.
fn method_in_class(tree: &Tree, blocks: &[Block], idx: usize) -> bool {
    tree.enclosing_class(blocks, idx)
        .is_some_and(|c| blocks[c].kind == BlockKind::Class)
}

fn is_static_concrete(block: &Block) -> bool {
    let mut is_static = false;
    for modifier in &block.modifiers {
        if modifier.eq_ignore_ascii_case("abstract") {
            return false;
        }
        if modifier.eq_ignore_ascii_case("static") {
            is_static = true;
        }
    }
    is_static
}

fn static_this_in(tree: &Tree, body: (usize, usize), out: &mut Vec<RawFinding>) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE
            && tree.tokens[i].value.eq_ignore_ascii_case("$this")
        {
            push(out, 13, tree.tokens[i].offset, static_this_message());
        }
        i += 1;
    }
}

fn static_this_message() -> &'static str {
    "You can not reference `$this` inside a static method."
}

/// XHP17: `exit` used as an expression (call form and bare
/// statement form are fine; `die` is never flagged).
fn rule_exit_expression(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_EXIT || !token.value.eq_ignore_ascii_case("exit") {
            continue;
        }
        if !exit_is_statement(tree, &blocks, i) {
            push(out, 17, token.offset, exit_expression_message());
        }
    }
}

fn exit_expression_message() -> &'static str {
    "Use `exit` as a statement, not an expression."
}

/// Call form or statement-position `exit`.
fn exit_is_statement(tree: &Tree, blocks: &[Block], exit: usize) -> bool {
    match tree.next_semantic(exit) {
        Some(next) if tree.tokens[next].kind == "(" => return true,
        Some(next)
            if tree.tokens[next].kind != ";" && tree.tokens[next].kind != kinds::T_CLOSE_TAG =>
        {
            return false;
        }
        _ => {}
    }
    // `exit` never leads the file (`T_EXIT` needs PHP mode, which
    // needs an opener first), so the `None` arm is unreachable.
    tree.prev_semantic(exit)
        .is_none_or(|prev| exit_prev_ok(tree, blocks, prev))
}

fn exit_prev_ok(tree: &Tree, blocks: &[Block], prev: usize) -> bool {
    match tree.tokens[prev].kind.as_str() {
        ";" | "{" | "}" | kinds::T_OPEN_TAG | kinds::T_CLOSE_TAG | kinds::T_ELSE | kinds::T_DO => {
            true
        }
        ":" => exit_colon_ok(tree, prev),
        ")" => exit_rparen_ok(blocks, prev),
        _ => false,
    }
}

/// A `:` before `exit` is statement position unless it is a ternary
/// colon (a depth-0 `?` sits before it).
fn exit_colon_ok(tree: &Tree, colon: usize) -> bool {
    let mut depth = 0;
    let mut i = colon;
    while let Some(prev) = prev_before(tree, &mut i) {
        match tree.tokens[prev].kind.as_str() {
            "?" if depth == 0 => return false,
            ";"
            | "{"
            | "}"
            | kinds::T_CASE
            | kinds::T_DEFAULT
            | kinds::T_OPEN_TAG
            | kinds::T_CLOSE_TAG
            | kinds::T_IF
            | kinds::T_ELSEIF
            | kinds::T_WHILE
            | kinds::T_FOR
            | kinds::T_FOREACH
            | kinds::T_SWITCH
            | kinds::T_DECLARE
            | kinds::T_FUNCTION => {
                return true;
            }
            ")" | "]" => depth += 1,
            "(" | "[" if depth > 0 => depth -= 1,
            _ => {}
        }
    }
    true
}

fn prev_before(tree: &Tree, i: &mut usize) -> Option<usize> {
    let prev = tree.prev_semantic(*i)?;
    *i = prev;
    Some(prev)
}

/// A `)` before `exit` is statement position when it closes a
/// control header that takes a statement body.
fn exit_rparen_ok(blocks: &[Block], rparen: usize) -> bool {
    blocks.iter().any(|block| {
        block.rparen == Some(rparen)
            && [
                BlockKind::If,
                BlockKind::ElseIf,
                BlockKind::While,
                BlockKind::For,
                BlockKind::Foreach,
                BlockKind::Declare,
            ]
            .contains(&block.kind)
    })
}

/// XHP19: a lone class/interface declaration must match the filename.
fn rule_class_filename_mismatch(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    let mut found: Vec<(String, usize)> = Vec::new();
    for block in &blocks {
        if block.kind != BlockKind::Class && block.kind != BlockKind::Interface {
            continue;
        }
        if let (Some(name), Some(name_idx)) = (block.name.as_ref(), block.name_idx) {
            found.push((name.clone(), name_idx));
        }
    }
    if found.len() != 1 {
        return;
    }
    let (name, name_idx) = &found[0];
    if !is_word(name) {
        return;
    }
    let rename = format!("{name}.php");
    if rename == file_basename(ctx.path) {
        return;
    }
    let message = format!(
        "The name of this file differs from the name of the \
         class or interface it declares. Rename the file to `{rename}`."
    );
    push(out, 19, tree.tokens[*name_idx].offset, &message);
}

fn is_word(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn file_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// XHP20: identical operands around comparison-ish operators, and
/// statically true/false sides of `&&` / `||`.
fn rule_tautological_expression(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if is_identical_op(&token.kind, &token.value) {
            check_identical_operands(tree, i, out);
        } else if token.kind == kinds::T_BOOLEAN_AND || token.kind == kinds::T_BOOLEAN_OR {
            check_static_logic(tree, i, out);
        }
    }
}

/// PHP's `$operators` set (`<>` deliberately excluded).
fn is_identical_op(kind: &str, value: &str) -> bool {
    matches!(
        (kind, value),
        ("-", "-")
            | ("/", "/")
            | ("-=", "-=")
            | ("/=", "/=")
            | ("<=", "<=")
            | ("<", "<")
            | (kinds::T_IS_EQUAL, "==")
            | (kinds::T_IS_IDENTICAL, "===")
            | (kinds::T_IS_NOT_EQUAL, "!=")
            | (kinds::T_IS_NOT_IDENTICAL, "!==")
            | (">=", ">=")
            | (">", ">")
    )
}

fn check_identical_operands(tree: &Tree, op: usize, out: &mut Vec<RawFinding>) {
    let (Some(left), Some(right)) = (
        scan_operand_left(tree, op, true),
        scan_operand_right(tree, op, true),
    ) else {
        return;
    };
    if semantic_text(tree, left.0, left.1) == semantic_text(tree, right.0, right.1) {
        push(out, 20, tree.tokens[left.0].offset, identical_message());
    }
}

fn identical_message() -> &'static str {
    "Both sides of this expression are identical, so it always \
     evaluates to a constant."
}

fn check_static_logic(tree: &Tree, op: usize, out: &mut Vec<RawFinding>) {
    let (Some(left), Some(right)) = (
        scan_operand_left(tree, op, false),
        scan_operand_right(tree, op, false),
    ) else {
        return;
    };
    let lval = static_boolean(&semantic_text(tree, left.0, left.1));
    let rval = static_boolean(&semantic_text(tree, right.0, right.1));
    let is_or = tree.tokens[op].kind == kinds::T_BOOLEAN_OR;
    let or_fires = is_or && (lval == Some(true) || rval == Some(true));
    let and_fires = !is_or && (lval == Some(false) || rval == Some(false));
    if or_fires || and_fires {
        push(out, 20, tree.tokens[left.0].offset, static_logic_message());
    }
}

/// Mirrors `evaluateStaticBoolean()` (`None` = not statically known).
fn static_boolean(semantic: &str) -> Option<bool> {
    match semantic.to_ascii_lowercase().as_str() {
        "0" | "null" | "false" => Some(false),
        "1" | "true" => Some(true),
        _ => None,
    }
}

fn static_logic_message() -> &'static str {
    "The logical value of this expression is static. \
     Did you forget to remove some debugging code?"
}

/// XHP22: duplicate keys in array initializers (one finding per
/// duplicated key, at its last occurrence).
fn rule_duplicate_keys(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (open, close) in array_literals(tree) {
        duplicate_keys_in(tree, open, close, out);
    }
}

fn duplicate_keys_in(tree: &Tree, open: usize, close: usize, out: &mut Vec<RawFinding>) {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (first, last) in tree.split_top_level(open, close) {
        if let Some((key, at)) = array_entry_key(tree, first, last) {
            groups.entry(key).or_default().push(at);
        }
    }
    let mut dups: Vec<usize> = groups
        .values()
        .filter(|nodes| nodes.len() > 1)
        .map(|nodes| nodes[nodes.len() - 1])
        .collect();
    dups.sort_unstable();
    for at in dups {
        push(out, 22, tree.tokens[at].offset, duplicate_key_message());
    }
}

fn duplicate_key_message() -> &'static str {
    "Duplicate key in array initializer. \
     PHP will ignore all but the last entry."
}

/// Grouping key plus key-start token for one array entry.
fn array_entry_key(tree: &Tree, first: usize, last: usize) -> Option<(String, usize)> {
    let arrow = scan_top_level(
        tree,
        first.saturating_sub(1),
        last + 1,
        kinds::T_DOUBLE_ARROW,
    )?;
    if arrow == 0 {
        return None; // LCOV_EXCL_LINE // Reason: dead: entries start after an opener, so a found arrow is never at index 0
    }
    let (kfirst, klast) = trim_semantic(tree, first, arrow - 1)?;
    key_group(tree, kfirst, klast).map(|key| (key, kfirst))
}

fn key_group(tree: &Tree, first: usize, last: usize) -> Option<String> {
    if first == last {
        return single_key_group(tree, first);
    }
    static_key_group(tree, first, last)
}

fn single_key_group(tree: &Tree, idx: usize) -> Option<String> {
    let token = &tree.tokens[idx];
    match token.kind.as_str() {
        kinds::T_CONSTANT_ENCAPSED_STRING => {
            unquote_plain_string(&token.value).map(|text| format!("scalar:{text}"))
        }
        kinds::T_LNUMBER => Some(format!("scalar:{}", normalize_int(&token.value))),
        kinds::T_DNUMBER => Some(format!("scalar:{}", normalize_float(&token.value))),
        kinds::T_STRING => Some(format!("symbol:{}", token.value)),
        kinds::T_VARIABLE => Some(format!("var:{}", token.value)),
        _ => None,
    }
}

/// Multi-token `X::...` keys group by exact source slice.
fn static_key_group(tree: &Tree, first: usize, last: usize) -> Option<String> {
    let mut i = first;
    while i <= last {
        if tree.tokens[i].kind == kinds::T_PAAMAYIM_NEKUDOTAYIM {
            return Some(format!("static:{}", tree.span_text(first, last)));
        }
        i += 1;
    }
    None
}

/// Integer literal value as PHP `evalStatic()` would stringify it.
fn normalize_int(value: &str) -> String {
    let text: String = value.chars().filter(|c| *c != '_').collect();
    let lower = text.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        return u64_from_radix(hex, 16).map_or(text, |n| n.to_string());
    }
    if let Some(bin) = lower.strip_prefix("0b") {
        return u64_from_radix(bin, 2).map_or(text, |n| n.to_string());
    }
    if lower.len() > 1 && lower.starts_with('0') && lower.bytes().all(|b| b.is_ascii_digit()) {
        if lower.bytes().all(|b| b < b'8') {
            return u64_from_radix(&lower, 8).map_or(text, |n| n.to_string());
        }
        return text;
    }
    text
}

fn u64_from_radix(text: &str, radix: u32) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    u64::from_str_radix(text, radix).ok()
}

/// Float literal value as PHP would stringify it.
fn normalize_float(value: &str) -> String {
    let text: String = value.chars().filter(|c| *c != '_').collect();
    let Ok(num) = text.parse::<f64>() else {
        return text;
    };
    if num.is_finite() && num.fract() == 0.0 && num.abs() < 1e18 {
        format!("{}", num as i64)
    } else if num.is_finite() {
        format!("{num}")
    } else {
        text
    }
}

/// XHP23: nested `for` / `foreach` loops reusing iterator variables.
fn rule_reused_iterator(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    let loops = loop_var_maps(tree, &blocks);
    for outer in &loops {
        for inner in &loops {
            check_loop_pair(tree, outer, inner, out);
        }
    }
}

/// One loop's iterator names (in order) plus its token span.
struct LoopVars {
    names: Vec<String>,
    kw: usize,
    end: usize,
    expr_at: usize,
}

fn loop_var_maps(tree: &Tree, blocks: &[Block]) -> Vec<LoopVars> {
    let mut out = Vec::new();
    for block in blocks {
        if block.kind != BlockKind::For && block.kind != BlockKind::Foreach {
            continue;
        }
        out.push(LoopVars {
            names: loop_iter_names(tree, block),
            kw: block.kw_idx,
            end: span_end_idx(tree, block),
            expr_at: block.lparen.unwrap_or(block.kw_idx),
        });
    }
    out
}

fn loop_iter_names(tree: &Tree, block: &Block) -> Vec<String> {
    let mut names = Vec::new();
    if block.kind == BlockKind::For {
        for (name, _) in for_assigned_vars(tree, block) {
            push_unique(&mut names, name);
        }
        return names;
    }
    let iters = foreach_iters(tree, block);
    for iter in [iters.key, iters.value].into_iter().flatten() {
        push_unique(&mut names, tree.tokens[iter.idx].value.clone());
    }
    names
}

fn push_unique(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

fn check_loop_pair(tree: &Tree, outer: &LoopVars, inner: &LoopVars, out: &mut Vec<RawFinding>) {
    if outer.kw == inner.kw || !loop_contains(outer, inner) {
        return;
    }
    let shared: Vec<&str> = outer
        .names
        .iter()
        .filter(|name| inner.names.iter().any(|other| other == *name))
        .map(String::as_str)
        .collect();
    if shared.is_empty() {
        return;
    }
    let message = format!(
        "This loop reuses iterator variables ({}) from an \
         outer loop. You might be clobbering the outer iterator. \
         Change the inner loop to use a different iterator name.",
        shared.join(", ")
    );
    push(out, 23, tree.tokens[inner.expr_at].offset, &message);
}

fn loop_contains(outer: &LoopVars, inner: &LoopVars) -> bool {
    outer.kw < inner.kw && inner.end <= outer.end
}

/// XHP24: opening braces share the line with a single preceding
/// space; single-statement bodies need braces; `else`/`elseif` hug
/// the closing brace.
fn rule_brace_formatting(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for block in &blocks {
        brace_open_gap(tree, block, out);
        brace_single_body(tree, block, out);
        brace_else_gap(tree, block, out);
    }
    brace_bare_blocks(tree, &blocks, out);
}

/// Statement braces without a keyword owner (bare blocks, `match`):
/// same gap check, index/deref/group-use positions excluded.
fn brace_bare_blocks(tree: &Tree, blocks: &[Block], out: &mut Vec<RawFinding>) {
    let mut owned = HashSet::new();
    for block in blocks {
        if let BodySpan::Brace { open, .. } = block.body {
            owned.insert(open);
        }
    }
    for (open, token) in tree.tokens.iter().enumerate() {
        if token.kind != "{" || owned.contains(&open) {
            continue;
        }
        let Some(prev) = tree.prev_semantic(open) else {
            continue;
        };
        if matches!(
            tree.tokens[prev].kind.as_str(),
            "$" | "\\" | kinds::T_VARIABLE | kinds::T_STRING | ")" | "]"
        ) {
            continue;
        }
        check_single_space_gap(tree, prev, open, tree.tokens[open].offset, out);
    }
}

/// Statement-list `{` needs exactly one preceding space.
fn brace_open_gap(tree: &Tree, block: &Block, out: &mut Vec<RawFinding>) {
    let BodySpan::Brace { open, .. } = block.body else {
        return;
    };
    let Some(prev) = tree.prev_semantic(open) else {
        push(out, 24, tree.tokens[open].offset, brace_open_message());
        return;
    };
    check_single_space_gap(tree, prev, open, tree.tokens[open].offset, out);
}

/// Shared `before`-gap check: empty or not-exactly-one-space flags.
fn check_single_space_gap(
    tree: &Tree,
    prev: usize,
    at: usize,
    empty_at: usize,
    out: &mut Vec<RawFinding>,
) {
    if at == prev + 1 {
        push(out, 24, empty_at, brace_open_message());
    } else if at == prev + 2 && tree.tokens[prev + 1].value != " " {
        push(out, 24, tree.tokens[prev + 1].offset, brace_open_message());
    }
}

fn brace_open_message() -> &'static str {
    "Put opening braces on the same line as control statements and \
     declarations, with a single space before them."
}

/// Single-statement control bodies need braces (except when the body
/// is itself a declaration or control structure).
fn brace_single_body(tree: &Tree, block: &Block, out: &mut Vec<RawFinding>) {
    if !matches!(
        block.kind,
        BlockKind::If
            | BlockKind::ElseIf
            | BlockKind::Else
            | BlockKind::While
            | BlockKind::DoWhile
            | BlockKind::For
            | BlockKind::Foreach
    ) {
        return;
    }
    let BodySpan::Single { first, .. } = block.body else {
        return;
    };
    if is_structured_body(tree.tokens[first].kind.as_str()) {
        return;
    }
    push(out, 24, tree.tokens[first].offset, brace_block_message());
}

fn is_structured_body(kind: &str) -> bool {
    matches!(
        kind,
        kinds::T_IF
            | kinds::T_WHILE
            | kinds::T_FOR
            | kinds::T_FOREACH
            | kinds::T_SWITCH
            | kinds::T_DO
            | kinds::T_TRY
            | kinds::T_FUNCTION
            | kinds::T_CLASS
            | kinds::T_INTERFACE
            | kinds::T_TRAIT
            | kinds::T_ENUM
            | kinds::T_DECLARE
    )
}

fn brace_block_message() -> &'static str {
    "Use braces to surround a statement block."
}

/// `else` / `elseif` ending with `}` hugs the previous token.
fn brace_else_gap(tree: &Tree, block: &Block, out: &mut Vec<RawFinding>) {
    if block.kind != BlockKind::Else && block.kind != BlockKind::ElseIf {
        return;
    }
    let Some(last) = tree.body_last(block) else {
        return;
    };
    if tree.tokens[last].kind != "}" {
        return;
    }
    let Some(prev) = tree.prev_semantic(block.kw_idx) else {
        return;
    };
    check_single_space_gap(
        tree,
        prev,
        block.kw_idx,
        tree.tokens[block.kw_idx].offset,
        out,
    );
}

/// XHP30: nonempty `case` / `default` arms need a top-level
/// terminator or a `fall...` comment (default determination; the
/// switch-hook override cannot run here).
fn rule_implicit_fallthrough(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for block in &blocks {
        if block.kind != BlockKind::Switch {
            continue;
        }
        for (n, case) in block.cases.iter().enumerate() {
            let end = case_range_end(tree, block, n);
            if !arm_is_ok(tree, &blocks, case.kw_idx, end) {
                push(
                    out,
                    30,
                    tree.tokens[case.kw_idx].offset,
                    fallthrough_message(),
                );
            }
        }
    }
}

/// Exclusive end of arm `n`: next arm keyword or switch-body end.
fn case_range_end(tree: &Tree, block: &Block, n: usize) -> usize {
    if let Some(next) = block.cases.get(n + 1) {
        return next.kw_idx;
    }
    match block.body {
        BodySpan::Brace { close, .. } => close.unwrap_or(tree.tokens.len()),
        BodySpan::Colon { end, .. } => end.unwrap_or(tree.tokens.len()),
        _ => tree.tokens.len(),
    }
}

fn fallthrough_message() -> &'static str {
    "This `case` or `default` has a nonempty block which does not end \
     with `break`, `continue`, `return`, `throw` or `exit`. Did you \
     forget to add one of those? If you intend to fall through, add a \
     `// fallthrough` comment to silence this warning."
}

/// An arm is OK with a top-level terminator, a `fall...` comment, or
/// no statements at all.
fn arm_is_ok(tree: &Tree, blocks: &[Block], start: usize, end: usize) -> bool {
    let (lower, scopes) = arm_nested_spans(tree, blocks, start, end);
    if arm_has_fall_comment(tree, start, end) {
        return true;
    }
    if arm_has_terminator(tree, start, end, &lower, &scopes) {
        return true;
    }
    !arm_is_nonempty(tree, start, end)
}

/// Token spans of nested loops/switches (`lower`) and nested
/// declarations (`scopes`) inside an arm.
type ArmSpans = (Vec<(usize, usize)>, Vec<(usize, usize)>);

fn arm_nested_spans(tree: &Tree, blocks: &[Block], start: usize, end: usize) -> ArmSpans {
    let mut lower = Vec::new();
    let mut scopes = Vec::new();
    for block in blocks {
        if block.kw_idx <= start || block.kw_idx >= end {
            continue;
        }
        let span = (block.kw_idx, span_end_idx(tree, block));
        if [
            BlockKind::While,
            BlockKind::DoWhile,
            BlockKind::For,
            BlockKind::Foreach,
            BlockKind::Switch,
        ]
        .contains(&block.kind)
        {
            lower.push(span);
        } else if [
            BlockKind::Function,
            BlockKind::ArrowFn,
            BlockKind::Class,
            BlockKind::Interface,
            BlockKind::Trait,
            BlockKind::Enum,
        ]
        .contains(&block.kind)
        {
            scopes.push(span);
        }
    }
    (lower, scopes)
}

fn arm_has_fall_comment(tree: &Tree, start: usize, end: usize) -> bool {
    let mut i = start;
    while i < end && i < tree.tokens.len() {
        let token = &tree.tokens[i];
        if !is_semantic(token) && token.value.to_ascii_lowercase().contains("fall") {
            return true;
        }
        i += 1;
    }
    false
}

/// Any top-level terminator anywhere in the arm silences it (mirrors
/// the `$statment_ok` typo quirk).
fn arm_has_terminator(
    tree: &Tree,
    start: usize,
    end: usize,
    lower: &[(usize, usize)],
    scopes: &[(usize, usize)],
) -> bool {
    let mut i = start;
    while i < end && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        let jumps = kind == kinds::T_BREAK || kind == kinds::T_CONTINUE;
        let exits = kind == kinds::T_RETURN || kind == kinds::T_THROW || kind == kinds::T_EXIT;
        if jumps && !inside_any(i, lower) && !inside_any(i, scopes) {
            return true;
        }
        if exits && !inside_any(i, scopes) {
            return true;
        }
        i += 1;
    }
    false
}

/// Nonempty means: any `;`, `function`, `class`, or `interface`.
fn arm_is_nonempty(tree: &Tree, start: usize, end: usize) -> bool {
    let mut i = start;
    while i < end && i < tree.tokens.len() {
        match tree.tokens[i].kind.as_str() {
            ";" | kinds::T_FUNCTION | kinds::T_CLASS | kinds::T_INTERFACE => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// XHP39: by-reference `foreach` iterators reused after the loop
/// without an intervening `unset()` or `=&` reassignment.
fn rule_reused_iterator_reference(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function {
            continue;
        }
        let Some(body) = body_range(tree, block) else {
            continue;
        };
        if matches!(block.body, BodySpan::Empty { .. }) {
            continue;
        }
        ref_reuse_in_def(tree, &blocks, idx, body, out);
    }
}

fn ref_reuse_in_def(
    tree: &Tree,
    blocks: &[Block],
    def: usize,
    body: (usize, usize),
    out: &mut Vec<RawFinding>,
) {
    let nested = nested_fn_ranges(tree, blocks, def, body, false);
    let mut excluded = HashSet::new();
    for range in &nested {
        excluded.extend(vars_between(tree, range.0, range.1));
    }
    let mut unsets = ref_unsets(tree, body, &nested, &mut excluded);
    let refs = ref_iterators(tree, blocks, body, &nested, &mut excluded);
    ref_reassign_unsets(tree, body, &mut unsets, &mut excluded);
    emit_ref_reuses(tree, body, &refs, &unsets, &excluded, out);
}

/// `unset()` variables per name (occurrences excluded).
fn ref_unsets(
    tree: &Tree,
    body: (usize, usize),
    nested: &[(usize, usize)],
    excluded: &mut HashSet<usize>,
) -> HashMap<String, Vec<usize>> {
    let mut unsets: HashMap<String, Vec<usize>> = HashMap::new();
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind != kinds::T_UNSET || inside_any(i, nested) {
            i += 1;
            continue;
        }
        let end = stmt_end(tree, i);
        for var in vars_between(tree, i, end) {
            unsets
                .entry(tree.tokens[var].value.clone())
                .or_default()
                .push(tree.tokens[var].offset);
            excluded.insert(var);
        }
        i += 1;
    }
    unsets
}

/// Depth-0 `;` (or closer) ending the statement at `from`.
fn stmt_end(tree: &Tree, from: usize) -> usize {
    let mut depth = 0;
    let mut i = from + 1;
    while i < tree.tokens.len() {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" if depth > 0 => depth -= 1,
            ";" if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    i
}

/// By-reference iterator offsets per name (iterator occurrences and
/// same-name in-loop uses excluded).
fn ref_iterators(
    tree: &Tree,
    blocks: &[Block],
    body: (usize, usize),
    nested: &[(usize, usize)],
    excluded: &mut HashSet<usize>,
) -> HashMap<String, Vec<usize>> {
    let mut refs: HashMap<String, Vec<usize>> = HashMap::new();
    for idx in blocks_in_range(blocks, body, BlockKind::Foreach) {
        let block = &blocks[idx];
        if inside_any(block.kw_idx, nested) {
            continue;
        }
        let iters = foreach_iters(tree, block);
        let Some(value) = iters.value else {
            continue;
        };
        if !value.by_ref {
            continue;
        }
        let name = tree.tokens[value.idx].value.clone();
        refs.entry(name.clone())
            .or_default()
            .push(tree.tokens[value.idx].offset);
        excluded.insert(value.idx);
        exclude_loop_var(tree, block, &name, excluded);
    }
    refs
}

fn exclude_loop_var(tree: &Tree, block: &Block, name: &str, excluded: &mut HashSet<usize>) {
    let end = span_end_idx(tree, block);
    let mut i = block.kw_idx;
    while i <= end && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE && tree.tokens[i].value == name {
            excluded.insert(i);
        }
        i += 1;
    }
}

/// `$x = &$y;` counts as unsetting `$x` (not scope-limited, like PHP).
fn ref_reassign_unsets(
    tree: &Tree,
    body: (usize, usize),
    unsets: &mut HashMap<String, Vec<usize>>,
    excluded: &mut HashSet<usize>,
) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == "="
            && let Some(lval) = ref_reassign_lval(tree, body, i)
            && is_ref_rval(tree, i)
        {
            unsets
                .entry(tree.tokens[lval].value.clone())
                .or_default()
                .push(tree.tokens[lval].offset);
            excluded.insert(lval);
        }
        i += 1;
    }
}

fn ref_reassign_lval(tree: &Tree, body: (usize, usize), eq: usize) -> Option<usize> {
    let lval = tree.prev_semantic(eq)?;
    if lval < body.0 || tree.tokens[lval].kind != kinds::T_VARIABLE {
        return None;
    }
    if is_dynamic_var(tree, lval) {
        return None;
    }
    Some(lval)
}

/// `= & $var` with a bare variable reference on the right.
fn is_ref_rval(tree: &Tree, eq: usize) -> bool {
    let Some(amp) = tree.next_semantic(eq) else {
        return false;
    };
    if tree.tokens[amp].kind != "&" {
        return false;
    }
    let Some(var) = tree.next_semantic(amp) else {
        return false;
    };
    if tree.tokens[var].kind != kinds::T_VARIABLE {
        return false;
    }
    !tree.next_semantic(var).is_some_and(|after| {
        matches!(
            tree.tokens[after].kind.as_str(),
            "[" | kinds::T_OBJECT_OPERATOR | kinds::T_NULLSAFE_OBJECT_OPERATOR
        )
    })
}

fn emit_ref_reuses(
    tree: &Tree,
    body: (usize, usize),
    refs: &HashMap<String, Vec<usize>>,
    unsets: &HashMap<String, Vec<usize>>,
    excluded: &HashSet<usize>,
    out: &mut Vec<RawFinding>,
) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_VARIABLE && !excluded.contains(&i) {
            check_ref_reuse(tree, i, refs, unsets, out);
        }
        i += 1;
    }
}

fn check_ref_reuse(
    tree: &Tree,
    var: usize,
    refs: &HashMap<String, Vec<usize>>,
    unsets: &HashMap<String, Vec<usize>>,
    out: &mut Vec<RawFinding>,
) {
    let name = &tree.tokens[var].value;
    let Some(offsets) = refs.get(name.as_str()) else {
        return;
    };
    let offset = tree.tokens[var].offset;
    let mut reference = None;
    for candidate in offsets {
        if *candidate < offset {
            reference = Some(*candidate);
        } else {
            break;
        }
    }
    let Some(reference) = reference else {
        return;
    };
    let cleared = unsets.get(name.as_str()).is_some_and(|list| {
        list.iter()
            .any(|unset| *unset > reference && *unset < offset)
    });
    if !cleared {
        push(out, 39, offset, ref_reuse_message());
    }
}

fn ref_reuse_message() -> &'static str {
    "This variable was used already as a by-reference iterator \
     variable. Such variables survive outside the `foreach` loop, \
     do not reuse."
}

/// PHP `!$version` / `empty($version)` gating: only `""` and
/// `"0"` are falsy (`"0.0"` still gates).
fn php_empty(version: &str) -> bool {
    version.is_empty() || version == "0"
}

/// XHP45: version-gated symbol-table checks (functions, classes,
/// constants, parameter versions, Windows availability) plus the
/// PHP 5.3/5.4 syntax checks. Without `xhpast.php-version` nothing
/// fires.
fn rule_php_compatibility(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let Some(version) = ctx.options.php_version.as_deref() else {
        return;
    };
    if php_empty(version) {
        return;
    }
    compat_symbols(ctx, version, out);
    if version_compare(version, "5.3.0") == std::cmp::Ordering::Less {
        compat53_features(ctx, version, out);
    }
    if version_compare(version, "5.4.0") == std::cmp::Ordering::Less {
        compat54_features(ctx, out);
    } else {
        compat54_incompat(ctx, out);
    }
}

fn compat53_features(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    compat53_closures(ctx, version, out);
    compat53_namespaces(ctx, version, out);
    compat53_static(ctx, version, out);
    compat53_short_ternary(ctx, version, out);
    compat53_nowdoc(ctx, version, out);
}

fn compat53_closures(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_FUNCTION {
            continue;
        }
        let anon = tree
            .next_semantic(i)
            .is_some_and(|next| tree.tokens[next].kind == "(");
        if anon {
            let message = format!(
                "This codebase targets PHP {version}, but anonymous \
                 functions were not introduced until PHP 5.3."
            );
            push(out, 45, token.offset, &message);
        }
    }
}

fn compat53_namespaces(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let message = format!(
        "This codebase targets PHP {version}, but namespaces were not \
         introduced until PHP 5.3."
    );
    for token in &tree.tokens {
        if token.kind == kinds::T_NAMESPACE {
            push(out, 45, token.offset, &message);
        }
    }
    for use_idx in import_use_idxs(tree) {
        push(out, 45, tree.tokens[use_idx].offset, &message);
    }
}

/// Import `use` statements (trait and closure uses excluded).
fn import_use_idxs(tree: &Tree) -> Vec<usize> {
    let blocks = tree.blocks();
    let classes = class_body_spans(tree, &blocks);
    let mut out = Vec::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_USE {
            continue;
        }
        if inside_any(i, &classes) || is_closure_use(tree, &blocks, i) {
            continue;
        }
        out.push(i);
    }
    out
}

fn is_closure_use(tree: &Tree, blocks: &[Block], use_idx: usize) -> bool {
    // `use` never leads the file (outside PHP mode it lexes as inline
    // HTML), so a previous token always exists; token 0 is never `)`,
    // exactly like the old `None` arm.
    let prev = tree.prev_semantic(use_idx).unwrap_or(0);
    if tree.tokens[prev].kind != ")" {
        return false;
    }
    blocks.iter().any(|block| {
        matches!(block.kind, BlockKind::Function | BlockKind::ArrowFn) && block.rparen == Some(prev)
    })
}

fn compat53_static(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
            continue;
        }
        let is_static = tree
            .prev_semantic(i)
            .is_some_and(|prev| tree.tokens[prev].value == "static");
        if is_static {
            let prev = tree.prev_semantic(i).unwrap_or(i);
            let message = format!(
                "This codebase targets PHP {version}, but `static::` was not \
                 introduced until PHP 5.3."
            );
            push(out, 45, tree.tokens[prev].offset, &message);
        }
    }
}

/// `?:` with an empty middle, reported at the condition start.
fn compat53_short_ternary(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "?" {
            continue;
        }
        if short_ternary_colon(tree, i).is_some() {
            let at = scan_operand_left(tree, i, true).map_or(i, |(first, _)| first);
            let message = format!(
                "This codebase targets PHP {version}, but short ternary was \
                 not introduced until PHP 5.3."
            );
            push(out, 45, tree.tokens[at].offset, &message);
        }
    }
}

/// The `:` closing a short ternary, if the middle holds only trivia.
fn short_ternary_colon(tree: &Tree, question: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = question + 1;
    while i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if is_open_kind(kind) {
            depth += 1;
        } else if is_close_kind(kind) {
            if depth == 0 {
                return None;
            }
            depth -= 1;
        } else if depth == 0 && kind == ":" {
            return short_ternary_middle(tree, question, i);
        } else if depth == 0 && (kind == ";" || kind == ",") {
            return None;
        }
        i += 1;
    }
    None
}

fn short_ternary_middle(tree: &Tree, question: usize, colon: usize) -> Option<usize> {
    let mut i = question + 1;
    while i < colon {
        if is_semantic(&tree.tokens[i]) {
            return None;
        }
        i += 1;
    }
    Some(colon)
}

fn compat53_nowdoc(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_NOWDOC {
            let message = format!(
                "This codebase targets PHP {version}, but nowdoc was not \
                 introduced until PHP 5.3."
            );
            push(out, 45, token.offset, &message);
        }
    }
}

fn compat54_features(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    compat54_call_index(ctx, out);
    compat54_short_array(ctx, out);
    compat54_closure_binding(ctx, out);
    compat54_binary_literal(ctx, out);
}

/// `f()[...]` / `$o->m()[...]` indexing, reported at the index.
fn compat54_call_index(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "[" {
            continue;
        }
        if let Some(at) = call_index_at(tree, i) {
            push(
                out,
                45,
                tree.tokens[at].offset,
                compat54_call_index_message(),
            );
        }
    }
}

fn compat54_call_index_message() -> &'static str {
    "The `f()[...]` syntax was not introduced until PHP 5.4, but this \
     codebase targets an earlier version of PHP. You can rewrite \
     this expression using `idx()`."
}

/// First index token when `[` indexes a call result.
fn call_index_at(tree: &Tree, bracket: usize) -> Option<usize> {
    let prev = tree.prev_semantic(bracket)?;
    if tree.tokens[prev].kind != ")" {
        return None;
    }
    let open = tree.match_open(prev)?;
    let name = tree.prev_semantic(open)?;
    if tree.tokens[name].kind != kinds::T_STRING {
        return None;
    }
    if tree.prev_semantic(name).is_some_and(|before| {
        matches!(
            tree.tokens[before].kind.as_str(),
            kinds::T_PAAMAYIM_NEKUDOTAYIM | kinds::T_NEW | kinds::T_FUNCTION
        )
    }) {
        return None;
    }
    tree.next_semantic(bracket)
}

fn compat54_short_array(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for (open, _) in array_literals(ctx.tree) {
        if ctx.tree.tokens[open].kind == "[" {
            push(
                out,
                45,
                ctx.tree.tokens[open].offset,
                compat54_short_array_message(),
            );
        }
    }
}

fn compat54_short_array_message() -> &'static str {
    "The short array syntax (\"[...]\") was not introduced until \
     PHP 5.4, but this codebase targets an earlier version of PHP. \
     You can rewrite this expression using `array(...)` instead."
}

/// `self::` / `$this->` inside anonymous closures.
fn compat54_closure_binding(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for block in &blocks {
        if block.kind != BlockKind::Function || !block.is_anonymous {
            continue;
        }
        let Some(body) = body_range(tree, block) else {
            continue;
        };
        compat54_closure_self(tree, body, out);
        compat54_closure_this(tree, body, out);
    }
}

fn compat54_closure_self(tree: &Tree, body: (usize, usize), out: &mut Vec<RawFinding>) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        if tree.tokens[i].kind == kinds::T_PAAMAYIM_NEKUDOTAYIM
            && let Some(prev) = tree.prev_semantic(i)
            && prev >= body.0
            && tree.tokens[prev].value.eq_ignore_ascii_case("self")
        {
            push(out, 45, tree.tokens[prev].offset, compat54_self_message());
        }
        i += 1;
    }
}

fn compat54_self_message() -> &'static str {
    "The use of `self` in an anonymous closure is not \
     available before PHP 5.4."
}

fn compat54_closure_this(tree: &Tree, body: (usize, usize), out: &mut Vec<RawFinding>) {
    let mut i = body.0;
    while i <= body.1 && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        let arrow = kind == kinds::T_OBJECT_OPERATOR || kind == kinds::T_NULLSAFE_OBJECT_OPERATOR;
        if arrow
            && let Some(prev) = tree.prev_semantic(i)
            && prev >= body.0
            && tree.tokens[prev].kind == kinds::T_VARIABLE
            && tree.tokens[prev].value == "$this"
        {
            push(out, 45, tree.tokens[prev].offset, compat54_this_message());
        }
        i += 1;
    }
}

fn compat54_this_message() -> &'static str {
    "The use of `$this` in an anonymous closure is not \
     available before PHP 5.4."
}

fn compat54_binary_literal(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_LNUMBER && is_binary_literal(&token.value) {
            push(out, 45, token.offset, compat54_binary_message());
        }
    }
}

fn is_binary_literal(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 2
        && bytes[0] == b'0'
        && (bytes[1] == b'b' || bytes[1] == b'B')
        && bytes[2..].iter().all(|b| *b == b'0' || *b == b'1')
}

fn compat54_binary_message() -> &'static str {
    "Binary integer literals are not available before PHP 5.4."
}

/// `break` / `continue` with non-numeric (or zero) arguments.
fn compat54_incompat(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_BREAK && token.kind != kinds::T_CONTINUE {
            continue;
        }
        if let Some(at) = break_arg_bad(tree, i) {
            push(out, 45, tree.tokens[at].offset, compat54_break_message());
        }
    }
}

/// The offending argument token, if the statement needs flagging:
/// anything but an empty or bare-nonzero-numeric argument flags.
fn break_arg_bad(tree: &Tree, kw: usize) -> Option<usize> {
    let mut args = Vec::new();
    let mut i = kw + 1;
    while i < tree.tokens.len() {
        match tree.tokens[i].kind.as_str() {
            ";" | kinds::T_CLOSE_TAG => break,
            _ if is_semantic(&tree.tokens[i]) => args.push(i),
            _ => {}
        }
        i += 1;
    }
    match args.as_slice() {
        [] => None,
        [only]
            if matches!(
                tree.tokens[*only].kind.as_str(),
                kinds::T_LNUMBER | kinds::T_DNUMBER
            ) && tree.tokens[*only].value != "0" =>
        {
            None
        }
        [first, ..] => Some(*first),
    }
}

fn compat54_break_message() -> &'static str {
    "The `break` and `continue` statements no longer accept \
     variable arguments."
}

/// Vendored `resources/php/symbol-information.json` (byte-identical
/// to the PHP-side copy): introduction/removal versions for
/// functions, classes, interfaces, constants, parameter positions,
/// and Windows availability. Parsed once per process.
const SYMBOL_JSON: &str = include_str!("../resources/php/symbol-information.json");

fn compat_db() -> &'static serde_json::Value {
    static DB: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    DB.get_or_init(|| serde_json::from_str(SYMBOL_JSON).unwrap_or(serde_json::Value::Null))
}

/// (`php.min`, `php.max`) for `name` in `section` (`functions`,
/// `classes`, `interfaces`, `constants`); missing entries yield
/// (`None`, `None`), mirroring PHP `idx()` defaults.
fn sym_min_max<'a>(
    db: &'a serde_json::Value,
    section: &str,
    name: &str,
) -> (Option<&'a str>, Option<&'a str>) {
    let entry = db.get(section).and_then(|entries| entries.get(name));
    match entry {
        Some(found) => (
            found.get("php.min").and_then(serde_json::Value::as_str),
            found.get("php.max").and_then(serde_json::Value::as_str),
        ),
        None => (None, None),
    }
}

/// True when `name` has a per-parameter version row (PHP checks
/// `array_key_exists` on the params table, not the function table).
fn sym_has_params(db: &serde_json::Value, name: &str) -> bool {
    db.get("params")
        .and_then(|params| params.get(name))
        .is_some()
}

/// Minimum version for positional parameter `idx` of `name`.
fn sym_param_min<'a>(db: &'a serde_json::Value, name: &str, idx: usize) -> Option<&'a str> {
    db.get("params")?.get(name)?.get(idx)?.as_str()
}

/// Windows availability of a function: no special considerations,
/// never available, or available since a version.
enum WinAvail<'a> {
    Plain,
    Unavailable,
    Since(&'a str),
}

fn sym_windows<'a>(db: &'a serde_json::Value, name: &str) -> WinAvail<'a> {
    let entry = db
        .get("functions_windows")
        .and_then(|table| table.get(name));
    match entry {
        Some(serde_json::Value::Bool(false)) => WinAvail::Unavailable,
        Some(serde_json::Value::String(since)) => WinAvail::Since(since),
        _ => WinAvail::Plain,
    }
}

/// Rank of a prerelease tag in PHP `version_compare()` order
/// (`dev < alpha = a < beta = b < RC = rc < release < pl = p`).
/// Unknown tags rank as release.
fn sym_tag_rank(tag: &str) -> i32 {
    match tag.to_ascii_lowercase().as_str() {
        "dev" => 0,
        "alpha" | "a" => 1,
        "beta" | "b" => 2,
        "rc" => 3,
        "pl" | "p" => 5,
        _ => 4,
    }
}

/// Split `5.4.0RC3` into core (`5.4.0`) plus (rank, number);
/// plain releases yield (`version`, (4, 0)).
fn sym_split_release(version: &str) -> (&str, (i32, u64)) {
    let bytes = version.as_bytes();
    let mut end = bytes.len();
    let mut start = end;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let num = version[start..end].parse::<u64>().unwrap_or(0);
    end = start;
    while start > 0 && bytes[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }
    if start == end {
        return (version, (4, 0));
    }
    (&version[..start], (sym_tag_rank(&version[start..end]), num))
}

/// PHP `version_compare()` order for symbol-table versions, which
/// may carry prerelease suffixes (`5.4.0RC3`, `5.6.0beta1`); the
/// shared [`version_compare`] compares suffixes lexically, which
/// wrongly orders `5.4.0RC3` above `5.4.0`.
fn sym_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let (left_core, left_rank) = sym_split_release(left);
    let (right_core, right_rank) = sym_split_release(right);
    version_compare(left_core, right_core).then(left_rank.cmp(&right_rank))
}

fn sym_gt(left: &str, right: &str) -> bool {
    sym_cmp(left, right) == std::cmp::Ordering::Greater
}

fn sym_lt(left: &str, right: &str) -> bool {
    sym_cmp(left, right) == std::cmp::Ordering::Less
}

/// Whitelist bucket for conditionally-guarded symbols. PHP also
/// files `interface_exists()` guards under an `interface` bucket
/// that no check ever reads; those guards are dropped here with
/// identical observable behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhiteKind {
    Class,
    Function,
    Constant,
}

/// One `if (xxx_exists('sym'))` guard: the guarded symbol plus the
/// then-branch token span.
struct WhiteEntry {
    kind: WhiteKind,
    symbol: String,
    first: usize,
    last: usize,
}

/// Guards from `if` conditions that are exactly one existence
/// check. `elseif` never guards: PHP selects `n_IF` only, and
/// `n_ELSEIF` is a separate node type.
fn compat_whitelist(tree: &Tree, blocks: &[Block]) -> Vec<WhiteEntry> {
    let mut out = Vec::new();
    for block in blocks {
        if block.kind != BlockKind::If {
            continue;
        }
        if let Some(entry) = white_entry(tree, block) {
            out.push(entry);
        }
    }
    out
}

/// The guard of one `if`, when its condition is exactly
/// `xxx_exists(<static scalar>, ...)`: a bare call, nothing else.
/// Negated or compound conditions do not guard, exactly like PHP.
fn white_entry(tree: &Tree, block: &Block) -> Option<WhiteEntry> {
    let lparen = block.lparen?;
    let rparen = block.rparen?;
    let name_idx = tree.next_semantic(lparen)?;
    if name_idx >= rparen {
        return None;
    }
    let kind = white_kind(&tree.tokens[name_idx])?;
    let open = tree.next_semantic(name_idx)?;
    if tree.tokens[open].kind != "(" {
        return None;
    }
    let close = tree.match_close(open)?;
    if tree.next_semantic(close) != Some(rparen) {
        return None;
    }
    let args = tree.split_top_level(open, close);
    let (first, last) = *args.first()?;
    let symbol = tree.static_scalar_text(first, last)?;
    let (span_first, span_last) = body_range(tree, block)?;
    Some(WhiteEntry {
        kind,
        symbol,
        first: span_first.min(span_last),
        last: span_first.max(span_last),
    })
}

fn white_kind(token: &Token) -> Option<WhiteKind> {
    if token.kind != kinds::T_STRING {
        return None;
    }
    match token.value.as_str() {
        "class_exists" => Some(WhiteKind::Class),
        "function_exists" => Some(WhiteKind::Function),
        "defined" => Some(WhiteKind::Constant),
        _ => None,
    }
}

fn whitelisted(entries: &[WhiteEntry], kind: WhiteKind, symbol: &str, idx: usize) -> bool {
    entries.iter().any(|entry| {
        entry.kind == kind && entry.symbol == symbol && in_range(idx, entry.first, entry.last)
    })
}

/// XHP45 symbol legs: functions (plus parameter versions and
/// Windows availability), classes/interfaces, then constants.
/// Class-name token positions feed the constants leg so class
/// names are never double-scanned as constants.
fn compat_symbols(ctx: &RuleCtx, version: &str, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let db = compat_db();
    let blocks = tree.blocks();
    let whitelist = compat_whitelist(tree, &blocks);
    let windows = ctx.options.php_version_windows.as_deref();
    compat_functions(tree, db, version, windows, &whitelist, out);
    let mut class_tokens = HashSet::new();
    compat_classes(
        tree,
        &blocks,
        db,
        version,
        &whitelist,
        &mut class_tokens,
        out,
    );
    compat_constants(tree, db, version, &whitelist, &class_tokens, out);
}

fn compat_functions(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    windows: Option<&str>,
    whitelist: &[WhiteEntry],
    out: &mut Vec<RawFinding>,
) {
    for call in tree.calls() {
        if call.is_construct {
            continue;
        }
        if has_leading_backslash(tree, call.name_idx) {
            continue;
        }
        compat_function_call(tree, db, &call, version, windows, whitelist, out);
    }
}

/// True when a call name carries a `\` prefix, so PHP's concrete
/// string (backslash included) misses the symbol table.
fn has_leading_backslash(tree: &Tree, name_idx: usize) -> bool {
    tree.prev_semantic(name_idx)
        .is_some_and(|prev| tree.tokens[prev].kind == "\\")
}

fn compat_function_call(
    tree: &Tree,
    db: &serde_json::Value,
    call: &CallSite,
    version: &str,
    windows: Option<&str>,
    whitelist: &[WhiteEntry],
    out: &mut Vec<RawFinding>,
) {
    let name = call.name.as_str();
    if whitelisted(whitelist, WhiteKind::Function, name, call.name_idx) {
        return;
    }
    let (min, max) = sym_min_max(db, "functions", name);
    if let Some(introduced) = min.filter(|found| sym_gt(found, version)) {
        push(
            out,
            45,
            tree.tokens[call.name_idx].offset,
            &format!(
                "This codebase targets PHP {version}, but `{name}()` was not \
                 introduced until PHP {introduced}."
            ),
        );
    } else if let Some(removed) = max.filter(|found| sym_lt(found, version)) {
        push(
            out,
            45,
            tree.tokens[call.name_idx].offset,
            &format!(
                "This codebase targets PHP {version}, but `{name}()` was removed in PHP {removed}."
            ),
        );
    } else if sym_has_params(db, name) {
        compat_call_params(tree, db, call, version, out);
    }
    compat_windows(tree, db, call, windows, out);
}

fn compat_call_params(
    tree: &Tree,
    db: &serde_json::Value,
    call: &CallSite,
    version: &str,
    out: &mut Vec<RawFinding>,
) {
    for (i, (first, _)) in call.args.iter().enumerate() {
        let need = sym_param_min(db, &call.name, i).filter(|found| sym_gt(found, version));
        if let Some(introduced) = need {
            let position = i + 1;
            push(
                out,
                45,
                tree.tokens[*first].offset,
                &format!(
                    "This codebase targets PHP {version}, but parameter {position} of `{}()` was not \
                     introduced until PHP {introduced}.",
                    call.name
                ),
            );
        }
    }
}

fn compat_windows(
    tree: &Tree,
    db: &serde_json::Value,
    call: &CallSite,
    windows: Option<&str>,
    out: &mut Vec<RawFinding>,
) {
    let Some(target) = windows else {
        return;
    };
    if php_empty(target) {
        return;
    }
    match sym_windows(db, &call.name) {
        WinAvail::Plain => {}
        WinAvail::Unavailable => push(
            out,
            45,
            tree.tokens[call.name_idx].offset,
            &format!(
                "This codebase targets PHP {target} on Windows, but `{}()` is not available there.",
                call.name
            ),
        ),
        WinAvail::Since(since) => {
            if sym_gt(since, target) {
                push(
                    out,
                    45,
                    tree.tokens[call.name_idx].offset,
                    &format!(
                        "This codebase targets PHP {target} on Windows, but `{}()` is not \
                         available there until PHP {since}.",
                        call.name
                    ),
                );
            }
        }
    }
}

/// One class-name occurrence: first/last token indices plus the
/// concrete name (leading `\` kept, so qualified names miss the
/// table exactly like PHP's `getConcreteString()` lookups).
struct ClassHit {
    first: usize,
    last: usize,
    name: String,
}

fn compat_classes(
    tree: &Tree,
    blocks: &[Block],
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    class_tokens: &mut HashSet<usize>,
    out: &mut Vec<RawFinding>,
) {
    let mut hits = Vec::new();
    compat_class_new(tree, &mut hits);
    compat_class_static(tree, &mut hits);
    compat_class_decl_names(blocks, &mut hits);
    compat_class_inheritance(tree, &mut hits);
    compat_class_instanceof(tree, &mut hits);
    compat_class_catch(tree, blocks, &mut hits);
    compat_class_param_types(tree, blocks, &mut hits);
    compat_class_return_types(tree, blocks, &mut hits);
    hits.sort_by_key(|hit| hit.first);
    hits.dedup_by_key(|hit| hit.first);
    for hit in &hits {
        for idx in hit.first..=hit.last {
            class_tokens.insert(idx);
        }
        compat_class_hit(tree, db, version, whitelist, hit, out);
    }
}

fn compat_class_hit(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    hit: &ClassHit,
    out: &mut Vec<RawFinding>,
) {
    if whitelisted(whitelist, WhiteKind::Class, &hit.name, hit.first) {
        return;
    }
    let in_classes = db
        .get("classes")
        .and_then(|classes| classes.get(hit.name.as_str()))
        .is_some();
    let section = if in_classes { "classes" } else { "interfaces" };
    let (min, max) = sym_min_max(db, section, &hit.name);
    if let Some(introduced) = min.filter(|found| sym_gt(found, version)) {
        push(
            out,
            45,
            tree.tokens[hit.first].offset,
            &format!(
                "This codebase targets PHP {version}, but `{}` was not introduced until PHP \
                 {introduced}.",
                hit.name
            ),
        );
    } else if let Some(removed) = max.filter(|found| sym_lt(found, version)) {
        push(
            out,
            45,
            tree.tokens[hit.first].offset,
            &format!(
                "This codebase targets PHP {version}, but `{}` was removed in PHP {removed}.",
                hit.name
            ),
        );
    }
}

/// Last token index of the qualified run starting at `idx`
/// (`T_STRING` or `\` segments).
fn qual_end(tree: &Tree, mut idx: usize) -> usize {
    while let Some(next) = tree.next_semantic(idx) {
        let kind = tree.tokens[next].kind.as_str();
        if kind != kinds::T_STRING && kind != "\\" {
            break;
        }
        idx = next;
    }
    idx
}

/// First segment of the qualified run ending at `last` (walk left
/// over `\` + `T_STRING` pairs).
fn qual_run_start(tree: &Tree, last: usize) -> usize {
    let mut first = last;
    while let Some(sep) = tree.prev_semantic(first) {
        if tree.tokens[sep].kind != "\\" {
            break;
        }
        // `\` never leads the file (outside PHP mode it lexes as inline
        // HTML), so a token before it always exists; token 0 is never
        // a `T_STRING`, exactly like the old `None` arm.
        let prev = tree.prev_semantic(sep).unwrap_or(0);
        if tree.tokens[prev].kind != kinds::T_STRING {
            break;
        }
        first = prev;
    }
    first
}

/// Concrete text of a qualified run: `T_STRING`/`\` values from
/// `first` to `end`, with an optional leading `\`.
fn qual_text(tree: &Tree, first: usize, end: usize, leading: bool) -> String {
    let mut name = String::new();
    if leading {
        name.push('\\');
    }
    let mut i = first;
    while i <= end && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if kind == kinds::T_STRING || kind == "\\" {
            name.push_str(&tree.tokens[i].value);
        }
        i += 1;
    }
    name
}

/// Qualified name starting AT `idx` (`T_STRING`, `\`, or a lone
/// `static`); `None` for anything else.
fn qual_from(tree: &Tree, idx: usize) -> Option<ClassHit> {
    if tree.tokens[idx].kind == kinds::T_STATIC {
        return Some(ClassHit {
            first: idx,
            last: idx,
            name: tree.tokens[idx].value.clone(),
        });
    }
    let mut first = idx;
    let mut leading = false;
    if tree.tokens[idx].kind == "\\" {
        leading = true;
        first = tree.next_semantic(idx)?;
    }
    if tree.tokens[first].kind != kinds::T_STRING {
        return None;
    }
    let last = qual_end(tree, first);
    Some(ClassHit {
        first,
        last,
        name: qual_text(tree, first, last, leading),
    })
}

/// Qualified name starting after `idx` (one optional leading `\`
/// skipped).
fn qual_after(tree: &Tree, idx: usize) -> Option<ClassHit> {
    let next = tree.next_semantic(idx)?;
    qual_from(tree, next)
}

/// Qualified name ending before `idx` (the `::` receiver); `None`
/// for `$var::`-style receivers.
fn qual_before(tree: &Tree, idx: usize) -> Option<ClassHit> {
    let end = tree.prev_semantic(idx)?;
    if tree.tokens[end].kind == kinds::T_STATIC {
        return Some(ClassHit {
            first: end,
            last: end,
            name: tree.tokens[end].value.clone(),
        });
    }
    if tree.tokens[end].kind != kinds::T_STRING {
        return None;
    }
    let first = qual_run_start(tree, end);
    let leading = tree
        .prev_semantic(first)
        .is_some_and(|prev| tree.tokens[prev].kind == "\\");
    Some(ClassHit {
        first,
        last: end,
        name: qual_text(tree, first, end, leading),
    })
}

fn compat_class_new(tree: &Tree, hits: &mut Vec<ClassHit>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_NEW {
            continue;
        }
        if let Some(hit) = qual_after(tree, i) {
            hits.push(hit);
        }
    }
}

fn compat_class_static(tree: &Tree, hits: &mut Vec<ClassHit>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
            continue;
        }
        if let Some(hit) = qual_before(tree, i) {
            hits.push(hit);
        }
    }
}

/// Class-like declaration names, which PHP models as
/// `n_CLASS_NAME` children of the declaration node.
fn compat_class_decl_names(blocks: &[Block], hits: &mut Vec<ClassHit>) {
    for block in blocks {
        let class_like = matches!(
            block.kind,
            BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
        );
        if !class_like {
            continue;
        }
        if let (Some(name), Some(idx)) = (block.name.as_deref(), block.name_idx) {
            hits.push(ClassHit {
                first: idx,
                last: idx,
                name: name.to_string(),
            });
        }
    }
}

fn compat_class_inheritance(tree: &Tree, hits: &mut Vec<ClassHit>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_EXTENDS && token.kind != kinds::T_IMPLEMENTS {
            continue;
        }
        inheritance_names(tree, i, hits);
    }
}

/// Comma-separated qualified names after `extends`/`implements`.
fn inheritance_names(tree: &Tree, kw_idx: usize, hits: &mut Vec<ClassHit>) {
    let mut cur = tree.next_semantic(kw_idx);
    while let Some(idx) = cur {
        if !is_name_token(&tree.tokens[idx].kind) {
            break;
        }
        let Some(hit) = qual_from(tree, idx) else {
            break;
        };
        cur = tree.next_semantic(hit.last);
        hits.push(hit);
        if cur.is_some_and(|next| tree.tokens[next].kind == ",") {
            cur = cur.and_then(|next| tree.next_semantic(next));
        } else {
            break;
        }
    }
}

fn compat_class_instanceof(tree: &Tree, hits: &mut Vec<ClassHit>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_INSTANCEOF {
            continue;
        }
        if let Some(hit) = qual_after(tree, i) {
            hits.push(hit);
        }
    }
}

fn compat_class_catch(tree: &Tree, blocks: &[Block], hits: &mut Vec<ClassHit>) {
    for block in blocks {
        if block.kind != BlockKind::Catch {
            continue;
        }
        let (Some(lparen), Some(rparen)) = (block.lparen, block.rparen) else {
            continue;
        };
        if lparen < rparen.saturating_sub(1) {
            compat_type_names(tree, lparen + 1, rparen - 1, hits);
        }
    }
}

fn compat_class_param_types(tree: &Tree, blocks: &[Block], hits: &mut Vec<ClassHit>) {
    for block in blocks {
        for param in &block.params {
            if let Some((first, last)) = param.ty {
                compat_type_names(tree, first, last, hits);
            }
        }
    }
}

fn compat_class_return_types(tree: &Tree, blocks: &[Block], hits: &mut Vec<ClassHit>) {
    for block in blocks {
        if !matches!(block.kind, BlockKind::Function | BlockKind::ArrowFn) {
            continue;
        }
        if let Some((first, last)) = return_type_range(tree, block) {
            compat_type_names(tree, first, last, hits);
        }
    }
}

/// Class-name starts in a type-ish token range (catch headers,
/// parameter/return types): name tokens that do not continue a
/// qualified run.
fn compat_type_names(tree: &Tree, first: usize, last: usize, hits: &mut Vec<ClassHit>) {
    let mut i = first;
    while i <= last && i < tree.tokens.len() {
        if is_name_token(&tree.tokens[i].kind)
            && is_type_name_start(tree, first, i)
            && let Some(hit) = qual_from(tree, i)
        {
            i = hit.last + 1;
            hits.push(hit);
            continue;
        }
        i += 1;
    }
}

fn is_name_token(kind: &str) -> bool {
    kind == kinds::T_STRING || kind == "\\" || kind == kinds::T_STATIC
}

/// True when `i` starts a name: the previous semantic token (if
/// any, inside the range) is not a `T_STRING`/`\` continuation.
fn is_type_name_start(tree: &Tree, first: usize, i: usize) -> bool {
    let Some(prev) = tree.prev_semantic(i) else {
        return true;
    };
    if prev < first {
        return true;
    }
    !matches!(tree.tokens[prev].kind.as_str(), kinds::T_STRING | "\\")
}

/// Token range of a declared return type (`: Type` after the
/// parameter list or closure `use`), if any.
fn return_type_range(tree: &Tree, block: &Block) -> Option<(usize, usize)> {
    let after = block
        .closure_use
        .map_or(block.rparen, |(_, close)| Some(close))?;
    let colon = tree.next_semantic(after)?;
    if tree.tokens[colon].kind != ":" {
        return None;
    }
    let first = tree.next_semantic(colon)?;
    return_type_end(tree, first).map(|last| (first, last))
}

/// Last token before the `{` / `;` / `=>` terminating a return type.
fn return_type_end(tree: &Tree, first: usize) -> Option<usize> {
    let mut depth: u32 = 0;
    let mut last = None;
    let mut i = first;
    while i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if depth == 0 && (kind == "{" || kind == ";" || kind == kinds::T_DOUBLE_ARROW) {
            break;
        }
        if kind == "(" || kind == "[" {
            depth += 1;
        } else if kind == ")" || kind == "]" {
            depth = depth.saturating_sub(1);
        }
        if is_semantic(&tree.tokens[i]) {
            last = Some(i);
        }
        i += 1;
    }
    last
}

/// XHP45 constants: every `T_STRING` outside class-name positions
/// (plus magic constants), with member, declaration, label, and
/// import-statement names excluded. `use`/`namespace` names get one
/// lookup of their full concrete string each (see
/// [`compat_constant_imports`]), and so does every other qualified
/// run (namespaced constants live in the table). PHP also scans
/// function-call names here (noted as TODO upstream); the tables
/// are disjoint so at most one leg fires per name.
fn compat_constants(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    class_tokens: &HashSet<usize>,
    out: &mut Vec<RawFinding>,
) {
    let mut import_tokens = HashSet::new();
    compat_constant_imports(tree, db, version, whitelist, &mut import_tokens, out);
    for (i, token) in tree.tokens.iter().enumerate() {
        if !is_const_token(&token.kind) {
            continue;
        }
        if class_tokens.contains(&i) || import_tokens.contains(&i) {
            continue;
        }
        if token.kind == kinds::T_STRING {
            compat_constant_string(tree, db, version, whitelist, i, &token.value, out);
        } else {
            if !has_uppercase(&token.value) {
                continue;
            }
            compat_constant_hit(tree, db, version, whitelist, i, &token.value, out);
        }
    }
}

/// One `T_STRING`: non-head qualified segments defer to their run
/// head, which looks up the FULL run text once (namespaced
/// constants like `http\Client\Curl\AUTH_ANY` live in the table);
/// single names look up as before.
fn compat_constant_string(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    idx: usize,
    name: &str,
    out: &mut Vec<RawFinding>,
) {
    if qual_run_start(tree, idx) != idx {
        return;
    }
    if const_name_excluded(tree, idx) {
        return;
    }
    let last = qual_end(tree, idx);
    if last == idx {
        if !has_uppercase(name) {
            return;
        }
        compat_constant_hit(tree, db, version, whitelist, idx, name, out);
    } else if !const_run_gapped(tree, idx, last) {
        let lookup = const_run_lookup(tree, idx, last);
        if has_uppercase(&lookup) {
            compat_constant_lookup(tree, db, version, whitelist, idx, &lookup, out);
        }
    }
}

/// Full lookup text of the qualified run `first..=last` (leading
/// `\` kept, so `\`-prefixed names miss exactly like PHP's
/// `getConcreteString()` lookups).
fn const_run_lookup(tree: &Tree, first: usize, last: usize) -> String {
    let leading = tree
        .prev_semantic(first)
        .is_some_and(|prev| tree.tokens[prev].kind == "\\");
    qual_text(tree, first, last, leading)
}

/// True when a qualified run holds anything but bare separators
/// between its segments (spaces, comments): PHP's concrete string
/// then differs from the joined text and misses the table, so no
/// lookup runs.
fn const_run_gapped(tree: &Tree, first: usize, last: usize) -> bool {
    let mut prev = first;
    let mut cur = tree.next_semantic(first);
    while let Some(idx) = cur {
        if idx > last {
            break;
        }
        if !tree.gap_text(prev, idx).is_empty() {
            return true;
        }
        prev = idx;
        cur = tree.next_semantic(idx);
    }
    false
}

/// Single-lookup pass for `use`/`namespace` names: each is one
/// `n_SYMBOL_NAME` node in PHP (`use.php.test`,
/// `namespace.php.test`), so the constants leg looks up its FULL
/// concrete string once (only namespaced constants are tabled
/// under multi-segment keys). `as` aliases are `n_STRING` and
/// never scan. `use function`/`use const` and group-`use` are
/// beyond the stock grammar (old `xhpast` reports only a syntax
/// error there), so those statements are skipped.
fn compat_constant_imports(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    import_tokens: &mut HashSet<usize>,
    out: &mut Vec<RawFinding>,
) {
    for use_idx in import_use_idxs(tree) {
        compat_constant_use(tree, db, version, whitelist, import_tokens, use_idx, out);
    }
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind == kinds::T_NAMESPACE {
            compat_constant_namespace(tree, db, version, whitelist, import_tokens, i, out);
        }
    }
}

/// One `use` statement: comma-separated imports, each optionally
/// `as`-aliased; one lookup per import name.
fn compat_constant_use(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    import_tokens: &mut HashSet<usize>,
    use_idx: usize,
    out: &mut Vec<RawFinding>,
) {
    hide_use_statement(tree, use_idx, import_tokens);
    let mut cur = tree.next_semantic(use_idx);
    if cur.is_some_and(|idx| is_use_qualifier(tree, idx)) {
        return;
    }
    while let Some(idx) = cur {
        if !is_import_start(tree, idx) {
            break;
        }
        let Some((first, last, name)) = import_name(tree, idx) else {
            break;
        };
        if use_group_open(tree, last) {
            return;
        }
        compat_constant_lookup(tree, db, version, whitelist, first, &name, out);
        cur = use_next_import(tree, last);
    }
}

/// `function`/`const` qualifiers after `use` (beyond stock grammar).
fn is_use_qualifier(tree: &Tree, idx: usize) -> bool {
    matches!(
        tree.tokens[idx].kind.as_str(),
        kinds::T_FUNCTION | kinds::T_CONST
    )
}

/// A `T_STRING` or `\` starts an import name.
fn is_import_start(tree: &Tree, idx: usize) -> bool {
    matches!(tree.tokens[idx].kind.as_str(), kinds::T_STRING | "\\")
}

/// Qualified import name starting at `idx`: first/last token plus
/// the concrete text (leading `\` kept, like PHP lookups).
fn import_name(tree: &Tree, idx: usize) -> Option<(usize, usize, String)> {
    let mut first = idx;
    let mut leading = false;
    if tree.tokens[idx].kind == "\\" {
        leading = true;
        first = tree.next_semantic(idx)?;
    }
    if tree.tokens[first].kind != kinds::T_STRING {
        return None;
    }
    let last = qual_end(tree, first);
    Some((first, last, qual_text(tree, first, last, leading)))
}

/// True when `{` opens right after an import name: group-`use`
/// (`use A\{B};`), beyond the stock grammar.
fn use_group_open(tree: &Tree, last: usize) -> bool {
    tree.next_semantic(last)
        .is_some_and(|next| tree.tokens[next].kind == "{")
}

/// Hide a whole `use` statement (`use` through its `;`) from the
/// per-token scan: every name in it is served exactly once by the
/// single-lookup pass, and aliases never scan.
fn hide_use_statement(tree: &Tree, use_idx: usize, import_tokens: &mut HashSet<usize>) {
    let mut depth = 0u32;
    let mut i = use_idx;
    while i < tree.tokens.len() {
        import_tokens.insert(i);
        match tree.tokens[i].kind.as_str() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            ";" if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
}

/// Token starting the next import after the optional `as ALIAS`
/// and `,` following one import; `None` ends the statement scan.
fn use_next_import(tree: &Tree, last: usize) -> Option<usize> {
    let mut cur = tree.next_semantic(last);
    if cur.is_some_and(|idx| tree.tokens[idx].kind == kinds::T_AS) {
        cur = cur
            .and_then(|as_idx| tree.next_semantic(as_idx))
            .and_then(|alias| tree.next_semantic(alias));
    }
    if cur.is_some_and(|idx| tree.tokens[idx].kind == ",") {
        cur.and_then(|comma| tree.next_semantic(comma))
    } else {
        None
    }
}

/// One `namespace` declaration name (`namespace\FOO` operator uses
/// and bare `namespace {` blocks carry no name of their own; anything
/// else there is invalid PHP, hidden so magic spellings never flag).
fn compat_constant_namespace(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    import_tokens: &mut HashSet<usize>,
    ns_idx: usize,
    out: &mut Vec<RawFinding>,
) {
    let Some(next) = tree.next_semantic(ns_idx) else {
        return;
    };
    if tree.tokens[next].kind != kinds::T_STRING {
        if is_const_token(&tree.tokens[next].kind) {
            import_tokens.insert(next);
        }
        return;
    }
    // `next` is a `T_STRING`, so the import name always resolves (the
    // `None` arm of `import_name` only fires past a leading `\`).
    let last = qual_end(tree, next);
    let name = qual_text(tree, next, last, false);
    compat_constant_lookup(tree, db, version, whitelist, next, &name, out);
}

fn is_const_token(kind: &str) -> bool {
    kind == kinds::T_STRING
        || kind == kinds::T_CLASS_C
        || kind == kinds::T_FUNC_C
        || kind == kinds::T_METHOD_C
        || kind == kinds::T_FILE
        || kind == kinds::T_DIR
        || kind == kinds::T_LINE
        || kind == kinds::T_NS_C
        || kind == kinds::T_TRAIT_C
}

/// True when `value` holds an ASCII uppercase letter; every
/// `constants` table key does, so anything else cannot match.
fn has_uppercase(value: &str) -> bool {
    value.bytes().any(|byte| byte.is_ascii_uppercase())
}

fn compat_constant_hit(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    idx: usize,
    name: &str,
    out: &mut Vec<RawFinding>,
) {
    let lookup = const_lookup_name(tree, idx, name);
    compat_constant_lookup(tree, db, version, whitelist, idx, &lookup, out);
}

/// One constants-table lookup (shared by the per-token scan and
/// the single-lookup `use`/`namespace` pass below).
fn compat_constant_lookup(
    tree: &Tree,
    db: &serde_json::Value,
    version: &str,
    whitelist: &[WhiteEntry],
    idx: usize,
    lookup: &str,
    out: &mut Vec<RawFinding>,
) {
    if whitelisted(whitelist, WhiteKind::Constant, lookup, idx) {
        return;
    }
    let (min, max) = sym_min_max(db, "constants", lookup);
    if let Some(introduced) = min.filter(|found| sym_gt(found, version)) {
        push(
            out,
            45,
            tree.tokens[idx].offset,
            &format!(
                "This codebase targets PHP {version}, but `{lookup}` was not introduced until PHP \
                 {introduced}."
            ),
        );
    } else if let Some(removed) = max.filter(|found| sym_lt(found, version)) {
        push(
            out,
            45,
            tree.tokens[idx].offset,
            &format!(
                "This codebase targets PHP {version}, but `{lookup}` was removed in PHP {removed}."
            ),
        );
    }
}

/// Lookup name for a constant token: a `\`-prefixed name keeps its
/// separator so qualified names miss the table like PHP's
/// `getConcreteString()` lookups.
fn const_lookup_name(tree: &Tree, idx: usize, name: &str) -> String {
    let qualified = tree
        .prev_semantic(idx)
        .is_some_and(|prev| tree.tokens[prev].kind == "\\");
    if qualified {
        format!("\\{name}")
    } else {
        name.to_string()
    }
}

/// True for `T_STRING`s that are member, declaration, label,
/// or import names rather than constant references.
fn const_name_excluded(tree: &Tree, idx: usize) -> bool {
    const_after_member_op(tree, idx)
        || const_after_decl_kw(tree, idx)
        || const_after_attribute(tree, idx)
        || const_is_label(tree, idx)
        || const_followed_by_assign(tree, idx)
}

/// Member names after `->`, `?->`, and `::`.
fn const_after_member_op(tree: &Tree, idx: usize) -> bool {
    tree.prev_semantic(idx).is_some_and(|prev| {
        matches!(
            tree.tokens[prev].kind.as_str(),
            kinds::T_OBJECT_OPERATOR
                | kinds::T_NULLSAFE_OBJECT_OPERATOR
                | kinds::T_PAAMAYIM_NEKUDOTAYIM
        )
    })
}

/// Declaration and import names: after `function` (allowing `&`),
/// `use`, `as`, `namespace`, `const`, or `goto`.
fn const_after_decl_kw(tree: &Tree, idx: usize) -> bool {
    let start = qual_run_start(tree, idx);
    let mut prev = tree.prev_semantic(start);
    if prev.is_some_and(|found| tree.tokens[found].kind == "&") {
        prev = prev.and_then(|found| tree.prev_semantic(found));
    }
    prev.is_some_and(|found| {
        matches!(
            tree.tokens[found].kind.as_str(),
            kinds::T_FUNCTION
                | kinds::T_USE
                | kinds::T_AS
                | kinds::T_NAMESPACE
                | kinds::T_CONST
                | kinds::T_GOTO
        )
    })
}

/// Attribute names (`#[FOO]`).
fn const_after_attribute(tree: &Tree, idx: usize) -> bool {
    tree.prev_semantic(idx)
        .is_some_and(|prev| tree.tokens[prev].kind == kinds::T_ATTRIBUTE)
}

/// Labels (`FOO:`) are `n_LABEL`/`n_STRING` in PHP, never scanned;
/// `case FOO:` and ternary branches are expressions and still
/// scan, so only statement-start colons exclude. (A label directly
/// after another `:` — `case 1: FOO:` — still scans; see the
/// module notes.)
fn const_is_label(tree: &Tree, idx: usize) -> bool {
    if tree
        .next_semantic(idx)
        .is_none_or(|next| tree.tokens[next].kind != ":")
    {
        return false;
    }
    // A label name never leads the file (outside PHP mode it lexes
    // as inline HTML), so a previous token always exists.
    tree.prev_semantic(idx).is_none_or(|prev| {
        [
            ";",
            "{",
            "}",
            kinds::T_OPEN_TAG,
            kinds::T_OPEN_TAG_WITH_ECHO,
            kinds::T_CLOSE_TAG,
        ]
        .contains(&tree.tokens[prev].kind.as_str())
    })
}

/// `const` multi-declarators (`const A = 1, B = 2`): a bare name
/// followed by `=` can only declare a class constant.
fn const_followed_by_assign(tree: &Tree, idx: usize) -> bool {
    tree.next_semantic(idx)
        .is_some_and(|next| tree.tokens[next].kind == "=")
}

#[cfg(test)]
#[path = "xhp_rem1_tests.rs"]
mod xhp_rem1_tests;
