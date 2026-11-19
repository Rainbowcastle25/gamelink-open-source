//! Explicit desktop actions for connecting to and importing from a GameLink
//! host. This module deliberately runs outside the main app mutex so bounded
//! host requests cannot block the desktop's existing transport state updates.
use crate::{bridge, host};
use gamelink_core::{requests::server_fingerprint, storage};
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
const UPDATE_AGENT: &str =
    "Update the GameLink agent on this node to manage access from the desktop app.";

type Result<T> = std::result::Result<T, String>;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostSnapshot {
    games: Vec<HostGame>,
    refreshed_at: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostGame {
    id: String,
    name: String,
    #[serde(rename = "type")]
    game_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    disabled: bool,
    approved_devices: usize,
    state: String,
    detail: String,
    server_fingerprint: Option<String>,
    #[serde(skip_deserializing)]
    legacy_agent: bool,
    #[serde(rename = "serverPublicKey", skip_serializing)]
    legacy_server_key: Option<String>,
    #[serde(default)]
    can_copy_invitation: bool,
    #[serde(default)]
    pending: Vec<PendingDevice>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingDevice {
    id: u64,
    public_key: String,
    last_seen: String,
}

fn project_snapshot(raw: Value) -> Result<Value> {
    // Current agents include the field even when a game has no readable keys.
    let legacy_agent = raw["games"].as_array().is_some_and(|games| {
        games
            .iter()
            .any(|game| game.get("serverFingerprint").is_none())
    });
    let mut snapshot: HostSnapshot =
        serde_json::from_value(raw).map_err(|_| "host returned an invalid status")?;
    for game in &mut snapshot.games {
        game.legacy_agent = legacy_agent;
        if game.server_fingerprint.is_none() {
            game.server_fingerprint = game.legacy_server_key.as_deref().map(server_fingerprint);
        }
    }
    serde_json::to_value(snapshot).map_err(|_| "could not encode host status".into())
}

// The node's admin listener serves one request at a time and a Refresh can
// take up to 95 s. While this app runs one, People keeps polling status, so
// answer from the last status of that endpoint or report that it is refreshing.
#[derive(Default)]
struct Endpoint {
    refreshing: usize,
    last: Option<Value>,
}
static ENDPOINTS: Mutex<BTreeMap<String, Endpoint>> = Mutex::new(BTreeMap::new());

fn endpoints() -> std::sync::MutexGuard<'static, BTreeMap<String, Endpoint>> {
    ENDPOINTS.lock().unwrap_or_else(|e| e.into_inner())
}

fn request_snapshot(
    saved: &SavedConnection,
    verb: &str,
    path: &str,
    body: Value,
    timeout_seconds: u64,
) -> Result<Value> {
    let value =
        host_request(saved, verb, path, body, timeout_seconds).and_then(project_snapshot)?;
    endpoints().entry(saved.endpoint.clone()).or_default().last = Some(value.clone());
    Ok(value)
}

fn status(saved: &SavedConnection) -> Result<Value> {
    if let Some(entry) = endpoints()
        .get(&saved.endpoint)
        .filter(|entry| entry.refreshing > 0)
    {
        return Ok(entry
            .last
            .clone()
            .unwrap_or_else(|| json!({"games":[],"refreshedAt":"","refreshing":true})));
    }
    request_snapshot(saved, "GET", "/v1/host/status", Value::Null, 5)
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
    request_snapshot(saved, "POST", "/v1/host/refresh", json!({}), 95)
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
    call_in(request, &config_dir()?)
}

fn call_in(request: Value, dir: &Path) -> Result<Value> {
    match request["method"].as_str().unwrap_or("") {
        "hostConnection" => {
            let saved = read_saved(dir).unwrap_or_default();
            Ok(json!(ConnectionView {
                configured: !saved.endpoint.is_empty() && !saved.token.is_empty(),
                endpoint: saved.endpoint,
            }))
        }
        "configureHost" => {
            let saved = configure(
                dir,
                request["endpoint"].as_str().unwrap_or(""),
                request["token"].as_str().unwrap_or(""),
            )?;
            Ok(json!(ConnectionView {
                endpoint: saved.endpoint,
                configured: true
            }))
        }
        "hostStatus" => {
            if !file_path(dir)
                .try_exists()
                .map_err(|_| "could not read host connection")?
            {
                return Ok(json!({"games":[],"refreshedAt":""}));
            }
            status(&read_configured(dir)?)
        }
        "refreshHost" => refresh(&read_configured(dir)?),
        "approveLinkedRequest" | "dismissLinkedRequest" => {
            let path = if request["method"] == "approveLinkedRequest" {
                "/v2/host/requests/approve"
            } else {
                "/v2/host/requests/dismiss"
            };
            game_request(&game_connection(dir)?, path, json!({
                "gameId":request["id"],"serverFingerprint":request["serverFingerprint"],
                "requestId":request["requestId"],"publicKey":request["publicKey"],"label":request["label"]
            }))
            .map(|_| Value::Null)
        }
        "importHostGame" => import_game(
            &game_connection(dir)?,
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

fn game_connection(dir: &Path) -> Result<SavedConnection> {
    let saved = read_configured(dir)?;
    let cache = endpoints();
    // A caller-supplied fingerprint does not establish agent compatibility.
    let games = cache
        .get(&saved.endpoint)
        .and_then(|entry| entry.last.as_ref())
        .and_then(|snapshot| snapshot["games"].as_array())
        .filter(|games| !games.is_empty())
        .ok_or("Refresh the linked games before managing access.")?;
    if games.iter().any(|game| game["legacyAgent"] == true) {
        return Err(UPDATE_AGENT.into());
    }
    Ok(saved)
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

fn game_request(saved: &SavedConnection, path: &str, body: Value) -> Result<Value> {
    host_request(saved, "POST", path, body, 15).map_err(|error| {
        if error == "HTTP transport returned status 404" {
            UPDATE_AGENT.into()
        } else {
            error
        }
    })
}

pub fn invitation(request: &Value) -> Result<String> {
    invitation_from(request, &config_dir()?)
}

fn invitation_from(request: &Value, dir: &Path) -> Result<String> {
    let result = game_request(
        &game_connection(dir)?,
        "/v2/host/invitation",
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

fn import_game(saved: &SavedConnection, game_id: &str, fingerprint: &Value) -> Result<Value> {
    if game_id.is_empty() || game_id.len() > 128 {
        return Err("select a game to import".into());
    }
    let identity = host::call(json!({"method":"identity"}))?;
    let public_key = identity["publicKey"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or("could not read this device's public key")?;
    let result = game_request(
        saved,
        "/v2/host/import",
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
    fn legacy_snapshots_are_projected_before_returning_and_caching() {
        let dir = tempfile::tempdir().unwrap();
        let server_key = format!("nodekey:{}", "ab".repeat(32));
        let fingerprint = gamelink_core::requests::server_fingerprint(&server_key);
        for (operation, path) in [
            (status as fn(&SavedConnection) -> Result<Value>, "status"),
            (refresh, "refresh"),
        ] {
            let saved = configure(
                dir.path(),
                &format!("https://legacy-{path}.example"),
                "admin",
            )
            .unwrap();
            let raw = json!({
                "refreshedAt":"2026-01-01T00:00:00Z",
                "extra":true,
                "games":[{
                    "id":"old-game","name":"Old Game","type":"factorio","proto":"udp","port":34197,
                    "disabled":false,"approvedDevices":2,"state":"running","detail":"",
                    "serverPublicKey":server_key,"canCopyInvitation":true,
                    "pending":[{"id":4,"publicKey":"device-key","lastSeen":"2026-01-01T00:00:00Z","extra":true}],
                    "extra":true
                }]
            });
            let url = format!("{}/v1/host/{path}", saved.endpoint);
            let _fake = bridge::fake::install(move |request| {
                assert_eq!(request["url"], url);
                Ok(raw.clone())
            });

            let output = operation(&saved).unwrap();
            assert!(
                output["games"][0].get("serverPublicKey").is_none(),
                "{path}"
            );
            assert_eq!(output["games"][0]["serverFingerprint"], fingerprint);
            assert_eq!(output["games"][0]["legacyAgent"], true);
            assert_eq!(output["games"][0]["pending"][0]["publicKey"], "device-key");
            assert!(output.get("extra").is_none());
            assert!(output["games"][0].get("extra").is_none());
            assert!(output["games"][0]["pending"][0].get("extra").is_none());
            assert_eq!(endpoints()[&saved.endpoint].last.as_ref(), Some(&output));
            let _busy = Refreshing::begin(&saved.endpoint);
            assert_eq!(status(&saved).unwrap(), output);
        }
    }

    #[test]
    fn legacy_snapshot_refuses_linked_actions_without_http() {
        let dir = tempfile::tempdir().unwrap();
        let saved = configure(dir.path(), "https://legacy-actions.example", "admin").unwrap();
        let server_key = format!("nodekey:{}", "cd".repeat(32));
        let snapshot = {
            let _fake = bridge::fake::install(move |request| {
                assert_eq!(
                    request["url"],
                    "https://legacy-actions.example/v1/host/status"
                );
                Ok(json!({"games":[{
                    "id":"legacy-game","name":"Legacy Game","type":"factorio","serverPublicKey":server_key,
                    "disabled":false,"approvedDevices":0,"state":"running","detail":"",
                    "canCopyInvitation":true
                }],"refreshedAt":""}))
            });
            status(&saved).unwrap()
        };
        let fingerprint = &snapshot["games"][0]["serverFingerprint"];

        let http_calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = http_calls.clone();
        let _fake = bridge::fake::install(move |_| {
            calls.set(calls.get() + 1);
            Err("unexpected bridge call".into())
        });
        let mut request = json!({
            "id":"legacy-game","gameID":"legacy-game","serverFingerprint":fingerprint,
            "requestId":1,"publicKey":"device-key","label":""
        });
        let mut results = vec![invitation_from(&request, dir.path()).map(Value::from)];
        for method in [
            "approveLinkedRequest",
            "dismissLinkedRequest",
            "importHostGame",
        ] {
            request["method"] = json!(method);
            results.push(call_in(request.clone(), dir.path()));
        }

        assert_eq!(
            results,
            vec![
                Err(
                    "Update the GameLink agent on this node to manage access from the desktop app."
                        .into()
                );
                4
            ]
        );
        assert_eq!(http_calls.get(), 0);
    }

    #[test]
    fn cached_current_snapshot_cannot_approve_after_agent_downgrade() {
        let dir = tempfile::tempdir().unwrap();
        let saved = configure(dir.path(), "https://downgraded.example", "admin").unwrap();
        let snapshot = {
            let _fake = bridge::fake::install(|request| {
                assert_eq!(request["url"], "https://downgraded.example/v1/host/refresh");
                Ok(json!({"games":[{
                    "id":"game","name":"Game","type":"factorio","serverFingerprint":"fingerprint",
                    "disabled":false,"approvedDevices":0,"state":"running","detail":""
                }],"refreshedAt":""}))
            });
            refresh(&saved).unwrap()
        };
        assert_eq!(snapshot["games"][0]["legacyAgent"], false);

        let approved = std::rc::Rc::new(std::cell::Cell::new(false));
        let approved_on_node = approved.clone();
        let _fake = bridge::fake::install(move |request| match request["url"].as_str() {
            Some("https://downgraded.example/v2/host/requests/approve") => {
                Err("HTTP transport returned status 404".into())
            }
            Some("https://downgraded.example/v1/host/requests/approve") => {
                approved_on_node.set(true);
                Ok(json!({}))
            }
            _ => panic!("unexpected host operation"),
        });
        let result = call_in(
            json!({"method":"approveLinkedRequest","id":"game","serverFingerprint":snapshot["games"][0]["serverFingerprint"],
                "requestId":1,"publicKey":"device-key","label":""}),
            dir.path(),
        );
        assert!(!approved.get(), "approval reached the legacy route");
        assert_eq!(result.unwrap_err(), UPDATE_AGENT);
    }

    #[test]
    fn status_during_a_refresh_with_no_snapshot_does_not_call_the_node() {
        let dir = tempfile::tempdir().unwrap();
        let saved = configure(dir.path(), "https://cold-refresh.example", "admin").unwrap();
        let _fake = bridge::fake::install(|_| panic!("status made an HTTP call during Refresh"));
        let _busy = Refreshing::begin(&saved.endpoint);
        assert_eq!(
            status(&saved).unwrap(),
            json!({"games":[],"refreshedAt":"","refreshing":true})
        );
        assert!(endpoints()[&saved.endpoint].last.is_none());
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
                Ok(json!({"games":[{
                    "id":"cached","name":"Cached Game","type":"factorio",
                    "disabled":false,"approvedDevices":0,"state":"running","detail":"","serverFingerprint":null
                }],"refreshedAt":""}))
            }
        });
        let cached = status(&saved).unwrap();
        assert_eq!(cached["games"][0]["legacyAgent"], false);
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
