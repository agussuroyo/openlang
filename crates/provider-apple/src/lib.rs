//! Apple on-device speech recognition and translation. Free, offline, nothing bundled.
#![cfg(target_os = "macos")]

use std::{sync::Arc, thread::JoinHandle};

use openlang_apple_bridge::{Status, speech, translation};
use openlang_core::{Error, Result, domain::AudioChunk, ports::*, registry::Registry};

pub fn register(registry: &mut Registry) {
    registry.stt.push(Arc::new(AppleSpeech::default()));
    registry
        .translators
        .push(Arc::new(AppleTranslator::default()));
}

pub struct AppleSpeech(Descriptor);

impl Default for AppleSpeech {
    fn default() -> Self {
        Self(Descriptor {
            id: "apple-speech",
            name: "Apple Speech",
            summary: "On this Mac, free, works offline.",
            fields: vec![],
        })
    }
}

fn source(job: &Job) -> Result<&str> {
    job.source.as_deref().ok_or_else(|| {
        Error::Language(
            "Apple Speech cannot detect the language; choose the spoken language".into(),
        )
    })
}

impl Provider for AppleSpeech {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }
    fn location(&self, _: &Options) -> Location {
        Location::OnDevice
    }
    fn languages(&self) -> Languages {
        Languages {
            tags: speech::languages(),
            can_detect: false,
        }
    }
    fn readiness(&self, job: &Job) -> Readiness {
        match source(job).map(|tag| (speech::status(tag), tag)) {
            Ok((Status::Ready, _)) => Readiness::Ready,
            Ok((Status::Downloadable, _)) => Readiness::NeedsDownload { bytes: None },
            Ok((Status::Unsupported, tag)) => unsupported(format!("Apple has no model for {tag}")),
            Err(Error::Language(message)) => unsupported(message),
            Err(e) => unsupported(e.to_string()),
        }
    }
    fn prepare(&self, job: &Job, progress: &dyn Fn(f32)) -> Result<()> {
        speech::install(source(job)?, progress)
    }
}

impl SpeechToText for AppleSpeech {
    fn open(&self, job: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        self.readiness(job).into_result()?;
        Ok(Box::new(Speech(speech::Session::open(source(job)?, out)?)))
    }
}

struct Speech(speech::Session);

impl SttSession for Speech {
    fn push(&mut self, chunk: &AudioChunk) -> Result<()> {
        self.0.push(&chunk.samples)
    }
    fn finish(&mut self) -> Result<()> {
        self.0.finish()
    }
}

pub struct AppleTranslator(Descriptor);

impl Default for AppleTranslator {
    fn default() -> Self {
        Self(Descriptor {
            id: "apple-translate",
            name: "Apple Translate",
            summary: "On this Mac, free, works offline.",
            fields: vec![],
        })
    }
}

impl Provider for AppleTranslator {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }
    fn location(&self, _: &Options) -> Location {
        Location::OnDevice
    }
    fn languages(&self) -> Languages {
        Languages {
            tags: translation::languages(),
            can_detect: false,
        }
    }
    fn readiness(&self, job: &Job) -> Readiness {
        // With automatic detection the pair is known only once someone speaks.
        let Some(source) = &job.source else {
            return Readiness::Ready;
        };
        match translation::status(source, &job.target) {
            Status::Ready => Readiness::Ready,
            Status::Downloadable => Readiness::NeedsAction {
                message: "Download this language in System Settings > General > Language & Region > Translation Languages.".into(),
            },
            Status::Unsupported => {
                unsupported(format!("Apple cannot translate {source} to {}", job.target))
            }
        }
    }
    fn prepare(&self, job: &Job, _: &dyn Fn(f32)) -> Result<()> {
        self.readiness(job).into_result()
    }
}

impl Translator for AppleTranslator {
    fn open(&self, job: &Job) -> Result<Box<dyn TranslatorSession>> {
        self.readiness(job).into_result()?;
        let (source, target) = (source(job)?.to_owned(), job.target.clone());
        Ok(Box::new(Translation {
            warm: warm_up(&source, &target),
            source,
            target,
            session: None,
        }))
    }
}

/// The first translation takes seconds while the system loads its model. Doing it before anyone
/// speaks keeps the first caption fast.
fn warm_up(source: &str, target: &str) -> Option<JoinHandle<translation::Session>> {
    let mut session = translation::Session::open(source, target);
    Some(std::thread::spawn(move || {
        // A failure here shows up again, with its message, on the first real sentence.
        let _ = session.translate("OK");
        session
    }))
}

/// Reopens when the spoken language changes, which happens with a detecting recognizer.
struct Translation {
    source: String,
    target: String,
    warm: Option<JoinHandle<translation::Session>>,
    session: Option<translation::Session>,
}

impl TranslatorSession for Translation {
    fn translate(&mut self, text: &str, source: &str) -> Result<String> {
        if let Some(warm) = self.warm.take() {
            self.session = warm.join().ok();
        }
        if self.source != source {
            self.source = source.to_owned();
            self.session = None;
        }
        self.session
            .get_or_insert_with(|| translation::Session::open(&self.source, &self.target))
            .translate(text)
    }
}
