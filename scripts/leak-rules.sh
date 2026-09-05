#!/usr/bin/env bash
# brazen leak rules — the TABLE, and only the table. `scripts/leak-scan.sh` is
# the mechanism that runs it; this file is what counts as a leak. Ported from
# yog (yog bl-fd5a/bl-167d/bl-1043, itself the rust-bootstrap template) under
# bl-39f1; the four adaptations this repo needed are marked BRAZEN below and
# each carries its reason. Two halves so both stay inside the 300-line cap and
# so "what may not be committed" reads as one list.
#
# brazen handles credentials (0600 cred files, OAuth tokens) and speaks to
# vendor APIs, so the material it could leak is exactly the material it
# handles — and, being a PROTOCOL crate, it also carries vendor-shaped test
# fixtures by the dozen. That tension is what every BRAZEN note below is
# about: the rule must still bite on the real thing while a tree full of
# deliberately fake ones passes, or the gate gets turned off within the day.
#
# Sourced, never executed.
#
#   PATTERN — ERE matched with `grep -o`, so a hit is the offending text.
#   EXCEPT  — dropped if the matched text matches this ERE, ANCHORED AT THE
#             START of the match (there is nowhere else to anchor: the hit line
#             is `path:line:match`, so `^` would mean the path, while `$` does
#             mean the end of the match, because the match ends the line).
#             Write a leading `.*` where you mean "contains".
#   WHY     — printed with the finding; it must say what to DO.
#
# ONE INVARIANT GOVERNS THIS FILE: **no pattern here may match this file.**
# The scanner scans itself and this table — bl-167d deleted the exemption that
# made the file the rules live in the one place a leak could not be seen — so a
# rule whose literal text is also an instance of itself would flag the table
# forever. Exactly one ever was: PuTTY's key banner, the only bare literal with
# nothing after it. It is written `Fil[e]` for that reason, the same idiom as
# `ps | grep '[s]shd'`. Every other pattern is self-immune already, because its
# literal prefix is followed by a bracket expression or an alternation bar.
#
# There is no per-rule path exemption and no allowlist, and none was added on
# the port. Every rule applies to every tracked file. Where this tree tripped a
# rule, the RULE was narrowed with its reason written into it (a reviewable
# exception) or the TREE was fixed — never a path put on a list, because a path
# list is a hole nobody re-reads. Add an escape back only when a rule is wrong,
# and fix the rule instead if you can.

