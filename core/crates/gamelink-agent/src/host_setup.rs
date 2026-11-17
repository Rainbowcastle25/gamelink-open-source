//! Read-only setup discovery and diagnostics. No listener or Panel mutations.
use crate::{host_admin, pterodactyl, rows, s, Result};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    path::Path,
    time::Duration,
};

pub(crate) fn classify(name: Option<&str>, startup: &str) -> (Value, &'static str, &'static str) {
    let name = name.unwrap_or_default().to_ascii_lowercase();
    let startup = startup.to_ascii_lowercase();
    let named = |words: &[&str]| words.iter().any(|word| name.contains(word));
    let profile =
        |typ: &str, proto: &str, port: u16| json!({"type":typ,"protocol":proto,"tunnelPort":port});
    if named(&["broadcast", "discord", "bot"]) {
        return (Value::Null, "name", "not a joinable game server");
    }
    for (word, command, typ, port) in [
        ("bedrock", "bedrock_server", "minecraft-bedrock", 19132),
        ("factorio", "factorio", "factorio", 34197),
    ] {
        if name.contains(word) || startup.contains(command) {
            return (
                profile(typ, "udp", port),
                if name.contains(word) {
                    "name"
                } else {
                    "startup"
                },
                "recognized game",
            );
        }
    }
    if named(&[
        "minecraft",
        "paper",
        "spigot",
        "bukkit",
        "forge",
        "neoforge",
        "fabric",
        "sponge",
        "purpur",
        "quilt",
        "velocity",
        "bungeecord",
        "waterfall",
    ]) {
        return (
            profile("minecraft-java", "tcp", 25565),
            "name",
            "recognized Java game egg",
        );
    }
    if startup.contains("java") && startup.contains("-jar") {
        return (
            profile("minecraft-java", "tcp", 25565),
            "guess",
            "guess based on Java jar startup; verify the game",
        );
    }
    (
        Value::Null,
        "none",
        "unknown game; map it as custom tcp/udp with its port, or leave it out",
    )
}

