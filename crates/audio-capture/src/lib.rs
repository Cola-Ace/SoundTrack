#[cfg(windows)]
mod capture;

#[cfg(windows)]
mod support;

#[cfg(windows)]
pub use capture::{capture, capture_stream, write_wav};

#[cfg(windows)]
pub use support::{CaptureSupport, probe_capture};
