use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
  Game,
  Media,
  Voice,
}

impl Route {
  pub const ALL: [Self; 3] = [Self::Game, Self::Media, Self::Voice];

  pub fn label(self) -> &'static str {
    match self {
      Self::Game => "Game",
      Self::Media => "Media",
      Self::Voice => "Voice",
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteControls {
  pub gain: f32,
  pub muted: bool,
}

impl Default for RouteControls {
  fn default() -> Self {
    Self {
      gain: 0.5,
      muted: false,
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRule {
  pub id: String,
  pub executable_path: Option<String>,
  pub file_name: Option<String>,
  pub window_title: Option<String>,
  pub window_class: Option<String>,
  pub route: Option<Route>,
  pub excluded: bool,
  #[serde(default)]
  pub file_name_fallback: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
  pub version: u32,
  #[serde(default)]
  pub file_name_fallback_enabled: bool,
  #[serde(default)]
  pub routes: BTreeMap<Route, RouteControls>,
  #[serde(default)]
  pub rules: Vec<ApplicationRule>,
}

impl Default for AppConfig {
  fn default() -> Self {
    let routes = Route::ALL
      .into_iter()
      .map(|route| (route, RouteControls::default()))
      .collect();
    Self {
      version: 1,
      file_name_fallback_enabled: false,
      routes,
      rules: Vec::new(),
    }
  }
}

impl AppConfig {
  pub fn ensure_defaults(&mut self) {
    self.version = 1;
    for route in Route::ALL {
      self.routes.entry(route).or_default();
    }
    for controls in self.routes.values_mut() {
      if !controls.gain.is_finite() {
        controls.gain = 0.5;
      }
      controls.gain = controls.gain.clamp(0.0, 1.0);
    }
    for rule in &mut self.rules {
      rule.file_name_fallback = self.file_name_fallback_enabled;
    }
  }
}

#[derive(Clone, Debug, Default)]
pub struct DetectedApplication {
  pub id: String,
  pub name: String,
  pub executable_path: Option<String>,
  pub file_name: Option<String>,
  pub window_title: Option<String>,
  pub window_class: Option<String>,
  pub pids: Vec<u32>,
  pub process_ancestors: BTreeMap<u32, Vec<u32>>,
  pub session_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationSummary {
  pub id: String,
  pub name: String,
  pub executable_path: Option<String>,
  pub file_name: Option<String>,
  pub window_title: Option<String>,
  pub window_class: Option<String>,
  pub pids: Vec<u32>,
  pub session_count: usize,
  pub route: Route,
  pub online: bool,
  pub automatic: bool,
  pub excluded: bool,
  pub conflict: Option<String>,
  pub capture_error: Option<String>,
  pub peak: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSummary {
  pub route: Route,
  pub gain: f32,
  pub muted: bool,
  pub peak: f32,
  pub clipping: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteMeter {
  pub route: Route,
  pub peak: f32,
  pub clipping: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
  pub capture_supported: bool,
  pub capture_available: bool,
  pub capture_error: Option<String>,
  pub file_name_fallback_enabled: bool,
  pub applications: Vec<ApplicationSummary>,
  pub routes: Vec<RouteSummary>,
  pub config_path: String,
}

pub fn normalized_path(path: &str) -> String {
  path
    .replace('/', "\\")
    .to_lowercase()
    .trim_start_matches("\\\\?\\")
    .to_string()
}

pub fn is_internal_process_name(name: &str) -> bool {
  matches!(
    name.to_ascii_lowercase().as_str(),
    "obs.exe" | "obs64.exe" | "soundtrack.exe" | "soundtrack-desktop.exe"
  )
}

pub fn is_shell_process_name(name: &str) -> bool {
  name.eq_ignore_ascii_case("explorer.exe")
}

pub fn rule_match_score(
  rule: &ApplicationRule,
  app: &DetectedApplication,
  config: &AppConfig,
) -> Option<u8> {
  let path_matches = rule
    .executable_path
    .as_deref()
    .zip(app.executable_path.as_deref())
    .is_some_and(|(rule_path, app_path)| normalized_path(rule_path) == normalized_path(app_path));

  if path_matches {
    if rule
      .window_title
      .as_deref()
      .zip(app.window_title.as_deref())
      .is_some_and(|(rule_title, app_title)| rule_title.eq_ignore_ascii_case(app_title))
    {
      return Some(5);
    }
    if rule
      .window_class
      .as_deref()
      .zip(app.window_class.as_deref())
      .is_some_and(|(rule_class, app_class)| rule_class.eq_ignore_ascii_case(app_class))
    {
      return Some(4);
    }
    return Some(3);
  }

  if rule
    .window_title
    .as_deref()
    .zip(app.window_title.as_deref())
    .is_some_and(|(rule_title, app_title)| rule_title.eq_ignore_ascii_case(app_title))
  {
    return Some(2);
  }

  if config.file_name_fallback_enabled
    && rule.file_name_fallback
    && rule
      .file_name
      .as_deref()
      .zip(app.file_name.as_deref())
      .is_some_and(|(rule_name, app_name)| rule_name.eq_ignore_ascii_case(app_name))
  {
    return Some(1);
  }

  None
}

pub fn resolve_route(
  app: &DetectedApplication,
  config: &AppConfig,
) -> (Route, bool, bool, Option<String>) {
  let matched = config
    .rules
    .iter()
    .filter_map(|rule| rule_match_score(rule, app, config).map(|score| (score, rule)))
    .collect::<Vec<_>>();

  if matched.iter().any(|(_, rule)| rule.excluded) {
    return (Route::Game, false, true, None);
  }

  let best_score = matched.iter().map(|(score, _)| *score).max();
  let routes = matched
    .iter()
    .filter(|(score, rule)| Some(*score) == best_score && rule.route.is_some())
    .filter_map(|(_, rule)| rule.route)
    .collect::<std::collections::BTreeSet<_>>();

  match routes.len() {
    0 => (Route::Game, true, false, None),
    1 => (
      *routes.first().expect("one route exists"),
      false,
      false,
      None,
    ),
    _ => (
      Route::Game,
      false,
      false,
      Some("匹配到互相冲突的显式路由规则".to_string()),
    ),
  }
}
