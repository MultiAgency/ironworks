# The serving loop ran, and a chained turn could not recover

Date: 2026-09-02. **First continuous run of `dev/relay-loop.sh` against a real runtime. Two of
three questions answered and delivered, one of them recovered from a real upstream 503 by
byte-identical replay — ADR 0005 working unprompted rather than provoked. The third took the same
503 and is permanently unrecoverable, because recovery rebuilds a chained turn as a fresh one. The
loop then refused it on every subsequent cycle, holding the cursor, indefinitely.**

## The run

Probe at `4cb47cfaf3` (`production` profile), `devstore` on the ten-record account-coordination
book, `devchannel` carrying three questions in one private conversation, one tenant in the
registry. Four cycles, each a fresh `relay` process, `RELAY_IDLE=2`.

| update | thread | envelope | outcome |
|---|---|---|---|
| 41 | fresh | 6,111 bytes, 10 records | 503 on cycle 1; **recovered on cycle 2**, answered, delivered |
| 42 | chained on 41 | 277 bytes, carried by thread | answered, delivered |
| 43 | chained on 42 | 272 bytes, carried by thread | 503; **unrecoverable**, refused on cycles 3 and 4 |

Update 41's recovery is the part worth stating plainly, because it is the first time the property
has been observed outside a `--crash-at` harness:

```
relay: update 41 -> key upd-41
ironworks: the runtime refused it: service_unavailable … (a turn MAY have run)
relay-loop: cycle 1 exited 2, waiting 2s
…
ironworks: rebuild is byte-identical; replaying the same key
alloy 25575 bytes (3770e0942624bda4) · envelope 6111 bytes · request b33c77ed899c8ddc · key upd-41
response resp_6669ee1b18aa4fc8abd4809f8dba2e47 (running)
usage: 10046 in (0 cached), 1503 out, cost 0.040145
relay: update 41 delivery=delivered
```

Same request digest, same key, one logical turn, an answer delivered on the next cycle. Nothing
staged that 503 — the provider produced it.

## The defect

Update 43 was chained. Its recovery refuses, every cycle, forever:

```
ironworks: REFUSING TO SERVE. The journalled turn upd-43 cannot be reconstructed
byte-identically — the records or the question have moved since it was recorded …
```

**The records had not moved and the question had not changed.** The message is a guess about a
cause, and it is the wrong one. `crates/cli/src/lib.rs::drive`:

```rust
let previous = match (&plan, conversation) {
    (None, Some(room)) => { /* … decide continuation … */ }
    // No room to continue, or a recovered turn that already decided.
    _ => None,
};
```

`plan` is `Some` exactly when this is a recovery. So on recovery the arm never matches and
`previous` is `None` — and `prior_supplied` is assigned only inside that same arm, so
`supply::decide(false, None, …)` returns `Full` as well. A recovered turn is therefore rebuilt as
**fresh and fully supplied**, whatever it originally was.

For update 41 that is correct by accident: it *was* fresh and fully supplied, so the rebuild
matched and ADR 0005 worked. For 43 it produces a different body on two independent axes — no
`previous_response_id`, and a 6 KB records envelope where the original carried 272 bytes — so the
digest cannot match and the replay is refused locally before it reaches the runtime.

The comment states the right rule. *"A recovered turn is NOT re-decided: it already chose, its
body is journalled, and a second decision could produce a body the idempotency key no longer
names."* The implementation of "do not re-decide" is "decide nothing", which is a different
decision rather than none. **What the turn chose is not journalled** — the digest is, but the
inputs that produced it are not, so nothing can reproduce them.

> **ADR 0005 recovery works for fresh turns and is structurally impossible for chained ones.**

That is not a rare corner. `DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30 seconds and the runtime answers
503 to any turn that outlasts it, so on a book of any size the ambiguous-failure path is ordinary
traffic — and every turn after the first in a conversation is chained.

## The second defect, which is worse

A turn that can never be reconstructed is never **settled**. Cycles 3 and 4 re-poll from offset 43,
re-journal the same batch, refuse again, and back off — 4s, 8s, 16s, toward the 120s cap:

```
relay: poll offset=43
relay: batch of 1 journalled
ironworks: REFUSING TO SERVE …
relay-loop: cycle 4 exited 2, waiting 16s
```

`ironworks status` reports it still open:

```
key=upd-43 … outcome=submitted_unknown delivery=pending response=-
```

`DESIGN.md` says this case was designed out:

> **`RecoveryBlocked` became `OperatorRequired`, and settling it is what keeps the bridge
> listening.** A turn nobody will act on again must stop holding the shared cursor, or one stuck
> conversation deafens every conversation behind it while the process looks healthy.

The mechanism exists and **this path does not use it**. The refusal is an `Err` out of the serving
function; nothing records `OperatorRequired`, so the cursor never advances past 43 and every later
message in that conversation — and, because the safe cursor is the lowest open update across every
tenant, in every *other* conversation too — is never answered. The process looks healthy. The loop
backs off politely. Nobody is being served.

This is the exact failure the design names, reached by a route the design did not anticipate: not a
turn nobody will act on, but a turn nobody *can* act on automatically.

## What the fix is, and what it owes

**The contained half** — settle it. A rebuild that cannot match will never match; it is not a
transient. Recording `OperatorRequired` releases the cursor and keeps the alarm, which is what the
state is for. That does not recover the answer, and should not pretend to.

**The real repair** — journal the continuation decision. `begin` records the key and the digest;
it must also record what the turn chose: the `previous_response_id` it chained from, and the
supply state it was built under. Then recovery *replays* the decision instead of recomputing or
nulling it, and the rebuild matches for a chained turn as it already does for a fresh one. That is
a schema change and it owes a crash-boundary proof of its own — B3 across a chained turn, which no
current proof exercises, because `dev/prove.sh` runs `turn` and `turn` never chains.

**And the refusal message must stop naming a cause it has not established.** "The records or the
question have moved" was written for the case where they had; here nothing moved and the operator
is sent to look at a record store that is fine. It should say what actually differs.

## The contained half, fixed and verified against the stuck journal

`cli` now settles a turn whose body cannot be rebuilt, instead of refusing it every cycle. The
verification is the same journal the run above left behind, not a reconstruction:

```
── before:
key=upd-43 … outcome=submitted_unknown delivery=pending response=-

