# Windows Dynamic Notch

> **English:** A Dynamic Island style notch for Windows, written in 100% native Rust
> (winit + DirectComposition, wgpu/DX12 + vello rendering, tokio services; no WebView,
> Electron or Tauri). Widgets: music with synced lyrics, focus timer, stopwatch, water,
> weather, file shelf, clipboard history, to-do, quick controls, system monitor, calendar.
> Popups: volume, brightness, Caps/Num Lock, battery and disk warnings, notifications,
> camera/microphone/screen-capture dots. Top or bottom of any monitor, hides itself in
> fullscreen apps. Build with Docker (`.\build.ps1`) or `cargo build --release`; the
> rest of this document is in Indonesian. Licensed under MIT.
Notch bergaya Dynamic Island di tengah atas (atau bawah) monitor utama Windows. 100% Rust
native: winit + Win32/DirectComposition untuk jendela, wgpu (DirectX 12) +
vello untuk rendering vektor di GPU, cosmic-text untuk teks, tokio untuk
semua I/O, WinRT, dan jaringan. Tanpa WebView, Electron, atau Tauri.

## Fitur

| Mode | Ukuran (px logis) | Isi |
|---|---|---|
| Resting | 160 x 36 | Dua mata yang berkedip tiap 3-6 detik dan sesekali melirik, pendaran biru-ungu di tepi bawah |
| Closed | 190 x 36 | Cincin baterai, jam (atau hitung mundur Pomodoro berwarna merah saat timer berjalan), soundwave 5 batang |
| Music | 420 x 175 | Album art, judul/artis/album, scrubber yang bisa digeser, prev/play-pause/next, tombol lirik |
| Lyrics | 540 x 175 | Player ringkas + lirik sinkron dari LRCLIB: baris aktif bercahaya (glow yang berdenyut) dan terisi kiri ke kanan seperti karaoke, baris lain mengikuti secara elastis dan memudar, titik tiga animasi saat intro/jeda, latar cahaya yang bergerak pelan |
| Focus timer | 420 x 175 | Pomodoro 25/5 menit, cincin merah bercahaya, reset, Short Break / Play / Focus Time, bunyi lonceng saat selesai |
| Stopwatch | 420 x 175 | Stopwatch dengan lap (lap tercepat hijau, terlambat merah) dan timer bebas 10 detik-12 jam (tombol +/-, scroll di angka), bunyi lonceng saat selesai |
| Water | 420 x 175 | Konsumsi hari ini, toggle pengingat, progress bar, Drink/Settings, grafik per jam |
| Weather | 420 x 175 | Suhu, kondisi, H/L, terasa seperti, kelembapan, angin, dan prakiraan 4 jam ke depan dari Open-Meteo (gratis, tanpa API key). Kota diisi sendiri lewat menu atau tombol *Set city*; diperbarui tiap 20 menit |
| Shelf | 420 x 175 | Drop file dari Explorer, drag keluar ke aplikasi lain, klik untuk membuka |
| Clipboard | 420 x 175 | 30 teks terakhir yang di-copy (terbaru di atas), klik untuk copy lagi, pin untuk menyimpan (maks. 20), *Clear* menghapus yang tidak di-pin. Isi yang ditandai privat oleh sumbernya (password manager) tidak pernah dicatat |
| To-do | 420 x 175 | Rasio selesai, tambah/edit/hapus tugas inline, daftar yang bisa di-scroll, disimpan ke JSON |
| Controls | 420 x 175 | Toggle Wi-Fi dan Bluetooth, mode daya (Efficiency / Balanced / Performance), tombol Focus yang membuka pengaturan notifikasi Windows, slider volume + mute |
| System | 420 x 175 | Cincin CPU, RAM, dan disk C: (berubah oranye/merah saat hampir penuh), kecepatan unduh/unggah, grafik riwayat CPU. Hanya mengambil data (tiap 1 detik) selama widget ini tampil |
| Calendar | 420 x 175 | Hari ini (nama hari, tanggal besar, minggu ke-, hari ke-, progres tahun) dan grid bulan; panah untuk pindah bulan, klik nama bulan untuk kembali ke hari ini |
| Headphones | 310 x 76 | Popup 4 detik saat perangkat audio Bluetooth tersambung (nama + baterai) |
| Charging | 310 x 76 | Popup 4 detik saat charger dicolokkan |
| Notification | 380 x 76 | Popup 5 detik untuk notifikasi dari aplikasi lain (ikon/inisial, nama aplikasi, judul, isi) |
| Indicator | 300 x 52 | Popup singkat (1,6 detik): bar volume + status mute, bar kecerahan layar, Caps Lock / Num Lock On/Off; peringatan Low Battery dan Disk C: hampir penuh (6 detik) |

