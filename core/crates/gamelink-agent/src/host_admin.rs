//! Opt-in administration inside the existing node agent. Panel requests are
//! exclusively user-triggered; the socket sleeps in accept between requests.
use crate::{
    bridge, config, invitations, json, now, pterodactyl, pure, requests, rows, s, self_update, Cli,
    Result, VERSION,
};
use gamelink_core::{
    requests::{server_fingerprint, Scope},
    update::valid_release_version,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

pub(crate) fn private_file(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path)
        .map_err(|_| "could not open private host configuration or credential file")?;
    let meta = file
        .metadata()
        .map_err(|_| "could not inspect private host file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(
                "host configuration and credential files require owner-only permissions (0600)"
                    .into(),
            );
        }
    }
    if !meta.is_file() || meta.len() > max as u64 {
        return Err("invalid or oversized private host file".into());
    }
    let mut data = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| "could not read private host file")?;
    if data.len() > max {
        return Err("oversized private host file".into());
    }
    Ok(data)
}
pub(crate) fn load(path: &Path) -> Result<(Value, String)> {
    let mut cfg: Value = serde_json::from_slice(&private_file(path, 65536)?)
        .map_err(|_| "invalid host configuration JSON")?;
    let read_token = |name: &str| -> Result<String> {
        if name.is_empty() {
            return Err("missing host credential file".into());
        }
        let file = Path::new(name);
        let resolved = if file.is_absolute() {
            file.to_owned()
        } else {
            path.parent().unwrap_or(Path::new(".")).join(file)
        };
        let token = String::from_utf8(private_file(&resolved, 8192)?)
            .map_err(|_| "invalid host credential file")?;
        let token = token.trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            return Err("invalid host credential file".into());
        }
        Ok(token.to_owned())
    };
    let admin = read_token(s(&cfg, "adminTokenFile"))?;
    if admin.len() < 32 {
        return Err("host admin token must have at least 32 characters".into());
    }
    cfg["applicationToken"] = json!(read_token(s(&cfg, "applicationTokenFile"))?);
    cfg["clientToken"] = if s(&cfg, "clientTokenFile").is_empty() {
        json!("")
    } else {
        json!(read_token(s(&cfg, "clientTokenFile"))?)
    };
    if admin == s(&cfg, "applicationToken") || admin == s(&cfg, "clientToken") {
        return Err("host admin token must differ from Panel credentials".into());
    }
    Ok((cfg, admin))
}

pub fn refresh(dir: &Path, path: &Path) -> Result<Value> {
    let (settings, _) = load(path)?;
    let cfg = config::dispatch("config.agent_load", json!({"path":dir.join("agent.json")}))?;
    let mut region = Value::Null;
    for game in rows(&cfg["games"]) {
        if let Ok(keys) = config::dispatch(
            "config.keys_load",
            json!({"id":game["id"],"path":dir.join("keys").join(format!("{}.json",s(game,"id")))}),
        ) {
            if keys["missing"] != true {
                region = keys["keys"]["region"].clone();
                break;
            }
        }
    }
    if region.is_null() {
        region = bridge::call(
            json!({"method":"resolve","regionID":settings["relayRegion"].as_i64().unwrap_or(0)}),
        )
        .map_err(|_| "could not resolve relay region for registration")?;
    }
    pterodactyl::refresh(
        dir,
        &settings,
        &bridge::call,
        &|| bridge::call(json!({"method":"generate"})),
        &region,
    )
}

