+++
title = "Spec 0002 — Canonical ⇄ OpenAI chat/completions mapping"
created = 1781559051
updated = 1781559051
priority = 80
tags = ["spec", "design"]
+++
The complete, normative projection between the canonical model and the OpenAI chat/completions wire dialect, both directions. Request: messages/roles (Role::Tool -> role:"tool" 1:1, content Vec<Content> -> string-or-array), tools nested as {type:function,function:{name,description,parameters}}, tool_choice (Auto->"auto", Any->"required", Tool->{type:function,...}, None->"none"), stream_options:{include_usage:true}. Response decode: positional choices[0].delta state machine, synthetic index 0 for text, synthesized ContentStart{ToolUse} on first sight of tool_calls[i].id+function.name, arguments fragments -> JsonDelta, finish_reason mapping (stop/length/tool_calls), final usage chunk -> Usage, data:[DONE] -> End, error-body parsing for 4xx/5xx. Includes the golden fixtures and the exact WireRequest goldens.