mod audio;
mod model;
mod service;
mod store;

#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod platform;

use service::SoundTrackService;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let service = SoundTrackService::new().expect("failed to initialize SoundTrack configuration");
  let service_for_setup = service.clone();

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_single_instance::init(|app, _, _| {
      if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
      }
    }))
    .manage(service)
    .setup(move |app| {
      service_for_setup.start(app.handle().clone());
      setup_tray(app)?;
      Ok(())
    })
    .on_window_event(|window, event| {
      if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
      }
    })
    .invoke_handler(tauri::generate_handler![
      service::get_app_state,
      service::refresh_app_state,
      service::move_application,
      service::set_route_controls,
      service::set_file_name_fallback_enabled,
      service::set_application_excluded,
      service::remove_application_rule,
      service::add_offline_application,
      service::get_config_path,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
  use tauri::menu::{Menu, MenuItem};
  use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

  let open = MenuItem::with_id(app, "open", "打开 SoundTrack", true, None::<&str>)?;
  let exit = MenuItem::with_id(app, "exit", "退出 SoundTrack", true, None::<&str>)?;
  let menu = Menu::with_items(app, &[&open, &exit])?;
  let icon = app
    .default_window_icon()
    .cloned()
    .expect("default window icon is configured");

  TrayIconBuilder::new()
    .icon(icon)
    .tooltip("SoundTrack")
    .menu(&menu)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, event| match event.id().as_ref() {
      "open" => {
        if let Some(window) = app.get_webview_window("main") {
          let _ = window.show();
          let _ = window.set_focus();
        }
      }
      "exit" => app.exit(0),
      _ => {}
    })
    .on_tray_icon_event(|tray, event| {
      if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
      } = event
      {
        if let Some(window) = tray.app_handle().get_webview_window("main") {
          let _ = window.show();
          let _ = window.set_focus();
        }
      }
    })
    .build(app)?;

  Ok(())
}