# **The table is a `case`, not a `declare -A` map** (bl-1015). Associative
# arrays need bash 4; macOS ships bash 3.2 and, the licence having changed under
# it, always will — so a subscripted table made this gate, and every test of it,
# Linux-only on a repo whose CI builds both platforms.
#
# The shape it takes is load-bearing twice over. A rule's three fields stay
# **together**, under its name and its own commentary, which is what makes this
# a table rather than three lists to keep in step. And the name on the left of
# every `=` is `PATTERN`/`EXCEPT`/`WHY`, never the rule's own — which is what
# keeps the file self-immune under the invariant above: spelled the other way
# (`WHY_vendor_token=`) the *variable name* is itself an instance of
# `credential-assignment`, and the table flags itself forever.
#
#   rule_fields home-path   ->  $PATTERN, $EXCEPT, $WHY for that rule
#
# A rule with no EXCEPT leaves it empty, and so does a name no arm matches;
# there is no error case, because RULES below is the only source of names.
rule_fields() {
  PATTERN='' EXCEPT='' WHY=''
  case "$1" in

  private-key)
    PATTERN='-----BEGIN( [A-Z0-9]+)* PRIVATE KEY-----|PuTTY-User-Key-Fil[e]'
    WHY='a private key. Remove it, then rotate it — it is burned.' ;;

  vendor-token)
    PATTERN='sk-ant-[A-Za-z0-9_-]{16,}|sk-(proj-)?[A-Za-z0-9]{32,}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|(AKIA|ASIA)[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|xox[abprs]-[A-Za-z0-9-]{10,}|glpat-[A-Za-z0-9_-]{20}|eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}'
    WHY='a live API token. Remove it, then revoke it at the vendor.' ;;

  credential-assignment)
    PATTERN="(pass(word|wd|phrase)?|secret|token|api[_-]?key|apikey|credential|authorization)['\"]?[[:space:]]*[:=][[:space:]]*['\"][^'\"]{8,}['\"]"
    # Two escapes. The first is the declared placeholder vocabulary. The second is
    # structural and does the real work: a multi-word ALL-ALPHABETIC value is
    # prose, not a secret — `"credential":"not required"` is a status field, and
    # `bz --list-providers --json` emits one per row. A secret that survives it
    # must carry a digit, a symbol, or no space at all, which every real one does.
    #
    # BRAZEN: `\{[A-Za-z_]` joins the placeholder vocabulary beside `${VAR}`. An
    # INTERPOLATION is a placeholder in every language that has one — a Rust
    # `format!("…{access}…")` names a variable exactly as `${VAR}` does — and the
    # token-response tests here build bodies that way. Widening the escape from
    # one interpolation syntax to two is a generalization, not a carve-out: it
    # cannot admit a literal, which is the only thing a secret can be.
    EXCEPT=".*(REDACTED|redacted|<[^>]*>|\\\$\{|\\\$[A-Z_]+|\\{[A-Za-z_]|example|placeholder|dummy|fake|changeme|xxxx|TODO|\.\.\.|test|invalid)|.*['\"][A-Za-z]+( [A-Za-z]+)+['\"]\$"
    WHY='a credential assigned a real-looking value. Use a placeholder ("<redacted>", "${VAR}", "example") if it is illustrative.' ;;

  ipv4-routable)
    PATTERN='\b((25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\.){3}(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\b'
    # Loopback, the unspecified and broadcast addresses, and the three RFC5737
    # documentation ranges. Everything else — RFC1918 included — is network
    # topology, and internal topology is exactly the kind of thing this scan is
    # for.
    EXCEPT='127\.|0\.0\.0\.0|255\.255\.255\.255|192\.0\.2\.|198\.51\.100\.|203\.0\.113\.'
    WHY='a routable IP address. Use a documentation range (192.0.2.x, 198.51.100.x, 203.0.113.x) or loopback.' ;;

  ipv6-address)
    PATTERN='\b[0-9a-fA-F]{1,4}(:[0-9a-fA-F]{1,4}){7}\b'
    EXCEPT='2001:0*[dD][bB]8:'
    WHY='an IPv6 address. Use the 2001:db8:: documentation prefix.' ;;

  mac-address)
    PATTERN='\b([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}\b'
    WHY='a MAC address — a permanent hardware identifier for one machine.' ;;

  # Every home root on every platform, NOT the scanning account's own `$HOME`
  # (bl-167d). The old rule was built from `$HOME` at scan time, which read as
  # identity-free and was in fact runner-dependent: CI runs as `runner`, so it
  # could not see the author's home at all, and no box but the author's could
  # see the author's paths. The rule that holds everywhere is the inverse — an absolute
  # path under ANY home root is a leak unless its account name is one of the
  # house's three synthetic roots (`/home/u`, `/home/op`, `/home/x`, AGENTS.md).
  #
  # `(^|[^A-Za-z0-9._/-])` is load-bearing: `/home` also occurs as an interior
  # path segment with a wholly different meaning — `<world>/walls/home/brazen`
  # is a wall leaf (DESIGN §3.1), not anybody's home — so the rule fires only
  # where `/home/` STARTS an absolute path. The trailing boundary is the other
  # half: a synthetic root must never match inside a longer account name.

  home-path)
    PATTERN='(^|[^A-Za-z0-9._/-])((/home|/Users)/|[A-Za-z]:[\\/][Uu]sers[\\/])[A-Za-z0-9._-]+([^A-Za-z0-9._-]|$)'
    EXCEPT='.?((/home|/Users)/|[A-Za-z]:[\\/][Uu]sers[\\/])(u|op|x)([^A-Za-z0-9._-]|$)'
    WHY='an absolute path under a home directory — it names an account, and it is a portability bug besides. Use a synthetic root (/home/u).' ;;

  # An address is an identity, and one pasted from a config, a transcript or a
  # stack trace names a person who did not publish it. The escape is the reserved
  # documentation and test space (RFC2606/RFC6761): example.com/org/net and the
  # `.invalid`, `.test`, `.local`, `.localhost` TLDs, anchored at the END of the
  # match, so a domain that merely BEGINS with a reserved label cannot borrow
  # the escape.
  #
  # The one real address in this tree is the MIT copyright holder's, in LICENSE.
  # That is a DECLARATION of authorship on a crate meant to be published, not a
  # disclosure, and it is written here rather than exempting the LICENSE path:
  # an exception you can read in the rule is reviewable, a path exemption is a
  # hole. It is the only literal identity in this file, and it is already public.

  personal-email)
    PATTERN='[A-Za-z0-9._%+-]+@[A-Za-z0-9._-]+\.[A-Za-z]{2,}'
    EXCEPT='.*@([A-Za-z0-9-]+\.)*(example\.(com|org|net)|invalid|test|local|localhost|localdomain)$|mudbungie@gmail\.com$'
    WHY='an email address. Use a documentation address (u@example.com) or a reserved TLD (t@t.local, t@test.invalid).' ;;

  # Pasted conversation. A transcript is content somebody said, and the shape it
  # arrives in is a speaker label at the head of a line — bare, quoted, bulleted
  # or bolded. The vendor JSON envelope is deliberately NOT here: yog PARSES
  # transcripts, so `"role":"assistant"` occurs by the hundred in its own tests,
  # and a rule that fires there would be turned off within the day. The envelope
  # is covered where it is unambiguous instead — see session-artifact.

  quoted-dialogue)
    PATTERN='^[[:space:]>*_#-]*(Human|Assistant|Claude|ChatGPT|Copilot|Gemini|Codex)[*_]{0,2}[[:space:]]*:[[:space:]]+[^[:space:]]'
    WHY='transcribed dialogue attributed to a speaker. A conversation is private content — cite the conclusion, do not paste the exchange.' ;;

  # (bl-9fbe below is the seat's ball, where the finding was filed; this port
  # landed as bl-1e67. The rule text is byte-identical across the four tables.)
  # THE URL IS THE FORM THAT ACTUALLY LEAKED (bl-9fbe; operator ruling
  # 2026-08-30, "ban them, no reason to allow it"). The one published instance
  # was not a bare id in a file — it was a vendor code-session URL in a
  # pull-request BODY, put there by harness convention and permanent because
  # the forge serves `refs/pull/<n>/head` forever. Two alternatives close it
  # and each owns its own fixture line, so neither can hide behind the other:
  # `session` joins the resource-prefix list, which catches the bare id
  # wherever it is written, and the code-session URL path shape catches an id
  # under that path whatever its prefix — the half a prefix list can never be
  # complete over. Both stay self-immune the way the header above requires: the
  # prefix is followed here by `)` and the path by a bracket expression.

  session-artifact)
    PATTERN='(msg|toolu|asst|thread|run|req|session)_[A-Za-z0-9]{20,}|chatcmpl-[A-Za-z0-9]{20,}|"(parentUuid|sessionId|leafUuid|isSidechain)"|claude\.ai/code/[A-Za-z0-9_-]{16,}'
    # BRAZEN: an id that spells `Fixture` inside itself is declaring its own
    # falsity, exactly as FIXTURE_MARKER makes a rule fixture declare it. This
    # crate DECODES vendor envelopes, so realistically-shaped resource ids are
    # its test data — `msg_01Fixture000000000000001` in the anthropic and
    # claude_code decode fixtures — and a rule that fired on them would be
    # switched off inside a week, which is the same reasoning that keeps the
    # vendor JSON envelope (`"role":"assistant"`) out of quoted-dialogue. The
    # escape is the VALUE, never a path: a vendor id is base62 random, so a real
    # one carries that word with probability ~0, and pasting a real id into this
    # tree cannot borrow the escape without the author typing the word onto it.
    EXCEPT='.*[Ff]ixture'
    WHY='a real agent-session artifact (a vendor resource id, a Claude Code transcript key, or an agent-session URL). Session transcripts are conversation content — they do not belong in a published crate, and by operator ruling no session URL belongs in the published text of this repository at all.' ;;

  forbidden-path)
    WHY='a file shape that carries credentials or session state. It should be gitignored, not tracked.' ;;

  binary-content)
    WHY='content the scanner cannot read — a binary the gate would have to take on faith. Track it only with a `<name>.provenance.md` beside it saying what it is, where it came from and what claim it serves; otherwise do not track it.' ;;
  esac
}

