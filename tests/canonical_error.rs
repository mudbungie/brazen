//! Exhaustive table tests for the error model (§3.3, §8): `retryable`,
//! `exit_code`, the `ExitClass` tables, and `CanonicalError`/`ErrorKind` serde.

use std::io;

use brazen::{CanonicalError, ErrorKind, ExitClass};
use serde_json::json;

fn err(kind: ErrorKind) -> CanonicalError {
    CanonicalError {
        kind,
        message: "boom".into(),
        provider_detail: None,
    }
}

#[test]
fn retryable_is_a_pure_query_over_kind() {
    let retryable = [
        ErrorKind::Transport,
        ErrorKind::Provider { status: 429 },
        ErrorKind::Provider { status: 500 },
        ErrorKind::Provider { status: 503 },
        ErrorKind::Provider { status: 599 },
    ];
    let not_retryable = [
        ErrorKind::Usage,
        ErrorKind::ParseInput,
        ErrorKind::Config,
        ErrorKind::Auth,
        ErrorKind::Interrupted,
        ErrorKind::Provider { status: 200 },
        ErrorKind::Provider { status: 400 },
        ErrorKind::Provider { status: 404 },
        ErrorKind::Provider { status: 499 },
    ];
    for k in retryable {
        assert!(err(k).retryable(), "{k:?} must be retryable");
    }
    for k in not_retryable {
        assert!(!err(k).retryable(), "{k:?} must not be retryable");
    }
}

#[test]
fn exit_code_maps_every_kind() {
    let cases = [
        (ErrorKind::Usage, 64),
        (ErrorKind::ParseInput, 64),
        (ErrorKind::Config, 78),
        (ErrorKind::Auth, 77),
        (ErrorKind::Transport, 69),
        (ErrorKind::Interrupted, 130),
        (ErrorKind::Provider { status: 400 }, 69),
        (ErrorKind::Provider { status: 429 }, 69),
        (ErrorKind::Provider { status: 499 }, 69),
        (ErrorKind::Provider { status: 500 }, 70),
        (ErrorKind::Provider { status: 503 }, 70),
    ];
    for (k, code) in cases {
        assert_eq!(err(k).exit_code(), code, "exit_code for {k:?}");
    }
}

#[test]
fn exit_class_code_table() {
    assert_eq!(ExitClass::Ok.code(), 0);
    assert_eq!(ExitClass::Usage.code(), 64);
    assert_eq!(ExitClass::NoInput.code(), 66);
    assert_eq!(ExitClass::Unavailable.code(), 69);
    assert_eq!(ExitClass::Software.code(), 70);
    assert_eq!(ExitClass::NoPerm.code(), 77);
    assert_eq!(ExitClass::Config.code(), 78);
    assert_eq!(ExitClass::Sig(130).code(), 130);
    assert_eq!(ExitClass::Sig(141).code(), 141);
    assert_eq!(ExitClass::Sig(143).code(), 143);
}

#[test]
fn from_kind_table() {
    assert_eq!(ExitClass::from_kind(ErrorKind::Usage), ExitClass::Usage);
    assert_eq!(
        ExitClass::from_kind(ErrorKind::ParseInput),
        ExitClass::Usage
    );
    assert_eq!(ExitClass::from_kind(ErrorKind::Config), ExitClass::Config);
    assert_eq!(ExitClass::from_kind(ErrorKind::Auth), ExitClass::NoPerm);
    assert_eq!(
        ExitClass::from_kind(ErrorKind::Transport),
        ExitClass::Unavailable
    );
    assert_eq!(
        ExitClass::from_kind(ErrorKind::Interrupted),
        ExitClass::Sig(130)
    );
    assert_eq!(
        ExitClass::from_kind(ErrorKind::Provider { status: 404 }),
        ExitClass::Unavailable
    );
    assert_eq!(
        ExitClass::from_kind(ErrorKind::Provider { status: 502 }),
        ExitClass::Software
    );
}

#[test]
fn from_io_table() {
    let pipe = io::Error::from(io::ErrorKind::BrokenPipe);
    assert_eq!(ExitClass::from_io(&pipe), ExitClass::Sig(141));
    let other = io::Error::from(io::ErrorKind::TimedOut);
    assert_eq!(ExitClass::from_io(&other), ExitClass::Unavailable);
}

#[test]
fn from_http_status_table() {
    use ErrorKind::Provider;
    // 401/403 are the only auth statuses; every other code rides Provider{status},
    // which already carries exit + retryable — so the status needs no second table.
    assert_eq!(ErrorKind::from_http_status(401), ErrorKind::Auth);
    assert_eq!(ErrorKind::from_http_status(403), ErrorKind::Auth);
    for s in [400u16, 404, 409, 422, 429, 500, 503, 529] {
        assert_eq!(ErrorKind::from_http_status(s), Provider { status: s });
    }
    // exit/retryable fall out of the status with no extra mapping.
    assert_eq!(err(ErrorKind::from_http_status(429)).exit_code(), 69);
    assert!(err(ErrorKind::from_http_status(500)).retryable());
    assert!(!err(ErrorKind::from_http_status(400)).retryable());
}

#[test]
fn error_kind_serde_each_variant() {
    let cases = [
        (ErrorKind::Usage, json!("usage")),
        (ErrorKind::ParseInput, json!("parse_input")),
        (ErrorKind::Config, json!("config")),
        (ErrorKind::Auth, json!("auth")),
        (ErrorKind::Transport, json!("transport")),
        (ErrorKind::Interrupted, json!("interrupted")),
        (
            ErrorKind::Provider { status: 429 },
            json!({"provider": {"status": 429}}),
        ),
    ];
    for (kind, wire) in cases {
        let v = serde_json::to_value(kind).unwrap();
        assert_eq!(v, wire, "serialize {kind:?}");
        let back: ErrorKind = serde_json::from_value(wire).unwrap();
        assert_eq!(back, kind, "round-trip {kind:?}");
    }
}

#[test]
fn canonical_error_roundtrips_with_and_without_detail() {
    let with = CanonicalError {
        kind: ErrorKind::Provider { status: 500 },
        message: "overloaded".into(),
        provider_detail: Some(json!({"type": "overloaded_error"})),
    };
    let s = serde_json::to_string(&with).unwrap();
    let back: CanonicalError = serde_json::from_str(&s).unwrap();
    assert_eq!(back, with);

    let without = err(ErrorKind::Auth);
    let s = serde_json::to_string(&without).unwrap();
    assert_eq!(
        s,
        r#"{"kind":"auth","message":"boom","provider_detail":null}"#
    );
    let back: CanonicalError = serde_json::from_str(&s).unwrap();
    assert_eq!(back, without);
    // provider_detail defaults to None when the key is absent.
    let bare: CanonicalError =
        serde_json::from_str(r#"{"kind":"transport","message":"x"}"#).unwrap();
    assert_eq!(bare.provider_detail, None);
}
