//! XHP flow / scoping rules: switches, loops, variables (T1128, dv-c).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`, mirrored at
//! `%TEMP%\xhp_php\rules\` on this machine; read them with
//! `Get-Content` via PowerShell).
//!
//! Assigned rules (ID: PHP class):
//! - 3: ArcanistVariableVariableXHPASTLinterRule (`$$x`)
//! - 5: ArcanistUndeclaredVariableXHPASTLinterRule (use before
//!   declaration; function/method bodies only; note `$this` +
//!   superglobals are predeclared, `isset`/`empty` + static-access
//!   variables are excluded, `extract()`/`$$x`/dynamic-globals
//!   destroy scope tracking)
//! - 17: ArcanistExitExpressionXHPASTLinterRule (`exit` as expression;
//!   statement-position `exit`/`exit;` and call-form `exit(...)` are OK)
//! - 20: ArcanistTautologicalExpressionXHPASTLinterRule (identical
//!   operands / static logical value)
//! - 22: ArcanistDuplicateKeysInArrayXHPASTLinterRule
//! - 23: ArcanistReusedIteratorXHPASTLinterRule (nested foreach)
//! - 30: ArcanistImplicitFallthroughXHPASTLinterRule (note the
//!   `$statment_ok` typo quirk: any top-level break/continue/return/
//!   throw/exit ANYWHERE in the arm (not just at the end) silences
//!   it; `;` counting includes nested tokens; `// fall...` comments
//!   silence; the switchhook override is not portable)
//! - 32: ArcanistReusedAsIteratorXHPASTLinterRule
//! - 39: ArcanistReusedIteratorReferenceXHPASTLinterRule
//! - 45: ArcanistPHPCompatibilityXHPASTLinterRule (version-gated; the
//!   1.4MB `resources/php/symbol-information.json` table checks are
//!   NOT portable — port the early return without
//!   `xhpast.php-version` plus the PHP 5.3 / 5.4 syntax-feature
//!   checks with exact messages; use `xhp_ast::version_compare`)
//! - 50: ArcanistDuplicateSwitchCaseXHPASTLinterRule
//! - 69: ArcanistInstanceOfOperatorXHPASTLinterRule
//! - 76: ArcanistArrayValueXHPASTLinterRule (one element per line)
//! - 77: ArcanistListAssignmentXHPASTLinterRule (`list()` commas)
//! - 79: ArcanistGlobalVariableXHPASTLinterRule (`global`)
//! - 90: ArcanistNestedNamespacesXHPASTLinterRule
//! - 97: ArcanistUseStatementNamespacePrefixXHPASTLinterRule
//! - 98: ArcanistNamespaceFirstStatementXHPASTLinterRule
//! - 99: ArcanistUnnecessarySymbolAliasXHPASTLinterRule (`use X as X`)
//! - 128: ArcanistContinueInsideSwitchXHPASTLinterRule
//! - 132: ArcanistPartialCatchXHPASTLinterRule (`catch (Exception`)
//!
//! Ported in this subset: 3, 79, 90, 97, 98, 99. The rest (5, 17,
//! 20, 22, 23, 30, 32, 39, 45, 50, 69, 76, 77, 128, 132) were
//! deferred, then ported by the remainder increment; see
//! `super::xhp_rem1`, `super::xhp_rem2`, `super::xhp_rem3`.
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (`Tree::blocks`
//! with `cases`/`linked`, token scans). Emit
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
//! `cargo fmt --check` clean. Touch ONLY this file and
//! `xhp_flow_tests.rs`.

use super::xhp_ast::{self, BlockKind, RawFinding, RuleCtx, Tree, is_semantic, kinds};

/// Run all flow rules, pushing findings into `out`.
pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    rule_variable_variable(ctx, out);
    rule_global_variable(ctx, out);
    rule_nested_namespaces(ctx, out);
    rule_use_prefix(ctx, out);
    rule_namespace_first(ctx, out);
    rule_symbol_alias(ctx, out);
}

fn push(out: &mut Vec<RawFinding>, rule: u32, offset: usize, message: &str) {
    out.push(RawFinding::at(rule, offset, message.to_string()));
}

