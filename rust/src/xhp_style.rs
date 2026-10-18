//! XHP style / spacing rules (T1128, divergence dv-c).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`, mirrored at
//! `%TEMP%\xhp_php\rules\` on this machine; read them with
//! `Get-Content` via PowerShell). ID 78 lives at
//! `src/lint/linter/ArcanistInlineHTMLXHPASTLinterRule.php`
//! (mirrored at `%TEMP%\xhp_php\ArcanistInlineHTMLXHPASTLinterRule.php`).
//!
//! Assigned rules (ID: PHP class):
//! - 6: ArcanistPHPShortTagXHPASTLinterRule (`<?`)
//! - 7: ArcanistPHPEchoTagXHPASTLinterRule (`<?=`)
//! - 8: ArcanistPHPCloseTagXHPASTLinterRule (`?>`)
//! - 15: ArcanistPHPOpenTagXHPASTLinterRule (must start with `<?php`)
//! - 16: ArcanistTodoCommentXHPASTLinterRule (TODO comments)
//! - 18: ArcanistCommentStyleXHPASTLinterRule (`#` comments)
//! - 25: ArcanistParenthesesSpacingXHPASTLinterRule
//! - 26: ArcanistControlStatementSpacingXHPASTLinterRule
//! - 27: ArcanistBinaryExpressionSpacingXHPASTLinterRule
//! - 28: ArcanistArrayIndexSpacingXHPASTLinterRule
//! - 34: ArcanistCommentSpacingXHPASTLinterRule
//! - 37: ArcanistCallParenthesesXHPASTLinterRule
//! - 38: ArcanistDeclarationParenthesesXHPASTLinterRule
//! - 40: ArcanistKeywordCasingXHPASTLinterRule
//! - 42: ArcanistElseIfUsageXHPASTLinterRule (`elseif`)
//! - 43: ArcanistSemicolonSpacingXHPASTLinterRule
//! - 44: ArcanistConcatenationOperatorXHPASTLinterRule
//! - 46: ArcanistLanguageConstructParenthesesXHPASTLinterRule
//! - 47: ArcanistEmptyStatementXHPASTLinterRule (empty `{}`)
//! - 48: ArcanistArraySeparatorXHPASTLinterRule (trailing commas)
//! - 49: ArcanistConstructorParenthesesXHPASTLinterRule (`new X`)
//! - 56: ArcanistUnnecessarySemicolonXHPASTLinterRule (`;;`)
//! - 58: ArcanistLogicalOperatorsXHPASTLinterRule (`and`/`or`)
//! - 66: ArcanistCastSpacingXHPASTLinterRule (`(int) $x`)
//! - 71: ArcanistModifierOrderingXHPASTLinterRule
//! - 73: ArcanistUnaryPrefixExpressionSpacingXHPASTLinterRule
//! - 74: ArcanistObjectOperatorSpacingXHPASTLinterRule (`->`)
//! - 75: ArcanistUnaryPostfixExpressionSpacingXHPASTLinterRule
//! - 78: ArcanistInlineHTMLXHPASTLinterRule (inline HTML)
//! - 81: ArcanistNewlineAfterOpenTagXHPASTLinterRule
//! - 82: ArcanistEmptyFileXHPASTLinterRule
//! - 83: ArcanistParentMemberReferenceXHPASTLinterRule
//! - 96: ArcanistPaamayimNekudotayimSpacingXHPASTLinterRule (`::`)
//! - 119: ArcanistCurlyBraceArrayIndexXHPASTLinterRule (`$x{0}`)
//! - 123: ArcanistVariableReferenceSpacingXHPASTLinterRule (`&$x`)
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (token scans,
//! `Tree::gap_text`, `Tree::calls`, `Tree::blocks`). Emit
//! [`super::xhp_ast::RawFinding`] with the PHP rule ID and the EXACT
//! PHP message text (`pht()` templates with `%s`/`%d` substituted
//! exactly as the PHP does, including quirks). Do not invent
//! messages. Spacing checks keyed on XHPAST "surrounding nonsemantic
//! tokens" map to `gap_text()` between adjacent semantic tokens.
//! Note the tokenizer normalizations in `xhp_ast` docs (open-tag
//! trivia, single-token strings). If a (sub-)check cannot be ported
//! meaningfully, leave it out and document the gap in the module
//! docs + final report; never silently diverge.
//!
//! Constraints: no new dependencies; every function cy<=15 / co<=20
//! (named private fns, same file; closures count into parents);
//! `cargo clippy --offline --all-targets -- -D warnings` clean;
//! `cargo fmt --check` clean. Touch ONLY this file and
//! `xhp_style_tests.rs`.
//!
//! Port notes: XHP74 also covers the nullsafe `?->` operator and
//! XHP40 also covers `fn` / `match` / `enum` / `readonly` keywords;
//! stock XHPAST predates these tokens (files using them fail to
//! parse there), so this only affects files PHP would report as
//! XHP2. XHP27 treats `declare(...)` `=` as exempt (PSR-12 mandates
//! no spaces there) and skips `=&` reference assignment.

use super::xhp_ast::{
    self, Block, BlockKind, BodySpan, RawFinding, RuleCtx, Token, Tree, is_comment, is_semantic,
    kinds,
};

/// Run all style rules, pushing findings into `out`.
pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    rule_short_tag(ctx, out);
    rule_echo_tag(ctx, out);
    rule_close_tag(ctx, out);
    rule_open_tag(ctx, out);
    rule_todo_comment(ctx, out);
    rule_comment_style(ctx, out);
    rule_elseif(ctx, out);
    rule_logical_operators(ctx, out);
    rule_inline_html(ctx, out);
    rule_newline_after_open_tag(ctx, out);
    rule_empty_file(ctx, out);
    rule_curly_brace_index(ctx, out);
    rule_paren_spacing(ctx, out);
    rule_control_spacing(ctx, out);
    rule_index_spacing(ctx, out);
    rule_comment_spacing(ctx, out);
    rule_call_parens(ctx, out);
    rule_semicolon_spacing(ctx, out);
    rule_concat_spacing(ctx, out);
    rule_empty_block(ctx, out);
    rule_new_parens(ctx, out);
    rule_unnecessary_semicolon(ctx, out);
    rule_cast_spacing(ctx, out);
    rule_object_op_spacing(ctx, out);
    rule_paamayim_spacing(ctx, out);
    rule_ref_spacing(ctx, out);
    rule_parent_ref(ctx, out);
    rule_binary_spacing(ctx, out);
    rule_decl_parens(ctx, out);
    rule_keyword_casing(ctx, out);
    rule_construct_parens(ctx, out);
    rule_array_separator(ctx, out);
    rule_modifier_ordering(ctx, out);
    rule_unary_prefix_spacing(ctx, out);
    rule_unary_postfix_spacing(ctx, out);
}

fn push(out: &mut Vec<RawFinding>, rule: u32, offset: usize, message: &str) {
    out.push(RawFinding::at(rule, offset, message.to_string()));
}

/// XHP6: the first `T_OPEN_TAG` must be the full `<?php` form.
fn rule_short_tag(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_OPEN_TAG {
            if token.value.trim() == "<?" {
                push(
                    out,
                    6,
                    token.offset,
                    "Use the full form of the PHP open tag, `<?php`.",
                );
            }
            break;
        }
    }
}

/// XHP7: every `<?=` tag is flagged.
fn rule_echo_tag(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_OPEN_TAG_WITH_ECHO {
            push(
                out,
                7,
                token.offset,
                "Avoid the PHP echo short form, `<?=`.",
            );
        }
    }
}

