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
const DISMISSED_LIMIT: usize = 256;
const LIFETIME: TimeDelta = TimeDelta::hours(24);

/// A one-way identifier for a game's server public key. Anyone who knows the
/// public key itself can knock, so only this crosses the UI and admin API.
pub fn server_fingerprint(public_key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"gamelink server fingerprint v1\n");
    hash.update(public_key.trim().to_ascii_lowercase().as_bytes());
    format!("{:x}", hash.finalize())
}

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
    // Legacy "dismissed" entries mixed approvals and dismissals. Ignore them
    // on load and persist only dismissals made under the rotation policy.
    #[serde(default, rename = "dismissedUntilRotation")]
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

    /// Removes a listed request without remembering its key, after approval.
    pub fn remove(&mut self, game: &str, id: u64) -> String {
        let requests = self.games.get_mut(game).expect("listed game");
        let index = requests
            .pending
            .iter()
            .position(|r| r.id == id)
            .expect("listed request");
        requests.pending.remove(index).knock.public_key
    }

    /// Removes a listed request and ignores its key until rotation.
    pub fn dismiss(&mut self, game: &str, id: u64, now: DateTime<Utc>) {
        let key = self.remove(game, id);
        let requests = self.games.get_mut(game).expect("listed game");
        if requests.dismissed.len() >= DISMISSED_LIMIT {
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
            match scopes.get(game) {
                // A new identity means the invitation was rotated.
                Some(scope) if scope.identity != requests.identity => return false,
                Some(scope) => requests.pending.retain(|r| active(&r.knock, scope, now)),
                // Keep dismissals while a game is paused or briefly unreadable.
                None => requests
                    .pending
                    .retain(|r| now - r.knock.first_seen < LIFETIME),
            }
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
                if !active(&knock, scope, now) || requests.dismissed.contains_key(&knock.public_key)
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

    /// A copy holding only dismissals, for hosts which keep pending requests
    /// in memory but must remember dismissals across restarts.
    pub fn dismissals(&self) -> Self {
        let mut out = self.clone();
        out.games.retain(|_, requests| {
            requests.pending.clear();
            !requests.dismissed.is_empty()
        });
        out
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
        // The host clears its batch every poll, so the same key knocks again
        // with fresh timestamps. A dismissal holds until rotation, even after
        // the game was paused and a day has passed.
        let again = later + LIFETIME + TimeDelta::seconds(1);
        store
            .merge(&BTreeMap::new(), &running, BTreeMap::new(), again)
            .unwrap();
        store
            .merge(&scopes, &running, batch(vec![knock(0, again)]), again)
            .unwrap();
        assert!(store
            .list(&scopes, again)
            .iter()
            .all(|r| r["publicKey"] != "peer-0"));
        let restored = store.dismissals();
        assert!(restored.games[&game].pending.is_empty());
        assert!(restored.games[&game].dismissed.contains_key("peer-0"));

        // Approval removes a request without ignoring the key later.
        store
            .merge(&scopes, &running, batch(vec![knock(1, again)]), again)
            .unwrap();
        let id = store.list(&scopes, again)[0]["id"].as_u64().unwrap();
        assert_eq!(store.remove(&game, id), "peer-1");
        let after = again + TimeDelta::seconds(1);
        store
            .merge(&scopes, &running, batch(vec![knock(1, after)]), after)
            .unwrap();
        assert_eq!(store.list(&scopes, after)[0]["publicKey"], "peer-1");

        let rotated = Scope::new(&json!({}), json!({"privateKey":"new"}));
        let running = BTreeMap::from([(game.clone(), rotated.identity.clone())]);
        scopes.insert(game.clone(), rotated);
        store
            .merge(&scopes, &running, batch(vec![knock(0, after)]), after)
            .unwrap();
        let listed = store.list(&scopes, after);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["publicKey"], "peer-0");
    }

    #[test]
    fn legacy_approval_markers_do_not_suppress_revoked_devices() {
        let now = DateTime::<Utc>::from(std::time::SystemTime::now());
        let game = "game".to_owned();
        let scope = Scope::new(&json!({}), json!({"privateKey":"old"}));
        let running = BTreeMap::from([(game.clone(), scope.identity.clone())]);
        let legacy = json!({
            "next_id": 42,
            "games": {
                "game": {
                    "identity": scope.identity,
                    "pending": [{"id":42,"publicKey":"peer-1","firstSeen":now,"lastSeen":now,"attempts":1}],
                    "dismissed": {"peer-0": now}
                }
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("requests.json");
        std::fs::write(&path, legacy.to_string()).unwrap();
        let mut store: Store = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let scopes = BTreeMap::from([(game.clone(), scope)]);
        let later = now + TimeDelta::seconds(1);
        store
            .merge(
                &scopes,
                &running,
                BTreeMap::from([(game, vec![knock(0, later)])]),
                later,
            )
            .unwrap();
        let requests = store.list(&scopes, later);
        assert_eq!(
            requests.len(),
            2,
            "a revoked device must be able to knock again"
        );
        assert_eq!(requests[0]["publicKey"], "peer-1");
        assert_eq!(requests[0]["id"], 42);
        assert_eq!(requests[1]["publicKey"], "peer-0");
        assert_eq!(requests[1]["id"], 43);
    }

    #[test]
    fn server_fingerprints_are_stable_and_hide_the_key() {
        let key = format!("nodekey:{}", "ab".repeat(32));
        let print = server_fingerprint(&key);
        assert_eq!(print, server_fingerprint(&key.to_ascii_uppercase()));
        assert_eq!(print.len(), 64);
        assert!(!print.contains("abab"));
        assert_ne!(
            print,
            server_fingerprint(&format!("nodekey:{}", "cd".repeat(32)))
        );
    }
}
