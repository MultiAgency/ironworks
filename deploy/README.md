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

**The relay no longer names it.** `rustworks-relay.service` used to carry
`Wants=rustworks-devstore.service`, which asserted a fact the unit cannot know: a tenant's store is
a credential and endpoint in the registry the unit already passes as `--registry`. On a host serving
from the Account Service that dependency started a synthetic store nothing read, and neither
operator-side switch turned it off — `disable` does not hold because `Wants=` restarts it, and
`mask` refuses while the unit file occupies the path the mask symlink needs. Both measured. The
dependency is gone; start `devstore` deliberately when a deployment actually serves the demo book.

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
    sudo install -d -o "$USER" -g "$USER" -m 0755 /opt/rustworks
    sudo install -d -o root -g root -m 0755 /opt/ironclaw/bin
    sudo install -d -m 0700 /etc/rustworks
    sudo useradd --system --no-create-home --shell /usr/sbin/nologin rustworks
    sudo install -d -o rustworks -g rustworks -m 0750 /var/lib/rustworks
    sudo install -d -o root -g root -m 0711 /var/lib/ironclaw

    # source, without build output, git, generated pages, secrets, the cargo cache, or any
    # tenant guidance a published registry points at
    rsync -az --delete --exclude /target/ --exclude /.git/ --exclude /site/ --exclude /local/ \
          --exclude /.cargo-home/ --exclude /GUIDANCE.md --exclude __pycache__/ \
          ./ multi-serve:/opt/rustworks/

**Every exclusion is rooted, and two of them protect files that exist only on the host.**
`/.cargo-home/` is the build cache, and losing it turns a one-minute rebuild into a cold one.
`/GUIDANCE.md` is worse: **a published registry names its tenant's guidance by absolute path**, and
a `publish-one` that has reached `Staged` will not republish against a different one — it compares
the live registry with the candidate and refuses any difference. So the guidance path is fixed at
publication and cannot be moved without resetting the lifecycle store, which means spending another
member on a one-way `confine`. **Choose that path before publishing, and never let a sync delete
what it names.** To check a sync before running it, use `--dry-run -i` and read the `*deleting`
lines; grepping for `^deleting` matches nothing and reads as safe when it is not.

    # binaries, built in a container so no toolchain lands on the host
    docker run --rm -u "$(id -u):$(id -g)" -v /opt/rustworks:/src -w /src \
      -e CARGO_HOME=/src/.cargo-home rust:1.97-bookworm \
      cargo build --release -p ironworks-cli -p ironworks-telegram -p ironworks-devstore

    sudo install -m 0644 /opt/rustworks/deploy/rustworks-*.service /etc/systemd/system/
    sudo install -d -o root -g root -m 0755 /etc/systemd/user
    sudo install -m 0644 /opt/rustworks/deploy/user/rustworks-docker.service /etc/systemd/user/
    sudo systemctl daemon-reload

**These three lines belong to every re-sync, not just the first install.** `rsync` updates
`/opt/rustworks/deploy/`; it does not touch `/etc/systemd/system/`, so systemd keeps running
whatever units it was given last. A drifted unit fails in the least obvious way available — the one
that cost a serving loop here pointed `--registry` at the wrong directory and omitted its
`EnvironmentFile`, so the relay started cleanly and then could not read the registry it had just
been handed. Check before starting anything:

    for u in /opt/rustworks/deploy/rustworks-*.service; do
      diff -q "/etc/systemd/system/$(basename "$u")" "$u" || echo "DRIFTED: $(basename "$u")"
    done

The two serving environment files are separate custody boundaries. Both are read by systemd as
root before it changes to `User=rustworks`, so neither has to be readable by the service account.

**`install /dev/null` truncates, so run it once per file and never again.** These files hold a
runtime member's `api_token`, which this pin issues exactly once and cannot reissue (`0043`); a
second `install` over a populated file destroys a credential replaceable only by minting a new
member and deleting the old one. It has cost one. To add a value to a file that already exists,
`sudoedit` it; to check first, `sudo test -s <path> && echo populated`.

    sudo install -o root -g root -m 0600 /dev/null /etc/rustworks/telegram.env
    sudoedit /etc/rustworks/telegram.env
    # exactly: TELEGRAM_BOT_TOKEN=<the BotFather token>

    sudo install -o root -g root -m 0600 /dev/null /etc/rustworks/relay.env
    sudoedit /etc/rustworks/relay.env
    # shared endpoints, plus every env:NAME handle the one-tenant registry names:
    # IRONWORKS_BASE=http://127.0.0.1:3997
    # IRONCLAW_API=http://127.0.0.1:3997               # lifecycle commands read this name
    # IRONWORKS_RECORDS=<the store serving this tenant>
    # <SLUG>_RECORDS=<records bearer>
    # <SLUG>_MEMBER=<IronClaw member bearer>

