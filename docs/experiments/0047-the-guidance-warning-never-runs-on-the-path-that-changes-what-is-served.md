# 0047 — The guidance warning never runs on the path that changes what is served

**2026-09-06.** A read-only alignment audit run from `docs/AUDIT-BRIEF.md`, against IronClaw
`4cb47cfaf3` (`ironclaw-v1.4.0`). No runtime, no probe, no model key, no host. Source reads and one
control run only.

**All nine of the brief's dimensions were examined against source, and none by measurement.** That
distinction is the whole limit of this record: it establishes what the code refuses, never that a
deployment refuses it. Finding 1 is precisely a case where those two came apart, so read the limits
section as the live part of this document rather than its disclaimer.

Control at the time of the audit: `cargo run -p ironworks-claims` reported 38 of 38 claims holding,
0 disagreeing, 0 unverifiable, on a tree carrying 117 uncommitted changes made by other sessions.

---

## Verdicts

| | |
|---|---|
| **Pinned upstream compatibility** | `ALIGNED` on what was read. No fallback literal for either pin exists or can be written; the seam speaks two endpoints and sends no `tools` array. Nothing read here required an IronClaw source change. |
| **Security** | `MISALIGNED`, one finding, `HIGH`. The control that detects an operator-notes leak in model-visible guidance does not run on the path that actually changes what is served — and that path has already leaked once, into real turns. **Closed 2026-09-07** — the verdict stands as measured; see the finding's closure for what closed it. |
| **Privacy and claim integrity** | `ALIGNED` for the narrow claim the repository actually makes. The turn journal holds identifiers, digests, states and counts, and no content. The broader claims stay qualified as they already are. |

---

## Finding 1 — the leak warning is bound to the publish path, and guidance changes without one

`MISALIGNED` · `HIGH` · `DEPLOYMENT/CONFIGURATION` + `MEASUREMENT`

**Threatens** §A-6/§A-7, operator comments and frontmatter stripped from model-visible input.

**Evidence, source-read.** `crates/domain/src/composition.rs::strip_html_comments` ends a comment
block at the **first** `-->` after its opener, which is correct HTML and is the whole problem: an
operator-notes block that writes a guidance marker out in full terminates itself at the example,
and everything after it survives into the model-visible body under the organization-guidance
heading. `crates/domain/src/composition.rs::Guidance` refuses none of this — the marker still
binds, and leaked notes push the body *over* `crates/domain/src/composition.rs::MIN_CONTENT`
rather than under it.

The detector existed in the CLI in functions then named leaked_note_bytes and
warn_guidance_hazards, called from exactly two places — `run_compose` and
`run_registry_publish_one` — and **not** from
`crates/cli/src/lib.rs::boot_tenant`, the function that actually composes what a tenant is served.
That omission was deliberate and reasoned in the code: a warning at boot "is noise rather than news,
and by the time either runs the composition is already serving."

*Those three functions no longer exist and are named here without citations for that reason. What
replaced them is in the closure at the end of this section.*

**Why the reasoning does not hold.** `boot_tenant` re-reads the guidance file from disk, at the
path the registry entry names, on every serving cycle — the registry supplies a *path*, not
content. `dev/relay-loop.sh` runs a fresh process per cycle. So the content that reaches the model
is whatever is on disk at that moment, and replacing that file changes what is served with no
publish and no compose.

**That is not hypothetical, and this repository already recorded it happening.**
`deploy/README.md` documents the 2026-09-04 incident: 1,270 bytes of operator instructions in every
prompt, the imperative "Add, as headed sections in the visible body" among them, under alloy
`4e68bf09c447a410`. It reached real turns. The same file records the fix as "a file copy into
`/opt/rustworks` and nothing else — no restart, no unit change, no registry publish" — which is
precisely the path on which neither warning command runs.

**Consequence.** Operator prose composed as the organization's own instructions, in the one channel
a persona is told governs its behaviour rather than the record text it is told to treat as data. It
is invisible in a diff, because the file is correct-looking markdown.

**Smallest remediation, and it is not seam code.** Do not add a third mechanism. Two options, in
order of cost:

1. `MEASUREMENT` — have `dev/prove-host.sh` assert the leaked-note and unwritten-stub detectors are
   clean for the guidance file the live registry names. It already reaches the host read-only and
   already parses the registry, so the check costs one read. A leak then becomes a failing proof
   rather than a silent composition.
