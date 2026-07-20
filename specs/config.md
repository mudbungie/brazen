# Config schema, resolution & the compiled config

> **Living document.** Edited like code. This spec derives from the canonical contract in architecture.md and MUST NOT contradict it. Where it cannot be written without changing the architecture, it raises a **change request to architecture.md** (§9) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md) — §3.1, §4.2, §4.3, §4.4, §6, §8, §13.

---

## 1. Purpose & Scope

Define, normatively, how `bz` turns four sparse inputs — **flags, environment, config file, embedded defaults** — into the one `ResolvedConfig` the pipeline runs on, and how it surfaces that resolution back out as a file via `--dump-config`. This is the spec for `config/resolve.rs` and `config/provider.rs` (architecture.md §11).

**In scope:** the `PartialConfig`/`PartialProvider` schema and why the four inputs are four instances of *one* type (§2), and the additive-only **forward-evolution** invariant that keeps a config valid across every future brazen version with no version field (§2.4); the `resolve` fold under `Option::or` with precedence-as-operand-order, the injected `EnvSnapshot` projection, embedded `defaults.toml` through the same parse path, and the missing-file-is-identity rule (§3); how the request is **not** a fold layer and how `fill_absent`/`getConfigValue` fill only the gaps, including the per-row `body_defaults` map (§4); the config-file location fold (`--config` > `$BRAZEN_CONFIG` > XDG) (§5); `--dump-config` as the only bridge between flag-encoding and file-encoding (§6); every `into_resolved()` `Config` (78) error, and the greedy-first model→provider resolution that retired the ambiguity error (§7); the `[ingress]` table's schema, fold, and serve-time resolution (§2.5 — semantics owned by [ingress.md](ingress.md)); the testability story (§8).

**Out of scope (owned elsewhere):** the credential store, `Secret` semantics, OAuth, and `bz --login` (architecture.md §6.4, §7 — the auth spec); the provider-row *contents* and which protocol/auth a row names (architecture.md §4.2 — the providers spec); each protocol's use of `ctx.model`/`req.max_tokens` after resolution (the mapping specs); the output `Sink`, exit-code driver, and signal handling (architecture.md §5, §8). This module is **vendor-blind**: it never `match`es a provider `name` — resolution is a *query over rows* (architecture.md §4.3).

### 1.1 Inherited invariants (from architecture.md — restated so this spec is self-contained)

1. **One schema, one fold, no privileged layer.** Flags, env, file, defaults are four `PartialConfig` values; precedence is the **order of operands** in an `Option::or` chain, which is data, never code (architecture.md §6.1).
2. **The pipe is clean data, not a config layer.** A request field the body *sets* is used as-is; a field it *omits* is filled by `getConfigValue` = **flag > env > config file > app/row default**. Per gen field the effective order is **request > flag > env > config > default**, expressed as **two mechanisms** — the request, then config-fills-the-rest — never one fold the caller must learn (architecture.md §4.4, §13.8).
3. **model→provider routing is a query, not a second table.** The user names a provider explicitly **or** brazen takes the **first row, in priority order, that owns** the model — its `model_aliases` spells it, or one of its `model_prefixes` claims its family. **Priority is declaration order** (§2.2): several rows may own one model, and the earliest wins — a greedy match over the operator's own list, not a silent pick (architecture.md §4.3, §4.3.1).
4. **Alias substitution is `model_aliases.get(model).unwrap_or(model)`** — an unaliased model passes through verbatim; substitution covers spelling, **not** routing (architecture.md §4.3). Routing-by-family is `model_prefixes` (`["claude-"]`, …), so a versioned wire id routes with no `--provider`.
5. **The embedded provider table is parsed through the *same* `resolve` path** — `include_str!("defaults.toml")` → `toml::from_str::<PartialConfig>` — not a bootstrap special case (architecture.md §4.2, §6.1).
6. **Nothing in the library reads `std::env`, opens a file, or calls `now()`.** Env arrives as an injected `EnvSnapshot`; the file arrives already read into a `PartialConfig`; impurity lives only in `main`'s wiring (architecture.md §6.5).
7. **`Secret` never leaks** into logs, `provider_detail`, or `--dump-config` (architecture.md §6.4) — `--dump-config` elides it to the inert `"<redacted>"` sentinel (architecture.md §13.2).

---

## 2. The schema — one `PartialConfig`, four instances

There is exactly **one** config type. Flags, env, file, and embedded defaults are not four schemas with a privileged "base" — they are four values of `PartialConfig`, every field `Option`, every provider entry sparse. This is the single-source-of-truth rule applied to configuration itself: a "flags struct" distinct from a "file struct" would be two homes for one fact and would drift on every new knob.

```rust
#[derive(Default, Deserialize, Serialize)]   // Serialize is for --dump-config (§6) only
#[serde(deny_unknown_fields)]                 // a typo'd scalar key is a Config error, NOT a silent passthrough (§2.3)
pub struct PartialConfig {
    pub provider:    Option<String>,                     // the SELECTOR: force this row, overrides model routing (§7)
    pub model:       Option<String>,
    pub api_key:     Option<Secret>,                     // inline key => stateless, bypasses CredStore (architecture.md §6.5)
    pub output:      Option<OutputMode>,                 // Text | Ndjson | Raw
    pub thinking:    Option<bool>,                        // --thinking: a flag on the text projection, NOT a 4th OutMode (architecture.md §5.3)
    pub max_tokens:  Option<u32>,
    pub temperature: Option<f32>,
    pub top_p:       Option<f32>,
    pub reasoning:   Option<ReasoningEffort>,             // --reasoning / BRAZEN_REASONING / file `reasoning = "high"`: the portable effort knob (architecture.md §3.1, §5.3). A typed gen field folded flag>env>file like the rest; NOT a body_defaults gen scalar — the exact-budget escape hatch stays the row's raw body_defaults object (§4.1)
    pub stream:      Option<bool>,
    pub timeout:     Option<u64>,                        // the silence budget in whole seconds (§4.3); floor in defaults.toml. One value, fanned per phase (connect / response-header / inter-chunk) at the seam — NOT a wall-clock total (architecture.md §5.10.3, §13.15)
    pub system:      Option<Vec<Content>>,               // --system: the leading config/flag/file system prompt, filled into a request that omits it (architecture.md §4.4, Decision 10; §4 line 209)
    #[serde(default)]
    pub providers:   Vec<(String, PartialProvider)>,     // sparse, name + row, in DECLARATION order = the routing priority list (§2.2); merged per-name, per-field (§3.2)
    pub ingress:     Option<PartialIngress>,             // the [ingress] table (§2.5, ingress.md §6): sparse, a top-level sibling of [[provider]]
    #[serde(default, flatten)]
    pub extra:       Map<String, Value>,                 // the long-tail valve, folded like everything else
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialProvider {                             // every Provider field made Option so a file can patch ONE field
    pub base_url:           Option<String>,
    pub protocol:           Option<ProtocolId>,
    pub auth:               Option<AuthId>,
    pub api_header:         Option<HeaderSpec>,
    pub beta_headers:       Option<Vec<(String, String)>>,
    pub model_aliases:      Option<Map<String, String>>,
    pub model_prefixes:     Option<Vec<String>>,         // owned model-id families for routing (§4.3, arch §4.3); routing only, not substitution
    #[serde(default)]
    pub body_defaults:      Map<String, Value>,          // the row's request-body defaults (§4.1); the row's OWN long-tail valve
    pub unsupported_body_keys: Option<Vec<String>>,      // canonical fields the backend REJECTS, stripped before encode (§4.1) — the inverse of body_defaults
    pub models:             Option<ModelsOverride>,      // per-row model-discovery override (§4.4): path/query/list keys + metadata keys over the protocol default
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]                            // a nested valveless struct: a typo'd key is a MalformedFile (§2.3), like `oauth`
pub struct ModelsOverride {                              // `[provider.models]` — the per-row `--list-models` discovery override (§4.4, model-discovery.md §3.2)
    pub path:      Option<String>,                       // GET path over the protocol's models_shape().path
    #[serde(default)]
    pub query:     Vec<(String, String)>,                // `?k=v&…` query pairs, URL-encoded like `authorize_params`; empty default = no query
    pub array_key: Option<String>,                       // the top-level array key over the protocol default ("data"/"models")
    pub id_key:    Option<String>,                       // the per-entry wire-id field over the protocol default ("id"/"name"/"slug")
    pub context_key:      Option<String>,                // per-entry metadata key → Model.context_window; over the protocol default ("" = unserved ⇒ None), model-discovery.md §3.2
    pub max_output_key:   Option<String>,                // per-entry metadata key → Model.max_output_tokens
    pub display_name_key: Option<String>,                // per-entry metadata key → Model.display_name
}
```

**`body_defaults` is the row's sanctioned long-tail valve — the generalization of the former `default_max_tokens`.** A `[[provider]]` row may pin request-body fields the backend always needs (`store`, a `max_tokens` cap) without the caller hand-crafting canonical JSON every run. It is **not** a wire-body map: its keys name *canonical request fields* — gen params fold into the typed request, the rest into the request's `extra` valve — so the canonical→wire mapping stays owned by `encode` (§4.1). `default_max_tokens` was already exactly this (a per-row default for the canonical `max_tokens`); folding it into `body_defaults` is the single-source-of-truth move — one mechanism for "what the row defaults on the body," not a scalar field beside a map (§4.1). Like the top-level `extra` valve, `body_defaults` is the open map *inside* an otherwise typo-checked row (§2.3): the row keys around it stay `deny_unknown_fields`, but anything may go *in* it (brazen does not model `store`, and need not, to let a row pin it).

