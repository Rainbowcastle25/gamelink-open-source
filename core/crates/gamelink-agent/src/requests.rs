use crate::{
    bridge, invitations::load_keys, json, print_table, pure, rows, s, storage, Cli, Result,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::ErrorKind,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const LIMIT: usize = 32;
const LIFETIME: TimeDelta = TimeDelta::hours(24);

pub fn identity(keys: &Value) -> String {
    let mut hash = Sha256::new();
    hash.update(s(keys, "privateKey").as_bytes());
    hash.update([0]);
    hash.update(s(keys, "presharedKey").as_bytes());
    format!("{:x}", hash.finalize())
}

pub struct Scope {
    pub identity: String,
    approved: BTreeSet<String>,
    pub keys: Value,
}

impl Scope {
    pub fn new(game: &Value, keys: Value) -> Self {
        Self {
            identity: identity(&keys),
            approved: rows(&game["devices"])
                .iter()
                .map(|d| s(d, "publicKey").to_ascii_lowercase())
                .collect(),
            keys,
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Knock {
    public_key: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    attempts: u64,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
struct Request {
    id: u64,
    #[serde(flatten)]
    knock: Knock,
}

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
struct GameRequests {
    identity: String,
    pending: Vec<Request>,
    // Ignore queued attempts which arrived before a dismissal. A fresh knock
    // from the same key can be listed again with its own ID.
    dismissed: BTreeMap<String, DateTime<Utc>>,
}

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
pub struct Store {
    next_id: u64,
    games: BTreeMap<String, GameRequests>,
}

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

fn active(knock: &Knock, scope: &Scope, now: DateTime<Utc>) -> bool {
    now - knock.first_seen < LIFETIME && !scope.approved.contains(&knock.public_key)
}

impl Store {
    pub fn load(dir: &Path) -> Result<Self> {
        match storage::read_bounded(&dir.join("requests.json")) {
            Ok(data) => serde_json::from_slice(&data).map_err(|_| "invalid request store".into()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("read request store: {e}")),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let bytes = serde_json::to_vec(self).map_err(|_| "cannot encode request store")?;
        storage::write_atomic(&dir.join("requests.json"), &bytes)
    }

    pub fn list(&self, scopes: &BTreeMap<String, Scope>, now: DateTime<Utc>) -> Vec<Value> {
        let mut out = Vec::new();
        for (game, pending) in &self.games {
            let Some(scope) = scopes
                .get(game)
                .filter(|scope| scope.identity == pending.identity)
            else {
                continue;
            };
            for req in &pending.pending {
                if active(&req.knock, scope, now) {
                    let mut value = json!(req);
                    value["game"] = json!(game);
                    out.push(value);
                }
            }
        }
        out.sort_unstable_by_key(|r| r["id"].as_u64());
        out
    }

    pub fn dismiss(&mut self, game: &str, id: u64, now: DateTime<Utc>) {
        let requests = self.games.get_mut(game).expect("listed game");
        let index = requests
            .pending
            .iter()
            .position(|r| r.id == id)
            .expect("listed request");
        let key = requests.pending.remove(index).knock.public_key;
        if requests.dismissed.len() >= LIMIT {
            if let Some(oldest) = requests
                .dismissed
                .iter()
                .min_by_key(|(_, at)| *at)
                .map(|(key, _)| key.clone())
            {
                requests.dismissed.remove(&oldest);
            }
        }
        requests.dismissed.insert(key, now);
    }

    fn merge(
        &mut self,
        scopes: &BTreeMap<String, Scope>,
        running: &BTreeMap<String, String>,
        batches: BTreeMap<String, Vec<Knock>>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.games.retain(|game, requests| {
            let Some(scope) = scopes
                .get(game)
                .filter(|scope| scope.identity == requests.identity)
            else {
                return false;
            };
            requests.pending.retain(|r| active(&r.knock, scope, now));
            requests.dismissed.retain(|_, at| now - *at < LIFETIME);
            !requests.pending.is_empty() || !requests.dismissed.is_empty()
        });
        for (game, mut batch) in batches {
            let Some(scope) = scopes
                .get(&game)
                .filter(|scope| running.get(&game) == Some(&scope.identity))
            else {
                continue;
            };
            let requests = self.games.entry(game).or_insert_with(|| GameRequests {
                identity: scope.identity.clone(),
                ..GameRequests::default()
            });
            batch.sort_unstable_by_key(|knock| knock.first_seen);
            for knock in batch {
                if !active(&knock, scope, now)
                    || requests
                        .dismissed
                        .get(&knock.public_key)
                        .is_some_and(|at| knock.first_seen <= *at)
                {
                    continue;
                }
                if let Some(existing) = requests
                    .pending
                    .iter_mut()
                    .find(|r| r.knock.public_key == knock.public_key)
                {
                    existing.knock.last_seen = existing.knock.last_seen.max(knock.last_seen);
                    existing.knock.attempts =
                        existing.knock.attempts.saturating_add(knock.attempts);
                } else if requests.pending.len() < LIMIT {
                    self.next_id = self.next_id.checked_add(1).ok_or("request IDs exhausted")?;
                    requests.pending.push(Request {
                        id: self.next_id,
                        knock,
                    });
                }
            }
        }
        Ok(())
    }
}

pub fn publish(
    dir: &Path,
    scopes: &BTreeMap<String, Scope>,
    running: &BTreeMap<String, String>,
) -> Result<()> {
    let _lock = lock(dir)?;
    let existing = Store::load(dir)?;
    let batches = if running.is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_value(bridge::call(json!({"method":"requests"}))?)
            .map_err(|_| "invalid request batch")?
    };
    let mut store = existing.clone();
    store.merge(scopes, running, batches, Utc::now())?;
    if store != existing {
        store.save(dir)?;
    }
    Ok(())
}

impl Cli {
    pub fn requests(&self, cmd: &str, m: &clap::ArgMatches) -> Result<()> {
        let list = cmd == "requests list";
        let _lock = if list { None } else { Some(lock(&self.dir)?) };
        let cfg = self.load()?;
        let filter = m.get_one::<String>("game").map(String::as_str);
        let mut scopes = BTreeMap::new();
        let mut found_game = false;
        for game in rows(&cfg["games"]) {
            let id = s(game, "id");
            if filter.is_some_and(|filter| filter != id) {
                continue;
            }
            found_game = true;
            if game["disabled"] != true {
                if let Ok(keys) = load_keys(&self.dir, id) {
                    if pure::check_invite_expiry(&keys["expiresAt"]).is_ok() {
                        scopes.insert(id.to_owned(), Scope::new(game, keys));
                    }
                }
            }
        }
        if let Some(game) = filter.filter(|_| !found_game) {
            return Err(format!("no game with id {game:?}"));
        }
        let mut store = Store::load(&self.dir)?;
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
        store.dismiss(game, id, chrono::Utc::now());
        if cmd == "requests approve" {
            println!("Approved request {id} for {game:?}. If the first attempt timed out, ask your friend to click Join again after the agent reloads.");
            if store.save(&self.dir).is_err() {
                eprintln!("The approval was saved, but the request store could not be updated.");
            }
        } else {
            store.save(&self.dir)?;
            println!("Dismissed request {id} for {game:?}.");
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

    fn knock(id: usize, at: DateTime<Utc>) -> Knock {
        Knock {
            public_key: format!("peer-{id}"),
            first_seen: at,
            last_seen: at,
            attempts: 1,
        }
    }

    #[test]
    fn pending_requests_deduplicate_expire_and_keep_their_ids() {
        let now = Utc::now();
        let game = "game".to_owned();
        let scope = Scope::new(&json!({}), json!({"privateKey":"old"}));
        let running = BTreeMap::from([(game.clone(), scope.identity.clone())]);
        let mut scopes = BTreeMap::from([(game.clone(), scope)]);
        let mut store = Store::default();
        let batch = |knocks| BTreeMap::from([(game.clone(), knocks)]);
        store
            .merge(
                &scopes,
                &running,
                batch(
                    (0..40)
                        .map(|id| knock(id, now + TimeDelta::milliseconds(id as i64)))
                        .collect(),
                ),
                now + TimeDelta::seconds(1),
            )
            .unwrap();
        let first = store.list(&scopes, now);
        assert_eq!(first.len(), LIMIT);
        assert_eq!(first[0]["id"], 1);
        let later = now + TimeDelta::hours(1);
        store
            .merge(&scopes, &running, batch(vec![knock(0, later)]), later)
            .unwrap();
        let repeated = store.list(&scopes, later);
        assert_eq!(repeated[0]["id"], first[0]["id"]);
        assert_eq!(repeated[0]["firstSeen"], first[0]["firstSeen"]);
        assert_eq!(repeated[0]["lastSeen"], json!(later));
        assert_eq!(repeated[0]["attempts"], 2);
        assert_eq!(
            store
                .list(&scopes, now + LIFETIME + TimeDelta::seconds(1))
                .len(),
            0
        );

        store.dismiss(&game, 1, later);
        store
            .merge(&scopes, &running, batch(vec![knock(0, later)]), later)
            .unwrap();
        assert_eq!(store.list(&scopes, later).len(), LIMIT - 1);
        let again = later + TimeDelta::seconds(1);
        store
            .merge(&scopes, &running, batch(vec![knock(0, again)]), again)
            .unwrap();
        assert_eq!(store.list(&scopes, again)[LIMIT - 1]["id"], 33);

        scopes.insert(
            game.clone(),
            Scope::new(&json!({}), json!({"privateKey":"new"})),
        );
        store
            .merge(&scopes, &running, batch(vec![knock(0, again)]), again)
            .unwrap();
        assert!(store.list(&scopes, again).is_empty());
    }
}
