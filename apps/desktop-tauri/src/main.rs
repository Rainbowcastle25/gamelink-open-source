#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;
mod game_icons;
mod host;
mod host_connection;
mod hosting;
mod minecraft_lists;
mod panel_inventory;
mod presence;
mod profile;
#[cfg(target_os = "linux")]
mod restart;
mod updater;
#[cfg(windows)]
mod windows_notice;

use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
#[cfg(not(windows))]
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn public_method(method: &str) -> bool {
    matches!(
        method,
        "version"
            | "identity"
            | "paths"
            | "overview"
            | "servers"
            | "settings"
            | "gameIcons"
            | "activity"
            | "clearActivity"
            | "join"
            | "disconnect"
            | "removeServer"
            | "launchGame"
            | "check"
            | "checkAll"
            | "diagnostics"
            | "previewInvite"
            | "importInvite"
            | "addServer"
            | "updateServer"
            | "updateSettings"
            | "panelConnection"
            | "refreshPanelInventory"
            | "hostConnection"
            | "configureHost"
            | "hostStatus"
            | "refreshHost"
            | "importHostGame"
            | "hostedGames"
            | "createHostedGame"
            | "shareLanWorld"
            | "scanLanWorlds"
            | "stopLanScan"
            | "approveHostedRequest"
            | "dismissHostedRequest"
            | "approveHostedDevice"
            | "revokeHostedDevice"
            | "setHostingEnabled"
            | "removeHostedGame"
            | "copyHostedInvitation"
            | "copyLinkedInvitation"
            | "approveLinkedRequest"
            | "dismissLinkedRequest"
            | "copyText"
            | "pickExecutable"
            | "openConfigFolder"
            | "openReleaseNotes"
    )
}

#[tauri::command]
async fn desktop(app: tauri::AppHandle, request: Value) -> Result<Value, String> {
    let size = serde_json::to_vec(&request)
        .map_err(|_| "invalid desktop request")?
        .len();
    if size > bridge::MAX_INPUT {
        return Err("desktop request exceeds size limit".into());
    }
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or("invalid desktop command")?;
    if !public_method(method) {
        return Err("unknown desktop command".into());
    }
    tauri::async_runtime::spawn_blocking(move || native_or_manager(&app, request))
        .await
        .map_err(|_| "desktop command failed".to_owned())?
}
fn native_or_manager(app: &tauri::AppHandle, request: Value) -> Result<Value, String> {
    if matches!(
        request["method"].as_str().unwrap_or(""),
        "hostConnection"
            | "configureHost"
            | "hostStatus"
            | "refreshHost"
            | "importHostGame"
            | "approveLinkedRequest"
            | "dismissLinkedRequest"
    ) {
        return host_connection::call(request);
    }
    match request["method"].as_str().unwrap_or("") {
        "panelConnection" | "refreshPanelInventory" => panel_inventory::call(request),
        "openReleaseNotes" => {
            if let Some(notice) = host::release_check_dir()
                .and_then(|dir| gamelink_core::update::cached_notice(&dir, VERSION))
            {
                app.opener()
                    .open_url(notice.release.release_notes_url, None::<&str>)
                    .map_err(|_| "could not open release notes")?;
            }
            Ok(Value::Null)
        }
        "openAptUpdateGuide" => {
            app.opener()
                .open_url(
                    "https://github.com/Rainbowcastle25/gamelink-open-source/blob/master/docs/agent-setup.md#enable-desktop-updates-with-apt",
                    None::<&str>,
                )
                .map_err(|_| "could not open the apt update guide")?;
            Ok(Value::Null)
        }
        "copyHostedInvitation" | "copyLinkedInvitation" => {
            let invite = if request["method"] == "copyLinkedInvitation" {
                host_connection::invitation(&request)?
            } else {
                host::call(json!({"method":"hostedInvitation","id":request["id"]}))?
                    .as_str()
                    .ok_or("could not create the invitation")?
                    .to_owned()
            };
            app.clipboard()
                .write_text(invite)
                .map_err(|_| "could not access the clipboard")?;
            Ok(Value::Null)
        }
        "copyText" => {
            let text = request["text"].as_str().ok_or("invalid clipboard text")?;
            if text.len() > 4096 {
                return Err("text too long".into());
            }
            app.clipboard()
                .write_text(text)
                .map_err(|_| "could not access the clipboard")?;
            Ok(Value::Null)
        }
        "pickExecutable" => {
            let title = request["title"].as_str().ok_or("invalid dialog title")?;
            let file = app
                .dialog()
                .file()
                .set_title(title)
                .add_filter("Programs", &["exe", "bat", "cmd", "lnk"])
                .add_filter("All files", &["*"])
                .blocking_pick_file();
            match file {
                None => Ok(json!("")),
                Some(file) => {
                    let path = file
                        .into_path()
                        .map_err(|_| "could not read selected file path")?;
                    Ok(json!(path
                        .to_str()
                        .ok_or("selected file path is not valid UTF-8")?))
                }
            }
        }
        "openConfigFolder" => {
            let paths = host::call(json!({"method":"paths"}))?;
            let path = paths["configDir"]
                .as_str()
                .ok_or("could not locate configuration folder")?;
            app.opener()
                .open_path(path, None::<&str>)
                .map_err(|_| "could not open configuration folder")?;
            Ok(Value::Null)
        }
        "gameIcons" => {
            let settings = host::call(json!({"method":"settings"}))?;
            let executables = &settings["gameExecutables"];
            Ok(app
                .state::<game_icons::Cache>()
                .get_or_load(executables, || game_icons::load(app, executables)))
        }
        _ => host::call(request),
    }
}

