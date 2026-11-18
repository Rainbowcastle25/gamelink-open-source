use super::*;
use minisign::KeyPair;
use serde_json::{json, Value};

fn manifest(version: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "version": version,
        "releaseDate": "2026-11-17",
        "severity": "security",
        "releaseNotesUrl": "https://example.org/releases/next",
        "artifacts": [{"name": "GameLink.exe", "sha256": "a".repeat(64)}]
    })
}

fn signed(key: &KeyPair, manifest: String, checked_at: u64) -> SignedRelease {
    let signature = minisign::sign(Some(&key.pk), &key.sk, manifest.as_bytes(), None, None)
        .unwrap()
        .into_string();
    SignedRelease {
        checked_at,
        manifest,
        signature,
    }
}

fn quiet(_: &'static str) {}

fn verifier(key: &KeyPair) -> PublicKey {
    PublicKey::from_base64(&key.pk.to_base64()).unwrap()
}

#[test]
fn valid_minisign_signature_and_overlapping_pins() {
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let next = KeyPair::generate_unencrypted_keypair().unwrap();
    let signed = signed(&key, manifest("1.0.0").to_string(), 1);
    let (release, version) = verify(&signed, &[verifier(&next), verifier(&key)]).unwrap();
    assert_eq!(version, Version::new(1, 0, 0));
    assert_eq!(release.severity, Severity::Security);
    assert_eq!(release.artifacts[0].sha256, "a".repeat(64));
}

#[test]
fn wrong_key_and_tampered_bytes_are_rejected_before_parsing() {
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    let mut signed = signed(&key, manifest("1.0.0").to_string(), 1);
    assert_eq!(
        verify(&signed, &[verifier(&other)]).unwrap_err(),
        "release signature verification failed"
    );
    signed.manifest.push('?');
    assert_eq!(
        verify(&signed, &[verifier(&key)]).unwrap_err(),
        "release signature verification failed"
    );
    signed.signature = String::from("invalid signature");
    assert_eq!(
        verify(&signed, &[verifier(&key)]).unwrap_err(),
        "invalid release signature"
    );
}

#[test]
fn signed_malformed_metadata_is_rejected() {
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    assert!(verify(&signed(&key, "{".into(), 1), keys).is_err());
    for (field, bad) in [
        ("schemaVersion", json!(2)),
        ("version", json!("v1.0")),
        ("releaseDate", json!("2026-02-29")),
        ("severity", json!("critical")),
        ("releaseNotesUrl", json!("http://example.org/")),
        ("releaseNotesUrl", json!("https://user:pass@example.org/")),
        ("artifacts", json!([])),
        (
            "artifacts",
            json!([{"name":"../GameLink.exe","sha256":"a".repeat(64)}]),
        ),
        (
            "artifacts",
            json!([{"name":"GameLink.exe","sha256":"z".repeat(64)}]),
        ),
    ] {
        let mut invalid = manifest("1.0.0");
        invalid[field] = bad;
        assert!(
            verify(&signed(&key, invalid.to_string(), 1), keys).is_err(),
            "{field}"
        );
    }
    let oversized = " ".repeat(MAX_MANIFEST + 1);
    assert!(verify(&signed(&key, oversized, 1), keys).is_err());
}

#[test]
fn orders_versions_by_semver_precedence() {
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::parse("1.2.0-alpha.2+local").unwrap();
    for (version, expected) in [
        ("1.2.0", true),
        ("1.2.0-alpha.10", true),
        ("1.10.0-alpha", true),
        ("1.2.0-alpha.2+build", false),
        ("1.2.0-alpha.1", false),
        ("1.1.9", false),
    ] {
        let signed = signed(&key, manifest(version).to_string(), 1);
        assert_eq!(
            result(&installed, &signed, keys).unwrap().available,
            expected
        );
    }
}

#[test]
fn replay_below_last_verified_version_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    let check = |version, now| {
        check_at(dir.path(), &installed, keys, false, now, quiet, |now| {
            Ok(signed(&key, manifest(version).to_string(), now))
        })
    };
    assert!(check("1.3.0", 1).unwrap().unwrap().available);
    assert_eq!(
        check("1.2.0", 2).unwrap_err(),
        "release manifest is older than the last verified release"
    );
    let cache = load_cache(dir.path(), quiet);
    assert!(!cache.last_attempt_succeeded);
    let retained = result(&installed, &cache.signed.unwrap(), keys).unwrap();
    assert_eq!(retained.release.version, "1.3.0");
}

