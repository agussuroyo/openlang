use std::path::{Path, PathBuf};

use openlang_core::{
    Error, Result, cpu,
    ports::{Readiness, unsupported},
};
use openlang_models::Download;

pub struct Model {
    pub id: &'static str,
    pub label: &'static str,
    pub bytes: u64,
    sha256: &'static str,
}

pub const ALL: &[Model] = &[
    Model {
        id: "ggml-base-q5_1",
        label: "Base, 60 MB (fast)",
        bytes: 59_707_625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
    },
    Model {
        id: "ggml-small-q5_1",
        label: "Small, 190 MB (more accurate)",
        bytes: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    },
    Model {
        id: "ggml-large-v3-turbo-q5_0",
        label: "Large v3 turbo, 574 MB (most accurate, heaviest)",
        bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
];

pub fn find(id: &str) -> Result<&'static Model> {
    ALL.iter()
        .find(|m| m.id == id)
        .ok_or_else(|| Error::Config(format!("model `{id}`")))
}

impl Model {
    pub fn path(&self, directory: &Path) -> PathBuf {
        directory.join(format!("{}.bin", self.id))
    }

    /// A file only gets its final name after its checksum matched.
    pub fn readiness(&self, directory: &Path) -> Readiness {
        if let Err(e) = cpu::check(cpu::Baseline::Ggml) {
            return unsupported(e.to_string());
        }
        if self.path(directory).is_file() {
            return Readiness::Ready;
        }
        Readiness::NeedsDownload {
            bytes: Some(self.bytes),
        }
    }

    pub fn download(&self, directory: &Path, progress: &dyn Fn(f32)) -> Result<()> {
        Download {
            url: &format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}.bin",
                self.id
            ),
            bytes: self.bytes,
            sha256: self.sha256,
            gzip: false,
        }
        .fetch(&self.path(directory), progress)
    }
}
