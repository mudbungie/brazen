# Model discovery — `list-models`, default & partial model resolution

> **Living document.** Edited like code. Derives from [Architecture & I/O Contract](architecture.md) — especially §1 (the one-round-trip data plane this amends), §3 (the canonical model), §4.1 (the `Protocol` trait + `WireRequest`), §4.3 (model→provider routing), §4.4 (dispatch with no match-on-provider), §5.9/§8 (errors, exit codes). It MUST NOT contradict architecture.md; where it must, it raises the change request inline and architecture.md changes first (the CRs in §7 are already folded into architecture.md §1 and §4.3).
>
> **Sibling control plane:** [`bz login`](auth.md) — the precedent for a non-pipe verb dispatched in the `bz` shim. **Per-protocol endpoints/list shapes:** §3.1 below (the one home). **Resolution mechanics:** [config.md](config.md) §7.

---

## 1. Purpose & Scope

Make `bz` **Just Work** when the user is imprecise about the model. Three behaviors, one mechanism:

1. **`bz list-models [--provider X] [--json]`** — a control verb (sibling of `bz login`) that does one GET to the resolved provider's models endpoint and prints the available models in the provider's own order, marking the default.
2. **Default selection.** A generation request with **no model** uses the provider's suggested default — the model the API flags as default if any, else the **first in list order** (the heuristic when there is no better signal, architecture.md §4.3).
3. **Partial matching.** `--model opus` resolves to a real wire id: the first model in list order whose id contains the partial (`claude-opus-4-…`) — "the suggested version."

