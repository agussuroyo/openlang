use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum Error {
    #[error("permission needed: {0}")]
    Permission(String),
    #[error("language not supported: {0}")]
    Language(String),
    #[error("model not installed: {0}")]
    ModelMissing(String),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("audio device: {0}")]
    Device(String),
    #[error("provider: {0}")]
    Provider(String),
    #[error("graph: {0}")]
    Graph(String),
}
