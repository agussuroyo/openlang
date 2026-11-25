//! Real recognition. Downloads the 60 MB base model once.
#![cfg(target_os = "macos")]

use openlang_contract as contract;
use openlang_core::{ports::*, registry::Registry};

#[test]
fn recognizes_speech_and_detects_the_language() {
    let mut registry = Registry::default();
    openlang_provider_whisper::register(
        &mut registry,
        std::env::temp_dir().join("openlang-test-models"),
    );
    let stt = registry.stt("whisper").unwrap().as_ref();
    contract::stops_without_audio(stt, &Job::default());
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