**The cost model (architecture.md §1, amended).** A **fully-specified** model (a full wire id its row owns by `model_prefixes`, an exact `model_aliases` key, or *any present model on a prefix-less row* — which does no fuzzy matching, so it takes the model literally, §5.1) stays **one round-trip** — resolution is offline, unchanged. An **imprecise** model (on a prefix-bearing row, a partial it does not own; or, on any row, an absent model that needs a default) prepends **exactly one** model-list probe to the *single resolved provider*, then the generation round-trip. This is the bounded, explicit-by-imprecision price of not naming a full id — **not** agentic behavior (no loop, retry, or fan-out — §2), and **not** caching (the list is fetched fresh and discarded — architecture.md §2's no-state non-goal holds; nothing is written to disk).

**In scope:** the `list-models` verb, the two new `Protocol` methods (`models_path` + `decode_models`), the canonical `Model`, the pure `select_model` resolver, the `WireRequest.method` field, and the `serve` probe orchestration. **Out of scope (owned elsewhere):** offline routing/alias substitution (config.md §7, architecture.md §4.3 — this spec only adds the *owned-vs-probe* query on top); auth (auth.md — the probe reuses `Auth::apply` verbatim); the impure `HttpTransport`/`CredStore` (architecture.md §6.5).

---

## 2. `bz list-models` — the control verb

Dispatched in the `bz` shim exactly like `login` (architecture.md §11): `args.argv.first() == Some("list-models")` routes to `brazen::list_models` instead of `brazen::run`. It is a **data-plane-config / control-flow** verb — it reuses the full flag parser and `into_resolved` (config.md §7), then replaces "read → encode → stream" with "GET models → print."

```
bz list-models --provider anthropic            # text: ordered ids, default annotated
bz list-models --provider openai --json        # the structured Model list
bz list-models                                  # provider from a configured `provider`/model; else NoProvider (78)
```

- **Provider resolution is the SAME query** (config.md §7): an explicit `--provider`, else the row that owns a configured `model`. Neither → `NoProvider` (78). No model is *needed* (the verb lists them), so a bare `--provider` is the common form.
- **One round-trip.** Build a `GET` `WireRequest` targeting `{base_url}{proto.models_path()}`, stamp the row's `beta_headers` onto it (the protocol headers `encode` would otherwise add — §5.2's note: Anthropic's required `anthropic-version`), apply `Auth::apply` (the same seam — api-key/bearer/oauth, refresh and all), `Transport::send`, then `proto.decode_models(&body)`. The probe (§5.2) shares this exact path through one `fetch_models` home.
- **Output.** `--json`: one JSON object `{"models":[{"id":…,"default":bool},…]}` (the `Model` list, serde-direct, same discipline as the event stream). Default/text: the ids one per line in provider order, the default suffixed ` (default)`. Both go to **stdout**; errors to **stderr** (the verb has no in-band event stream — §5.9's pre-sink rule).
- **Exit codes** (architecture.md §8): `0` success; `78` provider unresolved / empty list; `77` auth; a non-2xx models response maps through `ErrorKind::from_http_status` (4xx→69, 5xx→70) like the data plane; a malformed body (a drained 2xx that does not project to the dialect's list shape) is `ErrorKind::Provider { status: 502 }` — an upstream contract violation (Bad Gateway, exit 70, retryable), the single status `decode_models` raises.

> **Why a verb, not a `--list-models` flag.** It is a distinct *mode of operation* with its own output shape and no request body — the same reason `login` is a verb. A flag would have to no-op the entire request pipeline (prompt, stdin, encode, stream) it shares a parser with; a verb branches once in the shim and the data plane stays untouched (severability — AGENTS.md).

---

## 3. The two `Protocol` additions (DATA + a pure decoder)

`list-models` knowledge is wire-dialect-specific, so it lives on the `Protocol` trait — the one home of dialect knowledge — reached through the **same registry lookup**, never a vendor `match` (architecture.md §4.4). Two methods join `encode`/`decode`/`framing`/`path`:

```rust
pub trait Protocol: Send + Sync {
    // … encode / path / decode / framing …

    /// The models-listing endpoint appended to base_url for a GET. DATA, like `path`.
    /// e.g. openai_chat "/models" (base ends /v1), anthropic "/v1/models" (bare base),
    /// google "/v1beta/models", ollama "/api/tags".
    fn models_path(&self) -> &str;

    /// Decode the provider's (non-streaming) models-list body into the canonical
    /// ordered list. PURE — no IO, fixture-tested like `decode`. Vendor-blind: it
    /// projects the dialect's list shape onto `Vec<Model>`, preserving the provider's
    /// order (the authoritative sequence the default/partial heuristics read, §4).
    fn decode_models(&self, body: &[u8]) -> Result<Vec<Model>, CanonicalError>;
}
```

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

### 3.1 Per-protocol endpoints & list shapes (the one home)

`models_path` is **relative to the row's `base_url`** (so it composes to the full URL just like `path`); each `decode_models` projects that dialect's list shape onto `Vec<Model>`, **preserving order**. This table is the single home for these facts; the dialect mapping specs and providers.md point here rather than duplicate.

| `ProtocolId` | rows | `models_path` | full URL | `decode_models` reads | id |
|---|---|---|---|---|---|
| `OpenAiChat` | openai, mistral | `/models` | `…/v1/models` | `data[].id` (creation order) | as-is |
| `OpenAiResponses` | openai-responses | `/models` | `…/v1/models` | `data[].id` | as-is |
| `AnthropicMessages` | anthropic | `/v1/models` | `…/v1/models` | `data[].id` (newest-first) | as-is |
| `GoogleGenAi` | google | `/v1beta/models` | `…/v1beta/models` | `models[].name` | strip leading `models/` (so the id is usable in encode's `/v1beta/models/{model}:…` path) |
| `OllamaChat` | ollama | `/api/tags` | `…/api/tags` | `models[].name` | as-is (local tags, e.g. `llama3:latest`) |

None of these APIs flags a default today, so `Model.default` is always `false` and §4's first-in-list rule governs; the field stays so a provider that *does* mark one needs no code change. A non-2xx or unparseable body is an error (§2), never an empty list silently treated as "no models."

---

## 4. `select_model` — one resolver for default **and** partial

Default-selection and partial-matching are the **same operation** over the live list, distinguished only by whether the seed is empty — the empty-input dissolve of a special case (AGENTS.md):

```rust
/// Resolve a seed against the provider's ordered model list. PURE, table-tested.
///   seed == ""  → the default: the first model flagged `default`, else models[0].
///   seed != ""  → an exact id if present, else the FIRST id (list order) that
///                 contains the seed case-insensitively ("opus" → "claude-opus-4-…").
/// Empty list → Config (78); a non-empty seed that matches nothing → Config (78),
/// the message naming the seed and a few available ids.
fn select_model(models: &[Model], seed: &str) -> Result<String, CanonicalError>;
```

- **List order is authoritative.** Providers return newest-first (Anthropic) or creation order (OpenAI); the *first* match is "the suggested version" the user asked for. No ambiguity error — unlike provider routing (config.md §7), the order IS the tiebreak the user requested.
- **Exact-before-contains** so a full id the row simply doesn't prefix-own (e.g. an OpenAI id outside the `gpt-`/`o…` families) resolves to itself when the probe confirms it exists, rather than to a longer id that merely contains it.
- **Errors are `ErrorKind::Config`** (→78) — the same model-resolution-failure family as `NoProvider`/`AmbiguousModel` (config.md §7): an empty list (`NoModels`) or an unmatched seed (`NoMatch`) means the request cannot be routed to a real model.

---

## 5. The probe — owned-vs-probe, then `serve` expands

Resolution stays **pure** (config.md §7 has no transport). It adds one query and serve does the impure probe.

### 5.1 The `needs_probe` query (resolution, offline)

`into_resolved` already routes to a single provider and substitutes exact aliases (config.md §7). It now also computes one bool from facts it already has:

> **A model needs a probe iff it is absent (`""`), OR the resolved row opts into fuzzy matching (it declares `model_prefixes`) AND the model is neither an exact `model_aliases` key nor owned by one of those prefixes.** (i.e. `routing_model.is_none() || (row_has_prefixes(resolved_row) && !row_owns(resolved_row, routing_model))`, scoped to the *already-resolved* row — extended to the explicit-`--provider` case, which §7 routing does not check today.)

**Why the `row_has_prefixes` guard (the bl-3989 fix).** Fuzzy/partial expansion is the job of `model_prefixes` (and aliases). A row that declares prefixes *opts into* fuzzy matching, so a model it does not own is a partial seed → probe. A row that declares **no** prefixes *opts out*: it takes a *present* model **literally** — there is no fuzzy expansion to do — so it must NOT probe. The earlier rule `!row_owns(...)` alone was a **lossy proxy**: a prefix-less row owns *no* model (not even a fully-qualified exact wire id like `gpt-5.4`), so it probed on every present model, conflating "the row does fuzzy matching and this isn't a known id" with "the row does NO fuzzy matching at all." On a backend whose `models_path` 400s (Codex has no standard `/models`), that spurious GET was *fatal* to every canonical generation. The fix carries the real fact — *is `cfg.model` a final wire id or a seed to expand?* — instead of the proxy. An absent model on a prefix-less row still probes (it needs a default; there is no literal to take).

This rides `ResolvedConfig` as `pub probe: bool`. It is the **fact** "this is not yet a full wire id," carried — not a lossy proxy re-derived downstream (AGENTS.md): the prefixes that decide it are consumed at resolve and not retained, so serve cannot recompute it and must be told.

- `probe == false` → `cfg.model` is the final wire id (a prefix-owned id passed verbatim, an alias substituted to one, OR — on a prefix-less row — a present model taken literally). **No probe; one round-trip — unchanged.**
- `probe == true` → `cfg.model` holds the **seed**: the partial verbatim (alias substitution is identity for an unowned string, config.md §7) or `""` when absent.

### 5.2 The probe (serve, impure)

`serve` (architecture.md §4.4, the only place with `transport`/`store`/`clock`) gains one step *before* `encode`, taken only when `cfg.probe`:

```rust
// after into_resolved, before building ProviderCtx for the generation request.
// Taken only when cfg.probe AND not --raw: --raw skips encode and never reads the
// model, so the seed it carries is never consumed — probing for it would be waste
// AND would break --raw's one-round-trip-of-exactly-the-user's-bytes contract (§4.2).
if cfg.probe && !raw {
    let mut probe = WireRequest::get(format!("{}{}", cfg.provider.base_url, proto.models_path()));
    for (k, v) in &beta_headers { probe.set_header(k, v); }            // PROTOCOL headers (see below)
    probe.timeouts = cfg.timeouts();
    auth.apply(&mut probe, &ctx, &authc, store, clock, transport)?;  // SAME seam, no new IO surface
    let resp = transport.send(probe)?;                                // probe round-trip
    let models = proto.decode_models(&drain_2xx(resp)?)?;             // non-2xx → from_http_status (§2)
    cfg.model = select_model(&models, &cfg.model)?;                   // expand seed → wire id (§4)
}
// … unchanged: build ctx with the (now full) cfg.model, encode, auth, send, drive …
```

The probe reuses **every** existing seam — `WireRequest`, `Auth::apply`, `Transport::send`, the timeout stamping (auth/refresh inherits the same bounds, config.md §4.3) — and adds none. After it, `cfg.model` is a full wire id and the rest of `serve` is byte-for-byte the current path. The probe is a **GET** (§6); its 2xx body is read whole (a small JSON document, not a stream — it bypasses `drive`/the framers entirely).

> **Protocol headers on the bare GET.** The probe skips `encode`, but `encode` is where a dialect stamps its REQUIRED static headers — notably Anthropic's `anthropic-version` (a row `beta_header`), without which `/v1/models` is a **400**. So the probe (and the `list-models` verb, §2) stamp the resolved row's `beta_headers` onto the GET, exactly as `encode` applies `ctx.beta_headers` — the one place those headers are added on the encode-less path. Offline `MockTransport` ignores headers, so this is asserted by a unit check that the GET carries `anthropic-version`; live, a missing one is a 400. Both the probe and the verb reach this through the **one** `fetch_models` home (`run::models`), so "the GET carries the protocol headers" is single-sourced, never duplicated.

> **Two `Auth::apply` calls** (probe then generation) on the imprecise path. For OAuth the first may silently refresh; the second reuses the fresh token (`is_expired` is the query, auth.md). Idempotent and bounded — no new failure semantics.

---

## 6. `WireRequest.method` — GET joins POST

The models endpoint is a **GET**; every current request is a POST. `WireRequest` gains the method as data:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Method { #[default] Post, Get }

pub struct WireRequest { pub method: Method, /* url, headers, body, timeouts */ }
impl WireRequest {
    pub fn new(url, body) -> Self  // method = Post (the default; encode is unchanged)
    pub fn get(url) -> Self        // method = Get, empty body (the probe + list-models)
}
```

`encode` builds POSTs via the unchanged `new`/`Default`, so no protocol module changes for the method. The impure `HttpTransport` (the `bz` crate) reads `method` to pick the verb; `MockTransport` (testing) records it so a test asserts the probe was a GET to `models_path`. This is the **single** widening of the transport seam — data on the one struct that already crosses it (mirrors `timeouts`, config.md §4.3), not a new `send` parameter.

---

## 7. Change requests to architecture.md (folded in)

This capability could not be added without amending two architecture.md statements; both are CRs raised here and applied there (the providers.md §7 discipline):

- **§1 "exactly one network round-trip per process"** → narrowed to the **generation data plane**, with `bz login` and `bz list-models` named as distinct control paths, and the **imprecise-model probe** named as one bounded prepended round-trip (not a loop/retry/fan-out, not a cache). The fully-specified path is still one round-trip.
- **§4.3 model→provider resolution** → extended with the **owned-vs-probe** query and the live `select_model` expansion: an unowned/absent model is no longer an error or a verbatim passthrough but a *seed* expanded against the live list. The "unowned model requires explicit `--provider`" rule still holds **for routing** — a bare `bz -m opus "q"` with no `--provider` and no configured `provider` is still `NoProvider` (78), because a partial cannot select a provider (that would be a vendor-name table, or an N-provider fan-out — both forbidden). The partial story therefore "just works" once a provider is in scope (`--provider`, or a one-time `provider = "anthropic"` in config).

---

## 8. Testability — pure core, mocked probe

Every behavior is reachable behind the injected seams (architecture.md §6.5, §10); 100% line coverage (the close gate).

| What | Test |
|---|---|
| `decode_models` per protocol | A literal sample body per dialect (§3.1) → expected ordered `Vec<Model>`; a malformed body → `Provider` error. Offline fixtures, like `decode`. |
| `select_model` | Empty seed → first `default`-flagged, else `models[0]`; a partial → exact-before-contains, first-in-order on multiple contains; empty list → `Config`; unmatched seed → `Config` naming the seed. Pure, from literals. |
| `needs_probe` (resolution) | A prefix-owned full id and an exact alias → `false`; on a prefix-bearing row a partial → `true`; an absent model → `true`; and a **prefix-less** row — a *present* model → `false` (literal, no fuzzy expansion; the bl-3989 regression guard), an *absent* model → `true` (needs a default). Computed for both the explicit-`--provider` and the routed cases. |
| serve probe orchestration | `MockTransport` returns a models body on the **first** `send`, a chat stream on the **second**: assert send #1 is a **GET** to `{base_url}{models_path}` and that send #2's encoded body carries the **expanded** wire model. The `probe == false` path sends exactly once (no probe). |
| `list-models` verb | Run-level with a `MockTransport` models body: `--json` emits the `{"models":[…]}` object; default mode emits ids one-per-line with ` (default)` on the default; `NoProvider`/auth/non-2xx map to 78/77/69-70 on stderr. |
| `Method` on the wire | `WireRequest::get` sets `Method::Get` + empty body; `new`/`encode` stay `Post`; `MockTransport` records the method. |

The probe's two-`send` orchestration is the one new *impure-seam* test shape; everything else (`decode_models`, `select_model`, `needs_probe`) is a pure table test, consistent with the rest of the codebase.