**`providers` is an ordered `Vec<(String, PartialProvider)>` — the row table *is* the priority list.** The embedded `defaults.toml` (architecture.md §4.2) authors rows as a TOML array-of-tables; deserialization pairs each table's `name` with the rest as a `PartialProvider` and **keeps the array's order**, because that order is the routing priority (architecture.md §4.3.1). The merge stays **per-name, per-field** — a user file can override exactly Anthropic's `body_defaults` without redeclaring the whole row (architecture.md §6.1) — it just walks the list instead of indexing a map. Determinism, which a `BTreeMap` used to buy, now comes from declaration order: the same input dumps the same bytes.

**This is why order is not a field.** Priority is a fact about rows, so it lives *in* the row list; it is not a `provider_order: Vec<String>` beside a keyed map. That pairing was the obvious first design and it is the wrong one: it stores the name set **twice**, forces the invariant "the order vec is exactly the map's keys, permuted," and makes two functions (a map merge and an order merge) agree on one union forever — a policed invariant where an ordered list makes the same invariant **unrepresentable**. Nothing can drift from itself.

**The whole ruling falls out of the structure, which is how you know it is the right one.** Greedy-first routing is `providers.iter().find(|(_, row)| row_owns(row, model))` — the *first* owner, because the list is already in priority order. The zero-config default is `providers.first()`. Neither is an algorithm layered over the data; each is the data read forward. So the old `default_provider` field is **gone** (it stored the head of a list that now exists), the old `AmbiguousModel` scan-for-a-second-owner is **gone** (`find` stops), and no order-merge function is ever written.

The costs are real and small: a name lookup (`--provider`, §7 step 1) and the merge's per-name join become linear scans rather than `BTreeMap` probes, over a table of **≤10 rows** that resolution walks in full anyway. Duplicate-`name` detection needs the same one seen-set it always did. (The other candidate, an `IndexMap`, buys the same dissolve for a **fifth** runtime dependency — the crate ships four: `serde`, `serde_json`, `toml`, `sha2`, with `base64` riding in transitively on `ureq`. A dependency is not worth a linear scan of ten elements.)

### 2.1 Why every field is `Option`

`None` is the identity element of `Option::or`: `x.or(None) == x` and `None.or(y) == y`. A field a layer does not mention is `None` and contributes nothing to the fold — *absence is not a value*. This is what lets the four layers be one type with no "is this set?" flag beside each field (a second home for "absence"). The empty config is `PartialConfig::default()` — all `None`, an empty map — and it is the literal identity of the whole fold (§3.3), which is why a missing file is not a special case.

### 2.2 The `[[provider]]` array ⇄ row-list seam

`defaults.toml` and user files write providers as:

```toml
[[provider]]
name = "anthropic"
base_url = "https://api.anthropic.com"
# … (architecture.md §4.2)
```

A custom `Deserialize` for `PartialConfig` reads the `[[provider]]` array **in order**, lifting each table's `name` out and storing the remainder as a `PartialProvider`. A **duplicate `name` within one file** is a `Config` error (78) — surfaced against a seen-set, never last-wins, because two rows for one name is a contradiction inside a single layer (cross-layer override is the fold's job, §3.2).

`Serialize` (for `--dump-config`, §6) runs the inverse: rows emit in list order with `name` re-injected, so a dumped config re-parses to the **same priority** — the encoding round-trips the order because the order never left the list. **Priority has exactly one wire form: the `[[provider]]` array's own line order**, which is what the operator already reads and writes. There is deliberately **no** `provider_order`-style config key — a second wire form for one fact, and a way to author an order naming rows that do not exist. (§6 decision 4 owns the emission rule and what it supersedes.)

**Distinct row names are distinct credential files — the multi-account / rotation seam.** Because the cred-store key is the row name (the file is `<name>.json`; `store_key = provider.name`, auth.md §1.3, §5.2), N rows with distinct names give N **independent** credential files. So `anthropic` and `anthropic-work` are separate accounts, keys, and rotation targets for one vendor — a **supported interface guarantee** stated in full at auth.md §5.2, not an accident of this row-list encoding. Give only **one** of them `model_prefixes`/`model_aliases` and the account-scoped row is reached the honest way, with an explicit `--provider <name>` (architecture.md §4.3). If both claim the family, this is not an error — greedy-first silently routes to whichever is declared earlier (§7 step 3) — which is exactly why an account-scoped row should claim nothing: you want `--provider` to be the only way in, not a race with your own line order.

### 2.3 `deny_unknown_fields` — a typo is a `Config` error, not a passthrough

A misspelled **scalar** config key (`temperatue`, `maxtokens`) is rejected at `toml::from_str` → exit 78. This is the deliberate *opposite* of the canonical request's `extra` long-tail valve (architecture.md §3.1, where a misspelled request field silently becomes a passthrough knob): config is operator-authored and small, so a typo there is a bug to surface, not a knob to forward. The single sanctioned top-level long-tail in config is the top-level `extra` map (passthrough provider knobs that `fill_absent` seeds into `req.extra`, §4.1); it is `#[serde(flatten)]`, so genuinely-unmodeled top-level keys still land there rather than erroring. The line is drawn once: **named fields are typo-checked; the one `extra` map is the open valve.**

**Where the deny actually bites (implementation note).** A top-level `#[serde(flatten)] extra` and `deny_unknown_fields` are mutually exclusive in serde, and the flatten valve cannot tell a typo from a deliberate knob — so an unmodeled **top-level** key lands in `extra`, it does not error. The typo-check therefore lives where there is **no** valve: each `[[provider]]` **row** is `deny_unknown_fields` (a misspelled `bas_url` → `MalformedFile`/78), and a duplicate provider `name` within one file is rejected (§2.2). The row's *own* sanctioned valve is its `body_defaults` map (§2, §4.1): the row keys around it are typo-checked, but its contents are open (a `store` brazen does not model still lands there) — the row-scoped mirror of the top-level `extra` map. A mistyped top-level scalar is forwarded as a passthrough knob, exactly as a mistyped request field is (architecture.md §3.1) — the asymmetry the first paragraph asserts holds for **row** fields, not top-level ones. This is the coherent reading the resolver implements; the `MalformedFile` test surface (§8) is the row-typo + duplicate-name pair. The same deny extends to the row's nested valveless structs — `OAuthConfig`, `RedirectSpec` (auth §7.1, §10.1), and `ModelsOverride` (the `[provider.models]` block, §4.4) — so a TOP-LEVEL row key (e.g. `unsupported_body_keys`) misplaced under `[provider.oauth]`, a typo'd `redirect` key, or a typo'd `[provider.models]` key (`pth` for `path`), is a `MalformedFile`, not a silent drop that leaves the operator still 4xx-ing (bl-9649).

### 2.4 Forward evolution — the schema is additive-only, and needs no version field

**The invariant.** A config file valid under brazen *today* stays valid, **with the same meaning**, under **every future brazen**. This is *forward* compatibility only: brazen does **not** promise the converse — an **older** brazen need not read a file that uses a **newer** brazen's keys (no downgrade, no migration). Operators author config forward in time; the schema's one obligation is that a working file never has to be rewritten by a brazen upgrade.

**Why no version field.** Config is the one on-disk format that is **operator-authored and not self-healing**, so it is the only one that needs this discipline written down. The peers re-derive themselves: the per-provider model cache is **regenerable** (`bz --list-models` is its wholesale writer and a successful generation appends the one model it used — model-discovery.md §5.4; deleting it only forces a rebuild from the next list or from ordinary use), and the credential file is **self-describing and absolute** (the `Cred` variant is the discriminant, `expires_at` is an absolute instant — auth.md §5.1), so a stale one of those is re-derived or re-authed, never mis-read. A config file cannot be re-derived; it must keep working as written. A version marker + migration machinery would buy the *backward* compatibility brazen has decided it does **not** want — **rejected as cost without a customer.** The wire `Event` vocabulary *does* carry a handshake (`MessageStart.v`, architecture.md §3.2) because a machine consumer must *detect* a break it cannot see coming; an operator's TOML is read only by the brazen the operator runs, evolves forward-only, and so is kept compatible by **discipline, not a number.**

**The discipline (enforced by convention — the only kind that fits a version-less schema):**

1. **Never rename, remove, or repurpose an existing key, or change its meaning.** A rename is a remove + an add (it strands the old spelling); a repurpose silently changes what an already-valid file *means* — the subtlest break, and the one a version field could not catch anyway. New *meaning* takes a new *key*.
2. **Evolution is additive-only: a new key ships `Option`** (a map/collection `#[serde(default)]`), so a file that omits it still parses and contributes `None` — the identity of the fold (§2.1). This is already the shape of every field in §2; the invariant promotes "incidentally optional" to "mandatorily optional." An additive key is invisible to every older file, which is exactly forward compatibility.
3. **Removal is doubly forbidden, and `deny_unknown_fields` (§2.3) is why.** Drop a **provider-row** key and an old file that still names it becomes a `MalformedFile`/78 (the row deny bites) — a hard break of a once-valid file. Drop a **top-level** key and an old file's value silently lands in the `extra` valve, inert — meaning changed without a sound. Either way the file the operator wrote stops doing what it did, so the deny that surfaces typos also makes key-removal **load-bearing-forbidden**, not merely impolite.

**`--dump-config` is a forward-evolution artifact (corollary, §6).** A dumped file carries **no version marker** and **omits the defaults layer**, and both are sound *only* under this invariant. Omitting defaults is deliberate (§6): a key the operator never set stays absent, so a future brazen's better default reaches that file live. A key the operator *did* set is captured as an explicit value and is **frozen at its dump-time value forever** for that file — correctly, it is the operator's choice — and it can stay version-less because additive-only guarantees the file never becomes invalid and never changes meaning out from under it. So a dump is a permanent, re-readable snapshot of operator intent over a live-defaults floor; to adopt a newer floor for the keys they pinned, the operator re-dumps. No marker is missing — under additive-only there is no version a marker could protect against.

