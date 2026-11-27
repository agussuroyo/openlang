//! Talks to a real local HTTP server over TCP.

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use openlang_core::{domain::*, ports::*, registry::Registry};
use openlang_provider_openai::{Address, SERVICES, Service};

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
    assert!(request.contains(r#""temperature":0"#));
}

/// Speaks one second of tone and returns what the engine heard.
fn hear(stt: &dyn SpeechToText, job: &Job) -> Vec<SttUpdate> {
    let heard: Arc<Mutex<Vec<SttUpdate>>> = Arc::default();
    let log = heard.clone();
    let mut s = stt
        .open(job, Arc::new(move |u| log.lock().unwrap().push(u.unwrap())))
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
    heard.lock().unwrap().clone()
}

#[test]
fn recognizes_an_utterance() {
    let (base, seen) = serve("200 OK", r#"{"text":"hello"}"#);
    let r = registry();
    assert_eq!(hear(r.stt[0].as_ref(), &job(&base, ""))[0].text, "hello");
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

fn service(id: &str) -> Service {
    *SERVICES.iter().find(|s| s.id == id).unwrap()
}

/// A registry with one cloud service moved to a local test server.
fn moved(id: &str, base: &str) -> Registry {
    let base = Box::leak(base.trim_end_matches("/v1").to_owned().into_boxed_str());
    let mut r = Registry::default();
    openlang_provider_openai::add(
        &mut r,
        Service {
            address: Address::Editable(base),
            ..service(id)
        },
    );
    r
}

fn options(pairs: &[(&str, &str)]) -> Options {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn text_of(request: Vec<u8>) -> String {
    String::from_utf8_lossy(&request).into_owned()
}

#[test]
fn deepseek_translates_with_its_default_model_and_no_thinking() {
    let (base, seen) = serve("200 OK", r#"{"choices":[{"message":{"content":"Halo"}}]}"#);
    let r = moved("deepseek", &base);
    let job = Job {
        options: options(&[("key", "k")]),
        ..job(&base, "k")
    };
    let mut s = r.translators[0].open(&job).unwrap();
    assert_eq!(s.translate("Hello", "en").unwrap(), "Halo");
    let request = text_of(seen.recv().unwrap());
    assert!(request.starts_with("POST /chat/completions"), "{request}");
    assert!(request.contains(r#""model":"deepseek-flash""#), "{request}");
    assert!(
        request.contains(r#""thinking":{"type":"disabled"}"#),
        "{request}"
    );
    assert!(!request.contains("temperature"), "{request}");
}

#[test]
fn deepseek_hears_with_the_audio_file_only() {
    let (base, seen) = serve(
        "200 OK",
        r#"{"request_id":"r","text":"halo","duration_ms":1000}"#,
    );
    let r = moved("deepseek", &base);
    let job = Job {
        options: options(&[("key", "k")]),
        ..job(&base, "k")
    };
    let heard = hear(r.stt[0].as_ref(), &job);
    assert_eq!(heard[0].text, "halo");
    assert_eq!(heard[0].language, None);
    let request = text_of(seen.recv().unwrap());
    assert!(request.starts_with("POST /v1/tools/asr"), "{request}");
    assert!(request.contains(r#"name="file""#), "{request}");
    assert!(!request.contains(r#"name="model""#), "{request}");
    assert!(!request.contains(r#"name="language""#), "{request}");
    assert!(!r.stt[0].languages().can_detect);
}

#[test]
fn a_cloud_service_needs_a_key_and_shares_it() {
    let mut r = Registry::default();
    openlang_provider_openai::add(&mut r, service("deepseek"));
    let job = |key| Job {
        options: options(&[("key", key)]),
        ..job("", key)
    };
    for p in [
        r.translators[0].as_ref() as &dyn Provider,
        r.stt[0].as_ref(),
    ] {
        assert_eq!(p.descriptor().id, "deepseek");
        assert_eq!(p.location(&job("").options), Location::Network);
        assert!(matches!(
            p.readiness(&job("")),
            Readiness::NeedsAction { message } if message.contains("DeepSeek API key")
        ));
        assert_eq!(p.readiness(&job("k")), Readiness::Ready);
    }
}

#[test]
fn every_service_is_well_formed() {
    let r = registry();
    for p in r.translators.iter() {
        openlang_contract::descriptor(p.as_ref());
    }
    for p in r.stt.iter() {
        openlang_contract::descriptor(p.as_ref());
    }
    for s in SERVICES.iter().filter_map(|s| s.translate) {
        let extra: serde_json::Value = serde_json::from_str(s.extra).unwrap();
        assert!(extra.is_object(), "{}", s.extra);
    }
}

#[test]
fn openai_hears_and_reads_the_detected_language() {
    let (base, seen) = serve(
        "200 OK",
        r#"{"text":"bonjour","languages":[{"code":"fr"}]}"#,
    );
    let r = moved("openai", &base);
    let job = Job {
        options: options(&[("key", "k")]),
        ..job(&base, "k")
    };
    let heard = hear(r.stt[0].as_ref(), &job);
    assert_eq!(heard[0].text, "bonjour");
    assert_eq!(heard[0].language.as_deref(), Some("fr"));
    let request = text_of(seen.recv().unwrap());
    assert!(
        request.starts_with("POST /audio/transcriptions"),
        "{request}"
    );
    assert!(request.contains("gpt-transcribe"), "{request}");
}
