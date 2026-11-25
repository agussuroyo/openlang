//! Thin graph nodes around the ports.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use crate::{
    Error, Result,
    domain::*,
    graph::{Emitter, Node, QueuePolicy},
    ports::*,
    stabilizer::Stabilizer,
};

fn started(out: &Option<Emitter>) -> Result<&Emitter> {
    out.as_ref()
        .ok_or_else(|| Error::Graph("node used before start".into()))
}

pub struct TransformNode {
    transform: Box<dyn AudioTransform>,
    out: Option<Emitter>,
}

impl TransformNode {
    pub fn new(transform: Box<dyn AudioTransform>) -> Self {
        Self {
            transform,
            out: None,
        }
    }

    fn send(&self, chunk: Option<AudioChunk>) -> Result<()> {
        if let Some(c) = chunk {
            started(&self.out)?.emit(Packet::Audio(Arc::new(c)));
        }
        Ok(())
    }
}

impl Node for TransformNode {
    fn name(&self) -> String {
        "transform".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn start(&mut self, out: Emitter) -> Result<()> {
        self.out = Some(out);
        Ok(())
    }
    fn handle(&mut self, packet: Packet) -> Result<()> {
        if let Packet::Audio(chunk) = packet {
            let converted = self.transform.process(&chunk)?;
            self.send(converted)?;
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        let rest = self.transform.flush()?;
        self.send(rest)
    }
}

/// Gives provider updates a stable identity, so no provider has to manage segments itself.
struct Segmenter {
    id: SegmentId,
    revision: u32,
    origin: Origin,
    language: String,
    offset: Option<Duration>,
}

impl Segmenter {
    fn transcript(&mut self, u: SttUpdate) -> Transcript {
        let offset = self.offset.unwrap_or_default();
        let t = Transcript {
            segment: Segment {
                id: self.id,
                revision: self.revision,
                origin: self.origin,
                start: offset + u.start,
                end: offset + u.end,
                is_final: u.is_final,
                speaker: None,
            },
            language: u.language.unwrap_or_else(|| self.language.clone()),
            text: u.text,
        };
        self.revision += 1;
        if u.is_final {
            self.id = SegmentId::new();
            self.revision = 0;
        }
        t
    }
}

pub struct SttNode {
    providers: Vec<(Arc<dyn SpeechToText>, Job)>,
    segmenter: Arc<Mutex<Segmenter>>,
    session: Option<Box<dyn SttSession>>,
}

impl SttNode {
    /// Providers are tried in order until one opens.
    pub fn new(providers: Vec<(Arc<dyn SpeechToText>, Job)>, origin: Origin) -> Self {
        let language = providers
            .first()
            .and_then(|(_, j)| j.source.clone())
            .unwrap_or_else(|| "und".into());
        let segmenter = Segmenter {
            id: SegmentId::new(),
            revision: 0,
            origin,
            language,
            offset: None,
        };
        Self {
            providers,
            segmenter: Arc::new(Mutex::new(segmenter)),
            session: None,
        }
    }
}

impl Node for SttNode {
    fn name(&self) -> String {
        "stt".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[PacketKind::Transcript]
    }
    fn start(&mut self, out: Emitter) -> Result<()> {
        let segmenter = self.segmenter.clone();
        let emit = out.clone();
        let output: SttOutput = Arc::new(move |update| match update {
            Ok(u) if u.text.trim().is_empty() => {}
            Ok(u) => {
                let mut s = segmenter.lock().unwrap_or_else(PoisonError::into_inner);
                let transcript = s.transcript(u);
                emit.emit(Packet::Transcript(Arc::new(transcript)));
            }
            Err(e) => emit.report(e),
        });
        let mut last = Error::Config("no speech recognizer selected".into());
        for (provider, job) in &self.providers {
            match provider.open(job, output.clone()) {
                Ok(session) => {
                    self.session = Some(session);
                    return Ok(());
                }
                Err(e) => {
                    out.report(e.clone());
                    last = e;
                }
            }
        }
        Err(last)
    }
    fn handle(&mut self, packet: Packet) -> Result<()> {
        let (Packet::Audio(chunk), Some(session)) = (packet, &mut self.session) else {
            return Ok(());
        };
        self.segmenter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .offset
            .get_or_insert(chunk.at);
        session.push(&chunk)
    }
    fn finish(&mut self) -> Result<()> {
        self.session.take().map_or(Ok(()), |mut s| s.finish())
    }
}

pub struct TranslateNode {
    providers: Vec<(Arc<dyn Translator>, Job)>,
    target: String,
    interval: Duration,
    active: usize,
    session: Option<Box<dyn TranslatorSession>>,
    next: Option<Instant>,
    out: Option<Emitter>,
}

impl TranslateNode {
    /// `interval` is the shortest time between two translations of unfinished speech.
    pub fn new(
        providers: Vec<(Arc<dyn Translator>, Job)>,
        target: String,
        interval: Duration,
    ) -> Self {
        Self {
            providers,
            target,
            interval,
            active: 0,
            session: None,
            next: None,
            out: None,
        }
    }

    fn translate(&mut self, text: &str, source: &str) -> Result<String> {
        if primary(source) == primary(&self.target) {
            return Ok(text.to_owned());
        }
        let mut last = Error::Config("no translator selected".into());
        for i in self.active..self.providers.len() {
            let mut attempt = || {
                if self.session.is_none() || self.active != i {
                    let (provider, job) = &self.providers[i];
                    let job = Job {
                        source: Some(source.to_owned()),
                        ..job.clone()
                    };
                    self.session = Some(provider.open(&job)?);
                    self.active = i;
                }
                match &mut self.session {
                    Some(s) => s.translate(text, source),
                    None => Err(Error::Graph("translator session missing".into())),
                }
            };
            match attempt() {
                Ok(t) => return Ok(t),
                Err(e) => {
                    self.session = None;
                    started(&self.out)?.report(e.clone());
                    last = e;
                }
            }
        }
        // Start from the preferred provider again next time.
        self.active = 0;
        Err(last)
    }
}

/// `en-US` and `en` are the same language for translation purposes.
pub fn primary(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

impl Node for TranslateNode {
    fn name(&self) -> String {
        "translate".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Transcript]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[PacketKind::Translation]
    }
    fn queue(&self) -> QueuePolicy {
        QueuePolicy::Coalesce
    }
    fn ready_at(&self) -> Option<Instant> {
        self.next
    }
    fn start(&mut self, out: Emitter) -> Result<()> {
        self.out = Some(out);
        // Opening early lets a provider load its model before the first word. A failure is not
        // final: it is tried again, with fallbacks, on the first transcript.
        if let Some((provider, job)) = self.providers.first()
            && let Some(source) = &job.source
            && primary(source) != primary(&self.target)
        {
            self.session = provider.open(job).ok();
        }
        Ok(())
    }
    fn handle(&mut self, packet: Packet) -> Result<()> {
        let Packet::Transcript(t) = packet else {
            return Ok(());
        };
        let translated = self.translate(&t.text, &t.language);
        self.next = Some(Instant::now() + self.interval);
        // Already reported; the original text is still on screen, so keep going.
        let Ok(text) = translated else { return Ok(()) };
        started(&self.out)?.emit(Packet::Translation(Arc::new(Translation {
            segment: t.segment.clone(),
            source: t.text.clone(),
            language: self.target.clone(),
            text,
        })));
        Ok(())
    }
}

#[derive(Default)]
pub struct StabilizeNode {
    stabilizer: Stabilizer,
    out: Option<Emitter>,
}

impl Node for StabilizeNode {
    fn name(&self) -> String {
        "stabilize".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Translation]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[PacketKind::Caption]
    }
    fn start(&mut self, out: Emitter) -> Result<()> {
        self.out = Some(out);
        Ok(())
    }
    fn handle(&mut self, packet: Packet) -> Result<()> {
        if let Packet::Translation(t) = packet {
            let caption = self.stabilizer.apply((*t).clone());
            started(&self.out)?.emit(Packet::Caption(Arc::new(caption)));
        }
        Ok(())
    }
}

pub struct SinkNode {
    sink: Arc<dyn EventSink>,
    session: SessionId,
}

impl SinkNode {
    pub fn new(sink: Arc<dyn EventSink>, session: SessionId) -> Self {
        Self { sink, session }
    }
}

impl Node for SinkNode {
    fn name(&self) -> String {
        "sink".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[
            PacketKind::Transcript,
            PacketKind::Translation,
            PacketKind::Caption,
        ]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[]
    }
    fn queue(&self) -> QueuePolicy {
        QueuePolicy::Coalesce
    }
    fn handle(&mut self, packet: Packet) -> Result<()> {
        let body = match packet {
            Packet::Audio(_) => return Ok(()),
            Packet::Transcript(p) => EventBody::Transcript((*p).clone()),
            Packet::Translation(p) => EventBody::Translation((*p).clone()),
            Packet::Caption(p) => EventBody::Caption((*p).clone()),
        };
        self.sink.accept(&SessionEvent {
            schema: EVENT_SCHEMA,
            session: self.session,
            body,
        })
    }
}
