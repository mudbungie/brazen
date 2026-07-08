//! Direct table tests for the three framers (sse §6–§8): the edge branches the
//! fixture-driven determinism harness does not exercise — comment keep-alives,
//! CRLF lines, colon-less and value-space variants, multi-`data:` concatenation,
//! the `finish` flush of an unterminated tail, and Identity passthrough.

use crate::{Frame, Framing};

#[test]
fn identity_passes_each_chunk_through_verbatim() {
    let mut d = Framing::Identity.decoder();
    let first = d.push(b"abc".to_vec()).unwrap();
    assert_eq!(
        first,
        vec![Frame {
            event: None,
            data: b"abc".to_vec(),
            status: None,
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

/// A leading UTF-8 BOM (`EF BB BF`) is stripped at stream start (WHATWG SSE, §6.1) so
/// the first field name is not corrupted. For the OpenAI dialect the first block is a
/// bare `data:` with no `event:`, so an unstripped BOM would drop the WHOLE first frame.
#[test]
fn sse_strips_leading_bom_whole() {
    let mut d = Framing::Sse.decoder();
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"data: hi\n\n");
    let frames = d.push(bytes).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].event, None); // bare `data:` — the OpenAI shape the BOM would kill
    assert_eq!(frames[0].data, b"hi");
}

/// The BOM stripped even when the transport cut it byte-by-byte (§6.1/§10): each partial
/// prefix buffers and yields nothing until the mark completes, then framing is identical.
#[test]
fn sse_strips_bom_split_across_chunks() {
    let mut d = Framing::Sse.decoder();
    assert!(d.push(vec![0xEF]).unwrap().is_empty()); // an incomplete BOM prefix — waits
    assert!(d.push(vec![0xBB]).unwrap().is_empty());
    assert!(d.push(vec![0xBF]).unwrap().is_empty()); // BOM now complete → stripped
    let frames = d.push(b"data: hi\n\n".to_vec()).unwrap();
    assert_eq!(frames[0].data, b"hi");
    // A later `EF BB BF` is ordinary data, never a second BOM.
    let more = d.push(b"data: \xEF\xBB\xBFx\n\n".to_vec()).unwrap();
    assert_eq!(more[0].data, vec![0xEF, 0xBB, 0xBF, b'x']);
}

/// A `\r\n\r\n` terminator split across two chunks is still found: the scan resumes 3
/// bytes back (its longest incomplete prefix, `\r\n\r`), so the O(n^2)-avoiding offset
/// (§6.2) never skips a straddling boundary.
#[test]
fn sse_finds_crlf_terminator_straddling_a_chunk_boundary() {
    let mut d = Framing::Sse.decoder();
    assert!(d.push(b"data: x\r\n\r".to_vec()).unwrap().is_empty()); // 3 bytes of the CRLF term
    let frames = d.push(b"\n".to_vec()).unwrap(); // the final `\n` completes it
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].data, b"x");
}

/// A frame delivered in many small chunks frames once, at the same bytes, whatever the
/// resume offset (§6.2) — the incremental counterpart of the whole-push case.
#[test]
fn sse_reassembles_a_frame_pushed_in_pieces() {
    let mut d = Framing::Sse.decoder();
    assert!(d.push(b"da".to_vec()).unwrap().is_empty());
    assert!(d.push(b"ta: hel".to_vec()).unwrap().is_empty());
    assert!(d.push(b"lo\n".to_vec()).unwrap().is_empty());
    let frames = d.push(b"\n".to_vec()).unwrap();
    assert_eq!(frames[0].data, b"hello");
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
