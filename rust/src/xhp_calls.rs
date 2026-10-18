//! XHP call-site rules (T1128, divergence dv-c).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`, mirrored at
//! `%TEMP%\xhp_php\rules\` on this machine; read them with
//! `Get-Content` via PowerShell).
//!
//! Assigned rules (ID: PHP class):
//! - 4: ArcanistExtractUseXHPASTLinterRule (`extract()`)
//! - 12: ArcanistDynamicDefineXHPASTLinterRule (`define()` first arg)
//! - 21: ArcanistPlusOperatorOnStringsXHPASTLinterRule (`+` on strings)
//! - 36: ArcanistSlownessXHPASTLinterRule (slow string checks)
//! - 41: ArcanistDoubleQuoteXHPASTLinterRule (double quotes)
//! - 51: ArcanistBlacklistedFunctionXHPASTLinterRule (option map)
//! - 53: ArcanistCallTimePassByReferenceXHPASTLinterRule (`&` in calls)
//! - 54: ArcanistFormattedStringXHPASTLinterRule (printf format string)
//! - 61: ArcanistLowercaseFunctionsXHPASTLinterRule (builtin case)
//! - 62: ArcanistClassNameLiteralXHPASTLinterRule (`__CLASS__`)
//! - 65: ArcanistAliasFunctionXHPASTLinterRule (alias functions)
//! - 68: ArcanistLambdaFuncFunctionXHPASTLinterRule (`__lambda_func`)
//! - 80: ArcanistParseStrUseXHPASTLinterRule (`parse_str()`)
//! - 84: ArcanistArrayCombineXHPASTLinterRule (`array_combine()`)
//! - 85: ArcanistDeprecationXHPASTLinterRule (option map)
//! - 86: ArcanistUnsafeDynamicStringXHPASTLinterRule (option maps)
//! - 105: ArcanistFunctionCallShouldBeTypeCastXHPASTLinterRule
//! - 111: ArcanistIsAShouldBeInstanceOfXHPASTLinterRule (`is_a()`)
//! - 125: ArcanistInvalidOctalNumericScalarXHPASTLinterRule (`08`)
//! - 127: ArcanistHexadecimalNumericScalarCasingXHPASTLinterRule
//! - 129: ArcanistImplodeArgumentOrderXHPASTLinterRule (`implode()`)
//! - 131: ArcanistBinaryNumericScalarCasingXHPASTLinterRule (`0B`)
//! - 133: ArcanistEachUseXHPASTLinterRule (`each()`)
//! - 134: ArcanistProductNameLiteralXHPASTLinterRule (`pht()` names)
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (`Tree::calls`,
//! token scans). Emit [`super::xhp_ast::RawFinding`] with the PHP
//! rule ID and the EXACT PHP message text (`pht()` templates with
//! `%s`/`%d` substituted exactly as the PHP does, including quirks).
//! Do not invent messages. If a (sub-)check cannot be ported
//! meaningfully (e.g. it needs libphutil runtime behavior), leave it
//! out and document the gap in the module docs + final report; never
//! silently diverge.
//!
//! Constraints: no new dependencies; every function cy<=15 / co<=20
//! (named private fns, same file; closures count into parents);
//! `cargo clippy --offline --all-targets -- -D warnings` clean;
//! `cargo fmt --check` clean. Touch ONLY this file and
//! `xhp_calls_tests.rs`.
//!
//! Port notes and known gaps:
//! - XHP134 is omitted: product names come from libphutil runtime
//!   (`PlatformSymbols::getProductNames()`), which cannot run here.
//! - XHP54 ports only the missing-format-string check; the
//!   `xsprintf()` conversion validation needs libphutil and is not
//!   ported.
//! - XHP61 compares against a curated static list of common PHP
//!   internal functions ([`BUILTIN_FUNCTIONS`]); the PHP rule uses
//!   the runtime `get_defined_functions()` list, which varies by
//!   version and loaded extensions. Unlisted builtins are missed
//!   (false negatives only).
//! - The `isset` / `empty` / `eval` / `exit` constructs (XHPAST
//!   dedicated node types, not `n_FUNCTION_CALL`) are excluded from
//!   every name-based rule, matching the PHP `n_FUNCTION_CALL`
//!   selections. In particular XHP65 never fires on `die`, which the
//!   PHP alias table lists but the PHP AST presumably types as a
//!   dedicated exit node.
//! - XHP21 reports at the `+` operator; XHP86 `new` reports at the
//!   `new` keyword. PHP reports both at the enclosing node start,
//!   which the token model cannot recover exactly.
//! - XHP21 / XHP36 do not look through parentheses: PHP sees the AST
//!   child type, so `('a') + $b` or `(false) === strpos(...)` may
//!   behave differently there.
//! - XHP41 groups only strings joined by `.` at bracket depth zero;
//!   mixed-precedence siblings (e.g. `"a" + $n . "$y"`) group by AST
//!   parenting in PHP and may differ here.
//! - XHP105 reproduces the PHP `break` quirk: `intval($x, $nonnum)`
//!   stops the whole rule scan for the file.
//! - XHP111 with fewer than two arguments is skipped (the PHP rule
//!   would read a missing parameter node).

use super::xhp_ast::{
    BlockKind, CallSite, RawFinding, RuleCtx, Tree, XhpOptions, is_semantic, kinds,
};

/// Run all call-site rules, pushing findings into `out`.
pub fn apply(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    let calls = ctx.tree.calls();
    rule_extract(ctx.tree, &calls, out);
    rule_dynamic_define(ctx.tree, &calls, out);
    rule_plus_on_strings(ctx.tree, out);
    rule_slowness(ctx.tree, &calls, out);
    rule_double_quote(ctx.tree, out);
    rule_blacklisted(ctx, &calls, out);
    rule_call_time_ref(ctx.tree, out);
    rule_formatted_string(ctx, &calls, out);
    rule_lowercase_functions(ctx.tree, &calls, out);
    rule_class_name_literal(ctx.tree, out);
    rule_alias_functions(ctx.tree, &calls, out);
    rule_lambda_func(ctx.tree, out);
    rule_parse_str(ctx.tree, &calls, out);
    rule_array_combine(ctx.tree, &calls, out);
    rule_deprecation(ctx, &calls, out);
    rule_unsafe_dynamic(ctx, &calls, out);
    rule_cast_calls(ctx.tree, &calls, out);
    rule_is_a(ctx.tree, &calls, out);
    rule_invalid_octal(ctx.tree, out);
    rule_hex_casing(ctx.tree, out);
    rule_implode_order(ctx.tree, &calls, out);
    rule_binary_casing(ctx.tree, out);
    rule_each_use(ctx.tree, &calls, out);
}

/// Byte offset of a call's name token (the call node start).
fn name_offset(tree: &Tree, call: &CallSite) -> usize {
    tree.tokens[call.name_idx].offset
}

/// Push a call-level finding at the call's name token.
fn push_call(rule: u32, tree: &Tree, call: &CallSite, message: String, out: &mut Vec<RawFinding>) {
    out.push(RawFinding::at(rule, name_offset(tree, call), message));
}

/// Mirror of PHP `getFunctionCalls()`: case-insensitive plain call.
fn plain_call_named(call: &CallSite, name: &str) -> bool {
    !call.is_construct && call.name.eq_ignore_ascii_case(name)
}

fn is_open_kind(kind: &str) -> bool {
    matches!(kind, "(" | "{" | "[")
}

fn is_close_kind(kind: &str) -> bool {
    matches!(kind, ")" | "}" | "]")
}

/// Trim edge trivia from an inclusive token range.
fn trim_range(tree: &Tree, first: usize, last: usize) -> Option<(usize, usize)> {
    let mut start = first;
    let mut end = last;
    while start <= end && !is_semantic(&tree.tokens[start]) {
        start += 1;
    }
    while end > start && !is_semantic(&tree.tokens[end]) {
        end -= 1;
    }
    if start <= end && is_semantic(&tree.tokens[start]) {
        Some((start, end))
    } else {
        None
    }
}

/// Scan state for [`split_call_args`]: bracket depth and the current
/// argument's first token index.
struct ArgSplit {
    depth: u32,
    start: usize,
}

/// Top-level comma split of `(lparen, end)`, mirroring the framework
/// splitter for paren groups `Tree::calls` does not cover.
fn split_call_args(tree: &Tree, lparen: usize, end: usize) -> Vec<(usize, usize)> {
    let mut args = Vec::new();
    let mut state = ArgSplit {
        depth: 0,
        start: lparen + 1,
    };
    let mut i = state.start;
    while i <= end && i < tree.tokens.len() {
        let kind = tree.tokens[i].kind.as_str();
        if split_step(tree, kind, &mut state, &mut args, i) {
            break;
        }
        i += 1;
    }
    if i > state.start && state.start <= end {
        push_arg(tree, &mut args, state.start, end.min(i.saturating_sub(1)));
    }
    args
}

/// Handle one token of an argument scan; true means stop.
fn split_step(
    tree: &Tree,
    kind: &str,
    state: &mut ArgSplit,
    args: &mut Vec<(usize, usize)>,
    i: usize,
) -> bool {
    if is_open_kind(kind) {
        state.depth += 1;
    } else if is_close_kind(kind) {
        return close_arg(state);
    } else if kind == "," && state.depth == 0 {
        if i > state.start {
            push_arg(tree, args, state.start, i - 1);
        }
        state.start = i + 1;
    }
    false
}

/// Handle a closer during an argument scan; true means stop.
fn close_arg(state: &mut ArgSplit) -> bool {
    if state.depth == 0 {
        return true;
    }
    state.depth -= 1;
    false
}

fn push_arg(tree: &Tree, args: &mut Vec<(usize, usize)>, start: usize, stop: usize) {
    if let Some(range) = trim_range(tree, start, stop) {
        args.push(range);
    }
}

/// XHP4: any call to `extract()` (case-insensitive).
fn rule_extract(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if plain_call_named(call, "extract") {
            push_call(
                4,
                tree,
                call,
                "Avoid `extract`. It is confusing and hinders static analysis.".to_string(),
                out,
            );
        }
    }
}

