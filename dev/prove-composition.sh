#!/usr/bin/env bash
# COST: one measured model turn, and a model key; writes nothing and spends no member. It needs a
# probe, whose bring-up turn `dev/probe-up.sh` owns rather than this line double-counting it.
#
# Prove that the prompt IronClaw actually composed is the alloy IronWorks melted.
#
#   dev/prove-composition.sh      # needs a probe, a model key, and the operator token
#
# Six checks. Nothing here writes to the runtime: every request is a GET, and the one turn is the
# same turn `dev/prove.sh` drives.
#
#   1. control        one turn completes                   a response id and an alloy digest
#   2. run identity   where a run_id comes from            timeline, events, or neither
#   3. reachability   the inspector prompt route           200 to the operator token
#   4. bounds         the returned prompt is not truncated else BLOCKED, never FAIL
#   5. length         captured instructions = alloy bytes  the size the seam melted
#   6. structure      every composed part, in order        tail last, unnamed part absent
#   7. byte identity  not available through this route     BLOCKED, with the reason attached
#
# ── what question this answers ──────────────────────────────────────────────────────────────
#
# `ServiceInstance::alloy_digest` is a claim about what the seam **sent**. Nothing in this
# repository measures what the runtime **assembled** from it, and the gap is not hypothetical:
# `instructions` is sticky at the pin, IronClaw prepends it into the user message and persists it,
# and §L-2 measured a tools-enabled turn running the runtime's own loop around the envelope
# entirely. So "the alloy reached the model" has been an assumption with a digest beside it.
#
# The operator inspector is the only instrument that can settle it. Four operator-scoped routes
# exist at the pin (`ironclaw@4cb47cfaf3/crates/product/ironclaw_webui/src/webui_v2/descriptors.rs:268-275`),
# and one of them returns the reconstructed prompt with its components.
#
# **It settles less of it than it looks like it will.** The capture replaces every U+000A with
# U+0020, measured — so the byte count survives and the digest does not, and a digest comparison
# through this route cannot pass for any correct seam. Check 7 records that as BLOCKED rather than
# quietly dropping it, and checks 5 and 6 are what remains measurable: the exact melted length, and
# every composed part present in composition order with the safety tail last.
#
# ── what it does not answer, and cannot ─────────────────────────────────────────────────────
#
# **It does not close §I's "no turn can be attributed to a capability state".**
# `ironclaw@4cb47cfaf3/crates/contracts/ironclaw_product_contracts/src/inspector.rs:61` retains
# **four** runs per `(tenant, user, thread)` — a hard ceiling that `DiagnosticStoreLimits` may
# shrink and never raise. Four runs back is an instrument, not a record, and a proof that reads it
# is measuring the runtime's behaviour once rather than journalling anything.
#
# Check 6 carries its own negative control: a part the composition does not name must be absent
# from the capture. Without it, "every part is present" is a sentence a harness that matched
# nothing would also produce (§K-8). What no check here establishes is that the route *tracks* a
# composition change — that needs a second turn under different guidance, and this script does not
# spend one.
#
# ── why `jq` is required rather than grepped around ─────────────────────────────────────────
#
# No other proof here parses nested JSON, and the two defects this harness has actually shipped
# were both hand-written patterns over text — a bare `429` matching inside a hex digest (`0044`)
# and `timed out reading`, which nothing has ever emitted (`0050`). A grep over a components array
# can invent a match *or* invent a difference, and §K-8 records those as one defect. An absent `jq`
# is BLOCKED, which is the honest verdict for a measurement that did not happen.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The probe's own stack, named exactly as `dev/prove.sh` names it: this script measures the same
# turn that harness drives, and inventing a second slug, guidance or record store would measure a
# composition nothing else here runs.
STORE_PORT="${PROBE_STORE_PORT:-3996}"
STORE_TOKEN="dev-token-eval"
GUIDANCE="fixtures/eval.guidance.md"
SLUG="eval"
SERVICE="relationship-intelligence"
ASK="${PROVE_COMPOSITION_ASK:-Which account has the nearest renewal?}"

WORK="$(mktemp -d)"
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() { rm -rf "$WORK"; }
on_exit cleanup

