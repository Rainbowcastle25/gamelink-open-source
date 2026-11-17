//! Explicit one-shot Pterodactyl inventory and registration.
//!
//! This module never schedules discovery. The caller supplies the HTTP shim,
//! and `refresh` performs all Application inventory requests before changing
//! configuration. Client API power states are informational snapshots only.
use gamelink_core::config;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::Path,
    time::{Duration, Instant},
};
use url::Url;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAGES: usize = 1000;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Profile {
    game_type: String,
    protocol: String,
    tunnel_port: u16,
}

#[derive(Clone)]
struct Server {
    uuid: String,
    identifier: String,
    name: String,
    node_id: u64,
    egg_id: u64,
    suspended: bool,
    address: IpAddr,
    port: u16,
}

/// Refreshes one panel/node inventory. `request` is the host's authenticated
/// HTTP shim; its errors are intentionally replaced with generic text so a
/// URL or credential from the host never reaches the caller.
pub fn refresh(
    dir: &Path,
    settings: &Value,
    request: &dyn Fn(Value) -> Result<Value, String>,
    generated: &dyn Fn() -> Result<Value, String>,
    region: &Value,
) -> Result<Value, String> {
    let base = parse_settings(settings)?;
    let deadline = Instant::now() + REFRESH_TIMEOUT;
    let inventory = inventory(&base, deadline, request)?;
    let origin = origin_id(&base.url, base.node_id);

    let path = dir.join("agent.json").to_string_lossy().into_owned();
    let existing = config::dispatch("config.agent_load", json!({"path":path}))?;
    let existing_ids: std::collections::BTreeSet<String> = existing["games"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| g["id"].as_str().map(str::to_owned))
        .collect();

    let mut games = Vec::with_capacity(inventory.len());
    let mut keys = Map::new();
    for server in &inventory {
        let profile = base
            .profiles
            .get(&server.egg_id)
            .ok_or("invalid inventory mapping")?;
        let id = stable_id(&server.uuid);
        let mut game = json!({
            "id":id,
            "name":server.name,
            "description":"",
            "type":profile.game_type,
            "protocol":profile.protocol,
            "tunnelPort":profile.tunnel_port,
            "dest":SocketAddr::new(server.address, server.port).to_string(),
            "devices":[],
            "created":"",
            "consoleBridge":null,
            "disabled":server.suspended
        });
        if !existing_ids.contains(&id) {
            if region.is_null()
                || region["RegionID"].as_i64().unwrap_or(0) == 0
                || region["Nodes"].as_array().is_none_or(Vec::is_empty)
            {
                return Err("a relay region is required to register new games".into());
            }
            let mut identity = generated().map_err(|_| "could not generate a game identity")?;
            if !identity.is_object() {
                return Err("generated game identity was invalid".into());
            }
            identity["region"] = region.clone();
            if identity["created"].as_str().is_none() {
                identity["created"] =
                    json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            }
            game["created"] = identity["created"].clone();
            keys.insert(id, identity);
        }
        games.push(game);
    }

    config::dispatch(
        "config.agent_mutate",
        json!({"path":path,"action":"sync_managed","origin":origin,"games":games,"keys":keys}),
    )?;

    let mut statuses = Vec::with_capacity(inventory.len());
    for server in &inventory {
        let mut status = json!({
            "gameId":stable_id(&server.uuid),
            "serverUuid":server.uuid,
            "nodeId":server.node_id,
            "name":server.name,
            "disabled":server.suspended,
            "state":"unknown"
        });
        let client_token = settings["clientToken"].as_str().unwrap_or("");
        if client_token.is_empty() {
            status["detail"] = json!("Client API token not configured");
        } else if Instant::now() >= deadline {
            status["detail"] = json!("Client API status unavailable: refresh time limit reached");
        } else {
            let (state, detail) = power_status(&base.url, client_token, server, deadline, request);
            status["state"] = json!(state);
            if !detail.is_empty() {
                status["detail"] = json!(detail);
            }
        }
        statuses.push(status);
    }

    Ok(json!({
        "origin":origin,
        "discovered":inventory.len(),
        "disabled":inventory.iter().filter(|s|s.suspended).count(),
        "statuses":statuses
    }))
}

struct Settings {
    url: Url,
    node_id: u64,
    node_address: Option<IpAddr>,
    application_token: String,
    profiles: BTreeMap<u64, Profile>,
}

