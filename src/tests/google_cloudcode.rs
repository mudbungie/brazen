//! The Cloud Code (Antigravity) envelope dialect (providers §4.10, bl-cbd4): `google_genai`'s
//! body wrapped as `{"model","request"}` on `/v1internal:…`, and its chunk fold behind a
//! `{"response":…}` unwrap. The fixtures are the shapes captured live on 2026-09-25
//! (auth §11.4), with the base64/signature payloads replaced by `notreal` stand-ins. The
//! proof of "nothing Google-shaped is restated" is equality: every wrapped stream decodes
//! to EXACTLY what `GoogleGenAi` yields for the same bytes unwrapped. No network.

use serde_json::{json, Value};

use crate::config::provider::ProtocolId;
use crate::protocol::google_cloudcode::GoogleCloudCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::tests::decode_full_support::full;
use crate::{
    CanonicalRequest, Content, ContentKind, DecodeState, Delta, ErrorKind, Event, FinishReason,
    Frame, Framing, Message, Protocol, ProviderCtx, Role,
};

/// One live text turn: a leading empty thought part, two text deltas, a closing
/// `thoughtSignature` part beside the `finishReason` — each under the envelope.
const TEXT: &[u8] = b"data: {\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"thought\": true,\"text\": \"\"}]}}],\"usageMetadata\": {\"promptTokenCount\": 19,\"totalTokenCount\": 19},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"notreal-1\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}\n\n\
data: {\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"text\": \"ok\"}]}}],\"usageMetadata\": {\"promptTokenCount\": 19,\"candidatesTokenCount\": 1,\"totalTokenCount\": 20},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"notreal-1\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}\n\n\
data: {\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"text\": \" then\"}]}}],\"usageMetadata\": {\"promptTokenCount\": 19,\"candidatesTokenCount\": 3,\"totalTokenCount\": 22},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"notreal-1\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}\n\n\
data: {\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"thoughtSignature\": \"notreal-sig\",\"text\": \"\"}]},\"finishReason\": \"STOP\"}],\"usageMetadata\": {\"promptTokenCount\": 19,\"candidatesTokenCount\": 3,\"totalTokenCount\": 22},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"notreal-1\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}\n\n";

/// The image model's answer: ONE part carrying both a (huge, live) `thoughtSignature`
/// and the `inlineData` JPEG, then an empty text part with the terminator.
const IMAGE: &[u8] = b"data: {\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"thoughtSignature\": \"notreal-sig\",\"inlineData\": {\"mimeType\": \"image/jpeg\",\"data\": \"/9j/notreal=\"}}]}}],\"usageMetadata\": {\"promptTokenCount\": 19,\"candidatesTokenCount\": 1437,\"totalTokenCount\": 1456},\"modelVersion\": \"gemini-3.1-flash-image\",\"responseId\": \"notreal-2\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}\n\n\
data: {\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"text\": \"\"}]},\"finishReason\": \"STOP\"}],\"usageMetadata\": {\"promptTokenCount\": 19,\"candidatesTokenCount\": 1437,\"totalTokenCount\": 1456},\"modelVersion\": \"gemini-3.1-flash-image\",\"responseId\": \"notreal-2\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}\n\n";

/// The non-stream body: the same envelope around one whole response.
const FULL: &[u8] = b"{\"response\": {\"candidates\": [{\"content\": {\"role\": \"model\",\"parts\": [{\"thought\": true,\"text\": \"\"},{\"thoughtSignature\": \"notreal-sig\",\"text\": \"ok\"}]},\"finishReason\": \"STOP\"}],\"usageMetadata\": {\"promptTokenCount\": 5,\"candidatesTokenCount\": 1,\"totalTokenCount\": 6},\"modelVersion\": \"gemini-2.5-flash\",\"responseId\": \"notreal-3\"},\"traceId\": \"notreal-trace\",\"metadata\": {}}";

