#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;
mod host;
mod host_connection;
mod hosting;
mod minecraft_lists;
mod presence;
#[cfg(windows)]
mod windows_notice;

use gamelink_core::update;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::{Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
#[cfg(not(windows))]
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn release_notice(
    checked: Result<Option<update::CheckResult>, &'static str>,
    enabled_dir: impl FnOnce() -> Option<PathBuf>,
    cached_notice: impl FnOnce(&Path) -> Option<update::CheckResult>,
) -> Value {
    let notice = match checked {
        Ok(checked) => checked.filter(|notice| notice.available && enabled_dir().is_some()),
        Err(error) => {
            host::log_release_check_error(error);
            enabled_dir().and_then(|dir| cached_notice(&dir))
        }
    };
    notice.map_or(Value::Null, |notice| {
        json!({
            "version":notice.release.version,
            "severity":notice.release.severity,
            "releaseNotesUrl":notice.release.release_notes_url,
        })
    })
}

fn public_method(method: &str) -> bool {
    matches!(
        method,
        "version"
            | "identity"
            | "paths"
            | "overview"
            | "servers"
            | "settings"
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
            | "checkRelease"
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
        "checkRelease" => {
            let Some(dir) = host::release_check_dir() else {
                return Ok(Value::Null);
            };
            Ok(release_notice(
                update::check_daily(&dir, VERSION, host::log_release_check_error),
                host::release_check_dir,
                |dir| update::cached_notice(dir, VERSION),
            ))
        }
        "openReleaseNotes" => {
            if let Some(notice) =
                host::release_check_dir().and_then(|dir| update::cached_notice(&dir, VERSION))
            {
                app.opener()
                    .open_url(notice.release.release_notes_url, None::<&str>)
                    .map_err(|_| "could not open release notes")?;
            }
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
        .invoke_handler(tauri::generate_handler![desktop])
        .setup(move |app| {
            let session = bridge::Session::open().map_err(std::io::Error::other)?;
            app.manage(session);
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
    use super::*;

    #[test]
    fn failed_release_check_returns_the_cached_notice_in_its_response() {
        let dir = tempfile::tempdir().unwrap();
        let response = release_notice(
            Err("offline"),
            || Some(dir.path().to_path_buf()),
            |path| {
                assert_eq!(path, dir.path());
                Some(update::CheckResult {
                    installed_version: VERSION.into(),
                    checked_at: "2026-11-17T00:00:00Z".into(),
                    available: true,
                    release: update::Manifest {
                        schema_version: 1,
                        version: "1.1.0".into(),
                        release_date: "2026-11-17".into(),
                        severity: update::Severity::Security,
                        release_notes_url: "https://example.org/releases/next".into(),
                        artifacts: vec![],
                    },
                })
            },
        );
        assert_eq!(
            response,
            json!({
                "version": "1.1.0",
                "severity": "security",
                "releaseNotesUrl": "https://example.org/releases/next",
            })
        );
        assert_eq!(
            release_notice(Err("offline"), || None, |_| panic!("updates disabled")),
            Value::Null
        );
    }

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
