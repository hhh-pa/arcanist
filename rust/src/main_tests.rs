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
            conduit_uri: None,
        })
    );
    assert_eq!(
        classify(&args(&[
            "call-conduit",
            "--conduit-uri",
            "https://orsay.hhh.pa/",
            "--",
            "user.whoami"
        ])),
        Action::CallConduit(CallArgs {
            method: "user.whoami".to_string(),
            conduit_uri: Some("https://orsay.hhh.pa/".to_string()),
        })
    );
}

#[test]
fn rejects_bad_call_conduit_invocations() {
    for argv in [
        vec!["call-conduit"],
        vec!["call-conduit", "a", "b"],
        vec!["call-conduit", "m", "--bogus"],
        vec!["call-conduit", "m", "--conduit-uri"],
    ] {
        assert!(
            matches!(classify(&args(&argv)), Action::Error(_)),
            "{argv:?}"
        );
    }
    assert_eq!(
        classify(&args(&["call-conduit"])),
        Action::Error("Provide exactly one Conduit method name to call.".to_string())
    );
}

#[test]
fn reads_params_json_from_stdin_bytes() {
    assert_eq!(
        read_params_json(&b"{\"a\": 1}"[..]).unwrap(),
        serde_json::json!({"a": 1})
    );
    assert_eq!(
        read_params_json(&b"nope"[..]).unwrap_err(),
        "Provide method parameters on stdin as a JSON blob."
    );
    struct Failing;
    impl std::io::Read for Failing {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }
    assert!(
        read_params_json(Failing)
            .unwrap_err()
            .contains("failed to read stdin")
    );
}

#[test]
fn renders_call_conduit_envelope() {
    assert_eq!(
        render_envelope(None, serde_json::json!({"user": "x"})),
        "{\n  \"error\": null,\n  \"errorMessage\": null,\n  \"response\": {\n    \"user\": \"x\"\n  }\n}"
    );
    assert_eq!(
        render_envelope(Some(("E", "no")), serde_json::Value::Null),
        "{\n  \"error\": \"E\",\n  \"errorMessage\": \"no\",\n  \"response\": null\n}"
    );
}

#[test]
fn call_conduit_execute_rejects_bad_stdin_without_io() {
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: None,
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"nope"[..], &mut out),
        ExitCode::FAILURE
    );
    assert!(out.is_empty());
}

#[test]
fn call_conduit_execute_reports_missing_token_without_io() {
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some("https://call-conduit-test.invalid/".to_string()),
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"{}"[..], &mut out),
        ExitCode::FAILURE
    );
    assert!(out.is_empty());
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
