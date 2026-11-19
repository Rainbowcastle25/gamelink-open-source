use super::*;
use std::{cell::RefCell, os::unix::fs::symlink};

const OLD: &[u8] = b"#!/bin/sh\nprintf 'gamelink-agent 0.1.0\\n'\n";

fn sandbox(root: &Path) -> Paths {
    let paths = Paths {
        update: root.join("update"),
        installed: root.join("bin/gamelink-agent"),
    };
    fs::create_dir_all(paths.staged()).unwrap();
    fs::create_dir_all(paths.bin_dir()).unwrap();
    fs::write(&paths.installed, OLD).unwrap();
    fs::set_permissions(&paths.installed, fs::Permissions::from_mode(0o755)).unwrap();
    File::create(paths.update.join(SWAP_LOCK)).unwrap();
    paths
}

fn active() -> String {
    "ActiveState=active\nInvocationID=0123456789abcdef0123456789abcdef\n".into()
}

#[test]
fn swap_and_rollback_run_only_while_the_service_is_stopped() {
    for healthy in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let paths = sandbox(dir.path());
        let calls = RefCell::new(Vec::new());
        let replacement = b"new agent";
        let service = Service {
            systemctl: |args: &[&str]| {
                calls.borrow_mut().push(args[0].to_owned());
                assert_eq!(args.last(), Some(&UNIT));
                let bytes = fs::read(&paths.installed).unwrap();
                match (args[0], calls.borrow().len()) {
                    ("stop", 1) => assert_eq!(bytes, OLD),
                    ("start", 2) => assert_eq!(bytes, replacement),
                    ("stop", _) => assert_eq!(bytes, replacement),
                    ("start", _) => assert_eq!(bytes, OLD),
                    _ => {}
                }
                if args[0] == "show" && !healthy {
                    return Ok("ActiveState=failed\n".into());
                }
                Ok(active())
            },
            version: |_: &Path| Ok("gamelink-agent 2.0.0\n".into()),
            health_window: Duration::ZERO,
        };
        let result = service.swap(&paths, replacement, "2.0.0", || Ok(()));
        assert!(!paths.backup().exists());
        assert_eq!(
            fs::metadata(&paths.installed).unwrap().mode() & 0o777,
            0o755
        );
        if healthy {
            assert!(result.is_ok());
            assert_eq!(fs::read(&paths.installed).unwrap(), replacement);
            assert_eq!(*calls.borrow(), ["stop", "start", "show", "show"]);
        } else {
            assert!(result.unwrap_err().contains("previous agent restarted"));
            assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
            assert_eq!(*calls.borrow(), ["stop", "start", "show", "stop", "start"]);
        }
    }
}

#[test]
fn health_rejects_an_automatic_restart_or_the_wrong_on_disk_version() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let calls = std::cell::Cell::new(0);
    for version in ["gamelink-agent 2.0.0", "gamelink-agent 1.0.0"] {
        calls.set(0);
        let service = Service {
            systemctl: |_: &[&str]| {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Ok(active())
                } else {
                    Ok(active().replace("0123456789abcdef", "1123456789abcdef"))
                }
            },
            version: |_: &Path| Ok(version.into()),
            health_window: Duration::ZERO,
        };
        let error = service.health(&paths.installed, "2.0.0").unwrap_err();
        if version.contains("2.0.0") {
            assert!(error.contains("restarted"));
        } else {
            assert!(error.contains("signed release version"));
        }
    }
}

#[test]
fn concurrent_stage_and_installer_runs_are_locked_out() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let stage = Stage::acquire(paths.clone()).unwrap();
    assert!(matches!(
        Stage::acquire(paths.clone()),
        Err(QueueError::Busy)
    ));
    drop(stage);
    let lock = File::create(paths.update.join("install.lock")).unwrap();
    lock.try_lock().unwrap();
    let service = Service {
        systemctl: |_: &[&str]| panic!("unexpected service command"),
        version: |_: &Path| panic!("unexpected version check"),
        health_window: Duration::ZERO,
    };
    assert_eq!(
        install(&paths, 0, VERSION, &service, || panic!(
            "second installer ran"
        ))
        .unwrap_err(),
        "an agent installer is already running"
    );
}

#[test]
fn untrusted_stage_files_cannot_redirect_the_root_reader() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let meta = fs::metadata(&paths.installed).unwrap();
    let file = paths.staged().join("untrusted");
    symlink(&paths.installed, &file).unwrap();
    assert!(read_file(&file, 4096, Some(meta.uid())).is_err());
    fs::remove_file(&file).unwrap();
    fs::hard_link(&paths.installed, &file).unwrap();
    assert!(read_file(&file, 4096, Some(meta.uid())).is_err());
    fs::remove_file(&file).unwrap();
    assert_eq!(
        unsafe {
            libc::mkfifo(
                std::ffi::CString::new(file.as_os_str().as_encoded_bytes())
                    .unwrap()
                    .as_ptr(),
                0o600,
            )
        },
        0
    );
    assert!(read_file(&file, 4096, Some(meta.uid())).is_err());
}

