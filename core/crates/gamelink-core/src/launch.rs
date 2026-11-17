//! Desktop launch host effects. Policy and injectable Manager test hooks remain
//! at the Manager boundary; there is only one production platform launcher.
use serde_json::{json, Value};

pub fn dispatch(method: &str, value: Value) -> Result<Value, String> {
    let text = |key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .ok_or("invalid launch request".to_owned())
    };
    match method {
        "launch.program" => program(text("exe")?, text("args")?)?,
        "launch.uri" => uri(text("uri")?)?,
        _ => return Err("unknown launch method".into()),
    }
    Ok(json!(null))
}

#[cfg(not(windows))]
fn program(exe: &str, args: &str) -> Result<(), String> {
    let mut child = std::process::Command::new(exe)
        .args(args.split_whitespace())
        .current_dir(
            std::path::Path::new(exe)
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        )
        .spawn()
        .map_err(|_| "could not open the configured program")?;
    // Reap without blocking the UI. The game remains independent of GameLink.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
#[cfg(not(windows))]
fn uri(_: &str) -> Result<(), String> {
    Err("opening a game link is only supported on Windows".into())
}

#[cfg(windows)]
fn program(exe: &str, args: &str) -> Result<(), String> {
    shell(
        exe,
        Some(args),
        std::path::Path::new(exe).parent().and_then(|p| p.to_str()),
    )
}
#[cfg(windows)]
fn uri(uri: &str) -> Result<(), String> {
    shell(uri, None, None)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn shell(file: &str, args: Option<&str>, dir: Option<&str>) -> Result<(), String> {
    use windows::{
        core::PCWSTR,
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };
    fn wide(s: &str) -> Result<Vec<u16>, String> {
        if s.contains('\0') {
            return Err("invalid launch request".into());
        }
        Ok(s.encode_utf16().chain(Some(0)).collect())
    }
    let verb = wide("open")?;
    let file = wide(file)?;
    let args = args.filter(|s| !s.is_empty()).map(wide).transpose()?;
    let dir = dir.map(wide).transpose()?;
    let pointer = |v: &Option<Vec<u16>>| v.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    // ShellExecute preserves raw Windows arguments, shortcuts and visible batch consoles.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            pointer(&args),
            pointer(&dir),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        Err("could not open the configured program".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(not(windows))]
    fn whitespace_arguments_and_working_directory() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("game");
        fs::write(&exe, "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > args\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        super::program(exe.to_str().unwrap(), " one\t two\n\"three four\" ").unwrap();
        let out = d.path().join("args");
        for _ in 0..100 {
            if let Ok(s) = fs::read_to_string(&out) {
                if s.ends_with("four\"\n") {
                    assert_eq!(
                        s,
                        format!("{}\none\ntwo\n\"three\nfour\"\n", d.path().display())
                    );
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("program did not record its arguments");
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn rejects_unknown_methods_and_malformed_requests() {
        assert_eq!(
            dispatch("launch.nope", json!({})).unwrap_err(),
            "unknown launch method"
        );
        for (method, input) in [
            ("launch.program", json!({})),
            ("launch.program", json!({"exe":"x"})),
            ("launch.program", json!({"exe":1,"args":""})),
            ("launch.uri", json!({})),
            ("launch.uri", json!({"uri":5})),
        ] {
            assert_eq!(
                dispatch(method, input).unwrap_err(),
                "invalid launch request"
            );
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn uri_launch_is_windows_only() {
        assert_eq!(
            dispatch("launch.uri", json!({"uri":"minecraft://connect"})).unwrap_err(),
            "opening a game link is only supported on Windows"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn program_launch_reports_unstartable_programs() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert_eq!(
            dispatch(
                "launch.program",
                json!({"exe":missing,"args":"--connect 127.0.0.1:1"})
            )
            .unwrap_err(),
            "could not open the configured program"
        );
        // A non-executable file is not startable either.
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "text").unwrap();
        assert_eq!(
            dispatch("launch.program", json!({"exe":plain,"args":""})).unwrap_err(),
            "could not open the configured program"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn program_launch_succeeds_and_passes_no_arguments_for_empty_args() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game");
        fs::write(&exe, "#!/bin/sh\nprintf 'argc=%s\\n' \"$#\" > out\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            dispatch("launch.program", json!({"exe":exe,"args":""})).unwrap(),
            Value::Null
        );
        let out = dir.path().join("out");
        for _ in 0..200 {
            if let Ok(text) = fs::read_to_string(&out) {
                if text.ends_with('\n') {
                    assert_eq!(text, "argc=0\n");
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("program did not run");
    }

    // Ported from tests/testsupport/client TestLaunchPassesArguments: the real
    // ShellExecute path hands the connect arguments to the program.
    #[test]
    #[cfg(windows)]
    fn shell_execute_passes_arguments_to_a_batch_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("args.txt");
        let bat = dir.path().join("game.bat");
        let script = format!("@echo off\r\necho %*> \"{}\"\r\nexit\r\n", out.display());
        std::fs::write(&bat, script).unwrap();
        dispatch(
            "launch.program",
            json!({"exe":bat,"args":"--mp-connect 127.0.0.1:34198"}),
        )
        .unwrap();
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&out) {
                if !text.is_empty() {
                    assert_eq!(text.trim(), "--mp-connect 127.0.0.1:34198");
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the program was not started, or did not receive its arguments");
    }

    #[test]
    #[cfg(windows)]
    fn embedded_nul_is_rejected_before_calling_the_shell() {
        assert_eq!(
            dispatch("launch.program", json!({"exe":"a\u{0}b","args":""})).unwrap_err(),
            "invalid launch request"
        );
        assert_eq!(
            dispatch("launch.program", json!({"exe":"C:\\a.exe","args":"a\u{0}"})).unwrap_err(),
            "invalid launch request"
        );
        assert_eq!(
            dispatch("launch.uri", json!({"uri":"x\u{0}"})).unwrap_err(),
            "invalid launch request"
        );
    }
}