RULES=(private-key vendor-token credential-assignment ipv4-routable
       ipv6-address mac-address home-path personal-email quoted-dialogue
       session-artifact)

# The path rule. Not a content rule: what is wrong with `.env` is that it
# exists at all, whatever is in it.
#
# BRAZEN, two narrowings, both of a PROXY that misfires in this repo:
#
#   * `.claude/` was banned whole. That is right for a HOME directory, where
#     the directory holds credentials and session transcripts — but a repo's
#     own `.claude/` is project config Claude Code reads and that a repo is
#     meant to track, and this one tracks `settings.json` and
#     `hooks/docs-reminder.sh` (AGENTS.md, "Docs — advisory"). So the rule now
#     names the MEMBERS that carry credentials or session state
#     (`.credentials.json`, `projects/`, `todos/`, `shell-snapshots/`,
#     `statsig/`, `history*`), which is what the ban was ever about, and the
#     config half passes. A tracked `.claude/projects/` still fails.
#   * `.jsonl` was banned as an extension, as a proxy for a Claude Code
#     transcript dump. Here it is the crate's OWN output format — the canonical
#     event stream is newline-delimited JSON, and `tests/fixtures/golden_v1_*.jsonl`
#     are its golden files with byte-for-byte tests. The proxy costs three
#     deliberate fixtures and buys nothing the content rules do not already
#     hold: a `.jsonl` is TEXT, so every rule reads it, and the actual transcript
#     signature is `session-artifact`'s `"(parentUuid|sessionId|leafUuid|isSidechain)"`
#     alternatives, which fire on a real dump whatever it is named. The
#     unreadable-content rule below covers the case the extension list cannot.
FORBIDDEN_PATH='(^|/)(\.env(\..+)?|\.netrc|\.npmrc|\.pypirc|credentials\.json|id_(rsa|dsa|ecdsa|ed25519)(\.pub)?)$|(^|/)(\.ssh|\.aws|\.gnupg|\.config/brazen)/|(^|/)\.claude/(\.credentials\.json|projects/|todos/|shell-snapshots/|statsig/|history)|\.(pem|key|p12|pfx|jks|keystore|kdbx)$'

