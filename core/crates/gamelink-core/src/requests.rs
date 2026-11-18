//! Pending join requests observed at the Tailcat allowlist, scoped to each game identity.
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_default()
}

const LIMIT: usize = 32;
const LIFETIME: TimeDelta = TimeDelta::hours(24);

pub fn identity(keys: &Value) -> String {
    let mut hash = Sha256::new();
    hash.update(text(keys, "privateKey").as_bytes());
    hash.update([0]);
    hash.update(text(keys, "presharedKey").as_bytes());
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
            approved: game["devices"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| text(d, "publicKey").to_ascii_lowercase())
                .collect(),
            keys,
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Knock {
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

fn active(knock: &Knock, scope: &Scope, now: DateTime<Utc>) -> bool {
    now - knock.first_seen < LIFETIME && !scope.approved.contains(&knock.public_key)
}

impl Store {
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

    pub fn merge(
        &mut self,
        scopes: &BTreeMap<String, Scope>,
        running: &BTreeMap<String, String>,
        batches: BTreeMap<String, Vec<Knock>>,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let now = DateTime::<Utc>::from(std::time::SystemTime::now());
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
