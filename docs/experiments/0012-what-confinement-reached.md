# What confinement reached, and what it did not

Date: 2026-08-31. IronClaw `4cb47cfaf3`, disposable probe, a freshly minted member.
`ironworks lifecycle confine` applied the read-only-analyst policy and re-measured.

**This file was rewritten within the hour of first being written.** Its first version reported
that a disabled tool had survived under a different name. That was two mistakes, both recorded
below, because how they happened generalises better than the finding does.

## What was applied

53 catalogued tools. The plan wrote 51 disables plus approval-off, in the §L-3 order — approval
first, then the fourteen authority-widening tools, then the rest. Re-measured afterwards rather
than inferred from the writes:

```
VERIFIED  53 catalogued, 2 reachable, approval off
```

§L-8's floor exactly: everything disabled except the two the runtime refuses to disable, held at
`ask_each_time` with automatic approval off.

## What the model was offered afterwards

Asked to list what it can call, it named 26 before and 6 after. The six, resolved against the
catalogue:

| offered to the model | capability id | in the catalogue? |
|---|---|---|
| `builtin__notification_channels_set` | `builtin.notification_channels_set` | **yes — disabled by this run** |
| `builtin__outbound_delivery_targets_list` | `builtin.outbound_delivery_targets_list` | no |
| `builtin__project_create` | `builtin.project_create` | no |
| `builtin__result_read` | `builtin.result_read` | no |
| `builtin__skill_activate` | `builtin.skill_activate` | no |
| `capability_info` | — | no |

## Two mistakes, and what they were

**`__` is an encoding, not a different tool.** `ProviderToolName::encode_capability_str` at the
pin: *"For ordinary capability ids this is `.` → `__` (provider tool grammars reject dots)."* The
first version of this file read that convention as a capability escaping the permission catalogue
under an alias, and named `notification_channels_set` — an authority-widening tool — as the worst
case. It is catalogued and it was disabled. **The check that would have caught this is the one
this repo already applies elsewhere: resolve the name before reasoning about it.**

**Being offered is not being reachable.** `ToolState::Disabled` is documented here as *"refused at
dispatch"* — a disabled tool may still appear in the model's tool list and fail when called. So a
list of six names is evidence about the *offered surface*, not about authority. The first version
treated the two as the same thing, which made a routine observation look like a breach.

## What the measurement does support

Five of the six are absent from `/api/webchat/v2/settings/tools` under any spelling, so no state
written through that endpoint governs them. About those, `ironworks_lifecycle::confinement`'s own
header already says the right thing and this run neither confirms nor refutes it:

> The claim is "all catalogued model tools outside the allowed set are Disabled" — **not** "the
> model has no other executable authority". Whether the catalogue is the whole surface is
> UNPROVEN, and the reference's warning stands: *it cannot disable what nothing lists.*

Still UNPROVEN, now with five named candidates worth an upstream question rather than a guess.
None of them was invoked to find out: `outbound_delivery_targets_list` is the wrong thing to test
by trying.

## What is settled

- Confinement works on what it claims: 51 of 53 disabled, the two undisableable held by
  approval-off, verified by re-measurement rather than by its own writes.
- It is worth applying regardless: the offered surface fell 26 → 6, and input fell 19,599 → 9,709
  tokens, because tool definitions were half the prompt.
- The `VERIFIED` line now states its scope where an operator reads it.
- **The offered surface and the reachable surface are different measurements**, and this repo has
  only ever measured the second. A tool list read from a model answer is not evidence about
  authority, and nothing should cite it as such — including the earlier note in this session that
  the unconfined member "can call" twenty-six tools. It was offered twenty-six.

## Read against §L-1: one number reconciles, one does not

Added 2026-09-02. `docs/INVARIANTS.md` §L-1 reports the same endpoint at the same pin and its
figures are not the ones above. Both halves were checked; they are not the same kind of
disagreement, and neither is resolved by preferring the newer document.

**"The fourteen authority-widening tools" is not in conflict with §L-3's sixteen.** They count
different sets. `AUTHORITY_CHANGING` holds **16** rows and has since `2753384`, the day before this
run — but two of them, both `operator_config_set_*`, are also in `UNDISABLEABLE`, so the plan writes
**14** disables and holds those two at `ask_each_time`. `confinement.rs` computes exactly that
difference where it filters `AUTHORITY_CHANGING` by `!UNDISABLEABLE.contains(t)`. Sixteen is the
deny list; fourteen is what gets disabled. Both sentences are correct as written and each should
say which it means.

**The catalogue size was a real one-row disagreement. It is now RESOLVED, and this run was right.**
This run measured **53 catalogued**, 51 disabled plus the 2 undisableable. §L-1 measured **52
rows**, 50 `always_allow` plus the same 2, and printed the 52 it enumerated.
`ConfinementFacts::catalogued` is `observed.len()` — the row count `GET /settings/tools` actually
returned — so both were live measurements of one endpoint at one pin, one row apart, with no
experiment recording the pass behind the 52.

The probe named above was run on 2026-09-02: a freshly minted member at the same pin,
`GET /settings/tools`, counted and diffed against §L-1's printed list. It returned **53 `tool.*`
rows — 51 `always_allow`, 2 `ask_each_time`** — plus `agent.auto_approve_tools`, for 54 entries.
`dev/prove-confinement.sh` reported `53 catalogued, 2 reachable, approval off` in the same session,
which is the line at the top of this file.

**The diff was one-sided and the row named itself: `nearai.web_search`.** Every id §L-1 enumerated
is live; the live catalogue holds one more. So the enumeration had dropped a row rather than the
two figures measuring different things, and §L-1 has been corrected to 53 / 51. One consequence
travelled with it: `nearai.web_search` is *also* in the host-captured enumeration, so it had been
counted as captured-surface-only when it belongs to both, and that set falls from 9 to 8.

*Nothing in this file changed.* The reading above was correct when written and is correct now.

**Note for anyone re-running it.** `dev/prove-confinement.sh` consumes its member (§L-9, approval-off
is one-way), so a repeat wants `dev/probe-up.sh --down` and a fresh stack rather than an existing
one. Two further cautions, both met by walking into them: the script's check 1 counts *reachable*
rows, not catalogued ones, so a member with anything already disabled reads low and is not a
catalogue measurement; and the script must seed the lifecycle store with the member its credential
actually authenticates as, which it now reads from `/api/webchat/v2/session`.
