use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tauri::{AppHandle, Emitter, State};

use crate::audio::{AudioEngine, AudioPacket};
use crate::model::{
  AppConfig, AppSnapshot, ApplicationRule, ApplicationSummary, DetectedApplication, Route,
  RouteMeter, RouteSummary, is_internal_process_name, normalized_path, resolve_route,
};
use crate::store::ConfigStore;

#[derive(Clone)]
pub struct SoundTrackService {
  inner: Arc<ServiceInner>,
}

struct ServiceInner {
  config: ConfigStore,
  snapshot: RwLock<AppSnapshot>,
  capture_probe_error: Mutex<Option<String>>,
  capture_probe_running: AtomicBool,
  engine: AudioEngine,
  capture_workers: parking_lot::Mutex<HashMap<u32, CaptureWorker>>,
  worker_errors: Arc<parking_lot::Mutex<HashMap<u32, String>>>,
}

struct CaptureWorker {
  route: Route,
  stop: Arc<AtomicBool>,
  thread: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct EffectiveRoute {
  route: Route,
  automatic: bool,
  excluded: bool,
  conflict: Option<String>,
}

struct CapturePlan {
  routes: HashMap<String, EffectiveRoute>,
  roots: HashMap<u32, (String, Route)>,
  hidden: HashSet<String>,
}

impl CaptureWorker {
  fn start(
    pid: u32,
    route: Route,
    engine: AudioEngine,
    errors: Arc<parking_lot::Mutex<HashMap<u32, String>>>,
  ) -> Self {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let worker_thread = thread::spawn(move || {
      while !thread_stop.load(Ordering::Relaxed) {
        let result = soundtrack_audio_capture::capture_stream(
          pid,
          || !thread_stop.load(Ordering::Relaxed),
          |qpc_time_100ns, samples| {
            engine.send(AudioPacket {
              route,
              qpc_time_100ns,
              samples: samples.to_vec(),
            });
          },
        );
        if thread_stop.load(Ordering::Relaxed) {
          break;
        }
        let message = result
          .err()
          .unwrap_or_else(|| "Process Loopback capture stream stopped unexpectedly".to_string());
        errors.lock().insert(pid, message);
        for _ in 0..20 {
          if thread_stop.load(Ordering::Relaxed) {
            break;
          }
          thread::sleep(Duration::from_millis(100));
        }
      }
      errors.lock().remove(&pid);
    });
    Self {
      route,
      stop,
      thread: Some(worker_thread),
    }
  }

  fn stop(mut self) {
    self.stop.store(true, Ordering::Relaxed);
    if let Some(thread) = self.thread.take() {
      let _ = thread.join();
    }
  }
}

impl Drop for CaptureWorker {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::Relaxed);
  }
}

impl SoundTrackService {
  pub fn new() -> Result<Self, String> {
    let config = ConfigStore::load()?;
    let initial_config = config.get()?;
    let config_path = config.path().to_string_lossy().into_owned();
    let snapshot = AppSnapshot {
      capture_available: false,
      capture_error: Some("正在检测音频捕获能力".to_string()),
      file_name_fallback_enabled: initial_config.file_name_fallback_enabled,
      applications: Vec::new(),
      routes: Vec::new(),
      config_path,
    };
    Ok(Self {
      inner: Arc::new(ServiceInner {
        config,
        snapshot: RwLock::new(snapshot),
        capture_probe_error: Mutex::new(Some("正在检测音频捕获能力".to_string())),
        capture_probe_running: AtomicBool::new(false),
        engine: AudioEngine::new(initial_config.routes),
        capture_workers: parking_lot::Mutex::new(HashMap::new()),
        worker_errors: Arc::new(parking_lot::Mutex::new(HashMap::new())),
      }),
    })
  }

