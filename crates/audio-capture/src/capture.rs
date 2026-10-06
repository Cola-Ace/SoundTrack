use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
  AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
  AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
  AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
  AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
  IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
  IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
  PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
  WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{HRESULT, Interface, PCWSTR, Ref, implement};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const FLOAT_BYTES: u16 = 4;
const MAX_CAPTURE_SECONDS: u32 =
  (u32::MAX - 36) / (SAMPLE_RATE * CHANNELS as u32 * FLOAT_BYTES as u32);

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
  event: Arc<EventHandle>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
  fn ActivateCompleted(
    &self,
    _operation: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
  ) -> windows::core::Result<()> {
    unsafe { SetEvent(self.event.0) }?;
    Ok(())
  }
}

struct ComGuard;

impl Drop for ComGuard {
  fn drop(&mut self) {
    unsafe { CoUninitialize() };
  }
}

pub fn capture(pid: u32, seconds: u32) -> Result<Vec<f32>, String> {
  if seconds == 0 || seconds > MAX_CAPTURE_SECONDS {
    return Err(format!(
      "capture duration must be between 1 and {MAX_CAPTURE_SECONDS} seconds"
    ));
  }

  let deadline = Instant::now() + Duration::from_secs(seconds as u64);
  let mut samples = Vec::with_capacity(seconds as usize * SAMPLE_RATE as usize * CHANNELS as usize);
  capture_stream(
    pid,
    || Instant::now() < deadline,
    |_, packet| samples.extend_from_slice(packet),
  )?;
  Ok(samples)
}

pub fn capture_stream(
  pid: u32,
  mut should_continue: impl FnMut() -> bool,
  mut on_packet: impl FnMut(u64, &[f32]),
) -> Result<(), String> {
  if pid == 0 {
    return Err("PID must be greater than zero".to_string());
  }

  unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
    .ok()
    .map_err(|error| format!("CoInitializeEx: {error}"))?;
  let _com = ComGuard;

  let mut activation_params = AUDIOCLIENT_ACTIVATION_PARAMS {
    ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
      ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
        TargetProcessId: pid,
        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
      },
    },
  };
  let mut activation_blob = std::mem::ManuallyDrop::new(PROPVARIANT::default());
  unsafe {
    let inner = &mut *activation_blob.Anonymous.Anonymous;
    inner.vt = VT_BLOB;
    inner.Anonymous.blob.cbSize = std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
    inner.Anonymous.blob.pBlobData =
      (&mut activation_params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast::<u8>();
  }

  let event = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
    .map_err(|error| format!("CreateEventW: {error}"))?;
  let event = Arc::new(EventHandle(event));
  let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler {
    event: event.clone(),
  }
  .into();
  let operation = unsafe {
    ActivateAudioInterfaceAsync(
      VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
      &IAudioClient::IID,
      Some(&*activation_blob as *const PROPVARIANT),
      &handler,
    )
  }
  .map_err(|error| format!("ActivateAudioInterfaceAsync: {error}"))?;

  let activation_wait = unsafe { WaitForSingleObject(event.0, 5_000) };
  if activation_wait == WAIT_TIMEOUT {
    return Err("Process Loopback activation timed out".to_string());
  }
  if activation_wait != WAIT_OBJECT_0 {
    return Err(format!(
      "Process Loopback activation wait failed (status 0x{:08X})",
      activation_wait.0
    ));
  }

  let mut activation_result = HRESULT(0);
  let mut activated_interface = None;
  unsafe { operation.GetActivateResult(&mut activation_result, &mut activated_interface) }
    .map_err(|error| format!("GetActivateResult: {error}"))?;
  activation_result
    .ok()
    .map_err(|error| format!("Process Loopback activation returned {error}"))?;
  let audio_client: IAudioClient = activated_interface
    .ok_or_else(|| "Process Loopback returned no audio interface".to_string())?
    .cast()
    .map_err(|error| format!("IAudioClient query failed: {error}"))?;

  let wave_format = WAVEFORMATEX {
    wFormatTag: 3,
    nChannels: CHANNELS,
    nSamplesPerSec: SAMPLE_RATE,
    nAvgBytesPerSec: SAMPLE_RATE * CHANNELS as u32 * FLOAT_BYTES as u32,
    nBlockAlign: CHANNELS * FLOAT_BYTES,
    wBitsPerSample: FLOAT_BYTES * 8,
    cbSize: 0,
  };
  unsafe {
    audio_client.Initialize(
      AUDCLNT_SHAREMODE_SHARED,
      AUDCLNT_STREAMFLAGS_LOOPBACK
        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
      0,
      0,
      &wave_format,
      None,
    )
  }
  .map_err(|error| format!("IAudioClient::Initialize: {error}"))?;
  unsafe { audio_client.SetEventHandle(event.0) }
    .map_err(|error| format!("IAudioClient::SetEventHandle: {error}"))?;
  let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
    .map_err(|error| format!("IAudioClient::GetService: {error}"))?;
  unsafe { audio_client.Start() }.map_err(|error| format!("IAudioClient::Start: {error}"))?;
  let stop_guard = AudioClientStopGuard {
    client: &audio_client,
    stopped: false,
  };
  let capture_result = (|| {
    while should_continue() {
      let timeout = 200;
      let wait_status = unsafe { WaitForSingleObject(event.0, timeout) };
      if wait_status == WAIT_TIMEOUT {
        continue;
      }
      if wait_status != WAIT_OBJECT_0 {
        return Err(format!(
          "Audio capture wait failed (status 0x{:08X})",
          wait_status.0
        ));
      }

      loop {
        let packet_frames = unsafe { capture_client.GetNextPacketSize() }
          .map_err(|error| format!("IAudioCaptureClient::GetNextPacketSize: {error}"))?;
        if packet_frames == 0 {
          break;
        }

        let mut data = std::ptr::null_mut();
        let mut frames = 0;
        let mut flags = 0;
        let mut device_position = 0;
        let mut qpc_position = 0;
        unsafe {
          capture_client.GetBuffer(
            &mut data,
            &mut frames,
            &mut flags,
            Some(&mut device_position),
            Some(&mut qpc_position),
          )
        }
        .map_err(|error| format!("IAudioCaptureClient::GetBuffer: {error}"))?;

        let sample_count = frames as usize * CHANNELS as usize;
        let buffer_guard = CaptureBufferGuard {
          client: &capture_client,
          frames,
          released: false,
        };
        if flags as i32 & AUDCLNT_BUFFERFLAGS_SILENT.0 != 0 {
          let silence = vec![0.0; sample_count];
          on_packet(qpc_position, &silence);
        } else if !data.is_null() {
          let samples = unsafe { std::slice::from_raw_parts(data.cast::<f32>(), sample_count) };
          on_packet(qpc_position, samples);
        } else {
          return Err("WASAPI returned a null buffer for a non-silent packet".to_string());
        };
        buffer_guard
          .release()
          .map_err(|error| format!("IAudioCaptureClient::ReleaseBuffer: {error}"))?;
      }
    }
    Ok(())
  })();

  let stop_result = stop_guard.stop();
  capture_result?;
  stop_result.map_err(|error| format!("IAudioClient::Stop: {error}"))?;
  Ok(())
}

