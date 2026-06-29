# Model discovery — `--list-models`, default & partial model resolution

> **Living document.** Edited like code. Derives from [Architecture & I/O Contract](architecture.md) — especially §1 (the one-round-trip data plane + the spine this amends), §2 (the no-state non-goal this amends), §3 (the canonical model), §4.1 (the `Protocol` trait + `WireRequest`), §4.3 (model→provider routing), §4.4 (dispatch with no match-on-provider), §5.9/§8 (errors, exit codes), §6.5 (the injected seams). It MUST NOT contradict architecture.md; where it must, it raises the change request inline and architecture.md changes first (the CRs in §7 are folded into architecture.md §1, §2, §4.3 and §6.5).
>
> **Sibling control plane:** [`bz --login`](auth.md) — the precedent for a non-pipe control short-circuit flag routed in the `bz` shim. **Per-protocol endpoints/list shapes:** §3.1 below (the one home). **Resolution mechanics:** [config.md](config.md) §7.

---

## 1. Purpose & Scope

Make `bz` **Just Work** when the user is imprecise about the model — without ever turning `bz` into something that lists models behind your back. Three behaviors over one **cache**:

1. **`bz --list-models [--provider X] [--json]`** — a control short-circuit flag (sibling of `bz --login`) that does one GET to the resolved provider's models endpoint, prints the available models in the provider's own order (marking the default), **and writes them to a per-provider cache**. It is the **sole writer** of the cache — the only thing in `bz` that ever lists.
2. **Default selection.** A generation request with **no model** uses the provider's suggested default — the model the API flagged as default if any, else the **first in cache order**.
3. **Partial matching.** `--model opus` resolves to a real wire id: the first model **in the cache** whose id contains the partial (`claude-opus-4-…`) — "the suggested version."

**brazen never lists automatically.** The generation path is **read-only** against the cache: it never makes a model-list GET, never spawns, never retries (architecture.md §2 — "not an agent … the caller orchestrates"). A cold or stale cache is the *caller's* to refresh by running `bz --list-models`; `bz` only ever *reads* what that flag wrote.

**The cost model (architecture.md §1, amended).** Every generation resolves its model against the cache (a **local file read** — offline, no network), then does its **one** generation round-trip. There is no probe and no second round-trip, ever:

- **cache hit** (exact or partial match) → the matched wire id → one round-trip.
- **cache miss / no match** → the model string is attempted **verbatim** (§4) → one round-trip (which 404s if it was a partial or a typo; the caller then runs `bz --list-models`).
- `bz --list-models` is its own single round-trip, a separate invocation.

The cache is the **one sanctioned new state** (architecture.md §2, amended): a regenerable JSON file per provider under `$XDG_CACHE_HOME`, written only by `--list-models`, alongside the existing config + credential stores. Deleting it costs nothing — the next `--list-models` rebuilds it.

**In scope:** the `--list-models` control flag (now a cache writer), the one `Protocol` DATA method (`models_shape`) + the one generic `decode_models` it feeds and the per-row `[provider.models]` override (§3.2), the canonical `Model`, the pure **total** `select_model` resolver (verbatim on no match), the `ModelCache` seam, the `WireRequest.method` field, and `serve`'s unconditional cache lookup. **Out of scope (owned elsewhere):** offline routing/alias substitution (config.md §7, architecture.md §4.3); auth (auth.md — the flag's GET reuses `Auth::apply`); the impure `HttpTransport`/`CredStore`/`ModelCache` *impls* (architecture.md §6.5).

---

## 2. `bz --list-models` — the control flag

A **control short-circuit flag**, never an `argv[0]` verb (architecture.md §5.10.1, §13.13). Routed in the `bz` shim exactly like `--login`: the shim calls the lib's `route(argv)` (built on the one `parse_args`) and `Route::ListModels` wires `brazen::list_models` instead of `brazen::run`. It is a **data-plane-config / control-flow** operation — it reuses the full flag parser and `into_resolved` (config.md §7), then replaces "read → encode → stream" with "GET models → print." A leading bare word is therefore ALWAYS a prompt, so `bz "list-models"` / `bz "models"` are valid prompts forever.

