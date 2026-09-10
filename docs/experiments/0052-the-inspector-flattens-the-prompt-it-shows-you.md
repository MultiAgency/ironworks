# 0052 — the inspector flattens the prompt it shows you

**2026-09-06.** The first runs of `dev/prove-composition.sh`, against disposable probes at
`4cb47cfaf3` — first on the `production` profile (findings 1, 2, 3, 3b), then the whole harness
again on the served `hosted-single-tenant-volume-sandboxed` profile (finding 3c). **Seven model
turns in total**, plus a discarded warm-up per bring-up; three of the seven returned 503 at the
cliff and were scored BLOCKED (§3b). No member was minted, nothing was written to either runtime,
and no live host was touched: every request after a turn is a `GET`.

It was written to answer one question — **is the prompt IronClaw composed the alloy IronWorks
melted?** — and the answer is that the route this repository would have used to ask cannot answer
it, for a reason worth writing down. What it *can* answer, it did, on both profiles.

**Most of what this record contains is the instrument failing rather than the runtime.** Four
defects in the harness (§4), one of them serious enough to have produced a false correction of a
correct finding and to be withdrawn here (§3). That is the pattern
[`0045`](0045-the-first-proof-of-the-serving-host-found-its-own-defect.md) established and the
reason a proof's first run is itself a measurement.

---

## 1. The capture replaces every newline with a space

`MEASUREMENT`

The alloy travels as `instructions` — ADR 0003 puts the envelope in `input` and leaves the persona
where the runtime expects it — and `ironworks_ironclaw::model::CreateBody` carries
`instructions: t.instructions().as_str()`, the `Alloy` unmodified. `ironworks compose` re-melts the
same composition with no runtime and no model call and reports **26,850 bytes, digest
`5e75679666c8bf7e`**; the turn's own telemetry line reported the same digest.

The operator inspector's prompt view returns that field. It is **26,850 bytes** there too — and
hashes to `d832cfe820fed275`.

| | |
|---|---|
| melted, per `ironworks compose` | 26,850 bytes · `5e75679666c8bf7e` |
| captured, per the prompt route | 26,850 bytes · `d832cfe820fed275` |

**Same length, different digest** is a same-length substitution, and it is exactly one:
`U+000A → U+0020`. Measured against the first composed part, whose bytes are on disk —
`COORDINATOR.md` with every newline replaced by a space **equals** the captured segment, and no raw
newline or tab survives anywhere in the captured field. The first difference is at byte 22 of the
first part, where `"# Account Coordination\n\nYou are"` is captured as
`"# Account Coordination  You are"`.

`reconstructed_prompt` is not a way around it. It is 57,978 bytes, does contain raw newlines, and
still holds the safety tail only in flattened form.

**So a digest comparison through this route cannot pass for any correct seam**, and scoring that
FAIL would report a defect in IronWorks for a property of IronClaw's diagnostic capture. Byte
identity is **BLOCKED** at this pin. The check stays in the harness rather than being deleted,
because the reason has to stay attached to the number.

**What this does not establish, and the distinction is the whole point.** It says what the
*capture* retained. The capture is downstream of the send, and the seam demonstrably sends real
newlines. Nothing here shows the model saw flattened text, and this record must not be cited as
though it did.

## 2. What remains measurable, and was measured

`MEASUREMENT`

With byte identity unavailable, three properties survive, and all three held:

- **Length identity.** The captured `instructions` is 26,850 bytes, the exact size `compose`
  reports for the alloy. A rewrite that preserved the byte count to the digit would be a
  coincidence.
- **Every composed part, in composition order, safety tail last.** All three parts `compose` names
  — `personas/COORDINATOR.md`, `skills/relationship-record/SKILL.md`, `personas/_safety-tail.md` —
  appear at strictly increasing offsets, the tail last. That last clause is the one §A rests on: a
  safety tail the model reads before the guidance is not a safety tail.
- **Frontmatter is absent**, so `crates/domain/src/composition.rs::strip_frontmatter` ran on the
  bytes that were actually sent.

Score on `production`: **6 PASS, 0 FAIL, 1 BLOCKED**, exiting 3, because a blocked run exits 3
whatever else happened. The served profile returned the same, in §3c.

## 3. WITHDRAWN — the run id *is* on the timeline, and this finding was the harness's own defect

`WITHDRAWN, 2026-09-06, same day` · **it corrected
[`0051`](0051-the-probe-and-the-served-host-resolve-authority-differently.md) finding 4, and the
correction was wrong.** `0051` stands as written. The paragraphs below are kept because a withdrawn
finding that is deleted teaches nothing, and this one is the most expensive kind: it did not fail
to measure, it manufactured a conclusion and put it in two documents.

**What actually happened.** The harness asked the timeline for `.run_id` and `.runId`. The field is
`turn_run_id` — the name `0051` had already read off the type
(`ironclaw@4cb47cfaf3/crates/domains/ironclaw_threads/src/contract.rs:253`) and quoted correctly.
Re-fetching the same thread's timeline afterwards returns **200** with `turn_run_id` populated on
**both** messages, holding the exact run id the harness had reported as reachable only from the
event stream.

