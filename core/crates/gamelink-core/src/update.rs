//! Native release metadata checks shared by the desktop and Linux agent.
use crate::storage;
use chrono::{DateTime, NaiveDate, Utc};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::{ErrorKind, Read},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

const RELEASES_URL: &str = "https://github.com/Rainbowcastle25/gamelink-open-source/releases";
pub const APT_MANUAL_REPAIR: &str =
    "On your server, run sudo dpkg --configure -a && sudo apt-get install -f, then sudo systemctl start gamelink-agent if needed.";
pub const MANIFEST_NAME: &str = "release-manifest.json";
pub const MANIFEST_SIGNATURE_NAME: &str = "release-manifest.json.minisig";
pub const MAX_MANIFEST: usize = 64 * 1024;
pub const MAX_SIGNATURE: usize = 4 * 1024;
pub const MAX_AGENT_BINARY: usize = 64 * 1024 * 1024;
const MAX_AGENT_VERSION: usize = 254; // Leave one byte in the tag component for the v prefix.
const MAX_REDIRECTS: usize = 5;
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const DESKTOP_CHECK_INTERVAL: u64 = 4 * 60 * 60;
const CACHE: &str = "release-check.json";

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Normal,
    Security,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub version: String,
    pub release_date: String,
    pub severity: Severity,
    pub release_notes_url: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub installed_version: String,
    pub checked_at: String,
    pub available: bool,
    pub release: Manifest,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    last_attempt: Option<u64>,
    last_attempt_succeeded: bool,
    signed: Option<SignedRelease>,
}

#[derive(Deserialize)]
struct CachedCheckpoint {
    signed: Option<SignedRelease>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedRelease {
    #[serde(default, deserialize_with = "read_checked_at")]
    checked_at: Option<u64>,
    // Keep the original bytes and detached signature so cache reads verify
    // again, including after a binary upgrade or key rotation.
    manifest: String,
    signature: String,
}

fn read_checked_at<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?.as_u64())
}

fn https_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| "invalid HTTPS URL")?;
    if value.chars().any(|c| c.is_whitespace() || c.is_control())
        || url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("invalid HTTPS URL");
    }
    Ok(url)
}

fn manifest_url() -> Result<Url> {
    #[cfg(debug_assertions)]
    match std::env::var("GAMELINK_UPDATE_MANIFEST_URL") {
        Ok(value) => {
            let url = https_url(&value)?;
            if url.query().is_some() || url.fragment().is_some() {
                return Err("manifest URL must not contain a query or fragment");
            }
            return Ok(url);
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(_) => return Err("invalid manifest URL"),
    }
    latest_manifest_url()
}

fn latest_manifest_url() -> Result<Url> {
    https_url(&format!("{RELEASES_URL}/latest/download/{MANIFEST_NAME}"))
}

fn verify_signature(bytes: &[u8], signature: &str, keys: &[PublicKey]) -> Result<()> {
    if signature.len() > MAX_SIGNATURE {
        return Err("release metadata exceeds size limit");
    }
    let signature = Signature::decode(signature).map_err(|_| "invalid release signature")?;
    if keys
        .iter()
        .any(|key| key.verify(bytes, &signature, false).is_ok())
    {
        Ok(())
    } else {
        Err("release signature verification failed")
    }
}

fn verify(signed: &SignedRelease, keys: &[PublicKey]) -> Result<(Manifest, Version)> {
    if signed.manifest.len() > MAX_MANIFEST {
        return Err("release metadata exceeds size limit");
    }
    verify_signature(signed.manifest.as_bytes(), &signed.signature, keys)?;
    let mut manifest: Manifest =
        serde_json::from_str(&signed.manifest).map_err(|_| "invalid release manifest")?;
    let version = Version::parse(&manifest.version).map_err(|_| "invalid release version")?;
    let date = NaiveDate::parse_from_str(&manifest.release_date, "%Y-%m-%d")
        .map_err(|_| "invalid release date")?;
    if date.format("%Y-%m-%d").to_string() != manifest.release_date
        || manifest.release_date.len() != 10
        || manifest.release_date.starts_with("0000")
    {
        return Err("invalid release date");
    }
    let mut names = BTreeSet::new();
    if manifest.schema_version != 1
        || manifest.artifacts.is_empty()
        || manifest.artifacts.iter().any(|artifact| {
            artifact.name.is_empty()
                || matches!(artifact.name.as_str(), "." | "..")
                || !artifact
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                || !names.insert(&artifact.name)
                || artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        })
    {
        return Err("invalid release manifest");
    }
    manifest.release_notes_url = https_url(&manifest.release_notes_url)?.into();
    Ok((manifest, version))
}

fn fetch(agent: &ureq::Agent, mut url: Url, timeout: Duration) -> Result<ureq::Body> {
    let start = Instant::now();
    for _ in 0..=MAX_REDIRECTS {
        let remaining = timeout
            .checked_sub(start.elapsed())
            .ok_or("release download timed out")?;
        let response = agent
            .get(url.as_str())
            .config()
            .timeout_global(Some(remaining))
            .build()
            .call()
            .map_err(|_| "release download failed")?;
        match response.status().as_u16() {
            200 => return Ok(response.into_body()),
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or("invalid release download redirect")?;
                let target = url
                    .join(location)
                    .map_err(|_| "invalid release download redirect")?;
                url = https_url(target.as_str())?;
                url.set_fragment(None);
            }
            _ => return Err("release download failed"),
        }
    }
    Err("too many release download redirects")
}

