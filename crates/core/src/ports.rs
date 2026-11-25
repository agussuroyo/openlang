//! What the outside world must implement. Each trait is one small responsibility.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use crate::{
    Error, Result,
    domain::{AudioChunk, AudioFormat, Origin, SessionEvent},
    realtime::AudioReader,
};

pub type Options = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    OnDevice,
    /// Data may leave this computer.
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Descriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub fields: Vec<Field>,
}

/// One setting a provider needs. The settings screen is drawn from these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub default: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Secret,
    Choice { choices: Vec<Choice> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

impl Descriptor {
    /// The user's value for a field, or its default when empty.
    pub fn option<'a>(&'a self, options: &'a Options, key: &str) -> &'a str {
        options
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| {
                self.fields
                    .iter()
                    .find(|f| f.key == key)
                    .map_or("", |f| f.default)
            })
    }
}

/// What the user asked a provider to do.
#[derive(Debug, Clone, Default)]
pub struct Job {
    /// `None` asks the provider to detect the language.
    pub source: Option<String>,
    pub target: String,
    pub options: Options,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    NeedsDownload {
        bytes: Option<u64>,
    },
    /// The user has to do something the app cannot do for them.
    NeedsAction {
        message: String,
    },
    Unsupported {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Languages {
    /// BCP 47 tags.
    pub tags: Vec<String>,
    pub can_detect: bool,
}

pub trait Provider: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    /// Depends on the options: the same client may talk to localhost or to a cloud.
    fn location(&self, options: &Options) -> Location;
    fn languages(&self) -> Languages;
    fn readiness(&self, job: &Job) -> Readiness;
    /// Downloads whatever `readiness` asked for. Progress runs from 0 to 1.
    fn prepare(&self, job: &Job, progress: &dyn Fn(f32)) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct SttUpdate {
    pub text: String,
    /// Detected language; `None` means the one that was asked for.
    pub language: Option<String>,
    /// Offsets from the first pushed sample.
    pub start: Duration,
    pub end: Duration,
    pub is_final: bool,
}

pub type SttOutput = Arc<dyn Fn(Result<SttUpdate>) + Send + Sync>;

pub trait SpeechToText: Provider {
    fn open(&self, job: &Job, out: SttOutput) -> Result<Box<dyn SttSession>>;
}

pub trait SttSession: Send {
    fn push(&mut self, chunk: &AudioChunk) -> Result<()>;
    /// Returns after the last final update was delivered.
    fn finish(&mut self) -> Result<()>;
}

pub trait Translator: Provider {
    fn open(&self, job: &Job) -> Result<Box<dyn TranslatorSession>>;
}

pub trait TranslatorSession: Send {
    fn translate(&mut self, text: &str, source: &str) -> Result<String>;
}

pub trait AudioSource: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn origin(&self) -> Origin;
    /// Capture runs until the returned handle is dropped.
    fn open(&self) -> Result<(AudioReader, Box<dyn Capture>)>;
}

/// Keeps a capture alive; dropping it stops the capture.
pub trait Capture: Send {}

pub trait AudioTransform: Send {
    fn process(&mut self, chunk: &AudioChunk) -> Result<Option<AudioChunk>>;
    fn flush(&mut self) -> Result<Option<AudioChunk>>;
}

pub trait TransformFactory: Send + Sync {
    /// Converts any input format to `output`.
    fn create(&self, output: AudioFormat) -> Box<dyn AudioTransform>;
}

/// Receives everything that happens in a session: the screen today, history later.
pub trait EventSink: Send + Sync {
    fn accept(&self, event: &SessionEvent) -> Result<()>;
}

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
}

/// Shared by providers whose model list is fixed.
pub fn unsupported(message: impl Into<String>) -> Readiness {
    Readiness::Unsupported {
        message: message.into(),
    }
}

impl Readiness {
    pub fn into_result(self) -> Result<()> {
        match self {
            Self::Ready => Ok(()),
            Self::NeedsDownload { .. } => Err(Error::ModelMissing("download it first".into())),
            Self::NeedsAction { message } => Err(Error::ModelMissing(message)),
            Self::Unsupported { message } => Err(Error::Language(message)),
        }
    }
}
