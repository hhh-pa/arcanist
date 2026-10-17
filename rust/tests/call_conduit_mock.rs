use std::io::{Read, Write};
use std::net::TcpListener;

use arc_rs::conduit::{Client, Error};

/// Serve one request with a canned Conduit response; returns the base URI
/// and the received (request line, body).
fn serve_once(canned: &'static str) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (request_line, _headers, body) = read_request(&mut stream);
        tx.send((request_line, String::from_utf8(body).unwrap()))
            .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            canned.len(),
            canned
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (base, rx)
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, Vec<String>, Vec<u8>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let len: usize = head
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length: ")
                .or_else(|| line.strip_prefix("content-length: "))
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).unwrap();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("").to_string();
    let headers: Vec<String> = lines.map(str::to_string).collect();
    (request_line, headers, body)
}

/// Scripted mock: serves each canned (status, extra headers, body) in order,
/// capturing (request line, headers, raw body bytes) per request.
type Canned = (u16, Vec<(&'static str, &'static str)>, &'static str);
type Seen = (String, Vec<String>, Vec<u8>);

fn serve_script(script: Vec<Canned>) -> (String, std::sync::mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for (status, headers, canned) in script {
            let (mut stream, _) = listener.accept().unwrap();
            let (request_line, req_headers, body) = read_request(&mut stream);
            tx.send((request_line, req_headers, body)).unwrap();
            let reason = if status == 200 { "OK" } else { "Error" };
            let mut response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n",
                canned.len()
            );
            for (name, value) in &headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("Connection: close\r\n\r\n");
            response.push_str(canned);
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (base, rx)
}

fn recv_seen(rx: &std::sync::mpsc::Receiver<Seen>) -> Seen {
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
}

fn header_value(headers: &[String], name: &str) -> Option<String> {
    headers.iter().find_map(|line| {
        line.split_once(':').and_then(|(key, value)| {
            if key.eq_ignore_ascii_case(name) {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
    })
}

#[test]
fn posts_params_blob_and_returns_result() {
    let (base, seen) = serve_once(r#"{"result": {"user": "automation"}, "error_code": null}"#);
    let client = Client::new(&base, "api-test-token").unwrap();
    let result = client.call("user.whoami", &serde_json::json!({})).unwrap();
    assert_eq!(result, serde_json::json!({"user": "automation"}));
    let (request_line, body) = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(
        request_line.starts_with("POST /api/user.whoami "),
        "{request_line}"
    );
    assert!(body.contains("__conduit__"), "{body}");
    assert!(body.contains("api-test-token"), "{body}");
    assert!(body.contains("output=json"), "{body}");
    // PHP encodes boolean `true` as `1` in the querystring.
    assert!(body.contains("__conduit__=1"), "{body}");
}

#[test]
fn maps_api_errors() {
    // T1153 CORE-1: no `<method>` infix at ceb; `info` is raw `error_info`.
    let (base, _rx) = serve_once(r#"{"result": null, "error_code": "E", "error_info": "no"}"#);
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        client.call("m", &serde_json::json!({})).unwrap_err(),
        Error::Api {
            code: "E".to_string(),
            info: "no".to_string()
        }
    );
}

#[test]
fn rejects_non_object_params_without_io() {
    let client = Client::new("http://127.0.0.1:9/", "t").unwrap();
    assert!(matches!(
        client.call("m", &serde_json::json!([1])).unwrap_err(),
        Error::Protocol(_)
    ));
}

#[test]
fn strips_shield_and_reports_bad_json() {
    let (base, _rx) = serve_once(r#"for(;;);{"result": 1, "error_code": null}"#);
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        client.call("m", &serde_json::json!({})).unwrap(),
        serde_json::json!(1)
    );

    let (base, _rx) = serve_once("not json");
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        client.call("m", &serde_json::json!({})).unwrap_err(),
        Error::Protocol(
            "Host returned HTTP/200, but invalid JSON data in response to a Conduit method call."
                .to_string()
        )
    );
}

#[test]
fn http_error_status_is_a_transport_error() {
    let (base, _rx) = serve_script(vec![(500, vec![], "proxy meltdown")]);
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        client.call("m", &serde_json::json!({})).unwrap_err(),
        Error::Transport(
            "Host returned HTTP/500 in response to a Conduit method call.".to_string()
        )
    );
}

#[test]
fn gzip_capability_compresses_later_requests() {
    let (base, rx) = serve_script(vec![
        (
            200,
            vec![("X-Conduit-Capabilities", "gzip utf8")],
            r#"{"result": true, "error_code": null}"#,
        ),
        (200, vec![], r#"{"result": true, "error_code": null}"#),
    ]);
    let client = Client::new(&base, "t").unwrap();
    client.call("conduit.ping", &serde_json::json!({})).unwrap();
    let first = recv_seen(&rx);
    assert_eq!(header_value(&first.1, "content-encoding"), None);

    client.call("conduit.ping", &serde_json::json!({})).unwrap();
    let second = recv_seen(&rx);
    assert_eq!(
        header_value(&second.1, "content-encoding"),
        Some("gzip".to_string())
    );
    assert_eq!(
        header_value(&second.1, "content-type"),
        Some("application/x-www-form-urlencoded".to_string())
    );
    assert_eq!(&second.2[..2], &[0x1f, 0x8b]);
    // The gzipped body is the same querystring the plain request sent.
    assert_eq!(gunzip(&second.2), String::from_utf8(first.2).unwrap());
}

fn gunzip(data: &[u8]) -> String {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn session_keys_flow_into_later_requests() {
    let (base, rx) = serve_script(vec![
        (
            200,
            vec![],
            r#"{"result": {"sessionKey": "sess-9", "connectionID": "conn-4"}, "error_code": null}"#,
        ),
        (200, vec![], r#"{"result": [], "error_code": null}"#),
    ]);
    let client = Client::new(&base, "t").unwrap();
    client
        .call("conduit.connect", &serde_json::json!({}))
        .unwrap();
    recv_seen(&rx);
    client
        .call("differential.query", &serde_json::json!({}))
        .unwrap();
    let second = recv_seen(&rx);
    let body = String::from_utf8(second.2).unwrap();
    assert!(body.contains("sessionKey"), "{body}");
    assert!(body.contains("sess-9"), "{body}");
    assert!(body.contains("connectionID"), "{body}");
    assert!(body.contains("conn-4"), "{body}");
}

#[test]
fn connect_exchanges_certificate_for_auth_pair() {
    let (base, rx) = serve_script(vec![(
        200,
        vec![],
        r#"{"result": {"sessionKey": "s", "connectionID": "c"}, "error_code": null}"#,
    )]);
    let client = Client::new(&base, "t").unwrap();
    client
        .call(
            "conduit.connect",
            &serde_json::json!({"certificate": "CERT"}),
        )
        .unwrap();
    let seen = recv_seen(&rx);
    let body = String::from_utf8(seen.2).unwrap();
    assert!(!body.contains("certificate"), "{body}");
    assert!(body.contains("authToken"), "{body}");
    assert!(body.contains("authSignature"), "{body}");
}

#[test]
fn custom_host_reaches_the_wire() {
    let (base, rx) = serve_script(vec![(
        200,
        vec![],
        r#"{"result": null, "error_code": null}"#,
    )]);
    let mut client = Client::new(&base, "t").unwrap();
    client.set_host("node-7.internal");
    client.call("m", &serde_json::json!({})).unwrap();
    let seen = recv_seen(&rx);
    let port = base
        .strip_prefix("http://127.0.0.1:")
        .unwrap()
        .strip_suffix('/')
        .unwrap();
    assert_eq!(
        header_value(&seen.1, "host"),
        Some(format!("node-7.internal:{port}"))
    );
}

// T1153 Wave-2 imports for the appended finding tests below (kept here
// so the shared suite stays append-only).
use arc_rs::{conduit::EnvelopeKind, config};

// T1153 CORE-2 (Rule 4 CLEAN): neither side sends `User-Agent` — PHP's
// conduit path is cURL with no UA override (libcurl default: none),
// and reqwest likewise sends none by default.
#[test]
fn core2_no_user_agent_header_sent() {
    let (base, rx) = serve_script(vec![(200, vec![], r#"{"result": {}, "error_code": null}"#)]);
    let client = Client::new(&base, "t").unwrap();
    client.call("m", &serde_json::json!({})).unwrap();
    let seen = recv_seen(&rx);
    assert_eq!(header_value(&seen.1, "user-agent"), None);
}

// T1153 CORE-13/T1153 RB-D7: with `preserve_order`, request params keep
// insertion order like PHP's arrays (the sorted-`BTreeMap` era ended
// with Wave-1 R8); servers ignore order either way.
#[test]
fn core13_request_params_keep_insertion_order() {
    let (base, rx) = serve_script(vec![(200, vec![], r#"{"result": {}, "error_code": null}"#)]);
    let client = Client::new(&base, "t").unwrap();
    let mut params = serde_json::Map::new();
    params.insert("z".to_string(), serde_json::json!(1));
    params.insert("a".to_string(), serde_json::json!(2));
    client
        .call("m", &serde_json::Value::Object(params))
        .unwrap();
    let seen = recv_seen(&rx);
    let body = String::from_utf8(seen.2).unwrap();
    let z = body.find("%22z%22").unwrap();
    let a = body.find("%22a%22").unwrap();
    let meta = body.find("__conduit__").unwrap();
    assert!(z < a && a < meta, "{body}");
}

#[test]
fn core5_auth_guidance_for_maps_codes() {
    // Structured mapping for toolset callers: auth codes map, the
    // tokened session code and unknown codes pass through.
    let tokenless = Client::new("https://phab.example/", "").unwrap();
    let err = Error::Api {
        code: "ERR-INVALID-SESSION".to_string(),
        info: "stale".to_string(),
    };
    let guidance = tokenless.auth_guidance_for(&err).unwrap();
    assert_eq!(guidance.title, "LOGIN REQUIRED");
    let tokened = Client::new("https://phab.example/", "t").unwrap();
    assert_eq!(tokened.auth_guidance_for(&err), None);
    let err = Error::Api {
        code: "ERR-CONDUIT-CORE".to_string(),
        info: "nope".to_string(),
    };
    assert_eq!(tokenless.auth_guidance_for(&err), None);
    assert_eq!(
        tokenless.auth_guidance_for(&Error::Transport("down".to_string())),
        None
    );
}

#[test]
fn core9_authenticate_token_verifies_and_caches() {
    let (base, rx) = serve_script(vec![(
        200,
        vec![],
        r#"{"result": {"userName": "alice", "phid": "PHID-USER-1"}, "error_code": null}"#,
    )]);
    let mut client = Client::new(&base, "").unwrap();
    let creds = config::Credentials {
        token: Some("tok".to_string()),
        user: None,
        cert: None,
    };
    let (user, phid) = config::authenticate(&mut client, &creds, "arc test").unwrap();
    assert_eq!((user.as_str(), phid.as_str()), ("alice", "PHID-USER-1"));
    assert!(client.has_token());
    assert_eq!(
        client.authenticated_user(),
        Some(("alice".to_string(), "PHID-USER-1".to_string()))
    );
    let seen = recv_seen(&rx);
    assert!(seen.0.contains("user.whoami"), "{:?}", seen.0);
    let body = String::from_utf8(seen.2).unwrap();
    assert!(body.contains("tok"), "{body}");
}

#[test]
fn core9_authenticate_bad_token_renders_install_guidance() {
    let (base, _rx) = serve_script(vec![(
        200,
        vec![],
        r#"{"result": null, "error_code": "ERR-INVALID-AUTH", "error_info": "bad"}"#,
    )]);
    let mut client = Client::new(&base, "").unwrap();
    let creds = config::Credentials {
        token: Some("stale".to_string()),
        user: None,
        cert: None,
    };
    let err = config::authenticate(&mut client, &creds, "arc test").unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::Usage);
    assert!(
        err.message.contains("INSTALL A CERTIFICATE"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("ERR-INVALID-AUTH: bad"),
        "{}",
        err.message
    );
    // A failed verification unsets the token, like PHP.
    assert!(!client.has_token());
}

#[test]
fn core9_authenticate_legacy_connect_handshake() {
    let (base, rx) = serve_script(vec![(
        200,
        vec![],
        r#"{"result": {"userPHID": "PHID-USER-2"}, "error_code": null}"#,
    )]);
    let mut client = Client::new(&base, "").unwrap();
    let creds = config::Credentials {
        token: None,
        user: Some("bob".to_string()),
        cert: Some("cert-body".to_string()),
    };
    let (user, phid) = config::authenticate(&mut client, &creds, "arc test").unwrap();
    assert_eq!((user.as_str(), phid.as_str()), ("bob", "PHID-USER-2"));
    assert_eq!(
        client.authenticated_user(),
        Some(("bob".to_string(), "PHID-USER-2".to_string()))
    );
    let seen = recv_seen(&rx);
    assert!(seen.0.contains("conduit.connect"), "{:?}", seen.0);
    let body = String::from_utf8(seen.2).unwrap();
    assert!(!body.contains("certificate"), "{body}");
    assert!(body.contains("authToken"), "{body}");
    assert!(body.contains("authSignature"), "{body}");
    assert!(body.contains("clientVersion"), "{body}");
}

#[test]
fn core9_authenticate_empty_legacy_pair_fails_locally() {
    // No I/O: empty credentials are rejected before any call.
    let mut client = Client::new("http://127.0.0.1:9/", "").unwrap();
    for creds in [
        config::Credentials {
            token: None,
            user: Some("".to_string()),
            cert: Some("c".to_string()),
        },
        config::Credentials {
            token: None,
            user: Some("u".to_string()),
            cert: Some("".to_string()),
        },
    ] {
        let err = config::authenticate(&mut client, &creds, "arc test").unwrap_err();
        assert_eq!(err.kind, EnvelopeKind::Usage);
        assert!(
            err.message.contains("INSTALL A CERTIFICATE"),
            "{}",
            err.message
        );
    }
}
