+++
title = "Pre-0.1.0 one-way-door review: timeout knob taxonomy — three flags (connect/response/idle) or one 'upstream is not sending' budget?"
created = 1783472192
updated = 1783490096
priority = 6
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["design"]
+++
Owner musing 2026-07-08, filed for pre-freeze review: the three timeout flags (--timeout-connect/--timeout-response/--timeout-idle) freeze into the §5.10.3 surface at 0.1.0. Are connect/response/idle three facts or ONE — 'silence longer than N'? A wall-clock TOTAL timeout is separately REJECTED by the owner (footgun; the harness kills the child).

FEASIBILITY RESEARCH COMPLETE (2026-07-08, read-only agent; full report in the review-session transcript). RECOMMENDATION: COLLAPSE to one knob, --timeout-silence.

Decisive argument: all three timeouts already collapse at the only observable layer — every one surfaces as ErrorKind::Transport -> exit 69 (canonical/error.rs:195, 203-207). The split's only value is the message string, which survives the collapse for free. httpx is the strongest prior art: one number applied to all phases is the ergonomic default; per-phase is the advanced override. curl's --connect-timeout is a general-purpose-client relic with no constituency in a single-round-trip CLI to known LLM hosts.

CHEAP IMPL (recommended): keep one config value timeout_silence=N and fan it internally onto ureq timeout_connect(N) + timeout_recv_response(N) + IdleChunkReader::spawn(..., N). Near-zero cost, no new mechanism, ureq's phase-named diagnostics survive. Feeding ureq's connect timeout internally is not a SYNACK *knob* — it applies the one silence policy to the connect phase; some connect bound is required or a black-holed SYN hangs to the OS TCP default (~2 min). WATCHDOG ALT (not recommended): move send() onto an IdleChunkReader-style worker, recv_timeout(N) for first message + per chunk — buys only DNS-resolve-black-hole coverage at real complexity.

Behavior deltas vs today's 30/120/300: (1) connect black-hole waits N instead of 30s; (2) one N serves both connect and inter-token timescales (a legit token gap between N and 300s would newly trip). Both owner-endorsed by 'if it's not sending, it's not sending'. OS fast-fails (NXDOMAIN, ECONNREFUSED) unchanged. Suggested single default ~120s.

Deletion inventory (verified file:line in the report): cli/parse.rs:115-120 two arms, config/env.rs:62-63, partial/mod.rs:99-100+131-132, partial_de.rs:125-126, resolved.rs:53-54+72-73, resolve/mod.rs:72-73, dump.rs:103-107, transport.rs:44-45 (rename idle->silence), native/transport.rs:62-67+76-81, defaults.toml two lines, discovery.rs:105 help, specs config.md:47-49/232-234/342-344 + architecture.md:883/944-946, tests config_partial/config_dump/config_resolve/config_env/cli_args/config_defaults (config_defaults.rs:45-47 pins 30/120/300). No external consumer depends on the split (smoke.sh grep-clean; lernie refs are count-tokens only); OAuth refresh inherits the stamped timeouts automatically (auth.md:403).

AWAITING OWNER RULING on three questions: (1) collapse yes/no; (2) the single default (research suggests ~120s); (3) name — --timeout-silence (recommended; the budget spans connect->last-byte) vs reusing --timeout-idle (smaller test churn). Implementation dispatches only after the ruling.