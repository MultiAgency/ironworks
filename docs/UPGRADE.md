# Upgrading the pinned IronClaw

`IRONCLAW_PIN` names the revision every measured claim in this repository was taken against.
Changing it invalidates those claims until they are re-measured, so a bump is a qualification
pass, not an edit. This file is the record of each one — written so the *next* bump does not
require archaeology through terminal transcripts.

## The rule

A source reading goes stale silently. Diff the contracts to know **where to look**, then
**measure**, and record the measurement. Never infer compatibility from source similarity.

Three verdicts, and the third is not a failure:

```
PASS     reproduced against the target revision
FAIL     the measured contract changed incompatibly
BLOCKED  the measurement could not be performed
```

## Procedure

1. Resolve the exact target: tag, SHA, and whether it is a release, an RC, or a main commit.
   Check ancestry — a release branch is not an ancestor of `main`, so a bump can silently drop
   fixes that were never forward-ported.
2. Diff only the contracts IronWorks relies on (below).
3. Build an image **from the tag**, labelled `ironclaw.rev=<sha>`. `dev/probe-up.sh` refuses an
   unlabelled image on purpose: a third-party image re-tagged by hand would make the provenance
   claim ours without evidence, and the runtime reports only `CARGO_PKG_VERSION`, which cannot
   tell a release from its own release candidate.
4. Bump `IRONCLAW_PIN` **only once a labelled artifact exists**, and update the literal in
   `crates/domain/src/pins.rs::the_committed_pins_parse` — the test exists so a bump is deliberate
   in two places.
5. Re-measure, in this order: provenance → fresh-member authority → idempotent replay → session
   revocation → admin pagination → runtime behaviour → the **six** probe-backed proof scripts
   (`prove.sh`, `prove-delivery.sh`, `prove-lifecycle.sh`, `prove-tenancy.sh`, `prove-boot.sh`,
   `prove-continuation.sh`; `prove-lifecycle.sh` and `prove-tenancy.sh` need no model key).

   `prove-continuation.sh` is the one that chains. Every other recovery proof drives `turn`, which
   never continues a thread, and that gap hid a defect for the whole life of ADR 0005 —
   `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`. A bump that skips
   it re-qualifies only the half that was already working.

   `prove-confinement.sh` is deliberately not in that list: it **consumes its member** (§L-9,
   approval-off is one-way), so it needs a freshly minted probe and cannot be re-run beside the
   others. `prove-workspace.sh` and `prove-deployment-isolation.sh` need a sandbox-profile runtime
   and a deployment host respectively, neither of which a probe provides.
6. **Move `DESIGN.md`'s per-fact provenance markers, one at a time.** Its upstream-contract
   section carries facts marked `[measured @ <rev>]`, and five of them are the source for
   ADR 0003, ADR 0004, §B-14 and `crates/ironclaw/src/classify.rs`. A qualification re-measures some
   contracts and not others, so **only move the marker on a fact this bump actually re-measured**
   — and never restamp the section header to the new revision, which would assign fresh
   provenance to facts of differing vintage. That header used to claim one revision for all of
   them and had gone a whole pin stale before anyone noticed; per-fact markers exist so the next
   one cannot.

---

## 1.3.0 (`70795c16e`) → 1.4.0 (`4cb47cfaf3`), qualified 2026-08-30 — one row BLOCKED

`ironclaw-v1.4.0` is a **lightweight** tag at `4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`,
"promote 1.4.0-rc.1 to 1.4.0 (#7957)". It is **not a descendant of 1.3.0** — 1.3.0 is a release
branch, 1.4.0 comes from main: 85 commits forward, 6 back. Two of the six are real fixes, and
both were forward-ported, so nothing is lost:

```
505cf0a15c  fix(extension-registry): 1.2 activation_state row (#7721)  ->  13eeccabbc
b0ba342268  fix(docker): restore Reborn in-worker SSH (#7723)          ->  053c83f513
```