pub struct Admin {
    stop: Arc<AtomicBool>,
    address: SocketAddr,
    worker: Option<JoinHandle<()>>,
}
impl Drop for Admin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
pub fn start(dir: &Path, path: &Path, address: &str) -> Result<Admin> {
    let address: SocketAddr = address
        .parse()
        .map_err(|_| "host administration requires a literal loopback address")?;
    if !address.ip().is_loopback() {
        return Err("host administration must bind to loopback; use an authenticated local forwarding session for remote access".into());
    }
    let (_, token) = load(path)?;
    let listener =
        TcpListener::bind(address).map_err(|_| "could not bind host administration port")?;
    let address = listener
        .local_addr()
        .map_err(|_| "could not inspect host listener")?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let dir = dir.to_owned();
    let path = path.to_owned();
    let worker = std::thread::spawn(move || {
        let mut state = Host {
            dir,
            path,
            token,
            checked_at: String::new(),
            statuses: json!([]),
        };
        for connection in listener.incoming() {
            if stopping.load(Ordering::Acquire) {
                break;
            }
            if let Ok(mut stream) = connection {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                let response = read_request(&mut stream).and_then(|request| state.handle(request));
                let (code, value) =
                    response.unwrap_or_else(|(code, message)| (code, json!({"error":message})));
                let _ = respond(&mut stream, code, &value);
            }
        }
    });
    Ok(Admin {
        stop,
        address,
        worker: Some(worker),
    })
}
struct Host {
    dir: PathBuf,
    path: PathBuf,
    token: String,
    checked_at: String,
    statuses: Value,
}
type ApiResult<T> = std::result::Result<T, (u16, String)>;
struct Request {
    method: String,
    path: String,
    authorization: String,
    origin: bool,
    body: Vec<u8>,
}
fn read_request(stream: &mut TcpStream) -> ApiResult<Request> {
    let invalid = || (400, "Invalid host request".to_owned());
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    (&mut reader)
        .take(8193)
        .read_until(b'\n', &mut line)
        .map_err(|_| invalid())?;
    let first = String::from_utf8(line.clone()).map_err(|_| invalid())?;
    let parts: Vec<_> = first.split_whitespace().collect();
    if parts.len() != 3 || parts[2] != "HTTP/1.1" {
        return Err(invalid());
    }
    let mut request = Request {
        method: parts[0].into(),
        path: parts[1].into(),
        authorization: String::new(),
        origin: false,
        body: vec![],
    };
    let mut total = line.len();
    let mut length = None;
    loop {
        line.clear();
        (&mut reader)
            .take(8193)
            .read_until(b'\n', &mut line)
            .map_err(|_| invalid())?;
        total += line.len();
        if total > 8192 || line.is_empty() {
            return Err(invalid());
        }
        if line == b"\r\n" {
            break;
        }
        let header = std::str::from_utf8(&line).map_err(|_| invalid())?;
        let (name, value) = header.split_once(':').ok_or_else(invalid)?;
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "authorization" => {
                if !request.authorization.is_empty() {
                    return Err(invalid());
                }
                request.authorization = value.into();
            }
            "origin" => request.origin = true,
            "content-length" => {
                if length.is_some() {
                    return Err(invalid());
                }
                length = Some(value.parse::<usize>().map_err(|_| invalid())?);
            }
            "transfer-encoding" => return Err(invalid()),
            _ => {}
        }
    }
    let length = length.unwrap_or(0);
    if length > 4096 {
        return Err((413, "Host request too large".into()));
    }
    request.body.resize(length, 0);
    reader
        .read_exact(&mut request.body)
        .map_err(|_| invalid())?;
    Ok(request)
}
fn equal_secret(left: &str, right: &str) -> bool {
    let mut difference = left.len() ^ right.len();
    for (index, byte) in right.bytes().enumerate() {
        difference |= usize::from(byte ^ left.as_bytes().get(index).copied().unwrap_or(0));
    }
    difference == 0
}
fn managed(game: &Value) -> bool {
    !s(game, "origin").is_empty()
}

// The full server key is enough to knock, so only its fingerprint leaves.
fn game_fingerprint(keys: &Value) -> Result<String> {
    let key = bridge::call(json!({"method":"server_public_key","keys":keys}))?;
    let key = pure::dispatch("identity.public_key", json!({"text":key}))?;
    Ok(server_fingerprint(
        key.as_str().ok_or("invalid server key")?,
    ))
}

const UNKNOWN_GAME: &str = "unknown or disabled managed game";
const CHANGED_GAME: &str = "the selected game identity has changed";
const NOT_PENDING: &str = "request is no longer pending for that device and game";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequest {
    #[serde(default, deserialize_with = "read_version")]
    version: Option<String>,
}

