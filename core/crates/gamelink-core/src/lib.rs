#![deny(unsafe_code)]
//! Shared GameLink logic and private storage behind the JNI and C ABI hosts.
//! FFI lives in gamelink-ffi; unsafe DPAPI calls are isolated in storage.

pub mod launch;
pub mod pure;

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// Upper bound on any serialized input accepted by the core.
pub const MAX_INPUT_BYTES: usize = 32 * 1024;
const MAX_ID_BYTES: usize = 64;
// Go's canonical invitation parser accepts up to 64 Unicode characters.
// Metadata must accommodate their maximum UTF-8 representation.
const MAX_NAME_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Connecting,
    Connected,
    Reconnecting,
    Unreachable,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub phase: Phase,
    pub generation: u64,
    pub message: Option<String>,
    pub name: Option<String>,
    pub server_id: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            generation: 0,
            message: None,
            name: None,
            server_id: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    StartTransport {
        generation: u64,
        server_id: String,
        name: String,
        local_ipv4: String,
    },
    StopTransport {
        generation: u64,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Reduction {
    pub state: State,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerMetadata {
    id: String,
    name: String,
    game: String,
    #[serde(default)]
    bridge_version: Option<u32>,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Event {
    Start {
        server: ServerMetadata,
        local_ipv4: String,
    },
    Stop,
    NetworkLost {
        generation: u64,
    },
    TransportStatus {
        generation: u64,
        status: String,
    },
    Error {
        generation: u64,
        code: String,
    },
}

#[derive(Deserialize)]
struct Input {
    state: State,
    event: Event,
}

#[derive(Debug, PartialEq)]
pub enum ReduceError {
    InputTooLarge,
    InvalidJson,
    GenerationExhausted,
}

/// Reduce one serialized JSON event into the next safe UI state and effects.
/// The input contains only metadata already validated by GameLink's Go layer;
/// invitations, Tailcat addresses, and keys are not part of this interface.
pub fn reduce_json(input: &str) -> Result<String, ReduceError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(ReduceError::InputTooLarge);
    }
    let input: Input = serde_json::from_str(input).map_err(|_| ReduceError::InvalidJson)?;
    let reduction = reduce(input.state, input.event)?;
    serde_json::to_string(&reduction).map_err(|_| ReduceError::InvalidJson)
}

fn reduce(mut state: State, event: Event) -> Result<Reduction, ReduceError> {
    let mut effects = Vec::new();
    match event {
        Event::Start { server, local_ipv4 } => {
            if matches!(state.phase, Phase::Connecting | Phase::Connected)
                && state.server_id.as_deref() == Some(server.id.as_str())
            {
                return Ok(Reduction { state, effects });
            }
            if let Err(message) = validate_server(&server) {
                stop_current(&mut state, &mut effects)?;
                state.phase = Phase::Error;
                state.message = Some(message.to_owned());
                state.name = None;
                state.server_id = None;
                return Ok(Reduction { state, effects });
            }
            let address: Ipv4Addr = match local_ipv4.parse::<Ipv4Addr>() {
                Ok(ip) if ip.is_private() => ip,
                _ => {
                    stop_current(&mut state, &mut effects)?;
                    state.phase = Phase::Error;
                    state.message = Some(
                        "Choose a private IPv4 address on the same Wi-Fi network as the console."
                            .to_owned(),
                    );
                    state.name = None;
                    state.server_id = None;
                    return Ok(Reduction { state, effects });
                }
            };
            stop_current(&mut state, &mut effects)?;
            state.generation = next_generation(state.generation)?;
            state.phase = Phase::Connecting;
            state.message = Some(format!("Starting bridge for {}…", server.name));
            state.name = Some(server.name.clone());
            state.server_id = Some(server.id.clone());
            effects.push(Effect::StartTransport {
                generation: state.generation,
                server_id: server.id,
                name: server.name,
                local_ipv4: address.to_string(),
            });
        }
        Event::Stop => {
            stop_current(&mut state, &mut effects)?;
            state.phase = Phase::Idle;
            state.message = None;
            state.name = None;
            state.server_id = None;
        }
        Event::NetworkLost { generation } => {
            if state.generation == generation && state.phase != Phase::Idle {
                stop_current(&mut state, &mut effects)?;
                state.phase = Phase::Reconnecting;
                state.message = Some(
                    "The network changed. Start the bridge again when Wi-Fi is ready.".to_owned(),
                );
            }
        }
        Event::TransportStatus { generation, status } => {
            if generation == state.generation {
                match status.as_str() {
                    "connected"
                        if state.phase == Phase::Connecting
                            || state.phase == Phase::Reconnecting
                            || state.phase == Phase::Unreachable =>
                    {
                        state.phase = Phase::Connected;
                        state.message = state
                            .name
                            .as_ref()
                            .map(|name| format!("Connected to {name}"));
                    }
                    "reconnecting" if state.phase != Phase::Idle => {
                        state.phase = Phase::Reconnecting;
                        state.message = Some("Trying to reconnect…".to_owned());
                    }
                    "unreachable" if state.phase != Phase::Idle => {
                        state.phase = Phase::Unreachable;
                        state.message = Some(
                            "The server may be offline, or this device may need approval."
                                .to_owned(),
                        );
                    }
                    "connected" | "reconnecting" | "unreachable" => {}
                    _ => {}
                }
            }
        }
        Event::Error { generation, code } => {
            if generation == state.generation && state.phase != Phase::Idle {
                stop_current(&mut state, &mut effects)?;
                state.phase = Phase::Error;
                state.message = Some(error_message(&code).to_owned());
            }
        }
    }
    Ok(Reduction { state, effects })
}

fn stop_current(state: &mut State, effects: &mut Vec<Effect>) -> Result<(), ReduceError> {
    if state.phase != Phase::Idle && state.phase != Phase::Error {
        let old_generation = state.generation;
        state.generation = next_generation(state.generation)?;
        effects.push(Effect::StopTransport {
            generation: old_generation,
        });
    }
    Ok(())
}

fn validate_server(server: &ServerMetadata) -> Result<(), &'static str> {
    // Metadata comes from Go's canonical invitation parser. These checks bound
    // native UI/effect output without trying to parse or authorize invitations.
    if server.game != "minecraft-bedrock"
        || server.id.is_empty()
        || server.id.len() > MAX_ID_BYTES
        || !server
            .id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        || server.name.is_empty()
        || server.name.len() > MAX_NAME_BYTES
        || server.name.chars().any(char::is_control)
    {
        // An invalid metadata record is treated as an unsupported host rather
        // than allowing untrusted strings to reach the Android effect runner.
        return Err("This Bedrock server invitation is not compatible with the console bridge.");
    }
    if server.bridge_version != Some(1) && !server.capabilities.iter().any(|v| v == "console-v1") {
        return Err(error_message("unsupported_bridge"));
    }
    Ok(())
}

