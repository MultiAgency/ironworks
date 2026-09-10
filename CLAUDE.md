# Working in this repository

Everything below is either load-bearing for every task or tells you which document to open for
one. **Where a fact is restated here, it is because getting it wrong costs something on every
task, and the restatement names the file that owns it** — so a disagreement is resolved there, not
here. The cost lists are the only ones machine-checked against their owner; the rest are kept
honest by being short enough to see.

## What this is

An operator-run **seam** that serves scoped, managed agentic services to organizations over a chat
channel, on top of official unmodified IronClaw. A tenant gets a service we operate on its behalf —
not a runtime it configures, not a toolkit it builds with. IronClaw is a **third-party runtime this
repo talks to over HTTP**, pinned by `IRONCLAW_PIN`; it is not this codebase.

## The one rule

**Run official, unmodified IronClaw.** A change that would require editing IronClaw's source
belongs upstream, not here. Two corollaries:

- **Never add a fallback literal for a pin.** A default is the one value that can silently outrank
  the configuration, and the model pin is a privacy claim, not a version string.
- **The authority is the pinned source**, then upstream releases and issues, then nothing. A blog
  post, a forum answer, or a model's recollection is not a source for runtime behaviour.

## Vocabulary you will get wrong by reading normally

`CONTEXT.md` defines about fifty terms and is worth reading once. These are the ones whose ordinary
reading is actively misleading:

- **seam** — this repo. Not a proxy, not a wrapper.
- **alloy** / **melt** — the composed instructions, and the act of composing them.
- **book** — a tenant's records. **corpus** is ambiguous and must always be qualified: the
  *reference* corpus is the Python prior art, the *scrap* corpus is what compiles into the binary.
- **probe** — two things: the disposable stack `dev/probe-up.sh` stands up, and, in
  `docs/experiments/`, one case scored many times at a fixed composition.
- **confinement** / **custody-backed** — reducing a member's reachable tool surface by writing
  IronClaw's settings API *as that member*. **Custody-backed is not a lock: the member can undo
  it.** What that guarantee covers and where it stops is `docs/INVARIANTS.md` §L-6 and §L-7, and
  those are the words to quote — describing the policy as locked is the error they exist to catch.
- **Verdict** is three-valued and there is no SKIPPED. **BLOCKED is never a pass** — the
  measurement did not happen. **REVIEW is never a synonym for FAIL** — the grader could not tell.
  `CONTEXT.md` has the full entries; these two mistakes are the ones that change a conclusion.

## Shorthand

`§A-1`, `§L-7` is an invariant — a lettered section and a number (`docs/INVARIANTS.md`). `§13`,
`§19.1` is a PRD section. `B4`, `B11b` is a **crash boundary** (`DESIGN.md`). `0001`, `0046` is an
experiment. **`§B-2` and `B2` are one hyphen apart and unrelated.**

Shapes, not extents: an upper bound here is one more place to remember to edit, and both of the
ones that used to be stated had gone stale.

## Layering

**Cargo enforces the layering — it is not a convention anyone has to remember**, so the thing to
know before editing is that a dependency going the wrong way will not compile, not what the rows
say. `DESIGN.md`'s "Layering" is the shape, the diagram and the tables, and it is the copy the
control checks against the manifests.

## Which document answers which question

Load the section, not the document. Only the first row is needed for most tasks.

