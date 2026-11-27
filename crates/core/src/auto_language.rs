//! Automatic spoken language for recognizers that cannot detect it: a detector names the
//! language, then the recognizer opens for it.

use std::{
    collections::{BTreeSet, VecDeque},
    mem,
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{
            Receiver, RecvTimeoutError, SendError, SyncSender, TryRecvError, channel, sync_channel,
        },
    },
    time::Duration,
};

use crate::{
    Result,
    domain::{AudioChunk, AudioFormat},
    nodes::primary,
    ports::*,
};

const RATE: usize = AudioFormat::SPEECH.sample_rate as usize;
/// How much speech one detection hears.
const WINDOW: usize = RATE * 3;
/// How often a detection starts.
const STEP: usize = RATE;
/// How often a detection starts while a recognizer is open and no switch is pending.
const LOCKED_STEP: usize = RATE * 5;
/// Audio kept for the recognizer while the language is still unknown.
const WAITING: usize = RATE * 10;
/// Heard audio kept so a new recognizer can start where its language began.
const KEPT: usize = WINDOW + 2 * STEP;
/// How long `finish` waits for a recognizer that is still opening.
const FINISH_LIMIT: Duration = Duration::from_secs(3);

/// Opening needs one sure answer; switching needs the same new answer twice.
const FIRST: Vote = Vote {
    confidence: 0.5,
    times: 1,
};
const SWITCH: Vote = Vote {
    confidence: 0.6,
    times: 2,
};

struct Vote {
    confidence: f32,
    times: u32,
}

/// Auto was asked for, and the recognizer cannot do it itself.
pub fn needs_detector(stt: &dyn Provider, job: &Job) -> bool {
    job.source.is_none() && !stt.languages().can_detect
}

/// The provider that must be ready before `stt` can run `job`.
pub fn to_prepare<'a>(
    stt: &'a dyn Provider,
    detector: Option<&'a dyn Provider>,
    job: &Job,
) -> &'a dyn Provider {
    match detector {
        Some(d) if needs_detector(stt, job) => d,
        _ => stt,
    }
}

/// What the current recognizer hears, shared with its output.
#[derive(Default)]
struct Current {
    language: String,
    /// Where the recognizer's first sample sits in this session.
    start: Duration,
}

pub struct AutoLanguage {
    provider: Arc<dyn SpeechToText>,
    job: Job,
    out: SttOutput,
    current: Arc<Mutex<Current>>,
    recognizer: Option<Box<dyn SttSession>>,
    opening: Option<Opening>,
    /// Audio the recognizer has not heard yet.
    waiting: VecDeque<AudioChunk>,
    waiting_samples: usize,
    /// Audio the recognizer has heard, kept for the one that may replace it.
    heard: VecDeque<AudioChunk>,
    heard_samples: usize,
    window: VecDeque<f32>,
    since_detection: usize,
    first_at: Option<Duration>,
    /// Language, votes, and where the first window that voted for it began.
    candidate: Option<(String, u32, Duration)>,
    requests: SyncSender<(Duration, Vec<f32>)>,
    answers: Receiver<(Duration, Result<Option<Detected>>)>,
}

/// A recognizer being prepared and opened, and the old one finished, on their own thread, so
/// the graph never waits for them.
struct Opening {
    current: Arc<Mutex<Current>>,
    /// Heard by the old recognizer, but already in the new language.
    replay: VecDeque<AudioChunk>,
    /// The new recognizer, or the error and the old one, still unfinished, given back.
    done: Receiver<Opened>,
}

type Opened = (Result<Box<dyn SttSession>>, Option<Box<dyn SttSession>>);

