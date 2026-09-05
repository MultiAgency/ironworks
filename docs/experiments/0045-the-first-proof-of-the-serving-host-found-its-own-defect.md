# 0045 — The first proof of the serving host found its own defect

Date: 2026-09-04. Host: `multi-serve`, Ubuntu 24.04. Runtime: the tenant-specific
IronClaw 1.4.0 control plane pinned by `IRONCLAW_PIN`. Instrument: `dev/prove-host.sh`, run once
with the operator's approval in the moment.

No transcript file: this measures read-only host observations and the proof's verdicts, not model
answers, and no grader is in the path. The write-up is the evidence. The dated 7 PASS / 1 FAIL / 0
BLOCKED result is not revised after correcting the instrument.

## Why it was measured

Seven facts about `multi-serve` had previously been operator reports: the states of four systemd
units, the live member binding, its settings-catalogue confinement, and the registry digest. The
repository had no automated check that reached the serving host. Ticket 12 built a read-only proof
whose first check is a positive control for SSH and passwordless sudo, then the operator approved
its first live run.

The environment running it had no `Host multi-serve` block. The operator explicitly set
`PROVE_HOST_TARGET` to the literal `<user>@<address>`; the expected user and address stayed
unchanged. The route itself is deliberately not written here — this repository is public — and the
proof has since been changed to require it in `PROVE_HOST_ADDRESS` and `PROVE_HOST_USER` rather
than carry it as a default. That is a change to the instrument after this dated result, not a
revision of it: the run scored below was made against those values as defaults.

## What was measured

The control returned its per-run nonce through non-interactive SSH and sudo. Seven host facts then
matched their recorded claims:

- `rustworks-telegram.service` and `rustworks-relay.service` were loaded, enabled and active;
- `rustworks-ironclaw@relationship-demo.service` was loaded, enabled and active;
- `rustworks-devstore.service` was loaded, disabled and inactive;
- the confinement inspection returned a live report rather than BLOCKED;
- the live settings catalogue held 53 rows, with 2 reachable and approval off, verified against a
  recorded baseline; and
- the first sixteen hexadecimal characters of the live registry's SHA-256 digest were
  `a3f91ef2615d440f`.

The run scored **7 PASS, 1 FAIL, 0 BLOCKED**, exit 2. The failing check was `member.bound`.

## The failure was in the instrument

The runtime reported member `520144ef-c36a-4606-a885-62e553c56ce7`, but the proof defaulted to
`520144ef`. That shortened value had been copied from a truncated display in `deploy/README.md`.
The binding was correct; the matcher required ` in tenant` immediately after the expected value,
so the prefix could never match a correct full subject. The proof invented a difference in exactly
the direction §K-8 forbids.

The default now carries the full UUID. The same correction separates the check's three readings:
a leading lifecycle `BLOCKED` makes `member.bound` BLOCKED, a leading `FAIL` reports the lifecycle
binding failure, and a successful inspection naming another subject reports the different member.
`dev/test-prove-host.sh` exercises all three plus the matching full-UUID control through fake SSH;
it does not reach the host.

## What this establishes, and what it does not

**Establishes.** Seven previously operator-reported host facts became live measurements in one
read-only snapshot. Running the proof also demonstrated that its default member expectation was
wrong, while the host's full member binding was right.

**Does not establish.** The `multi-serve` SSH alias was not measured: the run proved the route to
the expected address and user because it used that address directly. And **7 PASS / 1 FAIL is
not a green proof**. Correcting the instrument does not retroactively produce an 8/8 live result;
that would require another operator-approved run against the real host.

