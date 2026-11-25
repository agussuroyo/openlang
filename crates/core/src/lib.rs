//! Domain model, ports and the processing graph. Knows no OS, UI or provider.

pub mod domain;
pub mod error;
pub mod graph;
pub mod nodes;
pub mod ports;
pub mod realtime;
pub mod registry;
pub mod session;
pub mod stabilizer;

pub use error::{Error, Result};