Indikator sistem:

- **Volume & mute**: muncul saat volume perangkat output default berubah (tombol
  volume/mute keyboard, slider Windows) lewat `IAudioEndpointVolume`. Mengikuti
  pergantian perangkat output default.
- **Kecerahan layar**: muncul saat kecerahan layar internal berubah (tombol Fn,
  slider Quick Settings) lewat notifikasi power setting
  `GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS`.
- **Caps Lock / Num Lock**: popup On/Off setiap kali statusnya berubah. Tanpa
  keyboard hook global; status dibaca tiap 200 ms dengan `GetKeyState`.
- **Low Battery**: sekali per siklus discharge saat baterai turun ke batas yang
  ditentukan (default 20%, bisa diisi 1-90% dari menu), disertai bunyi lonceng.
  Direset saat charger dicolokkan atau baterai naik di atas batas.
- **Disk penuh**: dicek tiap 10 menit; sekali per kejadian saat C: melewati batas
  (default 90% terpakai, bisa diisi 50-99% dari menu *Alerts*).
- **Notifikasi**: Windows meminta izin *akses notifikasi* sekali. Daftar
  notifikasi dibaca tiap 2 detik selama fitur aktif (event perubahan hanya untuk
  aplikasi ter-paket); notifikasi yang sudah ada saat start tidak ditampilkan.
- **Warna album**: widget Music dan Lyrics bercahaya dengan warna dominan album
  art, berganti halus antar lagu (bisa dimatikan di *Appearance*).
- **Privasi**: titik hijau saat kamera dipakai, titik oranye saat mikrofon
  dipakai, titik merah saat layar direkam/dibagikan (Windows.Graphics.Capture,
  misalnya share screen Discord/Teams), di tepi notch yang menempel ke layar, terlihat di semua mode. Status
  dibaca dari `CapabilityAccessManager\ConsentStore` (yang juga dipakai ikon
  privasi Windows) dan diperbarui lewat `RegNotifyChangeKeyValue`, tanpa polling.
- **Auto-hide fullscreen**: saat game atau aplikasi fullscreen (D3D exclusive,
  mode presentasi, atau jendela tanpa bingkai yang menutupi seluruh monitor
  utama) berada di depan selama 0,5 detik, notch disembunyikan total sehingga
  tepi layar bisa diklik. Notch muncul lagi begitu aplikasi itu keluar dari
  fullscreen atau tidak lagi di depan. Berbasis event (`SetWinEventHook`), tanpa
  polling.

Interaksi:

- **Hover** pada notch: mengembang (pegas) ke widget terakhir; default Music jika lagu sedang diputar, selain itu Shelf (jika berisi) atau Timer.
- **Kursor keluar**: menunggu 300 ms lalu kembali ke Closed (ada aktivitas) atau Resting.
- **Navigasi widget**: panah kiri/kanan di notch, titik paginasi, scroll horizontal (tilt wheel / touchpad), atau tombol panah keyboard selama kursor di atas notch.
- **Scroll vertikal**: menggulir daftar To-do dan Shelf; di atas angka timer (saat berhenti) mengubah durasi per menit.
- **Klik cincin baterai** di status bar membuka popup Charging, **klik ikon soundwave** membuka popup Headphones.
- **Menu** (klik kanan notch, atau klik ikon notch di System Tray):
  - *Position*: Top / Bottom, *Keep taskbar visible*, dan pilihan monitor jika ada lebih dari satu layar
  - *Widgets*: centang widget yang tampil di carousel (minimal satu)
  - *Appearance*: aksen tema (Aurora, Ocean, Violet, Rose, Mint, Sunset, Mono) dan *Music colors from album art*
  - *Alerts*: peringatan baterai lemah dan disk penuh beserta batasnya (dialog input angka)
  - *Show volume & brightness*, *Show Caps Lock & Num Lock*, *Camera, microphone & screen dots*,
    *Notification popups*, *Clipboard history*, *Hide in fullscreen apps* (semua aktif secara default)
  - *Weather city*: dialog input nama kota
  - *Launch at startup* dan *Quit Dynamic Notch*
