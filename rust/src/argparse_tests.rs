use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn core16_command_distance_uses_ceb_costs() {
    // Transpose costs 2, cheaper than two replacements (6).
    assert_eq!(command_distance("ab", "ba"), 2);
    assert_eq!(command_distance("same", "same"), 0);
    assert_eq!(command_distance("", "abc"), 12);
    assert_eq!(command_distance("abc", ""), 12);
    // One replacement costs 3.
    assert_eq!(command_distance("land", "lend"), 3);
}

#[test]
fn core16_flag_distance_allows_single_insdel_only() {
    assert_eq!(flag_distance("nolint", "no-lint"), 1);
    // A replacement prefers delete+insert (2) over replace (10);
    // both exceed the max distance of 1, so neither corrects.
    assert_eq!(flag_distance("abc", "adc"), 2);
}

#[test]
fn flag_distance_routes_through_match_and_delete_cells() {
    // Hand-computed on the stripped 3x3 matrix: the interior `b` match
    // takes the diagonal (replacement wins), and the trailing mismatch
    // after it deletes (delete wins over insert).
    assert_eq!(flag_distance("axbyc", "azbwc"), 4);
}

#[test]
fn core16_command_corrector_matches_php() {
    // Unique prefixes are accepted as shorthand.
    assert_eq!(
        correct_commands("ver", &["version", "weld"]),
        vec!["version".to_string()]
    );
    // Dash-led inputs never correct.
    assert!(correct_commands("--help", &["help"]).is_empty());
    // Far options are dropped (best capped at 6).
    assert!(correct_commands("zzzzzz", &["land"]).is_empty());
}

#[test]
fn rbd1_flag_corrector_is_strict() {
    assert_eq!(
        correct_flags("nolint", &["no-lint"]),
        vec!["no-lint".to_string()]
    );
    // A single replacement costs 10, over the max of 1.
    assert!(correct_flags("xb", &["ab"]).is_empty());
}

#[test]
fn correctors_with_no_options_match_nothing() {
    assert!(correct_commands("ver", &[]).is_empty());
    assert!(correct_flags("nolint", &[]).is_empty());
}

#[test]
fn flag_corrector_keeps_tied_suggestions() {
    // Two single-deletion candidates tie at the best distance, so both
    // survive (distance order, then name order).
    assert_eq!(
        correct_flags("ac", &["abc", "adc"]),
        vec!["abc".to_string(), "adc".to_string()]
    );
}

#[test]
fn rbd1_valued_flags_split_equals_and_reject_dupes() {
    let specs = &[FlagSpec::valued("uri", "uri")];
    let parsed = parse_toolset(&args(&["--uri=https://x/"]), specs, false, false).unwrap();
    assert_eq!(parsed.value("uri"), Some("https://x/"));
    let error =
        parse_toolset(&args(&["--uri", "a", "--uri", "b"]), specs, false, false).unwrap_err();
    assert_eq!(error, "Argument \"--uri\" was provided twice.");
    let error = parse_toolset(&args(&["--uri"]), specs, false, false).unwrap_err();
    assert_eq!(error, "Argument \"--uri\" requires a parameter.");
}

#[test]
fn rbd1_bool_with_value_rejected() {
    let specs = &[FlagSpec::boolean("json")];
    let error = parse_toolset(&args(&["--json=x"]), specs, false, false).unwrap_err();
    assert_eq!(error, "Argument \"--json\" does not take a parameter.");
}

#[test]
fn rbd1_unknown_flags_use_toolset_text() {
    // T1153 RB-D1: PHP's verbatim parse-error texts.
    let specs = &[FlagSpec::boolean("json")];
    let error = parse_toolset(&args(&["--bogus"]), specs, false, false).unwrap_err();
    assert_eq!(
        error,
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    let error = parse_toolset(&args(&["-x"]), specs, false, false).unwrap_err();
    assert_eq!(
        error,
        "Argument \"-x\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    let error = parse_toolset(&args(&["positional"]), specs, false, false).unwrap_err();
    assert_eq!(error, "Unrecognized argument \"positional\".");
}

#[test]
fn rbd6_lone_dash_is_positional() {
    let parsed = parse_toolset(&args(&["-"]), &[], true, false).unwrap();
    assert_eq!(parsed.positionals, vec!["-".to_string()]);
}

#[test]
fn core17_terminator_breaks_scan_and_strips_once() {
    let specs = &[FlagSpec::boolean("json")];
    let parsed = parse_toolset(&args(&["--", "--json"]), specs, true, false).unwrap();
    assert!(parsed.saw_terminator);
    assert!(!parsed.flag("json"));
    assert_eq!(parsed.positionals, vec!["--json".to_string()]);
    // A valued flag eats the next element even when it is `--`.
    let specs = &[FlagSpec::valued("current", "pos")];
    let parsed = parse_toolset(&args(&["--current", "--", "x"]), specs, true, false).unwrap();
    assert!(!parsed.saw_terminator);
    assert_eq!(parsed.value("current"), Some("--"));
    assert_eq!(parsed.positionals, vec!["x".to_string()]);
}

#[test]
fn rbd1_correction_applies_with_notice_when_interactive() {
    let specs = &[FlagSpec::valued("conduit-uri", "uri")];
    let parsed = parse_toolset(&args(&["--conduit-ur", "u"]), specs, false, true).unwrap();
    assert_eq!(parsed.value("conduit-uri"), Some("u"));
    assert_eq!(parsed.notices.len(), 1);
    assert!(parsed.notices[0].contains("British spelling"));
    let error = parse_toolset(&args(&["--conduit-ur", "u"]), specs, false, false).unwrap_err();
    assert!(error.contains("is unrecognized"));
}
