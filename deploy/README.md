# deploy

What a **served** deployment needs, as opposed to a probed one. The difference from `dev/` is the
whole point.

| | disposable contract probe | served sandbox workspace |
|---|---|---|
| IronClaw profile | `production` | `hosted-single-tenant-volume-sandboxed` |
| process backend | none | one persistent worker per runtime user |
| storage | disposable Compose volume | client-owned `/var/lib/ironclaw` |
| Docker authority | unavailable | one rootless daemon per client control plane |

A probe exists to answer contract questions and be destroyed. A thing that answers messages cannot
do either: a restart that forgets every thread is a restart that loses conversations.

## The units

    rustworks-ironclaw@.service   one trusted control plane and persistent /workspace per client
    user/rustworks-docker.service one rootless worker daemon in each client account's user manager
    rustworks-devstore.service    the demo record store — synthetic accounts only
    rustworks-telegram.service    the channel adapter; the token reaches one process, never a
                                  command line, because an argument is visible in `ps`
    rustworks-relay.service       dev/relay-loop.sh — one fresh `relay` process per cycle

**`devstore` here is a deliberate exception, not a contradiction.** It calls itself a proof
fixture and not a deployment, and that is correct: this unit serves the *demo*, whose records are
invented, RFC 2606-reserved and marked synthetic. It must never become the record store for a
tenant with real records — that path needs the Account Service, and this unit exists partly so
nobody is tempted to point the demo at it.

## Where it lives, and why not the obvious name

**`/opt/rustworks`, not `/opt/ironworks`.** That path is occupied by the Python implementation on
the host this deploys to — a live tree, written to the same day this was checked, carrying a
`DEPLOYED_MANIFEST.sha256` that something verifies. The two systems coexist for as long as
retirement takes, so the Rust one takes a name that cannot be mistaken for it: `/opt/rustworks`,
`/etc/rustworks`, `/var/lib/rustworks`, and `rustworks-*.service` so a `systemctl` listing is
unambiguous.

The units originally said `/opt/ironworks`, written when nothing was known about the host. Copying
a different codebase over a running production tree is what that would have done.

## Why IronClaw is no longer inside the served Compose stack

IronWorks still calls `POST /api/v1/responses`. That is the correct ingress; the mistake was
expecting a request field to select a workspace. In IronClaw 1.4.0, `builtin.shell` reaches a
persistent `/workspace` only when the runtime itself boots with
`IRONCLAW_REBORN_PROFILE=hosted-single-tenant-volume-sandboxed`. The `production` profile used by
`docker-compose.serve.yml` does not wire a user-sandbox process backend.

The local sandbox backend is Docker-managed. Giving the old capability-dropped IronClaw container
`/var/run/docker.sock` would give that container control of the host while making the deployment
look confined. The supported shape is explicit instead:

    client -> IronWorks -> Responses API -> trusted IronClaw control plane
                                           -> per-user worker container -> /workspace

There is one control plane, rootless Docker daemon and storage root per client, following ADR 0008.
The `ironclaw-<client>` service account can reach its own daemon because creating and supervising
that client's worker is its job. It cannot open another client's private runtime directory or
storage root, and it is not a member of the host `docker` group. Neither IronWorks nor the worker
receives a socket. IronClaw's managed proxy remains the worker's egress boundary.

## Installing it

**This is the procedure. It has not been performed.** It was originally written as though done
on `multi-serve` on 2026-09-01; no evidence supports that and the local evidence contradicts it
(see "What is not here"). Read every command below as untested. It is kept because the reasoning
behind two of the choices is not obvious and should not have to be reconstructed.

    # paths and a service user that owns nothing else
    sudo install -d -o "$USER" -g "$USER" -m 0755 /opt/rustworks
    sudo install -d -o root -g root -m 0755 /opt/ironclaw/bin
    sudo install -d -m 0700 /etc/rustworks
    sudo useradd --system --no-create-home --shell /usr/sbin/nologin rustworks
    sudo install -d -o rustworks -g rustworks -m 0750 /var/lib/rustworks
    sudo install -d -o root -g root -m 0711 /var/lib/ironclaw

    # source, without build output, git, generated pages or secrets
    rsync -az --delete --exclude target/ --exclude .git/ --exclude site/ --exclude local/ \
          --exclude __pycache__/ ./ multi-serve:/opt/rustworks/

    # binaries, built in a container so no toolchain lands on the host
    docker run --rm -u "$(id -u):$(id -g)" -v /opt/rustworks:/src -w /src \
      -e CARGO_HOME=/src/.cargo-home rust:1.97-bookworm \
      cargo build --release -p ironworks-cli -p ironworks-telegram -p ironworks-devstore

    sudo install -m 0644 /opt/rustworks/deploy/rustworks-*.service /etc/systemd/system/
    sudo install -d -o root -g root -m 0755 /etc/systemd/user
    sudo install -m 0644 /opt/rustworks/deploy/user/rustworks-docker.service /etc/systemd/user/
    sudo systemctl daemon-reload