/// XHP3: `$$x` and `${...}` dynamic variables.
fn rule_variable_variable(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != "$" {
            continue;
        }
        let dynamic = tree.next_semantic(i).is_some_and(|n| {
            tree.tokens[n].kind == kinds::T_VARIABLE || tree.tokens[n].kind == "{"
        });
        if dynamic {
            push(
                out,
                3,
                token.offset,
                "Rewrite this code to use an array. Variable variables are unclear and hinder \
                 static analysis.",
            );
        }
    }
}

/// XHP79: `global` declarations.
fn rule_global_variable(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for token in &ctx.tree.tokens {
        if token.kind == kinds::T_GLOBAL {
            push(
                out,
                79,
                token.offset,
                "Limit the use of global variables. Global variables are generally a bad idea \
                 and should be avoided when possible.",
            );
        }
    }
}

/// XHP90: namespaces nested inside namespaces.
fn rule_nested_namespaces(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    let blocks = tree.blocks();
    for block in &blocks {
        if block.kind != BlockKind::Namespace {
            continue;
        }
        if block
            .parent
            .is_some_and(|p| blocks[p].kind == BlockKind::Namespace)
        {
            push(
                out,
                90,
                tree.tokens[block.kw_idx].offset,
                "`namespace` declarations cannot be nested. This construct will cause a PHP \
                 fatal error.",
            );
        }
    }
}

/// A top-level `use` import statement (not trait/closure use).
struct ImportUse {
    use_idx: usize,
    end: usize,
}

fn import_uses(tree: &Tree) -> Vec<ImportUse> {
    let blocks = tree.blocks();
    let class_ranges: Vec<(usize, usize)> = blocks
        .iter()
        .filter(|b| {
            matches!(
                b.kind,
                BlockKind::Class | BlockKind::Interface | BlockKind::Trait | BlockKind::Enum
            )
        })
        .filter_map(|b| {
            let last = tree.body_last(b)?;
            Some((b.kw_idx, last))
        })
        .collect();
    let mut out = Vec::new();
    for (i, token) in tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_USE {
            continue;
        }
        if class_ranges.iter().any(|(s, e)| i > *s && i < *e) {
            continue;
        }
        if is_closure_use(tree, &blocks, i) {
            continue;
        }
        if let Some(end) = import_end(tree, i) {
            out.push(ImportUse { use_idx: i, end });
        }
    }
    out
}

fn is_closure_use(tree: &Tree, blocks: &[xhp_ast::Block], use_idx: usize) -> bool {
    let Some(prev) = tree.prev_semantic(use_idx) else {
        return false;
    };
    if tree.tokens[prev].kind != ")" {
        return false;
    }
    blocks.iter().any(|b| {
        matches!(b.kind, BlockKind::Function | BlockKind::ArrowFn) && b.rparen == Some(prev)
    })
}

