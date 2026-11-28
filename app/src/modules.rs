//! The only list of plugged-in modules. Remove a line or a cargo feature to unplug one.

use std::path::Path;

use openlang_core::registry::Registry;

/// `models` is where modules that download models keep them.
#[cfg_attr(
    not(any(feature = "whisper", feature = "bergamot")),
    allow(unused_variables)
)]
pub fn registry(models: &Path) -> Registry {
    let mut r = Registry::default();
    openlang_audio_dsp::register(&mut r);
    #[cfg(all(feature = "coreaudio", target_os = "macos"))]
    openlang_audio_coreaudio::register(&mut r);
    #[cfg(feature = "cpal")]
    openlang_audio_cpal::register(&mut r);
    #[cfg(all(feature = "apple", target_os = "macos"))]
    openlang_provider_apple::register(&mut r);
    #[cfg(feature = "bergamot")]
    openlang_provider_bergamot::register(&mut r, models.to_path_buf());
    #[cfg(feature = "whisper")]
    openlang_provider_whisper::register(&mut r, models.to_path_buf());
    #[cfg(feature = "openai")]
    openlang_provider_openai::register(&mut r);
    r
}
