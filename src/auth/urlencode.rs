//! The `application/x-www-form-urlencoded` codec the OAuth wire builders share
//! (auth §7.4, §7.5): encode `(key, value)` pairs into a query body and split a
//! query back into decoded pairs, with an RFC 3986 percent-codec underneath. All
//! pure — table-tested from literals (auth §8).

/// `key=value&…`, every key and value percent-encoded (auth §7.4).
pub(super) fn encode_pairs(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Split a query into decoded `(key, value)` pairs; a bare `key` (no `=`) yields an
/// empty value, and an empty segment is dropped.
pub(super) fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|seg| !seg.is_empty())
        .map(|seg| match seg.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(seg), String::new()),
        })
        .collect()
}

/// RFC 3986 unreserved: never percent-encoded.
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

/// Percent-encode every non-unreserved byte (form/query value safe).
fn encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

/// Percent-decode (and `+` → space); a truncated or non-hex `%xx` is left literal.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match hex(bytes[i + 1], bytes[i + 2]) {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                }
                None => {
                    out.push(b'%');
                    i += 1;
                }
            },
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Two hex digits → a byte, or `None` if either is not a hex digit.
fn hex(hi: u8, lo: u8) -> Option<u8> {
    Some((nibble(hi)? << 4) | nibble(lo)?)
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
