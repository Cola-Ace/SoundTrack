use std::sync::OnceLock;

use windows_version::OsVersion;

#[derive(Debug)]
pub struct CaptureSupport {
  pub windows_version: String,
  pub supported: bool,
}

impl CaptureSupport {
  pub fn current() -> &'static Self {
    static SUPPORT: OnceLock<CaptureSupport> = OnceLock::new();
    SUPPORT.get_or_init(|| {
      let version = OsVersion::current();
      Self {
        windows_version: format!(
          "Windows {}.{} build {}.{}",
          version.major,
          version.minor,
          version.build,
          windows_version::revision(),
        ),
        supported: supports_process_loopback(version),
      }
    })
  }

  pub fn ensure_supported(&self) -> Result<(), String> {
    if self.supported {
      Ok(())
    } else {
      Err(format!(
        "当前系统 {} 不支持按应用捕获；需要更新后的 Windows 10 2004（build 19041）及以上版本或 Windows 11。请升级系统后重试",
        self.windows_version,
      ))
    }
  }
}

fn supports_process_loopback(version: OsVersion) -> bool {
  // OBS also enables process capture on updated Windows 10 builds below 20348.
  (version.major, version.minor, version.build) >= (10, 0, 19041)
}

pub fn probe_capture() -> Result<(), String> {
  let support = CaptureSupport::current();
  support.ensure_supported()?;
  crate::capture_stream(std::process::id(), || false, |_, _| {}).map_err(|error| {
    format!(
      "{} 满足按应用捕获的版本要求，但 Process Loopback 初始化失败。请安装可用的 Windows 更新并检查 Windows Audio 服务后重试。详细错误：{error}",
      support.windows_version,
    )
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rejects_systems_before_windows_10_2004() {
    for version in [
      OsVersion::new(6, 3, 0, 9600),
      OsVersion::new(10, 0, 0, 17763),
      OsVersion::new(10, 0, 0, 18363),
      OsVersion::new(10, 0, 0, 19040),
    ] {
      assert!(!supports_process_loopback(version), "{version:?}");
    }
  }

  #[test]
  fn accepts_windows_10_2004_22h2_and_windows_11() {
    for build in [19041, 19045, 20348, 22000, 22631, 26100] {
      assert!(supports_process_loopback(OsVersion::new(10, 0, 0, build)));
    }
  }

  #[test]
  fn unsupported_error_includes_detected_build_and_upgrade_requirement() {
    let support = CaptureSupport {
      windows_version: "Windows 10.0 build 18363.0".to_string(),
      supported: false,
    };
    let error = support.ensure_supported().unwrap_err();
    assert!(error.contains("18363.0"));
    assert!(error.contains("19041"));
    assert!(error.contains("升级系统"));
  }
}
