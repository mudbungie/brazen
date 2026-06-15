+++
title = "Spec 0003 — Canonical ⇄ Anthropic messages mapping"
created = 1781559052
updated = 1781559052
priority = 79
tags = ["spec", "design"]
+++
The complete projection for Anthropic messages, the second v0.1 protocol. Request: system as top-level field, Role::Tool -> a user message carrying tool_result, content blocks (text/image/tool_use/tool_result/thinking with signature round-tripped verbatim), tools flat {name,description,input_schema}, tool_choice {type:auto|any|tool|none}, anthropic-version header from the row. Response decode: the named-event state machine (message_start -> MessageStart+Usage, content_block_start{text|thinking|tool_use} -> ContentStart, content_block_delta{text_delta|thinking_delta|input_json_delta} -> ContentDelta, content_block_stop -> ContentStop, message_delta{stop_reason,usage} -> Finish+Usage, message_stop -> End, ping ignored). stop_reason mapping incl. pause_turn->Pause and refusal->Finish{Refusal} at HTTP 200 (exit 0); error event (e.g. overloaded_error at 529) -> Error. Cache token fields -> Usage.cache_read/cache_write.