/// The live 403 the backend answers a non-`antigravity` User-Agent with — UNWRAPPED.
const DENIED: &[u8] = br#"{"error": {"code": 403,"message": "You do not have a valid license of this product. (#3501)","status": "PERMISSION_DENIED"}}"#;

fn ctx() -> ProviderCtx<'static> {
    ProviderCtx {
        base_url: "https://daily-cloudcode-pa.googleapis.com",
        model: "gemini-2.5-flash",
        beta_headers: &[],
        exec: None,
    }
}

fn request(stream: bool) -> CanonicalRequest {
    let mut req = CanonicalRequest {
        model: "gemini-2.5-flash".into(),
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text("hi".into())],
        }],
        temperature: Some(0.5),
        stream: Some(stream),
        ..Default::default()
    };
    req.extra.insert(
        "generationConfig".into(),
        json!({"responseModalities": ["TEXT", "IMAGE"]}),
    );
    req
}

/// Every wrapped `data:` line with its envelope stripped — what the direct wire
/// would have carried — so equality proves the unwrap is the dialect's whole decode.
fn unwrapped(sse: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in std::str::from_utf8(sse).unwrap().split("\n\n") {
        if let Some(body) = line.strip_prefix("data: ") {
            let v: Value = serde_json::from_str(body).unwrap();
            out.extend_from_slice(b"data: ");
            out.extend_from_slice(v["response"].to_string().as_bytes());
            out.extend_from_slice(b"\n\n");
        }
    }
    out
}

fn decode_all(proto: &dyn Protocol, bytes: &[u8], one_byte: bool) -> (Vec<Event>, bool) {
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
        events.extend(proto.decode(f, &mut state).unwrap());
    }
    (events, state.terminated)
}

fn text_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ContentDelta {
                delta: Delta::TextDelta(t),
                ..
            } => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn encode_wraps_the_google_body_in_a_two_key_envelope_with_no_model_in_the_path() {
    let wire = GoogleCloudCode.encode(&request(true), &ctx()).unwrap();
    assert_eq!(
        wire.url,
        "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
    );
    let body: Value = serde_json::from_slice(&wire.body).unwrap();
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "request"], "exactly the two envelope keys");
    assert_eq!(body["model"], "gemini-2.5-flash");
    // The inner body IS §4.2's: typed knobs and the `extra` fold both land INSIDE
    // `request`, merged one level (the same `body_map`, called not copied).
    assert_eq!(body["request"]["contents"][0]["parts"][0]["text"], "hi");
    assert_eq!(body["request"]["generationConfig"]["temperature"], 0.5);
    assert_eq!(
        body["request"]["generationConfig"]["responseModalities"],
        json!(["TEXT", "IMAGE"])
    );
    let direct: Value =
        serde_json::from_slice(&GoogleGenAi.encode(&request(true), &ctx()).unwrap().body).unwrap();
    assert_eq!(
        body["request"], direct,
        "the inner body is byte-identical to §4.2"
    );
    // Non-stream picks the sibling verb; `path()` (the --raw default) is the streaming one.
    let plain = GoogleCloudCode.encode(&request(false), &ctx()).unwrap();
    assert_eq!(
        plain.url,
        "https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent"
    );
    assert_eq!(
        GoogleCloudCode.path(&ctx()),
        "/v1internal:streamGenerateContent?alt=sse"
    );
}

#[test]
fn a_wrapped_text_stream_decodes_exactly_as_the_unwrapped_google_wire() {
    let (whole, terminated) = decode_all(&GoogleCloudCode, TEXT, false);
    assert!(terminated, "the finishReason chunk is the terminator");
    assert_eq!(
        decode_all(&GoogleCloudCode, TEXT, true).0,
        whole,
        "diverged under one-byte rechunk"
    );
    assert_eq!(
        whole,
        decode_all(&GoogleGenAi, &unwrapped(TEXT), false).0,
        "the unwrap is the whole difference"
    );
    assert_eq!(
        whole[0],
        Event::message_start(None, Some("gemini-2.5-flash".into()), Role::Assistant)
    );
    assert_eq!(text_of(&whole), "ok then");
    assert!(
        matches!(
            whole.last(),
            Some(Event::Finish {
                reason: FinishReason::Stop
            })
        ),
        "{whole:?}"
    );
}

