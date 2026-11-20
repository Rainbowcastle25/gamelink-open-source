use super::*;
use crate::self_update::tests::sandbox;
use std::cell::{Cell, RefCell};

const OLD: &str = "1.0.0";
const NEW: &str = "2.0.0";
const ID: &str = "0123456789abcdef0123456789abcdef";
const PAYLOAD: &[u8] = b"fake Debian package";

fn request(paths: &Paths, version: &str, id: &str, status_id: &str) {
    save_stage_status(
        &paths.staged().join(STAGE_STATUS),
        &Status {
            state: Phase::Staged,
            version: Some(version.into()),
            attempt: 42,
            attempt_id: Some(status_id.into()),
            updated_at: now_seconds(),
            ..Status::default()
        },
    )
    .unwrap();
    let request = Request {
        version: version.into(),
        attempt_id: id.into(),
    };
    publish_request_bytes(paths, &serde_json::to_vec(&request).unwrap()).unwrap();
}

fn test_service<'a>(running: &'a Cell<Option<&'static str>>) -> ConfiguredService<'a> {
    Service {
        systemctl: Box::new(|args: &[&str]| {
            assert_eq!(args.last(), Some(&UNIT));
            match args[0] {
                "show" if running.get().is_some() => {
                    Ok(format!("ActiveState=active\nInvocationID={ID}\n"))
                }
                "show" => Ok("ActiveState=failed\n".into()),
                "is-enabled" => Ok("enabled\n".into()),
                action => panic!("unexpected systemctl action {action}"),
            }
        }),
        version: Box::new(|_: &Path| {
            Ok(format!(
                "gamelink-agent {}",
                running.get().expect("active agent")
            ))
        }),
        health_window: Duration::ZERO,
        lock_wait: Duration::ZERO,
    }
}

