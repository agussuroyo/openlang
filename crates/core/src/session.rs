use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    Error, Result,
    domain::*,
    graph::{Graph, GraphBuilder, NodeId, Reporter},
    nodes::*,
    ports::*,
    registry::{Registry, check_consent},
};

#[derive(Debug, Clone)]
pub struct Choice {
    pub id: String,
    pub options: Options,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub sources: Vec<String>,
    /// In order of preference.
    pub stt: Vec<Choice>,
    pub translators: Vec<Choice>,
    pub source_language: Option<String>,
    pub target_language: String,
    pub allow_network: bool,
    pub partial_interval: Duration,
}

/// Where an extension can connect.
pub struct Taps {
    pub session: SessionId,
    pub origin: Origin,
    pub audio: NodeId,
    pub transcript: NodeId,
    pub translation: NodeId,
    pub caption: NodeId,
}

/// Adds branches, such as spoken output or note taking, without touching the live pipeline.
pub trait Extension: Send + Sync {
    fn attach(&self, graph: &mut GraphBuilder, taps: &Taps) -> Result<()>;
}

struct Lane {
    graph: Graph,
    pump: JoinHandle<()>,
}

pub struct Session {
    pub id: SessionId,
    stop: Arc<AtomicBool>,
    lanes: Vec<Lane>,
    sinks: Vec<Arc<dyn EventSink>>,
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn broadcast(sinks: &[Arc<dyn EventSink>], session: SessionId, body: EventBody) {
    let event = SessionEvent {
        schema: EVENT_SCHEMA,
        session,
        body,
    };
    for s in sinks {
        // A sink that cannot store a status event must not stop the translation.
        let _ = s.accept(&event);
    }
}

impl Session {
    pub fn start(
        registry: &Registry,
        plan: &Plan,
        sinks: Vec<Arc<dyn EventSink>>,
        extensions: &[Arc<dyn Extension>],
    ) -> Result<Self> {
        if plan.sources.is_empty() {
            return Err(Error::Config("no audio source selected".into()));
        }
        let resampler = registry
            .resampler
            .clone()
            .ok_or_else(|| Error::Config("no resampler installed".into()))?;
        let job = |c: &Choice| Job {
            source: plan.source_language.clone(),
            target: plan.target_language.clone(),
            options: c.options.clone(),
        };
        let mut stt = Vec::new();
        for c in &plan.stt {
            let p = registry.stt(&c.id)?.clone();
            check_consent(p.as_ref(), &c.options, plan.allow_network)?;
            stt.push((p, job(c)));
        }
        let mut translators = Vec::new();
        for c in &plan.translators {
            let p = registry.translator(&c.id)?.clone();
            check_consent(p.as_ref(), &c.options, plan.allow_network)?;
            translators.push((p, job(c)));
        }
        let sources = plan
            .sources
            .iter()
            .map(|s| registry.source(s).cloned())
            .collect::<Result<Vec<_>>>()?;

        let mut session = Self {
            id: SessionId::new(),
            stop: Arc::new(AtomicBool::new(false)),
            lanes: Vec::new(),
            sinks,
        };
        broadcast(
            &session.sinks,
            session.id,
            EventBody::Started(SessionInfo {
                started_at_unix_ms: unix_ms(),
                origins: sources.iter().map(|s| s.origin()).collect(),
                source_language: plan.source_language.clone(),
                target_language: plan.target_language.clone(),
                stt: plan.stt.iter().map(|c| c.id.clone()).collect(),
                translators: plan.translators.iter().map(|c| c.id.clone()).collect(),
            }),
        );
        // One clock for all lanes keeps the conversation in order.
        let clock = Instant::now();
        for source in sources {
            let origin = source.origin();
            let mut b = GraphBuilder::default();
            let audio = b.root(Box::new(TransformNode::new(
                resampler.create(AudioFormat::SPEECH),
            )))?;
            let transcript = b.add(
                Box::new(SttNode::new(stt.clone(), registry.detector.clone(), origin)),
                &[audio],
            )?;
            let translation = b.add(
                Box::new(TranslateNode::new(
                    translators.clone(),
                    plan.target_language.clone(),
                    plan.partial_interval,
                )),
                &[transcript],
            )?;
            let caption = b.add(Box::new(StabilizeNode::default()), &[translation])?;
            for sink in &session.sinks {
                b.add(
                    Box::new(SinkNode::new(sink.clone(), session.id)),
                    &[transcript, translation, caption],
                )?;
            }
            let taps = Taps {
                session: session.id,
                origin,
                audio,
                transcript,
                translation,
                caption,
            };
            for e in extensions {
                e.attach(&mut b, &taps)?;
            }
            let lane = session.open_lane(b, source, clock);
            match lane {
                Ok(lane) => session.lanes.push(lane),
                Err(e) => {
                    session.stop();
                    return Err(e);
                }
            }
        }
        Ok(session)
    }

