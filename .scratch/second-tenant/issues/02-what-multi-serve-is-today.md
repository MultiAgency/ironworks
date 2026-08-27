# What `multi-serve` is today

Type: task
Status: open
Blocked by: —

## Question

`docs/PRD.md` §23 rests two sizeable items on "the host is reachable (operator, 2026-09-02)", and
that cannot be reproduced from the primary workstation: `multi-serve` returns **NXDOMAIN**, has no
`~/.ssh/config` entry, and `deploy/README.md` records the same finding for 2026-09-01 plus the
statement that the install procedure **has not been performed** and every command in it is
untested.

This is not a dispute — it is an unresolved operational fact that two later tickets hang on
entirely. Establish which of these holds:

- reachable from somewhere other than this workstation (record the route),
- provisioned but not yet DNS-visible (record how it is addressed), or
- not yet standing (then provisioning it is the work of this ticket).

**The answer must record the concrete route** — hostname or address, the SSH path, who provisioned
it, and what it already runs — so no later ticket re-derives it. `deploy/README.md` notes the host
carries a live Python IronWorks tree under `/opt/ironworks` written to the same day it was checked,
which is why the Rust deployment takes `/opt/rustworks`; confirm whether that is still true, because
it is the reason for the whole naming scheme.

This is a `task`, not a decision: nothing is being chosen, but installing the relay and standing up
the synthetic isolation pair are both undecidable until it is answered.