#[test]
fn rejects_invalid_and_mismatched_requests_before_running_apt() {
    for (version, id, status_id, message) in [
        ("--help", ID, ID, "invalid apt update version or attempt ID"),
        (
            NEW,
            "bad-id",
            ID,
            "invalid apt update version or attempt ID",
        ),
        (
            NEW,
            "abcdef0123456789abcdef0123456789",
            ID,
            "apt update does not match the staged attempt",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let paths = sandbox(dir.path());
        let work = paths.update.join(WORK);
        request(&paths, version, id, status_id);
        let apt = Apt {
            work: &work,
            command: |program: &str, _: &[&str], _: &Path| {
                assert_eq!(program, "dpkg-query");
                Ok(String::new())
            },
        };
        let running = Cell::new(None);
        assert_eq!(
            run(
                &paths,
                effective_uid(),
                &apt,
                &test_service(&running),
                |_, _| panic!("unexpected verification"),
                || Ok(File::open(dir.path()).unwrap()),
                || panic!("unexpected source")
            )
            .unwrap_err(),
            message
        );
        let status = paths.status(effective_uid());
        assert_eq!(status.attempt_id.as_deref(), Some(ID));
        assert_eq!(status.state, Phase::Failed);
        assert!(!paths.marker().exists());
        assert_eq!(
            paths.update.join(QUARANTINE).join(REQUEST).exists(),
            id == "bad-id" || version == "--help"
        );
    }
}

#[test]
fn apt_scopes_downloads_and_installs_only_gamelink_then_rolls_back_on_failed_health() {
    for health_fails in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let paths = sandbox(dir.path());
        let work = paths.update.join(WORK);
        request(&paths, NEW, ID, ID);
        let installed = Cell::new(OLD);
        let running = Cell::new(Some(OLD));
        let calls = RefCell::new(Vec::new());
        let apt = Apt {
            work: &work,
            command: |program: &str, args: &[&str], dir: &Path| match program {
                "apt-get" | "apt-cache" => {
                    assert!(args.windows(2).any(|pair| pair
                        == [
                            "-o",
                            "Dir::Etc::sourcelist=/etc/apt/sources.list.d/gamelink.list"
                        ]));
                    assert!(args
                        .windows(2)
                        .any(|pair| pair == ["-o", "Dir::Etc::sourceparts=-"]));
                    assert!(args
                        .windows(2)
                        .any(|pair| pair == ["-o", "APT::Get::List-Cleanup=0"]));
                    assert!(args.windows(2).any(|pair| pair[0] == "-o"
                        && pair[1] == format!("Dir::State::lists={}/lists", dir.display())));
                    let action = args.split_last().unwrap().0;
                    calls.borrow_mut().push((*action).to_owned());
                    match *action {
                        "update" => Ok(String::new()),
                        PACKAGE => Ok(format!("  Candidate: {NEW}\n")),
                        "gamelink-agent=1.0.0" | "gamelink-agent=2.0.0" => {
                            assert_eq!(args[args.len() - 2], "download");
                            let version = action.split_once('=').unwrap().1;
                            fs::write(
                                dir.join(format!("gamelink-agent_{version}_amd64.deb")),
                                PAYLOAD,
                            )
                            .unwrap();
                            Ok(String::new())
                        }
                        _ => panic!("unexpected apt action {action}"),
                    }
                }
                "dpkg-deb" => {
                    assert_eq!(args[0], "--field");
                    let version = args[1].split('_').nth(1).unwrap();
                    Ok(format!("Package: {PACKAGE}\nVersion: {version}\n"))
                }
                "dpkg-query" => Ok(format!(
                    "{PACKAGE} install ok installed {}",
                    installed.get()
                )),
                "dpkg" => {
                    assert_eq!(args[..3], ["--force-confold", "--no-triggers", "--install"]);
                    calls.borrow_mut().push(args[3].into());
                    match args[3] {
                        "target.deb" => {
                            installed.set(NEW);
                            running.set((!health_fails).then_some(NEW));
                        }
                        "previous.deb" => installed.set(OLD),
                        archive => panic!("unexpected archive {archive}"),
                    }
                    Ok(String::new())
                }
                "policy-rc.d" => {
                    assert_eq!(args, [UNIT, "start"]);
                    Ok(String::new())
                }
                "deb-systemd-invoke" => {
                    assert_eq!(args, ["start", UNIT]);
                    running.set(Some(OLD));
                    Ok(String::new())
                }
                _ => panic!("unexpected command {program}"),
            },
        };
        let result = run(
            &paths,
            effective_uid(),
            &apt,
            &test_service(&running),
            |old, new| {
                assert_eq!((old, new), (OLD, NEW));
                Ok(format!("{:x}", Sha256::digest(PAYLOAD)))
            },
            || Ok(File::open(dir.path()).unwrap()),
            || Ok(()),
        );
        let expected = [
            "update",
            PACKAGE,
            "gamelink-agent=2.0.0",
            "gamelink-agent=1.0.0",
            "target.deb",
        ];
        let status = paths.status(effective_uid());
        assert_eq!(status.attempt_id.as_deref(), Some(ID));
        assert_eq!(status.version.as_deref(), Some(NEW));
        assert!(!paths.update.join(PENDING).exists());
        assert!(!paths.marker().exists());
        if health_fails {
            assert_eq!(
                result.unwrap_err(),
                "agent service is not active; previous agent restarted"
            );
            assert_eq!(status.state, Phase::RolledBack);
            assert_eq!(installed.get(), OLD);
            assert_eq!(running.get(), Some(OLD));
            assert_eq!(calls.borrow()[..5], expected);
            assert_eq!(calls.borrow()[5..], ["previous.deb"]);
        } else {
            result.unwrap();
            assert_eq!(status.state, Phase::Succeeded);
            assert_eq!(installed.get(), NEW);
            assert_eq!(*calls.borrow(), expected);
        }

        assert!(matches!(
            Stage::acquire(paths.clone(), now_seconds()),
            Err(QueueError::Cooldown)
        ));
        request(&paths, NEW, ID, ID);
        let retry = run(
            &paths,
            effective_uid(),
            &apt,
            &test_service(&running),
            |_, _| panic!("cooldown bypassed"),
            || Ok(File::open(dir.path()).unwrap()),
            || panic!("cooldown bypassed"),
        );
        assert!(retry.unwrap_err().contains("five minutes"));
    }
}

#[test]
fn held_package_is_declined_before_fetch_or_install() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let work = paths.update.join(WORK);
    request(&paths, NEW, ID, ID);
    let locked = Cell::new(false);
    let apt = Apt {
        work: &work,
        command: |program: &str, args: &[&str], _: &Path| {
            assert!(locked.get());
            assert_eq!(program, "dpkg-query");
            assert_eq!(args.last(), Some(&PACKAGE));
            // The old query hides the selection that apt-mark hold writes.
            if args[1].contains("Status-Want") {
                Ok(format!("{PACKAGE} hold ok installed {OLD}"))
            } else {
                Ok(format!("installed {OLD}"))
            }
        },
    };
    let reason = run(
        &paths,
        effective_uid(),
        &apt,
        &test_service(&Cell::new(Some(OLD))),
        |_, _| panic!("a held package must be declined before fetching"),
        || {
            locked.set(true);
            Ok(File::open(dir.path()).unwrap())
        },
        || Ok(()),
    )
    .unwrap_err();
    assert_eq!(reason, "GameLink is held on the server.");
    let status = paths.status(effective_uid());
    assert_eq!(status.state, Phase::Failed);
    assert_eq!(status.reason.as_deref(), Some(reason.as_str()));
    assert!(!paths.update.join(PENDING).exists());
    assert!(!paths.marker().exists());
}

