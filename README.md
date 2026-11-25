# OpenLang

Terjemahan langsung untuk suara apa pun di Mac: film, Meet, Zoom, Teams, rekaman, dan mikrofon.
Mesin bawaan (Apple) gratis dan berjalan di komputer ini. Audio dan teks tidak disimpan.

## Menjalankan

Butuh macOS 26+, Xcode, Rust, Node (hanya untuk CLI build), dan `cmake` (hanya untuk Whisper).

```sh
cd app && npx @tauri-apps/cli@2 build
open ../target/release/bundle/macos/OpenLang.app
```

Start/Stop: Control+Option+L, tombol di island, atau menu OpenLang di menu bar.
Izin macOS melekat pada tanda tangan app. Build ad-hoc baru akan ditanya izin lagi.

## Susunan

| Folder | Isi |
|---|---|
| `crates/core` | Domain, port, graf, sesi. Tidak mengenal OS, UI, atau provider |
| `crates/audio-*` | Audio Mac, mikrofon, resampler |
| `crates/provider-*` | Apple, Whisper, OpenAI-compatible |
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
cargo test --workspace   # memakai `say`; uji Whisper mengunduh model 60 MB sekali
```

## Batasan

- Windows dan Linux belum diuji; audio sistem di sana belum ada.
- Apple Speech tidak mendeteksi bahasa; pilih bahasanya, atau pakai Whisper.
- Pembicara di audio sistem tidak dipisahkan.
- Ikon aplikasi masih placeholder polos.

Gerak Read along, Morph island, dan Bottom bar mengikuti https://designbookmark.com/motion.
