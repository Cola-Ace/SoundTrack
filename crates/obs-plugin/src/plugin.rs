use std::ffi::{CStr, CString, c_char, c_void};
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use soundtrack_audio_protocol as protocol;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::{PCSTR, w};

mod bindings {
  include!(concat!(env!("OUT_DIR"), "/obs_bindings.rs"));
}

use bindings::{
  SOUNDTRACK_AUDIO_FORMAT_FLOAT, SOUNDTRACK_COMBO_FORMAT_STRING, SOUNDTRACK_COMBO_TYPE_LIST,
  SOUNDTRACK_OBS_API_VERSION, SOUNDTRACK_PATH_FILE, SOUNDTRACK_SOURCE_AUDIO,
  SOUNDTRACK_SOURCE_TYPE_INPUT, SOUNDTRACK_SPEAKERS_STEREO, SOUNDTRACK_TEXT_INFO, obs_data_t,
  obs_module_t, obs_properties_t, obs_property_t, obs_source_audio, obs_source_info, obs_source_t,
};

type RegisterSourceFn = unsafe extern "C" fn(*const obs_source_info, usize);
type OutputAudioFn = unsafe extern "C" fn(*mut obs_source_t, *const obs_source_audio);
type DataGetStringFn = unsafe extern "C" fn(*mut obs_data_t, *const c_char) -> *const c_char;
type DataSetDefaultStringFn = unsafe extern "C" fn(*mut obs_data_t, *const c_char, *const c_char);
type PropertiesCreateFn = unsafe extern "C" fn() -> *mut obs_properties_t;
type PropertiesAddListFn = unsafe extern "C" fn(
  *mut obs_properties_t,
  *const c_char,
  *const c_char,
  bindings::obs_combo_type,
  bindings::obs_combo_format,
) -> *mut obs_property_t;
type PropertyListAddStringFn =
  unsafe extern "C" fn(*mut obs_property_t, *const c_char, *const c_char) -> usize;
type PropertiesAddTextFn = unsafe extern "C" fn(
  *mut obs_properties_t,
  *const c_char,
  *const c_char,
  bindings::obs_text_type,
) -> *mut obs_property_t;
type PropertiesAddPathFn = unsafe extern "C" fn(
  *mut obs_properties_t,
  *const c_char,
  *const c_char,
  bindings::obs_path_type,
  *const c_char,
  *const c_char,
) -> *mut obs_property_t;
type PropertySetDescriptionFn = unsafe extern "C" fn(*mut obs_property_t, *const c_char);
type GetTimeNsFn = unsafe extern "C" fn() -> u64;
type BlogFn = unsafe extern "C" fn(i32, *const c_char, ...);
type FrontendEventCallbackFn = unsafe extern "C" fn(i32, *mut c_void);
type FrontendEventFn = unsafe extern "C" fn(Option<FrontendEventCallbackFn>, *mut c_void);

#[derive(Clone, Copy)]
struct ObsApi {
  register_source: RegisterSourceFn,
  output_audio: OutputAudioFn,
  data_get_string: DataGetStringFn,
  data_set_default_string: DataSetDefaultStringFn,
  properties_create: PropertiesCreateFn,
  properties_add_list: PropertiesAddListFn,
  property_list_add_string: PropertyListAddStringFn,
  properties_add_text: PropertiesAddTextFn,
  properties_add_path: PropertiesAddPathFn,
  property_set_description: PropertySetDescriptionFn,
  get_time_ns: GetTimeNsFn,
  blog: Option<BlogFn>,
}

#[derive(Clone, Copy)]
struct FrontendApi {
  add_event_callback: FrontendEventFn,
  remove_event_callback: FrontendEventFn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
  Game,
  Media,
  Voice,
}

impl Route {
  fn from_c_string(value: *const c_char) -> Self {
    if value.is_null() {
      return Self::Game;
    }
    match unsafe { CStr::from_ptr(value) }.to_bytes() {
      b"media" => Self::Media,
      b"voice" => Self::Voice,
      _ => Self::Game,
    }
  }

