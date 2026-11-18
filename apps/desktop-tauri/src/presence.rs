//! In-memory observations and rate limits for opt-in desktop notifications.
use std::{
    collections::{BTreeMap, VecDeque},
    hash::{BuildHasher, RandomState},
    time::{Duration, Instant},
};

const SERVER_COOLDOWN: Duration = Duration::from_secs(600);
const NOTICE_SPACING: Duration = Duration::from_secs(15);
const PROBE_SPACING: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Observation {
    online: Option<bool>,
    last_notice: Option<Instant>,
}

#[derive(Default)]
pub struct Presence {
    next_round: Option<Instant>,
    next_probe: Option<Instant>,
    pending: VecDeque<String>,
    observed: BTreeMap<String, Observation>,
    notices: VecDeque<String>,
    last_notice: Option<Instant>,
    pub active: Option<(String, u64)>,
}

impl Presence {
    pub fn set_enabled(&mut self, enabled: bool, now: Instant) -> Option<(String, u64)> {
        if !enabled {
            let active = self.active.take();
            *self = Self::default();
            return active;
        }
        self.next_round.get_or_insert_with(|| now + interval());
        None
    }

    pub fn schedule<'a>(&mut self, now: Instant, ids: impl Iterator<Item = &'a str>) {
        if self.next_round.is_some_and(|due| now >= due) && self.pending.is_empty() {
            self.next_round = Some(now + interval());
            self.pending.extend(ids.map(str::to_owned));
        }
    }

    pub fn next_probe(&mut self, now: Instant) -> Option<String> {
        if self.next_probe.is_some_and(|due| now < due) {
            return None;
        }
        let id = self.pending.pop_front()?;
        self.next_probe = Some(now + PROBE_SPACING);
        Some(id)
    }

    pub fn observe(&mut self, id: &str, online: bool, now: Instant) {
        if self.next_round.is_none() {
            return;
        }
        let observed = self.observed.entry(id.into()).or_default();
        if online
            && observed.online == Some(false)
            && observed
                .last_notice
                .is_none_or(|last| now.duration_since(last) >= SERVER_COOLDOWN)
        {
            self.notices.push_back(id.into());
        } else if !online {
            self.notices.retain(|queued| queued != id);
        }
        observed.online = Some(online);
    }

    pub fn take_notice(&mut self, now: Instant) -> Option<String> {
        if self
            .last_notice
            .is_some_and(|last| now.duration_since(last) < NOTICE_SPACING)
        {
            return None;
        }
        let id = self.notices.pop_front()?;
        self.last_notice = Some(now);
        self.observed.get_mut(&id).unwrap().last_notice = Some(now);
        Some(id)
    }

    pub fn remove(&mut self, id: &str) {
        self.pending.retain(|queued| queued != id);
        self.notices.retain(|queued| queued != id);
        self.observed.remove(id);
    }
}

fn interval() -> Duration {
    Duration::from_secs(105 + RandomState::new().hash_one(0) % 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baselines_transitions_and_both_rate_limits() {
        let start = Instant::now();
        let mut presence = Presence::default();
        presence.observe("a", false, start);
        presence.set_enabled(true, start);
        presence.observe("a", true, start);
        assert_eq!(presence.take_notice(start), None);
        for id in ["a", "b"] {
            presence.observe(id, false, start);
            presence.observe(id, true, start);
            presence.observe(id, true, start);
        }
        assert_eq!(presence.take_notice(start).as_deref(), Some("a"));
        assert_eq!(presence.take_notice(start), None);
        let later = start + NOTICE_SPACING;
        assert_eq!(presence.take_notice(later).as_deref(), Some("b"));
        presence.observe("a", false, later);
        presence.observe("a", true, later);
        assert_eq!(presence.take_notice(start + SERVER_COOLDOWN), None);
        let later = start + SERVER_COOLDOWN;
        presence.observe("a", false, later);
        presence.observe("a", true, later);
        assert_eq!(presence.take_notice(later).as_deref(), Some("a"));
    }

    #[test]
    fn disabling_forgets_pending_work_and_returns_owned_probe() {
        let start = Instant::now();
        let mut presence = Presence::default();
        presence.set_enabled(true, start);
        presence.schedule(start, ["a", "b"].into_iter());
        assert_eq!(presence.next_probe(start), None);
        let due = start + Duration::from_secs(135);
        presence.schedule(due, ["a", "b"].into_iter());
        assert_eq!(presence.next_probe(due).as_deref(), Some("a"));
        assert_eq!(presence.next_probe(due), None);
        presence.active = Some(("a".into(), 42));
        assert_eq!(presence.set_enabled(false, due), Some(("a".into(), 42)));
        assert_eq!(presence.next_probe(due + PROBE_SPACING), None);
    }
}
