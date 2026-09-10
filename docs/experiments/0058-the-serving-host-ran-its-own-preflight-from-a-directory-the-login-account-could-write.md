# 0058 — The serving host ran its own preflight from a directory the login account could write

**Date:** 2026-09-07
**Host:** `multi-serve`, tenant `relationship-demo`
**Pin:** `IRONCLAW_PIN=4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe` (ironclaw-v1.4.0)
**Verdict:** PASS — the host now scores 10 PASS / 0 FAIL / 0 BLOCKED, from 7/3/0 at the start

The intent was narrow: install the artifacts built by `dev/build-pinned-ironclaw-artifacts.sh`, so
the serving binary's provenance would be derivable rather than asserted. Reading the deployment
before touching it turned up a live privilege path that had nothing to do with the install, and
three of the five things worth recording here were found by preparing the change rather than by
making it.

## The privilege path, and why it was invisible

`ironworks-relay@relationship-demo` and `ironworks-telegram@relationship-demo` were active, and both
carry:

    ExecStartPre=+/opt/ironworks/deploy/check-serving-cell.sh %i

systemd's `+` prefix runs that command **with full privileges — as root, ignoring `User=`**. The
script was `multi:multi 0755`, and so was every one of the 11,142 objects under `/opt/ironworks` and
`/opt/rustworks`, including `target/release/ironworks-telegram` and `dev/relay-loop.sh`, which the
units execute. `multi` is the unprivileged login account. Both units are `Restart=always`, so the
script re-executed as root without anyone doing anything.

Two things kept this from being visible. The hardening around it is real — `NoNewPrivileges`,
`ProtectSystem=strict`, `ProtectHome=read-only`, per-client accounts and per-client rootless Docker
daemons — and reads as a unit that has been thought about. And the `+` is one character in a line
whose other 40 characters describe a safety check. A preflight that exists to fail closed was the
thing running attacker-controlled bytes as root.

**Severity, stated honestly:** `multi` is also in the host `docker` group, which is already
root-equivalent, so this was a second path rather than the only one. It still matters. The
per-client rootless daemons exist precisely so that no two clients share an authority boundary, and
removing the docker-group membership — which should happen — would not have closed this.

Closed by `chown -R -h root:root /opt/ironworks /opt/rustworks`. A pre-check first established that
every file was already world-readable and every directory world-traversable, so no service lost a
read: 0 files without `o+r`, 0 directories without `o+x`, 0 objects owned by a third user. The cost
is that deploys into `/opt` now need `sudo`, which is the right trade for a serving host.

## The install was not a binary swap

The live unit was `rustworks-ironclaw@.service`, which predates the artifact design entirely: no
manifest, no artifact preflight, and paths under `/opt/rustworks` and `/etc/rustworks`. The tree's
`deploy/ironworks-ironclaw@.service` adds the manifest as an overriding `EnvironmentFile` and three
preflights, the last being `deploy/check-ironclaw-artifacts.sh`, which refuses to start unless the
installed binary hashes to the root-owned manifest and this client's daemon resolves the manifest's
worker image.

That preflight would have refused the cutover for a second reason: it requires
`/opt/ironworks/IRONCLAW_PIN` to be owned by uid 0, and it was `multi`-owned. The ownership fix was
a prerequisite of the install as well as a fix in its own right — the fail-closed check and the
vulnerability had the same cause.

All three preflights were dry-run by hand under the unit's environment before the switch. The one
apparent failure was the harness: `check-ironclaw-instance.sh` reported `IRONCLAW_PORT must be an
integer from 1 through 65535` because the dry run had not loaded the `EnvironmentFile` that supplies
it. Re-run with the environment, it passed. **A preflight failure in a hand-built harness is a
statement about the harness until the harness is proved to match the unit.**

After the switch, `/proc/<MainPID>/exe` hashed to `47be2bda88…`, the manifest's binary, listening on
`127.0.0.1:3997`.

## The worker image ID is not a function of the image

The manifest pins `IRONCLAW_REBORN_SANDBOX_IMAGE` to a `sha256:` image ID. Three worker images
were compared: the one installed 2026-09-02, one built today, and a third built minutes after the
second from the same revision. All three agreed on **all eight layer digests** (`a7dc34d660dc…`), on
`created` to the nanosecond, on labels, and on entrypoint, env, workdir and user. All three had
**different image IDs**.

So the ID hashes a config carrying per-build history and is not derivable from anything observable
in the image. Of the two worker fields in the manifest, only the layer set is a reproducible
property; `IRONCLAW_REBORN_SANDBOX_IMAGE` identifies *these* bytes and cannot be re-derived by
rebuilding. That is what the builder's closing line — "the manifest identifies these bytes; it is
not a reproducible-build claim" — means in practice, and why `check-ironclaw-artifacts.sh` verifies
that the client daemon *resolves this exact ID* rather than that a rebuild would agree with it.

This entry exists because the opposite was written into
`dev/build-pinned-ironclaw-artifacts.sh` as a measurement first. The hypothesis was that a label-key
difference explained the differing IDs; the test built to confirm it refuted it instead.

The label fix stands on its own: `ironclaw.rev` is the one name the tree reads — `dev/probe-up.sh`
refuses an image that lacks it — and the builder was the only writer spelling it
`ironclaw.revision`, producing images whose provenance that check could not see.

## Two unit defects a reboot would have found first

