use super::*;
use std::{
    cell::{Cell, RefCell},
    os::unix::fs::symlink,
    panic::{catch_unwind, panic_any, AssertUnwindSafe},
};

const OLD: &[u8] = b"#!/bin/sh\nprintf 'gamelink-agent 0.1.0\\n'\n";

thread_local! {
    static FAIL_SYNC: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(super) fn check_sync(path: &Path) -> io::Result<()> {
    FAIL_SYNC.with_borrow_mut(|target| {
        if target.as_deref() == Some(path) {
            target.take();
            return Err(io::Error::other("injected directory sync failure"));
        }
        Ok(())
    })
}

fn sandbox(root: &Path) -> Paths {
    let paths = Paths {
        update: root.join("update"),
        installed: root.join("bin/gamelink-agent"),
        installer_uid: effective_uid(),
    };
    fs::create_dir_all(paths.staged()).unwrap();
    fs::create_dir_all(paths.bin_dir()).unwrap();
    fs::write(&paths.installed, OLD).unwrap();
    File::create(paths.update.join(SWAP_LOCK)).unwrap();
    for (path, mode) in [
        (root.to_path_buf(), 0o1770),
        (paths.update.clone(), 0o750),
        (paths.staged(), 0o700),
        (paths.update.join(SWAP_LOCK), 0o644),
        (paths.installed.clone(), 0o755),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    paths
}

fn active() -> String {
    "ActiveState=active\nInvocationID=0123456789abcdef0123456789abcdef\n".into()
}

type OfflineService = Service<fn(&[&str]) -> Result<String>, fn(&Path) -> Result<String>>;

fn no_service() -> OfflineService {
    Service {
        systemctl: |_: &[&str]| panic!("unexpected service command"),
        version: |_: &Path| panic!("unexpected version check"),
        health_window: Duration::ZERO,
        lock_wait: Duration::from_millis(20),
    }
}

fn assert_retired(paths: &Paths) {
    for path in [
        paths.marker(),
        paths.update.join(CLAIMED_REQUEST),
        paths.update.join(PENDING),
        paths.backup(),
        paths.candidate(),
    ] {
        assert!(
            !present(&path).unwrap(),
            "{} was not retired",
            path.display()
        );
    }
}

#[test]
fn health_rejects_an_automatic_restart_or_the_wrong_on_disk_version() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let calls = Cell::new(0);
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
            lock_wait: Duration::ZERO,
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
    let stage_lock = paths.update_dir().unwrap();
    stage_lock.try_lock().unwrap();
    assert!(matches!(
        Stage::acquire(paths.clone(), now_seconds()),
        Err(QueueError::Busy)
    ));
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(paths.update.join("install.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert_eq!(
        install(
            &paths,
            effective_uid(),
            VERSION,
            &no_service(),
            || panic!("second installer ran"),
            |_| {}
        )
        .unwrap_err(),
        "an agent installer is already running"
    );
}

#[test]
fn update_paths_require_exact_permissions_and_refuse_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    for bad_mode in [0o0770, 0o0750] {
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(bad_mode)).unwrap();
        assert!(paths.update_dir().is_err());
    }
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o1770)).unwrap();

    let staged = paths.staged();
    fs::rename(&staged, paths.update.join("original-stage")).unwrap();
    symlink(paths.update.join("original-stage"), &staged).unwrap();
    assert!(paths.staging_dir(effective_uid()).is_err());
    let original = dir.path().join("original-update");
    fs::rename(&paths.update, &original).unwrap();
    symlink(original, &paths.update).unwrap();
    assert!(paths.update_dir().is_err());
}

#[test]
fn untrusted_stage_files_cannot_redirect_the_root_reader() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let owner = effective_uid();
    let file = paths.staged().join("untrusted");
    symlink(&paths.installed, &file).unwrap();
    assert!(read_file(&file, 4096, owner).is_err());
    fs::remove_file(&file).unwrap();
    fs::hard_link(&paths.installed, &file).unwrap();
    assert!(read_file(&file, 4096, owner).is_err());
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
    assert!(read_file(&file, 4096, owner).is_err());
}

