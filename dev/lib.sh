# shellcheck shell=bash
# Shared by the dev/prove*.sh harnesses. Sourced, never executed.
#
#   # shellcheck source=dev/lib.sh
#   source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
#
# as the first two lines after the header, before anything else. It sets the shell options, puts the
# caller at the repository root, and defines the scoring vocabulary every proof uses.
#
# # Why this file exists
#
# There was no shared file here, and eleven copies of `verdict` had drifted into four column
# widths and three counter-naming schemes for one convention the documents describe as uniform.
# That was untidy. What made it worth fixing is what drifted underneath it: `degraded` — the
# predicate that decides whether an unavailable dependency scores BLOCKED or FAIL — was fixed in
# `dev/prove-delivery.sh` and in `dev/prove-cutover.sh`, which had copied it, and never in
# `dev/prove.sh`. The comment recording the fix names `dev/prove.sh` as the run that trips the
# rate limit in the first place, so the one script most likely to meet the condition was the one
# scoring it as a defect in IronWorks. A fix that lands in two of three copies is the failure this
# file removes the possibility of.
#
# # `set -uo pipefail`, deliberately without `-e`
#
# A proof script's failing command IS its measurement. Under `-e` the first FAIL would abort the
# run before it could be scored, reported or counted — so a check that fails must return, not
# exit. Stated once here rather than silently in twelve copies. `dev/relay-loop.sh` says the same
# thing for the same reason and is not a proof script.

set -uo pipefail

# ── the repository root ─────────────────────────────────────────────────────────────────────
# Computed from *this* file, which is why a caller needs only its own directory to get here. The
# `|| return` guard is the point: without it a failed `cd` leaves the run in whatever directory it
# started from, and every relative `fixtures/…` path silently misses — under `-u` with no `-e`,
# quietly and with a plausible-looking failure. Four scripts omitted it.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || return 3
cd "$ROOT" || return 3

# ── scoring ─────────────────────────────────────────────────────────────────────────────────
# PASS / FAIL / BLOCKED, exiting 0 / 2 / 3. **BLOCKED is never a pass**: a measurement that could
# not be performed has established nothing (`docs/INVARIANTS.md` §F-14), and it is never a FAIL
# either — scoring an unavailable dependency as a defect in IronWorks trains an operator to skim
# the ones that are real.
pass=0
failed=0
blocked=0

verdict() {
  printf '%-9s %-34s %s\n' "$2" "$1" "${3:-}"
  case "$2" in
    PASS) pass=$((pass + 1)) ;;
    FAIL) failed=$((failed + 1)) ;;
    BLOCKED) blocked=$((blocked + 1)) ;;
  esac
}

# The tally and the exit code, with any closing notes as arguments.
#
# **A blocked run exits 3 whatever else happened**, which is the whole reason this is one function
# rather than three lines per script: a run that could not measure must not exit 0 beside the word
# PASS, and must not report a measured failure while hiding that it was also incomplete. One copy
# had no `exit 3` at all, and stayed correct only because every BLOCKED it could reach exited on
# its own first — a property one new check would have quietly ended.
# shellcheck disable=SC2120  # callers pass closing notes; lib.sh itself has no call site.
summary() {
  printf '\n%d passed, %d failed, %d blocked\n' "$pass" "$failed" "$blocked"
  local note
  for note in "$@"; do
    printf '%s\n' "$note"
  done
  # Blocked outranks failed, the ranking `crates/eval/src/main.rs::exit_code` already applies and
  # tests by name: "some of this was never measured" is the fact an operator has to see first
  # (§F-14). The two must agree — one question with two answers is the defect a shared file exists
  # to remove, and these are the same question asked by a script and by a binary.
  [ "$blocked" -gt 0 ] && exit 3
  [ "$failed" -gt 0 ] && exit 2
  exit 0
}

# ── reading a turn's output ─────────────────────────────────────────────────────────────────