/// XHP12: first `define()` argument must be a static scalar.
fn rule_dynamic_define(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if !plain_call_named(call, "define") {
            continue;
        }
        let Some(&(first, last)) = call.args.first() else {
            continue;
        };
        if tree.static_scalar_text(first, last).is_none() {
            out.push(RawFinding::at(
                12,
                tree.tokens[first].offset,
                "First argument to `define` must be a string literal.".to_string(),
            ));
        }
    }
}

/// Token kinds that can end a value, i.e. follow a binary operator's
/// left operand (used to tell binary `+` from unary `+`).
const OPERAND_END: &[&str] = &[
    kinds::T_VARIABLE,
    kinds::T_STRING,
    kinds::T_STATIC,
    kinds::T_LNUMBER,
    kinds::T_DNUMBER,
    kinds::T_CONSTANT_ENCAPSED_STRING,
    kinds::T_INTERPOLATED_STRING,
    kinds::T_HEREDOC,
    kinds::T_NOWDOC,
    kinds::T_BACKTICK,
    ")",
    "]",
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

fn is_string_token(kind: &str) -> bool {
    kind == kinds::T_CONSTANT_ENCAPSED_STRING || kind == kinds::T_INTERPOLATED_STRING
}

/// XHP21: `+` with a string literal operand (constant or interpolated,
/// both of which are `n_STRING_SCALAR` in XHPAST).
fn rule_plus_on_strings(tree: &Tree, out: &mut Vec<RawFinding>) {
    for (idx, token) in tree.tokens.iter().enumerate() {
        if token.kind != "+" {
            continue;
        }
        if !prev_is_operand(tree, idx) {
            continue;
        }
        let left_string = tree
            .prev_semantic(idx)
            .is_some_and(|p| is_string_token(&tree.tokens[p].kind));
        let right_string = tree
            .next_semantic(idx)
            .is_some_and(|n| is_string_token(&tree.tokens[n].kind));
        if left_string || right_string {
            out.push(RawFinding::at(
                21,
                token.offset,
                "In PHP, `.` is the string concatenation operator, not `+`. \
                 This expression uses `+` with a string literal as an operand."
                    .to_string(),
            ));
        }
    }
}

fn prev_is_operand(tree: &Tree, idx: usize) -> bool {
    tree.prev_semantic(idx)
        .is_some_and(|p| OPERAND_END.contains(&tree.tokens[p].kind.as_str()))
}

/// XHP36: `strstr`/`strchr`/`stristr` compared strictly against
/// `false`, or `strpos`/`stripos` compared strictly against `0`.
fn rule_slowness(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if let Some(message) = slowness_message(tree, call) {
            push_call(36, tree, call, message.to_string(), out);
        }
    }
}

fn slowness_message(tree: &Tree, call: &CallSite) -> Option<&'static str> {
    if call.is_construct {
        return None;
    }
    let lower = call.name.to_ascii_lowercase();
    let (want_false, message) = match lower.as_str() {
        "strstr" | "strchr" => (
            true,
            "Use `strpos` for checking if the string contains something.",
        ),
        "stristr" => (
            true,
            "Use `stripos` for checking if the string contains something.",
        ),
        "strpos" => (
            false,
            "Use `strncmp` for checking if the string starts with something.",
        ),
        "stripos" => (
            false,
            "Use `strncasecmp` for checking if the string starts with something.",
        ),
        _ => return None,
    };
    if slowness_hit(tree, call, want_false) {
        Some(message)
    } else {
        None
    }
}

