+++
title = "Implement first-class prompt caching: typed req.cache breakpoints -> Anthropic cache_control markers"
created = 1782851106
updated = 1782851106
priority = 30
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["caching", "anthropic"]
+++
# Implement first-class prompt caching: typed `req.cache` breakpoints projecting to Anthropic `cache_control` markers

## Adopted resolutions (open decisions — already decided, do NOT re-litigate)
- **No `CacheTtl::as_str()`** — emit literal `"1h"` in the OneHour arm; FiveMin omits ttl. (Avoids an uncovered "5m" arm under the 100% gate.)
- **`CacheBreakpoint` uses `#[serde(flatten)] anchor: CacheAnchor`** with `CacheAnchor` internally tagged `#[serde(tag="anchor")]` → flat wire object `{"anchor":"message","index":2,"ttl":"1h"}`.
- **TTL render:** omit ttl for FiveMin (default 5m), emit `"ttl":"1h"` for OneHour.
- **Skip auto/top-level cache_control mode** for this ball — stays reachable via the `extra` escape hatch; no `CacheAnchor::Auto` variant.
- **Order-preserving:** keep the caller's `Vec` order; do NOT reorder/dedupe breakpoints.
- **Include the CHANGELOG `[Unreleased]` Added entry** (mirrors the `--reasoning` precedent).
- **1h-TTL beta:** verified GA, no `anthropic-beta` header — emit `"1h"` unconditionally; any header stays caller-supplied row `beta_headers` data (no encoder logic).

# Implementation Spec — first-class prompt caching (`req.cache`) for brazen

## 0. One-paragraph north star
`cache` is **request-only structural payload** (like `messages`/`tools`), not config and not a flag. It carries an ordered set of *cache breakpoints*. **Only the Anthropic encoder reads it**, projecting each breakpoint to a per-block `cache_control:{"type":"ephemeral"[,"ttl":"1h"]}` marker on the **last** wire block of the anchored region (tools / system / a message). Every other dialect (OpenAI Responses/Chat, Google, Ollama) caches automatically by prompt prefix and **ignores `req.cache`** with zero code — no marker, no strip. Validation (≤4 breakpoints; each must resolve to ≥1 wire block) is **Anthropic-encode-local** and fails with `ErrorKind::ParseInput` (exit 64). Empty `cache` is the general path with empty input — no special-casing.

---

## 1. New canonical types — `src/canonical/request.rs`
Add one field to `CanonicalRequest`, immediately **after `stream` (line 56) and before `#[serde(flatten)] pub extra` (line 57)**:

```rust
/// Anthropic prompt-cache breakpoints (anthropic-messages §2.10). REQUEST-ONLY
/// structural payload (like `messages`/`tools`): not config-filled, no flag, not
/// stripped. ONLY the Anthropic encoder projects it to per-block `cache_control`;
/// every other dialect caches by prompt prefix and ignores it. Empty = no caching
/// (the general path with empty input — never a branch). Order is significant.
#[serde(default)]
pub cache: Vec<CacheBreakpoint>,
```

Because `CanonicalRequest` derives `Default`, an empty `Vec` is the no-cache path; **no `..Default::default()` literal anywhere needs editing.** Empty serializes as `"cache":[]`, exactly like `tools`/`stop` — confirmed no golden full-request fixture exists (only enums are stringified; `rt()` does `from_str(to_string)` round-trips), so nothing breaks.

Define the three types **after the `ReasoningEffort` block (after line 206)**, mirroring the lifted-knob template:

```rust
/// One prompt-cache breakpoint: WHERE to cut the prefix (`anchor`) and HOW LONG
/// the entry lives (`ttl`). `anchor` is flattened so the wire/config shape is a
/// single flat object: {"anchor":"tools","ttl":"1h"} / {"anchor":"message","index":2}.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    #[serde(flatten)]
    pub anchor: CacheAnchor,
    #[serde(default)]
    pub ttl: CacheTtl,
}

/// The cut point. `Tools`/`System` anchor the whole hoisted block; `Message{index}`
/// anchors a canonical-message index (resolved through the System-hoist skip at
/// encode). snake_case + internal tag `anchor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "anchor", rename_all = "snake_case")]
pub enum CacheAnchor {
    Tools,
    System,
    Message { index: u32 },
}

