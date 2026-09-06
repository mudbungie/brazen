+++
title = "bz cannot borrow the Codex CLI's sign-in: the openai-chatgpt row has no ambient source, so a stale bz --login token refuses every ChatGPT request while codex itself is signed in"
created = 1788673232
updated = 1788674093
claimant = "Cantaloups-B1"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["usability-r1"]
+++
Observed: `bz --provider openai-chatgpt -m gpt-5 'say ok'` answers `token refresh failed; re-run bz --login --provider <id> if this persists` while `codex login status` on the same box answers `Logged in using ChatGPT`. The bz-owned credential (from a bz --login weeks earlier) holds a refresh token the vendor has since rotated under codex's own logins, so bz's refresh is refused for good.

Shape that fits auth §5.5: the openai-chatgpt row gains an `ambient` block naming a new pure parser for the Codex CLI's credential file (`~/.codex/auth.json`: `tokens.access_token`, `tokens.refresh_token`, `tokens.account_id`, expiry from the JWT `exp` claim or `last_refresh`), read only on a store miss, borrowed = read-only (never refreshed, never persisted, per §5.5 'borrowed means read-only'). Same mechanism as the shipped `claude_code` format, deleted by deleting the row's line.

Why it matters: the Codex sign-in is the free-tier route on this box; without it every litany role that names a gpt model is dead while the box is demonstrably signed in. Consider also: when the store hit is stale AND an ambient source exists, prefer the ambient source over a permanent refusal (a store-hit-but-expired case falling through to discovery), and make the error name the ambient file it would have read.