use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use openlang_core::{
    Error, Result,
    domain::*,
    graph::{Emitter, GraphBuilder, Node},
    ports::*,
    realtime::{self, AudioReader},
    registry::Registry,
    session::{Choice, Extension, Plan, Session, Taps},
};

fn descriptor(id: &'static str) -> Descriptor {
    Descriptor {
        id,
        name: id,
        summary: "",
        fields: vec![],
    }
}

struct Base {
    d: Descriptor,
    location: Location,
}

macro_rules! provider {
    ($t:ty) => {
        impl Provider for $t {
            fn descriptor(&self) -> &Descriptor {
                &self.base.d
            }
            fn location(&self, _: &Options) -> Location {
                self.base.location
            }
            fn languages(&self) -> Languages {
                Languages {
                    tags: vec![],
                    can_detect: true,
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

/// Emits partials as audio arrives and the final text only when the session ends.
struct FakeStt {
    base: Base,
    fail: bool,
}
provider!(FakeStt);
impl SpeechToText for FakeStt {
    fn open(&self, _: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        if self.fail {
            return Err(Error::Provider("down".into()));
        }
        Ok(Box::new(FakeSttSession(out)))
    }
}
struct FakeSttSession(SttOutput);
impl FakeSttSession {
    fn say(&self, text: &str, is_final: bool) {
        (self.0)(Ok(SttUpdate {
            text: text.into(),
            language: None,
            start: Duration::ZERO,
            end: Duration::from_secs(1),
            is_final,
        }));
    }
}
impl SttSession for FakeSttSession {
    fn push(&mut self, _: &AudioChunk) -> Result<()> {
        self.say("hello", false);
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.say("hello world", true);
        Ok(())
    }
}

struct FakeTranslator {
    base: Base,
    fail: bool,
}
provider!(FakeTranslator);
impl Translator for FakeTranslator {
    fn open(&self, _: &Job) -> Result<Box<dyn TranslatorSession>> {
        Ok(Box::new(FakeTranslatorSession(self.base.d.id, self.fail)))
    }
}
struct FakeTranslatorSession(&'static str, bool);
impl TranslatorSession for FakeTranslatorSession {
    fn translate(&mut self, text: &str, _: &str) -> Result<String> {
        if self.1 {
            return Err(Error::Provider("down".into()));
        }
        Ok(format!("{}:{text}", self.0))
    }
}

struct FakeSource {
    d: Descriptor,
    origin: Origin,
}
struct Running;
impl Capture for Running {}
impl AudioSource for FakeSource {
    fn descriptor(&self) -> &Descriptor {
        &self.d
    }
    fn origin(&self) -> Origin {
        self.origin
    }
    fn open(&self) -> Result<(AudioReader, Box<dyn Capture>)> {
        let (mut w, r) = realtime::channel(AudioFormat::SPEECH, 1);
        w.write(&[0.5; 1600]);
        Ok((r, Box::new(Running)))
    }
}

struct Passthrough;
impl AudioTransform for Passthrough {
    fn process(&mut self, c: &AudioChunk) -> Result<Option<AudioChunk>> {
        Ok(Some(c.clone()))
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

/// Stands in for a future history module: an in-memory recorder.
#[derive(Default)]
struct Recorder(Mutex<Vec<SessionEvent>>);
impl EventSink for Recorder {
    fn accept(&self, e: &SessionEvent) -> Result<()> {
        self.0.lock().unwrap().push(e.clone());
        Ok(())
    }
}
impl Recorder {
    fn events(&self) -> Vec<EventBody> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.body.clone())
            .collect()
    }
    fn finals(&self) -> Vec<Translation> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                EventBody::Translation(t) if t.segment.is_final => Some(t),
                _ => None,
            })
            .collect()
    }
}

struct Broken;
impl EventSink for Broken {
    fn accept(&self, _: &SessionEvent) -> Result<()> {
        Err(Error::Provider("disk full".into()))
    }
}

fn registry() -> Registry {
    let base = |id, location| Base {
        d: descriptor(id),
        location,
    };
    let mut r = Registry::default();
    r.sources.push(Arc::new(FakeSource {
        d: descriptor("system"),
        origin: Origin::System,
    }));
    r.sources.push(Arc::new(FakeSource {
        d: descriptor("mic"),
        origin: Origin::Microphone,
    }));
    r.stt.push(Arc::new(FakeStt {
        base: base("stt", Location::OnDevice),
        fail: false,
    }));
    r.stt.push(Arc::new(FakeStt {
        base: base("bad", Location::OnDevice),
        fail: true,
    }));
    for (id, location, fail) in [
        ("good", Location::OnDevice, false),
        ("bad", Location::OnDevice, true),
        ("cloud", Location::Network, false),
    ] {
        r.translators.push(Arc::new(FakeTranslator {
            base: base(id, location),
            fail,
        }));
    }
    r.resampler = Some(Arc::new(Passthrough));
    r
}

fn plan(stt: &[&str], translators: &[&str]) -> Plan {
    let choice = |x: &&str| Choice {
        id: x.to_string(),
        options: Options::default(),
    };
    Plan {
        sources: vec!["system".into()],
        stt: stt.iter().map(choice).collect(),
        translators: translators.iter().map(choice).collect(),
        source_language: Some("en-US".into()),
        target_language: "id".into(),
        allow_network: false,
        partial_interval: Duration::from_millis(10),
    }
}

fn run(p: Plan, sinks: Vec<Arc<dyn EventSink>>, ext: &[Arc<dyn Extension>]) {
    let s = Session::start(&registry(), &p, sinks, ext).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    s.stop();
}

#[test]
fn final_words_are_delivered_before_the_session_ends() {
    let rec = Arc::new(Recorder::default());
    run(plan(&["stt"], &["good"]), vec![rec.clone()], &[]);
    assert_eq!(rec.finals()[0].text, "good:hello world");
    assert!(matches!(rec.events().first(), Some(EventBody::Started(_))));
    assert!(matches!(rec.events().last(), Some(EventBody::Ended { .. })));
}

#[test]
fn partial_and_final_share_a_segment() {
    let rec = Arc::new(Recorder::default());
    run(plan(&["stt"], &["good"]), vec![rec.clone()], &[]);
    let ids: Vec<_> = rec
        .events()
        .into_iter()
        .filter_map(|e| match e {
            EventBody::Transcript(t) => Some((t.segment.id, t.segment.revision)),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].0, ids[1].0);
    assert!(ids[0].1 < ids[1].1);
}

#[test]
fn falls_back_to_the_next_provider() {
    let rec = Arc::new(Recorder::default());
    run(
        plan(&["bad", "stt"], &["bad", "good"]),
        vec![rec.clone()],
        &[],
    );
    assert_eq!(rec.finals()[0].text, "good:hello world");
    assert!(
        rec.events()
            .iter()
            .any(|e| matches!(e, EventBody::Failed { .. }))
    );
}

#[test]
fn network_provider_needs_consent() {
    let mut p = plan(&["stt"], &["good", "cloud"]);
    assert!(Session::start(&registry(), &p, vec![], &[]).is_err());
    p.allow_network = true;
    Session::start(&registry(), &p, vec![], &[]).unwrap().stop();
}

#[test]
fn same_language_is_not_translated() {
    let rec = Arc::new(Recorder::default());
    let mut p = plan(&["stt"], &["good"]);
    p.target_language = "en".into();
    run(p, vec![rec.clone()], &[]);
    assert_eq!(rec.finals()[0].text, "hello world");
}

#[test]
fn a_broken_history_sink_does_not_stop_captions() {
    let rec = Arc::new(Recorder::default());
    run(
        plan(&["stt"], &["good"]),
        vec![Arc::new(Broken), rec.clone()],
        &[],
    );
    assert_eq!(rec.finals().len(), 1);
}

#[test]
fn two_sources_share_one_session_and_keep_their_origin() {
    let rec = Arc::new(Recorder::default());
    let mut p = plan(&["stt"], &["good"]);
    p.sources.push("mic".into());
    run(p, vec![rec.clone()], &[]);
    let mut origins: Vec<_> = rec.finals().iter().map(|t| t.segment.origin).collect();
    origins.sort_by_key(|o| *o as u8);
    assert_eq!(origins, [Origin::System, Origin::Microphone]);
}

#[test]
fn events_survive_serialization() {
    let rec = Arc::new(Recorder::default());
    run(plan(&["stt"], &["good"]), vec![rec.clone()], &[]);
    for e in rec.0.lock().unwrap().iter() {
        let json = serde_json::to_string(e).unwrap();
        assert!(json.contains("\"schema\":1"));
        assert_eq!(&serde_json::from_str::<SessionEvent>(&json).unwrap(), e);
    }
}

/// Stands in for future speech synthesis: final translations become audio, slowly.
struct Voice {
    out: Option<Emitter>,
}
impl Node for Voice {
    fn name(&self) -> String {
        "voice".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Translation]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn start(&mut self, out: Emitter) -> Result<()> {
        self.out = Some(out);
        Ok(())
    }
    fn handle(&mut self, p: Packet) -> Result<()> {
        if let (Packet::Translation(t), Some(out)) = (p, &self.out)
            && t.segment.is_final
        {
            std::thread::sleep(Duration::from_millis(300));
            out.emit(Packet::Audio(Arc::new(AudioChunk {
                format: AudioFormat::SPEECH,
                samples: vec![0.1; 160],
                at: t.segment.start,
                segment: Some(t.segment.id),
            })));
        }
        Ok(())
    }
}
struct Speaker(mpsc::Sender<(SegmentId, Instant)>);
impl Node for Speaker {
    fn name(&self) -> String {
        "speaker".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[]
    }
    fn handle(&mut self, p: Packet) -> Result<()> {
        if let Packet::Audio(a) = p {
            self.0.send((a.segment.unwrap(), Instant::now())).unwrap();
        }
        Ok(())
    }
}
struct SpokenOutput(Mutex<mpsc::Sender<(SegmentId, Instant)>>);
impl Extension for SpokenOutput {
    fn attach(&self, g: &mut GraphBuilder, taps: &Taps) -> Result<()> {
        let voice = g.add(Box::new(Voice { out: None }), &[taps.translation])?;
        g.add(Box::new(Speaker(self.0.lock().unwrap().clone())), &[voice])?;
        Ok(())
    }
}
struct CaptionTime(Mutex<Option<(SegmentId, Instant)>>);
impl EventSink for CaptionTime {
    fn accept(&self, e: &SessionEvent) -> Result<()> {
        if let EventBody::Caption(c) = &e.body
            && c.translation.segment.is_final
        {
            *self.0.lock().unwrap() = Some((c.translation.segment.id, Instant::now()));
        }
        Ok(())
    }
}

#[test]
fn spoken_output_can_be_attached_without_delaying_text() {
    let (tx, rx) = mpsc::channel();
    let captions = Arc::new(CaptionTime(Mutex::new(None)));
    run(
        plan(&["stt"], &["good"]),
        vec![captions.clone()],
        &[Arc::new(SpokenOutput(Mutex::new(tx)))],
    );
    let (spoken, spoken_at) = rx.try_recv().unwrap();
    let (shown, shown_at) = captions.0.lock().unwrap().unwrap();
    assert_eq!(spoken, shown);
    assert!(spoken_at.duration_since(shown_at) > Duration::from_millis(200));
}

#[test]
fn incompatible_nodes_are_rejected() {
    let (tx, _rx) = mpsc::channel();
    let mut g = GraphBuilder::default();
    let root = g.root(Box::new(Voice { out: None })).unwrap();
    let err = g.add(Box::new(Voice { out: None }), &[root]);
    assert!(matches!(err, Err(Error::Graph(_))));
    g.add(Box::new(Speaker(tx)), &[root]).unwrap();
}

struct Panics;
impl Node for Panics {
    fn name(&self) -> String {
        "panics".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[]
    }
    fn handle(&mut self, _: Packet) -> Result<()> {
        panic!("boom")
    }
}

#[test]
fn a_panicking_node_is_reported_and_contained() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mut g = GraphBuilder::default();
    g.root(Box::new(Panics)).unwrap();
    let graph = g
        .start(Arc::new(move |n, _| log.lock().unwrap().push(n.to_owned())))
        .unwrap();
    for _ in 0..300 {
        graph.input().emit(Packet::Audio(Arc::new(AudioChunk {
            format: AudioFormat::SPEECH,
            samples: vec![],
            at: Duration::ZERO,
            segment: None,
        })));
    }
    graph.finish();
    assert_eq!(*seen.lock().unwrap(), ["panics"]);
}

struct Stuck;
impl Node for Stuck {
    fn name(&self) -> String {
        "stuck".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        &[PacketKind::Audio]
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[]
    }
    fn handle(&mut self, _: Packet) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        std::thread::sleep(Duration::from_secs(600));
        Ok(())
    }
}

#[test]
fn a_stuck_provider_cannot_block_stop() {
    let mut g = GraphBuilder::default();
    g.root(Box::new(Stuck)).unwrap();
    let graph = g.start(Arc::new(|_, _| {})).unwrap();
    let t = Instant::now();
    assert!(!graph.finish_within(Duration::from_millis(200)));
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn cancel_does_not_wait_for_queued_work() {
    let (tx, rx) = mpsc::channel();
    let mut g = GraphBuilder::default();
    let v = g.root(Box::new(Voice { out: None })).unwrap();
    g.add(Box::new(Speaker(tx)), &[v]).unwrap();
    let graph = g.start(Arc::new(|_, _| {})).unwrap();
    for _ in 0..20 {
        graph
            .input()
            .emit(Packet::Translation(Arc::new(Translation {
                segment: Segment {
                    id: SegmentId::new(),
                    revision: 0,
                    origin: Origin::System,
                    start: Duration::ZERO,
                    end: Duration::ZERO,
                    is_final: true,
                    speaker: None,
                },
                source: String::new(),
                language: "id".into(),
                text: String::new(),
            })));
    }
    let t = Instant::now();
    graph.cancel();
    assert!(t.elapsed() < Duration::from_secs(1));
    assert!(rx.try_iter().count() < 20);
}
