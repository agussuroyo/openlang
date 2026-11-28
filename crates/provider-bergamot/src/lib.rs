//! Mozilla's on-device translation models (Firefox Translations), run by `fxtranslate` on the
//! CPU. Every model translates to or from English; other pairs go through English. A pair's
//! files are downloaded on first use.

mod catalog;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use fxtranslate::engine::Engine;
use openlang_core::{
    Error, Result, cpu,
    nodes::{primary, traditional_chinese},
    ports::*,
    registry::Registry,
};
use openlang_models::Download;

pub fn register(registry: &mut Registry, models: PathBuf) {
    // Parts sit in <pair>/<version>/.
    openlang_models::remove_stale_parts(&models.join("bergamot"), 2);
    registry.translators.push(Arc::new(Bergamot {
        descriptor: Descriptor {
            id: "bergamot",
            name: "Bergamot",
            summary: "On this computer, free. Mozilla's models, 20 to 60 MB per language.",
            fields: vec![],
        },
        shared: Arc::new(Shared {
            root: models.join("bergamot"),
            engines: Mutex::default(),
            downloads: Mutex::default(),
        }),
    }));
}

/// One file of a model, as stored in Mozilla's registry.
pub struct File {
    /// Under `catalog::BASE`, gzip.
    path: &'static str,
    /// Packed.
    bytes: u64,
    /// Unpacked.
    sha256: &'static str,
}

/// One model: `from` to `to`, one of them English.
pub struct Direction {
    from: &'static str,
    to: &'static str,
    model: File,
    src_vocab: File,
    /// `None` when both sides share `src_vocab`.
    trg_vocab: Option<File>,
    /// Limits each step to likely words, which makes decoding several times faster.
    shortlist: File,
}

impl Direction {
    fn files(&self) -> impl Iterator<Item = &File> {
        [&self.model, &self.src_vocab, &self.shortlist]
            .into_iter()
            .chain(&self.trg_vocab)
    }

    /// Where this direction's versions live.
    fn home(&self, root: &Path) -> PathBuf {
        root.join(format!("{}-{}", self.from, self.to))
    }

    /// The registry's training run, `models/<pair>/<run>/...`. Files of one run belong together,
    /// so a newer catalog never mixes them with older ones.
    fn version(&self) -> &'static str {
        self.model.path.split('/').nth(2).unwrap_or("")
    }

    fn path(&self, root: &Path, file: &File) -> PathBuf {
        let name = file.path.rsplit('/').next().unwrap_or(file.path);
        self.home(root)
            .join(self.version())
            .join(name.strip_suffix(".gz").unwrap_or(name))
    }

    /// A file only gets its final name after its checksum matched.
    fn installed(&self, root: &Path) -> bool {
        self.files().all(|f| self.path(root, f).is_file())
    }

    fn bytes(&self) -> u64 {
        self.files().map(|f| f.bytes).sum()
    }

    fn download(&self, root: &Path, progress: &dyn Fn(f32)) -> Result<()> {
        let total = self.bytes().max(1) as f32;
        let mut done = 0;
        for file in self.files() {
            let path = self.path(root, file);
            if !path.is_file() {
                let url = format!("{}/{}", catalog::BASE, file.path);
                let before = done as f32;
                let size = file.bytes as f32;
                Download {
                    url: &url,
                    bytes: file.bytes,
                    sha256: file.sha256,
                    gzip: true,
                }
                .fetch(&path, &|p| progress((before + p * size) / total))?;
            }
            done += file.bytes;
        }
        // Older versions are no longer used. One still open fails to go on Windows; it goes
        // with the next download.
        if let Ok(entries) = std::fs::read_dir(self.home(root)) {
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_name() != self.version() {
                    let _ = if path.is_dir() {
                        std::fs::remove_dir_all(path)
                    } else {
                        std::fs::remove_file(path)
                    };
                }
            }
        }
        progress(1.0);
        Ok(())
    }

    fn load(&self, root: &Path) -> Result<Engine> {
        let src = self.path(root, &self.src_vocab);
        let trg = self
            .trg_vocab
            .as_ref()
            .map_or_else(|| src.clone(), |f| self.path(root, f));
        let shortlist = std::fs::read(self.path(root, &self.shortlist))
            .map_err(|e| Error::Provider(format!("bergamot: {e}")))?;
        Engine::load_mmapped(self.path(root, &self.model), &src, &trg)
            .map(|e| e.with_shortlist_bytes(&shortlist))
            .map_err(|e| Error::Provider(format!("bergamot: {e}")))
    }
}

