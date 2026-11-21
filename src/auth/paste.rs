//! The pasted-code sign-in (auth §7.7) — the headless login of a row that declares no
//! device endpoint, and the one a human can finish from any browser on any device.
//! It is the AuthCode grant with the redirect taken out of the loop: nothing listens
//! and no browser is launched. Because the caller may be able to send only command
//! lines whose stdin is fixed when they start (the call envelope, arch §5.10.5), it is
//! TWO command lines: [`begin`] prints the authorize URL and parks the PKCE verifier
//! and CSRF state in the store; [`finish`] takes what the human copied, checks it
//! against what was parked, and ends in the same [`exchange_auth_code`] the loopback
//! flow ends in. Which redirect the URL names is row DATA (`paste_redirect`), never a
//! vendor branch.

use super::flows::{exchange_auth_code, fatal};
use super::login::LoginIo;
use super::oauth::AuthError;
use super::wire::{build_authorize_url, parse_callback, Pkce};
use super::{auth_error, OAuthConfig};
use crate::canonical::CanonicalError;
use crate::store::{pending_key, Cred, Secret};

/// How long a printed authorize URL waits for its code, in seconds. A DECIDED
/// constant like the device flows' own (auth §7.3) — no flag, no config key: long
/// enough for a human to sign in on another device, short enough that an abandoned
/// sign-in stops being finishable on its own.
const WINDOW: u64 = 600;

/// The `redirect_uri` both command lines name — computed from the row each time,
/// never parked. The vendor's code-display page where the row declares one; else the
/// row's loopback redirect string, with its port when the row fixes one. Nothing
/// listens there: the address the human's browser is sent to is what they paste.
fn redirect_uri(cfg: &OAuthConfig) -> String {
    if let Some(uri) = &cfg.paste_redirect {
        return uri.clone();
    }
    let port = cfg
        .redirect
        .port
        .map_or_else(String::new, |p| format!(":{p}"));
    format!("http://{}{port}{}", cfg.redirect.host, cfg.redirect.path)
}

/// The first command line: build the PKCE-S256 authorize URL, park the verifier and
/// state under the row's pending key, and print the URL on STDOUT — the one product
/// of this run, so a caller relaying it need parse nothing — with the instruction on
/// stderr. No network, no listener, no browser; the credential the row already holds
/// is untouched. Starting again overwrites what was parked, so only the newest URL
/// can be finished.
pub(super) fn begin(
    provider: &str,
    cfg: &OAuthConfig,
    io: &mut LoginIo,
) -> Result<(), CanonicalError> {
    let redirect = redirect_uri(cfg);
    let pkce = Pkce::derive(io.verifier);
    let url = build_authorize_url(cfg, &pkce, io.state, &redirect);
    let pending = Cred::Pending {
        verifier: Secret::new(pkce.verifier),
        state: io.state.to_owned(),
        expires_at: io.clock.now().saturating_add(WINDOW),
    };
    io.store
        .put(&pending_key(provider), &pending)
        .map_err(|e| auth_error(&format!("could not park the sign-in: {e}")))?;
    let _ = crate::run::emit(io.stdout, &format!("{url}\n"));
    let copy = if cfg.paste_redirect.is_some() {
        "copy the code the page then shows".to_owned()
    } else {
        format!(
            "your browser is then sent to an address beginning `{redirect}` that will \
             not load — copy that whole address from the address bar"
        )
    };
    let _ = writeln!(
        io.stderr,
        "open the URL above in any browser, on any device, and sign in; {copy}\n\
         then, within {} minutes, finish with: bz --login --provider {provider} --code '<what you copied>'",
        WINDOW / 60
    );
    Ok(())
}

/// The second command line: redeem what the human copied against the parked
/// verifier. Nothing parked, or parked too long ago, is a login failure (→77) that
/// names the restart; a paste carrying a `state` is CSRF-checked against the parked
/// one before any exchange. The parked entry is not deleted — its lapse is a query —
/// so a mistyped paste may be retried inside the window.
pub(super) fn finish(
    provider: &str,
    cfg: &OAuthConfig,
    pasted: &str,
    io: &mut LoginIo,
) -> Result<Cred, CanonicalError> {
    let restart = format!("start again with `bz --login --provider {provider}`");
    let Some(Cred::Pending {
        verifier,
        state,
        expires_at,
    }) = io.store.get(&pending_key(provider))
    else {
        return Err(auth_error(&format!(
            "no sign-in is waiting for a code for `{provider}`; {restart}"
        )));
    };
    if io.clock.now() >= expires_at {
        return Err(auth_error(&format!(
            "the sign-in for `{provider}` lapsed ({} minutes); {restart}",
            WINDOW / 60
        )));
    }
    let code = pasted_code(pasted, &state).map_err(fatal)?;
    exchange_auth_code(cfg, io, &code, verifier.expose(), &redirect_uri(cfg))
}

/// Read the authorization code out of what was pasted (PURE). Three shapes, told
/// apart by what they contain: a redirect ADDRESS (or its bare query), read by the
/// loopback flow's own [`parse_callback`] — so a vendor's `error=` and a foreign
/// `state` are refused exactly as they are there; `code#state`, the shape a
/// code-display page joins the two in; or the code alone. A `state` that is present
/// must equal the parked one; one that is absent is not invented — the verifier
/// already binds the code to this sign-in.
fn pasted_code(pasted: &str, expected_state: &str) -> Result<String, AuthError> {
    let pasted = pasted.trim();
    if pasted.contains("code=") || pasted.contains("error=") {
        let query = pasted.split_once('?').map_or(pasted, |(_, query)| query);
        let query = query.split_once('#').map_or(query, |(query, _)| query);
        return parse_callback(query, expected_state).map(|callback| callback.code);
    }
    let (code, state) = pasted.split_once('#').unwrap_or((pasted, expected_state));
    if state != expected_state {
        return Err(AuthError::Fatal(
            "pasted code belongs to a different sign-in (`state` mismatch)".to_owned(),
        ));
    }
    if code.is_empty() {
        return Err(AuthError::Fatal("no code was pasted".to_owned()));
    }
    Ok(code.to_owned())
}
