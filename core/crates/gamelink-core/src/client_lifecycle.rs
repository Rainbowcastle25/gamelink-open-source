//! Desktop state policy. The host owns resources; tokens identify ownership,
//! independently of display phase (an unreachable tunnel may still be alive).
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const LIMIT: usize = 32 * 1024;
const ZERO_TIME: &str = "0001-01-01T00:00:00Z";
#[derive(Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct State {
    serial: u64,
    active: Option<u64>,
    pending: bool,
    tunnel: bool,
    demo: bool,
    closed: bool,
    name: String,
    phase: String,
    message: String,
    status: Value,
    last_status: String,
    check: Value,
    probe: Option<Probe>,
}
#[derive(Deserialize, Serialize)]
struct Probe {
    generation: u64,
    previous: Value,
}
#[derive(Deserialize)]
struct Input {
    state: Option<State>,
    event: Value,
}
fn text<'a>(v: &'a Value, field: &str) -> &'a str {
    v[field].as_str().unwrap_or("")
}
fn unknown_check() -> Value {
    json!({"state":"unknown","message":"Not checked yet","latencyMs":null,"path":"","checkedAt":ZERO_TIME})
}
fn check(s: &State) -> Value {
    if s.check.is_null() {
        unknown_check()
    } else {
        s.check.clone()
    }
}
fn ended(status: &str) -> bool {
    matches!(status, "error" | "unreachable" | "stopped")
}
fn friendly(s: &str) -> String {
    if s.is_empty() {
        return "unknown error".into();
    }
    // Preserve the original Go byte-prefix capitalization, even for a UTF-8
    // error beginning with a multibyte character.
    String::from_utf8_lossy(&s.as_bytes()[..1]).to_uppercase()
        + &String::from_utf8_lossy(&s.as_bytes()[1..])
}
fn describe_path(status: &Value) -> String {
    let mut parts = Vec::new();
    match text(status, "path") {
        "direct" => parts.push("direct".to_owned()),
        "relay" => parts.push("relayed".to_owned()),
        _ => {}
    }
    if status["latencyKnown"] == true {
        parts.push(format!(
            "{:.0} ms",
            status["latencyMs"].as_f64().unwrap_or(0.0)
        ));
    }
    if parts.is_empty() {
        "path not measured".into()
    } else {
        parts.join(", ")
    }
}
fn view(s: &State) -> Value {
    let status = &s.status;
    let mut v = json!({"state":"disconnected","message":"","localAddr":"","localPort":0,
        "portFallback":false,"path":"unknown","relay":"","latencyMs":null,"gameReachable":"unknown",
        "since":ZERO_TIME,"activeConns":0,"udpFlows":0,"reconnects":0,"oversizeDrops":0,
        "bytesToServer":0,"bytesFromServer":0,"simulated":s.demo && s.active.is_some()});
    for key in [
        "state",
        "message",
        "localAddr",
        "localPort",
        "portFallback",
        "path",
        "relay",
        "gameReachable",
        "since",
        "activeConns",
        "udpFlows",
        "reconnects",
        "oversizeDrops",
        "bytesToServer",
        "bytesFromServer",
    ] {
        if !status[key].is_null() {
            v[key] = status[key].clone();
        }
    }
    if status["latencyKnown"] == true {
        v["latencyMs"] = status["latencyMs"].clone();
    }
    if matches!(text(&v, "state"), "" | "stopped") {
        v["state"] = json!("disconnected");
    }
    if !s.phase.is_empty() && !s.tunnel {
        v["state"] = json!(s.phase);
        v["message"] = json!(s.message);
    }
    for field in ["path", "gameReachable"] {
        if text(&v, field).is_empty() {
            v[field] = json!("unknown");
        }
    }
    v
}
fn next(s: &mut State) -> Result<u64, String> {
    s.serial = s
        .serial
        .checked_add(1)
        .ok_or("client generation exhausted")?;
    Ok(s.serial)
}
fn cancel_probe(s: &mut State, effects: &mut Vec<Value>) {
    if let Some(probe) = s.probe.take() {
        s.check = probe.previous;
        effects.push(json!({"type":"cancel_probe","generation":probe.generation}));
    }
}
fn clear_active(s: &mut State, effects: &mut Vec<Value>) {
    if let Some(generation) = s.active.take() {
        effects.push(json!({"type":"stop","generation":generation}));
    }
    s.pending = false;
    s.tunnel = false;
    s.phase.clear();
    s.message.clear();
    s.status = Value::Null;
    s.last_status.clear();
}
/// Input and opaque state are bounded independently. No addresses, keys, or
/// resource handles belong in the state; the Go runner supplies status facts.
pub fn reduce(input: Value) -> Result<Value, String> {
    if serde_json::to_vec(&input)
        .map_err(|_| "invalid client event")?
        .len()
        > LIMIT
    {
        return Err("client lifecycle input exceeds 32 KiB".into());
    }
    let Input { state, event } =
        serde_json::from_value(input).map_err(|_| "invalid client event")?;
    let mut s = state.unwrap_or_default();
    let mut effects = Vec::new();
    let mut logs = Vec::new();
    let mut error = String::new();
    let mut notify = false;
    let mut result = Value::Null;
    let generation = event["generation"].as_u64();
    if let Some(status) = event.get("liveStatus").filter(|v| !v.is_null()) {
        s.status = status.clone();
    }
    let name = text(&event, "name");
    match text(&event, "type") {
        "view" => {}
        "edit_port" => {
            if (s.pending || s.tunnel) && event["changed"] == true {
                error = "disconnect before changing the local port".into();
            }
        }
        "restore" => {
            s.closed = false;
        }
        "join" | "demo_join" => {
            let demo = text(&event, "type") == "demo_join";
            if s.closed {
                error = "client is closed".into();
            } else if demo && event["enabled"] != true {
                error = "demo mode is off".into();
            } else if demo && event["valid"] != true {
                error = "server not found".into();
            } else if s.pending
                || (s.tunnel && !ended(text(&s.status, "state")))
                || (demo && s.active.is_some())
            {
                error = "already connecting or connected".into();
            } else {
                clear_active(&mut s, &mut effects);
                cancel_probe(&mut s, &mut effects);
                let generation = next(&mut s)?;
                s.active = Some(generation);
                s.pending = true;
                s.demo = demo;
                s.name = name.to_owned();
                s.phase = "connecting".into();
                s.message = if demo {
                    "Simulated connection (demo)"
                } else {
                    "Connecting…"
                }
                .into();
                effects.push(json!({"type":if demo {"start_timer"} else {"start_join"},"generation":generation}));
                if !demo {
                    logs.push(json!({"kind":"info","message":format!("Connecting to “{name}”…")}));
                }
                notify = true;
            }
        }
        "join_end" => {
            if generation != s.active || s.active.is_none() {
                if event["hasTunnel"] == true {
                    effects.push(json!({"type":"close_returned"}));
                }
            } else {
                s.pending = false;
                if event["failed"] == true {
                    if event["cancelled"] == true {
                        clear_active(&mut s, &mut effects);
                    } else {
                        s.phase = if event["noResponse"] == true {
                            "unreachable"
                        } else {
                            "error"
                        }
                        .into();
                        s.message = friendly(text(&event, "error"));
                        logs.push(
                            json!({"kind":if event["noResponse"] == true {"warning"} else {"error"},
                            "message":format!("Could not connect to “{}”: {}",s.name,s.message)}),
                        );
                    }
                } else {
                    s.tunnel = true;
                    s.phase.clear();
                    s.message.clear();
                    s.status = event["status"].clone();
                    logs.push(json!({"kind":"connect","message":format!("Connected to “{}” at {} ({})",s.name,text(&s.status,"localAddr"),describe_path(&s.status))}));
                }
                notify = true;
            }
        }
        "status" => {
            if generation == s.active && s.active.is_some() {
                let previous = s.last_status.clone();
                s.status = event["status"].clone();
                let current = text(&s.status, "state");
                s.last_status = current.to_owned();
                let message = match current {
                    "reconnecting" if previous != current => Some((
                        "warning",
                        format!("Connection to “{}” was interrupted; reconnecting", s.name),
                    )),
                    "unreachable" if previous == "reconnecting" => Some((
                        "warning",
                        format!(
                            "“{}” is not responding (offline, or access was removed)",
                            s.name
                        ),
                    )),
                    "connected" if matches!(previous.as_str(), "reconnecting" | "unreachable") => {
                        Some(("connect", format!("Reconnected to “{}”", s.name)))
                    }
                    _ => None,
                };
                if let Some((kind, message)) = message {
                    logs.push(json!({"kind":kind,"message":message}));
                }
                notify = true;
            }
        }
        "disconnect" | "remove" | "close" => {
            let was_active = s.active.is_some();
            if s.tunnel && !s.demo {
                logs.push(
                    json!({"kind":"disconnect","message":format!("Disconnected from “{}”",s.name)}),
                );
            }
            clear_active(&mut s, &mut effects);
            let had_probe = s.probe.is_some();
            cancel_probe(&mut s, &mut effects);
            if text(&event, "type") != "disconnect" {
                s.closed = true;
            }
            notify = was_active || had_probe;
        }
        "check" => {
            if s.tunnel && text(&s.status, "state") == "connected" {
                let v = view(&s);
                let (state, message) = match text(&v, "gameReachable") {
                    "yes" => ("online", "Game server answered"),
                    "no" => ("offline", "Game server is not responding"),
                    _ => ("unknown", "Agent online, game status unknown"),
                };
                result = json!({"state":state,"message":message,"latencyMs":v["latencyMs"],"path":v["path"],"checkedAt":event["now"]});
            } else if s.closed || s.pending || s.tunnel || s.probe.is_some() {
                result = check(&s);
            } else {
                let previous = check(&s);
                let generation = next(&mut s)?;
                s.probe = Some(Probe {
                    generation,
                    previous,
                });
                s.name = name.to_owned();
                s.check = json!({"state":"checking","message":"Checking…","latencyMs":null,"path":"","checkedAt":ZERO_TIME});
                result = s.check.clone();
                effects.push(json!({"type":"start_probe","generation":generation}));
                notify = true;
            }
        }
        "check_end" => {
            if s.probe.as_ref().map(|p| p.generation) == generation && s.probe.is_some() {
                let previous = s.probe.take().unwrap().previous;
                s.check = match text(&event, "outcome") {
                    "cancelled" => previous,
                    "address_error" => {
                        json!({"state":"unreachable","message":"Cannot read the saved address","checkedAt":event["now"]})
                    }
                    "start_error" => {
                        json!({"state":"unknown","message":"Could not start the server check","checkedAt":event["now"]})
                    }
                    "error" => {
                        json!({"state":"unreachable","message":format!("No response: the server may be offline, or this device is not approved for “{}”",s.name),"checkedAt":event["now"]})
                    }
                    _ => match text(&event, "gameReachable") {
                        "no" => {
                            json!({"state":"offline","message":"Game server is not responding","latencyMs":event["latencyMs"],"path":event["path"],"checkedAt":event["now"]})
                        }
                        "unknown" | "" => {
                            json!({"state":"unknown","message":"Agent online, game status unknown","latencyMs":event["latencyMs"],"path":event["path"],"checkedAt":event["now"]})
                        }
                        _ => {
                            json!({"state":"online","message":"Game server answered","latencyMs":event["latencyMs"],"path":event["path"],"checkedAt":event["now"]})
                        }
                    },
                };
                notify = true;
            }
            result = check(&s);
        }
        "cancel_check" => {
            if s.probe
                .as_ref()
                .is_some_and(|probe| Some(probe.generation) == generation)
            {
                cancel_probe(&mut s, &mut effects);
                notify = true;
            }
        }
        "demo_timer" => {
            if generation == s.active && s.active.is_some() && s.demo && s.pending {
                s.pending = false;
                s.phase.clear();
                s.message.clear();
                let port = event["port"].as_u64().unwrap_or(0);
                s.status = json!({"state":"connected","message":"Simulated connection: no network traffic is sent in demo mode",
                    "localAddr":format!("127.0.0.1:{port}"),"localPort":port,"path":"unknown","gameReachable":"unknown","since":event["now"]});
                notify = true;
            }
        }
        _ => return Err("unknown client lifecycle event".into()),
    }
    let connection = view(&s);
    let check = check(&s);
    let busy = s.pending || s.tunnel;
    let state = serde_json::to_value(s).map_err(|_| "invalid client state")?;
    if serde_json::to_vec(&state)
        .map_err(|_| "invalid client state")?
        .len()
        > LIMIT
    {
        return Err("client state exceeds 32 KiB".into());
    }
    Ok(
        json!({"state":state,"effects":effects,"logs":logs,"notify":notify,"connection":connection,"check":check,"busy":busy,"error":error,"result":result}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn step(state: &Value, event: Value) -> Value {
        reduce(json!({"state":state,"event":event})).unwrap()
    }
    #[test]
    fn frozen_go_policy_and_transition_fixtures() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../fixtures/lifecycle-go-6c89f64.json"))
                .unwrap();
        for f in fixtures.as_array().unwrap() {
            let got = match text(f, "kind") {
                "view" => view(&serde_json::from_value(json!({"status":f["status"]})).unwrap()),
                "path" => json!(describe_path(&f["status"])),
                "friendly" => json!(friendly(text(f, "text"))),
                "ended" => json!(ended(text(f, "state"))),
                "status" => {
                    let out = step(
                        &json!({"active":7,"name":"World","lastStatus":f["previous"],"status":{"state":f["previous"]}}),
                        json!({"type":"status","generation":7,"status":f["status"]}),
                    );
                    let expected = if f["logs"].is_null() {
                        json!([])
                    } else {
                        f["logs"].clone()
                    };
                    assert_eq!(out["logs"], expected, "{f}");
                    out["connection"].clone()
                }
                _ => panic!("unknown fixture"),
            };
            assert_eq!(got, f["output"], "{f}");
        }
    }
    #[test]
    fn retry_and_busy_ownership_are_independent_of_display() {
        let joined = step(&Value::Null, json!({"type":"join","name":"World"}));
        let gen = joined["state"]["active"].clone();
        let status = json!({"state":"unreachable"});
        let live = step(
            &joined["state"],
            json!({"type":"join_end","generation":gen,"hasTunnel":true,"status":status}),
        );
        assert_eq!(live["connection"]["state"], "unreachable");
        assert_eq!(live["busy"], true);
        let retry = step(&live["state"], json!({"type":"join","name":"World"}));
        assert_eq!(retry["error"], "");
        assert_eq!(retry["effects"][0], json!({"type":"stop","generation":gen}));
        assert_eq!(retry["effects"][1]["type"], "start_join");
        let duplicate = step(&retry["state"], json!({"type":"join"}));
        assert_eq!(duplicate["error"], "already connecting or connected");
    }
    #[test]
    fn probe_takeover_restores_check_and_stale_callbacks_are_inert() {
        let old = json!({"state":"online","message":"Server answered","checkedAt":"2026-11-14T12:00:00Z"});
        let probe = step(
            &json!({"check":old}),
            json!({"type":"check","name":"World"}),
        );
        let duplicate = step(&probe["state"], json!({"type":"check","name":"World"}));
        assert!(duplicate["effects"].as_array().unwrap().is_empty());
        let joined = step(&probe["state"], json!({"type":"join","name":"World"}));
        assert_eq!(joined["check"], old);
        assert_eq!(joined["effects"][0]["type"], "cancel_probe");
        let late = step(
            &joined["state"],
            json!({"type":"check_end","generation":1,"outcome":"online"}),
        );
        assert_eq!(late["state"], joined["state"]);
        assert_eq!(late["notify"], false);
        let stopped = step(&joined["state"], json!({"type":"disconnect"}));
        let stale = step(
            &stopped["state"],
            json!({"type":"join_end","generation":2,"hasTunnel":true}),
        );
        assert_eq!(stale["state"], stopped["state"]);
        assert_eq!(stale["effects"], json!([{"type":"close_returned"}]));
        let stale = step(
            &stopped["state"],
            json!({"type":"status","generation":2,"status":{"state":"connected"}}),
        );
        assert_eq!(stale["state"], stopped["state"]);
        assert_eq!(stale["notify"], false);
    }
    #[test]
    fn early_status_connected_shortcut_and_cancelled_checks() {
        let joining = step(&Value::Null, json!({"type":"join","name":"World"}));
        let early = step(
            &joining["state"],
            json!({"type":"status","generation":1,"status":{"state":"connected","path":"direct"}}),
        );
        assert_eq!(early["connection"]["state"], "connecting");
        let live = step(
            &early["state"],
            json!({"type":"join_end","generation":1,"hasTunnel":true,"status":{"state":"connected","path":"direct","latencyKnown":true,"latencyMs":2.0,"gameReachable":"yes"}}),
        );
        let checked = step(
            &live["state"],
            json!({"type":"check","now":"2026-11-14T12:00:00Z"}),
        );
        assert_eq!(checked["result"]["state"], "online");
        assert_eq!(checked["result"]["message"], "Game server answered");
        assert!(checked["effects"].as_array().unwrap().is_empty());
        let checking = step(&Value::Null, json!({"type":"check"}));
        let cancelled = step(
            &checking["state"],
            json!({"type":"check_end","generation":1,"outcome":"cancelled"}),
        );
        assert_eq!(cancelled["check"], unknown_check());
    }
    #[test]
    fn stale_demo_timer_and_close_do_not_reanimate() {
        let first = step(
            &Value::Null,
            json!({"type":"demo_join","enabled":true,"valid":true}),
        );
        let stopped = step(&first["state"], json!({"type":"disconnect"}));
        let second = step(
            &stopped["state"],
            json!({"type":"demo_join","enabled":true,"valid":true}),
        );
        let stale = step(
            &second["state"],
            json!({"type":"demo_timer","generation":1,"port":25565}),
        );
        assert_eq!(stale["state"], second["state"]);
        assert_eq!(stale["notify"], false);
        let closed = step(&second["state"], json!({"type":"close"}));
        let late = step(
            &closed["state"],
            json!({"type":"demo_timer","generation":2,"port":25565}),
        );
        assert_eq!(late["state"], closed["state"]);
        assert_eq!(
            step(&closed["state"], json!({"type":"join"}))["error"],
            "client is closed"
        );
    }
    #[test]
    fn input_is_bounded_and_invalid_events_rejected() {
        assert!(
            reduce(json!({"state":null,"event":{"type":"x","name":"x".repeat(LIMIT)}})).is_err()
        );
        assert!(reduce(json!({"state":null,"event":{"type":"x"}})).is_err());
    }

    // TestJoinFailurePreservesFriendlyError
    #[test]
    fn join_failures_keep_a_friendly_message_and_distinguish_no_response() {
        let joining = step(&Value::Null, json!({"type":"join","name":"World"}));
        let failed = step(
            &joining["state"],
            json!({"type":"join_end","generation":1,"failed":true,"error":"connection refused"}),
        );
        assert_eq!(failed["connection"]["state"], "error");
        assert_eq!(failed["connection"]["message"], "Connection refused");
        assert_eq!(failed["logs"][0]["kind"], "error");
        assert_eq!(failed["busy"], false);
        let silent = step(
            &joining["state"],
            json!({"type":"join_end","generation":1,"failed":true,"noResponse":true,"error":"no response"}),
        );
        assert_eq!(silent["connection"]["state"], "unreachable");
        assert_eq!(silent["logs"][0]["kind"], "warning");
        // A cancelled join leaves no trace and may be retried at once.
        let cancelled = step(
            &joining["state"],
            json!({"type":"join_end","generation":1,"failed":true,"cancelled":true}),
        );
        assert_eq!(cancelled["connection"]["state"], "disconnected");
        assert_eq!(cancelled["state"]["active"], Value::Null);
        assert_eq!(
            cancelled["effects"],
            json!([{"type":"stop","generation":1}])
        );
        assert_eq!(friendly(""), "unknown error");
    }

    // TestRetryRetainsBusyPortGuardUntilTunnelClosed
    #[test]
    fn local_port_edits_are_refused_while_a_resource_is_alive() {
        let idle = step(&Value::Null, json!({"type":"edit_port","changed":true}));
        assert_eq!(idle["error"], "");
        let joining = step(&Value::Null, json!({"type":"join","name":"World"}));
        let pending = step(
            &joining["state"],
            json!({"type":"edit_port","changed":true}),
        );
        assert_eq!(
            pending["error"],
            "disconnect before changing the local port"
        );
        let unchanged = step(
            &joining["state"],
            json!({"type":"edit_port","changed":false}),
        );
        assert_eq!(unchanged["error"], "");
        let live = step(
            &joining["state"],
            json!({"type":"join_end","generation":1,"hasTunnel":true,"status":{"state":"unreachable"}}),
        );
        let guarded = step(&live["state"], json!({"type":"edit_port","changed":true}));
        assert_eq!(
            guarded["error"],
            "disconnect before changing the local port"
        );
    }

    #[test]
    fn check_outcomes_and_removal_semantics() {
        let probing = step(&Value::Null, json!({"type":"check","name":"World"}));
        assert_eq!(probing["result"]["state"], "checking");
        assert_eq!(
            probing["effects"],
            json!([{"type":"start_probe","generation":1}])
        );
        let online = step(
            &probing["state"],
            json!({"type":"check_end","generation":1,"outcome":"online","gameReachable":"yes","latencyMs":4.0,"path":"direct","now":"2026-01-01T00:00:00Z"}),
        );
        assert_eq!(online["result"]["state"], "online");
        assert_eq!(online["result"]["latencyMs"], 4.0);
        let offline = step(
            &probing["state"],
            json!({"type":"check_end","generation":1,"outcome":"online","gameReachable":"no","now":"2026-01-01T00:00:00Z"}),
        );
        assert_eq!(offline["result"]["state"], "offline");
        let unknown = step(
            &probing["state"],
            json!({"type":"check_end","generation":1,"outcome":"online","gameReachable":"unknown","now":"2026-01-01T00:00:00Z"}),
        );
        assert_eq!(unknown["result"]["state"], "unknown");
        let failed = step(
            &probing["state"],
            json!({"type":"check_end","generation":1,"outcome":"error","now":"2026-01-01T00:00:00Z"}),
        );
        assert!(failed["result"]["message"]
            .as_str()
            .unwrap()
            .contains("not approved for \u{201c}World\u{201d}"));
        let address = step(
            &probing["state"],
            json!({"type":"check_end","generation":1,"outcome":"address_error","now":"2026-01-01T00:00:00Z"}),
        );
        assert_eq!(
            address["result"]["message"],
            "Cannot read the saved address"
        );
        // Removing a server cancels its probe and closes the entry until restored.
        let removed = step(&probing["state"], json!({"type":"remove"}));
        assert_eq!(
            removed["effects"],
            json!([{"type":"cancel_probe","generation":1}])
        );
        assert_eq!(removed["check"], unknown_check());
        assert_eq!(
            step(&removed["state"], json!({"type":"check"}))["effects"],
            json!([])
        );
        let restored = step(&removed["state"], json!({"type":"restore"}));
        assert_eq!(
            step(&restored["state"], json!({"type":"join","name":"World"}))["error"],
            ""
        );
    }

    #[test]
    fn demo_joins_require_demo_mode_and_a_known_server() {
        let off = step(
            &Value::Null,
            json!({"type":"demo_join","enabled":false,"valid":true}),
        );
        assert_eq!(off["error"], "demo mode is off");
        let unknown = step(
            &Value::Null,
            json!({"type":"demo_join","enabled":true,"valid":false}),
        );
        assert_eq!(unknown["error"], "server not found");
        let joined = step(
            &Value::Null,
            json!({"type":"demo_join","enabled":true,"valid":true}),
        );
        assert_eq!(
            joined["effects"],
            json!([{"type":"start_timer","generation":1}])
        );
        assert_eq!(
            joined["connection"]["message"],
            "Simulated connection (demo)"
        );
        assert_eq!(joined["logs"], json!([]), "demo connects are not logged");
        let again = step(
            &joined["state"],
            json!({"type":"demo_join","enabled":true,"valid":true}),
        );
        assert_eq!(again["error"], "already connecting or connected");
        let connected = step(
            &joined["state"],
            json!({"type":"demo_timer","generation":1,"port":19132,"now":"2026-01-01T00:00:00Z"}),
        );
        assert_eq!(connected["connection"]["localAddr"], "127.0.0.1:19132");
        assert_eq!(connected["connection"]["simulated"], true);
        assert_eq!(connected["notify"], true);
    }

    #[test]
    fn reconnect_transitions_are_logged_once() {
        let state = json!({"active":1,"name":"World","tunnel":true,"lastStatus":"connected","status":{"state":"connected"}});
        let reconnecting = step(
            &state,
            json!({"type":"status","generation":1,"status":{"state":"reconnecting"}}),
        );
        assert_eq!(reconnecting["logs"][0]["kind"], "warning");
        let repeated = step(
            &reconnecting["state"],
            json!({"type":"status","generation":1,"status":{"state":"reconnecting"}}),
        );
        assert_eq!(repeated["logs"], json!([]));
        let back = step(
            &repeated["state"],
            json!({"type":"status","generation":1,"status":{"state":"connected"}}),
        );
        assert_eq!(back["logs"][0]["kind"], "connect");
        assert!(back["logs"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Reconnected"));
        let disconnected = step(&back["state"], json!({"type":"disconnect"}));
        assert_eq!(disconnected["logs"][0]["kind"], "disconnect");
        assert_eq!(
            disconnected["effects"],
            json!([{"type":"stop","generation":1}])
        );
        assert_eq!(disconnected["connection"]["state"], "disconnected");
    }

    #[test]
    fn exhausted_generation_counter_is_an_error_and_oversized_state_is_refused() {
        let error = reduce(json!({"state":{"serial":u64::MAX},"event":{"type":"join","name":"x"}}))
            .unwrap_err();
        assert_eq!(error, "client generation exhausted");
        let huge = "m".repeat(LIMIT);
        assert_eq!(
            reduce(json!({"state":{"message":huge},"event":{"type":"view"}})).unwrap_err(),
            "client lifecycle input exceeds 32 KiB"
        );
        // State plus event data together stay bounded as well.
        let error = reduce(
            json!({"state":{"active":1,"message":"m".repeat(LIMIT - 400)},"event":{"type":"status","generation":1,"status":{"state":"connected","message":"s".repeat(600)}}}),
        )
        .unwrap_err();
        assert!(error.contains("exceeds 32 KiB"), "{error}");
    }
}