# States in which a dependency was unavailable, so the check measured nothing. BLOCKED, not FAIL.
#
# The model PROVIDER, upstream of the runtime, rate limits: measured, the runtime answers
# /api/health in ~2ms while its own log reports `Provider nearai_chat rate limited`. The runtime
# itself refuses a turn it cannot finish inside `DEFAULT_RESPONSES_WAIT_TIMEOUT` (30s) with a 503,
# which is the runtime declining rather than any boundary failing — 11 refusals in 17 asks of the
# shipped question, recorded in
# `docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md`. None of that is
# evidence about the chain, and a proof scoring it FAIL reports a defect in IronWorks for
# something upstream of it.
#
# **`429` is anchored, not prefixed, and both halves cost a measurement to learn.** A bare number
# matches inside a hex digest, and these scripts print an alloy digest, a request digest and a
# `resp_` id per check: two of eight sittings in `dev/prove-cutover.sh` blocked on runs that had
# delivered, both on a token like `request 4297bd26b3b174a1`. Requiring the literal `HTTP 429`
# instead was the first correction and it dropped a proxy's own `429 Too Many Requests` page,
# which carries the number and nothing else on its line — scoring an environment failure as FAIL,
# the worse of the two errors. Matching only where the number is not inside a hex run keeps both
# `HTTP 429: ` (`crates/records/src/append.rs`) and the bare proxy page.
#
# **This does not make a run a pass.** A BLOCKED end-to-end check is still the positive control
# the isolation refusals need, so a sitting that blocks it measured nothing about isolation either.
#
# **The first half of the pattern is the shell spelling of `ironworks_ironclaw::Cause` —
# specifically of `crates/ironclaw/src/classify.rs::Cause::blocks_measurement`.** It was written by
# hand and had drifted: `Cause::ReceiveTimeout` renders "timed out awaiting the response", which
# `timed out (writing|reading)` does not match, so the runtime's ~30s synchronous limit — the most
# common blocked condition there is — was scored FAIL by every script here. `Connect` and
# `BudgetExhausted` missed the same way; only `SendTimeout` happened to match. The Rust test
# `the_shell_agrees_about_which_causes_block_a_measurement` now reads this function and greps this
# pattern against every cause's words — each kind carrying relayed provider prose as well as its
# own — and `blocks_measurement` reads the second half's alternatives too, so neither half can drift
# from the Rust silently. The second half stays hand-written: it matches what the *provider* says
# through the runtime, which is prose this repository does not author and cannot enumerate.
degraded() {
  grep -qE "did not resolve|refused or unreachable|refused to tunnel|TLS handshake failed|timed out (writing|awaiting)|turn budget ran out|transport failure|rate limited|rate.?limit|(^|[^0-9a-f])429([^0-9a-f]|$)|provider .* unavailable" <<<"$1"
}

# §G-5's thirty-second cliff — **and it is not an outage**, which is why it is not in `degraded`.
#
# IronClaw's create call waits `DEFAULT_RESPONSES_WAIT_TIMEOUT` (30s) for the turn to *complete*
# and refuses a slower one with HTTP 503 `service_unavailable`. Nothing is unavailable: the
# submission already happened, the turn runs to completion server-side, and the answer is recovered
# by **replaying the idempotency key** (ADR 0005), never by polling — no response id was returned
# to poll. `docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md` measured
# the shipped service's own flagship question against it: eleven of seventeen refused, every
# refusal between 30.06s and 30.65s and every success between 26.5s and 30.0s, so which side a
# turn lands on is provider variance rather than configuration.
#
# It lived inside `degraded` for a while, which reported "dependency unavailable" for a turn that
# was still running — and sent a session looking for a provider incident that was not happening.
#
# **A harness retry is not the remedy, and that was measured rather than assumed.** Driving a
# second relay cycle in `dev/prove-delivery.sh` turned two BLOCKED checks into FAIL: both assert
# `creates == 1`, and a replay is a second create call at the adapter boundary (`create=2 keys=1`
# is the recovered-turn signature `dev/prove.sh`'s b3 check names). Worse for one of them —
# `deliver.known_unsent` exists to prove **"no automatic retry"**, so a harness that retries
# automatically contradicts the property it is measuring. `dev/prove-workspace.sh` and
# `dev/prove-freeze.sh` do retry, and may: they score an answer, not a call count.
#
# So the cliff stays BLOCKED wherever the count is the evidence. BLOCKED is the honest verdict
# there; what was wrong was calling it an outage.
#
# **Narrowing the question does not clear it either, and that was measured.** `0036` found the
# lever was the scope of the ask — one item cleared 9/10 where a brevity instruction cleared 1/3 —
# so `dev/prove-delivery.sh`'s whole-book question looked like the cause. Timed directly at this
# pin, 2026-09-10: a one-account ask and the forty-record ask both refused, **4 of 4, every one at
# 30-31s**. That is a different regime from `0036`, where completing turns fell between 26.5s and
# 30.0s and refusals between 30.06s and 30.65s — genuinely on the boundary. Here everything is past
# it whatever it asks, so the input size and the model's own reasoning tokens dominate, not the
# question. `0036` stands as a dated record; it is its *transferability* that does not.
#
# **And the obvious repair is worse than the problem, so it is written down here rather than
# rediscovered.** Counting unique keys instead of create calls would let a blocked check pass —
# `create=2 keys=1` is one logical turn — but in `deliver.known_unsent` the two relay cycles run
# against the *same* journal and the *same* call log, so `creates == 1` is precisely what proves
# the **second** cycle did not re-create. That assertion is not incidental to the check; it is the
# "no automatic retry" property the check is named for. Relaxing it to keys would make the one
# state it exists to catch indistinguishable from a pass.
outran_wait() {
  grep -q "service_unavailable" <<<"$1"
}

