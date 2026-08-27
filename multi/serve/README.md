# Serve host — the 24/7 deployment of the product path

These are the units that keep the multi-tenant product path running on a private host, and they
supersede running the bring-up in [`../README.md`](../README.md) § Running the product path
locally by hand. Everything here is operator-side: the files are installed on the host, and their
configuration lives in mode-`0600` files under `~/.agency/`, never in the repository.

The host runs three things the units depend on: the IronClaw runtime on `:3020`, the Account
Service on `:8443`, and the bridge. `../../deploy/README.md` owns lifecycle, recovery, and
backups; this file says only what each unit is.

## What is here

| file | what it is |
|---|---|
| `cloud-init.yml` | The host as code — pass at server creation and the box builds itself: `ufw` deny-in with SSH only, key-only SSH, unattended upgrades, and a non-root `multi` user. Rebuildable from this file alone. |
| `bridge.service` + `render-bridge-service.py` | The seam bridge as a hardened systemd unit. The checked-in unit is the default `/home/multi/.agency` rendering; the renderer moves its environment file, runtime `AGENCY_DIR`, bind mount, and writable-path exception together for a custom operator directory. |
| `multi-watchdog.sh` + `.service` + `.timer` | Every five minutes, checks the three things serving depends on and alerts a human on state *change* plus an hourly reminder while down. |
| `multi-backup.sh` + `.service` + `.timer` | Nightly at 03:30: `pg_dump` of both databases and `~/.agency` into an encrypted, deduplicated `restic` repository. |

For the default operator directory, install `bridge.service` directly. For a relocated tree,
run `AGENCY_DIR=/absolute/operator/path python3 multi/serve/render-bridge-service.py --output
/etc/systemd/system/bridge.service`. Then run `systemctl daemon-reload` and `systemctl enable
--now bridge`. Do not copy the default unit and set only `AGENCY_DIR` in `bridge.env`: systemd
does not interpolate that variable in `EnvironmentFile=` or `BindPaths=`, so those paths must be
rendered together.

## Two things worth knowing before you rely on them

**The watchdog asks the bridge for forward progress, not for liveness.** `systemctl is-active` and
"no errors in the log" both report green for the failure that costs every tenant at once: a bridge
wedged inside one long turn is not polling, so it logs nothing. The check runs
`./deploy/ironworks bridge status`, which compares the heartbeat against the last successful poll
and the in-flight turn's deadline — the only combination that separates *busy* from *stuck*. A
store it cannot read is unhealthy, never a pass. Poll-error counting is kept as a second,
independent signal because a revoked bot token and a stuck loop fail differently.

**A backup you have not restored is not a backup.** The `restic` password is the only thing that
can decrypt the repository, it is not stored with it, and losing it makes restore impossible —
escrow it separately. Do the restore drill once after setup and periodically after that, into a
fresh temporary target: a snapshot listing is not a restore test. `deploy/README.md` § Host
recovery and backups owns the procedure.
