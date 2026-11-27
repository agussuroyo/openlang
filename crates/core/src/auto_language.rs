//! Automatic spoken language for recognizers that cannot detect it: a detector names the
//! language, then the recognizer opens for it.

use std::{
    collections::{BTreeSet, VecDeque},
    mem,
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{Receiver, SyncSender, TryRecvError, channel, sync_channel},
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
/// Audio kept for the recognizer while the language is still unknown.
const WAITING: usize = RATE * 10;

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
    waiting: VecDeque<AudioChunk>,
    waiting_samples: usize,
    window: VecDeque<f32>,
    since_detection: usize,
    first_at: Option<Duration>,
    candidate: Option<(String, u32)>,
    requests: SyncSender<Vec<f32>>,
    answers: Receiver<Result<Option<Detected>>>,
}

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
        let (requests, inbox) = sync_channel::<Vec<f32>>(1);
        let (reply, answers) = channel();
        std::thread::spawn(move || {
            for samples in inbox {
                if reply.send(detection.detect(&samples)).is_err() {
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
            waiting: VecDeque::new(),
            waiting_samples: 0,
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
        if self.since_detection < STEP || self.window.len() < WINDOW {
            return;
        }
        self.since_detection = 0;
        // Full means a detection is still running; this window is skipped.
        let _ = self
            .requests
            .try_send(self.window.iter().copied().collect());
    }

    /// `next_at`: where the next audio for a recognizer starts.
    fn read_answers(&mut self, next_at: Duration) {
        loop {
            match self.answers.try_recv() {
                Ok(Ok(Some(found))) => self.vote(found, next_at),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => (self.out)(Err(e)),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    fn vote(&mut self, found: Detected, next_at: Duration) {
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
        let times = match &self.candidate {
            Some((language, times)) if *language == found.language => times + 1,
            _ => 1,
        };
        if times < rule.times {
            self.candidate = Some((found.language, times));
            return;
        }
        self.candidate = None;
        self.switch(found.language, next_at);
    }

    /// The old recognizer finishes in its language; the new one starts from the next chunk.
    fn switch(&mut self, language: String, next_at: Duration) {
        let job = Job {
            source: Some(language.clone()),
            ..self.job.clone()
        };
        let opened = self.prepare(&job).and_then(|()| {
            let current = self.current.clone();
            let out = self.out.clone();
            let labelled: SttOutput =
                Arc::new(move |update| out(update.map(|u| label(&current, u))));
            self.provider.open(&job, labelled)
        });
        let next = match opened {
            Ok(next) => next,
            // A language the recognizer cannot open keeps the one that works.
            Err(e) => return (self.out)(Err(e)),
        };
        if let Some(mut old) = self.recognizer.replace(next)
            && let Err(e) = old.finish()
        {
            (self.out)(Err(e));
        }
        let first_at = self.first_at.unwrap_or(next_at);
        *self.lock() = Current {
            language,
            start: next_at.saturating_sub(first_at),
        };
        self.waiting_samples = 0;
        for chunk in mem::take(&mut self.waiting) {
            self.push_recognizer(&chunk);
        }
    }

    fn prepare(&self, job: &Job) -> Result<()> {
        if matches!(
            self.provider.readiness(job),
            Readiness::NeedsDownload { .. }
        ) {
            self.provider.prepare(job, &|_| {})?;
        }
        Ok(())
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
        let next_at = self.waiting.front().map_or(chunk.at, |c| c.at);
        self.read_answers(next_at);
        self.listen(chunk);
        if self.recognizer.is_some() {
            self.push_recognizer(chunk);
            return Ok(());
        }
        self.waiting_samples += chunk.samples.len();
        self.waiting.push_back(chunk.clone());
        while self.waiting_samples > WAITING {
            let Some(old) = self.waiting.pop_front() else {
                break;
            };
            self.waiting_samples -= old.samples.len();
        }
        Ok(())
    }

    /// The detector thread ends by itself once its last request is done.
    fn finish(&mut self) -> Result<()> {
        self.recognizer.take().map_or(Ok(()), |mut r| r.finish())
    }
}
