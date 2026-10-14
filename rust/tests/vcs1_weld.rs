#[path = "../src/weld.rs"]
mod weld;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-vcs1-weld-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn write(&self, name: &str, body: &[u8]) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("vcs1")
        .join("weld")
        .join(name)
}

fn count_lines(bytes: &[u8]) -> usize {
    weld::split_lines_bytes(bytes).len()
}

#[test]
fn module_name_is_weld() {
    assert_eq!(weld::NAME, "weld");
}

#[test]
fn help_succeeds_and_prints_usage() {
    let dir = Fixture::named("help");
    let out = weld::execute(dir.path(), &args(&["--help"])).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("usage: arc-rs weld"), "unexpected: {text}");
    assert_eq!(
        weld::run_in(dir.path(), &args(&["--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn run_uses_current_dir() {
    let dir = Fixture::named("run-contract");
    dir.write("a.txt", b"a1\na2\na3\n");
    dir.write("b.txt", b"b1\nb2\nb3\n");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let code = weld::run(&args(&["--seed", "1", "a.txt", "b.txt"]));
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
}

#[test]
fn requires_two_files() {
    let dir = Fixture::named("arity");
    dir.write("a.txt", b"a\n");
    let err = weld::execute(dir.path(), &args(&["a.txt"])).unwrap_err();
    assert!(
        err.contains("Specify two or more files to weld together."),
        "unexpected: {err}"
    );
    let err = weld::execute(dir.path(), &args(&[])).unwrap_err();
    assert!(
        err.contains("Specify two or more files to weld together."),
        "unexpected: {err}"
    );
    assert_eq!(
        weld::run_in(dir.path(), &args(&["a.txt"])),
        ExitCode::FAILURE
    );
}

#[test]
fn missing_file_fails() {
    let dir = Fixture::named("missing");
    dir.write("a.txt", b"a\n");
    let err = weld::execute(dir.path(), &args(&["a.txt", "nope.txt"])).unwrap_err();
    assert!(err.contains("cannot read 'nope.txt'"), "unexpected: {err}");
    assert_eq!(
        weld::run_in(dir.path(), &args(&["a.txt", "nope.txt"])),
        ExitCode::FAILURE
    );
}

#[test]
fn bad_flags_fail() {
    let dir = Fixture::named("flags");
    let err = weld::execute(dir.path(), &args(&["--bogus", "a", "b"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = weld::execute(dir.path(), &args(&["--seed", "abc", "a", "b"])).unwrap_err();
    assert!(err.contains("invalid --seed"), "unexpected: {err}");
    assert_eq!(
        weld::run_in(dir.path(), &args(&["--bogus", "a", "b"])),
        ExitCode::FAILURE
    );
}

#[test]
fn seeded_weld_is_deterministic() {
    let dir = Fixture::named("deterministic");
    dir.write("a.txt", b"alpha\nbeta\ngamma\ndelta\n");
    dir.write("b.txt", b"one\ntwo\nthree\nfour\n");
    let argv = args(&["--seed", "1234", "a.txt", "b.txt"]);
    let first = weld::execute(dir.path(), &argv).unwrap();
    let second = weld::execute(dir.path(), &argv).unwrap();
    assert_eq!(first, second);
    assert!(!first.is_empty());
}

#[test]
fn weld_fuses_both_sides_within_line_bounds() {
    let dir = Fixture::named("bounds");
    let a: Vec<u8> = (0..20)
        .flat_map(|i| format!("a{i:02}\n").into_bytes())
        .collect();
    let b: Vec<u8> = (0..20)
        .flat_map(|i| format!("b{i:02}\n").into_bytes())
        .collect();
    dir.write("a.txt", &a);
    dir.write("b.txt", &b);
    let out = weld::execute(dir.path(), &args(&["--seed", "9", "a.txt", "b.txt"])).unwrap();
    // Two 20-line files fuse with a ceil(20/2) = 10-line overlap.
    assert_eq!(count_lines(&out), 30);
    let text = String::from_utf8(out).unwrap();
    // The untouched head of the first file and tail of the second survive.
    assert!(text.starts_with("a00\n"), "unexpected head: {text:?}");
    assert!(text.ends_with("b19\n"), "unexpected tail: {text:?}");
}

#[test]
fn empty_files_are_skipped() {
    let dir = Fixture::named("empty");
    dir.write("empty.txt", b"");
    dir.write("a.txt", b"a1\na2\n");
    dir.write("b.txt", b"b1\nb2\n");
    let with_empty = weld::execute(
        dir.path(),
        &args(&["--seed", "3", "empty.txt", "a.txt", "b.txt"]),
    )
    .unwrap();
    let without_empty =
        weld::execute(dir.path(), &args(&["--seed", "3", "a.txt", "b.txt"])).unwrap();
    assert_eq!(with_empty, without_empty);
}

#[test]
fn all_empty_files_weld_to_nothing() {
    let dir = Fixture::named("all-empty");
    dir.write("a.txt", b"");
    dir.write("b.txt", b"");
    let out = weld::execute(dir.path(), &args(&["a.txt", "b.txt"])).unwrap();
    assert_eq!(out, Vec::<u8>::new());
    assert_eq!(
        weld::run_in(dir.path(), &args(&["a.txt", "b.txt"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn dashed_filenames_work_after_separator() {
    let dir = Fixture::named("dashed");
    dir.write("--weird", b"w1\nw2\n");
    dir.write("b.txt", b"b1\nb2\n");
    let out = weld::execute(
        dir.path(),
        &args(&["--seed", "5", "--", "--weird", "b.txt"]),
    )
    .unwrap();
    assert!(!out.is_empty());
    assert_eq!(count_lines(&out), 4 - weld_overlap_for_seed(5, 2, 2));
}

fn weld_overlap_for_seed(seed: u64, buffered: usize, incoming: usize) -> usize {
    // Replay the overlap draws: the first file draws (and discards, since
    // the buffer is empty), then the second file draws the real joint.
    let mut rng = weld::Rng::new(seed);
    let _ = weld::overlap_for(0, buffered, &mut rng);
    weld::overlap_for(buffered, incoming, &mut rng)
}

#[test]
fn binary_input_round_trips_deterministically() {
    let dir = Fixture::named("binary");
    dir.write("a.txt", &[0xff, 0xfe, b'\n', 0x00, b'a', b'\n']);
    dir.write("b.txt", &[0x80, 0x81, b'\n', b'z', b'\n']);
    let argv = args(&["--seed", "11", "a.txt", "b.txt"]);
    let first = weld::execute(dir.path(), &argv).unwrap();
    let second = weld::execute(dir.path(), &argv).unwrap();
    assert_eq!(first, second);
    assert!(!first.is_empty());
}

#[test]
fn welds_the_shipped_fixtures() {
    let dir = Fixture::named("fixtures");
    let a = fixture_path("a.txt");
    let b = fixture_path("b.txt");
    assert!(a.exists(), "missing fixture: {}", a.display());
    assert!(b.exists(), "missing fixture: {}", b.display());
    let out = weld::execute(
        dir.path(),
        &args(&["--seed", "2026", a.to_str().unwrap(), b.to_str().unwrap()]),
    )
    .unwrap();
    // 6 + 5 lines with a 5-line overlap: 6 lines out.
    assert_eq!(count_lines(&out), 6);
    assert!(String::from_utf8(out).is_ok());
}

#[test]
fn single_text_input_passes_through() {
    let mut rng = weld::Rng::new(0);
    assert_eq!(weld::weld_texts(&["a\nb\n"], &mut rng), "a\nb\n");
}