**The fallback is what made it invisible.** Check 2 tries the timeline, then falls through to the
events projection, and reports which answered. The primary path failed, the fallback succeeded, and
the check scored **PASS** — so nothing in the run looked wrong, and the misattribution rode out on
a green line. A fallback that silently absorbs a defect in the path it is backing up is worse than
no fallback, because it converts a failure into a false fact.

**So the conclusion `0051` reached from source is confirmed by measurement, not overturned:** the
run id is reachable from `GET /api/webchat/v2/threads/{thread_id}/timeline`, a route `0037` measured
at 200 to the owner and 404 cross-member, and **calling the working cancel route does not require
adopting the cursored event projection. It is a patch, not an ADR.**

What follows is the withdrawn text, left as it was written.

---


`0051` read at the pin that `ThreadMessageRecord` carries `pub turn_run_id: Option<String>`,
serialized, and that `RebornTimelineResponse.messages` is a `Vec<ThreadMessageRecord>` — and
concluded that the run id is reachable from the timeline, so calling the working cancel route would
be a patch rather than an ADR.

**The harness asks the timeline first, precisely so this would be measured rather than inherited,
and the timeline did not yield one.** All three runs reported the run id reachable from the
**events** projection. `0051` stated the field is an `Option` and that nothing had seen one
populated; that caveat is now the finding.

So the coupling `0051` thought it had dissolved is intact: **at this pin, obtaining a `run_id`
still means reading the cursored event stream that `docs/adr/0002-create-then-poll-not-streaming.md`'s
pointer declines to license.** Calling the cancel route remains an ADR, not a patch. This is what
`docs/UPGRADE.md` means by *never infer compatibility from source similarity* — the type carried
the field and the response did not carry a value.

Not established: *why*. Whether the timeline returns a non-200 for this shape, or returns 200 with
`turn_run_id` null on every message, was not separated, and the harness does not currently report
which.

*— end of the withdrawn text. Separating those two, which took one `GET` against a thread still
inside the four-run retention window, is what found the defect. The unfinished sentence was the
whole finding.*

## 3b. The confirming run, after the fix

`MEASUREMENT`

A fresh turn on a fresh thread, with the field name corrected:

    PASS  compose.run_identity   run id reachable from the timeline projection

**6 PASS, 0 FAIL, 1 BLOCKED**, the blocked one being byte identity for the reason in finding 1. The
exit code was checked separately and is **3** — a blocked run exits 3 whatever else happened, which
is the property `dev/lib.sh` exists to make true in one place rather than eleven.

**All three refusals the header counts were scored BLOCKED rather than FAIL** by `degraded()` —
the classification [`0050`](0050-blocked-was-being-read-as-fail.md) repaired, working on a live
refusal rather than on a fixture. Three in seven is not a rate and must not be read as one:
several were issued back to back inside the 30-requests-per-60-seconds budget, which `CLAUDE.md`
names as the most common cause of a BLOCKED run, so the provider-variance and rate-limit
contributions are not separated here.
[`0036`](0036-the-demo-question-lands-on-the-thirty-second-cliff.md) is the measurement of that
cliff; this is an observation beside it, on a narrower question, with a confound named.

## 3c. The served profile, and the first measured instance of `0051` finding 1

`MEASUREMENT`

The whole harness was re-run on `dev/probe-up.sh --sandboxed` — the
`hosted-single-tenant-volume-sandboxed` profile `deploy/ironworks-ironclaw@.service` sets — at the
same pin, with a worker image built from that revision and a private Docker daemon.

**Everything in findings 1 and 2 reproduced exactly.** 6 PASS, 0 FAIL, 1 BLOCKED, exit 3. The
capture is flattened there too; the captured `instructions` is 26,850 bytes; all three parts appear
in order with the tail last and frontmatter stripped; the run id came from the timeline. The
captured `instructions` of the two profiles hash **identically**, so the composition and the
flattening are both profile-invariant.

**What is not invariant is the prompt around it.**

| | `production` | `hosted-single-tenant-volume-sandboxed` |
|---|---|---|
| prompt components | 58 | **60** |
| `capability_count` | 53 | **54** |
| capability-surface components | 53 | **54** |
| `total_estimated_tokens` | 14,588 | **16,481** |

The one capability present on the served profile and absent on the other is **`builtin.shell`** —
the same id [`0039`](0039-the-catalogue-has-two-axes.md) found as the profile axis of the settings
catalogue, now measured on a different surface: the model-visible capability list inside the
composed prompt, not the settings rows confinement writes against.

**The token figure is the part with a cost attached.** The served profile's prompt is
**1,893 estimated tokens larger** on the same question, the same records and the same alloy —
about 13% more context per turn than the profile every other `dev/prove-*.sh` measures. That is
[`0051`](0051-the-probe-and-the-served-host-resolve-authority-differently.md) finding 1 stated as a
quantity rather than as a code path: a result taken on the default probe overlay does not transfer,
and here is what it costs not to notice.

