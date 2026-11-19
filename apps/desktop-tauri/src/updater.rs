use crate::{host, VERSION};
use gamelink_core::update::{self, CheckResult, Manifest};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::{
    async_runtime::{spawn_blocking, Mutex},
    ipc::Channel,
    utils::{config::BundleType, platform::bundle_type},
    AppHandle, Manager, State,
};
use tauri_plugin_updater::{Error, Update, UpdaterBuilder, UpdaterExt};
use url::Url;

const RELEASE_DOWNLOAD: &str =
    "https://github.com/Rainbowcastle25/gamelink-open-source/releases/download";
const SUMMARY_ASSET: &str = "release-summary.txt";
const DOWNLOAD_INTERVAL: Duration = Duration::from_millis(100);
const INSTALLER_UNAVAILABLE: &str =
    "The installer is unavailable. Try again or open the release notes.";
const INSTALL_FAILED: &str =
    "Could not install the update. Check installation permissions or open the release notes.";

#[derive(Default)]
pub struct UpdateState(Mutex<Option<PreparedUpdate>>);

struct PreparedUpdate {
    update: Update,
    artifact_digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNotice {
    version: String,
    severity: update::Severity,
    release_notes_url: String,
    summary: Option<String>,
    can_install: bool,
    installer_error: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(tag = "stage", rename_all = "camelCase")]
pub enum UpdateProgress {
    Downloading { downloaded: u64, total: Option<u64> },
    Verifying,
    Installing,
}

fn enabled_dir() -> Option<PathBuf> {
    update::can_check().then(host::release_check_dir).flatten()
}

fn release_notice(
    checked: Result<Option<CheckResult>, &'static str>,
    enabled_dir: impl FnOnce() -> Option<PathBuf>,
    cached_notice: impl FnOnce(&Path) -> Option<CheckResult>,
) -> Option<CheckResult> {
    match checked {
        Ok(checked) => checked.filter(|notice| notice.available && enabled_dir().is_some()),
        Err(error) => {
            host::log_release_check_error(error);
            enabled_dir().and_then(|dir| cached_notice(&dir))
        }
    }
}

fn artifact_name(version: &str, bundle: Option<BundleType>, appimage: bool) -> Option<String> {
    match bundle {
        Some(BundleType::AppImage) if appimage => {
            Some(format!("GameLink_{version}_amd64.AppImage"))
        }
        Some(BundleType::Nsis) => Some(format!("GameLink_{version}_x64-setup.exe")),
        _ => None,
    }
}

fn artifact_digest<'a>(release: &'a Manifest, name: &str) -> Option<&'a str> {
    release
        .artifacts
        .iter()
        .find(|artifact| artifact.name == name)
        .map(|artifact| artifact.sha256.as_str())
}

fn matches_digest(bytes: &[u8], expected: &str) -> bool {
    format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(expected)
}

fn verified_summary(release: &Manifest, body: Option<&str>) -> Option<String> {
    let summary = body.filter(|body| !body.is_empty() && body.len() <= 512)?;
    let digest = artifact_digest(release, SUMMARY_ASSET)?;
    matches_digest(summary.as_bytes(), digest).then(|| summary.to_owned())
}

fn matching_installer(release: &Manifest, name: &str, version: &str, url: &Url) -> bool {
    version == release.version && url.as_str() == format!("{RELEASE_DOWNLOAD}/v{version}/{name}")
}

async fn prepare_update(app: &AppHandle) -> Result<Option<Update>, Error> {
    let endpoint = app.config().plugins.0["updater"]["endpoints"]
        .get(0)
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::EmptyEndpoints)?
        .parse()?;
    let feed = spawn_blocking(move || update::fetch_updater_feed(endpoint))
        .await
        .map_err(std::io::Error::other)?
        .map_err(std::io::Error::other)?;
    installer_updater(app)?.build()?.restore_update(feed)
}

fn installer_updater<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<UpdaterBuilder, Error> {
    Ok(app
        .updater_builder()
        .header("User-Agent", "GameLink")?
        .no_proxy()
        .configure_client(|client| {
            // The bounded feed bypasses Updater::check, which normally sets up rustls.
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::ring::default_provider().install_default();
            }
            client.https_only(true)
        })
        // The built-in installer exits after ShellExecuteW succeeds. Its default
        // cleanup hook hides the window before ShellExecuteW reports an error.
        .on_before_exit(|| {}))
}

fn appimage_running(app: &AppHandle) -> bool {
    #[cfg(target_os = "linux")]
    {
        app.env().appimage.is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        false
    }
}

