//! `arc-rs lint` XHPAST linter (T1128, divergence dv-c).
//!
//! PHP reference: `src/lint/linter/ArcanistXHPASTLinter.php` (plus
//! `ArcanistBaseXHPASTLinter.php` and the rules under
//! `src/lint/linter/xhpast/rules/`).
//!
//! Behavior: when the `xhpast` binary is present on `PATH`, it is run
//! on the file; a nonzero exit yields one path-level message
//! (`XHP1` "PHP Syntax Error!" at the reported line when stderr
//! carries a syntax error, else `XHP2` "Unable to Parse"),
//! mirroring PHP `resolveFuture()`. On a zero exit the JSON AST
//! output is parsed (see [`xhp_ast`]) and the ported rule tree
//! (`xhp_calls`, `xhp_style`, `xhp_flow`, `xhp_rem1`, plus the
//! `xhp_decl` declaration rules once implemented) runs over it, emitting
//! PHP-identical rule IDs, severities, and message text for the
//! ported subset ([`RULE_TABLE`]); the unported remainder is listed
//! in [`blocked_rules`]. When the binary is absent the linter is
//! inactive and reports no messages.
//!
//! Divergences from PHP:
//! - The real `xhpast` wire format is undocumented in-repo
//!   (libphutil is not vendored), so this port defines a JSON AST
//!   equivalent. Non-JSON tool output on a zero exit falls back to
//!   the built-in tokenizer over the real file bytes, so rule
//!   evaluation still runs.
//! - XHP45 (PHP Compatibility) ports the `xhpast.php-version`
//!   gating plus the PHP 5.3/5.4 syntax-feature checks; the
//!   symbol-table checks need the 1.4MB
//!   `resources/php/symbol-information.json` data file and are not
//!   ported.
//! - XHP54 (Formatted String) ports the missing-format-string check;
//!   libphutil `xsprintf()` conversion validation is not ported.
//! - The `xhpast.switchhook` / `xhpast.naminghook` overrides name
//!   custom PHP classes and cannot run here; XHP9/XHP30 enforce
//!   their default determinations.
//! - No autofix replacements are produced (`original`/`replacement`
//!   are always `None`); rule IDs, severities, names, and message
//!   text match PHP.
//! - Console output follows the engine convention (see `lint.rs`):
//!   no colors or markup; messages are plain `LintMessage`s.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{
    LintMessage, Linter, OptionContext, SEVERITY_ADVICE, SEVERITY_DISABLED, SEVERITY_ERROR,
    SEVERITY_WARNING,
};

#[path = "xhp_ast.rs"]
pub mod xhp_ast;
#[path = "xhp_calls.rs"]
mod xhp_calls;
#[path = "xhp_decl.rs"]
mod xhp_decl;
#[path = "xhp_flow.rs"]
mod xhp_flow;
#[path = "xhp_rem1.rs"]
mod xhp_rem1;
#[path = "xhp_rem3.rs"]
mod xhp_rem3;
#[path = "xhp_style.rs"]
mod xhp_style;

/// XHPAST linter: PHP rule evaluation over `xhpast` AST output.
#[derive(Debug, Default)]
pub struct XhpastLinter {
    options: std::collections::HashMap<String, serde_json::Value>,
}

impl XhpastLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Tool binary name looked up on `PATH`.
#[must_use]
pub fn binary_name() -> &'static str {
    "xhpast"
}

/// Exact tool argv flags: none (the file path is the sole argument).
#[must_use]
pub fn command_flags() -> Vec<String> {
    Vec::new()
}