#[test]
fn staging_lock_timeout_retires_the_request_without_stopping_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    fs::write(paths.marker(), b"").unwrap();
    let lock = paths.update_dir().unwrap();
    lock.try_lock().unwrap();

    let error = install(
        &paths,
        effective_uid(),
        VERSION,
        &no_service(),
        || Ok(()),
        |_| {},
    )
    .unwrap_err();
    assert!(error.contains("could not lock agent staging"));
    assert_eq!(paths.status(effective_uid()).state, Phase::Failed);
    assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
    assert_retired(&paths);
}

#[test]
fn malformed_requests_are_quarantined_in_one_private_slot() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let marker = paths.marker();
    let check = || {
        let error = install(
            &paths,
            effective_uid(),
            VERSION,
            &no_service(),
            || Ok(()),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("invalid staged update request"));
        assert_eq!(paths.status(effective_uid()).state, Phase::Failed);
        assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
        assert_retired(&paths);
    };

    fs::create_dir(&marker).unwrap();
    symlink(&paths.installed, marker.join("child")).unwrap();
    check();
    let quarantine = paths.update.join(QUARANTINE);
    check_metadata(&open_dir(&quarantine).unwrap(), effective_uid(), 0o700).unwrap();
    let slot = quarantine.join(REQUEST);
    assert_eq!(fs::read_link(slot.join("child")).unwrap(), paths.installed);

    symlink(paths.staged().join("missing"), &marker).unwrap();
    check();
    assert_eq!(
        fs::read_link(&slot).unwrap(),
        paths.staged().join("missing")
    );
    assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 1);
    let mut retired = fs::read_dir(paths.staged()).unwrap();
    let previous = retired.next().unwrap().unwrap().path();
    assert!(retired.next().is_none());
    assert_eq!(
        fs::read_link(previous.join("child")).unwrap(),
        paths.installed
    );
}

#[test]
fn a_dangling_request_symlink_does_not_block_staging() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let target = paths.staged().join("missing");
    symlink(&target, paths.marker()).unwrap();
    assert!(!paths.marker().exists());
    let stage = Stage::acquire(paths.clone(), now_seconds()).unwrap();
    assert!(!present(&paths.marker()).unwrap());
    assert!(!present(&target).unwrap());
    publish_request(&stage.paths).unwrap();
    assert!(paths.marker().exists());
}

#[test]
fn publishing_a_request_requires_a_staging_directory_sync() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    FAIL_SYNC.with_borrow_mut(|target| *target = Some(paths.staged()));
    let result = publish_request(&paths);
    FAIL_SYNC.with_borrow_mut(|target| target.take());
    assert!(result
        .unwrap_err()
        .contains("could not sync staged request: injected directory sync failure"));
    assert!(paths.marker().exists());
}

#[test]
fn an_unrecorded_backup_never_triggers_a_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    fs::write(paths.backup(), b"unrecorded backup").unwrap();
    let error = install(
        &paths,
        effective_uid(),
        VERSION,
        &no_service(),
        || Ok(()),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error, "no staged agent update request");
    assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
    assert_eq!(fs::read(paths.backup()).unwrap(), b"unrecorded backup");
}

#[test]
fn committed_recovery_retires_the_backup_without_rolling_back() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    fs::rename(&paths.installed, paths.backup()).unwrap();
    fs::write(&paths.installed, b"committed replacement").unwrap();
    let mut transaction = Transaction {
        phase: InstallPhase::Committed,
        attempt: 1,
        updated_at: 0,
        version: Some("2.0.0".into()),
        reason: None,
    };
    transaction
        .save(&paths, InstallPhase::Committed, &|_| {})
        .unwrap();

    install(
        &paths,
        effective_uid(),
        VERSION,
        &no_service(),
        || Ok(()),
        |_| {},
    )
    .unwrap();
    assert_eq!(paths.status(effective_uid()).state, Phase::Succeeded);
    assert_eq!(
        fs::read(&paths.installed).unwrap(),
        b"committed replacement"
    );
    assert_retired(&paths);
}

