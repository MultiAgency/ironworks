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

    ironworks-ironclaw@.service   one trusted control plane and persistent /workspace per client
    user/ironworks-docker.service one rootless worker daemon in each client account's user manager
    ironworks-devstore.service    the demo record store — synthetic accounts only
    ironworks-telegram@.service   one tenant channel adapter; its token reaches one process
    ironworks-relay@.service      one tenant relay; one fresh `relay` process per cycle
    ironworks-attest@.timer       refreshes that tenant's confinement evidence every five minutes

**`devstore` here is a deliberate exception, not a contradiction.** It calls itself a proof
fixture and not a deployment, and that is correct: this unit serves the *demo*, whose records are
invented, RFC 2606-reserved and marked synthetic. It must never become the record store for a
tenant with real records — that path needs the Account Service, and this unit exists partly so
nobody is tempted to point the demo at it.

**The relay no longer names it.** The predecessor of `ironworks-relay@.service` used to carry
`Wants=ironworks-devstore.service`, which asserted a fact the unit cannot know: a tenant's store is
a credential and endpoint in the registry the unit already passes as `--registry`. On a host serving
from the Account Service that dependency started a synthetic store nothing read, and neither
operator-side switch turned it off — `disable` does not hold because `Wants=` restarts it, and
`mask` refuses while the unit file occupies the path the mask symlink needs. Both measured. The
dependency is gone; start `devstore` deliberately when a deployment actually serves the demo book.

## Where it lives, and the two paths that keep the old name

**The procedure below describes the host.** `/opt/ironworks`, `/etc/ironworks`,
`/var/lib/ironworks/<slug>`, `/run/ironworks-attest/<slug>` and the `ironworks-ironclaw@`,
`ironworks-relay@`, `ironworks-telegram@` and `ironworks-attest@` units are live. The Python tree
moved aside to `/opt/ironworks-py` — moved, not removed, because it still runs the Account Service
this tenant reads its records from and a Telegram bridge serving three rooms of its own
(`docs/experiments/0056-the-served-host-was-never-running-the-pinned-model.md`).

**The runtime serves from the pinned artifacts.** `ironworks-ironclaw@relationship-demo` replaced
`rustworks-ironclaw@`, and its `check-ironclaw-artifacts.sh` preflight refuses to start unless the
installed binary hashes to the root-owned `/opt/ironclaw/artifact-manifest.env` and this client's
rootless daemon resolves the manifest's worker image. The running process's executable hashed to
the manifest's binary, and all four `rustworks-*` units were removed
(`docs/experiments/0058-the-serving-host-ran-its-own-preflight-from-a-directory-the-login-account-could-write.md`).
What `dev/prove-host.sh` scores there is under "What remains".

**Two paths keep the old name and are not oversights.** `/opt/rustworks` is pruned to
`GUIDANCE.md` alone: the published registry binds that guidance by absolute path, `publish-one`
reconstructs the exact former byte string, so the path could not move in the operation that
migrated the schema (`0056`), and moving it now means resetting the lifecycle store — "What
remains" says what that costs. `/var/lib/rustworks` is the frozen pre-cutover lifecycle store and
journals — the rollback path (`0056`). Delete it once the new cell has been trusted for a while; two
lifecycle stores for one tenant is a divergence waiting to be read from the wrong one.

**The rule the move earned, and it outlives this section:** read a deployed binding out of the
deployment, never out of the procedure that created it. This document described two guidance
layouts while the host ran a third, and a plan written from the document named the wrong directory
twice — `docs/experiments/0055-the-served-host-predates-the-registry-it-is-read-by.md`.

## Why IronClaw is no longer inside the served Compose stack

IronWorks still calls `POST /api/v1/responses`. That is the correct ingress; the mistake was
expecting a request field to select a workspace. In IronClaw 1.4.0, `builtin.shell` reaches a
persistent `/workspace` only when the runtime itself boots with
`IRONCLAW_REBORN_PROFILE=hosted-single-tenant-volume-sandboxed`. The `production` profile the
served stack used before does not wire a user-sandbox process backend, which is why the Compose
file that declared it has been retired rather than kept as a record: its own header told a reader
not to use it, nothing set the image tag it required, and it named a unit file that does not
exist. The contract it once probed is recorded in `docs/experiments/`, which is where a
measurement belongs.

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

**Each client's rootless daemon must be Docker 28 or newer, and this is a hard precondition rather
than a recommendation.** IronClaw creates the worker's egress network with
`com.docker.network.bridge.gateway_mode_ipv4=isolated`
(`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/managed_egress.rs:50`) and
then verifies the network carries it
(`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/railway.rs:136`), so a
daemon that does not know the option cannot host a sandbox at all. **Measured, not inferred from a version number:** 27.5.1 refuses the create with
`400 unknown gateway mode "isolated"` and the shell tool reports a sandbox infrastructure error
with nothing written; 28.5.2 and 29.8.0 both accept it and the network reports
`gateway_mode=isolated`. So the floor lies between 27.5.1 and 28.5.2 — **28.0.0 itself is
untested**, and an operator pinning an early 28 should confirm rather than assume
(`docs/experiments/0059-the-sandbox-works-once-the-model-is-told-the-tools-other-name.md`).
Upstream's own sandbox reports run 29.x. **Nothing on this host has been checked against this** —
the requirement was found on a laptop probe, and no served host's daemon version has been read.

## Installing it

**Exercised end to end on `multi-serve`**: first as far as a healthy control plane, then through
mint, adopt, confine, publish and a served turn. "What remains" below says what that does and does
not establish.

Running it found defects in the procedure each time, which is why the warnings below are specific.
From the first pass: rsync exclusions must be rooted, since an unrooted `--exclude site/` also
removed `crates/site` and left a workspace Cargo could not load; and the IronClaw unit needs
`ProtectHome=read-only` rather than `true`, because systemd includes `/run/user` in `ProtectHome`
while the rootless Docker socket the preflight verifies lives there. From the second: the cargo
cache and guidance exclusions, the unit reinstall on every re-sync, the truncating `install
/dev/null`, the store that must be running before `adopt`, and what `--dry-run` does not prove.

    # paths and a service user that owns nothing else
    sudo install -d -o root -g root -m 0755 /opt/ironworks
    sudo install -d -o root -g root -m 0755 /opt/ironclaw/bin
    sudo install -d -m 0700 /etc/ironworks
    sudo useradd --system --no-create-home --shell /usr/sbin/nologin ironworks
    sudo install -d -o ironworks -g ironworks -m 0750 /var/lib/ironworks
    sudo install -d -o root -g root -m 0711 /var/lib/ironclaw

    # source, without build output, git, generated pages, secrets, the cargo cache, or any
    # tenant guidance a published registry points at
    rsync -az --delete --rsync-path="sudo rsync" --no-o --no-g \
          --exclude /target/ --exclude /.git/ --exclude /site/ --exclude /local/ \
          --exclude /.cargo-home/ --exclude /GUIDANCE.md --exclude __pycache__/ \
          ./ multi-serve:/opt/ironworks/

**Three parts of that line are load-bearing, and so is `-o root -g root` above.** This tree is
root-owned because systemd executes part of it *as root*: a unit line beginning `ExecStartPre=+`
ignores `User=`, and all three serving units have one; `ironworks-attest@.service` is `User=root`
outright and runs `deploy/refresh-attestation.sh` every five minutes. When `/opt/ironworks` was
owned by the login account, that account could rewrite the preflight and get root at the next
restart — unattended,
because the units are `Restart=always`
(`docs/experiments/0058-the-serving-host-ran-its-own-preflight-from-a-directory-the-login-account-could-write.md`).

