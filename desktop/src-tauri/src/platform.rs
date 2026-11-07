use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, S_OK, TRUE};
use windows::Win32::Media::Audio::{
  AudioSessionStateExpired, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
  IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{
  CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
  CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
  OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
  EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
  IsWindowVisible,
};
use windows::core::{BOOL, Interface, PWSTR};

use crate::model::{
  DetectedApplication, is_internal_process_name, is_shell_process_name, normalized_path,
};

struct ComGuard;

impl Drop for ComGuard {
  fn drop(&mut self) {
    unsafe { CoUninitialize() };
  }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
  fn drop(&mut self) {
    let _ = unsafe { CloseHandle(self.0) };
  }
}

#[derive(Clone, Default)]
struct WindowInfo {
  title: Option<String>,
  class_name: Option<String>,
}

#[derive(Default)]
struct ProcessCatalog {
  parent_pids: HashMap<u32, u32>,
  file_names: HashMap<u32, String>,
}

pub fn discover_applications() -> Result<Vec<DetectedApplication>, String> {
  unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
    .ok()
    .map_err(|error| format!("CoInitializeEx: {error}"))?;
  let _com = ComGuard;

  let catalog = process_catalog()?;
  let windows = window_catalog();
  let enumerator: IMMDeviceEnumerator = unsafe {
    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
      .map_err(|error| format!("create audio device enumerator: {error}"))?
  };
  let devices = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
    .map_err(|error| format!("enumerate active audio devices: {error}"))?;
  let device_count =
    unsafe { devices.GetCount() }.map_err(|error| format!("get audio device count: {error}"))?;
  let mut applications = BTreeMap::<String, DetectedApplication>::new();

  for device_index in 0..device_count {
    let device = unsafe { devices.Item(device_index) }
      .map_err(|error| format!("get audio device: {error}"))?;
    let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }
      .map_err(|error| format!("activate audio session manager: {error}"))?;
    let sessions = unsafe { manager.GetSessionEnumerator() }
      .map_err(|error| format!("get audio session enumerator: {error}"))?;
    let session_count = unsafe { sessions.GetCount() }
      .map_err(|error| format!("get audio session count: {error}"))?;

    for session_index in 0..session_count {
      let Ok(session) = (unsafe { sessions.GetSession(session_index) }) else {
        continue;
      };
      let Ok(session_state) = (unsafe { session.GetState() }) else {
        continue;
      };
      if session_state == AudioSessionStateExpired {
        continue;
      }
      let Ok(session_control) = session.cast::<IAudioSessionControl2>() else {
        continue;
      };
      if unsafe { session_control.IsSystemSoundsSession() } == S_OK {
        continue;
      }
      let Ok(pid) = (unsafe { session_control.GetProcessId() }) else {
        continue;
      };
      if pid == 0 || pid == std::process::id() {
        continue;
      }

      let executable_path = process_path(pid);
      let file_name = executable_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .or_else(|| catalog.file_names.get(&pid).cloned());
      let process_ancestors = ancestors(pid, &catalog.parent_pids);
      if pid == std::process::id()
        || file_name.as_deref().is_some_and(is_internal_process_name)
        || file_name.as_deref().is_some_and(is_shell_process_name)
        || process_ancestors.iter().any(|ancestor| {
          catalog
            .file_names
            .get(ancestor)
            .is_some_and(|name| is_internal_process_name(name))
        })
      {
        continue;
      }
      let id = executable_path
        .as_deref()
        .map(normalized_path)
        .or_else(|| {
          file_name
            .as_deref()
            .map(|name| format!("file:{}", name.to_lowercase()))
        })
        .unwrap_or_else(|| format!("pid:{pid}"));
      let name = file_name
        .as_deref()
        .and_then(|name| Path::new(name).file_stem())
        .and_then(|name| name.to_str())
        .unwrap_or("未知应用")
        .to_string();
      let app = applications
        .entry(id.clone())
        .or_insert_with(|| DetectedApplication {
          id,
          name,
          executable_path: executable_path.clone(),
          file_name: file_name.clone(),
          ..DetectedApplication::default()
        });
      if !app.pids.contains(&pid) {
        app.pids.push(pid);
        app.process_ancestors.insert(pid, process_ancestors);
      }
      app.session_count += 1;
      if let Some(window) = windows.get(&pid) {
        if app.window_title.is_none() {
          app.window_title = window.title.clone();
        }
        if app.window_class.is_none() {
          app.window_class = window.class_name.clone();
        }
      }
    }
  }

  for app in applications.values_mut() {
    app.pids.sort_unstable();
  }
  Ok(applications.into_values().collect())
}

