# Auth, OAuth/SSO & the credential store

> **Living document.** Edited like code. This spec derives from the canonical contract in architecture.md and MUST NOT contradict it. Where the auth model cannot be expressed without an architecture change, this spec raises a **change request** (§9) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md)

---

## 1. Purpose & Scope

API-key, bearer, and OAuth2 are **one problem**: produce the finished auth headers on a `WireRequest`, given a `CredStore` and a `Clock` (architecture.md §7). This spec defines, normatively, that whole capability:

- the `Auth` trait and its **three** registered impls — `ApiKey`, `Bearer`, `OAuth2` (architecture.md §4.1, §4.4) — and exactly what each `apply` does, where the secret comes from, and how `ctx.api_header` data drives header naming with **no vendor branch** (§3);
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
3. **The core never matches on a vendor name.** `Auth` impls are reached by `registry.auths[&cfg.provider.auth]` — a map lookup keyed by `AuthId`, never a `match` on a name (architecture.md §4.4). The header *name* is `ctx.api_header` data, not a vendor branch (architecture.md §4.5).
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
        ctx:       &ProviderCtx,     // shared capabilities (api_header, beta_headers) — also handed to encode
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
    pub api_header:   &'a HeaderSpec,           // x-api-key | Authorization:Bearer | x-goog-api-key — DATA
    pub beta_headers: &'a [(&'a str, &'a str)], // provider-level STATIC headers (e.g. anthropic-version)
    pub extra:        &'a Map<String, Value>,
}

pub struct AuthCtx<'a> {                // auth-private — NEVER handed to encode
    pub store_key:  &'a str,                    // the provider name, used ONLY as a CredStore key — never matched on
    pub inline_key: Option<&'a Secret>,         // the §6.5 inline-key bypass; absent ⇒ store.get(store_key)
    pub oauth:      Option<&'a OAuthConfig>,    // resolved auth-row data (§7.1); Some iff AuthId::OAuth2
}
```

Both are `ResolvedConfig` projections (`ProviderCtx::from(&cfg)` / `AuthCtx::from(&cfg)`, architecture.md §4.4). Three consequences are load-bearing:

- **The credential never enters the protocol layer.** `inline_key` is a `Secret` on `AuthCtx`, not `ProviderCtx`, so `Protocol::encode` is **structurally barred** from it — this is what makes "`Auth::apply` is the ONLY data-plane function permitted to touch credentials" (architecture.md §6.5) a *type-level* fact, not a convention. The provider row's secret-free capabilities (`api_header`, `beta_headers`) stay on `ProviderCtx`, which encode legitimately needs.
- **`store_key` is a key, not an identity.** It is the resolved provider name used **solely** to index `CredStore`; nothing reads it to branch on *which* provider — the vendor name is still spent on the registry lookup before `apply` runs (architecture.md §4.1, §4.4). A *string key into a store*, never a `match` on it.
- **`oauth` is present exactly when needed.** Resolution pairs `AuthId::OAuth2` with a present `OAuthConfig` — else a **Config** error at resolve (78), the same surfaced-ambiguity rule as model→provider routing (architecture.md §4.3); `ApiKey`/`Bearer` ignore it.

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

## 3. The `Auth` impls — **two** structs behind **three** ids

There are **two** `Auth` impls, both **zero-cost, `&'static dyn`-shareable**, registered once in `Registry::builtin()` (architecture.md §4.4). `api_key` and `bearer` are **one** impl under two ids — they are "not two dispatch sites" (§3.1), so making them two structs would be exactly the redundant second representation single-source-of-truth forbids:

```rust
auths.insert(AuthId::ApiKey, &StaticSecretAuth);   // same impl…
auths.insert(AuthId::Bearer, &StaticSecretAuth);   // …under two intent-naming ids
auths.insert(AuthId::OAuth2, &OAuth2Auth);
```

