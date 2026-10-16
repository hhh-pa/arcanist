use super::*;

const TEST_RSA_PRIVATE_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAk4vwzs7Ymnq9esaKniWhO4/4GKw7dNPLQ53ZaBbQdhd81ayy\nAIC99YRn94My8A4uEPa91BD46gZ2SZmE/rXlLrODHR7n2MevP3SRnL414esRGsLW\n7qd18YBg/idBa0ZPvT527ACEGMEGMN1twza3SB7oLaBmI7w/Ch9YqsSV0jDN2T1n\nmCQsuKwOZJheRMevWsok5QHqysMOwusXB9F0LIAWecDt5IcDhbhuhzF7irSgoV7P\n7qLiVN+uiDy1B77/D4jZuMknR/FIam1LsEMeBEfVW/wnjnAVU1zA16cySFq+YHfW\nEjZ1eoX7VdjkCNjyNLNTwJ4hc98a9etlrGiyJQIDAQABAoIBAHXs9z6TnBN73k1u\nugPEV6VfYA94ON0bGe6lDspe0aZRB3lQWcSFIU5F0ULGHcQ30SHF09rW5dtF5uzU\nQrPBdbdpynVP6kYc8zhoVqlMbYxvcHBPIbIFLgMjdGItxd+Ths/eSPY/Q1T1kZkk\noUHRTZPa//7XoEsXfdr9SsTR8lzXZkMkL3JRDjjUHXMUaOpXKvANu9GJeNuEh5Vs\nKC2Gh4csqzJW1g4Ur7Jfl4ApxvX+0vv61lHo481xeSl3ce5YI8G1aFXqGLttqFFI\neQx7Z1UfMMrX1Eipu/YzdK2ghU6hVOmcwwKUiCc1iE0OA1L+8T3+mQTGbSFd4+B/\nt0O9LVECgYEAwui2PPv2HPuTLW7uVnHGIsnS4SIjey1nLNW+HZ/slw/gxYy5LZFO\nTf8ie5ry/SOvxrAwR1uop1K7l6oJnxmfKlJnyvDc3AFx2lg+yI8MH1BaQhBmKYHZ\n4g5suUhnDvFA3f0hoDGi/QR8u3zmf5W4SYr4BQOVJtFwbmWU6ZqpQM8CgYEAwcrs\nJW/R3Risx1qRwAt25e47pjc9jUkxF9qV3mZbHPi1g0BDCvXcCTi0R87G3/6OupIz\nn+ma3CSetdRm/g0ypZiV5RDUoFPboFt8/epHVtRFIMPA2Vo6LY+oj+5kT0qim4pA\nY+ZdbCpuIH8+IAWPt9Qsnvjv8kmzwXhVRhOQUssCgYEAgeQKBsw3MQtrAt0pkoxR\n6b8sfQJ+OMmCN7vd7eo1Blr7hUuJ4Nc0IwCDX3Jcutw8eSHTmsp3s8KL57/Wl6PA\nkbjHGNS5qxh/Ym5qsMe74WZwMBSzsLerl13bkQKyQ5/9hbL65MIPh6Sk52GOTrpd\nTQ2CyfygOnVbAwdJXQDICLsCgYEAs4u9VEYQiybkVWMdLG+yvf8F9s6xUWhherdD\nKSVt/x6SlUMBmnMFZOnLZcg3S6p+NG9mMx9tIc8v2EktDu3NYc1H+02noc9v6Vc6\nK8pWRCUw0tVqpDXLNxElXAzDlQ8SLsddNvFk1uVM2QDq3yIn8Go82SDMto8AYxyI\n7P8NNRcCgYB/wfEkLThqICjJHMlPCOB30I1PHkviJH0CvBmJE6E3Gy5HFamR5UNj\ncypPFpQWzecTFG0ElxCMriNKEZ6eTLj+Av6/btzm5BC5d26WX/iJTxxtmlATO+7S\nNswSK/f39vtjEYLDfvQHvV9M1UOrU8HgkEJcBCBqqMhFAXVbOseH8w==\n-----END RSA PRIVATE KEY-----\n";