/// Rule metadata for PORTED rules: (ID, lint name, severity),
/// mirroring the PHP `getLintName()` / `getLintSeverity()` pairs
/// (rules without an override default to error, per
/// `ArcanistXHPASTLinterRule`). Unported rules live in
/// [`blocked_rules`].
pub const RULE_TABLE: &[(u32, &str, &str)] = &[
    (1, "PHP Syntax Error!", SEVERITY_ERROR),
    (2, "Unable to Parse", SEVERITY_WARNING),
    (3, "Use of Variable Variable", SEVERITY_ERROR),
    (4, "Use of `extract`", SEVERITY_ERROR),
    (5, "Use of Undeclared Variable", SEVERITY_ERROR),
    (6, "Use of Short Tag `<?`", SEVERITY_ERROR),
    (7, "Use of Echo Tag `<?=`", SEVERITY_ERROR),
    (8, "Use of Close Tag `?>`", SEVERITY_ERROR),
    (9, "Naming Conventions", SEVERITY_WARNING),
    (10, "Implicit Constructor", SEVERITY_ERROR),
    (12, "Dynamic `define`", SEVERITY_ERROR),
    (13, "Use of `$this` in Static Context", SEVERITY_ERROR),
    (15, "Expected Open Tag", SEVERITY_ERROR),
    (16, "TODO Comment", SEVERITY_DISABLED),
    (17, "`exit` Used as Expression", SEVERITY_ERROR),
    (18, "Comment Style", SEVERITY_ERROR),
    (19, "Class-Filename Mismatch", SEVERITY_ERROR),
    (20, "Tautological Expression", SEVERITY_ERROR),
    (21, "Not String Concatenation", SEVERITY_ERROR),
    (22, "Duplicate Keys in Array", SEVERITY_ERROR),
    (23, "Reuse of Iterator Variable", SEVERITY_ERROR),
    (24, "Brace Placement", SEVERITY_WARNING),
    (25, "Spaces Inside Parentheses", SEVERITY_WARNING),
    (26, "Space After Control Statement", SEVERITY_WARNING),
    (27, "Space Around Binary Operator", SEVERITY_WARNING),
    (28, "Spacing Before Array Index", SEVERITY_WARNING),
    (30, "Implicit Fallthrough", SEVERITY_WARNING),
    (32, "Variable Reused As Iterator", SEVERITY_ERROR),
    (34, "Comment Spaces", SEVERITY_ADVICE),
    (36, "Slow Construct", SEVERITY_WARNING),
    (37, "Call Formatting", SEVERITY_WARNING),
    (38, "Declaration Formatting", SEVERITY_WARNING),
    (39, "Reuse of Iterator References", SEVERITY_WARNING),
    (40, "Keyword Conventions", SEVERITY_WARNING),
    (41, "Unnecessary Double Quotes", SEVERITY_ADVICE),
    (42, "`elseif` Usage", SEVERITY_ADVICE),
    (43, "Semicolon Spacing", SEVERITY_ADVICE),
    (44, "Concatenation Spacing", SEVERITY_WARNING),
    (45, "PHP Compatibility", SEVERITY_ERROR),
    (46, "Language Construct Parentheses", SEVERITY_WARNING),
    (47, "Empty Block Statement", SEVERITY_ADVICE),
    (48, "Array Separator", SEVERITY_ADVICE),
    (49, "Constructor Parentheses", SEVERITY_ADVICE),
    (51, "Use of Blacklisted Function", SEVERITY_ERROR),
    (53, "Call-Time Pass-By-Reference", SEVERITY_ERROR),
    (54, "Formatted String", SEVERITY_ERROR),
    (56, "Unnecessary Semicolon", SEVERITY_ADVICE),
    (58, "Logical Operators", SEVERITY_ADVICE),
    (61, "Lowercase Functions", SEVERITY_ADVICE),
    (62, "Class Name Literal", SEVERITY_ADVICE),
    (65, "Alias Functions", SEVERITY_ADVICE),
    (66, "Cast Spacing", SEVERITY_ADVICE),
    (68, "`__lambda_func` Function", SEVERITY_ERROR),
    (71, "Modifier Ordering", SEVERITY_ADVICE),
    (73, "Space After Unary Prefix Operator", SEVERITY_WARNING),
    (74, "Object Operator Spacing", SEVERITY_WARNING),
    (75, "Space Before Unary Postfix Operator", SEVERITY_WARNING),
    (78, "Inline HTML", SEVERITY_DISABLED),
    (79, "Global Variables", SEVERITY_WARNING),
    (80, "Questionable Use of `parse_str`", SEVERITY_ERROR),
    (81, "Newline After PHP Open Tag", SEVERITY_ADVICE),
    (82, "Empty File", SEVERITY_WARNING),
    (83, "Parent Member Reference", SEVERITY_WARNING),
    (84, "`array_combine()` Unreliable", SEVERITY_DISABLED),
    (85, "Use of Deprecated Function", SEVERITY_WARNING),
    (86, "Unsafe Usage of Dynamic String", SEVERITY_ERROR),
    (87, "Class Not `abstract` Or `final`", SEVERITY_DISABLED),
    (88, "Class Not Extending `Phobject`", SEVERITY_DISABLED),
    (90, "Nested `namespace` Statements", SEVERITY_ERROR),
    (91, "`$this` Reassignment", SEVERITY_ERROR),
    (92, "Unexpected `return` Value", SEVERITY_WARNING),
    (95, "Self Class Reference", SEVERITY_WARNING),
    (96, "Paamayim Nekudotayim Spacing", SEVERITY_WARNING),
    (97, "`use` Statement Namespace Prefix", SEVERITY_WARNING),
    (
        98,
        "`namespace` Statement Must Be The First Statement",
        SEVERITY_ERROR,
    ),
    (99, "Unnecessary Symbol Alias", SEVERITY_WARNING),
    (105, "Function Call Should Be Type Cast", SEVERITY_ADVICE),
    (
        107,
        "`abstract` Method Cannot Be Declared `private`",
        SEVERITY_ERROR,
    ),
    (108, "`abstract` Method Cannot Contain Body", SEVERITY_ERROR),
    (111, "`is_a` Should Be `instanceof`", SEVERITY_ADVICE),
    (
        113,
        "`class` Containing `abstract` Methods Must Be Declared `abstract`",
        SEVERITY_ERROR,
    ),
    (
        114,
        "`interface` Method Cannot Contain Body",
        SEVERITY_ERROR,
    ),
    (
        118,
        "`interface` Methods Cannot Be Marked `abstract`",
        SEVERITY_ERROR,
    ),
    (119, "Curly Brace Array Index", SEVERITY_WARNING),
    (123, "Variable Reference Spacing", SEVERITY_WARNING),
    (125, "Invalid Octal Numeric Scalar", SEVERITY_ERROR),
    (127, "Hexadecimal Integer Casing", SEVERITY_WARNING),
    (128, "Continue Inside Switch", SEVERITY_ERROR),
    (129, "Implode With Glue First", SEVERITY_ERROR),
    (130, "Use of `public` Properties", SEVERITY_ADVICE),
    (131, "Binary Integer Casing", SEVERITY_WARNING),
    (132, "Partial Catch", SEVERITY_WARNING),
    (133, "Use of Removed Function \"each()\"", SEVERITY_ERROR),
];

