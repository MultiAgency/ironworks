# The audit brief

A prompt to hand a coding agent that has read access to this repository and to the pinned IronClaw
source. It commissions a **read-only** security, privacy and upstream-compatibility audit, and it
is written to be copied whole — everything below the rule is addressed to the auditor.

Every path, command, script cost, section name and caveat in it was checked against the tree by the
pass that wrote it. That is the point of the document: a brief that names a file which moved, or a
command that does not exist, spends the auditor's first hour on archaeology and its conclusions on
a repository nobody has. Because it lives in `docs/`, `cargo run -p ironworks-claims` re-checks its
paths, its `path::symbol` citations and its experiment references on every run — so it goes stale
loudly rather than quietly.

---

You are conducting a **read-only security, privacy and upstream-compatibility alignment audit** of
the repository in your working directory.

## Objective

Establish whether the working tree is aligned with:

1. the exact official IronClaw revision it pins;
2. this repository's own measured invariants; and
3. the security and privacy properties it claims in public.

Produce an evidence-backed audit and prioritized remediations. Do not edit or create files, change
configuration, change either pin, operate a deployment, or implement a remediation.

## Product boundary

This repository is an operator-run **seam**: it serves scoped, managed agentic services to
organizations over a chat channel, on top of official, unmodified IronClaw, which it reaches over
HTTP. A tenant gets a service the operator runs on its behalf — not an IronClaw installation it
configures, not a general assistant, not a toolkit it extends.

The seam deliberately exposes far less authority than IronClaw does. A narrowing that is intended,
respects the pinned contract and is described truthfully is `DIVERGENT BY DESIGN`, not a defect.
Record-only turns, no tenant-facing model choice, no model-directed tool loop and no effect
execution are all in that class. A fix that would require editing IronClaw's source is
`UPSTREAM WORK` and does not belong in this repository. **`docs/UPSTREAM.md` is the register of what
has been classified that way** — check it before filing a finding as new, and add to it rather than
leaving the classification to be rediscovered from an experiment.

**Do not carry IronClaw's consumer-facing privacy promise across this boundary.** Whatever the
official product says about data staying on a user's own machine does not describe a
vendor-operated seam whose operator runs the record store, holds every tenant credential and sends
tenant records to a third-party model provider. Judge this repository's own, narrower claims on
their own evidence.

## Authority and evidence — source only, for anything upstream

Upstream documentation lags upstream code. **No claim about how IronClaw behaves may cite a
document** — not IronClaw's own agent guidance, not release notes, not an issue, not a document in
its tree. Anchor every such claim to source read at the pin.

The order, and a lower rank never silently overrules a higher one:

1. **Pinned IronClaw source at `4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`.** The only admissible
   evidence of runtime behaviour.
2. **This repository's dated measurements against that revision.** `DESIGN.md` "The upstream
   contract" — whose facts carry per-fact `[measured @ rev]` / `[source-read @ rev]` markers and
   deliberately mix 1.3.0 and 1.4.0 provenance; `docs/INVARIANTS.md` §G and §L; `docs/UPGRADE.md`;
   the dated records in `docs/experiments/`. Read the run that measured a claim before repeating
   it, and never edit a dated record to agree with a later tree.
3. **The official `ironclaw-v1.4.0` release page** — for release provenance only, meaning that this
   SHA is what the tag names. Never for behaviour.
4. **Current upstream `AGENTS.md` and the hosted multi-tenant review in the official issue
   tracker** — orientation only. They may tell you *where to look* in the source and what risks are
   worth testing. A principle read there is a hypothesis to confirm in code, never a finding, and
   every use must be labelled post-pin advisory.

No blogs, no forum answers, no third-party summaries, no recollection. Current `main` is never
evidence of behaviour at the pin. If you cannot produce a source read, the claim is `UNVERIFIED`.

For every material assertion, distinguish: source-read behaviour; behaviour measured live, with the
revision it was measured at; behaviour inferred from several facts; a target requirement rather
than current behaviour; and an unknown or blocked measurement.

## How to read the pinned source