fn test_client() -> Client {
    Client::new("https://conduit.test/", "api-test").unwrap()
}

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
fn rejects_uri_without_host() {
    // `ConduitClient::__construct()` message, byte for byte.
    assert_eq!(
        Client::new("not-a-uri", "t").unwrap_err(),
        Error::Transport("Conduit URI 'not-a-uri' must include a valid host.".to_string())
    );
    assert_eq!(
        Client::new("https://", "t").unwrap_err(),
        Error::Transport("Conduit URI 'https://' must include a valid host.".to_string())
    );
    assert_eq!(
        Client::new("https:///api/m", "t").unwrap_err(),
        Error::Transport("Conduit URI 'https:///api/m' must include a valid host.".to_string())
    );
}

#[test]
fn splits_uri_parts() {
    assert_eq!(
        split_uri("https://orsay.hhh.pa/api/").unwrap(),
        ("https".to_string(), "orsay.hhh.pa".to_string(), None)
    );
    assert_eq!(
        split_uri("http://127.0.0.1:9/x").unwrap(),
        ("http".to_string(), "127.0.0.1".to_string(), Some(9))
    );
    assert_eq!(
        split_uri("https://user@example.test:8443/").unwrap(),
        ("https".to_string(), "example.test".to_string(), Some(8443))
    );
    assert_eq!(
        split_uri("http://[::1]:8080/api").unwrap(),
        ("http".to_string(), "::1".to_string(), Some(8080))
    );
    // Port 0 behaves like no port (PHP `!$port` branches).
    assert_eq!(
        split_uri("http://example.test:0/").unwrap(),
        ("http".to_string(), "example.test".to_string(), None)
    );
}

#[test]
fn host_strings_follow_php_port_rules() {
    let client = test_client();
    assert_eq!(client.host_for_header(), "conduit.test");
    assert_eq!(client.host_for_signature(), "conduit.test:443");

    let plain = Client::new("http://example.test/", "t").unwrap();
    assert_eq!(plain.host_for_header(), "example.test");
    assert_eq!(plain.host_for_signature(), "example.test:80");

    let custom = Client::new("https://example.test:8443/", "t").unwrap();
    assert_eq!(custom.host_for_header(), "example.test:8443");
    assert_eq!(custom.host_for_signature(), "example.test:8443");

    let implicit = Client::new("https://example.test:443/", "t").unwrap();
    assert_eq!(implicit.host_for_header(), "example.test");

    let mut overridden = test_client();
    overridden.set_host("node-7.internal");
    assert_eq!(overridden.host_for_header(), "node-7.internal");
    assert_eq!(overridden.host_for_signature(), "node-7.internal:443");
}

#[test]
fn builds_meta_with_auth() {
    let client = test_client();
    let params = serde_json::json!({"constraints": {"query": "x"}});
    let meta = client.build_meta("m", params.as_object().unwrap()).unwrap();
    assert_eq!(meta["token"], "api-test");
    assert!(!meta.contains_key("sessionKey"));
    assert!(!meta.contains_key("auth.type"));

    let mut anonymous = Client::new("https://conduit.test/", "").unwrap();
    anonymous.set_oauth_token("oauth-1");
    let meta = anonymous
        .build_meta("m", params.as_object().unwrap())
        .unwrap();
    // PHP only sends `token` when it is truthy.
    assert!(!meta.contains_key("token"));
    assert_eq!(meta["access_token"], "oauth-1");
}

#[test]
fn stores_session_from_connect_result() {
    let client = test_client();
    client.store_session(&serde_json::json!({
        "sessionKey": "sess-1",
        "connectionID": 7,
    }));
    let params = serde_json::Map::new();
    let meta = client.build_meta("differential.query", &params).unwrap();
    assert_eq!(meta["sessionKey"], "sess-1");
    // Non-string values are dropped, like PHP's falsy `idx()` results.
    assert!(!meta.contains_key("connectionID"));
}

#[test]
fn rejects_non_object_params() {
    let client = test_client();
    let err = client.call("m", &serde_json::json!([1])).unwrap_err();
    assert_eq!(
        err,
        Error::Protocol("params must be a JSON object".to_string())
    );
}