- `--rsync-path="sudo rsync"` — without it a plain `rsync` now fails with
  `mkstemp … failed: Permission denied (13)`. **That failure is the guard working, not a step to
  route around.**
- **`--no-o --no-g` — without these the fix undoes itself.** `-a` implies `-o -g`, and rsync running
  as root on the far side *applies* them, stamping every file with the operator laptop's local uid
  and gid. Measured 2026-09-07: the same sync proposes **410** owner/group changes without these
  flags and **none** with them. A uid that means one person locally means something else, or
  nothing, on the host.
- `--dry-run -i` before a real run, reading the `*deleting` lines. A dry run proves nothing about
  *permissions*, though — it never attempts the write. The plain-`rsync` refusal above had to be
  measured for real, not inferred from a clean dry run.

`dev/prove-host.sh` check 11 asserts the ownership all of this depends on.

**Every exclusion is rooted, and two of them protect files that exist only on the host.**
`/.cargo-home/` is the build cache, and losing it turns a one-minute rebuild into a cold one.
`/GUIDANCE.md` is worse: **a published registry names its tenant's guidance by absolute path**, and
a `publish-one` that has reached `Staged` will not republish against a different one — it compares
the live registry with the candidate and refuses any difference. So the guidance path is fixed at
publication and cannot be moved without resetting the lifecycle store, which means spending another
member on a one-way `confine`. **Choose that path before publishing, and never let a sync delete
what it names.** To check a sync before running it, use `--dry-run -i` and read the `*deleting`
lines; grepping for `^deleting` matches nothing and reads as safe when it is not.

    # binaries, built in a container so no toolchain lands on the host. Root, not the invoker:
    # the tree is root-owned, and `target/release/` holds binaries two units execute.
    sudo docker run --rm -u 0:0 -v /opt/ironworks:/src -w /src \
      -e CARGO_HOME=/src/.cargo-home rust:1.97-bookworm \
      cargo build --release -p ironworks-cli -p ironworks-telegram -p ironworks-devstore

    sudo install -m 0644 /opt/ironworks/deploy/ironworks-*.service /etc/systemd/system/
    sudo install -m 0644 /opt/ironworks/deploy/ironworks-*.timer /etc/systemd/system/
    sudo install -d -o root -g root -m 0755 /etc/systemd/user
    sudo install -m 0644 /opt/ironworks/deploy/user/ironworks-docker.service /etc/systemd/user/
    sudo systemctl daemon-reload

**These unit-install lines belong to every re-sync, not just the first install.** `rsync` updates
`/opt/ironworks/deploy/`; it does not touch `/etc/systemd/system/`, so systemd keeps running
whatever units it was given last. A drifted unit fails in the least obvious way available — the one
that cost a serving loop here pointed `--registry` at the wrong directory and omitted its
`EnvironmentFile`, so the relay started cleanly and then could not read the registry it had just
been handed. Check before starting anything:

    for u in /opt/ironworks/deploy/ironworks-*.service \
             /opt/ironworks/deploy/ironworks-*.timer; do
      diff -q "/etc/systemd/system/$(basename "$u")" "$u" || echo "DRIFTED: $(basename "$u")"
    done

Each cell has four root-owned environment files. All are read by systemd as root before it changes
to `User=ironworks-<slug>`, so none has to be readable by the service account. The channel adapter
credential is deliberately shared by exactly that cell's relay and adapter; the others belong to
one unit or one cell.

**`install /dev/null` truncates, so run it once per file and never again.** These files hold a
runtime member's `api_token`, which this pin issues exactly once and cannot reissue (`0043`); a
second `install` over a populated file destroys a credential replaceable only by minting a new
member and deleting the old one. It has cost one. To add a value to a file that already exists,
`sudoedit` it; to check first, `sudo test -s <path> && echo populated`.

    sudo install -o root -g root -m 0600 /dev/null \
      /etc/ironworks/telegram-relationship-demo.env
    sudoedit /etc/ironworks/telegram-relationship-demo.env
    # exactly: TELEGRAM_BOT_TOKEN=<the BotFather token>

    sudo install -o root -g root -m 0600 /dev/null \
      /etc/ironworks/relay-relationship-demo.env
    sudoedit /etc/ironworks/relay-relationship-demo.env
    # endpoints and the exact handles named by this cell's one-entry registry:
    # IRONWORKS_BASE=http://127.0.0.1:3997
    # IRONCLAW_API=http://127.0.0.1:3997               # lifecycle commands read this name
    # IRONWORKS_RECORDS=<the store serving this tenant>
    # IRONWORKS_RECORDS_TOKEN=<records bearer>
    # IRONCLAW_MEMBER_TOKEN=<IronClaw member bearer>
    # IRONWORKS_MEMBER_ID=<member id recorded by lifecycle>

    sudo install -o root -g root -m 0600 /dev/null \
      /etc/ironworks/cell-relationship-demo.env
    sudoedit /etc/ironworks/cell-relationship-demo.env
    # exactly two literal, distinct, fleet-unique ports:
    # ADAPTER_PORT=3994
    # IRONCLAW_PORT=3997

### The channel adapter's own credential

**`/poll` is a destructive write.** The offset is the acknowledgement, so
`GET /poll?offset=999999999` tells Telegram to discard everything it is holding — before the seam
has journalled any of it, with no row left to recover from and nothing reporting an error. Loopback
is not a boundary on this host: the Python fleet under `/opt/ironworks-py` and one
`ironclaw-<slug>` process per client all run here, under their own accounts, and any of them can
open `127.0.0.1:3994`. So the adapter requires a bearer token on `/poll` and `/send`, and refuses to
start without one (exit 64). `/health` stays open; it is the one route with no backlog behind it.

**And loopback is not the edge of this host either.** A `cloudflared` tunnel publishes
`localhost:3011` — the `secretary` cell's IronClaw — to the public internet under its own hostname.
Nothing is bound to a public interface, so `ss -ltnp` shows only SSH and reads as a host with no
exposed service. **Do not reason about this machine's attack surface from its listeners.** Measured
2026-09-07: every data route on that instance refuses an unauthenticated request with `401`,
through the tunnel as well as on loopback — session, tool settings, threads and response retrieval
alike; only the WebUI shell answers `200`. So this is a note about where to look, not a defect. The
tunnel belongs to `secretary` rather than to anything here, and is recorded because a reader
working out what shares this host will not otherwise find it.

**One file, read by both units.** The same secret pasted into `telegram-<slug>.env` and
`relay-<slug>.env` is two
things to rotate, and a rotation that updates one copy is a channel that stops — which is the same
symptom as the adapter being down, several layers from its cause.

    sudo install -o root -g root -m 0600 /dev/null \
      /etc/ironworks/channel-adapter-relationship-demo.env
    sudoedit /etc/ironworks/channel-adapter-relationship-demo.env
    # exactly: CHANNEL_ADAPTER_TOKEN=<32+ random bytes, e.g. `openssl rand -hex 32`>

The same `install /dev/null` warning applies as above: it truncates, so run it once per file. This
one is cheaper to lose than a runtime member's `api_token` — it is a value you generate rather than
one a pin issues exactly once — but replacing it is still a coordinated restart of both units.

