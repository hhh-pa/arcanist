use super::*;

fn lint(data: &str) -> Vec<LintMessage> {
    PhutilLibraryLinter::new().lint_bytes("file.php", data.as_bytes())
}

fn def(symbol: &str, kind: SymbolType, library: &str, file: &str) -> SymbolDef {
    SymbolDef {
        symbol: symbol.to_string(),
        symbol_type: kind,
        library: library.to_string(),
        file: file.to_string(),
        offset: 0,
    }
}

#[test]
fn single_class_and_functions_only_are_clean() {
    assert!(lint("<?php\n\nfinal class Solo {}\n").is_empty());
    assert!(lint("<?php\n\ninterface Solo {}\n").is_empty());
    assert!(lint("<?php\n\nfunction alpha() {}\n\nfunction beta() {}\n").is_empty());
}

#[test]
fn two_classes_raise_at_last_class() {
    let messages = lint("<?php\n\nclass First {}\n\nclass Second {}\n");
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.code, "PHL3");
    assert_eq!(message.severity, "error");
    assert_eq!(message.name, "One Class Per File");
    assert_eq!((message.line, message.col), (5, 1));
    assert_eq!(
        message.description,
        "File 'file.php' declares more than one class or interface (First, Second). \
         A file which declares a class or interface MUST declare nothing else."
    );
}

#[test]
fn mixed_functions_and_classes_raise_at_last_function() {
    let messages = lint("<?php\n\nfunction helper() {}\n\nclass Thing {}\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHL3");
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert!(
        messages[0]
            .description
            .contains("mixes function (helper) and class/interface (Thing)")
    );
}

#[test]
fn scanner_ignores_non_code_and_anonymous_forms() {
    // Anonymous classes, closures, methods, `::class`, comments, strings,
    // heredocs, and HTML regions contribute no declarations.
    let data = concat!(
        "<?php\n",
        "// class FakeComment {}\n",
        "# interface FakeHash {}\n",
        "/* function fake_block() {} */\n",
        "$text = 'class FakeString {}';\n",
        "$html = \"function fake_interp() {}\";\n",
        "$heredoc = <<<EOT\n",
        "class FakeHeredoc {}\n",
        "EOT;\n",
        "class Real {\n",
        "  public function method() {}\n",
        "  public function make() { return new class {}; }\n",
        "}\n",
        "$name = Real::class;\n",
        "$closure = function () {};\n",
        "?>\n",
        "<p class=\"nope\">function nope() {}</p>\n",
    );
    assert!(lint(data).is_empty());

    // Uppercase keywords still declare.
    let messages = lint("<?php\n\nCLASS One {}\n\nCLASS Two {}\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHL3");

    // `new class extends Base` without a name is anonymous.
    assert!(lint("<?php\n\nclass Real {}\n\n$x = new class extends Real {};\n").is_empty());
}

#[test]
fn non_php_and_binary_inputs_skip_fast() {
    let linter = PhutilLibraryLinter::new();
    assert!(
        linter
            .lint_bytes("file.txt", b"<?php class A {} class B {}")
            .is_empty()
    );
    assert!(
        linter
            .lint_bytes("file.php", b"<?php\0 class A {}")
            .is_empty()
    );
    assert!(linter.lint_bytes("plain.php", b"no tags here").is_empty());
    assert!(is_php_file("a/b.PHP"));
    assert!(!is_php_file("a/php.txt"));
}