/// Look up rule metadata (name, severity) by rule ID.
#[must_use]
pub fn rule_meta(id: u32) -> Option<(&'static str, &'static str)> {
    RULE_TABLE
        .iter()
        .find(|(rule, _, _)| *rule == id)
        .map(|(_, name, severity)| (*name, *severity))
}

/// Rules with no port in this subset: (ID, PHP class, reason).
/// Every other rule under `src/lint/linter/xhpast/rules/` (plus ID
/// 78, `ArcanistInlineHTMLXHPASTLinterRule`) is ported; see
/// [`RULE_TABLE`].
#[must_use]
pub fn blocked_rules() -> Vec<(u32, &'static str, &'static str)> {
    vec![
        (50, "ArcanistDuplicateSwitchCaseXHPASTLinterRule", NOT_YET),
        (52, "ArcanistImplicitVisibilityXHPASTLinterRule", NOT_YET),
        (
            55,
            "ArcanistUnnecessaryFinalModifierXHPASTLinterRule",
            NOT_YET,
        ),
        (57, "ArcanistSelfMemberReferenceXHPASTLinterRule", NOT_YET),
        (59, "ArcanistInnerFunctionXHPASTLinterRule", NOT_YET),
        (60, "ArcanistDefaultParametersXHPASTLinterRule", NOT_YET),
        (
            63,
            "ArcanistUselessOverridingMethodXHPASTLinterRule",
            NOT_YET,
        ),
        (64, "ArcanistNoParentScopeXHPASTLinterRule", NOT_YET),
        (67, "ArcanistToStringExceptionXHPASTLinterRule", NOT_YET),
        (69, "ArcanistInstanceOfOperatorXHPASTLinterRule", NOT_YET),
        (
            70,
            "ArcanistInvalidDefaultParameterXHPASTLinterRule",
            NOT_YET,
        ),
        (72, "ArcanistInvalidModifiersXHPASTLinterRule", NOT_YET),
        (76, "ArcanistArrayValueXHPASTLinterRule", NOT_YET),
        (77, "ArcanistListAssignmentXHPASTLinterRule", NOT_YET),
        (
            134,
            "ArcanistProductNameLiteralXHPASTLinterRule",
            "product names come from libphutil runtime; cannot run here",
        ),
    ]
}

/// Shared reason for follow-up rules: portable, but deferred to a
/// later dv-c increment so this subset lands fully verified.
const NOT_YET: &str = "not yet ported; deferred to a follow-up increment";

fn is_supported(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["php", "phtml", "inc"];
    let Some(ext) = extension_of(path) else {
        return false;
    };
    EXTENSIONS
        .iter()
        .any(|known| ext.eq_ignore_ascii_case(known))
}