```
bz --list-models --provider anthropic          # text: ordered ids, default annotated
bz --list-models --provider openai --json      # the structured Model list
bz --list-models                                # provider from `--provider`/configured `provider`/model; else the FIRST-DECLARED row
```

- **Provider resolution is the SAME query** (config.md §7): an explicit `--provider`, else the row that owns a configured `model`, else (nothing specified) the **first-declared provider row** (config-file order, not alphabetical) — discovery shares the data plane's zero-config default (architecture.md §4.3). So a bare `bz --list-models` lists the default provider's models. No model is *needed* (the flag lists them), so a bare `--provider` is the common form; `NoProvider` (78) is left only for a config with no provider rows.
- **One round-trip.** Build a `GET` `WireRequest` targeting `{base_url}` + the **effective discovery path** — the protocol's `models_shape().path` plus the row's `[provider.models]` `query`, each overridable per row (§3.2) — stamp the row's `beta_headers` onto it (the protocol headers `encode` would otherwise add — Anthropic's required `anthropic-version`, without which `/v1/models` is a 400; the one place those headers ride the encode-less path), apply `Auth::apply` (the same seam — api-key/bearer/oauth, refresh and all), `Transport::send`, then the generic `decode_models(&body, …)` fed the effective `array_key`/`id_key` (protocol default, overridden per row, §3.2). This GET is the **only** model-list fetch in all of `bz`; the generation path never makes it — it reads the cache this flag wrote (§5).
- **Writes the cache.** After a successful decode, `--list-models` calls `cache.put(provider, &models)` (§5.1) — the **sole** write site. Best-effort: a cache-write failure warns on stderr but does not change the exit (the list still printed). This side effect is exactly why `--list-models` is a control short-circuit that the **data plane never triggers** — `run` has no path to it.
- **Output.** The shape is the **resolved `OutMode`** (flag/env/file), read from the same `into_resolved` fold the data plane reads (`ResolvedConfig.output`), not the `--json` flag alone: `--json`, `BRAZEN_OUTPUT=ndjson`, and a config-file `output = "ndjson"` all select `Ndjson` and emit one JSON object `{"models":[{"id":…,"default":bool},…]}` (the `Model` list, serde-direct, same discipline as the event stream — and the exact on-disk cache format, §5.1). Anything else (`Text` default, `Raw`) is the ids one per line in provider order, the default suffixed ` (default)`. Both go to **stdout**; errors to **stderr** (the control flag has no in-band event stream — §5.9's pre-sink rule).
- **Exit codes** (architecture.md §8): `0` success; `78` provider unresolved — the **empty provider table** residue of `NoProvider` (§1), **never** an empty *models* list; `77` auth; a non-2xx models response is routed through the **same `http_error` home the data plane uses** (`protocol::json::http_error`) — `ErrorKind::from_http_status` maps the status (4xx→69, 5xx→70) AND the drained body rides VERBATIM in `provider_detail` with a best-effort `message` (`error.message` / bare `error` / `detail`), so a discovery failure is exactly as diagnosable as a generation one (a 400 `missing anthropic-version`, a 401 auth hint, … reach the user, never a bespoke "HTTP {status}" that throws the body away); a malformed body (a drained 2xx that does not project to the dialect's list shape) is `ErrorKind::Provider { status: 502 }` — an upstream contract violation (Bad Gateway, exit 70, retryable), the single status `decode_models` raises.
- **A valid empty 200 is success (exit 0), not an error.** A well-formed 2xx body that decodes to **zero** models (`{"data":[]}`, `{"models":[]}`) is a successful *empty listing* — the verb LISTS, it does not select, so "the provider returned nothing right now" is honest data, not a failure. The list is **emptied honestly**: stdout prints the empty shape (`{"models":[]}` in `Ndjson`, nothing in `Text`) and a one-line `stderr` note (`no models returned for <provider>`) surfaces it so an empty list is never a silent void. This matters because a `[provider.models].query` pin (§3.2) can be **server-side version-gated** — a stale `client_version` returns a valid empty list, not an error — so the empty-200 path is the **surfaced, documented** behavior of a pin going stale, never an accident. (The empty-cache→`Config` (78) contract is `select_model`'s, on the *generation* path — §4 — not this verb's.)

> **Why a flag, not a verb (superseded — architecture.md §5.10.1, §13.13).** An earlier draft made this a `bz list-models` *verb*, reasoning that a distinct mode with its own output and no request body "should not be a flag." `--dump-config` refutes that: a flag *can* be a distinct mode that short-circuits in the flag layer rather than no-op-ing the request pipeline. The decisive cost is the namespace: an `argv[0]` verb permanently shrinks the set of bare prompts (`bz "models"` would silently break the day a `bz models` verb shipped). So control operations are **flags** in the existing `--help`/`--version`/`--dump-config` family; the data plane stays untouched (`run` has no branch to it), and the bare-prompt namespace is total and frozen.

---

## 3. The `Protocol` models-shape (DATA) + one generic decoder

`list-models` knowledge is wire-dialect-specific, so it lives on the `Protocol` trait — the one home of dialect knowledge — reached through the **same registry lookup**, never a vendor `match` (architecture.md §4.4). It is **all DATA**: one method joins `encode`/`decode`/`framing`/`path`, returning the dialect's models-discovery defaults, and the decode itself is ONE generic free function the verb feeds with those defaults (no per-protocol `decode_models`).

```rust
pub trait Protocol: Send + Sync {
    // … encode / path / decode / framing …

    /// The dialect's models-discovery DEFAULTS as DATA, like `path`: the GET `path`
    /// appended to `base_url`, the top-level `array_key`, the per-entry `id_key`, and
    /// Google's leading-`models/` `strip`. There is no per-protocol `decode_models`
    /// method — the decode is the ONE generic `json::decode_models(body, array_key,
    /// id_key, strip)` the verb feeds with these defaults, OVERRIDDEN per row by
    /// `[provider.models]` (§3.2). e.g. openai_chat path `/models` (base ends `/v1`),
    /// anthropic `/v1/models` (bare base), google `/v1beta/models`, ollama `/api/tags`.
    fn models_shape(&self) -> ModelsShape;
}
```

```rust
/// A dialect's models-list shape as DATA (§3.1). `array_key`/`id_key`/`path` are the
/// protocol DEFAULTS a row's `[provider.models]` block may override (§3.2); `strip` is
/// protocol-only (Google's leading `models/`), never row-overridable — it makes the id
/// usable in encode's path, a fact the operator cannot sensibly change. `&'static str`
/// because every value is a compile-time constant on the protocol impl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelsShape {
    pub path: &'static str,       // the GET path appended to base_url
    pub array_key: &'static str,  // the top-level array of model objects
    pub id_key: &'static str,     // the wire-id field on each entry
    pub strip: &'static str,      // a leading prefix to strip off each id ("" = none)
}
```

The single generic decoder is the existing `json::decode_models(body, array_key, id_key, strip)` — already parameterized, ORDER-PRESERVING, raising the lone `Provider{502}` on a body it cannot project (§3.1). Subtracting the five near-identical per-protocol `decode_models` impls (each just called it with constants) in favor of one path the protocol feeds with `models_shape()` data is the single-source-of-truth move: the keys have ONE home (the shape), and the row override and the decode read the SAME data. The verb (`fetch_models`) and the per-dialect decode tests call this ONE path; nothing forks it.

```rust
/// One available model, the canonical projection of a provider list entry. Ordered
/// position in the returned `Vec` IS the provider's suggested order — the single
/// source the heuristics read; no separate rank field (architecture.md §3.5).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: String,         // the wire id (google strips its `models/` prefix so it is usable in encode's path)
    pub default: bool,      // the API flagged this the default; today no provider does, so this is false and §4's first-in-list rule governs. The seam stays so a provider that DOES flag one needs no code change.
}
```

The `default` flag is **carried, not invented** (AGENTS.md): a protocol whose list shape marks a default sets it; the others leave it `false` and the order decides. There is no `default_model` config field — that would be a second home for "which model is default" that drifts from the list (single source of truth, AGENTS.md).

### 3.1 Per-protocol models-shape defaults (the one home)

`models_shape().path` is **relative to the row's `base_url`** (so it composes to the full URL just like `path`); the generic decoder projects that dialect's list shape onto `Vec<Model>` reading `array_key`/`id_key`, **preserving order**. This table is the single home for these *default* facts (a row may override them, §3.2); the dialect mapping specs and providers.md point here rather than duplicate.

| `ProtocolId` | rows | `path` | full URL | `array_key`.`id_key` | `strip` |
|---|---|---|---|---|---|
| `OpenAiChat` | openai, mistral | `/models` | `…/v1/models` | `data[].id` (creation order) | — |
| `OpenAiResponses` | openai-responses | `/models` | `…/v1/models` | `data[].id` | — |
| `AnthropicMessages` | anthropic | `/v1/models` | `…/v1/models` | `data[].id` (newest-first) | — |
| `GoogleGenAi` | google | `/v1beta/models` | `…/v1beta/models` | `models[].name` | `models/` (so the id is usable in encode's `/v1beta/models/{model}:…` path) |
| `OllamaChat` | ollama | `/api/tags` | `…/api/tags` | `models[].name` | — (local tags, e.g. `llama3:latest`) |

None of these APIs flags a default today, so `Model.default` is always `false` and §4's first-in-list rule governs; the field stays so a provider that *does* mark one needs no code change. A non-2xx or unparseable body is an error (§2); a well-formed **empty** 2xx body is a successful empty list (exit 0, §2), never an error.

**The shape is a DEFAULT, not a constant — the same protocol can serve two list shapes.** `OpenAiResponses` speaks the standard OpenAI `{"data":[{"id":…}]}` for the api-key `openai-responses` row, but the SAME protocol also fronts the ChatGPT-SSO Codex backend (`https://chatgpt.com/backend-api/codex`, an `oauth2` row), whose `/models` route demands a `?client_version=X.Y.Z` query and returns `{"models":[{"slug":…}]}`. The endpoint's **path, query, and list keys are ROW data, not protocol constants** — so they are a per-row override (§3.2), and the protocol still owns only the *default* shape.

