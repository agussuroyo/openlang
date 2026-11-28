# OpenLang

Live subtitles and translation for anything your computer plays or hears: films, Meet, Zoom,
Teams, recordings and your microphone. It runs on macOS, Windows and Linux, free and on your own
computer. OpenLang does not save audio or text.

| | Speech to text | Translation |
|---|---|---|
| macOS 26+ | Apple Speech (default) or Whisper | Apple Translate (default) or Bergamot |
| Windows, Linux | Whisper | Bergamot (Mozilla's Firefox Translations models) |
| Optional, any OS | OpenAI-compatible services | OpenAI-compatible services |

Whisper and Bergamot download their models the first time you need them. Cloud services
(OpenAI, DeepSeek, OpenRouter, Kimi, Z.ai GLM or any OpenAI-compatible address) stay off until
you allow network use in Settings; their keys go to the system keychain.

## Install

There are no published releases yet. Build it yourself (see [Build](#build)) with
`make install`, or download the `OpenLang-<os>` artifact from a green CI run.

### First launch

The builds are not signed with a paid certificate, so macOS and Windows ask once:

- **macOS:** open the `.dmg` and drag OpenLang to Applications. Right-click OpenLang, choose Open,
  then Open again. Or run `xattr -dr com.apple.quarantine /Applications/OpenLang.app`. On the
  first Start, allow system audio and microphone access.
- **Windows:** run the `.exe` installer. At "Windows protected your PC", click More info, then
  Run anyway.
- **Linux:** `sudo apt install ./OpenLang_*.deb`, or `chmod +x OpenLang_*.AppImage` and run it.
  No warning appears.

After that, OpenLang opens like any other app.

## Use

OpenLang shows two floating windows: the subtitles, and a small control island with Start/Stop.

Start or stop listening from any app with **Control+Option+Shift+Space** on macOS or
**Ctrl+Alt+Shift+Space** on Windows and Linux. The island button and the OpenLang menu in the
menu bar or system tray do the same.

Settings has five areas:

| Area | What you set |
|---|---|
| Translate | The spoken language (Auto by default) and the language to read |
| Listen | System audio, microphone, or both, and links to the system privacy pages |
| Subtitles | Text size, original and/or translation, click-through, position |
| Controls | When the island shows, sound bars, position, the shortcut |
| Engines | Which engine hears and which translates, network use, API keys |

## Build

You need Rust, Node (for the Tauri CLI), `make`, and `cmake` (for Whisper).

- **macOS 26+**, Apple Silicon or Intel: Xcode.
- **Linux**: `libasound2-dev libdbus-1-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev
  librsvg2-dev libclang-dev libvulkan-dev glslc`, with Vulkan headers 1.3.275 or newer (Ubuntu
  24.04, Debian 13). API keys are stored through Secret Service (GNOME Keyring or KWallet).
- **Windows**, x64 or ARM: the Vulkan SDK, `make`, and `app/vulkan-1.dll` built from Khronos
  Vulkan-Loader. Run `make` from Git Bash. ARM needs clang-cl for whisper.cpp; the exact
  environment is in `.github/workflows/ci.yml`.
- x86_64 processors need AVX2.

| Command | Does |
|---|---|
| `make` | Lists the commands |
| `make build` | `.app` and `.dmg`, `.exe` (NSIS), or `.deb` and `.AppImage` |
| `make run` | Builds, then opens it |
| `make install` | Builds, then installs to /Applications, runs the Windows installer, or uses apt |
| `make check` | Format check, clippy, UI syntax |
| `make test` | All tests; `ARGS=...` passes more arguments to `cargo test` |

CI runs these same targets on macOS (Apple Silicon and Intel), Linux (x64 and ARM) and Windows
(x64 and ARM).

macOS ties audio and microphone permission to the app's signature, so it asks again after every
ad-hoc rebuild.

## Develop

| Folder | Contents |
|---|---|
| `crates/core` | Domain types, ports, processing graph, sessions. No OS, UI or provider code |
| `crates/audio-*` | System audio (Core Audio on macOS, cpal elsewhere), microphone, resampling |
| `crates/provider-*` | Apple, Whisper, Bergamot, OpenAI-compatible |
| `crates/speech-chunks` | Cuts speech at pauses for engines that do not stream |
| `crates/models` | Model downloads checked by sha256 |
| `crates/contract` | The test suite every provider must pass |
| `vendor/fxtranslate` | Bergamot engine, patched (see `OPENLANG.md` there) |
| `app` | Tauri shell. `app/src/modules.rs` is the only list of modules |
| `ui` | Plain HTML, CSS and JS, no bundler |

To leave a module out, build without its feature, for example
`cargo build -p openlang --no-default-features --features apple,coreaudio`. Features: `apple`,
`bergamot`, `coreaudio`, `cpal`, `openai`, `whisper`.

To add one, write a crate that implements `SpeechToText`, `Translator` or `AudioSource` and
exposes `register`, then add one feature to `app/Cargo.toml` and one line to `modules.rs`. The
Settings page draws itself from the module's `Descriptor`, so the UI needs no change.

To add a new branch to the pipeline, such as spoken translation or history, implement
`Extension` or `EventSink`. `crates/core/tests/session.rs` shows both.

Tests that speak with `say` run only on macOS. The first `make test` downloads about 60 MB of
Whisper and 120 MB of Bergamot models. Architecture notes live in `CLAUDE.md`.

## Known limits

- Apple Speech cannot detect the language. Auto picks it with a small Whisper model, which takes
  a few seconds of speech; you can also set the language yourself.
- Speakers are not told apart.
- Windows is tested only in CI, without a GPU or audio device. Linux is tested in CI and Docker.
- Without a GPU, Whisper takes about 1 second per sentence.
- On Wayland, OpenLang runs through XWayland (`GDK_BACKEND=x11`) so its windows can place
  themselves, stay on top and use the global shortcut. Set `GDK_BACKEND` yourself to override.
- On Windows, the subtitles do not follow you to another virtual desktop.