  pub fn start(&self, app: AppHandle) {
    self.inner.engine.start();

    #[cfg(windows)]
    {
      let engine = self.inner.engine.clone();
      thread::spawn(move || crate::ipc::run_server(engine));
    }

    let service = self.clone();
    let state_app = app.clone();
    thread::spawn(move || {
      service.probe_capture_async();
      loop {
        service.refresh();
        if let Ok(snapshot) = service.snapshot() {
          let _ = state_app.emit("soundtrack-state", snapshot);
        }
        thread::sleep(Duration::from_millis(900));
      }
    });

    let engine = self.inner.engine.clone();
    thread::spawn(move || {
      loop {
        let meters = Route::ALL
          .into_iter()
          .map(|route| {
            let peak = engine.level(route);
            RouteMeter {
              route,
              peak,
              clipping: peak >= 1.0,
            }
          })
          .collect::<Vec<_>>();
        let _ = app.emit("route-levels", meters);
        thread::sleep(Duration::from_millis(100));
      }
    });
  }

  pub fn snapshot(&self) -> Result<AppSnapshot, String> {
    self
      .inner
      .snapshot
      .read()
      .map(|snapshot| snapshot.clone())
      .map_err(|_| "runtime state lock poisoned".to_string())
  }

  pub fn refresh_now(&self) -> Result<AppSnapshot, String> {
    let should_probe = self
      .inner
      .capture_probe_error
      .lock()
      .ok()
      .is_some_and(|error| error.is_some());
    if should_probe {
      self.probe_capture_async();
    }
    self.refresh();
    self.snapshot()
  }

  fn probe_capture_async(&self) {
    if self
      .inner
      .capture_probe_running
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
      .is_err()
    {
      return;
    }
    self.set_capture_probe(Some("正在检测音频捕获能力".to_string()));
    self.refresh();
    let service = self.clone();
    thread::spawn(move || {
      service.set_capture_probe(probe_capture().err());
      service
        .inner
        .capture_probe_running
        .store(false, Ordering::Release);
      service.refresh();
    });
  }

