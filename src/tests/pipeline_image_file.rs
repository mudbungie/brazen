//! A model-RETURNED image in text mode (architecture §5.3, interactive-output §5,
//! bl-0987): both text sinks write the SAME content-addressed file under the injected
//! dir and name it on stderr — bare under PLAIN, behind a cyan `▣` gutter when pretty —
//! while stdout stays byte-identical to the no-image stream. Fragments concatenate
//! before the one decode; a block dropped at the terminal writes nothing; a malformed
//! base64 is an `Err` out of `write`, never a panic or a file.

use std::io::Write;
use std::path::Path;

use crate::{
    file_name, write_image, CanonicalError, ContentKind, Delta, ErrorKind, Event, PrettySink, Sink,
    Style, TextSink,
};

/// `"hello"` — sha256 `2cf24dba5fb0…`, so the pinned name is `bz-2cf24dba5fb0.png`.
const HELLO_B64: &str = "aGVsbG8=";
const HELLO_NAME: &str = "bz-2cf24dba5fb0.png";

fn start(index: u32) -> Event {
    Event::ContentStart {
        index,
        kind: ContentKind::Image {
            media_type: "image/png".into(),
        },
    }
}

fn frag(index: u32, b64: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::ImageDelta(b64.into()),
    }
}

fn text(index: u32, t: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::TextDelta(t.into()),
    }
}

/// An image block (two fragments) interleaved with text on another index.
fn image_stream() -> Vec<Event> {
    vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Text {},
        },
        text(0, "Here: "),
        start(1),
        frag(1, "aGVs"),
        frag(1, "bG8="),
        text(0, "done"),
        Event::ContentStop { index: 1 },
        Event::ContentStop { index: 0 },
        Event::End,
    ]
}

/// Drive `stream` through a `TextSink` (or a `PrettySink` of `style`) writing under
/// `dir`; returns `(stdout, stderr)`. Each sink uses the ONE instantiation its sibling
/// suites use (`TextSink` over `&mut Vec<u8>`, `PrettySink` over `&mut dyn Write`) so
/// line coverage merges across files instead of splitting per monomorphization.
fn run(style: Option<Style>, dir: &Path, stream: Vec<Event>) -> (Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    match style {
        Some(style) => {
            let (o, e): (&mut dyn Write, &mut dyn Write) = (&mut out, &mut err);
            let mut sink = PrettySink::new(o, e, false, style, dir);
            for ev in stream {
                sink.write(&ev).unwrap();
            }
        }
        None => {
            let mut sink = TextSink::new(&mut out, &mut err, false, dir);
            for ev in stream {
                sink.write(&ev).unwrap();
            }
        }
    }
    (out, err)
}

#[test]
fn file_name_is_content_addressed_from_sha256_and_the_media_table() {
    assert_eq!(file_name("image/png", b"hello"), HELLO_NAME);
    assert_eq!(file_name("image/jpeg", b"hello"), "bz-2cf24dba5fb0.jpg");
    assert_eq!(file_name("image/webp", b"hello"), "bz-2cf24dba5fb0.webp");
    assert_eq!(file_name("image/gif", b"hello"), "bz-2cf24dba5fb0.gif");
    assert_eq!(
        file_name("application/pdf", b"hello"),
        "bz-2cf24dba5fb0.pdf"
    );
    // A type the table does not name still lands, honestly unnamed.
    assert_eq!(
        file_name("image/x-unknown", b"hello"),
        "bz-2cf24dba5fb0.bin"
    );
    // Idempotent for the same bytes, distinct for different ones.
    assert_ne!(file_name("image/png", b"hello!"), HELLO_NAME);
}

#[test]
fn write_image_decodes_once_and_rejects_malformed_base64() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_image(tmp.path(), "image/png", HELLO_B64).unwrap();
    assert_eq!(path, tmp.path().join(HELLO_NAME));
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    let err = write_image(tmp.path(), "image/png", "not base64!").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    // An unwritable dir is the fs error, surfaced the same way.
    assert!(write_image(&tmp.path().join("absent"), "image/png", HELLO_B64).is_err());
}