- **Posisi**: default-nya notch menempel di tepi atas layar (Y = 0); *Position:
  Bottom* menempelkannya di tepi bawah dan notch mengembang ke atas. Jika taskbar
  ada di tepi yang sama, *Keep taskbar visible* memindahkan notch tepat di
  sebelah taskbar sehingga ikon taskbar tidak tertutup (menu abu-abu jika tidak
  ada taskbar di tepi itu atau taskbar di-auto-hide). Langsung mengikuti jika
  taskbar dipindah atau diubah ukurannya.
- Notch tidak pernah mengambil fokus saat diklik (tombol media tidak mengganggu aplikasi yang sedang dipakai); fokus keyboard hanya diambil selama mengetik tugas.

## Build

Rust tidak perlu terpasang di Windows: build berjalan di Docker (image
`rust:1-bookworm` + MinGW) dan menghasilkan executable Windows
(`x86_64-pc-windows-gnu`).

```powershell
.\build.ps1            # release (LTO) -> dist\windows-dynamic-notch.exe
.\build.ps1 check      # cargo check
.\build.ps1 clippy     # clippy, warning = error
.\build.ps1 test       # build unit test lalu jalankan di Windows
.\build.ps1 installer  # release + installer NSIS -> dist\WindowsDynamicNotch-Setup-1.0.0.exe
```

### Installer

`WindowsDynamicNotch-Setup-1.0.0.exe` memasang aplikasi per pengguna (tanpa hak
admin) ke `%LOCALAPPDATA%\Programs\Windows Dynamic Notch`, membuat shortcut Start
Menu, opsional shortcut Desktop dan *Launch at startup*, serta entri di
*Settings > Apps* untuk uninstall. Uninstaller menanyakan apakah data di
`%LOCALAPPDATA%\WindowsDynamicNotch` ikut dihapus.

Build native dengan toolchain MSVC juga bisa (butuh Rust via rustup dan
Visual Studio Build Tools dengan workload C++):

```powershell
cargo build --release
```

## Menjalankan

Jalankan `dist\windows-dynamic-notch.exe`. Hanya satu instance yang bisa
berjalan. Startup pertama butuh sekitar 5 detik sebelum notch muncul karena
shader compute vello dikompilasi oleh FXC (lihat *Catatan performa*).

Flag untuk pengujian/presentasi (tidak menyimpan data apa pun):

- `--demo` memutar semua mode dengan data contoh.
- `--mode <nama>` menampilkan satu mode dengan data contoh, misalnya `--mode lyrics`.
- `--pin <nama>` menampilkan satu mode dengan data asli (media, baterai, dsb).
- Variabel lingkungan `NOTCH_LOG=debug` untuk log yang lebih rinci.

### Font

