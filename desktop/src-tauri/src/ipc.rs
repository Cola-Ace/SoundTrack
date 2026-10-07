use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::thread;
use std::time::Duration;

use soundtrack_audio_protocol as protocol;
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
  ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
  FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
  ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
  PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::audio::{AudioEngine, MixedAudioFrame};
use crate::model::Route;

const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
const MAX_PENDING_BLOCKS_PER_ROUTE: usize = 3;

pub fn run_server(engine: AudioEngine) {
  loop {
    let mut stream = match create_pipe_instance() {
      Ok(stream) => stream,
      Err(_) => {
        thread::sleep(Duration::from_secs(1));
        continue;
      }
    };

    if let Err(error) = connect_client(&mut stream) {
      eprintln!("SoundTrack OBS named pipe connection failed: {error}");
      continue;
    }

    if let Err(error) = serve_client(&mut stream, &engine) {
      if error.kind() != io::ErrorKind::UnexpectedEof {
        eprintln!("SoundTrack OBS audio client disconnected: {error}");
      }
    }
  }
}

fn create_pipe_instance() -> io::Result<File> {
  let mut descriptor = PSECURITY_DESCRIPTOR::default();
  unsafe {
    ConvertStringSecurityDescriptorToSecurityDescriptorW(
      w!("D:P(A;;GA;;;OW)"),
      SDDL_REVISION_1,
      &mut descriptor,
      None,
    )
    .map_err(io::Error::other)?;
  }
  let _descriptor = LocalSecurityDescriptor(descriptor);
  let attributes = SECURITY_ATTRIBUTES {
    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
    lpSecurityDescriptor: descriptor.0,
    bInheritHandle: BOOL(0),
  };
  let pipe_name = protocol::PIPE_NAME
    .encode_utf16()
    .chain([0])
    .collect::<Vec<_>>();
  let open_mode: FILE_FLAGS_AND_ATTRIBUTES = PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE;
  let pipe = unsafe {
    CreateNamedPipeW(
      PCWSTR(pipe_name.as_ptr()),
      open_mode,
      PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
      1,
      PIPE_BUFFER_SIZE,
      PIPE_BUFFER_SIZE,
      0,
      Some(&attributes as *const _),
    )
  };
  if pipe.is_invalid() {
    return Err(io::Error::last_os_error());
  }

  Ok(unsafe { File::from_raw_handle(pipe.0) })
}

fn connect_client(stream: &mut File) -> io::Result<()> {
  let handle = HANDLE(stream.as_raw_handle().cast());
  match unsafe { ConnectNamedPipe(handle, None) } {
    Ok(()) => Ok(()),
    Err(_) if unsafe { GetLastError() } == ERROR_PIPE_CONNECTED => Ok(()),
    Err(error) => Err(io::Error::other(error)),
  }
}

fn serve_client(stream: &mut File, engine: &AudioEngine) -> io::Result<()> {
  let mut hello_bytes = [0; protocol::CLIENT_HELLO_SIZE];
  stream.read_exact(&mut hello_bytes)?;
  let hello = protocol::decode_client_hello(&hello_bytes);
  let status = protocol::validate_client_hello(hello);
  stream.write_all(&protocol::encode_server_hello(status))?;
  if status != protocol::STATUS_OK {
    return Ok(());
  }

  for route in Route::ALL {
    let _ = engine.take_route_audio(route);
  }

  loop {
    let mut frames = Vec::new();
    for route in Route::ALL {
      if hello.route_mask & route_mask(route) != 0 {
        let queued = engine.take_route_audio(route);
        let first_frame = queued.len().saturating_sub(MAX_PENDING_BLOCKS_PER_ROUTE);
        frames.extend(
          queued
            .into_iter()
            .skip(first_frame)
            .map(|frame| (route_id(route), frame)),
        );
      }
    }
    frames.sort_by_key(|(route_id, frame)| (frame.qpc_time_100ns, *route_id));

    if frames.is_empty() {
      let handle = HANDLE(stream.as_raw_handle().cast());
      unsafe { PeekNamedPipe(handle, None, 0, None, None, None) }.map_err(io::Error::other)?;
      thread::sleep(Duration::from_millis(5));
      continue;
    }
    for (route_id, frame) in frames {
      write_frame(stream, route_id, frame)?;
    }
  }
}

fn write_frame(stream: &mut File, route_id: u8, frame: MixedAudioFrame) -> io::Result<()> {
  if frame.samples.len() != frame.frames as usize * protocol::CHANNELS as usize {
    return Ok(());
  }
  let header = protocol::AudioFrameHeader {
    route_id,
    sequence: frame.sequence,
    frames: frame.frames,
    qpc_time_100ns: frame.qpc_time_100ns,
  };
  stream.write_all(&protocol::encode_frame_header(header))?;

  let mut samples = Vec::with_capacity(frame.samples.len() * protocol::SAMPLE_BYTES);
  for sample in frame.samples {
    samples.extend_from_slice(&sample.to_le_bytes());
  }
  stream.write_all(&samples)
}

fn route_mask(route: Route) -> u16 {
  1u16 << route_id(route)
}

fn route_id(route: Route) -> u8 {
  match route {
    Route::Game => protocol::ROUTE_ID_GAME,
    Route::Media => protocol::ROUTE_ID_MEDIA,
    Route::Voice => protocol::ROUTE_ID_VOICE,
  }
}

struct LocalSecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for LocalSecurityDescriptor {
  fn drop(&mut self) {
    unsafe {
      let _ = LocalFree(Some(HLOCAL(self.0.0)));
    }
  }
}