`dev/prove-host.sh` scored the tenant's relay and Telegram adapter **active but disabled**: running,
and not configured to start at boot. They had been started but never enabled during the earlier
rename. Separately, `rustworks-relay.service` and `rustworks-telegram.service` were still *enabled*
while inactive, so a reboot would have started the superseded pair alongside the current one, both
driving the same tenant and contending for port 3994.

Neither is visible in `systemctl is-active`, which is what a person checks. Both were found by a
proof that asserts `loaded/enabled/active` as one fact rather than three.

## The guidance path is fixed, and `/opt/rustworks` cannot fully go

The live registry names this tenant's guidance at `/opt/rustworks/GUIDANCE.md` — an absolute path
from before the rename. `deploy/README.md` is explicit that a `publish-one` which has reached
`Staged` refuses any registry difference, so **the guidance path cannot be moved without resetting
the lifecycle store, which spends another member on a one-way `confine` (§L-9)**.

The rename is therefore complete except for one file, deliberately. All four `rustworks-*` units
were removed, `/opt/rustworks` was pruned to `GUIDANCE.md` alone, and its digest was verified
byte-identical across the prune. Retiring that last file is worth a member or it is worth nothing;
today it is worth nothing.

## What the proof said, before and after

    7 passed, 3 failed, 0 blocked    (exit 2)   ← immediately after cutover
    10 passed, 0 failed, 0 blocked   (exit 0)   ← after the two enables and the digest correction

The third failure was the proof's own: `registry.digest` expected `a3f91ef2615d440f`, the value from
before the `org`-field migration recorded in `0056`, which reported
`MIGRATED relationship-demo registry a3f91ef2615d440f -> 5c62e5168c417c8c`. The new digest had been
measured and written down; the proof's default and `deploy/README.md` were never updated to follow.
**A recorded constant that outlives the thing it records scores a correct host as a difference** —
the same defect shape as `0045`, where a truncated member id did it.

`dev/prove-host.sh`'s header also still claimed "Nine read-only checks" above a list of ten, and
"the positive control for all seven facts below it" where there are nine. Corrected.

## The one that was worth the whole exercise

`model.policy_matches_pin` **passed after the runtime was replaced.**

`MODEL_PIN` warns in as many words that "a runtime rebuilt, restored or replaced comes back with its
own default and serves", and that the check for it is a manual command on no proof path. Replacing
the binary and cutting to a new unit is exactly that scenario, and it is the first time that warning
has been tested rather than restated. It held — but it held because check 10 exists to say so, and
check 10 was added only days ago. Before that, this cutover would have silently reintroduced the
condition `0056` was written about.

## Limits

- ~~The privilege path was closed by ownership. Nothing prevents a future deploy from re-creating
  it: an `rsync` as `multi` into `/opt/ironworks` now fails rather than silently restoring the
  exposure, which is the intended failure, but no check asserts root ownership of the unit's
  `ExecStartPre` targets. That belongs on a proof path and is not on one.~~ **Closed in the same
  sitting.** `dev/prove-host.sh` check 11, `privilege.root_execution_paths`, reads each serving
  unit with `systemctl cat`, extracts every `ExecStart*=+` path, and walks that path and all of its
  ancestors, failing on any component not owned by uid 0 or writable by group or other. The host
  scores **3 privileged paths, all clean**, and the run went 10/0/0 → **11 PASS / 0 FAIL / 0
  BLOCKED**. Two offline controls in `dev/test-prove-host.sh` give it both polarities: `writable`
  reproduces the exact condition found today and must FAIL, `unscannable` emits no `PRIVSCANNED`
  row and must BLOCK — because a check that passes on silence is the §K-8 defect, and a host that
  runs nothing as root has to be distinguishable from a host nothing could be read from.

  Ancestors are walked rather than only the leaf, which is what makes it the check that would have
  caught this: on the morning of 2026-09-07 `check-serving-cell.sh` was mode 0755 and the exposure
  lived in the *directory* above it.
- `multi` remains in the host `docker` group, which is root-equivalent — **and that turns out to
  establish nothing, because the account already holds `(ALL) NOPASSWD: ALL`.** The group grants no
  authority the operator does not already have, so removing it would close one door beside an open
  one and break the compose stacks and the artifact builder for no gain. Nor is the passwordless
  sudo an oversight: `dev/prove-host.sh`'s remote leg is built on `sudo -n` and its header says so,
  because root is what reads the owner-only registry, the lifecycle store and `relay-<slug>.env`.
  **This is the reason the finding above is about a *service* and not about the operator.** The
  preflight path let a unit escalate through a directory, and it would have survived any change to
  how a person logs in; group membership held by an account with unrestricted sudo is the operator
  having operator access. Stated because this was carried as an open item three times before anyone
  ran `sudo -l`.
- The install proves provenance of the binary from the manifest forward. It does not make the binary
  reproducible: this build resolved IronClaw's own `rust-toolchain.toml` to 1.98.0 and is 40 MB
  larger than its predecessor, a difference concentrated in `.text` (83 MB → 104 MB) and `.rodata`
  (14 MB → 18 MB) and therefore a codegen difference, not different embedded content. Both binaries
  serve the frontend and report `ironclaw 1.4.0`.
- 10/10 is a read-only snapshot of units, member binding, catalogued confinement, registry bytes and
  requested-model policy. It is not a served turn, and confinement remains custody-backed and
  catalogue-scoped (§L-6, §L-7).