fn read_download(body: impl Read, limit: usize, too_large: &'static str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    body.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "could not read release download")?;
    if bytes.len() > limit {
        return Err(too_large);
    }
    Ok(bytes)
}

fn release_client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .proxy(None)
        .user_agent("GameLink")
        .build()
        .into()
}

/// Fetches the Tauri updater feed as untrusted metadata, with the same bound as the manifest.
pub fn fetch_updater_feed(url: Url) -> Result<serde_json::Value> {
    let body = fetch(&release_client(), url, FETCH_TIMEOUT)?;
    read_updater_feed(body.into_reader())
}

fn read_updater_feed(body: impl Read) -> Result<serde_json::Value> {
    let feed = read_metadata(body, MAX_MANIFEST)?;
    serde_json::from_str(&feed).map_err(|_| "invalid updater feed")
}

fn read_metadata(body: impl Read, limit: usize) -> Result<String> {
    let bytes = read_download(body, limit, "release metadata exceeds size limit")?;
    String::from_utf8(bytes).map_err(|_| "release metadata is not UTF-8")
}

fn fetch_metadata(agent: &ureq::Agent, url: Url, limit: usize) -> Result<String> {
    let body = fetch(agent, url, FETCH_TIMEOUT)?;
    if body
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err("release metadata exceeds size limit");
    }
    read_metadata(body.into_reader(), limit)
}

fn download_manifest(agent: &ureq::Agent, url: Url) -> Result<SignedRelease> {
    let signature_url = https_url(&format!("{url}.minisig"))?;
    let manifest = fetch_metadata(agent, url, MAX_MANIFEST)?;
    let signature = fetch_metadata(agent, signature_url, MAX_SIGNATURE)?;
    Ok(SignedRelease {
        checked_at: None,
        manifest,
        signature,
    })
}

fn download(now: u64) -> Result<SignedRelease> {
    let mut signed = download_manifest(&release_client(), manifest_url()?)?;
    signed.checked_at = Some(now);
    Ok(signed)
}