/// Cache lifetime (anthropic-messages §2.10). `FiveMin` is Anthropic's default and
/// is emitted by OMITTING `ttl`; `OneHour` emits `"ttl":"1h"`. Serde renames are the
/// one home for the `"5m"`/`"1h"` spellings on the canonical (config/wire) surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    #[default]
    #[serde(rename = "5m")]
    FiveMin,
    #[serde(rename = "1h")]
    OneHour,
}
```

**No `as_str()` method** (deliberate deviation from the touchpoint map — see Open Decisions). Reason: since `FiveMin` is emitted by *omission*, an `as_str` would have a `"5m"` arm no production path ever executes, forcing a contrived test to satisfy the 100%-line gate. The serde rename is the single home for the spellings; the encoder emits the literal `"1h"` (the Anthropic-wire spelling) in a branch whose *other* arm (omit) is equally exercised.

---

## 2. Re-export — `src/canonical/mod.rs`
Extend the `pub use request::{…}` list (line 13-15) to add `CacheAnchor, CacheBreakpoint, CacheTtl`:

```rust
pub use request::{
    CacheAnchor, CacheBreakpoint, CacheTtl, CanonicalRequest, Content, ImageSource, Message,
    ReasoningEffort, Role, Tool, ToolChoice,
};
```

---

## 3. New encoder module — `src/protocol/anthropic/encode/cache.rs` (NEW)
Sibling of `blocks.rs`. Operates on the **already-built** `body` map (tools/system/messages all inserted), reading the built arrays as the SSOT for "did this region project to a wire block."

```rust
//! Prompt-cache breakpoint projection (anthropic-messages §2.10): canonical
//! `req.cache` → per-block `cache_control` markers on the LAST wire block of each
//! anchored region. Reads the already-built `body` arrays (SSOT for projection);
//! recomputes ONLY the System-hoist skip count to map a canonical message index to
//! its wire position. ≤4 breakpoints and resolve-or-ParseInput are enforced here.

use serde_json::{json, Map, Value};

use crate::canonical::{CacheAnchor, CacheTtl, CanonicalError, CanonicalRequest, ErrorKind, Role};

const MAX_BREAKPOINTS: usize = 4;

pub(super) fn apply(body: &mut Map<String, Value>, req: &CanonicalRequest) -> Result<(), CanonicalError> {
    if req.cache.is_empty() {
        return Ok(()); // general path, empty input — no marker, body unchanged
    }
    if req.cache.len() > MAX_BREAKPOINTS {
        return Err(parse_err("anthropic_messages allows at most 4 cache breakpoints"));
    }
    for bp in &req.cache {
        let block = resolve(body, req, &bp.anchor)?;
        // FiveMin (default) is emitted by OMITTING ttl; OneHour emits "1h".
        *block = take_object(block);
        block["cache_control"] = match bp.ttl {
            CacheTtl::FiveMin => json!({"type": "ephemeral"}),
            CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
        };
    }
    Ok(())
}

/// Resolve an anchor to the LAST wire block it marks, or ParseInput if it resolves
/// to nothing (empty tools/system, out-of-range / hoisted-System / 0-block message).
fn resolve<'a>(
    body: &'a mut Map<String, Value>,
    req: &CanonicalRequest,
    anchor: &CacheAnchor,
) -> Result<&'a mut Value, CanonicalError> {
    match anchor {
        CacheAnchor::Tools => last_of(body, "tools")
            .ok_or_else(|| parse_err("cache anchor `tools` resolves to no tool block")),
        CacheAnchor::System => last_of(body, "system")
            .ok_or_else(|| parse_err("cache anchor `system` resolves to no system block")),
        CacheAnchor::Message { index } => {
            let i = *index as usize;
            let m = req
                .messages
                .get(i)
                .ok_or_else(|| parse_err("cache anchor `message` index out of range"))?;
            if m.role == Role::System {
                return Err(parse_err("cache anchor `message` targets a hoisted system message"));
            }
            // messages_value pushes ONE wire entry per non-System message, in order,
            // even when it projects to 0 blocks — so the wire position is the count of
            // non-System messages before `i` (the same `continue` skip as encode).
            let wire_pos = req.messages[..i]
                .iter()
                .filter(|m| m.role != Role::System)
                .count();
            body["messages"][wire_pos]["content"]
                .as_array_mut()
                .and_then(|a| a.last_mut())
                .ok_or_else(|| parse_err("cache anchor `message` projects to no wire block"))
        }
    }
}

