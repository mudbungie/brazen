+++
title = "Implement --reasoning <effort> request knob: canonical effort enum (low|medium|high) mapped per-protocol to each provider's native reasoning shape; supersedes bl-839c's no-flag decision"
created = 1782719445
updated = 1782719925
claimant = "Forge"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["feature"]
+++
User decision (2026-06-29): reasoning is table-stakes; reopen bl-839c's 'no flag' wontfix. LOCKED CLI surface: a SEPARATE `--reasoning low|medium|high` request flag; the existing `--thinking` stays DISPLAY-ONLY (unchanged). Abstraction = canonical EFFORT enum (portable user intent), mapped per-protocol; exact token budgets remain available via body_defaults (the escape hatch, bl-839c).

DESIGN (living docs first, then implement):
- Canonical: add a TYPED field reasoning: Option<ReasoningEffort> to CanonicalRequest (NOT extra/flatten — the whole point is canonical->per-protocol mapping). ReasoningEffort { Low, Medium, High }, serde rename lowercase. Document in architecture.md §3.1.
- CLI/config: `--reasoning <level>` in cli.rs -> cfg.reasoning; add the PartialConfig field, BRAZEN_REASONING env (config_env.rs), resolve fold + fill_absent into req.reasoning. Mirror the EXACT pattern of an existing knob (max_tokens/temperature) end to end.
- Per-protocol encode mapping (each encode maps req.reasoning to its wire shape — the protocol owns its dialect):
  * openai_responses -> reasoning:{effort:"..."}
  * openai_chat      -> reasoning_effort:"..."
  * anthropic        -> thinking:{type:"enabled",budget_tokens:N} via an effort->budget table; ALSO satisfy Anthropic's constraints (budget>=1024; max_tokens MUST exceed budget_tokens; temperature must be unset/1 with thinking). CONSULT the claude-api skill for exact extended-thinking params/constraints and encode correctly (bump max_tokens to budget+output-headroom if needed, or document). Attack this coupling in the spec.
  * google_genai     -> thinkingConfig:{thinkingBudget:N, includeThoughts:true} via the effort->budget table
  * ollama_chat      -> Ollama 'think' bool (any effort -> think:true) or no-op; decide + note
- Opt-out: a backend that rejects reasoning (e.g. Mistral on openai_chat) lists the CANONICAL key 'reasoning' in unsupported_body_keys so strip_unsupported drops it pre-encode. Verify strip operates on the canonical field name; note in config.md.
- Display: --thinking still gates ThinkingDelta display in text mode; reconcile architecture.md §5.3 (the bl-839c boundary text) to introduce --reasoning as the REQUEST knob (body_defaults stays the exact-budget escape hatch) — evolve the decision coherently, don't bluntly contradict it.

CONSTRAINTS: 100% line coverage; every *.rs <= 300 lines (encoders are near the cap — decompose if a mapping pushes one over); make check green. Table-test each protocol mapping + flag/env/config resolution.

FILE SCOPE: src/canonical/request.rs (+request_de.rs if needed), src/cli.rs, src/config/{partial.rs,partial_de.rs?,resolve.rs,resolved.rs,config_env.rs}, src/protocol/{anthropic,openai,openai_responses,google_genai,ollama_chat}/encode*, specs/{architecture.md §3.1+§5.3, config.md, providers.md}, tests. NOTE: a concurrent lane (the -f/--file task) is editing src/cli.rs and specs/architecture.md §5.5 — fold main + resolve (keep BOTH flag arms / BOTH spec sections) at close.