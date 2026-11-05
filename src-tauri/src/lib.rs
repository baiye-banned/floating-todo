use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_dialog::DialogExt;

#[cfg(target_os = "windows")]
mod windows_desktop;

/// 显示或隐藏悬浮窗。
fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

/// Windows：全局监听 Ctrl 按下/松开，让浮窗在按住 Ctrl 时临时切换鼠标穿透。
/// 使用轮询而不是注册热键，避免占用 Ctrl 组合键（Ctrl+C / Ctrl+V 等照常使用）。
#[cfg(target_os = "windows")]
fn spawn_ctrl_watcher(app: AppHandle) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};

    std::thread::spawn(move || {
        let mut was_down = false;
        loop {
            // 最高位为 1 表示当前按住；同时覆盖左右 Ctrl。
            let is_down = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
            if is_down != was_down {
                was_down = is_down;
                let _ = app.emit("ctrl-state", is_down);
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    });
}

/// 导出数据到用户选择的文件。
#[tauri::command]
async fn export_data(app: AppHandle, json: String) -> Result<bool, String> {
    let file = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name("floating-todo-backup.json")
        .blocking_save_file();
    match file {
        Some(p) => {
            let path = p.into_path().map_err(|e| e.to_string())?;
            std::fs::write(path, json).map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// 从用户选择的文件导入数据，返回文件内容。
#[tauri::command]
async fn import_data(app: AppHandle) -> Result<Option<String>, String> {
    let file = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file();
    match file {
        Some(p) => {
            let path = p.into_path().map_err(|e| e.to_string())?;
            let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            Ok(Some(content))
        }
        None => Ok(None),
    }
}

/// 将窗口放入 Windows 桌面层，使其在 Win+D 显示桌面后仍然可见。
#[tauri::command]
fn set_desktop_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| "main window not found".to_string())?;
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        windows_desktop::set_desktop_mode(hwnd, enabled)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, enabled);
        Ok(())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init());

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_global_shortcut::Builder::new().build())
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                Some(vec![]),
            ));
    }

    builder
        .invoke_handler(tauri::generate_handler![
            export_data,
            import_data,
            set_desktop_mode
        ])
        .setup(|app| {
            // macOS：作为状态栏小组件运行，不占用程序坞。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }

            // Windows：全局监听 Ctrl；前端根据当前穿透状态做临时反转。
            #[cfg(target_os = "windows")]
            spawn_ctrl_watcher(app.handle().clone());

            // 全局快捷键：CmdOrCtrl+Shift+Space 唤起并聚焦输入框（快速捕获）。
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                let handle = app.handle().clone();
                let _ = app.global_shortcut().on_shortcut(
                    "CommandOrControl+Shift+Space",
                    move |_app, _shortcut, event| {
                        if event.state == ShortcutState::Pressed {
                            if let Some(win) = handle.get_webview_window("main") {
                                let _ = win.show();
                                let _ = win.set_focus();
                                let _ = win.emit("quick-capture", ());
                            }
                        }
                    },
                );
            }

            let toggle_item =
                MenuItem::with_id(app, "toggle", "显示/隐藏小组件", true, None::<&str>)?;
            #[cfg(target_os = "windows")]
            let desktop_item =
                MenuItem::with_id(app, "desktop", "切换固定在桌面", true, None::<&str>)?;
            let passthrough_item =
                MenuItem::with_id(app, "passthrough", "切换鼠标穿透", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            #[cfg(target_os = "windows")]
            let menu = Menu::with_items(
                app,
                &[&toggle_item, &desktop_item, &passthrough_item, &quit_item],
            )?;
            #[cfg(not(target_os = "windows"))]
            let menu = Menu::with_items(app, &[&toggle_item, &passthrough_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("悬浮待办")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "toggle" => toggle_window(app),
                    "passthrough" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.emit("toggle-passthrough", ());
                        }
                    }
                    "desktop" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.emit("toggle-desktop-mode", ());
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
