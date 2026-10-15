//! XHP declaration rules: classes, methods, properties (T1128, dv-c).
//!
//! PHP reference: the rules under `src/lint/linter/xhpast/rules/`
//! listed below (commit `bd12c2df`, mirrored at
//! `%TEMP%\xhp_php\rules\` on this machine; read them with
//! `Get-Content` via PowerShell).
//!
//! Assigned rules (ID: PHP class):
//! - 9: ArcanistNamingConventionsXHPASTLinterRule (naming; see also
//!   the hook predicates in `%TEMP%\xhp_php\` — hook OVERRIDES are
//!   not portable, enforce defaults only)
//! - 10: ArcanistImplicitConstructorXHPASTLinterRule
//! - 13: ArcanistStaticThisXHPASTLinterRule (`$this` in statics)
//! - 19: ArcanistClassFilenameMismatchXHPASTLinterRule (uses
//!   `RuleCtx::path` for the filename)
//! - 24: ArcanistBraceFormattingXHPASTLinterRule (braces; note the
//!   alternative-syntax exemption implied by the AST shapes)
//! - 52: ArcanistImplicitVisibilityXHPASTLinterRule
//! - 55: ArcanistUnnecessaryFinalModifierXHPASTLinterRule
//! - 57: ArcanistSelfMemberReferenceXHPASTLinterRule (`self::`)
//! - 59: ArcanistInnerFunctionXHPASTLinterRule
//! - 60: ArcanistDefaultParametersXHPASTLinterRule
//! - 63: ArcanistUselessOverridingMethodXHPASTLinterRule
//! - 64: ArcanistNoParentScopeXHPASTLinterRule (`parent::`, note the
//!   case-sensitive `'parent'` comparison)
//! - 67: ArcanistToStringExceptionXHPASTLinterRule (`__toString`)
//! - 70: ArcanistInvalidDefaultParameterXHPASTLinterRule
//! - 72: ArcanistInvalidModifiersXHPASTLinterRule
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
//! - 130: ArcanistPublicPropertyXHPASTLinterRule (`public` props)
//!
//! Contract: read each PHP rule's `process()` and reproduce its
//! trigger conditions against [`super::xhp_ast`] (`Tree::blocks`,
//! `Tree::is_method`, `Tree::enclosing_class`, token scans for
//! property/constant declarations inside class bodies). Emit
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
//! `xhp_decl_tests.rs`.

use super::xhp_ast::{RawFinding, RuleCtx};

/// Run all declaration rules, pushing findings into `out`.
pub fn apply(_ctx: &RuleCtx, _out: &mut Vec<RawFinding>) {
    // TODO(dv-c): implement assigned rules.
}

#[cfg(test)]
#[path = "xhp_decl_tests.rs"]
mod xhp_decl_tests;
