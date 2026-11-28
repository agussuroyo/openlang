# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

OpenLang: desktop live translation of any sound (films, Meet/Zoom/Teams, recordings, microphone). Rust workspace + Tauri 2 shell + plain HTML/CSS/JS UI. macOS 26+, Windows and Linux; free and on-device on all three (Whisper + Bergamot off macOS). All UI text is English.

## Commands

The `Makefile` is the one place commands live, on every system; CI runs the same targets. On Windows run it from Git Bash.

```sh
make check                                               # fmt, clippy -D warnings, UI syntax
make test                                                # see test notes below; ARGS=... adds cargo test arguments
make build                                               # bundles in target/release/bundle/ (C:/t on Windows)
make run / make install                                  # open the build / install it on this computer
cargo test -p openlang-core --test session a_stuck       # one test by name filter
cargo test -p openlang-provider-apple -- --nocapture     # real Apple speech/translation
```

On macOS, `rustup` and `cmake` come from Homebrew and are not on PATH by default; the Makefile adds them. For direct `cargo` calls: `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`.

Unplug a module: `cargo build -p openlang --no-default-features --features apple,coreaudio` (features: `apple bergamot coreaudio cpal openai whisper`).

Windows and Linux build what CI builds (`.github/workflows/ci.yml`): Linux needs `libasound2-dev libdbus-1-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libclang-dev cmake libvulkan-dev glslc` with Vulkan headers 1.3.275 or newer (Ubuntu 24.04, Debian 13; Debian 12 is too old for ggml); Windows needs the Vulkan SDK (`VULKAN_SDK`) and `make`, and its bundle ships `app/vulkan-1.dll`, built from Khronos Vulkan-Loader as CI does. Windows on ARM builds whisper.cpp with clang-cl, since ggml refuses MSVC on ARM: `CMAKE_GENERATOR=Ninja CMAKE_C_COMPILER=clang-cl CMAKE_CXX_COMPILER=clang-cl CXXFLAGS_aarch64_pc_windows_msvc=-EHsc` inside the arm64 MSVC environment, without Git Bash's `link.exe` ahead of MSVC's (the cmake crate forces `-Thost=x64` on the Visual Studio generator). CI covers macOS arm64 and x86_64, Linux x64 and arm64, Windows x64 and arm64. Linux in Docker on this Mac: copy the source into the container first, `tauri-build` writes into `app/`.

Test notes:
- Provider tests that speak real audio with macOS `say` are macOS-only (`contract::speak` exists only there). Whisper's tests without speech run everywhere. Bergamot's test needs no audio and runs everywhere; it downloads about 120 MB of models once.
- `provider-whisper/tests/realtime.rs` plays speech in real time through Whisper and Bergamot, idle and with every core busy, and fails when translation adds 600 ms or partials lag. It skips itself on CI, whose virtual machines run Whisper about seven times slower than a real Mac.
- `FXTRANSLATE_REQUIRE_SIMD=1` (set in CI) fails a build that would fall back to the slow scalar translation kernel.
- The Whisper test downloads a 60 MB model to a temp dir once. It needs `cmake` and network.
- `provider-apple/tests/on_device.rs` uses `harness = false` with its own `main`. Apple Translation only makes progress while a main run loop is alive, so it runs `CFRunLoopRun` on the main thread.
- The `contract` crate is the shared provider test suite. Every provider test must call `contract::speech` / `translation` / `stops_without_audio`.

## Architecture

**Hexagonal.** `crates/core` defines domain types (`domain.rs`), small ports (`ports.rs`), the processing graph, sessions and the registry. It must never depend on an OS, UI or provider crate; `cargo tree -p openlang-core` shows this. Adapters live in `crates/audio-*` and `crates/provider-*`. Each exposes `register(&mut Registry)`. `app/src/modules.rs` is the only place modules are listed, each behind a cargo feature.

**Graph (`core/src/graph.rs`).** A session builds one typed graph per audio source:

`TransformNode` (resample to 16 kHz mono) → `SttNode` → `TranslateNode` → `StabilizeNode` → `SinkNode`s.

- Every node runs on its own thread behind its own queue.
- Edges are type-checked when the graph is built, and inputs must already exist, so there are no cycles.
- `QueuePolicy::Coalesce` replaces a queued partial with a newer one of the same segment; finals are never dropped. `Node::ready_at` throttles partials (`partial_interval`).
- `TranslateNode` translates finals inline and partials on its own `PartialWorker` thread with a second translator session, so a final never waits for a partial. The worker skips to the newest partial, and drops a partial whose segment's final was already emitted. Partials and finals share clause translations (`core/src/clauses.rs`): a clause is translated once and known by its words alone (volatile partials lack the final's punctuation), a final reuses the longest runs its partials translated and sends its missing ones in one call (Apple Translate costs ~0.5 s per call and answers one request at a time). The worker waits while a final is being translated.
- A node that errors or panics is reported and stopped. Other branches keep running.
- `Session::stop` drains finals but is bounded by `Graph::finish_within(STOP_LIMIT)`. Never add an unbounded wait on stop: the "Stop button does nothing" bug came from Apple Speech never finishing when no audio had arrived.