fn import_end(tree: &Tree, use_idx: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = use_idx + 1;
    while i < tree.tokens.len() {
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

/// One imported symbol: start index and full concrete name.
struct ImportSymbol {
    start: usize,
    name: String,
    alias: Option<String>,
}

fn import_symbols(tree: &Tree, stmt: &ImportUse) -> Vec<ImportSymbol> {
    let mut symbols = Vec::new();
    let mut i = tree.next_semantic(stmt.use_idx).unwrap_or(stmt.end);
    if tree.tokens[i].kind == kinds::T_FUNCTION || tree.tokens[i].kind == kinds::T_CONST {
        i = tree.next_semantic(i).unwrap_or(stmt.end);
    }
    if has_group_brace(tree, i, stmt.end) {
        group_symbols(tree, i, stmt.end, &mut symbols);
    } else {
        flat_symbols(tree, i, stmt.end, &mut symbols);
    }
    symbols
}

fn has_group_brace(tree: &Tree, from: usize, end: usize) -> bool {
    let mut depth = 0;
    let mut i = from;
    while i < end {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" => depth += 1,
            ")" | "]" => depth -= 1,
            "{" if depth == 0 => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

fn group_symbols(tree: &Tree, from: usize, end: usize, symbols: &mut Vec<ImportSymbol>) {
    let mut brace = from;
    while brace < end && tree.tokens[brace].kind != "{" {
        brace += 1;
    }
    let Some(close) = tree.match_close(brace) else {
        return;
    };
    for (first, last) in tree.split_top_level(brace, close.min(end)) {
        if let Some(symbol) = parse_symbol(tree, first, last) {
            symbols.push(symbol);
        }
    }
}

fn flat_symbols(tree: &Tree, from: usize, end: usize, symbols: &mut Vec<ImportSymbol>) {
    let mut depth = 0;
    let mut start = from;
    let mut i = from;
    while i <= end && i < tree.tokens.len() {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            "," if depth == 0 => {
                if let Some(symbol) = parse_symbol_bounded(tree, start, i) {
                    symbols.push(symbol);
                }
                start = i + 1;
            }
            ";" if depth == 0 => {
                if let Some(symbol) = parse_symbol_bounded(tree, start, i) {
                    symbols.push(symbol);
                }
                break;
            }
            _ => {}
        }
        i += 1;
    }
}

fn parse_symbol_bounded(tree: &Tree, start: usize, end: usize) -> Option<ImportSymbol> {
    let mut first = start;
    while first < end && !is_semantic(&tree.tokens[first]) {
        first += 1;
    }
    let mut last = end;
    while last > first && !is_semantic(&tree.tokens[last - 1]) {
        last -= 1;
    }
    if first >= last {
        return None;
    }
    parse_symbol(tree, first, last - 1)
}

fn parse_symbol(tree: &Tree, first: usize, last: usize) -> Option<ImportSymbol> {
    let mut alias_at = None;
    let mut i = first;
    while i <= last {
        if tree.tokens[i].kind == kinds::T_AS {
            alias_at = Some(i);
            break;
        }
        i += 1;
    }
    let name_end = alias_at.unwrap_or(last + 1);
    let mut name = String::new();
    let mut start = None;
    let mut i = first;
    while i < name_end {
        if is_semantic(&tree.tokens[i]) {
            if start.is_none() {
                start = Some(i);
            }
            name.push_str(&tree.tokens[i].value);
        }
        i += 1;
    }
    let start = start?;
    if name.is_empty() {
        return None;
    }
    let alias = alias_at.and_then(|a| {
        let mut j = a + 1;
        while j <= last && !is_semantic(&tree.tokens[j]) {
            j += 1;
        }
        if j <= last {
            Some(tree.tokens[j].value.clone())
        } else {
            None
        }
    });
    Some(ImportSymbol { start, name, alias })
}

/// XHP97: import symbols must not start with `\`.
fn rule_use_prefix(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for stmt in import_uses(tree) {
        for symbol in import_symbols(tree, &stmt) {
            if symbol.name.starts_with('\\') {
                push(
                    out,
                    97,
                    tree.tokens[symbol.start].offset,
                    "Imported symbols should not be prefixed with `\\`.",
                );
            }
        }
    }
}

/// XHP98: a file with `namespace` must start with it.
fn rule_namespace_first(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    if !tree.tokens.iter().any(|t| t.kind == kinds::T_NAMESPACE) {
        return;
    }
    let first = first_statement(tree).expect("a namespace token is itself a first statement");
    if tree.tokens[first].kind == kinds::T_NAMESPACE {
        return;
    }
    let node_type = statement_node_type(tree, first);
    push(
        out,
        98,
        tree.tokens[first].offset,
        &format!(
            "A script which contains a `namespace` statement expects the very first statement \
             to be a `namespace` statement. Otherwise, a PHP fatal error will occur. {node_type}"
        ),
    );
}

/// First code token after the open tag (if any).
fn first_statement(tree: &Tree) -> Option<usize> {
    let mut i = 0;
    while i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if kind == kinds::T_OPEN_TAG
            || kind == kinds::T_OPEN_TAG_WITH_ECHO
            || !is_semantic(&tree.tokens[i])
        {
            i += 1;
            continue;
        }
        return Some(i);
    }
    None
}

/// XHPAST node type name for the statement starting at `first`.
fn statement_node_type(tree: &Tree, first: usize) -> &'static str {
    let kind = tree.tokens[first].kind.as_str();
    if let Some(mapped) = keyword_statement_type(kind) {
        return mapped;
    }
    if kind == kinds::T_VARIABLE {
        return variable_statement_type(tree, first);
    }
    if kind == kinds::T_STRING {
        return string_statement_type(tree, first);
    }
    symbol_statement_type(kind)
}

fn symbol_statement_type(kind: &str) -> &'static str {
    const TABLE: &[(&str, &str)] = &[
        (";", "n_EMPTY"),
        ("{", "n_STATEMENT_LIST"),
        (kinds::T_LNUMBER, "n_NUMERIC_SCALAR"),
        (kinds::T_DNUMBER, "n_NUMERIC_SCALAR"),
        (kinds::T_CONSTANT_ENCAPSED_STRING, "n_STRING_SCALAR"),
        (kinds::T_INTERPOLATED_STRING, "n_CONCATENATION_LIST"),
        (kinds::T_HEREDOC, "n_HEREDOC"),
        (kinds::T_NOWDOC, "n_HEREDOC"),
        (kinds::T_BACKTICK, "n_BACKTICKS_EXPRESSION"),
        ("(", "n_PARENTHETICAL_EXPRESSION"),
        ("[", "n_ARRAY_LITERAL"),
        ("$", "n_VARIABLE_VARIABLE"),
        (kinds::T_INC, "n_UNARY_POSTFIX_EXPRESSION"),
        (kinds::T_DEC, "n_UNARY_POSTFIX_EXPRESSION"),
        ("+", "n_UNARY_PREFIX_EXPRESSION"),
        ("-", "n_UNARY_PREFIX_EXPRESSION"),
        ("!", "n_UNARY_PREFIX_EXPRESSION"),
        ("~", "n_UNARY_PREFIX_EXPRESSION"),
        ("@", "n_UNARY_PREFIX_EXPRESSION"),
        (kinds::T_CLASS_C, "n_MAGIC_SCALAR"),
        (kinds::T_FUNC_C, "n_MAGIC_SCALAR"),
        (kinds::T_METHOD_C, "n_MAGIC_SCALAR"),
        (kinds::T_FILE, "n_MAGIC_SCALAR"),
        (kinds::T_DIR, "n_MAGIC_SCALAR"),
        (kinds::T_LINE, "n_MAGIC_SCALAR"),
        (kinds::T_NS_C, "n_MAGIC_SCALAR"),
        (kinds::T_TRAIT_C, "n_MAGIC_SCALAR"),
        (kinds::T_INLINE_HTML, "n_INLINE_HTML"),
        (kinds::T_CLOSE_TAG, "n_EMPTY"),
    ];
    lookup_statement_type(TABLE, kind).unwrap_or("n_STATEMENT")
}

