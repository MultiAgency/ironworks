# `ProtectHome` hid the rootless daemon

The first isolated IronClaw control plane installed on `multi-serve`, on 2026-09-02, could not pass
its rootless-Docker preflight while its system unit used:

```ini
ProtectHome=true
```

The dedicated account's Docker daemon was healthy, and its socket existed at
`/run/user/996/docker.sock`. From the system service, however, that path was hidden by systemd's
home-directory protection. IronClaw consequently could not reach the daemon it was required to
use.

## What was measured

- Host: Ubuntu 24.04 `multi-serve`, using cgroup v2 and the systemd cgroup driver.
- Control-plane account: `ironclaw-relationship-demo`, UID 996.
- Runtime: IronClaw 1.4.0 at `4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`.
- Worker backend: a rootless Docker daemon in that account's user manager.
- With `ProtectHome=true`: the service could not reach its configured socket under `/run/user/996`.
- With `ProtectHome=read-only`: the socket remained visible, the preflight passed, and the control
  plane became healthy on `127.0.0.1:3997`.

The service still has `NoNewPrivileges=true`, `PrivateTmp=true`, `ProtectSystem=strict`, explicit
writable runtime paths, a dedicated account and a private rootless daemon. The correction exposes
the read-only home/runtime namespace that contains the socket; it does not grant the service write
access to another tenant's storage or daemon.

## Finding

For this topology, `ProtectHome=true` contradicts the configured rootless-Docker transport because
systemd includes `/run/user` in the hidden namespace. The unit cannot both hide that namespace and
require a socket inside it.

This is an integration defect in the RustWorks systemd policy, not an upstream IronClaw defect.
The exact runtime pin and host topology are part of the measurement because changing the worker
backend or socket location changes the conclusion.

## What changed

`deploy/rustworks-ironclaw@.service` now uses `ProtectHome=read-only`. The installed unit was
reloaded and the isolated control plane was restarted and observed healthy.