fn next_generation(generation: u64) -> Result<u64, ReduceError> {
    generation
        .checked_add(1)
        .ok_or(ReduceError::GenerationExhausted)
}

fn error_message(code: &str) -> &'static str {
    match code {
        "unsupported_bridge" => {
            "This host agent does not support console bridging. Update the host agent and create a new invitation."
        }
        "offline_or_approval_needed" => {
            "The server may be offline, or this device may need approval."
        }
        "network_lost" => "The network changed. Start the bridge again when Wi-Fi is ready.",
        _ => "The bridge stopped unexpectedly. Try again or check the host agent status.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(state: &State, event: &str) -> String {
        format!(
            r#"{{"state":{},"event":{}}}"#,
            serde_json::to_string(state).unwrap(),
            event
        )
    }

    fn bedrock_metadata() -> &'static str {
        r#"{"id":"our-server","name":"World","game":"minecraft-bedrock","bridgeVersion":1}"#
    }

    fn reduce_to(input: &str) -> Reduction {
        serde_json::from_str(&reduce_json(input).unwrap()).unwrap()
    }

    #[test]
    fn start_requires_bridge_capability_and_private_wifi_ipv4() {
        let old_agent = input(
            &State::default(),
            r#"{"type":"start","server":{"id":"s","name":"World","game":"minecraft-bedrock"},"localIpv4":"192.168.1.9"}"#,
        );
        let got = reduce_to(&old_agent);
        assert_eq!(got.state.phase, Phase::Error);
        assert_eq!(
            got.state.message.as_deref(),
            Some(error_message("unsupported_bridge"))
        );
        assert!(got.effects.is_empty());

        let public_ip = input(
            &State::default(),
            &format!(
                r#"{{"type":"start","server":{},"localIpv4":"8.8.8.8"}}"#,
                bedrock_metadata()
            ),
        );
        let got = reduce_to(&public_ip);
        assert_eq!(got.state.phase, Phase::Error);
        assert!(got.effects.is_empty());
    }

    #[test]
    fn valid_start_emits_only_safe_transport_metadata() {
        let raw = input(
            &State::default(),
            &format!(
                r#"{{"type":"start","server":{},"localIpv4":"192.168.1.9"}}"#,
                bedrock_metadata()
            ),
        );
        let output = reduce_json(&raw).unwrap();
        let got: Reduction = serde_json::from_str(&output).unwrap();
        assert_eq!(got.state.phase, Phase::Connecting);
        assert_eq!(got.state.generation, 1);
        assert_eq!(got.effects.len(), 1);
        assert_eq!(
            got.effects[0],
            Effect::StartTransport {
                generation: 1,
                server_id: "our-server".to_owned(),
                name: "World".to_owned(),
                local_ipv4: "192.168.1.9".to_owned(),
            }
        );
        assert!(!output.contains("secret"));
        assert!(!output.contains("addr"));
    }

    #[test]
    fn go_validated_unicode_names_fit_the_metadata_boundary() {
        for name in ["界".repeat(64), "🎮".repeat(64)] {
            let event = serde_json::json!({"type":"start", "server":{
                "id":"our-server", "name":name, "game":"minecraft-bedrock", "bridgeVersion":1
            }, "localIpv4":"192.168.1.9"});
            let result = reduce_to(&input(&State::default(), &event.to_string()));
            assert_eq!(result.state.phase, Phase::Connecting);
            assert_eq!(result.state.name.as_deref(), Some(name.as_str()));
            assert_eq!(result.effects.len(), 1);
        }
    }

    #[test]
    fn stale_callbacks_are_ignored_and_connected_is_generation_scoped() {
        let started = reduce_to(&input(
            &State::default(),
            &format!(
                r#"{{"type":"start","server":{},"localIpv4":"10.0.0.4"}}"#,
                bedrock_metadata()
            ),
        ));
        let stale = reduce_to(&input(
            &started.state,
            r#"{"type":"transport_status","generation":0,"status":"connected"}"#,
        ));
        assert_eq!(stale.state.phase, Phase::Connecting);
        assert!(stale.effects.is_empty());

        let current = reduce_to(&input(
            &started.state,
            r#"{"type":"transport_status","generation":1,"status":"unreachable"}"#,
        ));
        assert_eq!(current.state.phase, Phase::Unreachable);
        assert_eq!(
            current.state.message.as_deref(),
            Some("The server may be offline, or this device may need approval.")
        );
        let approved = reduce_to(&input(
            &current.state,
            r#"{"type":"transport_status","generation":1,"status":"connected"}"#,
        ));
        assert_eq!(approved.state.phase, Phase::Connected);
        assert!(approved.effects.is_empty());
    }

    #[test]
    fn network_loss_stops_transport_and_invalidates_old_callbacks() {
        let active = State {
            phase: Phase::Connected,
            generation: 8,
            message: Some("Connected to World".into()),
            name: Some("World".into()),
            server_id: Some("our-server".into()),
        };
        let lost = reduce_to(&input(&active, r#"{"type":"network_lost","generation":8}"#));
        assert_eq!(lost.state.phase, Phase::Reconnecting);
        assert_eq!(lost.state.generation, 9);
        assert_eq!(lost.effects, vec![Effect::StopTransport { generation: 8 }]);
        let stale = reduce_to(&input(
            &lost.state,
            r#"{"type":"transport_status","generation":8,"status":"connected"}"#,
        ));
        assert_eq!(stale.state.phase, Phase::Reconnecting);
    }

    #[test]
    fn stop_is_idempotent_and_clears_server_metadata() {
        let active = State {
            phase: Phase::Connected,
            generation: 2,
            message: Some("Connected to World".into()),
            name: Some("World".into()),
            server_id: Some("our-server".into()),
        };
        let stopped = reduce_to(&input(&active, r#"{"type":"stop"}"#));
        assert_eq!(stopped.state.phase, Phase::Idle);
        assert_eq!(stopped.state.generation, 3);
        assert_eq!(
            stopped.effects,
            vec![Effect::StopTransport { generation: 2 }]
        );
        let repeated = reduce_to(&input(&stopped.state, r#"{"type":"stop"}"#));
        assert_eq!(repeated.state, stopped.state);
        assert!(repeated.effects.is_empty());
    }

    #[test]
    fn malformed_and_oversized_input_is_rejected() {
        assert_eq!(reduce_json("{"), Err(ReduceError::InvalidJson));
        assert_eq!(
            reduce_json(&" ".repeat(MAX_INPUT_BYTES + 1)),
            Err(ReduceError::InputTooLarge)
        );
    }
}

pub mod storage;

pub mod config;

pub mod client_lifecycle;
pub mod console;
pub mod discovery;
pub mod requests;
pub mod supervisor;

pub mod desktop;

#[cfg(feature = "update-notifications")]
pub mod update;
