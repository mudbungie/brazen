//! Direct table tests for the three framers (sse §6–§8): the edge branches the
//! fixture-driven determinism harness does not exercise — comment keep-alives,
//! CRLF lines, colon-less and value-space variants, multi-`data:` concatenation,
//! the `finish` flush of an unterminated tail, and Identity passthrough.

use brazen::{Frame, Framing};

#[test]
fn identity_passes_each_chunk_through_verbatim() {
    let mut d = Framing::Identity.decoder();
    let first = d.push(b"abc".to_vec()).unwrap();
    assert_eq!(
        first,
        vec![Frame {
            event: None,
            data: b"abc".to_vec(),
            whole_body: false,
        }]
    );
    // Raw bytes (not UTF-8) ride through untouched; one frame per chunk.
    let second = d.push(vec![0xff, 0x00, 0x9f]).unwrap();
    assert_eq!(second[0].data, vec![0xff, 0x00, 0x9f]);
    assert!(d.finish().unwrap().is_empty()); // nothing buffered to flush
}

#[test]
fn sse_comment_keepalive_yields_no_frame() {
    let mut d = Framing::Sse.decoder();
    // A pure-comment block contributes no frame (parse_block -> None).
    assert!(d.push(b": keep-alive\n\n".to_vec()).unwrap().is_empty());
    // ...and does not swallow the next real block.
    let frames = d.push(b"data: {\"x\":1}\n\n".to_vec()).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].data, b"{\"x\":1}");
    assert_eq!(frames[0].event, None);
}

#[test]
fn sse_tolerates_crlf_ignored_fields_and_no_space() {
    let mut d = Framing::Sse.decoder();
    // CRLF boundary (\r\n\r\n), an ignored `id:` field, and a space-less `data:hi`.
    let frames = d
        .push(b"event: ping\r\nid: 7\r\ndata:hi\r\n\r\n".to_vec())
        .unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].event.as_deref(), Some("ping"));
    assert_eq!(frames[0].data, b"hi");
}

#[test]
fn sse_concatenates_multiple_data_lines() {
    let mut d = Framing::Sse.decoder();
    // Two `data:` lines in one block join with a single `\n` (the SSE spec).
    let frames = d.push(b"data: a\ndata: b\n\n".to_vec()).unwrap();
    assert_eq!(frames[0].data, b"a\nb");
}

#[test]
fn sse_finish_flushes_unterminated_tail_then_empties() {
    let mut d = Framing::Sse.decoder();
    // A partial block buffers and yields nothing until completed.
    assert!(d.push(b"data: hel".to_vec()).unwrap().is_empty());
    // finish recovers the blank-line-unterminated final block (a server quirk).
    let flushed = d.finish().unwrap();
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].data, b"hel");
    // A second finish has nothing left -> no frame (the dropped-partial branch).
    assert!(d.finish().unwrap().is_empty());
}

#[test]
fn ndjson_splits_lines_skips_blanks_strips_cr() {
    let mut d = Framing::Ndjson.decoder();
    // A blank line yields no frame; a CRLF line drops its trailing `\r`.
    let frames = d.push(b"{\"a\":1}\n\n{\"b\":2}\r\n".to_vec()).unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].data, b"{\"a\":1}");
    assert_eq!(frames[1].data, b"{\"b\":2}");
}

#[test]
fn ndjson_finish_flushes_newline_less_final_line() {
    let mut d = Framing::Ndjson.decoder();
    assert!(d.push(b"{\"c\":3}".to_vec()).unwrap().is_empty()); // partial, buffered
    let tail = d.finish().unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].data, b"{\"c\":3}");
    assert!(d.finish().unwrap().is_empty()); // empty buf -> no frame
}
