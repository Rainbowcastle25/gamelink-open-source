use crate::{
    bridge, invitations::load_keys, json, print_table, pure, rows, s, storage, Cli, Result,
};
use chrono::Utc;
use gamelink_core::requests::Store;
pub use gamelink_core::requests::{identity, Scope};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::ErrorKind,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub fn lock(dir: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("requests.lock"))
        .map_err(|e| format!("open request lock: {e}"))?;
    file.lock().map_err(|e| format!("lock requests: {e}"))?;
    Ok(file)
}

pub(crate) fn load(dir: &Path) -> Result<Store> {
    match storage::read_bounded(&dir.join("requests.json")) {
        Ok(data) => serde_json::from_slice(&data).map_err(|_| "invalid request store".into()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(format!("read request store: {e}")),
    }
}

pub(crate) fn save(store: &Store, dir: &Path) -> Result<()> {
    let bytes = serde_json::to_vec(store).map_err(|_| "cannot encode request store")?;
    storage::write_atomic(&dir.join("requests.json"), &bytes)
}

pub fn publish(
    dir: &Path,
    scopes: &BTreeMap<String, Scope>,
    running: &BTreeMap<String, String>,
) -> Result<()> {
    let _lock = lock(dir)?;
    let existing = load(dir)?;
    let batches = if running.is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_value(bridge::call(json!({"method":"requests"}))?)
            .map_err(|_| "invalid request batch")?
    };
    let mut store = existing.clone();
    store.merge(scopes, running, batches, Utc::now())?;
    if store != existing {
        save(&store, dir)?;
    }
    Ok(())
}

pub(crate) fn scopes<'a>(
    dir: &Path,
    games: impl IntoIterator<Item = &'a Value>,
) -> BTreeMap<String, Scope> {
    games
        .into_iter()
        .filter_map(|game| {
            if game["disabled"] == true {
                return None;
            }
            let id = s(game, "id");
            let keys = load_keys(dir, id).ok()?;
            pure::check_invite_expiry(&keys["expiresAt"]).ok()?;
            Some((id.to_owned(), Scope::new(game, keys)))
        })
        .collect()
}

impl Cli {
    pub fn requests(&self, cmd: &str, m: &clap::ArgMatches) -> Result<()> {
        let list = cmd == "requests list";
        let _lock = if list { None } else { Some(lock(&self.dir)?) };
        let cfg = self.load()?;
        let filter = m.get_one::<String>("game").map(String::as_str);
        if let Some(id) =
            filter.filter(|id| !rows(&cfg["games"]).iter().any(|game| game["id"] == *id))
        {
            return Err(format!("no game with id {id:?}"));
        }
        let scopes = scopes(
            &self.dir,
            rows(&cfg["games"])
                .iter()
                .filter(|game| filter.is_none_or(|id| game["id"] == id)),
        );
        let mut store = load(&self.dir)?;
        let pending = store.list(&scopes, chrono::Utc::now());
        if list {
            if m.get_flag("json") {
                println!("{}", json!({"requests":pending}));
            } else if pending.is_empty() {
                println!("No pending join requests.");
            } else {
                let mut table = vec![[
                    "ID",
                    "GAME",
                    "PUBLIC KEY",
                    "FIRST SEEN",
                    "LAST SEEN",
                    "ATTEMPTS",
                ]
                .map(str::to_owned)
                .to_vec()];
                for request in pending {
                    table.push(vec![
                        request["id"].to_string(),
                        s(&request, "game").into(),
                        s(&request, "publicKey").into(),
                        s(&request, "firstSeen").into(),
                        s(&request, "lastSeen").into(),
                        request["attempts"].to_string(),
                    ]);
                }
                print_table(table);
            }
            return Ok(());
        }
        let selector = m
            .get_one::<String>("positional")
            .ok_or("request ID or public key is required")?;
        let request = select_request(&pending, selector)?;
        let game = s(request, "game");
        let id = request["id"].as_u64().ok_or("invalid request ID")?;
        if cmd == "requests approve" {
            let label = m
                .get_one::<String>("label")
                .map(String::as_str)
                .unwrap_or("");
            self.approve_device(game, s(request, "publicKey"), label, &scopes[game].keys)?;
        }
        if cmd == "requests approve" {
            store.remove(game, id);
            println!("Approved request {id} for {game:?}. If the first attempt timed out, ask your friend to click Join again after the agent reloads.");
            if save(&store, &self.dir).is_err() {
                eprintln!("The approval was saved, but the request store could not be updated.");
            }
        } else {
            store.dismiss(game, id, chrono::Utc::now());
            save(&store, &self.dir)?;
            println!("Dismissed request {id} for {game:?}. GameLink ignores this device for this game until you rotate its invitation.");
        }
        Ok(())
    }
}

fn select_request<'a>(pending: &'a [Value], selector: &str) -> Result<&'a Value> {
    let id = selector.parse::<u64>().ok();
    let key = if id.is_none() {
        pure::dispatch("identity.public_key", json!({"text":selector}))?
    } else {
        Value::Null
    };
    let mut matching = pending.iter().filter(|r| {
        id.is_some_and(|id| r["id"] == id) || (!key.is_null() && r["publicKey"] == key)
    });
    let selected = matching.next().ok_or("no matching pending join request")?;
    if matching.next().is_some() {
        return Err("that key has requests for multiple games; use a request ID or --game".into());
    }
    Ok(selected)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_arguments_and_selectors_support_ids_and_keys() {
        let args = ["17", "--label", "Sam's PC", "--game", "one"].map(str::to_owned);
        let parsed = crate::parse("requests approve", args.to_vec()).unwrap();
        assert_eq!(parsed.get_one::<String>("positional").unwrap(), "17");
        assert_eq!(parsed.get_one::<String>("label").unwrap(), "Sam's PC");
        assert_eq!(parsed.get_one::<String>("game").unwrap(), "one");
        assert!(crate::parse("requests dismiss", vec![]).is_err());
        assert!(crate::parse("requests list", vec!["17".into()]).is_err());

        let public = format!("nodekey:{}", "ab".repeat(32));
        let first = json!({"id":17,"game":"one","publicKey":public});
        let second = json!({"id":42,"game":"two","publicKey":public});
        assert_eq!(
            select_request(&[first.clone(), second.clone()], "42").unwrap(),
            &second
        );
        assert_eq!(
            select_request(std::slice::from_ref(&first), &public).unwrap(),
            &first
        );
        assert!(select_request(&[first, second], &public)
            .unwrap_err()
            .contains("multiple games"));
        assert!(select_request(&[], "17").is_err());
    }
}
