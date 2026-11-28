# OpenLang

Terjemahan langsung untuk suara apa pun di Mac, Windows, dan Linux: film, Meet, Zoom, Teams,
rekaman, dan mikrofon. Mesin bawaannya gratis dan berjalan di komputer ini: Apple di Mac, Whisper
dan Bergamot (model terjemahan Firefox dari Mozilla) di semua OS. Audio dan teks tidak disimpan.

## Menjalankan

Butuh Rust, Node (hanya untuk CLI build), `make`, dan `cmake` (hanya untuk Whisper).

- macOS 26+ (Apple Silicon atau Intel): Xcode.
- Linux: `libasound2-dev libdbus-1-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev
  librsvg2-dev libclang-dev libvulkan-dev glslc` (header Vulkan 1.3.275+, misalnya Ubuntu 24.04
  atau Debian 13). Kunci API disimpan di Secret Service: GNOME Keyring atau KWallet.
- Windows (x64 atau ARM): Vulkan SDK, `make`, dan `app/vulkan-1.dll` yang dibangun dari Khronos
  Vulkan-Loader (lihat `.github/workflows/ci.yml`). Jalankan `make` dari Git Bash. Di ARM,
  whisper.cpp dibangun dengan clang-cl (`CMAKE_GENERATOR_TOOLSET=ClangCL`). Prosesor x86_64 perlu
  AVX2.

```sh
make            # daftar perintah
make build      # .app dan .dmg, .exe (NSIS), atau .deb dan .AppImage
make run        # build, lalu buka
make install    # build, lalu pasang: /Applications, installer Windows, atau apt
make check      # format, lint, sintaks UI
make test       # semua uji
```

CI menjalankan perintah `make` yang sama di macOS (Apple Silicon dan Intel), Linux (x64 dan ARM),
dan Windows (x64 dan ARM); hasil build ada di artifact tiap job.

Start/Stop: Control+Option+Shift+Space di Mac, Ctrl+Alt+Shift+Space di Windows dan Linux (bisa
diganti di Settings > Controls), tombol di island, atau menu OpenLang di menu bar atau system tray.
Izin macOS melekat pada tanda tangan app. Build ad-hoc baru akan ditanya izin lagi.

Sejak identifier `dev.openlang.desktop`, settings dan model dari `dev.openlang.app` dipindahkan
sekali saat app dibuka; macOS menanyakan izin audio dan mikrofon sekali lagi. Di Windows model
disimpan di AppData Local, bukan Roaming.

## Membuka pertama kali

Build OpenLang gratis dan tidak ditandatangani (tanpa sertifikat berbayar), jadi sistem bertanya
sekali saat pertama dibuka:

- macOS: buka `.dmg`, seret OpenLang ke Applications, lalu klik kanan OpenLang > Open > Open.
  Atau lewat Terminal: `xattr -dr com.apple.quarantine /Applications/OpenLang.app`.
  Saat Start pertama, izinkan audio sistem dan mikrofon.
- Windows: jalankan installer `.exe`; di "Windows protected your PC" klik More info > Run anyway.
- Linux: `sudo apt install ./OpenLang_*.deb`, atau jadikan AppImage bisa dijalankan:
  `chmod +x OpenLang_*.AppImage`. Tidak ada peringatan.

Setelah itu OpenLang terbuka seperti app biasa.

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
make test   # memakai `say` di Mac; mengunduh model Whisper 60 MB dan Bergamot 120 MB sekali
```

Uji yang memakai `say` hanya berjalan di Mac; Whisper tetap dimuat dan diuji tanpa suara di
Windows dan Linux.

## Batasan

- Windows hanya diuji di CI (tanpa GPU dan tanpa perangkat audio). Linux diuji di CI dan Docker.
- Di Linux Wayland, app berjalan lewat XWayland (`GDK_BACKEND=x11`) agar jendela bisa diletakkan,
  tetap di atas, dan shortcut global bekerja. Set `GDK_BACKEND` sendiri untuk memilih lain.
- Windows tidak memindahkan subtitle ke virtual desktop lain.
- Build tidak ditandatangani: macOS dan Windows bertanya sekali saat pertama dibuka (lihat
  "Membuka pertama kali").
- Di Windows dan Linux, Whisper memakai GPU lewat Vulkan; tanpa GPU sekitar 1 detik per kalimat.
- Loopback Windows dan monitor Linux juga mendengar suara aplikasi ini sendiri (saat ini tidak ada).
- Apple Speech tidak mendeteksi bahasa; pilih bahasanya, atau pakai Whisper.
- Pembicara di audio sistem tidak dipisahkan.
- Ikon aplikasi masih placeholder polos.

Gerak Morph island dan Bottom bar mengikuti https://designbookmark.com/motion.
