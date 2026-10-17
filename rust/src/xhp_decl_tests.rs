use super::super::xhp_ast as ast;

fn check(source: &str) {
    check_path("test.php", source);
}

fn check_path(path: &str, source: &str) {
    let canned = ast::tokens_to_canned(source, &ast::tokenize(source));
    let tree = ast::tree_from_canned(&canned).expect("canned output must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path,
        options: &ast::XhpOptions::default(),
    };
    let mut out = Vec::new();
    super::apply(&ctx, &mut out);
    // Assigned rules were ported to xhp_rem1/2/3: nothing runs here.
    assert!(out.is_empty());
}

#[test]
fn placeholder_decl_rules_run() {
    check("<?php echo 1;\n");
}
