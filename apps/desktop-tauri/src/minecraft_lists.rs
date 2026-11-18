//! Resolve the player's Minecraft directories and keep ownership across saves
//! by Minecraft, including when the configured Prism folder changes.
use gamelink_core::{minecraft, storage};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

type Ownership = BTreeMap<PathBuf, minecraft::Entries>;

#[derive(Default)]
pub struct MinecraftLists {
    default_ready: bool,
    prism_ready: bool,
    pub error: String,
}

impl MinecraftLists {
    pub fn refresh(&mut self, state_dir: &Path, cfg: &Value) {
        self.refresh_from(state_dir, cfg, default_minecraft_dir());
    }

    fn refresh_from(&mut self, state_dir: &Path, cfg: &Value, default: Option<PathBuf>) {
        self.error.clear();
        self.default_ready = false;
        self.prism_ready = false;
        if let Err(error) = self.sync(state_dir, cfg, default) {
            self.error = error;
        }
    }

    pub fn listed(&self, launcher: &str) -> bool {
        let prism = Path::new(launcher)
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("prismlauncher"));
        if prism {
            self.prism_ready
        } else {
            self.default_ready
        }
    }

    fn sync(
        &mut self,
        state_dir: &Path,
        cfg: &Value,
        default: Option<PathBuf>,
    ) -> Result<(), String> {
        let ledger = state_dir.join("minecraft-lists.json");
        let mut tracked: Ownership = match storage::read_bounded(&ledger) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "GameLink could not read its Minecraft list history")?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(_) => return Err("GameLink could not open its Minecraft list history".into()),
        };
        let empty = minecraft::Entries::new();
        let mut desired = minecraft::Entries::new();
        let mut default_dir = None;
        let mut prism = BTreeSet::new();
        let mut errors = Vec::new();
        if cfg["settings"]["minecraftServerList"] == true {
            for server in cfg["servers"].as_array().into_iter().flatten() {
                if server["game"] != "minecraft-java" {
                    continue;
                }
                if let (Some(id), Some(name), Some(port)) = (
                    server["id"].as_str(),
                    server["name"].as_str(),
                    server["stablePort"]
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok())
                        .filter(|&n| n != 0),
                ) {
                    desired.insert(
                        id.to_owned(),
                        minecraft::Entry {
                            name: name.to_owned(),
                            port,
                        },
                    );
                }
            }
            if let Some(dir) = default {
                let default = if desired.is_empty() {
                    dir.canonicalize()
                } else {
                    fs::create_dir_all(&dir).and_then(|()| dir.canonicalize())
                };
                match default {
                    Ok(dir) => default_dir = Some(dir),
                    Err(err)
                        if desired.is_empty() && err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => errors.push(format!("Default Minecraft folder ({err})")),
                }
            } else {
                errors.push("Could not locate the default Minecraft folder".into());
            }

            if let Some(root) = cfg["settings"]["prismInstancesDir"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                match prism_dirs(Path::new(root)) {
                    Ok(dirs) => prism = dirs,
                    Err(err) => errors.push(format!("Prism instances folder ({err})")),
                }
            }
        }

        let targets = default_dir
            .iter()
            .chain(&prism)
            .cloned()
            .collect::<BTreeSet<_>>();
        // Register new targets before file replacement so a failed ownership
        // save still leaves a path for recovering marker tags at next startup.
        let mut new_targets = false;
        if !desired.is_empty() {
            for dir in &targets {
                if !tracked.contains_key(dir) {
                    tracked.insert(dir.clone(), minecraft::Entries::new());
                    new_targets = true;
                }
            }
        }
        if new_targets {
            save_ownership(&ledger, &tracked)?;
        }

        let discovery_failed = !errors.is_empty();
        let dirs = targets
            .iter()
            .chain(tracked.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut ready = BTreeSet::new();
        for dir in dirs {
            let wanted = if targets.contains(&dir) {
                &desired
            } else if discovery_failed {
                // Keep history for a target that may be temporarily unavailable.
                continue;
            } else {
                &empty
            };
            match minecraft::sync(&dir, tracked.get(&dir).unwrap_or(&empty), wanted) {
                Ok(owned) => {
                    if owned.is_empty() && wanted.is_empty() {
                        if tracked.remove(&dir).is_some() {
                            save_ownership(&ledger, &tracked)?;
                        }
                    } else if tracked.get(&dir) != Some(&owned) {
                        tracked.insert(dir.clone(), owned);
                        save_ownership(&ledger, &tracked)?;
                    }
                    if !wanted.is_empty() {
                        ready.insert(dir);
                    }
                }
                Err(err) => errors.push(format!("{} ({err})", dir.display())),
            }
        }
        self.default_ready = default_dir.as_ref().is_some_and(|dir| ready.contains(dir));
        self.prism_ready = !prism.is_empty() && prism.is_subset(&ready);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(". "))
        }
    }
}

