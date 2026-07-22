+++
title = "Make bz transport-selectable so a private adapter can preserve a reference client's HTTP/TLS wire identity"
created = 1784699185
updated = 1784699274
claimant = "chowder"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["architecture", "transport"]
+++
## Problem

`WireRequest` carries method, URL, headers, body, and timeouts, but the `bz` binary always rematerializes it through `src/native/transport.rs`'s ureq/rustls stack. That stack determines server-observable facts outside `WireRequest`: generated/default headers, header casing/order, content encoding, HTTP framing/version, connection behavior, ALPN, and the TLS ClientHello (cipher/extension ordering and fingerprint). Therefore application-level request equality does not imply transport-wire equality. Header/config changes cannot make ureq/rustls wire-identical to a reference client implemented with another stack such as Bun/Node/undici.

The library `Transport` trait lets an embedder replace transport, but the installed `bz` CLI hard-wires `HttpTransport`; a private adapter cannot select its own transport stack while retaining Brazen's canonical encode/auth/decode pipeline and single-shot process behavior. This is a general Brazen gap, not a Claude-specific profile request.

## Deliverable

Design first: amend the living architecture/specs to distinguish **application-wire conformance** from **transport-wire conformance**, then choose and implement the narrowest operator-selectable transport seam for `bz`. An operator-installed/private adapter must be able to own the HTTP/TLS implementation without adding a built-in vendor profile or forking Brazen's protocol pipeline. Consider an external transport capability versus another mechanism; do not assume the first mechanism is correct.

## Acceptance

- The default ureq/rustls transport and ordinary provider rows remain unchanged.
- Selection is explicit and operator-owned; no vendor/client identity is compiled into Brazen.
- The selected transport receives the exact method, URL, ordered headers, body bytes, and timeout/cancellation intent, and returns status, response headers needed by Brazen (including `Retry-After`), and an incrementally streamed body.
- Secrets never appear in argv, logs, diagnostics, or temporary world-readable files.
- The generation invariant remains one upstream request, no retry, no tool execution/re-entry; OAuth refresh remains the separately documented auth control request.
- A conformance harness proves both layers independently: normalized application request equality, and the selected transport's server-observed HTTP/TLS identity against a captured reference. Include header/framing observations and a TLS ClientHello/ALPN fingerprint; do not call semantic JSON equality “wire-identical.”
- Transport startup, protocol, truncation, cancellation, and non-2xx failures map through Brazen's existing error/exit model and are fully tested.

## Non-goals

- No built-in Claude Code/session provider profile.
- No system-prompt, tool-list, beta-header, OAuth-policy, or credential-ownership work; those are application/profile concerns outside this ball.
- No claim that ureq/rustls itself can be configured into another runtime's exact TLS fingerprint without evidence.