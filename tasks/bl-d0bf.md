+++
title = "Remove the built-in `google` API-key provider row (quota-dead, misroutes agents; protocol stays)"
created = 1790733044
updated = 1790733044
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
The built-in google row (generativelanguage.googleapis.com, API key) has free-tier quota 0, so every call 429s, and agents keep picking it instead of the working antigravity row (google_cloudcode wrapping the same google_genai dialect). Remove the default row; the google_generative_ai protocol stays. An operator who wants it re-adds the row in config (severability: removing a default deletes config, not code).