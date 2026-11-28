//! The processor features native modules are built for.

use crate::Result;

/// Builds target a baseline (`.cargo/config.toml`): AVX2 with FMA, F16C and BMI2 on x86_64,
/// ARMv8.2 with dotprod and fp16 on aarch64. A module checks before it loads native code, so an
/// older processor gets a message instead of a crash.
pub fn check() -> Result<()> {
    #[cfg(target_arch = "x86_64")]
    let (ok, needs) = (
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
            && std::arch::is_x86_feature_detected!("bmi2"),
        "AVX2 (2013 or newer)",
    );
    #[cfg(target_arch = "aarch64")]
    let (ok, needs) = (
        std::arch::is_aarch64_feature_detected!("dotprod")
            && std::arch::is_aarch64_feature_detected!("fp16"),
        "ARMv8.2 with dotprod",
    );
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let (ok, needs) = (true, "");
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
    #[test]
    fn this_machine_has_the_baseline() {
        super::check().unwrap();
    }
}