#[test]
fn interrupted_update_leaves_an_owner_installed_package_alone() {
    for (record, running, phase, reason_fragment) in [
        (
            "gamelink-agent install ok installed 3.0.0",
            Some("3.0.0"),
            Phase::Failed,
            Some("another package operation superseded"),
        ),
        (
            "",
            None,
            Phase::Failed,
            Some("another package operation superseded"),
        ),
        (
            "gamelink-agent hold ok unpacked 2.0.0",
            None,
            Phase::Failed,
            Some("another package operation superseded"),
        ),
        (
            "gamelink-agent install ok installed 1.0.0",
            None,
            Phase::Failed,
            Some("the previous GameLink package remains installed"),
        ),
        (
            "gamelink-agent install ok installed 2.0.0",
            Some(NEW),
            Phase::Succeeded,
            None,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let paths = sandbox(dir.path());
        let work = paths.update.join(WORK);
        fs::create_dir(&work).unwrap();
        let previous = Previous {
            package: OLD.into(),
            binary: OLD.into(),
            enabled: "enabled".into(),
        };
        fs::write(
            work.join("previous.json"),
            serde_json::to_vec(&previous).unwrap(),
        )
        .unwrap();
        Transaction {
            phase: InstallPhase::Staged,
            attempt: 42,
            attempt_id: Some(ID.into()),
            updated_at: 0,
            version: Some(NEW.into()),
            reason: None,
        }
        .save(&paths, InstallPhase::HealthChecking, &|_| {})
        .unwrap();

        let locked = Cell::new(false);
        let apt = Apt {
            work: &work,
            command: |program: &str, _: &[&str], _: &Path| {
                assert!(locked.get());
                assert_eq!(program, "dpkg-query", "recovery must leave dpkg alone");
                Ok(record.into())
            },
        };
        let result = run(
            &paths,
            effective_uid(),
            &apt,
            &test_service(&Cell::new(running)),
            |_, _| panic!("recovery must not verify a new request"),
            || {
                locked.set(true);
                Ok(File::open(dir.path()).unwrap())
            },
            || panic!("recovery must not touch the apt source"),
        );
        let status = paths.status(effective_uid());
        if let Some(fragment) = reason_fragment {
            let reason = result.unwrap_err();
            assert!(reason.contains(fragment), "{record}: {reason}");
            assert_eq!(status.reason.as_deref(), Some(reason.as_str()));
        } else {
            result.unwrap();
            assert!(status.reason.is_none());
        }
        assert_eq!(status.state, phase);
        assert!(!paths.update.join(PENDING).exists());
    }
}

#[test]
#[ignore = "requires a signed Debian release fixture and compiled-in test pins"]
fn signed_api_request_stages_the_apt_release_and_preserves_the_attempt_id() {
    let fixture = PathBuf::from(std::env::var_os("GAMELINK_TEST_AGENT_RELEASE_DIR").unwrap());
    let latest = || {
        AgentRelease::from_staged(
            "0.0.0-0",
            fs::read_to_string(fixture.join(update::MANIFEST_NAME)).unwrap(),
            fs::read_to_string(fixture.join(update::MANIFEST_SIGNATURE_NAME)).unwrap(),
        )
    };
    let version = latest().unwrap().version();
    let dir = tempfile::tempdir().unwrap();
    let mut paths = sandbox(dir.path());
    paths.installed = BINARY.into();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o750)).unwrap();
    let owner = effective_uid();
    let body = serde_json::to_vec(&serde_json::json!({ "version": version })).unwrap();
    let (code, response) = crate::host_admin::request_update(&body, |expected| {
        let stage = Stage::acquire(paths.clone(), now_seconds())?;
        let attempt_id = stage.attempt_id().to_owned();
        stage
            .run(expected.as_deref(), latest, |_| {
                panic!("apt staging must not download the binary")
            })
            .unwrap();
        Ok(attempt_id)
    })
    .unwrap();
    let staged = read_request(&paths.marker(), owner).unwrap();
    assert_eq!(code, 202);
    assert_eq!(response["attemptId"], staged.attempt_id);
    assert_eq!(staged.version, version);
    let status = paths.status(owner).host_view(true);
    assert_eq!(status["attemptId"], staged.attempt_id);
    assert_eq!(status["state"], "staged");
    assert_eq!(
        staged_release(&paths, owner, "0.0.0-0").unwrap().version(),
        version
    );
}