#[test]
fn parses_success_response() {
    let v = parse_response(r#"{"result": {"a": 1}, "error_code": null}"#, "m").unwrap();
    assert_eq!(v, serde_json::json!({"a": 1}));
    // A missing `result` decodes to null, like PHP's null `idx()`.
    let v = parse_response(r#"{"error_code": null}"#, "m").unwrap();
    assert_eq!(v, serde_json::Value::Null);
}

#[test]
fn parses_api_error_with_method_prefix() {
    // T1153 CORE-1: at ceb `ConduitClientException` is `"{code}: {info}"`
    // with no `<method>` infix (the infix was a newer-baseline artifact);
    // `info` is the raw `error_info`.
    let err = parse_response(
        r#"{"result": null, "error_code": "E", "error_info": "no"}"#,
        "user.whoami",
    )
    .unwrap_err();
    assert_eq!(
        err,
        Error::Api {
            code: "E".to_string(),
            info: "no".to_string()
        }
    );
}

#[test]
fn falsy_error_codes_are_success() {
    for body in [
        r#"{"result": 1, "error_code": false}"#,
        r#"{"result": 1, "error_code": 0}"#,
        r#"{"result": 1, "error_code": ""}"#,
        r#"{"result": 1, "error_code": "0"}"#,
        r#"{"result": 1, "error_code": []}"#,
        r#"{"result": 1}"#,
    ] {
        assert_eq!(
            parse_response(body, "m").unwrap(),
            serde_json::json!(1),
            "{body}"
        );
    }
}

#[test]
fn rejects_malformed_response() {
    assert!(matches!(
        parse_response("not json", "m"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        parse_response("[1]", "m"),
        Err(Error::Protocol(_))
    ));
    // PHP `ConduitFuture` message, byte for byte.
    assert_eq!(
        parse_response("not json", "m").unwrap_err(),
        Error::Protocol(
            "Host returned HTTP/200, but invalid JSON data in response to a Conduit method call."
                .to_string()
        )
    );
}

#[test]
fn strips_response_shield() {
    let v = parse_response(r#"for(;;);{"result": 7, "error_code": null}"#, "m").unwrap();
    assert_eq!(v, serde_json::json!(7));
}

#[test]
fn request_timeout_matches_php_default() {
    // `ConduitClient::$timeout` is 300 seconds in PHP.
    assert_eq!(REQUEST_TIMEOUT, std::time::Duration::from_secs(300));
    assert_eq!(test_client().timeout, std::time::Duration::from_secs(300));
}

#[test]
fn rawurlencode_matches_php() {
    assert_eq!(rawurlencode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    assert_eq!(rawurlencode("a b"), "a%20b");
    assert_eq!(rawurlencode("{}"), "%7B%7D");
    assert_eq!(rawurlencode("ü"), "%C3%BC");
    assert_eq!(rawurlencode("a+b=c&d"), "a%2Bb%3Dc%26d");
}

#[test]
fn build_query_matches_php_form_fields() {
    // PHP posts `params`, `output=json`, `__conduit__=1` (bool true encodes
    // as `1` in the querystring).
    assert_eq!(
        build_query(r#"{"a":1}"#),
        "params=%7B%22a%22%3A1%7D&output=json&__conduit__=1"
    );
}

#[test]
fn connect_signature_matches_sha1_vector() {
    // `sha1("1700000000CERT")`, verified with an independent SHA1.
    assert_eq!(
        connect_signature("1700000000", "CERT"),
        "cf26184c8ab0511d44d2ef42efcfb8ba7b2dad87"
    );
}

#[test]
fn connect_auth_swaps_certificate() {
    let mut map = serde_json::json!({
        "certificate": "CERT",
        "user": "alice",
    })
    .as_object()
    .cloned()
    .unwrap();
    apply_connect_auth(&mut map);
    assert!(!map.contains_key("certificate"));
    assert_eq!(map["user"], "alice");
    let token = map["authToken"].as_u64().unwrap();
    assert!(token > 1_700_000_000);
    assert_eq!(
        map["authSignature"],
        serde_json::Value::String(connect_signature(&token.to_string(), "CERT"))
    );

    // Falsy certificates are dropped without signing, like PHP.
    let mut map = serde_json::json!({"certificate": ""})
        .as_object()
        .cloned()
        .unwrap();
    apply_connect_auth(&mut map);
    assert!(!map.contains_key("certificate"));
    assert!(!map.contains_key("authToken"));
}

#[test]
fn php_truthiness_matches() {
    assert!(!php_truthy(&serde_json::json!(null)));
    assert!(!php_truthy(&serde_json::json!(false)));
    assert!(php_truthy(&serde_json::json!(true)));
    assert!(!php_truthy(&serde_json::json!(0)));
    assert!(php_truthy(&serde_json::json!(7)));
    assert!(!php_truthy(&serde_json::json!("")));
    assert!(!php_truthy(&serde_json::json!("0")));
    assert!(php_truthy(&serde_json::json!("0.0")));
    assert!(!php_truthy(&serde_json::json!([])));
    assert!(php_truthy(&serde_json::json!([null])));
    assert_eq!(php_stringify(&serde_json::json!(null)), "");
    assert_eq!(php_stringify(&serde_json::json!(true)), "1");
    assert_eq!(php_stringify(&serde_json::json!(12)), "12");
}

#[test]
fn signature_encoding_matches_php_vector() {
    // Oracle: `ConduitClientTestCase::testConduitRequestEncoding`.
    // Note: PHP's empty array takes the list branch, so `empty` is `[]`.
    let input = serde_json::json!({
        "z": {"nothing": null, "emptystring": ""},
        "empty": [],
        "list": [15, "quack", true, false],
        "a": {"key": "value", "key2": "value2"},
    });
    assert_eq!(
        encode_raw_data_for_signature(&input).unwrap(),
        "O4:S1:aO2:S3:keyS5:valueS4:key2S6:value2S5:emptyA0:S4:listA4:I2:15\
         S5:quackB1:B0:S1:zO2:S11:emptystringS0:S7:nothingN:"
    );
}

#[test]
fn signature_encoding_rejects_floats() {
    assert_eq!(
        encode_raw_data_for_signature(&serde_json::json!({"x": 1.5})).unwrap_err(),
        Error::Protocol("Unexpected data type in request data: double.".to_string())
    );
}

#[test]
fn gzip_round_trips() {
    let data = b"params=%7B%7D&output=json&__conduit__=1".as_slice();
    let gzipped = gzip_bytes(data);
    assert_ne!(gzipped, data);
    assert_eq!(&gzipped[..2], &[0x1f, 0x8b]);
    let mut decoder = flate2::read::GzDecoder::new(gzipped.as_slice());
    let mut back = Vec::new();
    use std::io::Read;
    decoder.read_to_end(&mut back).unwrap();
    assert_eq!(back, data);
}

#[test]
fn capabilities_enable_gzip() {
    let client = test_client();
    assert!(!client.uses_gzip());
    client.enable_capabilities(&["gzip".to_string(), "other".to_string()]);
    assert!(client.uses_gzip());
}

#[test]
fn signs_requests_with_consign_prefix() {
    let params = serde_json::json!({"user": "alice"})
        .as_object()
        .cloned()
        .unwrap();
    let mut meta = serde_json::Map::new();
    meta.insert("auth.type".to_string(), json_string("asymmetric"));
    meta.insert("auth.key".to_string(), json_string("test-key"));
    meta.insert("auth.host".to_string(), json_string("conduit.test:443"));
    let first = sign_request("user.whoami", &params, &meta, TEST_RSA_PRIVATE_PEM).unwrap();
    let second = sign_request("user.whoami", &params, &meta, TEST_RSA_PRIVATE_PEM).unwrap();
    // PKCS#1 v1.5 signatures are deterministic.
    assert_eq!(first, second);
    assert!(first.starts_with(SIGNATURE_CONSIGN_1), "{first}");
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&first[SIGNATURE_CONSIGN_1.len()..])
        .unwrap();
    assert_eq!(raw.len(), 256);

    // The signature verifies as RSA/SHA1 PKCS#1 v1.5 (the `openssl_sign()`
    // default), pinning the algorithm independently of the signer.
    use rsa::pkcs1::DecodeRsaPrivateKey;
    let private = rsa::RsaPrivateKey::from_pkcs1_pem(TEST_RSA_PRIVATE_PEM).unwrap();
    let public = rsa::RsaPublicKey::from(&private);
    let structure = serde_json::json!({
        "method": "user.whoami",
        "protocol": meta,
        "parameters": params,
    });
    let input = encode_raw_data_for_signature(&structure).unwrap();
    use sha1::Digest;
    let digest = sha1::Sha1::digest(input.as_bytes());
    public
        .verify(rsa::Pkcs1v15Sign::new::<sha1::Sha1>(), &digest, &raw)
        .unwrap();

    assert_eq!(
        sign_request("user.whoami", &params, &meta, "not a key").unwrap_err(),
        Error::Protocol("Unable to sign Conduit request with signing key.".to_string())
    );
}

#[test]
fn signing_meta_flows_into_requests() {
    let mut client = test_client();
    client.set_signing_keys("test-key", TEST_RSA_PRIVATE_PEM);
    let params = serde_json::json!({"user": "alice"});
    let meta = client
        .build_meta("user.whoami", params.as_object().unwrap())
        .unwrap();
    assert_eq!(meta["auth.type"], "asymmetric");
    assert_eq!(meta["auth.key"], "test-key");
    assert_eq!(meta["auth.host"], "conduit.test:443");
    assert!(
        meta["auth.signature"]
            .as_str()
            .unwrap()
            .starts_with(SIGNATURE_CONSIGN_1)
    );
}

#[test]
fn signs_requests_with_known_vector() {
    // Fixed vector: RSA/SHA1 PKCS#1 v1.5 over the consigned encoding above,
    // independently verified (DigestInfo `30213009...` + SHA1 of the input
    // recovered via textbook RSA verification).
    let params = serde_json::json!({"user": "alice"})
        .as_object()
        .cloned()
        .unwrap();
    let mut meta = serde_json::Map::new();
    meta.insert("auth.type".to_string(), json_string("asymmetric"));
    meta.insert("auth.key".to_string(), json_string("test-key"));
    meta.insert("auth.host".to_string(), json_string("conduit.test:443"));
    assert_eq!(
        sign_request("user.whoami", &params, &meta, TEST_RSA_PRIVATE_PEM).unwrap(),
        "Consign1.0/eZ9bsSgjO3lvQcn9/4GLXfFcNWxK9kukYJLnj6mlhdJLDZpGcYzgvIUIwNnRvWEHiYq9pRCI05CaiMsFBAKMAkOLqN\
         CPkOVhWm9lM7/vFB9/6u9ToVeU/tyM62oYWs7S2gAkvnS+8NYIEO2sYv77m5nmVE99qXD/SLiCMAZsYT3NLN+QgGBojwNrcfg\
         of9Lip9ivQppVi+Srhwam3Zjmg2VUDWb/JUfQt4Fb9CwUWpVgiy+4JJaN7ZU4UnDhP6d7udHk/9BdOcKNj3M5rYgP75ILXDTL\
         EkNIkhFwvCIsCTgj+/AokXD+Ozpue+wARFT2/pR1lsKb4LXyv8uG0f/fbg=="
    );
}

#[test]
fn r3_search_pages_follows_after_until_null() {
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "differential.revision.search");
        seen.borrow_mut().push(params.clone());
        if seen.borrow().len() == 1 {
            Ok(serde_json::json!({"data": [{"id": 1}], "cursor": {"after": "c1"}}))
        } else {
            Ok(serde_json::json!({"data": [{"id": 2}]}))
        }
    };
    let base = serde_json::json!({"constraints": {"ids": [1, 2]}});
    let pages = search_pages(&call, "differential.revision.search", &base).unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0]["data"][0]["id"], serde_json::json!(1));
    assert_eq!(pages[1]["data"][0]["id"], serde_json::json!(2));
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].get("after").is_none());
    assert_eq!(seen[1]["after"], serde_json::json!("c1"));
    assert_eq!(seen[1]["constraints"], serde_json::json!({"ids": [1, 2]}));
}