  fn refresh(&self) {
    let config = match self.inner.config.get() {
      Ok(config) => config,
      Err(error) => {
        self.set_error(error);
        return;
      }
    };
    self.inner.engine.set_controls(config.routes.clone());

    #[cfg(windows)]
    let (detected, discovery_error) = match crate::platform::discover_applications() {
      Ok(applications) => (applications, None),
      Err(error) => (Vec::new(), Some(error)),
    };
    #[cfg(not(windows))]
    let (detected, discovery_error): (Vec<DetectedApplication>, Option<String>) = (
      Vec::new(),
      Some("SoundTrack 音频捕获目前仅支持 Windows".to_string()),
    );

    let plan = build_capture_plan(&detected, &config);
    let capability_error = self
      .inner
      .capture_probe_error
      .lock()
      .ok()
      .and_then(|error| error.clone());
    if capability_error.is_none() && discovery_error.is_none() {
      self.reconcile_captures(&plan.roots);
    } else {
      self.reconcile_captures(&HashMap::new());
    }

    let mut applications = self.summarize_applications(&detected, &config, &plan);
    applications.sort_by(|left, right| {
      left
        .excluded
        .cmp(&right.excluded)
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    let routes = Route::ALL
      .into_iter()
      .map(|route| {
        let controls = config.routes.get(&route).cloned().unwrap_or_default();
        let peak = self.inner.engine.level(route);
        RouteSummary {
          route,
          gain: controls.gain,
          muted: controls.muted,
          peak,
          clipping: peak >= 1.0,
        }
      })
      .collect();
    let capture_error = capability_error.or(discovery_error);
    let snapshot = AppSnapshot {
      capture_available: capture_error.is_none(),
      capture_error,
      file_name_fallback_enabled: config.file_name_fallback_enabled,
      applications,
      routes,
      config_path: self.inner.config.path().to_string_lossy().into_owned(),
    };
    if let Ok(mut current) = self.inner.snapshot.write() {
      *current = snapshot;
    }
  }

  fn summarize_applications(
    &self,
    detected: &[DetectedApplication],
    config: &AppConfig,
    plan: &CapturePlan,
  ) -> Vec<ApplicationSummary> {
    let mut seen = HashSet::new();
    let worker_errors = self.inner.worker_errors.lock();
    let mut applications = detected
      .iter()
      .filter(|app| !plan.hidden.contains(&app.id))
      .map(|app| {
        seen.insert(app.id.clone());
        let fallback = resolve_route(app, config);
        let effective = plan.routes.get(&app.id);
        let route = effective.map_or(fallback.0, |value| value.route);
        let automatic = effective.map_or(fallback.1, |value| value.automatic);
        let excluded = effective.map_or(fallback.2, |value| value.excluded);
        let conflict = effective
          .and_then(|value| value.conflict.clone())
          .or(fallback.3);
        let capture_error = app
          .pids
          .iter()
          .find_map(|pid| worker_errors.get(pid).cloned());
        ApplicationSummary {
          id: app.id.clone(),
          name: app.name.clone(),
          executable_path: app.executable_path.clone(),
          file_name: app.file_name.clone(),
          window_title: app.window_title.clone(),
          window_class: app.window_class.clone(),
          pids: app.pids.clone(),
          session_count: app.session_count,
          route,
          online: true,
          automatic,
          excluded,
          conflict,
          capture_error,
          peak: 0.0,
        }
      })
      .collect::<Vec<_>>();
    drop(worker_errors);

    for rule in &config.rules {
      if seen.contains(&rule.id) || is_internal_rule(rule) {
        continue;
      }
      let detected_rule = DetectedApplication {
        id: rule.id.clone(),
        name: rule
          .file_name
          .as_deref()
          .or(rule.executable_path.as_deref())
          .unwrap_or("未知应用")
          .rsplit(['\\', '/'])
          .next()
          .unwrap_or("未知应用")
          .trim_end_matches(".exe")
          .to_string(),
        executable_path: rule.executable_path.clone(),
        file_name: rule.file_name.clone(),
        window_title: rule.window_title.clone(),
        window_class: rule.window_class.clone(),
        ..DetectedApplication::default()
      };
      let (route, automatic, excluded, conflict) = resolve_route(&detected_rule, config);
      applications.push(ApplicationSummary {
        id: rule.id.clone(),
        name: detected_rule.name,
        executable_path: rule.executable_path.clone(),
        file_name: rule.file_name.clone(),
        window_title: rule.window_title.clone(),
        window_class: rule.window_class.clone(),
        pids: Vec::new(),
        session_count: 0,
        route,
        online: false,
        automatic,
        excluded,
        conflict,
        capture_error: None,
        peak: 0.0,
      });
    }
    applications
  }

  fn reconcile_captures(&self, desired: &HashMap<u32, (String, Route)>) {
    let mut stopped = Vec::new();
    {
      let mut workers = self.inner.capture_workers.lock();
      let stale = workers
        .iter()
        .filter(|(pid, worker)| desired.get(pid).map(|(_, route)| *route) != Some(worker.route))
        .map(|(pid, _)| *pid)
        .collect::<Vec<_>>();
      for pid in stale {
        if let Some(worker) = workers.remove(&pid) {
          stopped.push((pid, worker));
        }
      }
    }
    for (pid, worker) in stopped {
      worker.stop();
      self.inner.worker_errors.lock().remove(&pid);
    }

    let mut workers = self.inner.capture_workers.lock();
    for (pid, (_, route)) in desired {
      workers.entry(*pid).or_insert_with(|| {
        self.inner.worker_errors.lock().remove(pid);
        CaptureWorker::start(
          *pid,
          *route,
          self.inner.engine.clone(),
          self.inner.worker_errors.clone(),
        )
      });
    }
  }

  pub fn move_application(
    &self,
    application_id: &str,
    route: Route,
  ) -> Result<AppSnapshot, String> {
    let application = self
      .snapshot()?
      .applications
      .into_iter()
      .find(|application| application.id == application_id)
      .ok_or_else(|| "应用已不在当前列表中".to_string())?;
    self.upsert_rule(rule_from_application(&application, Some(route), false))?;
    self.refresh();
    self.snapshot()
  }

  pub fn set_application_excluded(
    &self,
    application_id: &str,
    excluded: bool,
  ) -> Result<AppSnapshot, String> {
    let application = self
      .snapshot()?
      .applications
      .into_iter()
      .find(|application| application.id == application_id)
      .ok_or_else(|| "应用已不在当前列表中".to_string())?;
    let config = self.inner.config.get()?;
    let existing = config
      .rules
      .iter()
      .find(|rule| rule.id == application_id)
      .cloned();
    if excluded {
      let route = existing.as_ref().and_then(|rule| rule.route);
      self.upsert_rule(rule_from_application(&application, route, true))?;
    } else if let Some(mut rule) = existing {
      if rule.route.is_none() {
        self.inner.config.update(|config| {
          config
            .rules
            .retain(|candidate| candidate.id != application_id);
        })?;
        self.refresh();
        return self.snapshot();
      }
      rule.excluded = false;
      self.upsert_rule(rule)?;
    } else {
      return self.snapshot();
    }
    self.refresh();
    self.snapshot()
  }

  pub fn remove_application_rule(&self, application_id: &str) -> Result<AppSnapshot, String> {
    self.inner.config.update(|config| {
      config.rules.retain(|rule| rule.id != application_id);
    })?;
    self.refresh();
    self.snapshot()
  }

  pub fn add_offline_application(&self, path: &str, route: Route) -> Result<AppSnapshot, String> {
    let executable = Path::new(path);
    if !executable.is_file()
      || !executable
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
      return Err("请选择有效的 .exe 文件".to_string());
    }
    let executable_path = executable
      .canonicalize()
      .map_err(|error| format!("读取可执行文件路径失败: {error}"))?
      .to_string_lossy()
      .into_owned()
      .trim_start_matches("\\\\?\\")
      .to_string();
    let file_name = executable
      .file_name()
      .and_then(|name| name.to_str())
      .map(str::to_string);
    let identity = normalized_path(&executable_path);
    let rule = ApplicationRule {
      id: identity,
      executable_path: Some(executable_path),
      file_name,
      window_title: None,
      window_class: None,
      route: Some(route),
      excluded: false,
      file_name_fallback: false,
    };
    self.upsert_rule(rule)?;
    self.refresh();
    self.snapshot()
  }

  pub fn set_route_controls(
    &self,
    route: Route,
    gain_percent: u8,
    muted: bool,
  ) -> Result<AppSnapshot, String> {
    self.inner.config.update(|config| {
      let controls = config.routes.entry(route).or_default();
      controls.gain = f32::from(gain_percent.min(100)) / 100.0;
      controls.muted = muted;
    })?;
    self.refresh();
    self.snapshot()
  }

  pub fn set_file_name_fallback_enabled(&self, enabled: bool) -> Result<AppSnapshot, String> {
    self.inner.config.update(|config| {
      config.file_name_fallback_enabled = enabled;
      for rule in &mut config.rules {
        rule.file_name_fallback = enabled;
      }
    })?;
    self.refresh();
    self.snapshot()
  }

  fn upsert_rule(&self, rule: ApplicationRule) -> Result<(), String> {
    self.inner.config.update(|config| {
      let mut rule = rule;
      rule.file_name_fallback = config.file_name_fallback_enabled;
      if let Some(existing) = config
        .rules
        .iter_mut()
        .find(|existing| existing.id == rule.id)
      {
        *existing = rule;
      } else {
        config.rules.push(rule);
      }
    })?;
    Ok(())
  }

  fn set_error(&self, error: String) {
    if let Ok(mut snapshot) = self.inner.snapshot.write() {
      snapshot.capture_available = false;
      snapshot.capture_error = Some(error);
    }
  }

  fn set_capture_probe(&self, error: Option<String>) {
    if let Ok(mut current) = self.inner.capture_probe_error.lock() {
      *current = error;
    }
  }
}

impl CapturePlan {
  fn build(detected: &[DetectedApplication], config: &AppConfig) -> Self {
    let internal_pids = detected
      .iter()
      .filter(|application| is_internal_application(application))
      .flat_map(|application| application.pids.iter().copied())
      .chain(std::iter::once(std::process::id()))
      .collect::<HashSet<_>>();
    let mut hidden = HashSet::new();
    let mut pid_to_application = HashMap::<u32, usize>::new();
    for (index, application) in detected.iter().enumerate() {
      if is_internal_application(application)
        || application.process_ancestors.values().any(|ancestors| {
          ancestors
            .iter()
            .any(|ancestor| internal_pids.contains(ancestor))
        })
      {
        hidden.insert(application.id.clone());
        continue;
      }
      for pid in &application.pids {
        pid_to_application.insert(*pid, index);
      }
    }

    let mut resolved = detected
      .iter()
      .map(|application| {
        let (route, automatic, excluded, conflict) = resolve_route(application, config);
        EffectiveRoute {
          route,
          automatic,
          excluded,
          conflict,
        }
      })
      .collect::<Vec<_>>();
    let mut order = (0..detected.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| {
      detected[*index]
        .process_ancestors
        .values()
        .map(Vec::len)
        .min()
        .unwrap_or_default()
    });

    for index in order {
      if hidden.contains(&detected[index].id) {
        continue;
      }
      let ancestor_indexes = ancestor_applications(index, detected, &pid_to_application);
      if ancestor_indexes.is_empty() {
        continue;
      }
      let child = resolved[index].clone();
      let active_ancestors = ancestor_indexes
        .into_iter()
        .filter(|parent_index| {
          !resolved[*parent_index].excluded && resolved[*parent_index].conflict.is_none()
        })
        .collect::<Vec<_>>();
      if active_ancestors.is_empty() {
        continue;
      }
      if child.conflict.is_some() {
        let conflict = child.conflict.clone().unwrap_or_default();
        for parent_index in active_ancestors {
          resolved[parent_index].conflict = Some(conflict.clone());
        }
        continue;
      }
      if child.excluded {
        let conflict = "进程树包含已排除的子应用，无法可靠隔离".to_string();
        resolved[index].conflict = Some(conflict.clone());
        for parent_index in active_ancestors {
          resolved[parent_index].conflict = Some(conflict.clone());
        }
        continue;
      }

      let parent_routes = active_ancestors
        .iter()
        .map(|parent_index| resolved[*parent_index].route)
        .collect::<std::collections::BTreeSet<_>>();
      let conflicting_ancestors = if child.automatic {
        if parent_routes.len() > 1 {
          active_ancestors.clone()
        } else {
          Vec::new()
        }
      } else {
        active_ancestors
          .iter()
          .copied()
          .filter(|parent_index| resolved[*parent_index].route != child.route)
          .collect::<Vec<_>>()
      };
      if !conflicting_ancestors.is_empty() {
        let conflict = "父子进程树跨越了不同路由，已暂停冲突捕获".to_string();
        resolved[index].conflict = Some(conflict.clone());
        for parent_index in conflicting_ancestors {
          resolved[parent_index].conflict = Some(conflict.clone());
        }
      } else if child.automatic {
        if let Some(route) = parent_routes.first() {
          resolved[index].route = *route;
        }
      }
    }

    for index in 0..detected.len() {
      let Some(conflict) = resolved[index].conflict.clone() else {
        continue;
      };
      for parent_index in ancestor_applications(index, detected, &pid_to_application) {
        if !resolved[parent_index].excluded && resolved[parent_index].conflict.is_none() {
          resolved[parent_index].conflict = Some(conflict.clone());
        }
      }
    }

    let mut roots = HashMap::new();
    for (index, application) in detected.iter().enumerate() {
      if hidden.contains(&application.id)
        || resolved[index].excluded
        || resolved[index].conflict.is_some()
      {
        continue;
      }
      for pid in &application.pids {
        let ancestors = ancestor_applications_for_pid(index, *pid, detected, &pid_to_application);
        let active_ancestor_routes = ancestors
          .iter()
          .filter_map(|parent_index| {
            let parent = &resolved[*parent_index];
            (!parent.excluded && parent.conflict.is_none()).then_some(parent.route)
          })
          .collect::<Vec<_>>();
        if active_ancestor_routes.contains(&resolved[index].route) {
          continue;
        }
        if !active_ancestor_routes.is_empty() {
          continue;
        }
        roots.insert(*pid, (application.id.clone(), resolved[index].route));
      }
    }
    let routes = detected
      .iter()
      .enumerate()
      .map(|(index, application)| (application.id.clone(), resolved[index].clone()))
      .collect();
    Self {
      routes,
      roots,
      hidden,
    }
  }
}

fn ancestor_applications(
  index: usize,
  detected: &[DetectedApplication],
  pid_to_application: &HashMap<u32, usize>,
) -> Vec<usize> {
  let mut application_depths = BTreeMap::<usize, usize>::new();
  for pid in &detected[index].pids {
    for (depth, ancestor_pid) in detected[index]
      .process_ancestors
      .get(pid)
      .into_iter()
      .flatten()
      .enumerate()
    {
      let Some(parent_index) = pid_to_application.get(ancestor_pid).copied() else {
        continue;
      };
      if parent_index == index {
        continue;
      }
      application_depths
        .entry(parent_index)
        .and_modify(|previous| *previous = (*previous).min(depth))
        .or_insert(depth);
    }
  }
  let mut ancestors = application_depths.into_iter().collect::<Vec<_>>();
  ancestors.sort_by_key(|(parent_index, depth)| (*depth, *parent_index));
  ancestors
    .into_iter()
    .map(|(parent_index, _)| parent_index)
    .collect()
}

fn ancestor_applications_for_pid(
  index: usize,
  pid: u32,
  detected: &[DetectedApplication],
  pid_to_application: &HashMap<u32, usize>,
) -> Vec<usize> {
  let mut ancestors = Vec::new();
  for ancestor_pid in detected[index]
    .process_ancestors
    .get(&pid)
    .into_iter()
    .flatten()
  {
    let Some(parent_index) = pid_to_application.get(ancestor_pid).copied() else {
      continue;
    };
    if !ancestors.contains(&parent_index) {
      ancestors.push(parent_index);
    }
  }
  ancestors
}

fn rule_from_application(
  application: &ApplicationSummary,
  route: Option<Route>,
  excluded: bool,
) -> ApplicationRule {
  ApplicationRule {
    id: application.id.clone(),
    executable_path: application.executable_path.clone(),
    file_name: application.file_name.clone(),
    window_title: application.window_title.clone(),
    window_class: application.window_class.clone(),
    route,
    excluded,
    file_name_fallback: false,
  }
}

fn is_internal_application(application: &DetectedApplication) -> bool {
  application
    .file_name
    .as_deref()
    .is_some_and(is_internal_process_name)
    || application.name.eq_ignore_ascii_case("SoundTrack")
    || application.pids.contains(&std::process::id())
}

fn is_internal_rule(rule: &ApplicationRule) -> bool {
  rule
    .file_name
    .as_deref()
    .is_some_and(is_internal_process_name)
}

fn build_capture_plan(detected: &[DetectedApplication], config: &AppConfig) -> CapturePlan {
  CapturePlan::build(detected, config)
}

fn probe_capture() -> Result<(), String> {
  #[cfg(windows)]
  {
    soundtrack_audio_capture::capture_stream(std::process::id(), || false, |_, _| {})
  }
  #[cfg(not(windows))]
  {
    Err("SoundTrack 音频捕获目前仅支持 Windows".to_string())
  }
}

#[tauri::command]
pub fn get_app_state(service: State<'_, SoundTrackService>) -> Result<AppSnapshot, String> {
  service.snapshot()
}

#[tauri::command]
pub fn refresh_app_state(service: State<'_, SoundTrackService>) -> Result<AppSnapshot, String> {
  service.refresh_now()
}

#[tauri::command]
pub fn move_application(
  service: State<'_, SoundTrackService>,
  application_id: String,
  route: Route,
) -> Result<AppSnapshot, String> {
  service.move_application(&application_id, route)
}

#[tauri::command]
pub fn set_route_controls(
  service: State<'_, SoundTrackService>,
  route: Route,
  gain_percent: u8,
  muted: bool,
) -> Result<AppSnapshot, String> {
  service.set_route_controls(route, gain_percent, muted)
}

#[tauri::command]
pub fn set_file_name_fallback_enabled(
  service: State<'_, SoundTrackService>,
  enabled: bool,
) -> Result<AppSnapshot, String> {
  service.set_file_name_fallback_enabled(enabled)
}

#[tauri::command]
pub fn set_application_excluded(
  service: State<'_, SoundTrackService>,
  application_id: String,
  excluded: bool,
) -> Result<AppSnapshot, String> {
  service.set_application_excluded(&application_id, excluded)
}

#[tauri::command]
pub fn remove_application_rule(
  service: State<'_, SoundTrackService>,
  application_id: String,
) -> Result<AppSnapshot, String> {
  service.remove_application_rule(&application_id)
}

#[tauri::command]
pub fn add_offline_application(
  service: State<'_, SoundTrackService>,
  path: String,
  route: Route,
) -> Result<AppSnapshot, String> {
  service.add_offline_application(&path, route)
}

#[tauri::command]
pub fn get_config_path(service: State<'_, SoundTrackService>) -> String {
  service.inner.config.path().to_string_lossy().into_owned()
}
