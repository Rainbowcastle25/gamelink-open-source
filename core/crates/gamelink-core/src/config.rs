//! Configuration policy and mutations; hosts supply generated Tailcat keys and timestamps.
use crate::{pure, storage};
use serde_json::{json, Value};
use std::{
    fs::{self, TryLockError},
    path::Path,
    time::Duration,
};
fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_owned()
}
fn check(method: &str, v: Value) -> Result<Value, String> {
    pure::dispatch(method, v)
}
fn display(field: &str, v: &str, max: usize, required: bool) -> Result<(), String> {
    check(
        "security.display",
        json!({"field":field,"text":v,"maxRunes":max,"required":required}),
    )?;
    Ok(())
}
fn protocol(game: &str, proto: &str) -> Result<(), String> {
    check("games.check", json!({"game":game,"proto":proto}))?;
    Ok(())
}
fn port(v: &Value, optional: bool) -> Result<(), String> {
    check("security.port", json!({"port":v,"optional":optional}))?;
    Ok(())
}
fn fields(v: &Value) -> Result<(), String> {
    display("name", &text(v, "name"), 64, true)?;
    display("description", &text(v, "description"), 200, false)?;
    protocol(&text(v, "game"), &text(v, "proto"))?;
    port(&v["port"], false)?;
    port(&v["localPort"], true)
}
pub fn bridge(v: &Value) -> Result<(), String> {
    if v.is_null() {
        return Ok(());
    }
    for (k, msg) in [
        (
            "serverIp",
            "console bridge server IP must be a unicast IPv4 address",
        ),
        (
            "bindIp",
            "console bridge bind IP must be a local unicast IPv4 address",
        ),
    ] {
        let ip = text(v, k).parse::<std::net::Ipv4Addr>().map_err(|_| msg)?;
        if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
            return Err(msg.into());
        }
    }
    let first = v["mediaPortFirst"].as_u64().unwrap_or(0);
    let last = v["mediaPortLast"].as_u64().unwrap_or(0);
    if first == 0 || last < first || last - first >= 256 {
        return Err("console bridge requires a valid nonempty media port range".into());
    }
    Ok(())
}
pub fn bedrock(v: &Value) -> Result<(), String> {
    if v.is_null() {
        return Ok(());
    }
    let first = v["mediaPortFirst"].as_u64().unwrap_or(0);
    let last = v["mediaPortLast"].as_u64().unwrap_or(0);
    if first == 0 || last < first || last - first >= 256 || last > 65535 {
        return Err("Bedrock requires a valid nonempty media port range".into());
    }
    Ok(())
}
fn default_client() -> Value {
    json!({"version":1,"servers":null,"settings":{"demoMode":false,"autoReconnect":true,"checkOnStartup":true,"checkForUpdates":false,"verboseLogging":false,"allowLocalRelay":false}})
}
// serde_json::Value sorts object keys and collapses exact duplicates. Retain
// every member until typed decoding, as encoding/json processes them in wire order.
enum OrderedJson {
    Object(Vec<(String, OrderedJson)>),
    Array(Vec<OrderedJson>),
    Scalar(Value),
}
impl<'de> serde::Deserialize<'de> for OrderedJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = OrderedJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON value")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut m: M,
            ) -> Result<Self::Value, M::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = m.next_entry()? {
                    entries.push(entry);
                }
                Ok(OrderedJson::Object(entries))
            }
            fn visit_seq<S: serde::de::SeqAccess<'de>>(
                self,
                mut s: S,
            ) -> Result<Self::Value, S::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = s.next_element()? {
                    entries.push(entry);
                }
                Ok(OrderedJson::Array(entries))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(OrderedJson::Scalar(Value::Null))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(OrderedJson::Scalar(json!(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(OrderedJson::Scalar(json!(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(OrderedJson::Scalar(json!(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(OrderedJson::Scalar(json!(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OrderedJson::Scalar(json!(v)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
fn canonical_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "agent" => &["version", "games"],
        "client" => &["version", "servers", "settings"],
        "game" => &[
            "id",
            "name",
            "description",
            "type",
            "protocol",
            "tunnelPort",
            "dest",
            "devices",
            "created",
            "consoleBridge",
            "bedrock",
            "knockEnabled",
            "disabled",
            "origin",
        ],
        "server" => &[
            "id",
            "name",
            "description",
            "game",
            "proto",
            "port",
            "localPort",
            "stablePort",
            "addr",
            "addedAt",
            "expiresAt",
        ],
        "device" => &["publicKey", "label", "approvedAt"],
        "bridge" => &[
            "serverIp",
            "bindIp",
            "discoveryPort",
            "mediaPortFirst",
            "mediaPortLast",
        ],
        "bedrock" => &["mediaPortFirst", "mediaPortLast"],
        "settings" => &[
            "demoMode",
            "autoReconnect",
            "checkOnStartup",
            "checkForUpdates",
            "verboseLogging",
            "allowLocalRelay",
            "gameExecutables",
            "gameLaunchArgs",
            "minecraftServerList",
            "prismInstancesDir",
            "presenceNotifications",
        ],
        _ => &[],
    }
}
fn field_matches(field: &str, key: &str) -> bool {
    // Go's foldName uses Unicode simple folding, including long s and Kelvin K.
    field.eq_ignore_ascii_case(&key.replace('ſ', "s").replace('K', "k"))
}
fn canonicalize(raw: OrderedJson, kind: &str, previous: Value) -> Result<Value, String> {
    let OrderedJson::Object(entries) = raw else {
        return match raw {
            OrderedJson::Scalar(v) if v.is_null() => Ok(previous),
            _ => Err("invalid configuration field type".into()),
        };
    };
    let mut out = previous.as_object().cloned().unwrap_or_default();
    for (key, raw) in entries {
        let Some(&field) = canonical_fields(kind)
            .iter()
            .find(|f| field_matches(f, &key))
        else {
            continue;
        };
        let sub = match field {
            "games" => "game",
            "servers" => "server",
            "devices" => "device",
            "settings" => "settings",
            "consoleBridge" => "bridge",
            "bedrock" => "bedrock",
            _ => "",
        };
        let old = out.get(field).cloned().unwrap_or(Value::Null);
        let is_null = matches!(&raw, OrderedJson::Scalar(v) if v.is_null());
        let value = if is_null {
            // null leaves scalar/struct values alone, but clears slices, maps,
            // and the console bridge pointer.
            if matches!(
                field,
                "games"
                    | "servers"
                    | "devices"
                    | "gameExecutables"
                    | "gameLaunchArgs"
                    | "consoleBridge"
                    | "bedrock"
            ) {
                Value::Null
            } else {
                old
            }
        } else if matches!(field, "games" | "servers" | "devices") {
            let OrderedJson::Array(items) = raw else {
                return Err("invalid configuration field type".into());
            };
            Value::Array(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| {
                        canonicalize(v, sub, old.get(i).cloned().unwrap_or_else(|| json!({})))
                    })
                    .collect::<Result<_, _>>()?,
            )
        } else if !sub.is_empty() {
            canonicalize(raw, sub, old)?
        } else if matches!(field, "gameExecutables" | "gameLaunchArgs") {
            let OrderedJson::Object(entries) = raw else {
                return Err("invalid configuration field type".into());
            };
            let mut map = old.as_object().cloned().unwrap_or_default();
            for (k, v) in entries {
                let OrderedJson::Scalar(v) = v else {
                    return Err("invalid configuration field type".into());
                };
                if !v.is_string() && !v.is_null() {
                    return Err("invalid configuration field type".into());
                }
                map.insert(k, if v.is_null() { json!("") } else { v });
            }
            Value::Object(map)
        } else {
            let OrderedJson::Scalar(v) = raw else {
                return Err("invalid configuration field type".into());
            };
            v
        };
        // Check each occurrence: an invalid earlier member cannot be hidden by
        // a later duplicate (including differently cased spellings).
        schema(&json!({field: value}), kind)?;
        out.insert(field.to_owned(), value);
    }
    Ok(Value::Object(out))
}
// Validate the complete typed snapshot before any mutation, including fields
// unrelated to that mutation. Go's former JSON decoder rejected these too.
fn schema(v: &Value, kind: &str) -> Result<(), String> {
    if v.is_null() {
        return Ok(());
    }
    let object = v.as_object().ok_or("invalid configuration field type")?;
    for (k, value) in object {
        if value.is_null() {
            continue;
        }
        let ty = match (kind, k.as_str()) {
            ("agent", "version") | ("client", "version") => "int",
            ("agent", "games") => "games",
            ("client", "servers") => "servers",
            ("client", "settings") => "settings",
            ("game", "id" | "name" | "description" | "type" | "protocol" | "dest" | "origin")
            | ("server", "id" | "name" | "description" | "game" | "proto" | "addr")
            | ("device", "publicKey" | "label")
            | ("bridge", "serverIp" | "bindIp") => "string",
            ("game", "tunnelPort")
            | ("server", "port" | "localPort" | "stablePort")
            | ("bridge", "discoveryPort" | "mediaPortFirst" | "mediaPortLast") => "port",
            ("bedrock", "mediaPortFirst" | "mediaPortLast") => "port",
            ("game", "created")
            | ("server", "addedAt" | "expiresAt")
            | ("device", "approvedAt") => "time",
            ("game", "devices") => "devices",
            ("game", "consoleBridge") => "bridge",
            ("game", "bedrock") => "bedrock",
            (
                "settings",
                "demoMode"
                | "autoReconnect"
                | "checkOnStartup"
                | "checkForUpdates"
                | "verboseLogging"
                | "allowLocalRelay"
                | "minecraftServerList"
                | "presenceNotifications",
            )
            | ("game", "disabled" | "knockEnabled") => "bool",
            ("settings", "gameExecutables" | "gameLaunchArgs") => "map",
            ("settings", "prismInstancesDir") => "string",
            _ => continue,
        };
        let valid = match ty {
            "int" => value.as_i64().is_some(),
            "port" => value.as_u64().is_some_and(|n| n <= 65535),
            "bool" => value.is_boolean(),
            "string" => value.is_string(),
            "time" => value
                .as_str()
                .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
            "map" => value
                .as_object()
                .is_some_and(|m| m.values().all(|v| v.is_string() || v.is_null())),
            "games" | "servers" | "devices" => {
                let sub = match ty {
                    "games" => "game",
                    "servers" => "server",
                    _ => "device",
                };
                value
                    .as_array()
                    .is_some_and(|a| a.iter().all(|v| schema(v, sub).is_ok()))
            }
            "settings" | "bridge" | "bedrock" => schema(value, ty).is_ok(),
            _ => false,
        };
        if !valid {
            return Err("invalid configuration field type or timestamp".into());
        }
    }
    Ok(())
}
fn decode(path: &str, data: &[u8], client: bool) -> Result<Value, String> {
    let raw: OrderedJson = serde_json::from_str(&pure::go_json(data)).map_err(|_| {
        if client {
            format!(
                "{path} is corrupt (invalid JSON); fix or move it aside, it was not overwritten"
            )
        } else {
            format!("{path} is corrupt: invalid JSON")
        }
    })?;
    let v = canonicalize(raw, if client { "client" } else { "agent" }, json!({}))
        .map_err(|e| format!("{path} is corrupt: {e}"))?;
    let ver = v["version"].as_i64().unwrap_or(0);
    if ver != 1 {
        return Err(format!("{path} has unsupported version {ver}"));
    }
    Ok(v)
}
fn read(path: &str, client: bool) -> Result<Value, String> {
    match storage::read_bounded(Path::new(path)) {
        Ok(data) => decode(path, &data, client),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if client {
                Ok(default_client())
            } else {
                Err(format!(
                    "agent is not initialised in {}; run `gamelink-agent init` first",
                    Path::new(path).parent().unwrap_or(Path::new(".")).display()
                ))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
fn array(v: &mut Value) -> &mut Vec<Value> {
    if !v.is_array() {
        *v = json!([])
    }
    v.as_array_mut().unwrap()
}
fn save(path: &str, v: &Value) -> Result<(), String> {
    storage::write_atomic(
        Path::new(path),
        &serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?,
    )
}

fn local_port(server: &Value, field: &str) -> Option<u16> {
    server[field]
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .filter(|&n| n != 0)
}

fn allocate_port(server: &Value, used: &std::collections::BTreeSet<u16>) -> Result<u16, String> {
    let preferred = crate::desktop::default_local_port(server);
    std::iter::once(preferred)
        .chain(20000..=29999)
        .find(|&port| port >= 1024 && !used.contains(&port))
        .ok_or("no local port is available for this server".into())
}

/// Upgrade older desktop configs before their servers can be joined or exported.
pub fn assign_client_ports(path: &str, mut cfg: Value) -> Result<Value, String> {
    let Some(servers) = cfg["servers"].as_array_mut() else {
        return Ok(cfg);
    };
    let mut used = std::collections::BTreeSet::new();
    let mut missing = Vec::new();
    for (index, server) in servers.iter_mut().enumerate() {
        if !local_port(server, "stablePort").is_some_and(|port| used.insert(port)) {
            missing.push(index);
        }
    }
    // Keep legacy explicit choices ahead of automatically allocated ports.
    for &index in &missing {
        let server = &mut servers[index];
        server["stablePort"] = local_port(server, "localPort")
            .filter(|&port| used.insert(port))
            .map_or(Value::Null, |port| json!(port));
    }
    for &index in &missing {
        let server = &mut servers[index];
        if server["stablePort"].is_null() {
            let port = allocate_port(server, &used)?;
            used.insert(port);
            server["localPort"] = json!(0);
            server["stablePort"] = json!(port);
        }
    }
    if !missing.is_empty() {
        save(path, &cfg)?;
    }
    Ok(cfg)
}

fn set_stable_port(server: &mut Value, others: impl Iterator<Item = u16>) -> Result<(), String> {
    let used = others.collect::<std::collections::BTreeSet<_>>();
    let port = if let Some(exact) = local_port(server, "localPort") {
        if used.contains(&exact) {
            return Err(format!(
                "local port {exact} is already assigned to another saved server"
            ));
        }
        exact
    } else if let Some(stable) =
        local_port(server, "stablePort").filter(|port| !used.contains(port))
    {
        stable
    } else {
        allocate_port(server, &used)?
    };
    server["stablePort"] = json!(port);
    Ok(())
}

fn valid_managed_game(game: &Value) -> Result<Value, String> {
    if !game.is_object() {
        return Err("invalid managed game".into());
    }
    let mut game = game.clone();
    let id = text(&game, "id");
    check("security.slug", json!({"text":id}))?;
    display("name", &text(&game, "name"), 64, true)?;
    display("description", &text(&game, "description"), 200, false)?;
    protocol(&text(&game, "type"), &text(&game, "protocol"))?;
    port(&game["tunnelPort"], false)?;
    game["dest"] = check("security.destination", json!({"text":game["dest"]}))?;
    if !game["disabled"].is_boolean() {
        return Err("invalid managed game disabled flag".into());
    }
    if !game["consoleBridge"].is_null() {
        bridge(&game["consoleBridge"])?;
        if game["type"] != "minecraft-bedrock" {
            return Err("console bridge requires Minecraft Bedrock".into());
        }
        if !game["bedrock"].is_null() {
            return Err("Bedrock media ports cannot be combined with the console bridge".into());
        }
    }
    if !game["bedrock"].is_null() {
        bedrock(&game["bedrock"])?;
        if game["type"] != "minecraft-bedrock" {
            return Err("Bedrock media ports require Minecraft Bedrock".into());
        }
    }
    game["devices"] = json!([]);
    Ok(game)
}

fn valid_managed_keys(keys: &Value) -> Result<(), String> {
    if !keys.is_object()
        || !keys["privateKey"].as_str().is_some_and(|v| !v.is_empty())
        || !keys["presharedKey"].as_str().is_some_and(|v| !v.is_empty())
        || !keys["created"]
            .as_str()
            .is_some_and(|v| chrono::DateTime::parse_from_rfc3339(v).is_ok())
    {
        return Err("invalid generated identity for managed game".into());
    }
    let region = &keys["region"];
    if region.is_null()
        || region["RegionID"].as_i64().unwrap_or(0) == 0
        || region["Nodes"]
            .as_array()
            .is_none_or(|nodes| nodes.is_empty())
    {
        return Err("a relay region is required for a new managed game".into());
    }
    Ok(())
}

// Reconcile one complete managed inventory under the caller's agent lock.
// All data is validated before writing new key files; failures clean up any
// new keys written before the atomic config save.
fn sync_managed(dir: &Path, cfg: &mut Value, input: &Value) -> Result<Value, String> {
    let origin = text(input, "origin");
    if origin.trim().is_empty() {
        return Err("managed game origin is required".into());
    }
    array(&mut cfg["games"]);
    let desired = input["games"]
        .as_array()
        .ok_or("managed games must be an array")?
        .iter()
        .map(valid_managed_game)
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = std::collections::BTreeSet::new();
    for game in &desired {
        let id = text(game, "id");
        if !seen.insert(id.to_owned()) {
            return Err(format!("duplicate managed game id {id:?}"));
        }
    }
    let key_map = input["keys"]
        .as_object()
        .ok_or("managed game keys must be an object")?;
    let indexes: std::collections::BTreeMap<String, usize> = cfg["games"]
        .as_array()
        .ok_or("invalid agent games")?
        .iter()
        .enumerate()
        .map(|(i, game)| (text(game, "id").to_owned(), i))
        .collect();
    if indexes.len() != cfg["games"].as_array().unwrap().len() {
        return Err("agent configuration has duplicate game IDs".into());
    }
    let mut new_ids = Vec::new();
    for game in &desired {
        let id = text(game, "id");
        if let Some(index) = indexes.get(&id.to_owned()) {
            if text(&cfg["games"][*index], "origin") != origin {
                return Err(format!(
                    "managed game id {id:?} belongs to a different origin"
                ));
            }
        } else {
            let keys = key_map
                .get(&id.to_owned())
                .ok_or_else(|| format!("missing generated identity for managed game {id:?}"))?;
            valid_managed_keys(keys)?;
            let path = dir.join("keys").join(format!("{id}.json"));
            if path.exists() {
                return Err(format!("keys for managed game {id:?} already exist"));
            }
            new_ids.push(id.to_owned());
        }
    }
    if key_map
        .keys()
        .any(|id| !new_ids.iter().any(|new_id| new_id == id))
    {
        return Err("generated identities included an unexpected game".into());
    }
    let old_len = cfg["games"].as_array().unwrap().len();
    if old_len + new_ids.len() > 32 {
        return Err("too many games (limit 32)".into());
    }

    let keys_dir = dir.join("keys");
    storage::ensure_dir(&keys_dir)?;
    let mut written = Vec::new();
    for id in &new_ids {
        let path = keys_dir.join(format!("{id}.json"));
        if let Err(error) = save(
            path.to_str().ok_or("invalid key path")?,
            key_map.get(id).ok_or("missing generated identity")?,
        ) {
            for written_path in &written {
                let _ = fs::remove_file(written_path);
            }
            return Err(error);
        }
        written.push(path);
    }

    let mut new_games = Vec::new();
    for mut game in desired {
        let id = text(&game, "id").to_owned();
        game["origin"] = json!(origin);
        if let Some(index) = indexes.get(&id) {
            let old = &cfg["games"][*index];
            game["created"] = old["created"].clone();
            game["devices"] = old["devices"].clone();
            game["consoleBridge"] = old["consoleBridge"].clone();
            if old["knockEnabled"] == true {
                game["knockEnabled"] = json!(true);
            }
            if game["bedrock"].is_null() {
                game["bedrock"] = old["bedrock"].clone();
            }
        } else {
            game["created"] = key_map[&id]["created"].clone();
            game["devices"] = json!([]);
        }
        new_games.push((id, game));
    }
    let games = array(&mut cfg["games"]);
    for (id, game) in new_games {
        if let Some(index) = indexes.get(&id) {
            games[*index] = game;
        } else {
            games.push(game);
        }
    }
    for game in array(&mut cfg["games"]) {
        if text(game, "origin") == origin.as_str() && !seen.contains(&text(game, "id")) {
            game["disabled"] = json!(true);
        }
    }
    if let Err(error) = save(input["path"].as_str().unwrap_or(""), cfg) {
        for written_path in &written {
            let _ = fs::remove_file(written_path);
        }
        return Err(error);
    }
    Ok(json!({"origin":origin,"discovered":seen.len()}))
}
/// Serializes changes to agent.json and its game key files.
pub fn lock_mutations(dir: &Path) -> Result<fs::File, String> {
    // Keep the file in place so everyone locks the same file. The OS releases
    // the lock when the handle closes, including if the process exits.
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts
        .open(dir.join("agent.lock"))
        .map_err(|e| e.to_string())?;
    for _ in 0..100 {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
            Err(TryLockError::Error(e)) => return Err(e.to_string()),
        }
    }
    Err(format!(
        "could not lock {} (another gamelink-agent command is running?)",
        dir.display()
    ))
}
pub fn dispatch(method: &str, input: Value) -> Result<Value, String> {
    let path = text(&input, "path");
    match method {
        "config.new_id" => {
            let mut bytes = [0u8; 6];
            getrandom::fill(&mut bytes).map_err(|_| "cannot generate server ID")?;
            Ok(json!(bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()))
        }
        "config.bridge" => {
            bridge(&input["bridge"])?;
            Ok(Value::Null)
        }
        "config.default_settings" => Ok(default_client()["settings"].clone()),
        "config.client_open" => read(&path, true),
        "config.agent_load" => read(&path, false),
        "config.agent_decode" => {
            let bytes = input["data"]
                .as_str()
                .ok_or("invalid snapshot transport")?
                .as_bytes();
            decode(&path, bytes, false)
        }
        "config.keys_load" => {
            let id = text(&input, "id");
            let data = match storage::read_bounded(Path::new(&path)) {
                Ok(data) => data,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(json!({"missing":true}))
                }
                Err(e) => return Err(format!("read keys for {id:?}: {e}")),
            };
            let v: Value = serde_json::from_slice(&data)
                .map_err(|_| format!("keys for {id:?} are corrupt: invalid JSON"))?;
            let v: Value = if input["sealed"] == true {
                let sealed = v["sealed"].as_str().ok_or("missing protected game keys")?;
                serde_json::from_str(&storage::unseal(sealed)?)
                    .map_err(|_| format!("keys for {id:?} are corrupt: invalid protected JSON"))?
            } else {
                v
            };
            if !v.is_object()
                || ["created", "expiresAt"].iter().any(|k| {
                    !v[*k].is_null()
                        && !v[*k]
                            .as_str()
                            .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
                })
                || ["privateKey", "presharedKey"]
                    .iter()
                    .any(|k| !v[*k].is_null() && !v[*k].is_string())
            {
                return Err(format!(
                    "keys for {id:?} are corrupt: invalid field type or timestamp"
                ));
            }
            let r = &v["region"];
            if r.is_null()
                || r["RegionID"].as_i64().unwrap_or(0) == 0
                || r["Nodes"].as_array().is_none_or(|x| x.is_empty())
            {
                return Err(format!("keys for {id:?} have no pinned relay region"));
            }
            Ok(json!({"keys":v,"missing":false}))
        }
        "config.save" => {
            save(&path, &input["config"])?;
            Ok(Value::Null)
        }
        "config.client_mutate" => {
            let mut cfg = input["config"].clone();
            let action = text(&input, "action");
            let v = &input["value"];
            let id = text(&input, "id");
            let result;
            match action.as_str() {
                "settings" => {
                    let known = check("games.all", json!({}))?;
                    for (field, max, msg) in [
                        ("gameExecutables", 1024, "executable path is too long"),
                        ("gameLaunchArgs", 512, "launch arguments are too long"),
                    ] {
                        if let Some(map) = v[field].as_object() {
                            for (game, value) in map {
                                if !known
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|x| x["type"] == game.as_str())
                                {
                                    return Err(format!("unknown game type {game:?}"));
                                }
                                let value = value.as_str().unwrap_or("");
                                if value.len() > max {
                                    return Err(msg.into());
                                }
                                if field == "gameLaunchArgs" && value.contains(['\r', '\n', '\0']) {
                                    return Err("launch arguments must be a single line".into());
                                }
                            }
                        }
                    }
                    cfg["settings"] = v.clone();
                    result = Value::Null;
                }
                "add" => {
                    fields(v)?;
                    let servers = array(&mut cfg["servers"]);
                    if servers.len() >= 64 {
                        return Err("too many saved servers (limit 64)".into());
                    }
                    let mut server = v.clone();
                    server["stablePort"] = Value::Null;
                    set_stable_port(
                        &mut server,
                        servers.iter().filter_map(|s| local_port(s, "stablePort")),
                    )?;
                    servers.push(server.clone());
                    result = server;
                }
                "update" | "remove" => {
                    let servers = array(&mut cfg["servers"]);
                    let index = servers
                        .iter()
                        .position(|x| x["id"] == id)
                        .ok_or("server not found")?;
                    if action == "remove" {
                        servers.remove(index);
                        result = Value::Null
                    } else {
                        let mut sv = servers[index].clone();
                        for k in ["name", "description", "localPort"] {
                            sv[k] = v[k].clone()
                        }
                        fields(&sv)?;
                        set_stable_port(
                            &mut sv,
                            servers
                                .iter()
                                .enumerate()
                                .filter(|(other, _)| *other != index)
                                .filter_map(|(_, s)| local_port(s, "stablePort")),
                        )?;
                        servers[index] = sv.clone();
                        result = sv
                    }
                }
                _ => return Err("unknown client mutation".into()),
            }
            save(&path, &cfg)?;
            Ok(json!({"config":cfg,"result":result}))
        }
        "config.game_prepare" => {
            let mut v = input["value"].clone();
            bridge(&v["consoleBridge"])?;
            bedrock(&v["bedrock"])?;
            if !v["consoleBridge"].is_null() && v["type"] != "minecraft-bedrock" {
                return Err("console bridge requires Minecraft Bedrock".into());
            }
            if !v["bedrock"].is_null() && v["type"] != "minecraft-bedrock" {
                return Err("Bedrock media ports require Minecraft Bedrock".into());
            }
            if !v["bedrock"].is_null() && !v["consoleBridge"].is_null() {
                return Err(
                    "Bedrock media ports cannot be combined with the console bridge".into(),
                );
            }
            let all = check("games.all", json!({}))?;
            let game = text(&v, "type");
            let info = all
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["type"] == game)
                .ok_or_else(|| format!("unknown game type {game:?}"))?;
            if text(&v, "protocol").is_empty() {
                v["protocol"] = info["protocol"].clone()
            }
            if v["tunnelPort"] == 0 {
                v["tunnelPort"] = info["defaultPort"].clone()
            }
            protocol(&game, &text(&v, "protocol"))?;
            port(&v["tunnelPort"], false).map_err(|e| format!("tunnel port: {e}"))?;
            if text(&v, "dest").is_empty() {
                v["dest"] = json!(format!("127.0.0.1:{}", v["tunnelPort"]))
            }
            v["dest"] = check("security.destination", json!({"text":v["dest"]}))?;
            if text(&v, "name").is_empty() {
                v["name"] = info["name"].clone()
            }
            display("name", &text(&v, "name"), 64, true)?;
            display("description", &text(&v, "description"), 200, false)?;
            if text(&v, "id").is_empty() {
                v["id"] = json!(slugify(&text(&v, "name")))
            }
            check("security.slug", json!({"text":v["id"]}))?;
            let r = &input["region"];
            if r.is_null()
                || r["RegionID"].as_i64().unwrap_or(0) == 0
                || r["Nodes"].as_array().is_none_or(|x| x.is_empty())
            {
                return Err("a relay region is required".into());
            }
            Ok(v)
        }
        "config.agent_mutate" => {
            let dir = Path::new(&path).parent().unwrap_or(Path::new("."));
            let action = text(&input, "action");
            if action == "init" {
                storage::ensure_dir(dir)?;
                storage::ensure_dir(&dir.join("keys"))?
            }
            let _lock = lock_mutations(dir)?;
            let mut cfg = if action == "init" {
                if Path::new(&path).exists() {
                    return Err(format!("{path} already exists; refusing to overwrite"));
                }
                json!({"version":1,"games":null})
            } else {
                read(&path, false)?
            };
            let id = text(&input, "id");
            let v = &input["value"];
            let mut result = Value::Null;
            let mut saved = false;
            match action.as_str() {
                "init" => {}
                "add" => {
                    let games = array(&mut cfg["games"]);
                    if games.len() >= 32 {
                        return Err("too many games (limit 32)".into());
                    }
                    if games.iter().any(|g| g["id"] == v["id"]) {
                        return Err(format!("a game with id {:?} already exists", text(v, "id")));
                    }
                    let keys = if input["sealed"] == true {
                        json!({"sealed": storage::seal(&input["keys"].to_string())?})
                    } else {
                        input["keys"].clone()
                    };
                    let key_path = dir.join("keys").join(format!("{}.json", text(v, "id")));
                    save(key_path.to_str().ok_or("invalid path")?, &keys)?;
                    games.push(v.clone());
                    if let Err(error) = save(&path, &cfg) {
                        let _ = fs::remove_file(&key_path);
                        return Err(error);
                    }
                    saved = true;
                    result = v.clone()
                }
                "set_destination" | "set_disabled" => {
                    let game = array(&mut cfg["games"])
                        .iter_mut()
                        .find(|g| g["id"] == id)
                        .ok_or_else(|| format!("no game with id {id:?}"))?;
                    if action == "set_destination" {
                        game["dest"] = check("security.destination", json!({"text":v["dest"]}))?;
                        game["disabled"] = json!(false);
                    } else {
                        game["disabled"] =
                            json!(v["disabled"].as_bool().ok_or("missing disabled flag")?);
                    }
                    result = game.clone();
                }
                "remove" => {
                    let games = array(&mut cfg["games"]);
                    let i = games
                        .iter()
                        .position(|g| g["id"] == id)
                        .ok_or_else(|| format!("no game with id {id:?}"))?;
                    games.remove(i);
                    match fs::remove_file(dir.join("keys").join(format!("{id}.json"))) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                "invite" => {
                    let g = array(&mut cfg["games"])
                        .iter_mut()
                        .find(|g| g["id"] == id)
                        .ok_or_else(|| format!("no game with id {id:?}"))?;
                    if [
                        "name",
                        "type",
                        "protocol",
                        "tunnelPort",
                        "consoleBridge",
                        "disabled",
                    ]
                    .iter()
                    .any(|k| g[*k] != input["expectedGame"][*k])
                    {
                        return Err("game settings changed; try again".into());
                    }
                    let key_path = dir.join("keys").join(format!("{id}.json"));
                    let keys = checked_current_keys(&key_path, &id, &input["expectedKeys"], false)?;
                    let replacement = &input["keys"];
                    pure::check_invite_expiry(&replacement["expiresAt"])?;
                    if g["knockEnabled"] != true {
                        // Enabling knocks first leaves the current identity
                        // intact if a following rotation cannot be saved.
                        g["knockEnabled"] = json!(true);
                        save(&path, &cfg)?;
                    }
                    if replacement != &keys {
                        save(key_path.to_str().ok_or("invalid path")?, replacement)?;
                    }
                    saved = true;
                }
                "approve" | "revoke" => {
                    let public = check("identity.public_key", json!({"text":input["publicKey"]}))?;
                    if action == "approve" {
                        display("label", &text(&input, "label"), 64, false)?
                    }
                    let games = array(&mut cfg["games"]);
                    let g = games
                        .iter_mut()
                        .find(|g| g["id"] == id)
                        .ok_or_else(|| format!("no game with id {id:?}"))?;
                    if action == "approve" && !input["expectedKeys"].is_null() {
                        if g["disabled"] == true {
                            return Err(format!("game {id:?} is disabled"));
                        }
                        let keys = checked_current_keys(
                            &dir.join("keys").join(format!("{id}.json")),
                            &id,
                            &input["expectedKeys"],
                            input["sealed"] == true,
                        )?;
                        pure::check_invite_expiry(&keys["expiresAt"])?;
                    }
                    let ds = array(&mut g["devices"]);
                    let i = ds.iter().position(|d| d["publicKey"] == public);
                    if action == "revoke" {
                        let i = i.ok_or_else(|| {
                            format!(
                                "device {} is not approved for {id:?}",
                                public.as_str().unwrap_or("")
                            )
                        })?;
                        ds.remove(i);
                    } else {
                        let label = text(&input, "label").trim().to_owned();
                        if let Some(i) = i {
                            if !label.is_empty() {
                                ds[i]["label"] = json!(label)
                            }
                            result = ds[i].clone()
                        } else {
                            if ds.len() >= 256 {
                                return Err("too many devices for this game (limit 256)".into());
                            }
                            result =
                                json!({"publicKey":public,"label":label,"approvedAt":input["now"]});
                            ds.push(result.clone())
                        }
                    }
                }
                "sync_managed" => {
                    result = sync_managed(dir, &mut cfg, &input)?;
                    saved = true;
                }
                _ => return Err("unknown agent mutation".into()),
            }
            if !saved {
                save(&path, &cfg)?;
            }
            Ok(result)
        }
        _ => Err("unknown config method".into()),
    }
}
fn checked_current_keys(
    path: &Path,
    id: &str,
    expected: &Value,
    sealed: bool,
) -> Result<Value, String> {
    check("security.slug", json!({"text":id}))?;
    let loaded = dispatch(
        "config.keys_load",
        json!({"id":id,"path":path,"sealed":sealed}),
    )?;
    if loaded["missing"] == true || &loaded["keys"] != expected {
        return Err("game identity changed; try again".into());
    }
    Ok(loaded["keys"].clone())
}
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    // Go uses simple lowercase: U+0130 maps to one ASCII i, while Rust's
    // full lowercase expands it to i plus a combining dot (a slug separator).
    for c in name.chars().flat_map(|c| {
        if c == '\u{0130}' {
            'i'.to_lowercase()
        } else {
            c.to_lowercase()
        }
    }) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true
        }
    }
    let out = out.trim_matches('-');
    let out = &out[..out.len().min(32)];
    let out = out.trim_matches('-');
    if out.is_empty() {
        "game".into()
    } else {
        out.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed_game(id: &str, name: &str, dest: &str, disabled: bool) -> Value {
        json!({
            "id":id,"name":name,"description":"","type":"minecraft-java",
            "protocol":"tcp","tunnelPort":25565,"dest":dest,"devices":[],
            "created":"","consoleBridge":null,"disabled":disabled
        })
    }
    fn managed_keys(created: &str) -> Value {
        json!({"privateKey":"privkey:test","presharedKey":"psk:test","created":created,
            "region":{"RegionID":900,"Nodes":[{"Name":"900a"}]}})
    }
    fn sync(dir: &Path, origin: &str, games: Value, keys: Value) -> Result<Value, String> {
        dispatch(
            "config.agent_mutate",
            json!({
                "path":dir.join("agent.json"),"action":"sync_managed","origin":origin,
                "games":games,"keys":keys
            }),
        )
    }
    fn load_agent(dir: &Path) -> Value {
        dispatch("config.agent_load", json!({"path":dir.join("agent.json")})).unwrap()
    }

    #[test]
    fn managed_sync_preserves_identity_approvals_and_uses_indices_after_append() {
        let dir = tempfile::tempdir().unwrap();
        dispatch(
            "config.agent_mutate",
            json!({"path":dir.path().join("agent.json"),"action":"init"}),
        )
        .unwrap();
        let created = "2026-01-01T00:00:00Z";
        sync(
            dir.path(),
            "panel-a-node-1",
            json!([managed_game("ptero-old", "Old", "192.0.2.1:25565", false)]),
            json!({"ptero-old":managed_keys(created)}),
        )
        .unwrap();
        dispatch("config.agent_mutate", json!({
            "path":dir.path().join("agent.json"),"action":"approve","id":"ptero-old",
            "publicKey":"nodekey:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "label":"approved","now":created
        })).unwrap();
        let old_key = storage::read_bounded(&dir.path().join("keys/ptero-old.json")).unwrap();

        // New item precedes an existing item. Existing key/device/creation
        // data must stay bound to the existing game's ID after the append.
        sync(
            dir.path(),
            "panel-a-node-1",
            json!([
                managed_game("ptero-new", "New", "192.0.2.3:25565", false),
                managed_game("ptero-old", "Renamed", "192.0.2.2:25565", false)
            ]),
            json!({"ptero-new":managed_keys("2026-02-02T00:00:00Z")}),
        )
        .unwrap();
        let cfg = load_agent(dir.path());
        let old = cfg["games"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"] == "ptero-old")
            .unwrap();
        assert_eq!(old["name"], "Renamed");
        assert_eq!(old["dest"], "192.0.2.2:25565");
        assert_eq!(old["created"], created);
        assert_eq!(old["devices"].as_array().unwrap().len(), 1);
        assert_eq!(old["origin"], "panel-a-node-1");
        assert_eq!(
            storage::read_bounded(&dir.path().join("keys/ptero-old.json")).unwrap(),
            old_key
        );
        assert_eq!(cfg["games"][0]["id"], "ptero-old");
        assert_eq!(cfg["games"][0]["dest"], "192.0.2.2:25565");
    }

    #[test]
    fn managed_sync_disables_missing_reenables_present_and_isolates_origins() {
        let dir = tempfile::tempdir().unwrap();
        dispatch(
            "config.agent_mutate",
            json!({"path":dir.path().join("agent.json"),"action":"init"}),
        )
        .unwrap();
        let key = managed_keys("2026-01-01T00:00:00Z");
        sync(
            dir.path(),
            "origin-a",
            json!([managed_game("owned", "Owned", "192.0.2.1:25565", false)]),
            json!({"owned":key}),
        )
        .unwrap();
        sync(
            dir.path(),
            "origin-b",
            json!([managed_game("foreign", "Foreign", "192.0.2.2:25565", false)]),
            json!({"foreign":key}),
        )
        .unwrap();
        sync(dir.path(), "origin-a", json!([]), json!({})).unwrap();
        let cfg = load_agent(dir.path());
        assert_eq!(cfg["games"][0]["disabled"], true);
        assert_eq!(cfg["games"][1]["disabled"], false);
        sync(
            dir.path(),
            "origin-a",
            json!([managed_game("owned", "Owned", "192.0.2.1:25565", false)]),
            json!({}),
        )
        .unwrap();
        let cfg = load_agent(dir.path());
        assert_eq!(cfg["games"][0]["disabled"], false);
        assert_eq!(cfg["games"][1]["disabled"], false);
    }

    #[test]
    fn invalid_managed_inventory_does_not_write_config_or_keys() {
        let dir = tempfile::tempdir().unwrap();
        dispatch(
            "config.agent_mutate",
            json!({"path":dir.path().join("agent.json"),"action":"init"}),
        )
        .unwrap();
        let before = load_agent(dir.path());
        let error = sync(
            dir.path(),
            "origin-a",
            json!([managed_game("new", "New", "192.0.2.1:25565", false)]),
            json!({}),
        )
        .unwrap_err();
        assert!(error.contains("missing generated identity"));
        assert_eq!(load_agent(dir.path()), before);
        assert!(!dir.path().join("keys/new.json").exists());
    }

    #[test]
    fn original_go_fixtures() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../fixtures/storage-go-original.json"))
                .unwrap();
        for f in fixtures.as_array().unwrap() {
            match f["kind"].as_str().unwrap() {
                "slug" => assert_eq!(json!(slugify(f["input"].as_str().unwrap())), f["output"]),
                "settings" => {
                    let mut expected = f["output"].clone();
                    expected["checkForUpdates"] = json!(false);
                    assert_eq!(default_client()["settings"], expected);
                }
                "unseal" => {
                    #[cfg(not(windows))]
                    {
                        let result = storage::unseal(f["input"].as_str().unwrap());
                        if f["error"] == "" {
                            assert_eq!(json!(result.unwrap()), f["output"])
                        } else {
                            assert_eq!(result.unwrap_err(), f["error"].as_str().unwrap())
                        }
                    }
                }
                _ => panic!("unknown fixture"),
            }
        }
    }
    #[test]
    fn ordered_typed_decode_matches_go_merge_and_null_rules() {
        let v = decode("agent.json", br#"{"version":2,"Version":1,"games":[{"id":"first","ID":"last","name":"keep","NAME":null,"consoleBridge":{"serverIp":"one"},"ConsoleBridge":{"bindIp":"two"}}]}"#, false).unwrap();
        assert_eq!(v["games"][0]["id"], "last");
        assert_eq!(v["games"][0]["name"], "keep");
        assert_eq!(
            v["games"][0]["consoleBridge"],
            json!({"serverIp":"one","bindIp":"two"})
        );
        let v = decode("client.json", br#"{"version":1,"settings":{"demoMode":true,"gameExecutables":{"custom":"first","custom":null}},"Settings":{"autoReconnect":true,"gameExecutables":{"other":"second"}},"settings":null}"#, true).unwrap();
        assert_eq!(
            v["settings"],
            json!({"demoMode":true,"autoReconnect":true,"gameExecutables":{"custom":"","other":"second"}})
        );
        let v = decode("client.json", br#"{"version":1,"settings":{"gameExecutables":{"old":"first"}},"settings":{"gameExecutables":null}}"#, true).unwrap();
        assert!(v["settings"]["gameExecutables"].is_null());
    }
    #[test]
    fn earlier_invalid_duplicates_remain_errors() {
        for data in [
            r#"{"version":"bad","version":1}"#,
            r#"{"version":1,"games":[{"name":42,"NAME":"valid"}]}"#,
            r#"{"version":1,"games":[{"consoleBridge":{"mediaPortFirst":"bad"},"consoleBridge":null}]}"#,
            r#"{"version":1,"games":[{"devices":[{"approvedAt":"bad","approvedAt":"2026-01-01T00:00:00Z"}]}]}"#,
        ] {
            assert!(
                decode("agent.json", data.as_bytes(), false).is_err(),
                "{data}"
            );
        }
        for data in [
            r#"{"version":1,"settings":{"demoMode":42,"demoMode":true}}"#,
            r#"{"version":1,"settings":{"gameExecutables":{"custom":42,"custom":"valid"}}}"#,
        ] {
            assert!(
                decode("client.json", data.as_bytes(), true).is_err(),
                "{data}"
            );
        }
    }
    #[test]
    fn file_size_bound_and_private_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.json");
        storage::write_atomic(&path, b"one").unwrap();
        storage::write_atomic(&path, b"two").unwrap();
        assert_eq!(storage::read_bounded(&path).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            )
        }
        fs::write(&path, vec![0; 8 * 1024 * 1024 + 1]).unwrap();
        assert!(storage::read_bounded(&path).is_err());
    }
}

// Behaviour ported from the legacy Go tests in tests/testsupport/config. The Go
// wrappers only forwarded to these dispatch entry points.
#[cfg(test)]
mod behaviour {
    use super::*;
    use std::path::PathBuf;

    const KEY_A: &str = "nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "nodekey:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn region() -> Value {
        json!({"RegionID":900,"RegionCode":"custom","RegionName":"Custom","Nodes":[{"Name":"900a","RegionID":900,"HostName":"derp.example.com"}]})
    }
    fn keys() -> Value {
        json!({"privateKey":"privkey:00","presharedKey":"psk","region":region(),"created":"2026-01-01T00:00:00Z"})
    }
    fn agent_json(dir: &Path) -> PathBuf {
        dir.join("agent.json")
    }
    fn mutate(dir: &Path, action: &str, id: &str, extra: Value) -> Result<Value, String> {
        let mut input = extra;
        input["path"] = json!(agent_json(dir));
        input["action"] = json!(action);
        input["id"] = json!(id);
        dispatch("config.agent_mutate", input)
    }
    fn load(dir: &Path) -> Result<Value, String> {
        dispatch("config.agent_load", json!({"path": agent_json(dir)}))
    }
    fn prepare(value: Value) -> Result<Value, String> {
        let mut v = json!({"tunnelPort":0});
        for (k, x) in value.as_object().unwrap() {
            v[k] = x.clone();
        }
        dispatch("config.game_prepare", json!({"value":v,"region":region()}))
    }
    fn add_game(dir: &Path, value: Value) -> Result<Value, String> {
        let mut game = prepare(value)?;
        game["created"] = json!("2026-01-01T00:00:00Z");
        game["devices"] = json!([]);
        mutate(dir, "add", "", json!({"value":game,"keys":keys()}))
    }
    fn approve(dir: &Path, game: &str, key: &str, label: &str) -> Result<Value, String> {
        mutate(
            dir,
            "approve",
            game,
            json!({"publicKey":key,"label":label,"now":"2026-02-02T00:00:00Z"}),
        )
    }
    fn fresh_agent() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("agent");
        mutate(&dir, "init", "", json!({})).unwrap();
        (root, dir)
    }
    fn client_path(dir: &Path) -> PathBuf {
        dir.join("config.json")
    }
    fn open_client(dir: &Path) -> Result<Value, String> {
        dispatch("config.client_open", json!({"path": client_path(dir)}))
    }
    fn client_mutate(
        dir: &Path,
        cfg: &Value,
        action: &str,
        id: &str,
        value: Value,
    ) -> Result<Value, String> {
        dispatch(
            "config.client_mutate",
            json!({"path":client_path(dir),"config":cfg,"action":action,"id":id,"value":value}),
        )
    }
    fn server_value() -> Value {
        json!({"id":"s1","name":"Survival","description":"","game":"minecraft-java","proto":"tcp","port":25565,"localPort":0,
               "addr":storage::seal("tcSECRETADDRESSVALUE").unwrap(),"addedAt":"2026-01-01T00:00:00Z"})
    }

    // TestConsoleBridgePortRange
    #[test]
    fn console_bridge_validation() {
        let ok = |b: Value| dispatch("config.bridge", json!({"bridge": b}));
        let bridge = |first: u64, last: u64| json!({"serverIp":"127.0.0.1","bindIp":"127.0.0.1","mediaPortFirst":first,"mediaPortLast":last});
        assert!(ok(bridge(1000, 1255)).is_ok());
        assert!(ok(bridge(1000, 1256)).is_err(), "257 ports accepted");
        assert!(ok(bridge(0, 10)).is_err());
        assert!(ok(bridge(10, 9)).is_err());
        assert!(ok(Value::Null).is_ok());
        for ip in ["", "nope", "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1"] {
            let mut b = bridge(1, 2);
            b["serverIp"] = json!(ip);
            assert!(ok(b).is_err(), "server ip {ip:?}");
            let mut b = bridge(1, 2);
            b["bindIp"] = json!(ip);
            assert!(ok(b).is_err(), "bind ip {ip:?}");
        }
    }

    #[test]
    fn ids_and_default_settings() {
        let a = dispatch("config.new_id", json!({})).unwrap();
        let b = dispatch("config.new_id", json!({})).unwrap();
        assert_eq!(a.as_str().unwrap().len(), 12);
        assert!(a.as_str().unwrap().bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        let s = dispatch("config.default_settings", json!({})).unwrap();
        assert_eq!(s["autoReconnect"], true);
        assert_eq!(s["demoMode"], false);
        assert_eq!(s["checkOnStartup"], true);
        assert_eq!(
            dispatch("config.nope", json!({})).unwrap_err(),
            "unknown config method"
        );
    }

    // TestClientStoreReadWritePersist
    #[test]
    fn client_store_read_write_persist() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = open_client(dir.path()).unwrap();
        assert_eq!(cfg["settings"]["autoReconnect"], true);
        assert_eq!(cfg["settings"]["demoMode"], false);
        assert!(!client_path(dir.path()).exists(), "open must not write");

        let added = client_mutate(dir.path(), &cfg, "add", "", server_value()).unwrap();
        assert_eq!(added["result"]["name"], "Survival");
        let mut cfg = added["config"].clone();

        let mut settings = cfg["settings"].clone();
        settings["demoMode"] = json!(true);
        settings["gameExecutables"] = json!({"factorio": "C:\\games\\factorio.exe"});
        cfg = client_mutate(dir.path(), &cfg, "settings", "", settings).unwrap()["config"].clone();

        // The address is persisted only in sealed form.
        let raw = fs::read_to_string(client_path(dir.path())).unwrap();
        assert!(raw.contains("gls1:"));
        #[cfg(windows)]
        assert!(!raw.contains("tcSECRETADDRESSVALUE"));

        // "Restart": a new open sees everything.
        let reopened = open_client(dir.path()).unwrap();
        assert_eq!(reopened, cfg);
        assert_eq!(reopened["servers"][0]["name"], "Survival");
        assert_eq!(reopened["servers"][0]["port"], 25565);
        assert_eq!(reopened["settings"]["demoMode"], true);
        assert_eq!(
            storage::unseal(reopened["servers"][0]["addr"].as_str().unwrap()).unwrap(),
            "tcSECRETADDRESSVALUE"
        );

        let updated = client_mutate(
            dir.path(),
            &reopened,
            "update",
            "s1",
            json!({"name":"Renamed","description":"d","localPort":30000}),
        )
        .unwrap();
        assert_eq!(updated["result"]["name"], "Renamed");
        assert_eq!(updated["result"]["localPort"], 30000);
        // Update leaves everything but the three editable fields alone.
        assert_eq!(updated["result"]["addr"], reopened["servers"][0]["addr"]);
        assert_eq!(updated["result"]["port"], 25565);

        let removed =
            client_mutate(dir.path(), &updated["config"], "remove", "s1", Value::Null).unwrap();
        assert_eq!(removed["result"], Value::Null);
        assert_eq!(
            open_client(dir.path()).unwrap()["servers"],
            json!([]),
            "removed server came back"
        );
    }

    #[test]
    fn assigns_unique_stable_ports_and_persists_migration_and_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = client_path(dir.path());
        let mut legacy = default_client();
        let first = server_value();
        let mut pinned = first.clone();
        pinned["id"] = json!("s2");
        pinned["localPort"] = json!(25565);
        legacy["servers"] = json!([first, pinned]);
        save(path.to_str().unwrap(), &legacy).unwrap();

        let migrated =
            assign_client_ports(path.to_str().unwrap(), open_client(dir.path()).unwrap()).unwrap();
        assert_eq!(migrated["servers"][0]["stablePort"], 20000);
        assert_eq!(migrated["servers"][1]["stablePort"], 25565);
        assert_eq!(open_client(dir.path()).unwrap(), migrated);

        let mut added = server_value();
        added["id"] = json!("s3");
        let added = client_mutate(dir.path(), &migrated, "add", "", added).unwrap();
        assert_eq!(added["result"]["stablePort"], 20001);
        let renamed = client_mutate(
            dir.path(),
            &added["config"],
            "update",
            "s3",
            json!({"name":"Renamed", "description":"", "localPort":0}),
        )
        .unwrap();
        assert_eq!(renamed["result"]["stablePort"], 20001);
        let reopened = open_client(dir.path()).unwrap();
        assert_eq!(renamed["config"], reopened);
        let duplicate = client_mutate(
            dir.path(),
            &reopened,
            "update",
            "s3",
            json!({"name":"Renamed", "description":"", "localPort":25565}),
        );
        assert!(duplicate.unwrap_err().contains("already assigned"));
    }

    // TestClientStoreRejectsInvalid
    #[test]
    fn client_store_rejects_invalid_servers() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = open_client(dir.path()).unwrap();
        type Mutation = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("empty name", Box::new(|v| v["name"] = json!(""))),
            ("whitespace name", Box::new(|v| v["name"] = json!("   "))),
            ("unknown game", Box::new(|v| v["game"] = json!("doom"))),
            ("wrong proto", Box::new(|v| v["proto"] = json!("udp"))),
            ("unknown proto", Box::new(|v| v["proto"] = json!("sctp"))),
            ("zero port", Box::new(|v| v["port"] = json!(0))),
            ("huge port", Box::new(|v| v["port"] = json!(70000))),
            (
                "bad local port",
                Box::new(|v| v["localPort"] = json!(70000)),
            ),
            (
                "long desc",
                Box::new(|v| v["description"] = json!("d".repeat(201))),
            ),
            ("control in name", Box::new(|v| v["name"] = json!("a\nb"))),
        ];
        for (name, mutate) in cases {
            let mut v = server_value();
            mutate(&mut v);
            assert!(
                client_mutate(dir.path(), &cfg, "add", "", v).is_err(),
                "{name} accepted"
            );
        }
        assert!(
            !client_path(dir.path()).exists(),
            "an invalid server caused a write"
        );
    }

    #[test]
    fn client_mutations_report_missing_servers_limits_and_unknown_actions() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = open_client(dir.path()).unwrap();
        for action in ["update", "remove"] {
            assert_eq!(
                client_mutate(dir.path(), &cfg, action, "nope", json!({})).unwrap_err(),
                "server not found"
            );
        }
        assert_eq!(
            client_mutate(dir.path(), &cfg, "explode", "", Value::Null).unwrap_err(),
            "unknown client mutation"
        );
        let full: Vec<Value> = (0..64).map(|_| server_value()).collect();
        let cfg = json!({"version":1,"servers":full,"settings":cfg["settings"]});
        assert_eq!(
            client_mutate(dir.path(), &cfg, "add", "", server_value()).unwrap_err(),
            "too many saved servers (limit 64)"
        );
        // Update validates the merged record, so a bad edit is rejected.
        let one = client_mutate(
            dir.path(),
            &open_client(dir.path()).unwrap(),
            "add",
            "",
            server_value(),
        )
        .unwrap();
        assert!(client_mutate(
            dir.path(),
            &one["config"],
            "update",
            "s1",
            json!({"name":"","description":"","localPort":0})
        )
        .is_err());
        assert!(client_mutate(
            dir.path(),
            &one["config"],
            "update",
            "s1",
            json!({"name":"ok","description":"","localPort":70000})
        )
        .is_err());
    }

    #[test]
    fn client_settings_validation() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = open_client(dir.path()).unwrap();
        let apply = |settings: Value| client_mutate(dir.path(), &cfg, "settings", "", settings);
        let base = || cfg["settings"].clone();
        let with = |field: &str, value: Value| {
            let mut s = base();
            s[field] = value;
            s
        };
        assert!(apply(with("gameExecutables", json!({"factorio":"/x"}))).is_ok());
        assert!(apply(with("gameLaunchArgs", json!({"factorio":""}))).is_ok());
        assert_eq!(
            apply(with("gameExecutables", json!({"doom":"/x"}))).unwrap_err(),
            "unknown game type \"doom\""
        );
        assert_eq!(
            apply(with(
                "gameExecutables",
                json!({"factorio":"x".repeat(1025)})
            ))
            .unwrap_err(),
            "executable path is too long"
        );
        assert!(apply(with(
            "gameExecutables",
            json!({"factorio":"x".repeat(1024)})
        ))
        .is_ok());
        assert_eq!(
            apply(with("gameLaunchArgs", json!({"factorio":"x".repeat(513)}))).unwrap_err(),
            "launch arguments are too long"
        );
        for bad in ["a\nb", "a\rb", "a\0b"] {
            assert_eq!(
                apply(with("gameLaunchArgs", json!({"factorio":bad}))).unwrap_err(),
                "launch arguments must be a single line"
            );
        }
        assert_eq!(
            apply(with("gameLaunchArgs", json!({"doom":"x"}))).unwrap_err(),
            "unknown game type \"doom\""
        );
    }

    // TestClientStoreCorruptFileIsNotOverwritten + TestRustClientRejectsTypedCorruptionWithoutOverwrite
    #[test]
    fn corrupt_client_files_are_rejected_and_left_untouched() {
        for (data, expect) in [
            (
                "{broken",
                "is corrupt (invalid JSON); fix or move it aside, it was not overwritten",
            ),
            (
                r#"{"version":1,"servers":[{"port":"secret"}]}"#,
                "is corrupt",
            ),
            (
                r#"{"version":1,"settings":{"demoMode":"secret"}}"#,
                "is corrupt",
            ),
            (
                r#"{"version":1,"servers":[{"addedAt":"invalid"}]}"#,
                "is corrupt",
            ),
            (r#"{"version":2}"#, "has unsupported version 2"),
            (r#"{"servers":[]}"#, "has unsupported version 0"),
            ("[1]", "is corrupt"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(client_path(dir.path()), data).unwrap();
            let error = open_client(dir.path()).unwrap_err();
            assert!(error.contains(expect), "{data}: {error}");
            assert_eq!(
                fs::read_to_string(client_path(dir.path())).unwrap(),
                data,
                "corrupt file overwritten"
            );
            // The secret value in a corrupt field must not leak into the error.
            assert!(!error.contains("secret\""), "{error}");
        }
    }

    // TestRustClientWriteFailurePreservesMemory (the input config is never mutated)
    #[test]
    fn client_write_failure_returns_an_error_and_keeps_the_input_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = open_client(dir.path()).unwrap();
        let before = cfg.clone();
        // A directory sits where config.json should be written.
        fs::create_dir(client_path(dir.path())).unwrap();
        assert!(client_mutate(dir.path(), &cfg, "add", "", server_value()).is_err());
        assert!(client_mutate(dir.path(), &cfg, "settings", "", cfg["settings"].clone()).is_err());
        assert_eq!(cfg, before);
    }

    // TestRustClientSettingsWireOrder
    #[test]
    fn client_settings_follow_go_wire_order_semantics() {
        let data = br#"{"version":2,"Version":1,"settings":{"demoMode":true,"gameExecutables":{"custom":"first","custom":null}},"Settings":{"autoReconnect":true,"gameExecutables":{"minecraft-java":"java"}},"settings":null}"#;
        let v = decode("config.json", data, true).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["settings"]["demoMode"], true);
        assert_eq!(v["settings"]["autoReconnect"], true);
        assert_eq!(
            v["settings"]["gameExecutables"],
            json!({"custom":"","minecraft-java":"java"})
        );
        for data in [
            r#"{"version":1,"settings":{"demoMode":"bad","demoMode":true}}"#,
            r#"{"version":1,"settings":{"gameExecutables":{"custom":42,"custom":"fixed"}}}"#,
            r#"{"version":1,"settings":{"demoMode":"bad"},"settings":{"demoMode":true}}"#,
            r#"{"version":1,"settings":{"gameExecutables":{"a":{}}}}"#,
            r#"{"version":1,"settings":{"gameExecutables":"x"}}"#,
            r#"{"version":1,"settings":5}"#,
            r#"{"version":1,"servers":5}"#,
        ] {
            assert!(
                decode("config.json", data.as_bytes(), true).is_err(),
                "{data}"
            );
        }
    }

    // TestRustAgentSnapshotWireOrder (expectations follow encoding/json merge rules)
    #[test]
    fn agent_snapshots_follow_go_wire_order_semantics() {
        let agent = |data: &[u8]| decode("agent.json", data, false).unwrap();
        let device = r#"[{"publicKey":"nodekey:approved"}]"#;
        let v = agent(
            format!(r#"{{"version":1,"games":[{{"id":"g","devices":{device},"Devices":[]}}]}}"#)
                .as_bytes(),
        );
        assert_eq!(v["games"][0]["devices"], json!([]));
        let v = agent(
            format!(r#"{{"version":1,"games":[{{"id":"g","Devices":[],"devices":{device}}}]}}"#)
                .as_bytes(),
        );
        assert_eq!(v["games"][0]["devices"][0]["publicKey"], "nodekey:approved");
        let v = agent(
            format!(r#"{{"version":1,"games":[{{"id":"g","devices":{device},"devices":[]}}]}}"#)
                .as_bytes(),
        );
        assert_eq!(v["games"][0]["devices"], json!([]));
        let v = agent(
            format!(r#"{{"version":1,"games":[{{"id":"g","devices":{device},"Devices":null}}]}}"#)
                .as_bytes(),
        );
        assert!(v["games"][0]["devices"].is_null());

        let v = agent(br#"{"version":2,"Version":1,"games":[{"id":"first","ID":"last","Name":"old","name":"new"}]}"#);
        assert_eq!(v["version"], 1);
        assert_eq!(v["games"][0]["id"], "last");
        assert_eq!(v["games"][0]["name"], "new");

        let v = agent(br#"{"version":1,"version":1,"games":[{"id":"g","name":"keep","name":null,"tunnelPort":12,"TUNNELPORT":null,"consoleBridge":{"serverIp":"192.168.1.1","mediaPortFirst":1},"ConsoleBridge":{"bindIp":"192.168.1.2","mediaPortLast":2}}]}"#);
        assert_eq!(v["games"][0]["name"], "keep");
        assert_eq!(v["games"][0]["tunnelPort"], 12);
        assert_eq!(
            v["games"][0]["consoleBridge"],
            json!({"serverIp":"192.168.1.1","bindIp":"192.168.1.2","mediaPortFirst":1,"mediaPortLast":2})
        );

        let v = agent(br#"{"version":1,"games":[{"id":"g","devices":[{"label":"first","LABEL":"last"}],"consoleBridge":{"serverIp":"old"},"consoleBridge":null,"consoleBridge":{"bindIp":"new"}}],"games":null}"#);
        assert!(v["games"].is_null());
        let v = agent(br#"{"version":1,"games":[{"id":"g","devices":[{"label":"first","LABEL":"last"}],"consoleBridge":{"serverIp":"old"},"consoleBridge":null,"consoleBridge":{"bindIp":"new"}}]}"#);
        assert_eq!(v["games"][0]["devices"][0]["label"], "last");
        assert_eq!(v["games"][0]["consoleBridge"], json!({"bindIp":"new"}));

        // Go reuses slice capacity, so a second array merges over the first.
        let v = agent(br#"{"version":1,"games":[{"id":"keep","devices":[{"label":"keep"}]}],"games":[{"devices":[{}]}]}"#);
        assert_eq!(v["games"][0]["id"], "keep");
        assert_eq!(v["games"][0]["devices"][0]["label"], "keep");
        let v = agent(br#"{"version":1,"games":[{"id":"keep"}],"games":[null]}"#);
        assert_eq!(v["games"][0]["id"], "keep");

        // Long s and a lone surrogate / invalid UTF-8 follow Go's decoder.
        let v = agent(
            "{\"version\":1,\"game\u{17f}\":[{\"id\":\"g\",\"name\":\"\\ud800\"}]}".as_bytes(),
        );
        assert_eq!(v["games"][0]["name"], "\u{fffd}");
        let v = agent(b"{\"version\":1,\"games\":[{\"id\":\"g\",\"name\":\"\xff\"}]}");
        assert_eq!(v["games"][0]["name"], "\u{fffd}");
    }

    // TestRustAgentDuplicateCorruptionCannotBeHiddenOrMutated
    #[test]
    fn duplicate_member_corruption_cannot_be_hidden_or_mutated() {
        for data in [
            r#"{"version":"bad","version":1,"games":[{"id":"g"}]}"#,
            r#"{"version":1,"games":[{"id":42,"id":"g"}]}"#,
            r#"{"version":1,"games":[{"devices":[{"label":42,"LABEL":"fixed"}]}]}"#,
            r#"{"version":1,"games":[{"consoleBridge":{"mediaPortFirst":"bad"},"consoleBridge":{"mediaPortFirst":1}}]}"#,
            r#"{"version":1,"games":[{"created":"invalid","created":"2026-01-01T00:00:00Z"}]}"#,
            // TestRustAgentCorruptionCannotBeMutated
            r#"{"version":1,"games":"oops"}"#,
            r#"{"version":1,"games":[{"devices":"oops"}]}"#,
            r#"{"version":1,"games":[{"created":"invalid"}]}"#,
            r#"{"version":1,"games":[5]}"#,
            r#"{"version":1,"games":[{"tunnelPort":70000}]}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(agent_json(dir.path()), data).unwrap();
            assert!(load(dir.path()).is_err(), "Load accepted {data}");
            assert!(
                dispatch(
                    "config.agent_decode",
                    json!({"path":agent_json(dir.path()),"data":data})
                )
                .is_err(),
                "Decode accepted {data}"
            );
            assert!(mutate(dir.path(), "remove", "g", json!({})).is_err());
            assert!(approve(dir.path(), "g", KEY_A, "x").is_err());
            assert_eq!(
                fs::read_to_string(agent_json(dir.path())).unwrap(),
                data,
                "snapshot changed"
            );
        }
    }

    #[test]
    fn agent_decode_requires_text_transport_and_load_reports_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            dispatch("config.agent_decode", json!({"path":"x"})).unwrap_err(),
            "invalid snapshot transport"
        );
        fs::create_dir(agent_json(dir.path())).unwrap();
        let error = load(dir.path()).unwrap_err();
        assert!(!error.contains("not initialised"), "{error}");
    }

    // TestRustLargeAgentSnapshotAndConcurrentMutations
    #[test]
    fn large_agent_snapshot_and_concurrent_mutations() {
        let (_root, dir) = fresh_agent();
        let games: Vec<Value> = (0..32)
            .map(|i| {
                let devices: Vec<Value> = (0..256)
                    .map(|_| json!({"publicKey":KEY_A,"label":"x".repeat(64)}))
                    .collect();
                json!({"id":"a".repeat(i + 1),"name":"Game","devices":devices})
            })
            .collect();
        let data = serde_json::to_vec(&json!({"version":1,"games":games})).unwrap();
        assert!(data.len() > 32 * 1024);
        fs::write(agent_json(&dir), &data).unwrap();
        let got = load(&dir).unwrap();
        assert_eq!(got["games"].as_array().unwrap().len(), 32);
        assert_eq!(got["games"][0]["devices"].as_array().unwrap().len(), 256);
        let decoded = dispatch(
            "config.agent_decode",
            json!({"path":agent_json(&dir),"data":String::from_utf8(data).unwrap()}),
        )
        .unwrap();
        assert_eq!(decoded, got);

        let handles: Vec<_> = ["a", "aa"]
            .into_iter()
            .map(|id| {
                let dir = dir.clone();
                std::thread::spawn(move || approve(&dir, id, KEY_A, "updated"))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let got = load(&dir).unwrap();
        assert_eq!(got["games"][0]["devices"][0]["label"], "updated");
        assert_eq!(got["games"][1]["devices"][0]["label"], "updated");
        drop(lock_mutations(&dir).expect("lock leaked"));
    }

    // TestExampleAgentConfigLoads
    #[test]
    fn shipped_example_agent_config_loads_and_has_no_key_material() {
        let raw = include_str!("../../../../config/examples/agent.example.json");
        let lower = raw.to_lowercase();
        assert!(!lower.contains("privatekey") && !raw.contains("presharedKey"));
        let dir = tempfile::tempdir().unwrap();
        fs::write(agent_json(dir.path()), raw).unwrap();
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg["games"].as_array().unwrap().len(), 1);
        assert_eq!(cfg["games"][0]["devices"].as_array().unwrap().len(), 1);
    }

    // TestAgentStoreLifecycle
    #[test]
    fn agent_store_lifecycle() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("agent");
        let error = load(&dir).unwrap_err();
        assert!(
            error.contains("agent is not initialised") && error.contains("gamelink-agent init"),
            "{error}"
        );
        assert!(mutate(&dir, "approve", "g", json!({"publicKey":KEY_A})).is_err());
        mutate(&dir, "init", "", json!({})).unwrap();
        assert!(dir.join("keys").is_dir());
        assert_eq!(load(&dir).unwrap(), json!({"version":1,"games":null}));
        let error = mutate(&dir, "init", "", json!({})).unwrap_err();
        assert!(
            error.ends_with("already exists; refusing to overwrite"),
            "{error}"
        );

        let g = add_game(
            &dir,
            json!({"name":"Survival World","type":"minecraft-java"}),
        )
        .unwrap();
        assert_eq!(g["id"], "survival-world");
        assert_eq!(g["tunnelPort"], 25565);
        assert_eq!(g["dest"], "127.0.0.1:25565");
        assert_eq!(g["protocol"], "tcp");
        let g2 = add_game(&dir, json!({"name":"Factory","type":"factorio"})).unwrap();
        assert_eq!(g2["id"], "factory");
        assert_eq!(g2["protocol"], "udp");
        let error = add_game(
            &dir,
            json!({"name":"Survival World","type":"minecraft-java"}),
        )
        .unwrap_err();
        assert!(error.contains("already exists"), "{error}");

        // Each game has its own persisted key file.
        let key_file = dir.join("keys").join("survival-world.json");
        let loaded = dispatch(
            "config.keys_load",
            json!({"id":"survival-world","path":key_file}),
        )
        .unwrap();
        assert_eq!(loaded["missing"], false);
        assert_eq!(loaded["keys"]["privateKey"], "privkey:00");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&key_file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        // Approval is idempotent and labels update; other games are unaffected.
        let first = approve(&dir, "survival-world", KEY_A, "  alice ").unwrap();
        assert_eq!(first["label"], "alice");
        assert_eq!(first["approvedAt"], "2026-02-02T00:00:00Z");
        approve(&dir, "survival-world", KEY_A, "alice-again").unwrap();
        // An empty label on re-approval keeps the existing one.
        let kept = approve(&dir, "survival-world", KEY_A, "").unwrap();
        assert_eq!(kept["label"], "alice-again");
        let cfg = load(&dir).unwrap();
        let games = cfg["games"].as_array().unwrap();
        let mine = games.iter().find(|g| g["id"] == "survival-world").unwrap();
        assert_eq!(mine["devices"].as_array().unwrap().len(), 1);
        assert_eq!(mine["devices"][0]["label"], "alice-again");
        let other = games.iter().find(|g| g["id"] == "factory").unwrap();
        assert!(other["devices"].as_array().unwrap().is_empty());

        mutate(&dir, "revoke", "survival-world", json!({"publicKey":KEY_A})).unwrap();
        let error =
            mutate(&dir, "revoke", "survival-world", json!({"publicKey":KEY_A})).unwrap_err();
        assert!(error.contains("is not approved for"), "{error}");
        let error = mutate(&dir, "revoke", "nope", json!({"publicKey":KEY_A})).unwrap_err();
        assert!(error.contains("no game with id"), "{error}");

        // agent.json never holds secrets.
        let raw = fs::read_to_string(agent_json(&dir)).unwrap();
        assert!(!raw.contains("privkey") && !raw.contains("presharedKey"));

        mutate(&dir, "remove", "survival-world", json!({})).unwrap();
        assert_eq!(
            dispatch(
                "config.keys_load",
                json!({"id":"survival-world","path":key_file})
            )
            .unwrap(),
            json!({"missing":true}),
            "keys survive game removal"
        );
        assert!(mutate(&dir, "remove", "survival-world", json!({})).is_err());
        assert_eq!(
            mutate(&dir, "explode", "", json!({})).unwrap_err(),
            "unknown agent mutation"
        );
        drop(lock_mutations(&dir).expect("lock leaked"));
    }

    // TestAgentStoreRejectsInvalid
    #[test]
    fn agent_store_rejects_invalid_games_and_keys() {
        let (_root, dir) = fresh_agent();
        let bad: Vec<(&str, Value)> = vec![
            ("unknown type", json!({"type":"x"})),
            (
                "bad dest",
                json!({"type":"factorio","dest":"example.com:1"}),
            ),
            (
                "bad dest port",
                json!({"type":"factorio","dest":"127.0.0.1:99999"}),
            ),
            (
                "wildcard dest",
                json!({"type":"factorio","dest":"0.0.0.0:34197"}),
            ),
            ("bad id", json!({"type":"factorio","id":"a/b"})),
            ("dotdot id", json!({"type":"factorio","id":".."})),
            ("uppercase id", json!({"type":"factorio","id":"ABC"})),
            (
                "proto mismatch",
                json!({"type":"factorio","protocol":"tcp"}),
            ),
            (
                "custom without protocol",
                json!({"type":"custom","tunnelPort":5000}),
            ),
            (
                "long name",
                json!({"type":"factorio","name":"n".repeat(65)}),
            ),
            (
                "long description",
                json!({"type":"factorio","description":"d".repeat(201)}),
            ),
            (
                "bridge on wrong game",
                json!({"type":"factorio","consoleBridge":{"serverIp":"127.0.0.1","bindIp":"127.0.0.1","mediaPortFirst":1,"mediaPortLast":2}}),
            ),
            (
                "bad bridge",
                json!({"type":"minecraft-bedrock","consoleBridge":{"serverIp":"nope"}}),
            ),
            (
                "Bedrock media ports on wrong game",
                json!({"type":"factorio","bedrock":{"mediaPortFirst":1000,"mediaPortLast":1001}}),
            ),
            (
                "Bedrock media ports with bridge",
                json!({"type":"minecraft-bedrock","consoleBridge":{"serverIp":"127.0.0.1","bindIp":"127.0.0.1","mediaPortFirst":1000,"mediaPortLast":1001},"bedrock":{"mediaPortFirst":2000,"mediaPortLast":2001}}),
            ),
            (
                "invalid Bedrock range",
                json!({"type":"minecraft-bedrock","bedrock":{"mediaPortFirst":0,"mediaPortLast":1}}),
            ),
            (
                "oversized Bedrock range",
                json!({"type":"minecraft-bedrock","bedrock":{"mediaPortFirst":1000,"mediaPortLast":1256}}),
            ),
        ];
        for (name, input) in bad {
            assert!(add_game(&dir, input).is_err(), "{name} accepted");
        }
        let no_region = |region: Value| {
            dispatch(
                "config.game_prepare",
                json!({"value":{"type":"factorio","tunnelPort":0},"region":region}),
            )
        };
        for region in [
            Value::Null,
            json!({"RegionID":0,"Nodes":[{}]}),
            json!({"RegionID":9,"Nodes":[]}),
        ] {
            assert_eq!(no_region(region).unwrap_err(), "a relay region is required");
        }
        assert!(
            approve(&dir, "nonexistent", KEY_A, "").is_err(),
            "approve for unknown game accepted"
        );
        let g = add_game(&dir, json!({"type":"factorio"})).unwrap();
        let id = g["id"].as_str().unwrap();
        for key in [
            "".to_owned(),
            "nodekey:00".into(),
            "hello".into(),
            format!("privkey:{}", "a".repeat(64)),
            format!("{KEY_A}; rm -rf /"),
            format!("nodekey:{}", "A".repeat(64)),
        ] {
            assert!(
                approve(&dir, id, &key, "").is_err(),
                "bad key {key:?} accepted"
            );
            assert!(mutate(&dir, "revoke", id, json!({"publicKey":key})).is_err());
        }
        assert!(approve(&dir, id, KEY_A, &"l".repeat(65)).is_err());
        assert!(approve(&dir, id, KEY_A, "tab\there").is_err());
        // A failed add leaves no key file behind.
        assert!(!dir.join("keys").join("a-b.json").exists());
        // A bedrock game may carry a valid bridge.
        let bedrock = add_game(&dir, json!({"type":"minecraft-bedrock","consoleBridge":{"serverIp":"127.0.0.1","bindIp":"127.0.0.1","mediaPortFirst":1000,"mediaPortLast":1255}})).unwrap();
        assert_eq!(bedrock["consoleBridge"]["mediaPortLast"], 1255);
        let bedrock = add_game(&dir, json!({"id":"bedrock-media","type":"minecraft-bedrock","bedrock":{"mediaPortFirst":19140,"mediaPortLast":19145}})).unwrap();
        assert_eq!(bedrock["bedrock"]["mediaPortFirst"], 19140);
        let persisted =
            dispatch("config.agent_load", json!({"path":dir.join("agent.json")})).unwrap();
        assert_eq!(persisted["games"][2]["bedrock"]["mediaPortLast"], 19145);
        assert!(persisted["games"][0].get("bedrock").is_none());
    }

    #[test]
    fn game_prepare_applies_defaults_and_normalises_destinations() {
        let g = prepare(json!({"type":"factorio"})).unwrap();
        assert_eq!(g["id"], "factorio");
        assert!(g["name"].as_str().unwrap().len() > 3);
        assert_eq!(g["tunnelPort"], 34197);
        assert_eq!(g["dest"], "127.0.0.1:34197");
        let g = prepare(json!({"type":"custom","protocol":"tcp","tunnelPort":5000,"dest":"localhost:6000","name":"  My Game!  "})).unwrap();
        assert_eq!(g["dest"], "127.0.0.1:6000");
        assert_eq!(g["tunnelPort"], 5000);
    }

    // TestAddGameSimpleLowercaseSlugsRemainDistinct
    #[test]
    fn slugs_use_go_simple_lowercase_and_stay_distinct() {
        let (_root, dir) = fresh_agent();
        for (name, id) in [("\u{130}World", "iworld"), ("\u{130} World", "i-world")] {
            let g = add_game(&dir, json!({"name":name,"type":"minecraft-java"})).unwrap();
            assert_eq!(g["id"], id, "{name}");
        }
        assert_eq!(load(&dir).unwrap()["games"].as_array().unwrap().len(), 2);
        assert_eq!(slugify("  --Hello,   World!!  "), "hello-world");
        assert_eq!(slugify("***"), "game");
        assert_eq!(slugify(""), "game");
        assert_eq!(slugify(&"a".repeat(40)), "a".repeat(32));
        assert_eq!(slugify(&format!("{}-b", "a".repeat(31))), "a".repeat(31));
        assert_eq!(slugify("caf\u{e9} 2"), "caf-2");
    }

    #[test]
    fn agent_limits_and_duplicate_ids() {
        let (_root, dir) = fresh_agent();
        let games: Vec<Value> = (0..32)
            .map(|i| json!({"id":format!("g{i}"),"name":"G","devices":[]}))
            .collect();
        fs::write(
            agent_json(&dir),
            serde_json::to_vec(&json!({"version":1,"games":games})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            add_game(&dir, json!({"type":"factorio"})).unwrap_err(),
            "too many games (limit 32)"
        );
        let devices: Vec<Value> = (0..256)
            .map(|i| json!({"publicKey":format!("nodekey:{i:064x}"),"label":""}))
            .collect();
        fs::write(
            agent_json(&dir),
            serde_json::to_vec(
                &json!({"version":1,"games":[{"id":"full","name":"G","devices":devices}]}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            approve(&dir, "full", KEY_B, "").unwrap_err(),
            "too many devices for this game (limit 256)"
        );
        // Re-approving an existing device is still allowed at the limit.
        assert!(approve(&dir, "full", &format!("nodekey:{:064x}", 5), "again").is_ok());
    }

    #[test]
    fn a_live_mutation_lock_cannot_be_stolen_when_its_file_ages() {
        let (_root, dir) = fresh_agent();
        let guard = lock_mutations(&dir).unwrap();
        let file = fs::OpenOptions::new()
            .write(true)
            .open(dir.join("agent.lock"))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        assert!(
            lock_mutations(&dir).is_err(),
            "active mutation lock was stolen"
        );
        drop(guard);
        lock_mutations(&dir).unwrap();
    }

    #[test]
    fn a_leftover_lock_file_does_not_block_mutations() {
        let (_root, dir) = fresh_agent();
        let lock_path = dir.join("agent.lock");
        let file = fs::File::create(&lock_path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        drop(file);
        add_game(&dir, json!({"type":"factorio"})).unwrap();
        drop(lock_mutations(&dir).expect("lock leaked"));
    }

    #[test]
    fn keys_load_validates_the_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.json");
        let load = |data: Option<&str>| {
            if let Some(data) = data {
                fs::write(&path, data).unwrap();
            }
            dispatch("config.keys_load", json!({"id":"g","path":&path}))
        };
        assert_eq!(load(None).unwrap(), json!({"missing":true}));
        assert!(load(Some(&keys().to_string())).unwrap()["keys"]["region"]["RegionID"] == 900);
        for (data, expect) in [
            ("{nope", "are corrupt: invalid JSON"),
            ("[]", "invalid field type or timestamp"),
            (
                r#"{"created":"yesterday","region":{}}"#,
                "invalid field type or timestamp",
            ),
            (r#"{"privateKey":5}"#, "invalid field type or timestamp"),
            (r#"{"presharedKey":5}"#, "invalid field type or timestamp"),
            (r#"{"privateKey":"x"}"#, "have no pinned relay region"),
            (
                r#"{"region":{"RegionID":1,"Nodes":[]}}"#,
                "have no pinned relay region",
            ),
            (
                r#"{"region":{"RegionID":0,"Nodes":[{}]}}"#,
                "have no pinned relay region",
            ),
        ] {
            let error = load(Some(data)).unwrap_err();
            assert!(error.contains(expect), "{data}: {error}");
            assert!(error.contains("\"g\""), "{error}");
        }
        let error = dispatch("config.keys_load", json!({"id":"g","path":dir.path()})).unwrap_err();
        assert!(error.starts_with("read keys for \"g\""), "{error}");
    }

    #[test]
    fn save_writes_the_exact_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("out.json");
        dispatch("config.save", json!({"path":path,"config":{"a":1}})).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap(),
            json!({"a":1})
        );
    }

    #[test]
    fn array_helper_replaces_non_arrays() {
        let mut v = json!("x");
        array(&mut v).push(json!(1));
        assert_eq!(v, json!([1]));
        array(&mut v).push(json!(2));
        assert_eq!(v, json!([1, 2]));
    }
}
