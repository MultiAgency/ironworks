#!/usr/bin/env bash
# COST: one runtime member, spent irreversibly — approval-off is one-way (§L-9).
# Needs a probe; no model key.
# Prove a member's tool authority can be removed, measured, and seen to break.
#
#   dev/probe-up.sh               # a pinned runtime; NO model key needed
#   dev/prove-confinement.sh      # this
#
# Six checks. Like prove-tenancy.sh and prove-lifecycle.sh this costs nothing and touches no
# model: confinement is entirely the settings API, and asking a model whether it is confined would
# measure what it was told rather than what it may do.
#
#   1. non-vacuity   a fresh member assesses DRIFTED       an empty catalogue would pass every check below
#   2. dry-run       --dry-run writes nothing              the plan is printed, the catalogue is unchanged
#   3. apply         confine reaches VERIFIED              §L-8's floor: all but two disabled, approval off
#   4. idempotent    a second run writes nothing           the plan is empty when already at policy
#   5. detection     one tool re-enabled → DRIFTED         and the drift names that tool
#   6. detection     approval back on → DRIFTED            approval-state drift is measured separately
#
# ── why check 1 is the one that makes the rest mean anything ────────────────────────────────
#
# §L-8's warning generalises: a catalogue that could not be read, or that listed nothing, would
# assess as fully confined. Checks 3 to 6 all assert *some* state of a catalogue; every one of
# them passes vacuously against a runtime that answers with an empty list. So check 1 proves the
# same member, in the same run, against the same process, is measurably NOT confined before
# anything is applied. Read 1+3 as a pair; either half alone certifies nothing.
#
# Check 5 exists for the reverse reason. A reconciler that always said VERIFIED would pass 3 and
# 4; breaking confinement on purpose and watching the verdict change is what distinguishes a
# measurement from a constant.
#
# ── what this proves, and what it does not ──────────────────────────────────────────────────
#
# It proves the catalogued surface can be closed, that closing it is idempotent, and that reopening
# it is detected. It does NOT prove the model cannot act:
#
#   - `Disabled` means refused **at dispatch**, and nothing here measures dispatch.
#   - The catalogue is not the whole surface. Measured at 4cb47cfaf3: 53 fresh-member settings rows
#     and 53 host-captured capability ids — not the same 53, overlapping in 45, with 8
#     catalogue-only and 8 captured-surface-only. Exhaustive fresh-member deferred reachability is
#     BLOCKED.
#   - Ordinary settings are custody-backed: the credential that applies confinement can undo them.
#     The two `operator_config_set_*` rows are locked and are the measured exception.
#
# This proof has a residual, and it is now confined to a member created for the run. Check 6
# re-enables approval; the restore reuses the already-completed deterministic `false` activity and
# is accepted without taking (§L-9). All six checks can pass while that member is left unconfined,
# and cleanup reads the state back and says so. Because the member is minted per run rather than
# borrowed from the probe, the residual is discarded with it: the stack is reusable, and two
# consecutive runs on one probe both score 6/6. Tear down when you are done, not between runs.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

API="${IRONCLAW_API:-http://127.0.0.1:3997}"
SLUG="${PROVE_CONFINE_SLUG:-confine-probe}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-confine.XXXXXX")"
STORE="$WORK/lifecycle.store"
# A tool that is ordinary, harmless, and catalogued — re-enabling it in check 5 grants nothing
# anyone would mind. Deliberately not one of the sixteen authority-widening ids.
REOPEN="${PROVE_CONFINE_TOOL:-builtin.time}"

