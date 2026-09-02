+++
title = "lift service_tier as the fifth canonical knob: priority processing is request shaping, not an extra-valve key"
created = 1788321293
updated = 1788321342
claimant = "Forge2"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
**The ask (operator, 2026-09-01).** Seat surfaces up the stack want a priority checkbox — spend the provider's priority/fast-token lane on a role's model calls. litany will carry `priority: true` on a role assignment (litany's board has that ball, gated on this one) and must set it on the typed canonical request it pipes to `bz` — litany builds `CanonicalRequest` structs, so the fail-open `extra` map is deliberately unreachable from there. A knob litany can set must be lifted.

**Today `service_tier` has no canonical home.** It is named only as an extra-valve key (`specs/openai-chat-mapping.md` "keys with no canonical home … `service_tier` …"; `src/ingress/anthropic_messages/decode.rs` forwards it verbatim), the response-side echo is dropped ("are ignored (no canonical home)"), and the only setter is a row's `body_defaults` — per-row, one spelling, no typed precedence, invisible to a typed caller.

**Design.** A fifth lifted knob beside `reasoning` (`specs/architecture.md` §3.1's family):

- `CanonicalRequest.service_tier: Option<ServiceTier>`, `#[serde(default)]`, `#[non_exhaustive] enum ServiceTier { Priority, Standard }` (serde lowercase). An enum, not a bool — the AGENTS rule ("a bool that is really 'is this fact present' is a lossy projection — widen it to carry the value"): OpenAI speaks `default|flex|priority`, Anthropic `auto|standard_only`; new variants (e.g. `Flex`) are additive later. `None` = absent = the provider's default lane, the key omitted.
- Projections, one arm per dialect (the `reasoning` pattern, `specs/providers.md` §6's shape):
  - openai_chat + openai_responses: `"service_tier": "priority"` / `"default"`.
  - anthropic_messages: `Priority` → `"service_tier": "auto"` (Anthropic has no request-side priority demand — priority is org provisioning, `auto` spends it when provisioned), `Standard` → `"standard_only"`. Record this asymmetry in the spec.
  - google_genai / ollama_chat / claude_code: no wire spelling — documented narrowing (drop), exactly the `output` knob's narrowings (`specs/providers.md` §6.1).
- `strip_unsupported` gains a `"service_tier"` arm (`src/config/resolved.rs`) so a row's `unsupported_body_keys` can decline it — a new typed knob without that arm cannot be opted out of.
- Precedence unchanged by construction: request > flag > env > file > row `body_defaults` > encoder baseline; `fill_absent` gains the one `Option::or` line. The full plumbing checklist is the `--reasoning`/`--temperature` trace: `src/cli/parse.rs` (`--tier <priority|standard>`), `src/config/env.rs` (`BRAZEN_TIER`), `src/config/partial/mod.rs` + `partial_de.rs`, `src/config/dump.rs` (or `--dump-config` silently drops it), `src/config/resolve/mod.rs`, `src/lib.rs` `pub use` (or `tests/interface_parity.rs` fails), `src/run/discovery.rs` HELP (note: `--reasoning` is missing from HELP today — fix both while there), ingress: `openai_chat/decode.rs` lifts wire `service_tier` → the knob; `anthropic_messages/decode.rs` moves it off the extra-passthrough list.
- **Goldens:** an additive `Option` field serializing `null` changes every `golden_v1_requests.jsonl` line — that is the sanctioned grows-only move: regenerate the fixtures, no `EVENT_SCHEMA_VERSION` bump.
- **The 300 cap bites first:** `src/canonical/request.rs` and `src/protocol/mod.rs` are both AT 300. Split before adding (precedent bl-2704 split `openai_responses/encode.rs` at exactly 300).
- Response-side `service_tier` echo stays dropped for now — note it in the spec rather than widening Usage here.

**The anti-flag argument, pre-made.** The `--reasoning` precedent (`specs/architecture.md`: supersedes the earlier no-flag ruling — "reasoning is table-stakes") is the template: priority spend is the same class of caller-facing generation shaping, and `body_defaults` stays the exact-shape escape hatch.

**Vocabulary note for the stack:** upstream configs speak "priority" (a checkbox fact); this crate speaks the wire's `service_tier` with `priority` as a value. Adapters translate; neither word leaks the other way.

**Release:** litany's `priority:` ball consumes the next published version — publish after landing.