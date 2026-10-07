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

The standalone CLI remains available as a capture diagnostic. On Windows build 20348 or later, run:

```powershell
cargo run -p soundtrack-audio-capture -- <PID> [seconds] [output.wav]
```

For example:

```powershell
cargo run -p soundtrack-audio-capture -- 1234 10 capture.wav
```

The CLI uses WASAPI Process Loopback in include-process-tree mode and requests 48 kHz, stereo, Float32 audio. It does not change the target application's playback volume. Windows 10 22H2 is below the documented minimum and must be tested separately; parent/child routing isolation also remains an acceptance gate.
