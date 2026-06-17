# Auth, OAuth/SSO & the credential store

> **Living document.** Edited like code. This spec derives from the canonical contract in architecture.md and MUST NOT contradict it. Where the auth model cannot be expressed without an architecture change, this spec raises a **change request** (§9) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md)

---

## 1. Purpose & Scope

API-key, bearer, and OAuth2 are **one problem**: produce the finished auth headers on a `WireRequest`, given a `CredStore` and a `Clock` (architecture.md §7). This spec defines, normatively, that whole capability:

- the `Auth` trait and its **four** registered ids over **three** impls — `ApiKey`/`Bearer` (`StaticSecretAuth`), `OAuth2`, `None` (`NoAuth`) (architecture.md §4.1, §4.4) — and exactly what each `apply` does, where the secret comes from, and how `auth.api_header` data drives header naming with **no vendor branch** (§3);
- the split between **auth-mode-*independent*** headers (data on the provider row) and **auth-mode-*dependent*** headers (data on the `OAuth2` auth row, applied only under OAuth) — and why a per-provider field cannot express the latter (§4);
- the `Cred` enum, the `CredStore` trait (XDG paths per OS, **0600** at `put`, **one file per provider**, atomic temp+rename), and the `Secret` newtype's redaction (§5);
- **silent in-band refresh** through the `Transport` seam — the only stateful thing in a normal run — as a pure staleness query plus a persist-then-use write (§6);
- the **control plane** `bz login <provider>`: Device flow (RFC 8628) and AuthCode + loopback (RFC 8252), quarantined out of the data plane, with `BrowserLauncher`/`CodeReceiver` injection (§7);
- the **five pure OAuth functions** (`build_authorize_url`, `parse_callback`, `build_token_exchange_request`, `parse_token_response`, `is_expired`) and the `Grant` enum that unifies auth-code/device/refresh into **one** token-exchange builder (§7.4);
- the offline test strategy for the whole flow (§8).

**In scope:** the `auth/` modules — `mod.rs` (`trait Auth`, `ApiKey`, `Bearer`) and `oauth.rs` (`OAuth2::apply` + the pure builders + `is_expired`) — plus `store.rs` (`CredStore`, `Cred`, `Secret`, `Clock`) and the `bz login` verb (architecture.md §11).

**Out of scope (owned elsewhere):** the request **body** and non-auth headers (set by `Protocol::encode` — architecture.md §4.5; the mapping specs); config/alias resolution and the `inline_key` plumbing (the config spec (planned), architecture.md §6.1); the `Transport` impl, framing, decode, the Sink, the exit-code driver loop and signal handling (architecture.md §5, §8); which `client_id`/`scope`/endpoints a provider uses (operator-supplied **data** on the auth row — architecture.md §13 item 3). This capability is **vendor-blind**: `ProviderCtx` carries no name / `ProtocolId` / `AuthId` (architecture.md §4.1); nothing here branches on which provider sent the request.

### 1.1 Inherited invariants (the grading rubric this spec upholds)

1. **`Auth::apply` is the ONLY data-plane function permitted to touch the credential store or the clock** (architecture.md §6.5). Everything before it (resolve/parse/encode) and after it (transport/decode/sink) is a pure function of `(bytes_in, ResolvedConfig)`. Even `apply` is pure *relative to its injected* `CredStore` + `Clock` + `Transport`.
2. **Nothing in the library reads `std::env`, opens `$XDG_*`, or calls `SystemTime::now`.** Those impurities live only in the three injected impls wired by `main()` (`HttpTransport`, XDG `CredStore`, `SystemClock`) (architecture.md §6.5, §10).
3. **The core never matches on a vendor name.** `Auth` impls are reached by `registry.auths[&cfg.provider.auth]` — a map lookup keyed by `AuthId`, never a `match` on a name (architecture.md §4.4). The header *name* is `auth.api_header` data, not a vendor branch (architecture.md §4.5).
4. **Auth failures → exit 77** (`EX_NOPERM`): missing creds, not-logged-in, OAuth refresh failure, `bz login` failure, and provider `401`/`403` (architecture.md §8). `Auth` is the `ErrorKind` (architecture.md §3.3).
5. **Single source of truth applied to creds:** no `is_valid` flag (freshness is the query `now + SKEW >= expires_at`); `expires_at` is **absolute** (computed once, never the relative value); no `token_type` flag (the `Cred` variant is the discriminant) (architecture.md §6.4).
6. **Refresh never escalates to a browser** (architecture.md §7.1). Interaction is forbidden in the data plane; a failed refresh is exit 77 telling the user to `bz login`.
7. **The OAuth functions are pure and table-testable from literals**, and the whole flow tests with **zero live network** (architecture.md §7.2, §9.4).

### 1.2 The trait this spec implements

For reference (architecture.md §4.1 — the **entire** contract between "which secret" and "how the request is authed"):

```rust
/// The ONLY consumer of CredStore. The stateless boundary is drawn exactly here.
pub trait Auth: Send + Sync {
    fn apply(
        &self,
        wire:      &mut WireRequest,
        ctx:       &ProviderCtx,     // shared capabilities (base_url, model, beta_headers) — also handed to encode
        auth:      &AuthCtx,         // auth-private: store key + inline secret + OAuth row data — NEVER handed to encode
        store:     &dyn CredStore,
        clock:     &dyn Clock,
        transport: &dyn Transport,   // for silent OAuth refresh — same seam, no new IO surface
    ) -> Result<(), Error>;
}
```

`apply` is called **once**, by `run`, between `encode` and `transport.send` (architecture.md §4.4):

```rust
let mut wire = …;                                                 // proto.encode(...) — body + non-auth headers
auth.apply(&mut wire, &ctx, &authctx, store, clock, transport)?;  // THE one cred seam — sets auth headers
let resp = transport.send(wire)?;                                 // THE one IO seam
```

`apply` mutates `wire` in place (adds headers), persists a refreshed token if needed, and returns `Ok(())` or an `Auth` error. It is **object-safe** — the pipeline holds `&dyn Auth`; no generic methods, no `-> impl Trait`, no associated types (architecture.md §4.1).

### 1.3 The two contexts `apply` reads

`apply` reads from **two** read-only projections of `ResolvedConfig`, and the split is a **security boundary**, not a convenience. `ProviderCtx` is **also** handed to `Protocol::encode`, so it carries only non-secret capabilities; the credential-bearing facts ride `AuthCtx`, which reaches **only** `Auth::apply` (architecture.md §4.1, §6.5):

```rust
pub struct ProviderCtx<'a> {            // shared with encode — NO name, NO secret (architecture.md §4.1)
    pub base_url:     &'a str,
    pub model:        &'a str,                  // alias-resolved
    pub beta_headers: &'a [(&'a str, &'a str)], // provider-level STATIC headers (e.g. anthropic-version)
    pub extra:        &'a Map<String, Value>,
}

pub struct AuthCtx<'a> {                // auth-private — NEVER handed to encode
    pub store_key:  &'a str,                    // the provider name, used ONLY as a CredStore key — never matched on
    pub inline_key: Option<&'a Secret>,         // the §6.5 inline-key bypass; absent ⇒ store.get(store_key)
    pub api_header: Option<&'a HeaderSpec>,     // x-api-key | Authorization:Bearer | x-goog-api-key — DATA; Some iff a keyed row
    pub oauth:      Option<&'a OAuthConfig>,    // resolved auth-row data (§7.1); Some iff AuthId::OAuth2
}
```

Both are `ResolvedConfig` projections (`ProviderCtx::from(&cfg)` / `AuthCtx::from(&cfg)`, architecture.md §4.4). Three consequences are load-bearing:

- **The credential never enters the protocol layer.** `inline_key` is a `Secret` on `AuthCtx`, not `ProviderCtx`, so `Protocol::encode` is **structurally barred** from it — this is what makes "`Auth::apply` is the ONLY data-plane function permitted to touch credentials" (architecture.md §6.5) a *type-level* fact, not a convention. The `api_header` rides `AuthCtx` for the same reason it is auth-only — `encode` never sets the auth header — so the only secret-free capabilities `ProviderCtx` carries (`base_url`, `model`, `beta_headers`) are ones encode legitimately needs.
- **`store_key` is a key, not an identity.** It is the resolved provider name used **solely** to index `CredStore`; nothing reads it to branch on *which* provider — the vendor name is still spent on the registry lookup before `apply` runs (architecture.md §4.1, §4.4). A *string key into a store*, never a `match` on it.
- **`api_header` and `oauth` are present exactly when needed.** Resolution pairs `api_header` with a keyed row and `OAuthConfig` with `AuthId::OAuth2` — else a **Config** error at resolve (78), the same surfaced-ambiguity rule as model→provider routing (architecture.md §4.3). `NoAuth` reads neither; `ApiKey`/`Bearer` read only `api_header`; `OAuth2` reads both.

---

## 2. `HeaderSpec` — the auth-header shape as DATA

The provider row carries an `api_header: HeaderSpec` (architecture.md §4.2). It is the **only** thing that names the auth header; it dissolves "x-api-key vs Authorization:Bearer vs x-goog-api-key" into one data record so `ApiKey`/`Bearer` need **no** per-vendor branch (architecture.md §4.5, §4.6).

```rust
#[derive(Deserialize, Clone)]
pub struct HeaderSpec {
    pub name:   String,          // "x-api-key" | "Authorization" | "x-goog-api-key"
    pub scheme: HeaderScheme,    // how the secret is written into the value
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HeaderScheme {
    Raw,        // value = <secret>                 (x-api-key, x-goog-api-key)
    Bearer,     // value = "Bearer " + <secret>     (Authorization)
}
```

Applying a secret is then **one** data-driven operation, shared by every secret-bearing impl:

```rust
fn set_auth_header(wire: &mut WireRequest, spec: &HeaderSpec, secret: &Secret) {
    let value = match spec.scheme {
        HeaderScheme::Raw    => secret.expose().to_string(),
        HeaderScheme::Bearer => format!("Bearer {}", secret.expose()),
    };
    wire.set_header(&spec.name, &value);   // value never logged; Secret::expose is the single read site
}
```

> **Reframe that dissolves the branch.** `x-api-key`, `Authorization: Bearer`, and Google's `x-goog-api-key` look like three providers' three header conventions; they are **one** `(name, scheme)` pair. Adding Google's `x-goog-api-key` is a `HeaderSpec { name: "x-goog-api-key", scheme: Raw }` on the row — **no new `Auth` impl, no branch** (architecture.md §4.6). `match` on `HeaderScheme` (two arms, total, no vendor name) is *value formatting*, not vendor dispatch — the scheme is data the row chose.

`HeaderScheme` is deliberately **small and closed**: the two arms cover every shipped wire convention. A third arm (e.g. a query-param key) would be added only when a provider genuinely needs it — a closed vocabulary config can name with a typo-checked spelling, exactly like `ProtocolId`/`AuthId` (architecture.md §4.2).