/// The catalog's code for a BCP 47 tag.
fn code(tag: &str) -> String {
    if traditional_chinese(tag) {
        return "zh_hant".into();
    }
    primary(tag).to_ascii_lowercase()
}

/// The BCP 47 tag for a catalog code.
fn tag(code: &str) -> String {
    match code {
        "zh" => "zh-Hans".into(),
        "zh_hant" => "zh-Hant".into(),
        other => other.into(),
    }
}

fn direction(from: &str, to: &str) -> Option<&'static Direction> {
    catalog::ALL.iter().find(|d| d.from == from && d.to == to)
}

/// The models that translate `source` to `target`, in order.
fn route(source: &str, target: &str) -> Result<Vec<&'static Direction>> {
    let (from, to) = (code(source), code(target));
    let legs: Vec<(&str, &str)> = match (from.as_str(), to.as_str()) {
        (f, t) if f == t => vec![],
        ("en", t) => vec![("en", t)],
        (f, "en") => vec![(f, "en")],
        (f, t) => vec![(f, "en"), ("en", t)],
    };
    legs.into_iter()
        .map(|(f, t)| {
            direction(f, t).ok_or_else(|| {
                Error::Language(format!("Bergamot has no model from {source} to {target}"))
            })
        })
        .collect()
}

/// The models a job needs before it starts. With automatic detection only the target is known.
fn needed(job: &Job) -> Result<Vec<&'static Direction>> {
    match &job.source {
        Some(source) => route(source, &job.target),
        None if code(&job.target) == "en" => Ok(vec![]),
        None => route("en", &job.target),
    }
}

struct Shared {
    root: PathBuf,
    /// Loaded once and used by every session, which keeps memory low.
    engines: Mutex<HashMap<(&'static str, &'static str), Weak<Engine>>>,
    /// Background downloads: `None` while running, the error once one failed.
    downloads: Mutex<HashMap<(&'static str, &'static str), Option<Error>>>,
}

impl Shared {
    fn engine(&self, d: &'static Direction) -> Result<Arc<Engine>> {
        let key = (d.from, d.to);
        let lock = || self.engines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(engine) = lock().get(&key).and_then(Weak::upgrade) {
            return Ok(engine);
        }
        // Loading takes a while; other sessions keep translating meanwhile.
        let loaded = Arc::new(d.load(&self.root)?);
        let mut engines = lock();
        if let Some(engine) = engines.get(&key).and_then(Weak::upgrade) {
            return Ok(engine);
        }
        engines.insert(key, Arc::downgrade(&loaded));
        Ok(loaded)
    }

    /// Engines for a route. A missing model starts downloading in the background, so the
    /// first sentences of a newly detected language fail fast instead of holding captions.
    fn engines(self: &Arc<Self>, source: &str, target: &str) -> Result<Vec<Arc<Engine>>> {
        let route = route(source, target)?;
        let missing: Vec<_> = route.iter().filter(|d| !d.installed(&self.root)).collect();
        if missing.is_empty() {
            return route.into_iter().map(|d| self.engine(d)).collect();
        }
        let mut downloads = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
        for &&d in &missing {
            let key = (d.from, d.to);
            match downloads.get(&key) {
                Some(None) => {}
                // Reported once; the next sentence tries again.
                Some(Some(_)) => {
                    if let Some(Some(e)) = downloads.remove(&key) {
                        return Err(e);
                    }
                }
                None => {
                    downloads.insert(key, None);
                    let shared = self.clone();
                    std::thread::spawn(move || {
                        let result = d.download(&shared.root, &|_| {});
                        let mut downloads =
                            shared.downloads.lock().unwrap_or_else(|e| e.into_inner());
                        match result {
                            Ok(()) => downloads.remove(&key),
                            Err(e) => downloads.insert(key, Some(e)),
                        };
                    });
                }
            }
        }
        Err(Error::ModelMissing(format!(
            "downloading the {source} to {target} model"
        )))
    }
}

