use super::*;

use std::path::{Path, PathBuf};

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn options(v: &[&str]) -> Options {
    match parse_args(&args(v)) {
        ParseOutcome::Args(options) => options,
        other => panic!("expected args, got {other:?}"),
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-cover-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init"]);
    git(tmp.path(), &["config", "user.email", "cover@test"]);
    git(tmp.path(), &["config", "user.name", "cover author"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    tmp
}

fn commit_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-m", contents]);
}

fn commit_as(dir: &Path, name: &str, contents: &str, author: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(
        dir,
        &[
            "-c",
            &format!("user.name={author}"),
            "commit",
            "-m",
            contents,
        ],
    );
}

fn commit_msg(dir: &Path, name: &str, contents: &str, message: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-m", message]);
}

/// Canonical cover call with empty stdin, non-TTY, and no user file
/// (COVER-C1: config-isolated and prompt-free).
fn cover_opts(dir: &Path, options: &Options, input: &str, tty: bool) -> Result<String, String> {
    let mut cursor = std::io::Cursor::new(input.as_bytes().to_vec());
    execute_cover_text_ansi_with_input_and_config(
        dir,
        options,
        false,
        &mut cursor,
        tty,
        None,
        &arc_rs::get_config::LoadOptions::default(),
    )
}

fn cover(dir: &Path, v: &[&str]) -> Result<String, String> {
    cover_opts(dir, &options(v), "", false)
}

/// COVER-C1: pin the default base via the scratch file (prompt-free).
fn write_scratch(dir: &Path, relative: &str) {
    let git_dir = git(dir, &["rev-parse", "--git-dir"]);
    let arc_dir = dir.join(git_dir).join("arc");
    std::fs::create_dir_all(&arc_dir).unwrap();
    std::fs::write(arc_dir.join("default-relative-commit"), relative).unwrap();
}

/// COVER-C1: point the current branch's upstream at `target`.
fn set_upstream(dir: &Path, target: &str) {
    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    git(dir, &["update-ref", "refs/remotes/origin/master", target]);
    git(
        dir,
        &["config", "remote.origin.url", "https://example.test/x.git"],
    );
    git(
        dir,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    git(
        dir,
        &["config", &format!("branch.{branch}.remote"), "origin"],
    );
    git(
        dir,
        &[
            "config",
            &format!("branch.{branch}.merge"),
            "refs/heads/master",
        ],
    );
}

#[test]
fn parses_rev_paths_and_help() {
    assert_eq!(
        parse_args(&args(&[])),
        ParseOutcome::Args(Options {
            rev: None,
            paths: Vec::new(),
        })
    );
    assert_eq!(
        parse_args(&args(&["--rev", "HEAD~1", "a.txt"])),
        ParseOutcome::Args(Options {
            rev: Some("HEAD~1".to_string()),
            paths: vec!["a.txt".to_string()],
        })
    );
    assert_eq!(
        parse_args(&args(&["--rev=abc", "--", "--weird"])),
        ParseOutcome::Args(Options {
            rev: Some("abc".to_string()),
            paths: vec!["--weird".to_string()],
        })
    );
    assert_eq!(parse_args(&args(&["--help"])), ParseOutcome::Help);
    // COVER-C14: PHP argument-parser bytes.
    assert_eq!(
        parse_args(&args(&["--rev"])),
        ParseOutcome::Error("Option '--rev' requires a parameter.".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus"])),
        ParseOutcome::Error("Unknown argument 'bogus'. Try 'arc help'.".to_string())
    );
}

#[test]
fn hunk_headers_yield_old_and_new_offsets() {
    assert_eq!(
        parse_hunk_header("-1,3 +5,7 @@"),
        Some(HunkWalk {
            old_line: 1,
            new_line: 5,
            old_rem: 3,
            new_rem: 7,
        })
    );
    // Missing counts imply one line; trailing context is ignored.
    assert_eq!(
        parse_hunk_header("-1 +2 @@ func +x"),
        Some(HunkWalk {
            old_line: 1,
            new_line: 2,
            old_rem: 1,
            new_rem: 1,
        })
    );
    assert_eq!(
        parse_hunk_header("-0,0 +1,2 @@"),
        Some(HunkWalk {
            old_line: 0,
            new_line: 1,
            old_rem: 0,
            new_rem: 2,
        })
    );
    assert_eq!(parse_hunk_header("garbage"), None);
    assert_eq!(parse_hunk_header("-1,0 +1,0 @@"), None);
}

#[test]
fn cover_map_tracks_old_side_lines() {
    // Modify middle line: `-` covers old 2; `+` covers old 2 and 3.
    // COVER-C5: PHP keys dedupe, so the repeated 2 appears once.
    let diff = "diff --git a/f.txt b/f.txt\n\
         index 1111111..2222222 100644\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -1,3 +1,3 @@\n\
         \x20one\n\
         -two\n\
         +TWO\n\
         \x20three\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![FileChanges {
            path: "f.txt".to_string(),
            lines: vec![2, 3],
        }]
    );
}

#[test]
fn cover_map_marks_insertion_neighbors() {
    // Pure insertion after old line 5 covers old 4 and 5 per added line.
    // COVER-C5: deduped to one pair.
    let diff = "diff --git a/f.txt b/f.txt\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -5,0 +6,2 @@\n\
         +a\n\
         +b\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![FileChanges {
            path: "f.txt".to_string(),
            lines: vec![4, 5],
        }]
    );
}

