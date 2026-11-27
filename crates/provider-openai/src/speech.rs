//! Speech to text. Finished utterances only, because every request may cost money.

use openlang_core::{Result, ports::*};
use openlang_speech_chunks::{ChunkedSession, Heard, Recognizer};
use serde_json::Value;

use crate::client::Client;
use crate::server::{Api, Server};
use crate::services::SpeechApi;

impl Api for SpeechApi {
    fn model_field(&self) -> Option<&'static str> {
        match self {
            SpeechApi::Whisper => Some(""),
            SpeechApi::OpenAi { .. } | SpeechApi::DeepSeek => None,
        }
    }
    fn can_detect(&self) -> bool {
        match self {
            SpeechApi::Whisper | SpeechApi::OpenAi { .. } => true,
            // It hears every language but never says which one.
            SpeechApi::DeepSeek => false,
        }
    }
}

impl SpeechToText for Server<SpeechApi> {
    fn open(&self, job: &Job, out: SttOutput) -> Result<Box<dyn SttSession>> {
        let model = match self.api.model_field() {
            Some(_) => self.model(&job.options)?,
            None => String::new(),
        };
        let listen = Listen {
            client: self.client(&job.options)?,
            api: self.api,
            model,
            language: job
                .source
                .as_deref()
                .map(|t| openlang_core::nodes::primary(t).to_owned()),
        };
        Ok(Box::new(ChunkedSession::new(listen, out, None)))
    }
}

struct Listen {
    client: Client,
    api: SpeechApi,
    model: String,
    language: Option<String>,
}

impl Recognizer for Listen {
    fn recognize(&mut self, samples: &[f32]) -> Result<Heard> {
        let wav = wav(samples);
        match self.api {
            SpeechApi::Whisper => self.whisper(&wav),
            SpeechApi::OpenAi { model } => self.open_ai(model, &wav),
            SpeechApi::DeepSeek => self.deep_seek(&wav),
        }
    }
}

impl Listen {
    fn whisper(&self, wav: &[u8]) -> Result<Heard> {
        let mut fields = vec![("model", self.model.as_str())];
        match &self.language {
            Some(language) => fields.push(("language", language)),
            None => fields.push(("response_format", "verbose_json")),
        }
        let reply = self.client.upload("audio/transcriptions", &fields, wav)?;
        Ok(Heard {
            text: crate::text(&reply["text"])?,
            language: detected(&reply["language"]),
        })
    }

    fn open_ai(&self, model: &str, wav: &[u8]) -> Result<Heard> {
        let reply = self
            .client
            .upload("audio/transcriptions", &[("model", model)], wav)?;
        Ok(Heard {
            text: crate::text(&reply["text"])?,
            language: detected(&reply["languages"][0]["code"]),
        })
    }

    fn deep_seek(&self, wav: &[u8]) -> Result<Heard> {
        let reply = self.client.upload("v1/tools/asr", &[], wav)?;
        Ok(Heard {
            text: crate::text(&reply["text"])?,
            language: None,
        })
    }
}

/// Servers answer with a code or an English name; only a code is usable.
fn detected(v: &Value) -> Option<String> {
    const NAMES: &[(&str, &str)] = &[
        ("arabic", "ar"),
        ("chinese", "zh"),
        ("dutch", "nl"),
        ("english", "en"),
        ("french", "fr"),
        ("german", "de"),
        ("hindi", "hi"),
        ("indonesian", "id"),
        ("italian", "it"),
        ("japanese", "ja"),
        ("korean", "ko"),
        ("malay", "ms"),
        ("portuguese", "pt"),
        ("russian", "ru"),
        ("spanish", "es"),
        ("thai", "th"),
        ("turkish", "tr"),
        ("vietnamese", "vi"),
    ];
    let name = v.as_str()?.to_lowercase();
    if name.len() == 2 {
        return Some(name);
    }
    NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, code)| (*code).to_owned())
}

/// 16-bit mono 16 kHz.
fn wav(samples: &[f32]) -> Vec<u8> {
    let bytes = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    for v in [1u16, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    for v in [2u16, 16] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes());
    }
    out
}