Build the pinned IronClaw binary and its worker image from the revision in `IRONCLAW_PIN`, then
install the binary as `/opt/ironclaw/bin/ironclaw` and export the image that the upstream profile
expects. Each rootless daemon has a separate image store, so a host-daemon tag alone is not enough:

    cargo build --release -p ironclaw
    sudo install -m 0755 target/release/ironclaw /opt/ironclaw/bin/ironclaw
    docker build -f Dockerfile.sandbox-worker -t ironclaw-worker:latest .
    docker save -o /opt/ironclaw/ironclaw-worker.tar ironclaw-worker:latest

Install Docker's rootless extras plus `uidmap`, `slirp4netns` and `fuse-overlayfs`. For client slug
`acme`, create a dedicated control-plane user and storage root. The subordinate UID/GID range must
be unique on the host; `100000-165535` is an example allocation, not a fleet-wide constant:

    sudo useradd --system --home-dir /var/lib/ironclaw/acme \
      --no-create-home --shell /usr/sbin/nologin ironclaw-acme
    sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 ironclaw-acme
    sudo install -d -o ironclaw-acme -g ironclaw-acme -m 0750 \
      /var/lib/ironclaw/acme /var/lib/ironclaw/acme/workspace

Copy `deploy/ironclaw-client.config.toml.example` to
`/var/lib/ironclaw/acme/config.toml`, replace every `CLIENT` with `acme`, make it mode 0600 and own
it by `ironclaw-acme`. The explicit `[identity] tenant` and `default_owner` are load-bearing:
IronClaw derives its worker, proxy and network names from that pair. The service preflight refuses
to start if either value differs from the service instance.

`/etc/rustworks/ironclaw-acme.env` is root-owned, mode 0600 and contains at least
`IRONCLAW_PORT=3997`, `IRONCLAW_REBORN_WEBUI_USER_ID=acme`,
`IRONCLAW_REBORN_DOCKER_HOST=unix:///run/user/UID/docker.sock` (with `UID` replaced by
`id -u ironclaw-acme`), the WebUI token, and the model provider key. The UID-specific socket is
operator-owned configuration, not client input. The startup preflight derives the account's actual
UID and rejects any other endpoint. The unit owns the home, profile and workspace path directly.
The WebUI user is checked against both `default_owner` and the service instance before boot.