# ── preconditions ───────────────────────────────────────────────────────────────────────
need_env IRONWORKS_BASE IRONWORKS_TOKEN IRONCLAW_API IRONCLAW_OPERATOR_TOKEN
need_runtime
need_store
need_jq
cargo build -q -p ironworks-cli 2>/dev/null || give_up "build" "cargo build failed"
BIN=./target/debug/ironworks
STORE="http://127.0.0.1:$STORE_PORT"

# GET as the operator, printing the body and appending the status code on its own line.
op_get() { curl -sS --max-time 20 -w '\n%{http_code}' \
  -H "Authorization: Bearer $IRONCLAW_OPERATOR_TOKEN" "$IRONCLAW_API$1" 2>/dev/null; }
# GET as the member the turn ran as — a thread projection is owner-scoped, not operator-scoped.
mem_get() { curl -sS --max-time 20 -w '\n%{http_code}' \
  -H "Authorization: Bearer $IRONWORKS_TOKEN" "$IRONWORKS_BASE$1" 2>/dev/null; }
status() { tail -n1 <<<"$1"; }
body()   { sed '$d' <<<"$1"; }

# ── 1. control: one turn completes and reports what it composed ──────────────────────────
out="$(IRONWORKS_RECORDS="$STORE" IRONWORKS_RECORDS_TOKEN="$STORE_TOKEN" \
  "$BIN" turn --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" --ask "$ASK" \
    --journal "$WORK/turn.db" --call-log "$WORK/turn.log" 2>&1)"; rc=$?
RESP="$(resp_of "$out")"
ALLOY_DIGEST="$(grep -oE 'alloy [0-9]+ bytes \([0-9a-f]{16}\)' <<<"$out" \
  | grep -oE '[0-9a-f]{16}' | head -n1)"
if no_model "$out"; then
  give_up "compose.control" "runtime has no usable model key"
elif unsettled "$out"; then
  give_up "compose.control" "$(unsettled_why "$out")"
elif [ $rc -eq 0 ] && [ -n "$RESP" ] && [ -n "$ALLOY_DIGEST" ]; then
  verdict "compose.control" PASS "$RESP composed alloy $ALLOY_DIGEST"
else
  verdict "compose.control" FAIL "rc=$rc resp='$RESP' digest='$ALLOY_DIGEST' | $(why "$out")"
  summary
fi

# ── 2. run identity: the inspector is addressed by (thread, run) and the seam holds neither ──
# Every response id is also a thread id, measured — `DESIGN.md`'s upstream contract. The run id is
# the open half: the projection known to carry it is the cursored event stream ADR 0002's pointer
# declines to license, and whether the timeline carries it too is the question that decides whether
# calling the working cancel route is a patch or an ADR. **This check is the input to that
# decision**, which is why it reports its source rather than only a verdict.
RUN=""; RUN_SOURCE=""
tl="$(mem_get "/api/webchat/v2/threads/$RESP/timeline")"
if [ "$(status "$tl")" = 200 ]; then
  # `turn_run_id` is the name the type declares
  # (`ironclaw@4cb47cfaf3/crates/domains/ironclaw_threads/src/contract.rs:253`). The first version
  # of this line looked for `run_id` and `runId`, found neither, fell through to the events
  # projection, and scored PASS — so the fallback hid the bug and the run reported a runtime fact
  # that was this line's defect. Search for the declared name, and never only for a guessed one.
  RUN="$(body "$tl" | jq -r '[.. | objects | (.turn_run_id // .run_id // empty)] | map(select(type=="string" and length>0)) | first // empty')"
  [ -n "$RUN" ] && RUN_SOURCE="timeline"
fi
if [ -z "$RUN" ]; then
  ev="$(mem_get "/api/webchat/v2/threads/$RESP/events")"
  if [ "$(status "$ev")" = 200 ]; then
    RUN="$(json_field '[.. | objects | (.turn_run_id // .run_id // empty)]
                       | map(select(type=="string" and length>0)) | first' "$(body "$ev")")"
    [ -n "$RUN" ] && RUN_SOURCE="events"
  fi
fi
if [ -n "$RUN" ]; then
  verdict "compose.run_identity" PASS "run id reachable from the $RUN_SOURCE projection"