# Checks 5 and 6 break confinement on purpose and restore it on the next line, so an interrupt
# between the two — Ctrl-C, a timeout, a killed terminal — skips the restore. The settings this
# proof writes are the member's own and outlive the process.
#
# **Only one of the two restores actually works, and that is measured, not assumed.**
#
# Tool state does restore: `set_state <tool> disabled` puts it back, confirmed by reading the
# catalogue afterwards. Approval, measured at `4cb47cfaf3` after check 6 has set it:
#
#     POST /api/webchat/v2/settings/tools {"enabled":false}
#     -> 200 {"entry":{"key":"agent.auto_approve_tools","value":true,"source":"override",...}}
#
# Unchanged. So `set_approval false` — check 6's own restore line and the one below — is a
# **silent no-op in that state**, and check 6 leaves the member with approval ON whether or not
# the run was interrupted. That matters because §L-4 and §L-5 make approval-off the only
# containment for the two tools the runtime refuses to disable: the member ends the run LESS
# confined than check 3 made it, and this script cannot put it back.
#
# Why the same payload works during `apply`: IronClaw derives the auto-approve activity id from
# tenant, user, agent, project and boolean value (`reborn_services.rs:1694-1714`). `apply` executes
# the first `false` activity. Check 6 executes the distinct `true` activity. Cleanup asks for
# `false` again and receives the completed first activity rather than a new store mutation. The
# direct store supports `true` -> `false`; the product-surface replay is the layer that does not.
#
# The trap earns its place for the tool-state half, and the read-back turns a silent regression
# into a sentence. `mutating` gates it so an early BLOCKED exit — before a token or the helpers
# exist — cannot call them.
mutating=0
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() {
  if [ "$mutating" -eq 1 ]; then
    set_state "$REOPEN" "disabled" >/dev/null 2>&1 || true
    # Attempted in case a later revision honours it, then verified rather than trusted.
    set_approval false >/dev/null 2>&1 || true
    if approval_is_on; then
      printf '\n%s\n%s\n' \
        "prove-confinement: approval is still ON — the restore was accepted but did not take (§L-9)." \
        "prove-confinement: that member is spent. It was minted for this run, so the probe is still good; a later run mints its own." >&2
    fi
  fi
  rm -rf "$WORK"
}
# EXIT alone is not enough: a script killed by an uncaught SIGINT or SIGTERM is not guaranteed to
# run its EXIT trap, and those are exactly how an operator interrupts a proof. Cleanup runs twice
# on a signal, which is harmless because every step in it is idempotent.
on_exit cleanup

cargo build -q -p ironworks-cli -p ironworks-lifecycle 2>/dev/null || {
  give_up "build" "cargo build failed"; }
BIN=./target/debug/ironworks

# Reachability, before the mint. This used to sit forty lines below, *after* the mint, where it
# could not do the job its own message promises: the mint is a POST to this same `$API`, so a
# runtime that is down fails there first and reports "could not mint a member" plus whatever HTML
# came back, instead of "run dev/probe-up.sh".
#
# **It buys the message, not a member.** Measured: pointed at a dead port, the old order also spent
# nothing, because a runtime that cannot answer cannot mint either. The gate earns its place by
# being the check whose advice an operator can act on, not by saving anything — which is the claim
# it would be easy to make here and is not the one this supports.
curl -fsS --max-time 5 "$API/api/health" >/dev/null 2>&1 || \
curl -fsS --max-time 5 "$API/health" >/dev/null 2>&1 || {
  give_up "runtime.up" "no runtime at $API; run dev/probe-up.sh"; }

# The member credential IS the runtime's own token here: confinement authenticates AS the member,
# because POST /settings/tools resolves the caller's scope and `?user_id=` is ignored.
#
# ── the two credentials are kept apart, and the member is minted ────────────────────────────
#
# This used to read one token from the container and use it as BOTH the operator and the member.
# That worked only because the WebUI env bearer happens to be both, and it measured a caller no
# tenant will ever have: measured at the pin, the env bearer's catalogue is **53 rows** and a
# member minted through `POST /admin/users` sees **52** — the difference is `nearai.web_search`
# (`docs/experiments/0037-what-the-seam-never-asked-for.md` check 3). §L-8's floor of 50 disabled
# + 2 locked is the *minted member's*, so proving it against the env bearer proved a neighbouring
# claim rather than this one.
#
# Minting per run also retires this script's worst property. §L-9 makes approval-off one-way, so
# the old run left the probe's only member permanently unconfinable and every run needed a fresh
# bring-up. The residual now lands on a member created for the run and used by nothing else.
OPERATOR="${IRONCLAW_OPERATOR_TOKEN:-}"
if [ -z "$OPERATOR" ] && command -v docker >/dev/null 2>&1; then
  OPERATOR="$(docker exec multiprobe-ic-1 sh -c 'echo $IRONCLAW_REBORN_WEBUI_TOKEN' 2>/dev/null | tr -d '\r')"
fi