fn parse_settings(v: &Value) -> Result<Settings, String> {
    let raw_url = v["panelUrl"].as_str().unwrap_or("").trim();
    let url = Url::parse(raw_url).map_err(|_| "Pterodactyl panel URL is invalid")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.scheme() != "https" && !loopback(&url))
    {
        return Err("Pterodactyl panel URL must use HTTPS except on loopback and contain no credentials or query".into());
    }
    let application_token = v["applicationToken"].as_str().unwrap_or("").trim();
    if application_token.is_empty() {
        return Err("Pterodactyl application token is required".into());
    }
    let node_id = v["nodeId"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("Pterodactyl node ID must be positive")?;
    let node_address = match v["nodeAddress"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(value) => Some(
            value
                .trim()
                .parse::<IpAddr>()
                .map_err(|_| "configured node address must be an IP address")?,
        ),
        None => None,
    };
    let raw_profiles = v["eggProfiles"]
        .as_object()
        .ok_or("Pterodactyl egg mappings are required")?;
    let mut profiles = BTreeMap::new();
    for (egg, row) in raw_profiles {
        let egg = egg
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("Pterodactyl egg IDs must be positive")?;
        let game_type = row["type"].as_str().unwrap_or("");
        let protocol = row["protocol"].as_str().unwrap_or("");
        let tunnel_port = row["tunnelPort"]
            .as_u64()
            .filter(|n| (1..=65535).contains(n))
            .ok_or("invalid mapped tunnel port")? as u16;
        let allowed = match game_type {
            "minecraft-java" => protocol == "tcp",
            "minecraft-bedrock" | "factorio" => protocol == "udp",
            "custom" => matches!(protocol, "tcp" | "udp"),
            _ => false,
        };
        if !allowed {
            return Err(format!(
                "Pterodactyl egg {egg} has an invalid GameLink profile"
            ));
        }
        profiles.insert(
            egg,
            Profile {
                game_type: game_type.to_owned(),
                protocol: protocol.to_owned(),
                tunnel_port,
            },
        );
    }
    Ok(Settings {
        url,
        node_id,
        node_address,
        application_token: application_token.to_owned(),
        profiles,
    })
}

fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

fn inventory(
    settings: &Settings,
    deadline: Instant,
    request: &dyn Fn(Value) -> Result<Value, String>,
) -> Result<Vec<Server>, String> {
    let mut out = Vec::new();
    walk_inventory(settings, deadline, request, false, &mut |rows| {
        for row in rows {
            parse_server(settings, row, &mut out)?;
        }
        Ok(())
    })?;
    Ok(out)
}

pub(crate) fn validate_settings(settings: &Value) -> Result<(), String> {
    parse_settings(settings).map(|_| ())
}

/// Shared pagination, deadline and response limits for refresh and setup.
pub(crate) fn application_inventory(
    settings: &Value,
    request: &dyn Fn(Value) -> Result<Value, String>,
    first_only: bool,
) -> Result<Vec<Value>, String> {
    let settings = parse_settings(settings)?;
    let mut out = Vec::new();
    walk_inventory(
        &settings,
        Instant::now() + REFRESH_TIMEOUT,
        request,
        first_only,
        &mut |rows| {
            // Project only the discovery fields. Never inspect container.environment.
            out.extend(rows.iter().map(|row| {
                let a = &row["attributes"];
                json!({"uuid":a["uuid"],"name":a["name"],"node":a["node"],"egg":a["egg"],
                "suspended":a["suspended"],"allocation":a["allocation"],
                "allocations":a["relationships"]["allocations"]["data"],
                "eggName":a["relationships"]["egg"]["attributes"]["name"],
                "startup":a["container"]["startup_command"]})
            }));
            Ok(())
        },
    )?;
    Ok(out)
}