#[test]
fn automatic_checks_keep_a_24_hour_limit_even_after_failed_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    let check = |now| {
        check_at(dir.path(), &installed, keys, true, now, quiet, |_| {
            Err("network failure")
        })
    };
    assert_eq!(check(10).unwrap_err(), "network failure");
    assert!(check(11).unwrap().is_none());
    assert!(check(10 + DAY - 1).unwrap().is_none());
    assert_eq!(check(10 + DAY).unwrap_err(), "network failure");
}

#[test]
fn automatic_checks_reuse_verified_result_until_due() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    let first = check_at(dir.path(), &installed, keys, true, 1, quiet, |now| {
        Ok(signed(&key, manifest("1.1.0").to_string(), now))
    })
    .unwrap()
    .unwrap();
    let second = check_at(dir.path(), &installed, keys, true, 2, quiet, |_| {
        panic!("early fetch")
    })
    .unwrap()
    .unwrap();
    assert_eq!(first.checked_at, second.checked_at);
    assert!(second.available);
    let mut cache = load_cache(dir.path(), quiet);
    cache.signed.as_mut().unwrap().manifest.push('?');
    save_cache(dir.path(), &cache).unwrap();
    assert!(
        check_at(dir.path(), &installed, keys, true, 3, quiet, |_| panic!(
            "early fetch"
        ))
        .is_err()
    );
}

#[test]
fn failed_attempt_keeps_returning_the_verified_notice() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    check_at(dir.path(), &installed, keys, true, 1, quiet, |now| {
        Ok(signed(&key, manifest("1.1.0").to_string(), now))
    })
    .unwrap();
    assert_eq!(
        check_at(dir.path(), &installed, keys, true, 1 + DAY, quiet, |_| Err(
            "offline"
        ))
        .unwrap_err(),
        "offline"
    );
    let reused = check_at(dir.path(), &installed, keys, true, 2 + DAY, quiet, |_| {
        panic!("not due")
    })
    .unwrap()
    .unwrap();
    assert!(reused.available);
    assert_eq!(reused.release.version, "1.1.0");
}

#[test]
fn last_attempt_in_the_future_is_due() {
    assert!(due(Some(20), 10));
    assert!(!due(Some(10), 11));
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let installed = Version::new(1, 0, 0);
    let check = |now| {
        check_at(
            dir.path(),
            &installed,
            &[verifier(&key)],
            true,
            now,
            quiet,
            |_| Err("network failure"),
        )
    };
    assert_eq!(check(10 * DAY).unwrap_err(), "network failure");
    assert_eq!(
        check(DAY).unwrap_err(),
        "network failure",
        "clock moved back"
    );
}

#[test]
fn corrupt_cache_is_treated_as_empty_and_repaired() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    fn debug(message: &'static str) {
        assert_eq!(message, "ignoring invalid release check cache");
        LOGGED.store(true, Ordering::SeqCst);
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CACHE), b"not json").unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    let checked = check_at(dir.path(), &installed, keys, true, 1, debug, |now| {
        Ok(signed(&key, manifest("1.1.0").to_string(), now))
    })
    .unwrap()
    .unwrap();
    assert!(checked.available);
    assert!(LOGGED.load(Ordering::SeqCst));
    let repaired = load_cache(dir.path(), |_| panic!("cache still corrupt"));
    assert_eq!(
        result(&installed, &repaired.signed.unwrap(), keys)
            .unwrap()
            .release
            .version,
        "1.1.0"
    );
}

#[test]
fn same_version_with_different_signed_bytes_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    let check = |manifest: String, now| {
        check_at(dir.path(), &installed, keys, false, now, quiet, |now| {
            Ok(signed(&key, manifest, now))
        })
    };
    let original = manifest("1.1.0").to_string();
    check(original.clone(), 1).unwrap();
    let mut resigned = manifest("1.1.0");
    resigned["releaseNotesUrl"] = json!("https://example.org/elsewhere");
    assert_eq!(
        check(resigned.to_string(), 2).unwrap_err(),
        "release manifest changed after publication"
    );
    assert!(check(original, 3).is_ok(), "identical bytes stay accepted");
    let cache = load_cache(dir.path(), quiet);
    let kept = result(&installed, &cache.signed.unwrap(), keys).unwrap();
    assert_eq!(
        kept.release.release_notes_url,
        "https://example.org/releases/next"
    );
}
