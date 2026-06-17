//! Seams: the credential store, `Cred`/`Secret`, and the injected `Clock`
//! (auth §5, §5.4). Exercises the data types and the in-memory/`FakeClock`
//! doubles end to end.

use brazen::store::{Clock, CredStore};
use brazen::testing::{FakeClock, MemoryCredStore};
use brazen::{Cred, Secret};
use serde_json::json;

#[test]
fn secret_redacts_everywhere_but_expose_and_serialize() {
    let s = Secret::new("sk-secret-123");
    assert_eq!(s.expose(), "sk-secret-123");
    assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
    assert_eq!(format!("{s}"), "<redacted>");
    // Serialize writes plaintext (the 0600-file path); Deserialize round-trips.
    assert_eq!(serde_json::to_value(&s).unwrap(), json!("sk-secret-123"));
    let back: Secret = serde_json::from_value(json!("sk-secret-123")).unwrap();
    assert_eq!(back, s);
    assert_eq!(s.clone(), s);
    assert_ne!(Secret::new("a"), Secret::new("b"));
}

#[test]
fn cred_each_variant_round_trips() {
    let cases = [
        Cred::ApiKey {
            key: Secret::new("ak"),
        },
        Cred::Bearer {
            token: Secret::new("bt"),
        },
        Cred::OAuth2 {
            access_token: Secret::new("at"),
            refresh_token: Secret::new("rt"),
            expires_at: 1_700_000_000,
            scope: Some("read write".into()),
            account_id: Some("acct-1".into()),
        },
    ];
    for cred in cases {
        let s = serde_json::to_string(&cred).unwrap();
        let back: Cred = serde_json::from_str(&s).unwrap();
        assert_eq!(back, cred);
        assert_eq!(cred.clone(), cred);
        // Debug never leaks the secret.
        assert!(format!("{cred:?}").contains("<redacted>"));
    }
}

#[test]
fn cred_oauth2_scope_defaults_to_none() {
    let cred: Cred = serde_json::from_value(json!({
        "OAuth2": {
            "access_token": "at",
            "refresh_token": "rt",
            "expires_at": 42
        }
    }))
    .unwrap();
    match cred {
        Cred::OAuth2 {
            scope, expires_at, ..
        } => {
            assert_eq!(scope, None);
            assert_eq!(expires_at, 42);
        }
        _ => panic!("expected OAuth2"),
    }
}

#[test]
fn memory_cred_store_put_get_and_miss() {
    let store = MemoryCredStore::new();
    assert!(store.get("anthropic").is_none()); // miss on an empty store

    let cred = Cred::ApiKey {
        key: Secret::new("sk"),
    };
    store.put("anthropic", &cred).unwrap();
    assert_eq!(store.get("anthropic"), Some(cred));
    assert!(store.get("openai").is_none());
}

#[test]
fn memory_cred_store_preloaded_with() {
    let cred = Cred::Bearer {
        token: Secret::new("tok"),
    };
    let store = MemoryCredStore::with("openai", cred.clone());
    // Exercise the trait object form the pipeline uses.
    let dyn_store: &dyn CredStore = &store;
    assert_eq!(dyn_store.get("openai"), Some(cred));
}

fn read_now(clock: &dyn Clock) -> u64 {
    clock.now()
}

#[test]
fn fake_clock_reads_sets_and_advances() {
    let clock = FakeClock::new(1_000);
    assert_eq!(read_now(&clock), 1_000);
    clock.advance(60);
    assert_eq!(clock.now(), 1_060);
    clock.set(5);
    assert_eq!(clock.now(), 5);
}