fn walk_inventory(
    settings: &Settings,
    deadline: Instant,
    request: &dyn Fn(Value) -> Result<Value, String>,
    first_only: bool,
    visit: &mut dyn FnMut(&[Value]) -> Result<(), String>,
) -> Result<(), String> {
    for page in 1..=MAX_PAGES {
        let timeout = timeout_seconds(deadline).ok_or("Pterodactyl inventory timed out")?;
        let mut url = settings.url.clone();
        url.set_path(&format!(
            "{}/api/application/servers",
            settings.url.path().trim_end_matches('/')
        ));
        url.set_query(Some(&format!(
            "include=allocations,egg&per_page=100&page={page}"
        )));
        let value = request(json!({
            "method":"http",
            "url":url.as_str(),
            "verb":"GET",
            "token":settings.application_token,
            "body":null,
            "timeoutSeconds":timeout,
            "maxBytes":MAX_RESPONSE_BYTES
        }))
        .map_err(|_| "Pterodactyl inventory request failed")?;
        if Instant::now() >= deadline {
            return Err("Pterodactyl inventory timed out".into());
        }
        let rows = value["data"]
            .as_array()
            .ok_or("Pterodactyl inventory response was invalid")?;
        let current_page = value["meta"]["pagination"]["current_page"]
            .as_u64()
            .unwrap_or(0);
        let total_pages = value["meta"]["pagination"]["total_pages"]
            .as_u64()
            .unwrap_or(0);
        if current_page != page as u64
            || total_pages < current_page
            || total_pages == 0
            || total_pages > MAX_PAGES as u64
        {
            return Err("Pterodactyl inventory pagination was invalid".into());
        }
        visit(rows)?;
        if first_only || current_page == total_pages {
            return Ok(());
        }
    }
    Err("Pterodactyl inventory exceeded the page limit".into())
}

