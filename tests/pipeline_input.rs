//! Input resolution tests (§5.5, §9.6): `--input FILE` and stdin yield the same
//! `Box<dyn Read>`, a missing `--input` errors (→ 66 at the call site), and the
//! file-vs-pipe parity invariant holds — identical bytes through a `Cursor`
//! ("the pipe") and a real `tempfile` ("the --input FILE") parse identically.

use std::fs;
use std::io::{self, Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use brazen::{open_input, parse, read_request, Content, Role};

/// A reader that always fails — proves the read-error arms surface, never panic.
struct FailReader;
impl Read for FailReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
}

/// A temp file that removes itself on drop — no external dep, unique per
/// (process, call) so parallel test threads never collide.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temp_with(bytes: &[u8]) -> TempFile {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("brazen_{}_{}.json", std::process::id(), n));
    fs::write(&path, bytes).unwrap();
    TempFile(path)
}

#[test]
fn open_input_none_locks_stdin_without_reading() {
    // The `None` arm constructs the stdin lock; constructing never reads, so
    // this is deterministic regardless of what stdin holds under the harness.
    assert!(open_input(None).is_ok());
}

#[test]
fn open_input_file_reads_back_the_bytes() {
    let tmp = temp_with(b"hello file");
    let mut r = open_input(Some(&tmp.0)).unwrap();
    let mut got = String::new();
    r.read_to_string(&mut got).unwrap();
    assert_eq!(got, "hello file");
}

#[test]
fn open_input_missing_file_is_an_error() {
    // A missing `--input FILE` is the open failure the caller maps to exit 66.
    let missing = std::env::temp_dir().join(format!("brazen_absent_{}.json", std::process::id()));
    assert!(open_input(Some(&missing)).is_err());
}

#[test]
fn stdin_input_parity_cursor_equals_tempfile() {
    // The executable proof file-vs-pipe dies at `open()`: identical bytes parse
    // to the identical `CanonicalRequest` whether they came from a `Cursor`
    // (the pipe) or a real file (the `--input FILE`).
    let bytes = br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#;

    let mut cursor = Cursor::new(bytes.to_vec());
    let from_pipe = parse(&mut cursor).unwrap();

    let tmp = temp_with(bytes);
    let mut file = open_input(Some(&tmp.0)).unwrap();
    let from_file = parse(&mut *file).unwrap();

    assert_eq!(from_pipe, from_file);
}

#[test]
fn read_request_positional_prompt_ignores_stdin_and_builds_one_user_message() {
    // POSIX filter idiom (§5.5): a positional prompt NEVER reads stdin. Handing it
    // a reader that panics-on-read proves `reader` is untouched — no block, no 64.
    let req = read_request(Some("what is 2+2"), &mut FailReader).unwrap();
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, Role::User);
    assert_eq!(
        req.messages[0].content,
        vec![Content::Text("what is 2+2".into())]
    );
    // model/system/gen-params are left for `fill_absent` (config/flags).
    assert!(req.model.is_empty());
}

#[test]
fn read_request_no_prompt_parses_canonical_stdin() {
    let mut stdin =
        Cursor::new(br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#.to_vec());
    let req = read_request(None, &mut stdin).unwrap();
    assert_eq!(req.model, "m");
}