/// True when the call is a direct operand of `===` / `!==` with the
/// expected `false` / `0` counterpart on the other side.
fn slowness_hit(tree: &Tree, call: &CallSite, want_false: bool) -> bool {
    let Some(rparen) = call.rparen else {
        return false;
    };
    if let Some(op) = tree.prev_semantic(call.name_idx)
        && is_strict_eq(&tree.tokens[op].kind)
        && tree
            .prev_semantic(op)
            .is_some_and(|c| is_counterpart(tree, c, want_false))
    {
        return true;
    }
    if let Some(op) = tree.next_semantic(rparen)
        && is_strict_eq(&tree.tokens[op].kind)
        && tree
            .next_semantic(op)
            .is_some_and(|c| is_counterpart(tree, c, want_false))
    {
        return true;
    }
    false
}

fn is_strict_eq(kind: &str) -> bool {
    kind == kinds::T_IS_IDENTICAL || kind == kinds::T_IS_NOT_IDENTICAL
}

/// PHP compares `false` case-sensitively and `0` as exact text.
fn is_counterpart(tree: &Tree, idx: usize, want_false: bool) -> bool {
    let token = &tree.tokens[idx];
    if want_false {
        token.kind == kinds::T_STRING && token.value == "false"
    } else {
        token.kind == kinds::T_LNUMBER && token.value == "0"
    }
}

/// Tokens that may appear at depth zero inside a `.` chain: operands,
/// the `.` joiner itself, and trivia.
const CHAIN_TOKENS: &[&str] = &[
    ".",
    kinds::T_WHITESPACE,
    kinds::T_COMMENT,
    kinds::T_DOC_COMMENT,
    kinds::T_VARIABLE,
    kinds::T_STRING,
    kinds::T_STATIC,
    kinds::T_LNUMBER,
    kinds::T_DNUMBER,
    kinds::T_CONSTANT_ENCAPSED_STRING,
    kinds::T_INTERPOLATED_STRING,
    kinds::T_OBJECT_OPERATOR,
    kinds::T_NULLSAFE_OBJECT_OPERATOR,
    kinds::T_PAAMAYIM_NEKUDOTAYIM,
    "\\",
    "$",
    "@",
    "!",
    "~",
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
    kinds::T_ISSET,
    kinds::T_EMPTY,
    kinds::T_EVAL,
    kinds::T_EXIT,
    kinds::T_PRINT,
    kinds::T_LIST,
    kinds::T_ARRAY,
    kinds::T_CLONE,
];

/// Escape sequences (raw two-character text) that justify double
/// quotes, mirroring the PHP `$allowed_chars` list.
const DOUBLE_ALLOWED: &[&str] = &[
    "\\n", "\\r", "\\t", "\\v", "\\e", "\\f", "\\'", "\\0", "\\1", "\\2", "\\3", "\\4", "\\5",
    "\\6", "\\7", "\\x",
];

/// XHP41: double-quoted constant strings without escapes, grouped by
/// `.` concatenation: one interpolated or escape-bearing member
/// validates the whole group.
fn rule_double_quote(tree: &Tree, out: &mut Vec<RawFinding>) {
    let mut visited = vec![false; tree.tokens.len()];
    for (i, token) in tree.tokens.iter().enumerate() {
        if !is_string_token(&token.kind) || visited[i] {
            continue;
        }
        let group = concat_group(tree, i);
        for &member in &group {
            visited[member] = true;
        }
        let (valid, invalid) = double_quote_group_status(tree, &group);
        if !valid {
            for member in invalid {
                out.push(RawFinding::at(
                    41,
                    tree.tokens[member].offset,
                    "String does not require double quotes. For consistency, \
                     prefer single quotes."
                        .to_string(),
                ));
            }
        }
    }
}

/// All string tokens in the `.` chain around `start`.
fn concat_group(tree: &Tree, start: usize) -> Vec<usize> {
    let mut group = vec![start];
    chain_extend(tree, start, true, &mut group);
    chain_extend(tree, start, false, &mut group);
    group.sort_unstable();
    group.dedup();
    group
}

fn chain_extend(tree: &Tree, start: usize, right: bool, group: &mut Vec<usize>) {
    let mut depth: i32 = 0;
    let step: isize = if right { 1 } else { -1 };
    let mut idx = start as isize + step;
    while idx >= 0 && (idx as usize) < tree.tokens.len() {
        if chain_visit(tree, idx as usize, right, &mut depth, group) {
            break;
        }
        idx += step;
    }
}

/// Visit one token while scanning a `.` chain; true means stop.
fn chain_visit(
    tree: &Tree,
    idx: usize,
    right: bool,
    depth: &mut i32,
    group: &mut Vec<usize>,
) -> bool {
    let kind = tree.tokens[idx].kind.as_str();
    if is_open_kind(kind) {
        return chain_open(right, depth);
    }
    if is_close_kind(kind) {
        return chain_close(right, depth);
    }
    if *depth != 0 {
        return false;
    }
    if !CHAIN_TOKENS.contains(&kind) {
        return true;
    }
    if is_string_token(kind) {
        group.push(idx);
    }
    false
}

/// Depth update for an opener while scanning; true means stop.
fn chain_open(right: bool, depth: &mut i32) -> bool {
    if right {
        *depth += 1;
        false
    } else if *depth == 0 {
        true
    } else {
        *depth -= 1;
        false
    }
}

/// Depth update for a closer while scanning; true means stop.
fn chain_close(right: bool, depth: &mut i32) -> bool {
    if right {
        if *depth == 0 {
            true
        } else {
            *depth -= 1;
            false
        }
    } else {
        *depth += 1;
        false
    }
}

/// `(valid, invalid)` for one concatenation group: interpolated or
/// escape-bearing double-quoted members validate the group; the rest
/// of the plain double-quoted members are reported.
fn double_quote_group_status(tree: &Tree, group: &[usize]) -> (bool, Vec<usize>) {
    let mut valid = false;
    let mut invalid = Vec::new();
    for &member in group {
        let token = &tree.tokens[member];
        if token.kind == kinds::T_INTERPOLATED_STRING {
            valid = true;
        } else if token.value.starts_with('"') {
            if has_allowed_escape(&token.value) {
                valid = true;
            } else {
                invalid.push(member);
            }
        }
    }
    (valid, invalid)
}

fn has_allowed_escape(literal: &str) -> bool {
    let Some(contents) = literal.get(1..literal.len().saturating_sub(1)) else {
        return false;
    };
    DOUBLE_ALLOWED.iter().any(|seq| contents.contains(seq))
}

/// XHP51: calls whose exact-case name is a key of the
/// `xhpast.blacklisted.function` map; the message is the mapped value.
fn rule_blacklisted(ctx: &RuleCtx, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        if let Some(reason) = blacklist_reason(ctx.options, &call.name) {
            push_call(51, ctx.tree, call, reason, out);
        }
    }
}

