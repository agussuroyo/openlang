use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use openlang_core::{
    Result,
    auto_language::{AutoLanguage, needs_detector},
    domain::*,
    ports::*,
};

const RATE: usize = 16_000;

fn descriptor() -> Descriptor {
    Descriptor {
        id: "fake",
        name: "fake",
        summary: "",
        fields: vec![],
    }
}

macro_rules! provider {
    ($t:ty, $detects:expr) => {
        impl Provider for $t {
            fn descriptor(&self) -> &Descriptor {
                &self.descriptor
            }
            fn location(&self, _: &Options) -> Location {
                Location::OnDevice
            }
            fn languages(&self) -> Languages {
                Languages {
                    tags: vec!["en-US".into(), "id-ID".into()],
                    can_detect: $detects,
                }
            }
            fn readiness(&self, _: &Job) -> Readiness {
                Readiness::Ready
            }
            fn prepare(&self, _: &Job, _: &dyn Fn(f32)) -> Result<()> {
                Ok(())
            }
        }
    };
}

/// Records which language each session opened with and how much audio it got.
struct Stt {
    descriptor: Descriptor,
    opened: Arc<Mutex<Vec<(String, usize)>>>,
}
provider!(Stt, false);
impl SpeechToText for Stt {
    fn open(&self, job: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        let mut opened = self.opened.lock().unwrap();
        opened.push((job.source.clone().unwrap(), 0));
        Ok(Box::new(Session {
            index: opened.len() - 1,
            opened: self.opened.clone(),
            out,
        }))
    }
}
struct Session {
    index: usize,
    opened: Arc<Mutex<Vec<(String, usize)>>>,
    out: SttOutput,
}
impl SttSession for Session {
    fn push(&mut self, chunk: &AudioChunk) -> Result<()> {
        self.opened.lock().unwrap()[self.index].1 += chunk.samples.len();
        (self.out)(Ok(SttUpdate {
            text: "word".into(),
            language: None,
            start: Duration::ZERO,
            end: Duration::from_secs(1),
            is_final: true,
        }));
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Gives scripted answers, one per detection.
struct Detector {
    descriptor: Descriptor,
    answers: Vec<(&'static str, f32)>,
}
provider!(Detector, true);
impl LanguageDetector for Detector {
    fn open(&self, allowed: Vec<String>) -> Result<Box<dyn DetectorSession>> {
        assert_eq!(allowed, ["en", "id"]);
        Ok(Box::new(Script(self.answers.clone().into_iter())))
    }
}
struct Script(std::vec::IntoIter<(&'static str, f32)>);
impl DetectorSession for Script {
    fn detect(&mut self, _: &[f32]) -> Result<Option<Detected>> {
        Ok(self.0.next().map(|(language, confidence)| Detected {
            language: language.into(),
            confidence,
        }))
    }
}

struct Run {
    auto: AutoLanguage,
    opened: Arc<Mutex<Vec<(String, usize)>>>,
    heard: Arc<Mutex<Vec<SttUpdate>>>,
    second: u64,
}

fn run(answers: Vec<(&'static str, f32)>) -> Run {
    let opened = Arc::new(Mutex::default());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    let out: SttOutput =
        Arc::new(move |u: Result<SttUpdate>| sink.lock().unwrap().push(u.unwrap()));
    let stt = Arc::new(Stt {
        descriptor: descriptor(),
        opened: opened.clone(),
    });
    let detector = Detector {
        descriptor: descriptor(),
        answers,
    };
    let auto = AutoLanguage::open(stt, Job::default(), &detector, out).unwrap();
    Run {
        auto,
        opened,
        heard,
        second: 0,
    }
}

impl Run {
    /// Pushes one second of audio per step and gives the detector time to answer.
    fn speak(&mut self, seconds: u64) {
        for _ in 0..seconds {
            let chunk = AudioChunk {
                format: AudioFormat::SPEECH,
                samples: vec![0.1; RATE],
                at: Duration::from_secs(self.second),
                segment: None,
            };
            self.auto.push(&chunk).unwrap();
            self.second += 1;
            thread::sleep(Duration::from_millis(30));
        }
    }

    fn opened(&self) -> Vec<(String, usize)> {
        self.opened.lock().unwrap().clone()
    }
}

#[test]
fn only_auto_on_a_recognizer_that_cannot_detect_needs_the_detector() {
    let stt = Stt {
        descriptor: descriptor(),
        opened: Arc::default(),
    };
    assert!(needs_detector(&stt, &Job::default()));
    let chosen = Job {
        source: Some("en-US".into()),
        ..Job::default()
    };
    assert!(!needs_detector(&stt, &chosen));
}

#[test]
fn opens_in_the_detected_language_without_losing_the_first_words() {
    let mut r = run(vec![("id", 0.9)]);
    r.speak(5);
    let opened = r.opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].0, "id");
    // The detector answered on second 3; seconds 0 to 4 all reached the recognizer.
    assert_eq!(opened[0].1, RATE * 5);
    let heard = r.heard.lock().unwrap();
    assert!(heard.iter().all(|u| u.language.as_deref() == Some("id")));
    assert_eq!(heard[0].start, Duration::ZERO);
}

#[test]
fn switches_only_after_two_sure_answers_in_a_row() {
    let mut r = run(vec![
        ("id", 0.9),
        ("en", 0.9),
        ("id", 0.9),
        ("en", 0.9),
        ("en", 0.9),
    ]);
    r.speak(9);
    let languages: Vec<String> = r.opened().into_iter().map(|(l, _)| l).collect();
    assert_eq!(languages, ["id", "en"]);
    let heard = r.heard.lock().unwrap();
    let last = heard.last().unwrap();
    assert_eq!(last.language.as_deref(), Some("en"));
    // The new recognizer starts later in the session, so its times move with it.
    assert!(last.start > Duration::from_secs(5));
}

#[test]
fn unsure_answers_are_ignored() {
    let mut r = run(vec![("id", 0.3), ("en", 0.4)]);
    r.speak(6);
    assert!(r.opened().is_empty());
}

#[test]
fn stops_at_once_when_nobody_spoke() {
    let mut r = run(vec![]);
    r.speak(1);
    let started = Instant::now();
    r.auto.finish().unwrap();
    assert!(started.elapsed() < Duration::from_millis(50));
}