### 2.5 The `[ingress]` table — schema, fold & serve-time resolution

The masquerade listener's one config surface ([ingress.md](ingress.md) §6). This spec owns only its **plumbing** — encoding, fold, dump, and where validation fires; every field's *meaning* (the no-sniffing dialect selector, the adapt-or-reject ladder, the security posture) is ingress.md's (§2, §4, §6, §7) and is cited, not restated.

```rust
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]                 // like a [[provider]] row (§2.3): a typo'd key is MalformedFile/78
pub struct PartialIngress {                   // a TOP-LEVEL table, sibling of [[provider]]
    pub dialect: Option<String>,              // the explicit dialect selector (ingress.md §2, §6); REQUIRED only to serve — checked at resolution, never at parse
    pub listen:  Option<String>,              // bind address as written; resolution defaults "127.0.0.1:4891" (ingress.md §6)
    pub token:   Option<Secret>,              // optional bearer (ingress.md §7); a Secret like api_key — redacted in --dump-config (§6)
    pub lossy:   Option<LossyMode>,           // adapt | reject (ingress.md §4); resolution defaults adapt
    #[serde(default)]
    pub lossy_overrides: BTreeMap<String, LossyMode>,  // per-adaptation-NAME overrides (ingress.md §4)
}
```

- **It folds like everything else (§3).** Per-field `Option::or` one level down; `lossy_overrides` merges **per-key** like `body_defaults` (§3.2); a missing table is the fold **identity** (§3.3 applied to a table). No env vars carry it — the table is file-authored (an env layer simply contributes `None`).
- **The defaults are resolution's, not `defaults.toml`'s.** An `[ingress]` row in the embedded defaults would put a listener table in every config, breaking ingress.md §6's severability ("delete the table and every ingress behavior is gone") — so `listen`'s `127.0.0.1:4891` and `lossy`'s `adapt` are applied at resolve, and `--serve` with **no table at all** is a `Config` error (78) naming it.
- **`--dump-config` round-trips it (§6):** the table rides the dump sparse (present fields only), `token` elided to the `"<redacted>"` sentinel exactly like `api_key`.
- **Resolution is `resolve_ingress` — a sibling of `into_resolved`, run only when a serve/ingress path asks.** An ordinary one-shot run never validates the table (the requiredness is *to-serve*, not *to-parse*). It lifts the merged table into the resolved `IngressConfig { dialect: String, listen: SocketAddr, token, lossy, lossy_overrides }` the listener consumes (plus the per-case query `lossy_for(name)` = the override, else the global `lossy` — policy has one home). Every failure is `ConfigError::Ingress` → 78 (§7): a missing table; a missing `dialect`; an unknown `lossy_overrides` **adaptation name** checked against the `KNOWN_ADAPTATIONS` const (`thinking_replay`, `document_url_drop` — each mapping spec that introduces an adaptation adds its name), because a typo'd override must never silently leave the default in force (ingress.md §4); a `listen` that is not a numeric `ip:port` (a hostname cannot be proven loopback without IO, so it is refused at resolve, not at bind); and the refuse-to-start rule — a **non-loopback `listen` without `token`** (ingress.md §7). The adaptation-name check stands alone as `PartialIngress::validate_lossy_overrides` — `resolve_ingress` calls it, and the `--in` filter (which needs no serve-complete table and so never resolves, ingress.md §11) calls it directly on any present table, so the never-silently-inert rule holds on both front doors.
- **No model routing lives here** (ingress.md §6): an inbound model resolves through the existing alias/prefix ladder (§7) — a second routing surface would be a second home for the model→row fact.

---

## 3. Resolution — the fold under `Option::or`

Resolution is not a single exported function: it is a **call-site composition** that
`run` and `bz --login` each perform, ending in the one public seam `PartialConfig::into_resolved`.
The composition is:

```rust
let env = partial_from_env(env);                          // pure projection of the injected snapshot (§3.4)
let cfg = flags.or(env).or(file).or(defaults);            // PRECEDENCE = OPERAND ORDER. flag > env > file > default.
cfg.into_resolved(req_model)                              // request.model wins for routing; else getConfigValue("model")
//  -> Result<ResolvedConfig, ConfigError>
```

where the four operands are `flags`, the injected `&EnvSnapshot` projection (the lib never
reads `std::env` — architecture.md §6.5), the already-read `file: PartialConfig` (a missing
file is `PartialConfig::default()`), and `defaults` (`toml::from_str(include_str!("defaults.toml"))`
— the same parse path, no bootstrap). `req_model` is the request's non-empty `model`, consulted
only for routing. It is **not** wrapped in a `resolve(flags, env, file, defaults, req)` helper:
the binary composes it inline because output mode must be resolved from the fold *before* the
body is read, while routing needs `req.model` *after* — one wrapper could not serve both phases,
so the fold lives at the call site and `into_resolved` is its single public continuation.

The whole of resolution is still **one expression**: a fold of four operands under
`PartialConfig::or`, then `into_resolved`. There is no `if layer == flags` anywhere — the
precedence policy is the *textual order of the operands*, which is data the reader can see, not
control flow buried in a merge function. Reordering precedence is editing operand order, not
editing code logic (the severability test, architecture.md guidance).

### 3.1 `PartialConfig::or` — the fold step

```rust
impl PartialConfig {
    /// self has higher precedence than other. self.field.or(other.field) per scalar;
    /// per-name recursive merge for providers; or for the extra map.
    fn or(self, other: PartialConfig) -> PartialConfig {
        PartialConfig {
            provider:    self.provider.or(other.provider),
            model:       self.model.or(other.model),
            api_key:     self.api_key.or(other.api_key),
            output:      self.output.or(other.output),
            thinking:    self.thinking.or(other.thinking),
            max_tokens:  self.max_tokens.or(other.max_tokens),
            temperature: self.temperature.or(other.temperature),
            top_p:       self.top_p.or(other.top_p),
            stream:      self.stream.or(other.stream),
            providers:   merge_providers(self.providers, other.providers),  // §3.2: hi's rows first, per-name field merge
            extra:       or_map(self.extra, other.extra),                   // higher-precedence key wins
        }
    }
}
```

Every scalar is a literal `Option::or`: the left (higher-precedence) `Some` wins; `None` defers. `or` is **associative**, so `a.or(b).or(c).or(d)` needs no parenthesization and the four-layer fold has one unambiguous result regardless of grouping — the law that lets "add a layer" mean "add an operand."

### 3.2 The provider table merges per-name, per-field

```rust
fn merge_providers(hi: Vec<(String, PartialProvider)>, lo: Vec<(String, PartialProvider)>)
    -> Vec<(String, PartialProvider)>
{
    // hi's rows in hi's order — each field-merged (`PartialProvider::or`) with lo's row of
    // the same name if it has one — then lo's rows whose names hi never mentioned, in lo's
    // order. One walk: content and position resolve together, because they are one list.
}
```

A name present in only one layer passes through; a name in both is merged **field-by-field** under the same `or`. Position follows the same "higher precedence first" law as every scalar: a row both layers name takes the **higher** layer's position, and rows only the lower layer names keep their relative order behind it.

So a user file with:

```toml
[[provider]]
name = "anthropic"
body_defaults = { max_tokens = 8192 }
```

overrides exactly that one **field** of the embedded Anthropic row — `base_url`, `protocol`, `auth`, `api_header`, `beta_headers` all defer to the lower-precedence embedded layer (architecture.md §6.1: "the file can override one header on Anthropic without redeclaring the row"). The **same `or` mechanism** drives scalars and the provider table — there is no second merge algorithm. `body_defaults` is itself a map, so within `PartialProvider::or` it merges **per-key** under the same `or_map` the top-level `extra` uses (higher-precedence key wins) — a user file's `body_defaults = { store = false }` adds a key without dropping the embedded row's `max_tokens`.

It overrides that one field — and it also **moves the row**, which is the one sharp edge of per-field merge meeting priority. `providers.first()` after the merge is hi's first row when hi has any, else lo's, so this subsumes the old `default_provider: self.default_provider.or(other.default_provider)` **exactly** — the identical result, now read off the head of a list instead of stored as a fact of its own. And the law survives: dedup-keep-first over concatenation is grouping-independent, so the merge is **associative** and §3.1's "add a layer = add an operand" still holds.

**But a redeclared row takes YOUR position while keeping the defaults' claims**, and that deserves stating outright. A user row `[[provider]] name = "openai"` carrying only a `body_defaults` tweak still resolves with the **embedded** row's `model_prefixes = ["gpt-", …]` — fields fall through — while taking the **user file's** position. So *touching one field of a defaults row is also a priority claim for everything that row owns*: put such a tweak above a `gpt-4o` alias row and it silently re-arms `gpt-` ahead of the alias, and the masquerade of ingress.md §6 stops working with no diagnostic. Declare the row that owns a model **before** any row you redeclare, or reach the redeclared row by explicit `--provider`.

### 3.3 Missing file = identity element

A missing or unreadable-as-absent config file is **not an error**: `main` hands `resolve` a `PartialConfig::default()` for the file layer, and since `default()` is all-`None`/empty it is the **identity of `or`** (`flags.or(env).or(default()).or(defaults) == flags.or(env).or(defaults)`). The "no config file" case is therefore *the general path with an empty operand*, not a branch — a missing-file class of edge cases dissolved by one invariant (architecture.md guidance: "a special case is usually a missing reframe"). (A file that *exists but is malformed TOML* is a different fact — a `Config` error, §7; absence and corruption are not conflated.)

### 3.4 `partial_from_env` — pure projection of the injected snapshot

