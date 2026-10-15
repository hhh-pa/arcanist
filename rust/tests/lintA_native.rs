//! Public-behavior tests for bundle A native linters (`NAME`, `JSON`, `XML`,
//! `MERGECONFLICT`, `GEN`, `NOLINT`, `CHMOD`) through the wired library.

use std::path::{Path, PathBuf};

use arc_rs::{
    lint::{self, Linter},
    lint_chmod, lint_filename, lint_generated, lint_json, lint_merge_conflict, lint_nolint,
    lint_xml,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-lintA-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file(&self, name: &str, contents: &[u8]) -> String {
        let disk = self.path.join(name);
        if let Some(parent) = disk.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&disk, contents).unwrap();
        disk.to_string_lossy().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new("tests/fixtures/lintA").join(name)).unwrap()
}

fn full_engine() -> lint::Engine {
    let mut engine = lint::Engine::empty();
    engine.add_linter(Box::new(lint_filename::FilenameLinter::new()));
    engine.add_linter(Box::new(lint_json::JsonLinter::new()));
    engine.add_linter(Box::new(lint_xml::XmlLinter::new()));
    engine.add_linter(Box::new(lint_merge_conflict::MergeConflictLinter::new()));
    engine.add_linter(Box::new(lint_generated::GeneratedLinter::new()));
    engine.add_linter(Box::new(lint_nolint::NoLintLinter::new()));
    engine.add_linter(Box::new(lint_chmod::ChmodLinter::new()));
    engine
}

#[test]
fn registry_names_match_config_names() {
    assert_eq!(lint_filename::FilenameLinter::new().name(), "NAME");
    assert_eq!(lint_json::JsonLinter::new().name(), "JSON");
    assert_eq!(lint_xml::XmlLinter::new().name(), "XML");
    assert_eq!(
        lint_merge_conflict::MergeConflictLinter::new().name(),
        "MERGECONFLICT"
    );
    assert_eq!(lint_generated::GeneratedLinter::new().name(), "GEN");
    assert_eq!(lint_nolint::NoLintLinter::new().name(), "NOLINT");
    assert_eq!(lint_chmod::ChmodLinter::new().name(), "CHMOD");
}

#[test]
fn filename_flags_bad_paths_through_engine() {
    let engine = full_engine();
    let found = engine.lint_bytes("has space.txt", b"content\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "NAME1");
    assert!(engine.lint_bytes("fine/name.txt", b"content\n").is_empty());
}

#[test]
fn json_fixtures_match_expectations() {
    let engine = full_engine();
    assert!(
        engine
            .lint_bytes("virtual/valid.json", &fixture("valid.json"))
            .is_empty()
    );
    let found = engine.lint_bytes("virtual/invalid.json", &fixture("invalid.json"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "JSON1");
    assert_eq!(found[0].severity, "error");
    assert_eq!(found[0].line, 3);
}

#[test]
fn xml_fixtures_match_expectations() {
    let engine = full_engine();
    assert!(
        engine
            .lint_bytes("virtual/valid.xml", &fixture("valid.xml"))
            .is_empty()
    );
    let found = engine.lint_bytes("virtual/invalid.xml", &fixture("invalid.xml"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "XML1");
    assert_eq!(found[0].severity, "error");
    assert!(found[0].description.contains("mismatched"));
}

#[test]
fn conflict_fixture_reports_three_markers() {
    let engine = full_engine();
    let found = engine.lint_bytes("virtual/conflict.txt", &fixture("conflict.txt"));
    assert_eq!(found.len(), 3);
    assert!(found.iter().all(|m| m.code == "MERGECONFLICT1"));
    assert_eq!(found[0].line, 2);
    assert_eq!(found[1].line, 4);
    assert_eq!(found[2].line, 6);
    assert!(
        engine
            .lint_bytes("virtual/clean.txt", &fixture("clean.txt"))
            .is_empty()
    );
}

#[test]
fn markers_are_detected_in_any_extension() {
    // The PHP linter has no path filter, so JSON/XML contents with markers
    // are still flagged (each linter self-filters only its own rule).
    let engine = full_engine();
    let found = engine.lint_bytes("virtual/data.json", b"{\n<<<<<<<\n}");
    assert!(found.iter().any(|m| m.code == "MERGECONFLICT1"));
    assert!(found.iter().any(|m| m.code == "JSON1"));
}

#[test]
fn generated_and_nolint_markers_detected_but_silent() {
    assert!(lint_generated::is_generated(&fixture(
        "generated_sample.js"
    )));
    assert!(!lint_generated::is_generated(&fixture("clean.txt")));
    assert!(lint_nolint::is_nolint(&fixture("nolint_sample.txt")));
    assert!(!lint_nolint::is_nolint(&fixture("clean.txt")));

    // Both linters raise no messages of their own.
    let engine = full_engine();
    assert!(
        engine
            .lint_bytes(
                "virtual/generated_sample.js",
                &fixture("generated_sample.js")
            )
            .is_empty()
    );
    assert!(
        engine
            .lint_bytes("virtual/nolint_sample.txt", &fixture("nolint_sample.txt"))
            .is_empty()
    );
}

#[test]
fn chmod_matrix_through_check_bytes() {
    // Non-executable: silent everywhere.
    assert!(lint_chmod::check_bytes("run.sh", b"echo\n", false).is_empty());
    // Executable text with shebang: silent.
    assert!(lint_chmod::check_bytes("virtual/script.sh", &fixture("script.sh"), true).is_empty());
    // Executable text without shebang: warning.
    let found = lint_chmod::check_bytes("virtual/plain.txt", &fixture("plain.txt"), true);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "CHMOD1");
    assert_eq!(found[0].severity, "warning");
}

#[test]
fn chmod_lint_bytes_silent_for_missing_and_plain_files() {
    let tmp = TempDir::new("chmod");
    let plain = tmp.file("plain.txt", b"echo hi\n");
    let linter = lint_chmod::ChmodLinter::new();
    assert!(linter.lint_bytes(&plain, b"echo hi\n").is_empty());
    assert!(
        linter
            .lint_bytes("virtual/missing.sh", b"echo hi\n")
            .is_empty()
    );
    assert!(tmp.path().join("plain.txt").exists());
}

#[test]
fn engine_results_are_sorted_and_combined() {
    let engine = full_engine();
    // Bad name plus conflict markers plus invalid JSON: one NAME1, one
    // JSON1, one MERGECONFLICT1, sorted by (line, col, code).
    let found = engine.lint_bytes("bad name.json", b"{\n<<<<<<<\n}");
    let codes: Vec<&str> = found.iter().map(|m| m.code.as_str()).collect();
    assert_eq!(codes, vec!["NAME1", "JSON1", "MERGECONFLICT1"]);
}
