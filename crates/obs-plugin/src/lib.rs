#[cfg(not(windows))]
compile_error!("The SoundTrack OBS plugin is supported on Windows x64 only");

#[cfg(windows)]
mod plugin;