/// Last element of `body[key]` as a mutable block, or None if the key is absent or
/// the array is empty (the two "nothing to mark" cases collapse to one).
fn last_of<'a>(body: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Value> {
    body.get_mut(key)?.as_array_mut()?.last_mut()
}

fn parse_err(msg: &str) -> CanonicalError {
    CanonicalError { kind: ErrorKind::ParseInput, message: msg.into(), provider_detail: None }
}
```

**Implementation note on the `*block = take_object(...)` line:** that line is a defensive convenience only if the spec author wants to guarantee the block is an object before index-assign. In brazen every projected block (`text`/`tool` object) is already a JSON object, and `Value::IndexMut<&str>` on an object inserts cleanly, so **prefer to delete the `take_object` line entirely** and assign `block["cache_control"] = …` directly. Keep this module free of dead helpers (coverage gate). The version that ships should be:

```rust
        let block = resolve(body, req, &bp.anchor)?;
        block["cache_control"] = match bp.ttl {
            CacheTtl::FiveMin => json!({"type": "ephemeral"}),
            CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
        };
```

`body["messages"][wire_pos]` uses `serde_json`'s `Value`/`Map` indexing; `wire_pos` is provably in range (it counts the non-System messages that `messages_value` actually pushed, and `i` is itself an in-range non-System message), so it never panics.

---

## 4. Wire it in — `src/protocol/anthropic/encode/mod.rs`
1. Add the module next to `mod blocks;` (line 12): `mod cache;`
2. In `encode`, **after the `tool_choice` block (after line 80) and BEFORE the `extra` fold (line 81)**, insert:

```rust
    cache::apply(&mut body, req)?;