#[test]
fn cover_map_tracks_deletions_and_new_files() {
    let diff = "diff --git a/gone.txt b/gone.txt\n\
         deleted file mode 100644\n\
         --- a/gone.txt\n\
         +++ /dev/null\n\
         @@ -2,2 +1,0 @@\n\
         -old\n\
         -older\n\
         diff --git a/new.txt b/new.txt\n\
         new file mode 100644\n\
         --- /dev/null\n\
         +++ b/new.txt\n\
         @@ -0,0 +1,2 @@\n\
         +one\n\
         +two\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![
            FileChanges {
                path: "gone.txt".to_string(),
                lines: vec![2, 3],
            },
            // New files cover old line 0 (the PHP quirk); callers prune
            // added files and never attribute line 0. COVER-C5: deduped.
            FileChanges {
                path: "new.txt".to_string(),
                lines: vec![0],
            },
        ]
    );
}

#[test]
fn corpus_lines_beat_section_headers_by_count() {
    // An added line reading `+++ foo` must stay corpus: the hunk expects
    // exactly one new line, so the `+++` prefix never misroutes. The `+`
    // covers old 1 (predecessor, since old_line is 2) and old 2.
    // COVER-C5: deduped.
    let diff = "diff --git a/f.txt b/f.txt\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -1 +1 @@\n\
         -old\n\
         ++++ foo\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1, 2],
        }]
    );
}

#[test]
fn no_newline_markers_advance_counters_like_php() {
    let diff = "diff --git a/f.txt b/f.txt\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -1 +1 @@\n\
         -a\n\
         \\ No newline at end of file\n\
         +b\n\
         \\ No newline at end of file\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1, 2, 3],
        }]
    );
}

#[test]
fn binary_and_mode_only_sections_stay_empty() {
    let diff = "diff --git a/img.png b/img.png\n\
         Binary files a/img.png and b/img.png differ\n\
         diff --git a/run.sh b/run.sh\n\
         old mode 100644\n\
         new mode 100755\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![
            FileChanges {
                path: "img.png".to_string(),
                lines: Vec::new(),
            },
            FileChanges {
                path: "run.sh".to_string(),
                lines: Vec::new(),
            },
        ]
    );
}

#[test]
fn parses_porcelain_blame_with_boundaries() {
    let sha = "a".repeat(40);
    let blame = format!(
        "{sha} 1 1 1\n\
         author cover author\n\
         committer cover author\n\
         \tfirst\n\
         {sha} 2 2 1\n\
         author Old Timer\n\
         boundary\n\
         \tsecond\n"
    );
    assert_eq!(
        parse_blame_porcelain(&blame),
        vec![
            BlameLine {
                author: "cover author".to_string(),
                boundary: false,
            },
            BlameLine {
                author: "Old Timer".to_string(),
                boundary: true,
            },
        ]
    );
    assert!(parse_blame_porcelain("").is_empty());
}

