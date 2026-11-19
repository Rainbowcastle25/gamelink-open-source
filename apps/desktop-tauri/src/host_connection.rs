//! Explicit desktop actions for connecting to and importing from a GameLink
//! host. This module deliberately runs outside the main app mutex so bounded
//! host requests cannot block the desktop's existing transport state updates.
use crate::{bridge, host};
use gamelink_core::storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};
use url::Url;

const FILE_NAME: &str = "host-connection.json";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

type Result<T> = std::result::Result<T, String>;

// The node's admin listener serves one request at a time and a Refresh can
// take up to 95 s. While this app runs one, People keeps polling status, so
// answer from the last status of that endpoint instead of timing out.
#[derive(Default)]
struct Endpoint {
    refreshing: usize,
    last: Option<Value>,
}
static ENDPOINTS: Mutex<BTreeMap<String, Endpoint>> = Mutex::new(BTreeMap::new());

fn endpoints() -> std::sync::MutexGuard<'static, BTreeMap<String, Endpoint>> {
    ENDPOINTS.lock().unwrap_or_else(|e| e.into_inner())
}

fn remember(endpoint: &str, result: &Result<Value>) {
    if let Ok(value) = result {
        endpoints().entry(endpoint.into()).or_default().last = Some(value.clone());
    }
}

fn status(saved: &SavedConnection) -> Result<Value> {
    let cached = endpoints()
        .get(&saved.endpoint)
        .filter(|e| e.refreshing > 0)
        .and_then(|e| e.last.clone());
    if let Some(value) = cached {
        return Ok(value);
    }
    let result = host_request(saved, "GET", "/v1/host/status", Value::Null, 5);
    remember(&saved.endpoint, &result);
    result
}

struct Refreshing<'a>(&'a str);
impl<'a> Refreshing<'a> {
    fn begin(endpoint: &'a str) -> Self {
        endpoints().entry(endpoint.into()).or_default().refreshing += 1;
        Self(endpoint)
    }
}
impl Drop for Refreshing<'_> {
    fn drop(&mut self) {
        if let Some(entry) = endpoints().get_mut(self.0) {
            entry.refreshing -= 1;
        }
    }
}

