+++
title = "Anthropic messages Protocol impl (encode + decode)"
created = 1781559064
updated = 1781559064
priority = 66
tags = ["impl"]
+++
Implement protocol/anthropic.rs per spec 0003: encode (system field, Role::Tool->user+tool_result, content blocks incl. thinking signature, flat tools, tool_choice, anthropic-version header) and decode (named-event state machine over Frames + DecodeState, stop_reason incl. pause_turn/refusal, error event -> Error, cache usage fields). Record the golden .sse fixtures (basic, thinking_tools, refusal, pause, overloaded). 100% from fixtures, no network.