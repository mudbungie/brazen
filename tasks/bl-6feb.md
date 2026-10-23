+++
title = "sever the preset catalog from the routing table: a row is routable only when named in config or holding a credential"
created = 1792725833
updated = 1792728530
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["feature"]
+++
## Ask
The operator doubts that config.md §3.2's no-tombstone ruling is a real invariant. Evaluate whether a user file should be able to delete a defaults.toml row, and either add the mechanism or record why not in the spec.

## The ruling under question (specs/config.md §3.2, bl-d67a)
> there is **no** tombstone syntax for deleting a default row: a row you never name costs nothing (routing is a `find` over ≤10 rows), a row whose claims you dislike is disarmed by redeclaring it with empty `model_prefixes`/`model_aliases`, and a delete verb would be a second, subtractive merge algorithm beside the one `or`.

## The evidence that a row is not free
A consumer that lists rows pays for each one. yog's per-workspace provider read is `--list-providers`'s table, and the lernie seat painted all ten in its provider picker; five (anthropic, openai, mistral, openai-responses, google) held no key on that box and answered every pick with exit 77 'no credential for provider'. The seat now hides credential-less rows (lernie bl-526d), but that is a consumer working around the floor. Two more costs the ruling does not count:
- a built-in row can duplicate an operator row: the shipped `ollama` and an operator `local` both point at localhost:11434, and the only merge is to rename the operator row;
- disarming claims does not remove the row from the listing, so it is no answer for anything that reads the table.

## Questions to settle
1. Is 'one merge algorithm' worth more than an operator's ability to say 'this row does not exist here'? Could deletion fit inside the existing per-field fold (e.g. a row-level `enabled = false` / `disabled = true` field that the fold carries like any other and resolution drops), so it is not a second algorithm?
2. Does it break the additive-only forward-evolution invariant (§2.4)? A new field is additive; check that a dumped file stays valid.
3. Does the 'floor reaches you with no edit' property (the reason for no replace-semantics) survive? A deleted row is opt-out by name, so later shipped rows still arrive.
4. Severability: removing a default should delete config, not edit code (AGENTS.md). Today removing a built-in row is not expressible in config at all.

## Done when
Either the spec and code gain the mechanism (with tests, and --list-providers / --dump-config behaviour specified), or §3.2 is amended with the answers to the four questions above.