**Extending without touching the pipeline.** `session::Extension::attach` gets `Taps` (node ids for audio/transcript/translation/caption) to add branches, such as future TTS or history. An `EventSink` receives serializable `SessionEvent`s (versioned by `EVENT_SCHEMA`). `core/tests/session.rs` shows both, using test-only nodes.

**Segments.** Providers only emit `SttUpdate { text, start, end, is_final }`. `SttNode`'s segmenter assigns `SegmentId` and revision. Partials and the final of one utterance share an id, and the UI updates the text in place by that id. `Stabilizer` marks the common word prefix of consecutive revisions as stable (`Caption::stable_words`); the subtitle UI does not use it now.

**Realtime audio.** Audio callbacks only write to an `rtrb` ring via `realtime::AudioWriter`: no locks, allocation or logging. Lost samples are counted. A source calls `Invalidator::invalidate` on device change; the session pump then reopens the source.

**Providers.**
- `apple-bridge`: Swift (`swift/*.swift`, compiled by `build.rs` with `swiftc` into a static lib) behind a C ABI.
  - Every bridge call blocks and must run off the main thread.
  - The Swift `Gate` guarantees no callback after `ol_stt_finish` returns; Rust frees the callback box relying on that.
  - Apple STT uses `SpeechTranscriber`, falling back to `DictationTranscriber` for locales such as `id-ID`. It cannot auto-detect the language.
  - Translation uses `TranslationSession(installedSource:)` with the low-latency strategy when its pack is installed (macOS 26.4+, about 30 ms a call), otherwise high fidelity (about 0.5 s a call, one request at a time across sessions). Low-latency packs download only through SwiftUI `.translationTask` + `prepareTranslation()`: `AppleTranslator::readiness` says `NeedsDownload` while a pack is missing, and `prepare` shows the system sheet (`swift/TranslationDownload.swift`), once per pair per run (`Offers`). With Auto, a pair is known only once someone speaks: opening the translator or switching its language offers the pack on its own thread while translation goes on, and sessions reopen once an offer finished. It needs a running `NSApp`; a process without one, such as a test, skips it. The sheet also stops a bare `CFRunLoopRun`. It warms up on open.
- `core/src/auto_language.rs`: Auto language for a recognizer that cannot detect (Apple). `LanguageDetector` (Whisper base model, `provider-whisper/src/detect.rs`) hears 3 s windows on its own thread; the recognizer opens with the detected primary tag (Apple accepts `id`) and gets the waiting audio. Switching needs the same new language twice in a row; audio is never held for it: the old recognizer keeps hearing while the new one opens, the new one then hears again from where its language began, and its output waits until the old one has said its last words (`Current::held`). Default spoken language is Auto.
- `speech-chunks`: shared by non-streaming engines (Whisper, OpenAI-compatible). It cuts at pauses with an energy VAD. A slow engine never falls further behind: finals that waited are heard in one call (`merge`), and partials are skipped while a call takes longer than `partial_every`.
- `provider-openai`:
  - `location()` is `OnDevice` only for loopback URLs.
  - Keys are refused over non-loopback `http`, and redirects are disabled.
  - Network providers also require `allow_network` (`registry::check_consent`).
  - Each service is one row in `services.rs` (address, default model, extra chat JSON, speech API). Its speech and translate entries share one id, so one key in the keychain.
  - Some models refuse `temperature` or think by default (slow captions); the row's `extra` turns thinking off or down.
