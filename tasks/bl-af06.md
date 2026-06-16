+++
title = "Registry dispatch as total exhaustive match (kill unregistered-id panic)"
created = 1781652729
updated = 1781652729
priority = 56
tags = ["impl"]
+++
# Make Registry dispatch a total exhaustive `match` (kill the unregistered-id panic)

## Problem (why this exists)

`src/registry.rs` builds two runtime `HashMap`s by hand-written `insert`s, and the
lookups return `Option`:

```rust
pub fn protocol(&self, id: ProtocolId) -> Option<&'static dyn Protocol> { self.protocols.get(&id).copied() }
pub fn auth(&self, id: AuthId) -> Option<&'static dyn Auth> { self.auths.get(&id).copied() }
```

`src/run/mod.rs` (the spine) unwraps both with `.expect(...)`:

```rust
#[allow(clippy::expect_used)]
let proto = registry.protocol(cfg.provider.protocol)
    .expect("every ProtocolId is registered in Registry::builtin");
#[allow(clippy::expect_used)]
let auth = registry.auth(cfg.provider.auth)
    .expect("every AuthId is registered in Registry::builtin");
```

Nothing forces `builtin()` to insert every enum variant. So a future task that adds
a `ProtocolId`/`AuthId` variant **and a provider row that uses it** but forgets the
`builtin()` insert will:
- compile (a `HashMap` insert is not exhaustiveness-checked),
- pass `clippy -D warnings`,
- pass the **100% line-coverage gate** (the `.expect(...)` line is static and its
  panic arm lives in stdlib `Option::expect`, so coverage never drops),
- pass that provider's own encode/decode unit tests (they call the protocol
  directly, never through `run`),

…and then **panic at runtime** on `bz --provider thatone` instead of a clean exit.

This is not hypothetical: it has already silently rotted. `tests/seams_protocol.rs`
`registry_builtin_dispatches_by_id` still asserts only 2 of the 5 protocols
(`OpenAiChat`, `AnthropicMessages`) even though `OpenAiResponses`, `GoogleGenAi`,
and `OllamaChat` have since landed. Nothing failed when those were added.

## Fix (what to do)

Dissolve the failure mode instead of detecting it: replace the runtime `HashMap`
with a **total, exhaustive `match`** that returns the impl directly — no `Option`,
no `.expect()`, no panic, no dead branch. Then the compiler enforces "every variant
has an impl" at the source (add a variant → `registry.rs` fails to compile until
you add the arm), and a per-variant test gives arm coverage that the gate backs.

### Reference implementation — `src/registry.rs` (full)

```rust
//! The dispatch seam (arch §4.4): `Registry` maps a `ProtocolId`/`AuthId` to a
//! shared `&'static dyn` impl by a TOTAL match over the closed key-enum — never a
//! match on a vendor name (§4.4). The match is the single source of truth: adding
//! a protocol/auth variant fails to compile here until its arm is added, so the
//! id-set and the impl-set cannot drift and an "unregistered id" is unrepresentable
//! (no `Option`, no runtime panic).

use crate::auth::{Auth, OAuth2Auth, StaticSecretAuth};
use crate::config::provider::{AuthId, ProtocolId};
use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::protocol::Protocol;

/// The protocol/auth dispatch seam (arch §4.4). A zero-field handle whose two
/// methods are total matches over the registry-key enums.
pub struct Registry;

impl Registry {
    /// The built-in dispatch tables. A new protocol/auth = ONE match arm + ONE
    /// module (and its `ProtocolId`/`AuthId` variant). Nothing else.
    pub fn builtin() -> Self {
        Registry
    }

    /// The protocol impl for a resolved row's `ProtocolId` — a TOTAL match on the
    /// closed key-enum (exhaustiveness is the registration guarantee), never a
    /// match on a vendor name.
    pub fn protocol(&self, id: ProtocolId) -> &'static dyn Protocol {
        match id {
            ProtocolId::OpenAiChat => &OpenAiChat,
            ProtocolId::AnthropicMessages => &AnthropicMessages,
            ProtocolId::OpenAiResponses => &OpenAiResponses,
            ProtocolId::GoogleGenAi => &GoogleGenAi,
            ProtocolId::OllamaChat => &OllamaChat,
        }
    }

    /// The auth impl for a resolved row's `AuthId`. `api_key` and `bearer` share
    /// one `StaticSecretAuth` (two names, one impl — auth §3.1); `oauth2` is
    /// `OAuth2Auth`.
    pub fn auth(&self, id: AuthId) -> &'static dyn Auth {
        match id {
            AuthId::ApiKey | AuthId::Bearer => &StaticSecretAuth,
            AuthId::OAuth2 => &OAuth2Auth,
        }
    }
}
```

(NOTE: confirm the exact module paths / impl type names against the current tree
when you start — the import block above is copied from `main` as of filing, but
the provider fleet is moving fast and more `ProtocolId` variants may have landed.
If so, **add their arms too** — that is exactly the guarantee this change buys.)

### Call sites to update (these break when the return type drops `Option`)

1. `src/run/mod.rs` (~lines 142–153): delete both `#[allow(clippy::expect_used)]`
   and the `.expect(...)` calls; becomes:
   ```rust
   let registry = Registry::builtin();
   let proto = registry.protocol(cfg.provider.protocol);
   let auth = registry.auth(cfg.provider.auth);
   ```
   Update the adjacent comment (it currently explains the expect invariant).