**Both are unit structs** (stateless, `&'static dyn`-shareable). `StaticSecretAuth` reads its secret via `AuthCtx`; `OAuth2Auth` reads its endpoints/`client_id`/`scope`/`beta_headers` from the **`OAuthConfig` on `AuthCtx`** (§1.3, §7.1) — never hard-coded and never stored on the impl, so the registry shares one `&OAuth2Auth` across every OAuth row.

### 3.1 `StaticSecretAuth` — the staleness-free impl behind `api_key`/`bearer`

`api_key` and `bearer` differ **only** in `HeaderScheme`, and that difference already lives on the row's `HeaderSpec` (`scheme: Raw` vs `Bearer`), so they are the **same code** modulo the header value — `set_auth_header` (§2) handles both. They are kept as two `AuthId`s purely so config names the intent (`auth = "api_key"` vs `auth = "bearer"`); they are **not** two dispatch sites, so they are **one** `StaticSecretAuth` impl the registry maps both ids onto.

```rust
impl Auth for StaticSecretAuth {
    fn apply(&self, wire, ctx, auth, store, _clock, _transport) -> Result<(), Error> {
        let secret = resolved_secret(store, auth)?;       // inline_key ?? store.get(store_key) ?? Err(MissingCreds→77)
        set_auth_header(wire, ctx.api_header, &secret);   // header NAME + scheme from ctx.api_header — DATA
        Ok(())                                            // no clock, no transport: refresh is the empty case
    }
}
```

- **Secret source, in precedence order:** the resolved `inline_key` (`--api-key` / `BRAZEN_API_KEY` / `ANTHROPIC_API_KEY`, flowing on `ResolvedConfig`) **if present**, else `store.get(provider)` yielding the matching `Cred` variant, else `Err(Auth::MissingCreds)` → **exit 77** (architecture.md §7). The inline-key path **never constructs a `CredStore`** (the store is built lazily — architecture.md §6.5), so a fully-stateless run touches zero files except stdin.
- **`_clock`/`_transport` are unused** — and that is the point. "Refresh if stale" has an **empty case** for a secret that never goes stale; `StaticSecretAuth` *is* that empty case, not a special one (architecture.md §3, §7). It does **no** I/O of any kind beyond reading the store.
- **No vendor branch.** The header *name* is `ctx.api_header.name`; the *scheme* is `ctx.api_header.scheme`. Google's `x-goog-api-key` is `StaticSecretAuth` against a `Raw` row whose `HeaderSpec` names it — the same impl (architecture.md §4.6).

The **bearer** path is the same `apply` reached through `AuthId::Bearer`; the row's `api_header = { name: "Authorization", scheme: "bearer" }` makes `set_auth_header` emit `Authorization: Bearer <token>`. The store yields a `Cred::Bearer { token }` (vs `Cred::ApiKey { key }`) — the variant is the discriminant, so there is **no `token_type` flag** to read (architecture.md §6.4). `resolved_secret` accepts **either** stored variant (the scheme, not the `Cred` kind, decides the header shape), so a `Cred::Bearer` under a `Raw` row or a `Cred::ApiKey` under a `Bearer` row both resolve — only a stored `OAuth2` cred is rejected (`WrongCredKind`).

```rust
fn resolved_secret(store: &dyn CredStore, auth: &AuthCtx) -> Result<Secret, Error> {
    if let Some(inline) = auth.inline_key            { return Ok(inline.clone()); } // stateless path — store untouched
    match store.get(auth.store_key) {
        Some(Cred::ApiKey { key })   => Ok(key),
        Some(Cred::Bearer { token }) => Ok(token),
        Some(Cred::OAuth2 { .. })    => Err(Error::auth(AuthError::WrongCredKind)), // row says api_key/bearer but stored cred is OAuth — config drift, 77
        None                         => Err(Error::auth(AuthError::MissingCreds)),  // → 77, message: set BRAZEN_API_KEY or `bz login`
    }
}
```