#[test]
fn formats_readable_sequences() {
    assert_eq!(readable_sequence(&[]), "");
    assert_eq!(readable_sequence(&[5]), "5");
    assert_eq!(readable_sequence(&[1, 2, 3, 5, 7, 8]), "1-3, 5, 7-8");
    assert_eq!(readable_sequence(&[3, 1, 2, 2]), "1-3");
}

#[test]
fn authors_render_in_first_seen_order() {
    let mut covers: Covers = Vec::new();
    let blame = vec![
        BlameLine {
            author: "zebra".to_string(),
            boundary: false,
        },
        BlameLine {
            author: "apple".to_string(),
            boundary: false,
        },
        BlameLine {
            author: "zebra".to_string(),
            boundary: true,
        },
    ];
    attribute_lines(
        &blame,
        &FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1, 2, 3, 0, 9],
        },
        &mut covers,
    );
    // Line 3 is boundary, 0 is the new-file quirk, 9 is past the end:
    // all stay unattributed. Authors keep first-seen (not alpha) order.
    // The noun pluralizes through the en_US table (count dropped).
    assert_eq!(
        render_covers_ansi(&covers, false),
        "zebra\n  f.txt: line 1\napple\n  f.txt: line 2\n"
    );
    assert_eq!(
        render_covers_ansi(&covers, true),
        "\x1b[1mzebra\x1b[m\n  f.txt: line 1\n\x1b[1mapple\x1b[m\n  f.txt: line 2\n"
    );
}

#[test]
fn line_noun_pluralizes_without_count() {
    let mut covers: Covers = Vec::new();
    let blame = vec![BlameLine {
        author: "ann".to_string(),
        boundary: false,
    }];
    attribute_lines(
        &blame,
        &FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1, 1],
        },
        &mut covers,
    );
    assert_eq!(
        render_covers_ansi(&covers, false),
        "ann\n  f.txt: lines 1\n"
    );
}

#[test]
fn messages_match_php_bytes() {
    assert_eq!(
        no_changes_message(),
        "You're covered, you didn't change anything.\n"
    );
    assert_eq!(
        untouched_message(),
        "You're covered, your changes didn't touch anyone else's code.\n"
    );
}

#[test]
fn blames_files_at_base_through_git() {
    let tmp = init_repo("blame");
    commit_file(tmp.path(), "f.txt", "one\ntwo\n");
    commit_file(tmp.path(), "f.txt", "one\ntwo\nthree\n");
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();
    // Blame runs at the base commit, so worktree edits never appear.
    // Only root-commit lines carry the boundary flag; lines from the
    // blamed (non-root) commit itself do not.
    let blame = blame_file_at(tmp.path(), &head, "f.txt").unwrap();
    assert_eq!(blame.len(), 3);
    assert_eq!(blame[0].author, "cover author");
    assert!(blame[0].boundary);
    assert!(!blame[2].boundary);
    // The two-argument form blames at HEAD.
    assert_eq!(blame_file(tmp.path(), "f.txt").unwrap(), blame);
    let root = git(tmp.path(), &["rev-list", "--max-parents=0", "HEAD"]);
    let root_blame = blame_file_at(tmp.path(), &root, "f.txt").unwrap();
    assert_eq!(root_blame.len(), 2);
    assert!(root_blame.iter().all(|line| line.boundary));
    assert!(blame_file_at(tmp.path(), &head, "no-such-file.txt").is_err());
}

#[test]
fn clean_tree_reports_no_changes() {
    let tmp = init_repo("clean");
    commit_file(tmp.path(), "f.txt", "one\n");
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[]), false),
        Ok("You're covered, you didn't change anything.\n".to_string())
    );
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn worktree_edits_attribute_base_authors() {
    let tmp = init_repo("worktree");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\n", "zelda");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\nfour\n", "ann");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\nfour\nfive\n", "bob");
    std::fs::write(tmp.path().join("f.txt"), "one\ntwo\nthree\nFOUR\nfive\n").unwrap();
    // COVER-C1: the default chain prompts without a configured base, so
    // pin HEAD via the scratch file to keep the historical base.
    write_scratch(tmp.path(), "HEAD");
    // `-` covers old 4 (ann); `+` covers old 4 and 5 (ann, bob).
    // COVER-C5: deduped, so each author owns one line (singular noun).
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("ann\n  f.txt: line 4\nbob\n  f.txt: line 5\n".to_string())
    );
    assert_eq!(
        cover(tmp.path(), &["f.txt"]),
        Ok("ann\n  f.txt: line 4\nbob\n  f.txt: line 5\n".to_string())
    );
}

