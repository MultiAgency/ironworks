# 0055 — The served host predates the registry schema it is read by

**2026-09-07.** Read-only inspection of `multi-serve` over SSH, taken while planning a path rename
that turned out not to be the operation needed. Four commands, no writes, no restarts. Recorded
because the gap it measures is invisible from the repository: every document here describes a
deployment shape the host has never run.

## What is deployed

| | host | this tree |
|---|---|---|
| relay unit | `rustworks-relay.service`, single-tenant | `ironworks-relay@.service`, templated per cell |
| telegram unit | `rustworks-telegram.service`, single-tenant | `ironworks-telegram@.service` |
| runtime unit | `rustworks-ironclaw@relationship-demo.service` — already templated | same shape |
| service account | one, `rustworks` uid 999 | `ironworks` plus per-tenant `ironworks-<slug>` |
| state | flat `/var/lib/rustworks/{registry.json,lifecycle.store,serve.journal}` | `/var/lib/ironworks/<slug>/…` |
| attestation | **none** — no attest unit, no timer, no `/run/…-attest` | `ironworks-attest@.timer`, and the relay passes `--confinement-attestation` |
| compose | `/etc/rustworks/serve.env` names a `rustworks-serve` stack | that Compose file is deleted here |

The relay's `ExecStart` is `dev/relay-loop.sh --registry /var/lib/rustworks/registry.json --journal
/var/lib/rustworks/serve.journal --channel http://127.0.0.1:3994`, running as `User=rustworks` with
`ReadWritePaths=/var/lib/rustworks`. So the deployed unit is coherent with the deployed layout; it
is simply an older generation of both.

## The finding

**The live registry has no `org` field**, and the current parser refuses a registry without one.
The deployed entry is five keys — channel, guidance, records credential, runtime credential,
service, slug — and `crates/domain/src/registry.rs::WireEntry` requires `org` with no default,
because absent would mean *serve whatever the credential turns out to authorise*. §B-4 refuses the
registry as a whole rather than the row.

So the current binary would not serve this host's registry at all. **The distance is a schema
generation, not a set of names**, and no amount of renaming closes it.

Two consequences worth carrying:

- **`publish-one` already migrates this exact case**, and no member is spent. It recognizes the
  byte string the former no-`org` publisher produced, publishes the current candidate durably, then
  compare-and-swaps the lifecycle binding, reporting `MIGRATED … old -> new`. `deploy/README.md`
  carries the procedure. An earlier reading of this pass claimed the migration would cost a
  lifecycle reset and a one-way `confine`; that applies to *changing a published guidance path*,
  not to this.
- **The legacy rollback marker is on disk and is replaced by that same migration.**
  `registry.json.previous` is 35 bytes — exactly `rustworks-registry-prior-absent-v1\n`. The
  sentinels were renamed here on 2026-09-07, so the old bytes would otherwise read as a real prior
  registry; the migration deliberately replaces a legacy rollback rather than trusting it, which
  absorbs that rename on this host without a separate cleanup.

## What it cost to find, and what it corrects

The registry names `"guidance": "/opt/rustworks/GUIDANCE.md"`. `deploy/README.md`'s current
procedure installs guidance at `/etc/<slug>/guidance.md` and its own incident record still names the
`/opt` path, so the document describes two layouts and the host runs the older one. A migration plan
written from the document named the wrong directory as the one needing a symlink — twice, in both
directions — and only reading the live registry settled it.

**The general rule this pass earned: a deployed binding is read out of the deployment, never out of
the procedure that once created it.** The procedure records what a fresh install would do; the
registry records what this host was actually told.

## What this establishes and what it does not

It establishes the deployed shape at one moment, read-only, from four commands. It does **not**
establish that the host is healthy, that its member credential still authenticates, or that
`publish-one` would succeed here — that command re-measures both credential identities before
writing, and neither was exercised. Nothing was restarted and nothing was written.

The operation this inspection was planning — renaming `/opt/rustworks` and friends in place — is
recorded here as **abandoned**, on the evidence above: the host wants the documented install
procedure, which produces the current names as a side effect, not a sideways rename of a shape the
tree no longer describes.