#[tauri::command]
pub async fn check_release(
    app: AppHandle,
    state: State<'_, UpdateState>,
) -> Result<Option<ReleaseNotice>, String> {
    let mut prepared = state.0.lock().await;
    *prepared = None;
    let Some(dir) = enabled_dir() else {
        return Ok(None);
    };
    let checked = spawn_blocking(move || {
        release_notice(
            update::check_desktop(&dir, VERSION, host::log_release_check_error),
            enabled_dir,
            |dir| update::cached_notice(dir, VERSION),
        )
    })
    .await
    .map_err(|_| "Could not check for GameLink updates.")?;

    let Some(checked) = checked else {
        return Ok(None);
    };
    let release = checked.release;
    let mut installer_error = None;
    let has_updater_pin = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config["pubkey"].as_str())
        .is_some_and(|key| !key.trim().is_empty());
    let candidate = if has_updater_pin {
        artifact_name(&release.version, bundle_type(), appimage_running(&app)).and_then(|name| {
            let digest = artifact_digest(&release, &name)?;
            Some((name, digest))
        })
    } else {
        None
    };
    if let Some((name, digest)) = candidate {
        match prepare_update(&app).await {
            Ok(Some(mut update))
                if matching_installer(&release, &name, &update.version, &update.download_url) =>
            {
                update.timeout = Some(Duration::from_secs(10 * 60));
                *prepared = Some(PreparedUpdate {
                    update,
                    artifact_digest: digest.to_owned(),
                });
            }
            _ => installer_error = Some(INSTALLER_UNAVAILABLE),
        }
    }
    if enabled_dir().is_none() {
        *prepared = None;
        return Ok(None);
    }
    let summary = prepared
        .as_ref()
        .and_then(|prepared| verified_summary(&release, prepared.update.body.as_deref()));
    Ok(Some(ReleaseNotice {
        version: release.version,
        severity: release.severity,
        release_notes_url: release.release_notes_url,
        summary,
        can_install: prepared.is_some(),
        installer_error,
    }))
}

fn ensure_install_allowed(
    version: &str,
    enabled: Option<&Path>,
    cached_notice: impl FnOnce(&Path) -> Option<CheckResult>,
) -> Result<(), &'static str> {
    let dir = enabled.ok_or("GameLink updates are disabled in Settings.")?;
    if cached_notice(dir).is_some_and(|notice| notice.release.version == version) {
        Ok(())
    } else {
        Err("The available update changed. Check again for the current version.")
    }
}

fn download_error(error: Error) -> &'static str {
    match error {
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion => "The installer signature could not be verified.",
        _ => "Could not download the installer. Check your connection and try again.",
    }
}

fn install_verified(
    update: &Update,
    bytes: &[u8],
    digest: &str,
    verify_current: impl FnOnce() -> Result<(), &'static str>,
    install: impl FnOnce(&[u8]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    update.verify(bytes).map_err(download_error)?;
    if !matches_digest(bytes, digest) {
        return Err("The installer does not match the signed release manifest.");
    }
    verify_current()?;
    install(bytes)
}

#[cfg(target_os = "linux")]
fn install_package(app: &AppHandle, _update: &Update, bytes: &[u8]) -> Result<(), &'static str> {
    let path = app.env().appimage.ok_or(INSTALL_FAILED)?;
    if !bytes.starts_with(b"\x7fELF") {
        return Err(INSTALL_FAILED);
    }
    replace_appimage(Path::new(&path), |file| {
        std::io::Write::write_all(file, bytes)
    })
    .map_err(|_| INSTALL_FAILED)
}

#[cfg(target_os = "linux")]
fn replace_appimage(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::{
        fs::{self, File, Permissions},
        io::{Error, ErrorKind},
        os::{
            fd::AsRawFd,
            unix::fs::{MetadataExt, PermissionsExt},
        },
    };

    let name = path.file_name().ok_or(ErrorKind::InvalidInput)?;
    let directory = File::open(path.parent().ok_or(ErrorKind::InvalidInput)?)?;
    let parent = directory.metadata()?;
    let uid = rustix::process::geteuid().as_raw();
    if (parent.uid() != uid && parent.uid() != 0)
        || (parent.mode() & 0o022 != 0 && parent.mode() & 0o1000 == 0)
    {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "AppImage directory allows file substitution",
        ));
    }

    // Anchor both names to the opened directory even if an ancestor is renamed.
    let directory_path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let live = directory_path.join(name);
    let original = fs::symlink_metadata(&live)?;
    if !original.is_file() || (original.uid() != uid && original.uid() != 0) {
        return Err(ErrorKind::PermissionDenied.into());
    }
    let mut staged = tempfile::Builder::new()
        .prefix(".gamelink-update-")
        .permissions(Permissions::from_mode(0o700))
        .tempfile_in(&directory_path)?;
    write(staged.as_file_mut())?;
    staged
        .as_file()
        .set_permissions(Permissions::from_mode(original.mode() & 0o755))?;
    staged.as_file().sync_all()?;
    staged.persist(live).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(any(windows, test))]