#[cfg(unix)]
pub(crate) fn local_ips() -> Result<BTreeSet<IpAddr>> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let mut first = std::ptr::null_mut();
    // SAFETY: getifaddrs initializes the linked list on success. Each address
    // is cast only after checking its family, and the list is freed once.
    unsafe {
        if libc::getifaddrs(&mut first) != 0 {
            return Err("could not inspect local interfaces".into());
        }
        let mut ips = BTreeSet::new();
        let mut current = first;
        while !current.is_null() {
            let address = (*current).ifa_addr;
            if !address.is_null() {
                match i32::from((*address).sa_family) {
                    libc::AF_INET => {
                        let addr = &*address.cast::<libc::sockaddr_in>();
                        ips.insert(IpAddr::V4(Ipv4Addr::from(
                            addr.sin_addr.s_addr.to_ne_bytes(),
                        )));
                    }
                    libc::AF_INET6 => {
                        let addr = &*address.cast::<libc::sockaddr_in6>();
                        ips.insert(IpAddr::V6(Ipv6Addr::from(addr.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            current = (*current).ifa_next;
        }
        libc::freeifaddrs(first);
        Ok(ips)
    }
}
#[cfg(not(unix))]
pub(crate) fn local_ips() -> Result<BTreeSet<IpAddr>> {
    Err("local interface discovery requires Unix".into())
}

fn token(path: &Path) -> Result<String> {
    let token = String::from_utf8(host_admin::private_file(path, 8192)?)
        .map_err(|_| "invalid host credential file")?;
    let token = token.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err("invalid host credential file".into());
    }
    Ok(token.to_owned())
}

pub(crate) fn detect(
    panel_url: &str,
    token_file: &Path,
    node: Option<u64>,
    out: Option<&Path>,
    local: &BTreeSet<IpAddr>,
    request: &dyn Fn(Value) -> Result<Value>,
) -> Result<Value> {
    let settings = json!({"panelUrl":panel_url,"applicationToken":token(token_file)?,"nodeId":node.unwrap_or(1),"eggProfiles":{}});
    let servers = pterodactyl::application_inventory(&settings, request, false)?;
    let mut result = discovery(&servers, node, local)?;
    if let Some(path) = out {
        if let Some(node_id) = result["selectedNode"].as_u64() {
            write_settings(path, token_file, &settings, node_id, &result)?;
            result["written"] = json!(path);
        }
    }
    Ok(result)
}

fn discovery(servers: &[Value], selected: Option<u64>, local: &BTreeSet<IpAddr>) -> Result<Value> {
    let mut groups: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
    for server in servers {
        let node = server["node"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("invalid Panel node ID")?;
        groups.entry(node).or_default().push(server);
    }
    let mut nodes = Vec::new();
    for (id, servers) in &groups {
        let mut addresses = BTreeMap::<IpAddr, usize>::new();
        for server in servers {
            let primary = rows(&server["allocations"])
                .iter()
                .find(|a| a["attributes"]["id"] == server["allocation"]);
            if let Some(ip) = primary
                .and_then(|a| s(&a["attributes"], "ip").parse::<IpAddr>().ok())
                .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
            {
                *addresses.entry(ip).or_default() += 1;
            }
        }
        let address = addresses
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(ip, _)| *ip);
        nodes.push(json!({"nodeId":id,"serverCount":servers.len(),"nodeAddress":address.map(|ip|ip.to_string()),"local":address.is_some_and(|ip|local.contains(&ip))}));
    }
    let locals: Vec<_> = nodes.iter().filter(|node| node["local"] == true).collect();
    let selected = match selected {
        Some(id) => Some(id),
        None if locals.len() == 1 => locals[0]["nodeId"].as_u64(),
        None if nodes.len() == 1 => nodes[0]["nodeId"].as_u64(),
        None => None,
    };
    let mut eggs: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
    if let Some(servers) = selected.and_then(|id| groups.get(&id)) {
        for server in servers {
            let id = server["egg"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or("invalid Panel egg ID")?;
            eggs.entry(id).or_default().push(server);
        }
    }
    let eggs: Vec<_> = eggs.into_iter().map(|(id, servers)| {
        let first = servers[0];
        let name = servers.iter().find_map(|s| s["eggName"].as_str().filter(|n| !n.is_empty()));
        let (proposal, confidence, reason) = classify(name, s(first, "startup"));
        let names: Vec<_> = servers.iter().map(|server| server["name"].clone()).collect();
        let multiple: Vec<_> = servers.iter().filter(|server| rows(&server["allocations"]).len() > 1).map(|server| server["name"].clone()).collect();
        json!({"eggId":id,"eggName":name,"serverNames":names,"serverCount":servers.len(),"multiAllocationServers":multiple,
            "warning":if multiple.is_empty(){Value::Null}else{json!("only the primary port is forwarded")},
            "proposal":proposal,"confidence":confidence,"reason":reason})
    }).collect();
    Ok(json!({"nodes":nodes,"selectedNode":selected,"eggs":eggs,"written":null}))
}

pub(crate) fn detect_exit(value: &Value) -> i32 {
    if value["selectedNode"].is_null() {
        3
    } else {
        0
    }
}

fn create_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "could not create private host file; it may already exist")?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "could not write private host file".into())
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| "could not generate host admin token")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_settings(
    path: &Path,
    token_file: &Path,
    settings: &Value,
    node_id: u64,
    result: &Value,
) -> Result<()> {
    if path.symlink_metadata().is_ok() {
        return Err("host settings already exist; refusing to overwrite".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent =
        fs::canonicalize(parent).map_err(|_| "could not resolve host settings directory")?;
    let application =
        fs::canonicalize(token_file).map_err(|_| "could not resolve credential file")?;
    let application = if application.parent() == Some(parent.as_path()) {
        application
            .file_name()
            .map(std::path::PathBuf::from)
            .ok_or("invalid credential path")?
    } else {
        application
    };
    let profiles: Map<_, _> = rows(&result["eggs"])
        .iter()
        .filter(|egg| {
            !egg["proposal"].is_null() && matches!(s(egg, "confidence"), "name" | "startup")
        })
        .map(|egg| (egg["eggId"].to_string(), egg["proposal"].clone()))
        .collect();
    let mut cfg = json!({"panelUrl":settings["panelUrl"],"applicationTokenFile":application,"adminTokenFile":"host-admin.token","nodeId":node_id,"eggProfiles":profiles});
    if let Some(node) = rows(&result["nodes"])
        .iter()
        .find(|node| node["nodeId"] == node_id)
    {
        if !node["nodeAddress"].is_null() {
            cfg["nodeAddress"] = node["nodeAddress"].clone();
        }
    }
    let mut validated = cfg.clone();
    validated["applicationToken"] = settings["applicationToken"].clone();
    pterodactyl::validate_settings(&validated)?;
    let encoded = serde_json::to_vec_pretty(&cfg).map_err(|_| "could not encode host settings")?;
    if encoded.len() > 65536 {
        return Err("host settings exceed the private configuration size limit".into());
    }
    let admin_path = parent.join("host-admin.token");
    if admin_path.symlink_metadata().is_err() {
        // create_new also protects against concurrent creators and symlinks.
        create_private(&admin_path, random_token()?.as_bytes())?;
    }
    let admin = token(&admin_path)?;
    if admin.len() < 32 || admin == s(settings, "applicationToken") {
        return Err("invalid or reused host admin token".into());
    }
    create_private(path, &encoded)
}

fn loopback_address(listen: &str) -> Result<SocketAddr> {
    listen
        .parse::<SocketAddr>()
        .ok()
        .filter(|a| a.ip().is_loopback())
        .ok_or_else(|| "host check requires a literal loopback address".into())
}

fn http_status(address: SocketAddr, token: Option<&str>) -> std::io::Result<u16> {
    let timeout = Duration::from_secs(3);
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "GET /v1/host/status HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n"
    )?;
    if let Some(token) = token {
        write!(stream, "Authorization: Bearer {token}\r\n")?;
    }
    write!(stream, "\r\n")?;
    let mut line = String::new();
    BufReader::new(stream).take(1024).read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    if !line.ends_with("\r\n") || !matches!(parts.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(std::io::Error::other("invalid HTTP response"));
    }
    parts
        .next()
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| std::io::Error::other("invalid HTTP response"))
}

fn admin_check(address: SocketAddr, token: &str) -> (&'static str, &'static str) {
    match http_status(address, None) {
        Ok(401) => match http_status(address, Some(token)) {
            Ok(200) => ("pass", "authenticated loopback API available"),
            _ => ("fail", "authenticated admin API did not return 200"),
        },
        Ok(_) => ("fail", "unauthenticated admin API did not return 401"),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            ("warn", "start serve with --host-config")
        }
        Err(_) => ("fail", "could not verify admin API authentication"),
    }
}

pub(crate) fn check(
    dir: &Path,
    path: &Path,
    listen: &str,
    request: &dyn Fn(Value) -> Result<Value>,
) -> Value {
    let mut checks = Vec::new();
    let mut add = |id: &str, status: &str, detail: String| {
        checks.push(json!({"id":id,"status":status,"detail":detail}))
    };
    let loaded = host_admin::load(path).and_then(|(settings, admin)| {
        pterodactyl::validate_settings(&settings)?;
        Ok((settings, admin))
    });
    add(
        "config",
        if loaded.is_ok() { "pass" } else { "fail" },
        if loaded.is_ok() {
            "private host configuration loaded"
        } else {
            "could not load host configuration; check settings and 0600 credential files"
        }
        .into(),
    );
    let cfg = crate::config::dispatch("config.agent_load", json!({"path":dir.join("agent.json")}));
    if let Ok(_cfg) = &cfg {
        #[cfg(unix)]
        let accessible = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(dir)
                .map(|m| m.permissions().mode() & 0o077 != 0)
                .unwrap_or(true)
        };
        #[cfg(not(unix))]
        let accessible = false;
        add(
            "agent_dir",
            if accessible { "warn" } else { "pass" },
            if accessible {
                "config directory is group/other-accessible"
            } else {
                "agent is initialized"
            }
            .into(),
        );
    } else {
        add("agent_dir", "fail", "run gamelink-agent init".into());
    }
    let runtime = crate::read_runtime(dir);
    let running = crate::runtime_fresh(&runtime);
    if let Ok((settings, admin)) = loaded {
        let cached = std::cell::RefCell::new(None);
        let first = pterodactyl::application_inventory(
            &settings,
            &|input| {
                let value = request(input)?;
                *cached.borrow_mut() = Some(value.clone());
                Ok(value)
            },
            true,
        );
        add(
            "panel",
            if first.is_ok() { "pass" } else { "fail" },
            if first.is_ok() {
                "Panel inventory available"
            } else {
                "Panel inventory request failed"
            }
            .into(),
        );
        // Fetch the remainder with the same pager, reusing the first response
        // through the shim wrapper so page one is never requested twice.
        let inventory = first.and_then(|_| {
            pterodactyl::application_inventory(
                &settings,
                &|input| {
                    if let Some(value) = cached.borrow_mut().take() {
                        Ok(value)
                    } else {
                        request(input)
                    }
                },
                false,
            )
        });
        match inventory {
            Ok(servers) => {
                let servers: Vec<_> = servers
                    .iter()
                    .filter(|server| server["node"] == settings["nodeId"])
                    .collect();
                let unmapped: BTreeSet<_> = servers
                    .iter()
                    .filter_map(|server| server["egg"].as_u64())
                    .filter(|egg| settings["eggProfiles"][egg.to_string()].is_null())
                    .collect();
                if servers.is_empty() {
                    add(
                        "node",
                        "fail",
                        format!("no servers on node {}; check nodeId", settings["nodeId"]),
                    );
                } else if !unmapped.is_empty() {
                    add(
                        "node",
                        "warn",
                        format!(
                            "unmapped egg IDs: {}",
                            unmapped
                                .iter()
                                .map(u64::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                } else {
                    add("node", "pass", "node has mapped servers".into());
                }
                if let Ok(cfg) = &cfg {
                    let missing = servers
                        .iter()
                        .filter(|server| {
                            server["suspended"] != true
                                && !settings["eggProfiles"][server["egg"].to_string()].is_null()
                        })
                        .filter(|server| {
                            let id = pterodactyl::stable_id(s(server, "uuid"));
                            !rows(&cfg["games"]).iter().any(|game| {
                                game["id"] == id
                                    && !s(game, "origin").is_empty()
                                    && game["disabled"] != true
                            })
                        })
                        .count();
                    add(
                        "registrations",
                        if missing == 0 { "pass" } else { "warn" },
                        if missing == 0 {
                            "mapped servers are registered".into()
                        } else {
                            format!("{missing} not registered yet; run host refresh")
                        },
                    );
                } else {
                    add("registrations", "fail", "skipped".into());
                }
            }
            Err(_) => {
                add("node", "fail", "skipped".into());
                add("registrations", "fail", "skipped".into());
            }
        }
        add(
            "client_token",
            if s(&settings, "clientToken").is_empty() {
                "warn"
            } else {
                "pass"
            },
            if s(&settings, "clientToken").is_empty() {
                "power status will be unknown"
            } else {
                "Client API token configured"
            }
            .into(),
        );
        match loopback_address(listen) {
            Err(_) => add(
                "admin_api",
                "fail",
                "host check requires a literal loopback address".into(),
            ),
            Ok(address) if running => {
                let (status, detail) = admin_check(address, &admin);
                add("admin_api", status, detail.into());
            }
            Ok(_) => {}
        }
    } else {
        for id in ["panel", "node", "registrations", "client_token"] {
            add(id, "fail", "skipped".into());
        }
        if loopback_address(listen).is_err() {
            add(
                "admin_api",
                "fail",
                "host check requires a literal loopback address".into(),
            );
        } else if running {
            add("admin_api", "fail", "skipped".into());
        }
    }
    add(
        "agent_running",
        if running { "pass" } else { "warn" },
        if running {
            "agent is running"
        } else {
            "agent is not running"
        }
        .into(),
    );
    json!({"ok":!checks.iter().any(|check|check["status"] == "fail"),"checks":checks})
}

pub(crate) fn print_detect(result: &Value) {
    for node in rows(&result["nodes"]) {
        println!(
            "Node {}: {} server(s), address {}, local {}",
            node["nodeId"], node["serverCount"], node["nodeAddress"], node["local"]
        );
    }
    if result["selectedNode"].is_null() {
        println!("Select a node with --node-id.");
    } else {
        println!("Selected node {}", result["selectedNode"]);
    }
    for egg in rows(&result["eggs"]) {
        println!(
            "Egg {} ({}), {} server(s): {} [{}] {}",
            egg["eggId"],
            egg["eggName"].as_str().unwrap_or("name unavailable"),
            egg["serverCount"],
            egg["proposal"],
            s(egg, "confidence"),
            s(egg, "reason")
        );
        if !rows(&egg["multiAllocationServers"]).is_empty() {
            println!(
                "  Warning: only the primary port is forwarded for {}",
                egg["multiAllocationServers"]
            );
        }
    }
    if !result["written"].is_null() {
        println!("Wrote {}", result["written"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, net::TcpListener, thread};
    use tempfile::{tempdir, TempDir};

    fn server(node: u64, egg: u64, name: Option<&str>, startup: &str, ip: &str) -> Value {
        json!({"attributes":{"uuid":format!("server-{node}-{egg}"),"name":format!("Game {egg}"),"node":node,"egg":egg,"suspended":false,"allocation":1,
            "container":{"startup_command":startup},"relationships":{"egg":{"attributes":name.map(|name|json!({"name":name}))},
            "allocations":{"data":[{"attributes":{"id":1,"ip":ip,"port":25565}},{"attributes":{"id":2,"ip":ip,"port":25566}}]}}}})
    }
    fn page(data: Vec<Value>, current: u64, total: u64) -> Value {
        json!({"data":data,"meta":{"pagination":{"current_page":current,"total_pages":total}}})
    }
    fn fixture() -> (TempDir, std::path::PathBuf, String) {
        let dir = tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let secret = random_token().unwrap();
        let token_file = dir.path().join("panel.token");
        create_private(&token_file, secret.as_bytes()).unwrap();
        let path = dir.path().join("host.json");
        detect(
            "https://panel.example",
            &token_file,
            None,
            Some(&path),
            &BTreeSet::new(),
            &|_| {
                Ok(page(
                    vec![server(7, 10, Some("Paper"), "", "192.0.2.1")],
                    1,
                    1,
                ))
            },
        )
        .unwrap();
        crate::config::dispatch(
            "config.agent_mutate",
            json!({"path":dir.path().join("agent.json"),"action":"init"}),
        )
        .unwrap();
        (dir, path, secret)
    }
    fn status<'a>(result: &'a Value, id: &str) -> &'a str {
        s(
            rows(&result["checks"])
                .iter()
                .find(|check| check["id"] == id)
                .unwrap(),
            "status",
        )
    }
    fn panel(_: Value) -> Result<Value> {
        Ok(page(
            vec![server(7, 10, Some("Paper"), "", "192.0.2.1")],
            1,
            1,
        ))
    }

    #[test]
    fn classification_table_and_precedence() {
        for (name, startup, typ, confidence) in [
            (
                Some("Broadcast Minecraft"),
                "java -jar game.jar",
                None,
                "name",
            ),
            (Some("Discord"), "factorio", None, "name"),
            (Some("BOT"), "bedrock_server", None, "name"),
            (
                Some("Minecraft Bedrock"),
                "",
                Some("minecraft-bedrock"),
                "name",
            ),
            (
                None,
                "./BEDROCK_SERVER",
                Some("minecraft-bedrock"),
                "startup",
            ),
            (Some("FACTORIO"), "", Some("factorio"), "name"),
            (
                None,
                "./factorio --start-server",
                Some("factorio"),
                "startup",
            ),
            (
                None,
                "JAVA -Xmx1G -JAR game.jar",
                Some("minecraft-java"),
                "guess",
            ),
            (None, "arbitrary-server", None, "none"),
        ] {
            let (proposal, actual, reason) = classify(name, startup);
            assert_eq!(proposal["type"].as_str(), typ);
            assert_eq!(actual, confidence);
            if confidence == "guess" {
                assert!(reason.contains("guess"));
            }
            if let Some(typ) = typ {
                assert_eq!(
                    proposal["protocol"],
                    if typ == "minecraft-java" {
                        "tcp"
                    } else {
                        "udp"
                    }
                );
                assert_eq!(
                    proposal["tunnelPort"],
                    match typ {
                        "minecraft-java" => 25565,
                        "minecraft-bedrock" => 19132,
                        _ => 34197,
                    }
                );
            }
        }
        for name in [
            "minecraft",
            "paper",
            "spigot",
            "bukkit",
            "forge",
            "neoforge",
            "fabric",
            "sponge",
            "purpur",
            "quilt",
            "velocity",
            "bungeecord",
            "waterfall",
        ] {
            assert_eq!(
                classify(Some(&name.to_uppercase()), "").0["type"],
                "minecraft-java"
            );
        }
        assert_eq!(classify(Some("Paper"), "factorio").0["type"], "factorio");
    }

    #[test]
    fn detect_paginates_selects_local_or_explicit_node_and_redacts() {
        let dir = tempdir().unwrap();
        let token_file = dir.path().join("panel.token");
        let secret = random_token().unwrap();
        create_private(&token_file, secret.as_bytes()).unwrap();
        let startup = format!("java -jar {}.jar", random_token().unwrap());
        let environment = random_token().unwrap();
        let calls = Cell::new(0);
        let request = |input: Value| {
            assert_eq!(input["verb"], "GET");
            assert!(input["token"] == secret);
            assert!(s(&input, "url").contains("include=allocations,egg&per_page=100&page="));
            calls.set(calls.get() + 1);
            if s(&input, "url").ends_with("page=1") {
                let mut value = server(7, 10, None, &startup, "192.0.2.7");
                value["attributes"]["container"]["environment"] = json!({"PASSWORD":environment});
                Ok(page(vec![value], 1, 2))
            } else {
                Ok(page(
                    vec![server(8, 20, Some("Discord"), "", "192.0.2.8")],
                    2,
                    2,
                ))
            }
        };
        let result = detect(
            "https://panel.example",
            &token_file,
            None,
            None,
            &BTreeSet::new(),
            &request,
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(detect_exit(&result), 3);
        assert!(result["selectedNode"].is_null());
        assert_eq!(result["nodes"].as_array().unwrap().len(), 2);
        let local = BTreeSet::from(["192.0.2.7".parse().unwrap()]);
        let result = detect(
            "https://panel.example",
            &token_file,
            None,
            None,
            &local,
            &request,
        )
        .unwrap();
        assert_eq!(detect_exit(&result), 0);
        assert_eq!(result["selectedNode"], 7);
        assert_eq!(result["eggs"][0]["eggName"], Value::Null);
        assert_eq!(result["eggs"][0]["confidence"], "guess");
        assert_eq!(result["eggs"][0]["proposal"]["type"], "minecraft-java");
        assert_eq!(
            result["eggs"][0]["multiAllocationServers"],
            json!(["Game 10"])
        );
        assert!(!result.to_string().contains(&secret));
        assert!(!result.to_string().contains(&startup));
        assert!(!result.to_string().contains(&environment));
        let result = detect(
            "https://panel.example",
            &token_file,
            Some(8),
            None,
            &local,
            &request,
        )
        .unwrap();
        assert_eq!(result["selectedNode"], 8);
        assert!(result["eggs"][0]["proposal"].is_null());
        assert_eq!(
            detect(
                "https://panel.example",
                &token_file,
                Some(9),
                None,
                &local,
                &request
            )
            .unwrap()["selectedNode"],
            9
        );
        assert_eq!(
            detect_exit(
                &detect(
                    "https://panel.example",
                    &token_file,
                    None,
                    Some(&dir.path().join("host.json")),
                    &BTreeSet::new(),
                    &request
                )
                .unwrap()
            ),
            3
        );
        assert!(!dir.path().join("host-admin.token").exists());
    }

    #[test]
    fn output_loads_is_private_and_never_overwrites() {
        let (dir, path, secret) = fixture();
        let (loaded, admin) = host_admin::load(&path).unwrap();
        pterodactyl::validate_settings(&loaded).unwrap();
        assert!(loaded["applicationToken"] == secret);
        assert_eq!(loaded["applicationTokenFile"], "panel.token");
        assert_eq!(loaded["adminTokenFile"], "host-admin.token");
        assert_eq!(admin.len(), 64);
        assert!(admin.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let before = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&before).contains(&secret));
        assert!(!String::from_utf8_lossy(&before).contains(&admin));
        assert!(detect(
            "https://panel.example",
            &dir.path().join("panel.token"),
            None,
            Some(&path),
            &BTreeSet::new(),
            &panel
        )
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(token(&dir.path().join("host-admin.token")).unwrap() == admin);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for file in [&path, &dir.path().join("host-admin.token")] {
                assert_eq!(
                    fs::metadata(file).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            for file in [
                &path,
                &dir.path().join("panel.token"),
                &dir.path().join("host-admin.token"),
            ] {
                fs::set_permissions(file, fs::Permissions::from_mode(0o644)).unwrap();
                assert!(host_admin::load(&path).is_err());
                if file.ends_with("panel.token") {
                    assert!(detect(
                        "https://panel.example",
                        file,
                        None,
                        None,
                        &BTreeSet::new(),
                        &panel
                    )
                    .is_err());
                }
                fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }

    #[test]
    fn unknown_eggs_write_empty_loadable_mapping_and_keep_existing_admin() {
        let (dir, _, _) = fixture();
        let admin = token(&dir.path().join("host-admin.token")).unwrap();
        let path = dir.path().join("unknown.json");
        detect(
            "https://panel.example",
            &dir.path().join("panel.token"),
            None,
            Some(&path),
            &BTreeSet::new(),
            &|_| Ok(page(vec![server(7, 10, None, "unknown", "0.0.0.0")], 1, 1)),
        )
        .unwrap();
        let (cfg, same_admin) = host_admin::load(&path).unwrap();
        pterodactyl::validate_settings(&cfg).unwrap();
        assert_eq!(cfg["eggProfiles"], json!({}));
        assert!(cfg["nodeAddress"].is_null());
        assert!(same_admin == admin);
    }

    #[test]
    fn output_leaves_out_java_jar_guesses() {
        // A broadcaster bot started with `java -jar` must not be registered
        // as a joinable game when the key cannot read egg names.
        let (dir, _, _) = fixture();
        let path = dir.path().join("guess.json");
        let result = detect(
            "https://panel.example",
            &dir.path().join("panel.token"),
            None,
            Some(&path),
            &BTreeSet::new(),
            &|_| {
                Ok(page(
                    vec![
                        server(7, 16, None, "./bedrock_server", "192.0.2.7"),
                        server(7, 22, None, "java -jar Broadcaster.jar", "192.0.2.7"),
                    ],
                    1,
                    1,
                ))
            },
        )
        .unwrap();
        assert_eq!(result["eggs"][1]["confidence"], "guess");
        let (cfg, _) = host_admin::load(&path).unwrap();
        assert_eq!(cfg["eggProfiles"]["16"]["type"], "minecraft-bedrock");
        assert!(cfg["eggProfiles"]["22"].is_null());
    }

    #[test]
    fn node_address_uses_most_common_primary_and_multiple_local_nodes_need_a_decision() {
        let settings = json!({"panelUrl":"https://panel.example","applicationToken":random_token().unwrap(),"nodeId":1,"eggProfiles":{}});
        let servers = pterodactyl::application_inventory(
            &settings,
            &|_| {
                Ok(page(
                    vec![
                        server(7, 1, None, "", "192.0.2.1"),
                        server(7, 2, None, "", "192.0.2.2"),
                        server(7, 3, None, "", "192.0.2.1"),
                        server(7, 4, None, "", "0.0.0.0"),
                        server(8, 5, None, "", "::1"),
                    ],
                    1,
                    1,
                ))
            },
            false,
        )
        .unwrap();
        let locals = BTreeSet::from(["192.0.2.1".parse().unwrap(), "::1".parse().unwrap()]);
        let result = discovery(&servers, None, &locals).unwrap();
        assert_eq!(
            result["nodes"][0],
            json!({"nodeId":7,"serverCount":4,"nodeAddress":"192.0.2.1","local":true})
        );
        assert_eq!(detect_exit(&result), 3);
        assert!(result["selectedNode"].is_null());
        assert_eq!(
            discovery(&servers, Some(7), &locals).unwrap()["selectedNode"],
            7
        );
        let only = discovery(&servers[..4], None, &BTreeSet::new()).unwrap();
        assert_eq!(only["selectedNode"], 7);
        assert_eq!(only["eggs"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn check_runs_all_checks_pass_warn_fail_and_redacts_panel_errors() {
        let (dir, path, secret) = fixture();
        let calls = Cell::new(0);
        let request = |input: Value| {
            calls.set(calls.get() + 1);
            panel(input)
        };
        let result = check(dir.path(), &path, "127.0.0.1:8467", &request);
        assert_eq!(calls.get(), 1);
        assert_eq!(result["ok"], true);
        for id in ["config", "agent_dir", "panel", "node"] {
            assert_eq!(status(&result, id), "pass");
        }
        for id in ["registrations", "client_token", "agent_running"] {
            assert_eq!(status(&result, id), "warn");
        }
        assert!(!rows(&result["checks"])
            .iter()
            .any(|check| check["id"] == "admin_api"));
        let startup = format!("java -jar {}.jar", random_token().unwrap());
        let result = check(dir.path(), &path, "127.0.0.1:8467", &|_| {
            let mut server = server(7, 10, None, &startup, "192.0.2.1");
            server["attributes"]["container"]["environment"] = json!({"PASSWORD":secret});
            Ok(page(vec![server], 1, 1))
        });
        assert!(!result.to_string().contains(&startup));
        assert!(!result.to_string().contains(&secret));
        let result = check(dir.path(), &path, "127.0.0.1:8467", &|_| {
            Err(format!("{secret} {startup}"))
        });
        assert_eq!(result["ok"], false);
        assert_eq!(status(&result, "panel"), "fail");
        assert_eq!(status(&result, "agent_running"), "warn");
        assert!(!result.to_string().contains(&secret));
        assert!(!result.to_string().contains(&startup));
        let result = check(dir.path(), &path, "192.0.2.1:8467", &|_| {
            Ok(page(vec![server(8, 10, None, "", "192.0.2.1")], 1, 1))
        });
        assert_eq!(status(&result, "node"), "fail");
        assert_eq!(status(&result, "admin_api"), "fail");
        let result = check(dir.path(), &path, "127.0.0.1:8467", &|_| {
            Ok(page(vec![server(7, 99, None, "", "192.0.2.1")], 1, 1))
        });
        assert_eq!(status(&result, "node"), "warn");
        assert_eq!(status(&result, "registrations"), "pass");
        assert!(result.to_string().contains("99"));
        fs::remove_file(&path).unwrap();
        let result = check(dir.path(), &path, "127.0.0.1:8467", &|_| {
            panic!("must skip Panel")
        });
        assert_eq!(status(&result, "config"), "fail");
        assert!(rows(&result["checks"])
            .iter()
            .any(|check| check["id"] == "panel" && check["detail"] == "skipped"));
        fs::remove_file(dir.path().join("agent.json")).unwrap();
        assert_eq!(
            status(
                &check(dir.path(), &path, "127.0.0.1:8467", &panel),
                "agent_dir"
            ),
            "fail"
        );
    }

    #[test]
    fn check_paginates_and_compares_stable_ids_only_for_mapped_active_servers() {
        let (dir, path, _) = fixture();
        let mut cfg: Value =
            serde_json::from_slice(&fs::read(dir.path().join("agent.json")).unwrap()).unwrap();
        cfg["games"] = json!([{"id":pterodactyl::stable_id("server-7-10"),"name":"Game", "type":"minecraft-java","protocol":"tcp","tunnelPort":25565,"dest":"192.0.2.1:25565","origin":"managed","devices":[]}]);
        // This test edits the non-secret game configuration only.
        fs::write(
            dir.path().join("agent.json"),
            serde_json::to_vec(&cfg).unwrap(),
        )
        .unwrap();
        let calls = Cell::new(0);
        let result = check(dir.path(), &path, "127.0.0.1:8467", &|input| {
            calls.set(calls.get() + 1);
            if s(&input, "url").ends_with("page=1") {
                Ok(page(vec![server(8, 10, None, "", "192.0.2.1")], 1, 2))
            } else {
                Ok(page(
                    vec![server(7, 10, Some("Paper"), "", "192.0.2.1")],
                    2,
                    2,
                ))
            }
        });
        assert_eq!(calls.get(), 2);
        assert_eq!(status(&result, "registrations"), "pass");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                status(
                    &check(dir.path(), &path, "127.0.0.1:8467", &panel),
                    "agent_dir"
                ),
                "warn"
            );
        }
    }

    #[test]
    fn admin_api_checks_both_auth_paths_and_rejects_unauthenticated_success() {
        for bad in [false, true] {
            let (dir, path, _) = fixture();
            let client = random_token().unwrap();
            create_private(&dir.path().join("client.token"), client.as_bytes()).unwrap();
            let mut cfg: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            cfg["clientTokenFile"] = json!("client.token");
            let path = dir.path().join("checked-host.json");
            create_private(&path, &serde_json::to_vec(&cfg).unwrap()).unwrap();
            let (_, admin) = host_admin::load(&path).unwrap();
            create_private(
                &dir.path().join("runtime.json"),
                serde_json::to_vec(&json!({"updated":crate::now()}))
                    .unwrap()
                    .as_slice(),
            )
            .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = thread::spawn(move || {
                for index in 0..if bad { 1 } else { 2 } {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        request.push_str(&line);
                    }
                    assert!(request.starts_with("GET /v1/host/status HTTP/1.1\r\n"));
                    if index == 0 {
                        assert!(!request.contains("Authorization:"));
                    } else {
                        assert!(request.contains(&format!("Authorization: Bearer {admin}\r\n")));
                    }
                    let code = if bad || index == 1 { 200 } else { 401 };
                    write!(
                        stream,
                        "HTTP/1.1 {code} Result\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                }
            });
            let check_panel = |_: Value| {
                let mut server = server(7, 10, Some("Paper"), "", "192.0.2.1");
                server["attributes"]["suspended"] = json!(true);
                Ok(page(vec![server], 1, 1))
            };
            let result = check(dir.path(), &path, &address.to_string(), &check_panel);
            worker.join().unwrap();
            assert_eq!(
                status(&result, "admin_api"),
                if bad { "fail" } else { "pass" }
            );
            assert_eq!(status(&result, "agent_running"), "pass");
            if !bad {
                assert_eq!(result["ok"], true);
                assert!(rows(&result["checks"])
                    .iter()
                    .all(|check| check["status"] == "pass"));
            }
            assert!(!result.to_string().contains(&client));
            assert!(!result
                .to_string()
                .contains(&token(&dir.path().join("host-admin.token")).unwrap()));
            // Listener is now dropped: running agent with no host interface warns.
            assert_eq!(
                status(
                    &check(dir.path(), &path, &address.to_string(), &panel),
                    "admin_api"
                ),
                "warn"
            );
        }
        for address in ["0.0.0.0:8467", "[::]:8467", "localhost:8467"] {
            assert!(loopback_address(address).is_err());
        }
    }

    #[test]
    fn interface_discovery_includes_loopback_without_binding() {
        assert!(local_ips().unwrap().iter().any(IpAddr::is_loopback));
    }

    #[test]
    fn malformed_pagination_and_shim_errors_are_generic() {
        let dir = tempdir().unwrap();
        let secret = random_token().unwrap();
        let file = dir.path().join("panel.token");
        create_private(&file, secret.as_bytes()).unwrap();
        for bad in [json!({}), page(vec![], 2, 2), page(vec![], 1, 1001)] {
            assert!(detect(
                "https://panel.example",
                &file,
                None,
                None,
                &BTreeSet::new(),
                &|_| Ok(bad.clone())
            )
            .is_err());
        }
        let error = detect(
            "https://panel.example",
            &file,
            None,
            None,
            &BTreeSet::new(),
            &|_| Err(secret.clone()),
        )
        .unwrap_err();
        assert!(!error.contains(&secret));
        assert!(detect(
            &format!("https://user:{secret}@panel.example"),
            &file,
            None,
            None,
            &BTreeSet::new(),
            &|_| panic!("invalid URL must not be requested")
        )
        .is_err());
    }
}