/// XHP8: `?>` is flagged unless the file has inline HTML.
fn rule_close_tag(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    if ctx
        .tree
        .tokens
        .iter()
        .any(|t| t.kind == kinds::T_INLINE_HTML)
    {
        return;
    }
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_CLOSE_TAG {
            push(
                out,
                8,
                token.offset,
                "Do not use the PHP closing tag, `?>`.",
            );
        }
    }
}

/// XHP15: the first token must be an open tag (or a `#!` line).
fn rule_open_tag(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let Some(first) = ctx.tree.tokens.first() else {
        return;
    };
    if first.kind == kinds::T_OPEN_TAG || first.kind == kinds::T_OPEN_TAG_WITH_ECHO {
        return;
    }
    if !first.value.starts_with("#!") {
        push(
            out,
            15,
            first.offset,
            "PHP files should start with `<?php`, which may be preceded by a `#!` line for scripts.",
        );
    }
}

/// XHP16: TODO markers in comments (one finding per match).
fn rule_todo_comment(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if !is_comment(token) {
            continue;
        }
        let doc = token.kind == kinds::T_DOC_COMMENT;
        for offset in todo_offsets(&token.value, doc) {
            push(out, 16, token.offset + offset, "This comment has a TODO.");
        }
    }
}

fn todo_offsets(value: &str, doc: bool) -> Vec<usize> {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if doc && bytes[i..].starts_with(b"@todo") {
            out.push(i);
            i += 5;
        } else if bytes[i..].starts_with(b"TODO") {
            out.push(i);
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

/// XHP18: `#` comments (except the `#[\\...]` attribute-compat hack).
fn rule_comment_style(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind != kinds::T_COMMENT || !token.value.starts_with('#') {
            continue;
        }
        if token.value.starts_with("#[\\") {
            continue;
        }
        push(
            out,
            18,
            token.offset,
            "Use `//` single-line comments, not `#`.",
        );
    }
}

/// XHP42: `elseif` should be spelled `else if`.
fn rule_elseif(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_ELSEIF {
            push(
                out,
                42,
                token.offset,
                "Usage of `else if` is preferred over `elseif`.",
            );
        }
    }
}

/// XHP58: `and` / `or` should be `&&` / `||`.
fn rule_logical_operators(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_LOGICAL_AND {
            push(out, 58, token.offset, "Use `&&` instead of `and`.");
        } else if token.kind == kinds::T_LOGICAL_OR {
            push(out, 58, token.offset, "Use `||` instead of `or`.");
        }
    }
}

/// XHP78: non-whitespace, non-shebang inline HTML (first only).
fn rule_inline_html(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind != kinds::T_INLINE_HTML {
            continue;
        }
        if token.value.starts_with("#!") {
            continue;
        }
        if token.value.bytes().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        push(
            out,
            78,
            token.offset,
            "PHP files must only contain PHP code.",
        );
        break;
    }
}

/// XHP81: `<?php` must be separated from code by an empty line.
fn rule_newline_after_open_tag(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_OPEN_TAG {
            continue;
        }
        if open_tag_has_blank_line(tree, i) {
            continue;
        }
        let offset = tree
            .tokens
            .get(i + 1)
            .map_or(token.offset, |next| next.offset);
        push(
            out,
            81,
            offset,
            "`<?php` should be separated from code by an empty line.",
        );
    }
}

fn open_tag_has_blank_line(tree: &Tree, open: usize) -> bool {
    let open_line = tree.line_of(tree.tokens[open].offset);
    let mut i = open + 1;
    while i < tree.tokens.len() {
        let token = &tree.tokens[i];
        if token.kind == kinds::T_WHITESPACE && has_blank_line(&token.value) {
            return true;
        }
        if tree.line_of(token.offset) != open_line {
            return false;
        }
        if token.kind == kinds::T_CLOSE_TAG {
            return true;
        }
        i += 1;
    }
    false
}

fn has_blank_line(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                if bytes[j] == b'\n' {
                    return true;
                }
                j += 1;
            }
        }
        i += 1;
    }
    false
}

/// XHP82: files with only tags/whitespace are empty (path-level).
fn rule_empty_file(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        match token.kind.as_str() {
            kinds::T_OPEN_TAG | kinds::T_CLOSE_TAG | kinds::T_WHITESPACE => {}
            _ => return,
        }
    }
    push(
        out,
        82,
        xhp_ast::PATH_LEVEL,
        "Empty files usually don't serve any useful purpose.",
    );
}

/// XHP119: `$x{'k'}` should be `$x['k']`.
fn rule_curly_brace_index(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    let body_opens: Vec<usize> = blocks
        .iter()
        .filter_map(|b| match b.body {
            xhp_ast::BodySpan::Brace { open, .. } => Some(open),
            _ => None,
        })
        .collect();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "{" || body_opens.contains(&i) {
            continue;
        }
        if !curly_index_before(tree, i) {
            continue;
        }
        push(
            out,
            119,
            index_object_start(tree, i),
            "Use `$x['key']` instead of `$x{'key'}`.",
        );
    }
}

fn curly_index_before(tree: &Tree, brace: usize) -> bool {
    let Some(prev) = tree.prev_semantic(brace) else {
        return false;
    };
    match tree.tokens[prev].kind.as_str() {
        kinds::T_VARIABLE | "]" => curly_index_after(tree, brace),
        kinds::T_STRING => !trait_use_brace(tree, prev) && curly_index_after(tree, brace),
        ")" => call_rparen(tree, prev) && curly_index_after(tree, brace),
        _ => false,
    }
}

fn trait_use_brace(tree: &Tree, name_idx: usize) -> bool {
    tree.prev_semantic(name_idx)
        .is_some_and(|p| tree.tokens[p].kind == kinds::T_USE)
}

fn call_rparen(tree: &Tree, rparen: usize) -> bool {
    tree.calls().iter().any(|call| call.rparen == Some(rparen))
}

fn curly_index_after(tree: &Tree, brace: usize) -> bool {
    tree.match_close(brace)
        .is_some_and(|close| tree.tokens[close].kind == "}")
}

fn is_only_spaces(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b == b' ')
}

fn is_all_whitespace(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_whitespace())
}

/// Trivia token indices strictly between `before` and `after`.
fn trivia_run(tree: &Tree, before: usize, after: usize) -> Vec<usize> {
    let mut run = Vec::new();
    let mut i = before + 1;
    while i < after && i < tree.tokens.len() {
        if !is_semantic(&tree.tokens[i]) {
            run.push(i);
        }
        i += 1;
    }
    run
}

fn trivia_text(tree: &Tree, run: &[usize]) -> String {
    run.iter().map(|i| tree.tokens[*i].value.as_str()).collect()
}

/// XHP25: parentheses should hug their contents.
fn rule_paren_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (lparen, rparen) in paren_groups(tree) {
        let open_run = trivia_run(tree, lparen, tree.next_semantic(lparen).unwrap_or(rparen));
        if open_run.is_empty() {
            continue;
        }
        if is_only_spaces(&trivia_text(tree, &open_run)) {
            push(out, 25, tree.tokens[open_run[0]].offset, hug_message());
        }
        let close_end = tree.prev_semantic(rparen).unwrap_or(lparen);
        let close_run = trivia_run(tree, close_end, rparen);
        if close_run != open_run && is_only_spaces(&trivia_text(tree, &close_run)) {
            push(out, 25, tree.tokens[close_run[0]].offset, hug_message());
        }
    }
}

fn hug_message() -> &'static str {
    "Parentheses should hug their contents."
}

/// Call, declaration, control, `array()`, and `list()` parens.
fn paren_groups(tree: &Tree) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    block_parens(tree, &mut groups);
    call_parens(tree, &mut groups);
    method_parens(tree, &mut groups);
    keyword_parens(tree, &mut groups);
    new_arg_parens(tree, &mut groups);
    groups
}