**Roll it out in this order, and the order is not a preference.** The relay ignores whether the far
end wants a credential; the adapter does not.

1. Write `/etc/ironworks/channel-adapter-relationship-demo.env`.
2. Install the unit files and `systemctl daemon-reload` — the `install`/`daemon-reload` lines under
   "Installing it", which belong to every re-sync and not only the first install. Both units gained
   an `EnvironmentFile` line, so a drifted unit here reads no token at all.
3. Restart **`ironworks-relay@relationship-demo.service` first** — the side that *sends* a token. An adapter that
   ignores it is unaffected, so the channel keeps working throughout.
4. Restart **`ironworks-telegram@relationship-demo.service` second** — the side that *requires* one.

Reverse those two and the channel is down between them: the adapter refuses every poll and send
from a relay that is not yet presenting a token. It fails in the safe direction — a refused poll
carries no `updates` array, so the seam reads it as malformed and the cursor does not move, and a
refused send carries `"accepted": false`, which is provable absence and licenses a free resend. But
"safe" here means recoverable, not invisible: the room goes unanswered for the length of the
window.

Verify from the host. The middle call is the control: without it, an adapter that authenticated
nothing would pass the other two:

    curl -s http://127.0.0.1:3994/health
    # {"ok":true}   — no credential, and no backlog behind it
    curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:3994/poll?offset=0"
    # 401           — the control: the same route, no credential
    sudo bash -c '. /etc/ironworks/channel-adapter-relationship-demo.env; \
      curl -s -o /dev/null -w "%{http_code}\n" \
        -H "authorization: Bearer $CHANNEL_ADAPTER_TOKEN" \
        "http://127.0.0.1:3994/poll?offset=0"'
    # 200

**Offset 0 acknowledges nothing, which is why it is the safe one to check with.** Any other offset
in that last command discards a real backlog.

**`IRONWORKS_RECORDS` names whichever store serves this tenant**, and the choice is a real one. A
tenant with real records uses the Account Service and needs a provisioned org and a bearer. The
demo book this repository ships uses `ironworks-devstore` on `3996`, whose identity is
`dev-token-coordination` resolving to org `coordination-eval` — written in the committed unit, so
that path carries no records secret at all. Point a tenant with real records at devstore and you
have done the one thing that unit's own comment forbids.

**Whichever it is must be running before `adopt`**, which resolves the handle through
`IRONWORKS_RECORDS` and records the org it observes. A store that is installed but inactive makes
`adopt` refuse, correctly and confusingly, since nothing about the instance is wrong. Getting this
wrong late is worse than early: `publish-one` refuses when the recorded org and the handle's org
disagree, and the fix is a fresh store, because `adopt` will not overwrite a slug.

Do not put `TELEGRAM_BOT_TOKEN` in `relay-<slug>.env`; only the adapter calls Telegram. Do not put
`CHANNEL_ADAPTER_TOKEN` in either of these two: it belongs to both units, and giving it a file of
its own is what keeps one copy rather than two. The files may be created before or after their unit
files are installed, but each must exist before its own unit is started. Installing them does not
start any service.

Build the IronClaw binary and worker image from the exact official Git object named by
`IRONCLAW_PIN`; do not build either artifact from a checkout's current files. The build script
archives that object into a fresh temporary tree, builds the unmodified archive, and emits one
local manifest beside its binary and worker archive. `IRONCLAW_PIN` is its only revision input and
has no fallback:

**This discipline covers the artifacts this repository builds, and nothing else sharing the host.**
Surveyed 2026-09-07: four IronClaw runtimes run on `multi-serve`, and three carry an `ironclaw.rev`
label matching the pin — but two of those, `multiclaw` and `secretary-ironclaw-1`, are tagged
`ironclaw:main`. **A floating tag pins nothing.** Their labels agree with `IRONCLAW_PIN` today; a
rebuild of `main` moves them and the label re-attests to whatever was built rather than catching
the drift, because a label records what an image *is*, not what it was supposed to be. `probe-up.sh`
refuses an unlabelled image for exactly this reason and cannot help here, since these are labelled
and correct — right up until they are not. The fourth, `ironclaw-nbot`, is deliberately
`rollback-v010-pre140` at 1.3.0, which `docs/UPGRADE.md` records is **not an ancestor of 1.4.0**.

**Pinned 2026-09-07, and the shape of the defect is worth keeping.** Their compose files read
`image: ${IRONCLAW_IMAGE:-ironclaw:main}` — **a fallback literal for a pin**, the exact hazard the
one rule names: a default is the one value that can silently outrank the configuration. It sat two
lines from a correctly digest-pinned `postgres:16-alpine@sha256:…`. Both stacks now set
`IRONCLAW_IMAGE` to the image **ID** in their `.env`, which takes effect on the next recreate and
changes nothing running. The ID rather than the `ironclaw:4cb47cfaf` revision tag that image also
carries, because a tag can be re-pointed and an ID cannot — and `name@sha256:` is a registry digest
form these locally-built images have no registry to satisfy.

    BUILD_ROOT="$(mktemp -d)"
    /opt/ironworks/dev/build-pinned-ironclaw-artifacts.sh \
      --source /path/to/official/ironclaw-checkout \
      --output "$BUILD_ROOT/artifacts"

Keep that output together in its private staging directory until every affected client can
be stopped and upgraded in one attended window. Installing the binary and manifest and loading
the image into each daemon is not atomic: a
running instance could otherwise see a new binary beside an old manifest, or a manifest naming an
image its daemon has not loaded. The safe rollout below stops every affected active IronClaw
instance, installs the manifest last, and starts all of them explicitly. `enable --now` is not an
upgrade operation and must not be read as restarting an already-running instance.

The manifest records the exact revision, the installed binary's SHA-256, the worker archive's
SHA-256, and Docker's immutable `sha256:…` image ID. These identify one local build's bytes; they
do **not** claim that another machine will reproduce them. The archive digest is checked before
loading, then the staging archive can be discarded: startup needs the loaded image, not the tarball
that transported it. The IronClaw unit loads the root-owned manifest after the client environment,
verifies the installed binary and image ID in that client's private daemon, and refuses startup on
any missing or mismatched value. The manifest uses the pin's
`IRONCLAW_REBORN_SANDBOX_IMAGE` name directly, never `ironclaw-worker:latest`, so systemd can exec
the installed binary without a variable-renaming wrapper.

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

`/etc/ironworks/ironclaw-acme.env` is root-owned, mode 0600 and contains at least
`IRONCLAW_PORT=3997`, `IRONCLAW_REBORN_WEBUI_USER_ID=acme`,
`IRONCLAW_REBORN_DOCKER_HOST=unix:///run/user/UID/docker.sock` (with `UID` replaced by
`id -u ironclaw-acme`), the WebUI token, and the model provider key. The UID-specific socket is
operator-owned configuration, not client input. The startup preflight derives the account's actual
UID and rejects any other endpoint. The unit owns the home, profile and workspace path directly.
The WebUI user is checked against both `default_owner` and the service instance before boot.

