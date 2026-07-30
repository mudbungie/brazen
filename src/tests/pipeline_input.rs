//! Input resolution tests (§5.5, §9.6): `--input FILE` and stdin yield the same
//! `Box<dyn Read>`, a missing `--input` errors (→ 66 at the call site), and the
//! file-vs-pipe parity invariant holds — identical bytes through a `Cursor`
//! ("the pipe") and a real `tempfile` ("the --input FILE") parse identically.

use std::io::{self, Cursor, Read};
use std::path::PathBuf;

use crate::{open_input, parse, read_files, read_request, Content, Role};
use tempfile::NamedTempFile;

/// A reader that always fails — proves the read-error arms surface, never panic.
struct FailReader;
impl Read for FailReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
}

/// A `--input FILE` holding `bytes`: a `tempfile` temp file (unique per call,
/// auto-removed on drop), the same dev-dep the rest of the suite uses.
fn temp_with(bytes: &[u8]) -> NamedTempFile {
    let tmp = NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), bytes).unwrap();
    tmp
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
    let mut r = open_input(Some(tmp.path())).unwrap();
    let mut got = String::new();
    r.read_to_string(&mut got).unwrap();
    assert_eq!(got, "hello file");
}

#[test]
fn open_input_missing_file_is_an_error() {
    // A missing `--input FILE` is the open failure the caller maps to exit 66:
    // a path inside a live tempdir that was never created.
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.json");
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
    let mut file = open_input(Some(tmp.path())).unwrap();
    let from_file = parse(&mut *file).unwrap();

    assert_eq!(from_pipe, from_file);
}

#[test]
fn read_request_positional_prompt_ignores_stdin_and_builds_one_user_message() {
    // POSIX filter idiom (§5.5): a positional prompt NEVER reads stdin. Handing it
    // a reader that panics-on-read proves `reader` is untouched — no block, no 64.
    let req = read_request(Some("what is 2+2"), vec![], &mut FailReader).unwrap();
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
    let req = read_request(None, vec![], &mut stdin).unwrap();
    assert_eq!(req.model, "m");
}

#[test]
fn read_files_reads_each_path_into_an_ordered_text_part() {
    // Each `-f` path's whole contents become one `Content::Text`, in argv order (§5.5).
    // The `FailReader` stdin proves a run with no `-` never touches the stream.
    let a = temp_with(b"alpha");
    let b = temp_with(b"beta");
    let parts = read_files(&[a.path().to_owned(), b.path().to_owned()], &mut FailReader).unwrap();
    assert_eq!(
        parts,
        vec![Content::Text("alpha".into()), Content::Text("beta".into())]
    );
    // Empty input is the general path with nothing to read — no parts, no error.
    assert!(read_files(&[], &mut FailReader).unwrap().is_empty());
}

#[test]
fn read_files_missing_returns_the_offending_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.txt");
    let (path, _e) = read_files(std::slice::from_ref(&missing), &mut FailReader).unwrap_err();
    assert_eq!(path, missing); // the caller names it on stderr, maps to exit 66
}

#[test]
fn read_files_non_utf8_is_an_error() {
    // A text part is UTF-8 — `read_to_string` folds a non-UTF-8 file into the same
    // `io::Error` class as a missing one (→ exit 66, §5.5).
    let bin = temp_with(&[0xff, 0xfe, 0x00]);
    assert!(read_files(&[bin.path().to_owned()], &mut FailReader).is_err());
}

#[test]
fn read_files_dash_reads_stdin_as_a_text_part() {
    // `-f -` NAMES stdin (§5.5), the portable spelling of `-f /dev/stdin`: the piped
    // bytes become the SAME `Content::Text` a named file yields — this is what makes
    // `bz "…" | bz -f - "…"` chain. Interleaved with a named file, argv order holds.
    let f = temp_with(b"from-file");
    let mut stdin = Cursor::new(b"from-stdin".to_vec());
    let paths = [PathBuf::from("-"), f.path().to_owned()];
    let parts = read_files(&paths, &mut stdin).unwrap();
    assert_eq!(
        parts,
        vec![
            Content::Text("from-stdin".into()),
            Content::Text("from-file".into())
        ]
    );
}

#[test]
fn read_files_dash_twice_yields_an_empty_second_part() {
    // Not a special case: stdin is read ONCE, the second `-` hits EOF and yields the
    // empty part `-f empty.txt` would (§5.5). Empty is the general path with no bytes.
    let mut stdin = Cursor::new(b"once".to_vec());
    let paths = [PathBuf::from("-"), PathBuf::from("-")];
    let parts = read_files(&paths, &mut stdin).unwrap();
    assert_eq!(
        parts,
        vec![Content::Text("once".into()), Content::Text(String::new())]
    );
}

#[test]
fn read_files_dash_read_failure_names_the_dash_path() {
    // A stdin that errors (or holds non-UTF-8) folds into the same `io::Error` a bad
    // file does → exit 66, reported against `-` so the message still names the source.
    let (path, _e) = read_files(&[PathBuf::from("-")], &mut FailReader).unwrap_err();
    assert_eq!(path, PathBuf::from("-"));
}

#[test]
fn read_request_prompt_with_files_puts_files_first_then_prompt() {
    // The user message is `[file parts…, prompt]` (§5.5); stdin (a panic-reader) is
    // never touched because the positional wins.
    let parts = vec![Content::Text("ctx-a".into()), Content::Text("ctx-b".into())];
    let req = read_request(Some("question"), parts, &mut FailReader).unwrap();
    assert_eq!(req.messages.len(), 1);
    assert_eq!(
        req.messages[0].content,
        vec![
            Content::Text("ctx-a".into()),
            Content::Text("ctx-b".into()),
            Content::Text("question".into()),
        ]
    );
}

#[test]
fn read_request_files_only_with_empty_stdin_is_just_the_file_parts() {
    // Files, no prompt, empty stdin → one user message of just the attachments (§5.5).
    let parts = vec![Content::Text("only".into())];
    let mut stdin = Cursor::new(b"   \n".to_vec()); // whitespace-only == "no request"
    let req = read_request(None, parts, &mut stdin).unwrap();
    assert_eq!(req.messages[0].content, vec![Content::Text("only".into())]);
}

#[test]
fn read_request_files_plus_piped_request_is_refused_64() {
    // A pre-assembled request on stdin can't merge with loose file parts → refuse (§5.5).
    let parts = vec![Content::Text("ctx".into())];
    let mut stdin = Cursor::new(br#"{"messages":[]}"#.to_vec());
    let err = read_request(None, parts, &mut stdin).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("cannot combine --file"));
}

#[test]
fn read_request_files_with_a_failing_stdin_read_is_input_error_64() {
    // The emptiness probe reads stdin; a read failure is a clean input error, not a panic.
    let parts = vec![Content::Text("ctx".into())];
    let err = read_request(None, parts, &mut FailReader).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("failed to read stdin"));
}