**Do not read this against §I's context ceiling.** That bullet's ~5,700–5,910 tokens/turn is the
*growth rate* of a chained thread; this is a *single* turn's prompt estimate including a
forty-record envelope. Different quantities, and the profile figure has been measured for one turn
only. What transfers is the direction and the cause, not the number.

## 4. Four instrument defects, found by running it

`INSTRUMENT`

None of these was visible to `shellcheck`, to `bash -n`, or to reading.

0. **The run-id search looked for a guessed field name, not the declared one** — `run_id` and
   `runId` against a type that declares `turn_run_id`, which this repository had already read and
   quoted. Section 3 above is what that cost. It is listed as a fourth defect and numbered zero
   because it is the one that did damage: the others produced FAILs that were obviously the
   instrument's, and this one produced a PASS and a false correction.
1. **The extraction hashed labels alongside bodies.** A component's `label` *and* its `content` are
   both `BoundedDiagnosticText` objects — `{content, original_bytes, truncated}` — so a recursive
   sweep for `.content` strings returned 119 values from 58 components, of which the labels
   (`"Skill 1"`, `"System 3"`, `"Conversation 4"`) were most. The first run hashed all of them.
2. **The needle included frontmatter that `melt` strips.** Cutting the search string from the top
   of `SKILL.md` on disk takes text the alloy never contained, so the order check matched **one part
   of three** and reported a FAIL that was entirely its own.
3. **The negative control was a string nobody had written.** It searched the capture for the phrase
   `"a part this composition does not name"` — a literal that cannot appear, so the control could
   only ever report absence. §K-8 counts a vacuous control and a missing one as the same defect,
   and this is what one looks like when it is written by someone who has just finished warning
   about them. It is now `version: 0.1.0`, which **is** in `SKILL.md`'s frontmatter and must **not**
   be in the alloy — a control that can fire, and that measures the stripping while it does.

**The controls were then made to fire**, offline against the saved capture, because a check that
has only ever passed has not been tested:

| capture | parts found | order | frontmatter | verdict |
|---|---|---|---|---|
| as returned | 3 | ok | absent | PASS |
| persona overwritten | 2 | broken | absent | FAIL |
| frontmatter injected | 3 | ok | **present** | FAIL |
| empty | 0 | — | absent | BLOCKED |

Every verdict the check claims to be able to produce was produced.

---

## 5. The same defects were looked for in the rest of the harness, and are not there

`AUDIT` · no findings, which is only worth recording because the method's controls are shown

Four defects in one new script is a rate, not an accident, so the other twelve `dev/` scripts and
`dev/lib.sh` were checked for the three classes it exhibited. **None of them is present.**

- **A predicate that matches nothing** — `0050`'s class. Twenty literals the proofs grep for were
  checked against every producer in `crates/` and `dev/`; all twenty have one. The first pass
  reported four with no producer and *all four were the method's fault*: Rust's `\`-at-end-of-line
  continuation strips the newline **and the next line's indentation**, so a long operator message is
  one string at runtime and two lines in source, and a literal grep cannot see it. Joining
  continuations first is what makes this decidable. `timed out reading` still appears twice and both
  are comments describing the repair, not live predicates.
- **A fallback that hides its primary path's failure** — this record's own defect zero. Every
  `|| echo` in the harness builds a diagnostic label inside an already-decided FAIL message, and
  every `|| true` is teardown or an optional read that is checked immediately afterwards.
- **A control that cannot fire** — §K-8. The absence assertions are paired by construction:
  `dev/prove-cutover.sh` checks candidate=1/prior=0 and then mirrors it for the rollback,
  `dev/prove.sh`'s wrong-credential checks sit against its own-org control, and
  `dev/prove-continuation.sh` goes further than this script did — it BLOCKs when the batch did not
  form, and **FAILs when the intended failure did not happen**, saying so: *"upd-E did not fail; the
  containment is untested."*

**The control on the audit.** A check that finds nothing has established nothing unless it could
have found something. The literal check reports FOUND for a plain message, FOUND for one that spans
a continuation, and MISSING for the vacuous control this script briefly carried — so it separates
present from absent in exactly the case that defeated the first attempt.

**What this says about the four defects above:** the harness had already been corrected into this
shape, one incident at a time — `0044`, `0050`, and the essay in `dev/prove-tenancy.sh`'s header
about why its first two checks are not padding. A script written in one pass does not inherit any
of it. That is an argument for a first run being treated as a measurement, not for writing fewer
scripts.

## What this run did not do

It did not measure the served profile. `dev/probe-up.sh` without `--sandboxed` boots `production`,
and `0051` finding 1 is the reason that distinction is now stated rather than assumed: the two
resolve their runtime policy through different constructors. Whether the inspector flattens on
`hosted-single-tenant-volume-sandboxed` too is unmeasured, and the answer is not predictable from
this run.

It spent three turns to score one, and the two it burned were spent on defects in the instrument.
That is the expected shape of a first run and not an argument for fewer of them.