```rust
pub struct EnvSnapshot(pub BTreeMap<String, String>);    // injected; main fills it from std::env once
fn partial_from_env(env: &EnvSnapshot) -> PartialConfig  // PURE: BTreeMap -> PartialConfig, table-driven
```

The library **never** reads `std::env` (architecture.md §6.5). `main` snapshots the process environment into an `EnvSnapshot` once and injects it; `partial_from_env` is a pure, table-tested mapping from variable names to fields:

| Env var | `PartialConfig` field |
|---|---|
| `BRAZEN_PROVIDER` | `provider` |
| `BRAZEN_MODEL` | `model` |
| `BRAZEN_BASE_URL` | `base_url` — the host override lifted onto the resolved row (§4.5) |
| `BRAZEN_API_KEY` | `api_key` (`Secret`) — the brazen-native, **provider-agnostic** key signal |
| `BRAZEN_MAX_TOKENS` | `max_tokens` (parsed; unparseable → §7 `Config`) |
| `BRAZEN_TEMPERATURE` | `temperature` |
| `BRAZEN_TOP_P` | `top_p` (parsed `f32`; unparseable → §7 `Config`) |
| `BRAZEN_REASONING` | `reasoning` (parsed `low\|medium\|high` via `FromStr`; unparseable → §7 `Config`) |
| `BRAZEN_OUTPUT` | `output` |
| `BRAZEN_THINKING` | `thinking` (parsed bool; `--thinking` on the text projection, architecture.md §5.3) |
| `BRAZEN_STREAM` | `stream` |
| `BRAZEN_TIMEOUT` | `timeout` — the silence budget (parsed seconds; unparseable → §7 `Config`) |

`$BRAZEN_CONFIG` is **not** in this table — it selects *which file* to read (§5), a pre-`resolve` concern, not a field of the resolved config. Because the projection is pure over an injected map, the entire env-precedence behavior is a table test with no process-environment dependency (§8).

A **vendor-conventional key alias** (`ANTHROPIC_API_KEY`) is deliberately **not** in this table either. Routed into the universal `api_key` it became `inline_key` — transmitted to *any* resolved provider and shadowing a stored/`bz --login`'d cred (the bl-5a43 cross-vendor leak + store-shadow bug). Instead it is **row-scoped data**: the anthropic row names it as a store-miss **ambient source** (`ambient = { format = "api_key_env", path = "ANTHROPIC_API_KEY" }`, auth §5.5), so it reaches only the row that names it, and only when neither `--api-key`/`BRAZEN_API_KEY` nor a stored cred is present. No `provider == "anthropic"` branch — deleting the row's `ambient` line deletes the alias (severability).

### 3.5 Embedded defaults through the same parse path

`defaults` is **not** a hand-built struct and **not** a bootstrap layer:

```rust
let defaults: PartialConfig = toml::from_str(include_str!("../../data/defaults.toml"))
    .expect("embedded defaults.toml is a compile-time-committed constant, validated by a unit test");
```

It travels the *identical* `toml::from_str::<PartialConfig>` path as a user file (architecture.md §4.2, §6.1). "Lowest precedence" is purely "**last operand** in the fold" — there is no privileged base into which the others are merged. The one permitted `expect` is on our own committed constant, not external input (consistent with the `unwrap_used`/`expect_used` deny on the data path, architecture.md §8); a unit test parses `defaults.toml` so a malformed edit fails the build, never a user run.

---

## 4. The request is not a layer — `fill_absent` & `getConfigValue`

The fold above produces config. The **request is clean data and is never an operand of that fold** (architecture.md §4.4, §13.8). Two mechanisms, never one merged precedence the caller must learn:

```rust
/// getConfigValue(key) = the resolved value = flag > env > config file > app/row default.
/// (It is just a field read on the merged-and-resolved config — the fold of §3 already ran.)

/// fill_absent: for each GEN field the request omits, fill from config. Request-present fields untouched.
fn fill_absent(req: &mut CanonicalRequest, cfg: &ResolvedConfig) {
    req.model       = take_or(req.model.take(),  cfg.model.clone());        // (model also drives routing, §4.3 arch)
    req.max_tokens  = req.max_tokens.or(cfg.max_tokens);                    // §4.1 — cfg.max_tokens already folds the row body_default beneath flag/env/file
    req.temperature = req.temperature.or(cfg.temperature);
    req.top_p       = req.top_p.or(cfg.top_p);
    req.reasoning   = req.reasoning.or(cfg.reasoning);                      // the portable effort knob (architecture.md §5.3); a typed gen field like the rest — cfg.reasoning is the flag>env>file fold (NOT a body_defaults rung, §4.1)
    req.stream      = req.stream.or(cfg.stream);                            // §4.2 — a gen field like the rest: request-set wins, else config, else absent
    req.system      = req.system.take().or_else(|| cfg.system.clone());
    // The request's OWN extra wins; config passthrough (top-level `extra` + the row's
    // non-gen `body_defaults`, merged into cfg.extra at resolve, §4.1) fills only keys
    // the request did not set — the row-default layer of the request's long-tail valve.
    for (k, v) in &cfg.extra { req.extra.entry(k.clone()).or_insert_with(|| v.clone()); }
    // messages, tools, tool_choice are the request's ALONE — never config-filled (architecture.md §4.4)
}
```

So **per gen field** the effective order is **request > flag > env > config > row default**, achieved by composition: the request is the outer `or`, and `getConfigValue` (the already-resolved `flag > env > file > row-default`) is the inner. The caller never reasons about "does my body beat a flag?" — a body value beats everything for that field by being present, and config fills only what the body leaves unset. `encode` then reads every gen param straight off `req`, the wire `model` off `ctx`, and folds `req.extra` last (typed fields win — the mapping specs); resolution has already done all merging.

### 4.1 `body_defaults` — the per-row request-body default layer

A `[[provider]]` row's `body_defaults` is the **lowest-precedence operand for the request body** — what the row pins when the request, flags, env, and file all leave a field unset. It generalizes the former `default_max_tokens` (a single scalar that defaulted `max_tokens`) into **one map** so a row can also pin `store`, `stream`, etc. (the OpenAI ChatGPT-SSO Codex backend mandates `store:false`, auth §10.5/§10.7; its `stream:true` mandate is satisfied by brazen's stream-native global default — §4.2 — though a row may still pin `body_defaults = { stream = true }` to force it explicitly, and the inverse `stream = false` is the severable home for a non-streamed provider). It is resolved in two moves, mirroring the canonical request's own split into typed gen fields + the `extra` valve:

```rust
// into_resolved, after routing to the single row (its body_defaults map in hand):
let mut bd = row.body_defaults;                       // consumed here; not carried on the resolved Provider
let max_tokens  = self.max_tokens.or(take_u32(&mut bd, "max_tokens")?);   // gen scalars fold into the
let temperature = self.temperature.or(take_f32(&mut bd, "temperature")?); //   resolved typed fields,
let top_p       = self.top_p.or(take_f32(&mut bd, "top_p")?);             //   BELOW flag/env/file (§3)
let stream      = self.stream.or(take_bool(&mut bd, "stream")?);          //   take_* removes the key
let extra       = or_map(bd, self.extra);            // whatever is LEFT (store, …) is row passthrough,
                                                     //   merged OVER the top-level `extra` (row is more specific)
```