fn refresh(saved: &SavedConnection) -> Result<Value> {
    let _busy = Refreshing::begin(&saved.endpoint);
    let result = host_request(saved, "POST", "/v1/host/refresh", json!({}), 95);
    remember(&saved.endpoint, &result);
    result
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedConnection {
    endpoint: String,
    token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionView {
    endpoint: String,
    configured: bool,
}

pub fn call(request: Value) -> Result<Value> {
    match request["method"].as_str().unwrap_or("") {
        "hostConnection" => {
            let dir = config_dir()?;
            let saved = read_saved(&dir).unwrap_or_default();
            Ok(json!(ConnectionView {
                configured: !saved.endpoint.is_empty() && !saved.token.is_empty(),
                endpoint: saved.endpoint,
            }))
        }
        "configureHost" => {
            let dir = config_dir()?;
            let saved = configure(
                &dir,
                request["endpoint"].as_str().unwrap_or(""),
                request["token"].as_str().unwrap_or(""),
            )?;
            Ok(json!(ConnectionView {
                endpoint: saved.endpoint,
                configured: true
            }))
        }
        "hostStatus" => {
            let dir = config_dir()?;
            if !file_path(&dir)
                .try_exists()
                .map_err(|_| "could not read host connection")?
            {
                return Ok(json!({"games":[],"refreshedAt":""}));
            }
            status(&read_configured(&dir)?)
        }
        "refreshHost" => refresh(&read_configured(&config_dir()?)?),
        "approveLinkedRequest" | "dismissLinkedRequest" => {
            let path = if request["method"] == "approveLinkedRequest" {
                "/v1/host/requests/approve"
            } else {
                "/v1/host/requests/dismiss"
            };
            game_request(path, json!({
                "gameId":request["id"],"serverFingerprint":request["serverFingerprint"],
                "requestId":request["requestId"],"publicKey":request["publicKey"],"label":request["label"]
            }))
            .map(|_| Value::Null)
        }
        "importHostGame" => import_game(
            request["gameID"].as_str().unwrap_or(""),
            &request["serverFingerprint"],
        ),
        _ => Err("unknown host command".into()),
    }
}

fn config_dir() -> Result<PathBuf> {
    let paths = host::call(json!({"method":"paths"}))?;
    paths["configDir"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| "could not locate configuration folder".into())
}

fn file_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

fn read_saved(dir: &Path) -> Result<SavedConnection> {
    let bytes = storage::read_bounded(&file_path(dir))
        .map_err(|_| "could not read the saved host connection")?;
    let saved: SavedConnection =
        serde_json::from_slice(&bytes).map_err(|_| "saved host connection is invalid")?;
    let endpoint = validate_endpoint(&saved.endpoint)?;
    if endpoint != saved.endpoint || saved.token.is_empty() {
        return Err("saved host connection is invalid".into());
    }
    Ok(saved)
}

fn read_configured(dir: &Path) -> Result<SavedConnection> {
    read_saved(dir).map_err(|_| "configure a host connection first".into())
}

fn configure(dir: &Path, endpoint: &str, token: &str) -> Result<SavedConnection> {
    let endpoint = validate_endpoint(endpoint)?;
    let current = read_saved(dir).unwrap_or_default();
    let token = token.trim();
    let sealed = if token.is_empty() {
        if current.endpoint != endpoint || current.token.is_empty() {
            return Err("enter an administrator token for this host".into());
        }
        current.token
    } else {
        storage::seal(token).map_err(|_| "could not protect the administrator token")?
    };
    let saved = SavedConnection {
        endpoint,
        token: sealed,
    };
    let data = serde_json::to_vec(&saved).map_err(|_| "could not save the host connection")?;
    storage::write_atomic(&file_path(dir), &data)
        .map_err(|_| "could not save the host connection")?;
    Ok(saved)
}

fn validate_endpoint(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let mut url = Url::parse(raw)
        .map_err(|_| "enter a valid HTTPS host endpoint (or a loopback HTTP endpoint)")?;
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    if authority.contains('@')
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("enter a valid HTTPS host endpoint (or a loopback HTTP endpoint)".into());
    }
    match url.scheme() {
        "https" => (),
        "http" => {
            let loopback = match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            };
            if !loopback {
                return Err(
                    "enter a valid HTTPS host endpoint (or a loopback HTTP endpoint)".into(),
                );
            }
        }
        _ => return Err("enter a valid HTTPS host endpoint (or a loopback HTTP endpoint)".into()),
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

fn host_request(
    saved: &SavedConnection,
    verb: &str,
    path: &str,
    body: Value,
    timeout_seconds: u64,
) -> Result<Value> {
    let token =
        storage::unseal(&saved.token).map_err(|_| "could not unlock the saved host connection")?;
    let url = format!("{}{}", saved.endpoint, path);
    bridge::call(json!({
        "method":"http",
        "url":url,
        "verb":verb,
        "token":token,
        "body":body,
        "timeoutSeconds":timeout_seconds,
        "maxBytes":MAX_RESPONSE_BYTES
    }))
}

fn game_request(path: &str, body: Value) -> Result<Value> {
    let saved = read_configured(&config_dir()?)?;
    host_request(&saved, "POST", path, body, 15)
}

pub fn invitation(request: &Value) -> Result<String> {
    let result = game_request(
        "/v1/host/invitation",
        json!({
            "gameId":request["id"],"serverFingerprint":request["serverFingerprint"]
        }),
    )?;
    Ok(invitation_text(&result)?.to_owned())
}

fn invitation_text(result: &Value) -> Result<&str> {
    result["invitation"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= MAX_RESPONSE_BYTES)
        .ok_or_else(|| "host returned an invalid invitation".into())
}

fn import_game(game_id: &str, fingerprint: &Value) -> Result<Value> {
    if game_id.is_empty() || game_id.len() > 128 {
        return Err("select a game to import".into());
    }
    let identity = host::call(json!({"method":"identity"}))?;
    let public_key = identity["publicKey"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or("could not read this device's public key")?;
    let result = game_request(
        "/v1/host/import",
        json!({"gameId":game_id,"serverFingerprint":fingerprint,"publicKey":public_key}),
    )?;
    let invitation = invitation_text(&result)?;
    // Keep the invitation inside the Rust native host. Only the imported
    // ServerView crosses the UI boundary.
    host::call(json!({"method":"importInvite","text":invitation,"localPort":0}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_requires_https_except_literal_loopback_http() {
        for bad in [
            "http://example.com",
            "http://localhost:8467",
            "https://user:pass@example.com",
            "https://@example.com",
            "https://example.com/?token=secret",
        ] {
            assert!(validate_endpoint(bad).is_err(), "accepted {bad}");
        }
        for good in [
            "https://host.example",
            "http://127.0.0.1:8467",
            "http://[::1]:8467",
        ] {
            assert!(validate_endpoint(good).is_ok(), "rejected {good}");
        }
    }

    #[test]
    fn token_is_sealed_and_blank_only_keeps_same_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let first = configure(dir.path(), "https://host.example/", "admin-secret").unwrap();
        let bytes = std::fs::read(file_path(dir.path())).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("admin-secret"));
        let saved = read_saved(dir.path()).unwrap();
        assert_eq!(storage::unseal(&saved.token).unwrap(), "admin-secret");
        assert_eq!(
            configure(dir.path(), "https://host.example", "")
                .unwrap()
                .token,
            first.token
        );
        assert!(configure(dir.path(), "https://different.example", "").is_err());
    }

    #[test]
    fn status_during_a_refresh_uses_the_last_snapshot_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let saved = configure(dir.path(), "https://refreshing.example", "admin").unwrap();
        let fail = std::rc::Rc::new(std::cell::Cell::new(false));
        let failing = fail.clone();
        let _fake = bridge::fake::install(move |request| {
            assert_eq!(request["method"], "http");
            if failing.get() {
                Err("timed out".into())
            } else {
                Ok(json!({"games":[{"id":"cached"}],"refreshedAt":""}))
            }
        });
        let cached = status(&saved).unwrap();
        fail.set(true);
        assert!(status(&saved).is_err());
        {
            let _busy = Refreshing::begin(&saved.endpoint);
            assert_eq!(status(&saved).unwrap(), cached);
        }
        assert!(status(&saved).is_err());
        assert!(refresh(&saved).is_err());
        assert_eq!(endpoints()[&saved.endpoint].refreshing, 0);
    }
}