── one cycle with the fix:
ironworks: REFUSING TO SERVE. The journalled turn upd-43 cannot be reconstructed byte-identically…
  recorded digest ee2d6c84a7b1c233… · rebuilt 20b0c026b92449a0…
  It is now journalled OperatorRequired, so it no longer holds the channel cursor.

── after:
key=upd-43 … outcome=operator_required … reason=the journalled body cannot be rebuilt: …

── next cycle (was pinned at offset=43 forever):
relay: poll offset=44
relay: batch of 0 journalled
exit: 0
```

**`poll offset=43` became `poll offset=44`, and the cycle exits 0 instead of 2.** The conversation
is listening again. The answer to update 43 is still lost and the message says so; what changed is
that losing one answer no longer costs every answer after it.

The refusal also stopped naming a cause it had not established. It now prints the recorded and
rebuilt digests rather than asserting that records or the question moved — which, in the run that
produced it, was false and would have sent an operator to inspect a record store that was fine.

## Both halves repaired, and the defect made deterministic first

The finding above rested on one accidental 503, which is a thin basis for the phrase *structurally
impossible*. `dev/prove-continuation.sh` now reproduces it on demand: it answers one turn, chains a
second, kills the process at `--crash-at before-response` — the state a 503 produces — and asks the
next process to recover it. Against the code as `0033` found it:

```
PASS  chain.established   update 27453 answered and delivered
PASS  chain.is_real       chained, and the book was not re-sent
PASS  chain.crashed       upd-27454 is submitted_unknown (exit 75)
FAIL  chain.recovers      a chained turn cannot be rebuilt — the decision it made was not journalled
PASS  cursor.released     offset advanced to 27455, past upd-27454
```

So the claim is measured rather than inferred, and `cursor.released` shows the settling fix already
holding at the point the recovery still failed. Check 2 exists so check 4 cannot pass vacuously: a
regression to always-`Fresh` would make every rebuild match while proving nothing.

**The repair is to journal the decision.** `turns` gains `prev_response` and `carried_records`
(schema 6), `Journal::begin` takes a `Decision`, and the recovery path replays it instead of
recomputing. The chained-from **response id** is stored rather than the pointer: a `ThreadPointer`
is rebuilt through the recovering tenant's own `ThreadScope`, so a pointer naming another tenant
stays unrepresentable rather than merely unwritten. `NULL` in either column is a value — fresh, and
supplied in full — which is exactly what the rows that always recovered correctly were.

After it, on the same proof:

```
PASS  chain.established   update 27769 answered and delivered
PASS  chain.is_real       chained, and the book was not re-sent
PASS  chain.crashed       upd-27770 is submitted_unknown (exit 75)
PASS  chain.recovers      rebuilt byte-identically, replayed, delivered
PASS  cursor.released     offset advanced to 27771, past upd-27770
```

**One correction to the settling half, found by reviewing it rather than by running it.** Settling
on a bare digest mismatch is too strong: `fetch_records` degrades on a store outage that happens
*after* verification, so a rebuild during that window produces a degraded envelope and mismatches
for a reason that has nothing to do with the turn. Settling there would let a store blink
permanently destroy a turn the next cycle would have recovered — trading indefinite deafness for
silent loss, which is not obviously the better trade.

So the verdict is three-valued, not two. `Matches` replays; `Differs` — a mismatch on a book we
actually read — settles; `Unestablished` refuses **without** settling, because a degraded read
establishes nothing about the body and the next cycle can re-read. That keeps the rule this
codebase already states: controls fail closed, data sources degrade, and a degraded data source
must not produce a permanent verdict. The window is narrow — `Book::resolve` fails closed, so a
store that is simply down never reaches the rebuild — but it is exactly the window
`dev/prove.sh` check 5 exercises, so it is not hypothetical.

**Schema 6 refuses an older journal rather than migrating it.** That is the existing fail-closed
rule, and it is right here: a store written at schema 5 has no record of what its turns decided, so
opening it under this code would mean recovering them by the guess this change exists to remove.

## What this run does not establish

- **Three questions, one conversation, one tenant.** Nothing about concurrency, multiple tenants,
  or the cross-tenant cursor barrier under load.
- **Four cycles over about two minutes.** Not an uptime claim.
- **The `production` profile probe**, not a served deployment. Nothing here was deployed anywhere;
  `deploy/README.md`'s host still does not resolve from this workstation (re-checked: NXDOMAIN, no
  ssh entry, no `hcloud` context, none of the three paths present).
- **Cost.** Two answered turns, $0.040145 on the one that reported usage.
