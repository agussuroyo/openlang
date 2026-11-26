//! What every service has in common: settings, address, key and readiness.

use openlang_core::{Error, Result, ports::*};

use crate::client::{self, Client};
use crate::services::{Address, Service};

const LANGUAGES: &[&str] = &[
    "ar", "bn", "cs", "da", "de", "el", "en", "es", "fa", "fi", "fr", "he", "hi", "hu", "id", "it",
    "ja", "jv", "ko", "ms", "nb", "nl", "pl", "pt", "ro", "ru", "su", "sv", "ta", "th", "tl", "tr",
    "uk", "ur", "vi", "zh",
];

/// What a speech or translate entry adds to the shared `Server`.
pub trait Api: Send + Sync + 'static {
    /// The default of the "Model name" field, or `None` when the model is not chosen by the user.
    fn model_field(&self) -> Option<&'static str>;
    fn can_detect(&self) -> bool;
}

pub struct Server<T> {
    pub service: Service,
    pub api: T,
    descriptor: Descriptor,
}

impl<T: Api> Server<T> {
    pub fn new(service: Service, api: T) -> Self {
        let descriptor = descriptor(&service, &api);
        Self {
            service,
            api,
            descriptor,
        }
    }

    pub fn client(&self, options: &Options) -> Result<Client> {
        let key = self.option(options, "key");
        if key.is_empty() && matches!(self.service.address, Address::Fixed(_)) {
            return Err(Error::Config(format!(
                "enter your {} API key in Settings > Engine",
                self.service.name
            )));
        }
        Client::new(&self.base(options), &key)
    }

    pub fn model(&self, options: &Options) -> Result<String> {
        let model = self.option(options, "model");
        if model.is_empty() {
            return Err(Error::Config(
                "enter a model name in Settings > Engine".into(),
            ));
        }
        Ok(model)
    }

    /// Checks the settings only; it never contacts the server.
    fn check(&self, options: &Options) -> Result<()> {
        if self.api.model_field().is_some() {
            self.model(options)?;
        }
        self.client(options).map(drop)
    }

    fn base(&self, options: &Options) -> String {
        match self.service.address {
            Address::Fixed(base) => base.to_owned(),
            Address::Editable(_) => self.option(options, "base"),
        }
    }

    fn option(&self, options: &Options, key: &str) -> String {
        self.descriptor.option(options, key).trim().to_owned()
    }
}

impl<T: Api> Provider for Server<T> {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    fn location(&self, options: &Options) -> Location {
        if client::is_loopback(&self.base(options)) {
            Location::OnDevice
        } else {
            Location::Network
        }
    }
    fn languages(&self) -> Languages {
        Languages {
            tags: LANGUAGES.iter().map(|t| (*t).to_owned()).collect(),
            can_detect: self.api.can_detect(),
        }
    }
    fn readiness(&self, job: &Job) -> Readiness {
        match self.check(&job.options) {
            Ok(()) => Readiness::Ready,
            Err(Error::Config(message)) => Readiness::NeedsAction { message },
            Err(e) => Readiness::NeedsAction {
                message: e.to_string(),
            },
        }
    }
    fn prepare(&self, job: &Job, _: &dyn Fn(f32)) -> Result<()> {
        self.check(&job.options)
    }
}

fn descriptor(service: &Service, api: &impl Api) -> Descriptor {
    let field = |key, label, kind, default| Field {
        key,
        label,
        kind,
        default,
    };
    let mut fields = Vec::new();
    if let Address::Editable(base) = service.address {
        fields.push(field("base", "Server address", FieldKind::Text, base));
    }
    if let Some(model) = api.model_field() {
        fields.push(field("model", "Model name", FieldKind::Text, model));
    }
    let key_label = match service.address {
        Address::Editable(_) => "API key (leave empty for a local server)",
        Address::Fixed(_) => "API key",
    };
    fields.push(field("key", key_label, FieldKind::Secret, ""));
    Descriptor {
        id: service.id,
        name: service.name,
        summary: service.summary,
        fields,
    }
}