#[test]
fn interrupted_terminal_rename_requires_sync_before_backup_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    fs::rename(&paths.installed, paths.backup()).unwrap();
    fs::write(&paths.installed, b"committed replacement").unwrap();
    let mut transaction = Transaction {
        phase: InstallPhase::HealthChecking,
        attempt: 1,
        updated_at: 0,
        version: Some("2.0.0".into()),
        reason: None,
    };
    transaction
        .save(&paths, InstallPhase::HealthChecking, &|_| {})
        .unwrap();
    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        transaction
            .save(&paths, InstallPhase::Committed, &|point| {
                if point == Checkpoint::Published(InstallPhase::Committed) {
                    panic_any(point);
                }
            })
            .unwrap();
    }));
    assert_eq!(
        interrupted.unwrap_err().downcast_ref::<Checkpoint>(),
        Some(&Checkpoint::Published(InstallPhase::Committed))
    );
    assert_eq!(
        paths.transaction(PENDING).unwrap().unwrap().phase,
        InstallPhase::Committed
    );
    FAIL_SYNC.with_borrow_mut(|target| *target = Some(paths.update.clone()));
    let recover = || {
        install(
            &paths,
            effective_uid(),
            VERSION,
            &no_service(),
            || Ok(()),
            |_| {},
        )
    };
    let error = recover().unwrap_err();
    assert!(error.contains("injected directory sync failure"));
    assert_eq!(fs::read(paths.backup()).unwrap(), OLD);
    recover().unwrap();
    assert_retired(&paths);
}

#[test]
fn cooldown_survives_failed_staging_and_returns_429() {
    use crate::host_admin::request_update;

    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let now = now_seconds();
    let stage = Stage::acquire(paths.clone(), now).unwrap();
    let request_at =
        |time| request_update(b"{}", |_| Stage::acquire(paths.clone(), time).map(drop));
    assert_eq!(request_at(now).unwrap_err().0, 429);
    assert_eq!(
        stage.run(
            None,
            || Err("release check failed"),
            |_| panic!("unexpected download")
        ),
        Err("release check failed".into())
    );
    let saved = paths.status(effective_uid());
    assert_eq!(saved.state, Phase::StagingFailed);
    assert_eq!(
        request_at(saved.updated_at + COOLDOWN.as_secs() - 1)
            .unwrap_err()
            .0,
        429
    );
    assert_eq!(
        request_at(saved.updated_at + COOLDOWN.as_secs()).unwrap().0,
        202
    );
}

#[test]
fn cooldown_ignores_an_implausible_future_staging_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let now = now_seconds();
    let request = || {
        crate::host_admin::request_update(b"{}", |_| Stage::acquire(paths.clone(), now).map(drop))
    };
    for (offset, response) in [
        (COOLDOWN.as_secs(), Err(429)),
        (COOLDOWN.as_secs() + 1, Ok(202)),
    ] {
        save_stage_status(
            &paths.staged().join(STAGE_STATUS),
            &Status {
                updated_at: now + offset,
                state: Phase::Staged,
                ..Status::default()
            },
        )
        .unwrap();
        assert_eq!(request().map(|ok| ok.0).map_err(|err| err.0), response);
    }
}

