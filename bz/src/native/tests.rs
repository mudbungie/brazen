//! XdgCredStore IO invariants (bl-5b5a, gap #1) — the one piece of the
//! coverage-excluded `bz` shim with security-relevant invariants worth pinning:
//! the atomic temp-file + `rename` write, the 0600 file / 0700 dir modes, and the
//! `get`/`put` round-trip (auth §5.2). Driven against the REAL store rooted at a
//! `tempfile` dir, never the operator's `$XDG_DATA_HOME`. A child module of
//! `native`, so it may root the store at its otherwise-private `dir` field.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use brazen::{Cred, CredStore, Secret};

use super::XdgCredStore;

/// The real store rooted at `dir` (the `credentials/` leaf the real `new()` would
/// derive from XDG), bypassing the env lookup so tests touch only the tempdir.
fn store_at(dir: std::path::PathBuf) -> XdgCredStore {
    XdgCredStore { dir: Some(dir) }
}

/// A whole, valid `Cred::OAuth2` — the variant the login flow persists.
fn oauth_cred(access: &str, expires_at: u64) -> Cred {
    Cred::OAuth2 {
        access_token: Secret::new(access),
        refresh_token: Secret::new("refresh-tok"),
        expires_at,
        scope: Some("openid profile".to_owned()),
    }
}

#[test]
fn get_after_put_roundtrips_oauth2() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_at(tmp.path().join("credentials"));
    assert_eq!(
        store.get("anthropic"),
        None,
        "a miss before any write is None"
    );

    let cred = oauth_cred("access-tok", 1_750_000_000);
    store.put("anthropic", &cred).unwrap();

    assert_eq!(
        store.get("anthropic"),
        Some(cred),
        "get must round-trip the persisted Cred::OAuth2 byte-for-byte"
    );
    assert_eq!(
        store.get("openai"),
        None,
        "an unwritten provider is still a miss, not a cross-read"
    );
}

#[cfg(unix)]
#[test]
fn written_file_is_0600_and_dir_is_0700() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("credentials");
    let store = store_at(dir.clone());
    store.put("anthropic", &oauth_cred("a", 1)).unwrap();

    let file_mode = std::fs::metadata(dir.join("anthropic.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(file_mode & 0o777, 0o600, "the cred file must be owner-only");

    let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700, "the cred dir must be owner-only");
}

#[test]
fn concurrent_reads_never_observe_a_partial_write() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(store_at(tmp.path().join("credentials")));

    // Two whole, valid creds the writer alternates between. A torn read would be
    // `None` (a half-written file fails to parse) or a value equal to NEITHER — both
    // are ruled out by the store's temp-file-create + atomic `rename`, which swaps a
    // complete file into place. Establish the file before readers start.
    let a = oauth_cred("token-aaaaaaaaaaaaaaaaaaaa", 100);
    let b = oauth_cred("token-bbbbbbbbbbbbbbbbbbbb", 200);
    store.put("anthropic", &a).unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (store, done, a, b) = (store.clone(), done.clone(), a.clone(), b.clone());
            thread::spawn(move || {
                let mut reads = 0u64;
                while !done.load(Ordering::Relaxed) {
                    let got = store.get("anthropic");
                    assert!(
                        got.as_ref() == Some(&a) || got.as_ref() == Some(&b),
                        "reader observed a torn/partial cred: {got:?}"
                    );
                    reads += 1;
                }
                reads
            })
        })
        .collect();

    for i in 0..3000u64 {
        store
            .put("anthropic", if i % 2 == 0 { &a } else { &b })
            .unwrap();
    }
    done.store(true, Ordering::Relaxed);

    let total: u64 = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(total > 0, "the readers must have raced at least one write");
}
