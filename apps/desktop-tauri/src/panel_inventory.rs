//! Read-only access to the Panel selected in a legacy desktop profile.
use crate::{bridge, host_connection, profile};
use gamelink_core::storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::Path, time::Instant};

const FILE_NAME: &str = "panel-connection.json";
const INVALID_FILE: &str = "The saved Pterodactyl profile is invalid. Restore panel-connection.json from a backup in the configuration folder, then reload.";
const LOCKED_KEY: &str = "Could not unlock the saved Pterodactyl credential. Restore panel-connection.json from a backup for this device, then reload.";
const INVALID_RESPONSE: &str =
    "Pterodactyl returned invalid server inventory. Try refreshing again.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPanel {
    endpoint: String,
    node_ids: Vec<u64>,
    token: String,
}

impl SavedPanel {
    fn connection_view(&self) -> Value {
        json!({"endpoint": self.endpoint, "nodeIds": self.node_ids})
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelServer {
    id: u64,
    name: String,
    #[serde(rename(deserialize = "node", serialize = "nodeId"))]
    node_id: u64,
    suspended: bool,
}

pub fn call(request: Value) -> Result<Value, String> {
    call_in(request, &profile::config_dir()?)
}

pub(crate) fn call_in(request: Value, dir: &Path) -> Result<Value, String> {
    let Some(bytes) = profile::read_optional(dir, FILE_NAME)? else {
        return Ok(Value::Null);
    };
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return Err(INVALID_FILE.into());
    }
    let mut saved: SavedPanel = serde_json::from_slice(&bytes).map_err(|_| INVALID_FILE)?;
    if host_connection::validate_endpoint(&saved.endpoint).as_deref() != Ok(saved.endpoint.as_str())
        || !saved.endpoint.starts_with("https://")
        || !(1..=50).contains(&saved.node_ids.len())
        || saved.node_ids.contains(&0)
        || saved.token.is_empty()
    {
        return Err(INVALID_FILE.into());
    }
    saved.node_ids.sort_unstable();
    saved.node_ids.dedup();

    match request["method"].as_str() {
        Some("panelConnection") => Ok(saved.connection_view()),
        Some("refreshPanelInventory") => refresh(saved),
        _ => Err("unknown Panel command".into()),
    }
}

fn refresh(saved: SavedPanel) -> Result<Value, String> {
    let token = storage::unseal(&saved.token).map_err(|_| LOCKED_KEY)?;
    let start = Instant::now();
    let mut servers = Vec::new();
    for page in 1..=1000 {
        let remaining = 30u64.saturating_sub(start.elapsed().as_secs());
        if remaining == 0 {
            return Err("Pterodactyl inventory refresh timed out. Try refreshing again.".into());
        }
        // The Application key only needs Servers read access. Keep pagination on
        // the saved HTTPS endpoint instead of following a link from a response.
        let response = bridge::call(json!({
            "method": "http",
            "verb": "GET",
            "url": format!("{}/api/application/servers?per_page=25&page={page}", saved.endpoint),
            "token": token,
            "timeoutSeconds": remaining,
            "maxBytes": 2 * 1024 * 1024,
        }))
        .map_err(|error| match error.as_str() {
            "HTTP transport returned status 401" | "HTTP transport returned status 403" =>
                "Pterodactyl denied access. Check that the saved Application API key is valid and can read servers.",
            _ => "Could not load Pterodactyl inventory. Check that the saved Panel is reachable, then refresh again.",
        })?;

        let pagination = &response["meta"]["pagination"];
        let last_page = pagination["total_pages"].as_u64().ok_or(INVALID_RESPONSE)?;
        if pagination["current_page"].as_u64() != Some(page) || !(page..=1000).contains(&last_page)
        {
            return Err(INVALID_RESPONSE.into());
        }

        for entry in response["data"].as_array().ok_or(INVALID_RESPONSE)? {
            let server =
                PanelServer::deserialize(&entry["attributes"]).map_err(|_| INVALID_RESPONSE)?;
            if saved.node_ids.binary_search(&server.node_id).is_ok() {
                servers.push(server);
            }
        }
        if page == last_page {
            servers.sort_unstable_by_key(|server| (server.node_id, server.id));
            return Ok(json!({"connection": saved.connection_view(), "servers": servers}));
        }
    }
    Err(INVALID_RESPONSE.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_nodes_refresh_using_only_the_saved_panel_and_return_inventory_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let original = json!({
            "endpoint": "https://panel.example/panel",
            "nodeIds": [2, 4],
            "token": storage::seal("fixture-panel-token").unwrap(),
        });
        let bytes = serde_json::to_vec(&original).unwrap();
        storage::write_atomic(&path, &bytes).unwrap();

        let mut requested = [(1, 4), (2, 2), (1, 4), (2, 2)].into_iter();
        let _bridge = bridge::fake::install(move |request| {
            let (page, node) = requested.next().expect("unexpected Panel request");
            assert_eq!(request["verb"], "GET");
            assert_eq!(
                request["url"],
                format!(
                    "https://panel.example/panel/api/application/servers?per_page=25&page={page}"
                )
            );
            assert_eq!(request["token"], "fixture-panel-token");
            assert!(request["body"].is_null());
            Ok(json!({
                "data": [
                    {"attributes":{"id": node + 1, "name": "Game", "node": node, "suspended": page == 2, "container":{"environment":{"SECRET":"hidden"}}}},
                    {"attributes":{"id": 8, "name": "Unselected", "node": 7, "suspended": false}}
                ],
                "meta": {"pagination":{"current_page":page,"total_pages":2,"links":{"next":"https://other.example"}}}
            }))
        });
        for _ in 0..2 {
            let metadata = call_in(json!({"method":"panelConnection"}), dir.path()).unwrap();
            assert_eq!(
                metadata,
                json!({"endpoint":"https://panel.example/panel","nodeIds":[2,4]})
            );

            // An IPC caller cannot redirect the credential or supply unrelated node IDs.
            let request = json!({"method":"refreshPanelInventory","endpoint":"https://other.example","nodeIds":[12]});
            let result = call_in(request, dir.path()).unwrap();
            assert_eq!(
                result,
                json!({"connection":metadata,"servers":[
                    {"id":3,"name":"Game","nodeId":2,"suspended":true},
                    {"id":5,"name":"Game","nodeId":4,"suspended":false},
                ]})
            );
        }
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn bad_panel_profiles_make_no_requests_and_remain_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let _bridge = bridge::fake::install(|_| panic!("unexpected Panel request"));
        assert_eq!(
            call_in(json!({"method":"panelConnection"}), dir.path()).unwrap(),
            Value::Null
        );
        for bytes in [
            b"{malformed" as &[u8],
            br#"["https://panel.example",[2,4],"bad"]"#,
            br#"{"endpoint":"http://127.0.0.1","nodeIds":[2,4],"token":"bad"}"#,
            br#"{"endpoint":"https://panel.example","nodeIds":[2,"4"],"token":"bad"}"#,
        ] {
            storage::write_atomic(&path, bytes).unwrap();
            assert_eq!(
                call_in(json!({"method":"panelConnection"}), dir.path()).unwrap_err(),
                INVALID_FILE
            );
            assert_eq!(
                call_in(json!({"method":"refreshPanelInventory"}), dir.path()).unwrap_err(),
                INVALID_FILE
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let bad_key = br#"{"endpoint":"https://panel.example","nodeIds":[2,4],"token":"bad"}"#;
        storage::write_atomic(&path, bad_key).unwrap();
        assert_eq!(
            call_in(json!({"method":"refreshPanelInventory"}), dir.path()).unwrap_err(),
            LOCKED_KEY
        );
        assert_eq!(std::fs::read(&path).unwrap(), bad_key);
    }
}