# Either shape means **this is not a measured result**, so no check may score it PASS or FAIL.
#
# Both are still BLOCKED where a check cannot retry, and the two must not be reported in the same
# words: one says a dependency is gone, the other says a turn is still running. Reporting the cliff
# as an outage sent this session looking for a provider incident that was not happening.
unsettled() {
  degraded "$1" || outran_wait "$1"
}

# Why a run is unsettled, in the words that fit which of the two it was.
unsettled_why() {
  if outran_wait "$1"; then
    printf 'the turn outran IronClaw'"'"'s 30s create wait (§G-5) — still running, recovers by replaying the key'
  else
    printf 'dependency unavailable: %s' "$(why "$1")"
  fi
}

no_model() { grep -q "not a valid model name\|model provider rejected" <<<"$1"; }

# The failing run's OWN last words, so a BLOCKED verdict names what stopped it. An earlier version
# grepped the first `ironworks:` line, which is the "scope verified" success message — so every
# FAIL reported the wrong sentence and a transient looked identical to a regression.
# The last substantive line of a refusal. Continuation lines are skipped as well as blank ones:
# the binary ends a refusal with indented `→` advice, so the last line is the advice rather
# than the reason on every script that reads one.
why() { grep -vE '^[[:space:]]*(→|$)' <<<"$1" | tail -n1 | cut -c1-88; }

# ── reading the adapter call log ────────────────────────────────────────────────────────────
# Counted at the runtime adapter boundary and appended to a file that outlives the process, so
# these survive a restart. What they can and cannot establish about model *executions* — as
# opposed to *calls* — is stated in `dev/prove.sh`.
# Missing evidence is not evidence of zero calls. Validate the whole append-only log before
# returning any count, including for a negative assertion. This also keeps a truncated or
# hand-written row from being interpreted as a call the adapter never recorded.
call_count() {
  local path="$1" wanted="$2" unique="${3:-no}"
  if [ ! -f "$path" ] || [ ! -r "$path" ]; then
    printf 'call log is missing or unreadable: %s\n' "$path" >&2
    return 2
  fi
  awk -F'\t' -v wanted="$wanted" -v unique="$unique" '
    NF != 2 || ($1 != "create" && $1 != "fetch") || $2 == "" { bad = 1; next }
    $1 == wanted && unique == "yes" { seen[$2] = 1; next }
    $1 == wanted { count++ }
    END {
      if (bad) exit 2
      if (unique == "yes") { for (key in seen) count++; }
      print count + 0
    }
  ' "$path"
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'call log is malformed: %s\n' "$path" >&2
  fi
  return "$rc"
}
creates() { call_count "$1" create; }
fetches() { call_count "$1" fetch; }
keys()    { call_count "$1" create yes; }
resp_of() { grep -oE 'resp_[0-9a-f]+' <<<"$1" | head -n1; }