fn block_parens(tree: &Tree, groups: &mut Vec<(usize, usize)>) {
    for block in tree.blocks() {
        if block.kind == BlockKind::Declare {
            continue;
        }
        if let (Some(lparen), Some(rparen)) = (block.lparen, block.rparen) {
            groups.push((lparen, rparen));
        }
    }
}

fn call_parens(tree: &Tree, groups: &mut Vec<(usize, usize)>) {
    for call in tree.calls() {
        if call.is_construct {
            continue;
        }
        if let Some(rparen) = call.rparen {
            groups.push((call.lparen, rparen));
        }
    }
}

fn method_parens(tree: &Tree, groups: &mut Vec<(usize, usize)>) {
    for call in method_call_sites(tree) {
        if let Some(rparen) = call.rparen {
            groups.push((call.lparen, rparen));
        }
    }
}

fn keyword_parens(tree: &Tree, groups: &mut Vec<(usize, usize)>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_ARRAY && token.kind != kinds::T_LIST {
            continue;
        }
        let Some(lparen) = tree.next_semantic(i) else {
            continue;
        };
        if tree.tokens[lparen].kind != "(" {
            continue;
        }
        if let Some(rparen) = tree.match_close(lparen) {
            groups.push((lparen, rparen));
        }
    }
}

/// `new X(...)` argument parens.
fn new_arg_parens(tree: &Tree, groups: &mut Vec<(usize, usize)>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_NEW {
            continue;
        }
        let Some(name) = tree.next_semantic(i) else {
            continue;
        };
        if !is_new_name(tree, name) {
            continue;
        }
        let end = new_name_end(tree, name);
        let Some(lparen) = tree.next_semantic(end) else {
            continue;
        };
        if tree.tokens[lparen].kind != "(" {
            continue;
        }
        if let Some(rparen) = tree.match_close(lparen) {
            groups.push((lparen, rparen));
        }
    }
}

/// XHP26: exactly one space after control keywords.
fn rule_control_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if !is_control_keyword(&token.kind) {
            continue;
        }
        let end = tree.next_semantic(i).unwrap_or(tree.tokens.len());
        let run = trivia_run(tree, i, end);
        if run.is_empty() {
            push(
                out,
                26,
                token.offset,
                "Convention: put a space after control statements.",
            );
        } else if run.len() == 1 {
            control_single_trivia(tree, token, run[0], out);
        }
    }
}

fn is_control_keyword(kind: &str) -> bool {
    matches!(
        kind,
        kinds::T_IF
            | kinds::T_ELSE
            | kinds::T_FOR
            | kinds::T_FOREACH
            | kinds::T_WHILE
            | kinds::T_DO
            | kinds::T_SWITCH
            | kinds::T_CATCH
    )
}

fn control_single_trivia(tree: &Tree, keyword: &Token, trivia: usize, out: &mut Vec<RawFinding>) {
    if keyword.kind == kinds::T_ELSE || keyword.kind == kinds::T_DO {
        return;
    }
    let token = &tree.tokens[trivia];
    if token.kind == kinds::T_WHITESPACE && token.value != " " {
        push(
            out,
            26,
            token.offset,
            "Convention: put a single space after control statements.",
        );
    }
}

/// XHP28: no spaces before index brackets.
fn rule_index_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "[" || !is_index_bracket(tree, i) {
            continue;
        }
        let prev = tree.prev_semantic(i).unwrap_or(0);
        let gap = tree.gap_text(prev, i);
        if is_only_spaces(&gap) {
            let offset = tree.tokens[prev].offset + tree.tokens[prev].value.len();
            push(
                out,
                28,
                offset,
                "Convention: no spaces before index access.",
            );
        }
    }
}

fn is_index_bracket(tree: &Tree, bracket: usize) -> bool {
    let Some(prev) = tree.prev_semantic(bracket) else {
        return false;
    };
    match tree.tokens[prev].kind.as_str() {
        kinds::T_VARIABLE
        | kinds::T_STRING
        | "]"
        | kinds::T_CONSTANT_ENCAPSED_STRING
        | kinds::T_INTERPOLATED_STRING => true,
        ")" => call_rparen(tree, prev),
        _ => false,
    }
}

/// XHP34: `//` and `/*` comments need a space after the marker.
fn rule_comment_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind != kinds::T_COMMENT || token.value.starts_with('#') {
            continue;
        }
        if needs_comment_space(&token.value) {
            push(out, 34, token.offset, "Put space after comment start.");
        }
    }
}

fn needs_comment_space(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'/' {
        return false;
    }
    if bytes[1] != b'/' && bytes[1] != b'*' {
        return false;
    }
    let mut j = 1;
    while j < bytes.len() && (bytes[j] == b'/' || bytes[j] == b'*') {
        j += 1;
    }
    j < bytes.len() && bytes[j] != b'/' && bytes[j] != b'*' && !bytes[j].is_ascii_whitespace()
}

/// XHP37: no spaces around call parentheses.
fn rule_call_parens(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for group in call_paren_groups(tree) {
        let gap = tree.gap_text(group.name_idx, group.lparen);
        if is_all_whitespace(&gap) {
            let offset =
                tree.tokens[group.name_idx].offset + tree.tokens[group.name_idx].value.len();
            push(
                out,
                37,
                offset,
                "Convention: no spaces before opening parentheses.",
            );
        }
        call_paren_trailing(tree, &group, out);
    }
}

struct CallParens {
    name_idx: usize,
    lparen: usize,
    rparen: Option<usize>,
    args: Vec<(usize, usize)>,
    is_array: bool,
}

fn call_paren_trailing(tree: &Tree, group: &CallParens, out: &mut Vec<RawFinding>) {
    if group.is_array {
        return;
    }
    let Some(rparen) = group.rparen else {
        return;
    };
    if last_arg_is_heredoc(tree, group) {
        return;
    }
    let before = tree.prev_semantic(rparen).unwrap_or(group.lparen);
    let gap = tree.gap_text(before, rparen);
    if is_all_whitespace(&gap) {
        let offset = if before == group.lparen {
            tree.tokens[group.lparen].offset + 1
        } else {
            tree.tokens[before].offset + tree.tokens[before].value.len()
        };
        push(
            out,
            37,
            offset,
            "Convention: no spaces before closing parentheses.",
        );
    }
}

fn last_arg_is_heredoc(tree: &Tree, group: &CallParens) -> bool {
    let Some((first, last)) = group.args.last() else {
        return false;
    };
    if first != last {
        return false;
    }
    matches!(
        tree.tokens[*first].kind.as_str(),
        kinds::T_HEREDOC | kinds::T_NOWDOC
    )
}

fn call_paren_groups(tree: &Tree) -> Vec<CallParens> {
    let mut groups = Vec::new();
    for call in tree.calls() {
        if call.is_construct {
            continue;
        }
        groups.push(CallParens {
            name_idx: call.name_idx,
            lparen: call.lparen,
            rparen: call.rparen,
            args: call.args,
            is_array: false,
        });
    }
    method_paren_groups(tree, &mut groups);
    keyword_paren_groups(tree, &mut groups);
    groups
}

fn method_paren_groups(tree: &Tree, groups: &mut Vec<CallParens>) {
    for call in method_call_sites(tree) {
        groups.push(CallParens {
            name_idx: call.name_idx,
            lparen: call.lparen,
            rparen: call.rparen,
            args: call.args,
            is_array: false,
        });
    }
}