fn blacklist_reason(options: &XhpOptions, name: &str) -> Option<String> {
    for (key, reason) in &options.blacklisted {
        if *key == *name && !reason.is_empty() {
            return Some(reason.clone());
        }
    }
    None
}

/// XHP53: `&$var` arguments (call-time pass-by-reference) in any
/// plain, method, static, or `new` call; declarations are excluded.
fn rule_call_time_ref(tree: &Tree, out: &mut Vec<RawFinding>) {
    for p in 0..tree.tokens.len() {
        if tree.tokens[p].kind != "(" {
            continue;
        }
        let Some(prev) = tree.prev_semantic(p) else {
            continue;
        };
        if !is_call_paren(tree, prev) {
            continue;
        }
        let end = tree
            .match_close(p)
            .unwrap_or_else(|| tree.tokens.len().saturating_sub(1));
        for (first, _) in split_call_args(tree, p, end) {
            if tree.tokens[first].kind == "&" && ref_target_follows(tree, first) {
                out.push(RawFinding::at(
                    53,
                    tree.tokens[first].offset,
                    "Call-time pass-by-reference calls are prohibited.".to_string(),
                ));
            }
        }
    }
}

fn is_call_paren(tree: &Tree, prev: usize) -> bool {
    match tree.tokens[prev].kind.as_str() {
        kinds::T_VARIABLE => true,
        kinds::T_ISSET | kinds::T_EMPTY | kinds::T_EVAL | kinds::T_EXIT => true,
        ")" | "]" => true,
        kinds::T_STRING => !is_declaration_name(tree, prev),
        _ => false,
    }
}

/// True for `foo` in `function foo(` / `function &foo(`.
fn is_declaration_name(tree: &Tree, prev: usize) -> bool {
    let Some(before) = tree.prev_semantic(prev) else {
        return false;
    };
    if tree.tokens[before].kind == kinds::T_FUNCTION {
        return true;
    }
    if tree.tokens[before].kind == "&" {
        return tree
            .prev_semantic(before)
            .is_some_and(|q| tree.tokens[q].kind == kinds::T_FUNCTION);
    }
    false
}

fn ref_target_follows(tree: &Tree, amp: usize) -> bool {
    tree.next_semantic(amp)
        .is_some_and(|n| tree.tokens[n].kind == kinds::T_VARIABLE || tree.tokens[n].kind == "$")
}

/// Built-in `printf`-style functions and their format-string argument
/// index, mirroring the PHP `$functions` table.
const PRINTF_FUNCS: &[(&str, i64)] = &[
    ("fprintf", 1),
    ("printf", 0),
    ("sprintf", 0),
    ("vfprintf", 1),
    ("csprintf", 0),
    ("execx", 0),
    ("exec_manual", 0),
    ("hgsprintf", 0),
    ("hsprintf", 0),
    ("jsprintf", 0),
    ("pht", 0),
    ("phutil_passthru", 0),
    ("qsprintf", 1),
    ("queryfx", 1),
    ("queryfx_all", 1),
    ("queryfx_one", 1),
    ("vcsprintf", 0),
    ("vqsprintf", 1),
];

/// XHP54: `printf`-style calls missing the format-string argument.
/// The `xsprintf()` conversion validation needs libphutil and is not
/// ported.
fn rule_formatted_string(ctx: &RuleCtx, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        let lower = call.name.to_ascii_lowercase();
        let Some(start) = printf_start(ctx.options, &lower) else {
            continue;
        };
        if call.args.len() as i64 - start < 1 {
            push_call(
                54,
                ctx.tree,
                call,
                "This function is expected to have a format string.".to_string(),
                out,
            );
        }
    }
}

/// Format-string index for a lowercased name. PHP unions
/// `$functions + $this->printfFunctions`, so built-ins win over
/// configuration on duplicate keys, and configuration keys match
/// only in exact (lowercase) form.
fn printf_start(options: &XhpOptions, lower: &str) -> Option<i64> {
    for entry in PRINTF_FUNCS {
        if entry.0 == lower {
            return Some(entry.1);
        }
    }
    for entry in &options.printf {
        if entry.0 == *lower {
            return Some(entry.1);
        }
    }
    None
}

