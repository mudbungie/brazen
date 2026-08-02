//! End-to-end `-f` media-attach coverage against the simulated providers (bl-d264,
//! the integration side of bl-b8ef's unit tests).
//!
//! The unit suite pins `read_files` (path → part) and the encode suites pin each
//! dialect's projection; what nothing else drives is the WHOLE chain through the
//! real binary — argv `-f` parsing → media detection → base64 → encode → the actual
//! HTTP request body on the wire. [`FakeProvider::capture`] hands that body back,
//! so each test asserts what the provider would have RECEIVED: the attachment's
//! bytes as standard base64, and the media type where the dialect keeps one
//! (Ollama's bare `images` array drops it). Ollama's no-document-slot narrowing is
//! pinned as behavior, not just a message: exit 64 with NO request reaching the
//! server. No real provider, no key; runs in plain `cargo test`.

#[allow(dead_code)]
#[path = "live_support/exec.rs"]
mod exec;
#[allow(dead_code)]
mod sim_support;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sim_support::{config_for, fixture, FakeProvider, Sim, PROVIDERS};

/// Deliberately non-UTF-8 payloads: the text path would refuse these (exit 66), so
/// a request that carries them proves the media path — not text — was taken.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef,
];
const PDF: &[u8] = b"%PDF-1.7\n\xde\xad\xbe\xef\ntrailer";

/// The attachment fixtures, extension-named in a kept-alive temp dir.
fn attachments(dir: &tempfile::TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let write = |name: &str, bytes: &[u8]| {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).expect("write attachment");
        p
    };
    (
        write("red.png", PNG),
        write("report.pdf", PDF),
        write("note.txt", b"from-note"),
    )
}

/// Drive the real `bz` against a body-capturing fake for `p`, attaching `files`
/// before the prompt. Returns (exit, stdout, stderr, captured-request channel).
fn drive(p: &Sim, files: &[&Path]) -> (i32, String, String, Receiver<Vec<u8>>) {
    let (server, rx) = FakeProvider::capture(p.content_type, fixture(p.fixture));
    let cfg = config_for(p, &server.base_url());
    let mut args = vec![
        "--config".to_string(),
        cfg.path().to_string_lossy().into_owned(),
        "--provider".into(),
        p.name.into(),
        "--model".into(),
        p.model.into(),
    ];
    if p.auth != "none" {
        args.push("--api-key".into());
        args.push("sk-sim-dummy".into());
    }
    for f in files {
        args.push("-f".into());
        args.push(f.to_string_lossy().into_owned());
    }
    args.push("what is attached?".into());
    let (code, out, err) = exec::run_bz(&args, "");
    (code, out, err, rx)
}

/// The one captured request as text (every dialect's body is JSON, so UTF-8).
fn wire_body(rx: &Receiver<Vec<u8>>, who: &str) -> String {
    let body = rx
        .try_recv()
        .unwrap_or_else(|_| panic!("{who}: no request reached the fake provider"));
    String::from_utf8(body).unwrap_or_else(|e| panic!("{who}: non-UTF-8 request body: {e}"))
}

/// `bz -f red.png -f note.txt "…"` → every provider's wire body carries the PNG's
/// bytes as standard base64, the media type where the dialect keeps one (all but
/// Ollama), and the text attachment + prompt alongside — the mixed-part message
/// through the real argv → detection → encode → HTTP chain.
#[test]
fn png_attachment_reaches_every_provider_wire_as_base64() {
    let dir = tempfile::tempdir().unwrap();
    let (png, _, txt) = attachments(&dir);
    for p in PROVIDERS {
        let (code, out, err, rx) = drive(p, &[&png, &txt]);
        assert_eq!(code, 0, "{}: exit {code} (stderr: {err})\n{out}", p.name);
        let body = wire_body(&rx, p.name);
        assert!(
            body.contains(&STANDARD.encode(PNG)),
            "{}: wire body lacks the png base64: {body}",
            p.name
        );
        let keeps_media_type = p.protocol != "ollama_chat"; // bare `images`, type dropped
        assert_eq!(
            body.contains("image/png"),
            keeps_media_type,
            "{}: media-type presence should be {keeps_media_type}: {body}",
            p.name
        );
        assert!(
            body.contains("from-note") && body.contains("what is attached?"),
            "{}: text part + prompt should ride alongside the image: {body}",
            p.name
        );
    }
}

/// `bz -f report.pdf "…"` → the four document-capable dialects carry the PDF as
/// base64 with its media type (Anthropic `document`, OpenAI data-URI `file`,
/// Responses `input_file`, Google `inlineData` — providers.md).
#[test]
fn pdf_attachment_reaches_every_document_dialect_as_base64() {
    let dir = tempfile::tempdir().unwrap();
    let (_, pdf, _) = attachments(&dir);
    for p in PROVIDERS.iter().filter(|p| p.protocol != "ollama_chat") {
        let (code, out, err, rx) = drive(p, &[&pdf]);
        assert_eq!(code, 0, "{}: exit {code} (stderr: {err})\n{out}", p.name);
        let body = wire_body(&rx, p.name);
        assert!(
            body.contains(&STANDARD.encode(PDF)) && body.contains("application/pdf"),
            "{}: wire body lacks the pdf base64 + media type: {body}",
            p.name
        );
    }
}

/// Ollama's chat wire has no document slot (providers.md §5.4 CR-O3): a PDF is an
/// encode-time reject — exit 64, the narrowing named on stderr, and, the part only
/// an integration test can pin, NO request ever reaching the provider.
#[test]
fn pdf_at_ollama_rejects_before_any_request_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (_, pdf, _) = attachments(&dir);
    let ollama = PROVIDERS.iter().find(|p| p.name == "ollama").unwrap();
    let (code, out, err, rx) = drive(ollama, &[&pdf]);
    assert_eq!(
        code, 64,
        "expected the encode reject (stderr: {err})\n{out}"
    );
    assert!(
        err.contains("document"),
        "the reject should name the missing document slot: {err}"
    );
    assert!(
        rx.try_recv().is_err(),
        "an encode reject must not reach the wire"
    );
}