Nothing is vendored here and there is no submodule. `ironclaw@4cb47cfaf3/` is a **citation prefix**,
not a path on disk. The source lives in a sibling clone at `~/agency/ironclaw`, whose checkout sits
on `main` at a later revision than the pin, and which has a second registered worktree at an older
one. Address the source by SHA, never by checkout:

```
git -C ~/agency/ironclaw ls-tree --name-only 4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe crates/
git -C ~/agency/ironclaw show 4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe:<path>
git -C ~/agency/ironclaw grep -n <pattern> 4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe -- <pathspec>
```

**Never `checkout`, never `fetch`, never touch that clone's state.** `dev/probe-up.sh` reads it,
and other sessions may be working in it.

**Confirm the SHA is the official one before calling anything official.** That clone carries a
`fork` remote beside `origin`/`upstream`, so the path alone establishes nothing; the release page
establishes that the tag names this SHA, and the SHA establishes the bytes.

A worked example, because two claims in this repository rest on it: `locked` in the tool-settings
response is computed per read from the capability's own declared effects and no write path sets it
— `ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/reborn_services.rs:1951-1962`. That is
what makes confinement here *necessarily* custody-backed rather than incidentally so.

## Safety constraints

- **The dirty working tree is intentional.** Do not modify, format, restore, stage, stash, commit
  or delete anything, in this repository or in the IronClaw clone.
- You may read files, inspect Git metadata, and run local non-mutating checks that write only build
  caches.
- **Do not print secrets or tenant content.** No environment values, no credentials, no record
  bodies, no proposal bodies. Names, types, schemas, digests and redacted diagnostics only. When
  inspecting a failure path, preserve its sanitization — report the safe classification and the
  location, never the raw payload.
- **Never turn an unavailable check into a pass.** `BLOCKED` means the measurement did not happen.
  `UNVERIFIED` means the evidence you have cannot establish the claim. Neither is a pass, and there
  is no `SKIPPED`.

### Commands you must not run, and what each one costs

Read their source and their recorded transcripts instead. **Each script states its own cost on a
`# COST:` line in its header, and that line is the fact** — this table is derived from those lines
and `crates/claims` fails if the two disagree in either direction.

| Command | Cost |
|---|---|
| `dev/prove.sh`, `dev/prove-delivery.sh`, `dev/prove-continuation.sh`, `dev/prove-workspace.sh` | real model turns — money, and the shared 30-request/60-second budget |
| `dev/probe-up.sh` | bills one model turn per bring-up (a warm-up turn is discarded deliberately); `--down` destroys volumes |
| `dev/prove-composition.sh` | one measured model turn, and a model key; writes nothing and spends no member |
| `dev/prove-confinement.sh` | **spends a member, irreversibly** — approval-off is one-way (§L-9) |
| `dev/prove-cutover.sh` | spends a member **and** a model key |
| `dev/prove-freeze.sh` | six model turns; mints two members and **burns neither** — §L-9 spends a member when approval-off is written, and this writes no settings at all |
| `dev/prove-boot.sh` | **destroys the probe** — its first act is to tear the stack down |
| `dev/prove-host.sh` | reaches a **live host serving a real tenant**; read-only, but every run is the operator's decision in the moment |
| `dev/prove-deployment-isolation.sh` | runs as root against live per-client Docker daemons |
| `dev/relay-loop.sh` | the production serving loop; drives real turns for as long as it runs |

You **may** run these: `dev/prove-tenancy.sh` and `dev/prove-lifecycle.sh` (no runtime, no model
key, no cost), and `dev/test-prove-host.sh`, `dev/test-ironclaw-artifacts.sh`,
`dev/test-state-backup.sh` and `dev/build-pinned-ironclaw-artifacts.sh` (offline). **Free is stated, never inferred from
absence** — a script missing from both lists is a defect in this section, not a licence.

## Start here

1. `AGENTS.md` — a symlink to `CLAUDE.md`, the always-loaded brief. Then `CONTEXT.md`, which is a
   glossary and nothing else, and worth reading in full before anything else.
2. `DESIGN.md`, sections "The upstream contract", "Layering", "Types that carry invariants",
   "Status", "One turn, end to end" and "Crash boundaries".