# An explicit member token wins: it is how an operator points this at a member that already
# exists. Otherwise mint one, which is the faithful path and the default.
TOKEN="${IRONCLAW_MEMBER_TOKEN:-}"
CALLER="a member supplied in IRONCLAW_MEMBER_TOKEN"
if [ -z "$TOKEN" ]; then
  if [ -z "$OPERATOR" ]; then
    give_up "runtime.credential" \
      "no operator token to mint a member with. Run dev/probe-up.sh, or set IRONCLAW_MEMBER_TOKEN"
  fi
  minted="$(curl -s --max-time 20 -X POST "$API/api/webchat/v2/admin/users" \
    -H "authorization: Bearer $OPERATOR" -H 'content-type: application/json' \
    -d "{\"email\":\"confine-probe-$$@example.com\",\"display_name\":\"confine probe $$\",\"role\":\"member\"}")"
  # `api_token` is returned once and never again, so a mint whose token we failed to read is a
  # member we can no longer act as — BLOCKED, not a fallback to the env bearer, because falling
  # back would silently measure the caller this change exists to stop measuring.
  TOKEN="$(printf '%s' "$minted" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("api_token") or "")
except Exception: pass')"
  if [ -z "$TOKEN" ]; then
    give_up "runtime.credential" \
      "could not mint a member: $(printf '%s' "$minted" | head -c 200)"
  fi
  CALLER="a member minted for this run"
fi
if [ -z "$OPERATOR" ]; then OPERATOR="$TOKEN"; fi
printf 'prove-confinement: confining %s\n\n' "$CALLER"
# Separate variables on purpose. `Admin` resolves the operator token for the presence read and the
# member secret for every settings write, and the reconciler keeps three request methods apart so
# the two cannot be confused at a call site; a script that exports one value into both undoes that.
export IRONCLAW_API="$API" IRONCLAW_OPERATOR_TOKEN="$OPERATOR" IRONCLAW_MEMBER_TOKEN="$TOKEN"

# Seed against the member the credential ACTUALLY authenticates as, not the `seed` example's
# default literal. `confine` binds before it reads anything: it asks `/session` who the token is
# and refuses if that is not the member lifecycle recorded. Seeding a literal here made every check
# below fail on a mismatch this script had itself created — a proof failing on its own fixture
# rather than on the property, which is the shape check 1 exists to prevent elsewhere.
#
# The tenant is deliberately not consulted: the binding compares the user and never the tenant
# (`crates/lifecycle/src/runtime.rs`), so asking for more here would assert something the product
# does not.
MEMBER="$(curl -s --max-time 10 "$API/api/webchat/v2/session" -H "authorization: Bearer $TOKEN" \
  | python3 -c 'import json,sys
try: print(json.load(sys.stdin)["user_id"])
except Exception: pass')"
[ -n "$MEMBER" ] || { give_up "runtime.subject" \
  "/session did not name a user_id; cannot seed a member the credential can bind to"; }

# `member_minted` is what `lifecycle confine` needs and as far as a seed can honestly go: the next
# stage IS confinement, and certifying it takes evidence of a measurement that actually happened.
# A store seeded past this point would defeat the proof rather than set it up.
#
# The exit status is what is checked, not the file: `LifecycleStore::open` creates the database
# before anything is written to it, so `[ -f "$STORE" ]` passed even when the seed died partway —
# which it silently did, leaving this proof to spend a member measuring a store at the wrong stage.
cargo run -q -p ironworks-lifecycle --example seed -- "$STORE" "$SLUG" member_minted "$MEMBER" \
  >/dev/null 2>&1 \
  || { give_up "store.seeded" "could not seed a lifecycle store"; }

confine() { "$BIN" lifecycle confine "$SLUG" --store "$STORE" ${1:+--dry-run} 2>&1; }

# `settings/tools` as the member: the same read the reconciler makes.
catalogue() {
  curl -s --max-time 15 "$API/api/webchat/v2/settings/tools" -H "authorization: Bearer $TOKEN"
}
reachable_count() {
  catalogue | python3 -c 'import json,sys
try: d = json.load(sys.stdin)
except Exception: print("?"); raise SystemExit
e = d.get("entries") or []
print(sum(1 for x in e if x["key"].startswith("tool.") and x["value"].get("state") != "disabled"))'
}
set_state() {
  curl -s --max-time 15 -X POST "$API/api/webchat/v2/settings/tools/$1" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d "{\"state\":\"$2\"}" >/dev/null
}
set_approval() {
  curl -s --max-time 15 -X POST "$API/api/webchat/v2/settings/tools" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d "{\"enabled\":$1}" >/dev/null
}