fn read_version<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(decoder).map(Some)
}

pub(super) fn request_update(
    body: &[u8],
    queue: impl FnOnce(Option<String>) -> std::result::Result<(), self_update::QueueError>,
) -> ApiResult<(u16, Value)> {
    let request = if body.is_empty() {
        UpdateRequest::default()
    } else {
        if body
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())
            != Some(b'{')
        {
            return Err((400, "Invalid agent update request".into()));
        }
        serde_json::from_slice::<UpdateRequest>(body)
            .map_err(|_| (400, "Invalid agent update request".into()))?
    };
    if request
        .version
        .as_deref()
        .is_some_and(|version| !valid_release_version(version))
    {
        return Err((400, "Invalid agent release version".into()));
    }
    queue(request.version).map_err(|error| match error {
        self_update::QueueError::Busy => {
            (409, "An agent update is already staged or running".into())
        }
        self_update::QueueError::Unavailable => (
            503,
            "Agent updates are unavailable on this installation".into(),
        ),
    })?;
    Ok((202, json!({"accepted":true})))
}

impl Host {
    fn handle(&mut self, request: Request) -> ApiResult<(u16, Value)> {
        if request.origin
            || !equal_secret(&request.authorization, &format!("Bearer {}", self.token))
        {
            return Err((401, "Host authentication failed".into()));
        }
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/v1/host/status") => {}
            ("POST", "/v2/host/update") => {
                return request_update(&request.body, self_update::queue)
            }
            ("POST", "/v1/host/refresh") => {
                let result=refresh(&self.dir,&self.path).map_err(|_|(502,"Refresh failed; check host settings and Panel connectivity. Previous registrations were retained.".into()))?;
                self.statuses = result["statuses"].clone();
                self.checked_at = now();
            }
            (
                "POST",
                "/v2/host/import"
                | "/v2/host/invitation"
                | "/v2/host/requests/approve"
                | "/v2/host/requests/dismiss",
            ) => {
                let body: Value = serde_json::from_slice(&request.body)
                    .map_err(|_| (400, "Invalid game request".into()))?;
                let result = match request.path.as_str() {
                    "/v2/host/import" => self.import(&body),
                    "/v2/host/invitation" => self.invitation(&body),
                    "/v2/host/requests/approve" => self.resolve_request(&body, true),
                    _ => self.resolve_request(&body, false),
                };
                // Only fixed selection errors are specific; others stay generic.
                return result.map(|value| (200, value)).map_err(|error| {
                    let message =
                        if [UNKNOWN_GAME, CHANGED_GAME, NOT_PENDING].contains(&error.as_str()) {
                            error
                        } else {
                            "Could not complete the action for the selected enabled game".into()
                        };
                    (400, message)
                });
            }
            (
                _,
                "/v1/host/status"
                | "/v1/host/refresh"
                | "/v2/host/import"
                | "/v2/host/update"
                | "/v2/host/invitation"
                | "/v2/host/requests/approve"
                | "/v2/host/requests/dismiss",
            ) => return Err((405, "Unsupported host request method".into())),
            _ => return Err((404, "Unknown host operation".into())),
        }
        self.snapshot()
            .map(|value| (200, value))
            .map_err(|_| (500, "Could not read host registrations".into()))
    }
    fn cli(&self) -> Cli {
        Cli {
            dir: self.dir.clone(),
        }
    }

    fn snapshot(&self) -> Result<Value> {
        let cfg = self.cli().load()?;
        let games = || rows(&cfg["games"]).iter().filter(|game| managed(game));
        let scopes = requests::scopes(&self.dir, games());
        let pending = requests::load(&self.dir)
            .map(|store| store.list(&scopes, chrono::Utc::now()))
            .unwrap_or_default();
        let games: Vec<_> = games().map(|game| {
            let id = s(game, "id");
            let status = rows(&self.statuses).iter().find(|v| v["gameId"] == game["id"]);
            let (state, detail) = if game["disabled"] == true {
                ("disabled", "Suspended, removed, or no longer mapped at the last refresh")
            } else if let Some(status) = status {
                (s(status, "state"), s(status, "detail"))
            } else {
                ("unknown", "Click Refresh to check the Panel")
            };
            let key = invitations::load_keys(&self.dir, id)
                .and_then(|keys| game_fingerprint(&keys)).map_or(Value::Null, Value::from);
            let pending: Vec<_> = pending.iter().filter(|request| request["game"] == id)
                .map(|request| json!({"id":request["id"],"publicKey":request["publicKey"],"lastSeen":request["lastSeen"]}))
                .collect();
            json!({
                "id":id,"name":game["name"],"type":game["type"],"proto":game["protocol"],"port":game["tunnelPort"],
                "disabled":game["disabled"]==true,"approvedDevices":rows(&game["devices"]).len(),
                "state":state,"detail":detail,"serverFingerprint":key,
                "canCopyInvitation":scopes.contains_key(id) && !key.is_null(),"pending":pending
            })
        }).collect();
        Ok(json!({
            "games":games,"refreshedAt":self.checked_at,
            "agentVersion":VERSION,"update":self_update::status()
        }))
    }

    fn selected_game(&self, body: &Value) -> Result<(Value, Value)> {
        let cfg = self.cli().load()?;
        let game = rows(&cfg["games"])
            .iter()
            .find(|game| game["id"] == body["gameId"] && managed(game) && game["disabled"] != true)
            .ok_or(UNKNOWN_GAME)?;
        let keys = invitations::load_keys(&self.dir, s(game, "id"))?;
        pure::check_invite_expiry(&keys["expiresAt"])?;
        Ok((game.clone(), keys))
    }

    fn check_selection(keys: &Value, body: &Value) -> Result<()> {
        if body["serverFingerprint"] != game_fingerprint(keys)? {
            return Err(CHANGED_GAME.into());
        }
        Ok(())
    }

    fn import(&self, body: &Value) -> Result<Value> {
        let (game, keys) = self.selected_game(body)?;
        Self::check_selection(&keys, body)?;
        let invitation = invitations::encode(&game, &keys, s(&game, "name"), "", false)?;
        self.cli().approve_device(
            s(&game, "id"),
            s(body, "publicKey"),
            "Host admin device",
            &keys,
        )?;
        Ok(json!({"invitation":invitation}))
    }

    fn invitation(&self, body: &Value) -> Result<Value> {
        let (game, keys) = self.selected_game(body)?;
        Self::check_selection(&keys, body)?;
        let invitation = invitations::encode(&game, &keys, s(&game, "name"), "", false)?;
        self.cli().mutate(
            "invite",
            json!({"id":game["id"],"expectedGame":game,"expectedKeys":keys,"keys":keys}),
        )?;
        Ok(json!({"invitation":invitation}))
    }

    fn resolve_request(&self, body: &Value, approve: bool) -> Result<Value> {
        let _lock = requests::lock(&self.dir)?;
        let (game, keys) = self.selected_game(body)?;
        Self::check_selection(&keys, body)?;
        let id = s(&game, "id");
        let scope = Scope::new(&game, keys);
        let scopes = [(id.to_owned(), scope)].into_iter().collect();
        let now = chrono::Utc::now();
        let mut store = requests::load(&self.dir)?;
        let request = store
            .list(&scopes, now)
            .into_iter()
            .find(|request| {
                request["id"] == body["requestId"] && request["publicKey"] == body["publicKey"]
            })
            .ok_or(NOT_PENDING)?;
        let request_id = request["id"].as_u64().ok_or("invalid request ID")?;
        if !approve {
            store.dismiss(id, request_id, now);
            requests::save(&store, &self.dir)?;
            return Ok(json!({}));
        }
        self.cli().approve_device(
            id,
            s(&request, "publicKey"),
            s(body, "label"),
            &scopes[id].keys,
        )?;
        store.remove(id, request_id);
        if requests::save(&store, &self.dir).is_err() {
            crate::log("The approval was saved, but the request store could not be updated.");
        }
        Ok(json!({}))
    }
}

