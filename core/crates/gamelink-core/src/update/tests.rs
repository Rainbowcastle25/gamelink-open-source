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
        check_at(dir.path(), &installed, keys, false, now, |now| {
            Ok(signed(&key, manifest(version).to_string(), now))
        })
    };
    assert!(check("1.3.0", 1).unwrap().unwrap().available);
    assert_eq!(
        check("1.2.0", 2).unwrap_err(),
        "release manifest is older than the last verified release"
    );
    let cache = load_cache(dir.path()).unwrap();
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
        check_at(dir.path(), &installed, keys, true, now, |_| {
            Err("network failure")
        })
    };
    assert_eq!(check(10).unwrap_err(), "network failure");
    assert!(check(9).unwrap().is_none(), "clock moved backwards");
    assert!(check(10 + DAY - 1).unwrap().is_none());
    assert_eq!(check(10 + DAY).unwrap_err(), "network failure");
}

#[test]
fn automatic_checks_reuse_verified_result_until_due() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    let first = check_at(dir.path(), &installed, keys, true, 1, |now| {
        Ok(signed(&key, manifest("1.1.0").to_string(), now))
    })
    .unwrap()
    .unwrap();
    let second = check_at(dir.path(), &installed, keys, true, 2, |_| {
        panic!("early fetch")
    })
    .unwrap()
    .unwrap();
    assert_eq!(first.checked_at, second.checked_at);
    assert!(second.available);
    let mut cache = load_cache(dir.path()).unwrap();
    cache.signed.as_mut().unwrap().manifest.push('?');
    save_cache(dir.path(), &cache).unwrap();
    assert!(check_at(dir.path(), &installed, keys, true, 3, |_| panic!(
        "early fetch"
    ))
    .is_err());
}