#[test]
fn r3_search_pages_single_page_without_cursor_calls_once() {
    let calls = std::cell::Cell::new(0);
    let call = |_method: &str, _params: &serde_json::Value| {
        calls.set(calls.get() + 1);
        Ok(serde_json::json!({"data": [], "cursor": {"after": null}}))
    };
    let pages = search_pages(&call, "file.search", &serde_json::json!({})).unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(calls.get(), 1);
}

#[test]
fn r4_core6_conduit_errors_render_bare() {
    // No `transport error:` / `protocol error:` / `conduit error`
    // infix: PHP prints the raw message under the envelope.
    assert_eq!(Error::Transport("boom".to_string()).to_string(), "boom");
    assert_eq!(
        Error::Protocol("bad frame".to_string()).to_string(),
        "bad frame"
    );
    // Exactly `ConduitClientException` (`"{code}: {info}"`).
    assert_eq!(
        Error::Api {
            code: "ERR-CONDUIT-CORE".to_string(),
            info: "nope".to_string(),
        }
        .to_string(),
        "ERR-CONDUIT-CORE: nope"
    );
}

#[test]
fn r4_core6_envelope_exit_codes() {
    use std::process::ExitCode;
    assert_eq!(EnvelopeKind::NoEffect.exit_code(), ExitCode::SUCCESS);
    assert_eq!(EnvelopeKind::Fatal.exit_code(), ExitCode::from(EXIT_FATAL));
    assert_eq!(EXIT_FATAL, 255);
    for kind in [
        EnvelopeKind::Usage,
        EnvelopeKind::Exception,
        EnvelopeKind::ToolsetUsage,
        EnvelopeKind::ToolsetConduit,
        EnvelopeKind::UserAbort,
    ] {
        assert_eq!(kind.exit_code(), ExitCode::FAILURE, "{kind:?}");
    }
    assert!(EnvelopeKind::NoEffect.uses_stdout());
    assert!(!EnvelopeKind::Usage.uses_stdout());
    assert!(!EnvelopeKind::Fatal.uses_stdout());
}