  fn id(self) -> u8 {
    match self {
      Self::Game => protocol::ROUTE_ID_GAME,
      Self::Media => protocol::ROUTE_ID_MEDIA,
      Self::Voice => protocol::ROUTE_ID_VOICE,
    }
  }
}

struct SourceRegistration {
  source: usize,
  route: Route,
  app_path: Option<PathBuf>,
}

struct SourceData {
  source: usize,
}

struct Runtime {
  sources: Mutex<Vec<SourceRegistration>>,
  status: RwLock<String>,
  stopping: AtomicBool,
  receiver_thread: Mutex<Option<JoinHandle<()>>>,
  finished_loading: AtomicBool,
  app_launcher: Mutex<AppLauncher>,
}

#[derive(Default)]
struct AppLauncher {
  configured_path: Option<PathBuf>,
  child: Option<Child>,
}

impl Runtime {
  fn new() -> Self {
    Self {
      sources: Mutex::new(Vec::new()),
      status: RwLock::new("等待 SoundTrack 连接".to_string()),
      stopping: AtomicBool::new(false),
      receiver_thread: Mutex::new(None),
      finished_loading: AtomicBool::new(false),
      app_launcher: Mutex::new(AppLauncher::default()),
    }
  }

  fn set_status(&self, status: String) {
    *self.status.write() = status;
  }
}

impl AppLauncher {
  fn update(&mut self, path: Option<PathBuf>, finished_loading: bool) {
    if path.is_some() || self.child.is_none() {
      if self.configured_path != path {
        self.stop();
        self.configured_path = path;
      }
    }
    if !finished_loading || self.child.is_some() {
      return;
    }
    let Some(path) = self.configured_path.as_ref() else {
      return;
    };
    if !path.is_file() {
      debug_message(&format!(
        "SoundTrack OBS plugin: configured application path is not a file: {}",
        path.display()
      ));
      return;
    }

    match Command::new(path).arg("--background").spawn() {
      Ok(child) => self.child = Some(child),
      Err(error) => debug_message(&format!(
        "SoundTrack OBS plugin: failed to start {}: {error}",
        path.display()
      )),
    }
  }

  fn stop(&mut self) {
    let Some(mut child) = self.child.take() else {
      return;
    };
    if matches!(child.try_wait(), Ok(Some(_))) {
      return;
    }
    if let Some(path) = self.configured_path.as_ref() {
      if let Ok(mut shutdown_request) = Command::new(path).arg("--shutdown").spawn() {
        let _ = shutdown_request.wait();
        for _ in 0..20 {
          if matches!(child.try_wait(), Ok(Some(_))) {
            return;
          }
          thread::sleep(Duration::from_millis(50));
        }
      }
    }
    if let Err(error) = child.kill() {
      debug_message(&format!(
        "SoundTrack OBS plugin: failed to stop SoundTrack: {error}"
      ));
    }
    let _ = child.wait();
  }
}

