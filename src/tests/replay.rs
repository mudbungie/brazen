//! The replay stash (ingress.md §5, tested per §14): hit, miss→None (fail-open),
//! prune (stale unlinked, fresh survives, exactly-at-horizon survives), rename
//! atomicity (no leftover temp, overwrite is whole-old-or-whole-new), concurrent
//! writers (one intact file per key), the unusable-key guard (a client-echoed
//! traversal key must not escape the stash), and the shared `content_key`
//! derivation both codec directions join on. Rooted at a tempdir standing in for
//! the injected XDG cache root — never the operator's real cache; time comes
//! from `FakeClock` (the §6.5 seam), never the system clock.

use std::time::{Duration, SystemTime};

use crate::store::{content_key, ReplayStash};
use crate::testing::FakeClock;

const WEEK: u64 = 7 * 24 * 60 * 60;

/// A stash rooted at `root` (the fake XDG cache root) and the spec'd leaf dir
/// (`<root>/brazen/replay`, ingress.md §5) its files land in.
fn stash_at(root: &std::path::Path) -> (ReplayStash, std::path::PathBuf) {
    (ReplayStash::new(root), root.join("brazen").join("replay"))
}

/// Rewind `path`'s mtime to `secs` unix-seconds — models an entry written by an
/// earlier turn (the mtime is the eviction record, ingress.md §5).
fn set_mtime(path: &std::path::Path, secs: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn stash_then_recall_hits() {
    let tmp = tempfile::tempdir().unwrap();
    let (stash, _) = stash_at(tmp.path());
    stash
        .stash(
            "toolu_01abc",
            br#"{"thinking":"...","signature":"sig"}"#,
            &FakeClock::new(0),
        )
        .unwrap();
    assert_eq!(
        stash.recall("toolu_01abc").as_deref(),
        Some(br#"{"thinking":"...","signature":"sig"}"#.as_slice()),
        "a stashed payload recalls byte-for-byte"
    );
}

#[test]
fn recall_of_a_never_stashed_key_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (stash, dir) = stash_at(tmp.path());
    // Cold stash: not even the directory exists yet — still None, never an error.
    assert_eq!(stash.recall("toolu_absent"), None, "cold miss fails open");
    stash.stash("other", b"x", &FakeClock::new(0)).unwrap();
    assert_eq!(
        stash.recall("toolu_absent"),
        None,
        "a warm dir misses unknown keys the same way"
    );
    assert!(dir.join("other").exists(), "the write landed where spec'd");
}

#[test]
fn prune_unlinks_stale_entries_and_keeps_fresh_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let (stash, dir) = stash_at(tmp.path());
    let now = 100 * WEEK; // an arbitrary injected "now", far from real time
    let clock = FakeClock::new(now);

    stash.stash("stale", b"old turn", &clock).unwrap();
    set_mtime(&dir.join("stale"), now - WEEK - 1); // just over the horizon
    stash.stash("boundary", b"exactly 7d", &clock).unwrap();
    set_mtime(&dir.join("boundary"), now - WEEK); // exactly AT the horizon
    stash.stash("fresh", b"this turn", &clock).unwrap();
    set_mtime(&dir.join("fresh"), now - 1);

    // The next write prunes as a side effect (best-effort, on write).
    stash.stash("trigger", b"new turn", &clock).unwrap();

    assert_eq!(stash.recall("stale"), None, "older than 7 days is unlinked");
    assert_eq!(
        stash.recall("boundary").as_deref(),
        Some(b"exactly 7d".as_slice()),
        "exactly-7-days is NOT stale (strictly older evicts)"
    );
    assert_eq!(
        stash.recall("fresh").as_deref(),
        Some(b"this turn".as_slice()),
        "a fresh entry survives the prune"
    );
    // A real-clock mtime is FUTURE of this fake now: saturating age 0, kept.
    assert!(
        stash.recall("trigger").is_some(),
        "the triggering write survives"
    );
}

#[test]
fn stash_renames_into_place_with_no_leftover_temp() {
    let tmp = tempfile::tempdir().unwrap();
    let (stash, dir) = stash_at(tmp.path());
    let clock = FakeClock::new(0);
    stash.stash("k", b"first", &clock).unwrap();
    // Overwrite: the reader sees whole-old or whole-new, and afterwards new.
    stash.stash("k", b"second, longer payload", &clock).unwrap();
    assert_eq!(
        stash.recall("k").as_deref(),
        Some(b"second, longer payload".as_slice()),
        "an overwrite replaces the payload wholesale"
    );
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["k"],
        "temp files are renamed away, one file per key"
    );
}

#[test]
fn concurrent_writers_of_one_key_leave_one_intact_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let handles: Vec<_> = (0..8u8)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                // One stash per thread, as separate `bz` processes would race.
                let stash = ReplayStash::new(root);
                stash
                    .stash("shared", &vec![b'a' + i; 4096], &FakeClock::new(0))
                    .unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let (stash, dir) = stash_at(&root);
    let payload = stash.recall("shared").expect("some writer won");
    assert_eq!(
        payload.len(),
        4096,
        "the winner's payload is whole, no tears"
    );
    assert!(
        payload.iter().all(|b| *b == payload[0]),
        "the file is ONE writer's bytes, never interleaved"
    );
    let count = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(
        count, 1,
        "one file per key, every temp consumed by its rename"
    );
}

#[test]
fn keys_that_cannot_be_file_names_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (stash, _) = stash_at(tmp.path());
    let clock = FakeClock::new(0);
    stash.stash("victim", b"secret", &clock).unwrap();
    // Recall keys are echoed by the CLIENT: a traversal key must miss, not
    // resolve outside the stash (and `.`-prefixes are the temp namespace).
    for key in ["../victim", "a/b", "a\\b", "", ".hidden", "..", "."] {
        assert_eq!(stash.recall(key), None, "unusable key {key:?} fails open");
        let err = stash.stash(key, b"x", &clock).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "for {key:?}");
    }
}

#[test]
fn stash_surfaces_io_failure_when_the_dir_cannot_exist() {
    // The root's `brazen` component is a FILE, so create_dir_all must fail —
    // the write half is honest io::Result; only recall is sworn to fail open.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("brazen"), b"in the way").unwrap();
    let (stash, _) = stash_at(tmp.path());
    assert!(stash.stash("k", b"x", &FakeClock::new(0)).is_err());
    assert_eq!(stash.recall("k"), None, "and the entry was never written");
}

#[test]
fn content_key_is_the_shared_pinned_derivation() {
    // The one definition decoder and encoders join on (ingress.md §5):
    // BASE64URL-NOPAD(SHA-256(text)). Pinned to a literal so a silent
    // redefinition (hex, another digest) cannot orphan stashed entries.
    assert_eq!(
        content_key("The answer is 42."),
        "l7OLLr2hykz06ikQBdl9B8cFPbKu0--GbAS0ns-zRI0"
    );
    assert_eq!(content_key("hello"), content_key("hello"), "deterministic");
    assert_ne!(
        content_key("hello"),
        content_key("hello "),
        "content-sensitive"
    );
    // And the derived key is always a usable stash file name.
    let key = content_key("any assistant text");
    assert_eq!(key.len(), 43, "SHA-256 in base64url-nopad is 43 chars");
    let tmp = tempfile::tempdir().unwrap();
    let (stash, _) = stash_at(tmp.path());
    stash.stash(&key, b"payload", &FakeClock::new(0)).unwrap();
    assert_eq!(stash.recall(&key).as_deref(), Some(b"payload".as_slice()));
}
