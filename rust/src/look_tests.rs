use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_empty_and_thing_words() {
    assert_eq!(parse_args(&args(&[])).unwrap(), Some(Vec::new()));
    assert_eq!(
        parse_args(&args(&["remotes"])).unwrap(),
        Some(args(&["remotes"]))
    );
    assert_eq!(
        parse_args(&args(&["--", "big", "rock"])).unwrap(),
        Some(args(&["big", "rock"]))
    );
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_unknown_flags() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
}

#[test]
fn computes_utc_hour() {
    assert_eq!(hour_of(0), 0);
    assert_eq!(hour_of(3600 * 13 + 59), 13);
    assert_eq!(hour_of(3600 * 24 + 3600 * 5), 5);
}

#[test]
fn names_every_hour_range() {
    assert!(hour_text(6, LunarPhase::Full).contains("early morning"));
    assert!(hour_text(9, LunarPhase::Full).contains("It is morning."));
    assert!(hour_text(12, LunarPhase::Full).contains("midday"));
    assert!(hour_text(15, LunarPhase::Full).contains("afternoon"));
    assert!(hour_text(18, LunarPhase::Full).contains("dusk"));
    assert!(hour_text(22, LunarPhase::Full).contains("evening"));
}

#[test]
fn night_hours_use_lunar_phase() {
    assert!(hour_text(2, LunarPhase::New).contains("pitch black"));
    assert!(hour_text(2, LunarPhase::Full).contains("full moon"));
    assert!(hour_text(2, LunarPhase::Waxing).contains("waxing"));
    assert!(hour_text(2, LunarPhase::Waning).contains("waning"));
    assert!(hour_text(20, LunarPhase::Full).contains("full moon"));
    assert!(hour_text(0, LunarPhase::Full).contains("full moon"));
}

#[test]
fn approximates_lunar_cycle() {
    let new_moon = REF_NEW_MOON as u64;
    let quarter = (SYNODIC_SECS / 4.0) as u64;
    let half = (SYNODIC_SECS / 2.0) as u64;
    assert_eq!(lunar_phase(new_moon), LunarPhase::New);
    assert_eq!(lunar_phase(new_moon + quarter), LunarPhase::Waxing);
    assert_eq!(lunar_phase(new_moon + half), LunarPhase::Full);
    assert_eq!(lunar_phase(new_moon + 3 * quarter), LunarPhase::Waning);
}

#[test]
fn renders_clearing_with_trails_hint() {
    // 2026-01-01T12:00:00Z is midday UTC.
    let out = render_clearing(1_767_268_800);
    assert!(out.contains("small clearing in the woods"), "{out}");
    assert!(out.contains("midday"), "{out}");
    assert!(out.contains("small trails and footpaths"), "{out}");
    assert!(out.contains("you can see remotes"), "{out}");
}

#[test]
fn renders_unknown_thing() {
    assert_eq!(
        render_unknown(&args(&["big", "rock"])),
        "You do not see \"big rock\" anywhere.\n"
    );
}

#[test]
fn renders_remote_block() {
    let remote = RemoteInfo {
        name: "origin".to_string(),
        push_uri: Some("git@example.test:repo.git".to_string()),
        fetch_uri: None,
    };
    let out = render_remote(&remote);
    assert!(out.contains("Remote \"origin\":"), "{out}");
    assert!(out.contains("Push URI: git@example.test:repo.git"), "{out}");
    assert!(out.contains("Fetch URI: -"), "{out}");
    assert!(out.contains("Push Repository: -"), "{out}");
    assert!(out.contains("Fetch Repository: -"), "{out}");
}

#[test]
fn renders_published_and_empty_river() {
    let out = render_published(&["abc123".to_string()]);
    assert!(out.contains("Floating on the water"), "{out}");
    assert!(out.contains("abc123"), "{out}");
    assert!(out.contains("They river bubbles peacefully."), "{out}");

    let out = render_published(&[]);
    assert!(out.contains("bubbles quietly"), "{out}");
    assert!(!out.contains("Floating"), "{out}");
}