#[test]
fn the_image_part_beside_its_signature_yields_an_image_block_closed_at_the_drain() {
    let (events, terminated) = decode_all(&GoogleCloudCode, IMAGE, false);
    assert!(terminated);
    assert_eq!(events, decode_all(&GoogleGenAi, &unwrapped(IMAGE), false).0);
    let start = events.iter().position(|e| {
        matches!(e, Event::ContentStart { kind: ContentKind::Image { media_type }, .. } if media_type == "image/jpeg")
    });
    let start = start.expect("an image block opens");
    assert_eq!(
        events[start + 1],
        Event::ContentDelta {
            index: 0,
            delta: Delta::ImageDelta("/9j/notreal=".into()),
        }
    );
    assert!(
        events[start + 2..].contains(&Event::ContentStop { index: 0 }),
        "closed at the terminal drain: {events:?}"
    );
}

#[test]
fn the_non_stream_body_unwraps_once_into_the_same_fold() {
    let (ev, term) = full(&GoogleCloudCode, FULL);
    assert!(term);
    let v: Value = serde_json::from_slice(FULL).unwrap();
    let (direct, _) = full(&GoogleGenAi, v["response"].to_string().as_bytes());
    assert_eq!(ev, direct);
    assert_eq!(text_of(&ev), "ok");
}

#[test]
fn errors_and_unwrapped_chunks_take_the_google_path_unchanged() {
    // The live 403 (a non-`antigravity` User-Agent): a whole-body non-2xx frame, its
    // body UNWRAPPED — status-authoritative, so PERMISSION_DENIED → Auth → 77.
    let denied = |proto: &dyn Protocol| {
        let frame = Frame {
            event: None,
            data: DENIED.to_vec(),
            status: Some(403),
        };
        proto.decode(frame, &mut DecodeState::default()).unwrap()
    };
    let ev = denied(&GoogleCloudCode);
    assert_eq!(ev, denied(&GoogleGenAi));
    match &ev[0] {
        Event::Error(e) => {
            assert_eq!(e.kind, ErrorKind::Auth);
            assert_eq!(e.exit_code(), 77);
        }
        other => panic!("expected Error, got {other:?}"),
    }
    // A mid-stream `{"error":…}` chunk on a 2xx stream, and a chunk that arrives with
    // NO envelope at all: both are the value itself, so both are §4's own decode.
    for body in [
        DENIED,
        br#"{"candidates":[{"content":{"role":"model","parts":[{"text":"bare"}]},"finishReason":"STOP"}]}"#,
    ] {
        let one = |proto: &dyn Protocol| {
            let frame = Frame {
                event: None,
                data: body.to_vec(),
                status: None,
            };
            proto.decode(frame, &mut DecodeState::default()).unwrap()
        };
        assert_eq!(one(&GoogleCloudCode), one(&GoogleGenAi));
    }
}

#[test]
fn the_dialect_is_data_selected_and_declines_what_it_cannot_prove() {
    assert_eq!(
        serde_json::from_str::<ProtocolId>("\"google_cloudcode\"").unwrap(),
        ProtocolId::GoogleCloudCode
    );
    assert_eq!(GoogleCloudCode.content_type(), "application/json");
    assert!(matches!(GoogleCloudCode.framing(), Framing::Sse));
    assert_eq!(GoogleCloudCode.tuning(), GoogleGenAi.tuning());
    assert_eq!(GoogleCloudCode.shapes(), GoogleGenAi.shapes());
    // POST-only, map-shaped listing → no `--list-models`; count_tokens is the default decline.
    assert!(GoogleCloudCode.models_shape().is_none());
    assert!(GoogleCloudCode
        .count_tokens(&request(true), &ctx())
        .is_none());
}
