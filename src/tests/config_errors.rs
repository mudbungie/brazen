//! The `Config` (78) error set (config §7): every variant has a message and
//! maps to one `Config`-kind canonical error.

use crate::{CanonicalError, ConfigError, ErrorKind};

fn message(err: ConfigError) -> String {
    err.to_string()
}

#[test]
fn every_variant_renders_a_message() {
    assert!(message(ConfigError::NoProvider).contains("no provider"));
    assert!(message(ConfigError::UnknownProvider { name: "x".into() }).contains("unknown provider"));
    assert!(message(ConfigError::AmbiguousModel {
        model: "m".into(),
        providers: vec!["a".into(), "b".into()],
    })
    .contains("a, b"));
    assert!(message(ConfigError::IncompleteProvider {
        name: "x".into(),
        field: "base_url",
    })
    .contains("base_url"));
    assert!(message(ConfigError::BadValue {
        key: "k".into(),
        detail: "d".into(),
    })
    .contains("bad value"));
    assert!(message(ConfigError::MalformedFile {
        detail: "oops".into()
    })
    .contains("malformed"));
}

#[test]
fn config_errors_map_to_exit_78() {
    let err: CanonicalError = ConfigError::NoProvider.into();
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(err.exit_code(), 78);
    assert_eq!(err.provider_detail, None);
    assert!(!err.message.is_empty());
}

#[test]
fn config_error_is_clone_debug_eq() {
    let err = ConfigError::UnknownProvider { name: "x".into() };
    assert_eq!(err.clone(), err);
    assert!(!format!("{err:?}").is_empty());
}
