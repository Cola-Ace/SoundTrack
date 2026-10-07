pub const PIPE_NAME: &str = r"\\.\pipe\SoundTrack.Audio";
pub const VERSION: u16 = 1;
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
pub const SAMPLE_BYTES: usize = 4;
pub const ROUTE_MASK_ALL: u16 = 0b111;
pub const ROUTE_ID_GAME: u8 = 0;
pub const ROUTE_ID_MEDIA: u8 = 1;
pub const ROUTE_ID_VOICE: u8 = 2;
pub const CLIENT_HELLO_SIZE: usize = 16;
pub const SERVER_HELLO_SIZE: usize = 16;
pub const FRAME_HEADER_SIZE: usize = 24;
pub const MAX_FRAMES_PER_PACKET: u32 = SAMPLE_RATE;

const CLIENT_MAGIC: &[u8; 8] = b"STOBSIPC";
const SERVER_MAGIC: &[u8; 8] = b"STOBACK\0";

pub const STATUS_OK: u16 = 0;
pub const STATUS_BAD_MAGIC: u16 = 1;
pub const STATUS_UNSUPPORTED_VERSION: u16 = 2;
pub const STATUS_INVALID_REQUEST: u16 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientHello {
  pub magic: [u8; 8],
  pub version: u16,
  pub route_mask: u16,
  pub reserved: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFrameHeader {
  pub route_id: u8,
  pub sequence: u64,
  pub frames: u32,
  pub qpc_time_100ns: u64,
}

pub fn encode_client_hello(route_mask: u16) -> [u8; CLIENT_HELLO_SIZE] {
  let mut bytes = [0; CLIENT_HELLO_SIZE];
  bytes[..8].copy_from_slice(CLIENT_MAGIC);
  bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
  bytes[10..12].copy_from_slice(&route_mask.to_le_bytes());
  bytes
}

pub fn decode_client_hello(bytes: &[u8; CLIENT_HELLO_SIZE]) -> ClientHello {
  ClientHello {
    magic: bytes[..8].try_into().expect("fixed-size magic"),
    version: u16::from_le_bytes(bytes[8..10].try_into().expect("fixed-size version")),
    route_mask: u16::from_le_bytes(bytes[10..12].try_into().expect("fixed-size route mask")),
    reserved: u32::from_le_bytes(bytes[12..16].try_into().expect("fixed-size reserved field")),
  }
}

pub fn validate_client_hello(hello: ClientHello) -> u16 {
  if &hello.magic != CLIENT_MAGIC {
    STATUS_BAD_MAGIC
  } else if hello.version != VERSION {
    STATUS_UNSUPPORTED_VERSION
  } else if hello.route_mask == 0 || hello.route_mask & !ROUTE_MASK_ALL != 0 || hello.reserved != 0
  {
    STATUS_INVALID_REQUEST
  } else {
    STATUS_OK
  }
}

pub fn encode_server_hello(status: u16) -> [u8; SERVER_HELLO_SIZE] {
  let mut bytes = [0; SERVER_HELLO_SIZE];
  bytes[..8].copy_from_slice(SERVER_MAGIC);
  bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
  bytes[10..12].copy_from_slice(&status.to_le_bytes());
  bytes
}

pub fn decode_server_hello(bytes: &[u8; SERVER_HELLO_SIZE]) -> Result<(), String> {
  let magic: [u8; 8] = bytes[..8].try_into().expect("fixed-size magic");
  let version = u16::from_le_bytes(bytes[8..10].try_into().expect("fixed-size version"));
  let status = u16::from_le_bytes(bytes[10..12].try_into().expect("fixed-size status"));
  let reserved = u32::from_le_bytes(bytes[12..16].try_into().expect("fixed-size reserved field"));

  if &magic != SERVER_MAGIC {
    return Err("SoundTrack 返回了无效的 OBS 音频协议握手".to_string());
  }
  if version != VERSION || status == STATUS_UNSUPPORTED_VERSION {
    return Err(format!(
      "OBS 音频协议版本不兼容：插件 v{VERSION}，SoundTrack v{version}"
    ));
  }
  if status != STATUS_OK || reserved != 0 {
    return Err(format!(
      "SoundTrack 拒绝了 OBS 音频协议握手（状态 {status}）"
    ));
  }
  Ok(())
}

pub fn encode_frame_header(header: AudioFrameHeader) -> [u8; FRAME_HEADER_SIZE] {
  let mut bytes = [0; FRAME_HEADER_SIZE];
  bytes[0] = header.route_id;
  bytes[1] = 0;
  bytes[2..4].copy_from_slice(&CHANNELS.to_le_bytes());
  bytes[4..12].copy_from_slice(&header.sequence.to_le_bytes());
  bytes[12..16].copy_from_slice(&header.frames.to_le_bytes());
  bytes[16..24].copy_from_slice(&header.qpc_time_100ns.to_le_bytes());
  bytes
}

pub fn decode_frame_header(bytes: &[u8; FRAME_HEADER_SIZE]) -> Result<AudioFrameHeader, String> {
  let route_id = bytes[0];
  let channels = u16::from_le_bytes(bytes[2..4].try_into().expect("fixed-size channel count"));
  let sequence = u64::from_le_bytes(bytes[4..12].try_into().expect("fixed-size sequence"));
  let frames = u32::from_le_bytes(bytes[12..16].try_into().expect("fixed-size frame count"));
  let qpc_time_100ns = u64::from_le_bytes(bytes[16..24].try_into().expect("fixed-size timestamp"));

  if route_id > 2 || bytes[1] != 0 || channels != CHANNELS {
    return Err("SoundTrack 发送了无效的 OBS 音频帧头".to_string());
  }
  if frames == 0 || frames > MAX_FRAMES_PER_PACKET {
    return Err("SoundTrack 发送了无效的 OBS 音频帧长度".to_string());
  }

  Ok(AudioFrameHeader {
    route_id,
    sequence,
    frames,
    qpc_time_100ns,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn client_hello_round_trips_and_validates() {
    let bytes = encode_client_hello(ROUTE_MASK_ALL);
    let hello = decode_client_hello(&bytes);

    assert_eq!(validate_client_hello(hello), STATUS_OK);

    let mut invalid = hello;
    invalid.route_mask = ROUTE_MASK_ALL << 1;
    assert_eq!(validate_client_hello(invalid), STATUS_INVALID_REQUEST);
  }

  #[test]
  fn server_hello_accepts_success_and_rejects_version_mismatch() {
    let success = encode_server_hello(STATUS_OK);
    assert_eq!(decode_server_hello(&success), Ok(()));

    let incompatible = encode_server_hello(STATUS_UNSUPPORTED_VERSION);
    assert!(decode_server_hello(&incompatible).is_err());
  }

  #[test]
  fn audio_frame_header_round_trips_and_rejects_invalid_routes() {
    let header = AudioFrameHeader {
      route_id: ROUTE_ID_MEDIA,
      sequence: 1234,
      frames: 480,
      qpc_time_100ns: 987_654,
    };
    assert_eq!(
      decode_frame_header(&encode_frame_header(header)),
      Ok(header)
    );

    let mut invalid = encode_frame_header(header);
    invalid[0] = ROUTE_ID_VOICE + 1;
    assert!(decode_frame_header(&invalid).is_err());
  }
}
