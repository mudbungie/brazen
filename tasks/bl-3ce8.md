+++
title = "Evaluate exit-code granularity: keep all 4xx (incl 429)→69 with retryable as discriminant, or split?"
created = 1782588037
updated = 1782589645
claimant = "plane"
priority = 3
tags = ["interface-review", "design"]
+++
## Context

One-way-door review (2026-06-27). architecture.md §8: all provider 4xx (including 429
rate-limit) map to exit 69; the rate-limit / retryable distinction lives in the COMPUTED
`retryable()` query and in `provider_detail`, not in a unique exit code ("a new code would
be a second home for is-it-retryable"). Owner asked: should we just split the exit codes?

## Decision (owner): evaluate.

## Deliverable

A short design decision (architecture.md §8). Considerations:
- Exit codes are for SHELL-LEVEL control flow; granularity is already available in `--json`
  (error `kind` carries `Provider{status}`; `provider_detail` carries the raw body).
- sysexits has no natural rate-limit code; a custom code re-homes the `retryable` fact,
  which §8 deliberately keeps as a single computed query.
- Reviewer's lean: KEEP coarse (4xx→69) and surface granularity via `--json`, per the
  single-source-of-truth argument already in §8 — but confirm no shell consumer genuinely
  needs to branch on 429 by exit code. If one does, the cheapest split is a single distinct
  "retryable provider error" code, NOT a per-status fan-out.

Type: **DESIGN** (eval + decision; record the outcome even if it's "no change").