/// `$object->method(...)` call sites with split arguments.
fn method_call_sites(tree: &Tree) -> Vec<xhp_ast::CallSite> {
    let mut out = Vec::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_STRING {
            continue;
        }
        let Some(prev) = tree.prev_semantic(i) else {
            continue;
        };
        if tree.tokens[prev].kind != kinds::T_OBJECT_OPERATOR
            && tree.tokens[prev].kind != kinds::T_NULLSAFE_OBJECT_OPERATOR
        {
            continue;
        }
        let Some(lparen) = tree.next_semantic(i) else {
            continue;
        };
        if tree.tokens[lparen].kind != "(" {
            continue;
        }
        let rparen = tree.match_close(lparen);
        let args = rparen.map_or(Vec::new(), |r| tree.split_top_level(lparen, r));
        out.push(xhp_ast::CallSite {
            name: token.value.clone(),
            name_idx: i,
            lparen,
            rparen,
            args,
            is_construct: false,
        });
    }
    out
}

fn keyword_paren_groups(tree: &Tree, groups: &mut Vec<CallParens>) {
    for (i, token) in tree.tokens.iter().enumerate() {
        let is_array = token.kind == kinds::T_ARRAY;
        if !is_array && token.kind != kinds::T_LIST {
            continue;
        }
        let Some(lparen) = tree.next_semantic(i) else {
            continue;
        };
        if tree.tokens[lparen].kind != "(" {
            continue;
        }
        groups.push(CallParens {
            name_idx: i,
            lparen,
            rparen: tree.match_close(lparen),
            args: Vec::new(),
            is_array,
        });
    }
}

/// XHP43: no space before `;`.
fn rule_semicolon_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != ";" || i == 0 {
            continue;
        }
        if tree.tokens[i - 1].kind == kinds::T_WHITESPACE {
            push(
                out,
                43,
                tree.tokens[i - 1].offset,
                "Space found before semicolon.",
            );
        }
    }
}

/// XHP44: no spaces around `.`.
fn rule_concat_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "." {
            continue;
        }
        if i > 0 {
            concat_side(tree, i - 1, None, out);
        }
        if i + 1 < tree.tokens.len() {
            let after = if i + 2 < tree.tokens.len() {
                Some(i + 2)
            } else {
                None
            };
            concat_side(tree, i + 1, after, out);
        }
    }
}

fn concat_side(tree: &Tree, ws: usize, after: Option<usize>, out: &mut Vec<RawFinding>) {
    if tree.tokens[ws].kind != kinds::T_WHITESPACE {
        return;
    }
    if tree.tokens[ws].value.contains('\n') {
        return;
    }
    if after.is_some_and(|a| tree.tokens[a].kind == kinds::T_COMMENT) {
        return;
    }
    push(
        out,
        44,
        tree.tokens[ws].offset,
        "Convention: no spaces around string concatenation operator.",
    );
}

/// XHP47: empty `{}` blocks must not contain only whitespace.
fn rule_empty_block(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    let exempt = exempt_brace_opens(&blocks);
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "{" || exempt.contains(&i) {
            continue;
        }
        let Some(close) = tree.match_close(i) else {
            continue;
        };
        if close == i + 1 {
            continue;
        }
        let mut all_ws = true;
        for inner in &tree.tokens[i + 1..close] {
            if inner.kind != kinds::T_WHITESPACE {
                all_ws = false;
                break;
            }
        }
        if all_ws {
            push(
                out,
                47,
                token.offset,
                "Braces for an empty block statement shouldn't contain only whitespace.",
            );
        }
    }
}

fn exempt_brace_opens(blocks: &[Block]) -> Vec<usize> {
    blocks
        .iter()
        .filter(|b| {
            matches!(
                b.kind,
                BlockKind::Class
                    | BlockKind::Interface
                    | BlockKind::Trait
                    | BlockKind::Enum
                    | BlockKind::Switch
            )
        })
        .filter_map(|b| match b.body {
            BodySpan::Brace { open, .. } => Some(open),
            _ => None,
        })
        .collect()
}

/// Walk back over the indexed object expression.
fn index_object_start(tree: &Tree, brace: usize) -> usize {
    let mut start = brace;
    while let Some(prev) = tree.prev_semantic(start) {
        match tree.tokens[prev].kind.as_str() {
            kinds::T_VARIABLE
            | kinds::T_STRING
            | kinds::T_OBJECT_OPERATOR
            | kinds::T_NULLSAFE_OBJECT_OPERATOR
            | kinds::T_PAAMAYIM_NEKUDOTAYIM
            | "\\" => {
                start = prev;
            }
            "]" | ")" => {
                let Some(open) = tree.match_open(prev) else {
                    break;
                };
                start = open;
            }
            _ => break,
        }
    }
    tree.tokens[start].offset
}

/// XHP49: `new X` needs constructor parentheses.
fn rule_new_parens(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_NEW {
            continue;
        }
        let Some(name) = tree.next_semantic(i) else {
            continue;
        };
        if tree.tokens[name].kind == kinds::T_CLASS || tree.tokens[name].kind == "(" {
            continue;
        }
        if !is_new_name(tree, name) {
            continue;
        }
        let end = new_name_end(tree, name);
        let has_parens = tree
            .next_semantic(end)
            .is_some_and(|next| tree.tokens[next].kind == "(");
        if !has_parens {
            push(
                out,
                49,
                tree.tokens[name].offset,
                "Use parentheses when invoking a constructor.",
            );
        }
    }
}

fn is_new_name(tree: &Tree, idx: usize) -> bool {
    matches!(
        tree.tokens[idx].kind.as_str(),
        kinds::T_STRING | kinds::T_VARIABLE | kinds::T_STATIC | "\\"
    )
}

fn new_name_end(tree: &Tree, mut idx: usize) -> usize {
    loop {
        if tree.tokens[idx].kind != kinds::T_STRING && tree.tokens[idx].kind != "\\" {
            return idx;
        }
        let Some(next) = tree.next_semantic(idx) else {
            return idx;
        };
        if tree.tokens[next].kind != kinds::T_STRING && tree.tokens[next].kind != "\\" {
            return idx;
        }
        idx = next;
    }
}

/// XHP56: lone `;` statements.
fn rule_unnecessary_semicolon(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    let for_ranges = for_header_ranges(&blocks);
    let empty_semis = empty_body_semis(&blocks);
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != ";" {
            continue;
        }
        if for_ranges.iter().any(|(s, e)| i > *s && i < *e) {
            continue;
        }
        if empty_semis.contains(&i) {
            if !declare_empty_semi(&blocks, i) {
                push(
                    out,
                    56,
                    token.offset,
                    "Unnecessary semicolons after statement.",
                );
            }
            continue;
        }
        if lone_semicolon(tree, i) {
            push(
                out,
                56,
                token.offset,
                "Unnecessary semicolons after statement.",
            );
        }
    }
}

fn for_header_ranges(blocks: &[Block]) -> Vec<(usize, usize)> {
    blocks
        .iter()
        .filter(|b| b.kind == BlockKind::For)
        .filter_map(|b| match (b.lparen, b.rparen) {
            (Some(l), Some(r)) => Some((l, r)),
            _ => None,
        })
        .collect()
}

fn empty_body_semis(blocks: &[Block]) -> Vec<usize> {
    blocks
        .iter()
        .filter_map(|b| match b.body {
            BodySpan::Empty { semi } => Some(semi),
            _ => None,
        })
        .collect()
}

fn declare_empty_semi(blocks: &[Block], semi: usize) -> bool {
    blocks.iter().any(|b| {
        b.kind == BlockKind::Declare && matches!(b.body, BodySpan::Empty { semi: s } if s == semi)
    })
}

