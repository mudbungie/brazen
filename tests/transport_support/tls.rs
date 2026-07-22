//! The TLS half of instrument B (transport spec §8.2): a raw ClientHello → a
//! JA3-form fingerprint string plus the ALPN offer, in order.
//!
//! JA3 form is `version,ciphers,extensions,groups,point-formats` (decimal,
//! dash-separated). The DIGEST is deliberately not taken: the string is the
//! fingerprint, and it diffs legibly in a fixture review — a hash would only tell a
//! reviewer that something changed, never what.
//!
//! GREASE values (RFC 8701) are dropped, as JA3 specifies, so a stack that varies
//! them per connection still fingerprints stably.
#![allow(dead_code)]

/// A ClientHello's server-observable identity, in parts: the legacy version, the
/// offered cipher suites and extension types IN THE ORDER SENT, the supported groups
/// and point formats, and the ALPN offer.
#[derive(Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub version: u16,
    pub ciphers: Vec<u16>,
    pub extensions: Vec<u16>,
    pub groups: Vec<u16>,
    pub formats: Vec<u16>,
    pub alpn: Vec<String>,
}

impl Fingerprint {
    /// The JA3-form string as sent — `version,ciphers,extensions,groups,formats`.
    /// **Not** stable per connection for every stack: rustls deliberately SHUFFLES
    /// its extension order, which is exactly why the committed capture uses
    /// [`Self::to_capture`] instead.
    pub fn ja3(&self) -> String {
        [
            self.version.to_string(),
            decimals(&self.ciphers),
            decimals(&self.extensions),
            decimals(&self.groups),
            decimals(&self.formats),
        ]
        .join(",")
    }

    /// The committed-capture form: the JA3 fields with the extension list SORTED, so
    /// the capture pins the offer (which extensions, which ciphers, which ALPN)
    /// rather than a per-connection permutation, plus the ALPN offer in order.
    pub fn to_capture(&self) -> String {
        let mut sorted = self.extensions.clone();
        sorted.sort_unstable();
        format!(
            "ja3_sorted_extensions={},{},{},{},{}\nalpn={}\n",
            self.version,
            decimals(&self.ciphers),
            decimals(&sorted),
            decimals(&self.groups),
            decimals(&self.formats),
            self.alpn.join(",")
        )
    }
}

/// Parse the first flight. `None` if the bytes are not a TLS handshake ClientHello
/// (a truncated or non-TLS connection), so a caller can fail loudly rather than
/// compare against an empty string.
pub fn fingerprint(flight: &[u8]) -> Option<Fingerprint> {
    // Record header: type 0x16 (handshake), 2-byte version, 2-byte length.
    let body = flight.get(5..)?;
    if flight.first() != Some(&0x16) || body.first() != Some(&0x01) {
        return None;
    }
    let mut c = Cursor {
        b: body.get(4..)?,
        i: 0,
    };
    let version = c.u16()?;
    c.skip(32)?; // random
    let session = c.u8()? as usize;
    c.skip(session)?;
    let ciphers = pairs(c.slice_u16_len()?);
    let methods = c.u8()? as usize;
    c.skip(methods)?;
    let ext_block = c.slice_u16_len()?;

    let (mut types, mut groups, mut formats, mut alpn) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut e = Cursor { b: ext_block, i: 0 };
    while let Some(kind) = e.u16() {
        let data = e.slice_u16_len()?;
        if !is_grease(kind) {
            types.push(kind);
        }
        match kind {
            10 => groups = pairs(data.get(2..).unwrap_or_default()),
            11 => {
                formats = data
                    .get(1..)
                    .unwrap_or_default()
                    .iter()
                    .map(|b| u16::from(*b))
                    .collect()
            }
            16 => alpn = alpn_names(data),
            _ => {}
        }
    }
    Some(Fingerprint {
        version,
        ciphers,
        extensions: types,
        groups,
        formats,
        alpn,
    })
}

/// ALPN extension data: 2-byte list length, then length-prefixed names.
fn alpn_names(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut c = Cursor {
        b: data.get(2..).unwrap_or_default(),
        i: 0,
    };
    while let Some(len) = c.u8() {
        match c.take(len as usize) {
            Some(name) => out.push(String::from_utf8_lossy(name).into_owned()),
            None => break,
        }
    }
    out
}

fn pairs(b: &[u8]) -> Vec<u16> {
    b.chunks_exact(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .filter(|v| !is_grease(*v))
        .collect()
}

fn decimals(v: &[u16]) -> String {
    v.iter().map(u16::to_string).collect::<Vec<_>>().join("-")
}

/// RFC 8701 GREASE: `0x?A?A` with both bytes equal.
fn is_grease(v: u16) -> bool {
    v.to_be_bytes()[0] == v.to_be_bytes()[1] && v & 0x0f0f == 0x0a0a
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(out)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    /// A 2-byte-length-prefixed block, returned without its length prefix.
    fn slice_u16_len(&mut self) -> Option<&'a [u8]> {
        let n = self.u16()? as usize;
        self.take(n)
    }
}
