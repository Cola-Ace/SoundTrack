mod audio;
mod model;
mod service;
mod store;

#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod platform;

use service::SoundTrackService;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

#[derive(Default)]
struct MainWindowCreation(AtomicBool);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let background_mode = std::env::args_os().any(|argument| argument == "--background");
  let shutdown_mode = std::env::args_os().any(|argument| argument == "--shutdown");
  let service = SoundTrackService::new().expect("failed to initialize SoundTrack configuration");
  let service_for_setup = service.clone();

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_single_instance::init(|app, args, _| {
      if args.iter().any(|argument| argument == "--shutdown") {
        app.exit(0);
        return;
      }
      if args.iter().any(|argument| argument == "--background") {
        return;
      }
      show_main_window(app);
    }))
    .manage(service)
    .manage(MainWindowCreation::default())
    .setup(move |app| {
      if shutdown_mode {
        app.handle().exit(0);
        return Ok(());
      }
      service_for_setup.start(app.handle().clone());
      if !background_mode {
        let creating = app.state::<MainWindowCreation>();
        if !creating.0.swap(true, Ordering::AcqRel) {
          let result = create_main_window(app.handle());
          creating.0.store(false, Ordering::Release);
          let window = result?;
          let _ = window.show();
          let _ = window.set_focus();
        }
      }
      setup_tray(app)?;
      Ok(())
    })
    .on_window_event(|window, event| {
      if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.destroy();
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

fn create_main_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
  let config = app
    .config()
    .app
    .windows
    .iter()
    .find(|window| window.label == "main")
    .expect("main window is configured");

  tauri::WebviewWindowBuilder::from_config(app, config)?
    .icon(tauri::include_image!("icons/128x128.png"))?
    .build()
}

fn show_main_window(app: &tauri::AppHandle) {
  if let Some(window) = app.get_webview_window("main") {
    let _ = window.show();
    let _ = window.set_focus();
    return;
  }

  let creating = app.state::<MainWindowCreation>();
  if creating.0.swap(true, Ordering::AcqRel) {
    return;
  }

  let app = app.clone();
  std::thread::spawn(move || {
    let result = create_main_window(&app);
    app
      .state::<MainWindowCreation>()
      .0
      .store(false, Ordering::Release);

    match result {
      Ok(window) => {
        let _ = window.show();
        let _ = window.set_focus();
      }
      Err(error) => eprintln!("failed to create main window: {error}"),
    }
  });
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
  use tauri::menu::{Menu, MenuItem};
  use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

  let open = MenuItem::with_id(app, "open", "打开 SoundTrack", true, None::<&str>)?;
  let exit = MenuItem::with_id(app, "exit", "退出 SoundTrack", true, None::<&str>)?;
  let menu = Menu::with_items(app, &[&open, &exit])?;
  let icon = tauri::include_image!("icons/tray-icon.png");

  TrayIconBuilder::new()
    .icon(icon)
    .tooltip("SoundTrack")
    .menu(&menu)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, event| match event.id().as_ref() {
      "open" => {
        show_main_window(app);
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
        show_main_window(tray.app_handle());
      }
    })
    .build(app)?;

  Ok(())
}