static API: OnceLock<ObsApi> = OnceLock::new();
static FRONTEND_API: OnceLock<FrontendApi> = OnceLock::new();
static OBS_BLOG: OnceLock<BlogFn> = OnceLock::new();
static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();
static MODULE_POINTER: AtomicPtr<obs_module_t> = AtomicPtr::new(ptr::null_mut());
static mut SOURCE_INFO: std::mem::MaybeUninit<obs_source_info> = std::mem::MaybeUninit::uninit();
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_module_set_pointer(module: *mut obs_module_t) {
  MODULE_POINTER.store(module, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_ver() -> u32 {
  SOUNDTRACK_OBS_API_VERSION as u32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_module_load() -> bool {
  let api = match unsafe { load_obs_api() } {
    Ok(api) => api,
    Err(error) => {
      debug_message(&format!("SoundTrack OBS plugin failed to load: {error}"));
      return false;
    }
  };
  if API.set(api).is_err() {
    debug_message("SoundTrack OBS plugin failed to load: OBS API was initialized more than once");
    return false;
  }

  let frontend_api = match unsafe { load_frontend_api() } {
    Ok(api) => api,
    Err(error) => {
      debug_message(&format!("SoundTrack OBS plugin failed to load: {error}"));
      return false;
    }
  };
  if FRONTEND_API.set(frontend_api).is_err() {
    debug_message(
      "SoundTrack OBS plugin failed to load: Frontend API was initialized more than once",
    );
    return false;
  }

  let runtime = Arc::new(Runtime::new());
  if RUNTIME.set(runtime.clone()).is_err() {
    debug_message("SoundTrack OBS plugin failed to load: runtime was initialized more than once");
    return false;
  }

  let mut source_info: obs_source_info = unsafe { std::mem::zeroed() };
  source_info.id = c"soundtrack_route_audio".as_ptr();
  source_info.type_ = SOUNDTRACK_SOURCE_TYPE_INPUT;
  source_info.output_flags = SOUNDTRACK_SOURCE_AUDIO as u32;
  source_info.get_name = Some(source_name);
  source_info.create = Some(source_create);
  source_info.destroy = Some(source_destroy);
  source_info.get_defaults = Some(source_defaults);
  source_info.get_properties = Some(source_properties);
  source_info.update = Some(source_update);

  let source_info_ptr = ptr::addr_of_mut!(SOURCE_INFO).cast::<obs_source_info>();
  unsafe {
    source_info_ptr.write(source_info);
    (api.register_source)(source_info_ptr, std::mem::size_of::<obs_source_info>());
  }

  let receiver_runtime = runtime.clone();
  let receiver_thread = thread::spawn(move || receiver_loop(receiver_runtime, api));
  *runtime.receiver_thread.lock() = Some(receiver_thread);
  unsafe { (frontend_api.add_event_callback)(Some(frontend_event), ptr::null_mut()) };
  true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_module_unload() {
  if let Some(api) = FRONTEND_API.get() {
    unsafe { (api.remove_event_callback)(Some(frontend_event), ptr::null_mut()) };
  }
  let Some(runtime) = RUNTIME.get() else {
    return;
  };
  runtime.finished_loading.store(false, Ordering::Release);
  runtime.app_launcher.lock().stop();
  runtime.stopping.store(true, Ordering::Release);
  if let Some(thread) = runtime.receiver_thread.lock().take() {
    unsafe {
      let _ = CancelSynchronousIo(HANDLE(thread.as_raw_handle().cast()));
    }
    let _ = thread.join();
  }
}

unsafe fn load_obs_api() -> Result<ObsApi, String> {
  let module = unsafe { GetModuleHandleW(w!("obs.dll")) }
    .map_err(|error| format!("obs.dll is unavailable: {error}"))?;
  let blog = unsafe {
    GetProcAddress(module, PCSTR(c"blog".as_ptr().cast()))
      .map(|procedure| std::mem::transmute::<_, BlogFn>(procedure))
  };
  if let Some(blog) = blog {
    let _ = OBS_BLOG.set(blog);
  }

  macro_rules! resolve {
    ($name:literal, $type:ty) => {{
      let procedure =
        unsafe { GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr().cast())) }
          .ok_or_else(|| format!("obs.dll is missing {}", $name))?;
      let address = procedure as *const ();
      unsafe { std::mem::transmute::<*const (), $type>(address) }
    }};
  }

  Ok(ObsApi {
    register_source: resolve!("obs_register_source_s", RegisterSourceFn),
    output_audio: resolve!("obs_source_output_audio", OutputAudioFn),
    data_get_string: resolve!("obs_data_get_string", DataGetStringFn),
    data_set_default_string: resolve!("obs_data_set_default_string", DataSetDefaultStringFn),
    properties_create: resolve!("obs_properties_create", PropertiesCreateFn),
    properties_add_list: resolve!("obs_properties_add_list", PropertiesAddListFn),
    property_list_add_string: resolve!("obs_property_list_add_string", PropertyListAddStringFn),
    properties_add_text: resolve!("obs_properties_add_text", PropertiesAddTextFn),
    properties_add_path: resolve!("obs_properties_add_path", PropertiesAddPathFn),
    property_set_description: resolve!("obs_property_set_description", PropertySetDescriptionFn),
    get_time_ns: resolve!("os_gettime_ns", GetTimeNsFn),
    blog,
  })
}

unsafe fn load_frontend_api() -> Result<FrontendApi, String> {
  let module = unsafe { GetModuleHandleW(w!("obs-frontend-api.dll")) }
    .map_err(|error| format!("obs-frontend-api.dll is unavailable: {error}"))?;
  macro_rules! resolve {
    ($name:literal) => {{
      let procedure =
        unsafe { GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr().cast())) }
          .ok_or_else(|| format!("obs-frontend-api.dll is missing {}", $name))?;
      let address = procedure as *const ();
      unsafe { std::mem::transmute::<*const (), FrontendEventFn>(address) }
    }};
  }
  Ok(FrontendApi {
    add_event_callback: resolve!("obs_frontend_add_event_callback"),
    remove_event_callback: resolve!("obs_frontend_remove_event_callback"),
  })
}