Semua teks memakai **Inter** (teks biasa) dan **Inter Display** (teks besar
seperti jam, tanggal, dan angka), versi 4.1 dari rilis resmi
[rsms/inter](https://github.com/rsms/inter). Font-nya ditanam di dalam exe
(`assets/fonts`), jadi tidak perlu dipasang di Windows. Inter berlisensi SIL
Open Font License 1.1 (`assets/fonts/OFL.txt`, ikut terpasang sebagai
`Inter-OFL.txt`). Untuk karakter yang tidak ada di Inter, fallback ke Segoe UI
Variable / Segoe UI. Semua angka memakai fitur OpenType `tnum`.
### Data

Semua disimpan di `%LOCALAPPDATA%\WindowsDynamicNotch`:
`todo.json`, `water.json`, `pomodoro.json`, `shelf.json`, `clipboard.json`, `settings.json`
(posisi, batas baterai, dan toggle fitur dari menu),
cache lirik di `lyrics\`, dan log di `notch.log`. Penulisan file bersifat
atomik (file sementara lalu rename) dan di-debounce di thread tokio.

## Arsitektur

```
src/
  main.rs              entry point: logging, single instance, OLE, tokio runtime, event loop
  app.rs               state machine mode, input, timer, penjadwalan frame, persistensi
  window/win32.rs      style HWND, DirectComposition, subclass WndProc (WM_NCHITTEST),
                       window region, hotkey panah, menu konteks, autostart, clipboard
  window/tray.rs       ikon System Tray (Shell_NotifyIcon) dan dialog input batas baterai
  window/fullscreen.rs deteksi aplikasi fullscreen (SetWinEventHook) untuk auto-hide
  window/dragdrop.rs   COM IDropTarget (+ IDropTargetHelper) dan IDropSource
  animation/spring.rs  SpringValue, RectSpring, ColorSpring (osilator harmonik teredam)
  renderer/mod.rs      wgpu DX12 + vello, blit premultiplied alpha ke swap chain DComp
  renderer/shapes.rs   outline notch (bahu terbalik + sudut squircle), squircle, ring
  renderer/text.rs     cosmic-text shaping + glyph run vello, cache layout, tnum
  renderer/icons.rs    ikon vektor orisinal (grid 24x24)
  services/            media (WinRT GSMTC), lyrics (LRCLIB), power (+ kecerahan), bluetooth,
                       audio (volume/mute), keys (Caps/Num Lock), privacy (kamera/mikrofon),
                       system (CPU/RAM/disk/jaringan), weather (Open-Meteo), controls (radio + mode daya),
                       notifications (UserNotificationListener),
                       shelf (thumbnail Explorer), storage (JSON), sound (chime)
  ui/                  18 view + komponen bersama (status bar, tombol, paginasi)
```

- **Event loop** selalu `ControlFlow::Wait`/`WaitUntil`. Frame hanya dirender
  saat ada pegas yang bergerak atau data berubah; wake-up berikutnya adalah
  deadline terdekat (kedip mata, pergantian menit, detik timer, baris lirik
  berikutnya, collapse 300 ms, popup 4 detik, tengah malam).
- **Pegas**: semi-implicit Euler dengan sub-step 1/480 detik, jadi geraknya
  sama di 60/120/144 Hz. Ekspansi notch: mass 1, stiffness 280, damping 26;
  hover: 400/30. `is_at_rest()` memakai ambang 0,05.
- **Kanvas statis**: jendela selalu 700 x 260 px logis di Y = 0, tidak pernah
  di-resize; ukuran notch hanya digambar ulang di GPU.
- **Service** berjalan di tokio dan mengirim data lewat channel mpsc; event
  loop winit dibangunkan lewat `EventLoopProxy` sekali per batch.

## Catatan performa (diukur di laptop ini: AMD Radeon iGPU, layar 1920 px)

| Kondisi | CPU | Private working set |
|---|---|---|
| Idle, mode Resting (hanya kedipan mata) | 2,3% dari 1 core (0,19% total CPU) | biasanya ~25 MB |
| Musik diputar, mode Closed (soundwave 25 fps) | 5,1% dari 1 core (0,42% total CPU) | 13-26 MB |
| Startup (kompilasi shader) | ~5 detik | puncak ~170 MB, lalu di-trim |

- **RAM**: kolom *Memory* di Task Manager adalah *private working set*.
  Setelah startup angkanya biasanya 13-30 MB; setelah perubahan UI besar bisa
  sempat naik (puncak terukur 47 MB) sampai trim berikutnya (maksimal 10
  detik kemudian). *Commit* (private bytes) tetap sekitar 170 MB: ~45 MB adalah
  runtime D3D12 + driver AMD sebelum apa pun digambar, sisanya hasil kompilasi
  pipeline oleh driver saat pertama dipakai, yang tidak disentuh lagi.
  Aplikasi memangkas working set (`K32EmptyWorkingSet`) setelah 3 detik tanpa
  animasi pegas, paling sering tiap 10 detik dan hanya jika > 32 MB.
- **Buffer vello**: vello 0.11 mengalokasikan ~165 MB buffer GPU tetap; di
  iGPU itu memakan RAM sistem. `vendor/vello_encoding` adalah salinan crate
  resmi dengan ukuran buffer yang disesuaikan untuk notch (~17 MB), lihat
  `vendor/README.md`.
- **120 FPS**: present memakai vsync, jadi animasi berjalan pada refresh rate
  monitor (120 FPS di layar 120 Hz; layar 60 Hz dibatasi 60).
- **Startup ~5 detik**: kompilasi shader vello oleh FXC (`d3dcompiler_47`).
  Bisa dipercepat dengan menyertakan `dxcompiler.dll` + `dxil.dll` (DXC) di
  samping exe; wgpu otomatis memakainya jika ada.

## Keputusan teknis yang perlu diketahui

- **Click-through**: `WM_NCHITTEST` mengembalikan `HTCLIENT` di dalam notch dan
  `HTTRANSPARENT` di luar, sesuai spesifikasi. Namun `HTTRANSPARENT` hanya
  meneruskan input ke jendela *di thread yang sama*, jadi window region juga
  dipersempit ke bounding box notch (+4 px). Di luar region itu klik langsung
  jatuh ke aplikasi lain (sudah diverifikasi dengan `WindowFromPoint`).
- **Transparansi**: `WS_POPUP`, `WS_EX_TOPMOST | WS_EX_TOOLWINDOW |
  WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP`,
  `DwmExtendFrameIntoClientArea(-1)`, dan swap chain DX12 sebagai konten
  visual DirectComposition dengan alpha premultiplied.
- **Panah keyboard tanpa mencuri fokus**: selama kursor di atas notch yang
  mengembang, panah kiri/kanan didaftarkan sebagai hotkey (`RegisterHotKey`)
  dan dilepas lagi saat kursor keluar.
- **Lirik** hanya diminta ke lrclib.net saat mode Lyrics dibuka, lalu di-cache.
- **Bluetooth**: baterai dibaca dari properti PnP Windows untuk headset
  (HFP/AVRCP). Perangkat yang tidak melaporkan baterai tampil "Connected".

## Batasan yang diketahui

- Hanya monitor utama. Dengan taskbar di tepi yang sama dengan notch, notch
  menutupi bagian tengah taskbar kecuali *Keep taskbar visible* diaktifkan.
- Auto-hide fullscreen hanya memperhatikan monitor tempat notch berada.
- Titik rekam layar hanya mendeteksi aplikasi yang memakai Windows.Graphics.Capture;
  perekam yang memakai metode lain (misalnya OBS mode Display Capture lama) tidak tercatat.
- Clipboard history hanya mencatat teks (bukan gambar/file).
- Windows tidak menyediakan API untuk menyalakan Do Not Disturb, jadi tombol Focus
  membuka halaman pengaturannya.
- Popup kecerahan hanya untuk layar internal laptop; monitor eksternal (DDC/CI)
  tidak melaporkan perubahan kecerahan ke Windows.
- Indikator privasi hanya tahu *bahwa* kamera/mikrofon dipakai, bukan oleh
  aplikasi apa (sama seperti informasi di registry Windows). Aplikasi yang
  crash saat memakai mikrofon bisa meninggalkan status "dipakai" sampai
  aplikasi itu dibuka/ditutup lagi.
- Perangkat Bluetooth LE Audio murni tidak terdeteksi (hanya Bluetooth klasik).

## Layanan pihak ketiga, lisensi, dan atribusi

- **Lisensi**: MIT, lihat `LICENSE`.
- **Font**: Inter 4.1 (SIL Open Font License 1.1), lisensinya di `assets/fonts/OFL.txt`.
- **vello_encoding**: salinan crate resmi dengan ukuran buffer yang diubah
  (`vendor/`, lisensi MIT/Apache-2.0 asli ikut di foldernya).
- **Jaringan**: aplikasi hanya menghubungi dua layanan, keduanya tanpa akun dan API key:
  [LRCLIB](https://lrclib.net) untuk lirik (hanya saat widget Lyrics dibuka) dan
  [Open-Meteo](https://open-meteo.com) untuk cuaca dan pencarian kota (hanya jika
  kota diisi). Open-Meteo gratis untuk pemakaian non-komersial; mohon hormati batas
  pemakaian kedua layanan. Tidak ada telemetri.
- **Privasi data**: semua data (tugas, riwayat clipboard, pengaturan) disimpan lokal
  di `%LOCALAPPDATA%\WindowsDynamicNotch` dan tidak pernah dikirim ke mana pun.
- **Installer tidak ditandatangani** (tanpa code signing), jadi Windows SmartScreen
  bisa menampilkan peringatan "Unknown publisher". Pilih *More info > Run anyway*,
  atau build sendiri dari sumber.
- **Merek dagang**: "Dynamic Island" adalah merek Apple. Proyek ini independen,
  tidak berafiliasi dengan Apple, dan tidak memakai aset Apple.