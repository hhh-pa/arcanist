use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn classifies_help_and_version() {
    assert_eq!(classify(&args(&["--help"])), Action::Help);
    assert_eq!(classify(&args(&["-h"])), Action::Help);
    assert_eq!(classify(&args(&["help"])), Action::Help);
    assert_eq!(classify(&args(&["--version"])), Action::Version);
    assert_eq!(classify(&args(&["-V"])), Action::Version);
}

#[test]
fn classifies_planned_commands() {
    for c in PLANNED {
        assert_eq!(classify(&args(&[*c])), Action::Planned((*c).to_string()));
    }
}

#[test]
fn classifies_call_conduit() {
    assert_eq!(
        classify(&args(&["call-conduit", "user.whoami"])),
        Action::CallConduit(CallArgs {
            method: "user.whoami".to_string(),
            params_json: "{}".to_string(),
            conduit_uri: None,
        })
    );
    assert_eq!(
        classify(&args(&[
            "call-conduit",
            "user.whoami",
            "{}",
            "--conduit-uri",
            "https://orsay.hhh.pa/"
        ])),
        Action::CallConduit(CallArgs {
            method: "user.whoami".to_string(),
            params_json: "{}".to_string(),
            conduit_uri: Some("https://orsay.hhh.pa/".to_string()),
        })
    );
}

#[test]
fn rejects_bad_call_conduit_invocations() {
    assert!(matches!(
        classify(&args(&["call-conduit"])),
        Action::Error(_)
    ));
    assert!(matches!(
        classify(&args(&["call-conduit", "m", "--bogus"])),
        Action::Error(_)
    ));
    assert!(matches!(
        classify(&args(&["call-conduit", "m", "{}", "--conduit-uri"])),
        Action::Error(_)
    ));
    assert!(matches!(
        classify(&args(&["call-conduit", "m", "{}", "{}"])),
        Action::Error(_)
    ));
}

#[test]
fn classifies_missing_and_unknown() {
    assert_eq!(classify(&args(&[])), Action::Missing);
    assert_eq!(
        classify(&args(&["bogus"])),
        Action::Unknown("bogus".to_string())
    );
}

#[test]
fn usage_lists_all_planned_commands() {
    let u = usage();
    for c in PLANNED {
        assert!(u.contains(c), "usage missing {c}");
    }
    assert!(u.contains("call-conduit"));
}
