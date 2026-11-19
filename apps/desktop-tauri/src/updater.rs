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
use tauri_plugin_updater::{Error, Update, UpdaterExt};
use url::Url;

const RELEASE_DOWNLOAD: &str =
    "https://github.com/Rainbowcastle25/gamelink-open-source/releases/download";
const SUMMARY_ASSET: &str = "release-summary.txt";
const DOWNLOAD_INTERVAL: Duration = Duration::from_millis(100);
const INSTALLER_UNAVAILABLE: &str =
    "The installer is unavailable. Try again or open the release notes.";

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

async fn prepare_update(app: &AppHandle) -> tauri_plugin_updater::Result<Option<Update>> {
    app.updater_builder()
        .header("User-Agent", "GameLink")?
        .timeout(Duration::from_secs(8))
        .no_proxy()
        .configure_client(|client| client.https_only(true))
        .build()?
        .check()
        .await
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

#[tauri::command]
pub async fn install_update(
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
    let verify_current = || {
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

    if !matches_digest(&bytes, &prepared.artifact_digest) {
        return Err("The installer does not match the signed release manifest.".into());
    }
    verify_current()?;
    let _ = progress.send(UpdateProgress::Installing);
    let update = prepared.update.clone();
    spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|_| "Could not start the GameLink installer.")?
        .map_err(|_| "Could not install the update. Check installation permissions or open the release notes.")?;
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
}