3. `docs/INVARIANTS.md` §A–§L. §I is the open-gaps section and distinguishes bounds nobody has
   measured from defects whose shape is understood; §K is what a type enforces rather than a test;
   §L is confinement at the pin.
4. `docs/PRD.md` §2, §4, §5, §13, §17, §19, §20, §21, §22 — a **target** document. A requirement in
   it is not evidence that anything is built.
5. `docs/UPGRADE.md`, `IRONCLAW_PIN`, `MODEL_PIN`.
6. `deploy/README.md` for the served profile and the systemd units; `docs/FRONTEND.md` for what is
   published and what the published pages claim.
7. `git status --short`, so your report describes the tree you actually audited.
8. The repository control, exactly as written, which takes no arguments:

   ```
   cargo run -p ironworks-claims
   ```

   Record its exit status and its complete tally. Exit `0` is every claim holding, `2` is at least
   one FAIL, `3` is at least one BLOCKED and no FAIL. Its documented-command check shells out to
   `target/debug/ironworks`, so build `ironworks-cli` first or that check reports BLOCKED for a
   reason that has nothing to do with the tree. **Do not repair a failure**, and do not run a
   formatter, which writes files.

### The baseline, to verify rather than assume

Measured 2026-09-06 on this tree: `cargo run -p ironworks-claims` reported **37 of 38 claims
holding, 1 disagreeing, 0 unverifiable**, exiting `2`. Every citation check passed — 196
repo-relative paths, 126 `path::symbol` citations, 57 experiments, 25 bare document names, no
in-tree line anchor — and `cargo test --workspace` passed and clippy was clean.

The single disagreement was `rustfmt`, naming one Rust file that another session was editing at the
moment of the run. **That is what a shared tree looks like, not a defect in the system you are
auditing.** Attribute a red row before reporting it — `git status --short` and `git diff --stat` on
the named file usually say whose it is — and do not repair it, because running a formatter rewrites
someone else's in-flight work.

*Later the same day, on a tree with no other `cargo` running, the same control reported 38 of 38
holding. The paragraph above is left as it was measured: it is the more useful of the two records,
because the green one teaches nothing about attributing a red row you did not cause.*

**An earlier run of this same control, the same day, reported `workspace tests` failing instead —
and that row was wrong.** The run had been terminated by SIGTERM part-way through a test binary, so
it was a measurement that did not finish: `BLOCKED`, not `FAIL`, by the definition this repository
uses everywhere else. Never report a failing suite on the strength of a killed process; re-run the
binary on its own first.

**A `workspace tests` row of that shape had a known cause in this tree, and it was not the
product.** `docs/experiments/0050-blocked-was-being-read-as-fail.md` measured it and records the
counts, the `EAGAIN` mechanism and the repair; the stub is now
`crates/testkit/src/lib.rs::serve`, which bounds both of its waits and returns `Option<String>`
so that "no client ever connected" cannot satisfy an assertion. Read the run rather than this
paragraph — a second copy of its numbers here would be one more thing to keep in step.

**The diagnostic is what you need, and it generalises past that one stub**, because a wedge can
arrive from anywhere that spawns a process: before drawing any conclusion from a slow or killed
suite, check `ps -o pid,etime,time` for a test binary with large elapsed time and near-zero CPU.
That is a wedge, not a build. Re-run that one binary on its own, single-threaded, rather than
re-running the workspace.

**Reproduce that baseline rather than trusting it, because this tree is edited while it is being
read.** The run above was taken against a working tree carrying 112 uncommitted changes it did not
make, with other sessions running `cargo` against the same build lock; the brief you are holding
was itself modified by another session between being written and being verified. Re-run the
control, report what it establishes then, and if what you observe disagrees with `DESIGN.md`'s
Status block, classify the present evidence precisely rather than carrying either observation
forward as fact.

## Read these before re-deriving them

Some write-ups in `docs/experiments/` are audits with no transcript and no grader in the path — the
write-up is the evidence — and several of the dimensions below are already partly answered in them.
The ones that bear most directly on this brief:

- `docs/experiments/0037-what-the-seam-never-asked-for.md` — the seam's six upstream path shapes
  against a surface an order of magnitude wider, asking whether the omissions were decisions
  anyone actually took. Eight checks. Feeds §L-1, §L-4, §L-6 and §I.