### 3.2 `[provider.models]` — the per-row discovery override (path, query, keys)

A row whose discovery endpoint diverges from its protocol's default shape (§3.1) carries one **optional, severable** block — all keys optional — that OVERRIDES the protocol defaults. Its single home is `config.md` (the schema); this is the model-discovery contract:

```toml
[[provider]]
name = "chatgpt"
base_url = "https://chatgpt.com/backend-api/codex"
protocol = "openai_responses"
auth = "oauth2"
# … oauth block …

[provider.models]                            # all keys optional
path      = "/models"                        # default: the protocol's models_shape().path
query     = [["client_version", "0.0.0"]]    # default: none (no `?` appended)
array_key = "models"                         # default: the protocol default ("data" here)
id_key    = "slug"                           # default: the protocol default ("id" here)
```

- **The keys INHERIT the protocol default when omitted (§3.1) — less config.** A row that pins only `query` keeps the protocol's `path`/`array_key`/`id_key`; `strip` is never row-overridable (protocol-only, §3). The effective request is the protocol's `models_shape()` defaults with each present override key replacing its default — computed by ONE pure helper the verb calls, never a per-row branch. **Severability holds:** delete `[provider.models]` → the request reverts to the protocol defaults (deletes config, not code). **Single source of truth:** the protocol still owns the DEFAULT shape; the row only overrides.
- **`query` is GENERAL, not a Codex-specific field.** It is `Vec<(String, String)>` — a list of `[key, value]` pairs, mirroring an `oauth` row's `authorize_params` — not a vendored `client_version` knob (it IS a query string; a name would lie). It is URL-encoded by the **same `encode_pairs` codec** the OAuth authorize URL uses (`auth/urlencode.rs`, reused, not reinvented), appended as `?k=v&…` only when non-empty; an empty/absent `query` appends no `?`, so a default-shape row's URL is byte-for-byte the pre-override `{base_url}{path}`.
- **Version-gating is a SURFACED fragility, not an accident.** The Codex `/models` list is server-side gated on `client_version`: a current version (`0.0.0`/`1.0.0`/`99.0.0`) returns the full list; a stale one (`0.36.0`) returns a valid empty `{"models":[]}`. A pinned `client_version` can therefore silently go stale. brazen **accepts and surfaces** this: the empty list is a successful exit 0 with a one-line `stderr` note (§2), never an error — so a stale pin is a known, documented, observable behavior the operator can re-pin, not a mysterious failure.
- **Whole-block merge, like `beta_headers`.** Across config layers `[provider.models]` is a whole-block `Option::or` (a higher-precedence layer replaces the block, not per-key) — the same discipline as `beta_headers`/`unsupported_body_keys` (config.md §3.2). No embedded `defaults.toml` row ships a `[provider.models]` (every shipped row uses its protocol default), so the override is purely user-authored; a typo'd key inside it is a `MalformedFile`/78 (the block is `deny_unknown_fields`, config.md §2.3).
- **`--raw`/`--json` interactions: none.** Discovery predates and is independent of the output projection — `[provider.models]` only shapes the `--list-models` GET (request URL + decode keys); `--json`/`Ndjson` still selects the `{"models":[…]}` object form (§2) over whatever shape the row decoded, and `--raw` is a generation-path concept the verb never reaches.

