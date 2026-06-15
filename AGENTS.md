# AGENTS.md — brazen

Conventions for anyone (human or agent) working in this repo.

## Workflow

- **Design first, implement second.** A capability begins as a spec in `specs/` (a living
  document, edited like code), then implementation follows.
- **Task tracking is `bl` (balls).** Run `bl prime --as <you>` at session start; `bl list`
  shows ready work. Claiming a task materializes a `work/<id>` worktree — **all edits happen
  there**. `bl close` delivers the worktree to `main` and runs the pre-commit gate.
- **Never edit `main` directly.** Always work in a `bl` worktree and let `bl close` deliver.
- **Never credit AI or tooling in commit messages.**

## Hard rules (enforced by `.githooks/pre-commit`)

- **100% line coverage** (`cargo llvm-cov --fail-under-lines 100`) once Rust sources exist.
- **No code file (`*.rs`) exceeds 300 lines.** Docs (`*.md`) and config (`*.toml`, …) are exempt.
- Enable the gate once per clone: `make hooks` (sets `core.hooksPath`).

## Architecture north stars

- **Single source of truth.** The canonical model is authoritative; protocols derive from it.
  Don't store what you can compute.
- **Minimize and deepen the interface.** Components meet only at it, never pairwise.
- **Dissolve special cases** into the general path with empty inputs. A new flag/config/verb
  is a smell — prefer an existing explicit signal.
- **Severability.** Removing a capability should delete config, not edit core code.
- **If it can't be tested, it isn't built.**

See `specs/` for the architecture; start with spec `0001`.