- **The gen scalars (`max_tokens`/`temperature`/`top_p`/`stream`) fold into the resolved typed fields**, so `ResolvedConfig.max_tokens` (etc.) already carries the row default beneath flag/env/file, and `fill_absent` needs only a plain `.or(cfg.max_tokens)`. There is no `effective_max_tokens()` query any more — the fold happens once at resolve, one home. Why fold here and not in `encode`? Because **`encode` cannot**: every encoder writes `stream` unconditionally (`req.stream.unwrap_or(false)`, §4.2) and `anthropic_messages` *requires* `max_tokens` (a `None` at encode is a `Config` error, the mapping spec) — a wire-body fold below the typed fields could never set `stream` and could never satisfy the required-param check. These are canonical fields; the row defaults them through the canonical request, and `encode` keeps sole ownership of the per-dialect rename (`max_tokens`→`max_output_tokens`, etc.). A row therefore writes the **canonical** key (`max_tokens`), never a wire spelling.
- **Every other key is request passthrough**: it merges into `cfg.extra` (the row's keys winning over the top-level `extra`, being more specific), and `fill_absent` seeds it into `req.extra` beneath the request's own keys. It reaches the wire through the **same `req.extra` fold every encoder already runs** (`body.entry(k).or_insert(v)`, typed fields win) — no encoder change, and the live seam, not the formerly-dead `ProviderCtx.extra` (§9).

**`reasoning` is a typed gen field but NOT a `body_defaults` gen scalar — a deliberate, single-source choice.** It has the usual flag/env/file rungs (`--reasoning`, `BRAZEN_REASONING`, top-level `reasoning = "high"`) folded into `cfg.reasoning` by the standard `PartialConfig::or`, and `fill_absent` fills it like any gen field. But `into_resolved` does **not** `take_reasoning(&mut bd, …)` — so a `body_defaults.reasoning` key is NOT absorbed into the typed field; it stays in `bd` as ordinary passthrough. This is exactly what makes `body_defaults` the *exact-shape escape hatch* (architecture.md §5.3): brazen's portable enum has three rungs (`low|medium|high`), and the raw provider-shaped reasoning object a row may need (`thinking = { type = "enabled", budget_tokens = 4096 }`, `reasoning = { effort = "high" }`, `thinkingConfig = { thinkingBudget = 2048 }`) is **not** something the typed `ReasoningEffort` can carry. Folding such an object into a string-enum field would be a type error at resolve; leaving it as `extra` passthrough lets it ride to the wire verbatim, where the typed knob (if `--reasoning` is set) wins on a same-named key through the encoder's one `extra` fold. So the portable enum and the raw escape hatch coexist without a second home for the same fact: the enum is the common case, `body_defaults` the long-tail.

**Full precedence for any body field: request (typed field / `extra` key) > flag > env > config file > row `body_defaults` > the encoder's protocol baseline.** A request value wins by being present; among config layers, a flag beats the row default; the row default beats the encoder's bare default (`stream:false`). A provider whose row pins nothing leaves the field `None`/absent, and `encode` omits it — brazen **never burdens the caller with a value the model needs** (the row supplies it) nor **invents one the model doesn't** (an unpinned, unrequested param stays absent). This generalizes architecture.md §13.1.

**The boundary (not a junk-drawer).** `body_defaults` defaults exactly the surface a *request* may fill: the gen params and the `extra` valve. It does **not** reach the fields the canonical model owns and the encoder derives — `model` (routing, §4.3), `messages`, `tools`, `tool_choice`. Those are not request-omitted-then-filled fields (`fill_absent` never fills them either, §4); a `body_defaults.model` key is treated as opaque passthrough into `req.extra`, where the encoder's typed `model` (always written) wins and the stray key is inert. So a row can never desync the canonical→wire mapping: it pins *inputs* to that mapping, never the mapping's outputs.

**Validation.** A gen-scalar key with the wrong JSON type or an out-of-range value (`max_tokens = "lots"`, `max_tokens = 0`, `temperature = true`) is a `BadValue` → exit 78, surfaced at resolve by `take_u32`/`take_f32`/`take_bool` — the same discipline as the top-level scalars (§7). Passthrough keys are unvalidated by design (the open valve, §2.3): a misspelled passthrough surfaces as an upstream 4xx, exactly as a misspelled request `extra` key does (architecture.md §3.1).

### 4.1.1 `unsupported_body_keys` — the inverse: per-row request-body *strip*

Some backends **reject** a standard param the canonical model would otherwise forward. The OpenAI ChatGPT-SSO Codex backend (`…/codex/responses`) 400s with `{"detail":"Unsupported parameter: …"}` on `temperature`, `top_p`, **and** `max_output_tokens` (auth §10.7, bl-73d8, bl-d54a) — the same field the standard OpenAI Responses API accepts. `body_defaults` *fills* a field a backend needs; `unsupported_body_keys` *strips* a field a backend forbids — the exact inverse, and the **second** datum to join `max_tokens` that lifts bl-73d8's "do not add speculatively" guard (a per-row strip was deferred until a second field joined; `temperature`/`top_p` are it).

```rust
// resolved.rs, the sibling of fill_absent — run AFTER it on the canonical path (serve):
pub fn strip_unsupported(req: &mut CanonicalRequest, cfg: &ResolvedConfig) {
    for key in &cfg.provider.unsupported_body_keys {
        match key.as_str() {
            "max_tokens"  => req.max_tokens  = None,      // typed gen fields cleared by name —
            "temperature" => req.temperature = None,      //   the same enumerate-the-typed-fields
            "top_p"       => req.top_p       = None,      //   shape as take_u32/take_f32 (§4.1)
            "reasoning"   => req.reasoning   = None,      //   the lifted reasoning knob (architecture.md §5.3)
            "output"      => req.output      = None,      //   the lifted structured-output knob (architecture.md §3.1)
            other         => { req.extra.remove(other); } // a non-gen key clears the `extra` valve
        }
    }
}
```

Three properties make this the elegant inverse, not a new mechanism:

- **Canonical keys, not wire keys.** The row names `max_tokens` (never the wire `max_output_tokens`) — identical to `body_defaults`, so the canonical→wire rename stays solely `encode`'s (§4.1). The operator learns no new vocabulary, and the strip is **protocol-agnostic**: it touches the canonical request, so it never branches on which dialect the row speaks. **`reasoning` joins the strippable canonical set** for exactly this reason: a model that doesn't reason (e.g. a Mistral chat model routed through `openai_chat`, which would 400 on a `reasoning_effort` it doesn't accept) lists `unsupported_body_keys = ["reasoning"]` on its row, and `strip_unsupported` clears `req.reasoning` before encode so the openai_chat encoder never emits `reasoning_effort`. Severable: the opt-out is one row datum, no code branch on "does this model reason." **`output` joins the same strippable set** — a backend that rejects structured output (`response_format`/`output_config`) lists `unsupported_body_keys = ["output"]`, and `strip_unsupported` clears `req.output` before encode (architecture.md §3.1, providers §6.1).
- **Run after `fill_absent`, so it beats every source.** A param the backend forbids must be dropped regardless of where it came from — an explicit `--temperature`, a request-body field, a flag, or a row default. Stripping post-fill clears the resolved value unconditionally; this is the **highest**-precedence body operation, the mirror image of `body_defaults` being the lowest.
- **Silent, like other normalizations.** brazen normalizes to what the provider accepts without a warning channel. (`stream` is deliberately **not** a strippable gen arm: it is a tri-state HONORED on the wire — §4.2 — not a forbidden param, so it is folded and routed, never dropped.)

**Severability.** Deleting the row's `unsupported_body_keys` deletes the behavior — no core edit (AGENTS.md). A row that pins nothing yields an empty `Vec`, the loop never runs, every field survives (the general path with empty input — §4's dissolve-special-cases rule). The single strip site is the canonical funnel (`serve`, between `fill_absent` and `encode`), **not** `encode`: putting it in `encode` would either fix only one of the five protocols (a config datum silently inert on the others) or force a strip in all five plus a `ProviderCtx` widening — strictly more mechanism for a strictly narrower fix.

### 4.2 `stream` folds like every other gen field, and the wire value is HONORED

`CanonicalRequest.stream` is `Option<bool>` (architecture.md §3.1), so it folds through `fill_absent`'s `Option::or` shape exactly like `temperature`/`top_p`/`system`: `req.stream = req.stream.or(cfg.stream).or(Some(true))`, the same **request > flag > env > config > row default** chain §4 states for every gen field (`--stream`/`--no-stream`/`BRAZEN_STREAM`/file; a row may pin its rung via `body_defaults`), with brazen's stream-native global default `true` as the lowest operand. (A bare `bool` here would have no "absent" state for that fold — that was a real bug, [bl-ad92] — so the `Option` shape stays.)

**The resolved value is HONORED on the wire, never force-reverted (bl-24c2).** `serve` reads the resolved tri-state and CARRIES the streaming intent (`streamed = req.stream.unwrap_or(true)`) to `drive`, which routes the 2xx body through the matching fold (architecture.md §3.2): `stream:true` wire-streams and SSE/NDJSON-decodes the framed body; `stream:false` sends a single-JSON body that `drive` drains whole and folds via `proto.decode_full` (the explode→replay reconstruction — each protocol replays the body through its OWN `decode`-internal helpers, no second parser). So an explicit `Some(false)` from request, flag, env, file, or a `body_defaults = { stream = false }` pin is **honored, not overridden** — and a flag is never silently ignored (the user-decision invariant: honor or error, never revert). `body_defaults = { stream = false }` is therefore **meaningful**, the severable home for a provider that works better non-streamed (policy in the row, not core); a row pinning `stream = true` (the Codex backend's mandate, auth §10.5) is the explicit opposite. The escape hatch for exact non-stream wire bytes — bypassing `encode` entirely — is `--raw`. Pure `encode` keeps `unwrap_or(false)` — at the protocol layer, absent means "don't ask"; the stream-native global default is `fill_absent`'s, so a direct `encode` caller is unaffected.

### 4.3 Transport timeout — the silence budget, config-sourced, applied per request

`timeout` is one `Option<u64>` scalar (whole seconds) that folds like any other config value (§3): the **silence budget** — abort when the upstream makes no progress (sends no bytes) for that many seconds (architecture.md §5.10.3, §13.15). It is **not** a gen param — it never touches the request body — so it rides neither `encode` nor `fill_absent`. Instead `ResolvedConfig::timeouts()` **fans** the one value onto a `Timeouts { connect, response, idle }` record (all three = the resolved `timeout`), and `run` stamps that onto the `WireRequest` just before `Transport::send` (and the silent OAuth refresh copies it onto its own token POST, so that sub-request shares the bounds — auth.md §6). The `WireRequest` is the one thing crossing the transport seam, so config-sourced policy reaches the impure `HttpTransport` (the `bz` crate) without widening the `send` signature. The fan-out lives in the pure query (not the coverage-excluded shim), so a test asserts at the `WireRequest` seam that one `--timeout` reaches all three internal budgets.

The number's single home is `data/defaults.toml` (`timeout = 120`) — the lowest-precedence operand of the fold (§3.5), so the bin (`HttpTransport`) carries **no** magic constant and removing the line from `defaults.toml` *unbounds* the timeout rather than editing code (the severability rule, AGENTS.md). The three internal budgets are ureq's phase vocabulary, fed from the one policy value: `connect` caps connection establishment and `response` caps awaiting the response **headers** (both map straight onto ureq's agent config, applied per request), while `idle` is the **inter-chunk** bound on the streaming body, reset on every chunk — so a provider that sends headers then stalls mid-stream is abandoned **without** capping total stream length, and a long-but-live generation is never truncated. (ureq's `timeout_recv_body` is a *total* body cap, which would be wrong here; the bin enforces `idle` off-thread instead — architecture.md §4.1, §10.) A **wall-clock total** timeout is deliberately absent (a footgun that would truncate a long live generation — architecture.md §13.15); ureq's phase-named diagnostics ("timeout: connect" / "timeout: receive response") and the idle "stream stalled" message survive, so one knob still yields a phase-diagnosable error, exit 69 in every case.

**Collapsed from three (architecture.md §13.15).** Pre-0.1.0 this was `timeout_connect`/`timeout_response`/`timeout_idle` (30/120/300); the owner ruled the three are one fact ("if it's not sending, it's not sending"). The removal is breaking against released 0.0.1/0.0.2 — see CHANGELOG `[Unreleased]`.

### 4.4 `[provider.models]` — the per-row model-discovery override

A row's optional `[provider.models]` block (`ModelsOverride`, §2) overrides the protocol's default model-discovery shape for the `bz --list-models` GET. It is **not** a request-body datum (it never touches the generation path, `fill_absent`, or `encode`); it shapes only the discovery GET, so its full semantics live in **model-discovery.md §3.2** and only the schema lives here. Every key is optional and defaults to the protocol's `models_shape()` (model-discovery.md §3.1):

| key | type | default when omitted |
|---|---|---|
| `path` | `String` | the protocol's `models_shape().path` (e.g. `/models`, `/v1/models`) |
| `query` | `Vec<[String, String]>` | none — no `?` is appended (the empty case is the general path, not a branch) |
| `array_key` | `String` | the protocol default (`data` / `models`) |
| `id_key` | `String` | the protocol default (`id` / `name`) |

- **It resolves verbatim onto the `Provider`** (`complete` copies `row.models` through unchanged — there is nothing to fold into a typed scalar, unlike `body_defaults`); the verb (`run/models.rs`) reads it and overlays the protocol default per key via ONE pure helper. `strip` (Google's leading `models/`) is **protocol-only** — not a key here — because it makes the decoded id usable in encode's path, a fact the operator cannot sensibly change (model-discovery.md §3).
- **Whole-block `Option::or` across layers**, like `beta_headers`/`unsupported_body_keys` (§3.2): a higher-precedence layer replaces the block rather than merging keys. No embedded `defaults.toml` row carries one (every shipped row uses its protocol default), so the block is purely user-authored — the severable home for a backend (the ChatGPT-SSO Codex backend) whose discovery endpoint diverges from its protocol's standard shape. `query` is URL-encoded by the **same `encode_pairs` codec** the OAuth authorize URL uses (auth §7.4), reused not reinvented.
- **`deny_unknown_fields` (§2.3):** a typo'd key inside the block (`pth` for `path`) is a `MalformedFile`/78, like a typo in `[provider.oauth]`.

### 4.5 `base_url` — the top-level host override (row-field lift)

`base_url` is the **one top-level scalar that overrides a resolved *row* field**: it replaces the routed provider row's `base_url` with a caller-supplied host, so an embedding harness can point a run at a **local proxy, mock server, vLLM instance, or tenant gateway** without generating a temp config file. It is `--base-url <url>` / `BRAZEN_BASE_URL` / a top-level `base_url = "…"` file key — a *scalar*, folded flag > env > file exactly like `--model` (§3), then laid over the routed row at resolve:

```rust
// into_resolved, immediately AFTER routing to the single row, BEFORE `complete` lifts it:
let (name, mut row) = self.route(routing_model)?;
row.base_url = self.base_url.or(row.base_url);   // top-level scalar (flag>env>file) wins; None defers to the row
```

The full precedence is therefore **flag > env > file-scalar > row `base_url`** — the same four-operand fold every other scalar rides, extended one field down onto the row. Five properties make this a lift of an existing field, not a new mechanism:

- **It overrides the resolved row's `base_url` exactly as `--model` overrides the routing model** (the established precedent — every row scalar the fold already lifts). `self.base_url` has already folded flag>env>file at the `PartialConfig` level; `.or(row.base_url)` places it over the row's own value (itself already folded across provider layers). A `None` scalar defers — the routed row's `base_url` survives untouched (the empty-input general path, §4's dissolve rule, **not** a special case).
- **It does NOT create a row.** Only the host swaps; `protocol`, `auth`, `api_header`, `beta_headers`, `oauth`, `ambient`, and routing/alias substitution all stay the resolved row's. This is the **common case** — *same provider, different endpoint* (Anthropic's dialect and auth, but pointed at `http://localhost:8080`). The override lands **before `complete`** (§7), so the lifted row is still validated whole: a scalar pointing a keyless-but-otherwise-incomplete row at a host does not paper over a missing required field.
- **It is DISTINCT from a `[[provider]]` row's own `base_url` field** (§3.2). A top-level `base_url = "…"` key is the override scalar; a `base_url` *inside* a `[[provider]]` table is that row's host. The two never collide (one is a bare top-level key, the other lives in an array-of-tables), so a single file may carry **both** — a row's default host plus a top-level override — and each round-trips through `--dump-config` independently (§6).
- **It applies uniformly through the one fold** — every entry point resolves the provider by the same `into_resolved`, so a `--base-url` override reaches **`bz` generation, `--list-models`, `--count-tokens`, and `--login`** identically (each calls `into_resolved`; none re-derives the host). A harness can therefore point *discovery* and *credential-write* at the same proxy it points generation at, with one flag.
- **`--dump-config` shows it** as a top-level `base_url` scalar (the merged *partial*, pre-resolve, §6) — the honest representation of an override the operator added over the floor, mirroring how `model` dumps as a top-level scalar even though it drives routing. Re-loading the dump re-applies it over whatever row the live defaults route to.

**Explicitly declined: full row injection (no `--protocol` / `--auth` flags).** The override lifts *one* field because a host is the *only* field that legitimately varies with the deployment while everything else stays the provider's. Protocol dialect and auth mode are **provider identity**, not deployment: a run that needs a *different* protocol or auth is talking to a **genuinely new provider**, which is **config-file territory** (a `[[provider]]` row — §3.2 — dumpable and reusable via `bz --dump-config`/`--config`). Adding `--protocol`/`--auth`/`--api-header` flags would be reconstructing a whole row on the command line one scalar at a time — the CLI growing a second, worse encoding of the `[[provider]]` table (a new-flag smell, AGENTS.md; the door §5.10.3 deliberately keeps shut). The boundary is a **capability line, not a size limit**: `base_url` is severable and self-contained (one host string), a protocol/auth pair is a coupled row that belongs in the one place rows live. This door stays shut deliberately; re-opening it is a change request to this section and architecture.md §5.10.3, argued there, not an additive flag.

---

## 5. Config-file location — a chicken-free fold (which file, not which value)

`resolve` receives an already-read `file: PartialConfig`. *Which file* `main` read is itself a fold, but over **paths**, resolved before `resolve` runs:

```
--config FILE  >  $BRAZEN_CONFIG  >  $XDG_CONFIG_HOME/brazen/config.toml   (fallback ~/.config/brazen/config.toml)
```

```rust
fn config_path(flags: &Flags, env: &EnvSnapshot) -> PathBuf {
    flags.config.clone()
        .or_else(|| env.get("BRAZEN_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| xdg_config_home(env).join("brazen/config.toml"))   // $XDG_CONFIG_HOME or ~/.config
}
```

The same `Option::or` shape as the value fold, one level up. The critical distinction, locked: **`--config` changes *which file* is the file layer; it does not change *layer precedence*.** A direct flag (`--model`, `--temperature`) still **beats** any value inside the file `--config` points at, because the flag is the *flags* layer and the file is the *file* layer regardless of how that file was located (architecture.md §6.3, §13.8). `--config prod.toml` and `--model foo` together resolve `model` to `foo`. The path fold answers "where do the file-layer bytes come from"; the value fold (§3) answers "who wins" — two separate questions, never conflated.

If the resolved path does not exist, `main` supplies `PartialConfig::default()` (§3.3) — absence is the identity element, not an error. If it exists but is malformed, that surfaces as a `Config` error (§7).

---

## 6. `--dump-config` — the only bridge between flag-encoding and file-encoding

"Compiling" a config is **not a build step and not a new verb** (architecture.md §6.2). A config file *is* a `PartialConfig` in TOML; flags are the *same fact* in another encoding. `--dump-config` is the single bridge:

```rust
// bz --dump-config [flags…]   -> TOML on stdout, exit 0; reads NO stdin request, performs NO transport.
fn dump_config(flags: PartialConfig, env: &EnvSnapshot, file: PartialConfig, defaults: PartialConfig) -> String {
    let merged = flags.or(partial_from_env(env)).or(file);   // SAME fold (§3) — but WITHOUT the defaults operand
    toml::to_string(&redact(merged)).expect("PartialConfig is infallibly serializable")
}
```

Four decisions, locked:

- **Rows emit in `providers` order — the dump round-trips *priority*, not just content.** Row order is routing priority (§2.2, architecture.md §4.3.1), so a dump that reordered rows would silently re-route the config it claims to reproduce. Emitting the list as it stands makes `parse(dump(cfg))` preserve the order by construction, and it needs no order key on the wire because the `[[provider]]` array's line order *is* the encoding (§2.2). This **replaces** the narrower rule that sorted rows by name and floated the `default_provider` row to the front — enough to round-trip a head, not a list.
- **`serialize(merged_without_defaults)`.** The embedded-defaults operand is **omitted** from the dump (architecture.md §6.2: "the merged `PartialConfig`", §6.1 defaults are the always-present floor). Dumping defaults would bake brazen's own floor into the user's file, so a later brazen that ships a better default could never reach them — the file would pin the old value forever. The dump captures only what the operator *added* over the floor; re-loading it (`bz --config dumped.toml`) re-applies the live defaults beneath it. It is the **same fold** as §3 minus one operand — no second serialization path. The dump carries **no version marker** either, and needs none: under the additive-only forward-evolution invariant (§2.4) a dumped file stays valid and unchanged in meaning forever, so the keys it *does* pin freeze at their dump-time values while every key it omits keeps re-deriving the live default.
- **Secrets elide to the inert `"<redacted>"` sentinel** — never a real key, never a `${VAR}` reference (architecture.md §6.2, §13.2). `Secret`'s `Serialize` writes plaintext **only** into the 0600 credential file (architecture.md §6.4); in a `--dump-config` context `redact()` replaces any `api_key`/secret-bearing field with the literal string `"<redacted>"` *before* serialization. The sentinel is **inert**: re-loading the dumped file yields an `api_key` of `"<redacted>"`, which is not a valid credential and forces the operator to point env/store at the real secret — exactly the desired failure (a config file is never a place a secret lives). **No env-expansion mechanism is added** — a `${VAR}` ref would be a new feature and a new parse path; the sentinel is a dead string, not a reference (architecture.md §13.2).
- **No `compile` subcommand.** A new verb is a smell (architecture.md guidance, §6.2). `--dump-config` is a flag on the one binary; the round-trip is `bz --dump-config > prod.toml` then `bz --config prod.toml`. One schema, one (de)serializer, flags and file the same fact in two encodings.

A row's `body_defaults`, its sibling `unsupported_body_keys` (§4.1.1), and its `[provider.models]` block (§4.4) all ride the dump verbatim as part of its `[[provider]]` table (each is row data, not credentials — `redact()` touches only `api_key`, §6); a dumped row therefore re-parses to the same maps/lists/block, and the round-trip golden (§8) covers it.

`--dump-config` and a normal run share the §3 fold; the dump merely stops before `into_resolved` (it serializes the merged *partial*, not the resolved config) and omits the defaults operand. Because rows emit in `providers` order (above), scalar maps iterate as `BTreeMap`s, and serde field order is fixed, the output is **deterministic** — byte-stable for a golden test (§8).

---

## 7. `into_resolved()` — validation & the `Config` (78) error set

`into_resolved` turns the merged `PartialConfig` into a `ResolvedConfig` (or a `ConfigError` → exit **78**, architecture.md §8). It is where routing is *computed* (architecture.md §4.3) and where every contradiction is surfaced — **never** a silent pick.

```rust
impl PartialConfig {
    fn into_resolved(self, req_model: Option<&str>) -> Result<ResolvedConfig, ConfigError> { … }
}
```

The model used for routing is **request.model if present, else `getConfigValue("model")`** (architecture.md §4.3: "the request's model, when set, wins for routing"). Routing then resolves a single provider row by this query:

1. If a provider is named (`provider` field, from flag/env/file) → find the row of that name in `providers`.
2. Else, **with no routing model at all** (the zero-config `bz "q"`) → default to `providers.first()`: config-file order, "whatever you find first reading from the top," **not** the alphabetically-first name. A **user file's rows outrank the built-in defaults'** (a config declaring `chatgpt` first defaults to `chatgpt`, never the built-in `anthropic` beside it in the merged table) because the merge puts the higher layer's rows first (§3.2). The empty model seed then takes `select_model`'s first cached model in `serve` — "no specification" resolves to (first row, first cached model). A config with **no provider rows at all** (an empty `providers`) is the lone `NoProvider` residue here. (`--login` opts out of this default — §7.1 below / auth.md §7.1 — a credential write must name its target via the *selector*.)
3. Else (a routing model is present, no provider named) → `providers.iter().find(|(_, row)| row_owns(row, model))`: the **first** row that **owns** it, where a row owns the model when its `model_aliases` **contains** it (substitution shorthand) **or** one of its `model_prefixes` is a prefix of it (family ownership). Either match owns; `find` **stops at the first**, so further owners are never consulted and are **not** an error (architecture.md §4.3 — the priority list is the tiebreak). Prefix ownership is what lets `bz --model claude-haiku-4-5-20251001 "q"` route with no `--provider` — a versioned wire id no alias could enumerate is routed by the family its row claims (e.g. anthropic ships `model_prefixes = ["claude-"]`). `openai-responses` and `ollama` ship none (the former shares OpenAI's ids over a second protocol; the latter has no stable family), so they stay opt-in via explicit `--provider`.

**Step 3 is the *only* order-sensitive resolution in brazen.** Step 1 (an explicit `--provider`) is a by-name lookup that ignores order entirely and remains the unconditional override for anyone who does not want to reason about priority at all.

**An empty-string `model_prefixes` element is `BadValue` (78) — the one validation greedy-first makes load-bearing.** `"anything".starts_with("")` is `true`, so a row carrying `model_prefixes = [""]` owns **every model**. Under the retired ambiguity rule such a row collided with any other owner and failed loudly; under greedy-first an early row with an empty prefix silently swallows all routing, and the operator sees no error — only the wrong provider, forever. An empty prefix is never an authored priority; it is a typo or a half-deleted entry, so it is refused at resolution rather than obeyed. **The distinction is exact and both halves matter:** `model_prefixes = []` — the empty *list*, claiming nothing — stays perfectly legal and is how a row opts out of family routing (`openai-responses` ships it); `[""]` — an empty *element* — is the error. This is the general rule that a value which cannot be meant should not be silently honored, not a special case for routing.

Every way this can fail, each → `ConfigError` → exit 78:

| `ConfigError` variant | Trigger | Why surfaced, not papered over |
|---|---|---|
| `NoProvider` | No provider named **and** EITHER a routing model owned by **zero** rows (no `model_aliases` entry and no `model_prefixes` match), OR **no model at all and an empty provider table**. A no-model request with a **non-empty** table is NOT this — it defaults to the first row (step 2). `--login` re-raises it when no provider is named. | Cannot route. A *given* model no row owns needs explicit `--provider` (architecture.md §4.3 — ownership and identity passthrough cover *routing*/*substitution* respectively, not each other); with no model there is nothing to own, so the first row is the default, not an error. |
| `UnknownProvider { name }` | A provider is named but no row with that key exists (typo, or a row referencing a deleted protocol/auth module — architecture.md §4.6) | Two homes for "which providers exist" would drift; the row table is the one home, and a miss is an error. |
| *(retired — **no** variant: greedy-first resolves it)* | Two or more rows owning one routing model **is not an error.** The first in `providers` wins (step 3). | architecture.md §4.3: a total order always has a first element, so there is no tie to surface. The former `AmbiguousModel` refused instead, which forced an operator diverting **one** id to disarm a whole vendor's family routing — collateral worse than the ambiguity. The operator authored the order; `--dump-config` prints it (§6), and `--provider` overrides it. |
| *(no `ConfigError` variant — fails closed at **dispatch**)* | A row names a `protocol`/`auth` whose `ProtocolId`/`AuthId` is a **valid enum value with no registry entry** (a dialect whose `Registry::builtin()` insert was removed) | An **unknown string** (a typo like `protocol = "openai_chatt"`) is already rejected at deserialize → `MalformedFile`, because `ProtocolId`/`AuthId` is a closed, typo-checked vocabulary (architecture.md §4.2). A *valid-but-unregistered* id is **not** pre-checked in resolution: `Registry::builtin()` is the one home for "which dialects exist" (architecture.md §4.4), and re-listing them in `resolve` would be a second home that drifts. The lookup (`registry.protocols[id]` / `registry.auths[id]`) returns `None` and `run` surfaces it as a `Config`/78 failure at dispatch — fail-closed, just not pre-emptively. This avoids both double-implementation and the risk of resolution rejecting a row the registry would actually serve. |
| `IncompleteProvider { name, field }` | The resolved (post-fold) row for the routed provider is missing a required field (`base_url`, `protocol`, `auth`, `api_header`) — e.g. a user added a partial `[[provider]]` that no embedded row completes | A `PartialProvider` is sparse by design (§2); the routed row must be *complete after the fold*. Surfaced per missing field, not a generic "bad config". |
| `BadValue { key, detail }` | A value parses as TOML/flag but is out of range or contradictory (`temperature` NaN, `max_tokens = 0`, an `output` brazen doesn't define, an env scalar that failed `from_str` in §3.4, **or a `body_defaults` gen-scalar of the wrong JSON type / out of range** — `body_defaults.max_tokens = "lots"`, `= 0`, §4.1). **Also an EMPTY STRING element in `model_prefixes`** (`model_prefixes = [""]`) — see below. | A contradictory config is a `Config` error (architecture.md §8). Surfaced with the offending key. |
| `MalformedFile { detail }` | The config file exists but is not valid `PartialConfig` TOML (incl. `deny_unknown_fields` §2.3 and the duplicate-`name` rule §2.2) | Corruption ≠ absence (§3.3): a present-but-broken file is an error, an absent file is the identity element. |
| `Ingress { detail }` | A serve/ingress path resolved the `[ingress]` table (§2.5) and it cannot serve: no table under `--serve`, `dialect` missing, an unknown `lossy_overrides` adaptation name, an unparseable `listen`, or a non-loopback `listen` without `token` (ingress.md §6–§7) | Surfaced ONLY by `resolve_ingress`, never by parse or an ordinary run (§2.5 severability). A typo'd override must never silently leave the default in force, and a routable listener wired to the operator's credentials must be a deliberate, authenticated act. |

After validation, `into_resolved` performs **alias substitution once**: `wire_model = row.model_aliases.get(routing_model).unwrap_or(routing_model)` (architecture.md §4.3), and stores it in `ResolvedConfig` so `ProviderCtx.model` is already the wire id and `encode` has no model logic (architecture.md §4.1). The substitution is **identity-passthrough** — an unaliased string passes through verbatim — so it never fails; only *routing* (steps 1–3 above) can error.

Resolution does **nothing further** about the model: there is no owned-vs-probe query and no `probe` field (this **dissolves** the former `ResolvedConfig.probe` — model-discovery.md §5). `model_prefixes` is consumed for **routing only** (which row owns a full id, step 3 above); whether the resolved `model` is a full wire id or a partial is **not** decided here. `cfg.model` carries the alias-substituted string **verbatim** — a full id, a partial seed, or `""` (absent) — and `serve` resolves it against the provider's model cache by the total `select_model` (an exact/partial match, the default, else the string verbatim — model-discovery.md §4–§5). Resolution stays pure (no transport, no filesystem); the cache lookup is `serve`'s. This does **not** widen the routing errors: a partial with no resolvable provider is still `NoProvider` (the table above), because a seed cannot select a provider. (The former `row_has_prefixes`/bl-3989 probe guard is gone with the probe — no generation path makes a `/models` GET, so the prefix-less-row regression it patched cannot recur; model-discovery.md §5.2.)

```rust
pub struct ResolvedConfig {
    pub provider:  Provider,          // the single resolved, complete row (architecture.md §4.2)
    pub model:     String,            // the alias-resolved model string VERBATIM — a full id, a partial seed, or "" (absent); serve resolves it against the model cache (model-discovery.md §4–§5)
    pub output:    OutMode,
    pub thinking:  bool,              // --thinking resolved to a concrete bool (default false); the text sink gates reasoning + the separator on it (architecture.md §5.3). Inert in NDJSON/raw.
    pub raw:       bool,              // == (output == Raw); a query, see note
    pub inline_key: Option<Secret>,   // the §6.5 stateless path; ApiKey/Bearer::apply prefer it
    pub max_tokens, temperature, top_p, stream, system …   // resolved gen defaults — each already folds the row body_default beneath flag/env/file (§4.1)
    pub extra: Map<String, Value>,    // top-level `extra` + the row's non-gen `body_defaults`, merged here; fill_absent seeds it into req.extra (§4.1)
}
```

> **`raw` is computed, not a second field.** architecture.md §4.4 reads `cfg.raw`; it is the query `self.output == OutMode::Raw`, exposed as a method/accessor so "is this a raw run" has one home (the output mode), consistent with the derived-vs-stored ledger (architecture.md §3.5). Listed above as a field for shape only — it is a getter.

**Resolution order vs. body read (architecture.md §4.4).** Output mode is resolved **first and body-independent** (`output_mode(...)` reads only flags/env/file/defaults) because it decides whether stdin is parsed or passed through `--raw`. The full `resolve`/`into_resolved` (which needs `req.model` for routing) runs after the body is read on the non-raw path. This spec defines the merge those calls share; the ordering is architecture.md's.

---

## 8. Testability — pure over injected inputs

Resolution is **pure**: `resolve` is a function of `(flags, EnvSnapshot, file, defaults, req)` with no IO, no clock, no `std::env`. Every behavior in this spec is a table test from literals — no process environment, no temp files in the core (the file is injected as an already-parsed `PartialConfig`; `main`'s actual disk read is the thin uncovered shim, architecture.md §9.5).

| What | Test |
|---|---|
| The fold precedence | `resolve` over four hand-built `PartialConfig`s asserts **flag > env > file > default** per field, and that a higher layer's `None` defers (the `Option::or` law). |
| Per-key provider merge | A file patching one `body_defaults` key leaves the embedded row's other fields intact, and `body_defaults` merges per-key under `or_map` (§3.2). |
| Missing file = identity | `resolve(flags, env, PartialConfig::default(), defaults, …)` == `resolve` with the file operand dropped (§3.3). |
| `partial_from_env` | A literal `EnvSnapshot` → expected `PartialConfig`; `$BRAZEN_CONFIG` absent from it; the `ANTHROPIC_API_KEY` < `BRAZEN_API_KEY` ordering (§3.4). |
| `base_url` host override (§4.5) | The top-level scalar replaces the routed row's `base_url` (protocol/auth/model untouched); precedence **flag > env > file-scalar > row**; a `None` scalar leaves the row's own host; the top-level scalar and a row's `base_url` co-exist in one file and round-trip the dump independently. |
| `fill_absent` (architecture.md §9.6) | A field the request *sets* returns untouched; a field it *omits* resolves request>flag>env>file>row-default; `--config FILE` only changes which file (a direct flag still beats it). |
| `body_defaults` | A gen scalar (`max_tokens`) folds into `cfg.max_tokens` only when flag/env/file all `None`, and a flag beats it; a non-gen key (`store`) reaches `req.extra` beneath a request's own key; a row that pins nothing leaves the field absent. A wrong-typed / out-of-range gen scalar is `BadValue`/78 (§4.1). |
| `unsupported_body_keys` (§4.1.1) | A row listing the gen trio + a non-gen key, run after `fill_absent` on a request that EXPLICITLY set all four: `max_tokens`/`temperature`/`top_p` clear to `None`, the non-gen key leaves `req.extra`. A row that pins nothing (empty `Vec`) leaves every field untouched. |
| Every `ConfigError` (§7) | One literal case each: `NoProvider`, `UnknownProvider`, `IncompleteProvider`, `BadValue`, `MalformedFile` (incl. `deny_unknown_fields` and duplicate `name`). A valid-but-unregistered `protocol`/`auth` id is **not** a `ConfigError` — it fails closed at dispatch (§7), tested at the registry seam. |
| **Greedy-first routing** (§7 step 3) | Two rows owning one model resolves to the **earlier**, no error: by alias-over-prefix, by prefix-over-prefix, and by a **user row over a defaults row**. Plus the negative that proves the *order* decided it and not an accident of the structure: reorder the two rows and the winner flips — the executable form of "the priority list is the tiebreak" (architecture.md §4.3). |
| **Redeclaring hoists a row** (§3.2) | A user row naming a defaults row with only a `body_defaults` tweak resolves with the defaults' `model_prefixes` **at the user's position** — the foot-gun stated in §3.2, pinned so it cannot regress silently. |
| **The merge's tail law** (§3.2) | A user file + defaults merge keeps the defaults' **unredeclared** rows in defaults order, behind the user's rows — the third position law, which the greedy-winner and redeclare-hoist cases do not reach. |
| **Empty prefix element** (§7) | `model_prefixes = [""]` → `BadValue` (78), while `model_prefixes = []` resolves fine — the pair, since only the contrast pins the rule. |
| **Order round-trips `--dump-config`** (§2.2, §6) | Dump a config whose priority differs from name order, re-parse it, and get the same row order — the property the old default-row-floats-first hack could only give for the head. |
| `--dump-config` | Golden TOML: `merged_without_defaults`, secrets as `"<redacted>"`, rows in `providers` order with scalar maps in `BTreeMap` order; and the **round-trip** `parse(dump(cfg)) == merged_without_defaults` (§6, §2.2). |
| The `[ingress]` table (§2.5) | Parse + the row-style deny (a typo'd key → `MalformedFile`); per-field fold with the per-key `lossy_overrides` merge; dump `token` redaction + round-trip; and `resolve_ingress` one case each — loopback defaults OK with no token, missing table, missing `dialect`, unknown adaptation name, unparseable `listen`, non-loopback-without-token, non-loopback-with-token OK. |
| `defaults.toml` validity | A unit test `toml::from_str::<PartialConfig>(include_str!(…))` succeeds — a malformed embedded edit fails the build, not a user run (§3.5). |

Because the four layers are one type and the merge is one associative `or`, the test surface is small: prove `or` once, prove the env projection once, prove each error once. The greedy-first tests are the executable form of architecture.md §4.3's "the priority list is the tiebreak" — and the pair of them (winner, then winner-after-reorder) is what proves the *order* decided it and not an accident of the map.

---

## 9. Edge cases & change requests

Model resolution **beyond routing + alias substitution** is not this spec's: `serve` resolves `cfg.model` against the model cache by the total `select_model` (model-discovery.md §4–§5). There is **no** `probe` field or owned-vs-probe query here — both are dissolved per the architecture.md §4.3 amendment. Otherwise this spec is fully derivable from architecture.md §3.1, §4.2–§4.4, §6, §8, and §13.1/§13.2/§13.8. Two seams are *named here for the first time* but introduce no new architectural fact — they mechanize decisions architecture.md already made:

- **`EnvSnapshot`** is the concrete injected type behind architecture.md §6.1's `env: &EnvSnapshot` parameter and §6.5's "nothing reads `std::env`" — a `BTreeMap<String,String>` newtype, not a new concept.
- **`ConfigError` variants** (§7) refine architecture.md §8's single "78 = no provider resolved / unknown / bad config" row into the specific surfaced errors; the exit code and class are unchanged (all → 78).

**`body_defaults` generalizes `default_max_tokens` (amends architecture.md §3.1, §4.1, §4.2, §6.1, §13.1).** The former scalar `default_max_tokens` row field is **removed**; a row's body defaults are one `body_defaults` map (§4.1). This is a single-source-of-truth consolidation, not a new capability — architecture.md §13.1's "sane default carried as provider-row data" now reads as the general map, and `Provider` (the resolved row) no longer carries the value at all (it is consumed into `ResolvedConfig` at resolve). Amended in architecture.md directly per the "fix the doc, don't deviate" rule.

**`ProviderCtx.extra` is removed (amends architecture.md §4.1).** It was wired from the top-level `cfg.extra` but **read by no encoder** — every encoder folds `req.extra`, not `ctx.extra` — so config-level body passthrough never reached the wire (a latent contradiction with §2.3's "a top-level passthrough knob is forwarded"). `body_defaults` and the top-level `extra` now both reach the wire through the **one live seam, `req.extra`**, seeded by `fill_absent` (§4.1); the dead `ProviderCtx.extra` field is deleted rather than belatedly wired into all five encoders (deeper/narrower interface, and it sidesteps the gen-field problem §4.1 raises). `ProviderCtx` keeps `base_url`, `model`, `beta_headers`.

If a future provider needs a config-time capability not expressible as a sparse row field (§2) — e.g. a per-auth-mode default that the row cannot hold (architecture.md §4.5 keeps such headers on the `Auth` impl, not the row) — that is a change request to architecture.md §4.2/§4.5, raised there, not absorbed silently here.