- `docs/experiments/0039-the-catalogue-has-two-axes.md` — repeats those checks on the *served*
  sandboxed profile and establishes that the settings catalogue varies by caller **and** by
  profile. A bare row count is under-specified.
- `docs/experiments/0040-the-cost-field-misses-the-pin-twice.md` — `usage.cost` at this pin, and a
  retraction of one of `0037`'s findings.
- `docs/experiments/0045-the-first-proof-of-the-serving-host-found-its-own-defect.md` — the first
  read-only proof against the live serving host: 7 PASS, 1 FAIL, where the FAIL was the
  instrument's own truncated identifier.
- `docs/experiments/0046-one-failing-update-took-its-whole-batch-with-it.md` — an audit of a code
  path rather than an answer, measured with no runtime and no key.
- `docs/experiments/0047-the-guidance-warning-never-runs-on-the-path-that-changes-what-is-served.md`
  — **the first run of this brief.** All nine dimensions examined against source, none by
  measurement. Read it before repeating its work, and read what it says about the difference
  between what the code refuses and what a deployment refuses.
- `docs/experiments/0049-guidance-publication-became-the-serving-change-path.md` — what `0047`'s
  finding 1 was remediated with, and what was still not measured afterwards.
- `docs/experiments/0048-thirteen-fail-open-defects-and-three-that-were-not.md` — a code-quality
  sweep by read-only agents, notable here for its three withdrawn findings: it is the worked
  example of re-reading a finding against source before reporting it.

The index in `docs/experiments/README.md` is complete and machine-checked; prefer it to this list,
which is a starting order rather than a set.

Then `docs/UPGRADE.md`'s 1.4.0 qualification table, and in particular the row that was written
`PASS` beside a run that exited 3 and was corrected to `BLOCKED`. What it cost is stated there: the
blocked check was the **positive control** for two isolation refusals, and an empty store passes
every isolation check.

## Vocabulary that will mislead you

**`CONTEXT.md` is the authority, and it is a glossary and nothing else — read it in full.** Its
entries for **seam**, **alloy**, **melt**, **book**, **corpus**, **probe**, **confinement** and
**custody-backed** are the ones whose ordinary reading is actively misleading, and its Shorthand
table is where the four numbering systems are defined — including why `§B-2` and `B2` are one
hyphen apart and unrelated. `CLAUDE.md` carries the same shortlist. Neither is restated here,
because a third copy is a third thing to keep in step.

One term is not in either, and it is the central noun of §L:

- **`member` has no `CONTEXT.md` entry.** It means an account on the IronClaw runtime. It is
  deliberately *not* the same thing as a **contributor**, which is a channel actor the seam refuses
  to claim is paired to any runtime account.

## One deliberately stale baseline

`docs/INVARIANTS.md` §K keeps a **stale** baseline as dated evidence and points at `DESIGN.md`'s
Status block for current figures. That is the house rule working, not drift, and quoting it as a
finding would be the error.

## Review dimensions

Audit all nine. For each, test the intended success **and** the dangerous opposite wherever local
evidence permits. A negative isolation check with no positive control establishes nothing (§F-14,
§K-8) — a harness can invent a match or invent a difference, and both are the same defect. The
concrete instance to learn the shape from is `dev/prove.sh`'s `isolation.own_org_control`, which
builds the envelope from real records under the real credential and stops before dispatch, so it
scores with no runtime and no model key. **Read it; do not run the script to see it.** That one
check is free, but `dev/prove.sh` as a whole spends model turns, and there is no way to ask it for
a single check.

Each dimension names live anchors. Start from them; do not treat them as the complete surface.

### 1. Pins, configuration precedence, and runtime integrity

`crates/domain/src/pins.rs::parse` is the single parse rule — first line, then everything before
the first `#`. The newtypes it generates have no `Default` and no infallible constructor, so a
fallback literal has no syntax to be written in; `crates/domain/src/pins.rs::the_committed_pins_parse`
asserts the committed values, making a bump deliberate in two places, and
`crates/domain/src/pins.rs::PinError` says an unreadable pin means a broken checkout rather than a
reason to substitute a literal.