2. `DEPLOYMENT/CONFIGURATION` — make `deploy/README.md`'s guidance-update procedure end with
   `ironworks compose` against the host copy, so the documented path for changing served guidance
   runs the detector that exists.

**Acceptance evidence.** A positive control is required and is cheap: plant a `-->` in a copy of
`fixtures/tenants/guidance.template.md`, confirm the check fires, then confirm it does not fire on
the corrected file. Without the first half this is a check that can pass by finding nothing.

**Residual risk.** Neither option closes the window between a file copy and the next proof run.
Closing that would require the serving path to carry the check, which the code has argued against
on noise grounds — a trade worth restating deliberately rather than inheriting.

**Superseded in flight, 2026-09-06, by a better shape than either option above.** While this was
being written another session was building `crates/cli/src/lib.rs::run_guidance_publish_one` and
`crates/cli/src/lib.rs::run_guidance_rollback_one`, making guidance a *published artifact* with a
registry lock and a digest, beside `crates/cli/tests/guidance_publish.rs`, whose
`each_known_hazard_is_refused_while_clean_guidance_is_the_positive_control` refuses the hazards
this record found rather than warning about them, with the control §K-8 asks for. That removes the
file copy as the delivery mechanism instead of adding a check beside it, which is the stronger
move and the one this record failed to reach.

Two things survive that work rather than being closed by it. The serving path still re-reads from
disk, so the guarantee holds only once `deploy/README.md` stops documenting a file copy as the way
to correct served guidance — the 2026-09-04 sequence was "a file copy into `/opt/rustworks` and
nothing else". And `guidance_digest` there is a full sha256 hex while
`crates/domain/src/composition.rs::digest16` is the sixteen-character shape every other fingerprint
in this repository uses and that `deploy/README.md` quotes; two digest shapes for one artifact is
how the next disagreement starts.

This section is left as it was reasoned. The options above were the right ones from what this pass
could see, and recording that they were overtaken is more useful than editing them away.

**Closed outright, 2026-09-07, and further upstream than either option.** The detectors left the
CLI entirely: `crates/domain/src/composition.rs::GuidanceError` gained `UnwrittenStubs`, and
`crates/domain/src/composition.rs::Guidance` now **refuses** leaked operator notes and unwritten
template stubs rather than warning about them — controlled both ways by
`crates/domain/src/composition.rs::legacy_leaked_notes_and_unwritten_stubs_are_not_guidance`.
Because `crates/cli/src/lib.rs::boot_tenant` calls that parser on every serving cycle, the serving
path now refuses the exact input that reached production on 2026-09-04. There is no warning to miss,
no publish-time-only gate, and no window between a file copy and the next proof run — the three
things this record was about. The residual it named is gone with them.

Worth recording *why* it landed there rather than where this pass proposed. Both options above tried
to catch the hazard on the path that changes, which meant reasoning about which paths those are.
Refusing it in the parser makes the question unnecessary: every path that composes guidance goes
through `Guidance::parse`, so there is no path to enumerate and none to miss. That is the same move
as deriving the channel cursor instead of storing it — dissolving the boundary rather than
defending it.

---

## What was examined and holds

**Pins and configuration precedence — `ALIGNED`.** `crates/domain/src/pins.rs::parse` is the single
rule, and the newtypes it generates implement neither `Default` nor an infallible constructor, so a
fallback literal has no syntax to be written in. `crates/domain/src/pins.rs::the_committed_pins_parse`
asserts the committed values, making a bump deliberate in two places. `boot_tenant` reads
`MODEL_PIN` by a relative path and **fails closed** when it cannot — no substitution. The deployed
units set a working directory and the whole tree is copied to the host, so the relative path
resolves there; it is an operational coupling, not a fallback.

One thing to keep an eye on rather than a finding: `dev/test-ironclaw-artifacts.sh` reimplements the
pin parse rule in shell. Two mechanisms for one rule is the shape this repository warns about, and
it is the kind that drifts silently because both halves look right in isolation.

**Re-checked later the same day, and this paragraph overstates it in two directions.** There are
**three** shell copies, not one — `dev/build-pinned-ironclaw-artifacts.sh` and
`dev/test-ironclaw-artifacts.sh` share one `sed` expression and `dev/probe-up.sh` spells the same
rule as `head | cut | tr`. But the divergence from `crates/domain/src/pins.rs::parse` — all
whitespace removed, against ends trimmed — **cannot produce a wrong value**, because
`IronclawPin` appears nowhere outside `pins.rs` but its own macro invocation and two tests, so no
production Rust reads it, and the only live consumer validates `^[0-9a-f]{40}$` before using it.
Three spellings of an unreachable divergence is a tidiness question, not an invariant at risk.
Recorded because the first reading proposed a machine check for it, and the evidence does not
support paying for one.