2. `tests/auth_apply.rs` lines 69 and 90: drop the trailing `.unwrap()`:
   `Registry::builtin().auth(brazen::AuthId::ApiKey)` (and `Bearer`).
3. `tests/seams_protocol.rs` `registry_builtin_dispatches_by_id` (~lines 123–130):
   it is STALE (2 of 5 protocols) — **replace** it with a per-variant resolve test
   that exercises every arm (so coverage backs completeness), e.g.:
   ```rust
   #[test]
   fn registry_resolves_every_key() {
       let reg = Registry::builtin();
       for id in [
           ProtocolId::OpenAiChat, ProtocolId::AnthropicMessages,
           ProtocolId::OpenAiResponses, ProtocolId::GoogleGenAi, ProtocolId::OllamaChat,
       ] {
           let _: &dyn Protocol = reg.protocol(id); // arm must be covered
       }
       for id in [AuthId::ApiKey, AuthId::Bearer, AuthId::OAuth2] {
           let _: &dyn Auth = reg.auth(id);
       }
   }
   ```
   (The compiler now enforces completeness in `registry.rs`; this test forces every
   arm to be *executed* — forget to list a new variant here and its arm is
   uncovered → 100% gate fails → caught.) Keep whatever this test file imports;
   it already uses `ProtocolId`/`AuthId`. The type may need `use brazen::{Protocol, Auth};`.

### Spec change — `specs/architecture.md` §4.4 (living doc; update it, do not leave it contradicting the code)

- Replace the `pub struct Registry { protocols: HashMap<…>, auths: HashMap<…> }` +
  `builtin()` `insert` sketch (around the "Dispatch with NO match-on-provider"
  block) with the total-match form above.
- The prose "adding a protocol = ONE insert + ONE enum arm + ONE module" → "ONE
  match arm + ONE module" (the enum arm and the match arm are now the same edit).
- §4.6 severability bullet that says "one `Registry::builtin()` insert" → "one
  match arm".
- Keep the load-bearing rule intact and clarify it: the core still **never matches
  on a vendor name**; a *total match over the closed `ProtocolId`/`AuthId`
  key-enum* is the exhaustiveness guarantee (compiler-enforced completeness), not a
  vendor branch — strictly more in the spirit of §4.4 than a partial runtime map.

## Acceptance criteria

- `make check` green: `fmt-check` + `clippy --all-targets -D warnings` + `cargo
  llvm-cov --fail-under-lines 100` (bin shim excluded).
- The registry lookup path contains no `Option`, no `.expect()`, no `.unwrap()`,
  no `#[allow(clippy::expect_used)]`, and no dead `None` branch.
- No `*.rs` file exceeds 300 lines.
- Sanity check the guarantee: temporarily add a dummy `ProtocolId` variant and
  confirm `cargo build` FAILS in `registry.rs` (then revert). This is the whole
  point — verify it, do not just assert it.
- Spec §4.4 (and the §4.6 bullet) updated to describe match-based dispatch.

## Gotchas / workflow notes

- The repo ROOT checkout (`/home/mark/dev/brazen`) goes stale after `bl close`
  (delivery moves `main` by plumbing). Do all work in the `bl claim` worktree; read
  authoritative content there (it merges `main`) or via `git show main:<path>`.
- **Merge `main` into your worktree first**, and again before `bl close` — the
  provider fleet is landing in parallel; new `ProtocolId`/`AuthId` variants may
  appear, and if they do you must add their match arms (which is the feature).
- Standard flow: `bl claim` → merge `main` → edit → `make check` (all pass) →
  commit (NO AI/tooling credit in the message) → `bl close` (its pre-commit hook
  re-runs the full gate before squashing to `main`).
- Background: this was surfaced reviewing the run() spine (bl-ea70). The current
  `.expect()` was a deliberate-but-flawed choice — a graceful `ok_or → 78` was
  impossible to cover (the `None` arm is unreachable, so it'd be a dead line that
  fails the 100% gate). The total-match form removes that dilemma entirely: the
  branch doesn't exist, so there's nothing to cover and nothing to panic.