fn lone_semicolon(tree: &Tree, semi: usize) -> bool {
    let Some(prev) = tree.prev_semantic(semi) else {
        return true;
    };
    matches!(
        tree.tokens[prev].kind.as_str(),
        ";" | "{" | "}" | ":" | kinds::T_DO | kinds::T_ELSE
    )
}

/// XHP66: casts must hug their operand.
fn rule_cast_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "(" {
            continue;
        }
        let Some(close) = cast_close(tree, i) else {
            continue;
        };
        if close + 1 < tree.tokens.len() && !is_semantic(&tree.tokens[close + 1]) {
            push(
                out,
                66,
                tree.tokens[close + 1].offset,
                "A cast statement must not be followed by a space.",
            );
        }
    }
}

fn cast_close(tree: &Tree, open: usize) -> Option<usize> {
    if open + 2 >= tree.tokens.len() {
        return None;
    }
    if tree.tokens[open + 1].kind != kinds::T_STRING {
        return None;
    }
    if !is_cast_word(&tree.tokens[open + 1].value) {
        return None;
    }
    if tree.tokens[open + 2].kind != ")" {
        return None;
    }
    Some(open + 2)
}

fn is_cast_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "int"
            | "integer"
            | "bool"
            | "boolean"
            | "float"
            | "double"
            | "real"
            | "string"
            | "binary"
            | "array"
            | "object"
            | "unset"
    )
}

/// XHP74: `->` must hug its operands (newline allowed before).
fn rule_object_op_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        // PHP selects `T_OBJECT_OPERATOR` only; `?->` is exempt.
        if token.kind != kinds::T_OBJECT_OPERATOR {
            continue;
        }
        if let Some(prev) = tree.prev_semantic(i) {
            let gap = tree.gap_text(prev, i);
            if !gap.is_empty() && !gap.contains('\n') {
                push(
                    out,
                    74,
                    tree.tokens[prev].offset + tree.tokens[prev].value.len(),
                    "There should be no whitespace before the object operator.",
                );
            }
        }
        if let Some(next) = tree.next_semantic(i) {
            let gap = tree.gap_text(i, next);
            if !gap.is_empty() {
                push(
                    out,
                    74,
                    token.offset + token.value.len(),
                    "There should be no whitespace after the object operator.",
                );
            }
        }
    }
}

/// XHP96: no single-line whitespace around `::`.
fn rule_paamayim_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
            continue;
        }
        let mut j = i;
        while j > 0 && !is_semantic(&tree.tokens[j - 1]) {
            j -= 1;
            paamayim_trivia(tree, j, out);
        }
        let mut k = i + 1;
        while k < tree.tokens.len() && !is_semantic(&tree.tokens[k]) {
            paamayim_trivia(tree, k, out);
            k += 1;
        }
    }
}

fn paamayim_trivia(tree: &Tree, idx: usize, out: &mut Vec<RawFinding>) {
    if tree.tokens[idx].kind == kinds::T_WHITESPACE && !tree.tokens[idx].value.contains('\n') {
        push(
            out,
            96,
            tree.tokens[idx].offset,
            "Unnecessary whitespace around paamayim nekudotayim (double colon) operator.",
        );
    }
}

/// XHP123: `&$x` references must not have a gap.
fn rule_ref_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "&" {
            continue;
        }
        let Some(var) = tree.next_semantic(i) else {
            continue;
        };
        if tree.tokens[var].kind != kinds::T_VARIABLE {
            continue;
        }
        if !ref_amp_position(tree, i) {
            continue;
        }
        if !tree.gap_text(i, var).is_empty() {
            push(
                out,
                123,
                token.offset + 1,
                "Variable references should not be prefixed with whitespace.",
            );
        }
    }
}

fn ref_amp_position(tree: &Tree, amp: usize) -> bool {
    let Some(prev) = tree.prev_semantic(amp) else {
        return false;
    };
    matches!(
        tree.tokens[prev].kind.as_str(),
        "(" | "," | "=" | kinds::T_DOUBLE_ARROW | kinds::T_AS | "["
    )
}

/// XHP83: `ParentName::` inside a subclass should be `parent::`.
fn rule_parent_ref(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    let version_ok = ctx
        .options
        .php_version
        .as_ref()
        .is_none_or(|v| xhp_ast::version_compare(v, "5.4.0") != std::cmp::Ordering::Less);
    for block in &blocks {
        if block.kind != BlockKind::Class {
            continue;
        }
        let Some((parent, _)) = &block.extends else {
            continue;
        };
        let Some((start, end)) = class_body_span(tree, block) else {
            continue;
        };
        parent_ref_in_span(tree, &blocks, parent, start, end, version_ok, out);
    }
}

fn class_body_span(tree: &Tree, block: &Block) -> Option<(usize, usize)> {
    match block.body {
        BodySpan::Brace { open, close } => Some((open, close.unwrap_or(tree.tokens.len()))),
        _ => None,
    }
}

fn parent_ref_in_span(
    tree: &Tree,
    blocks: &[Block],
    parent: &str,
    start: usize,
    end: usize,
    version_ok: bool,
    out: &mut Vec<RawFinding>,
) {
    let mut i = start;
    while i < end && i < tree.tokens.len() {
        if tree.tokens[i].kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
            i += 1;
            continue;
        }
        if let Some(name_start) = static_name_start(tree, i) {
            let name: String = tree.tokens[name_start..i]
                .iter()
                .filter(|t| is_semantic(t))
                .map(|t| t.value.as_str())
                .collect();
            if name.eq_ignore_ascii_case(parent) && (version_ok || !in_closure(blocks, tree, i)) {
                push(
                    out,
                    83,
                    tree.tokens[name_start].offset,
                    "Use `parent::` to call parent method.",
                );
            }
        }
        i += 1;
    }
}

fn static_name_start(tree: &Tree, colon: usize) -> Option<usize> {
    let mut start = tree.prev_semantic(colon)?;
    if tree.tokens[start].kind != kinds::T_STRING && tree.tokens[start].kind != "\\" {
        return None;
    }
    while let Some(prev) = tree.prev_semantic(start) {
        if tree.tokens[prev].kind != kinds::T_STRING && tree.tokens[prev].kind != "\\" {
            break;
        }
        start = prev;
    }
    Some(start)
}

fn in_closure(blocks: &[Block], tree: &Tree, idx: usize) -> bool {
    blocks.iter().any(|b| {
        if !b.is_anonymous {
            return false;
        }
        if !matches!(b.kind, BlockKind::Function | BlockKind::ArrowFn) {
            return false;
        }
        let end = tree.body_last(b).unwrap_or(b.kw_idx);
        b.kw_idx < idx && idx < end
    })
}

/// Token kinds that end an operand (for unary/binary disambiguation).
const OPERAND_END: &[&str] = &[
    kinds::T_VARIABLE,
    kinds::T_LNUMBER,
    kinds::T_DNUMBER,
    kinds::T_CONSTANT_ENCAPSED_STRING,
    kinds::T_INTERPOLATED_STRING,
    kinds::T_HEREDOC,
    kinds::T_NOWDOC,
    kinds::T_BACKTICK,
    kinds::T_STRING,
    ")",
    "]",
    "}",
    kinds::T_INC,
    kinds::T_DEC,
    kinds::T_CLASS_C,
    kinds::T_FUNC_C,
    kinds::T_METHOD_C,
    kinds::T_FILE,
    kinds::T_DIR,
    kinds::T_LINE,
    kinds::T_NS_C,
    kinds::T_TRAIT_C,
];

fn prev_is_operand(tree: &Tree, idx: usize) -> bool {
    tree.prev_semantic(idx)
        .is_some_and(|p| OPERAND_END.contains(&tree.tokens[p].kind.as_str()))
}