fn default_minecraft_dir() -> Option<PathBuf> {
    let base = std::env::var_os(if cfg!(windows) { "APPDATA" } else { "HOME" })?;
    (!base.is_empty()).then(|| PathBuf::from(base).join(".minecraft"))
}

fn save_ownership(ledger: &Path, tracked: &Ownership) -> Result<(), String> {
    let bytes = serde_json::to_vec(tracked).map_err(|_| "could not save Minecraft list history")?;
    storage::write_atomic(ledger, &bytes)
}

fn prism_dirs(root: &Path) -> std::io::Result<BTreeSet<PathBuf>> {
    let mut dirs = BTreeSet::new();
    for entry in fs::read_dir(root)? {
        let instance = entry?.path();
        if !instance.join("instance.cfg").is_file() {
            continue;
        }
        for name in [".minecraft", "minecraft"] {
            let dir = instance.join(name);
            if dir.is_dir() {
                dirs.insert(dir.canonicalize()?);
            }
        }
    }
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_only_existing_game_dirs_in_prism_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let prism = tmp.path();
        let instance = prism.join("Modded");
        let game_dir = instance.join(".minecraft");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(instance.join("instance.cfg"), "").unwrap();
        fs::create_dir_all(prism.join("Other/minecraft")).unwrap();
        assert_eq!(
            prism_dirs(prism).unwrap(),
            BTreeSet::from([game_dir.canonicalize().unwrap()])
        );
    }

    #[test]
    fn syncs_default_and_prism_and_removes_entries_even_after_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("GameLink");
        let default = tmp.path().join(".minecraft");
        let prism = tmp.path().join("instances");
        let instance = prism.join("Test");
        let game = instance.join("minecraft");
        fs::create_dir_all(&game).unwrap();
        fs::write(instance.join("instance.cfg"), "").unwrap();
        let mut cfg = json!({"settings":{"minecraftServerList":false,"prismInstancesDir":prism},
        "servers":[
            {"id":"012345abcdef","name":"Java World","game":"minecraft-java","stablePort":20000,"addr":"unrelated"},
            {"id":"abcdef012345","name":"Factory","game":"factorio","stablePort":20001}
        ]});
        let mut lists = MinecraftLists::default();
        lists.refresh_from(&state_dir, &cfg, Some(default.clone()));
        assert!(!default.exists() && !game.join("servers.dat").exists());

        cfg["settings"]["minecraftServerList"] = json!(true);
        fs::write(game.join("servers.dat"), b"malformed").unwrap();
        lists.refresh_from(&state_dir, &cfg, Some(default.clone()));
        assert!(lists.error.contains("servers.dat"));
        assert!(lists.listed(""));
        assert!(!lists.listed("prismlauncher.exe"));
        assert_eq!(fs::read(game.join("servers.dat")).unwrap(), b"malformed");
        fs::remove_file(game.join("servers.dat")).unwrap();

        lists.refresh_from(&state_dir, &cfg, Some(default.clone()));
        assert!(lists.listed("prismlauncher.exe"));
        assert!(lists.error.is_empty());
        let initial = fs::read(default.join("servers.dat")).unwrap();
        assert_eq!(fs::read(game.join("servers.dat")).unwrap(), initial);
        assert!(initial.windows(15).any(|x| x == b"127.0.0.1:20000"));
        assert!(!initial.windows(7).any(|x| x == b"Factory"));
        assert!(!initial.windows(9).any(|x| x == b"unrelated"));

        cfg["settings"]["minecraftServerList"] = json!(false);
        let mut reopened = MinecraftLists::default();
        reopened.refresh_from(&state_dir, &cfg, Some(default.clone()));
        assert!(!reopened.listed("") && !reopened.listed("prismlauncher.exe"));
        let disabled = fs::read(default.join("servers.dat")).unwrap();
        assert_eq!(fs::read(game.join("servers.dat")).unwrap(), disabled);
        assert!(!disabled.windows(10).any(|x| x == b"Java World"));

        cfg["settings"]["minecraftServerList"] = json!(true);
        reopened.refresh_from(&state_dir, &cfg, Some(default.clone()));
        assert_eq!(fs::read(default.join("servers.dat")).unwrap(), initial);
        cfg["servers"].as_array_mut().unwrap().remove(0);
        reopened.refresh_from(&state_dir, &cfg, Some(default.clone()));
        assert_eq!(fs::read(default.join("servers.dat")).unwrap(), disabled);
        assert_eq!(fs::read(game.join("servers.dat")).unwrap(), disabled);
    }
}
