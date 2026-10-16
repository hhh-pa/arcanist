use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn parses_files_and_seed() {
    assert_eq!(
        parse_options(&args(&["a.txt", "b.txt"])),
        Ok(Some(Options {
            files: vec!["a.txt".to_string(), "b.txt".to_string()],
            seed: None,
        }))
    );
    let options = parse_options(&args(&["--seed", "42", "--", "a.txt", "b.txt"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.seed, Some(42));
    assert_eq!(options.files.len(), 2);
    let options = parse_options(&args(&["--seed=7", "a.txt", "b.txt"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.seed, Some(7));
}

#[test]
fn separator_keeps_dashed_files() {
    let options = parse_options(&args(&["--", "--weird", "-x"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.files, vec!["--weird".to_string(), "-x".to_string()]);
}

#[test]
fn help_returns_none() {
    assert_eq!(parse_options(&args(&["--help"])), Ok(None));
    assert_eq!(parse_options(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    let err = parse_options(&args(&["--bogus", "a", "b"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    assert!(parse_options(&args(&["--seed"])).is_err());
    let err = parse_options(&args(&["--seed=abc", "a", "b"])).unwrap_err();
    assert!(err.contains("invalid --seed"), "unexpected: {err}");
}

#[test]
fn splits_lines_keeping_terminators() {
    assert_eq!(split_lines_keep(""), Vec::<&str>::new());
    assert_eq!(split_lines_keep("a\nb\n"), vec!["a\n", "b\n"]);
    assert_eq!(split_lines_keep("a\nb"), vec!["a\n", "b"]);
    assert_eq!(split_lines_keep("a\r\nb\r\n"), vec!["a\r\n", "b\r\n"]);
    assert_eq!(split_lines_keep("\n"), vec!["\n"]);
}

#[test]
fn splits_terminators() {
    assert_eq!(split_terminator("abc\n"), ("abc", Some("\n")));
    assert_eq!(split_terminator("abc\r\n"), ("abc", Some("\r\n")));
    assert_eq!(split_terminator("abc"), ("abc", None));
    assert_eq!(split_terminator("\n"), ("", Some("\n")));
    assert_eq!(split_terminator(""), ("", None));
}

#[test]
fn pick_follows_draw_against_bias() {
    assert_eq!(pick('a', 'b', 0.5, 0.9), 'a');
    assert_eq!(pick('a', 'b', 0.5, 0.1), 'b');
    // A draw exactly on the threshold takes the later glyph (strict `>`).
    assert_eq!(pick('a', 'b', 0.5, 0.5), 'b');
}

#[test]
fn pick_defers_to_the_non_space_side() {
    // Earlier space always loses, whatever the draw (threshold 1).
    assert_eq!(pick(' ', 'b', 0.0, 0.999), 'b');
    assert_eq!(pick(' ', 'b', 0.0, 1.0), 'b');
    // Later space always wins for any positive draw (threshold 0, strict
    // `>`, mirroring the PHP comparison).
    assert_eq!(pick('a', ' ', 1.0, 0.5), 'a');
    assert_eq!(pick('a', ' ', 1.0, 1.0), 'a');
    // Both spaces: PHP checks the later side last (threshold 0).
    assert!(!keep_earlier(true, true, 0.5, 0.0));
    assert!(keep_earlier(true, true, 0.5, 0.5));
}

#[test]
fn overlap_stays_within_bounds() {
    let mut rng = Rng::new(1);
    for _ in 0..50 {
        let overlap = overlap_for(100, 100, &mut rng);
        assert!((16..=32).contains(&overlap), "overlap={overlap}");
    }
    let mut rng = Rng::new(2);
    assert_eq!(overlap_for(0, 10, &mut rng), 0);
    assert_eq!(overlap_for(10, 0, &mut rng), 0);
    let mut rng = Rng::new(3);
    assert!(overlap_for(4, 4, &mut rng) <= 4);
    let mut rng = Rng::new(4);
    assert!(overlap_for(100, 5, &mut rng) <= 5);
}

#[test]
fn overlap_is_deterministic_for_a_seed() {
    let mut first = Rng::new(99);
    let mut second = Rng::new(99);
    for _ in 0..10 {
        assert_eq!(
            overlap_for(100, 100, &mut first),
            overlap_for(100, 100, &mut second)
        );
    }
}

#[test]
fn single_input_passes_through() {
    let mut rng = Rng::new(0);
    assert_eq!(weld_texts(&["a\nb\nc\n"], &mut rng), "a\nb\nc\n");
    assert_eq!(weld_texts(&["no-newline"], &mut rng), "no-newline");
}

#[test]
fn empty_inputs_are_skipped() {
    let mut rng = Rng::new(5);
    let with_empty = weld_texts(&["", "a\nb\n", ""], &mut rng);
    let mut rng = Rng::new(5);
    assert_eq!(with_empty, weld_texts(&["a\nb\n"], &mut rng));
}

#[test]
fn welded_output_is_deterministic_for_a_seed() {
    // T1153 CORE-47: the stream is splitmix64, not `mt_rand` (kept:
    // unportable), but `--seed` still pins the joint byte-identical.
    let inputs = ["alpha\nbeta\ngamma\n", "one\ntwo\nthree\n"];
    let mut first = Rng::new(1234);
    let mut second = Rng::new(1234);
    assert_eq!(
        weld_texts(&inputs, &mut first),
        weld_texts(&inputs, &mut second)
    );
}

#[test]
fn space_sides_are_deterministic() {
    // An all-space earlier line always yields the later content.
    let mut rng = Rng::new(0);
    assert_eq!(weld_lines("   \n", "abc\n", 0.0, &mut rng), "abc\n");
    // An all-space later line always yields the earlier content.
    let mut rng = Rng::new(0);
    assert_eq!(weld_lines("abc\n", "   \n", 1.0, &mut rng), "abc\n");
}

#[test]
fn terminator_prefers_the_later_line() {
    // Later line without a terminator: the earlier line's survives.
    let mut rng = Rng::new(0);
    assert!(weld_lines("abc\n", "xy", 0.5, &mut rng).ends_with('\n'));
    // Later line with a terminator: the later one wins.
    let mut rng = Rng::new(0);
    assert!(weld_lines("abc", "xy\r\n", 0.5, &mut rng).ends_with("\r\n"));
    // Neither line has one: no terminator is added.
    let mut rng = Rng::new(0);
    assert!(!weld_lines("abc", "xy", 0.5, &mut rng).contains('\n'));
}

#[test]
fn usage_covers_files_and_seed() {
    let text = usage();
    assert!(text.contains("usage: arc weld"));
    assert!(text.contains("<file>"));
    assert!(text.contains("--seed"));
}
#[test]
fn core48_combining_marks_fuse_as_one_glyph() {
    // T1153 CORE-48: `phutil_utf8v_combined` keeps `e` + U+0301
    // together, so the joint never splits them across sides.
    assert_eq!(split_glyphs(""), Vec::<String>::new());
    assert_eq!(split_glyphs("abc"), vec!["a", "b", "c"]);
    assert_eq!(split_glyphs("é"), vec!["é"]);
    assert_eq!(split_glyphs("áb"), vec!["á", "b"]);
    // A leading combining mark attaches to a synthetic space.
    assert_eq!(split_glyphs("́x"), vec![" ́", "x"]);
    // Single-byte chars are never combining, even beside marks.
    assert_eq!(split_glyphs("0́"), vec!["0́"]);
    for seed in 0..25 {
        let mut rng = Rng::new(seed);
        let out = weld_lines("é\n", "xy\n", 0.5, &mut rng);
        let body = out.strip_suffix('\n').unwrap();
        // One glyph against two: position 0 draws, position 1 always
        // takes the later side (earlier pads with a space), and the
        // mark never leaves its base.
        assert!(body == "éy" || body == "xy", "seed={seed} {out:?}");
    }
}

#[test]
fn core48_mixed_validity_pairs_split_per_side() {
    // T1153 CORE-48: `weldLines` tests each side for UTF-8
    // independently — the valid side fuses glyphs, the invalid side
    // bytes (`ArcanistWeldWorkflow.php:90-103` at ceb). A two-byte
    // glyph against one bad byte fuses whole-or-byte, never split.
    for seed in 0..25 {
        let mut rng = Rng::new(seed);
        let out = weld_line_bytes(b"\xff\n", "é\n".as_bytes(), 0.5, &mut rng);
        let body = out.strip_suffix(b"\n").unwrap();
        assert!(
            body == b"\xff" || body == "é".as_bytes(),
            "seed={seed} {out:?}"
        );
        let mut rng = Rng::new(seed);
        let out = weld_line_bytes("é\n".as_bytes(), b"\xff\n", 0.5, &mut rng);
        let body = out.strip_suffix(b"\n").unwrap();
        assert!(
            body == b"\xff" || body == "é".as_bytes(),
            "seed={seed} {out:?}"
        );
    }
    // Both-invalid still fuses byte by byte.
    for seed in 0..10 {
        let mut rng = Rng::new(seed);
        let out = weld_line_bytes(b"\xff\n", b"\xfe\n", 0.5, &mut rng);
        assert!(out == b"\xff\n" || out == b"\xfe\n", "seed={seed} {out:?}");
    }
}

#[test]
fn weld_line_bytes_fuses_invalid_utf8_bytewise() {
    let earlier = b"\xff\xfe data\n";
    let later = b"xy\n";
    for seed in 0..25 {
        let mut rng = Rng::new(seed);
        let out = weld_line_bytes(earlier, later, 0.5, &mut rng);
        assert!(out.ends_with(b"\n"), "seed={seed} {out:?}");
        let body = &out[..out.len() - 1];
        assert_eq!(body.len(), earlier.len() - 1, "seed={seed} {out:?}");
        for (index, byte) in body.iter().enumerate() {
            let choices = [earlier[index], *later.get(index).unwrap_or(&b' ')];
            assert!(choices.contains(byte), "seed={seed} index={index} {out:?}");
        }
    }
}

#[test]
fn weld_buffers_skips_empty_and_is_deterministic() {
    let files = vec![b"alpha\n".to_vec(), vec![], b"beta\n".to_vec()];
    let no_empty = vec![b"alpha\n".to_vec(), b"beta\n".to_vec()];
    let mut first = Rng::new(11);
    let mut second = Rng::new(11);
    assert_eq!(
        weld_buffers(&files, &mut first),
        weld_buffers(&no_empty, &mut second)
    );
    let mut third = Rng::new(7);
    assert_eq!(
        weld_buffers(&[b"a\nb\n".to_vec()], &mut third),
        b"a\nb\n".to_vec()
    );
}

#[test]
fn r4_core49_weld_usage_has_no_dump() {
    // Fewer than two files is toolset usage (exit 1, matching the
    // old exit) with no usage dump; read failures are fatal.
    match execute(&std::env::temp_dir(), &args(&["only"])) {
        Err(message) => {
            assert_eq!(message, "Specify two or more files to weld together.");
        }
        _ => panic!("expected usage error"),
    }
    assert_eq!(
        run_in(&std::env::temp_dir(), &args(&["only"])),
        std::process::ExitCode::FAILURE
    );
}