**Org resolution — `ALIGNED`, and the confused-deputy guard is the right shape.**
`crates/domain/src/registry.rs::ExpectedOrg` is deliberately not an `OrgId` and has no conversion
out of it; `crates/cli/src/lib.rs::agreed_org` compares it against what the store resolved and
**refuses** on disagreement rather than preferring either. The store's answer is authoritative and
the registry value can only ever cause a refusal, never a selection.

**Journal content — `ALIGNED`, for the narrow claim.** The `turns` and `threads` tables hold keys,
instance, org, timestamps, a request digest, an outcome, a response id, attempt and chunk counts,
an alloy digest and a book fingerprint. No message text, no answers, no record bodies, no
credentials. `docs/FRONTEND.md`'s statement is accurate as written. It does not extend to the
system: proposals, promoted activities, the channel and IronClaw itself are separate stores.

**The fact-field parser closes the delimiter trap that the body cannot.**
`crates/domain/src/composition.rs::parse_fact_fields` refuses a declared field containing `-->` or
a control character, with a test asserting it. That asymmetry is correct — a declared key is
structured and can be refused, tenant prose is not — but it is worth noticing that the closed half
is the one that was never the incident.

**Cross-tenant isolation on the read path — `ALIGNED`, and the omission hole is closed by a type.**
`crates/records/src/lib.rs::Book` takes no org parameter on either method; org comes from the
scope's evidence and is compared against what the store echoes, with
`crates/records/src/lib.rs::BookFailure` carrying the drift. The guard could have been skipped by a
store that simply omitted the field — and it cannot be, because `org` on both wire types is a
required `String` while every tolerant field beside it (`accounts`, `name`, `updated_at`) carries
`#[serde(default)]`. A response with no org fails to deserialize and is refused as malformed. The
tolerance is deliberately spent where it is safe and withheld where it is not.

**The append path — `ALIGNED`, and it separates drift from ambiguity correctly.**
`crates/records/src/append.rs::Append` treats its expected org as a confused-deputy guard: a
success naming another organization is `AppendFailure::ScopeDrift` even at HTTP 201, because the
store said plainly where it wrote. An org it cannot parse is *unreadable* and therefore retryable,
not a refusal — the write may have landed. An acknowledgement naming a different activity, or a
status disagreeing with the HTTP code, becomes retryable rather than a truthful `Appended`. That is
§G-1's "200 is not success" applied to the record store rather than only to the runtime, and it is
the distinction between a control that fails closed and a data source that degrades.

**Credentials — `ALIGNED`.** All four sealed types — `crates/records/src/credential.rs::Secret`,
`crates/ironclaw/src/client.rs::Credential`, `crates/reconcile/src/lib.rs::OperatorToken`,
`crates/channel-http/src/lib.rs::AdapterToken` — hand-write a redacting `Debug`, implement no
`Display`, and keep their exposing accessor private or crate-private. The containing clients redact
too rather than relying on it: `crates/ironclaw/src/client.rs::Ironclaw` prints only its base URL
and finishes non-exhaustively, as does `crates/journal/src/lib.rs::Journal`. A derived `Debug`
anywhere upstream would still print `Credential(redacted)`, so this holds by construction rather
than by discipline.

**Confinement read-back fails in the safe direction — `ALIGNED`, caveats unchanged.**
`crates/reconcile/src/ironclaw.rs::confirms_disabled` ends in `unwrap_or(false)`, so a malformed,
unexpected or truncated response reads as *not confirmed disabled* and
`crates/reconcile/src/ironclaw.rs::apply` stops rather than recording a write it cannot see. Given
§L-9 — a settings write can return 200 without applying — that default is the load-bearing half.
Nothing here re-measures the standing caveats, which stand as recorded: custody-backed and
member-reversible, completeness `BLOCKED` (§L-7), and a catalogue with two axes.

**Egress — `ALIGNED`, and checkable in the manifests rather than by argument.** `ureq` appears in
exactly five manifests — `channel-http`, `ironclaw`, `reconcile`, `records`, `telegram` — and
`crates/cli/Cargo.toml`, the manifest of the `ironworks` binary, declares no transport dependency.
The claim that the binary owns no network is a property of the dependency graph, not a convention.

