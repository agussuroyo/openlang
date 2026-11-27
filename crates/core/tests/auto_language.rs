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
    /// How long `prepare` takes; `None` means nothing to download.
    download: Option<Duration>,
    /// How long a session takes to say its last words.
    last_words: Duration,
}
impl Provider for Stt {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    fn location(&self, _: &Options) -> Location {
        Location::OnDevice
    }
    fn languages(&self) -> Languages {
        Languages {
            tags: vec!["en-US".into(), "id-ID".into()],
            can_detect: false,
        }
    }
    fn readiness(&self, _: &Job) -> Readiness {
        match self.download {
            Some(_) => Readiness::NeedsDownload { bytes: None },
            None => Readiness::Ready,
        }
    }
    fn prepare(&self, _: &Job, _: &dyn Fn(f32)) -> Result<()> {
        thread::sleep(self.download.unwrap_or_default());
        Ok(())
    }
}
impl SpeechToText for Stt {
    fn open(&self, job: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        let mut opened = self.opened.lock().unwrap();
        opened.push((job.source.clone().unwrap(), 0));
        Ok(Box::new(Session {
            last_words: self.last_words,
            index: opened.len() - 1,
            opened: self.opened.clone(),
            out,
        }))
    }
}
struct Session {
    last_words: Duration,
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
    /// Says its last words a little later, as a real recognizer does.
    fn finish(&mut self) -> Result<()> {
        thread::sleep(self.last_words);
        (self.out)(Ok(SttUpdate {
            text: "last".into(),
            language: None,
            start: Duration::ZERO,
            end: Duration::from_secs(1),
            is_final: true,
        }));
        Ok(())
    }
}

/// Gives scripted answers, one per detection.
struct Detector {
    descriptor: Descriptor,
    answers: Vec<(&'static str, f32)>,
    detections: Arc<Mutex<usize>>,
}
provider!(Detector, true);
impl LanguageDetector for Detector {
    fn open(&self, allowed: Vec<String>) -> Result<Box<dyn DetectorSession>> {
        assert_eq!(allowed, ["en", "id"]);
        Ok(Box::new(Script(
            self.answers.clone().into_iter(),
            self.detections.clone(),
        )))
    }
}
struct Script(std::vec::IntoIter<(&'static str, f32)>, Arc<Mutex<usize>>);
impl DetectorSession for Script {
    fn detect(&mut self, _: &[f32]) -> Result<Option<Detected>> {
        *self.1.lock().unwrap() += 1;
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
    detections: Arc<Mutex<usize>>,
    second: u64,
    /// The longest single `push`.
    slowest: Duration,
}

fn run(answers: Vec<(&'static str, f32)>) -> Run {
    run_with(answers, None, Duration::from_millis(50))
}

fn run_with(
    answers: Vec<(&'static str, f32)>,
    download: Option<Duration>,
    last_words: Duration,
) -> Run {
    let opened = Arc::new(Mutex::default());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let detections = Arc::new(Mutex::new(0));
    let sink = heard.clone();
    let out: SttOutput =
        Arc::new(move |u: Result<SttUpdate>| sink.lock().unwrap().push(u.unwrap()));
    let stt = Arc::new(Stt {
        descriptor: descriptor(),
        opened: opened.clone(),
        download,
        last_words,
    });
    let detector = Detector {
        descriptor: descriptor(),
        answers,
        detections: detections.clone(),
    };
    let auto = AutoLanguage::open(stt, Job::default(), &detector, out).unwrap();
    Run {
        auto,
        opened,
        heard,
        detections,
        second: 0,
        slowest: Duration::ZERO,
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
            let started = Instant::now();
            self.auto.push(&chunk).unwrap();
            self.slowest = self.slowest.max(started.elapsed());
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
        download: None,
        last_words: Duration::ZERO,
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
    // Detections end on seconds 3, 8, 9, 14 and 15: every 5 s once open, every second while
    // a switch is pending.
    r.speak(17);
    r.auto.finish().unwrap();
    let opened = r.opened();
    let languages: Vec<&str> = opened.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(languages, ["id", "en"]);
    // "id" heard everything until "en" took over on second 16.
    assert_eq!(opened[0].1, RATE * 16);
    let heard = r.heard.lock().unwrap();
    let last = heard.last().unwrap();
    assert_eq!(last.language.as_deref(), Some("en"));
    // The new recognizer starts later in the session, so its times move with it.
    assert!(last.start > Duration::from_secs(5));
}

#[test]
fn a_new_language_is_heard_from_the_first_window_that_voted_for_it() {
    let mut r = run(vec![("id", 0.9), ("en", 0.9), ("en", 0.9)]);
    r.speak(11);
    r.auto.finish().unwrap();
    let opened = r.opened();
    // The window of seconds 5 to 7 voted "en" first. "id" kept hearing until the switch on
    // second 10, so no audio waited; "en" heard seconds 5 to 10 again.
    assert_eq!(opened, [("id".into(), RATE * 10), ("en".into(), RATE * 6)]);
    let heard = r.heard.lock().unwrap();
    assert_eq!(heard.last().unwrap().start, Duration::from_secs(5));
}

#[test]
fn a_replaced_recognizer_says_its_last_words_before_the_new_one_speaks() {
    let mut r = run(vec![("id", 0.9), ("en", 0.9), ("en", 0.9)]);
    r.speak(11);
    r.auto.finish().unwrap();
    let heard = r.heard.lock().unwrap();
    let languages: Vec<&str> = heard.iter().filter_map(|u| u.language.as_deref()).collect();
    let first = languages.iter().position(|l| *l == "en").unwrap();
    assert!(
        languages[first..].iter().all(|l| *l == "en"),
        "{languages:?}"
    );
    assert_eq!(heard[first - 1].text, "last");
}

#[test]
fn a_recognizer_that_never_finishes_cannot_hold_up_the_new_language() {
    let mut r = run_with(
        vec![("id", 0.9), ("en", 0.9), ("en", 0.9)],
        None,
        Duration::from_secs(60),
    );
    r.speak(11);
    // The new recognizer's words wait for the old one's last words at most 3 s.
    thread::sleep(Duration::from_millis(3500));
    let heard = r.heard.lock().unwrap();
    assert!(heard.iter().any(|u| u.language.as_deref() == Some("en")));
    assert!(
        heard.iter().all(|u| u.text != "last"),
        "late words were dropped"
    );
}

#[test]
fn a_slow_download_never_holds_up_the_audio() {
    let mut r = run_with(
        vec![("id", 0.9)],
        Some(Duration::from_millis(500)),
        Duration::from_millis(50),
    );
    r.speak(8);
    assert!(r.slowest < Duration::from_millis(100), "{:?}", r.slowest);
    assert!(r.opened().is_empty());
    // Stopping waits for the recognizer, which then hears everything that was said.
    r.auto.finish().unwrap();
    assert_eq!(r.opened(), [("id".into(), RATE * 8)]);
}

#[test]
fn detects_less_often_once_the_language_is_known() {
    let mut r = run(vec![("id", 0.9); 20]);
    r.speak(15);
    // Seconds 2, 7 and 12: every 5 s after the first answer instead of every second.
    assert_eq!(*r.detections.lock().unwrap(), 3);
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