#[test]
fn boundary_lines_stay_unattributed() {
    let tmp = init_repo("boundary");
    // Single-commit repo: every base line predates the repository, so all
    // blame entries are boundary and nothing attributes -- like PHP.
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\n", "ann");
    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
}

#[test]
fn ranges_cover_committed_lines_since_rev() {
    let tmp = init_repo("rev");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\n", "zed");
    commit_as(tmp.path(), "f.txt", "one\ntwo!\nthree\n", "yara");
    commit_as(tmp.path(), "f.txt", "one\nTWO!\nthree\n", "bob");
    // Base is the merge-base of HEAD~1 and HEAD; blame runs there, so old
    // 2 attributes to yara while root-commit old 3 stays boundary.
    // COVER-C5: deduped to one line (singular noun).
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["--rev", "HEAD~1"]), false),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["--rev", "no-such-rev"]), false),
        Err("Unable to find any git commit named 'no-such-rev' in this repository.".to_string())
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--rev", "no-such-rev"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn added_and_untracked_files_are_omitted() {
    let tmp = init_repo("added");
    commit_file(tmp.path(), "f.txt", "one\n");
    std::fs::write(tmp.path().join("new.txt"), "hello\n").unwrap();
    git(tmp.path(), &["add", "new.txt"]);
    std::fs::write(tmp.path().join("scratch.txt"), "notes\n").unwrap();
    // Staged-new and untracked files carry the omit mask: as if unchanged.
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[]), false),
        Ok("You're covered, you didn't change anything.\n".to_string())
    );
    // Explicitly passed, they are unattributed but never fatal.
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["new.txt"]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["scratch.txt"]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
}

#[test]
fn deleted_files_count_but_do_not_attribute() {
    let tmp = init_repo("deleted");
    commit_file(tmp.path(), "f.txt", "one\n");
    std::fs::remove_file(tmp.path().join("f.txt")).unwrap();
    // In the diff, so not "no changes"; gone from the worktree, so the
    // file_exists skip leaves nothing attributed.
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
    // Explicitly naming a deleted file is a usage error, like PHP.
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["f.txt"]), false),
        Err("Path 'f.txt' does not exist!".to_string())
    );
}

#[test]
fn explicit_directories_contribute_nothing() {
    let tmp = init_repo("dir");
    commit_as(tmp.path(), "f.txt", "one\ntwo\n", "ann");
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    std::fs::write(tmp.path().join("sub").join("g.txt"), "x\n").unwrap();
    // An unrelated worktree edit must not leak into a directory query:
    // explicit directories contribute nothing, like PHP's `is_dir` skip.
    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\n").unwrap();
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["sub"]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["no-such-file.txt"]), false),
        Err("Path 'no-such-file.txt' does not exist!".to_string())
    );
}

