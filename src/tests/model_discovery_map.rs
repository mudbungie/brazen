//! The MAP-keyed listing (model-discovery §3, providers §4.10 CR-CC (2)): Cloud Code's
//! `POST :fetchAvailableModels` returns `{models:{"<id>":{…}}, defaultAgentModelId}`.
//! One datum (`id_key = ""`) tells the ONE generic decoder the ids are the map keys, and
//! `default_key` carries the body's named default; the shape's `method` makes the verb
//! POST. Pure decode tests plus one end-to-end `--list-models` on a `MockTransport`.

use crate::protocol::google_cloudcode::GoogleCloudCode;
use crate::protocol::{decode_models, Method, ModelKeys, ModelsShape};
use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::list_models_support::go;
use crate::tests::model_discovery_decode::{bare_keys, decode};
use crate::tests::run_support::temp;
use crate::{ErrorKind, Model, Protocol};

/// A trimmed live body (2026-09-28): keys out of order (the backend's order varies per
/// call), one entry with no label/output limit, and the top-level named default.
const BODY: &[u8] = br#"{"models":{
    "gemini-3.6-flash-high":{"displayName":"Gemini 3.6 Flash (High)","maxTokens":1048576,
        "maxOutputTokens":65536,"quotaInfo":{"remainingFraction":1}},
    "chat_20706":{"maxTokens":16384},
    "claude-sonnet-4-6":{"displayName":"Claude Sonnet 4.6 (Thinking)","maxTokens":250000,
        "maxOutputTokens":64000}
  },"defaultAgentModelId":"gemini-3.6-flash-high","tabModelIds":["chat_20706"]}"#;

#[test]
fn cloudcode_shape_is_a_bodiless_post_over_a_map_with_a_named_default() {
    assert_eq!(
        GoogleCloudCode.models_shape(),
        Some(ModelsShape {
            method: Method::Post,
            path: "/v1internal:fetchAvailableModels",
            keys: ModelKeys {
                array_key: "models",
                id_key: "",
                strip: "",
                context_key: "maxTokens",
                max_output_key: "maxOutputTokens",
                display_name_key: "displayName",
                default_key: "defaultAgentModelId",
            },
        })
    );
}

#[test]
fn a_map_decodes_in_key_order_with_metadata_and_the_named_default() {
    // A map has no order, so ids come out in key order (deterministic); the named
    // default is the ONE `default`; absent metadata stays `None`.
    let got = decode(&GoogleCloudCode, BODY).unwrap();
    let meta = |id: &str, default, cw, mo, dn: Option<&str>| Model {
        id: id.into(),
        default,
        context_window: cw,
        max_output_tokens: mo,
        display_name: dn.map(Into::into),
    };
    assert_eq!(
        got,
        [
            meta("chat_20706", false, Some(16384), None, None),
            meta(
                "claude-sonnet-4-6",
                false,
                Some(250000),
                Some(64000),
                Some("Claude Sonnet 4.6 (Thinking)")
            ),
            meta(
                "gemini-3.6-flash-high",
                true,
                Some(1048576),
                Some(65536),
                Some("Gemini 3.6 Flash (High)")
            ),
        ]
    );
}

#[test]
fn a_named_default_matching_no_id_flags_nothing() {
    let body = br#"{"models":{"a":{},"b":{}},"defaultAgentModelId":"gone"}"#;
    let got = decode(&GoogleCloudCode, body).unwrap();
    assert!(got.iter().all(|m| !m.default));
    // …and on an ARRAY shape the default_key works the same way (it is general).
    let keys = ModelKeys {
        default_key: "default",
        ..bare_keys("data", "id", "")
    };
    let got = decode_models(br#"{"data":[{"id":"x"},{"id":"y"}],"default":"y"}"#, &keys);
    assert_eq!(
        got.unwrap().iter().map(|m| m.default).collect::<Vec<_>>(),
        [false, true]
    );
}

#[test]
fn a_collection_of_the_wrong_shape_is_a_provider_error() {
    // An ARRAY where the map is named, and a MAP where an array is named → 502.
    let err = decode(&GoogleCloudCode, br#"{"models":[{"name":"x"}]}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Provider { status: 502 });
    assert!(err.message.contains("no `models` map"), "{}", err.message);
    let keys = bare_keys("models", "name", "");
    let err = decode_models(br#"{"models":{"x":{}}}"#, &keys).unwrap_err();
    assert!(err.message.contains("no `models` array"), "{}", err.message);
}

#[test]
fn list_models_on_a_cloudcode_row_posts_with_no_body_and_marks_the_default() {
    let cfg = temp(
        r#"
[[provider]]
name = "ag"
base_url = "https://cloudcode.test"
protocol = "google_cloudcode"
auth = "none"
"#,
    );
    let path = cfg.0.to_str().unwrap();
    let tx = MockTransport::ok(vec![BODY]);
    let argv = ["--list-models", "--provider", "ag", "--config", path];
    let o = go(&argv, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        o.stdout,
        "chat_20706\nclaude-sonnet-4-6\ngemini-3.6-flash-high (default)\n"
    );
    let sent = tx.requests();
    assert_eq!(sent[0].method, Method::Post);
    assert_eq!(
        sent[0].url,
        "https://cloudcode.test/v1internal:fetchAvailableModels"
    );
    assert!(sent[0].body.is_empty());
}
