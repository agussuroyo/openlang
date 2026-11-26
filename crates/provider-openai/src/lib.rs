//! Any OpenAI-compatible server: Ollama or LM Studio on this computer, or a cloud with your own key.

mod chat;
mod client;
mod server;
mod services;
mod speech;

use std::sync::Arc;

use openlang_core::{Error, Result, registry::Registry};
use serde_json::Value;

use server::Server;
pub use services::{Address, SERVICES, Service, SpeechApi, Translate};

pub fn register(registry: &mut Registry) {
    for service in SERVICES {
        add(registry, *service);
    }
}

/// Adds the speech and translate entries of one service.
pub fn add(registry: &mut Registry, service: Service) {
    if let Some(translate) = service.translate {
        registry
            .translators
            .push(Arc::new(Server::new(service, translate)));
    }
    if let Some(speech) = service.speech {
        registry.stt.push(Arc::new(Server::new(service, speech)));
    }
}

fn text(v: &Value) -> Result<String> {
    v.as_str()
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| Error::Provider("the server reply has no text".into()))
}