| Contract | 1.3 evidence | 1.4 source | 1.4 live | Result |
|---|---|---|---|---|
| idempotent replay, same key + same body | measured, 21ms vs 984ms | unchanged | **same id, 0.077s vs 15.78s** | **PASS** |
| same key + different body refused | measured 409 | unchanged | **409 `conflict_error`** | **PASS** |
| member deletion revokes bearer | **false** | byte-identical | **still false — bearer accepted after DELETE** | **PASS** (unchanged) |
| member suspension revokes bearer | false | byte-identical | **still false — accepted after suspend** | **PASS** (unchanged) |
| `/auth/logout` mounted in env-bearer mode | absent | unchanged | **404** | **PASS** (unchanged) |
| session lifetime | 365 days, no config path | `= 365` unchanged | not re-measured | SOURCE-UNCHANGED |
| admin pagination | measured | byte-identical contract | **page 1 = 100 + cursor, page 2 = 2, terminal** | **PASS** |
| adapter traverses to terminal page | n/a (new) | n/a | **observed 102 of 102** | **PASS** |
| fresh-member `builtin.http` | wildcard, `always_allow` | apparently unchanged | **`always_allow`, source `global`, `locked: false`** | **PASS** (unchanged) |
| catalog size | 50 entries | — | **53 entries** | CHANGED-COMPATIBLE |
| `AdminUserRecord` shape / limits | measured | byte-identical | 100/200 confirmed live | **PASS** |
| member creation writes `metadata` | no | unchanged | **no — absent on a fresh member** | **PASS** (unchanged) |
| container boot on our compose profile | worked | entrypoint +120 lines | **failed; fixed without widening** | **CHANGED-REQUIRES-IRONWORKS-CHANGE** |
| `dev/prove.sh` | 7/7 | — | 6 PASS, 1 BLOCKED — the provider rate-limited `chain.end_to_end`; the script exited 3 | **BLOCKED** |
| `dev/prove-delivery.sh` | 4/4 | — | **4/4** | **PASS** |
| `dev/prove-lifecycle.sh` | 5/5 | — | **5/5** | **PASS** |
| `dev/prove-boot.sh` | n/a (new) | — | **4/4** | **PASS** |
| `cargo test` / `clippy` | 275 / 0 | — | **275 / 0** | **PASS** |

### What the one BLOCKED row leaves unmeasured

This cell read `**PASS**` until it was corrected. The script had already answered correctly —
`dev/prove.sh` exits 3 whenever any check is BLOCKED, and it did — so the harness claimed nothing
and a `PASS` was written beside a non-zero exit. **A row whose run exited 3 is `BLOCKED`**, by the
definition at the top of this file: the measurement could not be performed. That the blocked leg
was an unavailable dependency rather than a defect is exactly why `BLOCKED` exists and is not
`FAIL`; it is not a reason to record it as a pass.

What it costs is specific, and larger than one missing row. `chain.end_to_end` is the only check in
`dev/prove.sh` that asserts `40 record(s) [complete]` — it is therefore the **positive control** for
`negative.wrong_credential` and `negative.scope_drift`, neither of which needs a model at all. An
empty store passes every isolation check, so two refusals passing with their control unrun is the
vacuous green §F-14 exists to refuse. **Cross-org isolation was cleared at 1.3.0 and is not cleared
at 1.4.0.**

Re-run `dev/prove.sh` against a probe stack with a working provider key before quoting any
isolation claim at this pin. Nothing else in this table depends on it.

**Re-run 2026-08-30, and the gap is closed: 8 passed, 0 failed, 0 blocked.** The control no longer
depends on a model. `dev/prove.sh` gained `isolation.own_org_control`, which builds the envelope
from real records under the real credential and stops at `--crash-at before-dispatch` — contacting
no runtime, so it still scores when everything upstream of IronWorks is unavailable. It reported
*40 records under the right credential; 0 create calls* beside both refusals in the same run, which
is the combination the qualification run lacked. **Cross-org isolation is cleared at 1.4.0.**

The table above is left as it was measured. A qualification record is a record of a run, not a
running status, and editing it to reflect a later run would destroy the evidence that the gap
existed.

### The one required change — FIXED WITHOUT AUTHORITY WIDENING

