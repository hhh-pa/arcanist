use super::*;

#[test]
fn linter_name_is_filename() {
    assert_eq!(FilenameLinter::new().name(), "NAME");
}

#[test]
fn boring_names_pass() {
    let linter = FilenameLinter::new();
    for path in [
        "src/main.rs",
        "README.md",
        "a/b_c-d.e",
        "UPPER.TXT",
        "dir\\win.txt",
        ".hidden",
        "a",
        "0/1_2-3.4",
        "with-dash_and.dot/slash",
    ] {
        assert!(linter.lint_bytes(path, b"").is_empty(), "for {path}");
        assert!(is_valid_filename(path), "for {path}");
    }
}

#[test]
fn creative_names_fail_with_path_message() {
    let linter = FilenameLinter::new();
    for path in [
        "my file.txt",
        "weird!name",
        "semi;colon",
        "quo\"te",
        "dol$lar",
        "par(en)t",
        "a+b",
        "eq=ual",
        "at@sign",
        "hash#tag",
        "per%cent",
        "q?mark",
        "st*ar",
        "br[ack]",
        "cu{rly}",
        "a<b>c",
        "pi|pe",
        "back`tick",
        "ti~lde",
        "co,lon",
        "co'lon",
        "co:lon",
        "",
    ] {
        let found = linter.lint_bytes(path, b"data");
        assert_eq!(found.len(), 1, "for {path:?}");
        assert_eq!(found[0].path, path);
        assert_eq!(found[0].code, "NAME1");
        assert_eq!(found[0].severity, "error");
        assert_eq!(found[0].name, "Bad Filename");
        assert_eq!((found[0].line, found[0].col), (0, 0));
        assert!(!is_valid_filename(path), "for {path:?}");
    }
}

#[test]
fn non_ascii_names_fail() {
    let linter = FilenameLinter::new();
    assert_eq!(linter.lint_bytes("ünïcödé.txt", b"x").len(), 1);
    assert!(!is_valid_filename("ünïcödé.txt"));
}

#[test]
fn contents_are_ignored() {
    // Mirrors shouldLintBinaryFiles/directories/symbolicLinks = true: even
    // binary-looking data under a good path passes, and contents never
    // rescue a bad path.
    let linter = FilenameLinter::new();
    assert!(linter.lint_bytes("ok/bin.dat", b"\x00\x01\x02").is_empty());
    assert_eq!(linter.lint_bytes("bad name", b"").len(), 1);
}

#[test]
fn description_matches_php() {
    let found = FilenameLinter::new().lint_bytes("bad name", b"");
    assert_eq!(
        found[0].description,
        "Name files using only letters, numbers, period, hyphen and underscore."
    );
}