#[test]
fn r4_core6_subprocess_arms_render_plain() {
    assert_eq!(
        EnvelopeKind::NoEffect.render("No linters to run.", false),
        "No linters to run.\n"
    );
    assert_eq!(
        EnvelopeKind::Usage.render("oops", false),
        "Usage Exception: oops\n"
    );
    // Usage messages are rstripped like PHP (`:436`).
    assert_eq!(
        EnvelopeKind::Usage.render("oops  \n", false),
        "Usage Exception: oops\n"
    );
    assert_eq!(
        EnvelopeKind::Exception.render("boom", false),
        " Exception \nboom\n(Run with `--trace` for a full exception trace.)\n"
    );
}

#[test]
fn r4_core6_toolset_arms_render_plain() {
    assert_eq!(
        EnvelopeKind::ToolsetUsage.render("oops", false),
        " USAGE EXCEPTION  oops\n"
    );
    assert_eq!(
        EnvelopeKind::ToolsetConduit.render("oops", false),
        " CONDUIT  oops\n"
    );
    assert_eq!(
        EnvelopeKind::UserAbort.render("User aborted the workflow.", false),
        " ---  User aborted the workflow.\n"
    );
    // Fatal: the raw message only (the PHP trace is unportable).
    assert_eq!(EnvelopeKind::Fatal.render("boom", false), "boom\n");
}

