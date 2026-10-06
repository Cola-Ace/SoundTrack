# SoundTrack

SoundTrack separates application audio into Game, Media, and Voice routes for OBS.

The first implementation milestone is a Windows Process Loopback prototype. It captures one process tree to a WAV file so the capture format, application isolation, and target OS behavior can be measured before the routing service and OBS plugin are built.

## Capture prototype

On Windows build 20348 or later, run:

```powershell
cargo run -p soundtrack-audio-capture -- <PID> [seconds] [output.wav]
```

For example:

```powershell
cargo run -p soundtrack-audio-capture -- 1234 10 capture.wav
```

The capture uses WASAPI Process Loopback in include-process-tree mode and requests 48 kHz, stereo, Float32 audio. It does not change the target application's playback volume. Windows 10 22H2 is below the documented minimum and must be tested separately; parent/child routing isolation also remains an acceptance gate.
