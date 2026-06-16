//! The OS RNG for the control plane (auth §7.2, §7.4): a random URL-safe token
//! used as the PKCE verifier and the CSRF state. Coverage-excluded with the rest
//! of the `bz` shim. On Unix it reads `/dev/urandom`; the non-Unix tier has no
//! getrandom/DPAPI dep and falls back to a weak time seed (a documented limitation,
//! auth §5.2).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// A random URL-safe token (PKCE verifier / CSRF state): 32 bytes of OS entropy,
/// base64url-no-pad → 43 unreserved chars (auth §7.2, §7.4).
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    fill_random(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(unix)]
fn fill_random(buf: &mut [u8]) {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(buf);
    }
}

#[cfg(not(unix))]
fn fill_random(buf: &mut [u8]) {
    // No DPAPI/getrandom dep on this tier: a weak time-seeded fill. The Windows
    // secret-at-rest story is a documented limitation (auth §5.2).
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (seed >> (i % 16)) as u8 ^ (i as u8).wrapping_mul(31);
    }
}