#[test]
fn r4_core6_usage_bold_on_tty() {
    assert_eq!(
        EnvelopeKind::Usage.render("oops", true),
        "\x1b[1mUsage Exception:\x1b[m oops\n"
    );
    let banner = EnvelopeKind::Exception.render("boom", true);
    assert!(banner.contains("\x1b[41m"), "{banner:?}");
    assert!(banner.contains("Exception"), "{banner:?}");
}

#[test]
fn core3_base_uri_strips_configured_path() {
    // PHP replaces the whole path (`setPath('/api/'.$method)`), so a
    // configured path never leaks into the request path.
    assert_eq!(
        normalize_base("http://h.test:8080/foo"),
        "http://h.test:8080/api/"
    );
    assert_eq!(
        normalize_base("https://h.test/foo/bar/"),
        "https://h.test/api/"
    );
    assert_eq!(test_client().base_uri(), "https://conduit.test/api/");
}

#[test]
fn core5_auth_guidance_login_blocks() {
    // Tokenless `ERR-INVALID-SESSION` maps to the login block with the
    // server domain and the install command (path `/`).
    let guidance =
        auth_guidance("ERR-INVALID-SESSION", false, "https://phab.example/api/").unwrap();
    assert_eq!(guidance.title, "LOGIN REQUIRED");
    assert_eq!(guidance.summary, "Conduit API login required.");
    assert!(guidance.body.contains("phab.example"), "{}", guidance.body);
    assert!(
        guidance
            .body
            .contains("arc install-certificate https://phab.example/"),
        "{}",
        guidance.body
    );
    assert!(
        guidance.body.contains("run this command:"),
        "{}",
        guidance.body
    );
}

