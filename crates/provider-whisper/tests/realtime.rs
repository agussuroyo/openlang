//! The free pipeline on every platform: Whisper, then Bergamot. Captions must keep up with
//! speech, also while every core is busy.
#![cfg(target_os = "macos")]

use openlang_contract as contract;
use openlang_core::{ports::*, registry::Registry};

#[test]
fn whisper_and_bergamot_keep_up_with_speech() {
    // CI runners are virtual machines without a usable GPU; Whisper there runs about seven times
    // slower than on a real Mac, so they cannot say whether captions keep up.
    if std::env::var_os("CI").is_some() {
        println!("skipped on CI: needs a real machine");
        return;
    }
    let models = std::env::temp_dir().join("openlang-test-models");
    let mut registry = Registry::default();
    openlang_provider_whisper::register(&mut registry, models.clone());
    openlang_provider_bergamot::register(&mut registry, models);
    let job = Job {
        source: Some("en-US".into()),
        target: "id".into(),
        options: Options::new(),
    };
    let talk = contract::speak(
        "Samantha",
        "Listeners, imagine a friend asks, would you rather have a picnic in the park or watch a \
         movie at home this Saturday? Choose one now, even if you could go either way. [[slnc 900]] \
         Last weekend we had Saturday afternoon free, so we talked about it over breakfast, and in \
         the end we chose the park, because the weather was perfect.",
    );
    for stress in [false, true] {
        let report = contract::latency(
            registry.stt("whisper").unwrap().clone(),
            registry.translator("bergamot").unwrap().clone(),
            &job,
            &talk,
            stress,
        );
        println!("stress {stress}: {report:#?}");
        let gap = report
            .translation
            .median
            .saturating_sub(report.transcript.median);
        assert!(gap < contract::budget(600), "translation adds {gap:?}");
        assert!(
            report.partial_lag.median < contract::budget(600),
            "{report:#?}"
        );
    }
}
