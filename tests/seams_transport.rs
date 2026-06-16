//! Seams: the single impure surface, exercised through `MockTransport` — a fixed
//! status, a canned body that may carry an injected mid-stream error, and request
//! capture for end-to-end encode+auth assertions (arch §4.1, §9.1).

use std::io;

use brazen::testing::{Chunk, MockTransport};
use brazen::{Transport, WireRequest};

#[test]
fn mock_transport_replays_status_body_and_injected_error() {
    let mock = MockTransport::new(
        200,
        vec![
            Chunk::Data(b"first".to_vec()),
            Chunk::Fail(io::ErrorKind::ConnectionReset),
        ],
    );

    let mut wire = WireRequest::new("https://api.example", b"body".to_vec());
    wire.set_header("authorization", "Bearer tok");

    let resp = mock.send(wire.clone()).unwrap();
    assert_eq!(resp.status, 200);

    let chunks: Vec<io::Result<Vec<u8>>> = resp.body.collect();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].as_ref().unwrap(), b"first");
    assert_eq!(
        chunks[1].as_ref().unwrap_err().kind(),
        io::ErrorKind::ConnectionReset
    );

    // The transport captured exactly the wire it was sent.
    assert_eq!(mock.requests(), vec![wire]);
}

#[test]
fn mock_transport_ok_constructor_and_dyn_dispatch() {
    let mock = MockTransport::ok(vec![b"a", b"b"]);
    let transport: &dyn Transport = &mock;
    let resp = transport
        .send(WireRequest::new("https://api.example", Vec::new()))
        .unwrap();
    assert_eq!(resp.status, 200);
    let body: Vec<io::Result<Vec<u8>>> = resp.body.collect();
    assert_eq!(body.len(), 2);
    assert_eq!(body[0].as_ref().unwrap(), b"a");
    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn mock_transport_with_no_requests_yet_is_empty() {
    let mock = MockTransport::new(500, vec![]);
    assert!(mock.requests().is_empty());
}