- `models`: the one verified download (sha256 of the stored file, gzip unpacked on the fly, a `.part` per download so two of the same file can race). Whisper and Bergamot use it.
- `provider-whisper`: models are downloaded on demand (`models.rs`). Metal on macOS, Vulkan elsewhere; without a usable GPU whisper.cpp runs on the CPU (CPU-only is about 1 s per final on an M1 Pro, three times Metal). `.cargo/config.toml` sets `GGML_NATIVE=OFF`, so a build never targets the build machine's processor.
- `provider-bergamot`: Mozilla's Firefox Translations models run by `fxtranslate` (pure Rust plus a vendored SIMD shim) on the CPU, 20 to 80 ms per sentence, quality close to Apple Translate (FLORES chrF en-id 70.5 vs 72.9). Every model is to or from English; other pairs go through English. `src/catalog.rs` is generated by `catalog.py` from Mozilla's registry and pins every file by sha256. `fxtranslate` is patched in `vendor/fxtranslate` (`OPENLANG.md`): NEON instead of i8mm on aarch64 (i8mm crashed Apple M1), MSVC flags, `Send`. A model missing for a newly detected language downloads in the background while translation reports `ModelMissing`.
- `core/src/cpu.rs`: one `Baseline` per native library, matching how it is built. `Ggml` (Whisper): AVX2 with FMA, F16C and BMI2 on x86_64 (named in `.cargo/config.toml`), ARMv8.2 with dotprod and fp16 on aarch64. `Gemmology` (Bergamot): AVX2 on x86_64, any aarch64. Each checks before loading native code, so an older processor gets a message instead of a crash.
- `audio-coreaudio`: global Core Audio process tap + private aggregate device, excluding this process so spoken output can never be re-captured.
- `audio-cpal`: the microphone everywhere, and system audio on Windows (WASAPI loopback of the default output) and Linux (the PulseAudio monitor of the default output; PipeWire serves the same protocol, and cpal's PulseAudio host is pure Rust). Linux asks for 20 ms buffers: PulseAudio's default delivers recordings in about 2 s chunks. Off macOS a thread checks the default device once a second and invalidates on change. Loopback hears this app's own output too; it plays none today.

**Settings UI is data-driven.** Providers describe settings via `Descriptor.fields`; `ui/settings` renders them. Secrets go to the OS keychain (`settings::Keychain`), never to `settings.json`.

## App shell (`app/`)

- Windows:
  - `caption`: Movie style subtitles on a transparent window, newest sentence only. It hides itself when quiet so it never blocks clicks. Look comes from `Settings::subtitle` (`SubtitleStyle`): `ui/caption/style.js` maps each field to a CSS variable, so a new option is one Rust field, one mapping line and one Settings control.
  - `island`: Morph island controls. The window is resized to the island shape and grows away from the screen edge it is anchored to.
  - `settings`: created on demand, destroyed on close. A labelled sidebar; each area is one entry in `VIEWS` (`ui/settings/settings.js`), shown by URL hash.
- Placement is pure geometry in `placement.rs` (multi-monitor, snap, never overlap) and has unit tests. It works in desktop units: points on macOS and Linux, pixels on Windows, where displays have their own scale (`Monitor::scale`); window sizes are points. `windows.rs` applies it. Dragging is manual (`ui/shared/drag.js` → `drag_start/move/end`), not Tauri drag regions. `watch_displays` re-arranges when displays change or wake; while every display sleeps the monitor list is empty.
- The accessory activation policy must be set in `run()` before windows exist. Otherwise the floating windows cannot appear over other apps' full-screen spaces. `windows::float` sets the collection behaviour and level.
- Start/Stop shortcut: `Settings::hotkey` (default `hotkey::DEFAULT`, Ctrl+Alt+Shift+Space, free on every desktop), recorded in Settings > Controls. `hotkey::replace` keeps the old one when the new one is taken; a shortcut another app owns never stops the app from starting.
- Linux Wayland runs through XWayland (`GDK_BACKEND=x11`, set in `main` when unset) so windows can place themselves, stay on top and take global shortcuts.
- Identifier `dev.openlang.desktop`. `main::folders` moves settings and models from `dev.openlang.app` once; models live in the local (not roaming) data dir. The keychain service stays `dev.openlang.app`.
- `commands::platform` tells the UI the OS and which privacy pages exist (`PRIVACY`); the UI never guesses from the user agent.
- `engine.rs` serializes start/stop through one mutex. `toggle` uses `try_lock` so presses made while start waits on a macOS permission dialog do not pile up.
- Events: `status` (to all windows), `session` (caption/transcript to `caption`, the rest to `island`), `settings`, `preview`, `edges`.

## UI (`ui/`)

- No bundler and no dependencies. ES modules use `window.__TAURI__` (`withGlobalTauri`).
- `shared/tokens.css` is the single source for colours (light and dark), radii, depth, easing, the subtitle outline and spring values ("stiffness damping").
- `shared/motion.js` is a spring engine with one rAF loop that sleeps when everything is at rest. Keep it sleeping when idle.
- Component motion follows https://designbookmark.com/motion. Morph island keeps its original spring values.

## macOS gotchas

- Permissions (system audio tap, microphone) are tied to the code signature. Every ad-hoc rebuild prompts again, and capture start blocks until the dialog is answered. The island then shows "Check the macOS prompt".
- `.cargo/config.toml` sets `MACOSX_DEPLOYMENT_TARGET=26.0` and the `/usr/lib/swift` rpath required by the Swift bridge.
- Translation of the first sentence after opening a session takes 2 to 3 s while the model loads; later ones take about 0.6 s.