/// Reads the cache, recovering the checkpoint independently of attempt metadata.
fn load_cache(dir: &Path, debug: fn(&'static str)) -> Cache {
    match storage::read_bounded(&dir.join(CACHE)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(cache) => cache,
            Err(_) => {
                debug("ignoring invalid release check cache");
                let signed = serde_json::from_slice::<CachedCheckpoint>(&bytes)
                    .ok()
                    .and_then(|checkpoint| checkpoint.signed);
                Cache {
                    signed,
                    ..Cache::default()
                }
            }
        },
        Err(e) => {
            if e.kind() != ErrorKind::NotFound {
                debug("ignoring unreadable release check cache");
            }
            Cache::default()
        }
    }
}

fn save_cache(dir: &Path, cache: &Cache) -> Result<()> {
    let bytes = serde_json::to_vec(cache).map_err(|_| "invalid release check cache")?;
    storage::write_atomic(&dir.join(CACHE), &bytes)
        .map_err(|_| "could not save release check cache")
}

fn keys() -> Result<Vec<PublicKey>> {
    // A release builder can embed overlapping pins during planned rotation.
    let pins = option_env!("GAMELINK_UPDATE_PUBLIC_KEYS")
        .filter(|value| !value.trim().is_empty())
        .ok_or("this build can't check for updates: it has no release verification key")?;
    pins.split_whitespace()
        .map(|pin| PublicKey::from_base64(pin).map_err(|_| "invalid embedded release signing key"))
        .collect()
}

fn agent_target(arch: &str) -> Result<(&'static str, u16)> {
    match arch {
        "x86_64" => Ok(("gamelink-agent-linux-amd64", 62)),
        "aarch64" => Ok(("gamelink-agent-linux-arm64", 183)),
        _ => Err("unsupported Linux agent architecture"),
    }
}

pub fn agent_asset_name() -> Result<&'static str> {
    agent_target(std::env::consts::ARCH).map(|(name, _)| name)
}

pub fn valid_release_version(version: &str) -> bool {
    version.len() <= MAX_AGENT_VERSION
        && Version::parse(version).is_ok_and(|version| version.build.is_empty())
}

pub fn release_is_newer(release: &str, installed: &str) -> Option<bool> {
    let release = Version::parse(release).ok()?;
    let installed = Version::parse(installed).ok()?;
    Some(release.cmp_precedence(&installed).is_gt())
}

/// Signed metadata for an agent upgrade, tied to a version and architecture.
pub struct AgentRelease {
    signed: SignedRelease,
    keys: Vec<PublicKey>,
    artifact: Artifact,
    deb_sha256: Option<String>,
    version: Version,
    machine: u16,
}

