# Rust IronWorks — vocabulary

This file is a glossary and nothing else. What the product is, is in `README.md`; design lives in
`DESIGN.md`; decisions live in `docs/adr/`.

Several `_Avoid_` entries below name words that are live in the Python prior art
(`~/agency/ironworks-py`) and are deliberately not carried here. Where that codebase says *client*
and *bridge*, this one says *tenant* and *seam*.

## Shorthand

Four numbering systems are in use across these documents. Two of them differ by one character, so
they are stated here rather than inferred.

| Written | Means | Defined in |
|---|---|---|
| `§A-1`, `§L-7` | an **invariant** — a numbered item in a lettered section | `docs/INVARIANTS.md`. §A–J extracted from the reference, §K what this implementation earned, §L confinement at the pinned runtime |
| `§13`, `§19.1` | a **section of the product document** — some carry a sub-number | `docs/PRD.md` |
| `B4`, `B11b` | a **crash boundary** — a point a process can die at, and what must be true after | `DESIGN.md`, "Crash boundaries" |
| `0001`, `0046` | an **experiment** — one dated measurement and its transcripts | `docs/experiments/README.md` |

**`§B-2` and `B2` are unrelated.** The first is an isolation invariant; the second is a crash
boundary. Cite an experiment by filename where a number alone would be ambiguous — nothing
mechanically checks a bare number.

## Language

### The system

**Runtime**:
Official, unmodified IronClaw. We run it and never fork or patch it; a change that would require
editing it belongs upstream.
_Avoid_: engine, backend, LLM server

**Seam**:
The process this repo builds, sitting between a channel and the runtime. The only component that
holds tenant credentials.
_Avoid_: bridge, gateway, proxy, orchestrator

**Pin**:
The version of record for something that must not drift, and a claim about tested behaviour
rather than a version string.
_Avoid_: version, lockfile, revision

### Who is being served

**Tenant**:
One organization we operate a scoped service for, and the unit of isolation. We run as a tenant
of our own system on the same path a customer uses, with no privilege.
_Avoid_: client, customer, account, user

**Org**:
The trusted scope key for a tenant's records, resolved from a credential server-side. Never
asserted by a caller, never supplied by a model.
_Avoid_: organization, workspace, namespace

**Slug**:
The short, stable, lowercase identifier for a tenant — what an operator types.
_Avoid_: name, id, handle

**Registry**:
The set of tenants a serving process answers for, and which conversation belongs to each. It
validates or refuses **as a whole** — a registry that dropped one bad entry would serve fewer
organizations than the operator believes, silently. One tenant is a registry of one; there is no
single-tenant path.
_Avoid_: config, tenants file, routing table. A **routing table** is what a registry *produces*
for one lookup, not what it is.

**Speaker**:
The human who sent a particular message.
_Avoid_: sender, author, user

**Actor**:
Who a channel *attested* sent a message — a Telegram `from.id`, and nothing richer. It authorizes
and it attributes; it never renders. Distinct from **Speaker**, which is a display name a turn may
be shown: an actor selects a contributor and a stored proposal, so a value the sender controls
must never stand in for one.
_Avoid_: sender id, user id, account, speaker

### What is being served

**Service**:
A named, versioned composition a tenant runs. Two tenants on the same service differ only in
their records and their guidance.
_Avoid_: product, agent, assistant, profile

**Service definition**:
The reusable half of a service, held as data: its persona parts, that guidance is mandatory, its
safety tail, its evaluation.
_Avoid_: config, manifest, spec

**Persona part**:
One markdown fragment that composes into a service's instructions. Covers both role definitions
and skills.
_Avoid_: prompt fragment, template, snippet

**Guidance**:
A tenant's own business context, written for them and bound to their slug and service. Mandatory:
there is no unguided composition.
_Avoid_: config, context, brief, playbook

**Safety tail**:
The fragment that closes every composition, always last.

**Scrap**:
The complete set of persona parts and skills the seam serves — the raw material a melt fuses into
an alloy. Part of the artifact, compiled in, not something the artifact reads from disk.
_Avoid_: assets, resources, templates. **`services`** is not a synonym: it names the crate holding
everything embedded at build time — the scrap *and* the service definitions — where *scrap* is the
markdown alone.