fn lookup_statement_type(
    table: &[(&'static str, &'static str)],
    kind: &str,
) -> Option<&'static str> {
    table
        .iter()
        .find(|(key, _)| *key == kind)
        .map(|(_, value)| *value)
}

fn keyword_statement_type(kind: &str) -> Option<&'static str> {
    const TABLE: &[(&str, &str)] = &[
        (kinds::T_ECHO, "n_ECHO_LIST"),
        (kinds::T_PRINT, "n_ECHO_LIST"),
        (kinds::T_INCLUDE, "n_INCLUDE_FILE"),
        (kinds::T_INCLUDE_ONCE, "n_INCLUDE_FILE"),
        (kinds::T_REQUIRE, "n_INCLUDE_FILE"),
        (kinds::T_REQUIRE_ONCE, "n_INCLUDE_FILE"),
        (kinds::T_IF, "n_IF"),
        (kinds::T_FUNCTION, "n_FUNCTION_DECLARATION"),
        (kinds::T_CLASS, "n_CLASS_DECLARATION"),
        (kinds::T_TRAIT, "n_CLASS_DECLARATION"),
        (kinds::T_INTERFACE, "n_INTERFACE_DECLARATION"),
        (kinds::T_RETURN, "n_RETURN"),
        (kinds::T_FOREACH, "n_FOREACH"),
        (kinds::T_FOR, "n_FOR"),
        (kinds::T_WHILE, "n_WHILE"),
        (kinds::T_DO, "n_DO_WHILE"),
        (kinds::T_SWITCH, "n_SWITCH"),
        (kinds::T_TRY, "n_TRY"),
        (kinds::T_THROW, "n_THROW"),
        (kinds::T_BREAK, "n_BREAK"),
        (kinds::T_CONTINUE, "n_CONTINUE"),
        (kinds::T_GLOBAL, "n_GLOBAL_DECLARATION_LIST"),
        (kinds::T_STATIC, "n_STATIC_DECLARATION_LIST"),
        (kinds::T_CONST, "n_CONSTANT_DECLARATION_LIST"),
        (kinds::T_USE, "n_USE_LIST"),
        (kinds::T_DECLARE, "n_DECLARE"),
        (kinds::T_GOTO, "n_GOTO"),
        (kinds::T_UNSET, "n_UNSET_LIST"),
        (kinds::T_YIELD, "n_YIELD"),
        (kinds::T_CLONE, "n_UNARY_PREFIX_EXPRESSION"),
        (kinds::T_EXIT, "n_UNARY_PREFIX_EXPRESSION"),
        (kinds::T_ISSET, "n_FUNCTION_CALL"),
        (kinds::T_EMPTY, "n_FUNCTION_CALL"),
        (kinds::T_EVAL, "n_FUNCTION_CALL"),
        (kinds::T_NAMESPACE, "n_NAMESPACE"),
        (kinds::T_LIST, "n_BINARY_EXPRESSION"),
        (kinds::T_NEW, "n_NEW"),
    ];
    lookup_statement_type(TABLE, kind)
}