impl AgentRelease {
    fn verify(
        signed: SignedRelease,
        keys: Vec<PublicKey>,
        installed: &str,
        arch: &str,
    ) -> Result<Self> {
        let (manifest, version) = verify(&signed, &keys)?;
        let installed = Version::parse(installed).map_err(|_| "invalid installed version")?;
        if !version.cmp_precedence(&installed).is_gt() {
            return Err("signed release is not newer than the installed agent");
        }
        if !version.build.is_empty() || manifest.version.len() > MAX_AGENT_VERSION {
            return Err("invalid agent release version");
        }
        let (name, machine) = agent_target(arch)?;
        let deb = match arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            _ => unreachable!("agent_target only accepts packaged architectures"),
        };
        let deb_name = format!("gamelink-agent_{version}_{deb}.deb");
        let deb_sha256 = manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.name == deb_name)
            .map(|artifact| artifact.sha256.clone());
        let artifact = manifest
            .artifacts
            .into_iter()
            .find(|artifact| artifact.name == name)
            .ok_or("signed release has no agent for this architecture")?;
        Ok(Self {
            signed,
            keys,
            artifact,
            deb_sha256,
            version,
            machine,
        })
    }

    /// The install flow always uses the built-in GitHub release feed.
    pub fn latest(installed: &str) -> Result<Self> {
        let pins = keys()?;
        let signed = download_manifest(&release_client(), latest_manifest_url()?)?;
        Self::verify(signed, pins, installed, std::env::consts::ARCH)
    }

    /// Reconstruct the verification context from staged bytes and embedded pins.
    pub fn from_staged(installed: &str, manifest: String, signature: String) -> Result<Self> {
        let signed = SignedRelease {
            manifest,
            signature,
            checked_at: None,
        };
        Self::verify(signed, keys()?, installed, std::env::consts::ARCH)
    }

    pub fn version(&self) -> String {
        self.version.to_string()
    }

    pub fn manifest_bytes(&self) -> &[u8] {
        self.signed.manifest.as_bytes()
    }

    pub fn manifest_signature(&self) -> &[u8] {
        self.signed.signature.as_bytes()
    }

    pub fn deb_sha256(&self) -> Result<&str> {
        self.deb_sha256
            .as_deref()
            .ok_or("signed release has no Debian package for this architecture")
    }

    pub fn verify_binary(&self, binary: &[u8], signature: &str) -> Result<()> {
        if binary.len() > MAX_AGENT_BINARY {
            return Err("agent binary exceeds size limit");
        }
        if !format!("{:x}", Sha256::digest(binary)).eq_ignore_ascii_case(&self.artifact.sha256) {
            return Err("agent binary does not match the signed SHA-256 digest");
        }
        verify_signature(binary, signature, &self.keys)?;
        // ELF64, little-endian Linux targets. Check e_machine as well as the
        // signed asset name so a mispackaged binary cannot reach the swap.
        if binary.len() < 64
            || !binary.starts_with(b"\x7fELF\x02\x01\x01")
            || u16::from_le_bytes([binary[18], binary[19]]) != self.machine
        {
            return Err("agent binary has the wrong Linux architecture");
        }
        Ok(())
    }

    pub fn download_binary(&self) -> Result<(Vec<u8>, String)> {
        let agent = release_client();
        // Use the verified version's immutable tag rather than the moving
        // latest alias. The asset name is selected from the native targets.
        let url = https_url(&format!(
            "{RELEASES_URL}/download/v{}/{}",
            self.version, self.artifact.name
        ))?;
        let signature =
            fetch_metadata(&agent, https_url(&format!("{url}.minisig"))?, MAX_SIGNATURE)?;
        let body = fetch(&agent, url, Duration::from_secs(120))?;
        if body
            .content_length()
            .is_some_and(|size| size > MAX_AGENT_BINARY as u64)
        {
            return Err("agent binary exceeds size limit");
        }
        let binary = read_download(
            body.into_reader(),
            MAX_AGENT_BINARY,
            "agent binary exceeds size limit",
        )?;
        Ok((binary, signature))
    }
}

fn result(installed: &Version, signed: &SignedRelease, keys: &[PublicKey]) -> Result<CheckResult> {
    let (release, version) = verify(signed, keys)?;
    let checked_at = checked_timestamp(signed.checked_at)?;
    Ok(checked_result(installed, release, version, checked_at))
}

fn checked_timestamp(checked_at: Option<u64>) -> Result<DateTime<Utc>> {
    checked_at
        .and_then(|time| i64::try_from(time).ok())
        .and_then(DateTime::<Utc>::from_timestamp_secs)
        .ok_or("invalid release check timestamp")
}

fn checked_result(
    installed: &Version,
    release: Manifest,
    version: Version,
    checked_at: DateTime<Utc>,
) -> CheckResult {
    CheckResult {
        installed_version: installed.to_string(),
        checked_at: checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        available: version.cmp_precedence(installed).is_gt(),
        release,
    }
}

fn due(last_attempt: Option<u64>, now: u64) -> bool {
    // A last attempt in the future came from a bad clock; do not wait for it.
    last_attempt.is_none_or(|last| last > now || now - last >= DESKTOP_CHECK_INTERVAL)
}