#[test]
fn duplicates_cover_prior_builtin_and_case() {
    let builtin = BuiltinSymbol {
        symbol: "strlen".to_string(),
        symbol_type: SymbolType::Function,
    };
    let defs = [
        def("Widget", SymbolType::Class, "lib", "a.php"),
        def("idx", SymbolType::Function, "lib", "a.php"),
        def("Widget", SymbolType::Class, "lib", "b.php"),
        def("strlen", SymbolType::Function, "lib", "c.php"),
        def("IdX", SymbolType::Function, "lib", "d.php"),
    ];
    let findings = check_duplicates(&defs, &[builtin]);
    assert_eq!(findings.len(), 3);
    assert!(findings.iter().all(|finding| finding.code == "PHL2"));
    assert_eq!(
        findings[0].description,
        "Definition of symbol \"Widget\" (of type \"class\") in file \"b.php\" in \
         library \"lib\" duplicates prior definition in file \"a.php\" in library \"lib\"."
    );
    assert!(
        findings[1]
            .description
            .contains("duplicates builtin definition")
    );
    assert!(findings[2].description.contains("\"IdX\""));
    assert_eq!(findings[2].file, "d.php");
}

#[test]
fn interfaces_share_the_class_namespace() {
    let defs = [
        def("Widget", SymbolType::Class, "lib", "a.php"),
        def("Widget", SymbolType::Interface, "lib", "b.php"),
    ];
    let findings = check_duplicates(&defs, &[]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "PHL2");
    assert!(findings[0].description.contains("(of type \"interface\")"));
}

#[test]
fn distinct_namespaces_do_not_collide() {
    let defs = [
        def("Thing", SymbolType::Class, "lib", "a.php"),
        def("Thing", SymbolType::Function, "lib", "b.php"),
    ];
    assert!(check_duplicates(&defs, &[]).is_empty());
}

#[test]
fn unknown_and_noncanonical_uses_report() {
    let defs = [def("idx", SymbolType::Function, "lib", "a.php")];
    let uses = [
        SymbolUse {
            symbol: "idx".to_string(),
            use_type: UseType::Function,
            library: "lib".to_string(),
            file: "b.php".to_string(),
            offset: 3,
        },
        SymbolUse {
            symbol: "IdX".to_string(),
            use_type: UseType::Function,
            library: "lib".to_string(),
            file: "b.php".to_string(),
            offset: 10,
        },
        SymbolUse {
            symbol: "Missing".to_string(),
            use_type: UseType::Class,
            library: "lib".to_string(),
            file: "b.php".to_string(),
            offset: 20,
        },
        SymbolUse {
            symbol: "MissingIface".to_string(),
            use_type: UseType::Interface,
            library: "lib".to_string(),
            file: "b.php".to_string(),
            offset: 30,
        },
        SymbolUse {
            symbol: "MissingEither".to_string(),
            use_type: UseType::ClassOrInterface,
            library: "lib".to_string(),
            file: "b.php".to_string(),
            offset: 40,
        },
    ];
    let findings = check_unknown_symbols(&uses, &defs, &[], "arc", "/arc");
    assert_eq!(findings.len(), 4);
    assert_eq!(findings[0].code, "PHL4");
    assert_eq!(
        findings[0].description,
        "Function symbol \"IdX\" should be written as \"idx\"."
    );
    assert_eq!(findings[1].code, "PHL1");
    assert!(
        findings[1]
            .description
            .starts_with("Use of unknown class symbol \"Missing\".\n\nCommon causes are:")
    );
    assert!(findings[1].description.contains("arc liberate"));
    assert_eq!(findings[2].code, "PHL1");
    assert!(
        findings[2]
            .description
            .starts_with("Use of unknown interface symbol \"MissingIface\".")
    );
    assert_eq!(findings[3].code, "PHL1");
    assert!(
        findings[3]
            .description
            .starts_with("Use of unknown class or interface symbol \"MissingEither\".")
    );

    let message = findings[1].to_message("b.php", 2, 5);
    assert_eq!(message.path, "b.php");
    assert_eq!((message.line, message.col), (2, 5));
    assert_eq!(message.code, "PHL1");
    assert_eq!(message.severity, "error");
}

#[test]
fn unknown_details_match_php_text() {
    assert_eq!(
        unknown_symbol_details("arc", "/root"),
        "Common causes are:\n\
         \n\
         \x20 - Your copy of arc is out of date.\n\
         \x20   This is the most common cause.\n\
         \x20   Update this copy of arc:\n\
         \n\
         \x20     /root\n\
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
    );
}
