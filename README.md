# OpenLang

Terjemahan langsung untuk suara apa pun di Mac, Windows, dan Linux: film, Meet, Zoom, Teams,
rekaman, dan mikrofon. Mesin bawaannya gratis dan berjalan di komputer ini: Apple di Mac, Whisper
dan Bergamot (model terjemahan Firefox dari Mozilla) di semua OS. Audio dan teks tidak disimpan.

## Menjalankan

Butuh Rust, Node (hanya untuk CLI build), dan `cmake` (hanya untuk Whisper).

- macOS 26+: Xcode.
- Linux: `libasound2-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
  libssl-dev libclang-dev libvulkan-dev glslc` (header Vulkan 1.3.275+, misalnya Ubuntu 24.04 atau
  Debian 13).
- Windows: Vulkan SDK, dan `app/vulkan-1.dll` yang dibangun dari Khronos Vulkan-Loader (lihat
  `.github/workflows/ci.yml`). Prosesor x86_64 perlu AVX2.

```sh
cd app && npx @tauri-apps/cli@2 build   # .dmg, .exe (NSIS), atau .deb dan .AppImage
open ../target/release/bundle/macos/OpenLang.app
```

CI membangun dan menguji ketiganya; hasil build ada di artifact tiap job.

Start/Stop: Control+Option+L (Ctrl+Alt+L di Windows dan Linux), tombol di island, atau menu
OpenLang di menu bar atau system tray.
Izin macOS melekat pada tanda tangan app. Build ad-hoc baru akan ditanya izin lagi.

## Susunan

| Folder | Isi |
|---|---|
| `crates/core` | Domain, port, graf, sesi. Tidak mengenal OS, UI, atau provider |
| `crates/audio-*` | Audio sistem (Mac, Windows, Linux), mikrofon, resampler |
| `crates/provider-*` | Apple, Whisper, Bergamot, OpenAI-compatible |
| `crates/models` | Unduhan model yang diverifikasi sha256 |
| `vendor/fxtranslate` | Mesin Bergamot, sudah ditambal (`OPENLANG.md`) |
| `crates/speech-chunks` | Potong suara per ucapan, untuk mesin non-streaming |
| `crates/contract` | Uji yang sama untuk semua provider |
| `app` | Shell Tauri. `app/src/modules.rs` = satu-satunya daftar modul |
| `ui` | HTML/CSS/JS tanpa bundler |

## Pasang atau copot modul

Copot: matikan fitur, misalnya `--no-default-features --features apple,coreaudio`.

Pasang: buat crate yang mengimplementasikan `SpeechToText`, `Translator`, atau `AudioSource`, beri
fungsi `register`, lalu tambahkan satu fitur di `app/Cargo.toml` dan satu baris di `modules.rs`.
Settings digambar dari `Descriptor`, jadi UI tidak perlu diubah.

Cabang baru (suara terjemahan, history, notulen): implementasikan `Extension` atau `EventSink`.
Contohnya ada di `crates/core/tests/session.rs`. Keduanya belum ada di aplikasi.

## Uji

```sh
cargo test --workspace   # memakai `say` di Mac; mengunduh model Whisper 60 MB dan Bergamot 120 MB sekali
```

## Batasan

- Windows hanya diuji di CI (tanpa GPU dan tanpa perangkat audio). Linux diuji di CI dan Docker.
- Di Windows dan Linux, Whisper memakai GPU lewat Vulkan; tanpa GPU sekitar 1 detik per kalimat.
- Loopback Windows dan monitor Linux juga mendengar suara aplikasi ini sendiri (saat ini tidak ada).
- Apple Speech tidak mendeteksi bahasa; pilih bahasanya, atau pakai Whisper.
- Pembicara di audio sistem tidak dipisahkan.
- Ikon aplikasi masih placeholder polos.

Gerak Morph island dan Bottom bar mengikuti https://designbookmark.com/motion.
