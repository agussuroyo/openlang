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
            // Primary tags, as the language detector gives them.
            "ja",
            "id",
            "今日は会議があります。よろしくお願いします。",
            "rapat",
        ),
        (
            "Damayanti",
            "id",
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
    let job = Job {
        source: Some("en-US".into()),
        target: "id".into(),
        options: Options::new(),
    };
    let talk = contract::speak(
        "Samantha",
        "Listeners, imagine a friend asks, would you rather have a picnic in the park or watch a \
         movie at home this Saturday? Choose one now, even if you could go either way. In a \
         moment, you will pause us and give yourself one minute to answer out loud, explaining \
         your choice, your reasons, and what you would do there. [[slnc 900]] Last weekend we had \
         Saturday afternoon free, so we talked about it over breakfast, and in the end we chose \
         the park, because the weather was perfect and we had not been outside all week.",
    );
    for stress in [false, true] {
        let report = contract::latency(
            registry.stt("apple-speech").unwrap().clone(),
            registry.translator("apple-translate").unwrap().clone(),
            &job,
            &talk,
            stress,
        );
        println!("stress {stress}: {report:#?}");
    }
    let bad = Job {
        source: Some("tlh".into()),
        ..Job::default()
    };
    assert!(matches!(stt.readiness(&bad), Readiness::Unsupported { .. }));
    assert!(stt.open(&Job::default(), Arc::new(|_| {})).is_err());
}
