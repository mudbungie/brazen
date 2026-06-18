//! Shared harness for the pure-OAuth splits (auth §7.5, §6.1, §8): the throwaway
//! `OAuthConfig` and the signature-less `jwt` builder both groups assert against.
//! A subdirectory module so cargo does not compile it as its own test binary;
//! `#![allow(dead_code)]` because each split test crate uses only a subset.
#![allow(dead_code)]

use brazen::{OAuthConfig, RedirectSpec};

pub fn cfg() -> OAuthConfig {
    OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device_url: Some("https://auth.example/device".into()),
        client_id: "cid".into(),
        scope: Some("read write".into()),
        beta_headers: vec![],
        system_preamble: None,
        redirect: RedirectSpec::default(),
        authorize_params: vec![],
        account_header: None,
    }
}

/// Build a (signature-less, unverified) JWT `hdr.payload.sig` whose payload is the
/// given JSON — the wire shape `jwt_exp`/`jwt_account_id` read (auth §10.3, §10.4).
pub fn jwt(payload: serde_json::Value) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("hdr.{p}.sig")
}
