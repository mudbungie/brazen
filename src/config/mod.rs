//! Config schema, resolution & provider rows (the config spec). This task lands
//! the provider-row data records and the id vocabularies the registry keys on;
//! the resolution fold (`PartialConfig`, `resolve`, `--dump-config`) lands with
//! its own task.

pub mod provider;
