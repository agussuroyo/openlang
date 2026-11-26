# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

OpenLang: desktop live translation of any sound (films, Meet/Zoom/Teams, recordings, microphone). Rust workspace + Tauri 2 shell + plain HTML/CSS/JS UI. macOS first (macOS 26+), designed to stay portable. All UI text is English.

## Commands

`rustup` comes from Homebrew and is not on PATH by default:

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
```

```sh
cargo fmt --all --check
cargo clippy -q --workspace --all-targets -- -D warnings
cargo test --workspace                                   # see test notes below
cargo test -p openlang-core --test session a_stuck       # one test by name filter
cargo test -p openlang-provider-apple -- --nocapture     # real Apple speech/translation
cd app && npx @tauri-apps/cli@2 build                    # .app + .dmg in target/release/bundle/
open target/release/bundle/macos/OpenLang.app
for f in ui/*/*.js; do node --check --input-type=module < $f; done   # UI has no build step
```

Unplug a module: `cargo build -p openlang --no-default-features --features apple,coreaudio` (features: `apple coreaudio cpal openai whisper`).

Test notes:
- Provider tests speak real audio with macOS `say`. They are macOS-only.
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
  - Translation uses `TranslationSession(installedSource:)` with the default high-fidelity strategy (low-latency packs are not installed). It warms up on open.
- `core/src/auto_language.rs`: Auto language for a recognizer that cannot detect (Apple). `LanguageDetector` (Whisper base model, `provider-whisper/src/detect.rs`) hears 3 s windows on its own thread; the recognizer opens with the detected primary tag (Apple accepts `id`) and gets the waiting audio. Switching needs the same new language twice in a row. Default spoken language is Auto.
- `speech-chunks`: shared by non-streaming engines (Whisper, OpenAI-compatible). It cuts at pauses with an energy VAD, and a slow engine skips partials but never finals.
- `provider-openai`:
  - `location()` is `OnDevice` only for loopback URLs.
  - Keys are refused over non-loopback `http`, and redirects are disabled.
  - Network providers also require `allow_network` (`registry::check_consent`).
- `provider-whisper`: models are downloaded on demand with sha256 verification (`models.rs`).
- `audio-coreaudio`: global Core Audio process tap + private aggregate device, excluding this process so spoken output can never be re-captured.

**Settings UI is data-driven.** Providers describe settings via `Descriptor.fields`; `ui/settings` renders them. Secrets go to the OS keychain (`settings::Keychain`), never to `settings.json`.

## App shell (`app/`)

- Windows:
  - `caption`: Movie style subtitles on a transparent window, newest sentence only. It hides itself when quiet so it never blocks clicks. Look comes from `Settings::subtitle` (`SubtitleStyle`): `ui/caption/style.js` maps each field to a CSS variable, so a new option is one Rust field, one mapping line and one Settings control.
  - `island`: Morph island controls. The window is resized to the island shape and grows away from the screen edge it is anchored to.
  - `settings`: created on demand, destroyed on close.
- Placement is pure geometry in `placement.rs` (logical points, multi-monitor, snap, never overlap) and has unit tests. `windows.rs` applies it. Dragging is manual (`ui/shared/drag.js` → `drag_start/move/end`), not Tauri drag regions. `watch_displays` re-arranges when displays change or wake; while every display sleeps the monitor list is empty.
- The accessory activation policy must be set in `run()` before windows exist. Otherwise the floating windows cannot appear over other apps' full-screen spaces. `windows::float` sets the collection behaviour and level.
- `engine.rs` serializes start/stop through one mutex. `toggle` uses `try_lock` so presses made while start waits on a macOS permission dialog do not pile up.
- Events: `status` (to all windows), `session` (caption/transcript to `caption`, the rest to `island`), `settings`, `preview`, `edges`.

## UI (`ui/`)

- No bundler and no dependencies. ES modules use `window.__TAURI__` (`withGlobalTauri`).
- `shared/tokens.css` is the single source for colours (light and dark), radii, depth, easing, the subtitle outline and spring values ("stiffness damping").
- `shared/motion.js` is a spring engine with one rAF loop that sleeps when everything is at rest. Keep it sleeping when idle.
- Component motion follows https://designbookmark.com/motion. Morph island and Bottom bar keep their original spring values.

## macOS gotchas

- Permissions (system audio tap, microphone) are tied to the code signature. Every ad-hoc rebuild prompts again, and capture start blocks until the dialog is answered. The island then shows "Check the macOS prompt".
- `.cargo/config.toml` sets `MACOSX_DEPLOYMENT_TARGET=26.0` and the `/usr/lib/swift` rpath required by the Swift bridge.
- Translation of the first sentence after opening a session takes 2 to 3 s while the model loads; later ones take about 0.6 s.