| Question | Where |
|---|---|
| what a word means | `CONTEXT.md` |
| the type model, the layering, the state machine | `DESIGN.md`, "Types that carry invariants" / "Layering" / "The state machine" — three questions, three sections |
| what the upstream contract actually says | `DESIGN.md`, "The upstream contract" — facts carry the revision they were measured at |
| what a turn does, end to end | `DESIGN.md`, "One turn, end to end" |
| what a crash at point X must leave behind | `DESIGN.md`, "Crash boundaries" |
| what the bar is for a model-input or authority path | `docs/INVARIANTS.md` §A–J |
| what is enforced by a type rather than a test | `docs/INVARIANTS.md` §J's status paragraph names the ones that landed; §K writes each out with what enforces it |
| anything touching confinement | `docs/INVARIANTS.md` §L-6 the guarantee, §L-7 its limit. `crates/lifecycle/src/confinement.rs` decides the policy; `crates/reconcile/src/ironclaw.rs::apply` writes it |
| what is measured, what is not built | `DESIGN.md` "Status", and `docs/INVARIANTS.md` §I "Gaps with a known shape" |
| may I run a `dev/` script, and what does it cost | "What costs something to run" below, then the script's own header — **the header is the fact** |
| editing anything a model will see | `fixtures/tenants/README.md` — the rule, the incident, and why operator notes live in a separate file |
| publishing a tenant, publishing a guidance update, handing over the cursor | `deploy/README.md` — its procedures |
| why a hard-to-reverse decision went that way | `docs/adr/README.md` — the index, with each decision's status |
| what a measurement established | `docs/experiments/README.md` — **read the run that measured a claim before repeating it** |
| product scope and what a capability must become | `docs/PRD.md` — a **target** document; a requirement in it is not evidence |
| bumping `IRONCLAW_PIN` | `docs/UPGRADE.md`, and only then |
| adding or binding a service definition | `services/README.md` |
| the dogfooded service and its suite | `docs/ACCOUNT-COORDINATION.md` |
| the published pages | `docs/FRONTEND.md` |
| systemd units and the served profile | `deploy/README.md` |
| commissioning a read-only security or privacy audit | `docs/AUDIT-BRIEF.md` — the prompt, and the source-only evidence order it holds an auditor to |
| what we have found that belongs upstream, and what we would offer them | `docs/UPSTREAM.md` — the register of `UPSTREAM WORK`, which cites the run that measured each finding and never restates it |

## What is machine-checked, and what is not

`cargo run -p ironworks-claims` is the control. It reads **this file, `README.md`, `CONTEXT.md`,
`DESIGN.md`, `docs/*.md` (one level), `services/*.md`, `deploy/*.md` and `fixtures/*/*.md`** and
checks:

- **referenced paths** exist, and **line anchors** — no `path:line` citation points into our own tree
- **symbol citations** — every `path::symbol` names a live symbol
- **cited experiments** and **named documents** exist, **experiment numbers** are claimed once, and
  the **experiment index** names the number every write-up and every transcript carries
- **documented commands** — every flag a document spells parses
- **workspace members** — the spelled-out crate count matches `Cargo.toml` — and **layering** —
  `DESIGN.md`'s dependency tables match the manifests
- **dev costs** — the scripts declaring a cost are exactly the ones the cost sections name
- **self-description** — this section names every check the binary runs
- **tallies** — an experiment's written verdict counts match the transcript it names
- **toolchain** — **workspace tests**, **clippy**, **rustfmt** and **rustdoc**: the suite passes and all three are clean, and a status claim `DESIGN.md` stops making is BLOCKED rather than dropped. `cargo doc` is here because `rustdoc::broken_intra_doc_links` is denied and rustdoc is the only thing that evaluates it: neither `cargo test` nor `cargo clippy` runs it, so without this the denial would be inert. It runs with `-D warnings`, so `clean` means no warning at all rather than an exit status of `0`

**Six checks read `docs/experiments/` as well** — the two citation checks, **cited experiments**,
the two experiment checks, and tallies. **`docs/adr/` is read by the two citation checks.** A
pointer is not a claim: a dated record may name a file that has since been deleted, but a
`path:line` or `path::symbol` that no longer resolves is wrong in the present tense however old the
record is. That is why both directories are exempt from the path check and neither is exempt from
the citation checks.

**Outside the path check:** `docs/adr/` and `docs/experiments/*.md`, because both are dated records
— their citations are checked and their paths are not. **`MODEL_PIN` and `IRONCLAW_PIN` are read by
nothing**, which is now the whole of that list.

`deploy/README.md` used to be on it, and `deploy/` paths were exempt wherever they were cited.
Together that left the operator procedure for a serving host unread by anything, and the gap filled
up: renaming the systemd units left nine citations naming a file that no longer existed, in an ADR
and four experiments. Both halves are closed. `deploy/lib/` stays foreign — it is the Python
reference's, and it was the whole reason the directory was once excluded.

**It checks only figures it can derive from the tree** — a crate count, a dependency column, a
transcript tally. Every other figure is asserted and unguarded: not the alloy digest, not the
lockfile count, not an envelope size, not a date. A number nothing derives is true exactly as long
as someone is watching, so prefer a shape, or a figure that carries the command that reproduces it.

## House rules, in the order they get broken