1.4.0's entrypoint gained workspace-root handling that does not exist at 1.3.0. It chowns
`$IRONCLAW_REBORN_HOME` to `ironclaw` and *then*, still running as root, `mkdir -p`s
`$HOME/workspace`. The image bakes `/data/ironclaw-reborn` as `1000:1000` mode `0755` and a fresh
named volume is seeded from it — so root, which under `cap_drop: [ALL]` has no `DAC_OVERRIDE`, is
subject to ordinary permission checks and boot died with:

```
mkdir: cannot create directory '/data/ironclaw-reborn/workspace': Permission denied
```

**The fix adds no capability to anything.** A one-shot `ic-init` service pre-creates the directory
**as `ironclaw`**, the uid that already owns it — so it needs no root and no capabilities, and
`mkdir -p` on an existing directory is a no-op that never writes to the parent. The failing
operation stops happening rather than being permitted.

```
runtime  user=root  cap_drop=[ALL]  cap_add=[CHOWN, SETGID, SETUID]   ← unchanged from 1.3.0
ic-init  user=1000  cap_drop=[ALL]  cap_add=[]                        ← strictly narrower
```

`DAC_OVERRIDE` was measured to work and **rejected**: it would have granted the long-lived process
that holds every tenant's credentials the ability to bypass every filesystem permission check, for
its whole life, to paper over one mkdir. What root can do on this profile is exactly what it could
do at 1.3.0.

Pinned by `dev/prove-boot.sh` (4/4), which asserts the fresh-volume boot, the *existing*-volume
restart, and both capability sets — so a future widening is a failing proof rather than a silent
diff. **Any production compose for this image needs the same `ic-init`.**

### Confinement: A-5 and B-8 survive unchanged

1.4.0 adds per-user sandbox containers and a managed egress proxy with a default-deny allowlist.
That governs the **sandbox** lane —
`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/…/managed_egress.rs`. `builtin.http` is a
**first-party host tool** in `ironclaw@4cb47cfaf3/crates/kernel/ironclaw_host_runtime/`, whose
policy is staged from the grant; that file changed by one line, and the permission kernels not at
all.

Measured on a freshly minted member, authenticating as the member (`?user_id=` is silently
ignored, so it must be the member's own bearer):

```
tool.builtin.http        state: always_allow   source: global   locked: false
tool.builtin.http.save   state: always_allow
agent.auto_approve_tools true
```

So a fresh 1.4.0 member still has unapproved outbound HTTP before IronWorks disables anything.
**IronWorks-side confinement is still required**, and the confinement reconciler remains the
next slice. The catalog grew 50 → 53, so the KEEP list should be re-derived rather than copied.

### New upstream capability, not yet used

The tool settings response carries `precedence: ["locked", "override", "global", "default"]`, and
each entry reports `locked: false`. If an operator can *lock* a disabled state, confinement could
become non-overridable by the member rather than merely set — stronger than what
`confine-member.sh` does today. Not measured; worth a probe before the confinement slice.

*Read `confine-member.sh` as the shell script that existed when this was written; the confinement
slice it anticipated was built, and the work is now `ironworks lifecycle confine` with
`dev/prove-confinement.sh` beside it. The paragraph is left as it was written — it records what
was known at the bump, and the question it asks is still open: §L-6 measures ordinary confinement
as reversible by anyone holding the member credential, so whether a **locked** disabled state is
reachable has still not been probed.*

**Closed 2026-09-02, by source read rather than by probe —
`docs/experiments/0037-what-the-seam-never-asked-for.md` check 1.** An operator cannot lock a
disabled state, because nothing can: `locked` is **computed per read** from the capability's own
declared effects — `default_permission == Deny`, or an effect in
`{Financial, ModifyApproval, ModifyBudget}`
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/reborn_services.rs:1951-1962`) — and no
write path sets it. `precedence` describes how a state was resolved, not a control that can be
written. So confinement is **necessarily** custody-backed at this revision, and §L-6 is structural
rather than incidental. The two paragraphs above stand as written; this note is what answered them.
