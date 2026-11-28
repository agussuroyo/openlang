//! The processor features native modules are built for.

use crate::Result;

/// What a native library's build needs from the processor. A module checks before it loads
/// native code, so an older processor gets a message instead of a crash.
#[derive(Clone, Copy, Debug)]
pub enum Baseline {
    /// whisper.cpp's ggml, built with `GGML_NATIVE=OFF` (`.cargo/config.toml`): AVX2 with FMA,
    /// F16C and BMI2 on x86_64, ARMv8.2 with dotprod and fp16 on aarch64.
    Ggml,
    /// fxtranslate's gemmology kernel (`vendor/fxtranslate/build.rs`): AVX2 on x86_64, where
    /// MSVC's `/arch:AVX2` may also emit FMA and BMI; plain NEON on aarch64, which every aarch64
    /// processor has.
    Gemmology,
}

pub fn check(baseline: Baseline) -> Result<()> {
    #[cfg(target_arch = "x86_64")]
    let (ok, needs) = {
        use std::arch::is_x86_feature_detected as has;
        let avx2 = has!("avx2") && has!("fma") && has!("bmi1") && has!("bmi2");
        let ok = match baseline {
            Baseline::Ggml => avx2 && has!("f16c"),
            Baseline::Gemmology => avx2,
        };
        (ok, "AVX2 (2013 or newer)")
    };
    #[cfg(target_arch = "aarch64")]
    let (ok, needs) = match baseline {
        Baseline::Ggml => (
            std::arch::is_aarch64_feature_detected!("dotprod")
                && std::arch::is_aarch64_feature_detected!("fp16"),
            "ARMv8.2 with dotprod",
        ),
        Baseline::Gemmology => (true, ""),
    };
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let (ok, needs) = {
        let _ = baseline;
        (true, "")
    };
    if ok {
        Ok(())
    } else {
        Err(crate::Error::Provider(format!(
            "this needs a processor with {needs}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{Baseline, check};

    #[test]
    fn this_machine_has_every_baseline() {
        check(Baseline::Ggml).unwrap();
        check(Baseline::Gemmology).unwrap();
    }
}
