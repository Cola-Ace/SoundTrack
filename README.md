# SoundTrack

[English](README.md) · [简体中文](README.zh-CN.md)

Route Windows application audio into **Game**, **Media**, and **Voice**, then bring each mix into OBS as a separate audio source. Adjust what your audience hears while keeping your own playback volume unchanged.

SoundTrack combines a Rust audio engine, a Tauri 2 / Vue 3 desktop mixer, and a companion OBS plugin. It captures application process trees through WASAPI Process Loopback and sends 48 kHz stereo Float32 audio to OBS over a local named pipe. No virtual audio device is required. The desktop interface is currently in Simplified Chinese.

## Features

- Discover applications with audio sessions; unconfigured apps go to Game automatically.
- Save route assignments and exclusions, or add rules for applications that are not running yet.
- Adjust each route's gain and mute state with live peak meters. The default gain is 50%.
- Match applications by executable path, with optional filename fallback.
- Keep capture running in the system tray when the window is minimized.
- Let OBS start SoundTrack in the background and stop the instance it launched when OBS exits.

Explorer and system sound sessions are excluded. Voice is for application audio, such as voice chat; microphone capture and routing individual browser tabs are outside the current feature set.

## Requirements and status

- **Windows x64:** updated Windows 10 2004 (build 19041) or later, including Windows 10 22H2 and Windows 11. The app also probes Process Loopback initialization at runtime.
- **OBS Studio 31.1.2 x64:** the plugin targets this version's headers and ABI.
- Run SoundTrack and OBS under the same Windows user account.

The project is in development. End-to-end audio capture, process isolation, and OBS loading/recording remain pending hardware verification. Use the capture diagnostic below and check a short OBS recording before relying on a setup.

## Get started

Build the desktop app and OBS plugin using the instructions below, or install an MSI/NSIS package and plugin DLL you have built previously.

1. With OBS closed, copy `soundtrack_obs_plugin.dll` into the OBS installation's `obs-plugins\64bit` directory, then start OBS.
2. Add a **SoundTrack 路由音频** source. In its properties, set **SoundTrack 程序位置** to the installed `soundtrack-desktop.exe` and select Game, Media, or Voice. The executable path is shared by all SoundTrack sources.
3. Open SoundTrack from its tray icon. Play audio in your applications and assign them to the desired routes. Use **添加应用** to create a rule for an offline `.exe`.
4. Add one OBS source for each route you need. Check that the SoundTrack meters and corresponding OBS sources receive audio.
5. To avoid recording the same audio twice, disable overlapping OBS Desktop Audio/application audio captures and leave audio monitoring off for these sources. Use 48 kHz in OBS.

OBS launches SoundTrack in the background after loading the configured source, or when you apply its executable path. An instance you opened manually remains running when OBS exits. Minimize the desktop window to keep it in the tray; closing the window or choosing **退出 SoundTrack** exits the app.

Settings are saved automatically to `%APPDATA%\SoundTrack\config.json`. Gain and mute affect the SoundTrack mix sent to OBS, not the original application's playback.

For recording separate tracks, use OBS Advanced Audio Properties to put all three sources on track 1, then Game on track 2, Media on track 3, and Voice on track 4. Enable those tracks in the recording output settings.

More setup and troubleshooting: [简体中文使用指南](docs/USER_GUIDE.zh-CN.md).

## Build from source

Use PowerShell 7 on Windows with:

- Rust's MSVC toolchain, Visual Studio Build Tools with C++ support, and a Windows SDK.
- Node.js compatible with Vite 8 and pnpm.
- Microsoft Edge WebView2 Runtime for the desktop app.
- For the OBS plugin: LLVM `libclang` and matching OBS Studio 31.1.2 source/build headers.

### Desktop app

```powershell
git clone https://github.com/Cola-Ace/SoundTrack.git
cd SoundTrack/desktop
pnpm install --frozen-lockfile
pnpm tauri dev
```

From `desktop/`, build the frontend or Windows installers:

```powershell
# Type-check and build the frontend
pnpm build

# Build MSI and NSIS installers
pnpm tauri build
```

With the default Cargo target directory, installers are written to `target/release/bundle/msi/` and `target/release/bundle/nsis/` at the repository root. The desktop executable is `target/release/soundtrack-desktop.exe`. NSIS uses a per-user installation.

### OBS plugin

The plugin is a separate Cargo project, outside the root workspace. From the repository root, set the header paths to your own OBS source/build directories:

```powershell
$env:OBS_INCLUDE_DIRS = 'C:\src\obs-studio-31.1.2\libobs;C:\src\obs-build-31.1.2\config;C:\src\obs-build-31.1.2\libobs'
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo build --locked --release --target x86_64-pc-windows-msvc --manifest-path crates/obs-plugin/Cargo.toml
```

The OBS installation alone does not supply the source/build headers. `OBS_INCLUDE_DIRS` is a semicolon-separated list of header directories, including generated headers. The output is `crates/obs-plugin/target/x86_64-pc-windows-msvc/release/soundtrack_obs_plugin.dll`.

See [OBS plugin documentation](crates/obs-plugin/README.md) for build and IPC details.

### Capture diagnostic

From the repository root, probe the capture API, then record a playing application's PID:

```powershell
cargo run -p soundtrack-audio-capture -- --check
cargo run -p soundtrack-audio-capture -- 1234 10 capture.wav
```

Replace `1234` with the target PID. The capture includes its child processes. Duration and output path are optional, defaulting to 10 seconds and `capture.wav`. `--check` tests initialization; listen to the recording to verify audio delivery.

## Project layout

| Path | Purpose |
| --- | --- |
| `desktop/` | Vue 3 / TypeScript interface and Tauri desktop host |
| `crates/audio-capture/` | Windows Process Loopback capture and diagnostic CLI |
| `crates/audio-protocol/` | Shared audio IPC protocol |
| `crates/obs-plugin/` | OBS audio source plugin, built separately |
| `docs/` | User documentation |

## License

[MIT](LICENSE).
