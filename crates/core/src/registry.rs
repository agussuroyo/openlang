use std::sync::Arc;

use crate::{
    Error, Result,
    ports::{
        AudioSource, LanguageDetector, Location, Options, Provider, SpeechToText, TransformFactory,
        Translator,
    },
};

/// Everything that was plugged in. An adapter crate adds itself with one `register` function.
#[derive(Default)]
pub struct Registry {
    pub sources: Vec<Arc<dyn AudioSource>>,
    pub stt: Vec<Arc<dyn SpeechToText>>,
    pub translators: Vec<Arc<dyn Translator>>,
    pub resampler: Option<Arc<dyn TransformFactory>>,
    pub detector: Option<Arc<dyn LanguageDetector>>,
}

fn find<'a, T: ?Sized>(
    list: &'a [Arc<T>],
    id: &str,
    get: impl Fn(&T) -> &str,
) -> Result<&'a Arc<T>> {
    list.iter()
        .find(|x| get(x) == id)
        .ok_or_else(|| Error::Config(format!("module `{id}` is not installed")))
}

impl Registry {
    pub fn source(&self, id: &str) -> Result<&Arc<dyn AudioSource>> {
        find(&self.sources, id, |x| x.descriptor().id)
    }

    pub fn stt(&self, id: &str) -> Result<&Arc<dyn SpeechToText>> {
        find(&self.stt, id, |x| x.descriptor().id)
    }

    pub fn translator(&self, id: &str) -> Result<&Arc<dyn Translator>> {
        find(&self.translators, id, |x| x.descriptor().id)
    }
}

/// Refuses to send speech or text off this computer without the user's consent.
pub fn check_consent(p: &dyn Provider, options: &Options, allow_network: bool) -> Result<()> {
    if p.location(options) == Location::Network && !allow_network {
        return Err(Error::Config(format!(
            "{} is online and online use is off",
            p.descriptor().name
        )));
    }
    Ok(())
}
