//! Rust desktop application host. One lock serializes policy and ordered effects.
//! Go callbacks only enqueue snapshots, so closure barriers cannot reenter it.
use crate::{bridge, hosting::Hosting};
use crate::{minecraft_lists::MinecraftLists, presence::Presence};
use gamelink_core::{client_lifecycle, config, desktop, launch, pure, storage};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, String>;
static APP: OnceLock<Mutex<Option<App>>> = OnceLock::new();
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn rows(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn policy(method: &str, v: Value) -> Result<Value> {
    desktop::dispatch(method, v)
}
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
struct SettingsInput {
    demo_mode: bool,
    auto_reconnect: bool,
    check_on_startup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_for_updates: Option<bool>,
    verbose_logging: bool,
    allow_local_relay: bool,
    minecraft_server_list: bool,
    prism_instances_dir: String,
    presence_notifications: bool,
    game_executables: Option<BTreeMap<String, String>>,
    game_launch_args: Option<BTreeMap<String, String>>,
}
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
struct FormInput {
    name: String,
    description: String,
    game: String,
    proto: String,
    port: i64,
    local_port: i64,
    address: String,
}
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
struct Request {
    method: String,
    id: String,
    text: String,
    name: String,
    description: String,
    local_port: i64,
    port: i64,
    game: String,
    public_key: String,
    request_id: Option<u64>,
    label: String,
    enabled: Option<bool>,
    limit: i64,
    form: FormInput,
    settings: SettingsInput,
}
pub fn release_check_dir() -> Option<PathBuf> {
    let lock = APP.get()?.lock().ok()?;
    let app = lock.as_ref()?;
    (app.cfg["settings"]["checkForUpdates"] == true).then(|| app.dir.clone())
}
pub fn log_release_check_error(error: &'static str) {
    let Some(lock) = APP.get().and_then(|app| app.lock().ok()) else {
        return;
    };
    if let Some(app) = lock.as_ref() {
        app.write_log(&format!("[debug] release check: {error}"));
    }
}
pub fn call(request: Value) -> Result<Value> {
    let result = call_with_region(request.clone(), None)?;
    if result["resolveRelay"] == true {
        let region = bridge::call(json!({"method":"host_resolve"}))?;
        return call_with_region(request, Some(&region));
    }
    Ok(result)
}

fn call_with_region(request: Value, region: Option<&Value>) -> Result<Value> {
    if serde_json::to_vec(&request)
        .map_err(|_| "invalid desktop request")?
        .len()
        > bridge::MAX_INPUT
    {
        return Err("desktop request exceeds size limit".into());
    }
    let request: Request =
        serde_json::from_value(request).map_err(|_| "invalid desktop request")?;
    let request = serde_json::to_value(request).map_err(|_| "invalid desktop request")?;
    let mut lock = APP
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "desktop host unavailable")?;
    let check_id = if request["method"] == "check" {
        Some(text(&request, "id").to_owned())
    } else {
        None
    };
    let already_checking = check_id.as_ref().is_some_and(|id| {
        lock.as_ref()
            .and_then(|app| app.states.get(id))
            .is_some_and(|state| !state["probe"].is_null())
    });
    let result = match text(&request, "method") {
        "init" => {
            if lock.is_some() {
                return Err("desktop is already initialized".into());
            }
            *lock = Some(App::open()?);
            Ok(Value::Null)
        }
        "shutdown" => {
            if let Some(mut app) = lock.take() {
                app.shutdown()?;
            }
            Ok(Value::Null)
        }
        _ => lock
            .as_mut()
            .ok_or("desktop is not initialized".to_owned())?
            .command_with_region(request, region),
    };
    drop(lock);
    // Preserve the synchronous Check result while permitting another command
    // to cancel it. No application lock is held during this bounded wait.
    if !already_checking && result.as_ref().is_ok_and(|v| v["state"] == "checking") {
        if let Some(id) = check_id {
            loop {
                std::thread::sleep(Duration::from_millis(20));
                let mut lock = APP
                    .get()
                    .expect("initialized host")
                    .lock()
                    .map_err(|_| "desktop host unavailable")?;
                let Some(app) = lock.as_mut() else {
                    return Ok(json!({"state":"unknown"}));
                };
                app.pump()?;
                let view = app.step(&id, json!({"type":"view"}))?;
                if view["check"]["state"] != "checking" {
                    return Ok(view["check"].clone());
                }
            }
        }
    }
    result
}

pub fn take_online_notification() -> Option<String> {
    let mut lock = APP.get()?.lock().ok()?;
    let app = lock.as_mut()?;
    let id = app.presence.take_notice(Instant::now())?;
    let server = app.server(&id).ok()?;
    Some(format!("{} is online", text(&server, "name")))
}