// Event values from the OBS 31.1.2 frontend API enum.
const FRONTEND_EVENT_EXIT: i32 = 17;
const FRONTEND_EVENT_FINISHED_LOADING: i32 = 26;

unsafe extern "C" fn frontend_event(event: i32, _private_data: *mut c_void) {
  let Some(runtime) = RUNTIME.get() else {
    return;
  };
  match event {
    FRONTEND_EVENT_FINISHED_LOADING => {
      runtime.finished_loading.store(true, Ordering::Release);
      synchronize_app_launcher(runtime);
    }
    FRONTEND_EVENT_EXIT => {
      runtime.finished_loading.store(false, Ordering::Release);
      runtime.app_launcher.lock().stop();
    }
    _ => {}
  }
}

fn synchronize_app_launcher(runtime: &Runtime) {
  let path = runtime
    .sources
    .lock()
    .iter()
    .find_map(|source| source.app_path.clone());
  let finished_loading = runtime.finished_loading.load(Ordering::Acquire);
  runtime.app_launcher.lock().update(path, finished_loading);
}

fn receiver_loop(runtime: Arc<Runtime>, api: ObsApi) {
  while !runtime.stopping.load(Ordering::Acquire) {
    match connect_and_receive(&runtime, api) {
      Ok(()) => runtime.set_status("SoundTrack 音频连接已关闭，正在重连".to_string()),
      Err(error) => {
        let message = error.to_string();
        if message.starts_with("OBS 音频协议版本不兼容") {
          runtime.set_status(message.clone());
          log_message(api, 200, &format!("SoundTrack OBS plugin: {message}"));
        } else if !runtime.stopping.load(Ordering::Acquire) {
          runtime.set_status("等待 SoundTrack 连接".to_string());
        }
      }
    }
    for _ in 0..10 {
      if runtime.stopping.load(Ordering::Acquire) {
        return;
      }
      thread::sleep(Duration::from_millis(100));
    }
  }
}

fn connect_and_receive(runtime: &Runtime, api: ObsApi) -> io::Result<()> {
  let mut stream = OpenOptions::new()
    .read(true)
    .write(true)
    .open(protocol::PIPE_NAME)?;
  runtime.set_status("已连接 SoundTrack".to_string());

  stream.write_all(&protocol::encode_client_hello(protocol::ROUTE_MASK_ALL))?;
  let mut reply = [0; protocol::SERVER_HELLO_SIZE];
  stream.read_exact(&mut reply)?;
  protocol::decode_server_hello(&reply).map_err(io::Error::other)?;
  let timestamp_offset_ns = clock_offset_ns(api).unwrap_or_default();
  let mut last_sequence = [None; 3];

  while !runtime.stopping.load(Ordering::Acquire) {
    let mut bytes = [0; protocol::FRAME_HEADER_SIZE];
    stream.read_exact(&mut bytes)?;
    let header = protocol::decode_frame_header(&bytes).map_err(io::Error::other)?;
    let mut sample_bytes =
      vec![0; header.frames as usize * protocol::CHANNELS as usize * protocol::SAMPLE_BYTES];
    stream.read_exact(&mut sample_bytes)?;
    if last_sequence[header.route_id as usize].is_some_and(|sequence| header.sequence <= sequence) {
      continue;
    }
    last_sequence[header.route_id as usize] = Some(header.sequence);

    let samples = sample_bytes
      .chunks_exact(protocol::SAMPLE_BYTES)
      .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte Float32 sample")))
      .collect::<Vec<_>>();
    output_to_sources(
      runtime,
      api,
      header.route_id,
      header.frames,
      header.qpc_time_100ns,
      timestamp_offset_ns,
      &samples,
    );
  }
  Ok(())
}