Docker supports a rootless daemon under that account's user manager, not a system unit with
`User=` ([Docker rootless-mode guidance](https://docs.docker.com/engine/security/rootless/tips/)).
Enable lingering so the user manager survives logout, enable its daemon, load the pinned
worker image into the private image store, then start IronClaw. This host must use cgroup v2 with
the systemd cgroup driver; otherwise rootless Docker ignores IronClaw's worker memory and PID
limits, and the deployment proof fails:

    ACME_UID="$(id -u ironclaw-acme)"
    sudo loginctl enable-linger ironclaw-acme
    sudo -u ironclaw-acme env XDG_RUNTIME_DIR="/run/user/$ACME_UID" \
      systemctl --user daemon-reload
    sudo -u ironclaw-acme env XDG_RUNTIME_DIR="/run/user/$ACME_UID" \
      systemctl --user enable --now rustworks-docker.service
    sudo -u ironclaw-acme env HOME=/var/lib/ironclaw/acme \
      XDG_RUNTIME_DIR="/run/user/$ACME_UID" \
      DOCKER_HOST="unix:///run/user/$ACME_UID/docker.sock" \
      docker load -i /opt/ironclaw/ironclaw-worker.tar
    sudo systemctl enable --now rustworks-ironclaw@acme.service

The second instance used here may be a disposable synthetic client; creating it is part of testing
the deployment and is not gated by the result it exists to measure. It gets another OS user,
non-overlapping subordinate-ID range, port, environment file, identity config, rootless daemon and
storage root, and it does not join the `docker` group. Run
`dev/prove-deployment-isolation.sh acme OTHER` before attaching real client credentials, records or
either client's relay; the proof uses same-named sentinels in the two daemons as positive controls
and verifies that each service user is denied by the other daemon socket. Run
`dev/prove-workspace.sh` once through each client's synthetic IronWorks environment as the separate
positive control that IronClaw's worker and managed proxy operate on that rootless daemon.

Apply the explicit workspace policy, which leaves `builtin.shell` reachable and disables every
other catalogued model tool (including authority-changing and general HTTP tools), then prove that
IronWorks really uses the sandbox:

    ironworks lifecycle confine CLIENT --store /var/lib/rustworks/lifecycle.store
    dev/prove-workspace.sh

The proof creates a random value inside the sandbox, then reads the same value from two fresh
Responses threads. It does not count conversation replay as persistence.

**There is no `--workspace` flag any more, and its absence is the point.** The policy is whatever
the recorded instance's service declares in `capabilities.runtime_tools`, so `confine` cannot be
told to grant a shell to a service that does not ask for one. `--workspace` is refused with a
message naming its replacement rather than ignored.

That closed a contradiction this section used to state and leave open. IronClaw 1.4.0's sandbox
shell always carries its built-in managed allowlist for package registries and GitHub; the only
operator setting adds domains and cannot replace that list with an empty one. Both shipped
services declare `egress: none`, so confining either of them *to a shell* would have made that
declaration false — and nothing stopped an operator doing it, because the flag and the definition
were two independent answers. Now `capabilities.runtime_tools: sandbox` alongside
`egress: none` **does not load at all** (`CapabilityProfile::incoherence`), so the allowlisted-egress
tier this paragraph asked for is `CapabilityProfile::SANDBOX_WORKSPACE`: `egress: read-only`,
records read-only, writes and outreach absent.

**No shipped service declares it**, so nothing here is exercised end to end yet. A workspace
deployment needs a service definition that asks for one.

**The container build is the choice worth keeping.** The host runs the live fleet; installing
rustup there would leave ~1 GB of toolchain on it permanently to produce three binaries. The
container leaves a 78 MB cargo cache under `/opt/rustworks/.cargo-home`, which is kept
deliberately — it makes the next rebuild about a minute rather than several.

**Verify the composition after building, not just that it compiled.** `ironworks compose` on the
host produced `a80c5ffed289fa6f`, byte-identical to the digest graded on an arm64 laptop. That the
alloy digest is architecture-independent is required rather than incidental, and it was not
established until measured.

That digest is now historical: the corpus has moved and the current composition is
`3770e0942624bda4`. Re-verify against whatever `ironworks compose` reports in this tree at the
time you build — the property being checked is that the two architectures agree, not that they
agree on any particular value.

## What is not here

**Nothing here has been deployed.** Checked 2026-09-01 from the primary workstation
(`mbp.local`):

- `multi-serve` does not resolve (NXDOMAIN), has no `/etc/hosts` entry, and appears in neither
  `~/.ssh/config` — which holds one unrelated host — nor `known_hosts`. The `rsync` above names
  `multi-serve:` as its destination, so as written it could not have connected from here.
- `/opt/rustworks`, `/etc/rustworks` and `/var/lib/rustworks` do not exist locally either.
- `hcloud` is installed but has no active context, so whether the server exists at all is
  unconfirmed — only that nothing here can reach it.

So: no `/etc/rustworks/`, no registry, no running unit. The units are written and unproven in
place; `dev/relay-loop.sh`'s own behaviour is tested, the systemd wrapping around it is not.

**One thing would overturn this**: the install running from a different machine that does have
the host configured. If that happened, record it here with the date and the machine, because the
claim it replaces was wrong precisely for lacking both.

**Do not confuse this with the local stack.** `multi/serve/multi-watchdog.sh:44-45` watches
ironclaw on `127.0.0.1:3020` and the Account Service on `127.0.0.1:8443`, and containers answer
on both of those ports on the workstation. That is the Python fleet and a local Account Service,
not this deployment. The operator turn recorded on 2026-09-01 read its records from that local
service.

**And nothing here confines the member.** A served deployment whose member is unconfined can call
`builtin.http` and `outbound_deliver` — see `lifecycle confine` and
`docs/experiments/0012-what-confinement-reached.md`. Applying it is a prerequisite for pointing
any of this at records that matter, not a later hardening step.
