# fxtranslate 0.5.0, patched for OpenLang

Copied from crates.io (MPL-2.0, see `LICENSE`) without its tests and examples. Used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

Changes:

- `build.rs`: aarch64 builds the plain NEON kernel (`FXT_GEMM_NEON64`, `-march=armv8-a`)
  instead of i8mm. The i8mm kernel stops the process with SIGILL on CPUs without i8mm, such as
  Apple M1. Measured on M1 Pro with shortlist: en-id 23 ms, ja-en 70 ms per sentence (median).
- `build.rs`: MSVC gets `/arch:AVX2` (it ignores `-mavx2`, which silently left Windows on the
  slow scalar kernel) and no flag on arm64, where NEON is always on.
- `src/gemmology_shim.cpp`: adds the `FXT_GEMM_NEON64` arch, and uses `_aligned_malloc` /
  `_aligned_free` on MSVC, which has no `std::aligned_alloc`.
- `src/gemm.rs`: `PreparedB` is `Send`, so an `Engine` can be shared between the translation
  threads and dropped on any of them. Its handle owns plain heap memory freed with `aligned_free`.
- `Cargo.toml`: `rlib` only; no test or example targets.

x86_64 still builds the AVX2 kernel; `openlang-provider-bergamot` checks for AVX2 at run time
and refuses to start without it, instead of crashing.

Drop this copy once upstream picks its kernel at run time.