fn respond(stream: &mut TcpStream, code: u16, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    write!(stream,"HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",bytes.len())?;
    stream.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamelink_core::storage;
    #[test]
    fn cached_status_and_auth_never_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.json");
        config::dispatch("config.agent_mutate", json!({"path":path,"action":"init"})).unwrap();
        let mut host = Host {
            dir: dir.path().to_owned(),
            path: dir.path().join("missing-host.json"),
            token: "test-token".repeat(8),
            checked_at: String::new(),
            statuses: json!([]),
        };
        let request = |method: &str, path: &str, token: &str| Request {
            method: method.into(),
            path: path.into(),
            authorization: format!("Bearer {token}"),
            origin: false,
            body: vec![],
        };
        assert_eq!(
            host.handle(request("POST", "/v1/host/refresh", "wrong"))
                .unwrap_err()
                .0,
            401
        );
        assert_eq!(
            host.handle(request("GET", "/v1/host/refresh", &host.token.clone()))
                .unwrap_err()
                .0,
            405
        );
        let (_, value) = host
            .handle(request("GET", "/v1/host/status", &host.token.clone()))
            .unwrap();
        assert_eq!(value["games"], json!([]));
        assert_eq!(value["refreshedAt"], "");
        assert_eq!(value["agentVersion"], VERSION);
        assert!(value["update"]["state"].is_string());
        let token = host.token.clone();
        let error = host
            .handle(request("POST", "/v1/host/refresh", &token))
            .unwrap_err();
        assert_eq!(error.0, 502);
        assert!(!error.1.contains(&token));
        assert_eq!(host.checked_at, "");
    }
    #[test]
    fn update_requires_bearer_and_rejects_browser_origins_and_extra_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let token = "test-token".repeat(8);
        let mut host = Host {
            dir: dir.path().to_owned(),
            path: dir.path().join("host.json"),
            token: token.clone(),
            checked_at: String::new(),
            statuses: json!([]),
        };
        let request = |path: &str, authorization, origin, body: &[u8]| Request {
            method: "POST".into(),
            path: path.into(),
            authorization,
            origin,
            body: body.to_vec(),
        };
        let path = "/v2/host/update";
        let bearer = || format!("Bearer {token}");
        assert_eq!(
            host.handle(request(path, String::new(), false, b"{}"))
                .unwrap_err()
                .0,
            401
        );
        assert_eq!(
            host.handle(request(path, bearer(), true, b"{}"))
                .unwrap_err()
                .0,
            401
        );
        for body in [
            br#"{"url":"https://example.com"}"#.as_slice(),
            br#"{"version":"9.0.0","path":"/tmp/agent"}"#,
            br#"{"command":"start"}"#,
            br#"{"version":null}"#,
            br#"{"version":"not-a-version"}"#,
            br#"{"version":"1.0.0","version":"2.0.0"}"#,
            br#"["1.0.0"]"#,
        ] {
            assert_eq!(
                host.handle(request(path, bearer(), false, body))
                    .unwrap_err()
                    .0,
                400
            );
        }
        assert_eq!(
            host.handle(request(
                "/v2/host/update?url=https://example.com",
                bearer(),
                false,
                b"{}"
            ))
            .unwrap_err()
            .0,
            404
        );
    }
    #[test]
    fn configuration_files_are_private_and_admin_binds_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("host.json");
        storage::write_atomic(&dir.path().join("app.token"), b"test-panel-token").unwrap();
        storage::write_atomic(&dir.path().join("admin.token"), "x".repeat(40).as_bytes()).unwrap();
        storage::write_atomic(
            &cfg,
            br#"{"applicationTokenFile":"app.token","adminTokenFile":"admin.token"}"#,
        )
        .unwrap();
        assert!(load(&cfg).is_ok());
        for address in [
            "0.0.0.0:8467",
            "192.168.1.1:8467",
            "localhost:8467",
            "[::]:8467",
        ] {
            assert!(start(dir.path(), &cfg, address).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&cfg, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load(&cfg).is_err());
        }
    }
}