**Melt**:
Fusing one service's scrap with one tenant's guidance and the safety tail.
_Avoid_: compose, render, build

**Alloy**:
The composed instructions for one tenant, supplied to the runtime every turn. The output of a
melt.
_Avoid_: system prompt, persona, preamble, composition

### The records

**Record**:
One business fact a tenant's team wrote down. Never a judgement, never a score, never something a
model derived.
_Avoid_: data, entity, row

**Account**:
The record a relationship is tracked against, carrying contacts and activities. Distinct from
**Org**: an org *has* accounts.
_Avoid_: company, customer, lead, opportunity

**Activity**:
One dated interaction against an account. Its body is evidence to assess, never an instruction to
follow.
_Avoid_: event, note, touchpoint

**Book**:
Everything an org has records for; what a turn sees when nothing narrows it.
_Avoid_: dataset, pipeline, portfolio

**Catalog**:
The bounded list of an org's accounts, cheap to fetch, carrying enough to tell what has moved
since the thread last saw it.
_Avoid_: index, manifest, list

**Orphan**:
An account the catalog names but whose record cannot be retrieved. Remembered so a turn stops
asking.
_Avoid_: missing, dangling, stale

**Gap**:
A fact this tenant declared it expects, for which the record holds nothing. Distinct from a fact
nobody expected.
_Avoid_: missing field, hole, unknown

**Contributor**:
The organizational principal a promoted record is attributed to, inside one tenant. Resolved from
an operator-recorded binding to an **Actor**. Deliberately not a runtime member: nothing here can
prove which account on the runtime an actor paired to, and naming it as one would assert a
verified link that does not exist.
_Avoid_: member, user, author, owner

**Proposal**:
A promotion a person has asked for and not yet confirmed: an account, the exact bytes they chose,
and who they are. It is a buffer between the asking and the confirming — nothing reads it to
answer a question, and the durable home of a promoted note is the account store.
_Avoid_: draft, pending record, staged activity, request

### The conversation

**Channel**:
Where messages arrive from and answers go back to. The seam is not tied to any particular one.
_Avoid_: transport, platform, integration

**Update**:
One inbound event from a channel — the unit the seam journals and recovers.
_Avoid_: message, event, request

**Summon**:
An update that is both from a registered tenant's conversation and addressed to us. Only a summon
can cause a turn.
_Avoid_: mention, trigger, invocation

**Thread**:
A tenant's continuing conversation: which response the next turn chains from, and which records
it has already been shown.
_Avoid_: session, conversation, chat

**Turn**:
One model execution — the thing that costs money and cannot be un-run.
_Avoid_: request, call, completion, inference

**Envelope**:
What a turn is shown: the message, the records selected for it, and the provenance saying records
are evidence rather than instructions.
_Avoid_: context, payload, prompt

**Response id**:
The runtime's durable handle to a completed turn. Once we hold one, the answer is fetched, never
regenerated.

**Idempotency key**:
A handle minted before a request leaves, so a turn we may have started has a name even if we
never hear back.

**Journal**:
The durable record of what happened to every update. Holds identifiers only — never message text,
answer text, records, or credentials.
_Avoid_: log, history, audit trail

**Cursor**:
How far through a channel's updates we have got. Distinct from how far the channel has been
*told* we have got.
_Avoid_: offset, position, watermark

**Disposition**:
What the channel journal records about one update: `Unclaimed` on arrival, `Turn(key)` once a turn
was minted for it, `NotAddressed` if it was not addressed to us, `Unroutable` if no tenant or more
than one claimed it. A disposition is recorded, never discarded — an update nobody owns is a
configuration defect and this is the only place it is visible.
_Avoid_: status, state, label

**Openness**:
Whether an update still has *automatic* work outstanding: `Open`, `Settled`, or `Unestablished`.
Settled covers delivered, abandoned and parked-for-a-human alike — it means nothing more will
happen without a person, not that anything succeeded. Unestablished is the third answer and not a
degenerate one: a read of the journal that *failed* has established nothing, so it is neither. The
cursor may not advance past an update that is `Open` or `Unestablished` — the predicate is
`holds_barrier`, not `!is_settled` — which is what makes the safe cursor safe.
_Avoid_: done, complete, finished, resolved