fn clock_offset_ns(api: ObsApi) -> Option<i128> {
  let mut counter = 0_i64;
  let mut frequency = 0_i64;
  unsafe {
    QueryPerformanceCounter(&mut counter).ok()?;
    QueryPerformanceFrequency(&mut frequency).ok()?;
  }
  if frequency <= 0 {
    return None;
  }
  let qpc_ns = i128::from(counter) * 1_000_000_000 / i128::from(frequency);
  Some(i128::from(unsafe { (api.get_time_ns)() }) - qpc_ns)
}

fn output_to_sources(
  runtime: &Runtime,
  api: ObsApi,
  route_id: u8,
  frames: u32,
  qpc_time_100ns: u64,
  timestamp_offset_ns: i128,
  samples: &[f32],
) {
  let timestamp_ns =
    (i128::from(qpc_time_100ns) * 100 + timestamp_offset_ns).clamp(0, i128::from(u64::MAX)) as u64;
  let mut audio: obs_source_audio = unsafe { std::mem::zeroed() };
  audio.data[0] = samples.as_ptr().cast();
  audio.frames = frames;
  audio.speakers = SOUNDTRACK_SPEAKERS_STEREO;
  audio.format = SOUNDTRACK_AUDIO_FORMAT_FLOAT;
  audio.samples_per_sec = protocol::SAMPLE_RATE;
  audio.timestamp = timestamp_ns;

  let sources = runtime.sources.lock();
  for source in sources
    .iter()
    .filter(|source| source.route.id() == route_id)
  {
    unsafe { (api.output_audio)(source.source as *mut obs_source_t, &audio) };
  }
}

unsafe extern "C" fn source_name(_type_data: *mut c_void) -> *const c_char {
  c"SoundTrack 路由音频".as_ptr()
}

unsafe extern "C" fn source_create(
  settings: *mut obs_data_t,
  source: *mut obs_source_t,
) -> *mut c_void {
  let (Some(runtime), Some(api)) = (RUNTIME.get(), API.get()) else {
    return ptr::null_mut();
  };
  let route = Route::from_c_string(unsafe { (api.data_get_string)(settings, c"route".as_ptr()) });
  let app_path = source_app_path(settings, api);
  let registration = SourceRegistration {
    source: source as usize,
    route,
    app_path,
  };
  runtime.sources.lock().push(registration);
  synchronize_app_launcher(runtime);
  Box::into_raw(Box::new(SourceData {
    source: source as usize,
  }))
  .cast()
}

unsafe extern "C" fn source_destroy(data: *mut c_void) {
  if data.is_null() {
    return;
  }
  let source_data = unsafe { Box::from_raw(data.cast::<SourceData>()) };
  if let Some(runtime) = RUNTIME.get() {
    runtime
      .sources
      .lock()
      .retain(|source| source.source != source_data.source);
    synchronize_app_launcher(runtime);
  }
}

unsafe extern "C" fn source_defaults(settings: *mut obs_data_t) {
  if let Some(api) = API.get() {
    unsafe { (api.data_set_default_string)(settings, c"route".as_ptr(), c"game".as_ptr()) };
    unsafe { (api.data_set_default_string)(settings, c"app_path".as_ptr(), c"".as_ptr()) };
  }
}

