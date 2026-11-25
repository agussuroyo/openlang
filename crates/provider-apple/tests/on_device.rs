//! Real recognition and translation of spoken audio.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopRun();
    }
    std::thread::spawn(|| {
        let ok = std::panic::catch_unwind(run).is_ok();
        std::process::exit(if ok { 0 } else { 1 });
    });
    unsafe { CFRunLoopRun() };
}

#[cfg(target_os = "macos")]
fn run() {
    use openlang_contract as contract;
    use openlang_core::{ports::*, registry::Registry};
    use std::sync::Arc;

    let mut registry = Registry::default();
    openlang_provider_apple::register(&mut registry);
    let stt = registry.stt("apple-speech").unwrap().as_ref();
    let translator = registry.translator("apple-translate").unwrap().as_ref();
    for (voice, source, target, text, expect) in [
        (
            "Samantha",
            "en-US",
            "id",
            "Good morning everyone. Today we will review the project budget.",
            "anggaran",
        ),
        (
            "Eddy (Japanese (Japan))",
            "ja-JP",
            "id",
            "今日は会議があります。よろしくお願いします。",
            "rapat",
        ),
        (
            "Damayanti",
            "id-ID",
            "en",
            "Selamat pagi. Hari ini kita membahas anggaran proyek.",
            "budget",
        ),
    ] {
        let job = Job {
            source: Some(source.into()),
            target: target.into(),
            options: Options::new(),
        };
        contract::stops_without_audio(stt, &job);
        let (heard, _) = contract::speech(stt, &job, &contract::speak(voice, text));
        let translated = contract::translation(translator, &job, &heard);
        assert!(
            translated.to_lowercase().contains(expect),
            "missing `{expect}`"
        );
    }
    let bad = Job {
        source: Some("tlh".into()),
        ..Job::default()
    };
    assert!(matches!(stt.readiness(&bad), Readiness::Unsupported { .. }));
    assert!(stt.open(&Job::default(), Arc::new(|_| {})).is_err());
}