fn check_at(
    dir: &Path,
    installed: &Version,
    keys: &[PublicKey],
    automatic: bool,
    now: u64,
    debug: fn(&'static str),
    fetch: impl FnOnce(u64) -> Result<SignedRelease>,
) -> Result<Option<CheckResult>> {
    storage::ensure_dir(dir).map_err(|_| "could not create release check directory")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("release-check.lock"))
        .map_err(|_| "could not open release check lock")?;
    lock.lock()
        .map_err(|_| "could not lock release check cache")?;
    let mut cache = load_cache(dir, debug);
    let previous = match cache
        .signed
        .as_ref()
        .map(|old| verify(old, keys))
        .transpose()
    {
        Ok(previous) => previous,
        Err(error) => {
            debug(error);
            cache = Cache::default();
            None
        }
    };
    if automatic && !due(cache.last_attempt, now) {
        let Some(signed) = cache.signed.as_ref() else {
            return Ok(None);
        };
        match checked_timestamp(signed.checked_at) {
            Ok(checked_at) => {
                return Ok(previous.map(|(release, version)| {
                    checked_result(installed, release, version, checked_at)
                }));
            }
            Err(error) => debug(error),
        }
    }
    // Record attempts before touching the network so failures and restarts
    // count toward the same four-hour limit.
    cache.last_attempt = Some(now);
    cache.last_attempt_succeeded = false;
    save_cache(dir, &cache)?;

    let signed = fetch(now)?;
    let (release, version) = verify(&signed, keys)?;
    if let Some((_, previous)) = previous {
        match version.cmp_precedence(&previous) {
            std::cmp::Ordering::Less => {
                return Err("release manifest is older than the last verified release")
            }
            // Published manifests are immutable: the same version must keep
            // the same signed bytes.
            std::cmp::Ordering::Equal
                if cache
                    .signed
                    .as_ref()
                    .is_some_and(|old| old.manifest != signed.manifest) =>
            {
                return Err("release manifest changed after publication")
            }
            _ => {}
        }
    }
    let checked_at = checked_timestamp(signed.checked_at)?;
    let checked = checked_result(installed, release, version, checked_at);
    cache.signed = Some(signed);
    cache.last_attempt_succeeded = true;
    save_cache(dir, &cache)?;
    Ok(Some(checked))
}

/// Whether this build embeds a release verification key. Builds without one
/// cannot check for updates.
pub fn can_check() -> bool {
    keys().is_ok()
}

/// Desktop-only network entry point. Call after checking the user's setting.
/// `debug` receives non-fatal problems, such as an ignored corrupt cache.
pub fn check_desktop(
    dir: &Path,
    installed: &str,
    debug: fn(&'static str),
) -> Result<Option<CheckResult>> {
    check(dir, installed, true, debug)
}

/// Explicit agent network entry point.
pub fn check_now(dir: &Path, installed: &str) -> Result<CheckResult> {
    check(dir, installed, false, |_| {})?.ok_or("release check did not run")
}

fn check(
    dir: &Path,
    installed: &str,
    automatic: bool,
    debug: fn(&'static str),
) -> Result<Option<CheckResult>> {
    let installed = Version::parse(installed).map_err(|_| "invalid installed version")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "invalid system clock")?
        .as_secs();
    check_at(dir, &installed, &keys()?, automatic, now, debug, download)
}

/// The agent's last verified check. Reads local state only.
pub fn cached(dir: &Path, installed: &str) -> Option<CheckResult> {
    let signed = load_cache(dir, |_| {}).signed?;
    result(&Version::parse(installed).ok()?, &signed, &keys().ok()?).ok()
}

/// The desktop may open notes only for the currently verified notification.
pub fn cached_notice(dir: &Path, installed: &str) -> Option<CheckResult> {
    let cache = load_cache(dir, |_| {});
    let notice = result(
        &Version::parse(installed).ok()?,
        cache.signed.as_ref()?,
        &keys().ok()?,
    )
    .ok()?;
    notice.available.then_some(notice)
}

#[cfg(test)]
mod tests;