/// Always-binary operator kinds (XHP27).
const BINARY_OPS: &[&str] = &[
    "*",
    "/",
    "%",
    kinds::T_POW,
    kinds::T_IS_EQUAL,
    kinds::T_IS_NOT_EQUAL,
    kinds::T_IS_IDENTICAL,
    kinds::T_IS_NOT_IDENTICAL,
    "<",
    ">",
    "<=",
    ">=",
    kinds::T_SPACESHIP,
    kinds::T_BOOLEAN_AND,
    kinds::T_BOOLEAN_OR,
    kinds::T_LOGICAL_AND,
    kinds::T_LOGICAL_OR,
    kinds::T_LOGICAL_XOR,
    "|",
    "^",
    kinds::T_SL,
    kinds::T_SR,
    kinds::T_COALESCE,
    "=",
    "+=",
    "-=",
    "*=",
    "/=",
    "%=",
    ".=",
    "&=",
    "|=",
    "^=",
    "<<=",
    ">>=",
    "**=",
    "??=",
];

/// XHP27: binary operators, commas, and `=>` need whitespace.
fn rule_binary_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let declare_ranges = declare_paren_ranges(tree);
    for (i, token) in tree.tokens.iter().enumerate() {
        match token.kind.as_str() {
            "," => comma_spacing(tree, i, out),
            kinds::T_DOUBLE_ARROW => arrow_spacing(tree, i, out),
            _ => binary_op_spacing(tree, &declare_ranges, i, out),
        }
    }
}

fn declare_paren_ranges(tree: &Tree) -> Vec<(usize, usize)> {
    tree.blocks()
        .iter()
        .filter(|b| b.kind == BlockKind::Declare)
        .filter_map(|b| match (b.lparen, b.rparen) {
            (Some(l), Some(r)) => Some((l, r)),
            _ => None,
        })
        .collect()
}

/// `=` inside property / constant / static declarations (not a
/// binary expression in XHPAST, so XHP27 skips it).
fn is_decl_equals(tree: &Tree, equals: usize) -> bool {
    let Some(prev) = tree.prev_semantic(equals) else {
        return false;
    };
    if tree.tokens[prev].kind != kinds::T_VARIABLE && tree.tokens[prev].kind != kinds::T_STRING {
        return false;
    }
    tree.prev_semantic(prev).is_some_and(|p| {
        matches!(
            tree.tokens[p].kind.as_str(),
            kinds::T_PUBLIC
                | kinds::T_PRIVATE
                | kinds::T_PROTECTED
                | kinds::T_VAR
                | kinds::T_STATIC
                | kinds::T_CONST
        )
    })
}

fn binary_op_spacing(
    tree: &Tree,
    declare_ranges: &[(usize, usize)],
    i: usize,
    out: &mut Vec<RawFinding>,
) {
    let kind = tree.tokens[i].kind.as_str();
    if !is_binary_op(tree, kind, i) {
        return;
    }
    if kind == "=" && is_exempt_equals(tree, declare_ranges, i) {
        return;
    }
    let before_empty = tree
        .prev_semantic(i)
        .is_none_or(|p| tree.gap_text(p, i).is_empty());
    let after_empty = tree
        .next_semantic(i)
        .is_none_or(|n| tree.gap_text(i, n).is_empty());
    if before_empty || after_empty {
        push(
            out,
            27,
            tree.tokens[i].offset,
            "Convention: logical and arithmetic operators should be surrounded by whitespace.",
        );
    }
}

fn is_binary_op(tree: &Tree, kind: &str, i: usize) -> bool {
    if kind == "+" || kind == "-" || kind == "&" {
        return prev_is_operand(tree, i);
    }
    BINARY_OPS.contains(&kind)
}

fn is_exempt_equals(tree: &Tree, declare_ranges: &[(usize, usize)], i: usize) -> bool {
    if tree
        .next_semantic(i)
        .is_some_and(|n| tree.tokens[n].kind == "&")
    {
        return true;
    }
    if declare_ranges.iter().any(|(s, e)| i > *s && i < *e) {
        return true;
    }
    is_decl_equals(tree, i)
}

fn comma_spacing(tree: &Tree, comma: usize, out: &mut Vec<RawFinding>) {
    let Some(next) = tree.tokens.get(comma + 1) else {
        return;
    };
    if next.kind != ")" && next.kind != kinds::T_WHITESPACE {
        push(
            out,
            27,
            tree.tokens[comma].offset,
            "Convention: comma should be followed by space.",
        );
    }
}

fn arrow_spacing(tree: &Tree, arrow: usize, out: &mut Vec<RawFinding>) {
    let prev_ws = arrow > 0 && tree.tokens[arrow - 1].kind == kinds::T_WHITESPACE;
    let next_ws = tree
        .tokens
        .get(arrow + 1)
        .is_some_and(|t| t.kind == kinds::T_WHITESPACE);
    if !prev_ws || !next_ws {
        push(
            out,
            27,
            tree.tokens[arrow].offset,
            "Convention: double arrow should be surrounded by whitespace.",
        );
    }
}

/// XHP38: declaration parentheses hug (except `function (`).
fn rule_decl_parens(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for block in &blocks {
        if block.kind != BlockKind::Function {
            continue;
        }
        let (Some(lparen), rparen) = (block.lparen, block.rparen) else {
            continue; // LCOV_EXCL_LINE // Reason: dead: parse_function returns None without `(`, so Function blocks always carry lparen
        };
        decl_paren_leading(tree, block, lparen, out);
        if let Some(rparen) = rparen {
            decl_paren_trailing(tree, lparen, rparen, out);
        }
        decl_use_spacing(tree, block, out);
    }
}

fn decl_paren_leading(tree: &Tree, block: &Block, lparen: usize, out: &mut Vec<RawFinding>) {
    let Some(prev) = tree.prev_semantic(lparen) else {
        return;
    };
    let gap = tree.gap_text(prev, lparen);
    let offset = tree.tokens[prev].offset + tree.tokens[prev].value.len();
    if block.name.is_none() {
        if gap != " " {
            push(
                out,
                38,
                offset,
                "Convention: space before opening parenthesis in anonymous function declarations.",
            );
        }
    } else if is_all_whitespace(&gap) {
        push(
            out,
            38,
            offset,
            "Convention: no spaces before opening parenthesis in function and method declarations.",
        );
    }
}

fn decl_paren_trailing(tree: &Tree, lparen: usize, rparen: usize, out: &mut Vec<RawFinding>) {
    let before = tree.prev_semantic(rparen).unwrap_or(lparen);
    let gap = tree.gap_text(before, rparen);
    if is_all_whitespace(&gap) {
        let offset = if before == lparen {
            tree.tokens[lparen].offset + 1
        } else {
            tree.tokens[before].offset + tree.tokens[before].value.len()
        };
        push(
            out,
            38,
            offset,
            "Convention: no spaces before closing parenthesis in function and method declarations.",
        );
    }
}

fn decl_use_spacing(tree: &Tree, block: &Block, out: &mut Vec<RawFinding>) {
    let (Some(rparen), Some((use_lparen, _))) = (block.rparen, block.closure_use) else {
        return;
    };
    let mut use_idx = None;
    let mut i = rparen + 1;
    while i < use_lparen {
        if tree.tokens[i].kind == kinds::T_USE {
            use_idx = Some(i);
            break;
        }
        i += 1;
    }
    let Some(use_idx) = use_idx else {
        return;
    };
    if tree
        .prev_semantic(use_idx)
        .is_some_and(|p| tree.gap_text(p, use_idx).is_empty())
    {
        push(
            out,
            38,
            tree.tokens[use_idx].offset,
            "Convention: space before `use` token.",
        );
    }
    if tree.gap_text(use_idx, use_lparen).is_empty() {
        push(
            out,
            38,
            tree.tokens[use_idx].offset + tree.tokens[use_idx].value.len(),
            "Convention: space after `use` token.",
        );
    }
}

