//! Row completion (config §4.1, §7): lift the one routed, post-fold sparse row into
//! a complete `Provider` — surfacing each missing required field by name — and the
//! `take_*` helpers that pull the gen-scalar `body_defaults` off it first, leaving
//! only non-gen passthrough. The routing half (which row?) stays in the parent.

use serde_json::{Map, Value};

use crate::config::errors::ConfigError;
use crate::config::partial::PartialProvider;
use crate::config::provider::{AuthId, Provider};

use super::bad;

/// Lift a sparse, post-fold row into a complete `Provider`, surfacing each
/// missing required field by name (config §7 `IncompleteProvider`).
pub(super) fn complete(name: String, row: PartialProvider) -> Result<Provider, ConfigError> {
    let need = |field| ConfigError::IncompleteProvider {
        name: name.clone(),
        field,
    };
    // An exec-transport row substitutes `exec` for `base_url` (claude-code spec
    // §7.1): its absent host completes as `""` — the empty-set path, never read by
    // the exec transport. A row with NEITHER still surfaces the missing `base_url`.
    let base_url = match (row.base_url, row.exec.is_some()) {
        (Some(url), _) => url,
        (None, true) => String::new(),
        (None, false) => return Err(need("base_url")),
    };
    // `exec` (the child IS the provider, claude-code §3) and `[provider.transport]`
    // (the child IS the transport, transport §4.2) are the two readings of one
    // subprocess seam, so a row asking for both is a contradiction — surfaced here
    // (→78) rather than silently resolved in favour of either.
    if row.exec.is_some() && row.transport.is_some() {
        return Err(bad(
            "transport",
            "cannot ride a row that also sets `exec` (that row's child IS the \
             provider); drop one",
        ));
    }
    let protocol = row.protocol.ok_or_else(|| need("protocol"))?;
    let auth = row.auth.ok_or_else(|| need("auth"))?;
    // A keyed row MUST carry an `api_header`; an `auth = "none"` row carries none.
    // Pairing it here makes a keyed impl's `api_header.is_some()` an invariant, never
    // a runtime branch — the same discipline as `oauth` below.
    let api_header = row.api_header;
    if auth != AuthId::None && api_header.is_none() {
        return Err(need("api_header"));
    }
    // An `oauth2` row MUST carry an `oauth` block — resolution pairs the two or
    // fails here (auth §1.3), so the `OAuth2` impl's `oauth.is_some()` is an
    // invariant, never a runtime branch.
    if auth == AuthId::OAuth2 && row.oauth.is_none() {
        return Err(need("oauth"));
    }
    Ok(Provider {
        base_url,
        exec: row.exec,
        transport: row.transport,
        protocol,
        auth,
        api_header,
        beta_headers: row.beta_headers.unwrap_or_default(),
        generation_query: row.generation_query.unwrap_or_default(),
        model_aliases: row.model_aliases.unwrap_or_default(),
        unsupported_body_keys: row.unsupported_body_keys.unwrap_or_default(),
        // The discovery override carries verbatim (config §4.4): nothing to fold into a
        // typed scalar, unlike body_defaults — the verb overlays it per key.
        models: row.models,
        oauth: row.oauth,
        ambient: row.ambient,
        name,
    })
}

/// Take a gen-scalar `u32` body default off the row (config §4.1): `None` if the
/// key is absent, the value if it is a positive integer in range, else `BadValue`
/// (→78). Removing the key leaves `bd` holding only non-gen passthrough.
pub(super) fn take_u32(bd: &mut Map<String, Value>, key: &str) -> Result<Option<u32>, ConfigError> {
    match bd.remove(key) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0 && *n <= u64::from(u32::MAX))
            .map(|n| Some(n as u32))
            .ok_or_else(|| bad(key, "must be a positive integer")),
    }
}

/// Take a gen-scalar `f32` body default off the row (config §4.1): `None` if
/// absent, the value if it is a number (an integer coerces), else `BadValue`.
pub(super) fn take_f32(bd: &mut Map<String, Value>, key: &str) -> Result<Option<f32>, ConfigError> {
    match bd.remove(key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| bad(key, "must be a number")),
    }
}

/// Take a gen-scalar `bool` body default off the row (config §4.1): `None` if
/// absent, the value if it is a boolean, else `BadValue`.
pub(super) fn take_bool(
    bd: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, ConfigError> {
    match bd.remove(key) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| bad(key, "must be a boolean")),
    }
}
