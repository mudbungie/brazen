+++
title = "bz --skill: dump an embedded skill doc (richer than --help, worked examples) to stdout"
created = 1784518855
updated = 1784518856
claimant = "Hawsers"
priority = 5
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Add a --skill discovery probe (sibling of --help/--version) emitting an embedded literal markdown file (data/skill.md via include_str!) to stdout, exit 0. Checked first at every entry point (run/count/models/serve), exempt from control-op mutual-exclusion, ignored by route(). Agent-facing doc: input model, output modes, auth, config/provider rows, control ops, ingress, exit codes, all with worked command examples.