/// Curated lowercase list of common PHP internal functions. The PHP
/// rule uses the runtime `get_defined_functions()` list; names missing
/// here are missed (false negatives only, never false positives).
const BUILTIN_FUNCTIONS: &[&str] = &[
    "abs",
    "acos",
    "addcslashes",
    "addslashes",
    "array_change_key_case",
    "array_chunk",
    "array_column",
    "array_combine",
    "array_count_values",
    "array_diff",
    "array_diff_assoc",
    "array_diff_key",
    "array_fill",
    "array_fill_keys",
    "array_filter",
    "array_flip",
    "array_intersect",
    "array_key_exists",
    "array_keys",
    "array_map",
    "array_merge",
    "array_merge_recursive",
    "array_multisort",
    "array_pad",
    "array_pop",
    "array_product",
    "array_push",
    "array_rand",
    "array_reduce",
    "array_replace",
    "array_reverse",
    "array_search",
    "array_shift",
    "array_slice",
    "array_splice",
    "array_sum",
    "array_unique",
    "array_unshift",
    "array_values",
    "array_walk",
    "arsort",
    "asin",
    "asort",
    "assert",
    "atan",
    "atan2",
    "base64_decode",
    "base64_encode",
    "basename",
    "bcadd",
    "bccomp",
    "bcdiv",
    "bcmod",
    "bcmul",
    "bcpow",
    "bcscale",
    "bcsqrt",
    "bcsub",
    "bin2hex",
    "bindec",
    "boolval",
    "call_user_func",
    "call_user_func_array",
    "ceil",
    "checkdate",
    "chop",
    "chr",
    "chunk_split",
    "class_exists",
    "class_implements",
    "class_parents",
    "closedir",
    "compact",
    "constant",
    "copy",
    "cos",
    "cosh",
    "count",
    "crc32",
    "crypt",
    "current",
    "date",
    "date_create",
    "debug_backtrace",
    "decbin",
    "dechex",
    "decoct",
    "define",
    "defined",
    "deg2rad",
    "dirname",
    "disk_free_space",
    "disk_total_space",
    "diskfreespace",
    "doubleval",
    "each",
    "end",
    "error_log",
    "error_reporting",
    "escapeshellarg",
    "escapeshellcmd",
    "exec",
    "exp",
    "explode",
    "expm1",
    "extract",
    "fclose",
    "feof",
    "fflush",
    "fgetc",
    "fgetcsv",
    "fgets",
    "file",
    "file_exists",
    "file_get_contents",
    "file_put_contents",
    "fileatime",
    "filectime",
    "filegroup",
    "fileinode",
    "filemtime",
    "fileowner",
    "fileperms",
    "filesize",
    "filetype",
    "floatval",
    "flock",
    "floor",
    "flush",
    "fmod",
    "fopen",
    "forward_static_call",
    "fpassthru",
    "fprintf",
    "fputcsv",
    "fputs",
    "fread",
    "fscanf",
    "fseek",
    "fsockopen",
    "fstat",
    "ftell",
    "ftruncate",
    "func_get_arg",
    "func_get_args",
    "func_num_args",
    "function_exists",
    "fwrite",
    "get_called_class",
    "get_class",
    "get_class_methods",
    "get_class_vars",
    "get_current_user",
    "get_declared_classes",
    "get_defined_constants",
    "get_defined_functions",
    "get_defined_vars",
    "get_extension_funcs",
    "get_headers",
    "get_html_translation_table",
    "get_include_path",
    "get_included_files",
    "get_loaded_extensions",
    "get_meta_tags",
    "get_object_vars",
    "get_parent_class",
    "get_resource_type",
    "getcwd",
    "getenv",
    "getimagesize",
    "getlastmod",
    "getmxrr",
    "getmygid",
    "getmyinode",
    "getmypid",
    "getmyuid",
    "getopt",
    "getprotobyname",
    "getprotobynumber",
    "getrandmax",
    "getservbyname",
    "getservbyport",
    "gettext",
    "glob",
    "gmdate",
    "gmmktime",
    "gzclose",
    "gzcompress",
    "gzdecode",
    "gzdeflate",
    "gzencode",
    "gzfile",
    "gzgetc",
    "gzgets",
    "gzinflate",
    "gzopen",
    "gzpassthru",
    "gzread",
    "gzseek",
    "gztell",
    "gzuncompress",
    "gzwrite",
    "hash",
    "hash_algos",
    "hash_equals",
    "hash_file",
    "hash_hmac",
    "header",
    "header_remove",
    "headers_list",
    "headers_sent",
    "hex2bin",
    "hexdec",
    "highlight_file",
    "highlight_string",
    "html_entity_decode",
    "htmlentities",
    "htmlspecialchars",
    "htmlspecialchars_decode",
    "http_build_query",
    "hypot",
    "idate",
    "ignore_user_abort",
    "implode",
    "in_array",
    "ini_alter",
    "ini_get",
    "ini_restore",
    "ini_set",
    "interface_exists",
    "intval",
    "ip2long",
    "is_a",
    "is_array",
    "is_bool",
    "is_callable",
    "is_dir",
    "is_double",
    "is_executable",
    "is_file",
    "is_finite",
    "is_float",
    "is_infinite",
    "is_int",
    "is_integer",
    "is_link",
    "is_long",
    "is_nan",
    "is_null",
    "is_numeric",
    "is_object",
    "is_readable",
    "is_real",
    "is_resource",
    "is_scalar",
    "is_string",
    "is_subclass_of",
    "is_uploaded_file",
    "is_writable",
    "is_writeable",
    "join",
    "json_decode",
    "json_encode",
    "json_last_error",
    "json_last_error_msg",
    "key",
    "key_exists",
    "krsort",
    "ksort",
    "lcfirst",
    "lcg_value",
    "lchgrp",
    "lchown",
    "levenshtein",
    "link",
    "linkinfo",
    "localeconv",
    "localtime",
    "log",
    "log10",
    "log1p",
    "long2ip",
    "lstat",
    "ltrim",
    "mail",
    "max",
    "mb_convert_encoding",
    "mb_detect_encoding",
    "mb_internal_encoding",
    "mb_strlen",
    "mb_strpos",
    "mb_strtolower",
    "mb_strtoupper",
    "mb_substr",
    "md5",
    "md5_file",
    "memory_get_peak_usage",
    "memory_get_usage",
    "metaphone",
    "method_exists",
    "microtime",
    "min",
    "mkdir",
    "mktime",
    "move_uploaded_file",
    "mt_getrandmax",
    "mt_rand",
    "mt_srand",
    "natcasesort",
    "natsort",
    "next",
    "nl2br",
    "number_format",
    "ob_clean",
    "ob_end_clean",
    "ob_end_flush",
    "ob_flush",
    "ob_get_clean",
    "ob_get_contents",
    "ob_get_flush",
    "ob_get_length",
    "ob_get_level",
    "ob_implicit_flush",
    "ob_start",
    "octdec",
    "opendir",
    "openlog",
    "ord",
    "output_add_rewrite_var",
    "output_reset_rewrite_vars",
    "pack",
    "parse_ini_file",
    "parse_str",
    "parse_url",
    "passthru",
    "pathinfo",
    "pclose",
    "pfsockopen",
    "php_ini_loaded_file",
    "php_sapi_name",
    "php_uname",
    "phpinfo",
    "phpversion",
    "pi",
    "popen",
    "pos",
    "pow",
    "preg_filter",
    "preg_grep",
    "preg_last_error",
    "preg_match",
    "preg_match_all",
    "preg_quote",
    "preg_replace",
    "preg_replace_callback",
    "preg_split",
    "prev",
    "printf",
    "print_r",
    "property_exists",
    "putenv",
    "quoted_printable_decode",
    "quoted_printable_encode",
    "quotemeta",
    "rad2deg",
    "rand",
    "range",
    "rawurldecode",
    "rawurlencode",
    "readdir",
    "readfile",
    "readlink",
    "realpath",
    "register_shutdown_function",
    "rename",
    "reset",
    "restore_error_handler",
    "restore_exception_handler",
    "rewind",
    "rewinddir",
    "rmdir",
    "round",
    "rsort",
    "rtrim",
    "scandir",
    "serialize",
    "session_id",
    "session_start",
    "set_error_handler",
    "set_exception_handler",
    "set_file_buffer",
    "set_include_path",
    "set_time_limit",
    "setcookie",
    "setlocale",
    "settype",
    "sha1",
    "sha1_file",
    "shell_exec",
    "similar_text",
    "sin",
    "sinh",
    "sizeof",
    "sleep",
    "sort",
    "soundex",
    "sprintf",
    "sqrt",
    "srand",
    "sscanf",
    "stat",
    "str_getcsv",
    "str_ireplace",
    "str_pad",
    "str_repeat",
    "str_replace",
    "str_rot13",
    "str_shuffle",
    "str_split",
    "str_word_count",
    "strcasecmp",
    "strchr",
    "strcmp",
    "strcoll",
    "strcspn",
    "strip_tags",
    "stripcslashes",
    "stripos",
    "stripslashes",
    "stristr",
    "strlen",
    "strnatcasecmp",
    "strnatcmp",
    "strncasecmp",
    "strncmp",
    "strpbrk",
    "strpos",
    "strptime",
    "strrchr",
    "strrev",
    "strripos",
    "strrpos",
    "strspn",
    "strstr",
    "strtok",
    "strtolower",
    "strtotime",
    "strtoupper",
    "strtr",
    "strval",
    "substr",
    "substr_compare",
    "substr_count",
    "substr_replace",
    "symlink",
    "syslog",
    "system",
    "tan",
    "tempnam",
    "time",
    "tmpfile",
    "token_get_all",
    "token_name",
    "touch",
    "trait_exists",
    "trim",
    "uasort",
    "ucfirst",
    "ucwords",
    "uksort",
    "umask",
    "uniqid",
    "unlink",
    "unpack",
    "unserialize",
    "urldecode",
    "urlencode",
    "usleep",
    "usort",
    "utf8_decode",
    "utf8_encode",
    "var_dump",
    "var_export",
    "version_compare",
    "vfprintf",
    "virtual",
    "vprintf",
    "vsprintf",
    "wordwrap",
];