fn variable_statement_type(tree: &Tree, first: usize) -> &'static str {
    let Some(next) = tree.next_semantic(first) else {
        return "n_VARIABLE";
    };
    match tree.tokens[next].kind.as_str() {
        kinds::T_OBJECT_OPERATOR | kinds::T_NULLSAFE_OBJECT_OPERATOR => "n_OBJECT_PROPERTY_ACCESS",
        kinds::T_INC | kinds::T_DEC => "n_UNARY_POSTFIX_EXPRESSION",
        "[" => "n_INDEX_ACCESS",
        "(" => "n_FUNCTION_CALL",
        _ => variable_expression_type(tree, first),
    }
}

fn variable_expression_type(tree: &Tree, first: usize) -> &'static str {
    if has_binary_operator(tree, first) {
        "n_BINARY_EXPRESSION"
    } else {
        "n_VARIABLE"
    }
}

fn string_statement_type(tree: &Tree, first: usize) -> &'static str {
    let Some(next) = tree.next_semantic(first) else {
        return "n_SYMBOL_NAME";
    };
    match tree.tokens[next].kind.as_str() {
        "(" => "n_FUNCTION_CALL",
        kinds::T_PAAMAYIM_NEKUDOTAYIM => "n_CLASS_STATIC_ACCESS",
        ":" => "n_LABEL",
        _ => string_expression_type(tree, first),
    }
}

fn string_expression_type(tree: &Tree, first: usize) -> &'static str {
    if has_binary_operator(tree, first) {
        "n_BINARY_EXPRESSION"
    } else {
        "n_SYMBOL_NAME"
    }
}

fn has_binary_operator(tree: &Tree, first: usize) -> bool {
    let mut depth = 0;
    let mut i = first;
    while i < tree.tokens.len() {
        match tree.tokens[i].kind.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            ";" if depth == 0 => return false,
            "=" | "+" | "-" | "*" | "/" | "%" | "." if depth == 0 => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// XHP99: `use X as X` aliases are unnecessary.
fn rule_symbol_alias(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let tree = ctx.tree;
    for stmt in import_uses(tree) {
        for symbol in import_symbols(tree, &stmt) {
            let Some(alias) = &symbol.alias else {
                continue;
            };
            let short = symbol.name.rsplit('\\').next().unwrap_or(&symbol.name);
            if short == alias {
                push(
                    out,
                    99,
                    tree.tokens[symbol.start].offset,
                    &format!(
                        "Importing `{}` with `as {}` is unnecessary because the aliased name is \
                         identical to the imported symbol name.",
                        symbol.name, alias
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "xhp_flow_tests.rs"]
mod xhp_flow_tests;
