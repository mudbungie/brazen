//! The `bz --list-providers` CAPABILITY columns (config §6.1): `tuning` — which
//! request knobs the row accepts — and `device` — which headless sign-in it serves.
//! Both are COMPUTED from the row and never stored, and both are severable in the
//! same direction: delete the row datum, lose the column's claim, edit no code. The
//! rest of the listing lives in `list_providers`; the shared harness in
//! `list_providers_support`.

use crate::testing::MemoryCredStore;
use crate::tests::list_providers_support::{floor, floor_argv, go, row};
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
    assert_eq!(row(&out.stdout, "sso")[5], "rfc8628");
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

/// The column bl-5053 exists for. The built-in `claude-code` row's dialect carries
/// NEITHER shape — `encode` rejects tool declarations and multi-turn transcripts with
/// `ParseInput`/64 (claude-code §4.1, §4.2) — and until this column that refusal was
/// visible only at call time, to a caller that had already built the request. Every
/// other built-in row carries both, so a host picking a row for a tool-bearing worker
/// can now refuse this one at SELECTION time.
#[test]
fn the_single_turn_toolless_dialect_is_listed_as_carrying_neither_shape() {
    let out = floor(&MemoryCredStore::new());
    assert_eq!(row(&out.stdout, "claude-code")[4], "-");
    assert_eq!(row(&out.stdout, "anthropic")[4], "tools,multi_turn");
    let json = floor_argv(&["--list-providers", "--json"]);
    let v: serde_json::Value = serde_json::from_str(json.stdout.trim()).unwrap();
    let by_name = |n: &str| {
        v["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == n)
            .unwrap()
            .clone()
    };
    let cc = by_name("claude-code");
    assert_eq!(cc["tools"], false);
    assert_eq!(cc["multi_turn"], false);
    // The row still tunes: the two capability groups are independent reads, and a row
    // that takes `--reasoning` while refusing tools is exactly what this one is.
    assert_eq!(cc["effort"], true);
    let anthropic = by_name("anthropic");
    assert_eq!(anthropic["tools"], true);
    assert_eq!(anthropic["multi_turn"], true);
}

/// The shape facts take NO row operand, and this pins that. `unsupported_body_keys` is
/// a STRIP (config §4.1.1): naming `tools` there would have to silently drop the
/// declaration, which arch §3.1 forbids, so it declines nothing and the column keeps
/// reporting the dialect's answer. The asymmetry with `effort`/`priority` above is the
/// design, not an omission — there is no honest per-row decline to read.
#[test]
fn a_row_cannot_decline_a_shape_the_way_it_declines_a_knob() {
    let cfg = temp(&DECLINING_ROW.replace(
        "unsupported_body_keys = [\"reasoning\", \"service_tier\"]",
        "unsupported_body_keys = [\"tools\", \"multi_turn\"]",
    ));
    let out = go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &MemoryCredStore::new(),
    );
    assert_eq!(row(&out.stdout, "plain")[3], "effort,priority");
    assert_eq!(row(&out.stdout, "plain")[4], "tools,multi_turn");
}