pub fn write_wav(path: &str, samples: &[f32]) -> Result<(), String> {
  let data_bytes = samples
    .len()
    .checked_mul(std::mem::size_of::<f32>())
    .and_then(|size| u32::try_from(size).ok())
    .ok_or_else(|| "captured audio is too large for a WAV file".to_string())?;
  let riff_size = data_bytes
    .checked_add(36)
    .ok_or_else(|| "captured audio is too large for a WAV file".to_string())?;
  let file = File::create(path).map_err(|error| format!("create {path}: {error}"))?;
  let mut writer = BufWriter::new(file);
  writer.write_all(b"RIFF").map_err(io_error)?;
  writer.write_all(&0_u32.to_le_bytes()).map_err(io_error)?;
  writer.write_all(b"WAVEfmt ").map_err(io_error)?;
  writer.write_all(&16_u32.to_le_bytes()).map_err(io_error)?;
  writer.write_all(&3_u16.to_le_bytes()).map_err(io_error)?;
  writer
    .write_all(&CHANNELS.to_le_bytes())
    .map_err(io_error)?;
  writer
    .write_all(&SAMPLE_RATE.to_le_bytes())
    .map_err(io_error)?;
  writer
    .write_all(&(SAMPLE_RATE * CHANNELS as u32 * FLOAT_BYTES as u32).to_le_bytes())
    .map_err(io_error)?;
  writer
    .write_all(&(CHANNELS * FLOAT_BYTES).to_le_bytes())
    .map_err(io_error)?;
  writer
    .write_all(&(FLOAT_BYTES * 8).to_le_bytes())
    .map_err(io_error)?;
  writer.write_all(b"data").map_err(io_error)?;
  writer
    .write_all(&data_bytes.to_le_bytes())
    .map_err(io_error)?;
  for sample in samples {
    writer.write_all(&sample.to_le_bytes()).map_err(io_error)?;
  }
  writer.flush().map_err(io_error)?;
  let mut file = writer.into_inner().map_err(|error| error.to_string())?;
  file.seek(SeekFrom::Start(4)).map_err(io_error)?;
  file.write_all(&riff_size.to_le_bytes()).map_err(io_error)?;
  Ok(())
}

fn io_error(error: std::io::Error) -> String {
  error.to_string()
}

struct EventHandle(HANDLE);

impl Drop for EventHandle {
  fn drop(&mut self) {
    let _ = unsafe { CloseHandle(self.0) };
  }
}

struct CaptureBufferGuard<'a> {
  client: &'a IAudioCaptureClient,
  frames: u32,
  released: bool,
}

impl CaptureBufferGuard<'_> {
  fn release(mut self) -> windows::core::Result<()> {
    let result = unsafe { self.client.ReleaseBuffer(self.frames) };
    self.released = result.is_ok();
    result
  }
}

impl Drop for CaptureBufferGuard<'_> {
  fn drop(&mut self) {
    if !self.released {
      let _ = unsafe { self.client.ReleaseBuffer(self.frames) };
    }
  }
}

struct AudioClientStopGuard<'a> {
  client: &'a IAudioClient,
  stopped: bool,
}

impl AudioClientStopGuard<'_> {
  fn stop(mut self) -> windows::core::Result<()> {
    let result = unsafe { self.client.Stop() };
    self.stopped = result.is_ok();
    result
  }
}

impl Drop for AudioClientStopGuard<'_> {
  fn drop(&mut self) {
    if !self.stopped {
      let _ = unsafe { self.client.Stop() };
    }
  }
}
