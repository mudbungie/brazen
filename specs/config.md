# Config schema, resolution & the compiled config

> **Living document.** Edited like code. This spec derives from the canonical contract in architecture.md and MUST NOT contradict it. Where it cannot be written without changing the architecture, it raises a **change request to architecture.md** (§9) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md) — §3.1, §4.2, §4.3, §4.4, §6, §8, §13.

---

## 1. Purpose & Scope

Define, normatively, how `bz` turns four sparse inputs — **flags, environment, config file, embedded defaults** — into the one `ResolvedConfig` the pipeline runs on, and how it surfaces that resolution back out as a file via `--dump-config`. This is the spec for `config/resolve.rs` and `config/provider.rs` (architecture.md §11).

**In scope:** the `PartialConfig`/`PartialProvider` schema and why the four inputs are four instances of *one* type (§2); the `resolve` fold under `Option::or` with precedence-as-operand-order, the injected `EnvSnapshot` projection, embedded `defaults.toml` through the same parse path, and the missing-file-is-identity rule (§3); how the request is **not** a fold layer and how `fill_absent`/`getConfigValue` fill only the gaps, including `default_max_tokens` (§4); the config-file location fold (`--config` > `$BRAZEN_CONFIG` > XDG) (§5); `--dump-config` as the only bridge between flag-encoding and file-encoding (§6); every `into_resolved()` `Config` (78) error including ambiguous model→provider resolution (§7); the testability story (§8).

**Out of scope (owned elsewhere):** the credential store, `Secret` semantics, OAuth, and `bz login` (architecture.md §6.4, §7 — the auth spec); the provider-row *contents* and which protocol/auth a row names (architecture.md §4.2 — the providers spec); each protocol's use of `ctx.model`/`req.max_tokens` after resolution (the mapping specs); the output `Sink`, exit-code driver, and signal handling (architecture.md §5, §8). This module is **vendor-blind**: it never `match`es a provider `name` — resolution is a *query over rows* (architecture.md §4.3).

### 1.1 Inherited invariants (from architecture.md — restated so this spec is self-contained)

