//! Live recognition for engines that only understand whole recordings: cuts the stream at pauses
//! and hands each utterance to the engine on a worker thread.

use std::{
    collections::VecDeque,
    sync::mpsc::{Receiver, Sender, channel},
    thread::JoinHandle,
    time::Duration,
};

use openlang_core::{
    Error, Result,
    domain::{AudioChunk, AudioFormat},
    ports::{SttOutput, SttSession, SttUpdate},
};

/// Text and, when the engine detects it, the language.
pub struct Heard {
    pub text: String,
    pub language: Option<String>,
}

pub trait Recognizer: Send + 'static {
    /// 16 kHz mono. `is_final` is the last call for this utterance; the next call starts a new one.
    fn recognize(&mut self, samples: &[f32], is_final: bool) -> Result<Heard>;
}

const RATE: usize = AudioFormat::SPEECH.sample_rate as usize;
const FRAME: usize = RATE / 50;
const PRE_ROLL: usize = RATE * 3 / 10;
const PAUSE_FRAMES: usize = 30;
const MIN_SPEECH_FRAMES: usize = 8;
const LONGEST: usize = RATE * 12;
/// A long utterance is cut at the quietest frame of its last stretch, so words stay whole.
const LOOK_BACK: usize = RATE * 3 / 2 / FRAME;
/// The noise floor rises to the quietest frame of this stretch, so steady music becomes background.
const NOISE_WINDOW: usize = RATE * 3 / 2 / FRAME;
/// Quieter than this is never speech, whatever the noise floor.
const FLOOR: f32 = 0.004;