fn extension_of(path: &str) -> Option<&str> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let dot = base.rfind('.')?;
    if dot + 1 >= base.len() {
        return None;
    }
    Some(&base[dot + 1..])
}

/// Locate `binary` on a `$PATH`-style search string (mirrors
/// `anoid::find_on_path` so this module stays standalone-includeable).
fn find_on_path(path_env: &std::ffi::OsStr, binary: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn binary_in(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    find_on_path(path_env, binary_name())
}

/// Locate the tool on the real `$PATH`; `None` means the linter is
/// inactive and reports no messages.
#[must_use]
pub fn binary_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    binary_in(&path)
}

/// Staged copy of the linted bytes: the external tool needs a real file.
struct StagedFile {
    dir: PathBuf,
    file: PathBuf,
}

impl StagedFile {
    fn stage(display_path: &str, data: &[u8]) -> Result<Self, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-lint-xhpast-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create temp dir: {e}"))?;
        let file = dir.join(staged_name(display_path));
        std::fs::write(&file, data).map_err(|e| format!("cannot stage lint input: {e}"))?;
        Ok(Self { dir, file })
    }

    fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Temp file name preserving a safe source extension so the tool sniffs
/// the file type correctly.
fn staged_name(display_path: &str) -> String {
    match extension_of(display_path) {
        Some(ext)
            if ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+') =>
        {
            format!("src.{ext}")
        }
        _ => "src.php".to_string(),
    }
}