/// XHP61: calls to built-in PHP functions must be lowercase.
fn rule_lowercase_functions(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        let lower = call.name.to_ascii_lowercase();
        if call.name != lower && BUILTIN_FUNCTIONS.contains(&lower.as_str()) {
            push_call(
                61,
                tree,
                call,
                "Calls to built-in PHP functions should be lowercase.".to_string(),
                out,
            );
        }
    }
}

/// Raw inner text of a quoted literal (quotes stripped, escapes kept),
/// mirroring the PHP `substr($semantic, 1, -1)` comparison.
fn raw_string_inner(literal: &str) -> Option<&str> {
    literal.get(1..literal.len().saturating_sub(1))
}

/// XHP62: string literals inside a named class body whose entire
/// contents equal the class name.
fn rule_class_name_literal(tree: &Tree, out: &mut Vec<RawFinding>) {
    let blocks = tree.blocks();
    for block in &blocks {
        if block.kind != BlockKind::Class {
            continue;
        }
        let (Some(name), Some(name_idx)) = (block.name.as_ref(), block.name_idx) else {
            continue;
        };
        let Some(end) = tree.body_last(block) else {
            continue;
        };
        for (i, token) in tree.tokens.iter().enumerate() {
            if i <= name_idx || i > end {
                continue;
            }
            if !is_string_token(&token.kind) {
                continue;
            }
            if raw_string_inner(&token.value).is_some_and(|inner| inner == name) {
                out.push(RawFinding::at(
                    62,
                    token.offset,
                    "Prefer \"__CLASS__\" over hard-coded class names.".to_string(),
                ));
            }
        }
    }
}

/// Lowercase alias-function names, mirroring the PHP alias table keys.
const ALIAS_FUNCTIONS: &[&str] = &[
    "_",
    "chop",
    "close",
    "com_get",
    "com_propset",
    "com_set",
    "die",
    "diskfreespace",
    "doubleval",
    "drawarc",
    "drawcircle",
    "drawcubic",
    "drawcubicto",
    "drawcurve",
    "drawcurveto",
    "drawglyph",
    "drawline",
    "drawlineto",
    "fbsql",
    "fputs",
    "gzputs",
    "i18n_convert",
    "i18n_discover_encoding",
    "i18n_http_input",
    "i18n_http_output",
    "i18n_internal_encoding",
    "i18n_ja_jp_hantozen",
    "i18n_mime_header_decode",
    "i18n_mime_header_encode",
    "imap_create",
    "imap_fetchtext",
    "imap_getmailboxes",
    "imap_getsubscribed",
    "imap_header",
    "imap_listmailbox",
    "imap_listsubscribed",
    "imap_rename",
    "imap_scan",
    "imap_scanmailbox",
    "ini_alter",
    "is_double",
    "is_integer",
    "is_long",
    "is_real",
    "is_writeable",
    "join",
    "key_exists",
    "ldap_close",
    "magic_quotes_runtime",
    "mbstrcut",
    "mbstrlen",
    "mbstrpos",
    "mbstrrpos",
    "mbsubstr",
    "ming_setcubicthreshold",
    "ming_setscale",
    "msql",
    "msql_createdb",
    "msql_dbname",
    "msql_dropdb",
    "msql_fieldflags",
    "msql_fieldlen",
    "msql_fieldname",
    "msql_fieldtable",
    "msql_fieldtype",
    "msql_freeresult",
    "msql_listdbs",
    "msql_listfields",
    "msql_listtables",
    "msql_numfields",
    "msql_numrows",
    "msql_regcase",
    "msql_selectdb",
    "msql_tablename",
    "mssql_affected_rows",
    "mssql_close",
    "mssql_connect",
    "mssql_data_seek",
    "mssql_fetch_array",
    "mssql_fetch_field",
    "mssql_fetch_object",
    "mssql_fetch_row",
    "mssql_field_seek",
    "mssql_free_result",
    "mssql_get_last_message",
    "mssql_min_client_severity",
    "mssql_min_error_severity",
    "mssql_min_message_severity",
    "mssql_min_server_severity",
    "mssql_num_fields",
    "mssql_num_rows",
    "mssql_pconnect",
    "mssql_query",
    "mssql_result",
    "mssql_select_db",
    "multcolor",
    "mysql",
    "mysql_createdb",
    "mysql_db_name",
    "mysql_dbname",
    "mysql_dropdb",
    "mysql_fieldflags",
    "mysql_fieldlen",
    "mysql_fieldname",
    "mysql_fieldtable",
    "mysql_fieldtype",
    "mysql_freeresult",
    "mysql_listdbs",
    "mysql_listfields",
    "mysql_listtables",
    "mysql_numfields",
    "mysql_numrows",
    "mysql_selectdb",
    "mysql_tablename",
    "ociassignelem",
    "ocibindbyname",
    "ocicancel",
    "ocicloselob",
    "ocicollappend",
    "ocicollassign",
    "ocicollmax",
    "ocicollsize",
    "ocicolltrim",
    "ocicolumnisnull",
    "ocicolumnname",
    "ocicolumnprecision",
    "ocicolumnscale",
    "ocicolumnsize",
    "ocicolumntype",
    "ocicolumntyperaw",
    "ocicommit",
    "ocidefinebyname",
    "ocierror",
    "ociexecute",
    "ocifetch",
    "ocifetchinto",
    "ocifetchstatement",
    "ocifreecollection",
    "ocifreecursor",
    "ocifreedesc",
    "ocifreestatement",
    "ocigetelem",
    "ociinternaldebug",
    "ociloadlob",
    "ocilogon",
    "ocinewcollection",
    "ocinewcursor",
    "ocinewdescriptor",
    "ocinlogon",
    "ocinumcols",
    "ociparse",
    "ocipasswordchange",
    "ociplogon",
    "ociresult",
    "ocirollback",
    "ocisavelob",
    "ocisavelobfile",
    "ociserverversion",
    "ocisetprefetch",
    "ocistatementtype",
    "ociwritelobtofile",
    "ociwritetemporarylob",
    "odbc_do",
    "odbc_field_precision",
    "pdf_add_outline",
    "pg_clientencoding",
    "pg_setclientencoding",
    "pos",
    "recode",
    "show_source",
    "sizeof",
    "snmpwalkoid",
    "strchr",
    "streammp3",
    "swfaction",
    "swfbitmap",
    "swfbutton",
    "swffill",
    "swffont",
    "swfgradient",
    "swfmorph",
    "swfmovie",
    "swfshape",
    "swfsprite",
    "swftext",
    "swftextfield",
    "xptr_new_context",
];

