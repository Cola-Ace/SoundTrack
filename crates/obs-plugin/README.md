# SoundTrack OBS plugin

This Windows x64 OBS module registers the `soundtrack_route_audio` audio input. Its `route` property selects Game, Media, or Voice. One receiver connection subscribes to all three routes and dispatches each frame to every OBS source configured for that route.

The plugin targets OBS Studio 31.1.2 and generates its libobs ABI bindings from the matching official OBS headers. Building requires Rust `x86_64-pc-windows-msvc`, LLVM `libclang`, and OBS 31.1.2 source/build headers. `OBS_INCLUDE_DIRS` must contain the `libobs` header directory and any generated OBS include directories, separated with the normal Windows path separator.

```powershell
$env:OBS_INCLUDE_DIRS = 'C:\src\obs-studio-31.1.2\libobs;C:\src\obs-build-31.1.2\config;C:\src\obs-build-31.1.2\libobs'
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo build --release --target x86_64-pc-windows-msvc --manifest-path crates/obs-plugin/Cargo.toml
```

The module resolves libobs functions from OBS's loaded `libobs.dll`, so an OBS import library is not required. Copy `crates\obs-plugin\target\x86_64-pc-windows-msvc\release\soundtrack_obs_plugin.dll` into the matching OBS installation's `obs-plugins\64bit` directory, then restart OBS.

## Pipe protocol v1

SoundTrack exposes `\\.\pipe\SoundTrack.Audio` with a protected DACL for the pipe owner's Windows account and rejects remote clients. The client sends a 16-byte hello (`STOBSIPC`, version, route mask, reserved field); the server replies with a 16-byte hello (`STOBACK\0`, version, status, reserved field). Integers are little-endian.

Each audio packet has a 24-byte header: route id (Game 0, Media 1, Voice 2), reserved byte, channel count, sequence, frame count, and QPC timestamp in 100-nanosecond units. The payload is interleaved little-endian Float32 stereo at 48 kHz. The OBS source maps the timestamp to OBS's monotonic nanosecond clock before calling `obs_source_output_audio`.