---

## 3. The `Auth` impls — **three** structs behind **four** ids

There are **three** `Auth` impls, all **zero-cost, `&'static dyn`-shareable**, registered once in `Registry::builtin()` (architecture.md §4.4). `api_key` and `bearer` are **one** impl under two ids — they are "not two dispatch sites" (§3.1), so making them two structs would be exactly the redundant second representation single-source-of-truth forbids:

```rust
auths.insert(AuthId::ApiKey, &StaticSecretAuth);   // same impl…
auths.insert(AuthId::Bearer, &StaticSecretAuth);   // …under two intent-naming ids
auths.insert(AuthId::OAuth2, &OAuth2Auth);
auths.insert(AuthId::None,   &NoAuth);             // keyless: no cred, no header
```

**All are unit structs** (stateless, `&'static dyn`-shareable). `StaticSecretAuth` reads its secret + `api_header` via `AuthCtx`; `OAuth2Auth` reads its endpoints/`client_id`/`scope`/`beta_headers` from the **`OAuthConfig` on `AuthCtx`** (§1.3, §7.1) — never hard-coded and never stored on the impl, so the registry shares one `&OAuth2Auth` across every OAuth row; `NoAuth` reads nothing (§3.3).

### 3.1 `StaticSecretAuth` — the staleness-free impl behind `api_key`/`bearer`

`api_key` and `bearer` differ **only** in `HeaderScheme`, and that difference already lives on the row's `HeaderSpec` (`scheme: Raw` vs `Bearer`), so they are the **same code** modulo the header value — `set_auth_header` (§2) handles both. They are kept as two `AuthId`s purely so config names the intent (`auth = "api_key"` vs `auth = "bearer"`); they are **not** two dispatch sites, so they are **one** `StaticSecretAuth` impl the registry maps both ids onto.

```rust
impl Auth for StaticSecretAuth {
    fn apply(&self, wire, ctx, auth, store, _clock, _transport) -> Result<(), Error> {
        let secret = resolved_secret(store, auth)?;          // inline_key ?? store.get(store_key) ?? Err(MissingCreds→77)
        set_auth_header(wire, require_header(auth)?, &secret); // header NAME + scheme from auth.api_header — DATA
        Ok(())                                               // no clock, no transport: refresh is the empty case
    }
}
```

- **Secret source, in precedence order:** the resolved `inline_key` (`--api-key` / `BRAZEN_API_KEY` / `ANTHROPIC_API_KEY`, flowing on `ResolvedConfig`) **if present**, else `store.get(provider)` yielding the matching `Cred` variant, else `Err(Auth::MissingCreds)` → **exit 77** (architecture.md §7). The inline-key path **never constructs a `CredStore`** (the store is built lazily — architecture.md §6.5), so a fully-stateless run touches zero files except stdin.
- **`_clock`/`_transport` are unused** — and that is the point. "Refresh if stale" has an **empty case** for a secret that never goes stale; `StaticSecretAuth` *is* that empty case, not a special one (architecture.md §3, §7). It does **no** I/O of any kind beyond reading the store.
- **No vendor branch.** The header *name* is `auth.api_header`'s `.name`; the *scheme* is its `.scheme`. `require_header` unwraps the `Option<&HeaderSpec>` — `Some` for every keyed row (a resolve invariant), else a defensive **Config** error (→78), never a panic. Google's `x-goog-api-key` is `StaticSecretAuth` against a `Raw` row whose `HeaderSpec` names it — the same impl (architecture.md §4.6).

The **bearer** path is the same `apply` reached through `AuthId::Bearer`; the row's `api_header = { name: "Authorization", scheme: "bearer" }` makes `set_auth_header` emit `Authorization: Bearer <token>`. The store yields a `Cred::Bearer { token }` (vs `Cred::ApiKey { key }`) — the variant is the discriminant, so there is **no `token_type` flag** to read (architecture.md §6.4). `resolved_secret` accepts **either** stored variant (the scheme, not the `Cred` kind, decides the header shape), so a `Cred::Bearer` under a `Raw` row or a `Cred::ApiKey` under a `Bearer` row both resolve — only a stored `OAuth2` cred is rejected (`WrongCredKind`).

```rust
fn resolved_secret(store: &dyn CredStore, auth: &AuthCtx) -> Result<Secret, Error> {
    if let Some(inline) = auth.inline_key            { return Ok(inline.clone()); } // stateless path — store untouched
    match store.get(auth.store_key) {
        Some(Cred::ApiKey { key })   => Ok(key),
        Some(Cred::Bearer { token }) => Ok(token),
        Some(Cred::OAuth2 { .. })    => Err(auth_error("…reconfigure the row or re-run `bz login`")), // row says api_key/bearer but stored cred is OAuth — config drift, 77
        None                         => Err(auth_error("…set BRAZEN_API_KEY or run `bz login`")),      // → 77
    }
}
```