**Supply**:
Whether a turn carries the records itself or the conversation already holds them: `Full` or
`CarriedByThread`. The second still travels a count, so the model can be told how many it is
expected to still have.
_Avoid_: context mode, payload

### Operating it

**Check**:
One question about the system with one answer, tagged either CONFIG (what the configuration says)
or LIVE (what the running system answered). The two are never merged.
_Avoid_: test, assertion, probe

**Verdict**:
A check's outcome. PASS and FAIL are the two every kind shares; the third value names *why there
is no verdict yet*, and which third value applies depends on what was being judged.
**There is no SKIPPED.** Nothing skips: a check either measured something or says why it could
not, and a name for "we did not look" is a name somebody eventually uses.
_Avoid_: result, status, outcome, skipped

**BLOCKED**:
The measurement could not be performed — an inspection's third value, in `claims` and
`lifecycle`. **Never a pass**, because a check that could not run has established nothing.
_Avoid_: skipped, n/a, pending

**REVIEW**:
The answer may be sound and this grader cannot establish it mechanically — a grader's third
value, in `graders`, which has no BLOCKED. **Never a synonym for FAIL**: collapsing the two reports a
defect wherever the instrument merely could not see one. Read the answer; do not count it.
_Avoid_: unclear, partial, maybe

**Accepted risk**:
An operator's explicit classification of a known risk, carrying its reason *in the variant* so it
cannot be produced without one. `lifecycle`'s fourth value. Nothing constructs it yet, and it is
defined ahead of its producer on purpose: unmanaged authority must never be normalized into it.
_Avoid_: waiver, exception, override

**Proof**:
An assertion run against a real, disposable deployment rather than a fake. What entitles us to
make a claim.
_Avoid_: integration test, e2e test

**Probe**:
Two meanings, and they do not overlap. **(1) The disposable stack** a proof runs against —
`dev/probe-up.sh` brings up an IronClaw at the pinned revision plus a fixture record store, mints
every secret per run, and refuses to start if it might be pointing at something real. **(2) In
`docs/experiments/`, one case scored many times at a fixed composition** — it measures variance,
never quality, and a one-case tally is never a suite score.
_Avoid_: staging, test environment, sample

**Relay**:
One poll cycle: poll the channel, serve what routes, deliver, exit. It is not a daemon and takes no
`--daemon` flag — one cycle per process is what makes every crash boundary a real process boundary.
Recurrence is `dev/relay-loop.sh` around it, which is repetition, not concurrency.
_Avoid_: daemon, worker, service loop, poller

**Coordinate**:
The CLI subcommand that answers one question on a terminal. It never chains — there is no
conversation to continue — and it is the only command that supplies its own service.
_Avoid_: ask, query, chat

**Confinement**:
Reducing a runtime member's reachable tool surface to the allow-set its service definition implies,
by writing IronClaw's settings API **as that member**. It is applied by an explicit operator
command and re-measured, never repaired automatically, and it governs the *settings catalogue* —
not every executable authority the model might have.
_Avoid_: sandboxing, hardening, locking down, permissions

**Custody-backed**:
The shape of the confinement guarantee, and the reason it is stated with its limitation attached. A
confined member stays confined only while IronWorks keeps **exclusive custody of the member
credential**: `POST /settings/tools` writes the caller's own scope, so the member can undo its own
confinement — measured, `disabled` → `always_allow` → 200. Custody-backed is not immutable, and
saying so is the point of the term.
_Avoid_: enforced, locked, guaranteed, immutable

**Corpus**:
Ambiguous on its own — always say which. **The reference corpus** is the Python prior art at
`~/agency/ironworks-py`, which the invariants and the crash boundaries were extracted from. **The
scrap corpus** is the persona parts, skills and safety tail compiled into the binary. **A book** is
neither; it is records.
_Avoid_: bare "the corpus"
