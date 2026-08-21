# deploy/ — stand up the fleet on the OFFICIAL ironclaw binary

Every MultiAgency agent runs **unmodified upstream ironclaw**, built from a pinned rev —
never a locally-patched fork. Pulling updates = bump the pin, rebuild the image,
re-provision. There is nothing of ironworks' inside ironclaw to conflict.

## 1) Build the image (once per upstream rev)

The image is built from **ironclaw's own `Dockerfile`** at a pinned rev — ironworks does
not maintain its own Dockerfile (that would duplicate and drift from upstream). The pin
is the one-line `IRONCLAW_PIN` file at this repo's root (full commit SHA, then a `#`
comment); it is the version of record — the rev images are built from. A freshly edited
pin describes the *next* image, not the running one, until `UPGRADE.md` completes.

```bash
cd /path/to/ironclaw           # an upstream checkout tracking nearai/ironclaw main
git fetch origin && git checkout "$(cut -d' ' -f1 /path/to/ironworks/IRONCLAW_PIN)"
docker build -f Dockerfile -t ironclaw:main .
```

The produced binary is `ironclaw` (entrypoint `ironclaw-reborn-entrypoint`); each
instance's home resolves to `/data/ironclaw-reborn` on its own mounted volume.

## 2) Provision an agent (`provision-agent.sh`)

One command stands up a fully-isolated agent instance — its own container, volume, bot,
persona, hostname, and Telegram wiring:

```bash
TELEGRAM_BOT_TOKEN="$(cat ~/.agency/<slug>.token)" \
TELEGRAM_BOT_USERNAME=<botusername> \
PROVISION_FROM_ENV=1 \
PERSONA_SOURCE=agent/identity/<PERSONA>.md AGENT_NAME=<Name> PURPOSE="<one line>" \
  ./provision-agent.sh "<slug>"
```

`PROVISION_FROM_ENV=1` is REQUIRED whenever you pass `PERSONA_SOURCE` (or `CONTAINER`,
`AGENT_HOSTNAME`, `IRONCLAW_REBORN_WEBUI_TOKEN`, `TELEGRAM_WEBHOOK_SECRET`). Without it
`provision-agent.sh` captures the inherited value and refuses the run — the guard exists because
silently inheriting a previous agent's persona file cross-wires the new agent invisibly. Omitting
it here made this example exit 1 before creating anything; it cost a real detour.

- `PERSONA_SOURCE` defaults to `agent/identity/MULTI.template.md` (the client-onboarding
  workhorse). The control plane is one agent today: Multron, `SOUL.md`.
- `NEARAI_API_KEY` is always passed explicitly — never copied from a running instance
  (which key a new agent got was an accident of container ordering).
- It writes per-agent secrets to `~/.agency/agents/<slug>.env` (chmod 600) and never
  echoes them. One manual step remains (printed at the end): add the bot to the group
  **as an admin**. (v1.3.0 is a device-link build — #7464 switched Telegram to device-link, so
  there's no per-member deep-link pairing to mint; see `UPGRADE.md`.)

See `docs/ARCHITECTURE.md` for the topology and `docs/agent-spec.md` for how a new agent
flows from spec → persona → provision.

### Master-key handling (fleet agents)

Each agent auto-generates its secret-store master key on first boot and persists it to
`/data/ironclaw-reborn/hosted-single-tenant-volume/.reborn-local-dev-secrets-master-key`
on the agent's own volume. If you ever set `SECRETS_MASTER_KEY` on an existing agent,
**read the current dotfile key from the volume first and set the env var to exactly that
value**: a different env value is silently ignored (a valid cached dotfile wins), and
deleting the dotfile while a different env key is set orphans the existing store — the
env key is also re-persisted to a new dotfile on the next boot, so the key cannot be
kept off the volume on this profile.

## 3) Health-check (`doctor.sh`)

```bash
./deploy/doctor.sh                  # whole fleet
./deploy/doctor.sh <slug> --deep    # one agent + assert the pinned Telegram auth model
```

End-to-end per agent: container, API, persona, public reachability, signed delivery,
ingress, bot token, webhook registration. Reads `~/.agency/agents/*.env`.

## Pull upstream updates

Follow `deploy/UPGRADE.md` — the pin-bump runbook: pick the new rev (prefer release
tags), edit `IRONCLAW_PIN`, rebuild (§1), swap instances in the documented order, re-run
the proofs. Each agent's data volume is reused, so no re-onboarding or re-pairing.

## Subdirectory status

Live:

- `secretary/` — the front-desk secretary (Cloudflare Worker; see its READMEs).
- `account-intel/` — the Account Service data layer (`data/`) and the demo/regression
  books (`fixtures/`, `data/candidates/`). Reached only by the seam, never by the model.

Frozen (no pilot value yet):

- `vidgen/` — derived toolchain image (`ironclaw:vidgen`) for the Multimediator
  experiment; zero active investment. Kept because `UPGRADE.md` depends on it: a derived
  image cannot be migrated to the bare rev tag and needs its own rebuild.

Removed rather than frozen (both from experiments this repo already calls retired):

- `broker/` — confinement primitives from the cross-agent broker experiment. Its
  deny-list (`tools-deny-specialist.txt`) is the shape `multi/provision/confine-member.sh`
  argues against by name: a deny-list lets a newly-added egress tool through until someone
  remembers to list it. Shipping it as "reusable" next to the allowlist that supersedes it
  invited the wrong one to be copied. The live confinement is `confine-member.sh` (apply),
  `confine-existing.sh` (back-fill), and `multi/verify/test_egress_closed.py` (prove).
- `hq/` — confidentiality demo for the MultiAgencyHQ onboarding experiment. It drove live
  state (`~/.agency/hq/hq.env`) that never had a provisioning path here, so no reader could
  run it; the property it demonstrated on stage is proven reproducibly and in-tree by
  `multi/verify/test_adversarial_cross_org.py`.

## Accepted risk: tokens visible in the local process table

These scripts expose credentials in the process table (`ps`) for the duration of each
call — and not only where the Telegram API forces the token into the URL: every
`curl -H "Authorization: Bearer $TOK"` / `-H "X-Service-Token: …"` and every python
heredoc taking a token as a positional argument is equally argv-visible. This is
**accepted on single-operator hosts** (the laptop and the single-operator VM), where
anyone who can read the process table already owns the box and the tokens.
**Multi-user hosts are out of scope** — do not run this tooling on one. New scripts
should prefer passing secrets via stdin where cheap (`curl --config /dev/stdin` or
`-H @-`); nobody should refactor existing scripts solely for this.

## What ironworks adds

Agent personas (`../agent/identity`), agent config (`../agent/config`), and this tooling
(`provision-agent.sh`, `doctor.sh`). All data and scripts — no harness code.
Runtime state stays out of this repo (each instance's `ironclaw-<slug>-data` volume);
secrets stay out of this repo (`~/.agency/`).