1. **One schema, one fold, no privileged layer.** Flags, env, file, defaults are four `PartialConfig` values; precedence is the **order of operands** in an `Option::or` chain, which is data, never code (architecture.md §6.1).
2. **The pipe is clean data, not a config layer.** A request field the body *sets* is used as-is; a field it *omits* is filled by `getConfigValue` = **flag > env > config file > app/row default**. Per gen field the effective order is **request > flag > env > config > default**, expressed as **two mechanisms** — the request, then config-fills-the-rest — never one fold the caller must learn (architecture.md §4.4, §13.8).
3. **model→provider routing is a query, not a second table.** The user names a provider explicitly **or** brazen finds the single row whose `model_aliases` contains the model; **two matches → `Config` (78)**, never a silent pick (architecture.md §4.3).
4. **Alias substitution is `model_aliases.get(model).unwrap_or(model)`** — an unaliased model passes through verbatim; substitution covers spelling, **not** routing (architecture.md §4.3).
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
    pub provider:    Option<String>,
    pub model:       Option<String>,
    pub api_key:     Option<Secret>,                     // inline key => stateless, bypasses CredStore (architecture.md §6.5)
    pub output:      Option<OutputMode>,                 // Text | Ndjson | Raw
    pub thinking:    Option<bool>,                        // --thinking: a flag on the text projection, NOT a 4th OutMode (architecture.md §5.3)
    pub max_tokens:  Option<u32>,
    pub temperature: Option<f32>,
    pub top_p:       Option<f32>,
    pub stream:      Option<bool>,
    #[serde(default)]
    pub providers:   BTreeMap<String, PartialProvider>,  // sparse, keyed by name; merged per-key (§3.2)
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
    pub default_max_tokens: Option<u32>,
}
```

**`providers` is a `BTreeMap`, not the `Vec<[[provider]]>` of the embedded table.** The embedded `defaults.toml` (architecture.md §4.2) authors rows as a TOML array-of-tables for human ergonomics; deserialization keys them by `name` into the map so the **merge is per-provider, per-field** — a user file can override exactly Anthropic's `default_max_tokens` without redeclaring the whole row (architecture.md §6.1). `BTreeMap` (not `HashMap`) makes `--dump-config` output deterministic (§6). Mapping the wire array-of-tables onto the keyed map is the one custom `Deserialize` in this module (§2.2).

### 2.1 Why every field is `Option`

`None` is the identity element of `Option::or`: `x.or(None) == x` and `None.or(y) == y`. A field a layer does not mention is `None` and contributes nothing to the fold — *absence is not a value*. This is what lets the four layers be one type with no "is this set?" flag beside each field (a second home for "absence"). The empty config is `PartialConfig::default()` — all `None`, an empty map — and it is the literal identity of the whole fold (§3.3), which is why a missing file is not a special case.

### 2.2 The array-of-tables ⇄ keyed-map seam

`defaults.toml` and user files write providers as:

```toml
[[provider]]
name = "anthropic"
base_url = "https://api.anthropic.com"
# … (architecture.md §4.2)
```

A custom `Deserialize` for `PartialConfig` reads the `[[provider]]` array, lifts each table's `name` to the `BTreeMap` key, and stores the remainder as a `PartialProvider`. A **duplicate `name` within one file** is a `Config` error (78) — `BTreeMap` insert collision is surfaced, never last-wins, because two rows for one name is a contradiction inside a single layer (cross-layer override is the fold's job, §3.2). `Serialize` (for `--dump-config`, §6) runs the inverse: keyed map → array-of-tables with `name` re-injected, so a dumped config re-parses identically — the encoding round-trips.

### 2.3 `deny_unknown_fields` — a typo is a `Config` error, not a passthrough

A misspelled **scalar** config key (`temperatue`, `maxtokens`) is rejected at `toml::from_str` → exit 78. This is the deliberate *opposite* of the canonical request's `extra` long-tail valve (architecture.md §3.1, where a misspelled request field silently becomes a passthrough knob): config is operator-authored and small, so a typo there is a bug to surface, not a knob to forward. The single sanctioned long-tail in config is the top-level `extra` map (passthrough provider knobs, architecture.md §4.1 `ProviderCtx.extra`); it is `#[serde(flatten)]`, so genuinely-unmodeled top-level keys still land there rather than erroring. The line is drawn once: **named fields are typo-checked; the one `extra` map is the open valve.**

**Where the deny actually bites (implementation note).** A top-level `#[serde(flatten)] extra` and `deny_unknown_fields` are mutually exclusive in serde, and the flatten valve cannot tell a typo from a deliberate knob — so an unmodeled **top-level** key lands in `extra`, it does not error. The typo-check therefore lives where there is **no** valve: each `[[provider]]` **row** is `deny_unknown_fields` (a misspelled `bas_url` → `MalformedFile`/78), and a duplicate provider `name` within one file is rejected (§2.2). A mistyped top-level scalar is forwarded as a passthrough knob, exactly as a mistyped request field is (architecture.md §3.1) — the asymmetry the first paragraph asserts holds for **row** fields, not top-level ones. This is the coherent reading the resolver implements; the `MalformedFile` test surface (§8) is the row-typo + duplicate-name pair.

---

## 3. `resolve` — the fold under `Option::or`

```rust
pub fn resolve(
    flags:    PartialConfig,
    env:      &EnvSnapshot,        // injected; the lib never reads std::env (architecture.md §6.5)
    file:     PartialConfig,       // already read from disk by main; missing file => PartialConfig::default()
    defaults: PartialConfig,       // toml::from_str(include_str!("defaults.toml")) — same path, no bootstrap
    req:      Option<&CanonicalRequest>,
) -> Result<ResolvedConfig, ConfigError> {
    let env = partial_from_env(env);                          // pure projection of the injected snapshot (§3.4)
    let cfg = flags.or(env).or(file).or(defaults);            // PRECEDENCE = OPERAND ORDER. flag > env > file > default.
    cfg.into_resolved(req.and_then(CanonicalRequest::model))  // request.model wins for routing; else getConfigValue("model")
}
```