**Corrected later the same day.** This paragraph first read "no transport and no storage dependency
at all", which was true when written and overreached even then. The rule is about **network** —
`domain` is effect-free, adapters own transport, the binary composes — and the binary has always
owned storage transitively through `journal`, `lifecycle` and `proposals`. Bundling storage into
the sentence implied a guarantee nobody made. It became visibly wrong within hours:
`crates/cli/Cargo.toml` now declares `rusqlite` directly, for
`crates/cli/src/state.rs::BackupConfig` and the off-box snapshot work that closes §I's
durable-state-has-no-backup gap. The network claim is unaffected.

**The one cross-tenant object — `ALIGNED`, and it is the strongest thing read in this pass.**
`crates/channel/src/cursor.rs::safe_cursor` is a pure function of the journalled rows: the lowest
update that still holds the barrier, `min` taken across every tenant rather than within one. A fast
tenant finishing a later update cannot carry the offset past a slower tenant's earlier one, and
graceful shutdown falls out of the same line rather than being handled. B6 is **dissolved rather
than defended** — the journal exposes no method that sets an offset, so a cursor advanced past
unfinished work is not a state this code declines to produce but one it cannot represent.

Two details carry the weight. `Openness::Unestablished` holds the barrier exactly as `Open` does,
so a row nobody could read is never mistaken for a finished one — the offset is what stops the
channel redelivering, and passing it would trade an answer for a read error. And a *parked* update
settles, so one turn awaiting an operator does not deafen the channel behind it.

**This file also answers the audit's own weakest point.** Its tests carry two labelled positive
controls, including an over-correction control reasoning that a `safe_cursor` refusing to pass
anything would satisfy the barrier assertion while being useless. That is §K-8 applied by the tree
to itself, and it is the standard the rest of this audit did not meet.

The known cost is documented elsewhere and is not a finding here: a permanently unreadable row
holds the barrier for every tenant sharing the journal, which `docs/FRONTEND.md` states plainly as
the reason a public demo bot needs its own deployment and journal.

**Recovery precedence — `ALIGNED`, and the ordering is the at-most-once property.**
`crates/journal/src/lib.rs::recover` answers `OperatorRequired` first, so a parked turn is never
re-run by anything below it; then consults **delivery evidence before turn outcome**, so a
delivered turn settles rather than matching an outcome arm and running again; and only then reads
the outcome, where a submitted-but-unknown turn replays the identical key against a recorded
request digest rather than issuing a second model call, and an accepted one fetches rather than
re-creating. Inverting any two of those three layers would produce a second billed turn at exactly
the moment a crash makes one most likely.

---

## What was not examined

Named, so that nothing above reads as coverage it is not. Each is `UNVERIFIED` in the brief's
sense: no evidence was gathered, which is not the same as no defect.

- **Every claim here is a source read.** No live measurement was taken, no proof script was run, no
  probe was stood up and no host was reached. A property that holds in the source and not in the
  deployment would look identical from here — which is exactly the gap Finding 1 turned out to sit
  in, so treat that as the shape to look for rather than as the only instance.
- **No pinned IronClaw source was read for any claim in this audit.** One read was made while
  validating a citation in `docs/AUDIT-BRIEF.md` and supports nothing here. Every upstream
  statement above is either this repository's own dated measurement or absent.
- **Revocation** remains a named absence attributed to the pinned runtime's session store. That
  attribution was not confirmed against source in this pass, and it is the one claim here that
  would move a finding from `UPSTREAM WORK` to `SEAM CODE` if it turned out to be wrong.
- **Confinement was not re-measured.** Nothing here touched a member, a catalogue or a settings
  write; only the read-back's failure direction was read.
- **This audit constructed no positive control of its own.** Every property above is a reading of
  code that refuses, not an observation of it refusing beside an observation of it permitting. That
  is a limit of the audit, not a finding about the tree — where the tree carries its own controls,
  they are noted as such. §F-14 and §K-8 are unmet *by this pass*, which makes `ALIGNED` a weaker
  word here than it looks.

The control was run three times over the pass. The first was terminated part-way through a test
binary and is recorded as `BLOCKED`, not `FAIL`; the last two reported 38 of 38.

**One structural caution about this record.** It was produced on a tree that three sessions were
editing concurrently, including edits to `docs/AUDIT-BRIEF.md` and `CLAUDE.md` while it was being
read. A finding here has a shelf life; re-verify before acting on it.