fn level(frame: &[f32]) -> f32 {
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// 16 kHz mono with at least one second of sound above silence.
pub fn has_speech(samples: &[f32]) -> bool {
    let (frames, _) = samples.as_chunks::<FRAME>();
    let loud = frames.iter().filter(|f| level(*f) > FLOOR).count();
    loud * FRAME >= RATE
}

/// Samples that end at `end` samples into the stream.
#[derive(Debug, PartialEq)]
struct Cut {
    samples: Vec<f32>,
    end: usize,
    is_final: bool,
}

/// Energy-based: speech is what stands clearly above a slowly adapting noise floor.
struct Cutter {
    pending: Vec<f32>,
    before: VecDeque<f32>,
    speech: Vec<f32>,
    noise: f32,
    /// Levels of the latest frames, up to `NOISE_WINDOW`.
    recent: VecDeque<f32>,
    quiet: usize,
    voiced: usize,
    partial_every: Option<usize>,
    since_partial: usize,
    /// Samples seen in total.
    position: usize,
}

impl Cutter {
    fn new(partial_every: Option<Duration>) -> Self {
        Self {
            pending: Vec::new(),
            before: VecDeque::new(),
            speech: Vec::new(),
            noise: FLOOR / 2.0,
            recent: VecDeque::new(),
            quiet: 0,
            voiced: 0,
            partial_every: partial_every.map(|d| (d.as_secs_f64() * RATE as f64) as usize),
            since_partial: 0,
            position: 0,
        }
    }

    fn push(&mut self, samples: &[f32]) -> Vec<Cut> {
        self.pending.extend_from_slice(samples);
        let mut cuts = Vec::new();
        let whole = self.pending.len() / FRAME * FRAME;
        let frames: Vec<f32> = self.pending.drain(..whole).collect();
        for frame in frames.chunks(FRAME) {
            cuts.extend(self.frame(frame));
        }
        cuts
    }

    fn frame(&mut self, frame: &[f32]) -> Option<Cut> {
        self.position += FRAME;
        let level = level(frame);
        let loud = level > (self.noise * 3.0).max(FLOOR);
        if !loud {
            self.noise += (level - self.noise) * 0.05;
        }
        self.recent.push_back(level);
        if self.recent.len() > NOISE_WINDOW {
            self.recent.pop_front();
            let quietest = self.recent.iter().copied().fold(f32::MAX, f32::min);
            self.noise = self.noise.max(quietest);
        }
        if self.speech.is_empty() {
            if !loud {
                self.before.extend(frame);
                let excess = self.before.len().saturating_sub(PRE_ROLL);
                self.before.drain(..excess);
                return None;
            }
            self.speech.extend(self.before.drain(..));
        }
        self.speech.extend_from_slice(frame);
        self.since_partial += FRAME;
        self.voiced += usize::from(loud);
        self.quiet = if loud { 0 } else { self.quiet + 1 };
        if self.quiet >= PAUSE_FRAMES {
            return self.finish();
        }
        if self.speech.len() >= LONGEST {
            return self.cut_long();
        }
        if self.voiced >= MIN_SPEECH_FRAMES
            && self.partial_every.is_some_and(|n| self.since_partial >= n)
        {
            self.since_partial = 0;
            return Some(Cut {
                samples: self.speech.clone(),
                end: self.position,
                is_final: false,
            });
        }
        None
    }

    fn finish(&mut self) -> Option<Cut> {
        let speech = std::mem::take(&mut self.speech);
        let voiced = std::mem::take(&mut self.voiced);
        self.quiet = 0;
        self.since_partial = 0;
        (voiced >= MIN_SPEECH_FRAMES).then_some(Cut {
            samples: speech,
            end: self.position,
            is_final: true,
        })
    }

    /// Ends the utterance after its quietest recent frame; what follows starts the next one.
    fn cut_long(&mut self) -> Option<Cut> {
        let frames = self.speech.len() / FRAME;
        let from = frames.saturating_sub(LOOK_BACK);
        let quietest = (from..frames)
            .min_by(|a, b| {
                let level = |i: usize| level(&self.speech[i * FRAME..(i + 1) * FRAME]);
                level(*a).total_cmp(&level(*b))
            })
            .unwrap_or(frames - 1);
        let rest = self.speech.split_off((quietest + 1) * FRAME);
        let threshold = (self.noise * 3.0).max(FLOOR);
        let rest_voiced = rest.chunks(FRAME).filter(|f| level(f) > threshold).count();
        let end = self.position - rest.len();
        let quiet = self.quiet.min(rest.len() / FRAME);
        self.voiced = self.voiced.saturating_sub(rest_voiced);
        let mut cut = self.finish();
        if let Some(cut) = &mut cut {
            cut.end = end;
        }
        self.speech = rest;
        self.voiced = rest_voiced;
        self.quiet = quiet;
        cut
    }
}

pub struct ChunkedSession {
    cutter: Cutter,
    jobs: Option<Sender<Cut>>,
    worker: Option<JoinHandle<()>>,
}

impl ChunkedSession {
    /// `partial_every: None` asks the engine only about finished utterances.
    pub fn new(
        recognizer: impl Recognizer,
        out: SttOutput,
        partial_every: Option<Duration>,
    ) -> Self {
        let (jobs, queue) = channel();
        let worker = std::thread::spawn(move || work(recognizer, &queue, &out));
        Self {
            cutter: Cutter::new(partial_every),
            jobs: Some(jobs),
            worker: Some(worker),
        }
    }

    fn send(&self, cut: Cut) -> Result<()> {
        self.jobs
            .as_ref()
            .and_then(|j| j.send(cut).ok())
            .ok_or_else(|| Error::Provider("recognizer stopped".into()))
    }
}

fn work(mut recognizer: impl Recognizer, queue: &Receiver<Cut>, out: &SttOutput) {
    while let Ok(mut job) = queue.recv() {
        // An engine slower than real time skips partials but never a finished utterance.
        while !job.is_final {
            match queue.try_recv() {
                Ok(next) => job = next,
                Err(_) => break,
            }
        }
        let seconds = |n: usize| Duration::from_secs_f64(n as f64 / RATE as f64);
        let result = recognizer
            .recognize(&job.samples, job.is_final)
            .map(|h| SttUpdate {
                text: h.text.trim().to_owned(),
                language: h.language,
                start: seconds(job.end.saturating_sub(job.samples.len())),
                end: seconds(job.end),
                is_final: job.is_final,
            });
        out(result);
    }
}

impl SttSession for ChunkedSession {
    fn push(&mut self, chunk: &AudioChunk) -> Result<()> {
        for cut in self.cutter.push(&chunk.samples) {
            self.send(cut)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if let Some(cut) = self.cutter.finish() {
            self.send(cut)?;
        }
        self.jobs = None;
        match self.worker.take().map(JoinHandle::join) {
            Some(Err(_)) => Err(Error::Provider("recognizer crashed".into())),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn voice(ms: usize) -> Vec<f32> {
        (0..RATE * ms / 1000)
            .map(|i| (i as f32 * 0.2).sin() * 0.3)
            .collect()
    }
    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0005; RATE * ms / 1000]
    }
    /// Voice with a short dip between words, every 350 ms.
    fn words(ms: usize) -> Vec<f32> {
        let word = RATE * 35 / 100;
        (0..RATE * ms / 1000)
            .map(|i| {
                if i % word < word * 4 / 5 {
                    (i as f32 * 0.2).sin() * 0.3
                } else {
                    0.0005
                }
            })
            .collect()
    }

    #[test]
    fn speech_needs_one_loud_second() {
        assert!(has_speech(&[silence(2000), voice(1000)].concat()));
        assert!(!has_speech(&[silence(2000), voice(900)].concat()));
    }
    fn finals(c: &[Cut]) -> usize {
        c.iter().filter(|c| c.is_final).count()
    }

    #[test]
    fn cuts_at_pauses_and_ignores_clicks_and_silence() {
        let mut c = Cutter::new(None);
        let audio = [
            silence(1000),
            voice(1000),
            silence(900),
            voice(40),
            silence(900),
            voice(800),
            silence(900),
        ]
        .concat();
        let cuts: Vec<_> = audio.chunks(777).flat_map(|p| c.push(p)).collect();
        assert_eq!(finals(&cuts), 2);
        assert_eq!(cuts.len(), 2);
    }

    #[test]
    fn keeps_the_start_of_the_first_word() {
        let mut c = Cutter::new(None);
        let cuts = c.push(&[silence(1000), voice(1000), silence(900)].concat());
        let Some(Cut {
            samples: s,
            is_final: true,
            ..
        }) = cuts.first()
        else {
            panic!("no utterance")
        };
        assert!(s.len() > RATE * 13 / 10);
    }

    #[test]
    fn long_speech_is_cut_and_partials_arrive() {
        let mut c = Cutter::new(Some(Duration::from_secs(1)));
        let cuts = c.push(&words(25_000));
        assert_eq!(finals(&cuts), 2);
        assert!(cuts.len() > 10);
    }

    #[test]
    fn long_speech_is_cut_between_words_without_losing_audio() {
        let mut c = Cutter::new(None);
        let audio = words(25_000);
        let cuts = c.push(&audio);
        assert_eq!(finals(&cuts), 2);
        let mut start = 0;
        for cut in &cuts {
            assert!(cut.samples.len() <= LONGEST);
            assert!(cut.samples.len() > LONGEST - LOOK_BACK * FRAME);
            assert_eq!(cut.end - cut.samples.len(), start, "gap or overlap");
            assert_eq!(cut.samples, audio[start..cut.end]);
            let last = &cut.samples[cut.samples.len() - FRAME..];
            assert!(level(last) < FLOOR, "cut inside a word");
            start = cut.end;
        }
        assert_eq!(c.speech, audio[start..c.position]);
    }

    #[test]
    fn pauses_are_found_over_steady_background() {
        let hum = |i: usize| (i as f32 * 0.05).sin() * 0.028;
        let mut audio: Vec<f32> = (0..RATE * 3).map(hum).collect();
        let mut burst_ends = Vec::new();
        for _ in 0..3 {
            for (gap, ms) in [(false, 1000), (true, 900)] {
                let from = audio.len();
                audio.extend((from..from + RATE * ms / 1000).map(|i| {
                    hum(i)
                        + if gap {
                            0.0
                        } else {
                            (i as f32 * 0.2).sin() * 0.3
                        }
                }));
                if !gap {
                    burst_ends.push(audio.len());
                }
            }
        }
        let mut c = Cutter::new(None);
        let cuts: Vec<_> = audio.chunks(777).flat_map(|p| c.push(p)).collect();
        for b in burst_ends {
            assert!(
                cuts.iter()
                    .any(|c| c.is_final && c.end > b && c.end <= b + RATE * 9 / 10),
                "no final in the pause after {b}: {:?}",
                cuts.iter().map(|c| c.end).collect::<Vec<_>>()
            );
        }
    }

    struct Slow;
    impl Recognizer for Slow {
        fn recognize(&mut self, s: &[f32], _: bool) -> Result<Heard> {
            std::thread::sleep(Duration::from_millis(30));
            Ok(Heard {
                text: s.len().to_string(),
                language: None,
            })
        }
    }

    #[test]
    fn a_slow_engine_drops_partials_but_never_finals() {
        let seen: Arc<Mutex<Vec<SttUpdate>>> = Arc::default();
        let log = seen.clone();
        let mut s = ChunkedSession::new(
            Slow,
            Arc::new(move |u| log.lock().unwrap().push(u.unwrap())),
            Some(Duration::from_millis(200)),
        );
        for _ in 0..5 {
            for samples in [voice(1500), silence(900)] {
                s.push(&AudioChunk {
                    format: AudioFormat::SPEECH,
                    samples,
                    at: Duration::ZERO,
                    segment: None,
                })
                .unwrap();
            }
        }
        s.finish().unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().filter(|u| u.is_final).count(), 5);
        assert!(seen.len() < 20);
        assert!(seen.windows(2).all(|w| w[0].end <= w[1].end));
    }
}
