//! Provider-row generation query data (config §4.3.1): the shared encoded/raw
//! tail appends encoded pairs after the protocol-owned path, while an empty list
//! is the byte-identical identity and an existing protocol query receives `&`.

use crate::append_query;
use crate::tests::run_support::{empty_store, go, ok_basic, temp};

const ROW: &str = r#"
[[provider]]
name = "query-test"
base_url = "https://example.test"
protocol = "anthropic_messages"
auth = "none"
generation_query = [["beta", "true"], ["space key", "a/b"]]
body_defaults = { max_tokens = 8 }
"#;

#[test]
fn encoded_and_raw_generation_share_the_encoded_query_tail() {
    let cfg = temp(ROW);
    let expected = "https://example.test/v1/messages?beta=true&space%20key=a%2Fb";

    for (argv, body) in [
        (
            vec![
                "--provider",
                "query-test",
                "--model",
                "claude-test",
                "prompt",
            ],
            &b""[..],
        ),
        (
            vec!["--raw=in", "--provider", "query-test"],
            &b"{\"stream\":true}"[..],
        ),
    ] {
        let tx = ok_basic();
        let out = go(
            &argv,
            &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
            body,
            &tx,
            &empty_store(),
        );
        assert_eq!(out.code, 0);
        assert_eq!(tx.requests()[0].url, expected);
    }
}

#[test]
fn append_query_handles_empty_existing_and_open_queries() {
    let pairs = vec![("next".into(), "a b".into())];
    for (start, expected) in [
        ("https://x/path", "https://x/path?next=a%20b"),
        (
            "https://x/path?alt=sse",
            "https://x/path?alt=sse&next=a%20b",
        ),
        ("https://x/path?", "https://x/path?next=a%20b"),
        (
            "https://x/path?alt=sse&",
            "https://x/path?alt=sse&next=a%20b",
        ),
    ] {
        let mut url = start.to_owned();
        append_query(&mut url, &pairs);
        assert_eq!(url, expected);
    }

    let mut unchanged = "https://x/path".to_owned();
    append_query(&mut unchanged, &[]);
    assert_eq!(unchanged, "https://x/path");
}