#[test]
fn staging_cannot_report_installer_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let owner = effective_uid();
    let mut transaction = Transaction {
        phase: InstallPhase::Failed,
        attempt: 1,
        updated_at: 0,
        version: None,
        reason: Some("installer rejected the binary".into()),
    };
    transaction
        .save(&paths, InstallPhase::Failed, &|_| {})
        .unwrap();
    finish(&paths, owner, transaction.phase).unwrap();

    for forged in [
        Phase::Installing,
        Phase::Succeeded,
        Phase::RolledBack,
        Phase::Failed,
    ] {
        save_stage_status(
            &paths.staged().join(STAGE_STATUS),
            &Status {
                attempt: 2,
                state: forged,
                reason: Some("forged status".into()),
                ..Status::default()
            },
        )
        .unwrap();
        let status = paths.status(owner);
        assert_eq!(status.state, Phase::Failed);
        assert_eq!(status.reason, transaction.reason);
    }
}

#[test]
#[ignore = "requires a locally signed agent artifact and compiled-in throwaway pins"]
fn signed_stage_install_tamper_and_rollback() {
    let fixture = std::env::var_os("GAMELINK_TEST_AGENT_RELEASE_DIR").unwrap();
    let fixture = Path::new(&fixture);
    let name = update::agent_asset_name().unwrap();
    let verified_bytes = fs::read(fixture.join(name)).unwrap();
    let latest = || {
        AgentRelease::from_staged(
            "0.1.0",
            fs::read_to_string(fixture.join(update::MANIFEST_NAME)).unwrap(),
            fs::read_to_string(fixture.join(update::MANIFEST_SIGNATURE_NAME)).unwrap(),
        )
    };
    let downloads = |_: &AgentRelease| {
        Ok((
            verified_bytes.clone(),
            fs::read_to_string(fixture.join(format!("{name}.minisig"))).unwrap(),
        ))
    };
    let version = latest().unwrap().version();
    let dir = tempfile::tempdir_in(fixture).unwrap();
    let paths = sandbox(dir.path());
    let owner = effective_uid();
    thread::scope(|scope| {
        let mut worker = None;
        let (code, _) = crate::host_admin::request_update(br#"{"version":"99.0.0"}"#, |expected| {
            let stage = Stage::acquire(paths.clone(), now_seconds())?;
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
        assert_eq!(paths.status(owner).reason, Some(error));
    });
    assert!(!paths.marker().exists());

    let stage = || {
        let after_cooldown = paths.status(owner).updated_at + COOLDOWN.as_secs();
        Stage::acquire(paths.clone(), now_seconds().max(after_cooldown))
            .unwrap()
            .run(Some(&version), latest, downloads)
            .unwrap();
        assert_eq!(paths.status(owner).state, Phase::Staged);
    };
    let systemctl = dir.path().join("fake-systemctl");
    let log = dir.path().join("fake-systemctl.log");
    fs::write(&systemctl, b"#!/bin/sh\nprintf '%s\\n' \"$3\" >> \"$1\"\nif [ \"$3\" = show ]; then printf 'ActiveState=%s\\nInvocationID=0123456789abcdef0123456789abcdef\\n' \"$2\"; fi\n").unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
    let install_fixture = |state: &str, checkpoint: &dyn Fn(Checkpoint)| {
        let service = Service {
            systemctl: |args: &[&str]| {
                command_output(Command::new(&systemctl).arg(&log).arg(state).args(args))
            },
            version: |path: &Path| command_output(Command::new(path).arg("--version")),
            health_window: Duration::from_millis(15),
            lock_wait: Duration::from_millis(20),
        };
        install(&paths, owner, "0.1.0", &service, || Ok(()), checkpoint)
    };
    let staged = paths.staged().join(name);
    let assert_old = || assert_eq!(fs::read(&paths.installed).unwrap(), OLD);
    let assert_new = || assert_eq!(fs::read(&paths.installed).unwrap(), verified_bytes);

    stage();
    let mut binary = fs::read(&staged).unwrap();
    *binary.last_mut().unwrap() ^= 1;
    fs::write(&staged, binary).unwrap();
    let error = install_fixture("active", &|_| {}).unwrap_err();
    assert!(error.contains("signed SHA-256"));
    assert_eq!(paths.status(owner).state, Phase::Failed);
    assert_retired(&paths);
    assert!(!log.exists());
    assert_old();
    eprintln!("tampered staged agent refused before service stop");

    stage();
    let cli = paths.swap_lock().unwrap();
    cli.try_lock_shared().unwrap();
    let error = install_fixture("active", &|_| {}).unwrap_err();
    assert!(error.contains("could not lock agent CLI commands"));
    assert_eq!(paths.status(owner).state, Phase::Failed);
    assert_retired(&paths);
    assert!(!log.exists());
    assert_old();
    drop(cli);
    eprintln!("held CLI lock timed out before service stop");

    stage();
    let error = install_fixture("failed", &|_| {}).unwrap_err();
    assert!(error.contains("previous agent restarted"));
    let rolled_back = paths.status(owner);
    assert_eq!(rolled_back.state, Phase::RolledBack);
    assert!(matches!(
        Stage::acquire(paths.clone(), rolled_back.updated_at),
        Err(QueueError::Cooldown)
    ));
    assert_retired(&paths);
    assert_old();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "stop\nstart\nshow\nstop\nstart\n"
    );
    eprintln!("failed health probe restored and restarted the old agent");

    stage();
    install_fixture("active", &|_| {}).unwrap();
    assert_eq!(paths.status(owner).state, Phase::Succeeded);
    assert_new();
    assert!(same_file(
        &fs::metadata(&paths.installed).unwrap(),
        &fs::metadata(paths.helper()).unwrap()
    ));
    assert_retired(&paths);
    eprintln!("installed signed agent {version} and recorded success");
    fs::write(&paths.installed, OLD).unwrap();

    stage();
    install_fixture("active", &|point| {
        if point == Checkpoint::Verified {
            let replacement = paths.staged().join("replacement");
            fs::write(&replacement, b"unverified replacement").unwrap();
            fs::rename(replacement, &staged).unwrap();
        }
    })
    .unwrap();
    let status = paths.status(owner);
    assert_eq!(status.state, Phase::Succeeded);
    assert_eq!(status.version.as_deref(), Some(version.as_str()));
    assert_eq!(
        fs::metadata(&paths.installed).unwrap().mode() & 0o777,
        0o755
    );
    assert_retired(&paths);
    assert_eq!(fs::read(&staged).unwrap(), b"unverified replacement");
    assert_new();
    eprintln!("installed signed agent {version} from verified bytes after a staged-file swap");

    use InstallPhase::*;
    for phase in [
        Staged,
        Stopping,
        Swapped,
        HealthChecking,
        Committed,
        RolledBack,
        Failed,
    ] {
        fs::write(&log, b"").unwrap();
        fs::write(&paths.installed, OLD).unwrap();
        stage();
        if phase == Failed {
            fs::write(&staged, b"unverified replacement").unwrap();
        }
        let state = if phase == RolledBack {
            "failed"
        } else {
            "active"
        };
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            install_fixture(state, &|point| {
                if point == Checkpoint::Recorded(phase) {
                    panic_any(phase);
                }
            })
        }));
        assert_eq!(
            interrupted.unwrap_err().downcast_ref::<InstallPhase>(),
            Some(&phase)
        );
        assert_eq!(paths.transaction(PENDING).unwrap().unwrap().phase, phase);
        assert_eq!(present(&paths.marker()).unwrap(), phase == Staged);
        assert_eq!(present(&paths.candidate()).unwrap(), phase == Stopping);
        let stage_lock = paths.update_dir().unwrap();
        stage_lock.try_lock().unwrap();
        let cli_lock = paths.swap_lock().unwrap();
        cli_lock.try_lock_shared().unwrap();
        let actions_before = fs::read_to_string(&log).unwrap();
        let recovery = install_fixture("active", &|_| {});
        let actions_after = fs::read_to_string(&log).unwrap();
        let outcome = match phase {
            Staged | Failed => {
                recovery.unwrap();
                assert_eq!(actions_after, actions_before);
                Phase::Failed
            }
            Stopping | Swapped | HealthChecking => {
                assert!(recovery.unwrap_err().contains("previous agent restarted"));
                assert_eq!(actions_after, format!("{actions_before}stop\nstart\n"));
                Phase::RolledBack
            }
            RolledBack | Committed => {
                recovery.unwrap();
                assert_eq!(actions_after, actions_before);
                if phase == RolledBack {
                    Phase::RolledBack
                } else {
                    Phase::Succeeded
                }
            }
        };
        assert_retired(&paths);
        assert_eq!(paths.status(owner).state, outcome);
        if phase == Committed {
            assert_new();
        } else {
            assert_old();
        }
        eprintln!("interrupted {phase:?} recovered to {outcome:?}");
    }

    let trusted = paths.bin_dir().join("trusted");
    fs::copy(std::env::current_exe().unwrap(), &trusted).unwrap();
    fs::set_permissions(&trusted, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(trusted, &paths.installed).unwrap();
    refresh_helper(&paths).unwrap();
    let old_inode = fs::metadata(paths.helper()).unwrap();
    fs::write(&log, b"").unwrap();
    stage();
    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        install_fixture("active", &|point| {
            if point == Checkpoint::Recorded(Swapped) {
                fs::set_permissions(&paths.installed, fs::Permissions::from_mode(0o644)).unwrap();
                panic_any(Swapped);
            }
        })
    }));
    assert_eq!(
        interrupted.unwrap_err().downcast_ref::<InstallPhase>(),
        Some(&Swapped)
    );
    assert_eq!(
        Command::new(&paths.installed)
            .arg("--version")
            .output()
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    let unit = include_str!("../../../../../packaging/systemd/gamelink-agent-update.service");
    let exec = unit
        .lines()
        .find_map(|line| {
            line.strip_prefix("ExecStart=")?
                .strip_suffix(" update install-staged")
        })
        .unwrap();
    let executable = paths.bin_dir().join(Path::new(exec).file_name().unwrap());
    let output = Command::new(&executable)
        .args([
            "--ignored",
            "--exact",
            "self_update::tests::stable_helper_recovery_child",
        ])
        .env("GAMELINK_TEST_RECOVERY_ROOT", dir.path())
        .output()
        .unwrap();
    assert_eq!(Path::new(exec), Paths::system().helper());
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(same_file(
        &old_inode,
        &fs::metadata(&paths.installed).unwrap()
    ));
    assert!(same_file(
        &old_inode,
        &fs::metadata(paths.helper()).unwrap()
    ));
    assert!(same_file(&old_inode, &fs::metadata(executable).unwrap()));
    assert_eq!(paths.status(owner).state, Phase::RolledBack);
    assert_eq!(fs::read_to_string(&log).unwrap(), "stop\nstop\nstart\n");
    assert_retired(&paths);
    eprintln!("the packaged helper executable recovered after an unlaunchable candidate swap");
}

#[test]
#[ignore = "subprocess entry point for the signed recovery test"]
fn stable_helper_recovery_child() {
    let root = PathBuf::from(std::env::var_os("GAMELINK_TEST_RECOVERY_ROOT").unwrap());
    let paths = Paths {
        update: root.join("update"),
        installed: root.join("bin/gamelink-agent"),
        installer_uid: effective_uid(),
    };
    let service = Service {
        systemctl: |args: &[&str]| {
            command_output(
                Command::new(root.join("fake-systemctl"))
                    .arg(root.join("fake-systemctl.log"))
                    .arg("active")
                    .args(args),
            )
        },
        version: |_: &Path| panic!("unexpected version check"),
        health_window: Duration::ZERO,
        lock_wait: Duration::ZERO,
    };
    let error = install(
        &paths,
        effective_uid(),
        VERSION,
        &service,
        || verify_running_helper(&paths),
        |_| {},
    )
    .unwrap_err();
    assert!(error.contains("previous agent restarted"), "{error}");
}
