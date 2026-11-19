mod bridge;
mod host_admin;
mod host_setup;
mod invitations;
mod pterodactyl;
mod requests;
mod runtime;
mod self_update;
use clap::{Arg, ArgAction, Command};
use gamelink_core::{config, pure, storage, update};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
type Result<T> = std::result::Result<T, String>;
const VERSION: &str = env!("CARGO_PKG_VERSION");
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn rows(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn media_ports(value: &str) -> Result<(u16, u16)> {
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 2 {
        return Err("--media-ports must be a first-last range".into());
    }
    let first = parts[0]
        .parse::<u16>()
        .map_err(|_| "invalid --media-ports")?;
    let last = parts[1]
        .parse::<u16>()
        .map_err(|_| "invalid --media-ports")?;
    Ok((first, last))
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
fn core(method: &str, v: Value) -> Result<Value> {
    if method.starts_with("config.") {
        config::dispatch(method, v)
    } else {
        pure::dispatch(method, v)
    }
}
struct Cli {
    dir: PathBuf,
}
impl Cli {
    fn path(&self) -> String {
        self.dir.join("agent.json").to_string_lossy().into_owned()
    }
    fn load(&self) -> Result<Value> {
        core("config.agent_load", json!({"path":self.path()}))
    }
    fn mutate(&self, action: &str, mut extra: Value) -> Result<Value> {
        extra["path"] = json!(self.path());
        extra["action"] = json!(action);
        core("config.agent_mutate", extra)
    }
    fn approve_device(
        &self,
        game: &str,
        key: &str,
        label: &str,
        expected_keys: &Value,
    ) -> Result<Value> {
        self.mutate("approve", json!({"id":game,"publicKey":key,"label":label,"now":now(),"expectedKeys":expected_keys}))
    }
    fn execute(&self, cmd: &str, m: &clap::ArgMatches) -> Result<i32> {
        let get = |k: &str| m.get_one::<String>(k).map(String::as_str).unwrap_or("");
        let flag = |k: &str| m.get_flag(k);
        match cmd {
            "init" => {
                self.mutate("init", json!({}))?;
                println!("Initialised {}\n\nNext: expose a game, for example\n  gamelink-agent games add --type minecraft-java",self.dir.display());
            }
            "games add" => {
                let typ = get("type");
                if typ.is_empty() {
                    return Err("--type is required (minecraft-java, minecraft-bedrock, factorio or custom)".into());
                }
                let port = get("port").parse::<i64>().unwrap_or(0);
                core("security.port", json!({"port":port,"optional":true}))
                    .map_err(|e| format!("--port: {e}"))?;
                let mut bridge = Value::Null;
                let mut bedrock = Value::Null;
                if flag("console-bridge") {
                    let (first, last) = media_ports(get("media-ports"))?;
                    bridge = json!({"serverIp":get("bedrock-ip"),"bindIp":get("bridge-bind-ip"),"mediaPortFirst":first,"mediaPortLast":last});
                    config::bridge(&bridge)?;
                } else if !get("bedrock-ip").is_empty() || !get("bridge-bind-ip").is_empty() {
                    return Err("bridge options require --console-bridge".into());
                } else if !get("media-ports").is_empty() {
                    if typ != "minecraft-bedrock" {
                        return Err("--media-ports requires --type minecraft-bedrock".into());
                    }
                    let (first, last) = media_ports(get("media-ports"))?;
                    bedrock = json!({"mediaPortFirst":first,"mediaPortLast":last});
                    config::bedrock(&bedrock)?;
                }
                if !self.dir.join("agent.json").exists() {
                    self.load()?;
                }
                let region = bridge::call(
                    json!({"method":"resolve","host":get("relay-host"),"regionID":get("relay-region").parse::<i64>().unwrap_or(0)}),
                )?;
                let mut game = core(
                    "config.game_prepare",
                    json!({"value":{"id":get("id"),"name":get("name"),"description":get("description"),"type":typ,"protocol":get("proto"),"tunnelPort":port,"dest":get("dest"),"consoleBridge":bridge,"bedrock":bedrock},"region":region}),
                )?;
                let mut keys = bridge::call(json!({"method":"generate"}))?;
                keys["created"] = json!(now());
                keys["region"] = region.clone();
                game["created"] = keys["created"].clone();
                game["devices"] = json!([]);
                self.mutate("add", json!({"value":game,"keys":keys}))?;
                println!(
                    "Added game {:?} ({})\n  {} port {} in the tunnel -> {}\n  relay: {}\n",
                    s(&game, "id"),
                    s(&game, "name"),
                    s(&game, "protocol"),
                    game["tunnelPort"],
                    s(&game, "dest"),
                    s(&region, "RegionName")
                );
                println!("Nobody can reach it yet: no devices are approved. Share a private invitation with:\n  gamelink-agent invite --game {}",s(&game,"id"));
            }
            "games list" => {
                let cfg = self.load()?;
                if flag("json") {
                    println!("{}", games_json(&cfg));
                    return Ok(0);
                }
                let games = rows(&cfg["games"]);
                if games.is_empty() {
                    println!("No games configured. Add one with: gamelink-agent games add --type minecraft-java");
                } else {
                    let mut table = vec![vec![
                        "ID".into(),
                        "NAME".into(),
                        "TYPE".into(),
                        "TUNNEL".into(),
                        "FORWARDS TO".into(),
                        "DEVICES".into(),
                    ]];
                    for g in games {
                        table.push(vec![
                            s(g, "id").into(),
                            s(g, "name").into(),
                            s(g, "type").into(),
                            format!("{}/{}", s(g, "protocol"), g["tunnelPort"]),
                            s(g, "dest").into(),
                            rows(&g["devices"]).len().to_string(),
                        ]);
                    }
                    print_table(table);
                }
            }
            "games remove" => {
                let id = get("positional");
                self.mutate("remove", json!({"id":id}))?;
                println!("Removed {id:?} and deleted its identity. Invitations for it no longer work. A running agent stops serving it within a few seconds.");
            }
            "devices approve" | "devices revoke" => {
                let game = get("game");
                if game.is_empty() || get("key").is_empty() {
                    return Err("--game and --key are required".into());
                }
                if cmd.ends_with("approve") {
                    let d = self.approve_device(game, get("key"), get("label"), &Value::Null)?;
                    print!("Approved {} for {game:?}", s(&d, "publicKey"));
                    if !s(&d, "label").is_empty() {
                        print!(" ({})", s(&d, "label"));
                    }
                    println!(".\nA running agent picks this up within a few seconds. Share the invitation privately: gamelink-agent invite --game {game}");
                } else {
                    self.mutate("revoke", json!({"id":game,"publicKey":get("key")}))?;
                    println!("Revoked. A running agent restarts {game:?} within a few seconds, which ends that device's open sessions and blocks new ones.\nOther approved devices reconnect automatically after the brief restart.");
                }
            }
            "devices list" => {
                let cfg = self.load()?;
                if flag("json") {
                    println!("{}", devices_json(&cfg, get("game")));
                    return Ok(0);
                }
                let mut table = vec![vec![
                    "GAME".into(),
                    "LABEL".into(),
                    "PUBLIC KEY".into(),
                    "APPROVED".into(),
                ]];
                for g in rows(&cfg["games"]) {
                    if !get("game").is_empty() && get("game") != s(g, "id") {
                        continue;
                    }
                    for d in rows(&g["devices"]) {
                        table.push(vec![
                            s(g, "id").into(),
                            s(d, "label").into(),
                            s(d, "publicKey").into(),
                            s(d, "approvedAt").chars().take(10).collect(),
                        ]);
                    }
                }
                if table.len() == 1 {
                    println!("No approved devices.");
                } else {
                    print_table(table);
                }
            }
            "invite" | "invite rotate" => self.invite(m, cmd == "invite rotate")?,
            "requests list" | "requests approve" | "requests dismiss" => self.requests(cmd, m)?,
            "status" => self.status(flag("json"))?,
            "update check" => {
                let checked = update::check_now(&self.dir, VERSION)?;
                if flag("json") {
                    println!("{}", json!(checked));
                } else {
                    print_update(&checked);
                }
            }
            "update stage" => {
                let version =
                    self_update::stage(m.get_one::<String>("version").map(String::as_str))?;
                println!("Staged signed agent update {version}");
            }
            "update install-staged" => self_update::install_staged()?,
            "host detect" => {
                if get("panel-url").is_empty() || get("token-file").is_empty() {
                    return Err("--panel-url and --token-file are required".into());
                }
                let node = if get("node-id").is_empty() {
                    None
                } else {
                    Some(
                        get("node-id")
                            .parse::<u64>()
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or("--node-id must be positive")?,
                    )
                };
                let out = (!get("out").is_empty()).then(|| std::path::Path::new(get("out")));
                let result = host_setup::detect(
                    get("panel-url"),
                    std::path::Path::new(get("token-file")),
                    node,
                    out,
                    &host_setup::local_ips()?,
                    &bridge::call,
                )?;
                if flag("json") {
                    println!("{result}");
                } else {
                    host_setup::print_detect(&result);
                }
                return Ok(host_setup::detect_exit(&result));
            }
            "host check" => {
                if get("config").is_empty() {
                    return Err("--config is required".into());
                }
                let listen = if get("listen").is_empty() {
                    "127.0.0.1:8467"
                } else {
                    get("listen")
                };
                let result = host_setup::check(
                    &self.dir,
                    std::path::Path::new(get("config")),
                    listen,
                    &bridge::call,
                );
                if flag("json") {
                    println!("{result}");
                } else {
                    for check in rows(&result["checks"]) {
                        println!(
                            "{}\t{}\t{}",
                            s(check, "id"),
                            s(check, "status"),
                            s(check, "detail")
                        );
                    }
                }
                return Ok(i32::from(result["ok"] != true));
            }
            "host refresh" => {
                if get("config").is_empty() {
                    return Err("--config is required".into());
                }
                let result = host_admin::refresh(&self.dir, std::path::Path::new(get("config")))?;
                if flag("json") {
                    println!("{result}");
                    return Ok(0);
                }
                println!(
                    "Refreshed {} game(s). No background discovery was scheduled.",
                    result["discovered"]
                );
                for status in rows(&result["statuses"]) {
                    println!(
                        "{}\t{}\t{}",
                        s(status, "gameId"),
                        s(status, "name"),
                        s(status, "state")
                    );
                }
            }
            "serve" => {
                let cfg = self.load()?;
                let stopping = Arc::new(AtomicBool::new(false));
                let signal = stopping.clone();
                ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
                    .map_err(|e| e.to_string())?;
                bridge::enable_signal_stack()?;
                if rows(&cfg["games"]).is_empty() {
                    log("no games configured yet; add one with `gamelink-agent games add`. Waiting for configuration changes.");
                }
                log(&format!(
                    "gamelink-agent {VERSION} started; configuration in {}",
                    self.dir.display()
                ));
                log("only approved devices can reach a game; games with no approved devices are not served");
                let _admin = if get("host-config").is_empty() {
                    None
                } else {
                    let address = if get("host-listen").is_empty() {
                        "127.0.0.1:8467"
                    } else {
                        get("host-listen")
                    };
                    let admin = host_admin::start(
                        &self.dir,
                        std::path::Path::new(get("host-config")),
                        address,
                    )?;
                    log("host administration enabled on loopback; Panel discovery runs only on Refresh");
                    Some(admin)
                };
                runtime::serve(&self.dir, &stopping, flag("verbose"))?;
                log("shut down cleanly");
            }
            _ => unreachable!(),
        }
        Ok(0)
    }
    fn status(&self, json: bool) -> Result<()> {
        let cfg = self.load()?;
        let runtime = read_runtime(&self.dir);
        let mut snapshot = status_json(&cfg, &runtime);
        snapshot["agentVersion"] = json!(VERSION);
        snapshot["updateState"] = self_update::status();
        let checked = update::cached(&self.dir, VERSION);
        if let Some(checked) = &checked {
            snapshot["update"] = json!(checked);
        }
        if json {
            println!("{snapshot}");
            return Ok(());
        }
        let fresh = runtime_fresh(&runtime);
        if fresh {
            let started = chrono::DateTime::parse_from_rfc3339(s(&runtime, "started"))
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_default();
            println!(
                "Agent: running (pid {}, up since {started})\n",
                runtime["pid"]
            );
        } else {
            println!("Agent: not running (start it with `gamelink-agent serve`)\n");
        }
        if let Some(checked) = &checked {
            println!("Last verified release check ({})", checked.checked_at);
            print_update(checked);
            println!();
        }
        if rows(&cfg["games"]).is_empty() {
            println!("No games configured.");
            return Ok(());
        }
        let mut table = vec![vec![
            "GAME".into(),
            "TUNNEL".into(),
            "FORWARDS TO".into(),
            "DEVICES".into(),
            "STATE".into(),
            "ACTIVE".into(),
            "ACCEPTED".into(),
            "DENIED".into(),
        ]];
        for g in rows(&snapshot["games"]) {
            let n = g["approvedDevices"].as_u64().unwrap_or(0);
            let mut state = s(g, "state").to_owned();
            if !s(g, "detail").is_empty() {
                state += &format!(" ({})", s(g, "detail"));
            }
            let mut stats = vec!["-".into(); 3];
            if !g["stats"].is_null() {
                stats = ["active", "accepted", "denied"]
                    .iter()
                    .map(|k| g["stats"][k].as_u64().unwrap_or(0).to_string())
                    .collect();
            }
            let mut row = vec![
                s(g, "id").into(),
                format!("{}/{}", s(g, "protocol"), g["tunnelPort"]),
                s(g, "dest").into(),
                n.to_string(),
                state,
            ];
            row.extend(stats);
            table.push(row);
        }
        print_table(table);
        Ok(())
    }
}

fn print_update(checked: &update::CheckResult) {
    if checked.available {
        let label = match checked.release.severity {
            update::Severity::Normal => "Update available",
            update::Severity::Security => "Security update available",
        };
        println!("{label}: GameLink {}", checked.release.version);
        println!("Release notes: {}", checked.release.release_notes_url);
    } else {
        println!(
            "No newer release in the verified manifest for GameLink {}",
            checked.installed_version
        );
    }
}
fn read_runtime(dir: &std::path::Path) -> Value {
    storage::read_bounded(&dir.join("runtime.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}
fn runtime_fresh(runtime: &Value) -> bool {
    chrono::DateTime::parse_from_rfc3339(s(runtime, "updated"))
        .is_ok_and(|t| chrono::Utc::now().signed_duration_since(t).num_seconds() < 20)
}
fn game_json(game: &Value) -> Value {
    json!({"id":game["id"],"name":game["name"],"type":game["type"],"protocol":game["protocol"],
        "tunnelPort":game["tunnelPort"],"dest":game["dest"],"disabled":game["disabled"] == true,"approvedDevices":rows(&game["devices"]).len()})
}
fn games_json(cfg: &Value) -> Value {
    json!({"games":rows(&cfg["games"]).iter().map(game_json).collect::<Vec<_>>()})
}
fn devices_json(cfg: &Value, filter: &str) -> Value {
    let devices: Vec<_> = rows(&cfg["games"]).iter().filter(|g| filter.is_empty() || s(g,"id") == filter).flat_map(|g| {
        rows(&g["devices"]).iter().map(move |d| json!({"game":g["id"],"publicKey":d["publicKey"],"label":d["label"],"approvedAt":d["approvedAt"]}))
    }).collect();
    json!({"devices":devices})
}
fn status_json(cfg: &Value, runtime: &Value) -> Value {
    let running = runtime_fresh(runtime);
    let games: Vec<_> = rows(&cfg["games"]).iter().map(|g| {
        let mut game = game_json(g);
        let st = rows(&runtime["games"]).iter().find(|st| running && st["id"] == g["id"]);
        game["state"] = json!(st.map(|st| s(st,"state")).unwrap_or(if rows(&g["devices"]).is_empty() {"inactive"}else{"not running"}));
        game["detail"] = json!(st.map(|st| s(st,"detail")).unwrap_or(if rows(&g["devices"]).is_empty() {"no approved devices"}else{""}));
        game["stats"] = st.map(|st| json!({"active":st["stats"]["active"].as_u64().unwrap_or(0),"accepted":st["stats"]["accepted"].as_u64().unwrap_or(0),"denied":st["stats"]["denied"].as_u64().unwrap_or(0)})).unwrap_or(Value::Null);
        game
    }).collect();
    json!({"agent":{"running":running,"pid":if running {runtime["pid"].clone()}else{Value::Null},"startedAt":if running {runtime["started"].clone()}else{Value::Null}},"games":games})
}
fn log(message: &str) {
    eprintln!(
        "{} {message}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
}
fn print_table(table: Vec<Vec<String>>) {
    let cols = table[0].len();
    let widths: Vec<_> = (0..cols - 1)
        .map(|i| {
            table
                .iter()
                .map(|r| r[i].chars().count() + 2)
                .max()
                .unwrap_or(0)
                .max(4)
        })
        .collect();
    for row in table {
        for (i, cell) in row.iter().enumerate() {
            print!("{cell}");
            if i < cols - 1 {
                print!("{}", " ".repeat(widths[i] - cell.chars().count()));
            }
        }
        println!();
    }
}
fn main() {
    std::process::exit(run());
}
fn run() -> i32 {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = std::env::var("GAMELINK_AGENT_DIR").unwrap_or_default();
    while args
        .first()
        .is_some_and(|a| a == "--dir" || a == "-dir" || a.starts_with("--dir="))
    {
        if let Some(v) = args[0].strip_prefix("--dir=") {
            dir = v.into();
            args.remove(0);
        } else if args.len() > 1 {
            dir = args.remove(1);
            args.remove(0);
        } else {
            eprintln!("error: --dir needs a value");
            return 2;
        }
    }
    if args.is_empty() || ["-h", "--help", "help"].contains(&args[0].as_str()) {
        print!("{}", include_str!("usage.txt"));
        return 0;
    }
    let mut cmd = args.remove(0);
    if cmd == "version" || cmd == "--version" {
        println!("gamelink-agent {VERSION}");
        return 0;
    }
    if cmd == "update" {
        if !args
            .first()
            .is_some_and(|sub| ["check", "stage", "install-staged"].contains(&sub.as_str()))
        {
            eprintln!("usage: gamelink-agent update check|stage|install-staged [options]");
            return 2;
        }
        cmd = format!("update {}", args.remove(0));
    }
    if dir.is_empty() && !matches!(cmd.as_str(), "update stage" | "update install-staged") {
        dir = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOME").ok().map(|s| format!("{s}/.config")))
            .map(|s| format!("{s}/gamelink-agent"))
            .unwrap_or_default();
        if dir.is_empty() {
            eprintln!("error: cannot determine a configuration directory; set GAMELINK_AGENT_DIR or pass --dir");
            return 2;
        }
    }
    if cmd == "host" {
        if !args
            .first()
            .is_some_and(|sub| ["refresh", "detect", "check"].contains(&sub.as_str()))
        {
            eprintln!("usage: gamelink-agent host detect|check|refresh [options]");
            return 2;
        }
        cmd = format!("host {}", args.remove(0));
    }
    if cmd == "invite" && args.first().is_some_and(|arg| arg == "rotate") {
        args.remove(0);
        cmd = "invite rotate".into();
    }
    if matches!(cmd.as_str(), "games" | "devices" | "requests") {
        let subcommands = match cmd.as_str() {
            "games" => ["add", "list", "remove"],
            "devices" => ["approve", "revoke", "list"],
            _ => ["list", "approve", "dismiss"],
        };
        if args.is_empty() {
            eprintln!("usage: gamelink-agent {cmd} {}", subcommands.join("|"));
            return 2;
        }
        let sub = args.remove(0);
        if !subcommands.contains(&sub.as_str()) {
            eprintln!("unknown {cmd} command {sub:?}");
            return 2;
        }
        cmd += &format!(" {sub}");
    }
    if ![
        "init",
        "games add",
        "games list",
        "games remove",
        "devices approve",
        "devices revoke",
        "devices list",
        "requests list",
        "requests approve",
        "requests dismiss",
        "invite",
        "invite rotate",
        "serve",
        "status",
        "update check",
        "update stage",
        "update install-staged",
        "host refresh",
        "host detect",
        "host check",
    ]
    .contains(&cmd.as_str())
    {
        eprint!("unknown command {cmd:?}\n\n{}", include_str!("usage.txt"));
        return 2;
    }
    let m = match parse(&cmd, args) {
        Ok(m) => m,
        Err(code) => return code,
    };
    let _cli_lock = if matches!(
        cmd.as_str(),
        "serve" | "update stage" | "update install-staged"
    ) {
        None
    } else {
        match self_update::cli_lock() {
            Ok(lock) => lock,
            Err(error) => {
                eprintln!("error: {error}");
                return 1;
            }
        }
    };
    let _host = if cmd.starts_with("update ") {
        None
    } else {
        match bridge::Host::init() {
            Ok(host) => Some(host),
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    };
    match (Cli {
        dir: PathBuf::from(dir),
    })
    .execute(&cmd, &m)
    {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
fn parse(cmd: &str, args: Vec<String>) -> std::result::Result<clap::ArgMatches, i32> {
    let specs = specs(cmd);
    let mut parser = Command::new("gamelink-agent")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .args_override_self(true);
    for (name, _, boolean) in &specs {
        let arg = Arg::new(*name).long(*name);
        parser = parser.arg(if *boolean {
            arg.action(ArgAction::SetTrue)
        } else {
            arg.action(ArgAction::Set).allow_hyphen_values(true)
        });
    }
    parser = parser.arg(
        Arg::new("positional")
            .num_args(0..)
            .action(ArgAction::Append),
    );
    let mut normalized = vec!["gamelink-agent".to_owned()];
    let mut i = 0;
    let mut positional = false;
    let options_after_positionals = cmd.starts_with("requests ");
    while i < args.len() {
        let a = &args[i];
        if positional || a == "-" || !a.starts_with('-') {
            if !positional && !options_after_positionals {
                normalized.push("--".into());
                positional = true;
            }
            normalized.push(a.clone());
            i += 1;
            continue;
        }
        if a == "--" {
            normalized.push(a.clone());
            positional = true;
            i += 1;
            continue;
        }
        let token = a.trim_start_matches('-');
        if ["h", "help"].contains(&token) {
            eprint!("{}", help(cmd, &specs));
            return Err(2);
        }
        let (name, val) = token
            .split_once('=')
            .map_or((token, None), |(n, v)| (n, Some(v)));
        let Some((_, _, boolean)) = specs.iter().find(|(n, _, _)| *n == name) else {
            eprintln!(
                "flag provided but not defined: -{name}\n{}",
                help(cmd, &specs).trim_end()
            );
            return Err(2);
        };
        if *boolean {
            let value = match val.unwrap_or("true") {
                "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                _ => {
                    eprintln!(
                        "invalid boolean value {:?} for -{name}: parse error\n{}",
                        val.unwrap(),
                        help(cmd, &specs).trim_end()
                    );
                    return Err(2);
                }
            };
            normalized.retain(|s| s != &format!("--{name}"));
            if value {
                normalized.push(format!("--{name}"));
            }
        } else {
            let value = if let Some(v) = val {
                v.to_owned()
            } else if i + 1 < args.len() {
                i += 1;
                args[i].clone()
            } else {
                eprintln!(
                    "flag needs an argument: -{name}\n{}",
                    help(cmd, &specs).trim_end()
                );
                return Err(2);
            };
            if ["port", "relay-region"].contains(&name) && value.parse::<i64>().is_err() {
                eprintln!(
                    "invalid value {value:?} for flag -{name}: parse error\n{}",
                    help(cmd, &specs).trim_end()
                );
                return Err(2);
            }
            normalized.push(format!("--{name}={value}"));
        }
        i += 1;
    }
    let m = parser.try_get_matches_from(normalized).map_err(|e| {
        eprintln!("{e}");
        2
    })?;
    let count = m.get_many::<String>("positional").map_or(0, |v| v.len());
    if let Some(operand) = match cmd {
        "games remove" if count != 1 => Some("<id>"),
        "requests approve" | "requests dismiss" if count != 1 => Some("<id|key> [options]"),
        "requests list" | "invite" | "invite rotate" if count != 0 => Some("[options]"),
        "update stage" | "update install-staged" if count != 0 => Some("[options]"),
        _ => None,
    } {
        eprintln!("usage: gamelink-agent {cmd} {operand}");
        return Err(2);
    }
    Ok(m)
}
fn specs(cmd: &str) -> Vec<(&'static str, &'static str, bool)> {
    match cmd {
 "games add"=>vec![
 ("type","game type: minecraft-java, minecraft-bedrock, factorio or custom (required)",false),("id","short identifier (default: derived from the name)",false),("name","display name shown to friends (default: the game type's name)",false),("description","optional short description",false),("proto","tcp or udp (required for custom games)",false),("port","port inside the tunnel (default: the game's standard port)",false),("dest","where to forward to, as ip:port (default: 127.0.0.1:<port>); use this for Docker or Pterodactyl",false),("console-bridge","enable Bedrock LAN console bridge",true),("bedrock-ip","Bedrock server LAN IPv4 address",false),("bridge-bind-ip","agent LAN IPv4 address",false),("media-ports","allowed Bedrock media port range, e.g. 19132-19136",false),("relay-region","use this region of the public Tailcat relay map (default: nearest)",false),("relay-host","use your own DERP relay at this host name instead of the public relays",false)],
 "devices approve"=>vec![("game","game id (required)",false),("key","the friend's public key, nodekey:... (required)",false),("label","a name to remember this device by",false)],
 "devices revoke"=>vec![("game","game id (required)",false),("key","public key to revoke, nodekey:... (required)",false)],
 "games list" | "status" | "update check"=>vec![("json","print machine-readable JSON",true)],
 "update stage"=>vec![("version","require this exact latest signed release version",false)],
 "devices list"=>vec![("game","only this game",false),("json","print machine-readable JSON",true)],
 "requests list"=>vec![("game","only this game",false),("json","print machine-readable JSON",true)],
 "requests approve"=>vec![("game","only this game",false),("label","a name to remember this device by",false)],
 "requests dismiss"=>vec![("game","only this game",false)],
 "invite" | "invite rotate"=>vec![("game","game id (required)",false),("name","name shown in the friend's GameLink (default: the game's name)",false),("note","optional short note shown on import",false),("allow-local-relay","allow a relay on a private IP (for LAN-only test setups)",true),("expires","rotate to a new game identity that stops serving after this duration, e.g. 24h",false)],
 "serve"=>vec![("verbose","include Tailcat's own (redacted) network log output",true),("host-config","enable loopback host administration using this private settings file",false),("host-listen","loopback host administration address (default 127.0.0.1:8467)",false)],
 "host refresh"=>vec![("config","private Pterodactyl host settings file (required)",false),("json","print machine-readable JSON",true)],
 "host detect"=>vec![("panel-url","Panel URL (required)",false),("token-file","private Application token file (required)",false),("node-id","select a node ID",false),("out","create private host settings (must not exist)",false),("json","print machine-readable JSON",true)],
 "host check"=>vec![("config","private Pterodactyl host settings file (required)",false),("listen","literal loopback admin address (default 127.0.0.1:8467)",false),("json","print machine-readable JSON",true)], _=>vec![] }
}
fn help(cmd: &str, specs: &[(&str, &str, bool)]) -> String {
    let mut out = format!("Usage of {cmd}:\n");
    let mut sorted = specs.to_vec();
    sorted.sort_by_key(|s| s.0);
    for (name, description, boolean) in sorted {
        let ty = if boolean {
            ""
        } else if ["port", "relay-region"].contains(&name) {
            " int"
        } else {
            " string"
        };
        out += &format!("  -{name}{ty}\n    \t{description}\n");
    }
    out
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn json_shapes_project_only_public_fields_and_keep_device_filter() {
        let public = format!("nodekey:{}", "a".repeat(64));
        let cfg = json!({"games":[{"id":"game","name":"Game","type":"minecraft-java","protocol":"tcp","tunnelPort":25565,"dest":"127.0.0.1:25565",
            "devices":[{"publicKey":public,"label":"Player","approvedAt":"2026-11-15T00:00:00Z","privateKey":"must not be projected"}],"invitation":"must not be projected","privateKey":"must not be projected"}]});
        let expected = json!({"id":"game","name":"Game","type":"minecraft-java","protocol":"tcp","tunnelPort":25565,"dest":"127.0.0.1:25565","disabled":false,"approvedDevices":1});
        assert_eq!(games_json(&cfg), json!({"games":[expected]}));
        assert_eq!(
            devices_json(&cfg, ""),
            json!({"devices":[{"game":"game","publicKey":public,"label":"Player","approvedAt":"2026-11-15T00:00:00Z"}]})
        );
        assert_eq!(devices_json(&cfg, "other"), json!({"devices":[]}));
        let result = status_json(&cfg, &Value::Null);
        let mut game = expected.clone();
        game["state"] = json!("not running");
        game["detail"] = json!("");
        game["stats"] = Value::Null;
        assert_eq!(
            result,
            json!({"agent":{"running":false,"pid":null,"startedAt":null},"games":[game]})
        );
        let updated = now();
        let runtime = json!({"updated":updated,"started":updated,"pid":123,"games":[{"id":"game","state":"serving","detail":"ready","stats":{"active":2,"accepted":3,"denied":4,"secret":"must not be projected"}}]});
        let result = status_json(&cfg, &runtime);
        assert_eq!(
            result["agent"],
            json!({"running":true,"pid":123,"startedAt":updated})
        );
        assert_eq!(
            result["games"][0]["stats"],
            json!({"active":2,"accepted":3,"denied":4})
        );
        assert_eq!(result["games"][0]["state"], "serving");
        assert_eq!(result["games"][0]["detail"], "ready");
        assert!(!result.to_string().contains("must not be projected"));
        let mut stale = runtime;
        stale["updated"] = json!("2000-01-01T00:00:00Z");
        assert_eq!(status_json(&cfg, &stale)["agent"]["running"], false);
        let mut inactive = cfg;
        inactive["games"][0]["devices"] = json!([]);
        inactive["games"][0]["disabled"] = json!(true);
        let result = status_json(&inactive, &Value::Null);
        assert_eq!(result["games"][0]["disabled"], true);
        assert_eq!(result["games"][0]["state"], "inactive");
        assert_eq!(result["games"][0]["detail"], "no approved devices");
    }

    #[test]
    fn json_flags_and_host_setup_specs_are_accepted() {
        for cmd in [
            "status",
            "games list",
            "devices list",
            "host refresh",
            "host detect",
            "host check",
        ] {
            assert!(parse(cmd, vec!["--json".into()]).unwrap().get_flag("json"));
            assert!(!parse(cmd, vec![]).unwrap().get_flag("json"));
            assert!(help(cmd, &specs(cmd)).contains("-json"));
        }
        let matches = parse(
            "host detect",
            [
                "--panel-url",
                "https://panel.example",
                "--token-file",
                "panel.token",
                "--node-id",
                "7",
                "--out",
                "host.json",
                "--json",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        )
        .unwrap();
        assert_eq!(matches.get_one::<String>("node-id").unwrap(), "7");
        let matches = parse(
            "host check",
            [
                "--config",
                "host.json",
                "--listen",
                "127.0.0.1:8467",
                "--json",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        )
        .unwrap();
        assert_eq!(
            matches.get_one::<String>("listen").unwrap(),
            "127.0.0.1:8467"
        );
    }
}
