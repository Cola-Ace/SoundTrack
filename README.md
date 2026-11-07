# SoundTrack

SoundTrack is a Windows desktop app for discovering application audio sessions, assigning apps to Game, Media, and Voice routes, and monitoring their mixed levels.

## User guide

See the [简体中文使用指南](docs/USER_GUIDE.zh-CN.md) for the desktop app and OBS plugin setup.

## Desktop app

The Tauri 2 desktop app lives in `desktop/`. From PowerShell 7:

```powershell
cd desktop
pnpm install
pnpm tauri dev
```

Build the Windows desktop package with:

```powershell
pnpm tauri build
```

The app stores settings at `%APPDATA%\SoundTrack\config.json`. Unconfigured apps use Game by default; route assignments, exclusions, gain, and mute settings are persisted. Closing the main window hides the app in the system tray.

The desktop app includes application discovery, Process Loopback capture workers, route mixing, level monitoring, and a current-user Windows named-pipe audio endpoint. The Rust OBS source plugin is in `crates/obs-plugin`; the current Windows x64 build targets OBS Studio 31.1.2 and requires the matching OBS headers and LLVM `libclang` to build. End-to-end audio capture and OBS recording have not yet been verified on Windows hardware.

## Capture prototype

The standalone CLI remains available as a capture diagnostic. On an updated Windows 10 2004 (build 19041) or later, including Windows 10 22H2 and Windows 11, check the system and initialize the capture API with:

```powershell
cargo run -p soundtrack-audio-capture -- --check
```

Then capture a playing application:

```powershell
cargo run -p soundtrack-audio-capture -- <PID> [seconds] [output.wav]
```

For example:

```powershell
cargo run -p soundtrack-audio-capture -- 1234 10 capture.wav
```

The CLI uses WASAPI Process Loopback in include-process-tree mode and requests 48 kHz, stereo, Float32 audio. It does not change the target application's playback volume. Both Windows 10 and Windows 11 use this same capture API. The build 19041 gate follows [OBS 31.1.2](https://github.com/obsproject/obs-studio/blob/31.1.2/plugins/win-wasapi/plugin-main.cpp#L35-L50); Microsoft's documented minimum remains build 20348. Runtime initialization is checked separately, since the version gate alone does not establish API availability on an unpatched system.

Older systems receive an explicit unsupported-system error before capture initialization. There is no device-loopback fallback: recording a device's entire mix would lose per-application isolation. `--check` verifies initialization only; actual audio delivery, parent/child isolation, and OBS recording still require end-to-end verification.