Look for: any fallback literal, implicit default, unpinned image, current-`main` assumption or
configuration path that could outrank a pin. Two specific things to weigh —
`crates/cli/src/lib.rs::boot_tenant` reads `MODEL_PIN` by a **relative** path, so the value depends
on the working directory; and `dev/test-ironclaw-artifacts.sh` reimplements the pin parse rule in
shell, which is a second mechanism for one invariant. Separate model-request pinning from provider
enforcement and from post-hoc auditability, which are three different claims.

### 2. Identity, tenancy and routing

`crates/domain/src/instance.rs::ServiceInstance` is the gate: the unverified state carries a
*claimed* org and has no accessor for one, `crates/domain/src/instance.rs::ScopeResolver` resolves
the real scope server-side, and `crates/domain/src/instance.rs::Ask` has neither an org field nor a
rendered-text field, so a caller-selected org is unrepresentable.
`crates/domain/src/instance.rs::InstanceError` carries the cross-wired-credential refusal.

`crates/domain/src/registry.rs::TenantRegistry` parses all-or-nothing, and
`crates/domain/src/registry.rs::ExpectedOrg` is deliberately not an `OrgId` with no conversion out
of it. `crates/domain/src/route.rs::resolve` returns a three-valued
`crates/domain/src/route.rs::RouteState` from a *slice* of claims, so a two-tenant collision is
detectable rather than already lost. `crates/domain/src/ids.rs::OrgId` and
`crates/domain/src/ids.rs::ActorRef` (no `Display`, redacting `Debug`) are the identity types.
`crates/cli/src/lib.rs::boot_tenant` is where the registry's expectation is compared against what
the store answers.

Trace cross-org, same-org/different-instance, shared-credential, response-id and continuation
cases. Demand a positive own-org control beside every negative result.

### 3. Cross-tenant isolation

`crates/records/src/lib.rs::Book` takes no org parameter — org comes from the scope's evidence and
is compared against what the store echoes, yielding
`crates/records/src/lib.rs::BookFailure`. `crates/records/src/append.rs::Append` treats its expected
org as a confused-deputy guard and never as a selector; exactly one variant of
`crates/records/src/append.rs::AppendFailure` is retryable.
`crates/journal/src/lib.rs::TurnRecord` compares both slug and org, because the idempotency key
names no tenant while one journal file holds every tenant's turns.

The one deliberately cross-tenant object is `crates/channel/src/cursor.rs::safe_cursor` — the lowest
open update across every tenant, where an unestablished state holds the barrier exactly as an open
one does. Note that the channel crate's isolation property is enforced by a test over its own
`Cargo.toml` rather than by any code: read `crates/channel/src/lib.rs::ChannelJournal` and that
manifest together.

### 4. Model-visible input and prompt injection

Enumerate every path by which bytes can reach `instructions`, `input`, tool output, runtime memory
or a continued thread. `crates/domain/src/lib.rs::ModelInput` is the taxonomy and deliberately keeps
a runtime-supplied variant rather than claiming to enumerate IronClaw's own replay and workspace
memory; `crates/domain/src/lib.rs::AuthorityPath` enumerates the authority paths, one of which is a
deterministic human approval path distinct from model write authority.

`crates/domain/src/composition.rs::Guidance` is a control that fails closed — marker, slug, service,
minimum length, leaked comment terminators and unwritten template stubs — and
`crates/domain/src/composition.rs::strip_html_comments` runs *after* marker validation.
`crates/domain/src/composition.rs::melt` is the one composition path and
`crates/domain/src/composition.rs::Alloy` is constructible only through it.
`crates/domain/src/envelope.rs::build` is where records are framed as evidence rather than
instructions; `crates/domain/src/envelope.rs::sanitize_speaker` stops a display name forging
envelope structure; `crates/domain/src/envelope.rs::render_account` destructures exhaustively so a
new store field cannot silently reach the model.

Related and live: HTML comments do not nest, and a comment-closing sequence inside an operator-notes
block once ended it early and composed the remainder into the prompt as the organization's own
instructions. `fixtures/tenants/README.md` carries the rule and the incident.

