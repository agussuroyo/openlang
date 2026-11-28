//! Whisper running locally. Detects the language by itself. Models are downloaded on first use.

mod detect;
mod models;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use openlang_core::{Error, Result, ports::*, registry::Registry};
use openlang_speech_chunks::{ChunkedSession, Heard, Recognizer};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    get_lang_max_id, get_lang_str,
};

pub fn register(registry: &mut Registry, models: PathBuf) {
    openlang_models::remove_stale_parts(&models, 0);
    whisper_rs::install_logging_hooks();
    registry.stt.push(Arc::new(Whisper {
        descriptor: Descriptor {
            id: "whisper",
            name: "Whisper",
            summary: "On this computer, free, detects the language. Heavier than Apple.",
            fields: vec![Field {
                key: "model",
                label: "Model",
                kind: FieldKind::Choice {
                    choices: models::ALL
                        .iter()
                        .map(|m| Choice {
                            value: m.id,
                            label: m.label,
                        })
                        .collect(),
                },
                default: models::ALL[0].id,
            }],
        },
        models: models.clone(),
    }));
    registry.detector = Some(Arc::new(detect::WhisperDetector::new(models)));
}

struct Whisper {
    descriptor: Descriptor,
    models: PathBuf,
}

impl Whisper {
    fn model(&self, options: &Options) -> Result<&'static models::Model> {
        models::find(self.descriptor.option(options, "model"))
    }
}

impl Provider for Whisper {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    fn location(&self, _: &Options) -> Location {
        Location::OnDevice
    }
    fn languages(&self) -> Languages {
        Languages {
            tags: (0..=get_lang_max_id())
                .filter_map(get_lang_str)
                .map(str::to_owned)
                .collect(),
            can_detect: true,
        }
    }
    fn readiness(&self, job: &Job) -> Readiness {
        self.model(&job.options).map_or_else(
            |e| unsupported(e.to_string()),
            |m| m.readiness(&self.models),
        )
    }
    fn prepare(&self, job: &Job, progress: &dyn Fn(f32)) -> Result<()> {
        self.model(&job.options)?.download(&self.models, progress)
    }
}

fn failed(e: whisper_rs::WhisperError) -> Error {
    Error::Provider(format!("whisper: {e}"))
}

fn load(model: &Path) -> Result<WhisperState> {
    openlang_core::cpu::check()?;
    let mut params = WhisperContextParameters::default();
    params.flash_attn(true);
    WhisperContext::new_with_params(model, params)
        .map_err(failed)?
        .create_state()
        .map_err(failed)
}

impl SpeechToText for Whisper {
    fn open(&self, job: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        self.readiness(job).into_result()?;
        let state = load(&self.model(&job.options)?.path(&self.models))?;
        let language = job
            .source
            .as_deref()
            .map(|tag| openlang_core::nodes::primary(tag).to_owned());
        Ok(Box::new(ChunkedSession::new(
            Listen {
                state,
                language,
                heard: None,
            },
            out,
            Some(Duration::from_millis(1200)),
        )))
    }
}

struct Listen {
    state: WhisperState,
    language: Option<String>,
    /// Detected for the current utterance, when no language is fixed.
    heard: Option<String>,
}

/// Whisper rejects anything shorter than a second.
const SHORTEST: usize = 17_600;
/// Enough sound to trust the detected language for the rest of the utterance.
const DETECT_AFTER: usize = 16_000 * 3;

impl Recognizer for Listen {
    fn recognize(&mut self, samples: &[f32], is_final: bool) -> Result<Heard> {
        let heard_enough = samples.len() >= DETECT_AFTER;
        let mut padded;
        let samples = if samples.len() < SHORTEST {
            padded = samples.to_vec();
            padded.resize(SHORTEST, 0.0);
            &padded
        } else {
            samples
        };
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        // Detecting runs the encoder twice, and partials of one utterance must not switch language.
        let known = self.language.clone().or_else(|| self.heard.clone());
        p.set_language(Some(known.as_deref().unwrap_or("auto")));
        p.set_n_threads(4);
        p.set_no_context(true);
        p.set_suppress_nst(true);
        p.set_temperature_inc(0.0);
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        self.state.full(p, samples).map_err(failed)?;
        // Whisper invents text for noise; it also says when it thinks nobody spoke.
        let text = self
            .state
            .as_iter()
            .filter(|s| s.no_speech_probability() < 0.6)
            .map(|s| s.to_string())
            .collect();
        let language = match (&self.language, known) {
            (Some(_), _) => None,
            (None, Some(heard)) => Some(heard),
            (None, None) => get_lang_str(self.state.full_lang_id_from_state()).map(str::to_owned),
        };
        if is_final {
            self.heard = None;
        } else if heard_enough {
            self.heard.clone_from(&language);
        }
        Ok(Heard { text, language })
    }
}