else
  verdict "compose.run_identity" BLOCKED \
    "no run id from the timeline or the event stream; the inspector cannot be addressed"
  summary
fi

# ── 3. reachability: the prompt route answers the operator token ─────────────────────────
pr="$(op_get "/api/webchat/v2/operator/inspector/threads/$RESP/runs/$RUN/prompt")"
code="$(status "$pr")"
PROMPT_JSON="$(body "$pr")"
if [ "$code" = 200 ]; then
  verdict "compose.prompt_route" PASS "200 for (thread $RESP, run $RUN)"
elif [ "$code" = 404 ]; then
  verdict "compose.prompt_route" BLOCKED \
    "404 — a run beyond the four the host retains, or a route absent on this profile"
  summary
else
  verdict "compose.prompt_route" FAIL "expected 200, got $code"
  summary
fi

# ── 4. bounds: a truncated prompt cannot answer an identity question ─────────────────────
# The DTO carries its own truncation metadata, so this is a read rather than an inference:
# a component caps at 64 KB and the reconstruction at 256 KB, and the served alloy is nowhere near
# either — but an envelope is not the alloy, and a check that compared a truncated body would
# report a difference that is the instrument's, not the runtime's. BLOCKED, never FAIL.
# The witness comes first, and it is not ceremony. `[] | any` is `false`, so a payload carrying no
# truncation field at all would report "nothing was truncated" while having read nothing — the
# §K-8 defect, in the exact shape `0044` and `0050` both shipped. Absent metadata is BLOCKED.
FLAGS="$(jq -r '[.. | objects | (.truncated, .components_truncated) | select(type=="boolean")] | length' \
  <<<"$PROMPT_JSON" 2>/dev/null)"
TRUNC="$(jq -r '[.. | objects | (.truncated // empty)] | any' <<<"$PROMPT_JSON" 2>/dev/null)"
COMP_TRUNC="$(jq -r '[.. | objects | (.components_truncated // empty)] | any' <<<"$PROMPT_JSON" 2>/dev/null)"
if [ -z "$FLAGS" ] || [ "$FLAGS" -eq 0 ] 2>/dev/null; then
  verdict "compose.not_truncated" BLOCKED \
    "the payload carries no truncation metadata, so nothing here read one"
  summary
elif [ "$TRUNC" = "false" ] && [ "$COMP_TRUNC" = "false" ]; then
  verdict "compose.not_truncated" PASS "$FLAGS truncation flag(s) read, all false"
else
  verdict "compose.not_truncated" BLOCKED \
    "truncated=$TRUNC components_truncated=$COMP_TRUNC; identity is unanswerable on a clipped body"
  summary
fi

# ── 5. length identity: the captured instructions are the alloy's exact size ─────────────
# The alloy travels as `instructions` (ADR 0003 puts the envelope in `input`, not the persona), so
# the comparison is against that field and not against the whole prompt. `ironworks compose`
# reports the size the seam melted; the capture reports the size the runtime holds. A rewrite that
# preserved the byte count to the digit would be a remarkable coincidence, which is what makes an
# equality of lengths worth scoring on its own.
CAPTURED="$WORK/instructions.txt"
jq -r '.prompt.components[]? | select((.label.content // "") | startswith("Conversation"))
       | .content.content' <<<"$PROMPT_JSON" 2>/dev/null \
  | head -n1 | jq -r '.instructions // empty' >"$CAPTURED" 2>/dev/null
CAP_BYTES="$(wc -c <"$CAPTURED" | tr -d ' ')"
# `compose` re-melts what the turn melted, with no runtime and no model call.
ALLOY_BYTES="$("$BIN" compose --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" 2>/dev/null \
  | sed -n 's/^alloy \([0-9]*\) bytes .*/\1/p')"
if [ -z "$CAP_BYTES" ] || [ "$CAP_BYTES" -le 1 ] 2>/dev/null; then
  verdict "compose.length_identity" BLOCKED "no instructions field in the captured conversation"
  summary
elif [ -z "$ALLOY_BYTES" ]; then
  verdict "compose.length_identity" BLOCKED "compose reported no alloy size to compare against"
  summary