/// XHP40 keyword kinds (PHP list plus modern `fn`/`match`/`enum`).
const KEYWORD_CASE_KINDS: &[&str] = &[
    kinds::T_ABSTRACT,
    kinds::T_ARRAY,
    kinds::T_AS,
    kinds::T_BREAK,
    kinds::T_CALLABLE,
    kinds::T_CASE,
    kinds::T_CATCH,
    kinds::T_CLASS,
    kinds::T_CLONE,
    kinds::T_CONST,
    kinds::T_CONTINUE,
    kinds::T_DECLARE,
    kinds::T_DEFAULT,
    kinds::T_DO,
    kinds::T_ECHO,
    kinds::T_ELSE,
    kinds::T_ELSEIF,
    kinds::T_EMPTY,
    kinds::T_ENDDECLARE,
    kinds::T_ENDFOR,
    kinds::T_ENDFOREACH,
    kinds::T_ENDIF,
    kinds::T_ENDSWITCH,
    kinds::T_ENDWHILE,
    kinds::T_EVAL,
    kinds::T_EXIT,
    kinds::T_EXTENDS,
    kinds::T_FINAL,
    kinds::T_FINALLY,
    kinds::T_FOR,
    kinds::T_FOREACH,
    kinds::T_FUNCTION,
    kinds::T_GLOBAL,
    kinds::T_GOTO,
    kinds::T_HALT_COMPILER,
    kinds::T_IF,
    kinds::T_IMPLEMENTS,
    kinds::T_INCLUDE,
    kinds::T_INCLUDE_ONCE,
    kinds::T_INSTANCEOF,
    kinds::T_INSTEADOF,
    kinds::T_INTERFACE,
    kinds::T_ISSET,
    kinds::T_LIST,
    kinds::T_LOGICAL_AND,
    kinds::T_LOGICAL_OR,
    kinds::T_LOGICAL_XOR,
    kinds::T_NAMESPACE,
    kinds::T_NEW,
    kinds::T_PRINT,
    kinds::T_PRIVATE,
    kinds::T_PROTECTED,
    kinds::T_PUBLIC,
    kinds::T_REQUIRE,
    kinds::T_REQUIRE_ONCE,
    kinds::T_RETURN,
    kinds::T_STATIC,
    kinds::T_SWITCH,
    kinds::T_THROW,
    kinds::T_TRAIT,
    kinds::T_TRY,
    kinds::T_UNSET,
    kinds::T_USE,
    kinds::T_VAR,
    kinds::T_WHILE,
    kinds::T_YIELD,
    kinds::T_FN,
    kinds::T_MATCH,
    kinds::T_ENUM,
    kinds::T_READONLY,
];

const MAGIC_CONST_KINDS: &[&str] = &[
    kinds::T_CLASS_C,
    kinds::T_METHOD_C,
    kinds::T_FUNC_C,
    kinds::T_LINE,
    kinds::T_FILE,
    kinds::T_NS_C,
    kinds::T_DIR,
    kinds::T_TRAIT_C,
];

/// XHP40: keywords lowercase, magic constants uppercase.
fn rule_keyword_casing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for token in &tree.tokens {
        if KEYWORD_CASE_KINDS.contains(&token.kind.as_str()) {
            let lower = token.value.to_ascii_lowercase();
            if token.value != lower {
                push(
                    out,
                    40,
                    token.offset,
                    &format!(
                        "Convention: spell keyword `{}` as `{}`.",
                        token.value, lower
                    ),
                );
            }
        } else if MAGIC_CONST_KINDS.contains(&token.kind.as_str()) {
            let upper = token.value.to_ascii_uppercase();
            if token.value != upper {
                push(
                    out,
                    40,
                    token.offset,
                    "Magic constants should be uppercase.",
                );
            }
        }
    }
    self_parent_casing(tree, out);
    boolean_null_casing(tree, out);
}

fn self_parent_casing(tree: &Tree, out: &mut Vec<RawFinding>) {
    let ranges = class_body_ranges(tree);
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_PAAMAYIM_NEKUDOTAYIM {
            continue;
        }
        if !ranges.iter().any(|(s, e)| i > *s && i < *e) {
            continue;
        }
        let Some(name) = tree.prev_semantic(i) else {
            continue; // LCOV_EXCL_LINE // Reason: dead: the class brace precedes i, so prev always exists
        };
        if tree.tokens[name].kind != kinds::T_STRING {
            continue;
        }
        let lower = tree.tokens[name].value.to_ascii_lowercase();
        if (lower == "self" || lower == "parent") && tree.tokens[name].value != lower {
            push(
                out,
                40,
                tree.tokens[name].offset,
                &format!(
                    "Convention: spell keyword `{}` as `{}`.",
                    tree.tokens[name].value, lower
                ),
            );
        }
    }
}

fn class_body_ranges(tree: &Tree) -> Vec<(usize, usize)> {
    tree.blocks()
        .iter()
        .filter(|b| {
            matches!(
                b.kind,
                BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
            )
        })
        .filter_map(|b| match b.body {
            BodySpan::Brace { open, close } => Some((open, close.unwrap_or(tree.tokens.len()))),
            _ => None,
        })
        .collect()
}

/// Positions where true/false/null are names, not literals.
const BOOLEAN_NAME_PREV: &[&str] = &[
    kinds::T_FUNCTION,
    kinds::T_CLASS,
    kinds::T_INTERFACE,
    kinds::T_TRAIT,
    kinds::T_ENUM,
    kinds::T_CONST,
    kinds::T_NAMESPACE,
    kinds::T_USE,
    kinds::T_AS,
    kinds::T_EXTENDS,
    kinds::T_IMPLEMENTS,
    kinds::T_NEW,
    kinds::T_GOTO,
    kinds::T_INSTANCEOF,
    kinds::T_OBJECT_OPERATOR,
    kinds::T_NULLSAFE_OBJECT_OPERATOR,
    kinds::T_PAAMAYIM_NEKUDOTAYIM,
    "\\",
];

fn boolean_null_casing(tree: &Tree, out: &mut Vec<RawFinding>) {
    let callees: Vec<usize> = tree.calls().iter().map(|c| c.name_idx).collect();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_STRING {
            continue;
        }
        let lower = token.value.to_ascii_lowercase();
        if lower != "true" && lower != "false" && lower != "null" {
            continue;
        }
        if token.value == lower {
            continue;
        }
        if callees.contains(&i) {
            continue;
        }
        if tree
            .prev_semantic(i)
            .is_some_and(|p| BOOLEAN_NAME_PREV.contains(&tree.tokens[p].kind.as_str()))
        {
            continue;
        }
        push(
            out,
            40,
            token.offset,
            &format!(
                "Convention: spell keyword `{}` as `{}`.",
                token.value, lower
            ),
        );
    }
}

/// XHP46: `echo` / `include` family need no parentheses.
fn rule_construct_parens(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        match token.kind.as_str() {
            kinds::T_INCLUDE
            | kinds::T_INCLUDE_ONCE
            | kinds::T_REQUIRE
            | kinds::T_REQUIRE_ONCE
            | kinds::T_ECHO => {}
            _ => continue,
        }
        if tree
            .next_semantic(i)
            .is_some_and(|n| tree.tokens[n].kind == "(")
        {
            let lparen = tree.next_semantic(i).unwrap_or(i);
            push(
                out,
                46,
                tree.tokens[lparen].offset,
                "Language constructs do not require parentheses.",
            );
        }
    }
}

