//! What every provider must do, checked the same way for all of them. Used by tests only.

use std::{
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

use openlang_core::{
    Result,
    domain::*,
    ports::*,
    realtime::{self, AudioReader},
    registry::Registry,
    session::{Choice, Plan, Session},
};

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

/// Delays of one kind, over `count` updates.
#[derive(Debug)]
pub struct Latency {
    pub count: usize,
    pub median: Duration,
    pub max: Duration,
}

impl Latency {
    fn of(mut late: Vec<Duration>) -> Self {
        late.sort();
        assert!(!late.is_empty(), "nothing arrived");
        Self {
            count: late.len(),
            median: late[late.len() / 2],
            max: late[late.len() - 1],
        }
    }
}

/// What a listener waits for.
#[derive(Debug)]
pub struct Report {
    /// From the end of speech to its final transcript.
    pub transcript: Latency,
    /// From the end of speech to its final translation.
    pub translation: Latency,
    /// From an unfinished transcript to its translation.
    pub partial_lag: Latency,
}

/// Plays `samples` in real time through a whole session, as the app runs it, and measures what
/// a listener waits for. `stress` keeps every core busy meanwhile.
pub fn latency(
    stt: Arc<dyn SpeechToText>,
    translator: Arc<dyn Translator>,
    job: &Job,
    samples: &[f32],
    stress: bool,
) -> Report {
    make_ready(stt.as_ref(), job);
    make_ready(translator.as_ref(), job);
    let mut registry = Registry::default();
    registry.sources.push(Arc::new(Player {
        descriptor: Descriptor {
            id: "player",
            name: "Player",
            summary: "Plays test audio in real time.",
            fields: vec![],
        },
        samples: samples.to_vec().into(),
    }));
    registry.resampler = Some(Arc::new(Passthrough));
    let choice = |id: &str| Choice {
        id: id.into(),
        options: job.options.clone(),
    };
    let plan = Plan {
        sources: vec!["player".into()],
        stt: vec![choice(stt.descriptor().id)],
        translators: vec![choice(translator.descriptor().id)],
        source_language: job.source.clone(),
        target_language: job.target.clone(),
        allow_network: false,
        partial_interval: Duration::from_millis(250),
    };
    registry.stt.push(stt);
    registry.translators.push(translator);
    let cores = std::thread::available_parallelism().map_or(4, usize::from);
    let busy: Vec<_> = (0..if stress { cores } else { 0 })
        .map(|_| Command::new("yes").stdout(Stdio::null()).spawn().unwrap())
        .collect();
    let sink = Arc::new(Arrivals::default());
    let began = Instant::now();
    let session = Session::start(&registry, &plan, vec![sink.clone()], &[]).unwrap();
    let rate = f64::from(AudioFormat::SPEECH.sample_rate);
    let length = Duration::from_secs_f64(samples.len() as f64 / rate);
    std::thread::sleep(length + Duration::from_secs(3));
    session.stop();
    for mut b in busy {
        let _ = b.kill();
        let _ = b.wait();
    }
    let arrivals = sink.0.lock().unwrap();
    let after_speech = |translated: bool| {
        Latency::of(
            arrivals
                .iter()
                .filter(|a| a.1.is_final && a.2 == translated)
                .map(|(at, s, _)| at.duration_since(began).saturating_sub(s.end))
                .collect(),
        )
    };
    let heard_at = |s: &Segment| {
        arrivals
            .iter()
            .find(|a| !a.2 && a.1.id == s.id && a.1.revision == s.revision)
            .map(|a| a.0)
    };
    let partial_lag = arrivals
        .iter()
        .filter(|a| a.2 && !a.1.is_final)
        .filter_map(|(at, s, _)| Some(at.duration_since(heard_at(s)?)))
        .collect();
    Report {
        transcript: after_speech(false),
        translation: after_speech(true),
        partial_lag: Latency::of(partial_lag),
    }
}

/// When each transcript and translation arrived, its segment, and whether it is the translation.
#[derive(Default)]
struct Arrivals(Mutex<Vec<(Instant, Segment, bool)>>);

impl EventSink for Arrivals {
    fn accept(&self, event: &SessionEvent) -> Result<()> {
        let (segment, translated) = match &event.body {
            EventBody::Transcript(t) => (&t.segment, false),
            EventBody::Translation(t) => (&t.segment, true),
            _ => return Ok(()),
        };
        let arrival = (Instant::now(), segment.clone(), translated);
        self.0.lock().unwrap().push(arrival);
        Ok(())
    }
}

struct Player {
    descriptor: Descriptor,
    samples: Arc<[f32]>,
}

struct Playing(Arc<AtomicBool>);

impl Capture for Playing {}

impl Drop for Playing {
    fn drop(&mut self) {
        self.0.store(true, SeqCst);
    }
}

impl AudioSource for Player {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    fn origin(&self) -> Origin {
        Origin::System
    }
    fn open(&self) -> Result<(AudioReader, Box<dyn Capture>)> {
        let (mut writer, reader) = realtime::channel(AudioFormat::SPEECH, 2);
        let stopped = Arc::new(AtomicBool::new(false));
        let (samples, stop) = (self.samples.clone(), stopped.clone());
        std::thread::spawn(move || {
            let began = Instant::now();
            let every = AudioFormat::SPEECH.sample_rate as usize / 50;
            for (i, part) in samples.chunks(every).enumerate() {
                if stop.load(SeqCst) {
                    return;
                }
                writer.write(part);
                let due = began + Duration::from_millis(20 * (i as u64 + 1));
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
        });
        Ok((reader, Box::new(Playing(stopped))))
    }
}

/// Test audio is already 16 kHz mono.
struct Passthrough;

impl AudioTransform for Passthrough {
    fn process(&mut self, chunk: &AudioChunk) -> Result<Option<AudioChunk>> {
        Ok(Some(chunk.clone()))
    }
    fn flush(&mut self) -> Result<Option<AudioChunk>> {
        Ok(None)
    }
}

impl TransformFactory for Passthrough {
    fn create(&self, _: AudioFormat) -> Box<dyn AudioTransform> {
        Box::new(Passthrough)
    }
}