(`auth.inline_key` / `auth.store_key` are the `AuthCtx` fields of §1.3 — auth-private data, *not* a vendor name. A `Cred` variant mismatching the row's `AuthId` is surfaced as `WrongCredKind`→77, never a silent fallthrough — the store could hold a stale OAuth cred for a row reconfigured to api-key.)

### 3.2 `OAuth2` — the only impl where staleness exists

`OAuth2::apply` is the sole `Auth` impl that uses `clock` and `transport` (architecture.md §7.1). Its full body is §6. It also applies the **auth-mode-dependent** beta header (§4). OAuth knowledge — refresh, the bearer header, *and* `anthropic-beta: oauth-2025-04-20` — is **fully contained in this one impl** (architecture.md §4.5).

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

---

## 5. The credential store

### 5.1 `Cred` — the stored secret, discriminant-as-variant

```rust
#[derive(Serialize, Deserialize)]
pub enum Cred {
    ApiKey { key: Secret },
    Bearer { token: Secret },
    OAuth2 { access_token: Secret, refresh_token: Secret, expires_at: u64, scope: Option<String> },
}
```

Single source of truth applied to creds (architecture.md §6.4):

- **No `is_valid` flag.** Freshness is the **query** `is_expired(expires_at, now)` (§6.1) — a stored boolean would be wrong the instant the clock moved past it.
- **`expires_at` is ABSOLUTE** (a unix-seconds instant), computed **once** as `clock.now() + expires_in` at the moment the token response is parsed (§7.4). Storing the **relative** `expires_in` would be wrong the instant it is read back from disk in a later process.
- **No `token_type` flag.** The `Cred` variant **is** the discriminant; `OAuth2` is always a bearer token, so a `token_type:"Bearer"` field would be a second, driftable home for a fact the variant already states.
- **`scope` is `Option<String>`** — the granted scope as returned, carried for audit/diagnostics; `None` is the no-scope case, not special.

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

The **only** time source in the data plane, injected into `run` and handed to `apply` (architecture.md §1, §6.5). The library **never** calls `SystemTime::now`; `SystemClock` (the `bin` impl) does, and `FakeClock` drives fresh/stale in tests with no sleeping (§8, architecture.md §9.4). `now()` feeds exactly two computations: `is_expired` (§6.1) and the **absolute** `expires_at = now + expires_in` (§7.4).

---

## 6. Silent refresh — the only stateful thing in a normal run

`OAuth2::apply` is the one place a normal (non-`login`) run reads **and writes** the store (architecture.md §7.1). It detects staleness with a pure clock comparison, refreshes over the **same `Transport` seam** (no second network path), persists the new token, and uses it — all before `transport.send` of the real request.

```rust
impl Auth for OAuth2Auth {
    fn apply(&self, wire, ctx, auth, store, clock, tx) -> Result<(), Error> {
        let cfg = auth.oauth.ok_or_else(Error::oauth_row_misconfigured)?;  // resolve guarantees Some (§1.3); defensive → 78
        let Some(Cred::OAuth2 { access_token, refresh_token, expires_at, scope }) = store.get(auth.store_key)
            else { return Err(Error::auth(AuthError::NotLoggedIn)); };   // → 77: "run `bz login <provider>`"

        let token = if is_expired(expires_at, clock.now()) {
            let wire_tok = build_token_exchange_request(cfg, Grant::Refresh(&refresh_token)); // PURE
            let bytes    = tx.send(wire_tok)?.collect_to_end()?;          // the ONE impure seam (mockable)
            let fresh    = parse_token_response(&bytes, clock.now())      // PURE; sets ABSOLUTE expires_at
                              .map_err(|_| Error::auth(AuthError::RefreshFailed))?;  // invalid_grant → 77
            store.put(auth.store_key, &fresh.as_cred(&refresh_token, &scope))?;  // persist-then-use
            fresh.access_token
        } else {
            access_token
        };

        set_auth_header(wire, ctx.api_header, &token);          // Authorization: Bearer <token> (scheme from row)
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
    #[serde(default)] pub beta_headers: Vec<(String, String)>,  // auth-mode-dependent headers (§4)
}
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

- **The literal `127.0.0.1:0`** (RFC 8252 §7.3) — **not** `localhost`, and **any** port (`:0` ⇒ the OS assigns an ephemeral port, read back to build the `redirect_uri`). `localhost` may resolve to IPv6 `::1` or hit a hosts-file override; a real shipping-client interop bug (architecture.md §7.2). The bound port is substituted into both the `redirect_uri` of the authorize URL and the listener.
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

`parse_token_response` reads `{ access_token, refresh_token?, expires_in, scope? }` and computes the **absolute** `expires_at = now + expires_in` **once** (§5.1, architecture.md §6.4) — `now` is the explicit argument, keeping the function pure. A token-endpoint error body (`{ "error": "invalid_grant" | "authorization_pending" | "slow_down" | "expired_token", … }`) parses to the corresponding `AuthError`/poll signal — the device-flow poll loop (§7.3) reads `authorization_pending`/`slow_down` as **continue** signals, while refresh (§6) and auth-code read `invalid_grant` as **fatal** (→77). The same parser, different callers' interpretation of the same parsed value — no second parse path.

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

This is the **single** OS-conditional in the **library** (architecture.md §7.3, §10). It returns **argv as data** and does **not** exec — so it is tested as data for all three OS values on one runner (§8). The real `Command::spawn(argv)` that consumes it lives in `SystemBrowserLauncher::open` in the `bin` shim, alongside the loopback socket `bind`/`accept`, the device-poll `sleep`, the OS RNG, and the atomic credential `put` — the whole impure shim is coverage-excluded as a unit (`src/bin/`), the pure parsing it calls (`browser_argv`, `query_from_request_line`, the OAuth builders, `Pkce::derive`) staying in the lib at 100% (architecture.md §9.4, §10).

---

## 8. Offline test strategy (zero live network, 100% lib coverage)

The whole auth capability tests with **no network, no clock dependency, no browser, no sleeping** (architecture.md §9.4). Coverage is 100% on the lib (the pure builders/parsers + the three flow drivers behind `MockTransport`/`ScriptedTransport`/`FakeClock`/`FakePacer`/`FakeBrowserLauncher`/`FakeCodeReceiver`); the impure `src/bin/` shim — the browser spawn, the socket `bind`/`accept`, the device-poll `sleep`, the OS RNG, the atomic file store — is excluded as a unit (Makefile `cov` `--ignore-filename-regex 'src/bin/'`), since its logic is the OS calls themselves and the lib already covers the pure parsing it delegates to (architecture.md §9.5, §10).

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
| Loopback `CodeReceiver` | The pure half — `query_from_request_line` (extract the query from the HTTP request line) + `parse_callback` (CSRF) — is table-tested in the lib at 100%. The real `bind`/`accept`/`read`/`write` in `bin` is the impure half, coverage-excluded with the rest of `src/bin/`. |
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

- **No open change requests.** The auth capability is fully expressible against architecture.md as written — the `Auth` trait (§4.1), the auth-mode-dependent-header split (§4.5), the `Cred`/`CredStore`/`Secret` model (§6.4), the stateless boundary (§6.5), §7 in full (silent refresh, the two `bz login` flows, `browser_argv`), exit 77 (§8), and the offline test strategy (§9.4) cover this spec end to end without amendment.
- **Watch item (not a change request) — `HeaderScheme` vocabulary.** `HeaderScheme { Raw, Bearer }` (§2) covers every shipped wire convention. A provider that authenticates via a **query parameter** (rather than a header) would need a third arm; recorded as a watch item only, since no v0.1 provider requires it and the `Auth` trait + `WireRequest` already permit the addition as data without a core branch.

---
