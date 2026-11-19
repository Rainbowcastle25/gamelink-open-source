//! Serialized host loop. Rust owns observations, effects and publication; Go
//! receives one generation's validated network parameters and owns its handles.
use crate::{
    bridge, config, invitations, json, log, now, pure, requests, rows, s, storage, Result,
};
use gamelink_core::supervisor::{self, Effect, Event, State};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, UNIX_EPOCH},
};

pub fn serve(dir: &Path, stopping: &AtomicBool, verbose: bool) -> Result<()> {
    let mut runtime = Runtime {
        dir,
        state: State::default(),
        origin: Instant::now(),
        started: now(),
        games: vec![],
        request_scopes: BTreeMap::new(),
        running_identities: BTreeMap::new(),
        request_store_failed: false,
        verbose,
    };
    let result = runtime.run(stopping);
    // Always release resources, including a successful start whose completion
    // could not be reduced. Dropping the outer Host repeats this idempotently.
    let cleanup = bridge::call(json!({"method":"shutdown"}));
    let _ = std::fs::remove_file(dir.join("runtime.json"));
    result.and(cleanup.map(|_| ()))
}
struct Runtime<'a> {
    dir: &'a Path,
    state: State,
    origin: Instant,
    started: String,
    games: Vec<Value>,
    request_scopes: BTreeMap<String, requests::Scope>,
    running_identities: BTreeMap<String, String>,
    request_store_failed: bool,
    verbose: bool,
}
impl Runtime<'_> {
    fn tick_ns(&self) -> i64 {
        self.origin.elapsed().as_nanos().min(i64::MAX as u128) as i64
    }
    fn reduce(&mut self, event: Event) -> Result<supervisor::Reduction> {
        let result = supervisor::reduce(std::mem::take(&mut self.state), event)?;
        self.state = result.state.clone();
        Ok(result)
    }
    fn run(&mut self, stopping: &AtomicBool) -> Result<()> {
        let mut poll = Instant::now();
        let mut heartbeat = poll;
        while !stopping.load(Ordering::Relaxed) {
            if Instant::now() >= poll {
                self.reconcile()?;
                poll = Instant::now() + Duration::from_secs(1);
            }
            if Instant::now() >= heartbeat {
                self.publish()?;
                heartbeat = Instant::now() + Duration::from_secs(5);
            }
            for line in rows(&bridge::call(json!({"method":"logs"}))?) {
                log(line.as_str().unwrap_or("[redacted]"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.publish_requests();
        let out = self.reduce(Event::Shutdown)?;
        self.finish_effects(&out.effects).map(|_| ())
    }
    fn observe_config(&mut self) -> Result<(Event, BTreeMap<String, Value>)> {
        let cfg = config::dispatch(
            "config.agent_load",
            json!({"path":self.dir.join("agent.json")}),
        );
        let mut games = BTreeMap::new();
        self.request_scopes.clear();
        let event = match cfg {
            Err(cause) => {
                log("configuration problem; stopping affected game resources");
                Event::InvalidConfig { cause }
            }
            Ok(cfg) => {
                let mut observed = vec![];
                for game in rows(&cfg["games"]) {
                    let id = s(game, "id");
                    let keys = invitations::load_keys(self.dir, id);
                    let mt = std::fs::metadata(self.dir.join("keys").join(format!("{id}.json")))
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|t| t.as_nanos().min(i64::MAX as u128) as i64)
                        .unwrap_or(0);
                    let mut approved_error = None;
                    for device in rows(&game["devices"]) {
                        if let Err(error) = pure::dispatch(
                            "identity.public_key",
                            json!({"text":device["publicKey"]}),
                        ) {
                            approved_error =
                                Some(format!("device {:?}: {error}", s(device, "label")));
                            break;
                        }
                    }
                    let mut observation =
                        json!({"game":game,"keyMtimeNs":mt,"approvedError":approved_error});
                    if let Ok(keys) = keys {
                        let expired = pure::check_invite_expiry(&keys["expiresAt"]).is_err();
                        let knock_enabled = game["knockEnabled"] == true;
                        let scope = requests::Scope::new(game, keys);
                        observation["keyIdentity"] = json!(format!(
                            "{}|{}",
                            scope.identity,
                            s(&scope.keys, "expiresAt")
                        ));
                        observation["knockEnabled"] = json!(knock_enabled);
                        observation["expired"] = json!(expired);
                        if !expired
                            && game["disabled"] != true
                            && observation["approvedError"].is_null()
                        {
                            self.request_scopes.insert(id.to_owned(), scope);
                        }
                    }
                    observed.push(observation);
                    games.insert(id.to_owned(), game.clone());
                }
                serde_json::from_value(
                    json!({"type":"observe","games":observed,"nowNs":self.tick_ns()}),
                )
                .map_err(|_| "invalid host observation")?
            }
        };
        Ok((event, games))
    }

    fn reconcile(&mut self) -> Result<()> {
        let (event, _) = self.observe_config()?;
        let mut change = supervisor::reduce(self.state.clone(), event)?;
        // Request approval takes the request lock before the config lock.
        self.publish_requests();
        loop {
            if !change.has_grants() {
                self.finish_effects(&change.effects)?;
                self.state = change.state;
                break;
            }
            let Some(lock) = config::try_lock_mutations(self.dir)? else {
                // Apply revocations even while a writer is busy. The next
                // poll will retry the grants without disturbing other games.
                self.finish_effects(&change.effects)?;
                return Ok(());
            };
            let (event, games) = self.observe_config()?;
            change = supervisor::reduce(self.state.clone(), event)?;
            if !change.has_grants() {
                drop(lock);
                continue;
            }
            if change.has_stops() {
                // A stop may wait for Go's resources to drain. Re-read the
                // config again before executing any grants after the stop.
                drop(lock);
                self.finish_effects(&change.effects)?;
            } else {
                self.state = change.state;
                let followups = self.apply_grants(change.effects, &games)?;
                drop(lock);
                if !self.finish_effects(&followups)? {
                    break;
                }
            }
            let (event, _) = self.observe_config()?;
            change = supervisor::reduce(self.state.clone(), event)?;
        }
        let final_state = self.reduce(Event::Commit)?;
        self.games = final_state
            .games
            .into_iter()
            .map(|g| serde_json::to_value(g).expect("game projection"))
            .collect();
        if final_state.changed {
            self.publish()?;
        }
        Ok(())
    }
    fn publish_requests(&mut self) {
        let failed =
            requests::publish(self.dir, &self.request_scopes, &self.running_identities).is_err();
        if failed && !self.request_store_failed {
            log("could not publish join requests");
        }
        self.request_store_failed = failed;
    }
    fn finish_effects(&mut self, effects: &[Effect]) -> Result<bool> {
        let mut stopped = false;
        for effect in effects {
            match effect {
                Effect::Log { message } => {
                    // Policy logs may contain a network failure. Apply the same
                    // fail-closed redactor used by all other public diagnostics.
                    let redacted =
                        pure::dispatch("security.redact", json!({"text":message,"secrets":[]}))?;
                    log(redacted.as_str().unwrap_or("[redacted]"));
                }
                Effect::Stop { id, generation } => {
                    bridge::call(json!({"method":"stop","id":id,"generation":generation}))?;
                    self.running_identities.remove(id);
                    self.state.record_stop(id, *generation);
                    stopped = true;
                }
                Effect::Start { .. } | Effect::Allow { .. } => {}
            }
        }
        Ok(stopped)
    }
    fn apply_grants(
        &mut self,
        effects: Vec<Effect>,
        games: &BTreeMap<String, Value>,
    ) -> Result<Vec<Effect>> {
        let mut followups = Vec::new();
        for effect in effects {
            let completed = match effect {
                Effect::Start { id, generation } => {
                    let error = self.start(&id, generation, games).err();
                    Event::StartCompleted {
                        id,
                        generation,
                        error,
                        now_ns: self.tick_ns(),
                    }
                }
                Effect::Allow {
                    id,
                    generation,
                    approved,
                } => {
                    let error = bridge::call(
                        json!({"method":"allow","id":id,"generation":generation,"approved":approved}),
                    )
                    .err();
                    Event::AllowCompleted {
                        id,
                        generation,
                        error,
                    }
                }
                effect => {
                    followups.push(effect);
                    continue;
                }
            };
            followups.extend(self.state.complete_grant(completed)?);
        }
        Ok(followups)
    }
    fn start(&mut self, id: &str, generation: u64, games: &BTreeMap<String, Value>) -> Result<()> {
        let mut game = games
            .get(id)
            .ok_or("supervisor requested an unknown game")?
            .clone();
        let keys = match self.request_scopes.get(id) {
            Some(scope) => scope.keys.clone(),
            None => invitations::load_keys(self.dir, id)?,
        };
        game["dest"] = pure::dispatch("security.destination", json!({"text":game["dest"]}))?;
        let approved: Vec<_> = rows(&game["devices"])
            .iter()
            .map(|d| d["publicKey"].clone())
            .collect();
        // Keep the bounded request independent of labels and UI metadata.
        game.as_object_mut()
            .ok_or("invalid game")?
            .remove("devices");
        bridge::call(
            json!({"method":"start","id":id,"generation":generation,"game":game,"keys":keys,"approved":approved,"verbose":self.verbose}),
        )?;
        self.running_identities
            .insert(id.to_owned(), requests::identity(&keys));
        Ok(())
    }
    fn publish(&self) -> Result<()> {
        let stats = bridge::call(json!({"method":"stats"}))?;
        let mut games = self.games.clone();
        for game in &mut games {
            game["stats"] = if stats[s(game, "id")].is_null() {
                json!({"active":0,"accepted":0,"denied":0,"dialFailures":0,"oversizeDrops":0,"overloaded":0})
            } else {
                stats[s(game, "id")].clone()
            };
        }
        let runtime =
            json!({"pid":std::process::id(),"started":self.started,"updated":now(),"games":games});
        storage::write_atomic(
            &self.dir.join("runtime.json"),
            &serde_json::to_vec_pretty(&runtime).map_err(|_| "cannot encode runtime")?,
        )
    }
}

// The Go supervisor tests (tests/testsupport/supervisor_test.go) drove the same
// reconcile loop against fake tunnels; these drive it against a scripted bridge.
#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, fs, path::PathBuf, rc::Rc, sync::Arc};

    const KEY_A: &str = "nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "nodekey:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    type LogHook = Box<dyn FnMut(&[Value])>;

    struct Script {
        calls: Vec<Value>,
        errors: BTreeMap<String, String>,
        stats: Value,
        logs: Vec<Value>,
        on_logs: Option<LogHook>,
        on_requests: Option<Box<dyn FnOnce()>>,
        on_stop: Option<Box<dyn FnOnce()>>,
    }
    type Shared = Rc<RefCell<Script>>;

    fn script() -> Shared {
        Rc::new(RefCell::new(Script {
            calls: vec![],
            errors: BTreeMap::new(),
            stats: json!({}),
            logs: vec![],
            on_logs: None,
            on_requests: None,
            on_stop: None,
        }))
    }
    // Strict: any bridge method the test did not script is a failure, so a stray
    // call can never reach the real Go archive.
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
                "start" | "allow" | "shutdown" | "init" => Ok(Value::Null),
                "stop" => {
                    if let Some(f) = s.on_stop.take() {
                        f();
                    }
                    Ok(Value::Null)
                }
                "stats" => Ok(s.stats.clone()),
                "requests" => {
                    if let Some(f) = s.on_requests.take() {
                        f();
                    }
                    Ok(json!({}))
                }
                "logs" => {
                    if let Some(mut f) = s.on_logs.take() {
                        f(&s.calls);
                        s.on_logs = Some(f);
                    }
                    Ok(json!(std::mem::take(&mut s.logs)))
                }
                other => panic!("unscripted bridge method {other:?}"),
            }
        })
    }
    fn lifecycle(script: &Shared) -> Vec<String> {
        script
            .borrow()
            .calls
            .iter()
            .filter_map(|c| {
                let method = c["method"].as_str().unwrap();
                match method {
                    "start" | "stop" | "allow" => {
                        Some(format!("{method}:{}:{}", c["id"], c["generation"]))
                    }
                    "shutdown" => Some("shutdown".into()),
                    _ => None,
                }
            })
            .collect()
    }
    fn count(script: &Shared, method: &str) -> usize {
        script
            .borrow()
            .calls
            .iter()
            .filter(|c| c["method"] == method)
            .count()
    }
    fn write_agent(dir: &Path, devices: &[&str]) {
        let devices: Vec<Value> = devices
            .iter()
            .map(|k| json!({"publicKey":k,"label":"a"}))
            .collect();
        write_games(
            dir,
            json!([{"id":"x","name":"Game","type":"minecraft-java","protocol":"tcp","tunnelPort":25565,"dest":"127.0.0.1:25565","devices":devices,"created":"2026-01-01T00:00:00Z"}]),
        );
    }
    fn write_games(dir: &Path, games: Value) {
        fs::write(
            dir.join("agent.json"),
            serde_json::to_vec(&json!({"version":1,"games":games})).unwrap(),
        )
        .unwrap();
    }
    fn keys_path(dir: &Path) -> PathBuf {
        dir.join("keys").join("x.json")
    }
    fn add_second_game(dir: &Path, devices: &[&str]) {
        let mut cfg: Value =
            serde_json::from_slice(&fs::read(dir.join("agent.json")).unwrap()).unwrap();
        let mut game = cfg["games"][0].clone();
        game["id"] = json!("y");
        game["tunnelPort"] = json!(25566);
        game["devices"] = json!(devices
            .iter()
            .map(|key| json!({"publicKey":key,"label":"b"}))
            .collect::<Vec<_>>());
        cfg["games"].as_array_mut().unwrap().push(game);
        write_games(dir, cfg["games"].clone());
        fs::copy(keys_path(dir), dir.join("keys/y.json")).unwrap();
    }
    fn write_keys(dir: &Path) {
        fs::create_dir_all(dir.join("keys")).unwrap();
        fs::write(
            keys_path(dir),
            r#"{"privateKey":"privkey:00","presharedKey":"p","region":{"RegionID":900,"Nodes":[{"Name":"n"}]},"created":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
    }
    fn dir_with_game(devices: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_agent(dir.path(), devices);
        write_keys(dir.path());
        dir
    }
    fn runtime(dir: &Path) -> Runtime<'_> {
        Runtime {
            dir,
            state: State::default(),
            origin: Instant::now(),
            started: now(),
            games: vec![],
            request_scopes: BTreeMap::new(),
            running_identities: BTreeMap::new(),
            request_store_failed: false,
            verbose: false,
        }
    }
    fn runtime_json(dir: &Path) -> Value {
        serde_json::from_slice(&fs::read(dir.join("runtime.json")).unwrap()).unwrap()
    }
    fn bump_key_mtime(dir: &Path, seconds: u64) {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(keys_path(dir))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn first_reconcile_starts_the_game_and_publishes_runtime_json() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.verbose = true;
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script), ["start:\"x\":1"]);

        let start = script
            .borrow()
            .calls
            .iter()
            .find(|c| c["method"] == "start")
            .cloned()
            .unwrap();
        assert_eq!(start["verbose"], true);
        assert_eq!(start["approved"], json!([KEY_A]));
        assert_eq!(start["keys"]["privateKey"], "privkey:00");
        assert_eq!(start["game"]["dest"], "127.0.0.1:25565");
        assert!(
            start["game"].get("devices").is_none(),
            "labels and device lists must not reach the network layer"
        );

        let published = runtime_json(dir.path());
        assert_eq!(published["pid"], std::process::id());
        assert_eq!(published["games"][0]["id"], "x");
        assert_eq!(published["games"][0]["state"], "serving");
        assert_eq!(published["games"][0]["approved"], 1);
        assert_eq!(published["games"][0]["stats"]["accepted"], 0);
    }

    // TestReconcileWritesRuntimeOnlyWhenStateChanges
    #[test]
    fn unchanged_reconcile_does_not_rewrite_runtime_json() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        let first = fs::read(dir.path().join("runtime.json")).unwrap();
        assert_eq!(count(&script, "stats"), 1);
        rt.reconcile().unwrap();
        rt.reconcile().unwrap();
        assert_eq!(
            fs::read(dir.path().join("runtime.json")).unwrap(),
            first,
            "unchanged reconcile rewrote runtime status"
        );
        assert_eq!(count(&script, "stats"), 1, "unchanged reconcile published");
        assert_eq!(lifecycle(&script), ["start:\"x\":1"]);
    }

    #[test]
    fn publish_merges_live_stats_into_the_game_rows() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        script.borrow_mut().stats = json!({"x":{"active":2,"accepted":9,"denied":1,"dialFailures":0,"oversizeDrops":0,"overloaded":0}});
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        assert_eq!(runtime_json(dir.path())["games"][0]["stats"]["accepted"], 9);
        script.borrow_mut().stats = json!({"x":{"active":3,"accepted":10}});
        rt.publish().unwrap();
        assert_eq!(runtime_json(dir.path())["games"][0]["stats"]["active"], 3);
        script
            .borrow_mut()
            .errors
            .insert("stats".into(), "bridge down".into());
        assert_eq!(rt.publish().unwrap_err(), "bridge down");
    }

    #[test]
    fn failed_allow_stops_and_restarts_with_the_full_approved_list() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();

        script
            .borrow_mut()
            .errors
            .insert("allow".into(), "allow failed".into());
        write_agent(dir.path(), &[KEY_A, KEY_B]);
        rt.reconcile().unwrap();
        assert_eq!(
            lifecycle(&script),
            [
                "start:\"x\":1",
                "allow:\"x\":1",
                "stop:\"x\":1",
                "start:\"x\":2"
            ]
        );
        let calls = &script.borrow().calls;
        let allow = calls.iter().find(|c| c["method"] == "allow").unwrap();
        assert_eq!(allow["approved"], json!([KEY_B]));
        let start = calls.iter().rfind(|c| c["method"] == "start").unwrap();
        assert_eq!(start["approved"], json!([KEY_A, KEY_B]));
        assert_eq!(runtime_json(dir.path())["games"][0]["state"], "serving");
        assert_eq!(runtime_json(dir.path())["games"][0]["approved"], 2);
    }

    #[test]
    fn revoked_device_is_never_added_from_a_stale_snapshot() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();

        write_agent(dir.path(), &[KEY_A, KEY_B]);
        let path = dir.path().join("agent.json");
        // Requests are sampled after the config read and before applying effects.
        script.borrow_mut().on_requests = Some(Box::new(move || {
            config::dispatch(
                "config.agent_mutate",
                json!({"path":path,"action":"revoke","id":"x","publicKey":KEY_B}),
            )
            .unwrap();
        }));
        rt.reconcile().unwrap();

        assert_eq!(lifecycle(&script), ["start:\"x\":1"]);
        assert_eq!(runtime_json(dir.path())["games"][0]["approved"], 1);
    }

    #[test]
    fn revoked_first_approval_is_never_started_from_a_stale_snapshot() {
        let dir = dir_with_game(&[KEY_A]);
        add_second_game(dir.path(), &[]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();

        let path = dir.path().join("agent.json");
        config::dispatch(
            "config.agent_mutate",
            json!({"path":path,"action":"approve","id":"y","publicKey":KEY_B,"now":now()}),
        )
        .unwrap();
        script.borrow_mut().on_requests = Some(Box::new(move || {
            config::dispatch(
                "config.agent_mutate",
                json!({"path":path,"action":"revoke","id":"y","publicKey":KEY_B}),
            )
            .unwrap();
        }));
        rt.reconcile().unwrap();

        assert_eq!(lifecycle(&script), ["start:\"x\":1"]);
        assert_eq!(runtime_json(dir.path())["games"][1]["approved"], 0);
    }

    #[test]
    fn unrelated_stop_releases_config_writers_before_revalidating_grants() {
        let dir = dir_with_game(&[KEY_A]);
        add_second_game(dir.path(), &[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();

        write_agent(dir.path(), &[KEY_A, KEY_B]);
        let path = dir.path().join("agent.json");
        let lock_path = dir.path().join("agent.lock");
        script.borrow_mut().on_stop = Some(Box::new(move || {
            // A writer can lock the file even while the stop is draining.
            let lock = fs::OpenOptions::new().write(true).open(lock_path).unwrap();
            lock.try_lock().expect("held the config lock during stop");
            drop(lock);
            config::dispatch(
                "config.agent_mutate",
                json!({"path":path,"action":"revoke","id":"x","publicKey":KEY_B}),
            )
            .unwrap();
        }));
        rt.reconcile().unwrap();

        assert_eq!(
            lifecycle(&script),
            ["start:\"x\":1", "start:\"y\":2", "stop:\"y\":2"]
        );
        assert_eq!(runtime_json(dir.path())["games"][0]["approved"], 1);
    }

    // TestSupervisorBridgeChangeRequestsRestart (device/key variants)
    #[test]
    fn revocation_and_key_changes_stop_before_they_restart() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();

        write_agent(dir.path(), &[KEY_B]);
        rt.reconcile().unwrap();
        assert_eq!(
            lifecycle(&script),
            ["start:\"x\":1", "stop:\"x\":1", "start:\"x\":2"],
            "revocation must close the old server before binding the new one"
        );
        let start = script
            .borrow()
            .calls
            .iter()
            .rfind(|c| c["method"] == "start")
            .cloned()
            .unwrap();
        assert_eq!(start["approved"], json!([KEY_B]));

        bump_key_mtime(dir.path(), 3600);
        rt.reconcile().unwrap();
        assert_eq!(
            lifecycle(&script)[3..],
            ["stop:\"x\":2", "start:\"x\":3"],
            "a rewritten key file restarts the game"
        );
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script).len(), 5, "stable state must not restart");
    }

    #[test]
    fn no_approved_devices_stops_the_game_and_reports_inactive() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        write_agent(dir.path(), &[]);
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script), ["start:\"x\":1", "stop:\"x\":1"]);
        assert_eq!(runtime_json(dir.path())["games"][0]["state"], "inactive");
    }

    #[test]
    fn removed_games_are_stopped_and_dropped_from_runtime_json() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        write_games(dir.path(), json!([]));
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script), ["start:\"x\":1", "stop:\"x\":1"]);
        assert_eq!(runtime_json(dir.path())["games"], json!([]));
    }

    // TestInvalidConfigStopsServingState. Unlike the Go supervisor, Reconcile
    // here does not return the configuration error: the loop keeps polling so
    // the next valid file recovers, and the failure is shown in runtime.json.
    #[test]
    fn invalid_config_stops_all_games_and_recovers_when_fixed() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();

        fs::write(dir.path().join("agent.json"), "{").unwrap();
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script), ["start:\"x\":1", "stop:\"x\":1"]);
        let published = runtime_json(dir.path());
        assert_eq!(published["games"][0]["state"], "error");
        assert!(
            published["games"][0]["detail"]
                .as_str()
                .unwrap()
                .starts_with("configuration is invalid: "),
            "{published}"
        );
        // Still invalid: nothing further to stop, nothing restarted.
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script).len(), 2);

        write_agent(dir.path(), &[KEY_A]);
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script)[2], "start:\"x\":2");
        assert_eq!(runtime_json(dir.path())["games"][0]["state"], "serving");
    }

    #[test]
    fn a_missing_agent_file_fails_closed_the_same_way() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        fs::remove_file(dir.path().join("agent.json")).unwrap();
        rt.reconcile().unwrap();
        assert_eq!(lifecycle(&script), ["start:\"x\":1", "stop:\"x\":1"]);
        assert!(runtime_json(dir.path())["games"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("agent is not initialised"));
    }

    #[test]
    fn duplicate_game_ids_abort_the_reconcile() {
        let dir = tempfile::tempdir().unwrap();
        let game = json!({"id":"x","name":"G","type":"custom","protocol":"tcp","tunnelPort":1,"dest":"127.0.0.1:1","devices":[{"publicKey":KEY_A}]});
        write_games(dir.path(), json!([game.clone(), game]));
        write_keys(dir.path());
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        assert_eq!(
            rt.reconcile().unwrap_err(),
            "duplicate game ID in configuration"
        );
        assert!(lifecycle(&script).is_empty(), "nothing may start");
    }

    // StartCompleted error path.
    #[test]
    fn failed_start_is_reported_and_retried_only_after_the_cooldown() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        script
            .borrow_mut()
            .errors
            .insert("start".into(), "bind: address in use".into());
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        let published = runtime_json(dir.path());
        assert_eq!(published["games"][0]["state"], "error");
        assert_eq!(published["games"][0]["detail"], "bind: address in use");
        // Inside the 10 second cooldown the start is not repeated.
        rt.reconcile().unwrap();
        assert_eq!(count(&script, "start"), 1);
        assert_eq!(rt.games[0]["state"], "error");
        // The failed generation was never active, so shutdown stops nothing.
        let out = rt.reduce(Event::Shutdown).unwrap();
        assert!(!out.effects.iter().any(|e| matches!(e, Effect::Stop { .. })));
    }

    #[test]
    fn start_errors_before_the_bridge_are_reported_without_calling_it() {
        // Missing key file.
        let dir = dir_with_game(&[KEY_A]);
        fs::remove_file(keys_path(dir.path())).unwrap();
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        assert_eq!(count(&script, "start"), 0);
        assert_eq!(
            runtime_json(dir.path())["games"][0]["detail"],
            "keys for \"x\" are missing"
        );

        // Corrupt key file.
        let dir = dir_with_game(&[KEY_A]);
        fs::write(keys_path(dir.path()), "{").unwrap();
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        assert_eq!(count(&script, "start"), 0);
        assert!(runtime_json(dir.path())["games"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("are corrupt"));

        // A destination that is not a local socket address.
        let dir = tempfile::tempdir().unwrap();
        write_games(
            dir.path(),
            json!([{"id":"x","name":"G","type":"custom","protocol":"tcp","tunnelPort":1,"dest":"example.com:80","devices":[{"publicKey":KEY_A}]}]),
        );
        write_keys(dir.path());
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        assert_eq!(count(&script, "start"), 0);
        assert_eq!(runtime_json(dir.path())["games"][0]["state"], "error");
    }

    #[test]
    fn an_invalid_approved_device_blocks_the_game() {
        let dir = dir_with_game(&["nodekey:short"]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        assert_eq!(count(&script, "start"), 0);
        let published = runtime_json(dir.path());
        assert_eq!(published["games"][0]["state"], "error");
        assert!(published["games"][0]["detail"]
            .as_str()
            .unwrap()
            .starts_with("device \"a\": public key must be 64 hex characters"));
    }

    #[test]
    fn a_stale_successful_start_is_stopped_at_its_exact_generation() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        let mut games = BTreeMap::new();
        games.insert(
            "x".to_owned(),
            json!({"id":"x","name":"G","dest":"127.0.0.1:25565","devices":[{"publicKey":KEY_A}]}),
        );
        // The supervisor never scheduled generation 5, so its completion is stale.
        let followups = rt
            .apply_grants(
                vec![Effect::Start {
                    id: "x".into(),
                    generation: 5,
                }],
                &games,
            )
            .unwrap();
        rt.finish_effects(&followups).unwrap();
        assert_eq!(lifecycle(&script), ["start:\"x\":5", "stop:\"x\":5"]);

        // A failed stale start has nothing to release.
        let before = lifecycle(&script).len();
        let followups = rt
            .apply_grants(
                vec![Effect::Start {
                    id: "unknown".into(),
                    generation: 6,
                }],
                &games,
            )
            .unwrap();
        rt.finish_effects(&followups).unwrap();
        assert_eq!(lifecycle(&script).len(), before);
    }

    #[test]
    fn bridge_failures_propagate_from_effects() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        rt.reconcile().unwrap();
        script
            .borrow_mut()
            .errors
            .insert("stop".into(), "stop failed".into());
        write_agent(dir.path(), &[]);
        assert_eq!(rt.reconcile().unwrap_err(), "stop failed");
    }

    #[test]
    fn log_effects_do_not_call_the_bridge() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let _bridge = fake(&script);
        let mut rt = runtime(dir.path());
        // A secret-bearing log effect goes through the redactor and stderr only.
        // Redaction output itself is covered by the `security.redact` fixtures.
        rt.finish_effects(&[Effect::Log {
            message: format!("failed: privkey:{}", "ab".repeat(16)),
        }])
        .unwrap();
        assert!(script.borrow().calls.is_empty());
    }

    #[test]
    fn tick_is_monotonic_and_saturating() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime(dir.path());
        let a = rt.tick_ns();
        let b = rt.tick_ns();
        assert!(a >= 0 && b >= a);
    }

    // serve(): shutdown ordering, runtime.json removal and error propagation.
    #[test]
    fn serve_runs_until_stopped_then_stops_games_shuts_down_and_removes_runtime_json() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        let stopping = Arc::new(AtomicBool::new(false));
        let seen_while_running = Rc::new(RefCell::new(false));
        {
            // The status file exists while the loop is running; the first log
            // poll then asks the loop to stop.
            let stopping = stopping.clone();
            let seen = seen_while_running.clone();
            let path = dir.path().join("runtime.json");
            let mut s = script.borrow_mut();
            s.logs = vec![json!("tunnel opened")];
            s.on_logs = Some(Box::new(move |_| {
                *seen.borrow_mut() = path.exists();
                stopping.store(true, Ordering::Relaxed);
            }));
        }
        let _bridge = fake(&script);
        serve(dir.path(), &stopping, false).unwrap();
        assert!(
            *seen_while_running.borrow(),
            "runtime.json was not published"
        );
        assert_eq!(
            lifecycle(&script),
            ["start:\"x\":1", "stop:\"x\":1", "shutdown"]
        );
        assert!(!dir.path().join("runtime.json").exists());
    }

    #[test]
    fn serve_keeps_games_running_during_lock_contention_and_retries_grants() {
        let dir = dir_with_game(&[KEY_A]);
        add_second_game(dir.path(), &[KEY_A]);
        let script = script();
        let stopping = Arc::new(AtomicBool::new(false));
        let signal = stopping.clone();
        let config_dir = dir.path().to_owned();
        let path = config_dir.join("agent.json");
        let mut held = None;
        let mut queued = false;
        script.borrow_mut().on_logs = Some(Box::new(move |calls| {
            let polls = calls
                .iter()
                .filter(|call| call["method"] == "requests")
                .count();
            assert!(calls.iter().all(|call| call["method"] != "stop"));
            match polls {
                0 if !queued => {
                    config::dispatch(
                        "config.agent_mutate",
                        json!({"path":path,"action":"approve","id":"x","publicKey":KEY_B,"now":now()}),
                    )
                    .unwrap();
                    held = Some(config::lock_mutations(&config_dir).unwrap());
                    queued = true;
                }
                1 if held.is_some() => {
                    assert!(calls.iter().all(|call| call["method"] != "allow"));
                    assert_eq!(runtime_json(&config_dir)["games"][1]["state"], "serving");
                    drop(held.take());
                }
                2 => {
                    let allow = calls.iter().find(|call| call["method"] == "allow").unwrap();
                    assert_eq!(allow["approved"], json!([KEY_B]));
                    signal.store(true, Ordering::Relaxed);
                }
                _ => {}
            }
        }));
        let _bridge = fake(&script);
        serve(dir.path(), &stopping, false).unwrap();

        assert_eq!(
            lifecycle(&script),
            [
                "start:\"x\":1",
                "start:\"y\":2",
                "allow:\"x\":1",
                "stop:\"x\":1",
                "stop:\"y\":2",
                "shutdown"
            ]
        );
    }

    #[test]
    fn serve_cleans_up_even_when_the_loop_fails() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        script
            .borrow_mut()
            .errors
            .insert("logs".into(), "bridge down".into());
        let _bridge = fake(&script);
        let stopping = AtomicBool::new(false);
        assert_eq!(
            serve(dir.path(), &stopping, false).unwrap_err(),
            "bridge down"
        );
        assert_eq!(lifecycle(&script).last().unwrap(), "shutdown");
        assert!(!dir.path().join("runtime.json").exists());
    }

    #[test]
    fn serve_reports_a_failed_shutdown_after_a_clean_stop() {
        let dir = dir_with_game(&[KEY_A]);
        let script = script();
        script
            .borrow_mut()
            .errors
            .insert("shutdown".into(), "close failed".into());
        let _bridge = fake(&script);
        let stopping = AtomicBool::new(true);
        assert_eq!(
            serve(dir.path(), &stopping, false).unwrap_err(),
            "close failed"
        );
        assert!(!dir.path().join("runtime.json").exists());
        // Stopped before the first poll: no game was ever started.
        assert_eq!(count(&script, "start"), 0);
    }
}