---

## 4. `select_model` — one **total** resolver (default, partial, verbatim)

Default-selection, partial-matching, and "the cache can't help — try it literally" are the **same operation** over the cached list, distinguished only by the seed and whether a match is found — the empty-input dissolve of a special case (AGENTS.md). It is **total**: the only failure is the one genuinely unanswerable case (no seed *and* no cache).

```rust
/// What produced the wire id — the provenance the §5.3 404 hint reads (carried, not
/// reconstructed downstream: AGENTS.md). `Cached` = an entry from the list; `Verbatim`
/// = the seed passed through because the cache could not resolve it.
pub enum Provenance { Cached, Verbatim }

/// Resolve a seed against the provider's cached model list. PURE, table-tested.
///   seed == ""  → the default: first `default`-flagged, else models[0] (→ Cached).
///                 EMPTY list → the lone error: ErrorKind::Config (78), "no model given
///                 and no model cache for <provider>; pass --model or run `bz --list-models`"
///                 — `provider` names which cache is cold (carried, not reconstructed).
///   seed != ""  → an exact id if present (Cached); else the FIRST id in list order
///                 containing the seed, case-insensitively (Cached); else the SEED ITSELF
///                 (Verbatim) — attempted literally, since the cache cannot resolve it. A
///                 cold cache (empty list) therefore yields Verbatim for any non-empty
///                 seed: cache-absent ≡ cache-present-but-empty.
fn select_model(models: &[Model], seed: &str, provider: &str)
    -> Result<(String, Provenance), CanonicalError>;
```

