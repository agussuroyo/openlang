//! Real translation. Downloads about 120 MB of models once, on any platform.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use openlang_contract as contract;
use openlang_core::{Error, ports::*, registry::Registry};

fn translator(models: PathBuf) -> std::sync::Arc<dyn Translator> {
    let mut registry = Registry::default();
    openlang_provider_bergamot::register(&mut registry, models);
    registry.translator("bergamot").unwrap().clone()
}

fn job(source: Option<&str>, target: &str) -> Job {
    Job {
        source: source.map(String::from),
        target: target.into(),
        options: Options::new(),
    }
}

#[test]
fn translates_directly_and_through_english_within_budget() {
    let t = translator(std::env::temp_dir().join("openlang-test-models"));
    contract::descriptor(t.as_ref());
    assert!(t.languages().tags.contains(&"id".to_owned()));
    for (source, target, text, expect) in [
        (
            "en-US",
            "id-ID",
            "Good morning everyone. Today we will review the project budget.",
            "anggaran",
        ),
        (
            "id-ID",
            "en",
            "Selamat pagi. Hari ini kita membahas anggaran proyek.",
            "budget",
        ),
        (
            "ja-JP",
            "id-ID",
            "おはようございます。今日はプロジェクトの予算を確認します。",
            "anggaran",
        ),
    ] {
        let job = job(Some(source), target);
        let first = contract::translation(t.as_ref(), &job, text);
        assert!(first.to_lowercase().contains(expect), "{first}");
        let mut session = t.open(&job).unwrap();
        let at = Instant::now();
        session.translate(text, source).unwrap();
        assert!(
            at.elapsed() < contract::budget(600),
            "{source} to {target}: {:?}",
            at.elapsed()
        );
    }
}

#[test]
fn follows_the_detected_language_within_one_session() {
    let t = translator(std::env::temp_dir().join("openlang-test-models"));
    let auto = job(None, "en");
    assert_eq!(t.readiness(&auto), Readiness::Ready);
    contract::make_ready(t.as_ref(), &job(Some("id"), "en"));
    contract::make_ready(t.as_ref(), &job(Some("ja"), "en"));
    let mut session = t.open(&auto).unwrap();
    let id = session.translate("Terima kasih banyak.", "id").unwrap();
    let ja = session.translate("ありがとうございます。", "ja").unwrap();
    assert!(id.to_lowercase().contains("thank"), "{id}");
    assert!(ja.to_lowercase().contains("thank"), "{ja}");
}

#[test]
fn a_newly_detected_language_downloads_in_the_background() {
    let models = std::env::temp_dir().join(format!("openlang-bergamot-{}", std::process::id()));
    let t = translator(models.clone());
    let mut session = t.open(&job(None, "en")).unwrap();
    let at = Instant::now();
    match session.translate("Terima kasih banyak.", "id") {
        Err(Error::ModelMissing(_)) => {}
        other => panic!("{other:?}"),
    }
    // Failing fast keeps captions moving while the model arrives.
    assert!(at.elapsed() < contract::budget(100));
    let deadline = Instant::now() + Duration::from_secs(120);
    let text = loop {
        match session.translate("Terima kasih banyak.", "id") {
            Ok(text) => break text,
            // A failed download is reported once and tried again on the next sentence.
            Err(Error::ModelMissing(_)) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(Error::Provider(e))
                if e.starts_with("model download") && Instant::now() < deadline =>
            {
                println!("retrying after {e}");
            }
            Err(e) => panic!("{e}"),
        }
    };
    assert!(text.to_lowercase().contains("thank"), "{text}");
    let _ = std::fs::remove_dir_all(models);
}

#[test]
fn readiness_names_what_is_missing() {
    let t = translator(std::env::temp_dir().join("openlang-bergamot-empty"));
    assert!(matches!(
        t.readiness(&job(Some("ja"), "id")),
        Readiness::NeedsDownload { bytes: Some(b) } if b > 10_000_000
    ));
    assert!(matches!(
        t.readiness(&job(Some("xx"), "id")),
        Readiness::Unsupported { .. }
    ));
    assert_eq!(t.readiness(&job(None, "en")), Readiness::Ready);
}
