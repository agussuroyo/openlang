//! Verified downloads for modules that keep their models on disk.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

use flate2::read::GzDecoder;
use openlang_core::{Error, Result};
use sha2::{Digest, Sha256};

/// One file to fetch.
pub struct Download<'a> {
    pub url: &'a str,
    /// Size on the wire; a larger reply is refused.
    pub bytes: u64,
    /// Of the stored file, after unpacking.
    pub sha256: &'a str,
    /// The reply is gzip and is stored unpacked.
    pub gzip: bool,
}

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Provider(format!("model download: {e}"))
}

/// Counts what was read off the wire.
struct Counted<'a, R> {
    inner: R,
    done: u64,
    total: u64,
    progress: &'a dyn Fn(f32),
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buffer)?;
        self.done += n as u64;
        (self.progress)(self.done as f32 / self.total.max(1) as f32);
        Ok(n)
    }
}

impl Download<'_> {
    /// Writes `path` only once its checksum matched, so an existing `path` is always complete.
    /// Several downloads of one file may run at once; each writes its own part. Progress runs
    /// from 0 to 1.
    pub fn fetch(&self, path: &Path, progress: &dyn Fn(f32)) -> Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).map_err(failed)?;
        }
        let mut part = path.as_os_str().to_owned();
        part.push(format!(
            ".{}-{}.part",
            std::process::id(),
            NEXT.fetch_add(1, Relaxed)
        ));
        let part = PathBuf::from(part);
        let result = self
            .write(&part, progress)
            .and_then(|()| place(&part, path));
        let _ = fs::remove_file(&part);
        result
    }

    fn write(&self, part: &Path, progress: &dyn Fn(f32)) -> Result<()> {
        let mut reply = ureq::get(self.url).call().map_err(failed)?;
        let wire = Counted {
            inner: reply
                .body_mut()
                .with_config()
                .limit(self.bytes + 1)
                .reader(),
            done: 0,
            total: self.bytes,
            progress,
        };
        let mut body: Box<dyn Read> = if self.gzip {
            Box::new(GzDecoder::new(wire))
        } else {
            Box::new(wire)
        };
        let mut file = File::create(part).map_err(failed)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 256 * 1024];
        loop {
            let n = body.read(&mut buffer).map_err(failed)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(failed)?;
        }
        file.sync_all().map_err(failed)?;
        let sum: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if sum != self.sha256 {
            return Err(failed("the file does not match its checksum"));
        }
        Ok(())
    }
}

/// Gives a verified `part` its final name, but never replaces a file that already has it: that
/// one is verified too, and someone may be opening it. On Windows, replacing a file fails an
/// `fopen` of it made at that moment (whisper.cpp then cannot load its model). A hard link
/// fails when `path` exists, atomically; a file system without hard links gets a rename.
fn place(part: &Path, path: &Path) -> Result<()> {
    if fs::hard_link(part, path).is_ok() || path.is_file() {
        return Ok(());
    }
    fs::rename(part, path).or_else(|e| {
        if path.is_file() {
            Ok(())
        } else {
            Err(failed(e))
        }
    })
}

/// Removes parts left behind by an app that was stopped mid-download, in `directory` and
/// `depth` levels of folders below it. Parts of this process may still be written and stay.
/// Links are never followed.
pub fn remove_stale_parts(directory: &Path, depth: usize) {
    let mine = format!(".{}-", std::process::id());
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() && depth > 0 {
            remove_stale_parts(&entry.path(), depth - 1);
        } else if kind.is_file() && name.ends_with(".part") && !name.contains(&mine) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_download_leaves_no_file() {
        let directory =
            std::env::temp_dir().join(format!("openlang-models-{}", std::process::id()));
        let path = directory.join("model.bin");
        let download = Download {
            url: "http://127.0.0.1:9/missing",
            bytes: 10,
            sha256: "",
            gzip: false,
        };
        assert!(download.fetch(&path, &|_| {}).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn a_finished_download_never_replaces_the_file() {
        let directory = std::env::temp_dir().join(format!("openlang-place-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("model.bin");
        let part = directory.join("model.bin.1-0.part");
        fs::write(&part, "first").unwrap();
        place(&part, &path).unwrap();
        // As `fetch` does: a linked part shares the file.
        fs::remove_file(&part).unwrap();
        fs::write(&part, "second").unwrap();
        // Held open, as a model being loaded is.
        let reader = File::open(&path).unwrap();
        place(&part, &path).unwrap();
        drop(reader);
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn stale_parts_go_and_live_ones_stay() {
        let directory = std::env::temp_dir().join(format!("openlang-parts-{}", std::process::id()));
        let nested = directory.join("en-id");
        fs::create_dir_all(&nested).unwrap();
        let live = nested.join(format!("model.bin.{}-0.part", std::process::id()));
        for name in ["ggml-base.part", "model.bin.1-0.part", "model.bin"] {
            fs::write(nested.join(name), b"").unwrap();
        }
        fs::write(&live, b"").unwrap();
        remove_stale_parts(&directory, 1);
        let mut left: Vec<_> = fs::read_dir(&nested)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "model.bin".to_owned(),
                format!("model.bin.{}-0.part", std::process::id())
            ]
        );
        let _ = fs::remove_dir_all(directory);
    }
}
