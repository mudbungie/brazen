//! XdgModelCache IO invariants (model-discovery §8) — the XDG-file `ModelCache` impl,
//! coverage-excluded with the rest of the `bz` shim, so these pin its load-bearing
//! behaviors by hand: the `put`→`get` round-trip, the FORGIVING reads (a missing,
//! garbage, or wrong-shape file is `None`, never an error — the cold-cache path that
//! degrades to `select_model`'s verbatim resolve), the `{"models":[]}`-is-`Some(empty)`
//! corner, and the atomic temp+rename write (the `{"models":[{id,default}]}` shape with
//! NO leftover `.tmp`). A child module of `native`, so it roots the real cache at a
//! `tempfile` dir via the otherwise-private `dir`, never the operator's XDG cache.

use brazen::{Model, ModelCache};

use super::XdgModelCache;

/// The real cache rooted at `dir` (the `models/` leaf the real `new()` derives from
/// XDG), bypassing the env lookup so tests touch only the tempdir.
fn cache_at(dir: std::path::PathBuf) -> XdgModelCache {
    XdgModelCache { dir: Some(dir) }
}

fn model(id: &str, default: bool) -> Model {
    Model {
        id: id.into(),
        default,
    }
}

#[test]
fn put_then_get_roundtrips_the_model_list() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_at(tmp.path().join("models"));
    assert_eq!(
        cache.get("anthropic"),
        None,
        "a miss before any write is None"
    );

    let models = vec![
        model("claude-opus-4-1", true),
        model("claude-sonnet-4-5", false),
    ];
    cache.put("anthropic", &models);

    assert_eq!(
        cache.get("anthropic"),
        Some(models),
        "get must round-trip the persisted Vec<Model> byte-for-byte"
    );
    assert_eq!(
        cache.get("openai"),
        None,
        "an unwritten provider is still a miss, not a cross-read"
    );
}

#[test]
fn get_is_none_for_absent_garbage_or_wrong_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("models");
    let cache = cache_at(dir.clone());

    // Absent provider: the cold-cache path → None.
    assert_eq!(cache.get("absent"), None, "no file ⇒ None (forgiving)");

    // Non-JSON garbage on disk → None, never an error.
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("garbage.json"), b"not json at all").unwrap();
    assert_eq!(cache.get("garbage"), None, "unparseable file ⇒ None");

    // The non-obvious branch: valid JSON whose `models` is NOT an array — `from_value`
    // on the taken value fails → None (a wrong shape is as forgiving as garbage).
    std::fs::write(dir.join("wrong.json"), br#"{"models": 42}"#).unwrap();
    assert_eq!(cache.get("wrong"), None, "non-array `models` ⇒ None");

    // A missing `models` key (the `?` on `get_mut`) → None.
    std::fs::write(dir.join("nokey.json"), br#"{"other": []}"#).unwrap();
    assert_eq!(cache.get("nokey"), None, "absent `models` key ⇒ None");
}

#[test]
fn get_is_some_empty_for_an_empty_models_array() {
    // An explicit empty list is DISTINCT from a miss: `{"models":[]}` parses to
    // `Some(vec![])` (the provider really has zero models cached), not `None`.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("empty.json"), br#"{"models": []}"#).unwrap();
    let cache = cache_at(dir);
    assert_eq!(
        cache.get("empty"),
        Some(vec![]),
        "an empty array is Some(empty), not None"
    );
}

#[test]
fn put_writes_the_models_shape_atomically_with_no_leftover_tmp() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("models");
    let cache = cache_at(dir.clone());
    let models = vec![model("claude-opus-4-1", true)];
    cache.put("anthropic", &models);

    // The written document is the `{"models":[{id,default},…]}` shape `list-models
    // --json` emits — `Model`'s own serde, never a second representation.
    let bytes = std::fs::read(dir.join("anthropic.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["models"][0]["id"], "claude-opus-4-1");
    assert_eq!(doc["models"][0]["default"], true);

    // Atomic temp+rename leaves NO `.tmp` behind — only the final file remains.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the temp file is renamed away, none left: {leftovers:?}"
    );
}