Treat IronClaw's own injection features as defence in depth, never as a substitute for removing
authority.

### 5. Credentials, custody and redaction

The domain holds locations, never values: `crates/domain/src/ids.rs::CredentialRef` and
`crates/domain/src/ids.rs::CredentialPurpose`, with four purposes that are checked at construction.
Four sealed secret types exist on purpose rather than one shared one —
`crates/records/src/credential.rs::Secret`, `crates/ironclaw/src/client.rs::Credential`,
`crates/reconcile/src/lib.rs::OperatorToken`, `crates/channel-http/src/lib.rs::AdapterToken` — so
that no adapter-to-adapter edge exists. `crates/records/src/credential.rs::EnvCredentials`
understands only one scheme, and an unknown one resolves to nothing rather than looking like an
outage.

`crates/lifecycle/src/attempt.rs::Blocked` is a payload-free enum, total and lossy by design,
because the three upstream strings it replaces can each carry a secret.
`crates/domain/src/custody.rs::restrict_to_owner` is owner-only permissions on every durable store,
with a positive-control test that widens the file first so the test cannot be measuring the umask.

Check logs, errors, CLI output, journals, lifecycle reports, registry publication and `Debug`
implementations. Then state plainly that platform custody makes the operator a custodian of tenant
data: credential custody does not prevent operator access to a store the operator runs.

### 6. Tools, confinement, network and sandboxing

`crates/reconcile/src/ironclaw.rs::MemberConfinement` binds the member by authenticating as it, and
`crates/reconcile/src/ironclaw.rs::BoundMember` is constructible only through that binding, so the
subject comparison cannot be skipped. `crates/reconcile/src/ironclaw.rs::apply` writes two body
shapes and validates each by **read-back**, not by HTTP status — that validation is load-bearing,
because a settings write can return 200 and not apply (§L-9).
`crates/reconcile/src/ironclaw.rs::confirms_disabled` fails closed, and
`crates/reconcile/src/ironclaw.rs::Applied` reports what was written, never whether the member is
confined.

`crates/lifecycle/src/confinement.rs::ConfinementPolicy` is a total function from the committed
service definition, so an operator cannot pick a wider policy than the service declares;
`crates/lifecycle/src/confinement.rs::AUTHORITY_CHANGING` is denied first, because ordering is what
stops later denials being member-reversible;
`crates/lifecycle/src/confinement.rs::REQUIRED_CATALOGUE_WITNESSES` is the anti-vacuity witness, so
a truncated all-disabled catalogue is not read as evidence;
`crates/lifecycle/src/confinement.rs::ToolState` never reads an unrecognized state as disabled.
The capability ceiling is in the type system: `crates/domain/src/capability.rs::Access` has no
write variant, `crates/domain/src/capability.rs::ToolAccess` has no full variant, and
`crates/domain/src/capability.rs::Incoherence` refuses a sandbox service that declares no egress.

Preserve these caveats exactly. Each names the entry that owns it, and `docs/INVARIANTS.md` is
where a disagreement is settled — the wording below is a checklist of overstatements to avoid, not
a second copy of the measurements:

- confinement is **custody-backed** — the member can undo it, measured (§L-6) — and a locked
  disabled state is not reachable at this revision, so that is structural rather than incidental;
- **completeness is BLOCKED** — §L-7 scopes the claim to the settings catalogue, which is not the
  complete executable surface;
- the catalogue has **two axes**, caller and profile, so a bare row count is under-specified
  (§L-1, §L-8, and `docs/experiments/0039-the-catalogue-has-two-axes.md`);
- a member whose approval has ever been turned back on **cannot be re-confined** (§L-9); the remedy
  is a new member;
- **custody of the member credential *is* the control** — the credential
  `crates/cli/src/lib.rs::run_lifecycle_confine` resolves is the one that can undo the confinement
  it applies.

Keep the lanes distinct: the first-party host HTTP tool, managed sandbox egress, client-declared
function tools, runtime built-ins and the sandbox shell are five different things, and a control
over one is not coverage of another.

### 7. Egress, and the claim that the binary owns no network

