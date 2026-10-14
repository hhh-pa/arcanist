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
        let request_line = head.lines().next().unwrap_or("").to_string();
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
}

#[test]
fn maps_api_errors() {
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
