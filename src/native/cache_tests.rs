//! XdgModelCache IO invariants (model-discovery §8) — the XDG-file `ModelCache` impl,
//! coverage-excluded with the rest of the `bz` shim, so these pin its load-bearing
//! behaviors by hand: the `put`→`get` round-trip, the FORGIVING reads (a missing,
//! garbage, or wrong-shape file is `None`, never an error — the cold-cache path that
//! degrades to `select_model`'s verbatim resolve), the `{"models":[]}`-is-`Some(empty)`
//! corner, and the atomic temp+rename write (the `{"models":[{id,default}]}` shape with
//! NO leftover `.tmp`). A child module of `native`, so it roots the real cache at a
//! `tempfile` dir via the otherwise-private `dir`, never the operator's XDG cache.

use brazen::{CachedModels, Model, ModelCache};

use super::XdgModelCache;

/// The real cache rooted at `dir` (the `models/` leaf the real `new()` derives from
/// XDG), bypassing the env lookup so tests touch only the tempdir.
/// A pointer-less document — what a `--list-models` write looks like on a cold cache.
fn doc(models: Vec<Model>) -> CachedModels {
    CachedModels {
        models,
        last_used: None,
    }
}

fn cache_at(dir: std::path::PathBuf) -> XdgModelCache {
    XdgModelCache { dir: Some(dir) }
}

fn model(id: &str, default: bool) -> Model {
    Model {
        id: id.into(),
        default,
        ..Default::default()
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
    cache.put("anthropic", &doc(models.clone()));

    assert_eq!(
        cache.get("anthropic"),
        Some(doc(models)),
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

    // The non-obvious branch: valid JSON whose `models` is NOT an array — the document
    // serde fails → None (a wrong shape is as forgiving as garbage).
    std::fs::write(dir.join("wrong.json"), br#"{"models": 42}"#).unwrap();
    assert_eq!(cache.get("wrong"), None, "non-array `models` ⇒ None");

    // A missing `models` key (no `serde(default)` on the list) → None.
    std::fs::write(dir.join("nokey.json"), br#"{"other": []}"#).unwrap();
    assert_eq!(cache.get("nokey"), None, "absent `models` key ⇒ None");

    // A `last_used` of the WRONG TYPE is file corruption like any other — the whole
    // document is None and the cache self-heals on the next `bz --list-models`. That is
    // this impl's ONE forgiveness rule, unchanged by the new key; a well-typed pointer
    // naming a model the list no longer carries is NOT corruption, and falls through to
    // the provider's suggestion inside `select_model` (model-discovery §4).
    std::fs::write(
        dir.join("badptr.json"),
        br#"{"models":[{"id":"a","default":false}],"last_used":42}"#,
    )
    .unwrap();
    assert_eq!(cache.get("badptr"), None, "a non-string `last_used` ⇒ None");
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
        Some(doc(vec![])),
        "an empty array is Some(empty), not None"
    );
}

#[test]
fn an_older_cache_file_without_metadata_reads_clean() {
    // OLD-CACHE COMPAT through the XDG impl (model-discovery §5.1): a file a bz that
    // predates the metadata wrote — entries are `{id,default}` only — must still `get`
    // clean, the absent fields folding to `None` (the v=1 grows-only discipline). Written
    // here as literal bytes to stand in for the older writer.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("openai.json"),
        br#"{"models":[{"id":"gpt-5","default":false},{"id":"o3","default":false}]}"#,
    )
    .unwrap();
    let cache = cache_at(dir);
    assert_eq!(
        cache.get("openai"),
        Some(doc(vec![model("gpt-5", false), model("o3", false)])),
        "an older metadata-less file reads clean, every metadata field None"
    );
}

#[test]
fn put_then_get_roundtrips_provider_metadata() {
    // A metadata-bearing list (as `--list-models` decodes from Google, §3) round-trips
    // through the atomic file cache unchanged — the write half extends additively.
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_at(tmp.path().join("models"));
    let rich = vec![Model {
        id: "gemini-2.5-pro".into(),
        default: false,
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        display_name: Some("Gemini 2.5 Pro".into()),
    }];
    cache.put("google", &doc(rich.clone()));
    assert_eq!(cache.get("google"), Some(doc(rich)));
}

#[test]
fn put_writes_the_models_shape_atomically_with_no_leftover_tmp() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("models");
    let cache = cache_at(dir.clone());
    let models = vec![model("claude-opus-4-1", true)];
    cache.put(
        "anthropic",
        &CachedModels {
            models,
            last_used: Some("claude-opus-4-1".into()),
        },
    );

    // The written document is the `{"models":[{id,default},…]}` shape `list-models
    // --json` emits — `Model`'s own serde, never a second representation.
    let bytes = std::fs::read(dir.join("anthropic.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["models"][0]["id"], "claude-opus-4-1");
    assert_eq!(doc["models"][0]["default"], true);
    // …with the §4 rung-2 pointer BESIDE the list, never a reordering of it.
    assert_eq!(doc["last_used"], "claude-opus-4-1");

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
