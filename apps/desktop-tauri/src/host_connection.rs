//! Explicit desktop actions for connecting to and importing from a GameLink
//! host. This module deliberately runs outside the main app mutex so bounded
//! host requests cannot block the desktop's existing transport state updates.
use crate::{bridge, host};
use gamelink_core::{
    requests::server_fingerprint,
    storage,
    update::{release_is_newer, valid_release_version},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

const FILE_NAME: &str = "host-connection.json";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const UPDATE_AGENT: &str =
    "Update the GameLink agent on this node to manage access from the desktop app.";
const MANUAL_AGENT_UPDATE: &str =
    "Update the GameLink agent on your server with the setup guide or your package manager.";
const AGENT_UPDATE_TIMEOUT: Duration = Duration::from_secs(4 * 60);
const AGENT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const AGENT_STATUS_TIMEOUT: u64 = 5;
const AGENT_TIMEOUT_MESSAGE: &str =
    "Timed out waiting for your server to finish updating. This app has not been updated.";

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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
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

pub enum LinkedAgent {
    Available(LinkedAgentUpdate),
    Manual,
    Unreachable,
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LinkedAgentNotice {
    Available { node_name: String },
    Manual,
    Unreachable,
}

impl LinkedAgent {
    pub fn notice(&self) -> LinkedAgentNotice {
        match self {
            Self::Available(update) => LinkedAgentNotice::Available {
                node_name: update.node_name.clone(),
            },
            Self::Manual => LinkedAgentNotice::Manual,
            Self::Unreachable => LinkedAgentNotice::Unreachable,
        }
    }
}

#[derive(Clone)]
pub struct LinkedAgentUpdate {
    connection: SavedConnection,
    node_name: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LinkedUpdateError {
    Manual { message: &'static str },
    Failed { message: String },
}

impl LinkedUpdateError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentUpdateProgress {
    Preparing,
    Installing,
    Restarting,
    CheckingHealth,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentStatus {
    agent_version: Option<String>,
    update: Option<AgentUpdateStatus>,
}

#[derive(Deserialize)]
struct AgentUpdateStatus {
    state: AgentUpdateState,
    supported: Option<bool>,
    version: Option<String>,
    reason: Option<String>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum AgentUpdateState {
    Idle,
    Checking,
    Downloading,
    Staged,
    StagingFailed,
    Installing,
    Succeeded,
    RolledBack,
    Failed,
    #[serde(other)]
    Unknown,
}

fn agent_status(saved: &SavedConnection, timeout: u64) -> Result<AgentStatus> {
    let value = host_request(saved, "GET", "/v1/host/status", Value::Null, timeout)?;
    serde_json::from_value(value)
        .map_err(|_| "The server returned an invalid agent update status.".into())
}

fn supports_update(status: &AgentStatus) -> bool {
    // The first agents with the update route report phases without the flag.
    status
        .update
        .as_ref()
        .is_some_and(|update| update.supported != Some(false))
}

pub fn linked_agent(version: &str) -> Result<Option<LinkedAgent>> {
    linked_agent_in(&config_dir()?, version)
}

fn linked_agent_in(dir: &Path, version: &str) -> Result<Option<LinkedAgent>> {
    if !file_path(dir)
        .try_exists()
        .map_err(|_| "could not read host connection")?
    {
        return Ok(None);
    }
    let connection = read_configured(dir)?;
    let status = agent_status(&connection, AGENT_STATUS_TIMEOUT)?;
    let newer = status
        .agent_version
        .as_deref()
        .and_then(|installed| release_is_newer(version, installed));
    if newer == Some(false) {
        return Ok(None);
    }
    if !supports_update(&status) {
        return Ok(Some(LinkedAgent::Manual));
    }
    if newer.is_none() {
        return Ok(Some(LinkedAgent::Unreachable));
    }
    let url = Url::parse(&connection.endpoint).map_err(|_| "invalid linked server endpoint")?;
    let node_name = url
        .host_str()
        .ok_or("invalid linked server hostname")?
        .to_owned();
    Ok(Some(LinkedAgent::Available(LinkedAgentUpdate {
        connection,
        node_name,
    })))
}

enum AgentPoll {
    Pending(AgentUpdateProgress),
    Succeeded,
}

impl AgentStatus {
    fn outcome(&self, version: &str) -> std::result::Result<AgentPoll, LinkedUpdateError> {
        use AgentUpdateProgress as Progress;
        use AgentUpdateState as Phase;

        let Some(update) = &self.update else {
            return Err(LinkedUpdateError::Manual {
                message: MANUAL_AGENT_UPDATE,
            });
        };
        let running_target = self.agent_version.as_deref() == Some(version);
        match update.state {
            Phase::Succeeded if running_target && update.version.as_deref() == Some(version) => {
                Ok(AgentPoll::Succeeded)
            }
            Phase::StagingFailed | Phase::RolledBack | Phase::Failed => {
                let message = if update.state == Phase::RolledBack {
                    "The server rolled back the agent update."
                } else {
                    "The server agent update failed."
                };
                let reason = update
                    .reason
                    .as_deref()
                    .filter(|reason| !reason.trim().is_empty());
                Err(LinkedUpdateError::failed(match reason {
                    Some(reason) => format!("{message} {reason}"),
                    None => message.to_owned(),
                }))
            }
            Phase::Installing if !running_target => Ok(AgentPoll::Pending(Progress::Installing)),
            Phase::Installing | Phase::Succeeded => {
                Ok(AgentPoll::Pending(Progress::CheckingHealth))
            }
            _ => Ok(AgentPoll::Pending(Progress::Preparing)),
        }
    }
}

fn request_agent_update(
    saved: &SavedConnection,
    version: &str,
) -> std::result::Result<(), LinkedUpdateError> {
    let response = host_request(
        saved,
        "POST",
        "/v2/host/update",
        json!({ "version": version }),
        AGENT_STATUS_TIMEOUT,
    );
    match response {
        Ok(response) if response["accepted"] == true => Ok(()),
        Err(error) if matches!(error.as_str(), "HTTP transport returned status 404" | "HTTP transport returned status 503") => {
            Err(LinkedUpdateError::Manual { message: MANUAL_AGENT_UPDATE })
        }
        Err(error) if error == "HTTP transport returned status 429" => {
            Err(LinkedUpdateError::failed("Try again in a few minutes."))
        }
        Err(error) if error == "HTTP transport returned status 409" => {
            Err(LinkedUpdateError::failed("An agent update is already running on your server. Try again in a few minutes."))
        }
        _ => Err(LinkedUpdateError::failed("Could not request the agent update on your server. Check the host connection and try again.")),
    }
}

fn restarting(error: &str) -> bool {
    matches!(
        error,
        "HTTP transport request failed"
            | "HTTP transport response exceeded its limit or could not be read"
            | "HTTP transport returned status 502"
            | "HTTP transport returned status 503"
            | "HTTP transport returned status 504"
    )
}

impl LinkedAgentUpdate {
    pub fn run(
        &self,
        version: &str,
        progress: impl FnMut(AgentUpdateProgress),
    ) -> std::result::Result<(), LinkedUpdateError> {
        let saved = config_dir().and_then(|dir| read_configured(&dir));
        if saved.as_ref() != Ok(&self.connection) {
            return Err(LinkedUpdateError::failed(
                "The linked server changed. Check for updates again.",
            ));
        }
        self.run_update(version, AGENT_UPDATE_TIMEOUT, AGENT_POLL_INTERVAL, progress)
    }

    fn run_update(
        &self,
        version: &str,
        timeout: Duration,
        poll_interval: Duration,
        mut progress: impl FnMut(AgentUpdateProgress),
    ) -> std::result::Result<(), LinkedUpdateError> {
        if !valid_release_version(version) {
            return Err(LinkedUpdateError::failed("Invalid agent release version."));
        }

        request_agent_update(&self.connection, version)?;
        let started = Instant::now();
        let mut last_progress = AgentUpdateProgress::Preparing;
        progress(last_progress);
        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            let request_timeout = (remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0))
                .min(AGENT_STATUS_TIMEOUT);
            let status = agent_status(&self.connection, request_timeout);
            if started.elapsed() >= timeout {
                break;
            }
            let phase = match status {
                Ok(status) => match status.outcome(version)? {
                    AgentPoll::Succeeded => return Ok(()),
                    AgentPoll::Pending(phase) => phase,
                },
                Err(error) if restarting(&error) => AgentUpdateProgress::Restarting,
                Err(_) => {
                    return Err(LinkedUpdateError::failed(
                        "Could not read the agent update status from your server.",
                    ))
                }
            };
            if phase != last_progress {
                progress(phase);
                last_progress = phase;
            }
            std::thread::sleep(poll_interval.min(timeout.saturating_sub(started.elapsed())));
        }
        Err(LinkedUpdateError::failed(AGENT_TIMEOUT_MESSAGE))
    }
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

    const TARGET: &str = "0.3.0";

    fn status_json(
        running: &str,
        phase: &str,
        version: Option<&str>,
        reason: Option<&str>,
    ) -> Value {
        json!({
            "agentVersion": running,
            "update": {"state":phase,"version":version,"reason":reason}
        })
    }

    #[test]
    fn finds_an_older_linked_agent_with_semver_and_reads_support() {
        let dir = tempfile::tempdir().unwrap();
        assert!(linked_agent_in(dir.path(), TARGET).unwrap().is_none());
        configure(dir.path(), "https://node.example/admin", "test-token").unwrap();
        let replies = [
            status_json("0.3.0-rc.1", "idle", None, None),
            status_json("0.3.0", "idle", None, None),
            status_json("0.10.0", "idle", None, None),
            json!({"agentVersion":"0.2.0","update":{"state":"idle","supported":false}}),
            json!({"games":[],"refreshedAt":""}),
        ];
        let mut replies = replies.into_iter();
        let _fake = bridge::fake::install(move |request| {
            assert_eq!(request["url"], "https://node.example/admin/v1/host/status");
            assert_eq!(request["verb"], "GET");
            assert_eq!(request["body"], Value::Null);
            Ok(replies.next().expect("unexpected status request"))
        });
        let agent = linked_agent_in(dir.path(), TARGET).unwrap().unwrap();
        assert_eq!(
            json!(agent.notice()),
            json!({"kind":"available","nodeName":"node.example"})
        );
        assert!(linked_agent_in(dir.path(), TARGET).unwrap().is_none());
        assert!(linked_agent_in(dir.path(), TARGET).unwrap().is_none());
        for _ in 0..2 {
            assert!(matches!(
                linked_agent_in(dir.path(), TARGET).unwrap(),
                Some(LinkedAgent::Manual)
            ));
        }
    }

    #[test]
    fn update_sends_only_the_expected_version_and_maps_unavailable_and_cooldown() {
        let dir = tempfile::tempdir().unwrap();
        let saved = configure(dir.path(), "https://node.example", "test-token").unwrap();
        for code in [404, 503, 429] {
            let _fake = bridge::fake::install(move |request| {
                assert_eq!(request["url"], "https://node.example/v2/host/update");
                assert_eq!(request["verb"], "POST");
                assert_eq!(request["token"], "test-token");
                assert_eq!(request["body"], json!({"version": TARGET}));
                Err(format!("HTTP transport returned status {code}"))
            });
            let error = request_agent_update(&saved, TARGET).unwrap_err();
            if code == 429 {
                assert_eq!(
                    json!(error),
                    json!({"kind":"failed","message":"Try again in a few minutes."})
                );
            } else {
                assert_eq!(
                    json!(error),
                    json!({"kind":"manual","message":MANUAL_AGENT_UPDATE})
                );
            }
        }
    }

    #[test]
    fn polls_through_restart_and_requires_success_for_the_exact_running_version() {
        let dir = tempfile::tempdir().unwrap();
        let connection = configure(dir.path(), "https://node.example", "test-token").unwrap();
        let agent = LinkedAgentUpdate {
            connection,
            node_name: "node.example".into(),
        };
        let responses = [
            Ok(json!({"accepted":true})),
            Ok(status_json("0.2.0", "checking", Some(TARGET), None)),
            Ok(status_json("0.2.0", "installing", Some(TARGET), None)),
            Err("HTTP transport request failed".into()),
            Err("HTTP transport returned status 502".into()),
            Ok(status_json(TARGET, "installing", Some(TARGET), None)),
            Ok(status_json(TARGET, "succeeded", Some("0.2.0"), None)),
            Ok(status_json("0.2.0", "succeeded", Some(TARGET), None)),
            Ok(status_json(TARGET, "succeeded", Some(TARGET), None)),
        ];
        let requests = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = requests.clone();
        let mut responses = responses.into_iter();
        let _fake = bridge::fake::install(move |request| {
            let (verb, path) = if calls.get() == 0 {
                ("POST", "v2/host/update")
            } else {
                ("GET", "v1/host/status")
            };
            assert_eq!(request["verb"], verb);
            assert_eq!(request["url"], format!("https://node.example/{path}"));
            calls.set(calls.get() + 1);
            responses.next().expect("unexpected update request")
        });
        let mut phases = Vec::new();
        agent
            .run_update(TARGET, Duration::from_secs(1), Duration::ZERO, |phase| {
                phases.push(phase)
            })
            .unwrap();
        assert_eq!(requests.get(), 9);
        assert_eq!(
            phases,
            [
                AgentUpdateProgress::Preparing,
                AgentUpdateProgress::Installing,
                AgentUpdateProgress::Restarting,
                AgentUpdateProgress::CheckingHealth,
            ]
        );
    }

    #[test]
    fn stops_on_a_terminal_failure_and_preserves_the_reason() {
        for (phase, message) in [
            ("stagingFailed", "The server agent update failed."),
            ("failed", "The server agent update failed."),
            ("rolledBack", "The server rolled back the agent update."),
        ] {
            let status: AgentStatus = serde_json::from_value(status_json(
                "0.2.0",
                phase,
                Some(TARGET),
                Some("The service failed its health check."),
            ))
            .unwrap();
            assert!(matches!(
                status.outcome(TARGET),
                Err(LinkedUpdateError::Failed { message: actual }) if actual == format!("{message} The service failed its health check.")
            ));
        }
    }

    #[test]
    fn polling_times_out_after_the_update_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let connection = configure(dir.path(), "https://node.example", "test-token").unwrap();
        let agent = LinkedAgentUpdate {
            connection,
            node_name: "node.example".into(),
        };
        let _fake = bridge::fake::install(|request| match request["verb"].as_str() {
            Some("POST") => Ok(json!({"accepted":true})),
            Some("GET") => Err("HTTP transport request failed".into()),
            _ => panic!("unexpected update request"),
        });
        let error = agent
            .run_update(
                TARGET,
                Duration::from_millis(10),
                Duration::from_millis(2),
                |_| {},
            )
            .unwrap_err();
        assert_eq!(json!(error)["message"], AGENT_TIMEOUT_MESSAGE);
    }

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