# The devchannel's observation log. A proof asserts against **this**, never against the fixture's
# internals, so it is testing behaviour a real channel could also exhibit. Takes a base so the
# caller's `$CHAN` is a default rather than a hidden requirement.
obs() { curl -fsS --max-time 3 "${1:-$CHAN}/observations" 2>/dev/null; }

# ── preconditions ───────────────────────────────────────────────────────────────────────────
# A boundary that was never set up is BLOCKED, never FAIL: a missing credential or an unreachable
# dependency is not a defect in anything these scripts measure. Each ends the run through
# `summary`, so a precondition and a check reach the same exit code by the same path.

# Every named variable must be set before a turn is spent.
need_env() {
  local name
  for name in "$@"; do
    [ -n "${!name:-}" ] || {
      verdict "environment" BLOCKED "$name is unset — run dev/probe-up.sh and export what it prints"
      summary
    }
  done
}

# The runtime answering at all. Takes a base so a script pointing at something other than
# `$IRONWORKS_BASE` still shares the check rather than writing a fifth copy of it.
#
# `dev/prove-confinement.sh` deliberately does NOT use this: it falls back to `/health` when
# `/api/health` is absent, and widening that fallback to every caller would let a runtime serving
# only the legacy path satisfy a precondition four scripts currently require the real one for.
need_runtime() {
  local base="${1:-$IRONWORKS_BASE}"
  curl -fsS --max-time 3 "$base/api/health" >/dev/null 2>&1 || {
    verdict "runtime.reachable" BLOCKED "$base did not answer /api/health"
    summary
  }
}

# End the run on a boundary that was never set up. The counterpart to a check calling `verdict`
# and carrying on: a precondition has nothing to carry on to.
give_up() { verdict "$1" BLOCKED "$2"; summary; }

# ── waiting, and cleaning up ────────────────────────────────────────────────────────────────

# Poll a URL until it answers. Returns 0 when it did, non-zero when the ceiling was reached — the
# caller keeps its own failure branch, because "the fixture did not start" scores differently in
# different proofs.
#
# Written out seven times before this, with the ceiling varying between 30×0.2s and 60×2s for no
# stated reason and a different failure branch in each copy (`echo ""`, `return 1`, `give_up`).
# The ceiling and interval are arguments, so a caller that genuinely needs longer — a container
# boot rather than a process start — says so rather than quietly keeping its own loop.
wait_for_http() {
  local url="$1" tries="${2:-30}" interval="${3:-0.2}" max_time="${4:-1}" _try
  for _try in $(seq 1 "$tries"); do
    curl -fsS --max-time "$max_time" "$url" >/dev/null 2>&1 && return 0
    sleep "$interval"
  done
  return 1
}

# ── reading JSON ────────────────────────────────────────────────────────────────────────────

# `jq` as a stated precondition, so its absence is BLOCKED and not a wrong answer.
need_jq() {
  command -v jq >/dev/null 2>&1 || give_up "jq" "jq is not on PATH; these checks read JSON"
}

# One field out of a JSON document, by `jq` path. Empty when the path is absent.
#
# # Why not `grep`
#
# JSON was being read three ways across `dev/`: `jq`, inline `python3 -c`, and `grep -oE`. The
# last cannot tell a key from a value — `grep -o '"account_id":"[^"]*"'` matches just as happily
# inside a record body as in a listing — and it breaks on escaped quotes and ignores nesting
# entirely. `dev/prove-composition.sh` had both in *adjacent branches of one check*, reading the
# same field, one with `jq` and one with a grep. **No `grep` reads JSON in `dev/` any more**; every
# remaining `grep -o` there reads the binary's terminal output, which is a different question.
#
# The `python3 -c` readers are deliberately left. They are a correct JSON parser, so they are a
# third *style* and not a third correctness level, and they live in `prove-confinement.sh`,
# `prove-cutover.sh` and `prove-workspace.sh` — scripts that spend a member or real model turns, so
# a rewrite there could not be run before it shipped. Two of them are not field reads at all: one
# returns `?` to mark a document it could not parse, and one answers with an exit status. Routing
# those through here would mean giving a script that spends a one-way member a new precondition
# (`jq`) to buy nothing.
#
# Usage: json_field '<jq path>' "$body"
json_field() {
  jq -r "$1 // empty" <<<"$2" 2>/dev/null
}

# ── pacing ──────────────────────────────────────────────────────────────────────────────────

