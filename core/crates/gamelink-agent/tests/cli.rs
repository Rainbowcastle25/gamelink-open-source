use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Command, Output},
};
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gamelink-agent"))
        .arg("--dir")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}
fn ok(dir: &Path, args: &[&str]) -> Output {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn legacy_help(text: &str) -> String {
    // Keep the frozen Go oracle for existing text; only the explicitly added
    // host, request, invitation and update features are outside that contract.
    text.replace(
        "  host refresh                  discover/register Pterodactyl games once (no background scan)\n",
        "",
    )
    .replace(
        "  host detect                   inspect Panel nodes and eggs; optionally create private host settings\n",
        "",
    )
    .replace(
        "  host check                    diagnose host settings, Panel access, registration and loopback API\n",
        "",
    )
    .replace(
        "  update check [--json]         verify release metadata and report available updates\n",
        "",
    )
    .replace(
        "\nMachine-readable output: status, games list, devices list, requests list and host refresh accept --json.\n  host detect --panel-url URL --token-file PATH [--node-id N] [--out PATH] [--json]\n  host check --config PATH [--listen 127.0.0.1:8467] [--json]\nDetect exits 3 when a node decision is needed; check exits 1 if any check fails.\n",
        "",
    )
    .replace("  requests list [--game ID]     list pending join requests\n", "")
    .replace("  requests approve <id|key>    approve a pending device for its game (optional --label)\n", "")
    .replace("  requests dismiss <id|key>    remove a pending request from the list\n", "")
    .replace(
        "  invite --game ID              print a private invitation (optional --expires 24h rotates the identity)\n  invite rotate --game ID       replace the invitation while keeping device approvals\n",
        "  invite                        print the invitation a friend imports into GameLink (a secret)\n",
    )
    .replace(
        "  -expires string\n    \trotate to a new game identity that stops serving after this duration, e.g. 24h\n",
        "",
    )
    .replace("  -json\n    \tprint machine-readable JSON\n", "")
    .replace(
        "  -host-config string\n    \tenable loopback host administration using this private settings file\n  -host-listen string\n    \tloopback host administration address (default 127.0.0.1:8467)\n",
        "",
    )
}
#[test]
fn frozen_go_cli_oracle() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/cli-go-c320fc7.json")).unwrap();
    for f in fixtures.as_array().unwrap() {
        let args: Vec<_> = f["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let out = run(Path::new("/FROZEN"), &args);
        assert_eq!(
            out.status.code(),
            f["code"].as_i64().map(|n| n as i32),
            "{args:?}"
        );
        let stdout = legacy_help(&String::from_utf8_lossy(&out.stdout));
        if args == ["version"] || args == ["--version"] {
            assert_eq!(
                stdout,
                concat!("gamelink-agent ", env!("CARGO_PKG_VERSION"), "\n")
            );
        } else {
            assert_eq!(stdout, f["stdout"].as_str().unwrap(), "{args:?}");
        }
        assert_eq!(
            legacy_help(&String::from_utf8_lossy(&out.stderr)),
            f["stderr"].as_str().unwrap(),
            "{args:?}"
        );
    }
}
#[test]
fn update_check_without_a_verification_key_fails_clearly() {
    if gamelink_core::update::can_check() {
        return; // This build embeds a release key.
    }
    let temp = tempfile::tempdir().unwrap();
    for args in [&["update", "check"][..], &["update", "check", "--json"]] {
        let out = run(temp.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(out.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "error: this build can't check for updates: it has no release verification key\n"
        );
    }
    assert!(!temp.path().join("release-check.json").exists());
}

#[test]
fn migrated_admin_flow_and_private_storage() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("agent");
    assert!(!run(&dir, &["games", "list"]).status.success());
    ok(&dir, &["init"]);
    assert!(!run(&dir, &["init"]).status.success());
    ok(
        &dir,
        &[
            "games",
            "add",
            "--type",
            "minecraft-java",
            "--relay-host",
            "derp.example.com",
            "--name",
            "Survival World",
        ],
    );
    let list = ok(&dir, &["games", "list"]);
    assert!(String::from_utf8_lossy(&list.stdout).contains("survival-world"));
    let inv = ok(&dir, &["invite", "--game", "survival-world"]);
    assert!(String::from_utf8_lossy(&inv.stderr).contains("requests list --game survival-world"));
    let cfg: Value =
        serde_json::from_slice(&std::fs::read(dir.join("agent.json")).unwrap()).unwrap();
    assert_eq!(cfg["games"][0]["knockEnabled"], true);
    let requests = ok(
        &dir,
        &["requests", "list", "--game", "survival-world", "--json"],
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests.stdout).unwrap(),
        json!({"requests":[]})
    );
    let pubkey = format!("nodekey:{}", "ab".repeat(32));
    ok(
        &dir,
        &[
            "devices",
            "approve",
            "--game",
            "survival-world",
            "--key",
            &pubkey,
            "--label",
            "alice",
        ],
    );
    let inv = ok(
        &dir,
        &["invite", "--game", "survival-world", "--note", "welcome"],
    );
    let parsed = gamelink_core::pure::dispatch(
        "invite.decode",
        json!({"text":String::from_utf8(inv.stdout).unwrap().trim()}),
    )
    .unwrap();
    assert_eq!(parsed["name"], "Survival World");
    assert_eq!(parsed["port"], 25565);
    assert_eq!(parsed["note"], "welcome");
    assert!(!parsed.to_string().contains("127.0.0.1"));
    assert!(!String::from_utf8_lossy(&inv.stderr).contains("Warning"));
    let list = ok(&dir, &["devices", "list"]);
    assert!(String::from_utf8_lossy(&list.stdout).contains("alice"));
    let cfg = std::fs::read_to_string(dir.join("agent.json")).unwrap();
    assert!(!cfg.contains("privkey:"));
    assert!(!cfg.contains("presharedKey"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.join("keys/survival-world.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    ok(
        &dir,
        &[
            "devices",
            "revoke",
            "--game",
            "survival-world",
            "--key",
            &pubkey,
        ],
    );
    assert!(!run(
        &dir,
        &[
            "devices",
            "revoke",
            "--game",
            "survival-world",
            "--key",
            &pubkey
        ]
    )
    .status
    .success());
    assert!(
        String::from_utf8_lossy(&ok(&dir, &["devices", "list"]).stdout)
            .contains("No approved devices")
    );
    ok(&dir, &["games", "remove", "survival-world"]);
    assert!(!dir.join("keys/survival-world.json").exists());
}
#[test]
fn migrated_bad_inputs_and_go_flag_quirks() {
    let dir = tempfile::tempdir().unwrap();
    ok(dir.path(), &["init", "ignored", "--unknown"]);
    let cases: Vec<Vec<&str>> = vec![
        vec!["--type", "quake"],
        vec!["--type", "custom", "--proto", "tcp", "--port", "70000"],
        vec![
            "--type",
            "minecraft-java",
            "--dest",
            "evil.example.com:25565",
        ],
        vec!["--type", "minecraft-java", "--dest", "0.0.0.0:25565"],
        vec!["--type", "factorio", "--proto", "tcp"],
        vec!["--type", "custom", "--port", "5000"],
        vec!["--type", "minecraft-java", "--id", "../etc"],
    ];
    for case in cases {
        let mut args = vec!["games", "add"];
        args.extend(case);
        args.extend(["--relay-host", "derp.example.com"]);
        assert!(!run(dir.path(), &args).status.success());
    }
    ok(
        dir.path(),
        &[
            "games",
            "add",
            "-type",
            "factorio",
            "--type=minecraft-java",
            "-relay-host=derp.example.com",
        ],
    );
    assert!(!run(
        dir.path(),
        &[
            "games",
            "add",
            "--type",
            "minecraft-java",
            "--relay-host",
            "derp.example.com"
        ]
    )
    .status
    .success());
    for key in ["hello", "privkey:aaaaaaaa", "nodekey:abcd"] {
        assert!(!run(
            dir.path(),
            &[
                "devices",
                "approve",
                "--game",
                "minecraft-java",
                "--key",
                key
            ]
        )
        .status
        .success());
    }
    assert!(!run(
        dir.path(),
        &[
            "games",
            "add",
            "--type",
            "minecraft-java",
            "--console-bridge=false",
            "--bedrock-ip",
            "127.0.0.1",
            "--relay-host",
            "derp.example.com"
        ]
    )
    .status
    .success());
}

#[test]
fn bedrock_media_ports_are_persisted_and_validated() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("agent");
    ok(&dir, &["init"]);
    ok(
        &dir,
        &[
            "games",
            "add",
            "--type",
            "minecraft-bedrock",
            "--media-ports",
            "19140-19145",
            "--relay-host",
            "derp.example.com",
        ],
    );
    let cfg: Value =
        serde_json::from_slice(&std::fs::read(dir.join("agent.json")).unwrap()).unwrap();
    assert_eq!(
        cfg["games"][0]["bedrock"],
        json!({"mediaPortFirst":19140,"mediaPortLast":19145})
    );
    assert!(
        cfg["games"][0].get("consoleBridge").is_none()
            || cfg["games"][0]["consoleBridge"].is_null()
    );

    for (game_type, range) in [
        ("minecraft-bedrock", "0-10"),
        ("minecraft-bedrock", "10-9"),
        ("minecraft-bedrock", "1000-1256"),
        ("minecraft-java", "19140-19145"),
    ] {
        assert!(
            !run(
                &dir,
                &[
                    "games",
                    "add",
                    "--type",
                    game_type,
                    "--media-ports",
                    range,
                    "--relay-host",
                    "derp.example.com",
                ],
            )
            .status
            .success(),
            "accepted {game_type} range {range}"
        );
    }
}

#[test]
fn frozen_go_admin_output() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/admin-go-c320fc7.json")).unwrap();
    for f in fixtures.as_array().unwrap() {
        let args: Vec<_> = f["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let out = run(dir.path(), &args);
        let normalize = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes)
                .replace(dir.path().to_str().unwrap(), "<DIR>")
                .replace(&chrono::Utc::now().format("%Y-%m-%d").to_string(), "<DATE>")
                .replace(
                    "Nobody can reach it yet: no devices are approved. Share a private invitation with:\n  gamelink-agent invite --game survival-world",
                    "Nobody can reach it yet: no devices are approved. Next:\n  gamelink-agent devices approve --game survival-world --key nodekey:<friend's key> --label <name>\n  gamelink-agent invite --game survival-world",
                )
        };
        assert_eq!(
            out.status.code(),
            f["code"].as_i64().map(|n| n as i32),
            "{args:?}"
        );
        assert_eq!(
            normalize(&out.stdout),
            f["stdout"].as_str().unwrap(),
            "{args:?}"
        );
        assert_eq!(
            normalize(&out.stderr),
            f["stderr"].as_str().unwrap(),
            "{args:?}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn serve_shutdown_when_signal_lands_on_a_runtime_thread() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    ok(dir.path(), &["init"]);
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        Command::new(env!("CARGO_BIN_EXE_gamelink-agent"))
            .arg("--dir")
            .arg(dir.path())
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dir.path().join("runtime.json").is_file() {
        assert!(Instant::now() < deadline, "serve did not initialize");
        assert!(child.0.try_wait().unwrap().is_none(), "serve exited early");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = child.0.id() as libc::pid_t;
    // Go initializes its runtime thread before ctrlc starts its handler thread.
    // Signal the first non-main thread to exercise SA_ONSTACK, not just the
    // usual process signal path that tends to select the Rust main thread.
    let tid = std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .parse::<libc::pid_t>()
                .unwrap()
        })
        .filter(|&tid| tid != pid)
        .min()
        .expect("Go runtime thread");
    assert_eq!(
        unsafe { libc::syscall(libc::SYS_tgkill, pid, tid, libc::SIGTERM) },
        0
    );
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "signal shutdown failed");
            break;
        }
        assert!(Instant::now() < deadline, "serve did not shut down");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!dir.path().join("runtime.json").exists());
}
