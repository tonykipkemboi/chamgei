use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

/// Get default config as JSON for the frontend.
#[tauri::command]
fn get_default_config() -> String {
    let config = chamgei_core::ChamgeiConfig::default();
    serde_json::to_string(&config).unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            // Build the tray menu
            let quit_item = MenuItem::with_id(app, "quit", "Quit Chamgei", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit_item])?;

            // Build the tray icon
            TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Chamgei — Voice Dictation")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_default_config])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