The whole of resolution is **one expression**: a fold of four operands under `PartialConfig::or`, then `into_resolved`. There is no `if layer == flags` anywhere — the precedence policy is the *textual order of the operands*, which is data the reader can see, not control flow buried in a merge function. Reordering precedence is editing operand order, not editing code logic (the severability test, architecture.md guidance).

### 3.1 `PartialConfig::or` — the fold step

```rust
impl PartialConfig {
    /// self has higher precedence than other. self.field.or(other.field) per scalar;
    /// per-key recursive merge for providers; or for the extra map.
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
            providers:   merge_providers(self.providers, other.providers),  // §3.2
            extra:       or_map(self.extra, other.extra),                   // higher-precedence key wins
        }
    }
}
```

Every scalar is a literal `Option::or`: the left (higher-precedence) `Some` wins; `None` defers. `or` is **associative**, so `a.or(b).or(c).or(d)` needs no parenthesization and the four-layer fold has one unambiguous result regardless of grouping — the law that lets "add a layer" mean "add an operand."

### 3.2 The provider table merges per-key, per-field

```rust
fn merge_providers(hi: BTreeMap<String, PartialProvider>, lo: BTreeMap<String, PartialProvider>)
    -> BTreeMap<String, PartialProvider>
{
    // union of keys; for a shared key, PartialProvider::or field-by-field (hi wins per field)
}
```

A key present in only one layer passes through; a key in both is merged **field-by-field** under the same `or`. So a user file with:

```toml
[[provider]]
name = "anthropic"
default_max_tokens = 8192
```

overrides exactly that one field of the embedded Anthropic row — `base_url`, `protocol`, `auth`, `api_header`, `beta_headers` all defer to the lower-precedence embedded layer (architecture.md §6.1: "the file can override one header on Anthropic without redeclaring the row"). The **same `or` mechanism** drives scalars and the provider table — there is no second merge algorithm.

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
| `BRAZEN_API_KEY` | `api_key` (`Secret`) |
| `ANTHROPIC_API_KEY` | `api_key` (`Secret`) — accepted as a vendor-conventional alias, lower precedence than `BRAZEN_API_KEY` within the projection |
| `BRAZEN_MAX_TOKENS` | `max_tokens` (parsed; unparseable → §7 `Config`) |
| `BRAZEN_TEMPERATURE` | `temperature` |
| `BRAZEN_OUTPUT` | `output` |
| `BRAZEN_THINKING` | `thinking` (parsed bool; `--thinking` on the text projection, architecture.md §5.3) |
| `BRAZEN_STREAM` | `stream` |

