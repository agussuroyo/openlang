use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use openlang_core::{
    Error, Result,
    domain::*,
    graph::{Emitter, Graph, GraphBuilder, Node},
    nodes::{SttNode, TranslateNode},
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

#[derive(Clone, Copy)]
enum Speech {
    /// Emits partials as audio arrives and the final text only when the session ends.
    Normal,
    /// The first chunk ends one utterance with an empty final, then a silent one, then starts
    /// another.
    EmptyFinals,
    /// Pushes 1, 2 and 4 fail.
    FlakyPush,
}

struct FakeStt {
    base: Base,
    fail: bool,
    speech: Speech,
}
provider!(FakeStt);
impl SpeechToText for FakeStt {
    fn open(&self, _: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        if self.fail {
            return Err(Error::Provider("down".into()));
        }
        Ok(Box::new(FakeSttSession {
            out,
            speech: self.speech,
            pushes: 0,
        }))
    }
}
struct FakeSttSession {
    out: SttOutput,
    speech: Speech,
    pushes: u32,
}
impl FakeSttSession {
    fn say(&self, text: &str, is_final: bool) {
        (self.out)(Ok(SttUpdate {
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
        self.pushes += 1;
        match self.speech {
            Speech::Normal => self.say("hello", false),
            Speech::EmptyFinals if self.pushes == 1 => {
                self.say("hello", false);
                self.say(" ", true);
                self.say("", true);
                self.say("again", false);
            }
            Speech::EmptyFinals => {}
            Speech::FlakyPush if matches!(self.pushes, 1 | 2 | 4) => {
                return Err(Error::Provider("bad chunk".into()));
            }
            Speech::FlakyPush => {}
        }
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
    for (id, fail, speech) in [
        ("stt", false, Speech::Normal),
        ("bad", true, Speech::Normal),
    ] {
        r.stt.push(Arc::new(FakeStt {
            base: base(id, Location::OnDevice),
            fail,
            speech,
        }));
    }
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

#[test]
fn loudness_ignores_room_noise_and_fills_with_loud_sound() {
    use openlang_core::session::loudness;
    assert_eq!(loudness(&[]), 0.0);
    assert_eq!(loudness(&[0.0; 160]), 0.0);
    assert_eq!(loudness(&[1.0, -1.0, 1.0, -1.0]), 1.0);
    assert_eq!(loudness(&[0.002, -0.002]), 0.0, "-54 dB room noise");
    let speech = loudness(&[0.1, -0.1, 0.1, -0.1]);
    assert!((speech - 0.75).abs() < 1e-3, "-20 dB speech: {speech}");
}

/// Forwards every packet it receives to a channel, with the time it arrived.
struct Collect(mpsc::Sender<(Packet, Instant)>, &'static [PacketKind]);
impl Node for Collect {
    fn name(&self) -> String {
        "collect".into()
    }
    fn accepts(&self) -> &'static [PacketKind] {
        self.1
    }
    fn emits(&self) -> &'static [PacketKind] {
        &[]
    }
    fn handle(&mut self, p: Packet) -> Result<()> {
        let _ = self.0.send((p, Instant::now()));
        Ok(())
    }
}

fn job() -> Job {
    Job {
        source: Some("en-US".into()),
        target: "id".into(),
        options: Options::default(),
    }
}

/// Runs an `SttNode` alone on `chunks` audio chunks. Returns the reported nodes and transcripts.
fn recognize(speech: Speech, chunks: usize) -> (Vec<String>, Vec<Transcript>) {
    let stt: Arc<dyn SpeechToText> = Arc::new(FakeStt {
        base: Base {
            d: descriptor("fake"),
            location: Location::OnDevice,
        },
        fail: false,
        speech,
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let (tx, rx) = mpsc::channel();
    let mut g = GraphBuilder::default();
    let node = SttNode::new(vec![(stt, job())], None, Origin::System);
    let root = g.root(Box::new(node)).unwrap();
    g.add(Box::new(Collect(tx, &[PacketKind::Transcript])), &[root])
        .unwrap();
    let graph = g
        .start(Arc::new(move |n, _| log.lock().unwrap().push(n.to_owned())))
        .unwrap();
    for _ in 0..chunks {
        graph.input().emit(Packet::Audio(Arc::new(AudioChunk {
            format: AudioFormat::SPEECH,
            samples: vec![0.1; 160],
            at: Duration::ZERO,
            segment: None,
        })));
    }
    graph.finish();
    let transcripts = rx
        .try_iter()
        .filter_map(|(p, _)| match p {
            Packet::Transcript(t) => Some((*t).clone()),
            _ => None,
        })
        .collect();
    let seen = seen.lock().unwrap().clone();
    (seen, transcripts)
}

#[test]
fn an_empty_final_closes_the_shown_line_and_the_next_one_starts_fresh() {
    let (_, t) = recognize(Speech::EmptyFinals, 1);
    let texts: Vec<_> = t
        .iter()
        .map(|t| (t.text.as_str(), t.segment.is_final))
        .collect();
    assert_eq!(
        texts,
        [
            ("hello", false),
            ("hello", true),
            ("again", false),
            ("hello world", true)
        ]
    );
    assert_eq!(t[0].segment.id, t[1].segment.id);
    assert_ne!(t[1].segment.id, t[2].segment.id);
    assert_eq!(t[2].segment.id, t[3].segment.id);
}

#[test]
fn a_failed_push_is_reported_once_and_recognition_goes_on() {
    let (seen, t) = recognize(Speech::FlakyPush, 5);
    assert_eq!(seen, ["stt", "stt"]);
    assert_eq!(t.last().map(|t| t.text.as_str()), Some("hello world"));
}

fn transcript(text: &str, is_final: bool) -> Packet {
    transcript_of(SegmentId::new(), text, is_final)
}

fn transcript_of(id: SegmentId, text: &str, is_final: bool) -> Packet {
    Packet::Transcript(Arc::new(Transcript {
        segment: Segment {
            id,
            revision: 0,
            origin: Origin::System,
            start: Duration::ZERO,
            end: Duration::ZERO,
            is_final,
            speaker: None,
        },
        language: "en-US".into(),
        text: text.into(),
    }))
}

#[test]
fn a_final_does_not_hold_back_the_next_partial() {
    let good: Arc<dyn Translator> = registry().translator("good").unwrap().clone();
    let (tx, rx) = mpsc::channel();
    let mut g = GraphBuilder::default();
    let node = TranslateNode::new(vec![(good, job())], "id".into(), Duration::from_secs(5));
    let root = g.root(Box::new(node)).unwrap();
    g.add(Box::new(Collect(tx, &[PacketKind::Translation])), &[root])
        .unwrap();
    let graph = g.start(Arc::new(|_, _| {})).unwrap();
    let t = Instant::now();
    graph.input().emit(transcript("one", true));
    graph.input().emit(transcript("two", false));
    let got: Vec<_> = (0..2)
        .map_while(|_| rx.recv_timeout(Duration::from_secs(1)).ok())
        .collect();
    assert_eq!(got.len(), 2);
    assert!(got[1].1.duration_since(t) < Duration::from_secs(1));
    graph.finish();
}

/// Takes 400 ms for unfinished speech, which here is any text ending in "...".
struct SlowOnPartials {
    base: Base,
}
provider!(SlowOnPartials);
impl Translator for SlowOnPartials {
    fn open(&self, _: &Job) -> Result<Box<dyn TranslatorSession>> {
        Ok(Box::new(SlowSession))
    }
}
struct SlowSession;
impl TranslatorSession for SlowSession {
    fn translate(&mut self, text: &str, _: &str) -> Result<String> {
        if text.ends_with("...") {
            std::thread::sleep(Duration::from_millis(400));
        }
        Ok(format!("slow:{text}"))
    }
}

/// A `TranslateNode` with a translator that is slow on partials, and what it emits.
fn translate_slowly() -> (Graph, mpsc::Receiver<(Packet, Instant)>) {
    let slow: Arc<dyn Translator> = Arc::new(SlowOnPartials {
        base: Base {
            d: descriptor("slow"),
            location: Location::OnDevice,
        },
    });
    let (tx, rx) = mpsc::channel();
    let mut g = GraphBuilder::default();
    let node = TranslateNode::new(vec![(slow, job())], "id".into(), Duration::ZERO);
    let root = g.root(Box::new(node)).unwrap();
    g.add(Box::new(Collect(tx, &[PacketKind::Translation])), &[root])
        .unwrap();
    (g.start(Arc::new(|_, _| {})).unwrap(), rx)
}

#[test]
fn a_final_is_not_held_back_by_a_slow_partial() {
    let (graph, rx) = translate_slowly();
    graph.input().emit(transcript("one...", false));
    std::thread::sleep(Duration::from_millis(50));
    let t = Instant::now();
    graph.input().emit(transcript("two", true));
    let (got, at) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let Packet::Translation(got) = got else {
        panic!("not a translation")
    };
    assert_eq!(got.text, "slow:two");
    assert!(at.duration_since(t) < Duration::from_millis(200));
    graph.finish();
}

#[test]
fn a_late_partial_never_replaces_its_final() {
    let (graph, rx) = translate_slowly();
    let id = SegmentId::new();
    graph.input().emit(transcript_of(id, "one...", false));
    std::thread::sleep(Duration::from_millis(50));
    graph.input().emit(transcript_of(id, "one two", true));
    std::thread::sleep(Duration::from_millis(600));
    let texts: Vec<String> = rx
        .try_iter()
        .filter_map(|(p, _)| match p {
            Packet::Translation(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts.last().map(String::as_str),
        Some("slow:one two"),
        "{texts:?}"
    );
    graph.finish();
}

/// Answers at once, and remembers every text it was asked.
struct Recording {
    base: Base,
    asked: Arc<Mutex<Vec<String>>>,
}
provider!(Recording);
impl Translator for Recording {
    fn open(&self, _: &Job) -> Result<Box<dyn TranslatorSession>> {
        Ok(Box::new(RecordingSession(self.asked.clone())))
    }
}
struct RecordingSession(Arc<Mutex<Vec<String>>>);
impl TranslatorSession for RecordingSession {
    fn translate(&mut self, text: &str, _: &str) -> Result<String> {
        self.0.lock().unwrap().push(text.to_owned());
        Ok(text.to_owned())
    }
}

#[test]
fn a_fast_translator_translates_a_final_whole_once_it_is_known_to_be_fast() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let fast: Arc<dyn Translator> = Arc::new(Recording {
        base: Base {
            d: descriptor("fast"),
            location: Location::OnDevice,
        },
        asked: asked.clone(),
    });
    let mut g = GraphBuilder::default();
    let node = TranslateNode::new(vec![(fast, job())], "id".into(), Duration::ZERO);
    g.root(Box::new(node)).unwrap();
    let graph = g.start(Arc::new(|_, _| {})).unwrap();
    let say = |partial: &str, end: &str| {
        let id = SegmentId::new();
        graph.input().emit(transcript_of(id, partial, false));
        std::thread::sleep(Duration::from_millis(50));
        asked.lock().unwrap().clear();
        graph.input().emit(transcript_of(id, end, true));
        std::thread::sleep(Duration::from_millis(50));
        asked.lock().unwrap().clone()
    };
    // Its speed is not known yet: the final reuses the clause its partial translated.
    assert_eq!(say("Hello, th", "Hello, there"), ["there"]);
    assert_eq!(say("Bye, no", "Bye, now"), ["Bye, now"]);
    graph.finish();
}

#[test]
fn a_failed_final_translation_closes_with_the_original_text() {
    let rec = Arc::new(Recorder::default());
    run(plan(&["stt"], &["bad"]), vec![rec.clone()], &[]);
    assert_eq!(rec.finals()[0].text, "hello world");
    let failures = rec
        .events()
        .iter()
        .filter(|e| matches!(e, EventBody::Failed { .. }))
        .count();
    assert_eq!(failures, 1, "the failed partial is not reported");
}
