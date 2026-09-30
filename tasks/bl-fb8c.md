+++
title = 'model-discovery: replace the id_key="" sentinel with a Collection enum (array_key+id_key | map_key)'
created = 1790733059
updated = 1790733059
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Design proposal, 2026-09-29, follows bl-885e. TOML has no bare identifiers, so 'id_key = map_key' cannot be spelled; the landed sentinel id_key = "" (= the id is the map key) is a string meaning a non-string thing, and array_key now names a map on the Cloud Code row.

Reframe: the collection SHAPE is the datum, and a map's id is necessarily its key, so name the container instead of the id:
  Array { array_key: "data", id_key: "id" }   -- the id is a field of each entry
  Map   { map_key: "models" }                    -- the id IS the entry's key
ModelKeys.{array_key,id_key} become one enum field (Collection<'a>). [provider.models] override: array_key/id_key XOR map_key, refused as MalformedFile (78) when both are given; no shipped row uses the block, so the surface change is free. decode_models matches on the variant; the 502 message derives from it. default_key/strip/metadata keys unchanged.

Spec homes: model-discovery.md §3 (structs + the 'two collection shapes' paragraph), §3.1 table, §3.2 override example; config.md schema. Tests: src/tests/model_discovery_map.rs + existing shape tests.