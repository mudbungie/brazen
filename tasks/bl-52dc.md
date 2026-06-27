+++
title = "CLI interface review: nail the bz surface before 0.1.0 — evaluate control verbs→flags (bz --login) and --raw=in/out directionality"
created = 1782588030
updated = 1782590283
claimant = "Sauteing"
priority = 4
tags = ["interface-review", "design"]
+++
## Context

One-way-door review (2026-06-27). The CLI is the real product, so its surface is the
hardest door to change post-ship. Two specific shapes to settle:

1. **Verb-vs-prompt namespace collision.** `src/main.rs:39` dispatches on `argv[0]`:
   `login` and `list-models` are verbs, everything else is a positional prompt. So
   `bz login` can NEVER be the prompt "login", and EVERY future top-level verb permanently
   shrinks the set of bare prompts that work (someone running `bz "models"` today breaks
   the day we add `bz models`). Our own AGENTS.md: "New flags/config/verbs are a smell."
   Owner's idea: move control verbs to flags — `bz --login`, `bz --list-models` — which
   removes the collision and KEEPS the charismatic `bz "what is 1+1"` simplicity (a bare
   word is then always a prompt, never a verb). Evaluate impact: does anything depend on
   the verb spelling? the login / list-models arg shapes? help text? model-discovery.md?

2. **`--raw` is symmetric input+output** (architecture.md §5.4). Owner's idea: let the flag
   take an optional key — `--raw=out` / `--raw=in` for unidirectional rawness, bare `--raw`
   = both. Evaluate feasibility + whether any real use-case needs the split (or whether
   symmetric-only is fine and we just document the limitation).

## Decision (owner)

Do a focused CLI interface review to confirm we've nailed it before 0.1.0. Be parsimonious
about interface width, but acknowledge there IS a legitimate control surface.

## Deliverable

A design note (extend interactive-output.md, or a new CLI section in architecture.md —
living doc) that:
- settles the verb-vs-flag question (recommendation + rationale + migration if we move
  control verbs to flags);
- settles `--raw` directionality;
- enumerates the FULL committed CLI surface (flags, control ops, output modes, input
  channels, exit codes) as the frozen-at-0.1.0 contract;
- states the rule that keeps the bare-prompt namespace from ever shrinking again.

Type: **DESIGN**.