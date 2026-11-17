//! Private durable storage. Secret envelopes keep the original gls1 format.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{fs, io::Write, path::Path};

// File reads are bounded before allocation as well as at the C ABI boundary.
pub fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    let mut data = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut data)?;
    if data.len() > 8 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "configuration file exceeds 8 MiB",
        ));
    }
    Ok(data)
}
pub fn ensure_dir(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| format!("create directory {}: {e}", path.display()))
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure_dir(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempfile_in(dir)
        .map_err(|e| format!("create temp file: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod temp file: {e}"))?;
    }
    tmp.write_all(data)
        .map_err(|e| format!("write temp file: {e}"))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| format!("sync temp file: {e}"))?;
    tmp.persist(path)
        .map_err(|e| format!("replace {}: {}", path.display(), e.error))?;
    Ok(())
}

pub fn seal(secret: &str) -> Result<String, String> {
    let data = protect(secret.as_bytes(), false).map_err(|e| format!("protect secret: {e}"))?;
    Ok(format!("gls1:{}", STANDARD.encode(data)))
}
pub fn unseal(sealed: &str) -> Result<String, String> {
    let raw = sealed
        .strip_prefix("gls1:")
        .ok_or("unrecognised secret format")?;
    // Go's base64 decoder ignores CR and LF.
    let raw = raw.replace(['\r', '\n'], "");
    let data = STANDARD
        .decode(raw)
        .map_err(|_| "protected data is corrupt")?;
    if data.is_empty() {
        return Err("protected data is corrupt".into());
    }
    let data = protect(&data, true).map_err(|e| format!("unprotect secret: {e}"))?;
    String::from_utf8(data).map_err(|_| "protected data is corrupt".into())
}
#[cfg(not(windows))]
fn protect(data: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}
#[cfg(windows)]
fn protect(data: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
    windows_dpapi::protect(data, decrypt)
}
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_dpapi {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    pub fn protect(data: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len().try_into().map_err(|_| "secret is too large")?,
            pbData: data.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        // DPAPI allocates output with LocalAlloc; copy before returning it to LocalFree.
        unsafe {
            let result = if decrypt {
                CryptUnprotectData(
                    &input,
                    None,
                    None,
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptProtectData(
                    &input,
                    None,
                    None,
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            result.map_err(|e| e.to_string())?;
            let bytes = if output.cbData == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
            };
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            Ok(bytes)
        }
    }
}

pub fn dispatch(method: &str, input: Value) -> Result<Value, String> {
    let path = Path::new(input["path"].as_str().unwrap_or(""));
    match method {
        "storage.ensure_dir" => {
            ensure_dir(path)?;
            Ok(Value::Null)
        }
        "storage.write" => {
            let data = STANDARD
                .decode(input["data"].as_str().unwrap_or(""))
                .map_err(|_| "invalid file data")?;
            write_atomic(path, &data)?;
            Ok(Value::Null)
        }
        "storage.read" => match read_bounded(path) {
            Ok(data) => Ok(json!({"data": STANDARD.encode(data), "missing":false})),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({"missing":true})),
            Err(e) => Err(e.to_string()),
        },
        "storage.seal" => Ok(json!(seal(input["value"].as_str().unwrap_or(""))?)),
        "storage.unseal" => Ok(json!(unseal(input["value"].as_str().unwrap_or(""))?)),
        "storage.identity_load" => {
            let data = match read_bounded(path) {
                Ok(data) => data,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(json!({"missing":true}))
                }
                Err(e) => return Err(e.to_string()),
            };
            let f: Value = serde_json::from_slice(&data).map_err(|_| {
                format!("identity file {} is corrupt: invalid JSON", path.display())
            })?;
            if !f.is_object()
                || (!f["created"].is_null()
                    && !f["created"]
                        .as_str()
                        .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()))
                || (!f["private_key"].is_null() && !f["private_key"].is_string())
            {
                return Err(format!(
                    "identity file {} is corrupt: invalid field type or timestamp",
                    path.display()
                ));
            }
            if f["version"] != 1 {
                return Err(format!(
                    "identity file {} has unsupported version {}",
                    path.display(),
                    f["version"].as_i64().unwrap_or(0)
                ));
            }
            let raw = unseal(f["private_key"].as_str().unwrap_or("")).map_err(|e| format!("identity file {} cannot be unlocked (was it created by another Windows user?): {e}", path.display()))?;
            Ok(json!({"privateKey":raw,"created":f["created"],"missing":false}))
        }
        "storage.identity_save" => {
            let f = json!({"version":1,"created":input["created"],"private_key":seal(input["privateKey"].as_str().unwrap_or(""))?});
            write_atomic(
                path,
                &serde_json::to_vec_pretty(&f).map_err(|e| e.to_string())?,
            )?;
            Ok(Value::Null)
        }
        _ => Err("unknown storage method".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn call(method: &str, input: Value) -> Result<Value, String> {
        dispatch(method, input)
    }
    fn b64(data: &[u8]) -> String {
        STANDARD.encode(data)
    }

    #[test]
    fn write_atomic_replaces_creates_parents_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("f.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1, "temp files left behind: {entries:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn concurrent_writers_never_tear_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("f"));
        let handles: Vec<_> = (0..20u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    // Replacing a file that another writer holds open may fail on
                    // Windows; only the final content has to be whole.
                    let _ = write_atomic(&path, &vec![b'a' + i; 5000]);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let data = fs::read(&*path).unwrap();
        assert_eq!(data.len(), 5000);
        assert!(data.iter().all(|b| *b == data[0]), "mixed writers");
    }

    #[test]
    fn write_atomic_reports_unreplaceable_targets_and_bad_parents() {
        let dir = tempfile::tempdir().unwrap();
        // Replacing a directory with a file fails and is reported, not panicked.
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        let error = write_atomic(&target, b"x").unwrap_err();
        assert!(error.starts_with("replace "), "{error}");
        // The temp file is cleaned up after the failure.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        // A regular file where the parent directory should be.
        let file = dir.path().join("file");
        fs::write(&file, b"x").unwrap();
        let error = write_atomic(&file.join("child"), b"x").unwrap_err();
        assert!(error.starts_with("create directory "), "{error}");
    }

    #[test]
    fn read_bounded_limits_size_and_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        assert_eq!(
            read_bounded(&path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        fs::write(&path, vec![1u8; 8 * 1024 * 1024]).unwrap();
        assert_eq!(read_bounded(&path).unwrap().len(), 8 * 1024 * 1024);
        fs::write(&path, vec![1u8; 8 * 1024 * 1024 + 1]).unwrap();
        let error = read_bounded(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "configuration file exceeds 8 MiB");
    }

    #[test]
    fn ensure_dir_is_recursive_idempotent_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        ensure_dir(&nested).unwrap();
        ensure_dir(&nested).unwrap();
        assert!(nested.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let file = dir.path().join("file");
        fs::write(&file, b"x").unwrap();
        let error = ensure_dir(&file.join("child")).unwrap_err();
        assert!(error.starts_with("create directory "), "{error}");
    }

    #[test]
    fn seal_round_trips_and_uses_the_gls1_envelope() {
        let secret = "privkey:abcdef0123456789";
        let sealed = seal(secret).unwrap();
        assert!(sealed.starts_with("gls1:"));
        assert_eq!(unseal(&sealed).unwrap(), secret);
        assert_ne!(sealed, secret);
    }

    #[test]
    fn unseal_rejects_malformed_envelopes() {
        for (bad, message) in [
            ("", "unrecognised secret format"),
            ("plain", "unrecognised secret format"),
            ("GLS1:abcd", "unrecognised secret format"),
            ("gls1:!!!", "protected data is corrupt"),
            ("gls1:", "protected data is corrupt"),
            ("gls1:\r\n", "protected data is corrupt"),
        ] {
            assert_eq!(unseal(bad).unwrap_err(), message, "{bad:?}");
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn unseal_ignores_line_breaks_like_go_and_rejects_invalid_utf8() {
        let sealed = seal("secret value").unwrap();
        let (head, tail) = sealed.split_at(8);
        assert_eq!(
            unseal(&format!("{head}\r\n{tail}\n")).unwrap(),
            "secret value"
        );
        // Off Windows the envelope is plain base64, so invalid UTF-8 surfaces.
        assert_eq!(
            unseal(&format!("gls1:{}", b64(&[0xff, 0xfe]))).unwrap_err(),
            "protected data is corrupt"
        );
    }

    #[test]
    fn dispatch_ensure_dir_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("x").join("y");
        assert_eq!(
            call("storage.ensure_dir", json!({"path": sub})).unwrap(),
            Value::Null
        );
        assert!(sub.is_dir());

        let file = sub.join("data.bin");
        assert_eq!(
            call("storage.read", json!({"path": file})).unwrap(),
            json!({"missing": true})
        );
        call(
            "storage.write",
            json!({"path": file, "data": b64(b"\x00\x01hello")}),
        )
        .unwrap();
        assert_eq!(
            call("storage.read", json!({"path": file})).unwrap(),
            json!({"data": b64(b"\x00\x01hello"), "missing": false})
        );
        assert_eq!(
            call("storage.write", json!({"path": file, "data": "!!!"})).unwrap_err(),
            "invalid file data"
        );
        // Missing data decodes as empty and truncates, as Go's nil slice did.
        call("storage.write", json!({"path": file})).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"");
        // A directory cannot be read as a file: not NotFound, so it is an error.
        assert!(call("storage.read", json!({"path": sub})).is_err());
        assert_eq!(
            call("storage.nope", json!({})).unwrap_err(),
            "unknown storage method"
        );
    }

    #[test]
    fn dispatch_seal_and_unseal() {
        let sealed = call("storage.seal", json!({"value": "tcSECRET"})).unwrap();
        let sealed = sealed.as_str().unwrap();
        assert!(sealed.starts_with("gls1:"));
        assert_eq!(
            call("storage.unseal", json!({"value": sealed})).unwrap(),
            json!("tcSECRET")
        );
        assert_eq!(
            call("storage.unseal", json!({"value": "plain"})).unwrap_err(),
            "unrecognised secret format"
        );
    }

    fn identity_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("identity.json")
    }

    #[test]
    fn identity_save_then_load_round_trips_and_is_sealed() {
        let dir = tempfile::tempdir().unwrap();
        let path = identity_path(&dir);
        assert_eq!(
            call("storage.identity_load", json!({"path": path})).unwrap(),
            json!({"missing": true})
        );
        let created = "2026-01-02T03:04:05Z";
        call(
            "storage.identity_save",
            json!({"path": path, "privateKey": "privkey:0011", "created": created}),
        )
        .unwrap();
        let on_disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk["version"], 1);
        assert_eq!(on_disk["created"], created);
        assert!(on_disk["private_key"]
            .as_str()
            .unwrap()
            .starts_with("gls1:"));
        let loaded = call("storage.identity_load", json!({"path": path})).unwrap();
        assert_eq!(
            loaded,
            json!({"privateKey": "privkey:0011", "created": created, "missing": false})
        );
        // A second "restart" returns the same identity.
        assert_eq!(
            call("storage.identity_load", json!({"path": path})).unwrap(),
            loaded
        );
    }

    #[test]
    fn identity_load_rejects_corrupt_files_without_replacing_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = identity_path(&dir);
        let sealed = seal("privkey:aa").unwrap();
        let cases: Vec<(String, &str)> = vec![
            ("{not json".into(), "invalid JSON"),
            ("[]".into(), "invalid field type or timestamp"),
            (
                json!({"version":1,"created":"yesterday","private_key":sealed}).to_string(),
                "invalid field type or timestamp",
            ),
            (
                json!({"version":1,"created":5,"private_key":sealed}).to_string(),
                "invalid field type or timestamp",
            ),
            (
                json!({"version":1,"private_key":5}).to_string(),
                "invalid field type or timestamp",
            ),
        ];
        for (data, reason) in cases {
            fs::write(&path, &data).unwrap();
            let error = call("storage.identity_load", json!({"path": path})).unwrap_err();
            assert!(error.contains("is corrupt"), "{data}: {error}");
            assert!(error.contains(reason), "{data}: {error}");
            assert_eq!(fs::read_to_string(&path).unwrap(), data);
        }
    }

    #[test]
    fn identity_load_rejects_wrong_version_and_unopenable_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = identity_path(&dir);
        let sealed = seal("privkey:aa").unwrap();
        fs::write(&path, json!({"version":2,"private_key":sealed}).to_string()).unwrap();
        let error = call("storage.identity_load", json!({"path": path})).unwrap_err();
        assert!(error.contains("has unsupported version 2"), "{error}");
        fs::write(&path, json!({"private_key":sealed}).to_string()).unwrap();
        let error = call("storage.identity_load", json!({"path": path})).unwrap_err();
        assert!(error.contains("has unsupported version 0"), "{error}");
        fs::write(
            &path,
            json!({"version":1,"private_key":"gls1:!!!"}).to_string(),
        )
        .unwrap();
        let error = call("storage.identity_load", json!({"path": path})).unwrap_err();
        assert!(error.contains("cannot be unlocked"), "{error}");
        assert!(error.contains("protected data is corrupt"), "{error}");
        fs::write(&path, json!({"version":1}).to_string()).unwrap();
        let error = call("storage.identity_load", json!({"path": path})).unwrap_err();
        assert!(error.contains("unrecognised secret format"), "{error}");
        // An unreadable path (a directory) is an I/O error, not "missing".
        let error = call("storage.identity_load", json!({"path": dir.path()})).unwrap_err();
        assert!(!error.contains("corrupt"), "{error}");
    }

    #[test]
    fn identity_save_reports_write_failures() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"x").unwrap();
        assert!(call(
            "storage.identity_save",
            json!({"path": blocker.join("identity.json"), "privateKey": "privkey:aa", "created": null}),
        )
        .is_err());
    }

    #[cfg(windows)]
    mod dpapi {
        use super::*;

        fn payload(sealed: &str) -> Vec<u8> {
            STANDARD
                .decode(sealed.strip_prefix("gls1:").unwrap())
                .unwrap()
        }

        #[test]
        fn seal_round_trips_through_dpapi() {
            let secret = "privkey:abcdef0123456789";
            let sealed = seal(secret).unwrap();
            assert!(sealed.starts_with("gls1:"));
            assert_eq!(unseal(&sealed).unwrap(), secret);
        }

        #[test]
        fn sealed_payload_is_encrypted_not_plaintext_base64() {
            let secret = "tcSECRETADDRESSVALUE";
            let data = payload(&seal(secret).unwrap());
            assert!(!data.windows(secret.len()).any(|w| w == secret.as_bytes()));
            // DPAPI blobs start with the 0x01000000 version header.
            assert_eq!(&data[..4], &[1, 0, 0, 0]);
        }

        #[test]
        fn a_plain_non_windows_envelope_is_rejected_on_windows() {
            let plain = format!("gls1:{}", STANDARD.encode(b"privkey:abcdef"));
            let error = unseal(&plain).unwrap_err();
            assert!(error.starts_with("unprotect secret: "), "{error}");
        }

        #[test]
        fn tampered_blob_fails_to_unprotect() {
            let mut data = payload(&seal("privkey:abcdef").unwrap());
            let last = data.len() - 1;
            data[last] ^= 0xff;
            let error = unseal(&format!("gls1:{}", STANDARD.encode(&data))).unwrap_err();
            assert!(error.starts_with("unprotect secret: "), "{error}");
            data.truncate(8);
            assert!(unseal(&format!("gls1:{}", STANDARD.encode(&data))).is_err());
        }

        #[test]
        fn two_seals_of_one_secret_both_open() {
            let first = seal("same").unwrap();
            let second = seal("same").unwrap();
            assert_eq!(unseal(&first).unwrap(), "same");
            assert_eq!(unseal(&second).unwrap(), "same");
        }

        #[test]
        fn line_wrapped_envelope_still_opens() {
            let sealed = seal("wrapped secret").unwrap();
            let (head, tail) = sealed.split_at(20);
            assert_eq!(
                unseal(&format!("{head}\r\n{tail}\n")).unwrap(),
                "wrapped secret"
            );
        }

        #[test]
        fn identity_file_stores_a_dpapi_protected_key() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("identity.json");
            call(
                "storage.identity_save",
                json!({"path": path, "privateKey": "privkey:0011", "created": "2026-01-02T03:04:05Z"}),
            )
            .unwrap();
            let raw = fs::read_to_string(&path).unwrap();
            assert!(!raw.contains("privkey:0011"));
            let loaded = call("storage.identity_load", json!({"path": path})).unwrap();
            assert_eq!(loaded["privateKey"], "privkey:0011");
        }

        #[test]
        fn write_atomic_replaces_existing_files_on_windows() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            write_atomic(&path, b"one").unwrap();
            write_atomic(&path, b"two").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"two");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
}
