#[cfg(windows)]
mod capture;

#[cfg(windows)]
pub use capture::{capture, capture_stream, write_wav};