# `wc -c` counts the newline `jq -r` adds, which the alloy does not have.
elif [ "$((CAP_BYTES - 1))" = "$ALLOY_BYTES" ]; then
  verdict "compose.length_identity" PASS "captured instructions are $ALLOY_BYTES bytes, as melted"
else
  verdict "compose.length_identity" FAIL \
    "captured $((CAP_BYTES - 1)) bytes against an alloy of $ALLOY_BYTES"
fi

# ── 6. the parts arrived, in composition order ───────────────────────────────────────────
# Under the normalisation check 7 measures, so this asks about presence and order rather than
# bytes. `compose` names the parts and the tail; the tail must come last, which is the property
# §A rests on — a safety tail the model reads before the guidance is not a safety tail.
#
# **The needle is taken after frontmatter, because `melt` is.**
# `crates/domain/src/composition.rs::strip_frontmatter` drops a leading `---` block, so a needle
# cut from the top of `SKILL.md` on disk is text the alloy never contained — the first run of this
# check failed on exactly that and matched one part of three.
#
# **The negative control is that same frontmatter.** `version: 0.1.0` is in the file and must not
# be in the alloy. It is a control that can actually fire, which "a string nobody wrote" — the
# first version of this line — could not: §K-8 counts a vacuous control as the same defect as a
# missing one.
strip_fm() { awk 'NR==1 && $0=="---" {fm=1; next} fm && $0=="---" {fm=0; next} !fm' "$1"; }
flat_head() { strip_fm "$1" | tr '\n\t' '  ' | head -c 120; }
ORDER_OK=1; PREV=0; PARTS=0; MISSING=""
while read -r role path _digest; do
  case "$role" in part|tail) ;; *) continue ;; esac
  [ -f "scrap/$path" ] || { MISSING="scrap/$path"; ORDER_OK=0; break; }
  needle="$(flat_head "scrap/$path")"
  off="$(awk -v n="$needle" '{ i = index($0, n); if (i) { print i; exit } }' "$CAPTURED")"
  [ -n "$off" ] || { MISSING="$path"; ORDER_OK=0; break; }
  [ "$off" -gt "$PREV" ] || ORDER_OK=0
  PREV="$off"; PARTS=$((PARTS + 1))
done < <("$BIN" compose --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" 2>/dev/null)
STRIPPED=0
grep -qF 'version: 0.1.0' "$CAPTURED" && STRIPPED=1
if [ "$PARTS" -eq 0 ]; then
  verdict "compose.parts_in_order" BLOCKED "compose named no parts to look for"
elif [ "$ORDER_OK" = 1 ] && [ "$STRIPPED" = 0 ]; then
  verdict "compose.parts_in_order" PASS \
    "$PARTS composed part(s) present in order, tail last, frontmatter absent"
else
  verdict "compose.parts_in_order" FAIL \
    "matched=$PARTS order_ok=$ORDER_OK frontmatter_present=$STRIPPED missing='$MISSING'"
fi

# ── 7. byte identity, and why it is not available here ───────────────────────────────────
# Measured, not assumed: the capture replaces U+000A with U+0020 one for one, which is why the
# length survives and the digest does not. A digest comparison through this route can therefore
# never pass, at this pin, for any correct seam — so scoring it FAIL would report a defect in
# IronWorks for a property of the instrument. **BLOCKED is the honest verdict, and the check exists
# to keep the reason attached to the number.**
#
# What it does not establish: whether the *model* saw flattened text. This route shows what the
# diagnostic capture retained, and the capture is downstream of the send. Do not read a flattened
# capture as evidence the prompt was flattened in flight.
if grep -q '	' "$CAPTURED" || grep -qc '' "$CAPTURED" >/dev/null && [ "$(wc -l <"$CAPTURED" | tr -d ' ')" -le 1 ]; then
  verdict "compose.byte_identity" BLOCKED \
    "the capture holds no line breaks; whitespace is flattened, so a digest cannot be compared"
else
  verdict "compose.byte_identity" BLOCKED \
    "not attempted: this route is not byte-faithful at the pin (see this script's header)"
fi

summary "The four-run retention bound means this is an instrument, not a record:" \
        "§I's per-turn capability attribution is not closed by anything measured here."