This claim is arranged in the manifests, not asserted in prose, and that is how to check it:
`crates/cli/Cargo.toml` — the `ironworks` binary — declares no transport and no storage dependency
at all. Transport lives in the adapters. `crates/ironclaw/src/client.rs::Ironclaw` speaks exactly
two endpoints with per-phase timeouts chosen against a measured server-side wait, and
`crates/ironclaw/src/model.rs::CreateBody` sends **no tools array** — the caller-hosted tool lane is
unused by choice, not by limitation, which is a seam policy to assess rather than an IronClaw
constraint to report. `crates/ironclaw/src/model.rs::request_bytes` is the single serializer both
the client and the journal digest, so the fingerprint describes what actually went out.

### 8. Lifecycle, authority and recovery

Intended, recorded and live authority are three separate things.
`crates/lifecycle/src/state.rs::Stage` is recorded progress in declaration order;
`crates/lifecycle/src/authority.rs::Presence` is where silence is never read as absence, and
`crates/lifecycle/src/authority.rs::Custody` distinguishes managed from known-unmanaged from
unverifiable. `crates/lifecycle/src/attempt.rs::Reached` makes a policy claim about an
unauthenticated credential unconstructible. `crates/lifecycle/src/inspect.rs::inspect` is read-only
and surfaces disagreement rather than normalizing it.

On the serving path, `crates/journal/src/lib.rs::Decision` is journalled at the start so recovery
**replays** rather than re-decides, and `crates/journal/src/lib.rs::recover` returns
`crates/journal/src/lib.rs::Recovery`, which is the crash-boundary table B1–B7 in type form.
`crates/ironclaw/src/classify.rs::TurnFailure` implements no `From`, so `?` cannot silently promote
"not sent" into "unknown".

**Revocation and deprovisioning are a named absence with a measured reason, not an omission.** That
reason is `docs/INVARIANTS.md` §F-4, which owns the measurement and its date: at the pin, deleting
a member does not revoke its token, because the session store never consults the user directory —
so suspending or deleting a member removes no authority from an already-issued session. Modelling
that as an authority-removal state would be a lie in the type
system. Confirm it in pinned source before repeating it, and if it holds, file it as `UPSTREAM WORK`
with the seam-side containment that should remain — not as a seam defect the seam can close.

Require durable or provider-issued evidence plus read-back for any side-effecting success. Where
read-back is impossible, require an explicitly unverified result. Confirm fail-closed behaviour for
identity, routing, pins, registry integrity and authority checks, and distinguish it from the
deliberately *degrading* record path, which the invariants permit: a control fails closed, a data
source degrades.

### 9. Architecture and change ownership

The layering is stated once, in `DESIGN.md`'s "Layering", with a table the repository control
checks against the manifests — **and that is the copy to read, because this one drifted.** It used
to assert here that every serving-path library depends on `domain` only, which stopped being true
when the SQLite open protocol was extracted into its own crate and three libraries came to depend
on it. Cargo enforces the shape; verify it in the manifests, never in prose, including this
sentence. Then classify every proposed fix as
seam code, deployment or configuration, a new measurement, a documentation correction, or upstream
work — and flag any remedy that would create a second mechanism for one invariant, or restate a
measured fact in a second place that can drift.

## Classifications

Give every reviewed claim exactly one:

- `ALIGNED` — implementation and truthful documentation match the applicable pinned contract, with
  sufficient evidence.
- `DIVERGENT BY DESIGN` — an intended narrowing that respects the pinned contract and is described
  truthfully.
- `MISALIGNED` — implementation, deployment or documentation contradicts a pinned contract, a
  repository invariant, or a claim made in public.
- `UNVERIFIED` — the evidence is absent, stale, ambiguous, or cannot establish the claim.
- `BLOCKED` — the measurement could not be performed. Never a pass.

Tag proposed work as `SEAM CODE`, `DEPLOYMENT/CONFIGURATION`, `MEASUREMENT`, `DOCUMENTATION` or
`UPSTREAM WORK`.

