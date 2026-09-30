//! The row's collection SHAPE over the protocol default (model-discovery §3.2):
//! `array_key`/`id_key` XOR `map_key`. The pure overlay inside `models_req` per case,
//! and the parse-time refusal of a block naming both shapes (`MalformedFile`/78).

use crate::config::parse_config;
use crate::config::ConfigError;
use crate::protocol::{Collection, ModelKeys, ModelsShape};
use crate::run::models_req;
use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::list_models_override::DEF;
use crate::tests::list_models_support::go;
use crate::tests::run_support::temp;
use crate::ModelsOverride;

/// A `Map` default (the Cloud Code shape, trimmed to the collection).
const MAP_DEF: ModelsShape = ModelsShape {
    keys: ModelKeys {
        collection: Collection::Map { key: "models" },
        ..DEF.keys
    },
    ..DEF
};

fn over(array: Option<&str>, id: Option<&str>, map: Option<&str>) -> ModelsOverride {
    ModelsOverride {
        array_key: array.map(Into::into),
        id_key: id.map(Into::into),
        map_key: map.map(Into::into),
        ..Default::default()
    }
}

fn coll(shape: ModelsShape, o: &ModelsOverride) -> Collection<'_> {
    models_req(shape, Some(o), "https://x.test").keys.collection
}

#[test]
fn id_key_alone_over_an_array_keeps_the_default_key() {
    let o = over(None, Some("slug"), None);
    let want = Collection::Array {
        key: "data",
        id_key: "slug",
    };
    assert_eq!(coll(DEF, &o), want);
}

#[test]
fn map_key_replaces_an_array_default_whole() {
    let o = over(None, None, Some("byId"));
    assert_eq!(coll(DEF, &o), Collection::Map { key: "byId" });
}

#[test]
fn array_keys_over_a_map_default_choose_an_array() {
    // The row chose the shape: the map's `key` carries over, the id field is `id`
    // unless named.
    let o = over(Some("list"), None, None);
    let want = Collection::Array {
        key: "list",
        id_key: "id",
    };
    assert_eq!(coll(MAP_DEF, &o), want);
    let o = over(None, Some("name"), None);
    let want = Collection::Array {
        key: "models",
        id_key: "name",
    };
    assert_eq!(coll(MAP_DEF, &o), want);
}

#[test]
fn naming_no_shape_keeps_a_map_default() {
    let o = over(None, None, None);
    assert_eq!(coll(MAP_DEF, &o), MAP_DEF.keys.collection);
}

const BOTH: &str = r#"
[[provider]]
name = "mixed"
base_url = "https://x.test"
protocol = "openai_chat"
auth = "none"

[provider.models]
id_key = "slug"
map_key = "models"
"#;

#[test]
fn a_block_naming_both_shapes_is_malformed() {
    let err = parse_config(BOTH).unwrap_err();
    let ConfigError::MalformedFile { detail } = err else {
        panic!("{err:?}")
    };
    assert!(detail.contains("provider `mixed`"), "{detail}");
    assert!(detail.contains("pick one collection shape"), "{detail}");
    // …and the verb surfaces it as exit 78 before any round-trip.
    let cfg = temp(BOTH);
    let path = cfg.0.to_str().unwrap();
    let tx = MockTransport::ok(vec![]);
    let argv = ["--list-models", "--provider", "mixed", "--config", path];
    let o = go(&argv, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(tx.requests().is_empty());
}
