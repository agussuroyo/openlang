//! Thin graph nodes around the ports.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use crate::{
    Error, Result,
    auto_language::{AutoLanguage, needs_detector},
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
    /// Text of the last partial, to close the segment when its final comes back empty.
    last: String,
}

impl Segmenter {
    /// An empty final ends the utterance: a shown partial becomes final, otherwise nothing shows.
    fn close(&mut self, u: SttUpdate) -> Option<Transcript> {
        if self.revision == 0 {
            return None;
        }
        let text = std::mem::take(&mut self.last);
        Some(self.transcript(SttUpdate { text, ..u }))
    }

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
            self.last.clear();
        } else {
            self.last.clone_from(&t.text);
        }
        t
    }
}

pub struct SttNode {
    providers: Vec<(Arc<dyn SpeechToText>, Job)>,
    detector: Option<Arc<dyn LanguageDetector>>,
    segmenter: Arc<Mutex<Segmenter>>,
    session: Option<Box<dyn SttSession>>,
    out: Option<Emitter>,
    /// The last push failed and was reported, so the same trouble is not reported again.
    push_failed: bool,
}

impl SttNode {
    /// Providers are tried in order until one opens. `detector` serves those that cannot detect.
    pub fn new(
        providers: Vec<(Arc<dyn SpeechToText>, Job)>,
        detector: Option<Arc<dyn LanguageDetector>>,
        origin: Origin,
    ) -> Self {
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
            last: String::new(),
        };
        Self {
            providers,
            detector,
            segmenter: Arc::new(Mutex::new(segmenter)),
            session: None,
            out: None,
            push_failed: false,
        }
    }

    fn open(
        &self,
        provider: &Arc<dyn SpeechToText>,
        job: &Job,
        out: SttOutput,
    ) -> Result<Box<dyn SttSession>> {
        let Some(detector) = self
            .detector
            .as_deref()
            .filter(|_| needs_detector(provider.as_ref(), job))
        else {
            return provider.open(job, out);
        };
        let auto = AutoLanguage::open(provider.clone(), job.clone(), detector, out)?;
        Ok(Box::new(auto))
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
        self.out = Some(out.clone());
        let segmenter = self.segmenter.clone();
        let emit = out.clone();
        let output: SttOutput = Arc::new(move |update| match update {
            Ok(u) if u.text.trim().is_empty() && !u.is_final => {}
            Ok(u) => {
                let mut s = segmenter.lock().unwrap_or_else(PoisonError::into_inner);
                let transcript = if u.text.trim().is_empty() {
                    s.close(u)
                } else {
                    Some(s.transcript(u))
                };
                if let Some(t) = transcript {
                    emit.emit(Packet::Transcript(Arc::new(t)));
                }
            }
            Err(e) => emit.report(e),
        });
        let mut last = Error::Config("no speech recognizer selected".into());
        for (provider, job) in &self.providers {
            match self.open(provider, job, output.clone()) {
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
        // One bad chunk must not end recognition for the rest of the session.
        match session.push(&chunk) {
            Ok(()) => self.push_failed = false,
            Err(e) if !self.push_failed => {
                self.push_failed = true;
                started(&self.out)?.report(e);
            }
            Err(_) => {}
        }
        Ok(())
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

    /// Failures of unfinished speech are not reported: the final retries and reports them.
    fn translate(&mut self, text: &str, source: &str, report: bool) -> Result<String> {
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
                    if report {
                        started(&self.out)?.report(e.clone());
                    }
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
        let is_final = t.segment.is_final;
        // A new segment's first partial is not held back by the previous one.
        self.next = (!is_final).then(|| Instant::now() + self.interval);
        let text = match self.translate(&t.text, &t.language, is_final) {
            Ok(text) => text,
            // Already reported. The original text closes the segment, so no stale partial
            // translation stays on screen.
            Err(_) if is_final => t.text.clone(),
            // The original text is still on screen, so keep going.
            Err(_) => return Ok(()),
        };
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
