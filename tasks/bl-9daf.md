+++
title = "--input: evaluate making it an extensible list (multi-file request composition)"
created = 1782708179
updated = 1782712902
claimant = "Lathe"
priority = 3
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["design"]
+++
Enhancement surfaced reviewing the shipped CLI.

WHAT EXISTS: --input is a single Option<PathBuf> (cli.rs:185), last-wins if repeated. It is a stdin REPLACEMENT — read the one canonical request document from this file instead of stdin. The whole input model is one request: positional prompt XOR one stdin/file (run/mod.rs read_request). No list, no concat.

THE ASK: make --input an extensible list — accept multiple files (repeated --input, and/or a value list) that compose into the request.

OPEN DESIGN QUESTIONS (resolve before building — per AGENTS.md 'attack a design before committing it'):
  1. What does composing N request documents MEAN? Concatenate message arrays in order? Merge top-level fields (which wins)? This is the crux — 'a list of inputs' has no obvious single semantics, and an ambiguous one will drift (single-source-of-truth).
  2. Interaction with the positional prompt and stdin: today prompt XOR stdin/file is a clean invariant (a present positional wins, stdin unread). A list muddies the XOR. Does prompt still win? Does stdin join the list?
  3. Is this actually a CLI concern, or does it belong INSIDE the one canonical request (the message array already composes turns)? AGENTS.md: 'a special case is usually a missing reframe'; 'build less'; new flags are a smell. The current single-document model may already be the right narrow interface, with multi-part composition living in the request JSON the caller assembles.

Deliverable: a decision in the arch doc (input model, §5.5) first; code only if multi-input earns its keep over composing in the request body. Likely a 'wontfix / reframe' candidate — file it so the question is recorded, not lost.