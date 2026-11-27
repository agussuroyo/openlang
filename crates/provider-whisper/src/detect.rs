//! Names the spoken language for recognizers that cannot, such as Apple Speech.

use std::path::PathBuf;

use openlang_core::{Result, ports::*};
use openlang_speech_chunks::has_speech;
use whisper_rs::{WhisperState, get_lang_id, get_lang_str};

use crate::{failed, load, models};

/// The base model tells languages apart well, and Whisper speech shares the file.
const MODEL: &models::Model = &models::ALL[0];
const THREADS: usize = 2;

pub struct WhisperDetector {
    descriptor: Descriptor,
    models: PathBuf,
}

impl WhisperDetector {
    pub fn new(models: PathBuf) -> Self {
        Self {
            descriptor: Descriptor {
                id: "whisper-language",
                name: "Language detection",
                summary: "Finds the spoken language on this computer.",
                fields: vec![],
            },
            models,
        }
    }
}

impl Provider for WhisperDetector {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    fn location(&self, _: &Options) -> Location {
        Location::OnDevice
    }
    fn languages(&self) -> Languages {
        Languages {
            tags: vec![],
            can_detect: true,
        }
    }
    fn readiness(&self, _: &Job) -> Readiness {
        MODEL.readiness(&self.models)
    }
    fn prepare(&self, _: &Job, progress: &dyn Fn(f32)) -> Result<()> {
        MODEL.download(&self.models, progress)
    }
}

impl LanguageDetector for WhisperDetector {
    fn open(&self, allowed: Vec<String>) -> Result<Box<dyn DetectorSession>> {
        self.readiness(&Job::default()).into_result()?;
        Ok(Box::new(Detect {
            state: load(&MODEL.path(&self.models))?,
            allowed: allowed.iter().filter_map(|tag| get_lang_id(tag)).collect(),
        }))
    }
}

struct Detect {
    state: WhisperState,
    allowed: Vec<i32>,
}

impl DetectorSession for Detect {
    fn detect(&mut self, samples: &[f32]) -> Result<Option<Detected>> {
        if !has_speech(samples) {
            return Ok(None);
        }
        self.state.pcm_to_mel(samples, THREADS).map_err(failed)?;
        let (_, chances) = self.state.lang_detect(0, THREADS).map_err(failed)?;
        let best = self
            .allowed
            .iter()
            .filter_map(|&id| Some((id, *chances.get(usize::try_from(id).ok()?)?)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        Ok(best.and_then(|(id, confidence)| {
            Some(Detected {
                language: get_lang_str(id)?.to_owned(),
                confidence,
            })
        }))
    }
}
