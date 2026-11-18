use crate::{bridge, config, core, json, now, pure, rows, s, Cli, Result};
use chrono::Datelike;
use serde_json::Value;
use std::path::Path;

pub fn load_keys(dir: &Path, id: &str) -> Result<Value> {
    pure::dispatch("security.slug", json!({"text":id}))?;
    let loaded = config::dispatch(
        "config.keys_load",
        json!({"id":id,"path":dir.join("keys").join(format!("{id}.json"))}),
    )?;
    if loaded["missing"] == true {
        return Err(format!("keys for {id:?} are missing"));
    }
    Ok(loaded["keys"].clone())
}

fn invite_expiry(duration: &str) -> Result<String> {
    let invalid = || "invalid --expires duration; use a positive number followed by s, m, h or d";
    let (amount, unit) = duration
        .split_at_checked(duration.len().saturating_sub(1))
        .ok_or_else(invalid)?;
    let seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(invalid().into()),
    };
    if amount.is_empty() || !amount.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid().into());
    }
    let expiry = amount
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| n.checked_mul(seconds))
        .and_then(chrono::TimeDelta::try_seconds)
        .and_then(|d| chrono::Utc::now().checked_add_signed(d))
        .filter(|date| date.year() <= 9999)
        .ok_or_else(invalid)?;
    Ok(expiry.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

impl Cli {
    pub fn invite(&self, m: &clap::ArgMatches, rotate: bool) -> Result<()> {
        let id = m.get_one::<String>("game").ok_or("--game is required")?;
        let expiry = m
            .get_one::<String>("expires")
            .map(|s| invite_expiry(s))
            .transpose()?;
        let rotate = rotate || expiry.is_some();
        let cfg = self.load()?;
        let game = rows(&cfg["games"])
            .iter()
            .find(|g| g["id"] == *id)
            .ok_or_else(|| format!("no game with id {id:?}"))?;
        let expected_keys = load_keys(&self.dir, id)?;
        let mut keys = expected_keys.clone();
        if rotate {
            let fresh = bridge::call(json!({"method":"generate"}))?;
            keys["privateKey"] = fresh["privateKey"].clone();
            keys["presharedKey"] = fresh["presharedKey"].clone();
            keys["created"] = json!(now());
            keys["expiresAt"] = json!(expiry);
        }
        if pure::check_invite_expiry(&keys["expiresAt"]).is_err() {
            return Err(format!(
                "invitation expired; run `gamelink-agent invite rotate --game {id}`"
            ));
        }
        let addr = bridge::call(
            json!({"method":"address","keys":keys,"allowLocalRelay":m.get_flag("allow-local-relay")}),
        )?;
        let name = m
            .get_one::<String>("name")
            .filter(|n| !n.is_empty())
            .map_or(s(game, "name"), String::as_str);
        let note = m
            .get_one::<String>("note")
            .map(String::as_str)
            .unwrap_or("");
        let invitation = json!({"v":1,"name":name,"game":game["type"],"proto":game["protocol"],"port":game["tunnelPort"],"addr":addr,"note":note,"consoleBridge":if game["consoleBridge"].is_null(){0}else{1},"expiresAt":keys["expiresAt"]});
        core("invite.fields", invitation.clone())?;
        let encoded = core("invite.encode", invitation)?;
        let encoded = encoded.as_str().ok_or("invalid invitation response")?;
        self.mutate(
            "invite",
            json!({"id":id,"expectedGame":game,"expectedKeys":expected_keys,"keys":keys}),
        )?;
        println!("{encoded}");
        eprintln!("Share this invitation privately. Ask your friend to import it and click Join, then run `gamelink-agent requests list --game {id}`.");
        if rotate {
            eprintln!("Device approvals were kept. The running agent replaces the old identity and closes its sessions on the next config update.");
        }
        if let Some(expires_at) = keys["expiresAt"].as_str() {
            eprintln!("This game identity stops serving at {expires_at}. Rotate its invitation to resume access.");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_duration_parsing_accepts_hours_and_rejects_invalid_values() {
        let parsed = crate::parse(
            "invite rotate",
            ["--game", "one", "--expires", "24h"]
                .map(str::to_owned)
                .to_vec(),
        )
        .unwrap();
        let before = chrono::Utc::now();
        let expires = invite_expiry(parsed.get_one::<String>("expires").unwrap()).unwrap();
        let after = chrono::Utc::now();
        let expires = chrono::DateTime::parse_from_rfc3339(&expires).unwrap();
        assert!(expires >= before + chrono::TimeDelta::hours(24));
        assert!(expires <= after + chrono::TimeDelta::hours(24));
        for invalid in ["", "0h", "+1h", "-1h", "24", "1y", "24ä", "99999999999999d"] {
            assert!(invite_expiry(invalid).is_err());
        }
    }
}
