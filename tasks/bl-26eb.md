+++
title = "Define the three traits + ProviderCtx + WireRequest + Provider data"
created = 1781559060
updated = 1781593258
claimant = "Juggled"
priority = 69
tags = ["impl"]

[[blockers]]
id = "bl-23cd"
on = "claim"
+++
Implement protocol/mod.rs (trait Protocol, ProviderCtx, WireRequest, Framing, Frame, DecodeState), transport.rs (trait Transport, TransportResponse), store.rs (trait CredStore, Cred, Secret, trait Clock), auth/mod.rs (trait Auth signature). Implement config/provider.rs (Provider record, ProtocolId/AuthId enums) and registry.rs (Registry::builtin skeleton). Build MockTransport, in-memory CredStore, and FakeClock as test doubles. Establishes the seams the rest plugs into.