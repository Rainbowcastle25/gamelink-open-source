//! Desktop adapter for the shared host config and supervisor.
use crate::{bridge, lan};
use gamelink_core::{
    config, pure,
    requests::{Scope, Store},
    storage,
    supervisor::{self, Effect, Event, State},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    time::{Duration, Instant, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, String>;
type Connections = BTreeMap<String, BTreeMap<String, String>>;
type Scopes = BTreeMap<String, Scope>;
const LAN_ORIGIN: &str = "desktop-minecraft-lan";

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_default()
}

fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn destination(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

fn is_lan(game: &Value) -> bool {
    game["origin"] == LAN_ORIGIN
}

fn desktop_game_type(game: &str) -> Result<()> {
    if matches!(game, "minecraft-java" | "minecraft-bedrock" | "factorio") {
        Ok(())
    } else {
        Err("Choose a supported game".into())
    }
}

pub struct Hosting {
    dir: PathBuf,
    state: State,
    games: Value,
    statuses: BTreeMap<String, Value>,
    clock: Instant,
    next_poll: Instant,
    lan_deadline: Option<Instant>,
    lan_ports: BTreeMap<String, u16>,
    connections: Connections,
    connections_dirty: bool,
    requests: Store,
    scopes: Scopes,
    running_identities: BTreeMap<String, String>,
    issue: Option<String>,
}

impl Hosting {
    pub fn new(dir: PathBuf) -> Self {
        let clock = Instant::now();
        let connections = storage::read_bounded(&dir.join("connections.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        Self {
            dir,
            state: State::default(),
            games: json!([]),
            statuses: BTreeMap::new(),
            clock,
            next_poll: clock,
            lan_deadline: None,
            lan_ports: BTreeMap::new(),
            connections,
            connections_dirty: false,
            requests: Store::default(),
            scopes: Scopes::new(),
            running_identities: BTreeMap::new(),
            issue: None,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("agent.json")
    }

    fn now_ns(&self) -> i64 {
        self.clock.elapsed().as_nanos().min(i64::MAX as u128) as i64
    }

    pub fn pump(&mut self) {
        let now = Instant::now();
        if self.lan_deadline.is_some_and(|deadline| deadline <= now) {
            let _ = self.close_lan();
        }
        if now >= self.next_poll {
            self.refresh();
        }
    }

    fn refresh(&mut self) {
        self.next_poll = Instant::now() + Duration::from_secs(1);
        self.issue = self.reconcile().err();
    }

    fn keys_path(&self, id: &str) -> PathBuf {
        self.dir.join("keys").join(format!("{id}.json"))
    }

    fn keys(&self, id: &str) -> Result<Value> {
        let value = config::dispatch(
            "config.keys_load",
            json!({"path":self.keys_path(id),"id":id,"sealed":true}),
        )?;
        if value["missing"] == true {
            return Err("game keys are missing".into());
        }
        Ok(value["keys"].clone())
    }

    fn load_games(&self) -> Result<Value> {
        match self.path().try_exists() {
            Ok(false) => Ok(json!([])),
            Ok(true) => Ok(
                config::dispatch("config.agent_load", json!({"path":self.path()}))?["games"]
                    .clone(),
            ),
            Err(_) => Err("could not open desktop hosting configuration".into()),
        }
    }

    fn observe(&self, games: &Value) -> Result<(Event, Scopes)> {
        if rows(games).len() > 32 {
            return Err("Too many hosted games".into());
        }
        let mut ids = BTreeSet::new();
        let mut observed = Vec::new();
        let mut scopes = Scopes::new();
        for game in rows(games) {
            let id = text(game, "id");
            pure::dispatch("security.slug", json!({"text":id}))?;
            desktop_game_type(text(game, "type"))?;
            pure::dispatch(
                "games.check",
                json!({"game":game["type"],"proto":game["protocol"]}),
            )?;
            if !ids.insert(id) {
                return Err("duplicate hosted game ID".into());
            }
            let dest: SocketAddrV4 = text(game, "dest")
                .parse()
                .map_err(|_| "invalid hosted destination")?;
            if *dest.ip() != Ipv4Addr::LOCALHOST
                || dest.port() == 0
                || !game["consoleBridge"].is_null()
                || !game["bedrock"].is_null()
            {
                return Err("desktop games require a single 127.0.0.1 destination".into());
            }
            let mt = std::fs::metadata(self.keys_path(id))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|t| t.as_nanos().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            let mut approved_error = None;
            if rows(&game["devices"]).len() > 256 {
                return Err("Too many devices for a hosted game".into());
            }
            for device in rows(&game["devices"]) {
                if pure::dispatch("identity.public_key", json!({"text":device["publicKey"]}))
                    .is_err()
                {
                    approved_error = Some("A device has an invalid public key");
                    break;
                }
            }
            let mut observed_game = game.clone();
            if is_lan(game) && self.lan_ports.get(id) != Some(&dest.port()) {
                observed_game["disabled"] = json!(true);
            }
            let enabled = observed_game["disabled"] != true && approved_error.is_none();
            let mut observation = json!({
                "game":observed_game,"keyMtimeNs":mt,"approvedError":approved_error,"knockEnabled":true
            });
            if enabled {
                match self.keys(id) {
                    Ok(keys) => {
                        let expired = pure::check_invite_expiry(&keys["expiresAt"]).is_err();
                        let scope = Scope::new(game, keys);
                        observation["expired"] = json!(expired);
                        observation["keyIdentity"] = json!(format!(
                            "{}|{}",
                            scope.identity,
                            text(&scope.keys, "expiresAt")
                        ));
                        if !expired {
                            scopes.insert(id.to_owned(), scope);
                        }
                    }
                    Err(_) => {
                        observation["approvedError"] = json!("Could not unlock this game's keys")
                    }
                }
            }
            observed.push(observation);
        }
        let event = serde_json::from_value(
            json!({"type":"observe","games":observed,"nowNs":self.now_ns()}),
        )
        .map_err(|_| "invalid desktop hosting configuration")?;
        Ok((event, scopes))
    }

    fn reconcile(&mut self) -> Result<()> {
        let loaded = self
            .load_games()
            .and_then(|games| Ok((self.observe(&games)?, games)));
        let ((event, scopes), games, invalid) = match loaded {
            Ok((observation, games)) => (observation, games, false),
            Err(_) => (
                (
                    Event::InvalidConfig {
                        cause: "invalid desktop hosting configuration".into(),
                    },
                    Scopes::new(),
                ),
                Value::Null,
                true,
            ),
        };
        self.scopes = scopes;
        // Collect before closing a generation, keeping requests for the same
        // identity across approval, revocation and destination changes.
        let sampled = self.sample_peers();
        let change = supervisor::reduce(self.state.clone(), event)?;
        // All old generations complete closure before any replacement starts.
        self.stop_generations(&change.effects)?;
        self.state = change.state;
        for effect in change.effects {
            if let Effect::Start { id, generation } = effect {
                let error = self.start(&games, &id, generation).err();
                self.state = supervisor::reduce(
                    self.state.clone(),
                    Event::StartCompleted {
                        id,
                        generation,
                        error,
                        now_ns: self.now_ns(),
                    },
                )?
                .state;
            }
        }
        let current = supervisor::reduce(self.state.clone(), Event::Commit)?;
        self.state = current.state;
        self.statuses = current
            .games
            .into_iter()
            .map(|status| {
                let status = json!(status);
                (text(&status, "id").to_owned(), status)
            })
            .collect();
        if invalid {
            return Err("Desktop hosting stopped because its configuration is invalid".into());
        }
        self.games = games;
        self.save_connections();
        sampled
    }

    fn stop_generations(&mut self, effects: &[Effect]) -> Result<()> {
        for effect in effects {
            if let Effect::Stop { id, generation } = effect {
                let peers =
                    bridge::call(json!({"method":"host_stop","id":id,"generation":generation}))?;
                self.record_connections(id, &peers["connected"]);
                self.running_identities.remove(id);
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let change = supervisor::reduce(std::mem::take(&mut self.state), Event::Shutdown)?;
        let stopped = self.stop_generations(&change.effects);
        self.save_connections();
        stopped
    }

    fn start(&mut self, games: &Value, id: &str, generation: u64) -> Result<()> {
        let scope = self.scopes.get(id).ok_or("Game identity is unavailable")?;
        let game = rows(games)
            .iter()
            .find(|g| g["id"] == id)
            .ok_or("Hosted game was removed")?;
        let approved: Vec<_> = rows(&game["devices"])
            .iter()
            .map(|d| &d["publicKey"])
            .collect();
        bridge::call(json!({
            "method":"host_start","id":id,"generation":generation,"keys":scope.keys,"approved":approved,
            "game":{"type":game["type"],"protocol":game["protocol"],"tunnelPort":game["tunnelPort"],"dest":game["dest"]}
        })).map_err(|_| "Could not start the Tailcat game tunnel")?;
        self.running_identities
            .insert(id.to_owned(), scope.identity.clone());
        Ok(())
    }

    fn record_connections(&mut self, id: &str, peers: &Value) {
        if let Some(peers) = peers.as_object().filter(|p| !p.is_empty()) {
            let known = self.connections.entry(id.into()).or_default();
            for (key, time) in peers {
                if let Some(time) = time.as_str() {
                    if known.get(key).is_none_or(|last| last != time) {
                        known.insert(key.clone(), time.into());
                        self.connections_dirty = true;
                    }
                }
            }
        }
    }

    fn sample_peers(&mut self) -> Result<()> {
        let mut batches = BTreeMap::new();
        if !self.running_identities.is_empty() {
            if let Ok(peers) = bridge::call(json!({"method":"host_peers"})) {
                if let Some(peers) = peers.as_object() {
                    for (id, peer) in peers {
                        self.record_connections(id, &peer["connected"]);
                        if let Some(requests) = peer.get("requests") {
                            let batch = serde_json::from_value(requests.clone())
                                .map_err(|_| "invalid desktop join request batch")?;
                            batches.insert(id.clone(), batch);
                        }
                    }
                }
            }
        }
        self.requests.merge(
            &self.scopes,
            &self.running_identities,
            batches,
            chrono::Utc::now(),
        )
    }

    fn save_connections(&mut self) {
        self.connections.retain(|id, peers| {
            let game = rows(&self.games).iter().find(|g| g["id"] == id.as_str());
            let len = peers.len();
            peers.retain(|key, _| {
                game.is_some_and(|g| {
                    rows(&g["devices"])
                        .iter()
                        .any(|d| d["publicKey"] == key.as_str())
                })
            });
            self.connections_dirty |= peers.len() != len;
            !peers.is_empty()
        });
        if self.connections_dirty {
            let value =
                serde_json::to_vec(&self.connections).expect("connection timestamps serialize");
            if storage::write_atomic(&self.dir.join("connections.json"), &value).is_ok() {
                self.connections_dirty = false;
            }
        }
    }

    fn close_lan(&mut self) -> Result<Value> {
        self.lan_deadline = None;
        bridge::call(json!({"method":"lan_close"}))
    }

    fn worlds(&self, method: &str) -> Result<BTreeSet<lan::World>> {
        let packets = bridge::call(json!({"method":method}))?;
        Ok(rows(&packets)
            .iter()
            .filter_map(|p| lan::parse(p.as_str()?.as_bytes()))
            .collect())
    }

    pub fn command(&mut self, request: &Value, region: Option<&Value>) -> Result<Value> {
        let id = text(request, "id");
        match text(request, "method") {
            "scanLanWorlds" => {
                let worlds = self.worlds("lan_scan")?;
                self.lan_deadline = Some(Instant::now() + Duration::from_secs(5));
                return Ok(json!(worlds));
            }
            "stopLanScan" => return self.close_lan(),
            _ => {}
        }
        if matches!(
            text(request, "method"),
            "approveHostedRequest" | "dismissHostedRequest"
        ) {
            self.refresh();
        } else {
            self.pump();
        }
        if let Some(issue) = &self.issue {
            return Err(issue.clone());
        }
        match text(request, "method") {
            "hostedGames" => Ok(self.view()),
            "createHostedGame" | "shareLanWorld" => self.create(request, region),
            "approveHostedDevice" | "revokeHostedDevice" => {
                self.game(id)?;
                let action = if request["method"] == "approveHostedDevice" {
                    "approve"
                } else {
                    "revoke"
                };
                self.mutate(action, json!({"id":id,"publicKey":request["publicKey"],"label":request["label"],"now":chrono::Utc::now().to_rfc3339()}))?;
                Ok(Value::Null)
            }
            "approveHostedRequest" | "dismissHostedRequest" => self.resolve_request(id, request),
            "setHostingEnabled" => {
                let game = self.game(id)?;
                let enabled = request["enabled"]
                    .as_bool()
                    .ok_or("missing hosting state")?;
                if is_lan(game) && enabled {
                    return Err("Share this LAN world again after detecting it in Host".into());
                }
                self.lan_ports.remove(id);
                self.mutate(
                    "set_disabled",
                    json!({"id":id,"value":{"disabled":!enabled}}),
                )?;
                Ok(Value::Null)
            }
            "removeHostedGame" => {
                self.game(id)?;
                self.lan_ports.remove(id);
                self.mutate("remove", json!({"id":id}))?;
                Ok(Value::Null)
            }
            "hostedInvitation" => self.invitation(self.game(id)?),
            _ => Err("unknown desktop hosting command".into()),
        }
    }

    fn resolve_request(&mut self, id: &str, action: &Value) -> Result<Value> {
        let request_id = action["requestId"]
            .as_u64()
            .ok_or("missing join request ID")?;
        let now = chrono::Utc::now();
        let request = self
            .requests
            .list(&self.scopes, now)
            .into_iter()
            .find(|request| request["id"] == request_id && request["game"] == id)
            .ok_or("That join request is no longer pending")?;
        if action["method"] == "approveHostedRequest" {
            let keys = &self.scopes[id].keys;
            self.mutate(
                "approve",
                json!({
                    "id":id,"publicKey":request["publicKey"],"label":action["label"],
                    "now":now.to_rfc3339(),"expectedKeys":keys,"sealed":true
                }),
            )?;
        } else {
            self.requests.dismiss(id, request_id, now);
        }
        Ok(Value::Null)
    }

    fn game(&self, id: &str) -> Result<&Value> {
        rows(&self.games)
            .iter()
            .find(|g| g["id"] == id)
            .ok_or_else(|| "Hosted game was removed".into())
    }

    fn mutate(&mut self, action: &str, mut input: Value) -> Result<()> {
        input["action"] = json!(action);
        input["path"] = json!(self.path());
        let result = config::dispatch("config.agent_mutate", input);
        // Observe even a failed write. The key file or config may have changed.
        self.refresh();
        result?;
        self.issue
            .as_ref()
            .map_or(Ok(()), |issue| Err(issue.clone()))
    }

    fn create(&mut self, request: &Value, region: Option<&Value>) -> Result<Value> {
        let port = request["port"]
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|&p| p != 0)
            .ok_or("Enter a port from 1 to 65535")?;
        let name = text(request, "name").trim();
        let lan = request["method"] == "shareLanWorld";
        let game_type = if lan {
            "minecraft-java"
        } else {
            text(request, "game")
        };
        desktop_game_type(game_type)?;
        pure::dispatch(
            "security.display",
            json!({"field":"name","text":name,"maxRunes":64,"required":true}),
        )?;
        let dest = destination(port).to_string();
        if lan {
            if self.lan_deadline.is_none()
                || !self.worlds("lan_packets")?.contains(&lan::World {
                    name: name.into(),
                    port,
                })
            {
                return Err(
                    "This LAN announcement expired. Open the world to LAN and try again".into(),
                );
            }
            if let Some(game) = rows(&self.games)
                .iter()
                .find(|g| is_lan(g) && g["name"] == name)
            {
                let id = text(game, "id").to_owned();
                self.lan_ports.insert(id.clone(), port);
                self.mutate("set_destination", json!({"id":id,"value":{"dest":dest}}))?;
                return Ok(json!({"id":id}));
            }
        }
        if rows(&self.games).len() >= 32 {
            return Err("Too many hosted games (limit 32)".into());
        }
        let Some(region) = region else {
            return Ok(json!({"resolveRelay":true}));
        };
        if !self
            .path()
            .try_exists()
            .map_err(|_| "could not open desktop hosting configuration")?
        {
            config::dispatch(
                "config.agent_mutate",
                json!({"action":"init","path":self.path()}),
            )?;
        }
        let id = config::dispatch("config.new_id", Value::Null)?;
        let mut game = config::dispatch(
            "config.game_prepare",
            json!({
                "value":{"id":id,"name":name,"type":game_type,"tunnelPort":0,"dest":dest},"region":region
            }),
        )?;
        let mut keys = bridge::call(json!({"method":"host_generate"}))?;
        keys["created"] = json!(chrono::Utc::now().to_rfc3339());
        keys["region"] = region.clone();
        game["created"] = keys["created"].clone();
        game["devices"] = json!([]);
        if lan {
            game["origin"] = json!(LAN_ORIGIN);
        }
        self.mutate("add", json!({"value":game,"keys":keys,"sealed":true}))?;
        if lan {
            self.lan_ports
                .insert(id.as_str().ok_or("invalid hosted ID")?.into(), port);
            self.refresh();
        }
        Ok(json!({"id":id}))
    }

    fn invitation(&self, game: &Value) -> Result<Value> {
        let keys = self
            .keys(text(game, "id"))
            .map_err(|_| "Could not unlock this game's keys")?;
        pure::check_invite_expiry(&keys["expiresAt"])?;
        let addr = bridge::call(json!({"method":"host_address","keys":keys}))
            .map_err(|_| "Could not create the invitation for this game's relay")?;
        let mut invite = json!({"v":1,"name":game["name"],"game":game["type"],"proto":game["protocol"],"port":game["tunnelPort"],"addr":addr});
        if let Some(expires) = keys.get("expiresAt") {
            invite["expiresAt"] = expires.clone();
        }
        pure::dispatch("invite.fields", invite.clone())?;
        pure::dispatch("invite.encode", invite)
    }

    fn view(&self) -> Value {
        let mut pending_by_game: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for request in self.requests.list(&self.scopes, chrono::Utc::now()) {
            pending_by_game
                .entry(text(&request, "game").into())
                .or_default()
                .push(request);
        }
        let mut result = Vec::new();
        for game in rows(&self.games) {
            let id = text(game, "id");
            let status = &self.statuses[id];
            let approved: Vec<_> = rows(&game["devices"])
                .iter()
                .map(|d| {
                    let key = text(d, "publicKey");
                    let last = self.connections.get(id).and_then(|peers| peers.get(key));
                    json!({"publicKey":key,"label":text(d, "label"),"lastConnected":last})
                })
                .collect();
            let pending = pending_by_game.remove(id).unwrap_or_default();
            let port = text(game, "dest")
                .parse::<SocketAddrV4>()
                .expect("observed loopback destination")
                .port();
            let enabled = game["disabled"] != true
                && (!is_lan(game) || self.lan_ports.get(id) == Some(&port));
            let detail = if is_lan(game) && !enabled {
                "Open this world to LAN and share it again"
            } else if !enabled {
                "Sharing paused"
            } else {
                text(status, "detail")
            };
            result.push(json!({
                "id":id,"name":game["name"],"game":game["type"],"port":port,"lan":is_lan(game),
                "enabled":enabled,"state":status["state"],"detail":detail,"approved":approved,"pending":pending
            }));
        }
        json!(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn hosted_lan_revocation_replacement_and_restart_use_shared_policy() {
        let dir = tempfile::tempdir().unwrap();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let packet = Rc::new(RefCell::new("[MOTD]Cabin[/MOTD][AD]51000[/AD]".to_owned()));
        let batch = Rc::new(RefCell::new(Value::Null));
        let calls = requests.clone();
        let announcement = packet.clone();
        let knocks = batch.clone();
        let _fake = bridge::fake::install(move |request| {
            calls.borrow_mut().push(request.clone());
            Ok(match text(request, "method") {
                "lan_scan" | "lan_packets" => json!([*announcement.borrow()]),
                "host_generate" => {
                    let secret = config::dispatch("config.new_id", Value::Null)?;
                    json!({"privateKey":secret,"presharedKey":secret})
                }
                "host_address" => json!("test-tailcat-address"),
                "host_peers" => std::mem::take(&mut *knocks.borrow_mut()),
                _ => Value::Null,
            })
        });
        let host_dir = dir.path().join("hosting");
        let mut hosting = Hosting::new(host_dir.clone());
        let region = json!({"RegionID":1,"Nodes":[{}]});

        let share = |port| json!({"method":"shareLanWorld","name":"Cabin","port":port});
        let snapshots = json!({"method":"hostedGames"});
        let scan = json!({"method":"scanLanWorlds"});
        assert!(hosting.command(&share(51000), Some(&region)).is_err());
        hosting.command(&scan, None).unwrap();
        let new = hosting.command(&share(51000), Some(&region)).unwrap();
        let id = text(&new, "id");
        assert_eq!(
            hosting.command(&snapshots, None).unwrap()[0]["state"],
            "inactive"
        );
        let start = requests
            .borrow()
            .iter()
            .find(|call| call["method"] == "host_start")
            .unwrap()
            .clone();
        assert_eq!(start["approved"], json!([]));
        assert!(hosting.command(&snapshots, None).unwrap()[0]["detail"]
            .as_str()
            .unwrap()
            .contains("waiting for join requests"));

        let game_key = hosting.keys(id).unwrap();
        let raw = std::fs::read_to_string(hosting.keys_path(id)).unwrap();
        assert!(!raw.contains("privateKey"));
        assert!(!raw.contains(text(&game_key, "privateKey")));
        let invite = hosting
            .command(&json!({"method":"hostedInvitation","id":id}), None)
            .unwrap();
        assert!(text(&json!({"invite":invite}), "invite").starts_with("glink1."));
        assert!(!hosting
            .command(&snapshots, None)
            .unwrap()
            .to_string()
            .contains("glink1."));

        let key = generated_public_key();
        let other = generated_public_key();
        let now = chrono::Utc::now();
        let queue = |entries: Vec<(&str, chrono::DateTime<chrono::Utc>)>| {
            let knocks: Vec<_> = entries
                .into_iter()
                .map(|(key, at)| json!({"publicKey":key,"firstSeen":at,"lastSeen":at,"attempts":1}))
                .collect();
            *batch.borrow_mut() = json!({(id):{"connected":{},"requests":knocks}});
        };
        queue(vec![(&key, now), (&other, now)]);
        hosting.refresh();
        let games = hosting.command(&snapshots, None).unwrap();
        let pending = rows(&games[0]["pending"]);
        assert_eq!(pending.len(), 2);
        assert!(rows(&games[0]["approved"]).is_empty());
        let request_id =
            |key: &str| pending.iter().find(|r| r["publicKey"] == key).unwrap()["id"].clone();
        let dismissed = request_id(&other);
        let approve = |request_id: &Value| json!({"method":"approveHostedRequest","id":id,"requestId":request_id,"label":"Friend"});
        hosting
            .command(
                &json!({"method":"dismissHostedRequest","id":id,"requestId":dismissed}),
                None,
            )
            .unwrap();
        queue(vec![(&other, now)]);
        hosting.refresh();
        assert!(hosting.command(&approve(&dismissed), None).is_err());
        assert_eq!(
            hosting.command(&snapshots, None).unwrap()[0]["pending"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let later = chrono::Utc::now();
        queue(vec![(&other, later)]);
        hosting.refresh();
        hosting.command(&approve(&request_id(&key)), None).unwrap();
        let games = hosting.command(&snapshots, None).unwrap();
        assert_eq!(games[0]["state"], "serving");
        assert_eq!(games[0]["approved"][0]["publicKey"], key);
        let remaining = rows(&games[0]["pending"]);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0]["publicKey"], other);
        assert_ne!(remaining[0]["id"], dismissed);
        assert!(hosting.command(&approve(&request_id(&key)), None).is_err());

        *packet.borrow_mut() = "[MOTD]Cabin[/MOTD][AD]52000[/AD]".into();
        assert!(hosting.command(&share(51000), None).is_err());
        assert_eq!(hosting.command(&share(52000), None).unwrap()["id"], id);
        let game = &hosting.command(&snapshots, None).unwrap()[0];
        assert_eq!(game["port"], 52000);
        assert_eq!(game["approved"][0]["publicKey"], key);
        assert_eq!(hosting.keys(id).unwrap(), game_key);
        assert_eq!(
            hosting
                .command(&json!({"method":"hostedInvitation","id":id}), None)
                .unwrap(),
            invite
        );

        let action = json!({"method":"revokeHostedDevice","id":id,"publicKey":key});
        hosting.command(&action, None).unwrap();
        assert_eq!(
            hosting.command(&snapshots, None).unwrap()[0]["state"],
            "inactive"
        );
        let observed: Vec<_> = requests
            .borrow()
            .iter()
            .filter(|call| matches!(text(call, "method"), "host_start" | "host_stop"))
            .cloned()
            .collect();
        assert_eq!(
            observed
                .iter()
                .map(|call| text(call, "method"))
                .collect::<Vec<_>>(),
            [
                "host_start",
                "host_stop",
                "host_start",
                "host_stop",
                "host_start",
                "host_stop",
                "host_start"
            ]
        );
        assert_eq!(observed[0]["game"]["dest"], "127.0.0.1:51000");
        assert_eq!(observed[1]["generation"], observed[0]["generation"]);
        assert_eq!(observed[2]["approved"], json!([key]));
        assert_eq!(observed[3]["generation"], observed[2]["generation"]);
        assert_eq!(observed[4]["game"]["dest"], "127.0.0.1:52000");
        assert_eq!(
            observed[4]["game"]["tunnelPort"],
            observed[0]["game"]["tunnelPort"]
        );
        assert_eq!(observed[5]["generation"], observed[4]["generation"]);
        assert_eq!(observed[6]["approved"], json!([]));

        let mut reopened = Hosting::new(host_dir);
        let saved = reopened.command(&snapshots, None).unwrap();
        assert_eq!(saved[0]["id"], id);
        assert_eq!(saved[0]["enabled"], false);
    }

    fn generated_public_key() -> String {
        let mut key = String::from("nodekey:");
        for _ in 0..6 {
            key.push_str(
                config::dispatch("config.new_id", Value::Null)
                    .unwrap()
                    .as_str()
                    .unwrap(),
            );
        }
        key.truncate("nodekey:".len() + 64);
        key
    }

    #[test]
    fn a_hosted_game_cannot_reconcile_to_a_non_loopback_destination() {
        let host = Hosting::new(PathBuf::new());
        assert!(host
            .observe(&json!([{
                "id":"game","name":"Example","type":"factorio","protocol":"udp",
                "tunnelPort":34197,"dest":"192.0.2.2:34197","devices":[]
            }]))
            .is_err());
    }
}