#[test]
fn rejects_bad_contexts_and_flags() {
    let plain = TempDir::new("plain");
    assert_eq!(
        execute_cover_text_ansi(plain.path(), &options(&[]), false),
        Err("This command must be run in a Git, Mercurial or Subversion working copy.".to_string())
    );
    assert_eq!(
        run_in(plain.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
    let hg = TempDir::new("hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(
        run_in(hg.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
    let tmp = init_repo("flags");
    commit_file(tmp.path(), "f.txt", "one\n");
    assert_eq!(
        run_in(tmp.path(), &args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn cover_c2_blame_attributes_correct_line() {
    // Keep (Rule 2a): PHP looks the 1-based line up in a 0-indexed
    // list, attributing the next line's author; Rust attributes line
    // N to blame entry N-1. Experiment: three single-author lines,
    // change the middle one — the middle author must own it, not the
    // third line's author.
    let mut covers: Covers = Vec::new();
    let blame = vec![
        BlameLine {
            author: "first".to_string(),
            boundary: false,
        },
        BlameLine {
            author: "second".to_string(),
            boundary: false,
        },
        BlameLine {
            author: "third".to_string(),
            boundary: false,
        },
    ];
    attribute_lines(
        &blame,
        &FileChanges {
            path: "f.txt".to_string(),
            lines: vec![2],
        },
        &mut covers,
    );
    assert_eq!(
        render_covers_ansi(&covers, false),
        "second\n  f.txt: line 2\n"
    );
}

#[test]
fn cover_c3_boundary_lines_keep_indexes_stable() {
    // Keep (Rule 2a): PHP drops boundary lines, shifting later
    // indexes; Rust preserves them flagged, so line N still maps to
    // entry N-1 and boundary lines stay unattributed.
    let mut covers: Covers = Vec::new();
    let blame = vec![
        BlameLine {
            author: "ancient".to_string(),
            boundary: true,
        },
        BlameLine {
            author: "recent".to_string(),
            boundary: false,
        },
    ];
    attribute_lines(
        &blame,
        &FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1, 2],
        },
        &mut covers,
    );
    assert_eq!(
        render_covers_ansi(&covers, false),
        "recent\n  f.txt: line 2\n"
    );
}

#[test]
fn cover_c4_blame_failure_stays_unattributed() {
    // Keep (Rule 2a): PHP fatals when a passed file cannot be blamed
    // at the base; Rust leaves it unattributed with exit 0.
    let tmp = init_repo("c4");
    commit_file(tmp.path(), "f.txt", "one\n");
    std::fs::write(tmp.path().join("new.txt"), "hello\n").unwrap();
    git(tmp.path(), &["add", "new.txt"]);
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["new.txt"]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
}

#[test]
fn cover_c5_first_line_change_uses_singular_noun() {
    // Deleting the only second line covers just old 2: a single
    // deduped line renders with the singular noun.
    let diff = "diff --git a/f.txt b/f.txt\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -1,2 +1 @@\n\
         \x20one\n\
         -two\n";
    assert_eq!(
        parse_diff_unified0(diff),
        vec![FileChanges {
            path: "f.txt".to_string(),
            lines: vec![2],
        }]
    );
}

#[test]
fn cover_c6_files_keep_cli_order() {
    // Files under one author render in first-seen (CLI/diff) order,
    // not alphabetical.
    let mut covers: Covers = Vec::new();
    let blame = vec![BlameLine {
        author: "ann".to_string(),
        boundary: false,
    }];
    for path in ["zebra.txt", "apple.txt"] {
        attribute_lines(
            &blame,
            &FileChanges {
                path: path.to_string(),
                lines: vec![1],
            },
            &mut covers,
        );
    }
    assert_eq!(
        render_covers_ansi(&covers, false),
        "ann\n  zebra.txt: line 1\n  apple.txt: line 1\n"
    );
}

#[test]
fn cover_c7_bad_path_beats_bad_rev() {
    let tmp = init_repo("c7");
    commit_file(tmp.path(), "f.txt", "one\n");
    assert_eq!(
        execute_cover_text_ansi(
            tmp.path(),
            &options(&["--rev", "no-such-rev", "no-such-file.txt"]),
            false
        ),
        Err("Path 'no-such-file.txt' does not exist!".to_string())
    );
}

#[test]
fn cover_c8_empty_rev_means_default_base() {
    let tmp = init_repo("c8");
    commit_file(tmp.path(), "f.txt", "one\n");
    // Empty `--rev=` behaves like no `--rev`: a clean tree reports no
    // changes instead of a revision error.
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["--rev="]), false),
        Ok("You're covered, you didn't change anything.\n".to_string())
    );
}

#[test]
fn cover_c9_empty_tree_rev_is_accepted() {
    let tmp = init_repo("c9");
    commit_file(tmp.path(), "f.txt", "one\n");
    // The magic empty-tree commit resolves as a base instead of
    // erroring like an unknown rev; a clean tree then reports no
    // changes (nothing exists at the empty base to blame).
    assert_eq!(
        execute_cover_text_ansi(
            tmp.path(),
            &options(&["--rev", "4b825dc642cb6eb9a060e54bf8d69288fbee4904"]),
            false
        ),
        Ok("You're covered, you didn't change anything.\n".to_string())
    );
}

#[test]
fn cover_c10_single_commit_repo_reports_untouched() {
    // Keep (Rule 2a): PHP fatals on one-commit repos; Rust attributes
    // against HEAD where every line is boundary, hence untouched.
    let tmp = init_repo("c10");
    commit_as(tmp.path(), "f.txt", "one\ntwo\n", "ann");
    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\n").unwrap();
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
}

#[test]
fn cover_c11_absolute_paths_select_files() {
    let tmp = init_repo("c11");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\n", "zelda");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\nfour\n", "ann");
    std::fs::write(tmp.path().join("f.txt"), "one\ntwo\nthree\nFOUR\n").unwrap();
    // COVER-C1: pin HEAD via the scratch file so the default chain stays
    // prompt-free on this two-commit repo.
    write_scratch(tmp.path(), "HEAD");
    let absolute = tmp.path().join("f.txt").to_string_lossy().into_owned();
    let by_relative = cover(tmp.path(), &["f.txt"]);
    let by_absolute = cover(tmp.path(), &[&absolute]);
    assert_eq!(by_absolute, by_relative);
    assert!(by_absolute.unwrap().contains("ann"));
}

#[test]
fn cover_c12_author_zero_is_attributed() {
    // Keep (Rule 2a): PHP's falsy check drops author "0"; Rust
    // attributes every non-empty author.
    let mut covers: Covers = Vec::new();
    let blame = vec![BlameLine {
        author: "0".to_string(),
        boundary: false,
    }];
    attribute_lines(
        &blame,
        &FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1],
        },
        &mut covers,
    );
    assert_eq!(render_covers_ansi(&covers, false), "0\n  f.txt: line 1\n");
}