`$BRAZEN_CONFIG` is **not** in this table — it selects *which file* to read (§5), a pre-`resolve` concern, not a field of the resolved config. Because the projection is pure over an injected map, the entire env-precedence behavior is a table test with no process-environment dependency (§8).

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
    req.max_tokens  = req.max_tokens.or(cfg.effective_max_tokens());        // §4.1 — row default at lowest precedence
    req.temperature = req.temperature.or(cfg.temperature);
    req.top_p       = req.top_p.or(cfg.top_p);
    req.system      = req.system.take().or_else(|| cfg.system.clone());
    // stream: req.stream (a bool, not Option) wins; cfg.stream only seeds it when the body is constructed (§4.2)
    // messages, tools, tool_choice, extra are the request's ALONE — never config-filled (architecture.md §4.4)
}
```

So **per gen field** the effective order is **request > flag > env > config > default**, achieved by composition: the request is the outer `or`, and `getConfigValue` (the already-resolved `flag > env > file > default`) is the inner. The caller never reasons about "does my body beat a flag?" — a body value beats everything for that field by being present, and config fills only what the body leaves unset. `encode` then reads every gen param straight off `req` and the wire `model` off `ctx` (the mapping specs); resolution has already done all merging.

### 4.1 `default_max_tokens` — lowest-precedence operand for a required param

`max_tokens` is the one param a provider may *require* (Anthropic), and its sane default is **data on the provider row** (`default_max_tokens`, architecture.md §4.2, §13.1), not a hard-coded constant:

```rust
impl ResolvedConfig {
    fn effective_max_tokens(&self) -> Option<u32> {
        // self.max_tokens already = flag > env > config-file (the §3 fold). The row default is BELOW all of those.
        self.max_tokens.or(self.provider.default_max_tokens)
    }
}
```

Combined with `fill_absent`, the full chain for `max_tokens` is **request value, else flag > env > config file > row default**. A provider whose row has no `default_max_tokens` (OpenAI Chat, anthropic-messages excepted) leaves it `None`, and a `None` `max_tokens` is **omitted from the wire** by `encode` (the mapping specs). brazen thus **never burdens the caller with a value the model needs** (the row supplies it) and **never invents one the model doesn't** (a non-required param the request omits and config doesn't set stays `None`). This is the resolved decision of architecture.md §13.1, mechanized here.

### 4.2 `stream` is a `bool`, not `Option`, on the request

`CanonicalRequest.stream` is a plain `bool` (architecture.md §3.1), so it has no "absent" state to `fill_absent`. The body's `stream` is authoritative once a body exists; `cfg.stream` (the resolved `Option<bool>`) seeds the field **only** when the request is *constructed* by brazen — the positional-prompt constructor (architecture.md §5.5) and an inbound canonical request that did not carry the key (serde `default` → `false`, then overridable). This keeps `stream` out of `fill_absent`'s `Option::or` shape without inventing a third precedence rule: it is the constructor's default, then the body's truth.

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

Three decisions, locked:

- **`serialize(merged_without_defaults)`.** The embedded-defaults operand is **omitted** from the dump (architecture.md §6.2: "the merged `PartialConfig`", §6.1 defaults are the always-present floor). Dumping defaults would bake brazen's own floor into the user's file, so a later brazen that ships a better default could never reach them — the file would pin the old value forever. The dump captures only what the operator *added* over the floor; re-loading it (`bz --config dumped.toml`) re-applies the live defaults beneath it. It is the **same fold** as §3 minus one operand — no second serialization path.
- **Secrets elide to the inert `"<redacted>"` sentinel** — never a real key, never a `${VAR}` reference (architecture.md §6.2, §13.2). `Secret`'s `Serialize` writes plaintext **only** into the 0600 credential file (architecture.md §6.4); in a `--dump-config` context `redact()` replaces any `api_key`/secret-bearing field with the literal string `"<redacted>"` *before* serialization. The sentinel is **inert**: re-loading the dumped file yields an `api_key` of `"<redacted>"`, which is not a valid credential and forces the operator to point env/store at the real secret — exactly the desired failure (a config file is never a place a secret lives). **No env-expansion mechanism is added** — a `${VAR}` ref would be a new feature and a new parse path; the sentinel is a dead string, not a reference (architecture.md §13.2).
- **No `compile` subcommand.** A new verb is a smell (architecture.md guidance, §6.2). `--dump-config` is a flag on the one binary; the round-trip is `bz --dump-config > prod.toml` then `bz --config prod.toml`. One schema, one (de)serializer, flags and file the same fact in two encodings.

`--dump-config` and a normal run share the §3 fold; the dump merely stops before `into_resolved` (it serializes the merged *partial*, not the resolved config) and omits the defaults operand. Because `providers` is a `BTreeMap` (§2) and serde field order is fixed, the output is **deterministic** — byte-stable for a golden test (§8).

---

## 7. `into_resolved()` — validation & the `Config` (78) error set

`into_resolved` turns the merged `PartialConfig` into a `ResolvedConfig` (or a `ConfigError` → exit **78**, architecture.md §8). It is where routing is *computed* (architecture.md §4.3) and where every contradiction is surfaced — **never** a silent pick.

```rust
impl PartialConfig {
    fn into_resolved(self, req_model: Option<&str>) -> Result<ResolvedConfig, ConfigError> { … }
}
```

The model used for routing is **request.model if present, else `getConfigValue("model")`** (architecture.md §4.3: "the request's model, when set, wins for routing"). Routing then resolves a single provider row by this query:

1. If a provider is named (`provider` field, from flag/env/file) → look it up by key in `providers`.
2. Else, find the row(s) whose `model_aliases` **contains** the routing model.

Every way this can fail, each → `ConfigError` → exit 78:

| `ConfigError` variant | Trigger | Why surfaced, not papered over |
|---|---|---|
| `NoProvider` | No provider named **and** the routing model matches **zero** rows' `model_aliases` (or no model at all) | Cannot route. An unaliased model matches no row and so needs explicit `--provider` (architecture.md §4.3 — identity passthrough is *substitution*, not *routing*). |
| `UnknownProvider { name }` | A provider is named but no row with that key exists (typo, or a row referencing a deleted protocol/auth module — architecture.md §4.6) | Two homes for "which providers exist" would drift; the row table is the one home, and a miss is an error. |
| **`AmbiguousModel { model, providers }`** | **No provider named, and the routing model matches `model_aliases` in *two or more* rows** | architecture.md §4.3: "Two matches is a `Config` error (78), never a silent pick — ambiguity is surfaced." The error names every matching provider so the operator adds `--provider` to disambiguate. |
| *(no `ConfigError` variant — fails closed at **dispatch**)* | A row names a `protocol`/`auth` whose `ProtocolId`/`AuthId` is a **valid enum value with no registry entry** (a dialect whose `Registry::builtin()` insert was removed) | An **unknown string** (a typo like `protocol = "openai_chatt"`) is already rejected at deserialize → `MalformedFile`, because `ProtocolId`/`AuthId` is a closed, typo-checked vocabulary (architecture.md §4.2). A *valid-but-unregistered* id is **not** pre-checked in resolution: `Registry::builtin()` is the one home for "which dialects exist" (architecture.md §4.4), and re-listing them in `resolve` would be a second home that drifts. The lookup (`registry.protocols[id]` / `registry.auths[id]`) returns `None` and `run` surfaces it as a `Config`/78 failure at dispatch — fail-closed, just not pre-emptively. This avoids both double-implementation and the risk of resolution rejecting a row the registry would actually serve. |
| `IncompleteProvider { name, field }` | The resolved (post-fold) row for the routed provider is missing a required field (`base_url`, `protocol`, `auth`, `api_header`) — e.g. a user added a partial `[[provider]]` that no embedded row completes | A `PartialProvider` is sparse by design (§2); the routed row must be *complete after the fold*. Surfaced per missing field, not a generic "bad config". |
| `BadValue { key, detail }` | A value parses as TOML/flag but is out of range or contradictory (`temperature` NaN, `max_tokens = 0`, an `output` brazen doesn't define, an env scalar that failed `from_str` in §3.4) | A contradictory config is a `Config` error (architecture.md §8). Surfaced with the offending key. |
| `MalformedFile { detail }` | The config file exists but is not valid `PartialConfig` TOML (incl. `deny_unknown_fields` §2.3 and the duplicate-`name` rule §2.2) | Corruption ≠ absence (§3.3): a present-but-broken file is an error, an absent file is the identity element. |

After validation, `into_resolved` performs **alias substitution once**: `wire_model = row.model_aliases.get(routing_model).unwrap_or(routing_model)` (architecture.md §4.3), and stores it in `ResolvedConfig` so `ProviderCtx.model` is already the wire id and `encode` has no model logic (architecture.md §4.1). The substitution is **identity-passthrough** — an unaliased string passes through verbatim — so it never fails; only *routing* (steps 1–2 above) can error.

```rust
pub struct ResolvedConfig {
    pub provider:  Provider,          // the single resolved, complete row (architecture.md §4.2)
    pub model:     String,            // alias-resolved WIRE id
    pub output:    OutMode,
    pub thinking:  bool,              // --thinking resolved to a concrete bool (default false); the text sink gates reasoning + the separator on it (architecture.md §5.3). Inert in NDJSON/raw.
    pub raw:       bool,              // == (output == Raw); a query, see note
    pub inline_key: Option<Secret>,   // the §6.5 stateless path; ApiKey/Bearer::apply prefer it
    pub max_tokens, temperature, top_p, stream, system, extra …   // the resolved gen defaults fill_absent reads
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
| Per-key provider merge | A file patching one `default_max_tokens` leaves the embedded row's other fields intact (§3.2). |
| Missing file = identity | `resolve(flags, env, PartialConfig::default(), defaults, …)` == `resolve` with the file operand dropped (§3.3). |
| `partial_from_env` | A literal `EnvSnapshot` → expected `PartialConfig`; `$BRAZEN_CONFIG` absent from it; the `ANTHROPIC_API_KEY` < `BRAZEN_API_KEY` ordering (§3.4). |
| `fill_absent` (architecture.md §9.6) | A field the request *sets* returns untouched; a field it *omits* resolves request>flag>env>file>row-default; `--config FILE` only changes which file (a direct flag still beats it). |
| `default_max_tokens` | `effective_max_tokens` returns the row default only when flag/env/file all `None`; a required param is filled, a non-required one stays `None`/omitted (§4.1). |
| Every `ConfigError` (§7) | One literal case each: `NoProvider`, `UnknownProvider`, **`AmbiguousModel` (two rows alias the same model → 78, never a silent pick)**, `IncompleteProvider`, `BadValue`, `MalformedFile` (incl. `deny_unknown_fields` and duplicate `name`). A valid-but-unregistered `protocol`/`auth` id is **not** a `ConfigError` — it fails closed at dispatch (§7), tested at the registry seam. |
| `--dump-config` | Golden TOML: `merged_without_defaults`, secrets as `"<redacted>"`, deterministic `BTreeMap` order; and the **round-trip** `parse(dump(cfg)) == merged_without_defaults` (§6, §2.2). |
| `defaults.toml` validity | A unit test `toml::from_str::<PartialConfig>(include_str!(…))` succeeds — a malformed embedded edit fails the build, not a user run (§3.5). |

Because the four layers are one type and the merge is one associative `or`, the test surface is small: prove `or` once, prove the env projection once, prove each error once. The `AmbiguousModel` test is the executable form of architecture.md §4.3's "surface ambiguity, never silently pick."

---

## 9. Edge cases & change requests

None outstanding. This spec is fully derivable from architecture.md §3.1, §4.2–§4.4, §6, §8, and §13.1/§13.2/§13.8 without amending it. Two seams are *named here for the first time* but introduce no new architectural fact — they mechanize decisions architecture.md already made:

- **`EnvSnapshot`** is the concrete injected type behind architecture.md §6.1's `env: &EnvSnapshot` parameter and §6.5's "nothing reads `std::env`" — a `BTreeMap<String,String>` newtype, not a new concept.
- **`ConfigError` variants** (§7) refine architecture.md §8's single "78 = no provider resolved / unknown / ambiguous / bad config" row into the specific surfaced errors; the exit code and class are unchanged (all → 78).

If a future provider needs a config-time capability not expressible as a sparse row field (§2) — e.g. a per-auth-mode default that the row cannot hold (architecture.md §4.5 keeps such headers on the `Auth` impl, not the row) — that is a change request to architecture.md §4.2/§4.5, raised there, not absorbed silently here.
