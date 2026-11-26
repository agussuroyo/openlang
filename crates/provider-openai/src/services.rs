//! Every service this crate knows. Adding one is one entry in `SERVICES`.

#[derive(Debug, Clone, Copy)]
pub enum Address {
    /// The user may change it, such as a server on this computer.
    Editable(&'static str),
    /// A cloud service with its own address. It needs an API key.
    Fixed(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Translate {
    pub model: &'static str,
    /// A JSON object added to every chat request, such as a switch that turns thinking off.
    pub extra: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub enum SpeechApi {
    /// `audio/transcriptions` of a Whisper-like server. The user types the model.
    Whisper,
    /// OpenAI `gpt-transcribe`: detected languages come back in `languages`.
    OpenAi { model: &'static str },
    /// DeepSeek `v1/tools/asr`: only the audio file, no language in or out.
    DeepSeek,
}

#[derive(Debug, Clone, Copy)]
pub struct Service {
    /// The speech and translate entries of one service share it, and so share one API key.
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub address: Address,
    pub translate: Option<Translate>,
    pub speech: Option<SpeechApi>,
}

const LOCAL_SUMMARY: &str = "Ollama, LM Studio, or a cloud with your own key.";

pub const SERVICES: &[Service] = &[
    Service {
        id: "openai-speech",
        name: "OpenAI-compatible",
        summary: LOCAL_SUMMARY,
        address: Address::Editable("http://localhost:8000/v1"),
        translate: None,
        speech: Some(SpeechApi::Whisper),
    },
    Service {
        id: "openai-translate",
        name: "OpenAI-compatible",
        summary: LOCAL_SUMMARY,
        address: Address::Editable("http://localhost:11434/v1"),
        translate: Some(Translate {
            model: "",
            extra: r#"{"temperature": 0}"#,
        }),
        speech: None,
    },
    Service {
        id: "deepseek",
        name: "DeepSeek",
        summary: "DeepSeek cloud with your own key. Hears speech and translates.",
        address: Address::Fixed("https://api.deepseek.com"),
        // Thinking is on by default and too slow for live captions.
        translate: Some(Translate {
            model: "deepseek-flash",
            extra: r#"{"thinking": {"type": "disabled"}}"#,
        }),
        speech: Some(SpeechApi::DeepSeek),
    },
    Service {
        id: "openai",
        name: "OpenAI",
        summary: "OpenAI cloud with your own key. Hears speech and translates.",
        address: Address::Fixed("https://api.openai.com/v1"),
        translate: Some(Translate {
            model: "gpt-6-luna",
            extra: r#"{"reasoning_effort": "none"}"#,
        }),
        speech: Some(SpeechApi::OpenAi {
            model: "gpt-transcribe",
        }),
    },
    Service {
        id: "openrouter",
        name: "OpenRouter",
        summary: "Many models with one key.",
        address: Address::Fixed("https://openrouter.ai/api/v1"),
        translate: Some(Translate {
            model: "deepseek/deepseek-v4.1-flash",
            extra: r#"{"temperature": 0, "reasoning": {"enabled": false}}"#,
        }),
        speech: None,
    },
    // Kimi K3 and GLM-5.3 always think and refuse a changed temperature, so they think less.
    Service {
        id: "kimi",
        name: "Kimi",
        summary: "Moonshot AI cloud with your own key.",
        address: Address::Fixed("https://api.moonshot.ai/v1"),
        translate: Some(Translate {
            model: "kimi-k3",
            extra: r#"{"reasoning_effort": "low"}"#,
        }),
        speech: None,
    },
    Service {
        id: "zai",
        name: "Z.ai GLM",
        summary: "Z.ai cloud with your own key.",
        address: Address::Fixed("https://api.z.ai/api/paas/v4"),
        translate: Some(Translate {
            model: "glm-5.3-flash",
            extra: r#"{"reasoning_effort": "low"}"#,
        }),
        speech: None,
    },
];
