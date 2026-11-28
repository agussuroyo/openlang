//! Thin graph nodes around the ports.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        mpsc::{Sender, channel},
    },
    time::{Duration, Instant},
};

use crate::{
    Error, Result,
    auto_language::{AutoLanguage, needs_detector},
    clauses::Clauses,
    domain::*,
    graph::{Emitter, Node, QueuePolicy, lock},
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

/// Translators in order of preference: a failing one hands the text to the next.
struct TranslatorChain {
    providers: Vec<(Arc<dyn Translator>, Job)>,
    target: String,
    active: usize,
    session: Option<Box<dyn TranslatorSession>>,
}

impl TranslatorChain {
    fn new(providers: Vec<(Arc<dyn Translator>, Job)>, target: String) -> Self {
        Self {
            providers,
            target,
            active: 0,
            session: None,
        }
    }

    /// The same translators with sessions of their own.
    fn twin(&self) -> Self {
        Self::new(self.providers.clone(), self.target.clone())
    }

    /// Opening early lets a provider load its model before the first word. A failure is not
    /// final: it is tried again, with fallbacks, on the first text.
    fn warm_up(&mut self) {
        if let Some((provider, job)) = self.providers.first()
            && let Some(source) = &job.source
            && primary(source) != primary(&self.target)
        {
            self.session = provider.open(job).ok();
        }
    }

    /// Every failure goes to `report`, when there is one.
    fn translate(&mut self, text: &str, source: &str, report: Option<&Emitter>) -> Result<String> {
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
                    if let Some(out) = report {
                        out.report(e.clone());
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

fn translation(t: &Transcript, language: &str, text: String) -> Packet {
    Packet::Translation(Arc::new(Translation {
        segment: t.segment.clone(),
        source: t.text.clone(),
        language: language.to_owned(),
        text,
    }))
}

/// Segments whose final the partial worker must not replace, and whether it may still emit.
#[derive(Default)]
struct Delivery {
    ended: VecDeque<SegmentId>,
    /// Finals emitted so far.
    ended_total: usize,
    closed: bool,
    /// Finals being translated. Translation services answer one request at a time, so the
    /// worker waits for these.
    finals: usize,
}

/// Ended segments remembered. A partial that took longer than this many finals is dropped, since
/// its own segment may be forgotten by then.
const ENDED_KEPT: usize = 16;

/// Translates unfinished speech on its own thread with its own translators, so a final never
/// waits for a partial. A busy worker skips to the newest partial.
struct PartialWorker {
    jobs: Sender<Arc<Transcript>>,
    delivery: Arc<(Mutex<Delivery>, Condvar)>,
}

impl PartialWorker {
    fn start(mut chain: TranslatorChain, clauses: Arc<Clauses>, out: Emitter) -> Self {
        let (jobs, queue) = channel::<Arc<Transcript>>();
        let delivery = Arc::new((Mutex::new(Delivery::default()), Condvar::new()));
        let shared = delivery.clone();
        std::thread::spawn(move || {
            chain.warm_up();
            let target = chain.target.clone();
            while let Ok(mut t) = queue.recv() {
                let (state, finals_done) = &*shared;
                let began_after = finals_done
                    .wait_while(lock(state), |d| d.finals > 0 && !d.closed)
                    .unwrap_or_else(PoisonError::into_inner)
                    .ended_total;
                while let Ok(newer) = queue.try_recv() {
                    t = newer;
                }
                // Not reported: the final retries and reports it.
                let translate = |clause: &str| chain.translate(clause, &t.language, None);
                let id = t.segment.id;
                let Ok(text) = clauses.translate(id, &t.text, &target, false, translate) else {
                    continue;
                };
                let delivery = lock(state);
                if delivery.closed {
                    return;
                }
                let late = delivery.ended_total - began_after >= ENDED_KEPT;
                if !late && !delivery.ended.contains(&t.segment.id) {
                    out.emit(translation(&t, &target, text));
                }
            }
        });
        Self { jobs, delivery }
    }

    fn send(&self, partial: Arc<Transcript>) {
        let _ = self.jobs.send(partial);
    }

    /// A final is being translated: the worker starts nothing new until it ends.
    fn final_started(&self) {
        lock(&self.delivery.0).finals += 1;
    }

    /// Called before the final of `segment` is emitted, so no partial can follow it.
    fn final_ended(&self, segment: SegmentId) {
        let mut delivery = lock(&self.delivery.0);
        delivery.finals -= 1;
        delivery.ended_total += 1;
        delivery.ended.push_back(segment);
        if delivery.ended.len() > ENDED_KEPT {
            delivery.ended.pop_front();
        }
        self.delivery.1.notify_all();
    }

    /// Nothing is emitted once this returns. A translation still running ends on its own.
    fn close(&self) {
        lock(&self.delivery.0).closed = true;
        self.delivery.1.notify_all();
    }
}

/// A translator that answers this fast translates a final whole, for the best wording. A slower
/// one reuses the clauses of its partials instead (see `Clauses`).
const FAST_CALL: Duration = Duration::from_millis(200);

/// Translates partials clause by clause, and finals whole or from clauses by the translator's
/// speed.
pub struct TranslateNode {
    finals: TranslatorChain,
    /// How long the last final translation call took.
    last_call: Duration,
    clauses: Arc<Clauses>,
    partials: Option<PartialWorker>,
    interval: Duration,
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
            finals: TranslatorChain::new(providers, target),
            last_call: Duration::MAX,
            clauses: Arc::default(),
            partials: None,
            interval,
            next: None,
            out: None,
        }
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
        self.finals.warm_up();
        let (chain, clauses) = (self.finals.twin(), self.clauses.clone());
        self.partials = Some(PartialWorker::start(chain, clauses, out.clone()));
        self.out = Some(out);
        Ok(())
    }
    fn handle(&mut self, packet: Packet) -> Result<()> {
        let (Packet::Transcript(t), Some(partials)) = (packet, &self.partials) else {
            return Ok(());
        };
        if !t.segment.is_final {
            self.next = Some(Instant::now() + self.interval);
            partials.send(t);
            return Ok(());
        }
        // A new segment's first partial is not held back by the previous one.
        self.next = None;
        let out = started(&self.out)?;
        partials.final_started();
        let target = self.finals.target.clone();
        let whole = self.last_call < FAST_CALL;
        let (finals, last_call) = (&mut self.finals, &mut self.last_call);
        let mut translate = |text: &str| {
            let began = Instant::now();
            let translated = finals.translate(text, &t.language, Some(out));
            *last_call = began.elapsed();
            translated
        };
        let translated = if whole {
            translate(&t.text)
        } else {
            let id = t.segment.id;
            self.clauses
                .translate(id, &t.text, &target, true, translate)
        };
        // Already reported. The original text closes the segment, so no stale partial
        // translation stays on screen.
        let text = translated.unwrap_or_else(|_| t.text.clone());
        partials.final_ended(t.segment.id);
        out.emit(translation(&t, &target, text));
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        if let Some(partials) = &self.partials {
            partials.close();
        }
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