/// XHP65: calls to alias functions (case-insensitive).
fn rule_alias_functions(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        let lower = call.name.to_ascii_lowercase();
        if ALIAS_FUNCTIONS.contains(&lower.as_str()) {
            push_call(
                65,
                tree,
                call,
                "Alias functions should be avoided.".to_string(),
                out,
            );
        }
    }
}

/// XHP68: named function declarations called `__lambda_func`
/// (case-sensitive); methods are `n_METHOD_DECLARATION` in PHP and are
/// skipped.
fn rule_lambda_func(tree: &Tree, out: &mut Vec<RawFinding>) {
    let blocks = tree.blocks();
    for (i, block) in blocks.iter().enumerate() {
        if block.kind != BlockKind::Function {
            continue;
        }
        if block.name.as_deref() != Some("__lambda_func") {
            continue;
        }
        if tree.is_method(&blocks, i) {
            continue;
        }
        out.push(RawFinding::at(
            68,
            tree.tokens[block.kw_idx].offset,
            "Declaring a function named `__lambda_func` causes any call to \
             create_function to fail. This is because `create_function` \
             eval-declares the function `__lambda_func`, then modifies the \
             symbol table so that the function is instead named \
             `\"\\0lambda_\".(++$i)`, and returns that name."
                .to_string(),
        ));
    }
}

/// XHP80: `parse_str()` without the second (result) parameter.
fn rule_parse_str(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if plain_call_named(call, "parse_str") && call.args.len() < 2 {
            push_call(
                80,
                tree,
                call,
                "Avoid `parse_str` unless the second parameter is specified. \
                 It is confusing and hinders static analysis."
                    .to_string(),
                out,
            );
        }
    }
}

/// XHP84: `array_combine($x, $x)` with identical arguments.
fn rule_array_combine(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if !plain_call_named(call, "array_combine") {
            continue;
        }
        if call.args.len() != 2 {
            continue;
        }
        if tree.arg_text(call, 0) == tree.arg_text(call, 1) {
            push_call(
                84,
                tree,
                call,
                "Prior to PHP 5.4, `array_combine()` fails when given empty arrays. \
                 Prefer to write `array_combine($x, $x)` as `array_fuse($x)`."
                    .to_string(),
                out,
            );
        }
    }
}

/// XHP85: calls whose lowercased name is a key of the
/// `xhpast.deprecated.functions` map; empty messages (PHP `empty()`,
/// including `"0"`) are skipped.
fn rule_deprecation(ctx: &RuleCtx, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        let lower = call.name.to_ascii_lowercase();
        if let Some(message) = deprecation_message(ctx.options, &lower) {
            push_call(85, ctx.tree, call, message, out);
        }
    }
}

fn deprecation_message(options: &XhpOptions, lower: &str) -> Option<String> {
    for (key, message) in &options.deprecated {
        if *key == *lower && !message.is_empty() && message != "0" {
            return Some(message.clone());
        }
    }
    None
}

/// XHP86: configured function / `new`-class calls whose watched
/// argument is not a constant string.
fn rule_unsafe_dynamic(ctx: &RuleCtx, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        let lower = call.name.to_ascii_lowercase();
        let Some(param) = dynamic_param(&ctx.options.dynamic_functions, &lower) else {
            continue;
        };
        if dynamic_arg_unsafe(ctx.tree, &call.args, param) {
            push_call(86, ctx.tree, call, unsafe_message(param, &call.name), out);
        }
    }
    unsafe_dynamic_classes(ctx, out);
}

/// Watched argument index for a lowercased name; PHP lowercases the
/// configured keys before lookup.
fn dynamic_param(map: &[(String, i64)], lower: &str) -> Option<i64> {
    for (key, param) in map {
        if key.to_ascii_lowercase() == lower {
            return Some(*param);
        }
    }
    None
}

/// True when the watched argument exists and is not a single plain
/// string literal.
fn dynamic_arg_unsafe(tree: &Tree, args: &[(usize, usize)], param: i64) -> bool {
    if param < 0 {
        return false;
    }
    let index =
        usize::try_from(param).expect("negative params return above, so try_from cannot fail");
    let Some(&(first, last)) = args.get(index) else {
        return false;
    };
    !(first == last && tree.tokens[first].kind == kinds::T_CONSTANT_ENCAPSED_STRING)
}

fn unsafe_message(param: i64, name: &str) -> String {
    format!(
        "Parameter {} of `{name}` should be a scalar string, otherwise it's not safe.",
        param + 1
    )
}

fn unsafe_dynamic_classes(ctx: &RuleCtx, out: &mut Vec<RawFinding>) {
    for (i, token) in ctx.tree.tokens.iter().enumerate() {
        if token.kind != kinds::T_NEW {
            continue;
        }
        let Some(name_idx) = ctx.tree.next_semantic(i) else {
            continue;
        };
        if ctx.tree.tokens[name_idx].kind != kinds::T_STRING {
            continue;
        }
        let (name, name_end) = qualified_name(ctx.tree, name_idx);
        let Some(lparen) = ctx.tree.next_semantic(name_end) else {
            continue;
        };
        if ctx.tree.tokens[lparen].kind != "(" {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let Some(param) = dynamic_param(&ctx.options.dynamic_classes, &lower) else {
            continue;
        };
        let end = ctx
            .tree
            .match_close(lparen)
            .unwrap_or_else(|| ctx.tree.tokens.len().saturating_sub(1));
        let args = split_call_args(ctx.tree, lparen, end);
        if dynamic_arg_unsafe(ctx.tree, &args, param) {
            out.push(RawFinding::at(
                86,
                token.offset,
                unsafe_message(param, &name),
            ));
        }
    }
}

/// A possibly qualified class name (`A\B`) and its last token index.
fn qualified_name(tree: &Tree, mut idx: usize) -> (String, usize) {
    let mut name = String::new();
    loop {
        name.push_str(&tree.tokens[idx].value);
        let Some(sep) = tree.next_semantic(idx) else {
            break;
        };
        if tree.tokens[sep].kind != "\\" {
            break;
        }
        let Some(after) = tree.next_semantic(sep) else {
            break;
        };
        if tree.tokens[after].kind != kinds::T_STRING {
            break;
        }
        name.push('\\');
        idx = after;
    }
    (name, idx)
}

/// Cast-style function calls and their cast names (case-insensitive).
const CAST_FUNCS: &[(&str, &str)] = &[
    ("boolval", "bool"),
    ("doubleval", "double"),
    ("floatval", "double"),
    ("intval", "int"),
    ("strval", "string"),
];

/// XHP105: `boolval()` etc. should be type casts. `intval($x, $base)`
/// with a non-numeric base reproduces the PHP `break` quirk and stops
/// the whole rule scan; a numeric non-`10` base skips just the call.
fn rule_cast_calls(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if call.is_construct {
            continue;
        }
        let lower = call.name.to_ascii_lowercase();
        let Some(cast) = cast_name(&lower) else {
            continue;
        };
        if lower == "intval" && call.args.len() >= 2 {
            let (first, last) = call.args[1];
            if !is_single_number(tree, first, last) {
                break;
            }
            if tree.tokens[first].value != "10" {
                continue;
            }
        }
        push_call(
            105,
            tree,
            call,
            format!(
                "For consistency, use `({cast})` (a type cast) instead of `{}` \
                 (a function call). Function calls impose additional overhead.",
                call.name
            ),
            out,
        );
    }
}