struct App {
    dir: PathBuf,
    cfg: Value,
    identity: Value,
    states: BTreeMap<String, Value>,
    activity: Vec<Value>,
    timers: BTreeMap<String, (u64, Instant, u16)>,
    resources: BTreeMap<String, u64>,
    updates: u64,
    startup: Option<Instant>,
    checks: Vec<String>,
    hosting: Hosting,
    minecraft: MinecraftLists,
    presence: Presence,
}
impl App {
    fn open() -> Result<Self> {
        bridge::call(json!({"method":"init"}))?;
        let result = Self::load();
        if result.is_err() {
            let _ = bridge::call(json!({"method":"shutdown"}));
        }
        result
    }
    fn load() -> Result<Self> {
        let dir = if let Some(dir) = std::env::var_os("GAMELINK_HOME").filter(|s| !s.is_empty()) {
            PathBuf::from(dir)
        } else {
            #[cfg(windows)]
            let base = std::env::var_os("APPDATA");
            #[cfg(not(windows))]
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|p| PathBuf::from(p).join(".config").into_os_string())
                });
            PathBuf::from(base.ok_or("cannot find a configuration directory")?).join("GameLink")
        };
        Self::load_in(dir)
    }
    fn load_in(dir: PathBuf) -> Result<Self> {
        storage::ensure_dir(&dir)?;
        let path = dir.join("identity.json");
        let mut identity = storage::dispatch("storage.identity_load", json!({"path":path}))?;
        let created = identity["missing"] == true;
        if created {
            identity = bridge::call(json!({"method":"generate"}))?;
            identity["created"] = json!(now());
            storage::dispatch(
                "storage.identity_save",
                json!({"path":path,"privateKey":identity["privateKey"],"created":identity["created"]}),
            )?;
        }
        identity["publicKey"] =
            bridge::call(json!({"method":"identity","privateKey":identity["privateKey"]}))?;
        identity
            .as_object_mut()
            .ok_or("invalid identity")?
            .remove("privateKey");
        let cfg_path = dir.join("config.json");
        let config_path = cfg_path
            .to_str()
            .ok_or("configuration path is not valid UTF-8")?;
        let opened = config::dispatch("config.client_open", json!({"path":config_path}))?;
        let cfg = config::assign_client_ports(config_path, opened.clone())?;
        // Migration resets a pinned port that clashes with another server's.
        let reset_pins = rows(&opened["servers"])
            .iter()
            .zip(rows(&cfg["servers"]))
            .filter(|(before, after)| {
                before["localPort"].as_u64().is_some_and(|p| p != 0) && after["localPort"] == 0
            })
            .map(|(before, _)| {
                format!(
                    "Server “{}” had local port {}, which another saved server already uses; it now uses an automatic port",
                    text(before, "name"),
                    before["localPort"]
                )
            })
            .collect::<Vec<_>>();
        let activity = storage::read_bounded(&dir.join("activity.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok())
            .unwrap_or_default();
        let activity = activity
            .into_iter()
            .rev()
            .take(200)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let startup = if cfg["settings"]["checkOnStartup"] == true {
            Some(Instant::now() + Duration::from_millis(1500))
        } else {
            None
        };
        let hosting = Hosting::new(dir.join("hosting"));
        let mut app = Self {
            dir,
            cfg,
            identity,
            states: BTreeMap::new(),
            activity,
            timers: BTreeMap::new(),
            resources: BTreeMap::new(),
            updates: 0,
            startup,
            checks: vec![],
            hosting,
            minecraft: MinecraftLists::default(),
            presence: Presence::default(),
        };
        app.minecraft.refresh(&app.dir, &app.cfg);
        app.presence.set_enabled(
            app.cfg["settings"]["presenceNotifications"] == true,
            Instant::now(),
        );
        if created {
            app.add_log("info", "", "Created this device's identity");
        }
        for message in reset_pins {
            app.add_log("warning", "", &message);
        }
        Ok(app)
    }
    fn shutdown(&mut self) -> Result<()> {
        self.timers.clear();
        self.checks.clear();
        self.startup = None;
        let mut failure = self.hosting.shutdown().err();
        for id in self.states.keys().cloned().collect::<Vec<_>>() {
            if let Err(error) = self.step(&id, json!({"type":"close"})) {
                failure = Some(error);
            }
        }
        bridge::call(json!({"method":"shutdown"}))?;
        failure.map_or(Ok(()), Err)
    }
    fn add_log(&mut self, kind: &str, id: &str, message: &str) {
        let message = pure::dispatch("security.redact", json!({"text":message,"secrets":[]}))
            .unwrap_or(json!("[redacted]"));
        self.activity
            .push(json!({"time":now(),"kind":kind,"serverId":id,"message":message}));
        if self.activity.len() > 200 {
            self.activity.remove(0);
        }
        self.save_activity();
        self.updates |= 2;
    }
    fn write_log(&self, line: &str) {
        use std::io::Write;
        let path = self.dir.join("gamelink.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 2 << 20) {
            let _ = std::fs::remove_file(&path);
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if let Ok(mut file) = options.open(path) {
            let _ = writeln!(
                file,
                "{} {}",
                chrono::Local::now().format("%H:%M:%S%.3f"),
                line
            );
        }
    }
    fn save_activity(&self) {
        if let Ok(bytes) = serde_json::to_vec(&self.activity) {
            let _ = storage::write_atomic(&self.dir.join("activity.json"), &bytes);
        }
    }
    fn server(&self, id: &str) -> Result<Value> {
        rows(&self.cfg["servers"])
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
            .ok_or("server not found".into())
    }
    fn mutate(&mut self, action: &str, id: &str, value: Value) -> Result<Value> {
        let result = config::dispatch(
            "config.client_mutate",
            json!({"path":self.dir.join("config.json"),"config":self.cfg,"action":action,"id":id,"value":value}),
        )?;
        self.cfg = result["config"].clone();
        Ok(result["result"].clone())
    }
    /// The loopback port a server's Minecraft list entry should use: the port
    /// its live tunnel actually bound (it differs from `stablePort` after a
    /// fallback), otherwise the stable port.
    fn list_port(&self, server: &Value) -> Value {
        self.states
            .get(text(server, "id"))
            .filter(|state| state["tunnel"] == true)
            .map(|state| &state["status"]["localPort"])
            .filter(|port| port.as_u64().is_some_and(|p| p != 0))
            .unwrap_or(&server["stablePort"])
            .clone()
    }
    fn refresh_minecraft_lists(&mut self) {
        let mut cfg = self.cfg.clone();
        for server in cfg["servers"].as_array_mut().into_iter().flatten() {
            server["stablePort"] = self.list_port(server);
        }
        self.minecraft.refresh(&self.dir, &cfg);
        self.updates |= 5;
    }
    fn minecraft_listed(&self) -> bool {
        self.minecraft.listed(text(
            &self.cfg["settings"]["gameExecutables"],
            "minecraft-java",
        ))
    }
    fn settings_view(&self) -> Result<Value> {
        let mut settings = policy(
            "desktop.settings_view",
            json!({"settings":self.cfg["settings"]}),
        )?;
        settings["minecraftSyncError"] = json!(self.minecraft.error);
        settings["canCheckForUpdates"] = json!(gamelink_core::update::can_check());
        Ok(settings)
    }
    fn address(&self, id: &str) -> Result<String> {
        let server = self.server(id)?;
        pure::check_invite_expiry(&server["expiresAt"])?;
        storage::unseal(text(&server, "addr"))
    }
    fn validate(&self, address: &str, allow: bool) -> Result<Value> {
        bridge::call(json!({"method":"validate","address":address,"allowLocalRelay":allow}))
    }
    fn step(&mut self, id: &str, mut event: Value) -> Result<Value> {
        if matches!(
            text(&event, "type"),
            "view" | "join" | "check" | "disconnect" | "remove" | "close" | "edit_port"
        ) {
            let stats = bridge::call(json!({"method":"stats"}))?;
            if !stats[id].is_null() {
                event["liveStatus"] = stats[id].clone();
            }
        }
        let result = client_lifecycle::reduce(json!({"state":self.states.get(id),"event":event}))?;
        self.states.insert(id.into(), result["state"].clone());
        if let Some(error) = result["error"].as_str().filter(|s| !s.is_empty()) {
            return Err(error.into());
        }
        for effect in rows(&result["effects"]) {
            let generation = effect["generation"].as_u64().unwrap_or(0);
            match text(effect, "type") {
                "stop" | "cancel_probe" | "close_returned" => {
                    bridge::call(json!({"method":"stop","id":id,"generation":generation}))?;
                    if self.resources.get(id) == Some(&generation) {
                        self.resources.remove(id);
                    }
                    if self
                        .presence
                        .active
                        .as_ref()
                        .is_some_and(|(server, current)| server == id && *current == generation)
                    {
                        self.presence.active = None;
                    }
                    if self.timers.get(id).is_some_and(|t| t.0 == generation) {
                        self.timers.remove(id);
                    }
                }
                _ => (),
            }
        }
        for log in rows(&result["logs"]) {
            self.add_log(text(log, "kind"), id, text(log, "message"));
        }
        if result["notify"] == true {
            self.updates |= 1;
        }
        Ok(result)
    }
    fn generation(result: &Value, kind: &str) -> Option<u64> {
        rows(&result["effects"])
            .iter()
            .find(|e| e["type"] == kind)
            .and_then(|e| e["generation"].as_u64())
    }
    fn stop_retiring(&mut self, id: &str) -> Result<()> {
        if let Some(generation) = self.resources.remove(id) {
            bridge::call(json!({"method":"stop","id":id,"generation":generation}))?;
        }
        self.timers.remove(id);
        Ok(())
    }
    fn join(&mut self, id: &str) -> Result<()> {
        if id.starts_with("demo-") {
            let info = demo_info(id);
            let result=self.step(id,json!({"type":"demo_join","enabled":self.cfg["settings"]["demoMode"]==true,"valid":!info.is_null()}))?;
            self.stop_retiring(id)?;
            if let Some(generation) = Self::generation(&result, "start_timer") {
                self.timers.insert(
                    id.into(),
                    (
                        generation,
                        Instant::now() + Duration::from_millis(900),
                        info["defaultPort"].as_u64().unwrap_or(0) as u16,
                    ),
                );
            }
            return Ok(());
        }
        let server = self.server(id)?;
        let address = self.address(id)?;
        let result = self.step(id, json!({"type":"join","name":server["name"]}))?;
        self.stop_retiring(id)?;
        if let Some(generation) = Self::generation(&result, "start_join") {
            if server["game"] == "minecraft-java"
                && self.cfg["settings"]["minecraftServerList"] == true
            {
                self.refresh_minecraft_lists();
            }
            let params = policy("desktop.join_parameters", json!({"server":server}))?;
            let reserved = rows(&self.cfg["servers"])
                .iter()
                .filter(|row| row["id"] != id)
                .filter_map(|row| row["stablePort"].as_u64())
                .collect::<Vec<_>>();
            let options = json!({"Name":server["name"],"Game":server["game"],"Proto":server["proto"],"TunnelPort":server["port"],"LocalPort":server["localPort"],"PreferPort":params["preferPort"],"ReservedPorts":reserved,"AutoReconnect":self.cfg["settings"]["autoReconnect"]});
            let started = bridge::call(
                json!({"method":"join","id":id,"generation":generation,"address":address,"verbose":self.cfg["settings"]["verboseLogging"],"allowLocalRelay":self.cfg["settings"]["allowLocalRelay"],"options":options}),
            );
            if let Err(error) = started {
                // The bridge never started a join, so no join_end will arrive.
                // Report the failure through the lifecycle as if it had, so the
                // display leaves "connecting" and the user can retry.
                self.step(
                    id,
                    json!({"type":"join_end","generation":generation,"failed":true,"error":error,"now":now()}),
                )?;
                return Err(error);
            }
            self.resources.insert(id.into(), generation);
        }
        Ok(())
    }
    fn check(&mut self, id: &str) -> Result<Value> {
        let server = match self.server(id) {
            Ok(s) => s,
            Err(_) => return Ok(json!({"state":"unknown"})),
        };
        let checked_at = now();
        let result = self.step(
            id,
            json!({"type":"check","name":server["name"],"now":checked_at}),
        )?;
        if let Some(generation) = Self::generation(&result, "start_probe") {
            self.stop_retiring(id)?;
            match self.address(id) {
                Ok(address) => {
                    if let Err(error) = bridge::call(
                        json!({"method":"check","id":id,"generation":generation,"address":address,"options":{"Game":server["game"],"Proto":server["proto"],"TunnelPort":server["port"]},"allowLocalRelay":self.cfg["settings"]["allowLocalRelay"]}),
                    ) {
                        self.step(id, json!({"type":"check_end","generation":generation,"outcome":"start_error","now":checked_at}))?;
                        return Err(error);
                    }
                    self.resources.insert(id.into(), generation);
                }
                Err(_) => {
                    return Ok(self.step(id,json!({"type":"check_end","generation":generation,"outcome":"address_error","now":now()}))?["result"].clone());
                }
            }
        } else if result["connection"]["state"] == "connected" {
            self.observe_presence(id, &result["result"], true);
        }
        Ok(result["result"].clone())
    }
    fn observe_presence(&mut self, id: &str, check: &Value, agent_answered: bool) {
        match text(check, "state") {
            "online" => self.presence.observe(id, true, Instant::now()),
            "offline" | "unreachable" => self.presence.observe(id, false, Instant::now()),
            "unknown"
                if agent_answered
                    && self.server(id).is_ok_and(|s| {
                        s["proto"] == "udp"
                            && !matches!(s["game"].as_str(), Some("minecraft-bedrock" | "factorio"))
                    }) =>
            {
                self.presence.observe(id, true, Instant::now());
            }
            _ => (),
        }
    }
    fn pump(&mut self) -> Result<()> {
        let events = bridge::call(json!({"method":"events"}))?;
        // A terminal callback carries its final status. Process statuses before
        // terminal events even when Go map iteration returns the opposite order.
        for terminal in [false, true] {
            for callback in rows(&events) {
                let kind = text(callback, "kind");
                if kind == "log" {
                    if terminal {
                        self.write_log(callback["value"].as_str().unwrap_or("[redacted]"));
                    }
                    continue;
                }
                if (kind != "status") != terminal {
                    continue;
                }
                let id = text(callback, "id");
                let generation = callback["generation"].clone();
                let mut event = if kind == "status" {
                    json!({"status":callback["value"]})
                } else {
                    callback["value"].clone()
                };
                event["type"] = json!(kind);
                event["generation"] = generation.clone();
                event["now"] = json!(now());
                let observed = kind == "check_end"
                    && matches!(text(&event, "outcome"), "online" | "error")
                    && generation.is_u64()
                    && self
                        .states
                        .get(id)
                        .is_some_and(|state| state["probe"]["generation"] == generation);
                let agent_answered = event["outcome"] == "online";
                let result = self.step(id, event)?;
                if kind == "join_end"
                    && callback["value"]["failed"] != true
                    && self.cfg["settings"]["minecraftServerList"] == true
                    && self
                        .server(id)
                        .is_ok_and(|server| server["game"] == "minecraft-java")
                {
                    // Point the list entry at the port the tunnel really bound.
                    self.refresh_minecraft_lists();
                }
                if observed {
                    self.observe_presence(id, &result["check"], agent_answered);
                }
                if kind == "check_end"
                    || (kind == "join_end" && callback["value"]["failed"] == true)
                {
                    bridge::call(json!({"method":"stop","id":id,"generation":generation}))?;
                    if self.resources.get(id).copied() == generation.as_u64() {
                        self.resources.remove(id);
                    }
                    if self
                        .presence
                        .active
                        .as_ref()
                        .is_some_and(|(server, active)| {
                            server == id && Some(*active) == generation.as_u64()
                        })
                    {
                        self.presence.active = None;
                    }
                }
            }
        }
        for (id, (generation, deadline, port)) in self.timers.clone() {
            if Instant::now() >= deadline {
                self.timers.remove(&id);
                self.step(
                    &id,
                    json!({"type":"demo_timer","generation":generation,"port":port,"now":now()}),
                )?;
            }
        }
        if self.startup.is_some_and(|t| Instant::now() >= t) {
            self.startup = None;
            self.schedule_checks();
        }
        let instant = Instant::now();
        self.presence.schedule(
            instant,
            rows(&self.cfg["servers"]).iter().map(|s| text(s, "id")),
        );
        if !self.resources.keys().any(|id| {
            self.states
                .get(id)
                .is_some_and(|s| s["check"]["state"] == "checking")
        }) {
            if let Some(id) = self.checks.pop() {
                self.check(&id)?;
            } else if let Some(id) = self.presence.next_probe(instant) {
                self.check(&id)?;
                if let Some(generation) = self
                    .states
                    .get(&id)
                    .and_then(|s| s["probe"]["generation"].as_u64())
                {
                    self.presence.active = Some((id, generation));
                }
            }
        }
        Ok(())
    }
    fn schedule_checks(&mut self) {
        if self.checks.is_empty() {
            self.checks = rows(&self.cfg["servers"])
                .iter()
                .rev()
                .map(|s| text(s, "id").into())
                .collect();
        }
    }
    fn view(&mut self, server: Value) -> Result<Value> {
        let id = text(&server, "id");
        let info = info(&server["game"]);
        let connection = self.step(id, json!({"type":"view"}))?;
        let details = storage::unseal(text(&server, "addr"))
            .ok()
            .and_then(|addr| self.validate(&addr, true).ok())
            .unwrap_or(Value::Null);
        Ok(
            json!({"id":server["id"],"name":server["name"],"description":text(&server,"description"),"game":server["game"],"gameName":info["name"],"proto":server["proto"],"port":server["port"],"localPort":server["localPort"],"stablePort":server["stablePort"],"minecraftListed":server["game"]=="minecraft-java"&&self.minecraft_listed(),"launchConnects":desktop::launch_connects(&server["game"],&self.cfg["settings"],cfg!(windows)),"defaultPort":info["defaultPort"],"addedAt":server["addedAt"],"hint":info["hint"],"hasExecutable":!text(&self.cfg["settings"]["gameExecutables"],text(&server,"game")).is_empty() || (cfg!(windows)&&!text(&info,"launchUri").is_empty()),"relayHosts":details["relayHosts"],"defaultRelay":details["defaultRelay"]==true,"serverKey":text(&details,"serverKey"),"serverFingerprint":details["serverPublicKey"].as_str().map(gamelink_core::requests::server_fingerprint),"demo":false,"connection":connection["connection"],"check":connection["check"]}),
        )
    }
    fn servers(&mut self) -> Result<Value> {
        let mut out = vec![];
        for server in rows(&self.cfg["servers"]).to_vec() {
            out.push(self.view(server)?);
        }
        if self.cfg["settings"]["demoMode"] == true {
            for (id, name, typ) in [
                ("demo-mc", "Demo: Survival World", "minecraft-java"),
                ("demo-bedrock", "Demo: Creative Realm", "minecraft-bedrock"),
                ("demo-factorio", "Demo: Mega Factory", "factorio"),
            ] {
                let info = info(&json!(typ));
                let state = self.step(id, json!({"type":"view"}))?;
                out.push(json!({"id":id,"name":name,"description":"Sample server for trying the interface","game":typ,"gameName":info["name"],"proto":info["protocol"],"port":info["defaultPort"],"defaultPort":info["defaultPort"],"localPort":0,"hint":info["hint"],"demo":true,"hasExecutable":false,"relayHosts":null,"serverKey":"","defaultRelay":false,"addedAt":"0001-01-01T00:00:00Z","check":{"state":"unknown","message":"Demo servers are not real","latencyMs":null,"path":"","checkedAt":"0001-01-01T00:00:00Z"},"connection":state["connection"]}));
            }
        }
        Ok(json!(out))
    }
    #[cfg(test)]
    fn command(&mut self, r: Value) -> Result<Value> {
        self.command_with_region(r, None)
    }

    fn command_with_region(&mut self, r: Value, region: Option<&Value>) -> Result<Value> {
        let method = text(&r, "method");
        match method {
            "hostedGames"
            | "createHostedGame"
            | "shareLanWorld"
            | "scanLanWorlds"
            | "stopLanScan"
            | "approveHostedRequest"
            | "dismissHostedRequest"
            | "approveHostedDevice"
            | "revokeHostedDevice"
            | "setHostingEnabled"
            | "removeHostedGame"
            | "hostedInvitation" => return self.hosting.command(&r, region),
            "updates" => self.hosting.pump(),
            _ => {}
        }
        self.pump()?;
        let id = text(&r, "id");
        match method {
            "updates" => {
                let bits = self.updates;
                self.updates = 0;
                Ok(json!(bits))
            }
            "version" => Ok(
                json!({"app":"GameLink","version":crate::VERSION,"transport":"Tailcat (WireGuard, direct with DERP relay fallback)"}),
            ),
            "identity" => Ok(
                json!({"publicKey":self.identity["publicKey"],"created":self.identity["created"],"configDir":self.dir,"protection":if cfg!(windows){"Windows DPAPI (current user) plus user-profile file permissions"}else{"file permissions (0600)"}}),
            ),
            "paths" => Ok(
                json!({"configDir":self.dir,"configFile":self.dir.join("config.json"),"identityFile":self.dir.join("identity.json"),"activityFile":self.dir.join("activity.json"),"logFile":self.dir.join("gamelink.log")}),
            ),
            "settings" => self.settings_view(),
            "updateSettings" => {
                let settings = policy(
                    "desktop.settings_prepare",
                    json!({"settings":r["settings"],"saved":self.cfg["settings"]}),
                )?;
                let old = &self.cfg["settings"];
                let list_changed = ["minecraftServerList", "prismInstancesDir"]
                    .iter()
                    .any(|&key| old[key] != settings[key]);
                #[cfg(windows)]
                let remove_notice = old["presenceNotifications"] == true
                    && settings["presenceNotifications"] != true;
                self.mutate("settings", "", settings)?;
                if list_changed {
                    self.refresh_minecraft_lists();
                }
                if let Some((id, generation)) = self.presence.set_enabled(
                    self.cfg["settings"]["presenceNotifications"] == true,
                    Instant::now(),
                ) {
                    self.step(&id, json!({"type":"cancel_check","generation":generation}))?;
                }
                #[cfg(windows)]
                if remove_notice {
                    self.updates |= 8;
                }
                self.updates |= 5;
                self.settings_view()
            }
            "servers" => self.servers(),
            "overview" => {
                let servers = self.servers()?;
                policy(
                    "desktop.overview",
                    json!({"servers":servers,"publicKey":self.identity["publicKey"]}),
                )
            }
            "activity" => {
                let limit = r["limit"].as_u64().filter(|n| *n > 0).unwrap_or(200) as usize;
                Ok(json!(self
                    .activity
                    .iter()
                    .rev()
                    .take(limit)
                    .collect::<Vec<_>>()))
            }
            "clearActivity" => {
                self.activity.clear();
                self.save_activity();
                Ok(Value::Null)
            }
            "previewInvite" | "importInvite" => {
                let inv = pure::dispatch("invite.decode", json!({"text":r["text"]}))?;
                let details = self.validate(
                    text(&inv, "addr"),
                    self.cfg["settings"]["allowLocalRelay"] == true,
                )?;
                if r["method"] == "previewInvite" {
                    policy("desktop.preview", json!({"invite":inv,"details":details}))
                } else {
                    let input = policy(
                        "desktop.server_input",
                        json!({"value":inv,"import":true,"localPort":r["localPort"]}),
                    )?;
                    self.save_server(input, "Imported")
                }
            }
            "addServer" => {
                self.validate(
                    text(&r["form"], "address"),
                    self.cfg["settings"]["allowLocalRelay"] == true,
                )?;
                let input = policy("desktop.server_input", json!({"value":r["form"]}))?;
                self.save_server(input, "Added")
            }
            "updateServer" => {
                pure::dispatch(
                    "security.port",
                    json!({"port":r["localPort"],"optional":true}),
                )?;
                let old = self.server(id)?;
                self.step(
                    id,
                    json!({"type":"edit_port","changed":old["localPort"]!=r["localPort"]}),
                )?;
                let server=self.mutate("update",id,json!({"name":text(&r,"name").trim(),"description":text(&r,"description").trim(),"localPort":r["localPort"]}))?;
                if server["game"] == "minecraft-java"
                    && self.cfg["settings"]["minecraftServerList"] == true
                {
                    self.refresh_minecraft_lists();
                }
                self.updates |= 1;
                self.view(server)
            }
            "removeServer" => {
                let server = self.server(id)?;
                self.step(id, json!({"type":"remove"}))?;
                if let Err(error) = self.mutate("remove", id, Value::Null) {
                    self.step(id, json!({"type":"restore"}))?;
                    return Err(error);
                }
                self.presence.remove(id);
                if server["game"] == "minecraft-java" {
                    self.refresh_minecraft_lists();
                }
                self.add_log(
                    "info",
                    "",
                    &format!("Removed server “{}”", text(&server, "name")),
                );
                self.updates |= 1;
                Ok(Value::Null)
            }
            "join" => {
                self.join(id)?;
                Ok(Value::Null)
            }
            "disconnect" => {
                self.step(id, json!({"type":"disconnect"}))?;
                Ok(Value::Null)
            }
            "check" => self.check(id),
            "checkAll" => {
                self.schedule_checks();
                Ok(Value::Null)
            }
            "launchGame" => {
                let server = self.server(id)?;
                let connection = self.step(id, json!({"type":"view"}))?["connection"].clone();
                let java_list = server["game"] == "minecraft-java"
                    && self.cfg["settings"]["minecraftServerList"] == true;
                if java_list {
                    self.refresh_minecraft_lists();
                }
                let mut plan = policy(
                    "desktop.launch_plan",
                    json!({"server":server,"connection":connection,"settings":self.cfg["settings"],"canOpenURI":cfg!(windows)}),
                )?;
                if java_list
                    && self.minecraft_listed()
                    && connection["localPort"] == self.list_port(&server)
                {
                    plan["result"]["minecraftEntry"] =
                        json!(format!("{} (GameLink)", text(&server, "name")));
                }
                launch::dispatch(
                    if plan["kind"] == "uri" {
                        "launch.uri"
                    } else {
                        "launch.program"
                    },
                    json!({"exe":plan["target"],"uri":plan["target"],"args":plan["args"]}),
                )
                .map_err(|e| format!("could not start the game: {e}"))?;
                self.add_log("info", id, text(&plan, "message"));
                Ok(plan["result"].clone())
            }
            "diagnostics" => {
                let server = self.server(id)?;
                let view = self.view(server)?;
                Ok(json!(diagnostics(&view, text(&self.identity, "publicKey"))))
            }
            _ => Err("unknown desktop command".into()),
        }
    }
    fn save_server(&mut self, mut input: Value, verb: &str) -> Result<Value> {
        input["addr"] = json!(storage::seal(text(&input, "addr"))?);
        input["id"] = config::dispatch("config.new_id", json!({}))?;
        input["addedAt"] = json!(now());
        let server = self.mutate("add", "", input)?;
        if server["game"] == "minecraft-java" && self.cfg["settings"]["minecraftServerList"] == true
        {
            self.refresh_minecraft_lists();
        }
        let id = text(&server, "id");
        self.add_log(
            "info",
            id,
            &format!("{verb} server “{}”", text(&server, "name")),
        );
        self.updates |= 1;
        self.checks.push(id.into());
        self.view(server)
    }
}
fn info(typ: &Value) -> Value {
    rows(&pure::dispatch("games.all", json!({})).expect("catalog"))
        .iter()
        .find(|g| g["type"] == *typ)
        .cloned()
        .unwrap_or(Value::Null)
}
fn demo_info(id: &str) -> Value {
    info(&json!(match id {
        "demo-mc" => "minecraft-java",
        "demo-bedrock" => "minecraft-bedrock",
        "demo-factorio" => "factorio",
        _ => "",
    }))
}
fn diagnostics(v: &Value, key: &str) -> String {
    let c = &v["connection"];
    let hosts = rows(&v["relayHosts"])
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let mut out=format!("GameLink {} diagnostics\nServer: {} ({})\nProtocol: {}, tunnel port {}, stable local port {}\nRelay: {} (public relay: {})\nServer key: {}\nThis device: {}\nState: {} - {}\n",crate::VERSION,text(v,"name"),text(v,"gameName"),text(v,"proto"),v["port"],v["stablePort"],hosts,v["defaultRelay"],text(v,"serverKey"),key,text(c,"state"),text(c,"message"));
    if !text(c, "localAddr").is_empty() {
        out += &format!(
            "Local address: {} (preferred port busy: {})\n",
            text(c, "localAddr"),
            c["portFallback"]
        );
    }
    out += &format!("Path: {}", text(c, "path"));
    if !text(c, "relay").is_empty() {
        out += &format!(" via relay {}", text(c, "relay"));
    }
    if let Some(ms) = c["latencyMs"].as_f64() {
        out += &format!(", latency {ms:.1} ms");
    }
    out+=&format!("\nGame port reachable through tunnel: {}\nActive connections: {}, UDP flows: {}, reconnects: {}, oversize UDP drops: {}\nTraffic: {} bytes to server, {} bytes from server\nLast check: {} {}\nLimits: UDP datagrams larger than 1232 bytes cannot be carried and are dropped.\n\nNo secrets are included in this report.\n",text(c,"gameReachable"),c["activeConns"],c["udpFlows"],c["reconnects"],c["oversizeDrops"],c["bytesToServer"],c["bytesFromServer"],text(&v["check"],"state"),text(&v["check"],"message"));
    out
}

// The Go client tests (tests/testsupport/client/lifecycle_test.go) used fake
// tunnels behind Manager. These drive the Rust application host the same way,
// against a scripted bridge. They build `App` directly, so they never touch the
// process-wide `APP` or `GAMELINK_HOME` used by bridge::tests.
#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, fs, rc::Rc};

    struct Script {
        calls: Vec<Value>,
        errors: BTreeMap<String, String>,
        stats: Value,
        events: Vec<Value>,
        validate: Value,
    }
    type Shared = Rc<RefCell<Script>>;

    fn script() -> Shared {
        Rc::new(RefCell::new(Script {
            calls: vec![],
            errors: BTreeMap::new(),
            stats: json!({}),
            events: vec![],
            validate: json!({"relayHosts":["derp.example.com"],"relayRegion":"Example","defaultRelay":true,"serverKey":"nodekey:servervalue","serverPublicKey":"nodekey:serverfullvalue"}),
        }))
    }
    // Strict: an unscripted bridge method is a test failure, never a real call.
    fn fake(script: &Shared) -> bridge::fake::Installed {
        let script = script.clone();
        bridge::fake::install(move |request| {
            let mut s = script.borrow_mut();
            s.calls.push(request.clone());
            let method = request["method"].as_str().unwrap().to_owned();
            if let Some(error) = s.errors.get(&method) {
                return Err(error.clone());
            }
            match method.as_str() {
                "init" | "shutdown" | "join" | "check" | "stop" => Ok(Value::Null),
                "generate" => {
                    Ok(json!({"privateKey":"privkey:0011","publicKey":"nodekey:generated"}))
                }
                "identity" => Ok(json!("nodekey:devicepublickey")),
                "stats" => Ok(s.stats.clone()),
                "events" => Ok(json!(std::mem::take(&mut s.events))),
                "validate" => Ok(s.validate.clone()),
                other => panic!("unscripted bridge method {other:?}"),
            }
        })
    }
    struct Env {
        app: App,
        script: Shared,
        dir: tempfile::TempDir,
        _bridge: bridge::fake::Installed,
    }
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let script = script();
        let bridge = fake(&script);
        let app = App::load_in(dir.path().to_path_buf()).unwrap();
        Env {
            app,
            script,
            dir,
            _bridge: bridge,
        }
    }
    impl Env {
        fn add(&mut self, id: &str, game: &str, proto: &str, port: u16) {
            self.app
                .mutate(
                    "add",
                    "",
                    json!({"id":id,"name":format!("Server {id}"),"description":"","game":game,"proto":proto,"port":port,"localPort":0,
                           "addr":storage::seal("synthetic-address").unwrap(),"addedAt":"2026-01-01T00:00:00Z"}),
                )
                .unwrap();
        }
        fn mc(&mut self, id: &str) {
            self.add(id, "minecraft-java", "tcp", 25565);
        }
        fn push(&self, id: &str, generation: u64, kind: &str, value: Value) {
            self.script
                .borrow_mut()
                .events
                .push(json!({"id":id,"generation":generation,"kind":kind,"value":value}));
        }
        fn fail(&self, method: &str, message: &str) {
            self.script
                .borrow_mut()
                .errors
                .insert(method.into(), message.into());
        }
        fn calls(&self, method: &str) -> Vec<Value> {
            self.script
                .borrow()
                .calls
                .iter()
                .filter(|c| c["method"] == method)
                .cloned()
                .collect()
        }
        fn lifecycle(&self) -> Vec<String> {
            self.script
                .borrow()
                .calls
                .iter()
                .filter_map(|c| match c["method"].as_str().unwrap() {
                    m @ ("join" | "check" | "stop") => Some(format!(
                        "{m}:{}:{}",
                        c["id"].as_str().unwrap(),
                        c["generation"]
                    )),
                    m @ ("init" | "shutdown") => Some(m.to_owned()),
                    _ => None,
                })
                .collect()
        }
        fn connection(&mut self, id: &str) -> Value {
            self.app.step(id, json!({"type":"view"})).unwrap()["connection"].clone()
        }
        fn check_view(&mut self, id: &str) -> Value {
            self.app.step(id, json!({"type":"view"})).unwrap()["check"].clone()
        }
        fn connect(&mut self, id: &str, port: u16) {
            self.app.join(id).unwrap();
            let generation = self.app.resources[id];
            self.push(
                id,
                generation,
                "join_end",
                json!({"hasTunnel":true,"failed":false,"status":connected(port)}),
            );
            self.app.pump().unwrap();
            assert_eq!(self.connection(id)["state"], "connected");
        }
        fn last_log(&self) -> Value {
            self.app.activity.last().cloned().unwrap()
        }
        fn break_config_file(&self) {
            let path = self.dir.path().join("config.json");
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
        }
        fn command(&mut self, request: Value) -> Result<Value> {
            self.app.command(request)
        }
    }
    fn connected(port: u16) -> Value {
        json!({"state":"connected","localAddr":format!("127.0.0.1:{port}"),"localPort":port,"path":"direct","latencyKnown":true,"latencyMs":3.0,"gameReachable":"yes"})
    }

    #[test]
    fn first_launch_creates_and_seals_the_device_identity_then_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let script = script();
        let _bridge = fake(&script);
        let app = App::load_in(dir.path().to_path_buf()).unwrap();
        assert_eq!(app.identity["publicKey"], "nodekey:devicepublickey");
        assert!(app.identity.get("privateKey").is_none());
        assert_eq!(
            app.activity.last().unwrap()["message"],
            "Created this device's identity"
        );
        assert!(app.startup.is_some(), "checkOnStartup defaults to on");
        let raw = fs::read_to_string(dir.path().join("identity.json")).unwrap();
        assert!(raw.contains("gls1:"));
        let asked: Vec<_> = script
            .borrow()
            .calls
            .iter()
            .map(|c| c["method"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(asked, ["generate", "identity"]);
        assert_eq!(
            script.borrow().calls[1]["privateKey"],
            "privkey:0011",
            "the public key is derived from the stored private key"
        );
        drop(app);

        // Second launch: the stored identity is used and nothing is generated.
        script.borrow_mut().calls.clear();
        let app = App::load_in(dir.path().to_path_buf()).unwrap();
        assert!(app.identity["created"].as_str().is_some());
        let asked: Vec<_> = script
            .borrow()
            .calls
            .iter()
            .map(|c| c["method"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(asked, ["identity"]);
        assert_eq!(script.borrow().calls[0]["privateKey"], "privkey:0011");
        assert_eq!(app.activity.len(), 1, "no second creation entry");
    }

    #[test]
    fn load_keeps_only_the_newest_200_activity_entries_and_honours_startup_off() {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<Value> = (0..250)
            .map(|i| json!({"time":"t","kind":"info","serverId":"","message":format!("m{i}")}))
            .collect();
        fs::write(
            dir.path().join("activity.json"),
            serde_json::to_vec(&entries).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"version":1,"servers":[],"settings":{"checkOnStartup":false,"autoReconnect":true}}"#,
        )
        .unwrap();
        let script = script();
        let _bridge = fake(&script);
        let app = App::load_in(dir.path().to_path_buf()).unwrap();
        // 200 kept, then the identity creation entry pushes the oldest out.
        assert_eq!(app.activity.len(), 200);
        assert_eq!(app.activity[0]["message"], "m51");
        assert_eq!(app.activity[198]["message"], "m249");
        assert_eq!(
            app.activity[199]["message"],
            "Created this device's identity"
        );
        assert!(app.startup.is_none());
        // Corrupt activity is ignored rather than fatal.
        fs::write(dir.path().join("activity.json"), "{").unwrap();
        assert!(App::load_in(dir.path().to_path_buf()).is_ok());
    }

    #[test]
    fn load_fails_on_a_corrupt_config_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.json"), "{broken").unwrap();
        let script = script();
        let _bridge = fake(&script);
        let error = App::load_in(dir.path().to_path_buf()).err().unwrap();
        assert!(error.contains("is corrupt"), "{error}");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.json")).unwrap(),
            "{broken"
        );
    }

    // TestJoinMarksPendingAndAwaitsProbeClientClosure (host half)
    #[test]
    fn join_passes_the_unsealed_address_and_owns_the_generation() {
        let mut e = env();
        e.mc("s1");
        e.app.join("s1").unwrap();
        assert_eq!(e.lifecycle(), ["join:s1:1"]);
        let request = e.calls("join").remove(0);
        assert_eq!(request["address"], "synthetic-address");
        assert_eq!(request["verbose"], false);
        assert_eq!(request["allowLocalRelay"], false);
        assert_eq!(request["options"]["Name"], "Server s1");
        assert_eq!(request["options"]["Game"], "minecraft-java");
        assert_eq!(request["options"]["Proto"], "tcp");
        assert_eq!(request["options"]["TunnelPort"], 25565);
        assert_eq!(request["options"]["LocalPort"], 0);
        assert_eq!(request["options"]["PreferPort"], 25565);
        assert_eq!(request["options"]["AutoReconnect"], true);
        assert_eq!(e.app.resources["s1"], 1);
        // Until the join completes the display stays "connecting", even if an
        // early status callback already says connected.
        e.push("s1", 1, "status", connected(25565));
        e.app.pump().unwrap();
        assert_eq!(e.connection("s1")["state"], "connecting");
        // A second join is refused while one is pending.
        assert_eq!(
            e.app.join("s1").unwrap_err(),
            "already connecting or connected"
        );
        assert_eq!(e.calls("join").len(), 1);
        e.push(
            "s1",
            1,
            "join_end",
            json!({"hasTunnel":true,"failed":false,"status":connected(25565)}),
        );
        e.app.pump().unwrap();
        let view = e.connection("s1");
        assert_eq!(view["state"], "connected");
        assert_eq!(view["localAddr"], "127.0.0.1:25565");
        assert_eq!(e.last_log()["kind"], "connect");
        assert!(e.last_log()["message"]
            .as_str()
            .unwrap()
            .contains("Connected to \u{201c}Server s1\u{201d} at 127.0.0.1:25565 (direct, 3 ms)"));
    }

    // A port the user pinned must be bound exactly (the bridge fails the join
    // if it is busy); only automatic stable ports may fall back.
    #[test]
    fn join_requires_a_pinned_local_port_exactly() {
        let mut e = env();
        e.mc("s1");
        e.mc("s2");
        e.app
            .mutate(
                "update",
                "s1",
                json!({"name":"Server s1","description":"","localPort":40000}),
            )
            .unwrap();
        e.app.join("s1").unwrap();
        let request = e.calls("join").remove(0);
        assert_eq!(request["options"]["LocalPort"], 40000);
        assert_eq!(request["options"]["PreferPort"], 40000);
        assert_eq!(request["options"]["ReservedPorts"], json!([20000]));
    }

    #[test]
    fn startup_logs_a_pinned_port_reset_by_migration() {
        let mut e = env();
        e.mc("s1");
        e.mc("s2");
        let path = e.dir.path().join("config.json");
        let mut legacy: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for server in legacy["servers"].as_array_mut().unwrap() {
            server["localPort"] = json!(30000);
            server.as_object_mut().unwrap().remove("stablePort");
        }
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        e.app = App::load_in(e.dir.path().to_path_buf()).unwrap();
        assert_eq!(e.app.server("s1").unwrap()["localPort"], 30000);
        assert_eq!(e.app.server("s2").unwrap()["localPort"], 0);
        let log = e.last_log();
        assert_eq!(log["kind"], "warning");
        let message = log["message"].as_str().unwrap();
        assert!(
            message.contains("Server s2") && message.contains("30000"),
            "{message}"
        );
        assert!(!message.contains("Server s1"), "{message}");
    }

    // The first Java server's stable port is 25565; if something else holds it
    // the tunnel falls back, and the list entry must follow the bound port.
    #[test]
    fn minecraft_list_entry_uses_the_port_the_tunnel_bound() {
        let mut e = env();
        e.mc("s1");
        let game_dir = e.dir.path().join("minecraft-home");
        e.app.minecraft.default_dir = Some(game_dir.clone());
        e.app.cfg["settings"]["minecraftServerList"] = json!(true);
        let server = e.app.server("s1").unwrap();
        assert_eq!(server["stablePort"], 25565);
        let listed = |port: u16| {
            let raw = fs::read(game_dir.join("servers.dat")).unwrap();
            let needle = format!("127.0.0.1:{port}");
            raw.windows(needle.len()).any(|w| w == needle.as_bytes())
        };
        e.connect("s1", 41234);
        assert!(listed(41234) && !listed(25565));
        assert_eq!(e.app.list_port(&server), 41234);
        e.command(json!({"method":"disconnect","id":"s1"})).unwrap();
        assert_eq!(e.app.list_port(&server), 25565);
    }

    #[test]
    fn join_rejects_unknown_servers_and_unreadable_addresses_before_any_effect() {
        let mut e = env();
        assert_eq!(e.app.join("nope").unwrap_err(), "server not found");
        e.mc("s1");
        // A sealed address that cannot be opened (for example another user's).
        e.app.mutate("remove", "s1", Value::Null).unwrap();
        e.app
            .mutate(
                "add",
                "",
                json!({"id":"s1","name":"Bad","description":"","game":"minecraft-java","proto":"tcp","port":25565,"localPort":0,
                       "addr":"not-sealed","addedAt":"2026-01-01T00:00:00Z"}),
            )
            .unwrap();
        assert_eq!(e.app.join("s1").unwrap_err(), "unrecognised secret format");
        assert!(e.calls("join").is_empty());
        assert!(
            !e.app.states.contains_key("s1"),
            "no lifecycle state was created"
        );
    }

    #[test]
    fn expired_saved_invitation_cannot_start_a_join() {
        let mut e = env();
        let stored = policy(
            "desktop.server_input",
            json!({"import":true,"value":{"name":"Expired","game":"minecraft-java","proto":"tcp","port":25565,
                   "addr":"synthetic-address","expiresAt":"2000-01-01T00:00:00Z"}}),
        )
        .unwrap();
        let view = e.app.save_server(stored, "Imported").unwrap();
        e.app.cfg = config::dispatch(
            "config.client_open",
            json!({"path":e.dir.path().join("config.json")}),
        )
        .unwrap();
        let id = text(&view, "id");
        assert!(e.app.join(id).unwrap_err().contains("expired"));
        assert!(e.calls("join").is_empty());
    }

    // When the bridge rejects the join outright, no join_end will ever arrive,
    // so the host reports the failure itself: the display must leave
    // "connecting" and a plain retry must work without a Disconnect first.
    #[test]
    fn failed_bridge_join_shows_an_error_and_can_be_retried() {
        let mut e = env();
        e.mc("s1");
        e.fail("join", "resource must be stopped before replacement");
        assert_eq!(
            e.app.join("s1").unwrap_err(),
            "resource must be stopped before replacement"
        );
        assert!(e.app.resources.is_empty());
        assert_eq!(e.connection("s1")["state"], "error");
        e.script.borrow_mut().errors.clear();
        e.app.join("s1").unwrap();
        assert_eq!(e.connection("s1")["state"], "connecting");
        assert!(e.app.resources.contains_key("s1"));
        assert_eq!(e.calls("join").len(), 2);
    }

    // TestJoinFailurePreservesFriendlyError
    #[test]
    fn failed_join_reports_a_friendly_error_and_releases_the_resource() {
        let mut e = env();
        e.mc("s1");
        e.app.join("s1").unwrap();
        e.push(
            "s1",
            1,
            "join_end",
            json!({"hasTunnel":false,"failed":true,"cancelled":false,"noResponse":false,"error":"connection refused"}),
        );
        e.app.pump().unwrap();
        let view = e.connection("s1");
        assert_eq!(view["state"], "error");
        assert_eq!(view["message"], "Connection refused");
        assert_eq!(e.lifecycle(), ["join:s1:1", "stop:s1:1"]);
        assert!(e.app.resources.is_empty());
        assert_eq!(e.last_log()["kind"], "error");
        assert_eq!(
            e.last_log()["message"],
            "Could not connect to \u{201c}Server s1\u{201d}: Connection refused"
        );
        // A retry after an error is allowed and gets a fresh generation.
        e.app.join("s1").unwrap();
        assert_eq!(e.lifecycle().last().unwrap(), "join:s1:2");
    }

    #[test]
    fn no_response_is_reported_as_unreachable_and_is_a_warning() {
        let mut e = env();
        e.mc("s1");
        e.app.join("s1").unwrap();
        e.push(
            "s1",
            1,
            "join_end",
            json!({"failed":true,"noResponse":true,"error":"no response from server"}),
        );
        e.app.pump().unwrap();
        assert_eq!(e.connection("s1")["state"], "unreachable");
        assert_eq!(e.last_log()["kind"], "warning");
    }

    // TestStaleJoinSuccessClosesBeforeReplacementBinds
    #[test]
    fn stale_generation_callbacks_cannot_change_a_newer_join() {
        let mut e = env();
        e.mc("s1");
        e.app.join("s1").unwrap();
        e.app
            .command(json!({"method":"disconnect","id":"s1"}))
            .unwrap();
        // The old generation is stopped before the replacement binds.
        assert_eq!(e.lifecycle(), ["join:s1:1", "stop:s1:1"]);
        e.app.join("s1").unwrap();
        assert_eq!(e.lifecycle(), ["join:s1:1", "stop:s1:1", "join:s1:2"]);
        let before = e.app.states["s1"].clone();
        let activity = e.app.activity.len();
        // Late callbacks from generation 1: a successful join that returned a
        // tunnel, a status, and an error.
        e.push(
            "s1",
            1,
            "join_end",
            json!({"hasTunnel":true,"failed":false,"status":connected(25565)}),
        );
        e.push("s1", 1, "status", json!({"state":"error"}));
        e.app.pump().unwrap();
        assert_eq!(e.app.states["s1"], before, "stale callback mutated state");
        assert_eq!(e.app.activity.len(), activity);
        assert_eq!(e.app.resources["s1"], 2);
        assert_eq!(e.calls("join").len(), 2);
        // The current generation still completes normally.
        e.push(
            "s1",
            2,
            "join_end",
            json!({"hasTunnel":true,"failed":false,"status":connected(25565)}),
        );
        e.app.pump().unwrap();
        assert_eq!(e.connection("s1")["state"], "connected");
    }

    #[test]
    fn terminal_events_are_applied_after_statuses_regardless_of_arrival_order() {
        let mut e = env();
        e.mc("s1");
        e.app.join("s1").unwrap();
        // The terminal event arrives first, but the status carries its final value.
        e.push(
            "s1",
            1,
            "join_end",
            json!({"hasTunnel":true,"failed":false,"status":{"state":"connecting"}}),
        );
        e.push("s1", 1, "status", connected(25565));
        e.app.pump().unwrap();
        // Status was applied first (ignored while pending), then join_end; the
        // tunnel is up with the join_end status.
        assert_eq!(e.app.states["s1"]["tunnel"], true);
    }

    #[test]
    fn disconnect_stops_the_exact_generation_and_clears_ownership() {
        let mut e = env();
        e.mc("s1");
        e.connect("s1", 25565);
        e.app
            .command(json!({"method":"disconnect","id":"s1"}))
            .unwrap();
        assert_eq!(e.lifecycle(), ["join:s1:1", "stop:s1:1"]);
        assert!(e.app.resources.is_empty());
        assert_eq!(e.connection("s1")["state"], "disconnected");
        assert_eq!(e.last_log()["kind"], "disconnect");
        // Disconnecting again is harmless and stops nothing more.
        e.app
            .command(json!({"method":"disconnect","id":"s1"}))
            .unwrap();
        assert_eq!(e.calls("stop").len(), 1);
    }

    // TestRetryRetainsBusyPortGuardUntilTunnelClosed
    #[test]
    fn retry_stops_the_predecessor_first_and_the_port_guard_persists_while_unreachable() {
        let mut e = env();
        e.mc("s1");
        e.app.join("s1").unwrap();
        e.push(
            "s1",
            1,
            "join_end",
            json!({"hasTunnel":true,"failed":false,"status":{"state":"unreachable"}}),
        );
        e.app.pump().unwrap();
        assert_eq!(e.connection("s1")["state"], "unreachable");
        // An unreachable resource still holds its local port.
        let error = e
            .command(json!({"method":"updateServer","id":"s1","name":"World","description":"","localPort":12345}))
            .unwrap_err();
        assert_eq!(error, "disconnect before changing the local port");
        assert_eq!(e.app.server("s1").unwrap()["localPort"], 0);
        // Retrying stops generation 1 before generation 2 binds.
        e.app.join("s1").unwrap();
        assert_eq!(e.lifecycle(), ["join:s1:1", "stop:s1:1", "join:s1:2"]);
        assert_eq!(e.app.resources["s1"], 2);
    }

    #[test]
    fn local_port_can_change_when_idle_and_renames_are_always_allowed() {
        let mut e = env();
        e.mc("s1");
        let view = e
            .command(json!({"method":"updateServer","id":"s1","name":"  Renamed ","description":" d ","localPort":30000}))
            .unwrap();
        assert_eq!(view["name"], "Renamed");
        assert_eq!(view["description"], "d");
        assert_eq!(view["localPort"], 30000);
        e.connect("s1", 30000);
        let view = e
            .command(json!({"method":"updateServer","id":"s1","name":"Again","description":"","localPort":30000}))
            .unwrap();
        assert_eq!(view["name"], "Again");
        assert_eq!(view["connection"]["state"], "connected");
        assert!(e
            .command(json!({"method":"updateServer","id":"s1","name":"x","description":"","localPort":70000}))
            .is_err());
        assert_eq!(
            e.command(json!({"method":"updateServer","id":"missing","name":"x","description":"","localPort":0}))
                .unwrap_err(),
            "server not found"
        );
    }

    // TestRustClientWriteFailurePreservesMemory
    #[test]
    fn a_failed_config_write_leaves_the_in_memory_configuration_unchanged() {
        let mut e = env();
        e.mc("s1");
        e.break_config_file();
        let before = e.app.cfg.clone();
        assert!(e
            .command(json!({"method":"updateServer","id":"s1","name":"after","description":"","localPort":0}))
            .is_err());
        assert_eq!(e.app.cfg, before);
        assert_eq!(e.app.server("s1").unwrap()["name"], "Server s1");
        let settings = json!({"demoMode":true,"autoReconnect":true});
        assert!(e
            .command(json!({"method":"updateSettings","settings":settings}))
            .is_err());
        assert_eq!(e.app.cfg, before);
        assert!(e.command(json!({"method":"settings"})).unwrap()["demoMode"] != true);
        assert_eq!(
            e.app.updates & 1,
            0,
            "a failed update must not announce a change"
        );
    }

    #[test]
    fn remove_server_restores_the_lifecycle_when_the_write_fails_and_logs_on_success() {
        let mut e = env();
        e.mc("s1");
        e.break_config_file();
        assert!(e
            .command(json!({"method":"removeServer","id":"s1"}))
            .is_err());
        assert!(e.app.server("s1").is_ok());
        // The failed removal did not leave the entry closed: it can still join.
        e.app.join("s1").unwrap();
        e.app
            .command(json!({"method":"disconnect","id":"s1"}))
            .unwrap();

        let dir = e.dir.path().join("config.json");
        fs::remove_dir(&dir).unwrap();
        e.mc("s2");
        e.connect("s2", 25565);
        e.command(json!({"method":"removeServer","id":"s2"}))
            .unwrap();
        assert!(e.app.server("s2").is_err());
        // Removal closed the live tunnel and logged the removal.
        assert_eq!(e.lifecycle().last().unwrap(), "stop:s2:1");
        assert_eq!(
            e.last_log()["message"],
            "Removed server \u{201c}Server s2\u{201d}"
        );
        assert_eq!(
            e.command(json!({"method":"removeServer","id":"s2"}))
                .unwrap_err(),
            "server not found"
        );
        // A removed id cannot be re-joined while closed: its lifecycle is closed.
        assert_eq!(e.app.join("s2").unwrap_err(), "server not found");
    }

    #[test]
    fn check_probes_the_server_and_releases_the_probe_when_it_ends() {
        let mut e = env();
        e.mc("s1");
        let started = e.app.check("s1").unwrap();
        assert_eq!(started["state"], "checking");
        assert_eq!(e.lifecycle(), ["check:s1:1"]);
        let request = e.calls("check").remove(0);
        assert_eq!(request["address"], "synthetic-address");
        assert_eq!(request["options"]["Game"], "minecraft-java");
        assert_eq!(request["options"]["Proto"], "tcp");
        assert_eq!(request["options"]["TunnelPort"], 25565);
        assert_eq!(request["allowLocalRelay"], false);
        assert_eq!(e.app.resources["s1"], 1);
        // A duplicate request while probing does not start a second probe.
        assert_eq!(e.app.check("s1").unwrap()["state"], "checking");
        assert_eq!(e.calls("check").len(), 1);

        e.push(
            "s1",
            1,
            "check_end",
            json!({"outcome":"online","gameReachable":"yes","latencyMs":12.5,"path":"direct"}),
        );
        e.app.pump().unwrap();
        let view = e.check_view("s1");
        assert_eq!(view["state"], "online");
        assert_eq!(view["latencyMs"], 12.5);
        assert_eq!(e.lifecycle(), ["check:s1:1", "stop:s1:1"]);
        assert!(e.app.resources.is_empty());
    }

    #[test]
    fn check_outcomes_error_and_cancelled() {
        let mut e = env();
        e.mc("s1");
        e.app.check("s1").unwrap();
        e.push("s1", 1, "check_end", json!({"outcome":"error"}));
        e.app.pump().unwrap();
        let view = e.check_view("s1");
        assert_eq!(view["state"], "unreachable");
        assert!(view["message"].as_str().unwrap().starts_with("No response"));
        // A cancelled probe restores the previous result.
        e.app.check("s1").unwrap();
        e.push("s1", 2, "check_end", json!({"outcome":"cancelled"}));
        e.app.pump().unwrap();
        assert_eq!(e.check_view("s1")["state"], "unreachable");
    }

    #[test]
    fn check_reports_unknown_servers_and_unreadable_addresses_without_probing() {
        let mut e = env();
        assert_eq!(e.app.check("nope").unwrap()["state"], "unknown");
        e.app
            .mutate(
                "add",
                "",
                json!({"id":"s1","name":"Bad","description":"","game":"minecraft-java","proto":"tcp","port":25565,"localPort":0,
                       "addr":"not-sealed","addedAt":"2026-01-01T00:00:00Z"}),
            )
            .unwrap();
        let result = e.app.check("s1").unwrap();
        assert_eq!(result["state"], "unreachable");
        assert_eq!(result["message"], "Cannot read the saved address");
        assert!(e.calls("check").is_empty());
        assert!(e.app.resources.is_empty());
    }

    // TestCheckDuplicateCancellationRestoreAndRemoveWait (host half) and the
    // connected shortcut from TestJoinMarksPending...
    #[test]
    fn check_while_connected_answers_from_the_live_status_and_join_cancels_a_probe() {
        let mut e = env();
        e.mc("s1");
        e.connect("s1", 25565);
        let result = e.app.check("s1").unwrap();
        assert_eq!(result["state"], "online");
        assert_eq!(result["message"], "Game server answered");
        assert!(e.calls("check").is_empty());

        let mut e = env();
        e.mc("s1");
        e.app.check("s1").unwrap();
        // Joining while probing cancels the probe (stop of its generation) first.
        e.app.join("s1").unwrap();
        assert_eq!(e.lifecycle(), ["check:s1:1", "stop:s1:1", "join:s1:2"]);
        // The cancelled probe's late result is inert.
        let before = e.app.states["s1"].clone();
        e.push(
            "s1",
            1,
            "check_end",
            json!({"outcome":"online","gameReachable":"yes","latencyMs":1.0,"path":"direct"}),
        );
        e.app.pump().unwrap();
        assert_eq!(e.app.states["s1"], before);
    }

    #[test]
    fn check_all_probes_servers_one_at_a_time() {
        let mut e = env();
        e.mc("s1");
        e.mc("s2");
        e.command(json!({"method":"checkAll"})).unwrap();
        e.app.pump().unwrap();
        assert_eq!(e.lifecycle(), ["check:s1:1"]);
        // While s1 is still checking, nothing else starts.
        e.app.pump().unwrap();
        e.app.pump().unwrap();
        assert_eq!(e.lifecycle(), ["check:s1:1"]);
        e.push(
            "s1",
            1,
            "check_end",
            json!({"outcome":"online","gameReachable":"yes","latencyMs":1.0,"path":"direct"}),
        );
        e.app.pump().unwrap();
        assert_eq!(e.lifecycle(), ["check:s1:1", "stop:s1:1", "check:s2:1"]);
    }

    #[test]
    fn background_checks_use_the_probe_and_ignore_completions_after_opt_out() {
        let mut e = env();
        e.mc("s1");
        e.app.startup = None;
        e.app.pump().unwrap();
        assert!(e.calls("check").is_empty());
        e.app
            .presence
            .set_enabled(true, Instant::now() - Duration::from_secs(135));
        e.app.pump().unwrap();
        let first = e.app.resources["s1"];
        assert_eq!(e.app.presence.active.as_ref().unwrap().1, first);
        e.push("s1", first, "check_end", json!({"outcome":"error"}));
        e.app.pump().unwrap();
        assert!(e.app.presence.take_notice(Instant::now()).is_none());

        e.app.check("s1").unwrap();
        let second = e.app.resources["s1"];
        e.push(
            "s1",
            second,
            "check_end",
            json!({"outcome":"online","gameReachable":"yes"}),
        );
        e.app.pump().unwrap();
        assert_eq!(
            e.app.presence.take_notice(Instant::now()).as_deref(),
            Some("s1")
        );

        let mut disabled = e.app.cfg["settings"].clone();
        disabled["presenceNotifications"] = json!(false);
        e.app.presence = Presence::default();
        e.app
            .presence
            .set_enabled(true, Instant::now() - Duration::from_secs(135));
        e.app.pump().unwrap();
        let pending = e.app.resources["s1"];
        e.command(json!({"method":"updateSettings","settings":disabled}))
            .unwrap();
        assert!(!e.app.resources.contains_key("s1"));
        e.push("s1", pending, "check_end", json!({"outcome":"error"}));
        e.app.pump().unwrap();
        assert_eq!(e.check_view("s1")["state"], "online");
        assert!(e.app.presence.take_notice(Instant::now()).is_none());
    }

    #[test]
    fn factorio_presence_requires_a_game_reply() {
        let mut e = env();
        e.add("factorio", "factorio", "udp", 34197);
        let app = &mut e.app;
        app.presence.set_enabled(true, Instant::now());
        app.observe_presence("factorio", &json!({"state":"unreachable"}), false);
        app.observe_presence("factorio", &json!({"state":"unknown"}), true);
        assert!(app.presence.take_notice(Instant::now()).is_none());

        app.observe_presence("factorio", &json!({"state":"online"}), true);
        assert_eq!(
            app.presence.take_notice(Instant::now()).as_deref(),
            Some("factorio")
        );
    }

    #[test]
    fn startup_checks_begin_once_the_deadline_passes() {
        let mut e = env();
        e.mc("s1");
        e.app.pump().unwrap();
        assert!(
            e.calls("check").is_empty(),
            "the startup delay has not elapsed"
        );
        e.app.startup = Some(Instant::now());
        e.app.pump().unwrap();
        assert_eq!(e.lifecycle(), ["check:s1:1"]);
        assert!(e.app.startup.is_none());
    }

    #[test]
    fn demo_servers_simulate_a_connection_without_touching_the_network() {
        let mut e = env();
        assert_eq!(e.app.join("demo-mc").unwrap_err(), "demo mode is off");
        let mut settings = e.app.cfg["settings"].clone();
        settings["demoMode"] = json!(true);
        e.app.mutate("settings", "", settings).unwrap();
        assert_eq!(e.app.join("demo-nope").unwrap_err(), "server not found");

        e.app.join("demo-mc").unwrap();
        assert_eq!(e.app.timers["demo-mc"].2, 25565);
        assert!(e.calls("join").is_empty(), "demo must not reach the bridge");
        assert_eq!(e.connection("demo-mc")["state"], "connecting");
        let servers = e.app.servers().unwrap();
        assert_eq!(servers.as_array().unwrap().len(), 3);
        assert_eq!(servers[0]["demo"], true);
        assert_eq!(servers[0]["connection"]["state"], "connecting");

        // The simulated connection completes when its timer fires.
        e.app.timers.get_mut("demo-mc").unwrap().1 = Instant::now();
        e.app.pump().unwrap();
        assert!(e.app.timers.is_empty());
        let view = e.connection("demo-mc");
        assert_eq!(view["state"], "connected");
        assert_eq!(view["localAddr"], "127.0.0.1:25565");
        assert_eq!(view["simulated"], true);

        e.app
            .command(json!({"method":"disconnect","id":"demo-mc"}))
            .unwrap();
        assert_eq!(e.connection("demo-mc")["state"], "disconnected");
    }

    #[test]
    fn replacing_a_demo_join_discards_the_old_timer() {
        let mut e = env();
        let mut settings = e.app.cfg["settings"].clone();
        settings["demoMode"] = json!(true);
        e.app.mutate("settings", "", settings).unwrap();
        e.app.join("demo-bedrock").unwrap();
        let first = e.app.timers["demo-bedrock"].0;
        e.app
            .command(json!({"method":"disconnect","id":"demo-bedrock"}))
            .unwrap();
        e.app.join("demo-bedrock").unwrap();
        let second = e.app.timers["demo-bedrock"].0;
        assert_eq!(second, first + 1);
        // A timer for the old generation firing late changes nothing.
        let before = e.app.states["demo-bedrock"].clone();
        e.app
            .step(
                "demo-bedrock",
                json!({"type":"demo_timer","generation":first,"port":19132,"now":now()}),
            )
            .unwrap();
        assert_eq!(e.app.states["demo-bedrock"], before);
        assert_eq!(e.app.timers["demo-bedrock"].2, 19132);
    }

    #[test]
    fn shutdown_closes_every_resource_then_shuts_the_bridge_down() {
        let mut e = env();
        e.mc("s1");
        e.mc("s2");
        e.connect("s1", 25565);
        e.app.check("s2").unwrap();
        e.app.checks.push("s2".into());
        e.app.startup = Some(Instant::now());
        e.app.shutdown().unwrap();
        let tail: Vec<_> = e.lifecycle().into_iter().skip(2).collect();
        assert_eq!(tail, ["stop:s1:1", "stop:s2:1", "shutdown"]);
        assert!(e.app.timers.is_empty());
        assert!(e.app.checks.is_empty());
        assert!(e.app.startup.is_none());
    }

    #[test]
    fn shutdown_reports_failures_but_still_shuts_the_bridge_down() {
        let mut e = env();
        e.mc("s1");
        e.connect("s1", 25565);
        e.fail("stop", "stop failed");
        assert_eq!(e.app.shutdown().unwrap_err(), "stop failed");
        assert_eq!(e.lifecycle().last().unwrap(), "shutdown");
        let mut e = env();
        e.fail("shutdown", "close failed");
        assert_eq!(e.app.shutdown().unwrap_err(), "close failed");
    }

    #[test]
    fn open_initialises_the_bridge_and_shuts_it_down_if_loading_fails() {
        let script = script();
        let _bridge = fake(&script);
        let calls = || -> Vec<String> {
            script
                .borrow()
                .calls
                .iter()
                .map(|c| c["method"].as_str().unwrap().to_owned())
                .collect()
        };
        // init failing short-circuits before anything is loaded.
        script
            .borrow_mut()
            .errors
            .insert("init".into(), "already".into());
        assert_eq!(App::open().err().unwrap(), "already");
        assert_eq!(calls(), ["init"]);
    }

    #[test]
    fn stats_are_attached_to_lifecycle_events_for_the_matching_server_only() {
        let mut e = env();
        e.mc("s1");
        e.mc("s2");
        e.connect("s1", 25565);
        e.script.borrow_mut().stats =
            json!({"s1":{"state":"reconnecting","localAddr":"127.0.0.1:25565","localPort":25565}});
        assert_eq!(e.connection("s1")["state"], "reconnecting");
        assert_eq!(e.connection("s2")["state"], "disconnected");
    }

    #[test]
    fn pump_writes_bridge_log_lines_and_rotates_a_large_log() {
        let mut e = env();
        e.push("s1", 1, "log", json!("first line"));
        e.app.pump().unwrap();
        let path = e.dir.path().join("gamelink.log");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.trim_end().ends_with("first line"), "{text}");
        fs::write(&path, vec![b'x'; (2 << 20) + 1]).unwrap();
        e.push("s1", 1, "log", json!("after rotation"));
        e.app.pump().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.len() < 200 && text.contains("after rotation"),
            "{text}"
        );
    }

    #[test]
    fn informational_commands() {
        let mut e = env();
        assert_eq!(
            e.command(json!({"method":"version"})).unwrap()["app"],
            "GameLink"
        );
        let identity = e.command(json!({"method":"identity"})).unwrap();
        assert_eq!(identity["publicKey"], "nodekey:devicepublickey");
        assert!(identity.get("privateKey").is_none());
        #[cfg(windows)]
        let protection = "Windows DPAPI (current user) plus user-profile file permissions";
        #[cfg(not(windows))]
        let protection = "file permissions (0600)";
        assert_eq!(identity["protection"], protection);
        let paths = e.command(json!({"method":"paths"})).unwrap();
        assert!(paths["configFile"]
            .as_str()
            .unwrap()
            .ends_with("config.json"));
        assert!(paths["logFile"].as_str().unwrap().ends_with("gamelink.log"));
        let settings = e.command(json!({"method":"settings"})).unwrap();
        assert_eq!(settings["autoReconnect"], true);
        assert_eq!(
            e.command(json!({"method":"bogus"})).unwrap_err(),
            "unknown desktop command"
        );
        // The `updates` mask is read-and-clear.
        let bits = e.command(json!({"method":"updates"})).unwrap();
        assert_ne!(bits, 0, "identity creation was logged");
        assert_eq!(e.command(json!({"method":"updates"})).unwrap(), 0);
        let overview = e.command(json!({"method":"overview"})).unwrap();
        assert_eq!(overview["status"], "idle");
        assert_eq!(overview["publicKeyShort"], "nodekey:devicepubli\u{2026}");
    }

    #[test]
    fn activity_log_limit_clear_and_redaction() {
        let mut e = env();
        for i in 0..5 {
            e.app.add_log("info", "", &format!("entry {i}"));
        }
        e.app.add_log(
            "error",
            "",
            &format!("failed with privkey:{}", "ab".repeat(16)),
        );
        let newest = e.command(json!({"method":"activity","limit":2})).unwrap();
        assert_eq!(newest.as_array().unwrap().len(), 2);
        assert_eq!(newest[1]["message"], "entry 4");
        assert!(!newest[0]["message"].as_str().unwrap().contains("abab"));
        assert!(
            e.command(json!({"method":"activity"}))
                .unwrap()
                .as_array()
                .unwrap()
                .len()
                >= 6
        );
        // The log persists across restarts.
        assert!(fs::read_to_string(e.dir.path().join("activity.json"))
            .unwrap()
            .contains("entry 4"));
        e.command(json!({"method":"clearActivity"})).unwrap();
        assert!(e
            .command(json!({"method":"activity"}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        for _ in 0..210 {
            e.app.add_log("info", "", "x");
        }
        assert_eq!(e.app.activity.len(), 200);
    }

    #[test]
    fn settings_report_whether_this_build_can_check_for_updates() {
        let mut e = env();
        let expected = json!(gamelink_core::update::can_check());
        let view = e.command(json!({"method":"settings"})).unwrap();
        assert_eq!(view["canCheckForUpdates"], expected);
        let view = e
            .command(json!({"method":"updateSettings","settings":{"checkForUpdates":true}}))
            .unwrap();
        assert_eq!(view["canCheckForUpdates"], expected);
    }

    #[test]
    fn update_settings_validates_persists_and_announces() {
        let mut e = env();
        e.command(json!({"method":"updates"})).unwrap();
        let view = e
            .command(json!({"method":"updateSettings","settings":{"demoMode":true,"autoReconnect":false,"checkForUpdates":true,"gameLaunchArgs":{"factorio":"--windowed"}}}))
            .unwrap();
        assert_eq!(view["demoMode"], true);
        assert_eq!(view["autoReconnect"], false);
        assert_eq!(view["checkForUpdates"], true);
        assert_eq!(view["gameLaunchArgs"]["factorio"], "--windowed");
        assert_eq!(e.command(json!({"method":"updates"})).unwrap(), 5);
        let saved: Value =
            serde_json::from_slice(&fs::read(e.dir.path().join("config.json")).unwrap()).unwrap();
        assert_eq!(saved["settings"]["demoMode"], true);
        assert_eq!(saved["settings"]["checkForUpdates"], true);
        assert!(e
            .command(json!({"method":"updateSettings","settings":{"gameExecutables":{"factorio":"relative"}}}))
            .is_err());
        assert!(e
            .command(json!({"method":"updateSettings","settings":{"gameLaunchArgs":{"nope":"x"}}}))
            .is_err());
    }

    #[test]
    fn add_server_validates_the_address_seals_it_and_queues_a_check() {
        let mut e = env();
        let form = json!({"name":" Friends ","description":"","game":"factorio","proto":"udp","port":34197,"localPort":0,"address":" tcAbc123 "});
        let view = e
            .command(json!({"method":"addServer","form":form}))
            .unwrap();
        assert_eq!(view["name"], "Friends");
        assert_eq!(view["gameName"], "Factorio");
        assert_eq!(view["serverKey"], "nodekey:servervalue");
        assert_eq!(view["defaultRelay"], true);
        // The full server key is enough to knock; the UI gets a fingerprint.
        assert!(!view.to_string().contains("serverfullvalue"));
        assert_eq!(
            view["serverFingerprint"],
            gamelink_core::requests::server_fingerprint("nodekey:serverfullvalue")
        );
        let id = view["id"].as_str().unwrap().to_owned();
        let asked = e.calls("validate");
        assert_eq!(asked[0]["address"], " tcAbc123 ");
        assert_eq!(asked[0]["allowLocalRelay"], false);
        let stored = e.app.server(&id).unwrap();
        assert_eq!(
            storage::unseal(stored["addr"].as_str().unwrap()).unwrap(),
            "tcAbc123"
        );
        assert!(stored.get("address").is_none());
        let on_disk = fs::read_to_string(e.dir.path().join("config.json")).unwrap();
        assert!(!on_disk.contains("tcAbc123") || cfg!(not(windows)));
        assert!(e.app.checks.contains(&id));
        assert_eq!(
            e.last_log()["message"],
            "Added server \u{201c}Friends\u{201d}"
        );
        // A rejected address stores nothing.
        let before = e.app.cfg.clone();
        e.fail(
            "validate",
            "the server address is not a valid Tailcat address",
        );
        let error = e
            .command(json!({"method":"addServer","form":{"name":"X","game":"factorio","proto":"udp","port":34197,"address":"bad"}}))
            .unwrap_err();
        assert_eq!(error, "the server address is not a valid Tailcat address");
        assert_eq!(e.app.cfg, before);
    }

    #[test]
    fn invitations_preview_without_the_address_and_import_with_the_chosen_local_port() {
        let mut e = env();
        let invite = pure::dispatch(
            "invite.encode",
            json!({"name":"Imported","game":"factorio","proto":"udp","port":34197,"addr":"tcInvite123","note":"hello"}),
        )
        .unwrap();
        let preview = e
            .command(json!({"method":"previewInvite","text":invite}))
            .unwrap();
        assert!(preview.get("addr").is_none());
        assert_eq!(preview["gameName"], "Factorio");
        assert_eq!(preview["warnings"], json!([]));
        assert!(e.app.cfg["servers"].as_array().is_none_or(Vec::is_empty));

        let view = e
            .command(json!({"method":"importInvite","text":invite,"localPort":31000}))
            .unwrap();
        assert_eq!(view["name"], "Imported");
        assert_eq!(view["description"], "hello");
        assert_eq!(view["localPort"], 31000);
        assert_eq!(
            e.last_log()["message"],
            "Imported server \u{201c}Imported\u{201d}"
        );

        assert!(e
            .command(json!({"method":"previewInvite","text":"not an invitation"}))
            .is_err());
        // Custom relays require an explicit warning.
        e.script.borrow_mut().validate =
            json!({"relayHosts":["relay.example"],"defaultRelay":false,"serverKey":"k"});
        let preview = e
            .command(json!({"method":"previewInvite","text":invite}))
            .unwrap();
        assert!(preview["warnings"][0]
            .as_str()
            .unwrap()
            .contains("relay.example"));
        // Console-bridge invitations are for Android only.
        let bridge_invite = pure::dispatch(
            "invite.encode",
            json!({"name":"B","game":"minecraft-bedrock","proto":"udp","port":19132,"addr":"tcBridge1","consoleBridge":1}),
        )
        .unwrap();
        let error = e
            .command(json!({"method":"previewInvite","text":bridge_invite}))
            .unwrap_err();
        assert!(error.contains("Android Console Bridge"), "{error}");
        assert!(e
            .command(json!({"method":"importInvite","text":bridge_invite}))
            .is_err());
    }

    #[test]
    fn servers_overview_and_diagnostics_never_expose_the_address() {
        let mut e = env();
        e.mc("s1");
        e.connect("s1", 25565);
        let servers = e.command(json!({"method":"servers"})).unwrap();
        assert_eq!(servers[0]["connection"]["state"], "connected");
        assert_eq!(servers[0]["hasExecutable"], false);
        assert_eq!(servers[0]["defaultPort"], 25565);
        assert!(!servers.to_string().contains("synthetic-address"));
        let overview = e.command(json!({"method":"overview"})).unwrap();
        assert_eq!(
            (overview["servers"].as_i64(), overview["connected"].as_i64()),
            (Some(1), Some(1))
        );
        assert_eq!(overview["status"], "connected");
        let report = e
            .command(json!({"method":"diagnostics","id":"s1"}))
            .unwrap();
        let report = report.as_str().unwrap();
        assert!(report.starts_with(concat!(
            "GameLink ",
            env!("CARGO_PKG_VERSION"),
            " diagnostics\nServer: Server s1 (Minecraft: Java Edition)"
        )));
        assert!(report.contains("Local address: 127.0.0.1:25565"));
        assert!(report.contains("Path: direct"));
        assert!(report.contains("latency 3.0 ms"));
        assert!(report.contains("This device: nodekey:devicepublickey"));
        assert!(report.ends_with("No secrets are included in this report.\n"));
        assert!(!report.contains("synthetic-address"));
        assert_eq!(
            e.command(json!({"method":"diagnostics","id":"nope"}))
                .unwrap_err(),
            "server not found"
        );
    }

    #[test]
    fn view_marks_servers_with_a_configured_executable() {
        let mut e = env();
        e.mc("s1");
        let mut settings = e.app.cfg["settings"].clone();
        settings["gameExecutables"] = json!({"minecraft-java":"/bin/true"});
        e.app.mutate("settings", "", settings).unwrap();
        let servers = e.command(json!({"method":"servers"})).unwrap();
        assert_eq!(servers[0]["hasExecutable"], true);
    }

    #[test]
    fn launch_requires_a_connection_and_reports_unstartable_programs() {
        let mut e = env();
        e.add("s1", "factorio", "udp", 34197);
        assert_eq!(
            e.command(json!({"method":"launchGame","id":"s1"}))
                .unwrap_err(),
            "join the server first"
        );
        assert_eq!(
            e.command(json!({"method":"launchGame","id":"nope"}))
                .unwrap_err(),
            "server not found"
        );
        e.connect("s1", 34197);
        // Connected but nothing configured for this game.
        let error = e
            .command(json!({"method":"launchGame","id":"s1"}))
            .unwrap_err();
        assert!(error.starts_with("no executable is configured"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn launch_runs_the_configured_program_with_the_expanded_local_address() {
        use std::os::unix::fs::PermissionsExt;
        let mut e = env();
        e.add("s1", "factorio", "udp", 34197);
        let exe = e.dir.path().join("factorio");
        fs::write(
            &exe,
            "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/args.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let mut settings = e.app.cfg["settings"].clone();
        settings["gameExecutables"] = json!({"factorio":exe});
        e.app.mutate("settings", "", settings).unwrap();
        e.connect("s1", 34197);
        let result = e.command(json!({"method":"launchGame","id":"s1"})).unwrap();
        assert_eq!(
            result,
            json!({"address":"127.0.0.1:34197","autoConnect":true})
        );
        assert_eq!(
            e.last_log()["message"],
            "Launched factorio and asked it to join 127.0.0.1:34197"
        );
        let out = e.dir.path().join("args.txt");
        for _ in 0..200 {
            if let Ok(text) = fs::read_to_string(&out) {
                if text.ends_with('\n') {
                    assert_eq!(text.trim(), "--mp-connect 127.0.0.1:34197");
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the game was not started with its connect arguments");
    }

    #[test]
    #[cfg(unix)]
    fn launch_failures_are_wrapped_for_the_user() {
        let mut e = env();
        e.add("s1", "factorio", "udp", 34197);
        // Exists, but is not executable.
        let exe = e.dir.path().join("not-executable");
        fs::write(&exe, "text").unwrap();
        let mut settings = e.app.cfg["settings"].clone();
        settings["gameExecutables"] = json!({"factorio":exe});
        e.app.mutate("settings", "", settings).unwrap();
        e.connect("s1", 34197);
        assert_eq!(
            e.command(json!({"method":"launchGame","id":"s1"}))
                .unwrap_err(),
            "could not start the game: could not open the configured program"
        );
    }

    // TestCheckAwaitsRetiringTunnelAfterDisconnect: the stop effect runs to
    // completion inside the same serialized command, so a probe can never overlap
    // the tunnel it follows.
    #[test]
    fn check_after_disconnect_starts_only_after_the_tunnel_was_stopped() {
        let mut e = env();
        e.mc("s1");
        e.connect("s1", 25565);
        e.app
            .command(json!({"method":"disconnect","id":"s1"}))
            .unwrap();
        e.app.check("s1").unwrap();
        assert_eq!(e.lifecycle(), ["join:s1:1", "stop:s1:1", "check:s1:2"]);
        assert_eq!(e.app.resources["s1"], 2);
    }

    #[test]
    fn request_helpers() {
        assert_eq!(rows(&json!(null)).len(), 0);
        assert_eq!(text(&json!({"a":1}), "a"), "");
        assert_eq!(info(&json!("nope")), Value::Null);
        assert_eq!(demo_info("demo-factorio")["type"], "factorio");
        assert_eq!(demo_info("other"), Value::Null);
        assert_eq!(
            App::generation(&json!({"effects":[{"type":"a","generation":3}]}), "a"),
            Some(3)
        );
        assert_eq!(App::generation(&json!({}), "a"), None);
        assert!(now().ends_with('Z'));
    }

    #[test]
    fn request_struct_preserves_hosted_join_request_ids() {
        for method in ["approveHostedRequest", "dismissHostedRequest"] {
            let request: Request = serde_json::from_value(
                json!({"method":method,"id":"selected-game","requestId":42}),
            )
            .unwrap();
            let forwarded = serde_json::to_value(request).unwrap();
            assert_eq!(forwarded["id"], "selected-game");
            assert_eq!(forwarded["requestId"], 42);
        }
    }

    #[test]
    fn request_structs_reject_unknown_shapes_but_default_missing_fields() {
        let r: Request = serde_json::from_value(json!({"method":"join","id":"x"})).unwrap();
        assert_eq!(r.id, "x");
        assert_eq!(r.limit, 0);
        assert!(serde_json::from_value::<Request>(json!({"method":5})).is_err());
        let s: SettingsInput = serde_json::from_value(json!({"demoMode":true})).unwrap();
        assert!(s.demo_mode && !s.auto_reconnect);
    }
}
