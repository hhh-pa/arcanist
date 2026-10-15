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
    let (base, _rx) = serve_once(r#"{"result": null, "error_code": "E", "error_info": "no"}"#);
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        client.call("m", &serde_json::json!({})).unwrap_err(),
        Error::Api {
            code: "E".to_string(),
            info: "<m> no".to_string()
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