# The unreadable-content rule (yog bl-167d). `grep -I` silently SKIPS binary
# files, so before this rule an archive, a database, a PDF, a HAR capture, a
# screenshot or a built executable passed the whole scan by being unreadable —
# the one class of file most likely to carry a dump. Unreadable is rejected,
# not skipped.
#
# BRAZEN: the escape is a DECLARATION, not a list. yog's version was an
# allowlist regex of paths (`BINARY_ALLOWED`), on the policy that a tracked
# binary must be a regenerable derivation with a byte-for-byte test. This repo's
# one tracked binary is not a derivation and could never be one — it is a real
# TLS first flight captured from a client that is NOT brazen's stack, which is
# the whole of its evidential value (transport §8.3), so "regenerate it" would
# destroy what it proves. It already carries the thing an allowlist entry can
# only gesture at: a tracked `<name>.provenance.md` beside it, naming the
# client, the command, the date, the byte count and the claim it serves.
#
# So the rule is: a binary passes iff a tracked provenance file sits beside it.
# That is strictly stronger than a path list — the entry is a DOCUMENT a
# reviewer reads rather than a pattern nobody re-reads, it cannot be satisfied
# by editing a regex in this file, and it is severable in the right direction:
# delete the provenance and the binary is refused again. `--self-test` proves
# both directions, so neither arm can rot.
BINARY_PROVENANCE='.provenance.md'

# Every non-comment line of every content-rule fixture must contain this
# marker (bl-167d). No regex can tell a real secret from a fabricated one, so
# the fixture that houses real-SHAPED values carries its falsity in the value
# itself: a rule fixture cannot be updated by pasting something real without
# the author also typing this word onto the line, and a reviewer reading the
# diff sees it. The scanner is what enforces it; see `--self-test`.
FIXTURE_MARKER='notreal'