/// XHP48: array separators and closing-bracket placement.
fn rule_array_separator(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (open, close) in array_literals(tree) {
        let values = tree.split_top_level(open, close);
        if values.is_empty() {
            continue;
        }
        let multiline =
            tree.line_of(tree.tokens[open].offset) != tree.line_of(tree.tokens[close].offset);
        let (first, last) = values[values.len() - 1];
        let comma = last + 1 < tree.tokens.len() && tree.tokens[last + 1].kind == ",";
        if multiline {
            array_multiline(tree, close, first, last, comma, out);
        } else if comma {
            push(
                out,
                48,
                tree.tokens[last + 1].offset,
                "Single lined arrays should not have a trailing comma.",
            );
        }
    }
}

fn array_multiline(
    tree: &Tree,
    close: usize,
    first: usize,
    last: usize,
    comma: bool,
    out: &mut Vec<RawFinding>,
) {
    if !comma {
        if last_value_is_heredoc(tree, first, last) {
            return;
        }
        push(
            out,
            48,
            tree.tokens[first].offset,
            "Multi-lined arrays should have trailing commas.",
        );
    } else if tree.line_of(tree.tokens[first].offset) == tree.line_of(tree.tokens[close].offset) {
        push(
            out,
            48,
            tree.tokens[close].offset,
            "Closing parenthesis should be on a new line.",
        );
    }
}

fn last_value_is_heredoc(tree: &Tree, first: usize, last: usize) -> bool {
    let mut start = first;
    let mut depth = 0;
    let mut i = first;
    while i <= last {
        let kind = tree.tokens[i].kind.as_str();
        if kind == "(" || kind == "[" || kind == "{" {
            depth += 1;
        } else if kind == ")" || kind == "]" || kind == "}" {
            depth -= 1;
        } else if depth == 0 && kind == kinds::T_DOUBLE_ARROW {
            start = i + 1;
        }
        i += 1;
    }
    while start <= last && !is_semantic(&tree.tokens[start]) {
        start += 1;
    }
    start == last
        && matches!(
            tree.tokens[last].kind.as_str(),
            kinds::T_HEREDOC | kinds::T_NOWDOC
        )
}

fn array_literals(tree: &Tree) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind == kinds::T_ARRAY {
            let Some(lparen) = tree.next_semantic(i) else {
                continue;
            };
            if tree.tokens[lparen].kind != "(" {
                continue;
            }
            if let Some(rparen) = tree.match_close(lparen) {
                out.push((lparen, rparen));
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

fn is_attribute_bracket(tree: &Tree, bracket: usize) -> bool {
    tree.prev_semantic(bracket)
        .is_some_and(|p| tree.tokens[p].kind == kinds::T_ATTRIBUTE)
}

/// XHP71: canonical modifier order on methods and properties.
fn rule_modifier_ordering(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for (idx, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function || !tree.is_method(&blocks, idx) {
            continue;
        }
        check_modifier_run(tree, &method_modifier_run(tree, block), METHOD_ORDER, out);
    }
    for (start, end) in class_body_ranges(tree) {
        check_property_runs(tree, start, end, out);
    }
}

const METHOD_ORDER: &[&str] = &[
    "abstract",
    "final",
    "public",
    "protected",
    "private",
    "static",
];
const PROPERTY_ORDER: &[&str] = &["public", "protected", "private", "static"];

fn method_modifier_run(tree: &Tree, block: &Block) -> Vec<(String, usize)> {
    let mut run = Vec::new();
    let mut i = block.kw_idx;
    while i > 0 {
        i -= 1;
        let kind = tree.tokens[i].kind.as_str();
        if is_modifier_word(kind) {
            run.push((tree.tokens[i].value.clone(), i));
        } else if kind == "]" {
            let Some(open) = tree.match_open(i) else {
                break;
            };
            i = open;
        } else if !is_semantic(&tree.tokens[i]) {
            continue;
        } else {
            break;
        }
    }
    run.reverse();
    run
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

fn check_modifier_run(
    tree: &Tree,
    run: &[(String, usize)],
    canon: &[&str],
    out: &mut Vec<RawFinding>,
) {
    if run.is_empty() {
        return;
    }
    let words: Vec<&str> = run.iter().map(|(w, _)| w.as_str()).collect();
    let expected: Vec<&str> = canon
        .iter()
        .filter(|m| words.contains(m))
        .copied()
        .collect();
    if words.len() != expected.len() {
        return;
    }
    if words != expected {
        push(
            out,
            71,
            tree.tokens[run[0].1].offset,
            "Non-conventional modifier ordering.",
        );
    }
}

fn check_property_runs(tree: &Tree, start: usize, end: usize, out: &mut Vec<RawFinding>) {
    let mut i = start + 1;
    while i < end {
        let kind = tree.tokens[i].kind.as_str();
        if kind == "{" {
            i = tree.match_close(i).map_or(end, |c| c + 1);
            continue;
        }
        if (is_modifier_word(kind) || kind == kinds::T_VARIABLE)
            && let Some(stmt_end) = statement_end(tree, i, end)
        {
            check_property_statement(tree, i, stmt_end, out);
            i = stmt_end + 1;
            continue;
        }
        i += 1;
    }
}

fn statement_end(tree: &Tree, from: usize, end: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = from;
    while i < end {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            ";" if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn check_property_statement(tree: &Tree, from: usize, end: usize, out: &mut Vec<RawFinding>) {
    let mut run: Vec<(String, usize)> = Vec::new();
    let mut i = from;
    while i < end {
        if is_modifier_word(tree.tokens[i].kind.as_str()) {
            run.push((tree.tokens[i].value.clone(), i));
        } else if !is_semantic(&tree.tokens[i]) {
        } else {
            break;
        }
        i += 1;
    }
    if run.is_empty() {
        return;
    }
    if tree.tokens[i].kind != kinds::T_VARIABLE {
        return;
    }
    let offset = tree.tokens[run[0].1].offset;
    let words: Vec<&str> = run.iter().map(|(w, _)| w.as_str()).collect();
    let expected: Vec<&str> = PROPERTY_ORDER
        .iter()
        .filter(|m| words.contains(m))
        .copied()
        .collect();
    if words.len() == expected.len() && words != expected {
        push(out, 71, offset, "Non-conventional modifier ordering.");
    }
}

/// XHP73: unary prefix operators hug their operand.
fn rule_unary_prefix_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        match token.kind.as_str() {
            "+" | "-" | "!" | "~" => {
                if prev_is_operand(tree, i) {
                    continue;
                }
            }
            "@" => {}
            kinds::T_INC | kinds::T_DEC => {
                if prev_is_operand(tree, i) {
                    continue;
                }
            }
            _ => continue,
        }
        if tree
            .next_semantic(i)
            .is_some_and(|n| !tree.gap_text(i, n).is_empty())
        {
            push(
                out,
                73,
                token.offset + token.value.len(),
                "Unary prefix operators should not be followed by whitespace.",
            );
        }
    }
}

/// XHP75: unary postfix operators hug their operand.
fn rule_unary_postfix_spacing(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_INC && token.kind != kinds::T_DEC {
            continue;
        }
        let Some(prev) = tree.prev_semantic(i) else {
            continue;
        };
        if !OPERAND_END.contains(&tree.tokens[prev].kind.as_str()) {
            continue;
        }
        if !tree.gap_text(prev, i).is_empty() {
            push(
                out,
                75,
                tree.tokens[prev].offset + tree.tokens[prev].value.len(),
                "Unary postfix operators should not be prefixed by whitespace.",
            );
        }
    }
}

#[cfg(test)]
#[path = "xhp_style_tests.rs"]
mod xhp_style_tests;
