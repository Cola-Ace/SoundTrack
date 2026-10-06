use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::model::AppConfig;

pub struct ConfigStore {
  path: PathBuf,
  config: Mutex<AppConfig>,
}

impl ConfigStore {
  pub fn load() -> Result<Self, String> {
    let app_data = std::env::var_os("APPDATA")
      .map(PathBuf::from)
      .ok_or_else(|| "APPDATA is not available".to_string())?;
    let directory = app_data.join("SoundTrack");
    fs::create_dir_all(&directory).map_err(|error| format!("create config directory: {error}"))?;
    let path = directory.join("config.json");

    let mut config = if path.exists() {
      let contents = fs::read(&path).map_err(|error| format!("read config: {error}"))?;
      serde_json::from_slice::<AppConfig>(&contents)
        .map_err(|error| format!("parse config: {error}"))?
    } else {
      AppConfig::default()
    };
    config.ensure_defaults();

    let store = Self {
      path,
      config: Mutex::new(config),
    };
    if !store.path.exists() {
      let current = store
        .config
        .lock()
        .map_err(|_| "config lock poisoned".to_string())?;
      store.write_atomic(&current)?;
    }
    Ok(store)
  }

  pub fn path(&self) -> &PathBuf {
    &self.path
  }

  pub fn get(&self) -> Result<AppConfig, String> {
    self
      .config
      .lock()
      .map(|config| config.clone())
      .map_err(|_| "config lock poisoned".to_string())
  }

  pub fn update(&self, update: impl FnOnce(&mut AppConfig)) -> Result<AppConfig, String> {
    let mut config = self
      .config
      .lock()
      .map_err(|_| "config lock poisoned".to_string())?;
    let mut updated = config.clone();
    update(&mut updated);
    updated.ensure_defaults();
    self.write_atomic(&updated)?;
    *config = updated.clone();
    Ok(updated)
  }

  fn write_atomic(&self, config: &AppConfig) -> Result<(), String> {
    let bytes =
      serde_json::to_vec_pretty(config).map_err(|error| format!("serialize config: {error}"))?;
    let temporary = self.path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
      .create(true)
      .truncate(true)
      .write(true)
      .open(&temporary)
      .map_err(|error| format!("create config temporary file: {error}"))?;
    file
      .write_all(&bytes)
      .map_err(|error| format!("write config: {error}"))?;
    file
      .sync_all()
      .map_err(|error| format!("flush config: {error}"))?;
    drop(file);
    replace_file(&temporary, &self.path).map_err(|error| format!("replace config: {error}"))?;
    Ok(())
  }
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> io::Result<()> {
  use std::os::windows::ffi::OsStrExt;

  use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
  };
  use windows::core::PCWSTR;

  let source = source
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect::<Vec<_>>();
  let destination = destination
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect::<Vec<_>>();
  unsafe {
    MoveFileExW(
      PCWSTR(source.as_ptr()),
      PCWSTR(destination.as_ptr()),
      MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
    )
  }
  .map_err(io::Error::other)
}

#[cfg(not(windows))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> io::Result<()> {
  fs::rename(source, destination)
}
