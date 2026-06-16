+++
title = "--thinking output projection: TextSink thinking-deltas + separator"
created = 1781595179
updated = 1781599105
claimant = "Stationery"
priority = 82
tags = ["impl"]

[[blockers]]
id = "bl-91b0"
on = "claim"
+++
Spec-d in architecture.md §5.3 / §5.5 and resolved decision #6, but currently untracked (bl-faf3 landed TextSink as text-deltas-only per its task scope; bl-91b0 OutputMode is Ndjson|Text|Raw only). --thinking is a text-mode variant: emit ContentDelta::ThinkingDelta text BEFORE the answer, then a single newline separator at the first non-thinking content, then the TextDelta answer (bz "2+2" --thinking -> "...reasoning...\n4"). Event::Error still goes to stderr. OutMode in §5.1 is {Text,Ndjson,Raw}; --thinking is a FLAG on the text projection, not a 4th OutMode -> extend TextSink (a thinking bool that gates ThinkingDelta + the one-time separator), and wire the --thinking flag through the flag/OutputMode layer (bl-91b0). Table-test from literal Event streams: thinking-then-text emits the separator exactly once; text-only emits none. Severs cleanly: deleting the flag + branch removes it.