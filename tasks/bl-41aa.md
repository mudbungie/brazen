+++
title = "Fix stale 'bz crate' references (store.rs:107, protocol/mod.rs:3)"
created = 1782204037
updated = 1782204037
parent = "bl-3d74"
tags = ["docs", "lane2"]
+++
store.rs:107 doc says 'The bz crate backs it with…' — there is no bz crate anymore (sibling native/transport.rs:2 correctly says 'the bz bin'). protocol/mod.rs:3 header lists only 'openai_chat, anthropic_messages' plus a mid-dev 'plug in via their own tasks' framing, though all 5 impls ship. FIX: store.rs:107 to 'bz bin'; protocol/mod.rs:3 names all five concrete protocols and drops the 'tasks' framing. protocol/mod.rs is owned by this task within lane 2.