#[test]
fn core5_auth_guidance_invalid_credentials() {
    // `ERR-INVALID-AUTH` maps with or without a token.
    for has_token in [false, true] {
        let guidance = auth_guidance(
            "ERR-INVALID-AUTH",
            has_token,
            "http://h.test:8080/api/conduit.ping",
        )
        .unwrap();
        assert_eq!(guidance.title, "INVALID CREDENTIALS");
        assert_eq!(guidance.summary, "Invalid Conduit API credentials.");
        // The prose domain excludes the port; the install URI keeps it.
        assert!(guidance.body.contains("\"h.test\""), "{}", guidance.body);
        assert!(
            guidance
                .body
                .contains("arc install-certificate http://h.test:8080/"),
            "{}",
            guidance.body
        );
    }
}

#[test]
fn core5_auth_guidance_passthrough() {
    // A tokened `ERR-INVALID-SESSION` rethrows the original (`break`),
    // and unknown codes never map.
    assert_eq!(
        auth_guidance("ERR-INVALID-SESSION", true, "https://h/"),
        None
    );
    assert_eq!(auth_guidance("ERR-CONDUIT-CORE", false, "https://h/"), None);
    assert_eq!(auth_guidance("ERR-OTHER", true, "https://h/"), None);
    assert_eq!(
        auth_error_code("ERR-INVALID-AUTH: bad"),
        Some("ERR-INVALID-AUTH")
    );
    assert_eq!(
        auth_error_code("ERR-INVALID-SESSION: gone"),
        Some("ERR-INVALID-SESSION")
    );
    assert_eq!(auth_error_code("query failed: ERR-INVALID-AUTH: bad"), None);
    assert_eq!(auth_error_code("boom"), None);
}

#[test]
fn core5_no_uri_block_shape() {
    // `raiseURIException()` block: wrapped paragraph, blank line,
    // three `      - ` items, trailing blank line.
    let block = no_uri_block();
    assert!(
        block.starts_with("This command needs to communicate with Phabricator,"),
        "{block:?}"
    );
    for item in [
        "Run in a working copy with \"phabricator.uri\" set in \".arcconfig\".",
        "Set a default URI with `arc set-config phabricator.uri <uri>`.",
        "Specify a URI explicitly with `--config phabricator.uri=<uri>`.",
    ] {
        assert!(block.contains(&format!("      - {item}")), "{block:?}");
    }
    assert!(block.ends_with("\n\n"), "{block:?}");
    assert_eq!(
        toolset_setup_message("no conduit URI: pass --conduit-uri"),
        block
    );
    assert_eq!(
        toolset_setup_message("Custom arcrc file was specified, but it was not found!"),
        "Custom arcrc file was specified, but it was not found!"
    );
}

#[test]
fn core8_tls_trusts_listed_domains() {
    let tls = TlsConfig {
        cabundle: None,
        blind_trust: vec!["phab.example".to_string()],
    };
    assert!(tls.trusts_domain("https://phab.example/api/"));
    assert!(tls.trusts_domain("https://PHAB.EXAMPLE:8080/foo"));
    assert!(!tls.trusts_domain("https://other.example/"));
    assert!(!TlsConfig::default().trusts_domain("https://phab.example/"));
}

