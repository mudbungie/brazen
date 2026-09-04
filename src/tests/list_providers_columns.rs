//! The `bz --list-providers` CAPABILITY columns (config §6.1): `tuning` — which
//! request knobs the row accepts — and `device` — which headless sign-in it serves.
//! Both are COMPUTED from the row and never stored, and both are severable in the
//! same direction: delete the row datum, lose the column's claim, edit no code. The
//! rest of the listing lives in `list_providers`; the shared harness in
//! `list_providers_support`.

use crate::testing::MemoryCredStore;
use crate::tests::list_providers_support::{go, row};
use crate::tests::run_support::temp;

const DECLINING_ROW: &str = r#"
[[provider]]
name = "plain"
base_url = "https://example.test"
protocol = "openai_chat"
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }
unsupported_body_keys = ["reasoning", "service_tier"]
"#;

/// The interesting bit is the per-ROW decline (config §4.1.1): the dialect projects
/// both knobs, this row refuses both, so the listing says so — the same `Vec<String>`
/// `strip_unsupported` reads on the data plane, never a second opinion. Severable:
/// deleting the row datum restores both, with no code edit.
#[test]
fn a_row_that_declines_a_knob_is_listed_as_declining_it() {
    let cfg = temp(DECLINING_ROW);
    let env = [("BRAZEN_CONFIG", cfg.0.to_str().unwrap())];
    let out = go(&["--list-providers"], &env, &MemoryCredStore::new());
    // Neither knob survives → the `-` cell, not an empty column.
    assert_eq!(row(&out.stdout, "plain")[3], "-");
    // The same row under the object shape, and a sibling that declines only one.
    let json = go(
        &["--list-providers", "--json"],
        &env,
        &MemoryCredStore::new(),
    );
    let v: serde_json::Value = serde_json::from_str(json.stdout.trim()).unwrap();
    let plain = v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "plain")
        .unwrap()
        .clone();
    assert_eq!(plain["effort"], false);
    assert_eq!(plain["priority"], false);
}

/// One decline, not both: the two facts are independent reads of one list.
#[test]
fn declining_one_knob_leaves_the_other_listed() {
    let cfg = temp(&DECLINING_ROW.replace(", \"service_tier\"", ""));
    let out = go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &MemoryCredStore::new(),
    );
    assert_eq!(row(&out.stdout, "plain")[3], "priority");
}

const DEVICE_ROW: &str = r#"
[[provider]]
name = "sso"
base_url = "https://example.test"
protocol = "openai_responses"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
oauth = { authorize_url = "https://a", token_url = "https://t", client_id = "c", device = { url = "https://d" } }
"#;

/// The `device` column is a DATA read of the row's own block (auth §7.3): a block
/// with no `style` is RFC 8628, and the column says which wire — never a bool, so a
/// consumer above brazen can branch its own login on the flow it will actually get.
/// Severable both ways: delete the block and the column reads `-`.
#[test]
fn a_row_declaring_a_device_endpoint_is_listed_with_its_flow_style() {
    let cfg = temp(DEVICE_ROW);
    let env = [("BRAZEN_CONFIG", cfg.0.to_str().unwrap())];
    let out = go(&["--list-providers"], &env, &MemoryCredStore::new());
    assert_eq!(row(&out.stdout, "sso")[4], "rfc8628");
    let json = go(
        &["--list-providers", "--json"],
        &env,
        &MemoryCredStore::new(),
    );
    let v: serde_json::Value = serde_json::from_str(json.stdout.trim()).unwrap();
    let sso = v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "sso")
        .unwrap()
        .clone();
    assert_eq!(sso["device"], "rfc8628");
}