#[test]
fn cover_c14_arg_errors_use_php_bytes() {
    assert_eq!(
        parse_args(&args(&["-x"])),
        ParseOutcome::Error("Unknown argument 'x'. Try 'arc help'.".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus=value"])),
        ParseOutcome::Error("Unknown argument 'bogus'. Try 'arc help'.".to_string())
    );
}

#[test]
fn cover_c15_help_uses_php_wording() {
    let help = super::usage();
    assert!(help.contains("usage: arc cover [--rev revision] [path ...]"));
    assert!(help.contains("Cover your... professional reputation."));
    assert!(help.contains("Cover changes since a specific revision."));
}

#[test]
fn cover_c16_merge_base_has_no_dashdash() {
    // The argv shape is fixed by `resolve_rev_base`; a bad rev still
    // reports the PHP bytes through the bare `merge-base` call.
    let tmp = init_repo("c16");
    commit_file(tmp.path(), "f.txt", "one\n");
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["--rev", "nope"]), false),
        Err("Unable to find any git commit named 'nope' in this repository.".to_string())
    );
}

/// Three-commit repo for COVER-C1 base-selection tests: with the base at
/// the middle commit, the worktree edit attributes line 2 to yara.
fn c1_repo(tag: &str) -> TempDir {
    let tmp = init_repo(tag);
    commit_as(tmp.path(), "f.txt", "one\n", "zelda");
    commit_as(tmp.path(), "f.txt", "one\ntwo\n", "yara");
    commit_as(tmp.path(), "f.txt", "one\ntwo\nthree\n", "ann");
    std::fs::write(tmp.path().join("f.txt"), "one\ntwo\nTHREE\n").unwrap();
    tmp
}