unsafe extern "C" fn source_properties(_data: *mut c_void) -> *mut obs_properties_t {
  let Some(api) = API.get() else {
    return ptr::null_mut();
  };
  let properties = unsafe { (api.properties_create)() };
  if properties.is_null() {
    return properties;
  }

  let route = unsafe {
    (api.properties_add_list)(
      properties,
      c"route".as_ptr(),
      c"路由".as_ptr(),
      SOUNDTRACK_COMBO_TYPE_LIST,
      SOUNDTRACK_COMBO_FORMAT_STRING,
    )
  };
  if !route.is_null() {
    unsafe {
      (api.property_list_add_string)(route, c"Game".as_ptr(), c"game".as_ptr());
      (api.property_list_add_string)(route, c"Media".as_ptr(), c"media".as_ptr());
      (api.property_list_add_string)(route, c"Voice".as_ptr(), c"voice".as_ptr());
    }
  }

  unsafe {
    (api.properties_add_path)(
      properties,
      c"app_path".as_ptr(),
      c"SoundTrack 程序位置".as_ptr(),
      SOUNDTRACK_PATH_FILE,
      c"SoundTrack 程序 (*.exe);;可执行文件 (*.exe)".as_ptr(),
      ptr::null(),
    );
  }

  let app_path_description = unsafe {
    (api.properties_add_text)(
      properties,
      c"soundtrack_app_path_info".as_ptr(),
      c"自动启动与退出".as_ptr(),
      SOUNDTRACK_TEXT_INFO,
    )
  };
  if !app_path_description.is_null() {
    unsafe {
      (api.property_set_description)(
        app_path_description,
        c"所有路由源共用该程序位置。OBS 完成加载后启动；退出 OBS 时关闭本插件启动的实例。".as_ptr(),
      )
    };
  }

  let status = RUNTIME
    .get()
    .map(|runtime| runtime.status.read().clone())
    .unwrap_or_else(|| "SoundTrack 尚未启动".to_string());
  let status_description =
    CString::new(status).unwrap_or_else(|_| c"SoundTrack 状态未知".to_owned());
  let status_property = unsafe {
    (api.properties_add_text)(
      properties,
      c"soundtrack_status".as_ptr(),
      c"SoundTrack 连接状态".as_ptr(),
      SOUNDTRACK_TEXT_INFO,
    )
  };
  if !status_property.is_null() {
    unsafe { (api.property_set_description)(status_property, status_description.as_ptr()) };
  }

  properties
}

unsafe extern "C" fn source_update(data: *mut c_void, settings: *mut obs_data_t) {
  if data.is_null() {
    return;
  }
  let (Some(runtime), Some(api)) = (RUNTIME.get(), API.get()) else {
    return;
  };
  let source_data = unsafe { &mut *data.cast::<SourceData>() };
  let route = Route::from_c_string(unsafe { (api.data_get_string)(settings, c"route".as_ptr()) });
  let app_path = source_app_path(settings, api);
  {
    if let Some(registration) = runtime
      .sources
      .lock()
      .iter_mut()
      .find(|source| source.source == source_data.source)
    {
      registration.route = route;
      registration.app_path = app_path;
    }
  }
  synchronize_app_launcher(runtime);
}

fn source_app_path(settings: *mut obs_data_t, api: &ObsApi) -> Option<PathBuf> {
  let value = unsafe { (api.data_get_string)(settings, c"app_path".as_ptr()) };
  if value.is_null() {
    return None;
  }
  let value = unsafe { CStr::from_ptr(value) }.to_string_lossy();
  let value = value.trim();
  (!value.is_empty()).then(|| PathBuf::from(value))
}

fn debug_message(message: &str) {
  let message = CString::new(message).unwrap_or_else(|_| c"SoundTrack OBS plugin error".to_owned());
  let message_text = message.to_string_lossy();
  if let Some(blog) = API
    .get()
    .and_then(|api| api.blog)
    .or_else(|| OBS_BLOG.get().copied())
  {
    unsafe { blog(100, c"%s".as_ptr(), message.as_ptr()) };
  }
  let log_path = std::env::temp_dir().join("soundtrack-obs-plugin.log");
  let _ = OpenOptions::new()
    .create(true)
    .append(true)
    .open(log_path)
    .and_then(|mut file| writeln!(file, "{message_text}"));
  unsafe { OutputDebugStringA(PCSTR(message.as_ptr().cast())) };
}

fn log_message(api: ObsApi, level: i32, message: &str) {
  let message =
    CString::new(message).unwrap_or_else(|_| c"SoundTrack OBS plugin message".to_owned());
  if let Some(blog) = api.blog {
    unsafe { blog(level, c"%s".as_ptr(), message.as_ptr()) };
  } else {
    unsafe { OutputDebugStringA(PCSTR(message.as_ptr().cast())) };
  }
}