**`IRONWORKS_RECORDS` names whichever store serves this tenant**, and the choice is a real one. A
tenant with real records uses the Account Service and needs a provisioned org and a bearer. The
demo book this repository ships uses `rustworks-devstore` on `3996`, whose identity is
`dev-token-coordination` resolving to org `coordination-eval` — written in the committed unit, so
that path carries no records secret at all. Point a tenant with real records at devstore and you
have done the one thing that unit's own comment forbids.

**Whichever it is must be running before `adopt`**, which resolves the handle through
`IRONWORKS_RECORDS` and records the org it observes. A store that is installed but inactive makes
`adopt` refuse, correctly and confusingly, since nothing about the instance is wrong. Getting this
wrong late is worse than early: `publish-one` refuses when the recorded org and the handle's org
disagree, and the fix is a fresh store, because `adopt` will not overwrite a slug.

Do not put `TELEGRAM_BOT_TOKEN` in `relay.env`; only the adapter calls Telegram. The files may be
created before or after their unit files are installed, but each must exist before its own unit is
started. Installing them does not start either service.

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

    ironworks lifecycle adopt CLIENT --store /var/lib/rustworks/lifecycle.store \
      --service relationship-intelligence --channel <channel id> \
      --records-credential env:CLIENT_RECORDS --member-credential env:CLIENT_MEMBER

    ironworks lifecycle confine CLIENT --store /var/lib/rustworks/lifecycle.store --dry-run
    ironworks lifecycle confine CLIENT --store /var/lib/rustworks/lifecycle.store

**`--dry-run` shows the plan; it does not rehearse the run.** It opens the store read-only and
writes nothing, which is the only way to see this step twice — confinement writes approval-off
first and §L-9 makes that one-way, so the real run spends the member whatever follows. But it plans
*without binding*, so a stale or missing `IRONCLAW_MEMBER_TOKEN` passes the dry run and stops the
real one. A clean dry run means the catalogue is reachable, not that the credential is good.

That failure is safe by design: `confine` binds before it reads or writes any setting, and a
mismatch exits 2 having touched nothing. Trust that over the dry run.

**Run both as the account that will own the store.** `adopt` creates the store *before* it validates
the environment, so a run with the wrong env still leaves a `lifecycle.store` owned by whoever ran
it — and a root-owned store breaks the `runuser -u rustworks` step below. Use the same account
there and here.

Confinement writes one settings row per catalogued tool, which is comfortably more than the
runtime's 30 requests per 60 seconds. Leave a minute before asking it for a turn, or the refusal
reads as a defect in whatever ran next.

### Publishing one attended tenant

After adoption and `lifecycle confine` have recorded a certified `member_confined` stage, construct
the registry from lifecycle state rather than copying identifiers into JSON by hand. Run as the
`rustworks` account: the live registry is mutable service state under `/var/lib/rustworks`, while
the root-only credentials remain in `/etc/rustworks/relay.env` and are loaded into this attended
process without appearing in arguments.

    sudo systemctl stop rustworks-relay.service
    sudo bash -c 'set -a; . /etc/rustworks/relay.env; set +a; \
      exec runuser -u rustworks --preserve-environment -- \
      /opt/rustworks/target/release/ironworks registry publish-one relationship-demo \
        --store /var/lib/rustworks/lifecycle.store \
        --registry /var/lib/rustworks/registry.json \
        --guidance /opt/rustworks/GUIDANCE.md \
        --records-credential env:RELATIONSHIP_DEMO_RECORDS \
        --member-credential env:RELATIONSHIP_DEMO_MEMBER'

`publish-one` refuses before writing unless the records handle resolves to the lifecycle org and
the member handle's complete `/session` subject equals the recorded runtime tenant and user. It
validates the guidance and constructed one-tenant registry, writes a same-directory mode-0600
temporary file, syncs the file and directory around every rename, and durably preserves the old
registry as `registry.json.previous` before publishing. It then records `staged`. A crash after the
rename but before SQLite recording is visible and recoverable by rerunning the same command; an
already-identical live file does not overwrite the real rollback artifact.

On the first publication, when no live registry exists, `.previous` is a versioned absence marker
rather than a fake empty registry. Rolling it back durably removes the live file, restoring the
actual prior state instead of leaving a process able to serve a file that did not previously exist.