#[test]
fn cover_c1_upstream_selects_merge_base() {
    let tmp = c1_repo("c1-up");
    let middle = git(tmp.path(), &["rev-parse", "HEAD~1"]);
    set_upstream(tmp.path(), &middle);
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_scratch_file_selects_base() {
    let tmp = c1_repo("c1-scratch");
    write_scratch(tmp.path(), "HEAD~1");
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_project_default_relative_wins() {
    let tmp = c1_repo("c1-proj");
    // The project setting beats upstream and scratch.
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    set_upstream(tmp.path(), &head);
    write_scratch(tmp.path(), "HEAD");
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"git.default-relative-commit": "HEAD~1"}"#,
    )
    .unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_prompt_answers_and_defaults() {
    let tmp = c1_repo("c1-prompt");
    // A typed answer resolves the base and persists the scratch file.
    assert_eq!(
        cover_opts(tmp.path(), &options(&[]), "HEAD~1\n", true),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    let git_dir = git(tmp.path(), &["rev-parse", "--git-dir"]);
    let scratch = tmp.path().join(git_dir).join("arc/default-relative-commit");
    assert_eq!(std::fs::read_to_string(&scratch).unwrap(), "HEAD~1");
    // Drop the scratch file so the next answers reach the prompt again.
    std::fs::remove_file(&scratch).unwrap();
    // An empty answer keeps origin/master, which does not exist here.
    let err = cover_opts(tmp.path(), &options(&[]), "\n", true).unwrap_err();
    assert!(err.starts_with("git cat-file failed: "), "{err}");
    // A non-TTY stdin echoes the prompt once, then fails.
    assert_eq!(
        cover_opts(tmp.path(), &options(&[]), "", false).unwrap_err(),
        arc_rs::prompt::NOT_INTERACTIVE.to_string()
    );
}

#[test]
fn cover_c1_prompt_banner_matches_php() {
    let banner = super::default_range_banner(false);
    assert!(banner.contains("Select a Default Commit Range"));
    assert!(banner.contains("arc no longer assumes 'HEAD^'"));
    assert!(!banner.contains("What default do you want"));
    assert!(banner.contains("origin/master"));
}

#[test]
fn cover_c1_zero_commit_repo_reports_no_changes() {
    let tmp = init_repo("c1-zero");
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("You're covered, you didn't change anything.\n".to_string())
    );
}

#[test]
fn cover_c1_base_rules_resolve_and_fall_through() {
    let tmp = c1_repo("c1-base");
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "git:no-such-ref, literal:HEAD~1"}"#,
    )
    .unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    // `arc:this` is HEAD^; `git:` plain names verify with cat-file.
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": "arc:this"}"#).unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "git:merge-base(HEAD~1)"}"#,
    )
    .unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_base_rule_errors_use_php_bytes() {
    let tmp = c1_repo("c1-err");
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "git:nope, git:also-nope"}"#,
    )
    .unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Err(
            "None of the rules in your 'base' configuration matched a valid commit. Adjust \
             rules or specify which commit you want to use explicitly."
                .to_string()
        )
    );
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": "bogus"}"#).unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Err(
            "Rule 'bogus' is invalid, it must have a type and name like 'arc:upstream'."
                .to_string()
        )
    );
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": "frobnicate:x"}"#).unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Err(
            "Base commit rule 'frobnicate:x' (from source 'project') is not a recognized rule."
                .to_string()
        )
    );
    // A mistyped `base` errors like `willReadValue()`.
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": 42}"#).unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Err("Type of setting 'base' must be string.".to_string())
    );
}

#[test]
fn cover_c1_relative_commit_must_be_a_commit() {
    let tmp = c1_repo("c1-cat");
    // An annotated tag passes the scratch read but fails the commit check.
    git(tmp.path(), &["tag", "-a", "-m", "v1", "v1", "HEAD~1"]);
    write_scratch(tmp.path(), "v1");
    assert_eq!(
        cover(tmp.path(), &[]),
        Err("Relative commit 'v1' is not the name of a commit!".to_string())
    );
    // An unknown name fails the git call itself, like PHP's throw.
    write_scratch(tmp.path(), "no-such-ref");
    let err = cover(tmp.path(), &[]).unwrap_err();
    assert!(err.starts_with("git cat-file failed: "), "{err}");
}