fn parse_server(settings: &Settings, row: &Value, out: &mut Vec<Server>) -> Result<(), String> {
    let attrs = &row["attributes"];
    let node_id = attrs["node"].as_u64().unwrap_or(0);
    let egg_id = attrs["egg"].as_u64().unwrap_or(0);
    if node_id != settings.node_id || !settings.profiles.contains_key(&egg_id) {
        return Ok(());
    }
    let uuid = attrs["uuid"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("selected Pterodactyl server had no valid UUID")?;
    let name = attrs["name"]
        .as_str()
        .ok_or("selected Pterodactyl server had no name")?;
    let identifier = attrs["identifier"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| uuid.get(..8))
        .ok_or("selected Pterodactyl server had no API identifier")?;
    if identifier.contains(['/', '?', '#']) {
        return Err("selected Pterodactyl server had an invalid API identifier".into());
    }
    let primary = attrs["allocation"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("selected Pterodactyl server had no primary allocation")?;
    let allocations = attrs["relationships"]["allocations"]["data"]
        .as_array()
        .ok_or("selected Pterodactyl server allocations were invalid")?;
    let allocation = allocations
        .iter()
        .find(|a| a["attributes"]["id"].as_u64() == Some(primary))
        .ok_or("selected Pterodactyl server primary allocation was missing")?;
    let allocation = &allocation["attributes"];
    let mut address = allocation["ip"]
        .as_str()
        .unwrap_or("")
        .trim()
        .parse::<IpAddr>()
        .ok();
    if address.is_some_and(|ip| ip.is_unspecified()) {
        address = settings.node_address;
        if address.is_none() {
            return Err("wildcard allocation requires a configured node address".into());
        }
    } else if address.is_none() {
        address = allocation["ip_alias"]
            .as_str()
            .and_then(|s| s.trim().parse::<IpAddr>().ok());
    }
    let address = address
        .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
        .ok_or("selected Pterodactyl allocation had no usable IP address")?;
    let port = allocation["port"]
        .as_u64()
        .filter(|n| (1..=65535).contains(n))
        .ok_or("selected Pterodactyl allocation had an invalid port")? as u16;
    out.push(Server {
        uuid: uuid.to_owned(),
        identifier: identifier.to_owned(),
        // Panel names allow 191 characters; GameLink display names allow 64.
        name: name.chars().take(64).collect(),
        node_id,
        egg_id,
        suspended: attrs["suspended"].as_bool().unwrap_or(false),
        address,
        port,
    });
    Ok(())
}

fn power_status(
    base: &Url,
    token: &str,
    server: &Server,
    deadline: Instant,
    request: &dyn Fn(Value) -> Result<Value, String>,
) -> (String, String) {
    let mut url = base.clone();
    url.set_path(&format!(
        "{}/api/client/servers/{}/resources",
        base.path().trim_end_matches('/'),
        server.identifier
    ));
    url.set_query(None);
    let Some(timeout) = timeout_seconds(deadline) else {
        return (
            "unknown".into(),
            "Client API status unavailable: refresh time limit reached".into(),
        );
    };
    let response = request(json!({
        "method":"http","url":url.as_str(),"verb":"GET","token":token,
        "body":null,"timeoutSeconds":timeout,"maxBytes":1048576
    }));
    let Ok(value) = response else {
        return ("unknown".into(), "Client API status unavailable".into());
    };
    if Instant::now() >= deadline {
        return (
            "unknown".into(),
            "Client API status unavailable: refresh time limit reached".into(),
        );
    }
    let Some(state) = value["attributes"]["current_state"].as_str() else {
        return ("unknown".into(), "Client API status unavailable".into());
    };
    let state = state.trim().to_ascii_lowercase();
    if state.is_empty()
        || state.len() > 32
        || !state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return ("unknown".into(), "Client API status unavailable".into());
    }
    (state, String::new())
}

pub(crate) fn stable_id(uuid: &str) -> String {
    let digest = Sha256::digest(uuid.as_bytes());
    format!(
        "ptero-{}",
        digest[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn origin_id(base: &Url, node_id: u64) -> String {
    let host = match base.host().expect("validated panel URL host") {
        url::Host::Domain(host) => host.to_ascii_lowercase(),
        url::Host::Ipv4(host) => host.to_string(),
        url::Host::Ipv6(host) => format!("[{host}]"),
    };
    let authority = base
        .port()
        .map_or(host.clone(), |port| format!("{host}:{port}"));
    let normalized = format!(
        "{}://{}{}|node={node_id}",
        base.scheme(),
        authority,
        base.path().trim_end_matches('/')
    );
    let digest = Sha256::digest(normalized.as_bytes());
    format!(
        "pterodactyl:{}",
        digest[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn timeout_seconds(deadline: Instant) -> Option<u64> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    if remaining.is_zero() {
        return None;
    }
    Some(remaining.as_secs().clamp(1, 10))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs};
    use tempfile::tempdir;

    fn settings(panel: &str) -> Value {
        json!({
            "panelUrl":panel,
            "applicationToken":"application-secret-token",
            "clientToken":"client-secret-token",
            "nodeId":7,
            "nodeAddress":"192.0.2.50",
            "eggProfiles":{"10":{"type":"minecraft-java","protocol":"tcp","tunnelPort":25565}}
        })
    }
    fn region() -> Value {
        json!({"RegionID":900,"Nodes":[{"Name":"900a","HostName":"derp.example"}]})
    }
    fn entry(
        uuid: &str,
        name: &str,
        node: u64,
        egg: u64,
        allocation: u64,
        ip: &str,
        port: u64,
    ) -> Value {
        json!({"attributes":{
            "uuid":uuid,"identifier":&uuid[..8],"name":name,"node":node,"egg":egg,
            "allocation":allocation,"suspended":false,
            "relationships":{"allocations":{"data":[
                {"attributes":{"id":allocation,"ip":ip,"port":port}},
                {"attributes":{"id":allocation+1,"ip":"192.0.2.99","port":25566}}
            ]}}
        }})
    }
    fn generated() -> Value {
        json!({"privateKey":"privkey:generated","presharedKey":"psk:generated"})
    }
    fn init(dir: &Path) {
        config::dispatch(
            "config.agent_mutate",
            json!({"path":dir.join("agent.json"),"action":"init"}),
        )
        .unwrap();
    }

    #[test]
    fn refresh_paginates_filters_node_and_egg_uses_primary_and_samples_status() {
        let dir = tempdir().unwrap();
        init(dir.path());
        let requests = Cell::new(0usize);
        let request = |input: Value| {
            assert_eq!(input["method"], "http");
            assert_eq!(input["verb"], "GET");
            assert!(input["timeoutSeconds"].as_u64().unwrap() > 0);
            let url = Url::parse(input["url"].as_str().unwrap()).unwrap();
            if url.path().contains("/api/client/") {
                assert_eq!(input["token"], "client-secret-token");
                return Ok(json!({"attributes":{"current_state":"running"}}));
            }
            assert_eq!(input["token"], "application-secret-token");
            assert_eq!(
                url.query_pairs().find(|(k, _)| k == "include").unwrap().1,
                "allocations,egg"
            );
            let page = url
                .query_pairs()
                .find(|(k, _)| k == "page")
                .unwrap()
                .1
                .parse::<u64>()
                .unwrap();
            requests.set(requests.get() + 1);
            if page == 1 {
                Ok(json!({"data":[
                    entry("wrong-node-uuid", "Wrong node", 8, 10, 3, "192.0.2.3", 25565),
                    entry("unmapped-egg-uuid", "Wrong egg", 7, 11, 4, "192.0.2.4", 25565)
                ],"meta":{"pagination":{"current_page":1,"total_pages":2}}}))
            } else {
                Ok(
                    json!({"data":[entry("mapped-server-uuid", "Mapped world", 7, 10, 5, "0.0.0.0", 25565)],
                    "meta":{"pagination":{"current_page":2,"total_pages":2}}}),
                )
            }
        };
        let generated_count = Cell::new(0);
        let gen = || {
            generated_count.set(generated_count.get() + 1);
            Ok(generated())
        };
        let result = refresh(
            dir.path(),
            &settings("http://127.0.0.1:8080/panel"),
            &request,
            &gen,
            &region(),
        )
        .unwrap();
        assert_eq!(requests.get(), 2);
        assert_eq!(generated_count.get(), 1);
        assert_eq!(result["discovered"], 1);
        assert_eq!(
            result
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["disabled", "discovered", "origin", "statuses"]
        );
        assert_eq!(result["statuses"][0]["state"], "running");
        assert_eq!(result["statuses"][0]["disabled"], false);
        let cfg = config::dispatch(
            "config.agent_load",
            json!({"path":dir.path().join("agent.json")}),
        )
        .unwrap();
        let game = &cfg["games"][0];
        assert_eq!(game["dest"], "192.0.2.50:25565");
        assert_eq!(game["type"], "minecraft-java");
        assert_eq!(game["protocol"], "tcp");
    }

    #[test]
    fn refresh_keeps_identity_when_name_and_primary_destination_change() {
        let dir = tempdir().unwrap();
        init(dir.path());
        let changed = Cell::new(false);
        let long_name = "界".repeat(191);
        let request = |input: Value| {
            let url = Url::parse(input["url"].as_str().unwrap()).unwrap();
            if url.path().contains("/api/client/") {
                return Ok(json!({"attributes":{"current_state":"stopped"}}));
            }
            let (name, ip, port) = if changed.get() {
                (long_name.as_str(), "192.0.2.22", 25570)
            } else {
                ("Original", "192.0.2.21", 25565)
            };
            Ok(
                json!({"data":[entry("stable-server-uuid", name, 7, 10, 8, ip, port)],"meta":{"pagination":{"current_page":1,"total_pages":1}}}),
            )
        };
        let generated_count = Cell::new(0);
        let gen = || {
            generated_count.set(generated_count.get() + 1);
            Ok(generated())
        };
        let settings = settings("http://127.0.0.1:8080/panel");
        refresh(dir.path(), &settings, &request, &gen, &region()).unwrap();
        let id = stable_id("stable-server-uuid");
        let keys_path = dir.path().join("keys").join(format!("{id}.json"));
        let first_keys = fs::read(&keys_path).unwrap();
        let first_cfg = config::dispatch(
            "config.agent_load",
            json!({"path":dir.path().join("agent.json")}),
        )
        .unwrap();
        let created = first_cfg["games"][0]["created"].clone();
        changed.set(true);
        let result = refresh(dir.path(), &settings, &request, &gen, &region()).unwrap();
        assert_eq!(result["statuses"][0]["state"], "stopped");
        assert_eq!(generated_count.get(), 1);
        assert_eq!(fs::read(&keys_path).unwrap(), first_keys);
        let cfg = config::dispatch(
            "config.agent_load",
            json!({"path":dir.path().join("agent.json")}),
        )
        .unwrap();
        assert_eq!(cfg["games"][0]["id"], id);
        assert_eq!(cfg["games"][0]["name"], "界".repeat(64));
        assert_eq!(cfg["games"][0]["dest"], "192.0.2.22:25570");
        assert_eq!(cfg["games"][0]["created"], created);
    }

    #[test]
    fn failed_inventory_preserves_configuration_and_redacts_request_error() {
        let dir = tempdir().unwrap();
        init(dir.path());
        let path = dir.path().join("agent.json");
        let before = config::dispatch("config.agent_load", json!({"path":path})).unwrap();
        let request = |_input: Value| Err("application-secret-token leaked by transport".into());
        let error = refresh(
            dir.path(),
            &settings("http://127.0.0.1:8080"),
            &request,
            &|| Ok(generated()),
            &region(),
        )
        .unwrap_err();
        assert!(!error.contains("application-secret-token"));
        assert_eq!(
            config::dispatch(
                "config.agent_load",
                json!({"path":dir.path().join("agent.json")})
            )
            .unwrap(),
            before
        );
    }

    #[test]
    fn panel_requires_tls_except_loopback() {
        assert!(parse_settings(&settings("http://example.com")).is_err());
        assert!(parse_settings(&settings("http://127.0.0.1:8080")).is_ok());
        assert!(parse_settings(&settings("http://[::1]:8080")).is_ok());
        assert!(parse_settings(&settings("http://localhost:8080")).is_err());
        assert!(parse_settings(&settings("https://user:pass@example.com")).is_err());
    }
}
