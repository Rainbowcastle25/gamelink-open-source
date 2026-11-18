//! Desktop application policy. The Go host supplies network observations and
//! executes the returned semantic effects; it does not validate UI commands.
use crate::pure;
use serde_json::{json, Value};
use std::path::Path;
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn rows(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn game(typ: &Value) -> Value {
    rows(&pure::dispatch("games.all", json!({})).expect("catalog"))
        .iter()
        .find(|g| g["type"] == *typ)
        .cloned()
        .unwrap_or(Value::Null)
}
fn args(settings: &Value, info: &Value) -> Value {
    settings["gameLaunchArgs"]
        .get(text(info, "type"))
        .cloned()
        .unwrap_or_else(|| info["launchArgs"].clone())
}
fn executable(p: &str) -> Result<(), String> {
    if !Path::new(p).is_absolute() {
        return Err("use a full path to the game's executable".into());
    }
    let meta = std::fs::metadata(p).map_err(|_| "that file does not exist")?;
    if meta.is_dir() {
        return Err("that is a folder, not a program".into());
    }
    Ok(())
}
const BRIDGE: &str = "this invitation requires the Android Console Bridge; use a standard game invitation with the desktop app";
pub fn dispatch(method: &str, v: Value) -> Result<Value, String> {
    match method {
        "desktop.executable" => {
            executable(text(&v, "path"))?;
            Ok(Value::Null)
        }
        "desktop.settings_view" => {
            let mut out = v["settings"].clone();
            out["gameExecutables"] = out["gameExecutables"]
                .as_object()
                .cloned()
                .map(Value::Object)
                .unwrap_or(json!({}));
            let mut templates = json!({});
            for info in rows(&pure::dispatch("games.all", json!({}))?) {
                templates[text(info, "type")] = args(&out, info);
            }
            out["gameLaunchArgs"] = templates;
            Ok(out)
        }
        "desktop.settings_prepare" => {
            let mut out = v["settings"].clone();
            let mut exes = json!({});
            if let Some(values) = out["gameExecutables"].as_object() {
                for (typ, p) in values {
                    let p = p.as_str().unwrap_or("").trim();
                    if p.is_empty() {
                        continue;
                    }
                    if v["saved"]["gameExecutables"][typ] != p {
                        executable(p).map_err(|e| format!("{typ}: {e}"))?;
                    }
                    exes[typ] = json!(p);
                }
            }
            let mut templates = json!({});
            if let Some(values) = out["gameLaunchArgs"].as_object() {
                for (typ, a) in values {
                    let info = game(&json!(typ));
                    if info.is_null() {
                        return Err(format!("unknown game type {typ:?}"));
                    }
                    let a = a.as_str().unwrap_or("").trim();
                    if a != text(&info, "launchArgs") {
                        templates[typ] = json!(a);
                    }
                }
            }
            out["gameExecutables"] = exes;
            out["gameLaunchArgs"] = templates;
            Ok(out)
        }
        "desktop.preview" => {
            let inv = &v["invite"];
            let details = &v["details"];
            if inv["consoleBridge"].as_u64().unwrap_or(0) != 0 {
                return Err(BRIDGE.into());
            }
            let mut out = inv.clone();
            out.as_object_mut()
                .ok_or("invalid invitation")?
                .remove("addr");
            out["gameName"] = game(&inv["game"])["name"].clone();
            for field in ["relayHosts", "relayRegion", "defaultRelay", "serverKey"] {
                out[field] = details[field].clone();
            }
            out["warnings"] = if details["defaultRelay"] == true {
                json!([])
            } else {
                let hosts = rows(&details["relayHosts"])
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                json!([format!("This server uses a custom relay ({hosts}) instead of the public Tailcat relays. Only continue if you trust whoever sent you this invitation.")])
            };
            Ok(out)
        }
        "desktop.server_input" => {
            let mut out = v["value"].clone();
            if v["import"] == true {
                if out["consoleBridge"].as_u64().unwrap_or(0) != 0 {
                    return Err(BRIDGE.into());
                }
                out["description"] = out["note"].clone();
                out["localPort"] = v["localPort"].clone();
            } else {
                out["addr"] = out["address"].clone();
            }
            if v["import"] != true {
                pure::dispatch("security.port", json!({"port":out["port"]}))?;
            }
            pure::dispatch(
                "security.port",
                json!({"port":out["localPort"],"optional":true}),
            )?;
            for field in ["name", "description", "addr"] {
                out[field] = json!(text(&out, field).trim());
            }
            // Match the former typed Go conversion: only saved-record fields
            // may reach storage. In particular, `address` is an unsealed secret.
            out.as_object_mut()
                .ok_or("invalid server input")?
                .retain(|key, _| {
                    matches!(
                        key.as_str(),
                        "name"
                            | "description"
                            | "game"
                            | "proto"
                            | "port"
                            | "localPort"
                            | "addr"
                            | "expiresAt"
                    )
                });
            Ok(out)
        }
        "desktop.launch_plan" => {
            let conn = &v["connection"];
            let port = conn["localPort"].as_u64().unwrap_or(0);
            if !matches!(text(conn, "state"), "connected" | "reconnecting") || port == 0 {
                return Err("join the server first".into());
            }
            let info = game(&v["server"]["game"]);
            let typ = text(&info, "type");
            let exe = text(&v["settings"]["gameExecutables"], typ);
            let address = format!("127.0.0.1:{port}");
            let (kind, target, template, message_prefix) = if exe.is_empty() {
                if text(&info, "launchUri").is_empty() || v["canOpenURI"] != true {
                    return Err("no executable is configured for this game (Settings → Local game executables)".into());
                }
                (
                    "uri",
                    text(&info, "launchUri").to_owned(),
                    info["launchUri"].clone(),
                    format!("Opened {}", text(&info, "name")),
                )
            } else {
                executable(exe)?;
                (
                    "program",
                    exe.to_owned(),
                    args(&v["settings"], &info),
                    format!(
                        "Launched {}",
                        Path::new(exe)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    ),
                )
            };
            let auto = kind == "uri"
                || pure::dispatch("games.placeholder", json!({"text":template}))? == true;
            let expanded = pure::dispatch("games.expand", json!({"text":template,"port":port}))?;
            let message = if auto {
                format!("{message_prefix} and asked it to join {address}")
            } else {
                message_prefix
            };
            Ok(
                json!({"kind":kind,"target":if kind=="uri"{expanded.clone()}else{json!(target)},"args":expanded,"message":message,"result":{"address":address,"autoConnect":auto}}),
            )
        }
        "desktop.overview" => {
            let mut servers = 0;
            let mut connected = 0;
            let mut online = 0;
            let mut status = "idle";
            for server in rows(&v["servers"]) {
                if server["demo"] == true {
                    continue;
                }
                servers += 1;
                match text(&server["connection"], "state") {
                    "connected" => connected += 1,
                    "connecting" | "reconnecting" if status == "idle" => status = "connecting",
                    "error" | "unreachable" if connected == 0 => status = "problem",
                    _ => (),
                }
                if (server["connection"]["state"] == "connected"
                    && server["connection"]["gameReachable"] == "yes")
                    || (server["connection"]["state"] != "connected"
                        && server["check"]["state"] == "online")
                {
                    online += 1;
                }
            }
            if connected > 0 {
                status = "connected";
            }
            let key = text(&v, "publicKey");
            let short = if key.len() > 19 {
                format!("{}…", key.chars().take(19).collect::<String>())
            } else {
                key.into()
            };
            Ok(
                json!({"servers":servers,"connected":connected,"online":online,"devices":1,"publicKeyShort":short,"status":status,"security":"Private keys stay on this device. Only devices approved by a server's owner can connect."}),
            )
        }
        "desktop.join_parameters" => {
            let server = &v["server"];
            let info = game(&server["game"]);
            let prefer = if server["localPort"].as_u64().unwrap_or(0) != 0 {
                json!(0)
            } else if info["defaultPort"].as_u64().unwrap_or(0) != 0 {
                info["defaultPort"].clone()
            } else {
                server["port"].clone()
            };
            Ok(json!({"preferPort":prefer}))
        }
        _ => Err("unknown desktop policy method".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_validate_only_changed_paths_and_preserve_empty_override() {
        let settings = json!({"demoMode":true,"gameExecutables":{"minecraft-java":"/missing/program"},"gameLaunchArgs":{"factorio":"","minecraft-java":"  --custom  "}});
        let out = dispatch(
            "desktop.settings_prepare",
            json!({"settings":settings,"saved":settings}),
        )
        .unwrap();
        assert_eq!(out["gameLaunchArgs"]["factorio"], "");
        assert_eq!(out["gameLaunchArgs"]["minecraft-java"], "--custom");
        assert!(dispatch(
            "desktop.settings_prepare",
            json!({"settings":settings,"saved":{}})
        )
        .is_err());
        assert!(dispatch(
            "desktop.settings_prepare",
            json!({"settings":{"gameLaunchArgs":{"unknown":"x"}},"saved":{}})
        )
        .is_err());
    }
    #[test]
    fn manual_server_discards_unsealed_form_address() {
        let mut out = dispatch(
            "desktop.server_input",
            json!({"value":{
                "name":"World", "game":"minecraft-java", "proto":"tcp",
                "port":25565, "localPort":0, "address":"synthetic-secret"
            }}),
        )
        .unwrap();
        assert_eq!(out["addr"], "synthetic-secret");
        assert!(out.get("address").is_none());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        out["addr"] = json!(crate::storage::seal(text(&out, "addr")).unwrap());
        out["id"] = json!("test-server");
        out["addedAt"] = json!("2026-01-01T00:00:00Z");
        crate::config::dispatch(
            "config.client_mutate",
            json!({
                "path":path, "config":{"version":1,"servers":[]}, "action":"add", "value":out
            }),
        )
        .unwrap();
        let persisted: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(persisted["servers"][0].get("address").is_none());
        assert_eq!(
            crate::storage::unseal(text(&persisted["servers"][0], "addr")).unwrap(),
            "synthetic-secret"
        );
    }
    #[test]
    fn preview_rejects_bridge_and_never_returns_full_address() {
        let invite = json!({"game":"minecraft-java","addr":"synthetic-address","consoleBridge":0});
        let details =
            json!({"relayHosts":["relay.example"],"defaultRelay":false,"serverKey":"short"});
        let out = dispatch(
            "desktop.preview",
            json!({"invite":invite,"details":details}),
        )
        .unwrap();
        assert!(out.get("addr").is_none());
        assert_eq!(out["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(
            dispatch(
                "desktop.server_input",
                json!({"value":{"consoleBridge":1},"import":true,"localPort":0})
            )
            .unwrap_err(),
            BRIDGE
        );
    }
    #[test]
    fn launch_requires_connection_and_expands_only_local_listener() {
        let exe = tempfile::NamedTempFile::new().unwrap();
        let input = json!({"server":{"game":"minecraft-java"},"connection":{"state":"reconnecting","localPort":32123},"settings":{"gameExecutables":{"minecraft-java":exe.path()},"gameLaunchArgs":{"minecraft-java":"{address} {host}:{port}"}},"canOpenURI":false});
        let out = dispatch("desktop.launch_plan", input.clone()).unwrap();
        assert_eq!(out["args"], "127.0.0.1:32123 127.0.0.1:32123");
        assert_eq!(out["result"]["autoConnect"], true);
        let mut disconnected = input;
        disconnected["connection"]["state"] = json!("disconnected");
        assert_eq!(
            dispatch("desktop.launch_plan", disconnected).unwrap_err(),
            "join the server first"
        );
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn exe_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().unwrap()
    }

    #[test]
    fn executable_validation_messages() {
        let call = |p: &str| dispatch("desktop.executable", json!({"path":p}));
        assert_eq!(
            call("relative/game").unwrap_err(),
            "use a full path to the game's executable"
        );
        assert_eq!(
            call("").unwrap_err(),
            "use a full path to the game's executable"
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            call(dir.path().to_str().unwrap()).unwrap_err(),
            "that is a folder, not a program"
        );
        let missing = dir.path().join("missing");
        assert_eq!(
            call(missing.to_str().unwrap()).unwrap_err(),
            "that file does not exist"
        );
        let file = exe_file();
        assert_eq!(call(file.path().to_str().unwrap()).unwrap(), Value::Null);
        assert_eq!(
            dispatch("desktop.nope", json!({})).unwrap_err(),
            "unknown desktop policy method"
        );
    }

    #[test]
    fn settings_view_fills_launch_templates_and_defaults_maps() {
        let out = dispatch(
            "desktop.settings_view",
            json!({"settings":{"demoMode":true,"gameExecutables":null,"gameLaunchArgs":{"factorio":"--custom"}}}),
        )
        .unwrap();
        assert_eq!(out["demoMode"], true);
        assert_eq!(out["gameExecutables"], json!({}));
        assert_eq!(out["gameLaunchArgs"]["factorio"], "--custom");
        assert_eq!(out["gameLaunchArgs"]["minecraft-java"], "");
        assert_eq!(out["gameLaunchArgs"]["custom"], "");
        // An explicit empty override is not the same as the default template.
        let out = dispatch(
            "desktop.settings_view",
            json!({"settings":{"gameLaunchArgs":{"factorio":""}}}),
        )
        .unwrap();
        assert_eq!(out["gameLaunchArgs"]["factorio"], "");
        let out = dispatch("desktop.settings_view", json!({"settings":{}})).unwrap();
        assert_eq!(out["gameLaunchArgs"]["factorio"], "--mp-connect {address}");
    }

    #[test]
    fn settings_prepare_trims_skips_blanks_and_names_the_bad_game() {
        let file = exe_file();
        let path = file.path().to_str().unwrap().to_owned();
        let out = dispatch(
            "desktop.settings_prepare",
            json!({"settings":{
                "gameExecutables":{"factorio":format!("  {path}  "),"custom":"   "},
                "gameLaunchArgs":{"factorio":"  --mp-connect {address}  ","custom":" x "}
            },"saved":{}}),
        )
        .unwrap();
        assert_eq!(out["gameExecutables"], json!({"factorio":path}));
        // The default template is not stored as an override; custom values are.
        assert_eq!(out["gameLaunchArgs"], json!({"custom":"x"}));
        let error = dispatch(
            "desktop.settings_prepare",
            json!({"settings":{"gameExecutables":{"factorio":"relative"}},"saved":{}}),
        )
        .unwrap_err();
        assert_eq!(error, "factorio: use a full path to the game's executable");
        // An unchanged saved path is not revalidated, so a moved game still saves.
        let missing = "/definitely/missing/game";
        assert!(dispatch(
            "desktop.settings_prepare",
            json!({"settings":{"gameExecutables":{"factorio":missing}},"saved":{"gameExecutables":{"factorio":missing}}}),
        )
        .is_ok());
    }

    #[test]
    fn preview_with_the_default_relay_has_no_warning() {
        let out = dispatch(
            "desktop.preview",
            json!({"invite":{"game":"factorio","addr":"secret","name":"n"},"details":{"relayHosts":["a"],"relayRegion":"r","defaultRelay":true,"serverKey":"k"}}),
        )
        .unwrap();
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(out["gameName"], "Factorio");
        assert_eq!(out["relayRegion"], "r");
        assert!(out.get("addr").is_none());
        assert_eq!(
            dispatch(
                "desktop.preview",
                json!({"invite":{"consoleBridge":1},"details":{}})
            )
            .unwrap_err(),
            BRIDGE
        );
        // The custom-relay warning names every host.
        let out = dispatch(
            "desktop.preview",
            json!({"invite":{"game":"factorio"},"details":{"relayHosts":["a.example","b.example"],"defaultRelay":false}}),
        )
        .unwrap();
        assert!(out["warnings"][0]
            .as_str()
            .unwrap()
            .contains("a.example, b.example"));
    }

    #[test]
    fn server_input_for_imports_and_manual_forms() {
        let out = dispatch(
            "desktop.server_input",
            json!({"import":true,"localPort":30000,"value":{
                "name":"  World ","note":" hi ","game":"factorio","proto":"udp","port":34197,
                "addr":" tcsecret ","relay":"ignored","extra":1}}),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"name":"World","description":"hi","game":"factorio","proto":"udp","port":34197,"localPort":30000,"addr":"tcsecret"})
        );
        // Manual forms: the port is validated and the address is trimmed.
        let form = json!({"name":"W","game":"factorio","proto":"udp","port":0,"localPort":0,"address":" tc "});
        assert!(dispatch("desktop.server_input", json!({"value":form})).is_err());
        let form = json!({"name":"W","game":"factorio","proto":"udp","port":5,"localPort":70000,"address":"tc"});
        assert!(dispatch("desktop.server_input", json!({"value":form})).is_err());
        assert!(dispatch(
            "desktop.server_input",
            json!({"import":true,"localPort":70000,"value":{"name":"x"}})
        )
        .is_err());
        assert_eq!(
            dispatch(
                "desktop.server_input",
                json!({"import":true,"localPort":0,"value":{"consoleBridge":1}})
            )
            .unwrap_err(),
            BRIDGE
        );
    }

    #[test]
    fn launch_plan_variants() {
        let connected = json!({"state":"connected","localPort":32123});
        let uri = dispatch(
            "desktop.launch_plan",
            json!({"server":{"game":"minecraft-bedrock"},"connection":connected,"settings":{},"canOpenURI":true}),
        )
        .unwrap();
        assert_eq!(uri["kind"], "uri");
        assert_eq!(
            uri["target"],
            "minecraft://connect?serverUrl=127.0.0.1&serverPort=32123"
        );
        assert_eq!(uri["result"]["autoConnect"], true);
        assert_eq!(
            uri["message"],
            "Opened Minecraft: Bedrock Edition and asked it to join 127.0.0.1:32123"
        );
        // URIs are unavailable off Windows, and games without one need an executable.
        for (game, can_open) in [("minecraft-bedrock", false), ("factorio", true)] {
            assert_eq!(
                dispatch(
                    "desktop.launch_plan",
                    json!({"server":{"game":game},"connection":connected,"settings":{},"canOpenURI":can_open}),
                )
                .unwrap_err(),
                "no executable is configured for this game (Settings \u{2192} Local game executables)"
            );
        }
        // A configured executable that disappeared is reported.
        let scratch = tempfile::tempdir().unwrap();
        let gone = scratch.path().join("factorio");
        let error = dispatch(
            "desktop.launch_plan",
            json!({"server":{"game":"factorio"},"connection":connected,"settings":{"gameExecutables":{"factorio":gone}},"canOpenURI":false}),
        )
        .unwrap_err();
        assert_eq!(error, "that file does not exist");
        // A template without placeholders does not claim to auto-connect.
        let file = exe_file();
        let out = dispatch(
            "desktop.launch_plan",
            json!({"server":{"game":"factorio"},"connection":connected,"settings":{"gameExecutables":{"factorio":file.path()},"gameLaunchArgs":{"factorio":"--windowed"}},"canOpenURI":false}),
        )
        .unwrap();
        assert_eq!(out["kind"], "program");
        assert_eq!(out["args"], "--windowed");
        assert_eq!(out["result"]["autoConnect"], false);
        assert!(out["message"].as_str().unwrap().starts_with("Launched "));
        assert_eq!(out["result"]["address"], "127.0.0.1:32123");
        // The default template applies when no override exists.
        let out = dispatch(
            "desktop.launch_plan",
            json!({"server":{"game":"factorio"},"connection":connected,"settings":{"gameExecutables":{"factorio":file.path()}},"canOpenURI":false}),
        )
        .unwrap();
        assert_eq!(out["args"], "--mp-connect 127.0.0.1:32123");
        // No local listener yet.
        for connection in [
            json!({"state":"connected","localPort":0}),
            json!({"state":"connecting","localPort":5}),
        ] {
            assert_eq!(
                dispatch(
                    "desktop.launch_plan",
                    json!({"server":{"game":"factorio"},"connection":connection,"settings":{},"canOpenURI":true}),
                )
                .unwrap_err(),
                "join the server first"
            );
        }
    }

    #[test]
    fn overview_summarises_connection_state() {
        let overview = |servers: Value, key: &str| {
            dispatch(
                "desktop.overview",
                json!({"servers":servers,"publicKey":key}),
            )
            .unwrap()
        };
        let s = |conn: &str, check: &str| json!({"connection":{"state":conn,"gameReachable":if conn == "connected" {"yes"} else {"unknown"}},"check":{"state":check}});
        let idle = overview(json!([]), "short");
        assert_eq!(idle["status"], "idle");
        assert_eq!(idle["servers"], 0);
        assert_eq!(idle["publicKeyShort"], "short");
        assert_eq!(idle["devices"], 1);

        let out = overview(
            json!([s("disconnected", "online"), s("disconnected", "unknown")]),
            "",
        );
        assert_eq!(
            (out["servers"].as_i64(), out["online"].as_i64()),
            (Some(2), Some(1))
        );
        assert_eq!(out["status"], "idle");

        let offline_tunnel = json!({"connection":{"state":"connected","gameReachable":"no"},"check":{"state":"unknown"}});
        let unknown_tunnel = json!({"connection":{"state":"connected","gameReachable":"unknown"},"check":{"state":"unknown"}});
        assert_eq!(overview(json!([offline_tunnel]), "")["online"], 0);
        assert_eq!(overview(json!([unknown_tunnel]), "")["online"], 0);
        let stale_online_check = json!({"connection":{"state":"connected","gameReachable":"no"},"check":{"state":"online"}});
        assert_eq!(overview(json!([stale_online_check]), "")["online"], 0);

        assert_eq!(
            overview(json!([s("connecting", "")]), "")["status"],
            "connecting"
        );
        assert_eq!(
            overview(json!([s("reconnecting", "")]), "")["status"],
            "connecting"
        );
        assert_eq!(overview(json!([s("error", "")]), "")["status"], "problem");
        assert_eq!(
            overview(json!([s("unreachable", "")]), "")["status"],
            "problem"
        );
        // A connected server wins over problems anywhere in the list.
        let out = overview(
            json!([s("error", ""), s("connected", ""), s("unreachable", "")]),
            "",
        );
        assert_eq!(out["status"], "connected");
        assert_eq!(out["connected"], 1);
        assert_eq!(out["online"], 1);
        // Demo servers are never counted.
        let demo =
            json!({"demo":true,"connection":{"state":"connected"},"check":{"state":"online"}});
        let out = overview(json!([demo]), "");
        assert_eq!(
            (out["servers"].as_i64(), out["connected"].as_i64()),
            (Some(0), Some(0))
        );
        assert_eq!(out["status"], "idle");
        // Long keys are shortened on a character boundary.
        let key = format!("nodekey:{}", "a".repeat(64));
        assert_eq!(
            overview(json!([]), &key)["publicKeyShort"],
            format!("{}\u{2026}", &key[..19])
        );
    }

    #[test]
    fn join_parameters_prefer_the_requested_then_default_then_server_port() {
        let params = |server: Value| {
            dispatch("desktop.join_parameters", json!({"server":server})).unwrap()["preferPort"]
                .clone()
        };
        assert_eq!(
            params(json!({"game":"factorio","localPort":4000,"port":1})),
            0
        );
        assert_eq!(
            params(json!({"game":"factorio","localPort":0,"port":1})),
            34197
        );
        assert_eq!(
            params(json!({"game":"custom","localPort":0,"port":4242})),
            4242
        );
        assert_eq!(params(json!({"game":"unknown","port":7})), 7);
    }
}