# Exit 0 when automatic approval is on. Read from the catalogue rather than inferred from the
# write, because the write is exactly what cannot be trusted here.
approval_is_on() {
  catalogue | python3 -c 'import json,sys
try: d = json.load(sys.stdin)
except Exception: raise SystemExit(1)
v = [x for x in (d.get("entries") or []) if x["key"] == "agent.auto_approve_tools"]
raise SystemExit(0 if v and v[0]["value"] is True else 1)'
}

# No approval preflight here, deliberately. §L-1: a fresh member's SETTINGS CATALOGUE starts open
# and carries `agent.auto_approve_tools: true`, so gating on approval-off would refuse every clean
# probe. This says nothing about completeness over the model surface.

# ── 1 ───────────────────────────────────────────────────────────────────────────────────────
# The guard. Everything below asserts a state of the catalogue, and an unreadable or empty one
# would satisfy all of it — so the run starts by proving this member is measurably NOT confined.
before="$(reachable_count)"
out="$(confine --dry-run)"
case "$out" in
  *"REFUSING TO APPLY"*) verdict "non_vacuity.fresh_member_is_open" BLOCKED "catalogue unreadable" ;;
  *) case "$before" in
       ""|"?") verdict "non_vacuity.fresh_member_is_open" BLOCKED "could not count reachable tools" ;;
       0)      verdict "non_vacuity.fresh_member_is_open" FAIL \
                 "nothing was reachable before applying; every check below would pass vacuously" ;;
       *)      verdict "non_vacuity.fresh_member_is_open" PASS "$before tool(s) reachable before" ;;
     esac ;;
esac

# ── 2 ───────────────────────────────────────────────────────────────────────────────────────
after_dry="$(reachable_count)"
if [ "$after_dry" = "$before" ]; then
  verdict "dry_run.writes_nothing" PASS "still $after_dry reachable"
else
  verdict "dry_run.writes_nothing" FAIL "--dry-run changed the catalogue: $before -> $after_dry"
fi

# ── 3 ───────────────────────────────────────────────────────────────────────────────────────
applied="$(confine)"
case "$applied" in
  *VERIFIED*) verdict "apply.reaches_policy" PASS \
                "$(printf '%s' "$applied" | grep -oE '[0-9]+ catalogued, [0-9]+ reachable, approval [a-zA-Z]+' | head -1)" ;;
  *DRIFTED*)  verdict "apply.reaches_policy" FAIL "still drifted after applying" ;;
  *BLOCKED*)  verdict "apply.reaches_policy" BLOCKED "could not measure after applying" ;;
  *)          verdict "apply.reaches_policy" FAIL "unrecognised outcome: $(printf '%s' "$applied" | tail -1)" ;;
esac

# ── 4 ───────────────────────────────────────────────────────────────────────────────────────
# Idempotence is what makes this safe to run on a schedule, and it is a property of `plan`
# containing only what is not already at policy.
again="$(confine)"
case "$again" in
  *"already at policy"*|*"0 step(s) written"*)
    verdict "apply.is_idempotent" PASS "a second run wrote nothing" ;;
  *VERIFIED*)
    verdict "apply.is_idempotent" FAIL "verified, but it wrote again on an unchanged member" ;;
  *) verdict "apply.is_idempotent" FAIL "unrecognised second outcome" ;;
esac

# ── 5 ───────────────────────────────────────────────────────────────────────────────────────
# Break it on purpose. A reconciler that always said VERIFIED would have passed 3 and 4.
# From here to the end of check 6 the member is deliberately off policy; `cleanup` restores both
# settings if anything interrupts the run before their own restore lines are reached.
mutating=1
set_state "$REOPEN" "always_allow"
reopened="$(confine --dry-run)"
case "$reopened" in
  *"$REOPEN"*) verdict "detection.reopened_tool" PASS "$REOPEN reappears in the plan" ;;
  *)           verdict "detection.reopened_tool" FAIL \
                 "re-enabling $REOPEN was not detected; the verdict does not track the catalogue" ;;
esac
set_state "$REOPEN" "disabled"

# ── 6 ───────────────────────────────────────────────────────────────────────────────────────
# Approval is the one setting that decides whether `ask_each_time` means anything, which is why
# it is step one of the plan rather than the last thing written.
set_approval true
approval_back="$(confine --dry-run)"
case "$approval_back" in
  *"automatic approval off"*) verdict "detection.approval_reenabled" PASS \
                                "approval-on is detected and planned first" ;;
  *) verdict "detection.approval_reenabled" FAIL \
       "approval was turned back on and nothing noticed" ;;
esac
set_approval false

echo
summary
