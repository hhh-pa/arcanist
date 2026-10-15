use super::*;

#[test]
fn md5_matches_rfc1321_vectors() {
    let cases = [
        ("", "d41d8cd98f00b204e9800998ecf8427e"),
        ("a", "0cc175b9c0f1b6a831c399e269772661"),
        ("abc", "900150983cd24fb0d6963f7d28e17f72"),
        ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
        (
            "abcdefghijklmnopqrstuvwxyz",
            "c3fcd3d76192e4007dfb496cca67e13b",
        ),
        (
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            "d174ab98d277d9f5a5611c2c9f419d9f",
        ),
        (
            "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            "57edf4a22be3c955ac49da2e2107b67a",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(md5_hex(input.as_bytes()), expected, "for {input:?}");
    }
}

#[test]
fn lock_hash_extracts_string_hashes() {
    assert_eq!(
        lock_hash(r#"{"hash": "abc", "packages": []}"#).unwrap(),
        Some("abc".to_string())
    );
    assert_eq!(lock_hash(r#"{"packages": []}"#).unwrap(), None);
    assert_eq!(lock_hash(r#"{"hash": 7}"#).unwrap(), None);
    assert_eq!(lock_hash(r#"[]"#).unwrap(), None);
    assert!(lock_hash("{nope").is_err());
    assert!(lock_hash(r#""str""#).is_err());
}

#[test]
fn check_pair_compares_md5_to_hash() {
    let json = br#"{"name": "example"}"#;
    let matching = format!(r#"{{"hash": "{}"}}"#, md5_hex(json));
    assert!(!check_pair(json, &matching).unwrap());
    assert!(check_pair(json, r#"{"hash": "nope"}"#).unwrap());
    assert!(check_pair(json, r#"{}"#).unwrap());
    assert!(check_pair(json, "{nope").is_err());
}

#[test]
fn only_composer_json_files_are_checked() {
    assert!(is_composer_json("composer.json"));
    assert!(is_composer_json("some/dir/composer.json"));
    assert!(!is_composer_json("composer.lock"));
    assert!(!is_composer_json("my-composer.json.bak"));
    assert!(!is_composer_json("Composer.JSON"));

    let linter = ComposerLinter::new();
    assert!(linter.lint_bytes("other.json", b"{}").is_empty());
    assert!(linter.lint_bytes("composer.lock", b"{}").is_empty());
    // A composer.json with no sibling lock stays silent (PHP would throw;
    // there is no failure channel here).
    assert!(
        linter
            .lint_bytes("lintB-missing-dir-xyz/composer.json", b"{}")
            .is_empty()
    );
    assert!(linter.lint_bytes("composer.json", b"{\0}").is_empty());
}
