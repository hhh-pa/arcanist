use super::*;

#[test]
fn normalizes_base_uris() {
    assert_eq!(
        normalize_base("https://orsay.hhh.pa"),
        "https://orsay.hhh.pa/api/"
    );
    assert_eq!(
        normalize_base("https://orsay.hhh.pa/"),
        "https://orsay.hhh.pa/api/"
    );
    assert_eq!(
        normalize_base("https://orsay.hhh.pa/api"),
        "https://orsay.hhh.pa/api/"
    );
    assert_eq!(
        normalize_base("https://orsay.hhh.pa/api/"),
        "https://orsay.hhh.pa/api/"
    );
}

#[test]
fn builds_params_blob_with_auth() {
    let params = serde_json::json!({"constraints": {"query": "x"}});
    let blob = build_params(&params, "api-test").unwrap();
    let back: serde_json::Value = serde_json::from_str(&blob).unwrap();
    assert_eq!(back["constraints"]["query"], "x");
    assert_eq!(back["__conduit__"]["token"], "api-test");
}

#[test]
fn rejects_non_object_params() {
    let err = build_params(&serde_json::json!([1]), "t").unwrap_err();
    assert_eq!(
        err,
        Error::Protocol("params must be a JSON object".to_string())
    );
}

#[test]
fn parses_success_response() {
    let v = parse_response(r#"{"result": {"a": 1}, "error_code": null}"#).unwrap();
    assert_eq!(v, serde_json::json!({"a": 1}));
}

#[test]
fn parses_api_error() {
    let err =
        parse_response(r#"{"result": null, "error_code": "E", "error_info": "no"}"#).unwrap_err();
    assert_eq!(
        err,
        Error::Api {
            code: "E".to_string(),
            info: "no".to_string()
        }
    );
}

#[test]
fn rejects_malformed_response() {
    assert!(matches!(
        parse_response("not json"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(parse_response("[1]"), Err(Error::Protocol(_))));
}

#[test]
fn request_timeout_matches_php_default() {
    // `ConduitClient::$timeout` is 300 seconds in PHP.
    assert_eq!(REQUEST_TIMEOUT, std::time::Duration::from_secs(300));
}
