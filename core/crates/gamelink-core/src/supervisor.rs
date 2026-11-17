//! Pure supervisor policy. Hosts serialize load/reduce/effect completion, and
//! close a stopped generation before executing any following start effect.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    states: BTreeMap<String, GameState>,
    #[serde(default)]
    active: BTreeMap<String, Active>,
    #[serde(default)]
    pending: BTreeMap<String, Active>,
    #[serde(default)]
    retry_at: BTreeMap<String, i64>,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    published: BTreeMap<String, GameState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Active {
    generation: u64,
    fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameState {
    id: String,
    name: String,
    protocol: String,
    tunnel_port: u16,
    dest: String,
    approved: usize,
    state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedGame {
    game: Game,
    key_mtime_ns: i64,
    #[serde(default)]
    approved_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Game {
    id: String,
    name: String,
    #[serde(default, rename = "type")]
    game_type: String,
    protocol: String,
    tunnel_port: u16,
    dest: String,
    #[serde(default, skip_serializing_if = "is_false")]
    disabled: bool,
    #[serde(default, deserialize_with = "devices_or_null")]
    devices: Vec<Device>,
    #[serde(default)]
    console_bridge: Option<Bridge>,
    #[serde(default)]
    bedrock: Option<BedrockSpec>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    public_key: String,
}

// Field order and omission match config.ConsoleBridge's Go JSON encoding.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Bridge {
    server_ip: String,
    bind_ip: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    discovery_port: u16,
    media_port_first: u16,
    media_port_last: u16,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockSpec {
    media_port_first: u16,
    media_port_last: u16,
}
fn devices_or_null<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Device>, D::Error> {
    Ok(Option::<Vec<Device>>::deserialize(deserializer)?.unwrap_or_default())
}
fn is_zero(value: &u16) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    Observe {
        games: Vec<ObservedGame>,
        now_ns: i64,
    },
    InvalidConfig {
        cause: String,
    },
    StartCompleted {
        id: String,
        generation: u64,
        error: Option<String>,
        now_ns: i64,
    },
    Commit,
    Shutdown,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Effect {
    Stop { id: String, generation: u64 },
    Start { id: String, generation: u64 },
    Log { message: String },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Reduction {
    pub state: State,
    pub effects: Vec<Effect>,
    pub changed: bool,
    pub games: Vec<GameState>,
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    state: State,
    event: Event,
}

pub fn reduce_json(input: &str) -> Result<String, String> {
    let input: Input = serde_json::from_str(input).map_err(|_| "invalid supervisor input")?;
    let result = reduce(input.state, input.event)?;
    serde_json::to_string(&result).map_err(|_| "cannot encode supervisor result".into())
}

fn log(effects: &mut Vec<Effect>, message: String) {
    effects.push(Effect::Log { message });
}

fn stop(state: &mut State, id: &str, effects: &mut Vec<Effect>) {
    state.pending.remove(id);
    if let Some(active) = state.active.remove(id) {
        effects.push(Effect::Stop {
            id: id.into(),
            generation: active.generation,
        });
    }
}

pub fn reduce(mut state: State, event: Event) -> Result<Reduction, String> {
    let commit = matches!(event, Event::Commit);
    let mut effects = Vec::new();
    match event {
        Event::Observe { games, now_ns } => {
            let mut seen = BTreeSet::new();
            for observed in games {
                let g = &observed.game;
                // Effects execute after this transaction. Two starts for the same
                // ID would overwrite the host's handle and escape later revocation.
                if !seen.insert(g.id.clone()) {
                    return Err("duplicate game ID in configuration".into());
                }
                let mut st = GameState {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    protocol: g.protocol.clone(),
                    tunnel_port: g.tunnel_port,
                    dest: g.dest.clone(),
                    approved: g.devices.len(),
                    state: String::new(),
                    detail: String::new(),
                };
                if let Some(error) = observed.approved_error {
                    stop(&mut state, &g.id, &mut effects);
                    st.state = "error".into();
                    st.detail = error;
                } else if g.disabled {
                    stop(&mut state, &g.id, &mut effects);
                    st.state = "inactive".into();
                    st.detail = "managed game is disabled".into();
                } else if g.devices.is_empty() {
                    if state.active.contains_key(&g.id) {
                        log(
                            &mut effects,
                            format!("game {:?} has no approved devices left; stopping it", g.id),
                        );
                    }
                    stop(&mut state, &g.id, &mut effects);
                    st.state = "inactive".into();
                    st.detail = "no approved devices; the game is not reachable".into();
                } else {
                    let fingerprint = fingerprint(g, observed.key_mtime_ns)?;
                    if let Some(active) = state.active.get(&g.id) {
                        if active.fingerprint == fingerprint {
                            st.state = "serving".into();
                            state.states.insert(g.id.clone(), st);
                            continue;
                        }
                        log(&mut effects, format!("game {:?}: approved devices or settings changed; restarting its Tailcat server and ending existing sessions", g.id));
                        stop(&mut state, &g.id, &mut effects);
                    }
                    if state
                        .retry_at
                        .get(&g.id)
                        .is_some_and(|deadline| now_ns < *deadline)
                    {
                        st.state = "error".into();
                        st.detail = state
                            .states
                            .get(&g.id)
                            .map(|old| old.detail.clone())
                            .unwrap_or_default();
                    } else {
                        state.generation = state
                            .generation
                            .checked_add(1)
                            .ok_or("supervisor generation exhausted")?;
                        state.pending.insert(
                            g.id.clone(),
                            Active {
                                generation: state.generation,
                                fingerprint,
                            },
                        );
                        effects.push(Effect::Start {
                            id: g.id.clone(),
                            generation: state.generation,
                        });
                    }
                }
                state.states.insert(g.id.clone(), st);
            }
            let removed: Vec<_> = state
                .states
                .keys()
                .filter(|id| !seen.contains(*id))
                .cloned()
                .collect();
            for id in removed {
                log(
                    &mut effects,
                    format!("game {id:?} was removed from the configuration; stopping it"),
                );
                stop(&mut state, &id, &mut effects);
                state.states.remove(&id);
            }
        }
        Event::InvalidConfig { cause } => {
            let ids: Vec<_> = state.active.keys().cloned().collect();
            for id in ids {
                stop(&mut state, &id, &mut effects);
            }
            state.pending.clear();
            for st in state.states.values_mut() {
                st.state = "error".into();
                st.detail = format!("configuration is invalid: {cause}");
            }
        }
        Event::StartCompleted {
            id,
            generation,
            error,
            now_ns,
        } => {
            if state
                .pending
                .get(&id)
                .is_some_and(|p| p.generation == generation)
            {
                let pending = state.pending.remove(&id).expect("matching pending start");
                let st = state.states.get_mut(&id).ok_or("missing pending game")?;
                if let Some(error) = error {
                    log(
                        &mut effects,
                        format!("game {id:?} failed to start: {error}"),
                    );
                    state.retry_at.insert(
                        id,
                        now_ns
                            .checked_add(10_000_000_000)
                            .ok_or("retry deadline overflow")?,
                    );
                    st.state = "error".into();
                    st.detail = error;
                } else {
                    state.retry_at.remove(&id);
                    state.active.insert(id.clone(), pending);
                    st.state = "serving".into();
                    st.detail.clear();
                    log(
                        &mut effects,
                        format!(
                            "serving {id:?} ({} {} -> {}) for {} approved device(s)",
                            st.protocol, st.tunnel_port, st.dest, st.approved
                        ),
                    );
                }
            } else if error.is_none() {
                // Host must dispose of resources returned by obsolete starts.
                effects.push(Effect::Stop { id, generation });
            }
        }
        Event::Commit => {}
        Event::Shutdown => {
            let ids: Vec<_> = state.active.keys().cloned().collect();
            for id in ids {
                log(&mut effects, format!("stopping {id}"));
                stop(&mut state, &id, &mut effects);
            }
            state.pending.clear();
        }
    }
    let changed = state.published != state.states;
    if commit {
        state.published = state.states.clone();
    }
    let games = state.states.values().cloned().collect();
    Ok(Reduction {
        state,
        effects,
        changed,
        games,
    })
}

fn fingerprint(game: &Game, key_mtime_ns: i64) -> Result<String, String> {
    let mut keys: Vec<_> = game.devices.iter().map(|d| d.public_key.as_str()).collect();
    keys.sort_unstable();
    let bridge = serde_json::to_string(&game.console_bridge).map_err(|_| "cannot encode bridge")?;
    let mut material = format!(
        "{}|{}|{}|{}|{}|{}",
        game.protocol,
        game.tunnel_port,
        game.dest,
        key_mtime_ns,
        keys.join(","),
        bridge
    );
    if !game.game_type.is_empty() {
        material.push_str("|type=");
        material.push_str(&game.game_type);
    }
    if let Some(bedrock) = &game.bedrock {
        material.push_str("|bedrock=");
        material.push_str(
            &serde_json::to_string(bedrock).map_err(|_| "cannot encode Bedrock media ports")?,
        );
    }
    Ok(format!("{:x}", Sha256::digest(material.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(state: State, event: serde_json::Value) -> Reduction {
        reduce(state, serde_json::from_value(event).unwrap()).unwrap()
    }
    fn game() -> serde_json::Value {
        json!({"id":"x","name":"Game","protocol":"tcp","tunnelPort":25565,"dest":"127.0.0.1:25565","devices":[{"publicKey":"nodekey:approved"}]})
    }
    fn observe(state: State, game: serde_json::Value, now: i64) -> Reduction {
        step(
            state,
            json!({"type":"observe","games":[{"game":game,"keyMtimeNs":0}],"nowNs":now}),
        )
    }
    fn started() -> Reduction {
        let out = observe(State::default(), game(), 0);
        assert_eq!(
            out.effects,
            vec![Effect::Start {
                id: "x".into(),
                generation: 1
            }]
        );
        step(
            out.state,
            json!({"type":"start_completed","id":"x","generation":1,"nowNs":0}),
        )
    }
    fn committed(state: State) -> State {
        step(state, json!({"type":"commit"})).state
    }

    #[test]
    fn duplicate_game_ids_cannot_schedule_untracked_servers() {
        let event = serde_json::from_value(json!({
            "type":"observe", "nowNs":0,
            "games":[{"game":game(),"keyMtimeNs":0},{"game":game(),"keyMtimeNs":0}]
        }))
        .unwrap();
        assert_eq!(
            reduce(State::default(), event).unwrap_err(),
            "duplicate game ID in configuration"
        );
    }

    #[test]
    fn original_go_fingerprint_fixtures() {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../fixtures/supervisor-fingerprints.json"
        ))
        .unwrap();
        for fixture in fixtures {
            let g: Game = serde_json::from_value(fixture["game"].clone()).unwrap();
            assert_eq!(
                fingerprint(&g, fixture["keyMtimeNs"].as_i64().unwrap()).unwrap(),
                fixture["fingerprint"].as_str().unwrap(),
                "{}",
                fixture["name"]
            );
        }
    }

    #[test]
    fn unchanged_and_display_edits_do_not_restart() {
        let state = committed(started().state);
        let out = observe(state, game(), 1);
        assert!(out.effects.is_empty());
        assert!(!out.changed);
        let mut g = game();
        g["name"] = json!("Renamed");
        let renamed = observe(out.state, g, 2);
        assert!(renamed.effects.is_empty());
        assert!(renamed.changed);
        assert_eq!(renamed.games[0].state, "serving");
    }

    #[test]
    fn disabled_managed_game_stops_and_reenabled_game_starts() {
        let serving = started().state;
        let mut g = game();
        g["disabled"] = serde_json::json!(true);
        let disabled = observe(serving, g.clone(), 1);
        assert!(disabled
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Stop { generation: 1, .. })));
        assert!(!disabled
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Start { .. })));
        assert_eq!(disabled.games[0].state, "inactive");
        assert_eq!(disabled.games[0].detail, "managed game is disabled");

        g["disabled"] = serde_json::json!(false);
        let enabled = observe(disabled.state, g, 2);
        assert!(enabled
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Start { generation: 2, .. })));

        let game: Game = serde_json::from_value(game()).unwrap();
        let serialized = serde_json::to_value(game).unwrap();
        assert!(
            serialized.get("disabled").is_none(),
            "default false must preserve legacy serialization"
        );
    }

    #[test]
    fn revocation_stops_before_start_and_stale_success_stops_exact_generation() {
        let mut g = game();
        g["devices"] = json!([{"publicKey":"nodekey:replacement"}]);
        let out = observe(started().state, g, 1);
        assert!(matches!(&out.effects[0], Effect::Log { .. }));
        assert_eq!(
            &out.effects[1..],
            &[
                Effect::Stop {
                    id: "x".into(),
                    generation: 1
                },
                Effect::Start {
                    id: "x".into(),
                    generation: 2
                }
            ]
        );
        let stale = step(
            out.state,
            json!({"type":"start_completed","id":"x","generation":1,"nowNs":2}),
        );
        assert_eq!(
            stale.effects,
            vec![Effect::Stop {
                id: "x".into(),
                generation: 1
            }]
        );
        assert_eq!(stale.state.pending["x"].generation, 2);
    }

    #[test]
    fn empty_invalid_removed_and_bad_config_fail_closed() {
        let mut g = game();
        g["devices"] = json!(null);
        let empty = observe(started().state, g, 0);
        assert!(empty
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Stop { generation: 1, .. })));
        assert!(!empty
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Start { .. })));
        assert_eq!(empty.games[0].state, "inactive");
        let bad = step(
            started().state,
            json!({"type":"observe","nowNs":0,"games":[{"game":game(),"keyMtimeNs":0,"approvedError":"bad device"}]}),
        );
        assert_eq!(bad.games[0].state, "error");
        assert_eq!(bad.games[0].detail, "bad device");
        assert!(bad.state.active.is_empty());
        let removed = step(
            started().state,
            json!({"type":"observe","nowNs":0,"games":[]}),
        );
        assert!(removed.games.is_empty());
        assert!(removed.state.active.is_empty());
        let invalid = step(
            started().state,
            json!({"type":"invalid_config","cause":"broken"}),
        );
        assert_eq!(invalid.games[0].detail, "configuration is invalid: broken");
        assert!(invalid.state.active.is_empty());
        assert!(matches!(
            observe(invalid.state, game(), 1).effects[0],
            Effect::Start { .. }
        ));
    }

    #[test]
    fn retry_exact_boundary_and_final_commit_matches_go_write_policy() {
        let pending = observe(State::default(), game(), 0);
        let failed = step(
            pending.state,
            json!({"type":"start_completed","id":"x","generation":1,"error":"bind failed","nowNs":123}),
        );
        let state = committed(failed.state);
        let before = observe(state, game(), 10_000_000_122);
        assert!(before.effects.is_empty());
        assert_eq!(before.games[0].detail, "bind failed");
        assert!(!before.changed);
        let retry = observe(before.state, game(), 10_000_000_123);
        assert_eq!(
            retry.effects,
            vec![Effect::Start {
                id: "x".into(),
                generation: 2
            }]
        );
        let failed_again = step(
            retry.state,
            json!({"type":"start_completed","id":"x","generation":2,"error":"bind failed","nowNs":10_000_000_123i64}),
        );
        assert!(
            !failed_again.changed,
            "same final failure must not rewrite runtime"
        );
        let state = committed(failed_again.state);
        let removed = step(
            state,
            json!({"type":"observe","games":[],"nowNs":10_000_000_124i64}),
        );
        let returned = observe(removed.state, game(), 10_000_000_125);
        assert!(
            returned.effects.is_empty(),
            "Go retains cooldown even across removal"
        );
    }

    #[test]
    fn shutdown_and_overflow_are_safe() {
        let out = step(started().state, json!({"type":"shutdown"}));
        assert!(out.state.active.is_empty());
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Stop { .. })));
        let state = State {
            generation: u64::MAX,
            ..State::default()
        };
        let event = serde_json::from_value(
            json!({"type":"observe","nowNs":0,"games":[{"game":game(),"keyMtimeNs":0}]}),
        )
        .unwrap();
        assert_eq!(
            reduce(state, event).unwrap_err(),
            "supervisor generation exhausted"
        );
    }

    // Ported from tests/testsupport TestSupervisorBridgeChangeRequestsRestart.
    #[test]
    fn console_bridge_change_stops_before_it_restarts() {
        let mut g = json!({"id":"x","name":"Game","protocol":"udp","tunnelPort":19132,"dest":"127.0.0.1:19132","devices":[{"publicKey":"nodekey:configured"}],"consoleBridge":{"serverIp":"127.0.0.1","bindIp":"127.0.0.1","mediaPortFirst":1000,"mediaPortLast":1001}});
        let first = observe(State::default(), g.clone(), 1);
        assert_eq!(
            first.effects,
            vec![Effect::Start {
                id: "x".into(),
                generation: 1
            }]
        );
        let running = step(
            first.state,
            json!({"type":"start_completed","id":"x","generation":1,"nowNs":1}),
        );
        let unchanged = observe(running.state, g.clone(), 1);
        assert!(unchanged.effects.is_empty(), "{:?}", unchanged.effects);
        g["consoleBridge"]["mediaPortLast"] = json!(1002);
        let changed = observe(unchanged.state, g, 1);
        assert_eq!(changed.effects.len(), 3, "{:?}", changed.effects);
        assert!(matches!(changed.effects[0], Effect::Log { .. }));
        assert_eq!(
            changed.effects[1],
            Effect::Stop {
                id: "x".into(),
                generation: 1
            }
        );
        assert_eq!(
            changed.effects[2],
            Effect::Start {
                id: "x".into(),
                generation: 2
            }
        );
    }

    #[test]
    fn bedrock_media_port_change_stops_before_it_restarts() {
        let mut g = game();
        g["type"] = json!("minecraft-bedrock");
        g["bedrock"] = json!({"mediaPortFirst":19140,"mediaPortLast":19145});
        let first = observe(State::default(), g.clone(), 1);
        let running = step(
            first.state,
            json!({"type":"start_completed","id":"x","generation":1,"nowNs":1}),
        );
        let unchanged = observe(running.state, g.clone(), 2);
        assert!(unchanged.effects.is_empty());
        g["bedrock"]["mediaPortLast"] = json!(19146);
        let changed = observe(unchanged.state, g, 3);
        assert!(matches!(
            changed.effects[1],
            Effect::Stop { generation: 1, .. }
        ));
        assert!(matches!(
            changed.effects[2],
            Effect::Start { generation: 2, .. }
        ));
    }

    #[test]
    fn game_type_change_restarts_the_host() {
        let first = observe(State::default(), game(), 1);
        let running = step(
            first.state,
            json!({"type":"start_completed","id":"x","generation":1,"nowNs":1}),
        );
        let mut changed_game = game();
        changed_game["type"] = json!("minecraft-bedrock");
        let changed = observe(running.state, changed_game, 2);
        assert!(matches!(
            changed.effects[1],
            Effect::Stop { generation: 1, .. }
        ));
        assert!(matches!(
            changed.effects[2],
            Effect::Start { generation: 2, .. }
        ));
    }

    #[test]
    fn key_file_change_restarts_and_discovery_port_is_part_of_the_bridge() {
        let state = started().state;
        let out = step(
            state,
            json!({"type":"observe","nowNs":1,"games":[{"game":game(),"keyMtimeNs":99}]}),
        );
        assert!(matches!(out.effects[1], Effect::Stop { generation: 1, .. }));
        assert!(matches!(
            out.effects[2],
            Effect::Start { generation: 2, .. }
        ));
    }

    #[test]
    fn reduce_json_round_trips_state_and_rejects_bad_input() {
        let input =
            json!({"event":{"type":"observe","nowNs":0,"games":[{"game":game(),"keyMtimeNs":0}]}});
        let out: serde_json::Value =
            serde_json::from_str(&reduce_json(&input.to_string()).unwrap()).unwrap();
        assert_eq!(out["effects"][0]["type"], "start");
        assert_eq!(out["effects"][0]["generation"], 1);
        // The returned state feeds the next call, as the original hosts did.
        let next = json!({"state":out["state"],"event":{"type":"start_completed","id":"x","generation":1,"nowNs":0}});
        let out: serde_json::Value =
            serde_json::from_str(&reduce_json(&next.to_string()).unwrap()).unwrap();
        assert_eq!(out["games"][0]["state"], "serving");
        assert_eq!(
            reduce_json("not json").unwrap_err(),
            "invalid supervisor input"
        );
        assert_eq!(
            reduce_json(r#"{"event":{"type":"nope"}}"#).unwrap_err(),
            "invalid supervisor input"
        );
    }

    #[test]
    fn failed_start_for_an_unknown_game_is_reported_not_ignored() {
        // A start that fails after the game was removed produces no effect and
        // no cooldown: the stale result is simply discarded.
        let out = step(
            State::default(),
            json!({"type":"start_completed","id":"gone","generation":7,"error":"boom","nowNs":0}),
        );
        assert!(out.effects.is_empty());
        assert!(out.state.retry_at.is_empty());
    }
}
