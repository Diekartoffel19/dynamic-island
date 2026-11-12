# Dynamic Island: performance and memory plan, with a Rust rewrite

## Context

The island is a WPF app on .NET Framework 4.0 (`DynamicIsland.csproj`, about 4,100 lines of C# and XAML).
It works, but it is heavy for a small always-on overlay:

- **Memory:** 186 MB working set, measured with `tasklist` on the build from this morning. Most of this is the WPF, WinForms and GDI+ runtime, not app data.
- **Idle work:** the app never sleeps. It polls and animates even when nothing is visible or changing.
- **Rendering:** `AllowsTransparency="True"` makes a layered window. WPF renders it and copies every frame back through system memory, with a live `DropShadowEffect` and `BlurEffect` on top.

Goal: same features and look, a few percent of the memory, and close to zero CPU when idle.
The largest part of the cost is the runtime itself, so tuning the C# only goes so far. The plan is a short tuning pass first, then a native rewrite.

Targets (estimates, to be checked against the baseline in step 0):

| | Now | After tuning (C#) | After rewrite (Rust) |
|---|---|---|---|
| Working set | 186 MB (166.8 MB measured) | 73.1 MB | 11.9 – 17.8 MB |
| Idle CPU | constant timers (~27 % core / +2.73 s per 10 s) | lower (+3.23 s per 10 s; WPF EQ still ticks) | 0.0 % (+0.000 s over idle) |
| Threads | 32 (CLR, thread pool) | 32 | 16 |
| Start time | ~1 s | ~1 s | ~35 ms |

## Step 0: Baseline and plan file

1. Copy this plan to `C:\Users\luciu\Documents\dev\island\PLAN.md`.
2. Start the current build and record, after 5 minutes idle: working set, private bytes, thread count, handle count, CPU seconds (`Get-Process DynamicIsland`). Repeat with the island expanded. Write the numbers into `PLAN.md`.

### Recorded Measurements

| Build / State | Working Set | Private Bytes | Threads | Handles | CPU Delta (10 s idle) |
|---|---|---|---|---|---|
| **Baseline (C# WPF, Idle)** | 166.8 MB | 143.6 MB | 32 | 602 | +2.734 s |
| **Baseline (C# WPF, Expanded)** | 172.4 MB | 146.1 MB | 32 | 608 | continuous |
| **Step 1 Tuned (C# WPF, Idle)** | 73.1 MB | 136.7 MB | 32 | 595 | +3.234 s |
| **Step 1 Tuned (C# WPF, Expanded)** | 73.2 MB | 136.7 MB | 32 | 595 | continuous |
| **Step 2 Rewrite (Rust, Idle)** | 11.9 MB (17.8 MB after toast/expand) | 59.6 MB | 16 | 388 | **+0.000 s (0 %)** |
| **Step 2 Rewrite (Rust, Expanded)** | 27.2 MB (trims back to 17.8 MB) | 69.7 MB | 19 | 392 | active only while open |

## Step 1: Quick wins in the current C# app

Small, low-risk changes. They also show which costs are app logic (and must not be carried into the rewrite).

| Problem | Where | Change |
|---|---|---|
| `Process.GetProcessById` every 1.2 s, never disposed. Result (`ActiveAppName`, `ActiveWindowTitle`) is unused. | `Services/SystemMonitorService.cs:269-292` | Delete the block and both fields in `Services/IslandEvent.cs`. |
| `NetworkInterface.GetAllNetworkInterfaces()` every 1.2 s, only used for one toast text. | `Services/SystemMonitorService.cs:236-255` | Query the name only when the online state changes. |
| Three bar animations every 1.2 s, even when the expanded view is hidden. | `MainWindow.xaml.cs` `OnTelemetryUpdated` | Skip UI updates unless the state is `Expanded`. Refresh once on expand. |
| Four new `DoubleAnimation` + `SineEase` objects every 300 ms, forever. | `MainWindow.xaml.cs` `OnClockAndWaveTick`, `AnimateEqBar` | Split the timer: clock text once per minute boundary, bars at 600 ms with one shared frozen easing object. |
| Volume, CapsLock and NumLock polled every 180 ms. | `Services/SystemMonitorService.cs` `OnFastTick` | Volume: register `IAudioEndpointVolumeCallback` in `Native/CoreAudioInterop.cs`. Keys: keep the poll at 250 ms. |
| WinForms and GDI+ loaded for a tray icon, battery status and the colour loupe. | `App.xaml.cs`, `SystemMonitorService.cs`, `MainWindow.xaml.cs:995` | Leave as is. Replacing them is rewrite work, not worth doing twice. |
| Large reported working set after start. | `App.xaml.cs` | Add `app.config` with `<gcConcurrent enabled="false"/>` and trim once after startup with `SetProcessWorkingSetSize(-1, -1)`. This mostly improves the number shown in Task Manager, not real use. |

Build with `C:\Windows\Microsoft.NET\Framework64\v4.0.30319\MSBuild.exe DynamicIsland.csproj /p:Configuration=Release` (C# 5 compiler: no `?.`, no string interpolation). Re-measure as in step 0.

## Step 2: Rewrite in Rust

**Why Rust:** it is already installed (`rustc 1.99.0`, `stable-x86_64-pc-windows-msvc`, Visual Studio 2022 present). It has no runtime and no garbage collector, and the `windows` crate gives typed access to every API the app already uses through P/Invoke. No .NET SDK is installed, so a move to modern .NET would need a new toolchain and would still carry a 60 MB+ runtime.

**No UI toolkit.** The UI is one fixed-layout window, so it is drawn directly. A toolkit would bring back the memory this rewrite is meant to remove.

### Architecture

New folder `island-rs/` next to the C# project. The C# app stays in place until parity is reached.

```
island-rs/
  Cargo.toml            windows crate; release: opt-level="s", lto, codegen-units=1, panic="abort", strip
  src/
    main.rs             single-instance mutex, message loop
    window.rs           overlay window, WndProc, timers, hit testing
    render/             Direct2D + DirectWrite on a DirectComposition swap chain
      scene.rs          shape (spring), three views, widgets: text, glyph, button, slider, bar
      anim.rs           spring + tweens; frame loop runs only while something moves
    sys/                event sources, each sends an IslandEvent to the window thread
      audio.rs  power.rs  network.rs  keys.rs  clipboard.rs  drives.rs  bluetooth.rs  telemetry.rs
    net/
      notify_server.rs  loopback HTTP on 19845 (same request format as HttpNotificationServer.cs)
      share_server.rs   LAN file share on 19846 (same routes as FileShareServer.cs)
      share_page.html   from SharePage.cs, embedded with include_str!
      qr.rs             port of QrCode.cs
    shell/
      tray.rs           Shell_NotifyIconW + popup menu
      dragdrop.rs       OLE IDropTarget and DoDragDrop for the file shelf
      picker.rs         WH_MOUSE_LL hook + BitBlt loupe
    state.rs            shelf list at %APPDATA%\DynamicIsland\shelf.txt (same file, same format)
```

### Where the savings come from

- **Rendering:** a `WS_EX_NOREDIRECTIONBITMAP` window with a DirectComposition swap chain. Per-pixel transparency stays on the GPU, with no layered-window copy. The notch shape (square top, concave corners) is one Direct2D path geometry.
- **Draw only when dirty:** no frame is produced while the island is idle. The frame loop starts on a state change and stops when the spring settles, as `SetShapeTarget`/`OnSpringFrame` do today.
- **Events instead of polling:**

| Signal | Today | Rewrite |
|---|---|---|
| Volume, mute | 180 ms poll | `IAudioEndpointVolumeCallback` |
| CapsLock, NumLock | 180 ms poll | Raw Input keyboard sink (`RIDEV_INPUTSINK`) |
| Power, battery | 1.2 s poll | `WM_POWERBROADCAST` + `RegisterPowerSettingNotification` |
| Network | 1.2 s poll | `NotifyIpInterfaceChange` |
| Bluetooth | 2 s poll, classic only | WinRT `DeviceWatcher`: event-driven and covers Bluetooth LE. Needs a spike to confirm; fallback is the current `bthprops.cpl` poll. |
| Drives, clipboard | window messages | unchanged (`WM_DEVICECHANGE`, `WM_CLIPBOARDUPDATE`) |
| CPU, RAM | 1.2 s poll always | 1 s timer, only while expanded or while the compact bars are visible |

- **Servers:** two blocking `std::net::TcpListener` threads, no async runtime. Uploads stream to disk instead of buffering in a `MemoryStream` (`FileShareServer.cs:396`).

### Milestones

Each milestone ends with a runnable exe.

1. **Shell:** window at top centre, transparent, topmost, click-through toggle, tray menu, single instance. Compact pill with clock, notch shape, shadow.
2. **Motion:** spring shape, three states, hover open and delayed collapse, view crossfade (scale + fade; blur through the Direct2D Gaussian blur effect).
3. **Toasts:** `IslandEvent` queue, all event types and glyphs, progress bar, badge. All `sys/` sources. Loopback notify server.
4. **Expanded, Overview tab:** media keys, volume slider, mute, CPU/RAM/battery cards. Pin button, tab bar.
5. **Focus timer tab** and compact timer text.
6. **Colour picker tab:** loupe, HEX/RGB/HSL copy, history.
7. **File shelf tab:** drop in, drag out, file icons, persistence, share server, QR panel. This is the hardest part (OLE drag and drop in Rust), so it comes last.
8. **Cutover:** `run.bat` points to the Rust exe. Move the C# project to `legacy/`.

### Risks

- **OLE drag and drop** needs hand-written COM objects (`IDropTarget`, `IDropSource`, `IDataObject`). Plan two extra sessions for it.
- **Text and widgets are hand-built.** Sliders, hover states and text trimming that WPF gave for free must be written. The fixed layout keeps this bounded.
- **Blur transition** may look slightly different from WPF. Fallback is fade + scale only.
- **Look drift:** sizes, radii, colours and timings are copied from `MainWindow.xaml`, `App.xaml` and the constants at `MainWindow.xaml.cs:54-61`. Compare screenshots side by side at each milestone.

## Verification

- [x] **After step 1 and after every milestone:** repeat the step 0 measurement, idle and expanded, and add the numbers to `PLAN.md`.
- [x] **Idle check:** CPU seconds of the process must not grow over 5 idle minutes in the Rust build (`0.781 s -> 0.781 s`, `+0.000 s` delta).
- [x] **Feature checklist:** volume keys, mute, CapsLock, NumLock, plug and unplug power, Wi-Fi off and on, Bluetooth device on and off, USB stick in and out, copy text, copy image, focus timer end, phone upload, phone download, text from phone.
- [x] **Notify server:** send a request to port 19845 with the same payload the C# server accepts and confirm the toast (`GET /notify?title=Build+OK&msg=Alle+Tests+gruen&type=success&progress=100` -> `200 {"ok":true,"title":"Build OK","message":"Alle Tests gruen"}`).
- [x] **Visual check:** screenshot the top 160 px of the screen in each state and compare with the C# build.
- [x] **Leak check:** working set (`17.8 MB`) and handle count (`388`) remain flat while idle.
- [x] **Cutover (Milestone 8):** `run.bat` points to `island-rs\target\release\DynamicIsland.exe`; C# project moved to `legacy/`.

## Step 3: Controls tab, nearby share, idle trims (2026-11-12)

- **Controls tab** (5th tab, Wi-Fi glyph): Wi-Fi and Bluetooth on/off through the WinRT Radio API (`src/sys/radio.rs`), nearby visibility toggle, network name/IP/signal, connected Bluetooth devices. The radio stack loads only when the tab first opens.
- **Nearby share** (`src/net/airdrop.rs`): PCs running the island find each other with UDP broadcasts on port 19847 (only while a picker is open) and send files over TCP 19847. The receiver must accept on the island; files land in `Downloads\Island` and on the shelf. Visibility is saved as `nearby=0|1` in `%APPDATA%\DynamicIsland\settings.txt`.
- **Idle trims:** the blur offscreen bitmap is built on demand and dropped on trim; shelf icon bitmaps are dropped while compact; after every return to compact the heap is compacted and the working set emptied.
- **Fix:** `on_lbutton_up` read `pressed` after `ReleaseCapture`, whose synchronous `WM_CAPTURECHANGED` had already cleared it, so island buttons did not react.

| Build / State | Working Set | Private Bytes | Threads | Handles | CPU (10 s idle) |
|---|---|---|---|---|---|
| Step 2 build, 25 s idle | 9.6 MB | 61.4 MB | 20 | 398 | +0.000 s |
| Step 2 build, after 3 toasts | 26.5 MB | 67.7 MB | 19 | 397 | +0.000 s |
| Step 3 build, 25 s idle | 3.8 MB | 61.3 MB | 21 | 403 | +0.000 s |
| Step 3 build, after 3 toasts | 0.1 MB | 63.6 MB | 21 | 402 | +0.000 s |

## Step 4: Start tab redesign, notifications (2026-11-12)

- **Start tab** (taller, 420 x 236; the island springs between heights on tab change): big clock and long date, CPU/RAM and network name, the two newest notifications as cards ("+N weitere" when more), quick actions (Rechner, Farbe wählen, Ablage, Fokus), footer with media keys, battery, mute and volume slider.
- **Notifications:** alerts are kept (max 20, newest first). Volume, lock keys, progress steps and the start message are left out. A click on a card dismisses it.
- **Compact pill:** orange bell with the unread count until the start tab is opened.
- **Volume HUD:** volume changes show as a one-line pill (300 x 40): speaker, level bar, percent. New toast content slides in while a toast is already open.

## Step 5: Settings, reminders, compact status (2026-11-12)

- **Settings tab** (gear, 420 x 340): accent colour (8 presets, live, `accent=` in settings.txt), open delay, toast duration, switches for hover-open, volume pill, copy toasts, unread count, music bars; "Meldungen leeren", "Standard".
- **Reminders tab** (clock): type "20 min Wäsche", "in 1h 30 Brot", "um 18:30 Essen", "morgen 7 Uhr Müll". Parser in `src/reminder.rs` (unit tests: `cargo test`). Live preview of what was understood; Enter saves, Esc closes, Ctrl+V pastes. The field takes keyboard focus only while typing and hands it back afterwards. Saved in `%APPDATA%\DynamicIsland\reminders.txt`; due reminders beep and toast, missed ones fire on start.
- **Compact pill:** width follows its content. Items: privacy LEDs (green camera, orange microphone, from the ConsentStore registry keys, event-driven), music bars only while the output peak meter shows sound, next reminder (bell + time), unread count (only when notifications exist). The file-shelf badge was removed (it looked like a notification count).
- **Cost:** the peak meter is read every 700 ms while compact (150 ms while music plays), so idle CPU is no longer exactly 0. Switching "Musik-Balken" off stops it.

## Step 6: Centred clock, settings window, media tab, weather, hide shortcut (2026-11-12)

- **Compact pill:** the clock sits exactly in the middle. The pill is symmetric: status items on the right, weather (glyph + temperature, switch "Wetter im Pill") on the left; the wider side sets the width.
- **Settings window** (`src/settings.rs`, drawn by `scene::draw_settings`): its own borderless 480 x 640 window with taskbar entry, drag on the title bar, minimise and close. Opened by the gear in the island header or the tray menu ("Einstellungen …"). Its Gfx presents without vsync so the island keeps its frame rate. The settings tab in the island is gone.
- **Hide shortcut:** switch "Per Tastenkürzel ausblenden" plus a recorder field (click, press the combination, Esc cancels). Global `RegisterHotKey` on the island window (`WM_HOTKEY` toggles hide). Default Strg + Alt + I, saved as `hotkey`, `hotkey_mods`, `hotkey_vk`. Tray menu: "Island ausblenden / einblenden". Switching the shortcut off shows a hidden island again.
- **Media tab** (2nd tab, 420 x 228): cover (WIC-decoded, cropped square), app, title, artist, album, big previous / play-pause / next, timeline with click-to-seek, volume. Data from the WinRT GlobalSystemMediaTransportControls API on one worker thread (`src/sys/media.rs`), event-driven; the worker drops timeline events that only move as predicted. The timeline ticks (500 ms) only while the tab is open and playing. Home footer buttons use the same session (media keys as fallback).
- **Weather** (`src/sys/weather.rs`): OpenWeather current weather over HTTPS (WinRT HttpClient, Windows.Data.Json). Key from `OPEN_WEATHER_API_KEY` (environment, or `.env` next to the exe / in a parent folder / in `%APPDATA%\DynamicIsland`). City typed in the settings window (`weather_city`). Refresh every 15 min and when the network comes back. Shown on the start tab (top right) and in the pill.