fn run_tool(binary: &Path, file: &Path) -> Option<(i32, String, String)> {
    let mut command = Command::new(binary);
    command.args(command_flags()).arg(file).stdin(Stdio::null());
    let output = arc_rs::lint::spawn_with_retry(&mut command).ok()?;
    let code = output.status.code().unwrap_or(1);
    Some((
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Run the full rule tree over a parsed tree and emit messages
/// sorted by (offset, rule).
pub fn lint_tree(
    path: &str,
    tree: &xhp_ast::Tree,
    options: &xhp_ast::XhpOptions,
) -> Vec<LintMessage> {
    let ctx = xhp_ast::RuleCtx {
        tree,
        path,
        options,
    };
    let mut findings = Vec::new();
    xhp_calls::apply(&ctx, &mut findings);
    xhp_style::apply(&ctx, &mut findings);
    xhp_decl::apply(&ctx, &mut findings);
    xhp_flow::apply(&ctx, &mut findings);
    xhp_rem1::apply(&ctx, &mut findings);
    xhp_rem3::apply(&ctx, &mut findings);
    findings.sort_by_key(|finding| (finding.offset, finding.rule));
    findings
        .into_iter()
        .map(|finding| emit(path, tree, &finding))
        .collect()
}

fn emit(path: &str, tree: &xhp_ast::Tree, finding: &xhp_ast::RawFinding) -> LintMessage {
    let (name, severity) = rule_meta(finding.rule).unwrap_or(("XHP Lint", SEVERITY_WARNING));
    let (line, col) = if finding.offset == xhp_ast::PATH_LEVEL {
        (0, 0)
    } else {
        tree.line_col(finding.offset)
    };
    LintMessage {
        path: path.to_string(),
        line,
        col,
        code: format!("XHP{}", finding.rule),
        severity: severity.to_string(),
        name: name.to_string(),
        description: finding.message.clone(),
        original: None,
        replacement: None,
    }
}

/// Lint from canned `xhpast` JSON output (no binary required). The
/// JSON carries the source (see [`xhp_ast::tree_from_canned`]).
/// Unparseable output yields one `XHP2` warning.
pub fn lint_canned(path: &str, json: &str, options: &xhp_ast::XhpOptions) -> Vec<LintMessage> {
    match xhp_ast::tree_from_canned(json) {
        Ok(tree) => lint_tree(path, &tree, options),
        Err(_) => vec![unable_to_parse(path, 1, "")],
    }
}

fn unable_to_parse(path: &str, exit_code: i32, stderr: &str) -> LintMessage {
    let first_line = stderr.lines().next().unwrap_or("").trim();
    let description = if first_line.is_empty() {
        format!("xhpast could not parse this file (exit status {exit_code}).")
    } else {
        format!("xhpast could not parse this file (exit status {exit_code}): {first_line}")
    };
    LintMessage {
        path: path.to_string(),
        line: 0,
        col: 0,
        code: "XHP2".to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: "Unable to Parse".to_string(),
        description,
        original: None,
        replacement: None,
    }
}

fn stderr_line(stderr: &str) -> Option<usize> {
    let lower = stderr.to_ascii_lowercase();
    let mut best = None;
    for needle in ["on line ", "line "] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(needle) {
            let digits: String = lower[from + pos + needle.len()..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .map(|b| b as char)
                .collect();
            if let Ok(line) = digits.parse::<usize>()
                && line > 0
                && best.is_none()
            {
                best = Some(line);
            }
            from += pos + needle.len();
        }
    }
    best
}

fn looks_like_syntax_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("parse")
        || lower.contains("syntax")
        || lower.contains("unexpected")
        || lower.contains("error")
}

fn syntax_error(path: &str, stderr: &str, line: usize) -> LintMessage {
    let detail = stderr.lines().next().unwrap_or("").trim();
    let description = if detail.is_empty() {
        "This file contains a syntax error.".to_string()
    } else {
        format!("This file contains a syntax error: {detail}")
    };
    LintMessage {
        path: path.to_string(),
        line,
        col: 1,
        code: "XHP1".to_string(),
        severity: SEVERITY_ERROR.to_string(),
        name: "PHP Syntax Error!".to_string(),
        description,
        original: None,
        replacement: None,
    }
}

/// Linter body with an explicit tool path (mirrors the PHP `bin` option).
/// `None` (tool absent) reports no messages.
pub fn lint_with_binary(path: &str, data: &[u8], binary: Option<&Path>) -> Vec<LintMessage> {
    lint_with_binary_and_options(path, data, binary, &xhp_ast::XhpOptions::default())
}

/// Linter body with an explicit tool path and rule options.
pub fn lint_with_binary_and_options(
    path: &str,
    data: &[u8],
    binary: Option<&Path>,
    options: &xhp_ast::XhpOptions,
) -> Vec<LintMessage> {
    let Some(binary) = binary else {
        return Vec::new();
    };
    let Ok(staged) = StagedFile::stage(path, data) else {
        return Vec::new();
    };
    let Some((code, stdout, stderr)) = run_tool(binary, staged.path()) else {
        return Vec::new();
    };
    if code != 0 {
        return parse_output(path, code, &stdout, &stderr);
    }
    let source = String::from_utf8_lossy(data).into_owned();
    match xhp_ast::tree_from_canned_with_source(&source, &stdout) {
        Ok(tree) => lint_tree(path, &tree, options),
        Err(_) => lint_tree(path, &xhp_ast::tree_from_source(&source), options),
    }
}

/// Interpret the `xhpast` run, mirroring PHP `resolveFuture()`: a
/// nonzero exit yields `XHP1` (syntax error at the reported line)
/// when stderr carries one, else path-level `XHP2`. A zero exit with
/// canned JSON output runs the rule tree with default options;
/// anything else yields no messages.
#[must_use]
pub fn parse_output(path: &str, exit_code: i32, stdout: &str, stderr: &str) -> Vec<LintMessage> {
    if exit_code != 0 {
        if looks_like_syntax_error(stderr)
            && let Some(line) = stderr_line(stderr)
        {
            return vec![syntax_error(path, stderr, line)];
        }
        return vec![unable_to_parse(path, exit_code, stderr)];
    }
    match xhp_ast::tree_from_canned(stdout) {
        Ok(tree) => lint_tree(path, &tree, &xhp_ast::XhpOptions::default()),
        Err(_) => Vec::new(),
    }
}

impl Linter for XhpastLinter {
    fn name(&self) -> &'static str {
        "XHP"
    }

    fn severity_lookup_code<'a>(&self, full_code: &'a str) -> &'a str {
        full_code.strip_prefix("XHP").unwrap_or(full_code)
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported(path) {
            return Vec::new();
        }
        if data.contains(&0) {
            return Vec::new();
        }
        let options = xhp_ast::XhpOptions::from_map(&self.options);
        lint_with_binary_and_options(path, data, binary_on_path().as_deref(), &options)
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        _ctx: &OptionContext,
    ) -> Result<(), String> {
        match key {
            "xhpast.php-version"
            | "xhpast.php-version.windows"
            | "xhpast.blacklisted.function"
            | "xhpast.deprecated.functions"
            | "xhpast.printf-functions"
            | "xhpast.switchhook"
            | "xhpast.naminghook"
            | "xhpast.dynamic-string.classes"
            | "xhpast.dynamic-string.functions" => {
                self.options.insert(key.to_string(), value.clone());
                Ok(())
            }
            _ => Err(format!("unknown option '{key}'.")),
        }
    }
}

#[cfg(test)]
#[path = "lint_xhpast_tests.rs"]
mod lint_xhpast_tests;