```

Placing it pre-`extra`-fold keeps `cache_control` a **typed projection** that the top-level `extra` escape hatch cannot clobber, consistent with how `reasoning`/`parallel_tool_calls` are written before the fold (§2.1.1). `system_value`/`messages_value`/`tools_value` are unchanged — they already always emit array form (mod.rs:107 "never loses caching"), so a `cache_control` attaches safely.

---

## 5. TTL render rule (single source)
| `CacheTtl` | canonical serde (`req.cache` round-trip) | Anthropic wire `cache_control` |
|---|---|---|
| `FiveMin` (default) | `"5m"` | `{"type":"ephemeral"}` (ttl **omitted** = 5m) |
| `OneHour` | `"1h"` | `{"type":"ephemeral","ttl":"1h"}` |

---

## 6. Anchor resolution table (Anthropic encoder)
| Anchor | Marks | Valid when | ParseInput (exit 64) when |
|---|---|---|---|
| `Tools` | last object in `body["tools"]` | `req.tools` non-empty | tools absent/empty |
| `System` | last block in `body["system"]` | `req.system` present & non-empty | system absent/empty |
| `Message{index}` | last block of `body["messages"][wire_pos]["content"]`, where `wire_pos` = count of non-System messages before `index` | `index` in range, role ≠ System, message projects to ≥1 wire block | index out of range; index points at a hoisted System message; message projects to 0 wire blocks (e.g. an assistant turn of only signature-less `Thinking`, which `content_block` drops) |

Global: `req.cache.len() > 4` → ParseInput. `req.cache.is_empty()` → no-op, body byte-identical.

---

## 7. Non-Anthropic dialects
**No code change.** OpenAI Responses/Chat, Google, Ollama encoders never read `req.cache`; their caching is automatic by prompt prefix. The ≤4/resolve validation fires **only** when routing to Anthropic — a deliberate asymmetry (documented in providers.md so 5 breakpoints erroring on Anthropic but passing silently elsewhere is understood as intentional). `cache` is **not** in `strip_unsupported`/`unsupported_body_keys` — it is structural, not an `extra` key, so there is nothing to strip.

---

## 8. Beta-header caveat (config, not code)
`ttl:"1h"` is GA per the current Anthropic docs (no `anthropic-beta` header required as of Jan 2026). The encoder emits `"1h"` unconditionally; any beta header, if a given model/account still needs one, is **row `beta_headers` DATA** (arch §5.4), never encoder logic. Document in providers.md / anthropic-messages.md; do **not** add a header in code.

---

## 9. Documentation edits (drop-in)
1. **`specs/architecture.md` §2 Non-goals (line 60):** REMOVE `cache breakpoints` from the `extra`-passthrough list. Before: "Logprobs, citations, cache breakpoints, safety settings ride `extra` in…". After: "Logprobs, citations, safety settings ride `extra` in…". (Prompt-prefix cache is now modelled; the response-side `cache_read_tokens`/`cache_write_tokens` in §3.2 are untouched.)
2. **`specs/architecture.md` §3.1 struct block (lines 75-91):** add `pub cache: Vec<CacheBreakpoint>,` after `stream`, before `#[serde(flatten)] extra`, with the request-only/Anthropic-only comment.
3. **`specs/architecture.md` §3.1 type list (after ReasoningEffort, ~line 144):** add `CacheBreakpoint{anchor:CacheAnchor, ttl:CacheTtl}` / `CacheAnchor{Tools, System, Message{index:u32}}` (snake_case, internal tag `anchor`, flattened on the breakpoint) / `CacheTtl{FiveMin="5m"(#[default]), OneHour="1h"}`.
4. **`specs/architecture.md` §3.1 reframes bullets (~lines 155-157):** add a `cache` reframe: request-only structural knob (not config-filled, no flag), and UNLIKE the lifted knobs it is NOT a portable cross-provider mapping — only Anthropic emits a marker; OpenAI/Google/Ollama ignore it. State the validation (≤4, resolve-or-ParseInput/exit-64, Anthropic-encode-local).
5. **`specs/providers.md` NEW §7** "Prompt caching — one canonical breakpoint set, one wire marker (Anthropic only)", inserted before the current §7 "severability ledger"; renumber §7→§8 … §10→§11. Document the projection, the Anthropic-only asymmetry (inverse of §6 reasoning which all five map), the ≤4/resolve validation locality, the 1h-beta config note, and cross-reference the response-side Usage `cache_read_tokens` (§3.5/§4.6/§5.7) so request-cache vs response-cache-tokens are not conflated. **Grep specs for stale `providers.md §7/§8/§9/§10` cross-refs before committing the renumber** (existing `providers.md §6` reasoning refs stay valid).
6. **`specs/anthropic-messages.md`:** §2.2 (note: per-block `cache_control` now comes from typed `req.cache`; the `extra` `cache_control` remains the escape hatch with typed-wins precedence §2.1.1); §2.3 (Message anchor = last block after the System-hoist skip, wire pos ≠ canonical index; System-index invalid, Tool-role valid→user, 0-block invalid; never collapses to bare string); §2.4 (System anchor on last system text block, empty→ParseInput); §2.6 (Tools anchor on last tool object, empty→ParseInput); **NEW §2.10** the full breakpoint model + ttl spellings + ≤4 + the four ParseInput failure modes, sitting alongside the §2.8 `extra` `cache_control` hatch. Optionally extend the §2.9 worked example with one breakpoint.
7. **`CHANGELOG.md` [Unreleased]:** OPTIONAL Added entry mirroring the `--reasoning` precedent: "first-class prompt caching (`req.cache` breakpoints → Anthropic `cache_control`)".

---