struct Bergamot {
    descriptor: Descriptor,
    shared: Arc<Shared>,
}

impl Provider for Bergamot {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    fn location(&self, _: &Options) -> Location {
        Location::OnDevice
    }
    /// Languages it translates into.
    fn languages(&self) -> Languages {
        let mut tags: Vec<String> = catalog::ALL
            .iter()
            .filter(|d| d.from == "en")
            .map(|d| tag(d.to))
            .chain(["en".into()])
            .collect();
        tags.sort();
        Languages {
            tags,
            can_detect: false,
        }
    }
    fn readiness(&self, job: &Job) -> Readiness {
        if let Err(e) = cpu::check(cpu::Baseline::Gemmology) {
            return unsupported(e.to_string());
        }
        match needed(job) {
            Err(e) => unsupported(e.to_string()),
            Ok(route) => {
                let missing: u64 = route
                    .iter()
                    .filter(|d| !d.installed(&self.shared.root))
                    .map(|d| d.bytes())
                    .sum();
                if missing == 0 {
                    Readiness::Ready
                } else {
                    Readiness::NeedsDownload {
                        bytes: Some(missing),
                    }
                }
            }
        }
    }
    fn prepare(&self, job: &Job, progress: &dyn Fn(f32)) -> Result<()> {
        cpu::check(cpu::Baseline::Gemmology)?;
        let route = needed(job)?;
        let total = route.iter().map(|d| d.bytes()).sum::<u64>().max(1) as f32;
        let mut done = 0.0;
        for d in route {
            let size = d.bytes() as f32;
            d.download(&self.shared.root, &|p| progress((done + p * size) / total))?;
            done += size;
        }
        Ok(())
    }
}

impl Translator for Bergamot {
    fn open(&self, job: &Job) -> Result<Box<dyn TranslatorSession>> {
        cpu::check(cpu::Baseline::Gemmology)?;
        let mut session = Session {
            shared: self.shared.clone(),
            target: job.target.clone(),
            source: String::new(),
            engines: vec![],
        };
        if let Some(source) = &job.source {
            // The first sentence of a fresh model touches its pages; do it before anyone speaks.
            session.translate("OK", source)?;
        }
        Ok(Box::new(session))
    }
}

struct Session {
    shared: Arc<Shared>,
    target: String,
    /// What `engines` translate from; a detecting recognizer may change it.
    source: String,
    engines: Vec<Arc<Engine>>,
}

impl TranslatorSession for Session {
    fn translate(&mut self, text: &str, source: &str) -> Result<String> {
        if self.source != source || self.engines.is_empty() {
            self.engines = self.shared.engines(source, &self.target)?;
            self.source = source.to_owned();
        }
        Ok(self
            .engines
            .iter()
            .fold(text.to_owned(), |text, e| e.translate_long(&text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_map_to_catalog_codes() {
        assert_eq!(code("id-ID"), "id");
        assert_eq!(code("zh-Hans"), "zh");
        assert_eq!(code("zh-CN"), "zh");
        assert_eq!(code("zh-Hant"), "zh_hant");
        assert_eq!(code("zh_TW"), "zh_hant");
        assert_eq!(tag("zh_hant"), "zh-Hant");
    }

    #[test]
    fn pairs_without_english_go_through_english() {
        let legs = |s, t| {
            route(s, t)
                .unwrap()
                .iter()
                .map(|d| format!("{}-{}", d.from, d.to))
                .collect::<Vec<_>>()
        };
        assert_eq!(legs("en-US", "id-ID"), ["en-id"]);
        assert_eq!(legs("ja-JP", "en"), ["ja-en"]);
        assert_eq!(legs("ja-JP", "id-ID"), ["ja-en", "en-id"]);
        assert!(legs("id", "id-ID").is_empty());
        assert!(route("xx", "en").is_err());
    }

    #[test]
    fn the_catalog_pins_every_file() {
        assert!(catalog::ALL.len() > 100);
        for d in catalog::ALL {
            assert!(d.from == "en" || d.to == "en");
            for f in d.files() {
                assert_eq!(f.sha256.len(), 64);
                assert!(f.bytes > 0 && f.path.ends_with(".gz"));
            }
        }
    }
}