struct Worker {
    running: Arc<AtomicBool>,
    thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Worker {
    fn stop(&self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    restart::wait_for_parent();
    let running = Arc::new(AtomicBool::new(true));
    let active = running.clone();
    let for_setup = running.clone();
    let builder = tauri::Builder::default()
        // This plugin must be first: a second process never opens private state.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    #[cfg(not(windows))]
    let builder = builder.plugin(tauri_plugin_notification::init());
    #[cfg(target_os = "linux")]
    let builder = builder.manage(restart::Handoff::default());
    #[cfg(windows)]
    let builder = builder.on_window_event(|window, event| {
        if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
            windows_notice::remove(window.app_handle());
        }
    });
    let app = builder
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::UpdateState::default())
        .invoke_handler(tauri::generate_handler![
            desktop,
            updater::check_release,
            updater::update_linked_agent,
            updater::install_update,
            #[cfg(target_os = "linux")]
            restart::restart_game_link
        ])
        .setup(move |app| {
            let session = bridge::Session::open().map_err(std::io::Error::other)?;
            app.manage(session);
            app.manage(game_icons::Cache::default());
            let handle = app.handle().clone();
            let worker = std::thread::spawn(move || {
                while active.load(Ordering::Acquire) {
                    if let Ok(bits) = host::call(json!({"method":"updates"})) {
                        let bits = bits.as_u64().unwrap_or(0);
                        for (bit, topic) in [(1, "servers"), (2, "activity"), (4, "settings")] {
                            if bits & bit != 0 {
                                let _ = handle.emit("gamelink:update", topic);
                            }
                        }
                        #[cfg(windows)]
                        if bits & 8 != 0 {
                            let app = handle.clone();
                            let _ = handle.run_on_main_thread(move || windows_notice::remove(&app));
                        }
                    }
                    if let Some(message) = host::take_online_notification() {
                        #[cfg(not(windows))]
                        let _ = handle.notification().builder().title(message).show();
                        #[cfg(windows)]
                        {
                            let app = handle.clone();
                            let _ = handle
                                .run_on_main_thread(move || windows_notice::show(&app, &message));
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
            app.manage(Worker {
                running: for_setup.clone(),
                thread: std::sync::Mutex::new(Some(worker)),
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|_| {
            eprintln!("GameLink could not start");
            std::process::exit(1)
        });
    app.run(move |app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            running.store(false, Ordering::Release);
            app.state::<Worker>().stop();
            // Explicitly release Manager/tunnels, even while app handles remain
            // in the event worker. Shutdown is idempotent with Session::drop.
            let _ = host::call(json!({"method":"shutdown"}));
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn webview_cannot_control_ownership() {
        for method in [
            "init",
            "shutdown",
            "updates",
            "launch.program",
            "launch.uri",
            "hostedInvitation",
            "host_start",
            "host_address",
        ] {
            assert!(!super::public_method(method));
        }
    }
}