- **List order is authoritative.** Providers return newest-first (Anthropic) or creation order (OpenAI); the *first* match is "the suggested version." No ambiguity error — the order IS the tiebreak.
- **Exact-before-contains** so a full id resolves to *itself* when the cache contains it, rather than to a longer id that merely contains it.
- **Verbatim, not error, on no match.** A non-empty seed the cache can't place is passed through unchanged and attempted against the provider. This **self-heals a stale cache**: a brand-new model not yet listed is a full id with no match → tried verbatim → *succeeds*. A partial with no match is tried verbatim → 404 → the caller runs `bz --list-models`. (This replaces the earlier `NoMatch → Config 78`: a present-but-incomplete cache must not veto a model the provider may well accept.)
- **The lone `Config` (78) error** is `seed == "" && models.is_empty()` — nothing to send and no list to default from. It joins `NoProvider`/`AmbiguousModel` in the model-resolution family (config.md §7); **66 (`EX_NOINPUT`) is deliberately *not* used** — that code is the file-open failure (`--input FILE` missing, architecture.md §8) reached outside `from_kind`, and "no model resolvable" is a config-resolution gap, not a missing input file. Reusing the existing family adds no `ErrorKind` variant and no exit-table row (AGENTS.md: minimize mechanism).

---

## 5. The cache — `ModelCache` seam + `serve`'s unconditional lookup

