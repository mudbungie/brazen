//! A model-RETURNED image on the Google wire (providers §4.4, bl-0987): a
//! `parts[].inlineData{mimeType,data}` part synthesizes `ContentStart{Image}` then ONE
//! `ImageDelta`, left open to close at the terminal drain — the `functionCall`
//! discipline. Before this arm the part fell through the loop and the image was
//! silently dropped. Inline SSE bytes (the shape the image models answer with when
//! `responseModalities` includes `"IMAGE"`; no live capture — the operator's key is
//! free-tier, providers §9 CR-Img); identical under one-byte rechunking; the same
//! through the non-stream `decode_full` fold. No network.

use crate::protocol::google_genai::GoogleGenAi;
use crate::tests::decode_full_support::full;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage};

const STREAM: &[u8] = b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Here\"},{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}}]},\"index\":0}],\"modelVersion\":\"gemini-2.5-flash-image\"}\n\n\
data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":2}}\n\n";

const BARE: &[u8] = b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"inlineData\":{}}]},\"finishReason\":\"STOP\",\"index\":0}]}\n\n";

const FULL: &[u8] = b"{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":2},\"modelVersion\":\"gemini-2.5-flash-image\"}";

fn decode_all(bytes: &[u8], one_byte: bool) -> Vec<Event> {
    let mut dec = Framing::Sse.decoder();
    let mut frames = Vec::new();
    if one_byte {
        for b in bytes {
            frames.extend(dec.push(vec![*b]).unwrap());
        }
    } else {
        frames.extend(dec.push(bytes.to_vec()).unwrap());
    }
    frames.extend(dec.finish().unwrap());
    let mut state = DecodeState::default();
    let mut events = Vec::new();
    for f in frames {
        events.extend(GoogleGenAi.decode(f, &mut state).unwrap());
    }
    assert!(state.terminated, "the finishReason chunk is the terminator");
    events.push(Event::End);
    events
}

fn image_start(index: u32) -> Event {
    Event::ContentStart {
        index,
        kind: ContentKind::Image {
            media_type: "image/png".into(),
        },
    }
}

fn image_delta(index: u32) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::ImageDelta("iVBORw0KGgo=".into()),
    }
}

fn usage() -> Event {
    Event::Usage(Usage {
        input_tokens: Some(5),
        output_tokens: Some(2),
        input_total_tokens: Some(5),
        ..Default::default()
    })
}

#[test]
fn inline_data_part_opens_an_image_block_that_closes_at_the_drain() {
    let whole = decode_all(STREAM, false);
    assert_eq!(
        decode_all(STREAM, true),
        whole,
        "diverged under one-byte rechunk"
    );
    assert_eq!(
        whole,
        vec![
            Event::message_start(None, Some("gemini-2.5-flash-image".into()), Role::Assistant),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {},
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Here".into()),
            },
            // identity (media type) at open, the bytes as ONE base64 delta — the whole
            // part arrives in one chunk, exactly like functionCall
            image_start(1),
            image_delta(1),
            // both blocks stay open until the finishReason chunk drains them ascending
            Event::ContentStop { index: 0 },
            Event::ContentStop { index: 1 },
            usage(),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn inline_data_without_fields_still_opens_with_empty_identity() {
    // The lenient path: a bare `inlineData:{}` reads its two fields as "" — identity
    // still precedes content, and the block still closes.
    assert_eq!(
        decode_all(BARE, false),
        vec![
            Event::message_start(None, None, Role::Assistant),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Image {
                    media_type: String::new(),
                },
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::ImageDelta(String::new()),
            },
            Event::ContentStop { index: 0 },
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn non_stream_body_folds_the_image_through_the_same_arm() {
    let (ev, term) = full(&GoogleGenAi, FULL);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            Event::message_start(None, Some("gemini-2.5-flash-image".into()), Role::Assistant),
            image_start(0),
            image_delta(0),
            Event::ContentStop { index: 0 },
            usage(),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}