Severity applies to defects and risks only: `CRITICAL` for plausible cross-tenant disclosure,
credential compromise, unauthorized external effect, or silent loss of the runtime or model privacy
boundary; `HIGH` for reachable authority widening, fail-open identity or confinement, a material
false privacy claim, or unrecoverable turn or delivery behaviour; `MEDIUM` for bounded exposure,
incomplete enforcement, or misleading operational evidence; `LOW` for a defence-in-depth weakness or
a precision problem with no demonstrated unsafe path; `INFO` for an intentional difference, a
confirmed alignment, or an upgrade opportunity.

Every non-aligned finding carries: a stable identifier and title; its classification, severity and
work-owner tag; the invariant or pinned contract it threatens; its evidence, labelled as source,
measurement, inference or target; the concrete consequence; the smallest remediation that keeps
IronClaw official and unmodified and keeps the existing layering; the acceptance evidence, including
a positive control for any negative security check and a read-back for any authority-changing write;
and the residual risk, or the reason it stays unverified or blocked.

Never recommend editing IronClaw in this repository. Where only upstream can close a gap, say so and
describe the seam-side containment that should remain until an official release is qualified and
pinned.

## Citation rules

- Cite local implementation as `path::symbol`. **Never a local line number** — a line number's only
  durable property is being in bounds, and the repository's own checker refuses in-tree line
  anchors for that reason.
- Cite local prose by document and section, or by invariant identifier.
- Cite pinned IronClaw source as `ironclaw@4cb47cfaf3/<path>:<line>`. Line numbers are correct there
  and only there, because that tree cannot drift under a citation.
- Cite official online material with a direct link to the release, source or issue — never a search
  result, and never for behaviour.
- Put each citation beside the claim it supports. If you cannot produce a valid one, downgrade the
  claim to inference or `UNVERIFIED`.

## The report

Return these sections, in order.

**1. Executive verdict.** Three separate verdicts — pinned upstream compatibility; security
alignment; privacy alignment and claim integrity. For each: the classification, the strongest
evidence, the highest-severity open finding, and the most important limitation. Do not collapse them
into one score.

**2. Evidence matrix.** `Dimension | Local mechanism or claim | Pinned IronClaw contract | Evidence | Classification`,
covering all nine dimensions, split into more rows where one dimension has materially different
results. Label any post-pin advisory principle as such.

**3. Prioritized findings**, grouped: current implementation or deployment defects; documentation or
measured-claim drift; measurement gaps and blocked claims; intentional product differences. Order
defects by severity, then exploitability. Do not mix `docs/PRD.md` targets with present defects
unless current prose falsely claims a target is built.

**4. Privacy-claims table.**
`Candidate public claim | MAY SAY / QUALIFY / MUST NOT SAY | Evidence | Required wording or caveat`.
Assess at minimum: tenant isolation, journal content, overall conversation retention, operator
access, credential handling, network and tool confinement, model pinning, model-provider disclosure,
local storage, telemetry, encryption, deletion and export, and TEE readiness.

Four claims are already on the record and must appear in that table:

- `README.md` says a tenant's "records stay its own" and "the model never sees another tenant's
  anything";
- `docs/FRONTEND.md` says the journal "holds identifiers only — no message text, no answers, no
  records, no credentials", and that rendering that restraint *is* the privacy proof;
- the turn journal being content-free does **not** make the system content-free — proposals,
  promoted activities, the channel and IronClaw itself may retain content;
- `MODEL_PIN` records that the provider's catalogue reports every TEE-attested model, including the
  pinned one, as not ready, and says outright to resolve that before making a privacy claim to
  anyone outside the repository. The pin is also not post-hoc auditable: the runtime echoes the
  model on create and reports a fixed literal on retrieval.

**5. Recommended next actions.** A short ordered list. For each: its work-owner tag, why it comes
next, the smallest safe change, and the exact acceptance evidence. Recommend only; implement
nothing.

**6. Audit limits.** Pinned source you could not read, commands not run, live behaviour not
re-measured, dirty-tree effects, and every inference that must not become a public claim.

End with this sentence, filled in accurately:

> Audited working tree `<commit-or-description>` against official IronClaw `<full-pin>`
> (`<release>`); upstream behaviour was established from pinned source only, and current upstream
> guidance was used as `<advisory-or-not-used>`.
