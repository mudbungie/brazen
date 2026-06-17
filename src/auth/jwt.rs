//! Minimal, UNVERIFIED JWT payload reads (auth §10.3, §10.4). We are not the
//! token's audience, so we never verify the signature — we only read our OWN
//! token's stated `exp` (to schedule refresh when the token endpoint returns no
//! `expires_in`, as OpenAI's does) and the OpenAI `chatgpt_account_id` claim
//! (echoed as a data-plane header). Pure: base64url-decode the payload segment and
//! read a field, table-tested through `parse_token_response` from literal JWTs
//! (auth §8, §10.6). The authorization server enforces real validity; a
//! malformed/opaque token here simply yields `None`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;

/// Decode a JWT's payload — the middle of `header.payload.signature` — to JSON, or
/// `None` when the token is not a three-segment base64url JWT with a JSON payload.
fn payload(token: &str) -> Option<Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The token's absolute `exp` (unix seconds) when present and numeric (auth §10.3).
/// Already absolute — the caller does NOT add `now`.
pub(super) fn jwt_exp(token: &str) -> Option<u64> {
    payload(token)?.get("exp")?.as_u64()
}

/// OpenAI's `chatgpt_account_id`, nested under the id_token's
/// `https://api.openai.com/auth` claim (auth §10.4) — echoed as the
/// `ChatGPT-Account-ID` data-plane header.
pub(super) fn jwt_account_id(token: &str) -> Option<String> {
    payload(token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_owned)
}