Rollback is an implemented and tested operation, not a hand edit:

    sudo systemctl stop rustworks-relay.service
    sudo -u rustworks /opt/rustworks/target/release/ironworks registry rollback-one \
      --registry /var/lib/rustworks/registry.json

It validates the preserved registry, restores it through the same file-sync/directory-sync/rename
sequence, keeps `registry.json.previous` for another recovery, and does not rewrite historical
lifecycle progress. Restarting the relay remains an attended operator action. The acceptance test
publishes a new registry, loads and routes through it, rolls back, and loads and routes through the
prior registry again.

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
  `rustworks`, then cannot open;
- a relay pointed at a journal from a *different* tenant's history inherits that tenant's cursor.

### What a rehearsal must show

Every criterion is measured from **inside the room**, not from the relay's own account of itself:

1. every message sent during the window is answered **exactly once**;
2. no message is answered twice, across the handoff instant specifically;
3. after the handoff, a relay restart re-serves nothing already answered — the cursor advanced and
   is durable. **Provoke the compacted case explicitly**: once rows below the acknowledged offset
   are gone, `cursor_acked` is the only thing carrying the position, and a restart that survives
   with rows present proves the easier half;
4. `ironworks status --journal /var/lib/rustworks/serve.journal` — the path
   `rustworks-relay.service` actually passes — ends at `0 unresolved turn(s)`;
5. handing back leaves 1-4 true in the other direction.

### Why two

**Both now start from a populated journal.** `/var/lib/rustworks/serve.journal` holds the turn of
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

5. **Repoint the existing tenant's `channel`, and accept the two publishes.** A second published
   tenant is the cheaper-looking option and it is closed to us: `docs/PRD.md` §23 step 1 keeps the
   constraint that **the cursor is shared across every admitted tenant, so one client relay is safe
   and two are not**, and step 5 — per-tenant journal and loop — is not done. Publishing a second
   tenant to avoid two publishes would break the ordering the PRD sets in order to protect a
   rehearsal. The publishes themselves are the half already proven, as the paragraph above records.

## What remains

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
`rustworks-telegram` and `rustworks-relay` are enabled and active, and one message in the tenant's
room was polled, routed, scope-verified against the record store, composed over 8 records, answered
by the confined member and delivered — `ironworks status` reporting no unresolved turn afterwards.
`rustworks-devstore` is stopped and disabled: this tenant's records come from the Account Service,
and the relay no longer names a store.

**Most of it is now independently verified in this repository.** `dev/prove-host.sh` has reached
this host read-only, with a per-run nonce as its positive control, and scored 7 PASS, 1 FAIL, 0
BLOCKED against the recorded facts: org `relationship-demo` (8 accounts), runtime member
`520144ef-c36a-4606-a885-62e553c56ce7`, the tenant's channel, confinement applied and re-measured
at 53 catalogued, 2 reachable, approval off, and registry digest `a3f91ef2615d440f`. The channel id
is the client's own room and is not written here for the reason the address is not; `PROVE_HOST_*`
carries no expectation for it, and no check reaches it.
The units, the confinement and the digest measured as recorded here.

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
`ironworks compose` now reports that count itself, as a warning rather than a refusal: guidance is
a tenant's own prose and may legitimately hold the sequence, so refusing it would stop serving a
tenant over punctuation.

It reached real turns rather than merely being able to. `/var/lib/rustworks/registry.json` names
`/opt/rustworks/GUIDANCE.md` for this tenant, that file was written at 21:18:08 UTC on 2026-09-04,
and `rustworks-relay` entered active at 23:00:08 UTC, so every cycle since composed with it — the
turn this section records included, under alloy `4e68bf09c447a410` rather than the
`d6b30a462440e6ae` the corrected file produces. The same trap is already documented on
`ironworks_domain::composition`'s fact-field parser, for a declared key containing that delimiter.
The template did it to itself, by example.

**Corrected and installed 2026-09-05 01:18:46 UTC**, and the fix was a file copy into
`/opt/rustworks` and nothing else — no restart, no unit change, no registry publish. It composed
`guidance 1401 bytes (347488a6f7cbf3bb)` under alloy `d6b30a462440e6ae`, matching the repository
copy part for part. `dev/relay-loop.sh` runs a fresh process per cycle, so guidance is re-read from
disk every cycle: the first cycle on the corrected file ran two seconds after it landed, under a
new pid, and the loop polled cleanly through it.

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
