//! Use the notification area directly so unpacked Windows builds can show
//! notifications without a registered Start Menu shortcut.
use tauri::{AppHandle, Manager};
use windows::Win32::UI::{
    Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NOTIFYICONDATAW,
    },
    WindowsAndMessaging::{LoadIconW, IDI_APPLICATION},
};

fn icon_data(app: &AppHandle) -> Option<NOTIFYICONDATAW> {
    let window = app.get_webview_window("main")?;
    Some(NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window.hwnd().ok()?,
        uID: 1,
        ..Default::default()
    })
}

fn copy_text<const N: usize>(out: &mut [u16; N], text: &str) {
    let mut offset = 0;
    for c in text.chars() {
        let size = c.len_utf16();
        if offset + size >= N {
            break;
        }
        c.encode_utf16(&mut out[offset..offset + size]);
        offset += size;
    }
}

pub fn show(app: &AppHandle, message: &str) {
    let Some(mut data) = icon_data(app) else {
        return;
    };
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    copy_text(&mut data.szInfoTitle, "GameLink");
    copy_text(&mut data.szInfo, message);

    // The shell copies the data during each call. LoadIconW returns a shared
    // system icon which remains valid after this function returns.
    unsafe {
        if Shell_NotifyIconW(NIM_MODIFY, &data).as_bool() {
            return;
        }
        let Ok(icon) = LoadIconW(None, IDI_APPLICATION) else {
            return;
        };
        data.hIcon = icon;
        data.uFlags = NIF_ICON | NIF_TIP;
        copy_text(&mut data.szTip, "GameLink");
        if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            data.uFlags = NIF_INFO;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}

pub fn remove(app: &AppHandle) {
    if let Some(data) = icon_data(app) {
        // The shell identifies the icon by the window and ID used to add it.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
}