impl AutoLanguage {
    pub fn open(
        provider: Arc<dyn SpeechToText>,
        job: Job,
        detector: &dyn LanguageDetector,
        out: SttOutput,
    ) -> Result<Self> {
        let allowed: BTreeSet<String> = provider
            .languages()
            .tags
            .iter()
            .map(|t| primary(t).to_owned())
            .collect();
        let mut detection = detector.open(allowed.into_iter().collect())?;
        // One request at a time: a busy detector skips windows instead of falling behind.
        // No queue either, so every answer is about the newest window.
        let (requests, inbox) = sync_channel::<(Duration, Vec<f32>)>(0);
        let (reply, answers) = channel();
        std::thread::spawn(move || {
            for (from, samples) in inbox {
                if reply.send((from, detection.detect(&samples))).is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            provider,
            job,
            out,
            current: Arc::default(),
            recognizer: None,
            opening: None,
            waiting: VecDeque::new(),
            waiting_samples: 0,
            heard: VecDeque::new(),
            heard_samples: 0,
            window: VecDeque::with_capacity(WINDOW),
            since_detection: 0,
            first_at: None,
            candidate: None,
            requests,
            answers,
        })
    }

    fn listen(&mut self, chunk: &AudioChunk) {
        self.window.extend(&chunk.samples);
        let excess = self.window.len().saturating_sub(WINDOW);
        self.window.drain(..excess);
        self.since_detection += chunk.samples.len();
        let locked =
            self.candidate.is_none() && (self.recognizer.is_some() || self.opening.is_some());
        let step = if locked { LOCKED_STEP } else { STEP };
        if self.since_detection < step || self.window.len() < WINDOW {
            return;
        }
        self.since_detection = 0;
        let from = end(chunk).saturating_sub(Duration::from_secs_f64(seconds(WINDOW)));
        // Full means a detection is still running; this window is skipped.
        let _ = self
            .requests
            .try_send((from, self.window.iter().copied().collect()));
    }

    fn read_answers(&mut self) {
        loop {
            match self.answers.try_recv() {
                Ok((from, Ok(Some(found)))) => self.vote(found, from),
                Ok((_, Ok(None))) => {}
                Ok((_, Err(e))) => (self.out)(Err(e)),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    /// `from`: where the window that gave `found` began.
    fn vote(&mut self, found: Detected, from: Duration) {
        // The recognizer being opened already follows an answer.
        if self.opening.is_some() {
            return;
        }
        let rule = if self.recognizer.is_some() {
            SWITCH
        } else {
            FIRST
        };
        let current = self.lock().language.clone();
        if found.language == current || found.confidence < rule.confidence {
            self.candidate = None;
            return;
        }
        let (times, from) = match &self.candidate {
            Some((language, times, first)) if *language == found.language => (times + 1, *first),
            _ => (1, from),
        };
        if times < rule.times {
            self.candidate = Some((found.language, times, from));
            return;
        }
        self.candidate = None;
        self.switch(found.language, from);
    }

    /// The old recognizer finishes in its language; the new one starts where the first window
    /// that voted for its language began. Preparing, opening and finishing run on their own
    /// thread; audio is held meanwhile.
    fn switch(&mut self, language: String, from: Duration) {
        let job = Job {
            source: Some(language.clone()),
            ..self.job.clone()
        };
        let current = Arc::new(Mutex::new(Current {
            language,
            start: Duration::ZERO,
        }));
        let labels = current.clone();
        let out = self.out.clone();
        let labelled: SttOutput = Arc::new(move |update| out(update.map(|u| label(&labels, u))));
        let provider = self.provider.clone();
        let mut old = self.recognizer.take();
        let out = self.out.clone();
        let (answer, done) = channel::<Opened>();
        std::thread::spawn(move || {
            let opened =
                prepare(provider.as_ref(), &job).and_then(|()| provider.open(&job, labelled));
            // The old one says its last words before the new one hears anything, so the two
            // never mix in one segment.
            if opened.is_ok()
                && let Some(mut old) = old.take()
                && let Err(e) = old.finish()
            {
                out(Err(e));
            }
            // Nobody waits any more: the session stopped first.
            if let Err(SendError((opened, old))) = answer.send((opened, old)) {
                for mut late in opened.into_iter().chain(old) {
                    let _ = late.finish();
                }
            }
        });
        let replay = mem::take(&mut self.heard)
            .into_iter()
            .filter(|c| end(c) > from)
            .collect();
        self.heard_samples = 0;
        self.opening = Some(Opening {
            current,
            replay,
            done,
        });
    }

    /// Puts the new recognizer in place once it opened, waiting at most `wait` for it.
    fn take_opened(&mut self, wait: Duration) {
        let Some(opening) = self.opening.take() else {
            return;
        };
        let next = match opening.done.recv_timeout(wait) {
            Ok((Ok(next), _)) => next,
            // A language the recognizer cannot open keeps the one that works.
            Ok((Err(e), old)) => {
                self.recognizer = old;
                return (self.out)(Err(e));
            }
            Err(RecvTimeoutError::Timeout) => {
                self.opening = Some(opening);
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let first_at = self.first_at.unwrap_or_default();
        let next_at = opening
            .replay
            .front()
            .or(self.waiting.front())
            .map_or(first_at, |c| c.at);
        opening
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .start = next_at.saturating_sub(first_at);
        self.current = opening.current;
        self.recognizer = Some(next);
        for chunk in opening.replay {
            self.hear(chunk);
        }
    }

    /// Gives waiting audio to the recognizer, unless a new one may still claim it. Held audio
    /// stays bounded: the oldest goes to the recognizer there is, or is dropped.
    fn feed(&mut self) {
        let hold = self.recognizer.is_none() || self.candidate.is_some() || self.opening.is_some();
        while self.waiting_samples > WAITING || (!hold && !self.waiting.is_empty()) {
            let Some(chunk) = self.waiting.pop_front() else {
                break;
            };
            self.waiting_samples -= chunk.samples.len();
            if self.recognizer.is_some() {
                self.hear(chunk);
            }
        }
    }

    fn hear(&mut self, chunk: AudioChunk) {
        self.push_recognizer(&chunk);
        self.heard_samples += chunk.samples.len();
        self.heard.push_back(chunk);
        while self.heard_samples > KEPT {
            let Some(old) = self.heard.pop_front() else {
                break;
            };
            self.heard_samples -= old.samples.len();
        }
    }

    fn push_recognizer(&mut self, chunk: &AudioChunk) {
        let Some(recognizer) = &mut self.recognizer else {
            return;
        };
        if let Err(e) = recognizer.push(chunk) {
            (self.out)(Err(e));
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Current> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn prepare(provider: &dyn SpeechToText, job: &Job) -> Result<()> {
    if matches!(provider.readiness(job), Readiness::NeedsDownload { .. }) {
        provider.prepare(job, &|_| {})?;
    }
    Ok(())
}

fn seconds(samples: usize) -> f64 {
    samples as f64 / RATE as f64
}

/// Where the chunk's last sample ends in this session.
fn end(chunk: &AudioChunk) -> Duration {
    chunk.at + Duration::from_secs_f64(seconds(chunk.samples.len()))
}

fn label(current: &Mutex<Current>, mut update: SttUpdate) -> SttUpdate {
    let current = current.lock().unwrap_or_else(PoisonError::into_inner);
    update
        .language
        .get_or_insert_with(|| current.language.clone());
    update.start += current.start;
    update.end += current.start;
    update
}

impl SttSession for AutoLanguage {
    fn push(&mut self, chunk: &AudioChunk) -> Result<()> {
        self.first_at.get_or_insert(chunk.at);
        self.waiting_samples += chunk.samples.len();
        self.waiting.push_back(chunk.clone());
        self.take_opened(Duration::ZERO);
        self.read_answers();
        self.listen(chunk);
        self.feed();
        Ok(())
    }

    /// The detector thread ends by itself once its last request is done. A recognizer still
    /// opening is waited for at most `FINISH_LIMIT`, then left to its thread to finish.
    fn finish(&mut self) -> Result<()> {
        self.take_opened(FINISH_LIMIT);
        self.opening = None;
        self.candidate = None;
        self.feed();
        self.recognizer.take().map_or(Ok(()), |mut r| r.finish())
    }
}
