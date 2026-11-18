//! Native release metadata checks shared by the desktop and Linux agent.
use crate::storage;
use chrono::{DateTime, NaiveDate, Utc};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::{ErrorKind, Read},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

const DEFAULT_URL: &str = "https://github.com/Rainbowcastle25/gamelink-open-source/releases/latest/download/release-manifest.json";
const MAX_MANIFEST: usize = 64 * 1024;
const MAX_SIGNATURE: usize = 4 * 1024;
const MAX_REDIRECTS: usize = 5;
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const DAY: u64 = 24 * 60 * 60;
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedRelease {
    checked_at: u64,
    // Keep the original bytes and detached signature so cache reads verify
    // again, including after a binary upgrade or key rotation.
    manifest: String,
    signature: String,
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
    let configured = match std::env::var("GAMELINK_UPDATE_MANIFEST_URL") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => DEFAULT_URL.to_owned(),
        Err(_) => return Err("invalid manifest URL"),
    };
    let url = https_url(&configured)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err("manifest URL must not contain a query or fragment");
    }
    Ok(url)
}

fn verify(signed: &SignedRelease, keys: &[PublicKey]) -> Result<(Manifest, Version)> {
    if signed.manifest.len() > MAX_MANIFEST || signed.signature.len() > MAX_SIGNATURE {
        return Err("release metadata exceeds size limit");
    }
    let signature =
        Signature::decode(&signed.signature).map_err(|_| "invalid release signature")?;
    if !keys.iter().any(|key| {
        key.verify(signed.manifest.as_bytes(), &signature, false)
            .is_ok()
    }) {
        return Err("release signature verification failed");
    }
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

fn fetch(agent: &ureq::Agent, mut url: Url, limit: usize) -> Result<String> {
    let start = Instant::now();
    for _ in 0..=MAX_REDIRECTS {
        let remaining = FETCH_TIMEOUT
            .checked_sub(start.elapsed())
            .ok_or("release metadata request timed out")?;
        let response = agent
            .get(url.as_str())
            .config()
            .timeout_global(Some(remaining))
            .build()
            .call()
            .map_err(|_| "release metadata request failed")?;
        match response.status().as_u16() {
            200 => return read_metadata(response.into_body().into_reader(), limit),
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or("invalid release metadata redirect")?;
                let target = url
                    .join(location)
                    .map_err(|_| "invalid release metadata redirect")?;
                url = https_url(target.as_str())?;
                url.set_fragment(None);
            }
            _ => return Err("release metadata request failed"),
        }
    }
    Err("too many release metadata redirects")
}

fn read_metadata(body: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    body.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "could not read release metadata")?;
    if bytes.len() > limit {
        return Err("release metadata exceeds size limit");
    }
    String::from_utf8(bytes).map_err(|_| "release metadata is not UTF-8")
}

fn download(now: u64) -> Result<SignedRelease> {
    let url = manifest_url()?;
    let signature_url = https_url(&format!("{url}.minisig"))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .proxy(None)
        .user_agent("GameLink")
        .build()
        .into();
    let manifest = fetch(&agent, url, MAX_MANIFEST)?;
    let signature = fetch(&agent, signature_url, MAX_SIGNATURE)?;
    Ok(SignedRelease {
        checked_at: now,
        manifest,
        signature,
    })
}

fn load_cache(dir: &Path) -> Result<Cache> {
    match storage::read_bounded(&dir.join(CACHE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "invalid release check cache"),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Cache::default()),
        Err(_) => Err("could not read release check cache"),
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
        .ok_or("this build has no release signing key")?;
    pins.split_whitespace()
        .map(|pin| PublicKey::from_base64(pin).map_err(|_| "invalid embedded release signing key"))
        .collect()
}

fn result(installed: &Version, signed: &SignedRelease, keys: &[PublicKey]) -> Result<CheckResult> {
    let (release, version) = verify(signed, keys)?;
    checked_result(installed, release, version, signed.checked_at)
}

fn checked_result(
    installed: &Version,
    release: Manifest,
    version: Version,
    checked_at: u64,
) -> Result<CheckResult> {
    let checked_at = i64::try_from(checked_at)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_secs)
        .ok_or("invalid release check timestamp")?;
    Ok(CheckResult {
        installed_version: installed.to_string(),
        checked_at: checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        available: version.cmp_precedence(installed).is_gt(),
        release,
    })
}

fn due(last_attempt: Option<u64>, now: u64) -> bool {
    last_attempt.is_none_or(|last| now.saturating_sub(last) >= DAY)
}

fn check_at(
    dir: &Path,
    installed: &Version,
    keys: &[PublicKey],
    daily: bool,
    now: u64,
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
    let mut cache = load_cache(dir)?;
    if daily && !due(cache.last_attempt, now) {
        return if cache.last_attempt_succeeded {
            cache
                .signed
                .as_ref()
                .map(|signed| result(installed, signed, keys))
                .transpose()
        } else {
            Ok(None)
        };
    }
    // Record attempts before touching the network so failures and restarts
    // count toward the same 24-hour limit.
    cache.last_attempt = Some(now);
    cache.last_attempt_succeeded = false;
    save_cache(dir, &cache)?;

    let signed = fetch(now)?;
    let (release, version) = verify(&signed, keys)?;
    let previous = cache.signed.as_ref().and_then(|old| verify(old, keys).ok());
    if previous.is_some_and(|(_, previous)| version.cmp_precedence(&previous).is_lt()) {
        return Err("release manifest is older than the last verified release");
    }
    let checked = checked_result(installed, release, version, signed.checked_at)?;
    cache.signed = Some(signed);
    cache.last_attempt_succeeded = true;
    save_cache(dir, &cache)?;
    Ok(Some(checked))
}

/// Desktop-only network entry point. Call after checking the user's setting.
pub fn check_daily(dir: &Path, installed: &str) -> Result<Option<CheckResult>> {
    check(dir, installed, true)
}

/// Explicit agent network entry point.
pub fn check_now(dir: &Path, installed: &str) -> Result<CheckResult> {
    check(dir, installed, false)?.ok_or("release check did not run")
}

fn check(dir: &Path, installed: &str, daily: bool) -> Result<Option<CheckResult>> {
    let installed = Version::parse(installed).map_err(|_| "invalid installed version")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "invalid system clock")?
        .as_secs();
    check_at(dir, &installed, &keys()?, daily, now, download)
}

/// The agent's last verified check. Reads local state only.
pub fn cached(dir: &Path, installed: &str) -> Option<CheckResult> {
    let signed = load_cache(dir).ok()?.signed?;
    result(&Version::parse(installed).ok()?, &signed, &keys().ok()?).ok()
}

/// The desktop may open notes only for the currently verified notification.
pub fn cached_notice(dir: &Path, installed: &str) -> Option<CheckResult> {
    let cache = load_cache(dir).ok()?;
    if !cache.last_attempt_succeeded {
        return None;
    }
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
