use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use openlang_core::{Error, Result};
use sha2::{Digest, Sha256};

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

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Provider(format!("model download: {e}"))
}

impl Model {
    pub fn path(&self, directory: &Path) -> PathBuf {
        directory.join(format!("{}.bin", self.id))
    }

    /// A file only gets its final name after its checksum matched.
    pub fn is_installed(&self, directory: &Path) -> bool {
        self.path(directory).is_file()
    }

    pub fn download(&self, directory: &Path, progress: &dyn Fn(f32)) -> Result<()> {
        fs::create_dir_all(directory).map_err(failed)?;
        let part = directory.join(format!("{}.part", self.id));
        let result = self.fetch(&part, progress);
        if result.is_err() {
            let _ = fs::remove_file(&part);
            return result;
        }
        fs::rename(&part, self.path(directory)).map_err(failed)
    }

    fn fetch(&self, part: &Path, progress: &dyn Fn(f32)) -> Result<()> {
        let url = format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}.bin",
            self.id
        );
        let mut reply = ureq::get(url).call().map_err(failed)?;
        let mut body = reply
            .body_mut()
            .with_config()
            .limit(self.bytes + 1)
            .reader();
        let mut file = File::create(part).map_err(failed)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 256 * 1024];
        let mut done = 0u64;
        loop {
            let n = body.read(&mut buffer).map_err(failed)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(failed)?;
            done += n as u64;
            progress(done as f32 / self.bytes as f32);
        }
        file.sync_all().map_err(failed)?;
        let sum: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if sum != self.sha256 {
            return Err(failed("the file does not match its checksum"));
        }
        Ok(())
    }
}
