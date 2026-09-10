# 0056 — The served host was never running the pinned model

**2026-09-07.** The Rust cell on `multi-serve` was redeployed from the old single-tenant shape onto
the current architecture: templated units, a per-tenant service account, per-cell state, confinement
attestation, and a registry migrated onto the `org` schema. It worked, and it found four things on
the way. One of them is not about the deployment.

## The finding

**`MODEL_PIN` was not in effect on this host, and had not been for the life of the deployment.**
`ironworks model-policy check` against the cell's runtime reported:

```
selection_enabled=false, default_matches=false, exact_allowlist=false, allowed_count=0
```

No allowlist, selection disabled, and IronClaw's own default not equal to the pin. So the runtime
answered from whatever model it defaults to, and the `model` field the seam sends was not what
decided the answer. `ironworks model-policy apply` corrected it and read the policy back exactly.

**Why this is not a deployment defect.** `MODEL_PIN` states its own status plainly — *"a pin is a
BEHAVIOURAL CLAIM, not a version string"* — and its first reason for choosing
`qwen/qwen3.5-397b-a17b` is privacy: the provider describes it as TEE-attested, and the file notes
that an anonymising proxy "is NOT the same guarantee". Every turn this host served before
2026-09-07 was served without that constraint applied. Nothing was lying: `DESIGN.md` never claimed
the policy was enforced on the host, and §G records that the runtime reports the model as `"reborn"`
on retrieval so the request is not auditable after the fact. What was missing is that **nothing
measured whether the pin reached the runtime**, and the command that measures it is not on any
proof path.

What the correction does and does not establish is printed by the command itself: *"effective
provider model and TEE execution are not verified by this check."* It constrains what may be
**requested**. It does not establish what a provider ran.

## Two defects in units that had never been run

Both were found by starting them, and both are fixed in `deploy/ironworks-attest@.service` with the
measurement recorded beside the line.

- **`ProtectSystem=strict` with only the runtime directory writable made the lifecycle store
  unopenable.** SQLite needs to create its `-wal` and `-shm` companions beside the database, so a
  read-only parent directory fails the open outright. It surfaced as `unable to open database file`,
  which reads as a missing store rather than a sandbox refusing a present one.
- **`RuntimeDirectory=` is removed when a `Type=oneshot` unit exits.** The unit wrote the
  attestation and systemd deleted it on the same tick. The service reported `Finished`; the relay
  would then have found no `current.json` for any new logical turn. A unit that succeeds while its
  output no longer exists is the least visible failure available, which is why
  `RuntimeDirectoryPreserve=yes` now carries a comment rather than sitting there unexplained.

Neither could have been caught by the offline controls: both are properties of systemd's sandbox,
not of the code.

## What the migration cost, which was less than expected

`publish-one` migrated the pre-`org` registry with **no member spent** —
`MIGRATED relationship-demo registry a3f91ef2615d440f -> 5c62e5168c417c8c` — and replaced the legacy
35-byte rollback marker while doing it, which absorbed the same day's rename of the rollback
sentinels without a separate cleanup.

Two preconditions had to be met first, and both were re-measurements rather than changes:
confinement had to be re-certified because the **policy fingerprint had moved** with the binary
(`f948276f77d4fb0f` → `51a4df89ba58f7c6`), and the re-run reported `0 step(s) to policy` — the
member already satisfied the new policy, so nothing was written to IronClaw. Then the model policy
had to agree with the pin, which is the finding above.

**The guidance path could not move in the same operation.** The migration reconstructs the exact
byte string the former no-`org` publisher produced, so a candidate naming a new guidance path is not
recognised as the same binding. The registry still names `/opt/rustworks/GUIDANCE.md`, and that
directory therefore cannot be removed until a later, separate publication moves it.

## What remains

- **The IronClaw unit is still `rustworks-ironclaw@relationship-demo`**, and renaming it is not a
  rename: `ironworks-ironclaw@.service` requires `/opt/ironclaw/artifact-manifest.env`, which this
  host does not have. Both units run the same native binary; the new one adopts the pinned-artifact
  deployment that `dev/build-pinned-ironclaw-artifacts.sh` produces. Until that runs,
  `dev/prove-host.sh` fails on this host — it now expects the new unit name.
- **Two lifecycle stores exist for one tenant.** `/var/lib/rustworks` is the frozen pre-cutover copy
  and is the rollback path; `/var/lib/ironworks` is live. Delete the old one once the new cell has
  been trusted for a while — two stores for one tenant is a divergence waiting for someone to start
  the old units.
- **The Python system was migrated too**, to `/opt/ironworks-py`, with `bridge.service`,
  `multi-watchdog.service` and `multi-backup.service` rewritten. It stopped cleanly — *"clean stop;
  every completed turn is durable"* — and came back serving three tenants: `proof-a`, `nf` and a
  `relationship-demo` on a different channel from the Rust cell's. That it carries three live rooms
  is worth knowing against §F: retiring it is a larger question than one dogfooding tenant.

## What this establishes and what it does not

The cell serves on the current architecture: 148 relay cycles and five attestation refreshes in the
first ten minutes, no failures. Nothing here re-measures the confinement caveats, which stand as
recorded: custody-backed, catalogue-scoped, completeness `BLOCKED`.

## One turn, and it recovered rather than merely ran

`coordinate` was then run against the live Account Service, on a scratch journal so the relay's own
was untouched. It is the first turn this host has served under an applied model policy.

The first attempt returned **`service_unavailable`** — the 30-second wait cliff `DESIGN.md` records,
where the runtime returns 503 rather than a partial status. The seam classified it correctly as *a
turn MAY have run*, refused to retry, and left `outcome=submitted_unknown, delivery=pending,
response=-` in the journal.

Re-running recovered it:

```
recovering … from journal evidence (SubmittedUnknown)
rebuild is byte-identical; replaying the same key
alloy 25328 bytes (6e04406a5f45a59e) · envelope 9253 bytes · 8 record(s) [complete]
request 7cb46f5a5959e368 · key ironworks-334d72-18d319b66cfcc840
response resp_73060aec1cd644b69b25f36f788b5322 (completed)
terminal delivery=delivered
```

**The original turn had completed; only the wait timed out.** Because the rebuild was byte-identical
it replayed the same idempotency key and recovered the existing response rather than billing a
second one — ADR 0005 and crash boundary B3, exercised on a real turn against a live provider rather
than on a probe.

Two things this settles that the deployment alone did not. The applied model policy **does not
break serving**: the pin is requested, accepted and answered, which was open because every prior
turn on this host ran under IronClaw's default. And `usage` reported `runtime cost estimate 0`,
which is `0040`'s finding reproduced in production — a `qwen/` name trips the runtime's
`is_local_model` heuristic and prices the turn at zero. That field remains unusable for spend.

**What the answer establishes is narrow.** One question, one book of eight records, one sample. It
was not graded, and nothing here is a suite result or a pass rate. What it shows is that the whole
path holds end to end: credential to server-resolved scope, scoped records, melt, envelope, dispatch,
a real refusal, and recovery to a delivered answer.