(`auth.inline_key` / `auth.store_key` are the `AuthCtx` fields of §1.3 — auth-private data, *not* a vendor name. A `Cred` variant mismatching the row's `AuthId` is surfaced as `WrongCredKind`→77, never a silent fallthrough — the store could hold a stale OAuth cred for a row reconfigured to api-key.)

> **On the error names.** `MissingCreds`, `WrongCredKind`, `NotLoggedIn`, and `RefreshFailed` are **condition labels**, not Rust variants. Every `Auth::apply` failure is one realized type — `auth_error(message)` building a `CanonicalError{ kind: Auth }` (exit 77, architecture.md §8) — where the *message* differs by what would fix it; the cases are distinguished by message, not by a typed enum. The only typed `AuthError` in the code is the OAuth **token-parser** signal `AuthError { Pending, SlowDown, Fatal(String) }` (§7.5), internal to the pure `parse_callback`/`parse_token_response` and mapped to `auth_error(..)` at the `apply` boundary.

### 3.2 `OAuth2` — the only impl where staleness exists

`OAuth2::apply` is the sole `Auth` impl that uses `clock` and `transport` (architecture.md §7.1). Its full body is §6. It also applies the **auth-mode-dependent** beta header (§4). OAuth knowledge — refresh, the bearer header, *and* `anthropic-beta: oauth-2025-04-20` — is **fully contained in this one impl** (architecture.md §4.5).

### 3.3 `NoAuth` — the keyless impl behind `none`

`auth = "none"` is a row whose provider needs **no credential** — local Ollama is the shipped case (providers.md §5). `NoAuth::apply` reads no `CredStore`, writes no header, and so touches **none** of `apply`'s seams: its whole body is `Ok(())`. A keyless row carries **no `api_header`** (`AuthCtx.api_header` is `None`, a resolve invariant), and a stray `--api-key` / stored cred is simply **ignored** — the row declares the header is not wanted. It is the exact dual of the keyed impls' "missing key → **77**": where they *require* a secret, `NoAuth` *forbids* needing one. This is why a keyless local provider must **not** be modeled as `bearer` with a tolerated-missing-key (which would silently downgrade a *forgotten* key on a keyed provider from a clean 77 to a provider-side 401); keylessness is its own declared capability, not a hole in a keyed one.

---

## 4. Auth-mode-*dependent* headers live on the `OAuth2` impl, not the row

The Anthropic `anthropic-beta: oauth-2025-04-20` header differs **by auth mode on the same provider**: it is sent when talking to `api.anthropic.com` via OAuth, and **not** sent when talking to the same base URL via an API key (architecture.md §4.5). This is the load-bearing reason the split exists.

| Header | Varies by | Home | Applied by |
|---|---|---|---|
| `anthropic-version: 2023-06-01` | provider only (always sent) | `Provider.beta_headers` (row data) | `encode`, copying `ctx.beta_headers` verbatim |
| `Authorization: Bearer <token>` | auth mode (OAuth only) | computed from the stored `Cred::OAuth2` | `OAuth2::apply` |
| `anthropic-beta: oauth-2025-04-20` | auth mode (OAuth only) | `OAuthConfig.beta_headers` (auth-row data) | `OAuth2::apply` |

> **Why a per-provider field cannot express it.** `Provider.beta_headers` is keyed by provider; it has exactly one value per `(provider, header)`. It cannot say "send this header **only under OAuth**" without the core learning "is this request OAuth?" — a per-mode branch on the provider row, which is precisely the vendor-shaped `if` the registry design exists to forbid (architecture.md §4.4, §4.5). The clean reframe: a header that varies **by auth mode** is **auth data**, so it lives on the **auth row** (`OAuthConfig.beta_headers`) and is applied by the one impl that owns that mode. `Provider.beta_headers` keeps **only** the auth-mode-*independent* headers (`anthropic-version`), always sent by `encode`. The provider row and the auth row each hold exactly the facts they own; neither holds the other's, so neither can drift.

Concretely, the **same** `api.anthropic.com` base URL is reached two ways with no core branch:

- **api-key mode:** row `auth = "api_key"`, `api_header = { name: "x-api-key", scheme: "raw" }`; `StaticSecretAuth` sets `x-api-key`; `encode` sets `anthropic-version`; **no** `anthropic-beta: oauth-…`. (This is the only mode that ships built-in for v0.1 — architecture.md §13 item 3.)
- **OAuth mode:** an operator-configured row `auth = "oauth2"` with an `OAuthConfig` carrying `beta_headers = [["anthropic-beta", "oauth-2025-04-20"]]`; `OAuth2Auth` sets `Authorization: Bearer …` **and** that beta header; `encode` still sets `anthropic-version`.

`OAuthConfig.beta_headers` is the **auth-mode-dependent** analogue of `Provider.beta_headers`, applied inside `apply` after the bearer token (§6). It is plain `Vec<(String, String)>` row data — adding a future per-mode header is config, not code (the severability test, architecture.md §4.6).

### 4.1 Auth-mode-*dependent* BODY shape: the required system preamble

§4 places an auth-mode-dependent **header** on the auth row and applies it in `apply`. Some auth modes also constrain the request **body**: a Claude-Code-scoped Anthropic OAuth token is **rejected** unless the request's system prompt **leads with** the exact line `You are Claude Code, Anthropic's official CLI for Claude.` This is the same *shape* of fact as the `anthropic-beta` header — it varies **by auth mode on the same provider** (an `sk-ant-api…` key needs no preamble; the OAuth token requires it) — so it is **auth-row data** (`OAuthConfig.system_preamble: Option<String>`, §7.1), not a provider field and not a line the user must type.

| Auth-mode-dependent fact | Plane | Home | Applied by |
|---|---|---|---|
| `anthropic-beta: oauth-…` | header | `OAuthConfig.beta_headers` | `OAuth2::apply` (header-only) |
| `You are Claude Code, …` system lead | **body** | `OAuthConfig.system_preamble` | **resolution** (`lead_with_preamble`, before `encode`) |

> **Why it cannot ride `apply`.** `Auth::apply` (§1.2) runs on the **already-encoded** `WireRequest` and is **header-only** — `encode` has frozen `wire.body` before `apply` (architecture.md §4.1). The preamble joins the system field, a **body** edit, so `apply` is the wrong seam; making it touch the body would re-couple auth identity to request-shaping — the exact split §4 exists to keep. The clean reframe: the preamble is not a header to write but **content the resolved request must lead with** — the canonical fact `req.system` already carries ("the leading, config-/flag-/file-sourced system prompt", architecture.md §3.1). So it is **sourced from** the auth row but **applied one step earlier than `apply`**, in request resolution.

**The seam — resolution prepends it, before `encode` (one site, every dialect).** After `fill_absent` supplies the user/config system (config §4), `lead_with_preamble(&mut req, &cfg)` ensures `req.system` **leads with** the auth mode's preamble. Then every protocol's existing `req.system` projection carries it with **no per-protocol code**: Anthropic hoists it to top-level `system`, `openai_responses` folds it into `instructions` (§10.7), `openai_chat` carries it in system position (architecture.md §3.1). Prepending here, not in each `encode`, is the load-bearing choice — `encode` stays a pure `(req, ctx)` projection with no auth-mode awareness, and the prepend is written **once** instead of duplicated across all five encoders (architecture.md §4.1, the minimal-interface rule).

**The invariant is "leads with", not "prepended once" — so it is idempotent.** `lead_with_preamble` prepends the preamble block **only if** `req.system` does not already begin with it. So the cases collapse to one rule, with no branch per case: the positional one-liner (`bz -m claude-… "q"`, no system) yields `[preamble]`; a user `--system "X"` yields `[preamble, X]`; a re-fed transcript already leading with the Claude-Code line (a harness piping a full canonical request) is left untouched (no double). The empty case — a row with **no** `system_preamble`, or any non-OAuth row — is a no-op that leaves `req.system` exactly as `fill_absent` left it (incl. `None`): the general path with empty input, not a special case (architecture.md §4.6).

**`--raw` is unaffected, by the existing fork.** `--raw` bypasses `encode` **and** `fill_absent` (verbatim wire bytes, architecture.md §4.4), so it also bypasses `lead_with_preamble`: a caller sending raw bytes under OAuth owns the whole body, preamble included. No new branch — the canonical-vs-raw fork already there decides it.

**Severability.** The preamble is row config: delete `system_preamble` (or the whole OAuth row) and the behavior is gone with **zero** core change (architecture.md §4.6). It is the **body** analogue of `OAuthConfig.beta_headers`; both are auth-mode-dependent data on the auth row, differing only in *which* seam applies them (a header in `apply`; a system lead in resolution), because a body fact cannot ride a header-only `apply`.

```toml
# Anthropic via a Claude-Code-scoped OAuth token — the preamble is row DATA, not a typed flag.
[provider.oauth]
# … authorize_url / token_url / client_id / beta_headers per §4, §7.1 …
system_preamble = "You are Claude Code, Anthropic's official CLI for Claude."
```

This removes the magic `--system "You are Claude Code…"` from the one-liner: `bz -m claude-haiku-4-5-20251001 "question"` after a one-time `bz login` (parent bl-ce84's north star). A standard `sk-ant-api…` row pins no `system_preamble` and is unchanged.

---

## 5. The credential store

### 5.1 `Cred` — the stored secret, discriminant-as-variant

```rust
#[derive(Serialize, Deserialize)]
pub enum Cred {
    ApiKey { key: Secret },
    Bearer { token: Secret },
    OAuth2 { access_token: Secret, refresh_token: Secret, expires_at: u64, scope: Option<String>,
             account_id: Option<String> },   // §10.4 — derived ONCE from the id_token claim at login; None for OAuth rows that carry no account id (Anthropic)
}
```

Single source of truth applied to creds (architecture.md §6.4):

- **No `is_valid` flag.** Freshness is the **query** `is_expired(expires_at, now)` (§6.1) — a stored boolean would be wrong the instant the clock moved past it.
- **`expires_at` is ABSOLUTE** (a unix-seconds instant), computed **once** as `clock.now() + expires_in` at the moment the token response is parsed (§7.4). Storing the **relative** `expires_in` would be wrong the instant it is read back from disk in a later process.
- **No `token_type` flag.** The `Cred` variant **is** the discriminant; `OAuth2` is always a bearer token, so a `token_type:"Bearer"` field would be a second, driftable home for a fact the variant already states.
- **`scope` is `Option<String>`** — the granted scope as returned, carried for audit/diagnostics; `None` is the no-scope case, not special.
- **`account_id` is `Option<String>`** (§10.4) — a non-secret account identifier some OAuth providers bind to the credential and require **echoed as a request header** on the data plane (OpenAI's `ChatGPT-Account-ID`, derived from the id_token's `https://api.openai.com/auth` → `chatgpt_account_id` claim). It is **derived once at login** and stored, not re-parsed from a stored JWT per request (single source of truth: the claim's one data-plane home is this field; storing the whole id_token would be a second, larger home for the one fact we use). `None` is the no-account-id case (Anthropic OAuth), not special. **It is not a `Secret`** — it is an account id echoed in a header, not a credential.

A `Cred` is exactly one secret bundle for exactly one provider — there is **no provider name inside it** (the file path is the name, §5.2; storing it twice would drift, architecture.md §1 single-source-of-truth).

### 5.2 `CredStore` — two methods, XDG-correct, 0600, atomic

```rust
pub trait CredStore {
    fn get(&self, provider: &str) -> Option<Cred>;
    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()>;
}
```

**Two methods only** — no `is_valid` (freshness is a query, §6.1), no `refresh` (that is `OAuth2::apply`, §6, *using* `get`+`put`), no `list`/`delete` in the data-plane trait (architecture.md §6.4). `list`/`delete` are **control-plane** affordances of `bz login`/a future `bz logout` if ever needed; the data-plane trait stays minimal so the stateless boundary (architecture.md §6.5) is as small as it can be. `get` returns `Option` (a missing file is `None`, **not** an error — the absence is the no-creds path, surfaced as `MissingCreds`→77 by the caller, §3.1).

**One file per provider** (architecture.md §6.4) — `<provider>.json` — keeps the blast radius of a corrupt/leaked file to one provider and makes every write a single atomic file replace. XDG paths (`directories`/`etcetera`, architecture.md §10):

| Kind | Unix (`$XDG_*`) | macOS | Windows |
|---|---|---|---|
| Secrets (one file/provider) | `$XDG_DATA_HOME/brazen/credentials/<provider>.json` (fallback `~/.local/share/brazen/credentials/<provider>.json`) | `~/Library/Application Support/brazen/credentials/<provider>.json` | `%APPDATA%\brazen\credentials\<provider>.json` |

`put` writes **atomically** and enforces the mode at write time:

1. serialize `cred` to JSON (`Secret::Serialize` writes **plaintext** — the only sanctioned plaintext site, §5.3);
2. create a **temp file in the same directory** (same filesystem, so rename is atomic) with mode **0600** **at create time** on Unix (`OpenOptions::mode(0o600)`, never a create-then-chmod window);
3. `write_all` + `sync_all` the bytes;
4. **`rename(temp, <provider>.json)`** — atomic replace; a concurrent reader sees either the whole old file or the whole new one, never a partial write.

Mode **0600** is enforced **at `put`** (architecture.md §6.4); the temp file is created 0600 so the secret is never briefly world-readable. **Windows** inherits the user-profile ACL (no DPAPI) — a **documented limitation** (architecture.md §13 item 4), **not** a code branch: the same atomic temp+rename runs, only the Unix `mode(0o600)` call is `#[cfg(unix)]`. The credentials **directory** is created `0700` on Unix on first `put`.

> **`bz login` writes synchronously *inside* the control plane, and `OAuth2::apply` writes synchronously *before* any streaming begins** (architecture.md §5.8). There is nothing stateful to unwind on a signal — which is why brazen installs **no** signal handlers (architecture.md §5.8). Atomic rename is what makes "no handler" safe.

### 5.3 `Secret` — redaction at the type level

```rust
pub struct Secret(String);

impl fmt::Debug   for Secret { fn fmt(&self, f) -> fmt::Result { f.write_str("Secret(<redacted>)") } }
impl fmt::Display for Secret { fn fmt(&self, f) -> fmt::Result { f.write_str("<redacted>") } }
impl Secret { pub fn expose(&self) -> &str { &self.0 } }   // the single audited read site

impl Serialize for Secret {
    // writes PLAINTEXT — ONLY ever reached via CredStore::put serializing into the 0600 file.
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&self.0) }
}
```

`Secret` makes leakage a **type error to express casually**: `{:?}`/`{}` redact, so a secret in a log line, a `provider_detail`, or a panic message renders `<redacted>` (architecture.md §6.4). The **only** way to read the plaintext is `expose()` — a single grep-able call site — used by `set_auth_header` (§2) and the OAuth builders (§7.4). `Serialize` writes plaintext, but is **only** reachable through `CredStore::put` writing the 0600 file; `--dump-config` redacts secrets to the inert `"<redacted>"` sentinel via the same `Display` (architecture.md §6.2, §13 item 2), never the real key, never a `${VAR}` reference.

### 5.4 `Clock` — the injected now

```rust
pub trait Clock { fn now(&self) -> u64; }   // unix seconds
```

The **only** time source in the data plane, injected into `run` and handed to `apply` (architecture.md §1, §6.5). The library **never** calls `SystemTime::now`; `SystemClock` (the `bin` impl) does, and `FakeClock` drives fresh/stale in tests with no sleeping (§8, architecture.md §9.4). `now()` feeds exactly two computations: `is_expired` (§6.1) and the **absolute** `expires_at` (§7.4, §10.3 — `now + expires_in`, or the JWT `exp` directly when the response carries no `expires_in`).

### 5.5 Ambient credential discovery — the zero-setup source

A credential is reachable two ways. The first is the store the user filled themselves: `--api-key`/`BRAZEN_API_KEY` inline, or a `bz login` that wrote one `Cred` under the provider name (§5.2) — both already implicit, `apply` reads `store.get(provider)` with no further flag (§3.1, §6). The second is **ambient discovery**: a credential another tool on the box *already* holds, in *its* file and *its* format. The shipped case is the Claude Code OAuth credential at `~/.claude/.credentials.json`, so `bz -m claude-… "question"` works with **no `bz login` and no key** when Claude Code is signed in.

This is data, not a vendor branch. A provider row MAY carry an `ambient` block:

```toml
[[provider]]
name = "anthropic-oauth"        # the OAuth row (its `oauth` block + headers ship separately)
auth = "oauth2"
ambient = { format = "claude_code", path = "~/.claude/.credentials.json" }
```

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct AmbientSpec { pub format: AmbientFormat, pub path: String }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AmbientFormat { ClaudeCode }
```

- **`path` is operator DATA** (the foreign file's location); **`format` selects the pure parser** that maps that file's bytes to a brazen `Cred`. Both pieces explicit, neither in core code — deleting the `ambient` line deletes the capability with no code edit (severability, architecture.md §1).
- **`AmbientFormat` is a closed enum, not a JSON-pointer DSL.** Each foreign shape needs a parser anyway; one variant for the one known source is less mechanism than a speculative mapping language (architecture.md "build less"). A second source is a second variant + parser.

**The cred-fetch is one query, ambient is its empty case.** Both `apply` impls resolve the credential through one helper — `store.get(store_key).or_else(|| ambient.and_then(|spec| store.discover(spec)))` — so "where a credential comes from" has a single home. A row with no `ambient` block makes the `or_else` arm `None`: the general path with an empty input, not a special case (architecture.md §1). `AuthCtx` gains `ambient: Option<&AmbientSpec>` (the third resolve-paired field beside `oauth`/`api_header`).

**The seam carries the read; the parse is pure; the file IO is the shim's.** `CredStore` gains a third method — `discover(&self, spec: &AmbientSpec) -> Option<Cred>` — because reading a foreign credential file is still a credential read, and the data plane reads credentials only through this seam (architecture.md §6.5). The `bz` `XdgCredStore` impl expands `~`/`$HOME` in `spec.path` (the one place `$HOME`, an ambient input, is read — like `restore_sigpipe`/`isatty`, the shim's impurity, architecture.md §5.5) and reads the file; the **pure `parse_ambient(format, &bytes) -> Option<Cred>`** lives in the library (100%-tested from byte fixtures, the `claude_code` case parsing `claudeAiOauth` → `Cred::OAuth2`). A missing/malformed/foreign file is `None` — the no-creds path, exactly like `get` (§5.2). The in-memory test double’s `discover` lets the auth tests drive the ambient arm with no file.

> **The discovered token is read once, never written back.** Discovery is a bootstrap, not adoption: `OAuth2::apply` (§6) persists a *refresh* to brazen's **own** XDG store under `store_key`, so the foreign file is touched read-only and exactly once, and subsequent runs read brazen's fresher copy. Claude Code's file is never mutated by brazen.

> **`claudeAiOauth.expiresAt` is MILLISECONDS** (e.g. `1781693903571`), unlike brazen's absolute unix-**seconds** `Cred::expires_at` (§5.1). The parser divides by 1000 once at the point of conversion — the single home for the unit mismatch, never re-derived downstream (architecture.md "carry the fact").

---

## 6. Silent refresh — the only stateful thing in a normal run

`OAuth2::apply` is the one place a normal (non-`login`) run reads **and writes** the store (architecture.md §7.1). It detects staleness with a pure clock comparison, refreshes over the **same `Transport` seam** (no second network path), persists the new token, and uses it — all before `transport.send` of the real request.

```rust
impl Auth for OAuth2Auth {
    fn apply(&self, wire, ctx, auth, store, clock, tx) -> Result<(), Error> {
        let cfg = auth.oauth.ok_or_else(Error::oauth_row_misconfigured)?;  // resolve guarantees Some (§1.3); defensive → 78
        let Some(Cred::OAuth2 { access_token, refresh_token, expires_at, scope }) = store.get(auth.store_key)
            else { return Err(auth_error("not logged in … run `bz login <provider>`")); };   // → 77

        let token = if is_expired(expires_at, clock.now()) {
            let wire_tok = build_token_exchange_request(cfg, Grant::Refresh(&refresh_token)); // PURE
            let bytes    = tx.send(wire_tok)?.collect_to_end()?;          // the ONE impure seam (mockable)
            let fresh    = parse_token_response(&bytes, clock.now())      // PURE; sets ABSOLUTE expires_at; Err = AuthError::Fatal
                              .map_err(|_| auth_error("token refresh failed (revoked or expired): run `bz login`"))?;  // invalid_grant → 77
            store.put(auth.store_key, &fresh.as_cred(&refresh_token, &scope))?;  // persist-then-use
            fresh.access_token
        } else {
            access_token
        };

        set_auth_header(wire, require_header(auth)?, &token);   // Authorization: Bearer <token> (scheme from row)
        for (k, v) in &cfg.beta_headers {                       // auth-mode-dependent headers (§4)
            wire.set_header(k, v);                              // e.g. anthropic-beta: oauth-2025-04-20
        }
        Ok(())
    }
}
```

The opening `auth.oauth.ok_or_else(…)?` is **defensive, not a live branch**: resolution pairs an `oauth2` row with a present `OAuthConfig` or fails at resolve with a Config error (§1.3, architecture.md §4.3), so a resolved run never reaches `apply` with `oauth: None`. Per the 100%-coverage discipline (architecture.md §9.5: "an unreachable arm is either dead code or a missing test"), the arm is **not** excluded — it is exercised by a direct `OAuth2Auth::apply` unit test handed an `AuthCtx { oauth: None, .. }`, proving the no-panic contract executes. `store_key` is the **single** source of the provider name on both reads and the `put` (§1.3) — the `OAuthConfig` carries none.

### 6.1 `is_expired` — freshness is a query, not a field

```rust
pub const SKEW: u64 = 60;   // seconds of clock-skew / in-flight margin
pub fn is_expired(expires_at: u64, now: u64) -> bool { now + SKEW >= expires_at }
```

`SKEW` (60 s) refreshes slightly **early** so a token does not expire mid-flight between `apply` and the provider receiving the request. This is a **pure comparison** against the injected clock (architecture.md §7.1) — table-tested from literals (§8): `now + SKEW < expires_at` ⇒ fresh; `>=` ⇒ stale; the `now == expires_at - SKEW` boundary is stale (`>=`). There is **no `is_valid` field** that could fall out of sync (architecture.md §6.4).

### 6.2 Refresh discipline (the owned tradeoffs)

- **Same `Transport` seam, no second network path.** The refresh `POST` goes through the injected `transport`, so it is mocked by the *same* `MockTransport` as the data request (architecture.md §7.1, §9.1) — offline-testable, no parallel HTTP surface.
- **Same transport bounds.** The refresh `POST` shares the data request's hang risk, so `apply` copies the resolved `timeouts` `run` stamped on `wire` onto the refresh request (config.md §4.3) — a stalled token endpoint can no longer hang `bz` mid-refresh.
- **Persist-then-use.** The fresh token is `put` to the store **before** it is used on the wire, so the **next** `bz` process starts fresh — refresh amortizes across processes (architecture.md §7.1). A stateless binary still benefits from the one sanctioned stateful exception.
- **A failed refresh is exit 77, never a browser.** `invalid_grant` (revoked/expired refresh token) → `RefreshFailed` → **exit 77** with a message to `bz login`. Refresh **never escalates to a browser** — blocking the data plane on interaction is forbidden (architecture.md §7.1). The data plane is non-interactive, full stop; interaction is quarantined in `bz login` (§7).
- **No concurrent-refresh lock.** Two `bz` processes could each refresh and double-`put`; last-write-wins on the atomic rename (§5.2) is acceptable because either refreshed token is valid (architecture.md §12). A lock would be mechanism for a non-problem.
- **A transport error on the refresh `POST`** (connect/DNS/TLS/5xx) propagates as the transport's own `Error` (`Transport`→69 or `Provider{status}`→69/70 per architecture.md §8) — it is an upstream failure of the **refresh** request, distinct from `RefreshFailed` (a *parsed* `invalid_grant`, which is an auth fact → 77).

`fresh.as_cred(&refresh_token, &scope)` rebuilds a `Cred::OAuth2` carrying the **new** `access_token`/`expires_at` and **reuses the prior `refresh_token`** when the token response omits a rotated one (many providers return only a new access token on refresh); a rotated refresh token in the response **replaces** it. `scope` is carried through.

---

## 7. `bz login` — the quarantined control plane

Interactive login is the **only** interactive surface in brazen, deliberately quarantined out of the data plane so `run` never blocks on a browser (architecture.md §7.2, §13 item 5). It is a `bz` **subcommand** (not a sibling binary): its entire job is to obtain a `Cred::OAuth2` and `CredStore::put` it. `run`'s data path is unchanged by its existence.

```
bz login <provider>           # default: Device flow (RFC 8628) — headless-friendly
bz login <provider> --browser # AuthCode + loopback (RFC 8252)
```

The flow is **selected by capability, not vendor**: Device by default (works over SSH / no browser), `--browser` opts into the loopback flow. Both end in the same `store.put(provider, &Cred::OAuth2 { … })`. The provider's `client_id`/`scope`/endpoints are **operator-supplied data** on the auth row (`OAuthConfig`, §7.1), never hard-coded vendor policy (architecture.md §13 item 3); **no built-in OAuth row ships for v0.1**.

### 7.1 `OAuthConfig` — the auth row as data

```rust
#[derive(Deserialize, Clone)]
pub struct OAuthConfig {
    pub authorize_url: String,                 // RFC 8252 authorization endpoint
    pub token_url:     String,                 // token endpoint (auth-code, device, AND refresh)
    pub device_url:    Option<String>,         // RFC 8628 device-authorization endpoint; None ⇒ --browser required
    pub client_id:     String,
    pub scope:         Option<String>,         // space-delimited; None ⇒ omit the scope param
    #[serde(default)] pub beta_headers: Vec<(String, String)>,  // auth-mode-dependent STATIC headers (§4)
    #[serde(default)] pub system_preamble: Option<String>,      // §4.1 — system the body must LEAD with (Anthropic OAuth's Claude-Code line); applied in resolution, not apply (BODY, not a header). None ⇒ no preamble
    // ── §10 additions: each defaults to today's behavior, so an existing row (Anthropic) is byte-identical ──
    #[serde(default)] pub redirect:         RedirectSpec,         // §10.1 — loopback redirect host/port/path AS DATA
    #[serde(default)] pub authorize_params: Vec<(String, String)>,// §10.2 — extra authorize-URL params (vendor login knobs)
    #[serde(default)] pub account_header:   Option<String>,       // §10.4 — header NAME for Cred.account_id; None ⇒ not emitted
}

#[derive(Deserialize, Clone)]
pub struct RedirectSpec {                       // §10.1 — the loopback redirect, as data
    #[serde(default = "default_host")] pub host: String,        // "127.0.0.1" (RFC 8252 default) | "localhost" (OpenAI registered)
    #[serde(default)]                  pub port: Option<u16>,   // None ⇒ ephemeral :0 (today) | Some(1455) ⇒ fixed (OpenAI)
    #[serde(default = "default_path")] pub path: String,        // "/callback" (today) | "/auth/callback" (OpenAI)
}
// default_host() = "127.0.0.1"; default_path() = "/callback"; Default for RedirectSpec = { host, port: None, path }.
```

Everything provider-specific about OAuth is **here, as data** — except the provider **name**, which is deliberately absent. The name has one home per plane: the row's key in the config table, the `bz login <provider>` argv in the control plane, and `AuthCtx.store_key` in the data plane (§1.3); storing it on `OAuthConfig` too would be a second home that could drift (architecture.md §1, single source of truth). The pure functions (§7.4) take `&OAuthConfig` and literals; nothing about Anthropic (or any vendor) is compiled into the core. A row missing `device_url` simply cannot run the Device flow — `bz login <provider>` without `--browser` errors with "this provider has no device endpoint; use `--browser`" (a **Config** error → 78), never a silent fallback.

The `OAuthConfig` is an **optional `oauth` block on the provider row** (`Provider.oauth: Option<OAuthConfig>`), so it folds through the same four-layer config resolution as the rest of the row (config §3) and is keyed by the row name with no second name home. Resolution enforces the §1.3 pairing **in `complete()`**: an `auth = "oauth2"` row with no `oauth` block is an `IncompleteProvider { field: "oauth" }` → 78, exactly like a row missing `base_url`. Both planes then read `provider.oauth`: the data plane projects it onto `AuthCtx.oauth` (Some for every resolved oauth2 row, the §6 invariant), and `bz login <provider>` **routes the same resolution by the provider name** to obtain the `OAuthConfig` before running a flow (a row with no oauth block → "provider has no oauth config" → 78).

### 7.2 Injection seams — `BrowserLauncher`, `CodeReceiver`, `Pacer`

```rust
pub trait BrowserLauncher { fn open(&self, url: &str) -> io::Result<()>; }   // argv asserted as DATA when faked
pub trait CodeReceiver {
    fn port(&self) -> u16;                       // the OS-assigned 127.0.0.1:<port> (RFC 8252 §7.3)
    fn await_query(&self) -> io::Result<String>; // the raw `code=…&state=…`; parse_callback then CSRF-checks it
}
pub trait Pacer { fn wait(&self, secs: u64); }   // device-poll pacing: real bin sleeps; fake records, no sleep

pub struct Callback { pub code: String, pub state: String }  // parse_callback's OUTPUT (§7.5)
```

These are the interactive (`BrowserLauncher`, `CodeReceiver`) and pacing (`Pacer`) impurities of the control plane, all injected so the whole flow is offline-testable (§8, architecture.md §9.4). `SystemBrowserLauncher`/the loopback `CodeReceiver`/`RealPacer`, the RNG that mints the PKCE `verifier`+CSRF `state`, and the atomic `XdgCredStore` live in `bin`; `FakeBrowserLauncher`/`FakeCodeReceiver`/`FakePacer` drive tests.

> **Why `await_query` returns the raw query, not a `Callback`.** The CSRF check is the one assertable security branch (§7.5), and it belongs to the **pure** `parse_callback` — so the impure receiver captures only the bytes off the socket (deferring the request-line parse to the pure `query_from_request_line`) and hands the raw `code=…&state=…` to `parse_callback`, which validates `state` and extracts `code`. Keeping the parse pure is what makes the whole receiver-side logic table-tested with the real socket `bind`/`accept` the only coverage-excluded lines.

> **Why `Pacer` is a seam, not a `Clock::sleep`.** The device poll must pause `interval` seconds between polls in production but must NOT sleep in tests (§7.3). The pause is a *control-plane* pacing concern, kept off the read-only data-plane `Clock` (which stays a single `now()` method, used by the data plane). The real `Pacer` sleeps; the fake records the interval and returns instantly, so `slow_down`'s cumulative `+5 s` is asserted with zero wall-clock. Interaction stays quarantined; this is pacing, not interaction.

### 7.3 (a) Device-code flow (RFC 8628) — default, headless-friendly

```
1. POST {device_url}  (client_id, scope?)           → { device_code, user_code, verification_uri, expires_in, interval? }
2. print user_code + verification_uri to STDERR     (the user opens it on any device)
3. poll {token_url} with Grant::Device every `interval` s (default 5 if absent):
     authorization_pending → keep polling
     slow_down             → add 5 s to interval, CUMULATIVELY, then keep polling
     success               → parse_token_response → store.put → done
     expired_token / device_code deadline passed → error (→ 77)
```

- **`user_code` + `verification_uri` go to STDERR** (architecture.md §7.2) — stdout is reserved for the data-plane stream; `bz login` produces no stdout payload, only the human prompt on stderr and the side-effecting `put`.
- **`interval` defaults to 5 s** when the device-authorization response omits it (RFC 8628 §3.5).
- **`slow_down` adds 5 s cumulatively** (RFC 8628 §3.5) — each `slow_down` raises the polling interval by another 5 s; the increases accumulate, they do not reset.
- **Expiry is a deadline via the injected `Clock`** — `deadline = clock.now() + expires_in` computed once; each poll checks `clock.now() >= deadline` → stop with `DeviceExpired`→77. **Tests do not sleep** (architecture.md §7.2): `FakeClock` advances time and the poll loop's "wait `interval`" is itself a `Clock`-driven step in tests (the real `bin` sleeps; the library's loop is a pure state machine over `(now, last_response)`), so the whole flow runs instantly offline (§8).
- The polling `POST` and the device-authorization `POST` both go through the injected **`Transport`** — `MockTransport` returns the canned `authorization_pending` → `slow_down` → success sequence (§8).

### 7.4 (b) AuthCode + loopback flow (RFC 8252) — `--browser`

```
1. bind ephemeral port on the IPv4 loopback LITERAL 127.0.0.1:0     (CodeReceiver — RFC 8252 §7.3)
2. build_authorize_url with PKCE S256, redirect_uri = http://127.0.0.1:<port>/callback
3. BrowserLauncher::open(url)                                       (the user authenticates)
4. CodeReceiver::await_query() captures the raw ?code=&state= off the loopback
5. parse_callback validates state (CSRF) and extracts code          (PURE)
6. build_token_exchange_request(Grant::AuthCode { code, verifier }) → transport.send → parse_token_response  (PURE builders)
7. store.put(provider, &cred)
```

- **The literal `127.0.0.1:0`** (RFC 8252 §7.3) is the **default** redirect — `127.0.0.1` not `localhost`, and **any** port (`:0` ⇒ the OS assigns an ephemeral port, read back to build the `redirect_uri`). `localhost` may resolve to IPv6 `::1` or hit a hosts-file override; a real shipping-client interop bug (architecture.md §7.2). The bound port is substituted into both the `redirect_uri` of the authorize URL and the listener. **This is a default, not a law: §10.1 makes the redirect host/port/path operator data** (`OAuthConfig.redirect`), because a provider whose registered redirect is `localhost:1455/auth/callback` (OpenAI) must be able to name it; the interop reasoning here is exactly *why* the default is `127.0.0.1`/ephemeral/`/callback`. The socket still binds the IPv4 loopback even when the redirect *string* says `localhost` (§10.1).
- **PKCE S256** (RFC 7636): `bz login` generates a random `code_verifier`, derives `code_challenge = BASE64URL(SHA256(verifier))`, sends `code_challenge` + `code_challenge_method=S256` in the authorize URL, and replays the **`verifier`** in the token exchange. PKCE protects the public client (no client secret) against code interception.
- **`parse_callback` validates `state` (CSRF)** — the `state` returned on the callback MUST byte-equal the `state` `bz login` generated and put in the authorize URL; a mismatch is `CsrfMismatch`→77, never proceeding to token exchange. An `?error=access_denied` callback (the user declined) is surfaced as a login failure→77, not a panic.

### 7.5 The five pure OAuth functions + the unifying `Grant`

OAuth logic is the set of **pure functions** below — table-testable from literals, **zero** I/O, **zero** clock except the explicit `now` argument (architecture.md §7.2, §9.2):

```rust
fn build_authorize_url(cfg: &OAuthConfig, pkce: &Pkce, state: &str, redirect_uri: &str) -> String;
fn parse_callback(query: &str, expected_state: &str) -> Result<Callback, AuthError>;  // CSRF check
fn build_token_exchange_request(cfg: &OAuthConfig, grant: Grant) -> WireRequest;       // over the Grant enum
fn parse_token_response(bytes: &[u8], now: u64) -> Result<TokenResponse, AuthError>;   // sets ABSOLUTE expires_at
fn is_expired(expires_at: u64, now: u64) -> bool;                                      // §6.1
```

```rust
pub enum Grant<'a> {
    AuthCode { code: &'a str, verifier: &'a str, redirect_uri: &'a str },  // loopback flow
    Device   { device_code: &'a str },                                     // device flow poll
    Refresh  { refresh_token: &'a Secret },                                // silent refresh (§6)
}
```

> **The reframe: `Grant` unifies three "paths" into ONE builder.** Auth-code exchange, device-code polling, and silent refresh look like three token requests; they are **one** `POST {token_url}` differing only in form-body parameters — `grant_type` plus a couple of fields. `build_token_exchange_request` matches on `Grant` to fill the body (`authorization_code` + `code`/`redirect_uri`/`code_verifier`; `urn:ietf:params:oauth:grant-type:device_code` + `device_code`; `refresh_token` + the token) and is otherwise identical (same URL, same `client_id`, same content-type, same `parse_token_response` on the way back). There are **not** three token-exchange code paths; there is one builder over a three-armed `Grant` (architecture.md §7.2). This is why §6's refresh and §7.3/§7.4's logins share the same parser and the same `MockTransport` assertions.

`parse_token_response` reads `{ access_token, refresh_token?, expires_in?, scope?, id_token? }` and computes the **absolute** `expires_at` **once** (§5.1, architecture.md §6.4) — `now` is the explicit argument, keeping the function pure. **Expiry source is single-pathed with an empty case** (§10.3): `expires_in` present ⇒ `now + expires_in` (the standard OAuth path, Anthropic); `expires_in` **absent** ⇒ the access token's own `exp` JWT claim (`jwt_exp`, already absolute — no `now +`), which is how OpenAI's token endpoint conveys expiry (it returns **no** `expires_in`); neither present ⇒ `now` (immediately stale, safely forcing a refresh rather than a fixed `unwrap_or(0)` refresh-storm). The optional `id_token` feeds `account_id` derivation (§10.4), not expiry. A token-endpoint error body (`{ "error": "invalid_grant" | "authorization_pending" | "slow_down" | "expired_token", … }`) parses to the corresponding `AuthError`/poll signal — the device-flow poll loop (§7.3) reads `authorization_pending`/`slow_down` as **continue** signals, while refresh (§6) and auth-code read `invalid_grant` as **fatal** (→77). The same parser, different callers' interpretation of the same parsed value — no second parse path.

### 7.6 `browser_argv` — the only conditional compilation

```rust
fn browser_argv(url: &str) -> Vec<String> {   // PURE: returns argv, does NOT exec
    match std::env::consts::OS {
        "macos"   => vec!["open".into(), url.into()],
        "windows" => vec!["cmd".into(), "/C".into(), "start".into(), "".into(), url.into()],
        _         => vec!["xdg-open".into(), url.into()],
    }
}
```

This is the **single** OS-conditional in the **library** (architecture.md §7.3, §10). It returns **argv as data** and does **not** exec — so it is tested as data for all three OS values on one runner (§8). The real `Command::spawn(argv)` that consumes it lives in `SystemBrowserLauncher::open` in the `bin` shim, alongside the loopback socket `bind`/`accept`, the device-poll `sleep`, the OS RNG, and the atomic credential `put` — the whole impure shim is coverage-excluded as a unit (the `bz` bin crate), the pure parsing it calls (`browser_argv`, `query_from_request_line`, the OAuth builders, `Pkce::derive`) staying in the lib at 100% (architecture.md §9.4, §10).

---

## 8. Offline test strategy (zero live network, 100% lib coverage)

The whole auth capability tests with **no network, no clock dependency, no browser, no sleeping** (architecture.md §9.4). Coverage is 100% on the lib (the pure builders/parsers + the three flow drivers behind `MockTransport`/`ScriptedTransport`/`FakeClock`/`FakePacer`/`FakeBrowserLauncher`/`FakeCodeReceiver`); the impure `bz` shim crate — the browser spawn, the socket `bind`/`accept`, the device-poll `sleep`, the OS RNG, the atomic file store — is excluded as a unit (Makefile `cov` `--ignore-filename-regex 'bz/'`), since its logic is the OS calls themselves and the lib already covers the pure parsing it delegates to (architecture.md §9.5, §10).

| Surface | How it is exercised offline |
|---|---|
| `is_expired` | **Pure table test** from literals: fresh (`now+SKEW < expires_at`), stale (`>=`), the exact boundary `now == expires_at - SKEW` (stale), and `now` far past `expires_at`. No clock. |
| `build_authorize_url` | **Pure**: assert the exact URL string — `client_id`, `scope?` (present/absent), `redirect_uri`, `state`, `code_challenge`, `code_challenge_method=S256` — from a fixed `OAuthConfig` + `Pkce` + `state`. |
| `parse_callback` | **Pure table**: matching `state` → `Ok(Callback)`; mismatched `state` → `CsrfMismatch`; `?error=access_denied` → login failure; missing `code` → error. CSRF check is one assertable branch. |
| `build_token_exchange_request` | **Pure**: one assertion per `Grant` arm — `AuthCode` body carries `grant_type=authorization_code`+`code`+`redirect_uri`+`code_verifier`; `Device` carries the device grant_type+`device_code`; `Refresh` carries `grant_type=refresh_token`+token. Same URL/`client_id` across all three (the one-builder proof). |
| `parse_token_response` | **Pure**: a success body → `expires_at == now + expires_in` (absolute, with an injected `now`); rotated vs omitted `refresh_token`; `invalid_grant`/`authorization_pending`/`slow_down`/`expired_token` error bodies → the right `AuthError`/poll signal. |
| `set_auth_header` / `HeaderScheme` | **Pure**: `Raw` → bare secret as `x-api-key`/`x-goog-api-key`; `Bearer` → `Authorization: Bearer <secret>`. Proves the no-vendor-branch header naming (§2). |
| `ApiKey`/`Bearer::apply` | `apply` with an **in-memory `CredStore`**: inline-key path (store untouched), store-hit path, `MissingCreds`→77, `WrongCredKind`→77. Asserts the header on the `WireRequest`. |
| `OAuth2::apply` (refresh) | **`FakeClock` + `MockTransport`**: fresh clock → no refresh `POST`, existing token used; stale clock → one refresh `POST` (asserted body via `MockTransport`), `parse_token_response`, `store.put` (asserted), new token on the wire; `invalid_grant` response → `RefreshFailed`→77; not-logged-in → `NotLoggedIn`→77. **Refresh reuses the data `MockTransport`** — no second mock (architecture.md §9.1). Asserts the `anthropic-beta` auth-mode-dependent header is set (§4). |
| Device flow | **`FakeClock` + `MockTransport`**: canned sequence `authorization_pending` → `slow_down` (assert interval += 5) → success; assert `user_code`/`verification_uri` written to the captured stderr; `FakeClock` past the deadline → `DeviceExpired`→77 — **no sleeping** (the loop's wait is a `Clock` step). |
| AuthCode + loopback flow | **`FakeBrowserLauncher` + `FakeCodeReceiver` + `MockTransport`**: `FakeBrowserLauncher` **records the argv as data** (asserted, never executes — architecture.md §9.4); `FakeCodeReceiver` returns a canned `?code=&state=`; `parse_callback` validates; `MockTransport` serves the token exchange; assert the `put` cred. A **CSRF-mismatch** variant asserts `CsrfMismatch`→77 with **no** token exchange. |
| Loopback `CodeReceiver` | The pure half — `query_from_request_line` (extract the query from the HTTP request line) + `parse_callback` (CSRF) — is table-tested in the lib at 100%. The real `bind`/`accept`/`read`/`write` in the `bz` bin crate is the impure half, coverage-excluded with the rest of that crate. |
| `browser_argv` | **Pure data test** for all three OS strings (`macos`/`windows`/other → the argv vectors) on one Linux runner (architecture.md §9.4). The real `Command::spawn(argv)` in `bin` is the one excluded spawn line. |
| `CredStore` (XDG) | The trait is exercised via the in-memory impl above (data plane). The XDG-path file impl is `bin`-side; `put`'s **atomicity** (temp+rename), **0600** mode-at-create, and **one-file-per-provider** layout are tested against a `tempfile`-rooted store: assert the written file mode is `0o600` on Unix, that `get` after `put` round-trips, and that a partial write is never observable (rename atomicity). |
| `Secret` | **Pure**: `{:?}`/`{}` render `<redacted>`; `Serialize` round-trips plaintext through `CredStore::put` only; `--dump-config` emits `"<redacted>"` (architecture.md §6.2). |

The executable proof of the stateless boundary: the **only** functions that take `&dyn CredStore`/`&dyn Clock` in the data plane are the three `Auth::apply` impls (architecture.md §6.5). A grep-test (or the type signatures themselves) confirms no `resolve`/`parse`/`encode`/`decode`/Sink function touches the store or the clock — the boundary is drawn at exactly one line and is checkable.

---

## 9. Edge cases & architecture change requests

**Decided edge cases (no change request — expressible today):**

- **Inline key beats the store, and never builds a store.** `--api-key`/`BRAZEN_API_KEY` flows on `ResolvedConfig.inline_key`; `ApiKey`/`Bearer::apply` prefer it and the store is constructed lazily, so a fully-stateless run touches zero credential files (§3.1, architecture.md §6.5).
- **`Cred` variant vs row `AuthId` mismatch.** A stored OAuth cred for a row reconfigured to `api_key` is `WrongCredKind`→77, never a silent fallthrough (§3.1).
- **Missing creds vs not-logged-in.** `ApiKey`/`Bearer` with no inline key and no stored cred → `MissingCreds`→77 ("set `BRAZEN_API_KEY` or `bz login`"); `OAuth2` with no stored cred → `NotLoggedIn`→77 ("run `bz login <provider>`"). Both exit 77 (architecture.md §8); the message differs by what would fix it.
- **Refresh never escalates to a browser** (§6.2, architecture.md §7.1). A failed refresh is 77, full stop.
- **`device_url` absent.** `bz login` without `--browser` against a row that has no device endpoint is a **Config** error→78 ("use `--browser`"), never a silent flow switch (§7.1).
- **`slow_down` is cumulative; `interval` defaults to 5 s** (§7.3, RFC 8628 §3.5) — both are decided, not configurable knobs (no new flag — severability, architecture.md §4.6).
- **Rotated vs reused refresh token.** A token response omitting `refresh_token` reuses the prior one; a present one replaces it (§6). `expires_at` is recomputed absolute every time (§5.1).
- **Concurrent refresh.** No lock; last-write-wins on atomic rename, both tokens valid (§6.2, architecture.md §12).
- **Windows secret-at-rest.** User-profile ACL, no DPAPI — a documented limitation, not a branch (§5.2, architecture.md §13 item 4).
- **`Secret` leakage** is a type-level non-issue: `Debug`/`Display` redact, `expose()` is the single audited read site, `Serialize` is reachable only through the 0600 `put` (§5.3).

**Architecture change requests (raised, scoped, NOT silently worked around):**

- **No open change requests against architecture.md.** The auth capability is fully expressible against architecture.md as written — the `Auth` trait (§4.1), the auth-mode-dependent-header split (§4.5), the `Cred`/`CredStore`/`Secret` model (§6.4), the stateless boundary (§6.5), §7 in full (silent refresh, the two `bz login` flows, `browser_argv`), exit 77 (§8), and the offline test strategy (§9.4) cover this spec end to end without amendment. **OpenAI ChatGPT-SSO (§10) likewise needs no architecture change** — it is four data additions to this spec's own structures (`OAuthConfig.redirect`/`authorize_params`/`account_header`, `Cred.account_id`) plus one expiry-source generalization, each defaulting to today's behavior, applied by the existing `OAuth2::apply` with **no new vendor branch**. The vendor stays out of the core (architecture.md §4.4): every OpenAI fact is operator-supplied row data.
- **Watch item (not a change request) — `HeaderScheme` vocabulary.** `HeaderScheme { Raw, Bearer }` (§2) covers every shipped wire convention. A provider that authenticates via a **query parameter** (rather than a header) would need a third arm; recorded as a watch item only, since no v0.1 provider requires it and the `Auth` trait + `WireRequest` already permit the addition as data without a core branch.

---

## 10. OpenAI ChatGPT-SSO ("Sign in with ChatGPT" / Codex) — the OAuth model exercised end to end

> **Status: design source-grounded + live-validated (2026-06-16).** Every OpenAI wire fact below is verified against the open-source `openai/codex` Rust client (`codex-rs/login/`, `codex-rs/model-provider*`, `codex-rs/codex-api/`) AND confirmed against the live service (§10.7). The §10.x changes are **additions to this spec's own structures** (§5.1, §7.1, §7.5) — no architecture.md amendment (§9). Each addition **defaults to today's behavior**, so Anthropic OAuth and every existing row stay byte-identical; OpenAI is reached purely by row data (architecture.md §4.4). §10.7 now records the **validation results** — the design held; one item (refresh content-type) remains open, and one brazen bug (non-2xx body swallow, bl-5fe6) surfaced.

### 10.0 What this flow is, and why it needs more than a config row

"Sign in with ChatGPT" lets a ChatGPT subscriber (Plus/Pro/Team/Enterprise) authorize `bz` against their **subscription** instead of paying per-token with an API key. It is an ordinary RFC 8252 AuthCode + PKCE loopback login (§7.4) — brazen already has the whole machine — **except** for four OpenAI specifics that today's `OAuthConfig`/`Cred` cannot express. The design rule (architecture.md §4.4, §4.6): each specific becomes **row data with a today-preserving default**, applied by the existing `OAuth2::apply`; **zero new vendor branches** enter the core. The verified facts:

| Fact | Value (verified) | Brazen gap → fix |
|---|---|---|
| authorize / token endpoints | `https://auth.openai.com/oauth/authorize`, `.../oauth/token` | none — `OAuthConfig` data today |
| `client_id` | `app_EMoamEEZ73f0CkXaXp7hrann` (Codex's public client) | none — data today |
| `scope` | `openid profile email offline_access api.connectors.read api.connectors.invoke` | none — data today |
| **redirect_uri** | `http://localhost:1455/auth/callback` — host `localhost`, **fixed** port 1455, path `/auth/callback` | **§10.1** — brazen hardcodes `http://127.0.0.1:{ephemeral}/callback` |
| **extra authorize params** | `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `originator=codex_cli_rs` | **§10.2** — brazen emits a fixed param set |
| **token response** | `{ id_token, access_token, refresh_token }` — **no `expires_in`**; expiry is the access token's JWT `exp` | **§10.3** — brazen does `now + expires_in.unwrap_or(0)` ⇒ instantly-stale refresh-storm |
| **account id** | id_token claim `https://api.openai.com/auth` → `chatgpt_account_id`, echoed as data-plane header `ChatGPT-Account-ID` | **§10.4** — brazen has no JWT parse and `beta_headers` are static |
| data-plane base + path | `https://chatgpt.com/backend-api/codex` + protocol path `/responses` ⇒ `…/codex/responses` | none — provider-row `base_url` + the existing `openai_responses` protocol (§10.5) |

### 10.1 The loopback redirect is operator DATA, not a hardcoded literal

Today `redirect_uri` is built in the control plane as `format!("http://127.0.0.1:{}/callback", receiver.port())` (`auth/flows.rs`), with the receiver bound on `127.0.0.1:0` (ephemeral). All three parts — host, port, path — are fixed in code. OpenAI's client registered **exactly** `http://localhost:1455/auth/callback`; redirect-URI matching is byte-exact (loopback gets per-RFC-8252 port latitude, but `localhost` ≠ `127.0.0.1` and `/auth/callback` ≠ `/callback` are hard mismatches). So the redirect must be data.

**Fix — `OAuthConfig.redirect: RedirectSpec` (§7.1), defaulting to today's literal:**

- `host` default `"127.0.0.1"`, `port` default `None` (ephemeral `:0`), `path` default `"/callback"` — the **whole default `RedirectSpec` reproduces the current redirect byte-for-byte**, so deleting the block restores today's behavior (the **severability test**, architecture.md §4.6: removing a default deletes config, not code). Anthropic/any existing row never sets it and is unchanged.
- The **control plane** builds the redirect from the spec: `format!("http://{host}:{port}{path}", port = receiver.port())`, where `receiver.port()` is the **actually-bound** port — single-sourcing the port through the receiver whether ephemeral or fixed (no second home for the number).
- The **bin's `LoopbackReceiver::bind`** takes the requested port: `None ⇒ TcpListener::bind("127.0.0.1:0")` (today), `Some(p) ⇒ "127.0.0.1:p"`. It **always binds the IPv4 loopback `127.0.0.1`**, even when `host = "localhost"`: the browser resolves `localhost` → `127.0.0.1` and connects there, so the listener is reachable; binding the literal avoids the IPv6-`::1` ambiguity the original §7.4 note warns about. The redirect **string** carries `localhost` (to match what OpenAI registered); the **socket** binds `127.0.0.1`. These are not in tension — the string is what the AS validates, the bind is what the browser reaches.
- OpenAI row: `redirect = { host = "localhost", port = 1455, path = "/auth/callback" }`.

> **Reframe — the §7.4 "use `127.0.0.1`, never `localhost`" rule becomes a *default*, not a law.** That rule was right as a default (it dodges a hosts-file / IPv6 interop bug) but wrong as an absolute: OpenAI **registered** `localhost`, so the host is a fact the *provider* chose, i.e. data. The interop reasoning survives intact as the default's justification; it just no longer forbids the operator from naming what their AS requires. This is the same move as `HeaderSpec` dissolving "x-api-key vs Authorization" into `(name, scheme)` data (§2): a hardcoded vendor convention becomes a typed data field.

**Out of scope (documented limitation, not a knob):** Codex's 1455→1457 fallback when 1455 is busy. brazen binds the one configured port; a busy port fails `bz login` with a clear "could not bind 127.0.0.1:1455" (→77), and the operator frees it or re-runs. A fallback-port list would be mechanism for a rare case (severability — no new flag, architecture.md §4.6).

### 10.2 Extra authorize params are operator DATA

`build_authorize_url` (§7.5) emits a fixed set (`response_type`, `client_id`, `redirect_uri`, `state`, `code_challenge`, `code_challenge_method`, `scope?`). OpenAI's consent flow needs three more. **Fix — `OAuthConfig.authorize_params: Vec<(String,String)>` (§7.1), default empty**, appended verbatim after the standard params (percent-encoded by the same `encode_pairs`). Default empty ⇒ existing authorize URLs are byte-identical (the §8 golden-URL tests are untouched; new rows get their own assertion). OpenAI row:

```toml
authorize_params = [
  ["id_token_add_organizations", "true"],
  ["codex_cli_simplified_flow", "true"],
  ["originator", "codex_cli_rs"],
]
```

These are plain key/value login knobs — naming them as data (not a hardcoded OpenAI branch in `build_authorize_url`) keeps the builder vendor-blind.

### 10.3 Expiry: single-path with an empty case, not an `expires_in` special-case

The bug this fixes is concrete: `parse_token_response` computes `expires_at = now + raw.expires_in.unwrap_or(0)`. OpenAI's token response carries **no `expires_in`**, so `unwrap_or(0)` ⇒ `expires_at = now` ⇒ `is_expired` true on the very next request ⇒ a **refresh on every single call** (a self-inflicted refresh-storm, and likely a rate-limit / `invalid_grant` cascade).

**Fix — the expiry source is one expression with an empty case** (the architecture.md §3/§7 "dissolve special cases" discipline), reading the JWT `exp` only when the standard field is absent:

```rust
let expires_at = match raw.expires_in {
    Some(secs) => now + secs,                       // standard OAuth (Anthropic): relative → absolute, once
    None       => jwt_exp(&access).unwrap_or(now),  // OpenAI: the access token's own `exp` (ALREADY absolute)
};
```

- **`jwt_exp(token) -> Option<u64>`** is a new **pure** helper: split on `'.'`, base64url-NOPAD-decode the **payload** (middle) segment, `serde_json` it, read the numeric `exp`. No signature verification — we are not the audience; we only read our own token's stated expiry to schedule refresh (the AS enforces validity). Pure ⇒ table-tested from literal JWTs (§10.6).
- **`exp` is already an absolute unix instant**, so there is **no `now +`** on that arm — the absolute-`expires_at` invariant (§5.1) holds, and adding `now` would double-count.
- **`unwrap_or(now)`** (not `unwrap_or(0)`): an unparseable/opaque token with no `expires_in` reads as *stale now*, safely forcing **one** refresh, never a 1970 timestamp or a never-expires. This is strictly better than today's `unwrap_or(0)` for the existing path too.

This is not an OpenAI branch — it is "expiry comes from `expires_in`, or from the token's own `exp`, or it is unknown (refresh)." Anthropic (which returns `expires_in`) takes the first arm exactly as before.

### 10.4 The account id: derived once at login, echoed as a per-cred header

OpenAI's data plane requires `ChatGPT-Account-ID: <account_id>`. The account id lives in a **claim inside the id_token** (`https://api.openai.com/auth` → `chatgpt_account_id`). This is the one genuinely new *mechanism*, and it splits cleanly along brazen's existing seam:

1. **Derive once, at login (control plane).** `parse_token_response` gains an optional `id_token` read (`RawToken.id_token: Option<String>`); when present it derives `account_id` via a pure **`jwt_account_id(id_token) -> Option<String>`** (the same JWT-payload decoder as `jwt_exp`, reading the nested `https://api.openai.com/auth.chatgpt_account_id`). `TokenResponse` carries `account_id: Option<String>`; `as_cred` stores it on `Cred::OAuth2.account_id` (§5.1), **reusing the prior account_id** when a refresh response omits the id_token (the account does not change on refresh — same carry-forward rule as `refresh_token`/`scope`, §6.2).
2. **Echo on the data plane.** The header is **auth-mode-dependent with a per-credential value** — so it is *not* a static `beta_header` (those are fixed `(k,v)`), but it is the **same shape applied at the same point**: `OAuth2::apply`, right after the bearer token and the static `beta_headers` (§6), does

   ```rust
   if let (Some(name), Some(id)) = (cfg.account_header.as_deref(), account_id.as_deref()) {
       wire.set_header(name, id);     // ChatGPT-Account-ID: <account_id> — NAME is row data, value is the cred fact
   }
   ```

   `cfg.account_header: Option<String>` (§7.1) is the header **name** as data (no vendor string in code); the **value** is the cred's `account_id`. Both `None` for Anthropic ⇒ no header. This stays inside the principle of §4 ("a header that varies by auth mode is auth data, applied by the one impl that owns the mode") — it only widens "static value" to "value sourced from the credential," which is strictly within `OAuth2::apply`'s remit (it already reads the cred).

> **Why store `account_id`, not the id_token.** The id_token's *only* data-plane use is this one claim. Storing the whole JWT (a second, larger home for a fact we already extracted) and re-parsing it per request would violate single-source-of-truth (architecture.md §1) and put JWT parsing in the hot data path. We derive the fact once, store the fact, and the id_token is spent. (If a future need wanted other id_token claims at request time, *that* would justify storing it — not now.) `account_id` is **not** a `Secret`: it is an account identifier echoed in a header, not a credential, and redacting it would corrupt the request (§5.1).

### 10.5 The provider + auth row — the deliverable's payload (pure config, no code)

With §10.1–§10.4 landed, OpenAI ChatGPT-SSO is a **user-config row**, no further code. The data plane is the existing `openai_responses` protocol (`POST {base_url}/responses`, verified to compose to OpenAI's `…/codex/responses`) under the existing `OAuth2` auth:

```toml
[[provider]]
name       = "openai-chatgpt"
base_url   = "https://chatgpt.com/backend-api/codex"
protocol   = "openai_responses"
auth       = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }   # Bearer <access_token> (§3.1 scheme = data)
unsupported_body_keys = ["max_tokens","temperature","top_p"]  # Codex 400s on each (§10.7, bl-73d8/bl-d54a); stripped before encode — the INVERSE of body_defaults (config §4.1.1). A TOP-LEVEL row key (sibling of [provider.oauth]); placing it under [provider.oauth] is now a MalformedFile, not a silent drop — OAuthConfig (and RedirectSpec) carry deny_unknown_fields for parity with the row (config §2.3), so the misplacement is caught rather than swallowed — bl-2869, bl-9649.

[provider.oauth]
authorize_url    = "https://auth.openai.com/oauth/authorize"
token_url        = "https://auth.openai.com/oauth/token"
client_id        = "app_EMoamEEZ73f0CkXaXp7hrann"
scope            = "openid profile email offline_access api.connectors.read api.connectors.invoke"
redirect         = { host = "localhost", port = 1455, path = "/auth/callback" }   # §10.1
authorize_params = [["id_token_add_organizations","true"],["codex_cli_simplified_flow","true"],["originator","codex_cli_rs"]]  # §10.2
account_header   = "ChatGPT-Account-ID"                       # §10.4
beta_headers     = [["originator","codex_cli_rs"]]            # static data-plane header (§4); originator is BOTH an authorize param and a request header per Codex

[provider.body_defaults]                                      # the row's request-body defaults (config §4.1)
store  = false                                                # Codex 400s unless store:false (§10.7); brazen does not model `store`, so it rides the row's passthrough valve
stream = true                                                 # Codex 400s unless stream:true (§10.7); folds into the canonical `stream` gen field at resolve
```

**Why `body_defaults`, and what it buys (config §4.1).** §10.7 found the Codex backend rejects a request unless it carries `store:false` **and** `stream:true`. Before per-row body defaults, the only way to set `store` was a hand-crafted canonical request with a flattened `extra` on every call. The `[provider.body_defaults]` block above makes the ergonomic path just work:

```
bz --provider openai-chatgpt --model gpt-5.4 --system "…" "hi"
```

`stream = true` folds into the canonical `stream` gen field (so the encoder writes `"stream": true`), and `store = false` rides the request's passthrough valve (`req.extra`, seeded from the row) so the encoder emits `"store": false` — both at lowest precedence, beaten by an explicit flag or request field (config §4.1 precedence). Deliberately **absent** from `body_defaults`: `max_tokens`. §10.7 found the Codex backend 400s on `max_output_tokens` (`"Unsupported parameter: max_output_tokens"`), so this row pins **no** `max_tokens` body default — unset, the field is omitted. A standard (non-Codex) OpenAI Responses row may pin `body_defaults = { max_tokens = … }`; this one must not.

**The Codex backend rejects more than `max_output_tokens` — `temperature` and `top_p` 400 the same way** (validated live 2026-06-17, bl-d54a): the second and third fields that lift bl-73d8's deferral of a per-row strip. Omitting `max_tokens` from `body_defaults` stops the *default* from carrying it, but an operator passing `--max-tokens`/`--temperature`/`--top-p` (or those keys in the input JSON) would still 400. `unsupported_body_keys = ["max_tokens","temperature","top_p"]` (above) closes that hole: `strip_unsupported` drops each from the canonical request after `fill_absent`, so the value is silently normalized away and the request streams (config §4.1.1 — the inverse of `body_defaults`). With it, **every** Codex quirk is handled by data, none by operator discipline.

Then: `bz login openai-chatgpt --browser` → ChatGPT consent in the browser → `Cred::OAuth2` (with `account_id`) stored → ordinary `bz` runs stream against the subscription, `OAuth2::apply` silently refreshing (§6).

> **Decision deferred to the operator — ship this row built-in, or keep it a recipe?** §7 states "**no built-in OAuth row ships for v0.1**" (vendor policy = operator data). The Codex `client_id` is a *published public* client, so shipping a built-in `openai-chatgpt` row in `defaults.toml` would be defensible UX. But it would reverse §7's deliberate stance and bake one vendor's login policy into the binary. **Recommendation: keep it a documented recipe** (README + this §10.5 block) the operator pastes into their config — preserving "the core never ships vendor OAuth policy" — unless we consciously revise §7. This is a one-line decision, not a design fork; flagged, not silently chosen.

### 10.6 Offline test additions (100% lib coverage held)

All §10 logic is pure or already-mocked; the §8 strategy extends with **no new live network**:

| Surface | Offline test |
|---|---|
| `RedirectSpec` default | `RedirectSpec::default()` ⇒ `{ "127.0.0.1", None, "/callback" }`; the built redirect for a default row byte-equals today's `http://127.0.0.1:{port}/callback` (no regression). |
| redirect from data | `host="localhost", port=Some(1455), path="/auth/callback"` ⇒ redirect string `http://localhost:1455/auth/callback`; assert `build_authorize_url`'s `redirect_uri` param and the `LoopbackReceiver::bind(Some(1455))` request (the bind itself is the bin's one excluded line). |
| `authorize_params` | empty ⇒ URL byte-identical to today (existing golden holds); the OpenAI triple ⇒ the three extra params appear, percent-encoded, after the standard set. |
| `jwt_exp` | pure table: a literal JWT with `exp` ⇒ that absolute value; missing `exp` / non-numeric / malformed base64 / wrong segment count ⇒ `None`. |
| `parse_token_response` expiry | `expires_in` present ⇒ `now + secs` (Anthropic path, unchanged); absent + JWT `exp` ⇒ that `exp` (no `now +`); absent + unparseable ⇒ `now`. |
| `jwt_account_id` | pure table: id_token with the nested `https://api.openai.com/auth.chatgpt_account_id` ⇒ the id; absent claim / absent namespace / malformed ⇒ `None`. |
| `account_id` carry-forward | refresh response **with** id_token ⇒ new account_id; **without** ⇒ prior account_id reused (mirrors refresh_token reuse, §6.2). |
| `OAuth2::apply` account header | `account_header=Some("ChatGPT-Account-ID")` + cred `account_id=Some(..)` ⇒ header set; either `None` ⇒ **not** set (Anthropic regression guard). Reuses the existing `FakeClock`+`MockTransport` apply harness (§8). |

### 10.7 Live-flow validation results (validated 2026-06-16 / re-confirmed 2026-06-17)

The "go through the flow" phase ran end to end against a real ChatGPT Business account (`bz login openai-chatgpt --browser` → consent → stored `Cred::OAuth2`, then live `bz` runs against `https://chatgpt.com/backend-api/codex/responses`). The design held: every item below was a *data-only* concern and none changed §10.1–§10.5. What follows is the **result**, not a checklist; one item (refresh) remains open because the token has not yet aged out.

**Validated:**

- **Login handshake (end to end).** `account_id` is derived from the id_token claim `https://api.openai.com/auth.chatgpt_account_id` (the `jwt_account_id` pure table, §10.6) — confirmed populated in the stored cred. `expires_at` is parsed from the **access-token JWT `exp`**: OpenAI's token response carries **no `expires_in`**, so the §10.3 JWT-`exp` fallback is **load-bearing, not theoretical**. Full requested scope granted (`openid profile email offline_access api.connectors.read api.connectors.invoke`); cred file stored `0600`.
- **Data-plane auth + wire shape (risk #2).** Accepted with `Authorization: Bearer` + `ChatGPT-Account-ID: <account_id>` + `originator: codex_cli_rs` — no 401/403, **no per-request UUID / `session-id` / `thread-id` / `x-codex-*` required**. brazen's `openai_responses` body is accepted **verbatim** (a normal `bz` run streams `response.*` events and exits 0). The speculative "synthesized-id source" mechanism is therefore **not needed** — drop it from consideration.
- **`originator` placement (risk #3).** Sent in **both** locations (authorize param + `beta_headers` request header) per the §10.5 row; **neither 400s**. No change.
- **Codex backend mandatory request fields** — each omission is a **400 with a descriptive `detail`** (literal service messages, captured live):
  - missing/empty `instructions` (brazen folds `system` → `instructions`, §3.2) → `{"detail":"Instructions are required"}`
  - `stream` not `true` → `{"detail":"Stream must be set to true"}`
  - `store` not explicitly `false` (brazen carries `store` via the `extra` flatten passthrough, §3.2) → `{"detail":"Store must be set to false"}`
  - **`max_output_tokens` present → `{"detail":"Unsupported parameter: max_output_tokens"}`** — NEW finding (2026-06-17). The Codex backend rejects the token cap that the standard Responses API accepts, so a brazen run with `--max-tokens`/`max_tokens` against this row **always 400s**. brazen encodes correctly per the Responses spec (§3.2 renames `max_tokens`→`max_output_tokens`); the Codex backend is the non-standard party. **Resolved as a documented limitation (bl-73d8):** the operator omits `--max-tokens`/`max_tokens` for this row (now warned in the README recipe). A per-row "drop unsupported field" data flag was **deliberately not added** — it would be lone-case mechanism for a single field, against the severability discipline (architecture.md §4.6 / AGENTS.md); add it only if a second backend-rejected field ever joins this one.
- **Working model:** `gpt-5.4`. The `-codex` variants are **gated** for a ChatGPT account: `gpt-5-codex` → 400 `"… model is not supported when using Codex with a ChatGPT account"`. The default `reasoning.effort` echoed by `response.created` is `"none"`.

**Still open (the lone unresolved item):**

1. **Refresh body content-type (was risk #1).** NOT yet confirmed: the live access token is valid for ~10 days, so no refresh has fired. The question stands — Codex sends the refresh as `application/json`; brazen's one-builder (§7.5) sends RFC-6749 `application/x-www-form-urlencoded`, which per RFC 6749 §4.1.3/§6 the endpoint **MUST** accept. *If* a real refresh is rejected, the fix is a per-row `refresh_json: bool` data flag on `OAuthConfig` (defer until proven — architecture.md §4.6). Re-test by forcing a stale clock once the token nears `exp`, or with `auth/refresh` against a captured near-expiry cred.

**Bug discovered during validation — FIXED (bl-5fe6).** brazen used to **swallow the non-2xx response body** — every 400 above decoded to `{"kind":{"provider":{"status":400}},"message":"","provider_detail":null}` (empty message, null detail), because each protocol's whole-body `http_error` read only `v["error"]["message"]` while the Codex backend returns the flat `{"detail":"…"}` envelope. The five per-protocol whole-body functions are now collapsed into **one shared `json::http_error`** that surfaces the **RAW body verbatim in `provider_detail`** (whatever its shape) and a best-effort `message` (known field, else the body itself). So the Codex 400 now decodes to `{"kind":{"provider":{"status":400}},"message":"Store must be set to false","provider_detail":{"detail":"Store must be set to false"}}` — diagnosable from `bz --json` alone, no re-curl. The exit-code mapping was always correct (400 → exit 69).

Cross-refs: design commit `7beffef`, implementation `bccc73b`. The §10 design is **complete** against the verified flow; only the refresh content-type remains to confirm.