fn process_catalog() -> Result<ProcessCatalog, String> {
  let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
    .map_err(|error| format!("create process snapshot: {error}"))?;
  let _snapshot = HandleGuard(snapshot);
  let mut entry = PROCESSENTRY32W {
    dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
    ..PROCESSENTRY32W::default()
  };
  unsafe { Process32FirstW(snapshot, &mut entry) }
    .map_err(|error| format!("read process snapshot: {error}"))?;
  let mut catalog = ProcessCatalog::default();

  loop {
    let file_name = String::from_utf16_lossy(
      &entry.szExeFile[..entry
        .szExeFile
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(entry.szExeFile.len())],
    );
    catalog
      .parent_pids
      .insert(entry.th32ProcessID, entry.th32ParentProcessID);
    catalog.file_names.insert(entry.th32ProcessID, file_name);
    if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
      break;
    }
  }

  Ok(catalog)
}

fn ancestors(pid: u32, parents: &HashMap<u32, u32>) -> Vec<u32> {
  let mut result = Vec::new();
  let mut current = pid;
  for _ in 0..64 {
    let Some(parent) = parents.get(&current).copied() else {
      break;
    };
    if parent == 0 || parent == current {
      break;
    }
    result.push(parent);
    current = parent;
  }
  result
}

fn process_path(pid: u32) -> Option<String> {
  let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
  let _process = HandleGuard(process);
  let mut buffer = vec![0_u16; 32_768];
  let mut length = buffer.len() as u32;
  unsafe {
    QueryFullProcessImageNameW(
      process,
      PROCESS_NAME_WIN32,
      PWSTR(buffer.as_mut_ptr()),
      &mut length,
    )
  }
  .ok()?;
  Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn window_catalog() -> HashMap<u32, WindowInfo> {
  let mut windows = HashMap::new();
  let parameter = LPARAM((&mut windows as *mut HashMap<u32, WindowInfo>) as isize);
  let _ = unsafe { EnumWindows(Some(collect_window), parameter) };
  windows
}

unsafe extern "system" fn collect_window(
  hwnd: windows::Win32::Foundation::HWND,
  parameter: LPARAM,
) -> BOOL {
  if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
    return TRUE;
  }
  let mut pid = 0;
  unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
  if pid == 0 {
    return TRUE;
  }

  let text_length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
  let mut title_buffer = vec![0_u16; text_length.saturating_add(1)];
  let title_length = unsafe { GetWindowTextW(hwnd, &mut title_buffer) }.max(0) as usize;
  let title = String::from_utf16_lossy(&title_buffer[..title_length]);
  let mut class_buffer = [0_u16; 256];
  let class_length = unsafe { GetClassNameW(hwnd, &mut class_buffer) }.max(0) as usize;
  let class_name = String::from_utf16_lossy(&class_buffer[..class_length]);
  let windows = unsafe { &mut *(parameter.0 as *mut HashMap<u32, WindowInfo>) };
  windows.entry(pid).or_insert_with(|| WindowInfo {
    title: (!title.is_empty()).then_some(title),
    class_name: (!class_name.is_empty()).then_some(class_name),
  });
  TRUE
}
