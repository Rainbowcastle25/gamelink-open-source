//! Frozen deterministic results produced by Go at 6c89f64 before parser removal.
use base64::{engine::general_purpose::STANDARD, Engine};
use gamelink_core::{console, discovery};
use serde_json::{json, Value};
fn ad_strings(v: &mut Value) {
    for key in ["ServerName", "Version", "LevelName", "Nonce"] {
        v[key] = json!(STANDARD.encode(v[key].as_str().unwrap().as_bytes()));
    }
}
#[test]
fn original_go_wire_fixtures() {
    for (name, raw) in [
        (
            "console",
            include_str!("fixtures/gamelink-console-fixtures.json"),
        ),
        (
            "discovery",
            include_str!("fixtures/gamelink-discovery-fixtures.json"),
        ),
    ] {
        let fixtures: Vec<Value> = serde_json::from_str(raw).unwrap();
        for (i, fixture) in fixtures.into_iter().enumerate() {
            let method = fixture["method"].as_str().unwrap();
            let mut input = fixture["input"].clone();
            let mut expected = fixture["output"].clone();
            if method == "discovery.ad_encode" {
                ad_strings(&mut input);
            }
            if method == "discovery.ad_decode" && fixture["error"] == false {
                ad_strings(&mut expected);
            }
            let got = if name == "console" {
                console::dispatch(method, input)
            } else {
                discovery::dispatch(method, input)
            };
            if fixture["error"] == true {
                assert!(got.is_err(), "{name}[{i}] {method}: {got:?}");
            } else {
                assert_eq!(
                    got.unwrap_or_else(|e| panic!("{name}[{i}] {method}: {e}")),
                    expected,
                    "{name}[{i}] {method}"
                );
            }
        }
    }
}
#[test]
fn candidate_limits_and_pinned_media_policy() {
    let base = "a=candidate:1 1 UDP 1 192.0.2.4 4000 typ host\r\n";
    let input = |data: &[u8]| json!({"data":STANDARD.encode(data),"allowed":"192.0.2.4","first":4000,"last":4010,"relay":"192.0.2.8:5000"});
    for data in [
        base.repeat(33),
        "x".repeat(65537),
        format!("{}{}", base, "a=remote-candidates:1 192.0.2.4 4000\r\n"),
    ] {
        assert!(console::dispatch("console.candidates_map", input(data.as_bytes())).is_err());
    }
    let raw = [b"opaque\xff\r\n".as_slice(), base.as_bytes()].concat();
    let got = console::dispatch("console.candidates_map", input(&raw)).unwrap();
    assert!(STANDARD
        .decode(got["data"].as_str().unwrap())
        .unwrap()
        .starts_with(b"opaque\xff\r\n"));
    for (first, last, expected) in [
        (4000, 4255, true),
        (4000, 4256, false),
        (0, 10, false),
        (4000, 3999, false),
    ] {
        let got = console::dispatch(
            "console.host_config",
            json!({"server":"192.0.2.4","bind":"192.0.2.8","first":first,"last":last}),
        )
        .unwrap();
        assert_eq!(got, json!(expected));
    }
}
#[test]
fn control_limits_unknown_fields_and_duplicate_order() {
    for raw in [
        r#"{"version":1,"type":"hello","bogus":1}"#,
        r#"{"version":1,"type":"hello"} {}"#,
        r#"{"version":1,"type":"offer","session":0}"#,
    ] {
        assert!(console::dispatch(
            "console.control_decode",
            json!({"data":STANDARD.encode(raw)})
        )
        .is_err());
    }
    let raw = r#"{"version":1,"TYPE":"offer","SESSION":1,"session":18446744073709551615,"data":"first","DATA":null}"#;
    let got = console::dispatch(
        "console.control_decode",
        json!({"data":STANDARD.encode(raw)}),
    )
    .unwrap();
    assert_eq!(
        got,
        json!({"version":1,"type":"offer","session":u64::MAX,"data":"first"})
    );
    assert!(console::dispatch(
        "console.control_encode",
        json!({"version":1,"type":"offer","session":1,"data":"x".repeat(65537)})
    )
    .is_err());
    // Valid data can still exceed the encoded frame cap after JSON escaping.
    assert!(console::dispatch(
        "console.control_encode",
        json!({"version":1,"type":"offer","session":1,"data":"\0".repeat(30000)})
    )
    .is_err());
}
