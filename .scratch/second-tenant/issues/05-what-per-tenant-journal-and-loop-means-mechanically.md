# What per-tenant journal and loop means mechanically

Type: grilling
Status: open
Blocked by: 04

## Question

§23 step 5, and the hard gate: **admitting a second client tenant must not land before this.**

§13 states the requirement as a *property* on purpose, so the mechanism can move:

> **No tenant's failure is observable by another tenant** — not in latency, not in delivery, not in
> quota, not in whether their turn happens at all.

Today the property does not hold, and it is concrete rather than theoretical:
`crates/telegram/src/main.rs` reads a single `TELEGRAM_BOT_TOKEN` from the environment and runs one
poller; `crates/channel/src/lib.rs::safe_cursor` computes the safe offset as the lowest open update
**across every tenant**, for the reason ADR 0006 gives and §E-17 states. One tenant's stuck turn
holds the offset for everyone, including the operator's own coordination.

Choose the mechanism and say why the others lose:

- **Per-tenant journal and per-tenant relay process.** §13 calls this the recommended mechanism —
  the seam's blast radius equals the runtime's, one boundary instead of two that can disagree.
- **One poller, per-tenant cursors.** Cheaper, but leaves two boundaries that can disagree, and the
  acknowledgement barrier in `acknowledge` exists precisely because a slower tenant can lose a race.
- **Per-tenant channel credentials**, which changes what a poller even is.

Whatever is chosen must keep: §B-2 (tenancy in the key), the `acknowledge` barrier's refusal to
regress or pass the barrier, and every crash boundary B1–B12 still landing on a real process
boundary — `relay` is one poll cycle per process and takes no `--daemon` flag, which is what makes
that true.

Also settle whether the **single-poller availability cap** (§I) is closed by this decision or
recorded as an accepted risk.
