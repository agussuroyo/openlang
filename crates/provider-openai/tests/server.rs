//! Talks to a real local HTTP server over TCP.

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use openlang_core::{domain::*, ports::*, registry::Registry};

fn serve(status: &'static str, body: &'static str) -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut seen = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                seen.extend_from_slice(&buffer[..n]);
                let head = String::from_utf8_lossy(&seen).to_lowercase();
                if let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: ")?.trim().parse().ok())
                        .unwrap_or(0usize);
                    if seen.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://example.invalid/\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let _ = tx.send(seen);
        }
    });
    (base, rx)
}

fn registry() -> Registry {
    let mut r = Registry::default();
    openlang_provider_openai::register(&mut r);
    r
}

fn job(base: &str, key: &str) -> Job {
    let options = [("base", base), ("model", "m"), ("key", key)]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .into();
    Job {
        source: Some("en-US".into()),
        target: "id".into(),
        options,
    }
}

#[test]
fn translates() {
    let (base, seen) = serve(
        "200 OK",
        r#"{"choices":[{"message":{"content":" Selamat pagi "}}]}"#,
    );
    let r = registry();
    let mut s = r.translators[0].open(&job(&base, "k")).unwrap();
    assert_eq!(
        s.translate("Good \"morning\"", "en").unwrap(),
        "Selamat pagi"
    );
    let request = String::from_utf8(seen.recv().unwrap()).unwrap();
    assert!(request.starts_with("POST /v1/chat/completions"));
    assert!(request.to_lowercase().contains("authorization: bearer k"));
    assert!(request.contains(r#"Good \"morning\""#));
}

#[test]
fn recognizes_an_utterance() {
    let (base, seen) = serve("200 OK", r#"{"text":"hello"}"#);
    let r = registry();
    let heard: Arc<Mutex<Vec<SttUpdate>>> = Arc::default();
    let log = heard.clone();
    let mut s = r.stt[0]
        .open(
            &job(&base, ""),
            Arc::new(move |u| log.lock().unwrap().push(u.unwrap())),
        )
        .unwrap();
    let samples = (0..16_000).map(|i| (i as f32 * 0.2).sin() * 0.3).collect();
    s.push(&AudioChunk {
        format: AudioFormat::SPEECH,
        samples,
        at: Duration::ZERO,
        segment: None,
    })
    .unwrap();
    s.finish().unwrap();
    assert_eq!(heard.lock().unwrap()[0].text, "hello");
    let request = seen.recv().unwrap();
    assert!(request.windows(4).any(|w| w == b"RIFF"));
}

#[test]
fn reports_server_errors_and_never_follows_redirects() {
    for (status, body, expect) in [
        (
            "401 Unauthorized",
            r#"{"error":{"message":"bad key"}}"#,
            "bad key",
        ),
        ("302 Found", "", "302"),
        ("200 OK", "{}", "has no text"),
    ] {
        let (base, _seen) = serve(status, body);
        let r = registry();
        let mut s = r.translators[0].open(&job(&base, "")).unwrap();
        let e = s.translate("x", "en").unwrap_err().to_string();
        assert!(e.contains(expect), "{e}");
    }
}

#[test]
fn knows_what_leaves_this_computer() {
    let r = registry();
    let p = &r.translators[0];
    for (base, expected) in [
        ("http://localhost:11434/v1", Location::OnDevice),
        ("http://127.0.0.1:1/v1", Location::OnDevice),
        ("http://[::1]:1/v1", Location::OnDevice),
        ("http://localhost.evil.com/v1", Location::Network),
        ("http://localhost@evil.com/v1", Location::Network),
        ("https://api.example.com/v1", Location::Network),
        ("not a url", Location::Network),
    ] {
        assert_eq!(p.location(&job(base, "").options), expected, "{base}");
    }
}

#[test]
fn never_sends_a_key_unencrypted() {
    let r = registry();
    assert!(
        r.translators[0]
            .open(&job("http://api.example.com/v1", "secret"))
            .is_err()
    );
    assert!(
        r.translators[0]
            .open(&job("http://api.example.com/v1", ""))
            .is_ok()
    );
}
