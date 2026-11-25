//! What every provider must do, checked the same way for all of them. Used by tests only.

use std::{
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use openlang_core::{domain::*, ports::*};

/// 16 kHz mono speech made by macOS `say`.
pub fn speak(voice: &str, text: &str) -> Vec<f32> {
    let path = std::env::temp_dir().join(format!("openlang-{}.wav", SegmentId::new().0));
    let status = Command::new("say")
        .args([
            "-v",
            voice,
            "--file-format=WAVE",
            "--data-format=LEF32@16000",
            "-o",
        ])
        .arg(&path)
        .arg(text)
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    bytes[data..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

pub fn descriptor(p: &dyn Provider) {
    let d = p.descriptor();
    assert!(!d.id.is_empty() && !d.name.is_empty() && !d.summary.is_empty());
    for f in &d.fields {
        if let FieldKind::Choice { choices } = &f.kind {
            assert!(
                choices.iter().any(|c| c.value == f.default),
                "{}: bad default",
                f.key
            );
        }
    }
}

pub fn make_ready(p: &dyn Provider, job: &Job) {
    descriptor(p);
    if matches!(p.readiness(job), Readiness::NeedsDownload { .. }) {
        let seen = Mutex::new(0.0_f32);
        p.prepare(job, &|x| *seen.lock().unwrap() = x).unwrap();
    }
    assert_eq!(p.readiness(job), Readiness::Ready);
}

/// Stopping before any sound arrived must not hang.
pub fn stops_without_audio(stt: &dyn SpeechToText, job: &Job) {
    make_ready(stt, job);
    let mut session = stt.open(job, Arc::new(|_| {})).unwrap();
    let began = Instant::now();
    session.finish().unwrap();
    drop(session);
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "stop took {:?}",
        began.elapsed()
    );
}

/// Plays `samples` at real-time speed and returns the final text and detected language.
pub fn speech(stt: &dyn SpeechToText, job: &Job, samples: &[f32]) -> (String, Option<String>) {
    make_ready(stt, job);
    let heard: Arc<Mutex<Vec<(Instant, SttUpdate)>>> = Arc::default();
    let log = heard.clone();
    let out: SttOutput = Arc::new(move |u| log.lock().unwrap().push((Instant::now(), u.unwrap())));
    let mut session = stt.open(job, out).unwrap();
    let began = Instant::now();
    for part in samples.chunks(800) {
        let chunk = AudioChunk {
            format: AudioFormat::SPEECH,
            samples: part.to_vec(),
            at: Duration::ZERO,
            segment: None,
        };
        session.push(&chunk).unwrap();
        std::thread::sleep(Duration::from_millis(50));
    }
    session.finish().unwrap();
    let delivered = heard.lock().unwrap().len();
    session.finish().unwrap();
    drop(session);
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), delivered, "updates arrived after finish");
    assert!(heard.iter().any(|(_, u)| !u.is_final), "no partial results");
    let finals: Vec<_> = heard
        .iter()
        .map(|(_, u)| u)
        .filter(|u| u.is_final)
        .collect();
    assert!(!finals.is_empty(), "no final result");
    assert!(heard.last().unwrap().1.is_final, "ended on a partial");
    assert!(
        finals.windows(2).all(|w| w[0].start <= w[1].start),
        "segments out of order"
    );
    println!(
        "{}: first partial {:.2}s, {} finals",
        stt.descriptor().id,
        heard[0].0.duration_since(began).as_secs_f64(),
        finals.len()
    );
    let text = finals
        .iter()
        .map(|u| u.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    (text, finals[0].language.clone())
}

pub fn translation(t: &dyn Translator, job: &Job, text: &str) -> String {
    make_ready(t, job);
    let mut session = t.open(job).unwrap();
    let source = job.source.as_deref().unwrap();
    let at = Instant::now();
    let first = session.translate(text, source).unwrap();
    let cold = at.elapsed();
    let at = Instant::now();
    session.translate(text, source).unwrap();
    println!(
        "{}: first {:.2}s, next {:.2}s: {first}",
        t.descriptor().id,
        cold.as_secs_f64(),
        at.elapsed().as_secs_f64()
    );
    assert!(!first.trim().is_empty());
    first
}
