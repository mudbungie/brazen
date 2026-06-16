//! Canonical-in parsing tests (§5.5, §8): a well-formed request decodes to the
//! one `CanonicalRequest`; malformed bytes are a `ParseInput` error that exits
//! 64 and never panics.

use std::io::Cursor;

use brazen::{parse, Content, ErrorKind, Message, Role};

#[test]
fn parses_a_canonical_request() {
    let bytes = br#"{"model":"claude","messages":[{"role":"user","content":"hi"}]}"#;
    let req = parse(&mut Cursor::new(bytes.to_vec())).unwrap();
    assert_eq!(req.model, "claude");
    assert_eq!(
        req.messages,
        vec![Message {
            role: Role::User,
            content: vec![Content::Text("hi".into())],
        }]
    );
}

#[test]
fn malformed_input_is_parse_input_exit_64() {
    let err = parse(&mut Cursor::new(b"{ not json".to_vec())).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ParseInput);
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.starts_with("malformed canonical request:"));
}