fn launch_windows_installer<T, E>(
    launch: impl FnOnce() -> Result<T, E>,
    restore: impl FnOnce(),
) -> Result<T, E> {
    launch().inspect_err(|_| restore())
}

#[cfg(windows)]
fn install_package(app: &AppHandle, update: &Update, bytes: &[u8]) -> Result<(), &'static str> {
    launch_windows_installer(
        || update.install(bytes),
        || {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        },
    )
    .map_err(|_| INSTALL_FAILED)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn install_package(_app: &AppHandle, _update: &Update, _bytes: &[u8]) -> Result<(), &'static str> {
    Err(INSTALL_FAILED)
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
    version: String,
    progress: Channel<UpdateProgress>,
) -> Result<(), String> {
    let mut state = state
        .0
        .try_lock()
        .map_err(|_| "An update check or installation is already running. Try again shortly.")?;
    let prepared = state
        .as_ref()
        .filter(|prepared| prepared.update.version == version)
        .ok_or("The available update changed. Check again for the current version.")?;
    let verify_current = move || {
        ensure_install_allowed(&version, enabled_dir().as_deref(), |dir| {
            update::cached_notice(dir, VERSION)
        })
    };
    verify_current()?;

    let _ = progress.send(UpdateProgress::Downloading {
        downloaded: 0,
        total: None,
    });
    let mut downloaded = 0;
    let mut last_sent = None;
    let bytes = prepared
        .update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                if last_sent.is_none_or(|last: Instant| last.elapsed() >= DOWNLOAD_INTERVAL)
                    || total == Some(downloaded)
                {
                    let _ = progress.send(UpdateProgress::Downloading { downloaded, total });
                    last_sent = Some(Instant::now());
                }
            },
            || {
                let _ = progress.send(UpdateProgress::Verifying);
            },
        )
        .await
        .map_err(download_error)?;

    let update = prepared.update.clone();
    let digest = prepared.artifact_digest.clone();
    spawn_blocking(move || {
        install_verified(&update, &bytes, &digest, verify_current, |bytes| {
            let _ = progress.send(UpdateProgress::Installing);
            install_package(&app, &update, bytes)
        })
    })
    .await
    .map_err(|_| "Could not start the GameLink installer.")??;
    *state = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamelink_core::update::{Artifact, Severity};

    fn notice() -> CheckResult {
        CheckResult {
            installed_version: VERSION.into(),
            checked_at: "2026-11-17T00:00:00Z".into(),
            available: true,
            release: Manifest {
                schema_version: 1,
                version: "1.1.0".into(),
                release_date: "2026-11-17".into(),
                severity: Severity::Security,
                release_notes_url: "https://example.org/releases/next".into(),
                artifacts: vec![Artifact {
                    name: SUMMARY_ASSET.into(),
                    sha256: format!("{:x}", Sha256::digest(b"Bug fixes")),
                }],
            },
        }
    }

    #[test]
    fn failed_check_can_return_a_verified_notice_if_checks_are_still_enabled() {
        let dir = Path::new("/profile");
        let response = release_notice(
            Err("offline"),
            || Some(dir.into()),
            |path| {
                assert_eq!(path, dir);
                Some(notice())
            },
        );
        assert_eq!(response.unwrap().release.severity, Severity::Security);
        assert!(release_notice(Err("offline"), || None, |_| panic!("updates disabled")).is_none());
    }

    #[test]
    fn only_supported_installer_bundles_can_self_update() {
        assert_eq!(
            artifact_name("1.1.0", Some(BundleType::AppImage), true).as_deref(),
            Some("GameLink_1.1.0_amd64.AppImage")
        );
        assert!(artifact_name("1.1.0", Some(BundleType::AppImage), false).is_none());
        assert!(artifact_name("1.1.0", Some(BundleType::Deb), true).is_none());
        assert_eq!(
            artifact_name("1.1.0", Some(BundleType::Nsis), false).as_deref(),
            Some("GameLink_1.1.0_x64-setup.exe")
        );
    }

    #[test]
    fn installer_must_match_the_verified_version_and_the_published_asset_url() {
        let release = notice().release;
        let name = "GameLink_1.1.0_amd64.AppImage";
        let url = Url::parse(&format!("{RELEASE_DOWNLOAD}/v1.1.0/{name}")).unwrap();
        assert!(matching_installer(&release, name, "1.1.0", &url));
        assert!(!matching_installer(&release, name, "1.2.0", &url));
        assert!(!matching_installer(
            &release,
            name,
            "1.1.0",
            &"https://other.example/update".parse().unwrap()
        ));
    }

    #[test]
    fn summary_is_shown_only_if_its_digest_is_in_the_signed_manifest() {
        let release = notice().release;
        assert_eq!(
            verified_summary(&release, Some("Bug fixes")).as_deref(),
            Some("Bug fixes")
        );
        assert_eq!(verified_summary(&release, Some("Different notes")), None);
        assert_eq!(verified_summary(&release, None), None);
    }

    #[test]
    fn install_requires_checks_enabled_and_the_requested_version_still_verified() {
        let dir = Path::new("/profile");
        assert!(ensure_install_allowed("1.1.0", Some(dir), |_| Some(notice())).is_ok());
        assert!(ensure_install_allowed("1.1.1", Some(dir), |_| Some(notice())).is_err());
        assert!(ensure_install_allowed("1.1.0", Some(dir), |_| None).is_err());
        assert!(ensure_install_allowed("1.1.0", None, |_| panic!("updates disabled")).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_replacement_leaves_live_bytes_in_place_until_commit() {
        use std::{fs, io::Write, os::unix::fs::PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("GameLink.AppImage");
        fs::write(&live, b"old").unwrap();
        fs::set_permissions(&live, fs::Permissions::from_mode(0o751)).unwrap();

        let result = replace_appimage(&live, |staged| {
            assert_eq!(staged.metadata()?.permissions().mode() & 0o777, 0o700);
            staged.write_all(b"partial")?;
            assert_eq!(fs::read(&live)?, b"old");
            Err(std::io::ErrorKind::Interrupted.into())
        });
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

        replace_appimage(&live, |staged| {
            staged.write_all(b"new")?;
            assert_eq!(fs::read(&live)?, b"old");
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert_eq!(
            fs::metadata(live).unwrap().permissions().mode() & 0o777,
            0o751
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_replacement_rejects_a_directory_where_other_users_can_swap_the_stage() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("GameLink.AppImage");
        fs::write(&live, b"old").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
        let result = replace_appimage(&live, |_| panic!("would stage in a shared directory"));
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o1777)).unwrap();
        replace_appimage(&live, |_| Ok(())).unwrap();
    }

    #[test]
    fn windows_installer_launch_precedes_any_window_change() {
        let steps = std::cell::RefCell::new(Vec::new());
        let launch = |result| {
            launch_windows_installer(
                || {
                    assert!(steps.borrow().is_empty());
                    steps.borrow_mut().push("launch");
                    result
                },
                || steps.borrow_mut().push("restore"),
            )
        };
        assert_eq!(launch(Ok(())), Ok(()));
        assert_eq!(steps.replace(Vec::new()), ["launch"]);
        assert_eq!(launch(Err("launch rejected")), Err("launch rejected"));
        assert_eq!(steps.into_inner(), ["launch", "restore"]);
    }

    fn update_signed_by(
        signer: &minisign::KeyPair,
        pinned: &minisign::KeyPair,
        bytes: &[u8],
    ) -> Update {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let signature = minisign::sign(
            Some(&signer.pk),
            &signer.sk,
            bytes,
            Some("file:installer\tversion:1.1.0"),
            None,
        )
        .unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": STANDARD.encode(pinned.pk.to_box().unwrap().into_string()),
                "endpoints": ["https://example.org/latest.json"],
            }),
        );
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        installer_updater(app.handle())
            .unwrap()
            .executable_path("test/GameLink")
            .build()
            .unwrap()
            .restore_update(serde_json::json!({
                "version": "1.1.0",
                "url": "https://127.0.0.1:0/installer",
                "signature": STANDARD.encode(signature.into_string()),
            }))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn restored_update_can_build_its_download_client_without_tauris_feed_check() {
        let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let mut update = update_signed_by(&key, &key, b"installer");
        update.timeout = Some(Duration::from_secs(1));
        let result = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}));
        assert!(matches!(result, Err(Error::Reqwest(_))));
    }

    #[test]
    fn valid_tauri_signature_cannot_install_bytes_outside_the_signed_manifest() {
        let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = b"valid Tauri installer";
        let update = update_signed_by(&key, &key, bytes);
        assert!(update.verify(bytes).is_ok());
        let digest = format!("{:x}", Sha256::digest(b"different installer"));

        let result = install_verified(
            &update,
            bytes,
            &digest,
            || Ok(()),
            |_| panic!("installer reached the platform handoff"),
        );
        assert_eq!(
            result,
            Err("The installer does not match the signed release manifest.")
        );
    }

    #[test]
    fn manifest_matching_bytes_with_an_invalid_tauri_signature_cannot_install() {
        let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let other_key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = b"manifest-matching installer";
        let update = update_signed_by(&other_key, &key, bytes);
        let digest = format!("{:x}", Sha256::digest(bytes));

        let result = install_verified(
            &update,
            bytes,
            &digest,
            || Ok(()),
            |_| panic!("installer reached the platform handoff"),
        );
        assert_eq!(
            result,
            Err("The installer signature could not be verified.")
        );
    }
}