The probe is **dissolved**. There is no `needs_probe` query and no `ResolvedConfig.probe`: resolution (config.md §7, pure) routes to a provider and substitutes aliases, and **that is all it does about the model**. Every generation then resolves its model string (full, partial, or absent) against the cache — uniformly, with no owned-vs-probe branch. `model_prefixes` survives, but now only for **routing** (which row owns a full id, architecture.md §4.3), never to decide whether to expand.

### 5.1 The `ModelCache` seam

The cache is filesystem state, so — like creds — it lives behind an **injected trait** (architecture.md §6.5); the pure lib never touches the disk. It is a sibling of `CredStore`, not folded into it: secrets and a regenerable model list are different facts with different files (minimize-and-don't-conflate, AGENTS.md).

```rust
/// The per-provider model-list cache (model-discovery.md §5). The bz bin (`src/native/`) backs it
/// with one JSON file per provider under $XDG_CACHE_HOME/brazen/models/<provider>.json;
/// `testing` has an in-memory double. Regenerable: a miss — or an unreadable/corrupt
/// file — is `None`, never an error (it self-heals on the next `list-models`).
pub trait ModelCache {
    fn get(&self, provider: &str) -> Option<Vec<Model>>;   // None == no usable cache
    fn put(&self, provider: &str, models: &[Model]);       // list-models ONLY; atomic temp+rename, best-effort
}
```

- **Key = the provider row name** (`cfg.provider.name`) — the same key `CredStore` uses (`AuthCtx.store_key`). One identity per provider across both stores.
- **Format = the `{"models":[{"id":…,"default":…}]}` shape `list-models --json` emits** (§2) — one serialization, reused, never re-invented.
- **`get` is forgiving:** a missing file, a parse error, or garbage is `None` (the empty list), so a corrupt cache degrades to the verbatim path, never a hard failure.
- **`put` is the verb's alone** and best-effort (atomic `temp + rename` so a concurrent `bz` never reads a half-written file); a write failure warns but does not fail `list-models`.

`run` gains `cache: &dyn ModelCache` — the **one** spine widening this capability needs (architecture.md §1 CR, §7). `main` wires the XDG-file impl; tests inject the in-memory double.

### 5.2 The lookup (serve, impure)

`serve` (the only place with `transport`/`store`/`clock`/`cache`) resolves the model against the cache **before `encode`**, for every request — no `probe` guard:

```rust
// after into_resolved (which no longer computes probe), before building ProviderCtx.
// --raw skips it: encode is bypassed and the model is never read, so resolving it would
// be waste and would break --raw's exactly-the-user's-bytes contract (config.md §4.2).
if !raw {
    let models = cache.get(&cfg.provider.name).unwrap_or_default();   // miss → empty list
    let (wire, prov) = select_model(&models, &cfg.model)?;            // §4: match → Cached, else Verbatim
    cfg.model = wire;                                                 // now a concrete string to send
    cfg.model_from_cache = matches!(prov, Provenance::Cached);        // carried for the §5.3 404 hint
}
// … unchanged: build ctx with cfg.model, encode, auth, send, drive …
```

This is a **local file read, not a round-trip** — offline, microseconds, and a miss costs nothing (empty list → `select_model` returns the seed verbatim). A fully-qualified `bz -m gpt-5.4 "hi"` against an empty cache resolves to `gpt-5.4` verbatim, **byte-for-byte the pre-cache behavior** — so the feature is transparent until someone runs `list-models`.

> **This subsumes the bl-3989 regression entirely.** The old probe could fire a fatal `/models` GET on a prefix-less row's fully-qualified `--model`; the fix was a `row_has_prefixes` guard on `needs_probe`. With no auto-GET at all — the lookup is a file read — that whole failure mode and its guard **disappear**. No generation path ever GETs `/models`.

### 5.3 The `404` on the generation request — provenance, not a retry

A model that resolved (from cache or verbatim) and then **404s** at the provider is **not** auto-refetched or retried (architecture.md §2). It fails with the provider's status (exit 69) — but the message is **enriched by the carried `model_from_cache` provenance** so the caller knows the next move:

- **resolved from the cache** (`Cached`) that 404s → the listed entry was deprecated *since* `list-models` ran → hint: *"`<model>` was in the cache but the provider rejected it; the cache may be stale — re-run `bz --list-models`."* We **know** it was on the list.
- **attempted verbatim** (`Verbatim`) that 404s → either a cold/partial cache or a typo → hint: *"`<model>` is not in the model cache; run `bz --list-models` to refresh or enable partial matching."*

Both exit **69**; only the message differs, driven by the one provenance bool. The symmetric staleness — a *new* model missing from a stale cache — surfaces on the **same** path with no error at all: a full id with no cache match is tried verbatim and simply *succeeds* (§4).

> **One `Auth::apply` on the generation path.** The cache read is local and needs no auth, so generation auths exactly once (the probe's second auth call is gone). `bz --list-models` does its own single `Auth::apply` for its GET. No double-auth, no new failure semantics.

---

## 6. `WireRequest.method` — GET joins POST

The models endpoint is a **GET**; every current request is a POST. `WireRequest` gains the method as data:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Method { #[default] Post, Get }

pub struct WireRequest { pub method: Method, /* url, headers, body, timeouts */ }
impl WireRequest {
    pub fn new(url, body) -> Self  // method = Post (the default; encode is unchanged)
    pub fn get(url) -> Self        // method = Get, empty body (the list-models verb's GET — the one GET in bz)
}
```

`encode` builds POSTs via the unchanged `new`/`Default`, so no protocol module changes for the method. The impure `HttpTransport` (the `bz` crate) reads `method` to pick the verb; `MockTransport` (testing) records it so a test asserts the `list-models` verb's GET targets the effective models endpoint (`models_shape().path`, overridable per row, §3.2). This is the **single** widening of the transport seam — data on the one struct that already crosses it (mirrors `timeouts`, config.md §4.3), not a new `send` parameter.

---

## 7. Change requests to architecture.md (folded in)

This capability amends four architecture.md statements; all are CRs raised here and applied there (the providers.md §7 discipline):

- **§1 spine + cost model.** (a) `run` gains a fourth injected seam, `cache: &dyn ModelCache` (§5.1) — the model-list cache, sibling of `store: &dyn CredStore`. (b) "exactly one round-trip": the generation data plane is **still one round-trip**, but the imprecise case no longer prepends a probe — it reads the **cache** (a local file, offline) and falls back to a verbatim attempt. `bz --login` and `bz --list-models` remain the named control paths; `--list-models` is now also the cache's sole writer.
- **§2 non-goals.** "No cache" is amended: a **regenerable model-list cache** (`$XDG_CACHE_HOME`, written only by `list-models`) joins XDG config + credentials as a sanctioned state exception. The "not an agent / no retry / caller orchestrates" non-goal is **strengthened, not bent**: the generation path now *never* lists or retries — a cold/stale cache is the caller's to refresh (`bz --list-models`), and a wrapper that wants auto-list-then-retry maps the 404 itself.
- **§4.3 resolution.** The "owned-vs-probe" query and `ResolvedConfig.probe` are **removed**. Resolution does routing + alias substitution only; the model string (full, partial, or empty) is then a **seed** resolved against the cache in `serve` by the total `select_model` (§4). The "a partial cannot pick a provider" rule is unchanged — `bz -m opus "q"` with no provider in scope is still `NoProvider` (78).
- **§6.5 seams.** `ModelCache` joins `Transport`/`CredStore`/`Clock` as an injected impure seam, with an XDG-file impl in `bz` and an in-memory double in `testing`.

---

## 8. Testability — pure core, mocked cache + transport

Every behavior is reachable behind the injected seams (architecture.md §6.5, §10); 100% line coverage (the close gate).

| What | Test |
|---|---|
| `models_shape` + generic decode per protocol | Each protocol's `models_shape()` → the expected DATA defaults (§3.1); the generic `decode_models` fed those defaults on a literal sample body per dialect → the expected ordered `Vec<Model>`; a malformed body → `Provider` error; a well-formed empty body → an empty `Vec`. Offline fixtures, like `decode`. |
| `[provider.models]` override (§3.2) | The pure request-shape helper from literals: an omitted block inherits the protocol `path`/`array_key`/`id_key` (and appends no `?`); a row overriding each replaces it; a partial block (only `query`) keeps the rest; `query` URL-encodes via `encode_pairs` (a value needing percent-encoding asserts the `?k=v` tail). The Codex `{"models":[{"slug":…}]}` shape decodes via `array_key="models"`/`id_key="slug"`; a valid empty `{"models":[]}` over the override is exit 0. |
| `select_model` | Empty seed → first `default`-flagged else `models[0]` (`Cached`); empty seed + empty list → `Config` (78); a partial → exact-before-contains, first-in-order on multiple contains (`Cached`); a non-empty seed with no match → the seed verbatim (`Verbatim`); a full id present in the list → itself (`Cached`). Pure, from literals. |
| `ModelCache` round-trip | The in-memory double: `put` then `get` returns the list; `get` on an unknown provider → `None`; the XDG-file impl — a corrupt/missing file → `None` (forgiving), `put` is atomic (temp+rename). |
| serve cache lookup | `MockTransport` returns a chat stream on its **only** `send` (no probe send): a primed cache makes a partial resolve to the expanded wire id in the encoded body; an **empty** cache makes a full id pass through verbatim; `--raw` skips the lookup entirely. |
| 404 provenance | A 404 on a `Cached`-resolved model → exit 69 + the "cache may be stale" hint; a 404 on a `Verbatim` model → exit 69 + the "not in cache" hint. |
| `list-models` verb | Run-level with a `MockTransport` models body: `--json` **and** `BRAZEN_OUTPUT=ndjson` (the resolved `OutMode`, no flag) both emit the `{"models":[…]}` object; default mode emits ids one-per-line with ` (default)`; a bare `--list-models` defaults to the first provider row; the cache double records the `put`; unknown-provider/auth/non-2xx map to 78/77/69-70 on stderr. |
| `Method` on the wire | `WireRequest::get` sets `Method::Get` + empty body; `new`/`encode` stay `Post`; `MockTransport` records the method (the verb's GET to the effective models endpoint, `models_shape().path`). |

The cache lookup makes `serve` a **single-`send`** path again (the generation round-trip only) — the two-`send` probe orchestration is gone. Everything but the `MockTransport`/`ModelCache` doubles is a pure table test (`decode_models`, `select_model`), consistent with the rest of the codebase.
