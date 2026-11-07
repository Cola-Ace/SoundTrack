#[cfg(not(windows))]
compile_error!("the Process Loopback prototype requires Windows");

#[cfg(windows)]
use soundtrack_audio_capture::{CaptureSupport, capture, probe_capture, write_wav};

#[cfg(windows)]
fn main() {
  if let Err(error) = run() {
    eprintln!("Capture failed: {error}");
    std::process::exit(1);
  }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
  let mut args = std::env::args().skip(1);
  let target = args.next().ok_or_else(|| {
    "usage: soundtrack-audio-capture <PID> [seconds] [output.wav] | --check".to_string()
  })?;
  if target == "--check" {
    if args.next().is_some() {
      return Err("usage: soundtrack-audio-capture --check".to_string());
    }
    println!("{}", CaptureSupport::current().windows_version);
    probe_capture()?;
    println!("Process Loopback initialization succeeded (per-application capture).");
    return Ok(());
  }
  let pid = target
    .parse::<u32>()
    .map_err(|error| format!("invalid PID: {error}"))?;
  if pid == 0 {
    return Err("PID must be greater than zero".to_string());
  }
  let seconds = args
    .next()
    .map(|value| value.parse::<u32>())
    .transpose()
    .map_err(|error| format!("invalid capture duration: {error}"))?
    .unwrap_or(10);
  let output_path = args.next().unwrap_or_else(|| "capture.wav".to_string());
  if args.next().is_some() {
    return Err(
      "too many arguments; usage: soundtrack-audio-capture <PID> [seconds] [output.wav]"
        .to_string(),
    );
  }

  let samples = capture(pid, seconds)?;
  write_wav(&output_path, &samples)?;

  let peak = samples
    .iter()
    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
  println!(
    "Captured {} frames at 48000 Hz stereo Float32 to {} (peak {:.4}).",
    samples.len() / 2,
    output_path,
    peak,
  );
  Ok(())
}
