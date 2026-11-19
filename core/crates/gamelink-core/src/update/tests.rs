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
        checked_at: Some(checked_at),
        manifest,
        signature,
    }
}

fn quiet(_: &'static str) {}

fn verifier(key: &KeyPair) -> PublicKey {
    PublicKey::from_base64(&key.pk.to_base64()).unwrap()
}

#[test]
fn manifest_url_override_is_only_enabled_in_debug_builds() {
    const IN_CHILD: &str = "GAMELINK_TEST_MANIFEST_URL_OVERRIDE";
    const OVERRIDE: &str = "https://example.org/custom-manifest.json";
    if std::env::var_os(IN_CHILD).is_some() {
        let default_url = latest_manifest_url().unwrap();
        let expected = if cfg!(debug_assertions) {
            OVERRIDE
        } else {
            default_url.as_str()
        };
        assert_eq!(manifest_url().unwrap().as_str(), expected);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "update::tests::manifest_url_override_is_only_enabled_in_debug_builds",
        ])
        .env(IN_CHILD, "1")
        .env("GAMELINK_UPDATE_MANIFEST_URL", OVERRIDE)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn oversized_feed_is_rejected_while_reading_even_when_json_is_valid() {
    let mut json = String::from("{}");
    json.extend(std::iter::repeat_n(' ', MAX_MANIFEST - json.len()));
    assert_eq!(read_updater_feed(json.as_bytes()), Ok(json!({})));
    json.extend([' ', ' ']);
    let mut body = std::io::Cursor::new(json);
    assert_eq!(
        read_updater_feed(&mut body),
        Err("release metadata exceeds size limit")
    );
    assert_eq!(body.position(), MAX_MANIFEST as u64 + 1);
}

fn agent_manifest(name: &str, version: &str, binary: &[u8]) -> String {
    let mut value = manifest(version);
    value["artifacts"] = json!([{"name":name,"sha256":format!("{:x}", Sha256::digest(binary))}]);
    value.to_string()
}

#[test]
fn agent_update_requires_a_newer_signed_manifest_and_the_native_asset() {
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    let amd64 = "gamelink-agent-linux-amd64";
    for (signer, name, version, error) in [
        (
            &other,
            amd64,
            "2.0.0",
            "release signature verification failed",
        ),
        (
            &key,
            amd64,
            "1.0.0",
            "signed release is not newer than the installed agent",
        ),
        (
            &key,
            amd64,
            "0.9.0",
            "signed release is not newer than the installed agent",
        ),
        (
            &key,
            "gamelink-agent-linux-arm64",
            "2.0.0",
            "signed release has no agent for this architecture",
        ),
    ] {
        let signed = signed(signer, agent_manifest(name, version, b"binary"), 1);
        assert_eq!(
            AgentRelease::verify(signed, vec![verifier(&key)], "1.0.0", "x86_64").err(),
            Some(error)
        );
    }
}

#[test]
fn agent_update_checks_the_binary_pin_digest_size_and_elf_architecture() {
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    let (name, _) = agent_target("x86_64").unwrap();
    let sign = |key: &KeyPair, binary: &[u8]| {
        minisign::sign(Some(&key.pk), &key.sk, binary, None, None)
            .unwrap()
            .into_string()
    };
    let release = |binary: &[u8]| {
        AgentRelease::verify(
            signed(&key, agent_manifest(name, "2.0.0", binary), 1),
            vec![verifier(&key)],
            "1.0.0",
            "x86_64",
        )
        .unwrap()
    };
    let mut binary = [0; 64];
    binary[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    binary[18] = 62;
    let candidate = release(&binary);
    let signature = sign(&key, &binary);
    assert!(candidate.verify_binary(&binary, &signature).is_ok());
    assert_eq!(
        candidate.verify_binary(&binary, &sign(&other, &binary)),
        Err("release signature verification failed")
    );
    binary[63] = 1;
    assert_eq!(
        candidate.verify_binary(&binary, &signature),
        Err("agent binary does not match the signed SHA-256 digest")
    );
    binary[18] = 183;
    assert_eq!(
        release(&binary).verify_binary(&binary, &sign(&key, &binary)),
        Err("agent binary has the wrong Linux architecture")
    );
    assert_eq!(
        read_download(std::io::repeat(0), 8, "agent binary exceeds size limit"),
        Err("agent binary exceeds size limit")
    );
    assert_eq!(
        candidate.verify_binary(&vec![0; MAX_AGENT_BINARY + 1], &signature),
        Err("agent binary exceeds size limit")
    );
}

#[test]
fn staging_feed_and_redirects_use_https() {
    assert_eq!(
        latest_manifest_url().unwrap().as_str(),
        "https://github.com/Rainbowcastle25/gamelink-open-source/releases/latest/download/release-manifest.json"
    );
    for target in ["http://github.com/game", "https://user@github.com/game"] {
        assert!(https_url(target).is_err());
    }
}

fn corrupt_attempt_metadata(dir: &Path) {
    let path = dir.join(CACHE);
    let mut cache: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    cache["lastAttemptSucceeded"] = json!("damaged");
    storage::write_atomic(&path, &serde_json::to_vec(&cache).unwrap()).unwrap();
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
#[ignore = "requires a signed release manifest and compiled-in release pins"]
fn verifies_release_pipeline_manifest() {
    let dir = std::env::var_os("GAMELINK_TEST_SIGNED_RELEASE_DIR")
        .expect("set GAMELINK_TEST_SIGNED_RELEASE_DIR to the staged release directory");
    let dir = Path::new(&dir);
    let mut signed = SignedRelease {
        manifest: std::fs::read_to_string(dir.join("release-manifest.json")).unwrap(),
        signature: std::fs::read_to_string(dir.join("release-manifest.json.minisig")).unwrap(),
        checked_at: None,
    };
    let keys = keys().expect("compile with GAMELINK_UPDATE_PUBLIC_KEYS");
    verify(&signed, &keys).expect("verify the staged release manifest");

    signed.manifest.push(' ');
    assert_eq!(
        verify(&signed, &keys).unwrap_err(),
        "release signature verification failed"
    );
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
    corrupt_attempt_metadata(dir.path());
    assert_eq!(
        check("1.2.0", 3).unwrap_err(),
        "release manifest is older than the last verified release"
    );
    let cache = load_cache(dir.path(), quiet);
    assert!(!cache.last_attempt_succeeded);
    let retained = result(&installed, &cache.signed.unwrap(), keys).unwrap();
    assert_eq!(retained.release.version, "1.3.0");
}

#[test]
fn automatic_checks_keep_a_four_hour_limit_even_after_failed_attempts() {
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
    assert!(check(10 + DESKTOP_CHECK_INTERVAL - 1).unwrap().is_none());
    assert_eq!(
        check(10 + DESKTOP_CHECK_INTERVAL).unwrap_err(),
        "network failure"
    );
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
}

#[test]
fn signature_corruption_is_repaired_before_cooldown_expires() {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate_unencrypted_keypair().unwrap();
    let keys = &[verifier(&key)];
    let installed = Version::new(1, 0, 0);
    check_at(dir.path(), &installed, keys, true, 1, quiet, |now| {
        Ok(signed(&key, manifest("1.1.0").to_string(), now))
    })
    .unwrap();
    let mut cache = load_cache(dir.path(), quiet);
    cache.signed.as_mut().unwrap().signature = "damaged".into();
    save_cache(dir.path(), &cache).unwrap();
    let repaired = check_at(dir.path(), &installed, keys, true, 2, quiet, |now| {
        Ok(signed(&key, manifest("1.2.0").to_string(), now))
    })
    .unwrap()
    .unwrap();
    assert_eq!(repaired.release.version, "1.2.0");
    let cache = load_cache(dir.path(), quiet);
    assert_eq!(
        result(&installed, &cache.signed.unwrap(), keys)
            .unwrap()
            .release
            .version,
        "1.2.0"
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
        check_at(
            dir.path(),
            &installed,
            keys,
            true,
            1 + DESKTOP_CHECK_INTERVAL,
            quiet,
            |_| Err("offline")
        )
        .unwrap_err(),
        "offline"
    );
    let reused = check_at(
        dir.path(),
        &installed,
        keys,
        true,
        2 + DESKTOP_CHECK_INTERVAL,
        quiet,
        |_| panic!("not due"),
    )
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
    assert_eq!(
        check(10 * DESKTOP_CHECK_INTERVAL).unwrap_err(),
        "network failure"
    );
    assert_eq!(
        check(DESKTOP_CHECK_INTERVAL).unwrap_err(),
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
    corrupt_attempt_metadata(dir.path());
    assert_eq!(
        check(resigned.to_string(), 3).unwrap_err(),
        "release manifest changed after publication"
    );
    assert!(check(original, 4).is_ok(), "identical bytes stay accepted");
    let cache = load_cache(dir.path(), quiet);
    let kept = result(&installed, &cache.signed.unwrap(), keys).unwrap();
    assert_eq!(
        kept.release.release_notes_url,
        "https://example.org/releases/next"
    );
}
