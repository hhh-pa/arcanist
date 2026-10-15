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
    assert_eq!(
        parse_args(&args(&["--rev"])),
        ParseOutcome::Error("--rev requires a value".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus"])),
        ParseOutcome::Error("unknown flag '--bogus'".to_string())
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
            lines: vec![2, 2, 3],
        }]
    );
}

#[test]
fn cover_map_marks_insertion_neighbors() {
    // Pure insertion after old line 5 covers old 4 and 5 per added line.
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
            lines: vec![4, 5, 4, 5],
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
            // added files and never attribute line 0.
            FileChanges {
                path: "new.txt".to_string(),
                lines: vec![0, 0],
            },
        ]
    );
}

#[test]
fn corpus_lines_beat_section_headers_by_count() {
    // An added line reading `+++ foo` must stay corpus: the hunk expects
    // exactly one new line, so the `+++` prefix never misroutes. The `+`
    // covers old 1 (predecessor, since old_line is 2) and old 2.
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
            lines: vec![1, 1, 2],
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
    // `-` covers old 4 (ann); `+` covers old 4 and 5 (ann, bob).
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&[]), false),
        Ok("ann\n  f.txt: lines 4\nbob\n  f.txt: line 5\n".to_string())
    );
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["f.txt"]), false),
        Ok("ann\n  f.txt: lines 4\nbob\n  f.txt: line 5\n".to_string())
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
    assert_eq!(
        execute_cover_text_ansi(tmp.path(), &options(&["--rev", "HEAD~1"]), false),
        Ok("yara\n  f.txt: lines 2\n".to_string())
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