Docker supports a rootless daemon under that account's user manager, not a system unit with
`User=` ([Docker rootless-mode guidance](https://docs.docker.com/engine/security/rootless/tips/)).
Enable lingering so the user manager survives logout and enable its daemon. Each private daemon
preserves the
content-addressed image ID recorded in the manifest; the startup preflight checks it before the
control plane is allowed to start. This host must use cgroup v2 with
the systemd cgroup driver; otherwise rootless Docker ignores IronClaw's worker memory and PID
limits, and the deployment proof fails:

    ACME_UID="$(id -u ironclaw-acme)"
    sudo loginctl enable-linger ironclaw-acme
    sudo -u ironclaw-acme env XDG_RUNTIME_DIR="/run/user/$ACME_UID" \
      systemctl --user daemon-reload
    sudo -u ironclaw-acme env XDG_RUNTIME_DIR="/run/user/$ACME_UID" \
      systemctl --user enable --now ironworks-docker.service

For a first installation, finish creating every client account and daemon before continuing, and
list every new instance below. For an upgrade, put **every currently active instance receiving
these shared artifacts** in `AFFECTED_CLIENTS`; do not leave an old process running across the
non-atomic installation window. The example names are placeholders. The staging archive is
verified, loaded, and its manifest identity read back from every daemon before the root-owned,
non-writable manifest becomes authoritative:

    (
    set -euo pipefail
    AFFECTED_CLIENTS=(acme globex)
    for client in "${AFFECTED_CLIENTS[@]}"; do
      sudo systemctl stop "ironworks-ironclaw@$client.service"
    done
    sudo install -o root -g root -m 0755 \
      "$BUILD_ROOT/artifacts/ironclaw" /opt/ironclaw/bin/ironclaw
    worker_archive="$BUILD_ROOT/artifacts/ironclaw-worker.tar"
    worker_archive_sha="$(sed -n 's/^IRONCLAW_WORKER_ARCHIVE_SHA256=//p' \
      "$BUILD_ROOT/artifacts/artifact-manifest.env")"
    test "$(sha256sum "$worker_archive" | awk '{print $1}')" = "$worker_archive_sha"
    worker_image="$(sed -n 's/^IRONCLAW_REBORN_SANDBOX_IMAGE=//p' \
      "$BUILD_ROOT/artifacts/artifact-manifest.env")"
    [[ "$worker_image" =~ ^sha256:[0-9a-f]{64}$ ]]
    for client in "${AFFECTED_CLIENTS[@]}"; do
      uid="$(id -u "ironclaw-$client")"
      sudo -u "ironclaw-$client" env HOME="/var/lib/ironclaw/$client" \
        XDG_RUNTIME_DIR="/run/user/$uid" \
        DOCKER_HOST="unix:///run/user/$uid/docker.sock" \
        docker load < "$worker_archive"
      sudo -u "ironclaw-$client" env HOME="/var/lib/ironclaw/$client" \
        XDG_RUNTIME_DIR="/run/user/$uid" \
        DOCKER_HOST="unix:///run/user/$uid/docker.sock" \
        docker image inspect "$worker_image" >/dev/null
    done
    sudo install -o root -g root -m 0644 \
      "$BUILD_ROOT/artifacts/artifact-manifest.env" /opt/ironclaw/artifact-manifest.env
    sudo systemctl daemon-reload
    for client in "${AFFECTED_CLIENTS[@]}"; do
      sudo systemctl restart "ironworks-ironclaw@$client.service"
      sudo systemctl is-active --quiet "ironworks-ironclaw@$client.service"
      test "$(sudo systemctl show "ironworks-ironclaw@$client.service" \
        --property=Result --value)" = success
      test "$(sudo -u "ironclaw-$client" env HOME="/var/lib/ironclaw/$client" \
        IRONCLAW_REBORN_HOME="/var/lib/ironclaw/$client" \
        /opt/ironclaw/bin/ironclaw config get identity.tenant)" = "$client"
    done
    sudo systemctl enable "ironworks-ironclaw@acme.service"
    )

An active unit after that explicit restart means both `ExecStartPre` checks ran successfully
against the newly installed files and client custody. The final `config get` is the non-secret
identity read-back from the restarted binary; it must print the loop's client slug. If any command
fails, leave the affected control planes stopped and reconcile the installed set rather than
installing another manifest over the conflict.

Passing the startup checks establishes local artifact identity and availability in this one
daemon. It does not establish a reproducible build, the effective model used for a turn, resistance
to the root operator, or that a worker was created or invoked. In particular, no shipped service
currently declares the sandbox capability, so active worker use still requires the separate
synthetic workspace proof described below before any product claim can rely on it.

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

    ironworks lifecycle confine CLIENT --store /var/lib/ironworks/lifecycle.store
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
container leaves a 78 MB cargo cache under `/opt/ironworks/.cargo-home`, which is kept
deliberately — it makes the next rebuild about a minute rather than several.

**Verify the composition after building, not just that it compiled.** `ironworks compose` on the
host produced `a80c5ffed289fa6f`, byte-identical to the digest graded on an arm64 laptop. That the
alloy digest is architecture-independent is required rather than incidental, and it was not
established until measured.

That digest is now historical: the corpus has moved and the current composition is
`3770e0942624bda4`. Re-verify against whatever `ironworks compose` reports in this tree at the
time you build — the property being checked is that the two architectures agree, not that they
agree on any particular value.

### Minting, adopting and confining the member

`dev/prove-cutover.sh` runs this same sequence against a probe with a disposable candidate. Here
nothing is disposable.

**Mint.** The bearer is returned once and never again, and at this pin user creation precedes it
(`docs/experiments/0043-user-creation-can-outlive-token-minting.md`): a mint that fails after the
user exists leaves a **tokenless user that cannot be reissued**, because the pin has no reissue
endpoint. Read the token before doing anything else, and if it is absent reconcile and delete the
orphan before retrying rather than minting a second user beside the first.

    curl -s -X POST "$IRONCLAW_API/api/webchat/v2/admin/users" \
      --config - -H 'content-type: application/json' \
      -d '{"email":"...","display_name":"...","role":"member"}' <<'CURL'
    header = "authorization: Bearer <operator token>"
    CURL

**Never fall back to the operator bearer when the member token cannot be read.** It would confine
the operator and report success — the caller scope is the member's own (§L), and an operator-scoped
confinement measures a caller no tenant will ever have.

**Adopt, then confine.** Both take the slug and the store; neither accepts an identity, because
neither may assert one. `--channel` is the bare channel id the relay will answer in, e.g. a Telegram
chat id like `-1002000001`.

    export IRONCLAW_API=http://127.0.0.1:3997
    export IRONWORKS_RECORDS=<account service base>
    export CLIENT_RECORDS=<records bearer> CLIENT_MEMBER=<the minted member bearer>
    export IRONCLAW_OPERATOR_TOKEN=<operator token> IRONCLAW_MEMBER_TOKEN="$CLIENT_MEMBER"

    ironworks lifecycle adopt CLIENT --store /var/lib/ironworks/lifecycle.store \
      --service relationship-intelligence --channel <channel id> \
      --records-credential env:CLIENT_RECORDS --member-credential env:CLIENT_MEMBER

    ironworks lifecycle confine CLIENT --store /var/lib/ironworks/lifecycle.store --dry-run
    ironworks lifecycle confine CLIENT --store /var/lib/ironworks/lifecycle.store

**`--dry-run` shows the plan; it does not rehearse the run.** It opens the store read-only and
writes nothing, which is the only way to see this step twice — confinement writes approval-off
first and §L-9 makes that one-way, so the real run spends the member whatever follows. But it plans
*without binding*, so a stale or missing `IRONCLAW_MEMBER_TOKEN` passes the dry run and stops the
real one. A clean dry run means the catalogue is reachable, not that the credential is good.

That failure is safe by design: `confine` binds before it reads or writes any setting, and a
mismatch exits 2 having touched nothing. Trust that over the dry run.

**Run both as the account that will own the store.** `adopt` creates the store *before* it validates
the environment, so a run with the wrong env still leaves a `lifecycle.store` owned by whoever ran
it — and a root-owned store breaks the `runuser -u ironworks` step below. Use the same account
there and here.

Confinement writes one settings row per catalogued tool, which is comfortably more than the
runtime's 30 requests per 60 seconds. Leave a minute before asking it for a turn, or the refusal
reads as a defect in whatever ran next.

Enable the cell's attestation timer after that first clean measurement. It binds the supplied
member credential, derives policy from the lifecycle-recorded service, reads the complete
catalogue, and publishes JSON only for a drift-free result:

    sudo systemctl enable --now ironworks-attest@relationship-demo.timer

The timer refreshes every five minutes; evidence expires after ten. A measured drift removes the
current file immediately, while an unavailable runtime leaves the prior file to expire naturally.
The relay unit requires `/run/ironworks-attest/relationship-demo/current.json` for every new
logical turn and records its four fingerprints with that turn. Recovery and fetch of an already
accepted response do not consult current attestation state. So a stopped or disabled timer is a
cell that refuses every new turn within ten minutes while every unit it serves through stays
active: `dev/prove-host.sh` check 12 asserts the timer is enabled and active, and check 13 that the
current file names this cell's slug and member and has not expired.

### Publishing one attended tenant

After adoption and `lifecycle confine` have recorded a certified `member_confined` stage, construct
the registry from lifecycle state rather than copying identifiers into JSON by hand. Run as the
`ironworks-relationship-demo` account: the live registry is mutable service state under its cell,
while the root-only credentials remain in `/etc/ironworks/relay-relationship-demo.env` and are loaded into this attended
process without appearing in arguments.

Install the initial guidance at the tenant-specific canonical path. This direct installation is
only bootstrap: once a registry names the path, all replacements go through `guidance
publish-one`.

    sudo install -d -o ironworks-relationship-demo -g ironworks-relationship-demo -m 0700 \
      /etc/ironworks/relationship-demo
    sudo install -o ironworks-relationship-demo -g ironworks-relationship-demo -m 0600 \
      ./GUIDANCE.initial.md /etc/ironworks/relationship-demo/guidance.md

    sudo systemctl stop ironworks-relay@relationship-demo.service
    sudo bash -c 'set -a; . /etc/ironworks/relay-relationship-demo.env; set +a; \
      exec runuser -u ironworks-relationship-demo --preserve-environment -- \
      /opt/ironworks/target/release/ironworks registry publish-one relationship-demo \
        --store /var/lib/ironworks/lifecycle.store \
        --registry /var/lib/ironworks/relationship-demo/registry.json \
        --guidance /etc/ironworks/relationship-demo/guidance.md \
        --records-credential env:RELATIONSHIP_DEMO_RECORDS \
        --member-credential env:RELATIONSHIP_DEMO_MEMBER'

**A registry published before the `org` field existed is migrated by the same `publish-one`
command — never by hand.** Registry entries now carry the org the records credential resolves to,
and the relay refuses the whole registry when it is absent (§B-4). After re-measuring both
credential identities, `publish-one` recognizes only the exact byte string its former no-`org`
publisher produced for this tenant. It durably publishes the current candidate first, then
compare-and-swaps the current registry identity. The original `staged` history row keeps the
legacy digest as the record of what first reached that stage; appending another would falsely say
the stage was reached twice. A byte-different file, even semantically equivalent JSON, is refused
unchanged. A crash-released lock beside the registry serializes publish and rollback commands and
is held until the lifecycle transaction commits.

Rerun the command above. `MIGRATED … old -> new` means both the file and lifecycle binding are
current. A valid current-schema `.previous` file is preserved. A missing or legacy rollback cannot
be promised as loadable by the strict current reader, so the command deliberately replaces it
with the versioned prior-absence marker and says that rollback now removes the live registry. If a
crash lands after the file rename but before SQLite commits, rerunning the same command completes
the compare-and-swap without republishing or replacing a valid rollback. Confirm with `ironworks
status` and the relay's own `relay: tenant … scope … verified by …` line before re-enabling the
unit.

`publish-one` refuses before writing unless the records handle resolves to the lifecycle org and
the member handle's complete `/session` subject equals the recorded runtime tenant and user. It
parses the guidance and constructed one-tenant registry, writes a same-directory mode-0600
temporary file, syncs the file and directory around every rename, and durably preserves the old
registry as `registry.json.previous` before publishing. It then records `staged`. A crash after the
rename but before SQLite recording is visible and recoverable by rerunning the same command; an
already-identical live file does not overwrite the real rollback artifact.

On the first publication, when no live registry exists, `.previous` is a versioned absence marker
rather than a fake empty registry. Rolling it back durably removes the live file, restoring the
actual prior state instead of leaving a process able to serve a file that did not previously exist.

Rollback is an implemented and tested operation, not a hand edit:

    sudo systemctl stop ironworks-relay@relationship-demo.service
    sudo -u ironworks-relationship-demo /opt/ironworks/target/release/ironworks registry rollback-one \
      --registry /var/lib/ironworks/relationship-demo/registry.json

It restores the preserved registry through the same file-sync/directory-sync/rename
sequence, keeps `registry.json.previous` for another recovery, and does not rewrite historical
lifecycle progress. Restarting the relay remains an attended operator action. The acceptance test
publishes a new registry, loads and routes through it, rolls back, and loads and routes through the
prior registry again.

### Publishing a guidance update

After the registry exists, direct copying to the canonical guidance path is not a supported update
path. Put the candidate beside operator staging state, stop the tenant relay, and let the binary
derive both the service and canonical destination from the matching registry entry:

    sudo install -o ironworks-relationship-demo -g ironworks-relationship-demo -m 0600 \
      ./GUIDANCE.candidate.md /var/lib/ironworks/relationship-demo/GUIDANCE.candidate.md
    sudo systemctl stop ironworks-relay@relationship-demo.service
    sudo -u ironworks-relationship-demo /opt/ironworks/target/release/ironworks guidance publish-one \
      relationship-demo --registry /var/lib/ironworks/relationship-demo/registry.json \
      --source /var/lib/ironworks/relationship-demo/GUIDANCE.candidate.md

The command parses the candidate against that derived slug and service and publishes through a
same-directory owner-only temporary
file with file and directory sync around the atomic rename. It retains the prior bytes as
`GUIDANCE.md.previous` under the registry publication lock. Its output contains only a byte count
and SHA-256 digest; tenant guidance is never echoed. Inspect those values, remove the staging copy,
then restart the relay as a separate attended action.

Rollback is equally explicit and does not consume the prior artifact. It parses the preserved
bytes against the registry-derived slug and service before changing the live file:

    sudo systemctl stop ironworks-relay@relationship-demo.service
    sudo -u ironworks-relationship-demo /opt/ironworks/target/release/ironworks guidance rollback-one \
      relationship-demo --registry /var/lib/ironworks/relationship-demo/registry.json

Run `dev/prove-host.sh` after restart. It derives the live slug, service, and guidance path from the
one-entry registry and requires `compose` to accept it read-only.

## Encrypted state backup and restore

`deploy/backup.example.json` is the closed-world configuration shape. Copy it to
`/etc/ironworks/backup.json`, replace every example with an exact tenant or fleet path, and keep it
root-owned at mode `0600`. The repository and password file are mandatory; the binary carries no
fallback. Roots must be narrow absolute literal directories, and every named file must resolve
below its corresponding root.

Install `restic`, then take an attended first snapshot:

```text
sudo install -d -m 0700 /var/lib/ironworks/backup-staging
sudo /opt/ironworks/target/release/ironworks state snapshot \
  --config /etc/ironworks/backup.json \
  --output /var/lib/ironworks/backup-staging
```

The command takes online SQLite backups of the lifecycle store, each cell journal and proposal
store, copies the backup configuration, bound registry, guidance, credentials and configured pin/artifact manifests into
a root-only staging directory, verifies its digest manifest, uploads that directory through
restic, reports success only from restic's exit status, and removes the plaintext staging tree.
The encrypted manifest records each archived file's canonical
restore destination; credentials and configured fleet files additionally require root ownership.
The complete manifest remains inside the encrypted snapshot beside the files it describes. After
the first upload, run
`check-state-backup.sh` and `rehearse-state-restore.sh` manually. The latter restores the newest
tagged snapshot into a disposable directory and runs `ironworks state verify-snapshot` without
contacting IronClaw, a channel, or a record service. Both exit `2` (FAIL) for a repository that was
reached and failed integrity or authentication, and the rehearsal also for a corrupt, incomplete,
wrongly bound or wrongly permissioned restore. Both exit `3` (BLOCKED) whenever that was never
measured: the repository unreachable or locked by another process, `restic` or `python3` not
installed, or the configuration or its password file unreadable — and, for the rehearsal, no
disposable directory, no verifier at `/opt/ironworks/target/release/ironworks`, or a verifier that
exits without a verdict. `dev/test-state-backup.sh` holds both scripts to that offline.

There is deliberately no timer or automated retention yet. Schedule and pruning become justified
only after an attended upload and restore have both produced PASS; automating an unproved restore
path would turn a missing measurement into unattended state change.

An attended recovery restores beside, never over, live state. Run `verify-snapshot` first; stop
only the affected tenant's relay, Telegram adapter, attestation timer and IronClaw unit; retain the
current tenant directory under a timestamped rollback name; install the verified tenant directory
at `/var/lib/ironworks/<slug>`; and start only those four units. Directory replacement must use a
host facility that provides an atomic exchange (Linux `renameat2(RENAME_EXCHANGE)` or an
equivalent deployment primitive). If that facility is absent, stop: a two-rename approximation is
not this procedure. Keep the displaced directory until a recovered open turn has followed normal
recovery and delivery, then remove it only as a separate attended decision.

## Rehearsing the cursor handoff

**Settled 2026-09-05, on the evidence named below rather than by an operator's recollection.** The
phrase "cursor handoff" reached this file from a work handoff rather than from anything in the tree
— `grep` still finds it in no other document — so the definition here was derived from what this
code can actually get wrong. The five points this section used to leave open are decided at the end
of it, each with the thing that decided it. **A decision made from evidence is not a decision made
by the person who owns the consequence**: reverse any of them and the rest of the section still
holds, but reverse them deliberately rather than by running a rehearsal that quietly assumes
otherwise.

### What is being handed over

Responsibility for answering a room, from the bot the Python fleet runs to the Rust relay, and
back. The two bots are two tokens and therefore **two independent update streams**: stopping one
does not pass its backlog to the other, it strands it. Anything the losing bot had not answered
stays unanswered, and no cursor arithmetic will find it.

### Why the journal is the cursor

**Two axes, kept apart on purpose**, and `ironworks_channel` opens by saying why. *Delivery* is
tenant-scoped and lives with the turn; *acknowledgement* is channel-scoped and lives in the channel
journal. Collapsing them is the defect that crate names after the reference implementation's
`DELIVERED` versus `ACKED` — an answer can be in a client's room while the channel still intends to
redeliver the question that produced it. This code does not have that defect, which is what makes
decision 2 below about the *other* fleet's version of it rather than ours.

**The cursor is derived; the acknowledgement is stored, and they are not the same fact.** §E-17 is
the first half: the safe cursor is the lowest open update across every tenant, computed from the
durable rows on every read, so a cursor advanced past unfinished work has no representation and B6
is absent rather than enforced. ADR 0006 states the second half in the same breath —
*"acknowledgement stays stored, because it is not derivable"*: what the channel was **told** cannot
be implied by any amount of local state, so `cursor_acked` is durable and monotone, and it is the
bound on compaction. An earlier draft of this section blurred them by saying the channel journal
exposes no method that sets an offset. `ironworks_channel::ChannelJournal::acknowledge` sets that
one, and it is the half that is meant to be stored.

The consequence for a handoff is in `ironworks_channel::ChannelJournal::poll_offset`, which has
three cases rather than one: with rows, the derived barrier governs; **with no rows but a recorded
acknowledgement, the stored value carries the position**, because compaction removed the rows the
derivation reads; with neither, zero. ADR 0006 names that middle case as the one failure mode
deriving the cursor introduces, since a fallback to zero there would redeliver the entire history.

So the handoff risk is sharper than "the journal file is the cursor", though that is the right
instinct. The channel journal holds **two** things that answer where to resume — the open rows and
the acknowledged offset — and `ironworks_channel::ChannelJournal::compact` deletes rows below the
acknowledged offset once they never cost a turn. **A compacted journal therefore carries its
position only in `cursor_acked`.** Point a relay at a different `--journal`, or at one whose
metadata did not travel with its rows, and it starts from zero, re-reads every update Telegram
still retains, and answers each of them again — into a real room. That is one wrong path away, it
is silent, and it is the most expensive mistake available at cutover.

Two related ways to reach the same place:

- an attended relay run as `root` writes a root-owned journal that the unit, running as
  `ironworks`, then cannot open;
- a relay pointed at a journal from a *different* tenant's history inherits that tenant's cursor.

### What a rehearsal must show

Every criterion is measured from **inside the room**, not from the relay's own account of itself:

1. every message sent during the window is answered **exactly once**;
2. no message is answered twice, across the handoff instant specifically;
3. after the handoff, a relay restart re-serves nothing already answered — the cursor advanced and
   is durable. **Provoke the compacted case explicitly**: once rows below the acknowledged offset
   are gone, `cursor_acked` is the only thing carrying the position, and a restart that survives
   with rows present proves the easier half;
4. `ironworks status --journal /var/lib/ironworks/relationship-demo/serve.journal` — the path
   `ironworks-relay@relationship-demo.service` actually passes — ends at `0 unresolved turn(s)`;
5. handing back leaves 1-4 true in the other direction.

### Why two

**Both now start from a populated journal.** `/var/lib/ironworks/serve.journal` holds the turn of
2026-09-04 and the cursor has advanced past it, so the clean start this section was first written
around no longer exists. The only way to recover one is to point the relay at a fresh `--journal`,
which is precisely the mistake described above: it re-reads every update Telegram still retains
and answers each of them again, into a real room. **Do not manufacture a clean start.**

The two runs are still worth their days, for a different reason than before. The first measures the
forward direction across a handoff instant. The second starts from what the first leaves behind — a
deeper journal, more advanced cursor, more live threads — and is the closer match to a cutover that
has already been running. What is gone is the easy case: neither run is the gentle one now, and a
plan that treats the first as a warm-up is planning against a condition this host left on
2026-09-04.

### What makes it a rehearsal

Run it in a room that is not the client's, with both bots in that room, and with the tenant's
`channel` in the published registry pointing at it. Nothing else about the path changes, which is
the point: a rehearsal that also swaps the registry, the journal or the units is measuring four
things and will attribute the failure to the wrong one.

`dev/prove-cutover.sh` already proves the registry half of this against a disposable channel —
publish, serve, roll back, serve again, with the answers read out of the channel. What it does not
and cannot prove is the part above, because it has no second bot and no retained backlog.

### The five decisions, and what decided them

1. **The definition stands as written.** Nothing else in the tree names this, so there is no
   competing definition to prefer — only the code's own failure modes, which is what the section
   above is built from. Adopted for that reason and no stronger one.

2. **Quiesce and drain. The stranded backlog is not accepted.** Whatever the losing bot has not
   answered is answerable *only* by the losing bot, for the two-stream reason stated at the top of
   this section. So the switch happens when
   the losing side has nothing outstanding: stop new work reaching the room, let it finish what it
   holds, confirm zero outstanding, then start the winner. Accepting the loss instead means silent
   unanswered messages in a client room, which is the failure the whole exercise exists to catch.
   **The Python side can answer this, measured 2026-09-05.** `bridge.service` runs
   `multi/seam/telegram_bridge.py` from `/opt/ironworks`, and its state module defines an explicit
   per-update ladder — `RECEIVED`, `TURN_STARTED`, `TURN_COMPLETED`, `DELIVERY_STARTED`,
   `DELIVERED`, `ACKED`, beside `IGNORED`, `DELIVERY_RETRY`, `DELIVERY_RECONCILE`,
   `RECOVERY_BLOCKED` and `FAILED_TERMINAL` — over `~/.agency/bridge-threads.db`. So *zero
   outstanding* is a query rather than a judgement: **no row outside `ACKED`, `IGNORED` and
   `FAILED_TERMINAL`.** Two traps in that sentence. `DELIVERED` is **not** terminal — only `ACKED`
   is, so a delivered-but-unacknowledged update is still outstanding and is exactly the row a
   handoff would strand. And the bridge exposes both a `cursor` and a `cursor_acked`; the second is
   the one that means what this criterion means, for the same reason our own cursor is derived from
   the lowest open row rather than the highest seen.

   Its graceful stop is the drain: on `SIGTERM` each active tenant worker finishes its current
   update and queued updates stay `RECEIVED`, holding the global cursor behind them for whatever
   starts next. That is a pause, not a drain — those `RECEIVED` rows are precisely the stranded
   backlog, so stopping the bridge is the *beginning* of the quiesce and the query above is what
   says it finished.

3. **The window: at least 20 messages over at least 10 minutes, at least 3 of them inside the
   minute either side of the switch, and at least one that provokes a long answer.** The criteria
   fix *exactly once, measured from inside the room*; without a size, one message satisfies them
   and proves close to nothing. The long-answer requirement is not padding:
   `0036` measured the demo question landing on the runtime's 30-second wait timeout — eleven 503s
   in seventeen asks — and `0044` measured the recovery that follows reaching an 86-second tail.
   Double-answering is most likely across a refusal and its recovery, so a window too short to
   contain one cannot see the case that matters.

4. **Two runs, two days, booked as equal.** The sizing stands, but not the shape it was costed
   with: both runs now start from a populated journal, so the first is not a warm-up and nothing
   should be planned as learned on day one at day two's expense.

5. **Repoint the existing tenant's `channel`, and accept the two publishes.** This remains the
   rehearsal shape even though the repository now carries templated per-tenant relay and Telegram
   units: the two-cell availability and isolation qualification has not produced PASS, so a second
   client is still not admitted. The publishes themselves are the half already proven, as the
   paragraph above records.

## What remains

**The cell was redeployed onto this architecture on 2026-09-07.** What that moved, and the two
paths that keep the old name, are under "Where it lives" above;
`docs/experiments/0056-the-served-host-was-never-running-the-pinned-model.md` records the run, the
two unit defects it found, and the first turn served under an applied model policy.

`multi-serve` is a Hetzner Ubuntu 24.04 host reached over SSH as an unprivileged operator account;
direct root SSH remains disabled. **The name is not DNS and is not an alias anywhere by default** —
it resolves NXDOMAIN, so every `multi-serve:` command in this file needs a `Host multi-serve` block
naming the address, or it fails in a way that looks like the host is gone. **This repository is
public and therefore names neither the address nor the account.** They live in the operator's own
`~/.ssh/config` and, for `dev/prove-host.sh`, in `PROVE_HOST_ADDRESS` and `PROVE_HOST_USER`, which
that proof has no defaults for and refuses to run without. An SSH endpoint is not a credential, but
it is a targeting aid and git history does not take one back. On 2026-09-02 the operator account
was reached with the SSH key registered in Hetzner. The host has cgroup v2,
and the dedicated `ironclaw-relationship-demo` daemon measured both rootless security and the
`systemd` cgroup driver. `ironclaw 1.4.0`, its pinned worker image, the current RustWorks binaries
and all four units are installed. A Hetzner firewall admits inbound SSH only.

**It serves, and that is still one turn rather than a behavioural claim.**
`ironworks-telegram` and `ironworks-relay` are enabled and active, and one message in the tenant's
room was polled, routed, scope-verified against the record store, composed over 8 records, answered
by the confined member and delivered — `ironworks status` reporting no unresolved turn afterwards.
`ironworks-devstore` is stopped and disabled: this tenant's records come from the Account Service,
and the relay no longer names a store.

**Most of it is now independently verified in this repository.** `dev/prove-host.sh` has reached
this host read-only, with a per-run nonce as its positive control, and scored 7 PASS, 1 FAIL, 0
BLOCKED against the recorded facts: org `relationship-demo` (8 accounts), runtime member
`520144ef-c36a-4606-a885-62e553c56ce7`, the tenant's channel, confinement applied and re-measured
at 53 catalogued, 2 reachable, approval off, and registry digest `a3f91ef2615d440f`. The channel id
is the client's own room and is not written here for the reason the address is not; `PROVE_HOST_*`
carries no expectation for it, and no check reaches it.
The units, the confinement and the digest measured as recorded here.

**Superseded on 2026-09-07, and the digest above is the old one.** Publishing the tenant's `org`
field rewrote the registry — `MIGRATED relationship-demo registry a3f91ef2615d440f ->
5c62e5168c417c8c` — so the recorded value that run scored against is no longer what the host holds.
The live digest is now `5c62e5168c417c8c`, and `dev/prove-host.sh` defaults to it. A later run the
same day, after IronClaw was cut over to the artifact-manifest unit, scored **9 PASS, 1 FAIL, 0
BLOCKED**: the remaining FAIL was this stale digest, in the proof rather than on the host. With that
corrected and an eleventh check added, the host scored **11 PASS, 0 FAIL, 0 BLOCKED** (`0058`).
Checks 12 and 13 — the attestation timer, and the age of the evidence it keeps — came later, and
check 11 widened to every command systemd runs as root and to the deployment tree those commands
run from. Measured on 2026-09-10 with all thirteen, the host scored **13 PASS, 0 FAIL, 0 BLOCKED**
(`0063`).

That eleventh check, `privilege.root_execution_paths`, exists because of what this same day found:
a unit line beginning `ExecStartPre=+` runs as **root** regardless of `User=`, and all three serving
units have one. It walks every path systemd runs as root — a `+` or `!` prefix, or any command of a
unit whose effective `User=` is root or unset, which takes in `ironworks-attest@.service` and
whatever its timer triggers — and every directory above it, failing on any component not owned by
uid 0 or writable by group or other. A unit it cannot read blocks it rather than passing unread.
Whenever one of those paths lies under `/opt/ironworks`, it also scans that whole tree, without
following symlinks or crossing filesystems, for any object not owned by uid 0 or writable by group
or other. That is what covers what a root command runs in turn, which no unit line names — the
`ironworks` binary `refresh-attestation.sh` executes — and it holds the tree to the root ownership
this document already requires of it. It reports the first five offenders and a count; a tree it
cannot read in full blocks it.
**Deploys into `/opt/ironworks` must now run as root** — an `rsync` as the login account will
fail, and that failure is the point.

Two things that run established are worth keeping. The tenant's IronClaw now starts through
`ironworks-ironclaw@.service`, whose `check-ironclaw-artifacts.sh` preflight refuses to start
unless the installed binary hashes to the root-owned manifest and this client's rootless daemon
resolves the manifest's worker image — so the serving binary's provenance is now checked at every
start rather than asserted here. And `model.policy_matches_pin` passed *after* the runtime was
replaced, which is the drift `MODEL_PIN` warns about and the first time that warning has been
tested rather than restated.

**`/opt/rustworks/GUIDANCE.md` is load-bearing and must not be deleted.** The live registry names
this tenant's guidance at that absolute path, and by the rule above the path is fixed at
publication: moving it means resetting the lifecycle store, which spends another member on a
one-way `confine` (§L-9). The rest of `/opt/rustworks` is gone — the directory is pruned to that
file, all four `rustworks-*` units were removed, and `ironworks-*` units serve in their place
(`0058`) — but that one file stays until there is a reason worth a member.

**The failure was the proof's own, and this file caused it.** It expected the member id in the
truncated form this paragraph used to carry — `520144ef` — while the runtime reports the full
UUID, so a correct host scored a difference that was not there. The id is written out in full
above for that reason: **never derive a check from a shortened value.** The account count and the
channel remain operator-reported; no check reaches either. What the host proves it can do it
proves on the host.

Three things went wrong on the way and are worth carrying forward. The first `adopt` resolved
`coordination-eval` — a devstore org — rather than the Account Service's, and `publish-one` refused
the mismatch rather than publishing a valid registry against the wrong records scope; the store was
reset and re-adopted. That is the check working exactly as designed and is the first time it has
fired outside a test. Separately one member was orphaned and deleted, for the reason the truncation
warning above now documents.

**The third is the sharpest, because the file that carried it promised the opposite.**
`GUIDANCE.md`'s operator notes open by saying everything in an HTML comment "is stripped before
composition and never reaches a model". `ironworks_domain::composition`'s comment stripper closes a
block at the **first** closing delimiter it finds, and the notes illustrated the marker's optional
third field by writing a complete example marker — whose own delimiter ended the block about forty
lines early. Everything after it was model-visible text, composed under the organization-guidance
heading: the channel `scrap/personas/COORDINATOR.md` tells the agent governs its behaviour, not the
data channel it is told to distrust. Measured on the host with its own binary, 2026-09-05:
`guidance 2676 bytes (ea05f11f902bed58)` against `1401` for the corrected file — **1,270 bytes of
operator instructions in every prompt**, the imperative "Add, as headed sections in the visible
body" among them, pointing at the sales-shaped sections that persona exists to refuse.
The first containment reported that count from `compose` as a warning. The control now lives in
`Guidance::parse`: this guidance format has no use for a stray HTML terminator, and treating the
hazardous shape as valid was what forced every activation path to remember a second gate.

It reached real turns rather than merely being able to. `/var/lib/ironworks/registry.json` names
`/opt/ironworks/GUIDANCE.md` for this tenant, that file was written at 21:18:08 UTC on 2026-09-04,
and `ironworks-relay` entered active at 23:00:08 UTC, so every cycle since composed with it — the
turn this section records included, under alloy `4e68bf09c447a410` rather than the
`d6b30a462440e6ae` the corrected file produces. The same trap is already documented on
`ironworks_domain::composition`'s fact-field parser, for a declared key containing that delimiter.
The template did it to itself, by example.

**Corrected and installed 2026-09-05 01:18:46 UTC**, and the fix at that time was a file copy into
`/opt/ironworks` and nothing else — no restart, no unit change, no registry publish. It composed
`guidance 1401 bytes (347488a6f7cbf3bb)` under alloy `d6b30a462440e6ae`, matching the repository
copy part for part. `dev/relay-loop.sh` runs a fresh process per cycle, so guidance is re-read from
disk every cycle: the first cycle on the corrected file ran two seconds after it landed, under a
new pid, and the loop polled cleanly through it.

That direct-copy procedure is historical evidence, not current instruction. It was superseded by
the `guidance publish-one` gate above, which refuses the two hazards before replacing live bytes and
keeps a durable rollback artifact.

**The served guidance moved again at 02:08:35 UTC the same day, deliberately and not as part of
that fix.** Settling the guidance shape (recorded in `fixtures/tenants/guidance.template.md`)
corrected two lines of the model-visible body that contradicted the persona composed
beside them — a missing *budget* offered as the example of an absence worth reporting,
where `scrap/personas/COORDINATOR.md` says an empty legacy sales field is "not a gap,
not a finding, and not something to report"; and "opportunities" where this service has
commitments. The host now composes `guidance 1604 bytes (3cf2240fd1fe92d5)` under alloy
`82655c7d3e1f201f`. **An answer-quality result is bound to an alloy digest**, so anything
measured against `d6b30a462440e6ae` or the leaked `4e68bf09c447a410` was scored on a composition
this host no longer runs.

**The confinement count is the right one, and it identifies the caller.** `0039` measured the
catalogue along two independent axes — `nearai.web_search` for the caller, `builtin.shell` for the
profile — giving 53 for the operator bearer and 52 for a minted member on `production`, and **54
and 53** respectively on `hosted-single-tenant-volume-sandboxed`, which is the profile this host
runs. Its Finding 2 reached `53 catalogued, 2 reachable, approval off` against a member minted on
that profile, which is exactly what was observed here. Had the WebUI env bearer been confined by
mistake the count would have read 54, so this number is positive evidence that the minted member
was the caller — not merely an absence of alarm.

`crates/lifecycle/examples/seed.rs` remains unsuitable for any of this: it would record
`acme-sales` and a fixture channel as production evidence.

**Do not confuse this with the existing Python stack on the same host.** Its watchdog watches
IronClaw on `127.0.0.1:3020` and the Account Service on `127.0.0.1:8443`; both ports still answer
for that fleet. The Rust control plane uses `127.0.0.1:3997`. As of 2026-09-02 no Rust relay was
enabled and that work neither cut over nor interrupted the Python service. The relay is enabled
now, and nothing on this host arbitrates between the two fleets: two bots are two tokens and
therefore two independent update streams, which is what "Rehearsing the cursor handoff" above is
about.

**Confining the member is a prerequisite, not a later hardening step.** A served deployment whose
member is unconfined can call `builtin.http` and `outbound_deliver` — see `lifecycle confine` and
`docs/experiments/0012-what-confinement-reached.md`. It was applied to
`520144ef-c36a-4606-a885-62e553c56ce7` before this host
answered anything, and the turn it did answer was answered by the confined member. The same holds
for every member minted after it: confine before pointing it at records that matter.