    fn open_lane(
        &self,
        builder: GraphBuilder,
        source: Arc<dyn AudioSource>,
        clock: Instant,
    ) -> Result<Lane> {
        let (sinks, id) = (self.sinks.clone(), self.id);
        let reporter: Reporter = Arc::new(move |node, error| {
            broadcast(
                &sinks,
                id,
                EventBody::Failed {
                    node: node.into(),
                    error,
                },
            );
        });
        let opened = source.open()?;
        let graph = builder.start(reporter)?;
        let pump = Pump {
            source,
            opened: Some(opened),
            stop: self.stop.clone(),
            sinks: self.sinks.clone(),
            session: id,
            clock,
            silent_for: Duration::ZERO,
            silent: false,
            level: 0.0,
        };
        let input = graph.input();
        let pump = std::thread::spawn(move || pump.run(&input));
        Ok(Lane { graph, pump })
    }

    /// Waits for the last spoken words to be translated and delivered, but never longer than
    /// `STOP_LIMIT` per source.
    pub fn stop(self) {
        self.stop.store(true, SeqCst);
        for lane in self.lanes {
            let _ = lane.pump.join();
            lane.graph.finish_within(STOP_LIMIT);
        }
        broadcast(
            &self.sinks,
            self.id,
            EventBody::Ended {
                ended_at_unix_ms: unix_ms(),
            },
        );
    }
}

const STOP_LIMIT: Duration = Duration::from_secs(6);
const TICK: Duration = Duration::from_millis(50);
const SILENCE_AFTER: Duration = Duration::from_secs(8);
const RETRY: Duration = Duration::from_secs(1);

struct Pump {
    source: Arc<dyn AudioSource>,
    opened: Option<(crate::realtime::AudioReader, Box<dyn Capture>)>,
    stop: Arc<AtomicBool>,
    sinks: Vec<Arc<dyn EventSink>>,
    session: SessionId,
    clock: Instant,
    silent_for: Duration,
    silent: bool,
    level: f32,
}

/// Loudness of a chunk from 0 to 1: RMS on a 60 dB scale, so speech fills most of it.
pub fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let decibels = 10.0 * power.max(1e-12).log10();
    ((decibels + 60.0) / 60.0).clamp(0.0, 1.0)
}

impl Pump {
    fn send(&self, body: EventBody) {
        broadcast(&self.sinks, self.session, body);
    }

    fn run(mut self, input: &crate::graph::Emitter) {
        while !self.stop.load(SeqCst) {
            if self.opened.as_ref().is_some_and(|(r, _)| r.is_invalid()) {
                self.drain(input);
                self.opened = None;
            }
            if self.opened.is_none() {
                match self.source.open() {
                    Ok(o) => self.opened = Some(o),
                    Err(error) => {
                        self.send(EventBody::Failed {
                            node: "source".into(),
                            error,
                        });
                        self.sleep(RETRY);
                        continue;
                    }
                }
            }
            self.sleep(TICK);
            self.drain(input);
        }
        self.drain(input);
    }

    fn sleep(&self, d: Duration) {
        let until = Instant::now() + d;
        while !self.stop.load(SeqCst) && Instant::now() < until {
            std::thread::sleep(TICK.min(until.saturating_duration_since(Instant::now())));
        }
    }

    fn drain(&mut self, input: &crate::graph::Emitter) {
        let Some((reader, _)) = &mut self.opened else {
            return;
        };
        let samples = reader.read(usize::MAX);
        let (format, lost) = (reader.format, reader.take_lost());
        let origin = self.source.origin();
        if lost > 0 {
            self.send(EventBody::Overrun {
                origin,
                lost_samples: lost,
            });
        }
        if samples.is_empty() {
            return;
        }
        let length = Duration::from_secs_f64(
            samples.len() as f64 / f64::from(format.channels) / f64::from(format.sample_rate),
        );
        if samples.iter().all(|s| *s == 0.0) {
            self.silent_for += length;
        } else {
            self.silent_for = Duration::ZERO;
        }
        let silent = self.silent_for >= SILENCE_AFTER;
        if silent != self.silent {
            self.silent = silent;
            self.send(EventBody::Silent { origin, silent });
        }
        // Quiet after quiet sends nothing, so an idle session stays idle.
        let level = loudness(&samples);
        if level > 0.0 || self.level > 0.0 {
            self.level = level;
            self.send(EventBody::Level { origin, level });
        }
        let at = self.clock.elapsed().saturating_sub(length);
        input.emit(Packet::Audio(Arc::new(AudioChunk {
            format,
            samples,
            at,
            segment: None,
        })));
    }
}
