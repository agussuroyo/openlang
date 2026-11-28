//! Real recognition. Downloads the 60 MB base model once. Speech comes from macOS `say`, so the
//! tests that need it run on macOS only.

use openlang_contract as contract;
use openlang_core::{ports::*, registry::Registry};

fn registry() -> Registry {
    let mut registry = Registry::default();
    openlang_provider_whisper::register(
        &mut registry,
        std::env::temp_dir().join("openlang-test-models"),
    );
    registry
}

#[test]
fn stops_without_audio() {
    let registry = registry();
    contract::stops_without_audio(registry.stt("whisper").unwrap().as_ref(), &Job::default());
}

#[cfg(target_os = "macos")]
#[test]
fn recognizes_speech_and_detects_the_language() {
    let registry = registry();
    let stt = registry.stt("whisper").unwrap().as_ref();
    for (voice, text, expect, language) in [
        (
            "Samantha",
            "Good morning everyone. Today we will review the project budget.",
            "budget",
            "en",
        ),
        (
            "Damayanti",
            "Selamat pagi. Hari ini kita membahas anggaran proyek.",
            "anggaran",
            "id",
        ),
    ] {
        let (heard, detected) =
            contract::speech(stt, &Job::default(), &contract::speak(voice, text));
        println!("{heard}");
        assert!(heard.to_lowercase().contains(expect), "{heard}");
        assert_eq!(detected.as_deref(), Some(language));
    }
}

fn detector() -> Box<dyn DetectorSession> {
    let detector = registry().detector.unwrap();
    detector.prepare(&Job::default(), &|_| {}).unwrap();
    detector
        .open(["en", "id", "ja"].map(String::from).to_vec())
        .unwrap()
}

#[test]
fn detector_ignores_silence() {
    assert_eq!(detector().detect(&vec![0.0; 48_000]).unwrap(), None);
}

#[cfg(target_os = "macos")]
#[test]
fn detector_names_the_language() {
    let mut session = detector();
    for (voice, text, language) in [
        (
            "Samantha",
            "Good morning everyone. Today we review the budget.",
            "en",
        ),
        (
            "Damayanti",
            "Selamat pagi. Hari ini kita membahas anggaran proyek.",
            "id",
        ),
        (
            "Eddy (Japanese (Japan))",
            "今日は会議があります。よろしくお願いします。",
            "ja",
        ),
    ] {
        let speech = contract::speak(voice, text);
        let started = std::time::Instant::now();
        let found = session.detect(&speech).unwrap().unwrap();
        println!("{language}: {found:?} in {:?}", started.elapsed());
        assert_eq!(found.language, language);
    }
}
