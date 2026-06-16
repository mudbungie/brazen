+++
title = "Carry HTTP status on Frame; derive ErrorKind from it (delete error-type tables)"
created = 1781638876
updated = 1781638882
claimant = "Merging"
priority = 64
tags = ["impl"]
+++
The non-2xx whole-body error path currently reconstructs the HTTP status from the error body's type/code strings (openai) or maps type->status directly (anthropic). That re-derives a fact the transport already owns (TransportResponse.status) from a lossy proxy — single-source-of-truth violation; only lossless for anthropic by luck.

Fix (subtraction):
1. frame.rs: replace `whole_body: bool` with `status: Option<u16>` (None=streaming frame; Some(code)=non-2xx whole-body error). whole_body was always just status.is_some().
2. canonical/error.rs: add `ErrorKind::from_http_status(u16)` = `match s { 401|403 => Auth, _ => Provider{status:s} }`. Collapses because Provider{status} already computes exit (4xx->69/5xx->70) and retryable() (429||>=500). Verified against openai §4.2 and anthropic §4.3 tables — reproduces every row.
3. openai/decode.rs: on Some(status) frame, kind = from_http_status(status); body parses message+provider_detail only. DELETE error_kind(type,code).
4. anthropic/decode.rs: HTTP whole-body (frame.status Some) -> from_http_status; keep the type-table ONLY for the mid-stream 2xx `event: error` case (no governing status exists there — the one legitimate string-derived kind).
5. Update Frame{..} literals across tests (anthropic_fixtures, openai_*, seams_protocol, determinism).
6. Specs: architecture §8 + sse-decoder §9 state 'whole-body error frames carry the status; kind derives from it'; openai §4 and anthropic §4.3 drop the status<-string reconstruction.

Gate: make check (100% line cov, clippy -D warnings, <300 lines/file).