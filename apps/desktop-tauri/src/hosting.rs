//! Desktop adapter for the shared host config and supervisor.
use crate::bridge;
use gamelink_core::{
    config, pure,
    requests::{Scope, Store},
    storage,
    supervisor::{self, Effect, Event, State},
};
use serde::{Deserialize, Serialize};
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
const REQUESTS_FILE: &str = "requests.json";

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct LanWorld {
    name: String,
    port: u16,
}

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
    lan_open: bool,
    // Host-confirmed LAN shares and the IDs that were live at reconciliation.
    lan_ports: BTreeMap<String, u16>,
    lan_live: BTreeSet<String>,
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
        // Pending requests stay in memory; dismissals last until rotation.
        let requests = storage::read_bounded(&dir.join(REQUESTS_FILE))
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
            lan_open: false,
            lan_ports: BTreeMap::new(),
            lan_live: BTreeSet::new(),
            connections,
            connections_dirty: false,
            requests,
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
            self.lan_deadline = None;
            self.close_idle_lan();
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

    fn observe(&self, games: &Value, lan_live: &BTreeSet<String>) -> Result<(Event, Scopes)> {
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
            if is_lan(game) && !lan_live.contains(id) {
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
                        let mut fingerprint =
                            format!("{}|{}", scope.identity, text(&scope.keys, "expiresAt"));
                        // The running tunnel's dial guard also depends on the LAN name.
                        if is_lan(game) {
                            fingerprint.push_str("|lan=");
                            fingerprint.push_str(text(game, "name"));
                        }
                        observation["keyIdentity"] = json!(fingerprint);
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

    fn observe_config(
        &mut self,
        loaded: Result<Value>,
        worlds: &BTreeSet<LanWorld>,
    ) -> (Event, Value, BTreeSet<String>, bool) {
        let lan_live = self.live_lan(loaded.as_ref().unwrap_or(&Value::Null), worlds);
        let loaded = loaded.and_then(|games| Ok((self.observe(&games, &lan_live)?, games)));
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
        (event, games, lan_live, invalid)
    }

    fn reconcile(&mut self) -> Result<()> {
        let loaded = self.load_games();
        let worlds = self.observe_lan(loaded.as_ref().unwrap_or(&Value::Null));
        let (event, mut games, mut lan_live, mut invalid) = self.observe_config(loaded, &worlds);
        // Collect before closing a generation, keeping requests for the same
        // identity across approval, revocation and destination changes.
        let sampled = self.sample_peers();
        let mut change = supervisor::reduce(self.state.clone(), event)?;
        loop {
            if !change.has_grants() {
                self.stop_generations(&change.effects)?;
                self.state = change.state;
                break;
            }
            let Some(lock) = config::try_lock_mutations(&self.dir)? else {
                self.stop_generations(&change.effects)?;
                self.save_connections();
                return sampled;
            };
            let mut event;
            (event, games, lan_live, invalid) = self.observe_config(self.load_games(), &worlds);
            change = supervisor::reduce(self.state.clone(), event)?;
            if !change.has_grants() {
                drop(lock);
                continue;
            }
            if change.has_stops() {
                // Close all old generations before replanning any grants.
                drop(lock);
                self.stop_generations(&change.effects)?;
            } else {
                self.state = change.state;
                let followups = self.apply_grants(change.effects, &games)?;
                drop(lock);
                if !self.stop_generations(&followups)? {
                    break;
                }
            }
            (event, games, lan_live, invalid) = self.observe_config(self.load_games(), &worlds);
            change = supervisor::reduce(self.state.clone(), event)?;
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
        self.lan_live = lan_live;
        self.save_connections();
        sampled
    }

    fn apply_grants(&mut self, effects: Vec<Effect>, games: &Value) -> Result<Vec<Effect>> {
        let mut followups = Vec::new();
        for effect in effects {
            let completed = match effect {
                Effect::Start { id, generation } => {
                    let error = self.start(games, &id, generation).err();
                    Event::StartCompleted {
                        id,
                        generation,
                        error,
                        now_ns: self.now_ns(),
                    }
                }
                Effect::Allow {
                    id,
                    generation,
                    approved,
                } => {
                    let error = bridge::call(
                        json!({"method":"host_allow","id":id,"generation":generation,"approved":approved}),
                    )
                    .err();
                    Event::AllowCompleted {
                        id,
                        generation,
                        error,
                    }
                }
                Effect::Stop { .. } | Effect::Log { .. } => continue,
            };
            followups.extend(self.state.complete_grant(completed)?);
        }
        Ok(followups)
    }

    fn stop_generations(&mut self, effects: &[Effect]) -> Result<bool> {
        let mut stopped = false;
        for effect in effects {
            if let Effect::Stop { id, generation } = effect {
                let peers =
                    bridge::call(json!({"method":"host_stop","id":id,"generation":generation}))?;
                self.record_connections(id, &peers["connected"]);
                self.running_identities.remove(id);
                self.state.record_stop(id, *generation);
                stopped = true;
            }
        }
        Ok(stopped)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let change = supervisor::reduce(std::mem::take(&mut self.state), Event::Shutdown)?;
        let stopped = self.stop_generations(&change.effects);
        self.save_connections();
        stopped.map(|_| ())
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
        let mut request = json!({
            "method":"host_start","id":id,"generation":generation,"keys":scope.keys,"approved":approved,
            "game":{"type":game["type"],"protocol":game["protocol"],"tunnelPort":game["tunnelPort"],"dest":game["dest"]}
        });
        if is_lan(game) {
            request["game"]["lanName"] = game["name"].clone();
        }
        bridge::call(request).map_err(|_| "Could not start the Tailcat game tunnel")?;
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

    fn observe_lan(&mut self, games: &Value) -> BTreeSet<LanWorld> {
        let shared = |id: &str| {
            rows(games)
                .iter()
                .find(|game| is_lan(game) && game["id"] == id && game["disabled"] != true)
        };
        self.lan_ports.retain(|id, port| {
            shared(id).is_some_and(|game| {
                text(game, "dest")
                    .parse::<SocketAddrV4>()
                    .is_ok_and(|dest| dest.port() == *port)
            })
        });
        if self.lan_ports.is_empty() {
            self.close_idle_lan();
            return BTreeSet::new();
        }
        // Discover worlds even while their shares are paused. Go uses the
        // same announcement deadline here and immediately before dialing.
        let Ok(worlds) = Self::worlds("lan_scan", true) else {
            self.lan_open = false;
            return BTreeSet::new();
        };
        self.lan_open = true;
        worlds
    }

    fn live_lan(&self, games: &Value, worlds: &BTreeSet<LanWorld>) -> BTreeSet<String> {
        rows(games)
            .iter()
            .filter_map(|game| {
                let id = text(game, "id");
                let &port = self.lan_ports.get(id)?;
                let dest = text(game, "dest").parse::<SocketAddrV4>().ok()?;
                if !is_lan(game) || game["disabled"] == true || dest.port() != port {
                    return None;
                }
                let name = text(game, "name").into();
                worlds
                    .contains(&LanWorld { name, port })
                    .then(|| id.to_owned())
            })
            .collect()
    }

    fn close_idle_lan(&mut self) {
        if self.lan_open && self.lan_deadline.is_none() && self.lan_ports.is_empty() {
            self.lan_open = bridge::call(json!({"method":"lan_close"})).is_err();
        }
    }

    fn worlds(method: &str, sharing: bool) -> Result<BTreeSet<LanWorld>> {
        let worlds = bridge::call(json!({"method":method,"sharing":sharing}))?;
        serde_json::from_value(worlds)
            .map_err(|_| "invalid Minecraft LAN discovery response".into())
    }

    pub fn command(&mut self, request: &Value, region: Option<&Value>) -> Result<Value> {
        let id = text(request, "id");
        match text(request, "method") {
            "scanLanWorlds" => {
                let worlds = Self::worlds("lan_scan", false)?;
                self.lan_open = true;
                self.lan_deadline = Some(Instant::now() + Duration::from_secs(5));
                return Ok(json!(worlds));
            }
            "stopLanScan" => {
                self.lan_deadline = None;
                self.close_idle_lan();
                return Ok(Value::Null);
            }
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
                self.close_idle_lan();
                Ok(Value::Null)
            }
            "removeHostedGame" => {
                self.game(id)?;
                self.lan_ports.remove(id);
                self.mutate("remove", json!({"id":id}))?;
                self.close_idle_lan();
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
            let saved = serde_json::to_vec(&self.requests.dismissals())
                .map_err(|_| "could not save the dismissal")?;
            storage::write_atomic(&self.dir.join(REQUESTS_FILE), &saved)
                .map_err(|_| "Dismissed until GameLink restarts; could not save it")?;
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
        let approval_id = (action == "approve").then(|| text(&input, "id").to_owned());
        let result = config::dispatch("config.agent_mutate", input);
        // Observe even a failed write. The key file or config may have changed.
        self.refresh();
        result?;
        if let Some(issue) = &self.issue {
            return Err(issue.clone());
        }
        if let Some(status) = approval_id
            .and_then(|id| self.statuses.get(&id))
            .filter(|status| status["state"] == "error")
        {
            return Err(text(status, "detail").into());
        }
        Ok(())
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
                || !Self::worlds("lan_packets", false)?.contains(&LanWorld {
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
            let shared = game["disabled"] != true
                && (!is_lan(game) || self.lan_ports.get(id) == Some(&port));
            let paused = shared && is_lan(game) && !self.lan_live.contains(id);
            let detail = if paused {
                "World closed or not announced; sharing resumes if it reopens on the same port"
            } else if is_lan(game) && !shared {
                "Open this world to LAN and share it again"
            } else if !shared {
                "Sharing paused"
            } else {
                text(status, "detail")
            };
            let fingerprint = self
                .keys(id)
                .and_then(|keys| bridge::call(json!({"method":"host_public_key","keys":keys})))
                .ok()
                .and_then(|key| {
                    key.as_str()
                        .map(gamelink_core::requests::server_fingerprint)
                });
            result.push(json!({
                "serverFingerprint":fingerprint,"id":id,"name":game["name"],"game":game["type"],"port":port,"lan":is_lan(game),
                "enabled":shared,"paused":paused,"state":status["state"],"detail":detail,"approved":approved,"pending":pending
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
        let worlds = Rc::new(RefCell::new(json!([{"name":"Cabin","port":51000}])));
        let batch = Rc::new(RefCell::new(Value::Null));
        let calls = requests.clone();
        let announcements = worlds.clone();
        let knocks = batch.clone();
        let _fake = bridge::fake::install(move |request| {
            calls.borrow_mut().push(request.clone());
            Ok(match text(request, "method") {
                "lan_scan" | "lan_packets" => announcements.borrow().clone(),
                "host_generate" => {
                    let secret = config::dispatch("config.new_id", Value::Null)?;
                    json!({"privateKey":secret,"presharedKey":secret})
                }
                "host_address" => json!("test-tailcat-address"),
                "host_public_key" => json!("nodekey:hostedfullkey"),
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
        // The shim clears its batch every poll; a dismissed key knocking again
        // later stays ignored until the invitation is rotated.
        assert!(rows(&games[0]["pending"]).is_empty());
        assert!(hosting.command(&approve(&request_id(&key)), None).is_err());
        let saved = std::fs::read_to_string(host_dir.join(REQUESTS_FILE)).unwrap();
        assert!(saved.contains(&other) && !saved.contains(&key));
        assert!(!games.to_string().contains("hostedfullkey"));
        assert_eq!(
            games[0]["serverFingerprint"],
            gamelink_core::requests::server_fingerprint("nodekey:hostedfullkey")
        );

        *worlds.borrow_mut() = json!([{"name":"Cabin","port":52000}]);
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
        assert_eq!(observed[0]["game"]["lanName"], "Cabin");
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

        // A config reload must replace the dial guard when the LAN name changes.
        let path = hosting.path();
        let mut config: Value =
            serde_json::from_slice(&storage::read_bounded(&path).unwrap()).unwrap();
        config["games"][0]["name"] = json!("Renamed Cabin");
        storage::write_atomic(&path, &serde_json::to_vec(&config).unwrap()).unwrap();
        *worlds.borrow_mut() = json!([
            {"name":"Cabin","port":52000},
            {"name":"Renamed Cabin","port":52000},
        ]);
        hosting.refresh();
        let calls = requests.borrow();
        let replacement: Vec<_> = calls
            .iter()
            .filter(|call| matches!(text(call, "method"), "host_start" | "host_stop"))
            .skip(observed.len())
            .collect();
        assert_eq!(replacement.len(), 2);
        assert_eq!(replacement[0]["method"], "host_stop");
        assert_eq!(replacement[1]["game"]["lanName"], "Renamed Cabin");
        drop(calls);

        let mut reopened = Hosting::new(host_dir);
        assert!(reopened.requests.dismissals() == hosting.requests.dismissals());
        let saved = reopened.command(&snapshots, None).unwrap();
        assert_eq!(saved[0]["id"], id);
        assert_eq!(saved[0]["enabled"], false);
    }

    #[test]
    fn closed_lan_world_pauses_and_resumes_only_on_its_confirmed_port() {
        let dir = tempfile::tempdir().unwrap();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let port = Rc::new(RefCell::new(Some(51000)));
        let fail_stop = Rc::new(std::cell::Cell::new(false));
        let reject_stop = fail_stop.clone();
        let calls = requests.clone();
        let announcement = port.clone();
        let _fake = bridge::fake::install(move |request| {
            calls.borrow_mut().push(text(request, "method").to_owned());
            Ok(match text(request, "method") {
                "lan_scan" | "lan_packets" => match *announcement.borrow() {
                    Some(port) => json!([{"name":"Cabin","port":port}]),
                    None => json!([]),
                },
                "host_stop" if reject_stop.get() => return Err("test stop failure".into()),
                "host_generate" => {
                    let secret = config::dispatch("config.new_id", Value::Null)?;
                    json!({"privateKey":secret,"presharedKey":secret})
                }
                _ => Value::Null,
            })
        });
        let count = |method: &str| requests.borrow().iter().filter(|m| *m == method).count();
        let mut hosting = Hosting::new(dir.path().join("hosting"));
        let region = json!({"RegionID":1,"Nodes":[{}]});
        let scan = json!({"method":"scanLanWorlds"});
        let leave = json!({"method":"stopLanScan"});
        let snapshot = |hosting: &mut Hosting| {
            hosting.refresh();
            hosting
                .command(&json!({"method":"hostedGames"}), None)
                .unwrap()[0]
                .clone()
        };

        // Closing Host with nothing shared stops discovery.
        hosting.command(&scan, None).unwrap();
        hosting.command(&leave, None).unwrap();
        assert_eq!(count("lan_close"), 1);

        hosting.command(&scan, None).unwrap();
        let share = json!({"method":"shareLanWorld","name":"Cabin","port":51000});
        let id = text(&hosting.command(&share, Some(&region)).unwrap(), "id").to_owned();
        hosting.command(&leave, None).unwrap();
        assert_eq!(count("lan_close"), 1, "a shared world keeps discovery open");
        let scans = count("lan_scan");
        let game = snapshot(&mut hosting);
        assert!(
            count("lan_scan") > scans,
            "discovery keeps receiving after Host closes"
        );
        assert_eq!(
            (game["enabled"].clone(), game["paused"].clone()),
            (json!(true), json!(false))
        );
        assert_eq!(count("host_start"), 1);

        // The world closes: no announcement for the whole grace period.
        *port.borrow_mut() = None;
        let game = snapshot(&mut hosting);
        assert_eq!(
            count("host_stop"),
            1,
            "the tunnel to the closed world stops"
        );
        assert_eq!(
            (game["enabled"].clone(), game["paused"].clone()),
            (json!(true), json!(true))
        );
        assert!(text(&game, "detail").contains("resumes if it reopens on the same port"));
        assert_eq!(game["state"], "inactive");

        // Another world on a new port needs the host's click.
        *port.borrow_mut() = Some(52000);
        assert_eq!(snapshot(&mut hosting)["paused"], true);
        assert_eq!(count("host_start"), 1);

        // The same world on the same port resumes by itself.
        *port.borrow_mut() = Some(51000);
        let game = snapshot(&mut hosting);
        assert_eq!(game["paused"], false);
        assert_eq!(count("host_start"), 2);

        let approve = json!({
            "method":"approveHostedDevice","id":id,"publicKey":generated_public_key(),"label":"Friend"
        });
        hosting.command(&approve, None).unwrap();
        assert_eq!(hosting.view()[0]["state"], "serving");

        // An expired announcement only changes the published state after the tunnel stops.
        *port.borrow_mut() = None;
        assert_eq!(hosting.view()[0]["paused"], false);
        fail_stop.set(true);
        hosting.refresh();
        assert_eq!(hosting.issue.as_deref(), Some("test stop failure"));
        let still_serving = &hosting.view()[0];
        assert_eq!(still_serving["state"], "serving");
        assert_eq!(still_serving["paused"], false);
        fail_stop.set(false);
        let paused = snapshot(&mut hosting);
        assert_eq!(paused["state"], "inactive");
        assert_eq!(paused["paused"], true);
        assert_eq!(count("lan_close"), 1, "a paused share keeps discovery open");

        // Stop sharing while paused with Host closed stops discovery.
        let stop = json!({"method":"setHostingEnabled","id":id,"enabled":false});
        hosting.command(&stop, None).unwrap();
        assert_eq!(count("lan_close"), 2);
        let scans = count("lan_scan");
        assert_eq!(snapshot(&mut hosting)["enabled"], false);
        assert_eq!(count("lan_scan"), scans, "nothing shared and Host closed");
    }

    #[test]
    fn live_approval_uses_the_current_generation_and_restarts_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosting = Hosting::new(dir.path().join("hosting"));
        let lock_path = hosting.dir.join("agent.lock");
        let requests = Rc::new(RefCell::new(Vec::new()));
        let calls = requests.clone();
        let fail_allow = Rc::new(std::cell::Cell::new(false));
        let reject_allow = fail_allow.clone();
        let fail_stop = Rc::new(std::cell::Cell::new(false));
        let reject_stop = fail_stop.clone();
        let _fake = bridge::fake::install(move |request| {
            let method = text(request, "method");
            if matches!(method, "host_start" | "host_stop" | "host_allow") {
                calls.borrow_mut().push(request.clone());
            }
            Ok(match method {
                "host_generate" => {
                    let secret = config::dispatch("config.new_id", Value::Null)?;
                    json!({"privateKey":secret,"presharedKey":secret})
                }
                "host_allow" if reject_allow.get() => return Err("test allow failure".into()),
                "host_stop" if reject_stop.get() => return Err("test stop failure".into()),
                "host_stop" => {
                    let lock = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&lock_path)
                        .unwrap();
                    lock.try_lock().expect("held the config lock during stop");
                    lock.unlock().unwrap();
                    Value::Null
                }
                _ => Value::Null,
            })
        });
        let game = hosting.command(
            &json!({"method":"createHostedGame","game":"minecraft-java","name":"Game","port":25565}),
            Some(&json!({"RegionID":1,"Nodes":[{}]})),
        ).unwrap();
        let id = text(&game, "id");
        let approve = |key: &str| json!({"method":"approveHostedDevice","id":id,"publicKey":key,"label":"Friend"});
        let alice = generated_public_key();
        let bob = generated_public_key();
        hosting.command(&approve(&alice), None).unwrap();
        requests.borrow_mut().clear();
        hosting.command(&approve(&bob), None).unwrap();
        assert_eq!(
            *requests.borrow(),
            [json!({"method":"host_allow","id":id,"generation":2,"approved":[bob]})]
        );
        assert_eq!(hosting.view()[0]["state"], "serving");

        requests.borrow_mut().clear();
        fail_allow.set(true);
        let carol = generated_public_key();
        hosting.command(&approve(&carol), None).unwrap();
        let observed = requests.borrow();
        assert_eq!(observed.len(), 3);
        assert_eq!(
            observed[0],
            json!({"method":"host_allow","id":id,"generation":2,"approved":[carol]})
        );
        assert_eq!(
            observed[1],
            json!({"method":"host_stop","id":id,"generation":2})
        );
        assert_eq!(observed[2]["method"], "host_start");
        assert_eq!(observed[2]["generation"], 3);
        assert_eq!(observed[2]["approved"], json!([alice, bob, carol]));
        drop(observed);

        requests.borrow_mut().clear();
        fail_stop.set(true);
        let saved = storage::read_bounded(&hosting.path()).unwrap();
        assert_eq!(
            hosting
                .command(&approve(&generated_public_key()), None)
                .unwrap_err(),
            "test stop failure"
        );
        assert_eq!(requests.borrow().len(), 2);
        // The failed update may have applied. Even reverting the config
        // requires closing that generation before it can serve again.
        storage::write_atomic(&hosting.path(), &saved).unwrap();
        fail_stop.set(false);
        hosting.reconcile().unwrap();
        let observed = requests.borrow();
        assert_eq!(observed.len(), 4);
        assert_eq!(
            observed[2],
            json!({"method":"host_stop","id":id,"generation":3})
        );
        assert_eq!(observed[3]["method"], "host_start");
        assert_eq!(observed[3]["generation"], 4);
        assert_eq!(observed[3]["approved"], json!([alice, bob, carol]));
    }

    #[test]
    fn revoked_hosted_device_is_never_added_from_a_stale_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosting = Hosting::new(dir.path().join("hosting"));
        let path = hosting.path();
        let revoke = Rc::new(RefCell::new(None));
        let pending_revoke = revoke.clone();
        let _fake = bridge::fake::install(move |request| match text(request, "method") {
            "host_generate" => {
                let secret = config::dispatch("config.new_id", Value::Null)?;
                Ok(json!({"privateKey":secret,"presharedKey":secret}))
            }
            "host_peers" => {
                // Peers are sampled after the config read and before applying effects.
                if let Some(mutation) = pending_revoke.borrow_mut().take() {
                    config::dispatch("config.agent_mutate", mutation)?;
                }
                Ok(Value::Null)
            }
            "host_allow" => panic!("allowed a device from a stale snapshot"),
            _ => Ok(Value::Null),
        });
        let game = hosting.command(
            &json!({"method":"createHostedGame","game":"minecraft-java","name":"Game","port":25565}),
            Some(&json!({"RegionID":1,"Nodes":[{}]})),
        ).unwrap();
        let id = text(&game, "id");
        let alice = generated_public_key();
        let bob = generated_public_key();
        hosting
            .command(
                &json!({"method":"approveHostedDevice","id":id,"publicKey":alice,"label":"Alice"}),
                None,
            )
            .unwrap();

        config::dispatch(
            "config.agent_mutate",
            json!({"path":path,"action":"approve","id":id,"publicKey":bob,"now":chrono::Utc::now()}),
        )
        .unwrap();
        *revoke.borrow_mut() = Some(json!({"path":path,"action":"revoke","id":id,"publicKey":bob}));
        hosting.reconcile().unwrap();

        let view = hosting.view();
        assert_eq!(view[0]["state"], "serving");
        assert_eq!(rows(&view[0]["approved"]).len(), 1);
        assert_eq!(view[0]["approved"][0]["publicKey"], alice);
    }

    #[test]
    fn revoked_first_hosted_approval_is_never_started_from_a_stale_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosting = Hosting::new(dir.path().join("hosting"));
        let path = hosting.path();
        let revoke = Rc::new(RefCell::new(None));
        let pending_revoke = revoke.clone();
        let _fake = bridge::fake::install(move |request| match text(request, "method") {
            "host_generate" => {
                let secret = config::dispatch("config.new_id", Value::Null)?;
                Ok(json!({"privateKey":secret,"presharedKey":secret}))
            }
            "host_peers" => {
                if let Some(mutation) = pending_revoke.borrow_mut().take() {
                    config::dispatch("config.agent_mutate", mutation).unwrap();
                }
                Ok(Value::Null)
            }
            "host_start" => {
                assert_eq!(request["approved"], json!([]));
                Ok(Value::Null)
            }
            "host_allow" => panic!("allowed a device from a stale snapshot"),
            _ => Ok(Value::Null),
        });
        let game = hosting.command(
            &json!({"method":"createHostedGame","game":"minecraft-java","name":"Game","port":25565}),
            Some(&json!({"RegionID":1,"Nodes":[{}]})),
        ).unwrap();
        let id = text(&game, "id");
        assert!(
            hosting.running_identities.contains_key(id),
            "the initial generation must be running to sample peers"
        );
        let alice = generated_public_key();
        config::dispatch(
            "config.agent_mutate",
            json!({"path":path,"action":"approve","id":id,"publicKey":alice,"now":chrono::Utc::now()}),
        )
        .unwrap();
        *revoke.borrow_mut() =
            Some(json!({"path":path,"action":"revoke","id":id,"publicKey":alice}));
        hosting.reconcile().unwrap();

        assert!(revoke.borrow().is_none(), "the revoke hook did not run");
        assert_eq!(hosting.view()[0]["state"], "inactive");
        assert_eq!(hosting.view()[0]["approved"], json!([]));
    }

    #[test]
    fn failed_live_approval_and_restart_report_an_error_and_keep_the_approval() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosting = Hosting::new(dir.path().join("hosting"));
        let batch = Rc::new(RefCell::new(Value::Null));
        let knocks = batch.clone();
        let fail = Rc::new(std::cell::Cell::new(false));
        let reject = fail.clone();
        let _fake = bridge::fake::install(move |request| match text(request, "method") {
            "host_generate" => {
                let secret = config::dispatch("config.new_id", Value::Null)?;
                Ok(json!({"privateKey":secret,"presharedKey":secret}))
            }
            "host_peers" => Ok(std::mem::take(&mut *knocks.borrow_mut())),
            "host_allow" | "host_start" if reject.get() => Err("test tunnel failure".into()),
            _ => Ok(Value::Null),
        });
        let game = hosting.command(
            &json!({"method":"createHostedGame","game":"minecraft-java","name":"Game","port":25565}),
            Some(&json!({"RegionID":1,"Nodes":[{}]})),
        ).unwrap();
        let id = text(&game, "id");
        hosting.command(
            &json!({"method":"approveHostedDevice","id":id,"publicKey":generated_public_key(),"label":"Alice"}),
            None,
        ).unwrap();

        let bob = generated_public_key();
        let now = chrono::Utc::now();
        *batch.borrow_mut() = json!({(id):{"connected":{},"requests":[{
            "publicKey":bob,"firstSeen":now,"lastSeen":now,"attempts":1
        }]}});
        hosting.refresh();
        let request = hosting.view()[0]["pending"][0]["id"].clone();
        assert!(request.is_u64());

        fail.set(true);
        let error = hosting
            .command(
                &json!({"method":"approveHostedRequest","id":id,"requestId":request,"label":"Bob"}),
                None,
            )
            .unwrap_err();
        assert_eq!(error, "Could not start the Tailcat game tunnel");
        assert_eq!(hosting.view()[0]["state"], "error");
        assert_eq!(
            hosting.load_games().unwrap()[0]["devices"][1]["publicKey"],
            bob
        );

        fail.set(false);
        hosting.clock -= Duration::from_secs(11);
        hosting.refresh();
        assert_eq!(hosting.view()[0]["state"], "serving");
        assert_eq!(hosting.view()[0]["approved"][1]["publicKey"], bob);
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
            .observe(
                &json!([{
                    "id":"game","name":"Example","type":"factorio","protocol":"udp",
                    "tunnelPort":34197,"dest":"192.0.2.2:34197","devices":[]
                }]),
                &BTreeSet::new()
            )
            .is_err());
    }
}