#[test]
fn installer_recovers_an_interrupted_swap_before_reading_staged_files() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    fs::rename(&paths.installed, paths.backup()).unwrap();
    fs::write(&paths.installed, b"interrupted replacement").unwrap();
    fs::write(paths.marker(), b"malformed").unwrap();
    let calls = RefCell::new(Vec::new());
    let service = Service {
        systemctl: |args: &[&str]| {
            calls.borrow_mut().push(args[0].to_owned());
            Ok(String::new())
        },
        version: |_: &Path| panic!("recovery must not execute the candidate"),
        health_window: Duration::ZERO,
    };
    let error = install(&paths, 0, VERSION, &service, || Ok(())).unwrap_err();
    assert!(error.contains("installation was interrupted; previous agent restarted"));
    assert_eq!(*calls.borrow(), ["stop", "start"]);
    assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
    assert!(!paths.marker().exists());
    assert!(!paths.backup().exists());
    let status = paths.status();
    assert!(status.state == Phase::Failed);
    assert_eq!(status.reason, Some(error));
}

#[test]
#[ignore = "requires a locally signed agent artifact and compiled-in throwaway pins"]
fn signed_stage_install_tamper_and_rollback() {
    let fixture = std::env::var_os("GAMELINK_TEST_AGENT_RELEASE_DIR").unwrap();
    let fixture = Path::new(&fixture);
    let name = update::agent_asset_name().unwrap();
    let latest = || {
        AgentRelease::from_staged(
            "0.1.0",
            fs::read_to_string(fixture.join(update::MANIFEST_NAME)).unwrap(),
            fs::read_to_string(fixture.join(update::MANIFEST_SIGNATURE_NAME)).unwrap(),
        )
    };
    let downloads = |_: &AgentRelease| {
        Ok((
            fs::read(fixture.join(name)).unwrap(),
            fs::read_to_string(fixture.join(format!("{name}.minisig"))).unwrap(),
        ))
    };
    let version = latest().unwrap().version();
    let dir = tempfile::tempdir_in(fixture).unwrap();
    let paths = sandbox(dir.path());
    thread::scope(|scope| {
        let mut worker = None;
        let (code, _) = crate::host_admin::request_update(br#"{"version":"99.0.0"}"#, |expected| {
            let stage = Stage::acquire(paths.clone())?;
            worker = Some(scope.spawn(move || {
                stage.run(expected.as_deref(), latest, |_| {
                    panic!("wrong version must not download")
                })
            }));
            Ok(())
        })
        .unwrap();
        assert_eq!(code, 202);
        let error = worker.unwrap().join().unwrap().unwrap_err();
        assert_eq!(error, "requested version is not the latest signed release");
        assert_eq!(paths.status().reason, Some(error));
    });
    assert!(!paths.marker().exists());

    let stage = || {
        Stage::acquire(paths.clone())
            .unwrap()
            .run(Some(&version), latest, downloads)
            .unwrap();
        assert!(paths.status().state == Phase::Staged);
    };
    let systemctl = dir.path().join("fake-systemctl");
    let log = dir.path().join("fake-systemctl.log");
    fs::write(&systemctl, b"#!/bin/sh\nprintf '%s\\n' \"$3\" >> \"$1\"\nif [ \"$3\" = show ]; then printf 'ActiveState=%s\\nInvocationID=0123456789abcdef0123456789abcdef\\n' \"$2\"; fi\n").unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
    let owner = fs::metadata(paths.staged()).unwrap().uid();
    let install_fixture = |state: &str| {
        let service = Service {
            systemctl: |args: &[&str]| {
                command_output(Command::new(&systemctl).arg(&log).arg(state).args(args))
            },
            version: |path: &Path| command_output(Command::new(path).arg("--version")),
            health_window: Duration::from_millis(15),
        };
        install(&paths, owner, "0.1.0", &service, || Ok(()))
    };

    stage();
    let staged = paths.staged().join(name);
    let mut binary = fs::read(&staged).unwrap();
    let last = binary.last_mut().unwrap();
    *last ^= 1;
    fs::write(&staged, binary).unwrap();
    let error = install_fixture("active").unwrap_err();
    assert!(error.contains("signed SHA-256"));
    assert!(paths.status().state == Phase::Failed);
    assert!(!paths.marker().exists());
    assert!(!log.exists());
    assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
    eprintln!("tampered staged agent refused before service stop");

    stage();
    let error = install_fixture("failed").unwrap_err();
    assert!(error.contains("previous agent restarted"));
    assert!(paths.status().state == Phase::Failed);
    assert!(!paths.marker().exists());
    assert!(!paths.backup().exists());
    assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "stop\nstart\nshow\nstop\nstart\n"
    );
    eprintln!("failed health probe restored and restarted the old agent");

    stage();
    install_fixture("active").unwrap();
    let status = paths.status();
    assert!(status.state == Phase::Succeeded);
    assert_eq!(status.version.as_deref(), Some(version.as_str()));
    assert!(!paths.marker().exists());
    assert!(!paths.backup().exists());
    assert_eq!(
        fs::read(&paths.installed).unwrap(),
        fs::read(fixture.join(name)).unwrap()
    );
    eprintln!("installed signed agent {version} and recorded succeeded");
}
