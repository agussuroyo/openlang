use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Bump when a serialized event changes shape, so stored history stays readable.
pub const EVENT_SCHEMA: u32 = 2;

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);

        impl $name {
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
    };
}
id!(SessionId);
id!(SegmentId);

/// Which side of the conversation the audio came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    System,
    Microphone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub const SPEECH: Self = Self {
        sample_rate: 16_000,
        channels: 1,
    };
}

/// Interleaved PCM. Used for captured audio and, with `segment` set, for synthesized speech.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    pub format: AudioFormat,
    pub samples: Vec<f32>,
    /// Offset from the start of the session.
    pub at: Duration,
    pub segment: Option<SegmentId>,
}

/// Identity and timing shared by everything derived from one stretch of speech.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub revision: u32,
    pub origin: Origin,
    pub start: Duration,
    pub end: Duration,
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub segment: Segment,
    pub language: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Translation {
    pub segment: Segment,
    pub source: String,
    pub language: String,
    pub text: String,
}

/// A translation plus how many leading words are no longer expected to change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub translation: Translation,
    pub stable_words: usize,
}

#[derive(Debug, Clone)]
pub enum Packet {
    Audio(Arc<AudioChunk>),
    Transcript(Arc<Transcript>),
    Translation(Arc<Translation>),
    Caption(Arc<Caption>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    Audio,
    Transcript,
    Translation,
    Caption,
}

impl Packet {
    pub fn kind(&self) -> PacketKind {
        match self {
            Self::Audio(_) => PacketKind::Audio,
            Self::Transcript(_) => PacketKind::Transcript,
            Self::Translation(_) => PacketKind::Translation,
            Self::Caption(_) => PacketKind::Caption,
        }
    }

    pub fn segment(&self) -> Option<&Segment> {
        match self {
            Self::Audio(_) => None,
            Self::Transcript(p) => Some(&p.segment),
            Self::Translation(p) => Some(&p.segment),
            Self::Caption(p) => Some(&p.translation.segment),
        }
    }

    /// True when a newer revision of the same segment makes this one worthless.
    pub fn is_partial(&self) -> bool {
        self.segment().is_some_and(|s| !s.is_final)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub schema: u32,
    pub session: SessionId,
    pub body: EventBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum EventBody {
    Started(SessionInfo),
    Transcript(Transcript),
    Translation(Translation),
    Caption(Caption),
    /// No sample above digital zero for a while. Normal when nothing is playing.
    Silent {
        origin: Origin,
        silent: bool,
    },
    /// How loud the source is right now, 0 (quiet) to 1, sent while there is sound.
    Level {
        origin: Origin,
        level: f32,
    },
    Overrun {
        origin: Origin,
        lost_samples: u64,
    },
    Failed {
        node: String,
        error: crate::Error,
    },
    /// Sent after every final segment has been delivered.
    Ended {
        ended_at_unix_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub started_at_unix_ms: u64,
    pub origins: Vec<Origin>,
    pub source_language: Option<String>,
    pub target_language: String,
    pub stt: Vec<String>,
    pub translators: Vec<String>,
}
