# Working in this repository

Everything below is either load-bearing for every task or tells you which document to open for
one. Nothing here is a summary of another file — where a fact lives elsewhere, this points at it.

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
  IronClaw's settings API *as that member*. Custody-backed means it holds only while we keep
  exclusive custody of the credential: **the member can undo it**, measured.
- **Verdict** is three-valued. **BLOCKED is never a pass** — the measurement did not happen.
  **REVIEW is never a synonym for FAIL** — the grader could not tell. There is no SKIPPED.

## Shorthand

`§A-1`…`§L-10` is an invariant (`docs/INVARIANTS.md`). `§1`…`§23` is a PRD section. `B1`…`B12`,
`B11b` is a **crash boundary** (`DESIGN.md`). `0001`…`0036` is an experiment. **`§B-2` and `B2` are
one hyphen apart and unrelated.**

## Layering

`domain` depends on nothing. Every other serving-path library depends only on `domain`. The binary
owns no network. `lifecycle` is operator-side and is read by no serving code. **Cargo enforces
this** — it is not a convention anyone has to remember. The diagram is in `DESIGN.md`.

## Which document answers which question

Load the section, not the document. Only the first row is needed for most tasks.

| Question | Where |
|---|---|
| what a word means | `CONTEXT.md` |
| the type model, the layering, the state machine | `DESIGN.md` |
| what the upstream contract actually says | `DESIGN.md`, "The upstream contract" — facts carry the revision they were measured at |
| what a turn does, end to end | `DESIGN.md`, "One turn, end to end" |
| what a crash at point X must leave behind | `DESIGN.md`, "Crash boundaries" |
| what the bar is for a model-input or authority path | `docs/INVARIANTS.md` §A–J |
| what is enforced by a type rather than a test | `docs/INVARIANTS.md` §K |
| anything touching confinement or `crates/reconcile` | `docs/INVARIANTS.md` §L |
| what is measured, what is not built | `DESIGN.md` "Status", and `docs/INVARIANTS.md` §I for open gaps |
| why a hard-to-reverse decision went that way | `docs/adr/` |
| what a measurement established | `docs/experiments/README.md` — **read the run that measured a claim before repeating it** |
| product scope and what a capability must become | `docs/PRD.md` — a **target** document; a requirement in it is not evidence |
| bumping `IRONCLAW_PIN` | `docs/UPGRADE.md`, and only then |
| adding or binding a service definition | `services/README.md` |
| the dogfooded service and its suite | `docs/ACCOUNT-COORDINATION.md` |
| the published pages | `docs/FRONTEND.md` |
| systemd units and the served profile | `deploy/README.md` |

## What is machine-checked, and what is not

`cargo run -p ironworks-claims` is the control. It reads **this file, `README.md`, `CONTEXT.md`,
`DESIGN.md`, `docs/*.md` (one level), `services/*.md` and `fixtures/*/*.md`** and checks: referenced paths
exist, no `path:line` citation points into our own tree, every `path::symbol` citation names a live
symbol, cited experiments and bare document names exist, documented command flags parse, the
spelled-out crate count matches `Cargo.toml`, the experiment tallies match their transcripts, and
that the suite passes and clippy is clean.

**Outside every path and anchor check:** `docs/adr/`, `docs/experiments/*.md` (except the citation
and tally checks, which add the directory back deliberately), `deploy/README.md`, `MODEL_PIN` and
`IRONCLAW_PIN`. It checks **no figure and no digest** — not the alloy digest, not the lockfile
count, not an envelope size. A number in a document is true exactly as long as someone is watching.

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