#[test]
fn text_sink_writes_the_file_and_names_it_bare_on_stderr_with_stdout_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, err) = run(None, tmp.path(), image_stream());
    let path = tmp.path().join(HELLO_NAME);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"hello",
        "two fragments, one decode"
    );
    assert_eq!(err, format!("{}\n", path.display()).into_bytes());
    // stdout is byte-identical to the same stream with the image block removed.
    let no_image: Vec<Event> = image_stream()
        .into_iter()
        .filter(|ev| {
            !matches!(
                ev,
                Event::ContentStart { index: 1, .. }
                    | Event::ContentDelta { index: 1, .. }
                    | Event::ContentStop { index: 1 }
            )
        })
        .collect();
    let (plain_out, plain_err) = run(None, tmp.path(), no_image);
    assert_eq!(out, plain_out);
    assert_eq!(out, b"Here: done");
    assert!(plain_err.is_empty());
}

#[test]
fn pretty_sink_writes_the_same_file_behind_a_cyan_gutter() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(HELLO_NAME);
    let (out, err) = run(
        Some(Style::Pretty { ascii: false }),
        tmp.path(),
        image_stream(),
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert_eq!(out, b"Here: done");
    assert_eq!(
        err,
        format!("\x1b[36m▣\x1b[0m {}\n", path.display()).into_bytes()
    );
    let (_, ascii_err) = run(
        Some(Style::Pretty { ascii: true }),
        tmp.path(),
        image_stream(),
    );
    assert_eq!(
        ascii_err,
        format!("\x1b[36m#\x1b[0m {}\n", path.display()).into_bytes()
    );
}

#[test]
fn a_block_open_at_the_terminal_is_dropped_never_written() {
    // Truncated: start + a fragment, then End (no stop) — and the Error variant.
    for style in [None, Some(Style::Pretty { ascii: false })] {
        for terminal in [
            Event::End,
            Event::Error(CanonicalError {
                kind: ErrorKind::Transport,
                message: "cut".into(),
                provider_detail: None,
                retry_after_seconds: None,
            }),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let is_end = matches!(terminal, Event::End);
            let (out, err) = run(
                style,
                tmp.path(),
                vec![
                    start(0),
                    frag(0, "aGVs"),
                    terminal,
                    Event::ContentStop { index: 0 },
                ],
            );
            assert!(out.is_empty());
            assert_eq!(
                std::fs::read_dir(tmp.path()).unwrap().count(),
                0,
                "nothing written"
            );
            assert_eq!(err.is_empty(), is_end, "Error still reports its message");
        }
    }
}

#[test]
fn a_stop_with_no_fragments_or_no_image_block_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, err) = run(
        None,
        tmp.path(),
        vec![
            start(0),
            Event::ContentStop { index: 0 },
            // A fragment for an index with no open image block is dropped.
            frag(7, HELLO_B64),
            Event::ContentStop { index: 7 },
        ],
    );
    assert!(out.is_empty() && err.is_empty());
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

/// Feed a malformed block into `sink` and assert its stop is an `Err`, not a file.
fn malformed_stop_errs(sink: &mut dyn Sink) {
    sink.write(&start(0)).unwrap();
    sink.write(&frag(0, "@@@")).unwrap();
    assert!(sink.write(&Event::ContentStop { index: 0 }).is_err());
}

#[test]
fn malformed_base64_is_an_err_out_of_write_for_both_sinks() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    malformed_stop_errs(&mut TextSink::new(&mut out, &mut err, false, tmp.path()));
    let (o, e): (&mut dyn Write, &mut dyn Write) = (&mut out, &mut err);
    let style = Style::Pretty { ascii: false };
    malformed_stop_errs(&mut PrettySink::new(o, e, false, style, tmp.path()));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}