- **Cite `path::symbol`, never `path:line`.** A line number's only durable property is being in
  bounds, which carries no information; the checker refuses in-tree line anchors for that reason.
  Anchors into the pinned IronClaw source keep line numbers and carry an `ironclaw@<rev>/` prefix,
  because that tree cannot drift under a citation.
- **A dated record is never revised to agree with a later tree.** Experiments, ADRs and
  qualification tables take a *pointer* to what superseded them, not an edit.
- **Two mechanisms for one invariant is one mechanism and one thing to keep in sync.** The same is
  true of a fact stated in two documents.
- **Re-measure rather than repeat.** Before quoting a number, run the thing or read the file that
  is checked.
- **A negative check needs a positive control.** An empty store passes every cross-org test; a
  harness can invent a match *or* invent a difference, and both are the same defect (§K-8, §F-14).
- **Verify before you edit.** This tree is often edited while a pass is reading it. Match on
  content, not on a line number you read a minute ago.

## What costs something to run

**Every script in `dev/` declares its own cost on a `# COST:` line in its header, and that line is
the fact.** The two lists below are derived from those lines and are checked against them, in both
directions — a script that declares a cost and is missing here fails the control, and so does a
script named here that declares none. Read the header before running, not after.

These cost something:

- **`dev/probe-up.sh` bills one model turn per bring-up** — a warm-up turn, discarded deliberately,
  because a stack's first turn is not a measurement. `--down` destroys the volumes. It is the
  prerequisite of most of what follows, so its cost is part of theirs.
- **`dev/prove.sh`, `dev/prove-delivery.sh`, `dev/prove-continuation.sh` and
  `dev/prove-workspace.sh` each spend real model turns**, so they cost money and they consume the
  runtime's rate-limit budget — 30 requests per 60 seconds, per user, shared with the chat surface.
  Running two back to back is itself the most common cause of a BLOCKED run.
- **`dev/prove-composition.sh` spends one model turn** and reads the operator inspector back. It
  writes nothing, spends no member and needs `jq`. It measures that the prompt the runtime composed
  carries the alloy at its exact melted length with every part in composition order and the safety
  tail last — **not** byte identity, which the capture's whitespace flattening puts out of reach at
  this pin. It is bounded by a four-run retention ceiling upstream, so it is an instrument and never
  a record; its header and `docs/experiments/0052-the-inspector-flattens-the-prompt-it-shows-you.md`
  say why.
- **`dev/prove-confinement.sh` and `dev/prove-cutover.sh` each spend a member.** Approval-off is
  one-way (§L-9). Both mint their own rather than borrowing the probe's, so the stack stays reusable
  across runs — but the member is gone either way, and `prove-cutover.sh` also needs a model key.
- **`dev/prove-freeze.sh` spends six model turns and mints two members, burning neither.** The
  distinction is the point: §L-9 spends a member when approval-off is written, and this writes no
  settings at all — the two exist only to be two workspaces. It needs a sandboxed probe and reaches
  the probe's Docker volume to take the copy it is measuring.
- **`dev/prove-boot.sh` destroys the probe**, because a fresh volume requires a fresh volume.
- **`dev/prove-host.sh` reaches a live host that serves a real tenant.** It is read-only, and it
  refuses to run until it is told the route rather than carrying one — its header says why. Every
  run there is the operator's decision in the moment, not once. `deploy/README.md` is the procedure.
- **`dev/prove-deployment-isolation.sh` runs as root** against live per-client Docker daemons on a
  deployment host.
- **`dev/relay-loop.sh` is the production serving loop.** It drives real turns for as long as it
  runs.

These cost nothing, and saying so is the point: `dev/prove-tenancy.sh` and `dev/prove-lifecycle.sh`
need no runtime and no model key, and `dev/test-prove-host.sh`, `dev/test-ironclaw-artifacts.sh`,
`dev/test-state-backup.sh` and `dev/build-pinned-ironclaw-artifacts.sh` are offline.
**Free is stated, never inferred from absence** — an unlisted script is a defect in this section,
not a licence.

## Editing anything a model will see

**Tenant guidance is model-visible; operator notes must stay in a separate file.** That is the
whole rule, and it is not a style preference — commenting notes out has already leaked them into a
served prompt as the organization's own instructions, in production.

`fixtures/tenants/README.md` owns the mechanism, the incident and the template, and sits beside the
model-visible `guidance.template.md` so the two are read together. `deploy/README.md` has the
served measurements.