fn cast_name(lower: &str) -> Option<&str> {
    for entry in CAST_FUNCS {
        if entry.0 == lower {
            return Some(entry.1);
        }
    }
    None
}

/// Single numeric literal token, mirroring `n_NUMERIC_SCALAR`.
fn is_single_number(tree: &Tree, first: usize, last: usize) -> bool {
    first == last
        && (tree.tokens[first].kind == kinds::T_LNUMBER
            || tree.tokens[first].kind == kinds::T_DNUMBER)
}

/// XHP111: `is_a()` should be `instanceof`, unless a third argument
/// other than literal `false` is present. Fewer than two arguments is
/// skipped (the PHP rule would read a missing parameter node).
fn rule_is_a(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if !plain_call_named(call, "is_a") {
            continue;
        }
        if call.args.len() > 2 {
            let (first, last) = call.args[2];
            if !tree.span_text(first, last).eq_ignore_ascii_case("false") {
                continue;
            }
        }
        if call.args.len() < 2 {
            continue;
        }
        push_call(
            111,
            tree,
            call,
            "Use `instanceof` instead of `is_a`. The former is a language \
             construct whereas the latter is a function call, which has \
             additional overhead."
                .to_string(),
            out,
        );
    }
}

/// XHP125: decimal-looking `0...` literals that are not valid octal.
fn rule_invalid_octal(tree: &Tree, out: &mut Vec<RawFinding>) {
    for token in &tree.tokens {
        if token.kind != kinds::T_LNUMBER {
            continue;
        }
        if !is_octal_candidate(&token.value) {
            continue;
        }
        if token.value.bytes().all(|b| b.is_ascii_digit() && b < b'8') {
            continue;
        }
        out.push(RawFinding::at(
            125,
            token.offset,
            format!(
                "Invalid octal numeric scalar. `{}` is not a valid octal and \
                 will be interpreted as `{}`.",
                token.value,
                octal_value(&token.value)
            ),
        ));
    }
}

/// PHP `/^0\d+$/`.
fn is_octal_candidate(value: &str) -> bool {
    value.len() >= 2 && value.starts_with('0') && value.bytes().all(|b| b.is_ascii_digit())
}

/// PHP `intval($value, 8)`: leading valid digits parsed base 8.
fn octal_value(value: &str) -> i64 {
    let mut num: i64 = 0;
    for byte in value.bytes() {
        if !(b'0'..=b'7').contains(&byte) {
            break;
        }
        num = num.saturating_mul(8).saturating_add(i64::from(byte - b'0'));
    }
    num
}

/// XHP127: hexadecimal literals must read `0x` plus uppercase digits.
fn rule_hex_casing(tree: &Tree, out: &mut Vec<RawFinding>) {
    for token in &tree.tokens {
        if token.kind != kinds::T_LNUMBER {
            continue;
        }
        if !is_hex_candidate(&token.value) {
            continue;
        }
        if is_upper_hex(&token.value) {
            continue;
        }
        out.push(RawFinding::at(
            127,
            token.offset,
            "For consistency, write hexadecimals integers in uppercase with a \
             leading `0x`."
                .to_string(),
        ));
    }
}

/// PHP `/^0x[0-9A-F]+$/i`.
fn is_hex_candidate(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 2
        && bytes[0] == b'0'
        && (bytes[1] == b'x' || bytes[1] == b'X')
        && bytes[2..].iter().all(|b| b.is_ascii_hexdigit())
}

/// PHP `/^0x[0-9A-F]+$/`.
fn is_upper_hex(value: &str) -> bool {
    value.starts_with("0x")
        && value
            .bytes()
            .skip(2)
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

/// XHP129: two-argument `implode()` whose second argument looks like
/// the glue (static scalar or bare constant).
fn rule_implode_order(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if !plain_call_named(call, "implode") {
            continue;
        }
        if call.args.len() != 2 {
            continue;
        }
        let (first, last) = call.args[1];
        let scalar = tree.static_scalar_text(first, last).is_some();
        let constant = first == last && tree.tokens[first].kind == kinds::T_STRING;
        if scalar || constant {
            push_call(
                129,
                tree,
                call,
                "When calling \"implode()\", pass the \"glue\" argument first. (The \
                 other parameter order is deprecated in PHP 7.4 and raises a \
                 warning.)"
                    .to_string(),
                out,
            );
        }
    }
}

/// XHP131: binary literals must read `0b` plus `0`/`1` digits.
fn rule_binary_casing(tree: &Tree, out: &mut Vec<RawFinding>) {
    for token in &tree.tokens {
        if token.kind != kinds::T_LNUMBER {
            continue;
        }
        if !is_binary_candidate(&token.value) {
            continue;
        }
        if token.value.starts_with("0b") {
            continue;
        }
        out.push(RawFinding::at(
            131,
            token.offset,
            "For consistency, write binary integers with a leading `0b`.".to_string(),
        ));
    }
}

/// PHP `/^0b[01]+$/i`.
fn is_binary_candidate(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 2
        && bytes[0] == b'0'
        && (bytes[1] == b'b' || bytes[1] == b'B')
        && bytes[2..].iter().all(|b| *b == b'0' || *b == b'1')
}

/// XHP133: any call to `each()` (case-insensitive). Note the PHP
/// message has no trailing period.
fn rule_each_use(tree: &Tree, calls: &[CallSite], out: &mut Vec<RawFinding>) {
    for call in calls {
        if plain_call_named(call, "each") {
            push_call(
                133,
                tree,
                call,
                "Do not use \"each()\". This function was deprecated in PHP 7.2 \
                 and removed in PHP 8.0"
                    .to_string(),
                out,
            );
        }
    }
}

// XHP134 (ArcanistProductNameLiteralXHPASTLinterRule) is intentionally
// omitted: product names come from libphutil runtime
// (`PlatformSymbols::getProductNames()`), which cannot run here.

#[cfg(test)]
#[path = "xhp_calls_tests.rs"]
mod xhp_calls_tests;