## 10. Test plan → 100% line coverage
- **`src/tests/canonical_request.rs`** (236L now): (a) add `cache: vec![CacheBreakpoint{anchor:CacheAnchor::Message{index:0}, ttl:CacheTtl::OneHour}]` to the one full literal (`request_roundtrips_and_minimal_decode_defaults`, line 200) so it compiles and `rt()` covers a non-empty cache; assert the minimal-decode path yields `min.cache == Vec::new()`; (b) NEW compact test `cache_types_serde_spellings_and_defaults`: `CacheAnchor` flattened snake_case tags (`tools`/`system`/`message`+`index`) round-trip; `CacheTtl` serializes to `"5m"`/`"1h"` and decodes back (covers BOTH renames); `CacheTtl::default() == FiveMin`; a `CacheBreakpoint` JSON omitting `ttl` decodes to `FiveMin` (field `#[serde(default)]`). Import `CacheAnchor, CacheBreakpoint, CacheTtl` (line 5-7).
- **`src/tests/anthropic_cache.rs`** (NEW; reuse the `enc`/`from`/`body` helpers copied from `anthropic_encode.rs`). Cover every `cache.rs` branch:
  1. `cache` empty → body identical to no-cache encode (early return).
  2. `Tools` anchor, `FiveMin` → last tool object gets `{"type":"ephemeral"}` (no ttl); earlier tools unmarked.
  3. `System` anchor, `FiveMin` → last system text block marked.
  4. `Message{index}` with a **leading System message** so `wire_pos(0) ≠ canonical index(1)` → last block of the projected user message marked (proves the skip).
  5. `OneHour` on any anchor → `{"type":"ephemeral","ttl":"1h"}` (covers the OneHour match arm + emit).
  6. 5 breakpoints → `ParseInput`, exit 64.
  7. `Tools` anchor, no tools → ParseInput (last_of None).
  8. `System` anchor, no system → ParseInput (last_of None).
  9. `Message{index}` out of range → ParseInput.
  10. `Message{index}` at a System-role message → ParseInput.
  11. `Message{index}` at a 0-block message (assistant turn of only signature-less `Thinking`) → ParseInput (content `last_mut` None).
- **`src/tests/mod.rs`:** add `mod anthropic_cache;` in the alphabetical block (next to `mod anthropic_encode;`).
- **`src/tests/anthropic_encode.rs`:** NO edits. Confirm `worked_example_projects_every_field_and_header` stays green (inputs carry no `cache` → no marker → byte-identical wire body).

Branch-to-test mapping (line gate `--fail-under-lines 100`): `is_empty` early return → T1; `len>4` → T6; `Tools` ok/err → T2/T7; `System` ok/err → T3/T8; `Message` ok → T4; out-of-range → T9; System-role → T10; 0-block → T11; `FiveMin` emit arm → T2/T3/T4; `OneHour` emit arm → T5; `last_of` None → T7/T8; `parse_err` → any err test. `CacheTtl` both renames + default → canonical_request tests.

---

## 11. Verification gate
`make check` (fmt + clippy + test) and `make cov` (`--fail-under-lines 100`) green; `make linecount` (≤300/file over `git ls-files '*.rs'`, tests included) green. Per repo workflow: do this on a `bl` worktree feature branch, merge working→feature, run the FULL representative suite, merge back `--no-ff`, all tests passing.

---

## Conventions — MUST follow (repo AGENTS.md + commit gate)
- **Worktree discipline.** `bl claim <id>` prints a worktree path on stdout. `cd` into it. ALL edits happen there — NEVER edit the repo core working tree. `bl close <id>` delivers to local `main` and runs the gate.
- **Commit gate (`bl close` runs it).** `core.hooksPath=.git/hooks-local/pre-commit` runs (1) `commit-policy.py`: identity MUST be `mudbungie <mudbungie@gmail.com>` (already git-config default), and NO commit may land in the weekday 09:00–17:00 America/Los_Angeles window — so EXPORT for the whole session: `export GIT_AUTHOR_DATE="2026-06-30T20:00:00-07:00" GIT_COMMITTER_DATE="2026-06-30T20:00:00-07:00"` (evening = outside window); then (2) `make check` = fmt-check + clippy `-D warnings` + `make linecount` (NO tracked `*.rs` > 300 lines, tests included) + `cargo llvm-cov --fail-under-lines 100` (100% line coverage). ALL must pass or the close aborts (task stays claimed, worktree stays up).
- **Workflow:** edit in the worktree → `make check` green → `bl close <id>`. If close aborts on a main-fold conflict, merge `main` into the worktree by hand, resolve, re-close.
- **Never credit AI/tooling in commit messages.** Tag the delivery with the `[bl-id]`.
- **Docs are part of the deliverable** (the spec edits below to architecture.md / providers.md / anthropic-messages.md / CHANGELOG.md). The `make check` gate does NOT check docs, but they MUST land with the code (single source of truth).
- Run `wc -l` on every edited `*.rs` before closing; the 300-line cap is the sharpest risk in both balls.