#[test]
fn core8_tls_client_build_paths() {
    // Blind trust builds (no I/O); a missing bundle fails transport-class.
    let tls = TlsConfig {
        cabundle: None,
        blind_trust: vec!["phab.example".to_string()],
    };
    let client = Client::new_with_tls("https://phab.example/", "t", &tls).unwrap();
    assert_eq!(client.base_uri(), "https://phab.example/api/");
    let tls = TlsConfig {
        cabundle: Some(std::path::PathBuf::from("/nonexistent-ca-bundle.pem")),
        blind_trust: Vec::new(),
    };
    let err = Client::new_with_tls("https://phab.example/", "t", &tls).unwrap_err();
    assert!(
        err.to_string().contains("failed to read CA bundle"),
        "{err}"
    );
}

#[test]
fn rbd8_auth_identity_cache_roundtrips() {
    // PHP caches the auth identity on the workflow; `user_phid`
    // helpers consult this before calling `user.whoami`.
    let client = test_client();
    assert_eq!(client.authenticated_user(), None);
    client.set_authenticated_user("alice", "PHID-USER-1");
    assert_eq!(
        client.authenticated_user(),
        Some(("alice".to_string(), "PHID-USER-1".to_string()))
    );
}

#[test]
fn land22_cached_user_phid_reuses_handshake() {
    // T1153 LAND-22: a cached handshake identity answers without any
    // `user.whoami` call (no network here: the test client points at
    // an unroutable host, so a call would fail loudly).
    let client = test_client();
    assert_eq!(client.authenticated_user(), None);
    client.set_authenticated_user("alice", "PHID-USER-1");
    assert_eq!(client.cached_user_phid().unwrap(), "PHID-USER-1");
}

#[test]
fn unitu21_wire_escapes_non_ascii_like_php() {
    // T1153 UNIT-U21: PHP `json_encode()` emits lowercase `\uXXXX`
    // (surrogate pairs past U+FFFF); ASCII — including serde's own
    // escapes and the caller's `\/` — passes through untouched.
    assert_eq!(escape_non_ascii_json(r#"{"a":1}"#), r#"{"a":1}"#);
    assert_eq!(escape_non_ascii_json("a\\/b\"c\\\\d"), "a\\/b\"c\\\\d");
    assert_eq!(escape_non_ascii_json("é"), "\\u00e9");
    assert_eq!(escape_non_ascii_json("ü/中"), "\\u00fc/\\u4e2d");
    assert_eq!(escape_non_ascii_json("😀"), "\\ud83d\\ude00");
    assert_eq!(escape_non_ascii_json("a\u{2028}b"), "a\\u2028b");
}

#[test]
fn unitu21_call_body_carries_escaped_params() {
    // T1153 UNIT-U21: end to end through `encode_call_body` (offline:
    // no gzip capability, no post): the form body carries the
    // `\uXXXX` escapes rawurlencoded, never raw UTF-8.
    let client = test_client();
    let mut map = serde_json::Map::new();
    map.insert(
        "name".to_string(),
        serde_json::Value::String("héllo.wörld/a".to_string()),
    );
    let (body, gzipped) = client.encode_call_body("conduit.ping", &mut map).unwrap();
    assert!(!gzipped);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("%5Cu00e9"), "{body}");
    assert!(body.contains("%5Cu00f6"), "{body}");
    assert!(body.contains("%5C%2F"), "{body}");
    assert!(!body.contains("é"), "{body}");
    assert!(!body.contains("ö"), "{body}");
}

#[test]
fn core16_toolset_parse_error_is_bold() {
    // Bold `**Usage Exception:**` rendering like `Usage` (the toolset
    // `parse()` catch), exit 1; `report()` rethrows only
    // `Usage`/`Exception` under `--trace`, never this arm.
    let rendered = EnvelopeKind::ToolsetParseError.render("boom", false);
    assert_eq!(rendered, "Usage Exception: boom\n");
    assert_eq!(
        EnvelopeKind::ToolsetParseError.exit_code(),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        ClassifiedError::toolset_parse_error("boom".to_string()).report(),
        std::process::ExitCode::FAILURE
    );
}