# Wait out the model provider's rate window between checks that each spend turns.
#
# The runtime allows **30 requests per 60 seconds, per user**, and one check is a create plus its
# polls — so two checks back to back is the most common cause of a BLOCKED run. The arithmetic was
# written once, in `dev/prove.sh`, above one of nine bare `sleep`s that all meant this; the other
# eight said nothing, and a reader tuning one had no way to know the rest were the same constant.
pace() {
  sleep "${PACE_SECONDS:-5}"
}

# A full rate window, for a run that has just spent most of one.
pace_window() {
  sleep "${PACE_WINDOW_SECONDS:-60}"
}

# Start a background fixture and wait for it to answer. Prints its pid; empty if it never came up.
#
# The five copies of this each wrote the same prologue —
#
#     ( exec "$BIN" … ) >/dev/null 2>&1 </dev/null &
#     PID=$!
#     disown 2>/dev/null || true
#
# — and none of the three parts is decoration. `exec` makes `$!` the fixture rather than the
# subshell, so the pid a cleanup kills is the process holding the port. `</dev/null` stops it
# inheriting the script's stdin. `disown` keeps a killed proof from printing job-control noise over
# its own verdict. A copy that dropped any one of them left a fixture holding its port, which is the
# state `dev/prove-tenancy.sh` records having been bitten by: *"a store left behind by a concurrent
# session answered here, its tokens differed, and five checks 'failed' against it."*
#
# # Why it does not use `wait_for_http`'s window
#
# Six seconds is a guess about how fast the machine is, and the guess is wrong under load: a
# `cargo test --workspace` in another session was enough to make `dev/prove-tenancy.sh` report
# BLOCKED for a store that was starting normally, and it passed 7/7 the moment the build lock was
# free. `dev/probe-up.sh` measured the same store at 11s, 23s and over 30s against a settling
# stack. So the ceiling here is generous, and waiting longer is kept from becoming the way a dead
# fixture is missed by watching the **process** rather than the clock — an exit ends the wait at
# once, whatever the ceiling says.
#
# Measured, because a liveness check that cannot see a dead process is worth nothing: a
# `devstore` given a malformed `--book` returns empty **at once** rather than at the ceiling, and
# one that stays up without answering returns empty at the ceiling. Both, and a fixture that comes
# up, are the three outcomes this has.
#
# Usage: pid="$(start_fixture <health-url> <command...>)"
start_fixture() {
  local url="$1"; shift
  ( exec "$@" ) >/dev/null 2>&1 </dev/null &
  local pid=$!
  disown 2>/dev/null || true
  local _try
  for _try in $(seq 1 "${FIXTURE_TRIES:-150}"); do
    curl -fsS --max-time 1 "$url" >/dev/null 2>&1 && { printf '%s' "$pid"; return; }
    kill -0 "$pid" 2>/dev/null || { printf ''; return; }
    sleep 0.2
  done
  kill "$pid" 2>/dev/null
  printf ''
}

# Install one cleanup on every way a run can end, rather than only on the tidy one.
#
# Eight of ten proof scripts installed `EXIT` alone. Ctrl-C on any of those left a `devstore` or
# `devchannel` holding its port — and `dev/prove-tenancy.sh` records what that costs: *"a store
# left behind by a concurrent session answered here, its tokens differed, and five checks 'failed'
# against it."* The two exit codes are the conventional 128+SIGINT and 128+SIGTERM, so a caller
# that inspects `$?` still sees which signal ended the run.
on_exit() {
  # shellcheck disable=SC2064  # `$1` MUST expand now: it is the caller's handler, and `$1` inside
  # a deferred trap would be the trap's own argument list, which is empty.
  trap "$1" EXIT
  # shellcheck disable=SC2064
  trap "$1; exit 130" INT
  # shellcheck disable=SC2064
  trap "$1; exit 143" TERM
}

# The disposable record store `dev/probe-up.sh` brings up.
need_store() {
  local port="${1:-$STORE_PORT}"
  curl -fsS --max-time 3 "http://127.0.0.1:$port/health" >/dev/null 2>&1 || {
    verdict "store.reachable" BLOCKED "no record store on 127.0.0.1:$port (dev/probe-up.sh)"
    summary
  }
}