#[test]
fn cover_c1_amended_and_empty_and_exec_rules() {
    let tmp = init_repo("c1-amend");
    commit_as(tmp.path(), "f.txt", "one\n", "zelda");
    commit_as(tmp.path(), "f.txt", "one\ntwo\n", "yara");
    commit_msg(
        tmp.path(),
        "f.txt",
        "one\ntwo\nthree\n",
        "do the thing\n\nDifferential Revision: D123\n",
    );
    std::fs::write(tmp.path().join("f.txt"), "one\ntwo\nTHREE\n").unwrap();
    // HEAD carries a revision ID, so `arc:amended` resolves HEAD^.
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": "arc:amended"}"#).unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    // `arc:empty` blames nothing: no file exists at the empty tree.
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": "arc:empty"}"#).unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("You're covered, you didn't change anything.\n".to_string())
    );
    // `arc:exec` runs in the project root; success resolves stdout.
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "arc:exec(git rev-parse HEAD~1)"}"#,
    )
    .unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_branch_unique_rule() {
    let tmp = init_repo("c1-bu");
    commit_as(tmp.path(), "f.txt", "zero\n", "zelda");
    commit_as(tmp.path(), "f.txt", "zero\none\n", "ann");
    git(tmp.path(), &["branch", "-m", "main"]);
    git(tmp.path(), &["checkout", "-b", "feat"]);
    commit_as(tmp.path(), "f.txt", "zero\none\ntwo\n", "bob");
    std::fs::write(tmp.path().join("f.txt"), "zero\none\nTWO\n").unwrap();
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "git:branch-unique(main)"}"#,
    )
    .unwrap();
    // The merge-base sits on two branches while HEAD sits on one, so it wins.
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("ann\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_arc_prompt_skip_and_halt_rules() {
    let tmp = c1_repo("c1-arc");
    // `arc:prompt` answers resolve; empty answers fall to the next rule.
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"base": "arc:prompt"}"#).unwrap();
    assert_eq!(
        cover_opts(tmp.path(), &options(&[]), "HEAD~1\n", true),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "arc:prompt, literal:HEAD~1"}"#,
    )
    .unwrap();
    assert_eq!(
        cover_opts(tmp.path(), &options(&[]), "\n", true),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    // `arc:skip` moves on; `arc:halt` stops everything.
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "arc:skip, literal:HEAD~1"}"#,
    )
    .unwrap();
    assert_eq!(
        cover(tmp.path(), &[]),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "arc:halt, literal:HEAD~1"}"#,
    )
    .unwrap();
    assert!(
        cover(tmp.path(), &[])
            .unwrap_err()
            .starts_with("None of the rules in your 'base' configuration")
    );
}

#[test]
fn cover_c1_runtime_source_switches_to_project() {
    let tmp = c1_repo("c1-switch");
    std::fs::write(
        tmp.path().join(".arcconfig"),
        r#"{"base": "literal:HEAD~1"}"#,
    )
    .unwrap();
    // The runtime rule jumps to the project source, which resolves.
    let mut runtime = serde_json::Map::new();
    runtime.insert(
        "base".to_string(),
        serde_json::Value::String("arc:project".to_string()),
    );
    let opts = arc_rs::get_config::LoadOptions {
        runtime,
        ..arc_rs::get_config::LoadOptions::default()
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    assert_eq!(
        execute_cover_text_ansi_with_input_and_config(
            tmp.path(),
            &options(&[]),
            false,
            &mut cursor,
            false,
            None,
            &opts,
        ),
        Ok("yara\n  f.txt: line 2\n".to_string())
    );
}

#[test]
fn cover_c1_tokenizer_drops_falsy_rules() {
    assert_eq!(super::tokenize_base_spec("").unwrap(), Vec::<String>::new());
    assert_eq!(
        super::tokenize_base_spec("0").unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        super::tokenize_base_spec("arc:skip,, 0 , literal:x").unwrap(),
        vec!["arc:skip".to_string(), "literal:x".to_string()]
    );
    assert!(
        super::tokenize_base_spec("bogus")
            .unwrap_err()
            .contains("Rule 'bogus' is invalid")
    );
}

#[test]
fn cover_c11_outside_paths_contribute_nothing() {
    // Paths outside the repo can never match diff output: like PHP's
    // hunk-map miss they stay unattributed, never a git failure.
    let tmp = init_repo("c11-out");
    commit_file(tmp.path(), "f.txt", "one\n");
    let outside = TempDir::new("c11-elsewhere");
    std::fs::write(outside.path().join("o.txt"), "x\n").unwrap();
    let absolute = outside.path().join("o.txt").to_string_lossy().into_owned();
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[&absolute]), false),
        Ok("You're covered, your changes didn't touch anyone else's code.\n".to_string())
    );
}

#[test]
fn cover_c13_svn_and_hg_stay_unported() {
    // Rule 3 (T1128): svn/hg cover is unported; the refusals stay put.
    let hg = TempDir::new("c13-hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(
        execute_cover_text_ansi(hg.path(), &options(&[]), false),
        Err("Mercurial working copies are not yet ported (see T1128)".to_string())
    );
    let svn = TempDir::new("c13-svn");
    std::fs::create_dir(svn.path().join(".svn")).unwrap();
    assert_eq!(
        execute_cover_text_ansi(svn.path(), &options(&[]), false),
        Err("Subversion working copies are not yet ported (see T1128)".to_string())
    );
}
