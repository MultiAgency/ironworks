# agent/ — agent data the harness loads

Pure agent data, no code. Two composition paths read from here, and in both the repo file is the
source of truth while a running instance holds only a composed copy.

- **The product path** composes per turn in the seam: a service definition selects persona parts
  and `multi/seam/persona.py` appends `_safety-tail.md`. Nothing is baked into the instance.
- **The fleet path** composes at install time: `deploy/provision-agent.sh` joins a chosen
  `PERSONA_SOURCE` with `_operational-tail.md` and writes the result as that instance's global
  system prompt.

## `identity/`

- `ANALYST.md` — the client-generic analyst; external tenants get this plus their own slug-bound
  guidance, never an internal composition.
- `RELATIONSHIP_INTELLIGENCE.md` — MultiAgency's internal composition: relationship state derived
  from accounts, contacts, and dated activities.
- `SOUL.md` — Multron, the internal contributors' agent. A live prompt: an edit here is not live
  until `deploy/update-persona.sh` re-installs it and restarts the container, and until then the
  repo file and the running agent disagree. No code names this path — nothing composes it on a
  schedule and no test reads it — so the install and re-install commands are written into the
  file's own leading comment, which `deploy/lib/compose-persona` strips before anything reaches
  the model.
- `MULTIMEDIATOR.md` — the vidgen contributors' agent. Also a live prompt, wired the same way.
- `MULTI.template.md` — the parametrized per-group default (`{{AGENT_NAME}}` / `{{PURPOSE}}`)
  that `provision-agent.sh` stamps out for a fleet group agent.
- `_operational-tail.md` — Response Style / Computation / Tool Continuation / Files / Safety,
  appended to every *baked* persona.
- `_safety-tail.md` — the product-path counterpart, appended to **every** channel-injected
  composition so a tenant turn can never run without it.

One tail per composition path and neither is optional. `deploy/lib/test_tail_parity.py` pins that
a Safety rule added to one is added to the other.

Personas written for a single partner are not published; they sit gitignored in the operator's
tree and `provision-agent.sh` builds from a local copy via `PERSONA_SOURCE`, so nothing is lost
operationally.

**Upstream attribution:** `_operational-tail.md` is preserved verbatim from the seeded default
prompt of [nearai/ironclaw](https://github.com/nearai/ironclaw), and `_safety-tail.md` is derived
from that prompt's Safety section. Both are upstream's work under its MIT OR Apache-2.0 license,
kept intact so the harness guardrails ride along with whatever persona sits above. The
attribution lives here rather than inside those files because they are composed verbatim into
live system prompts.

## `config/`

Agent-level configuration notes. Thin by design: on the stock binary most configuration is
entered through the runtime's own admin surface or passed as deployment environment variables,
not stored here. See `config/README.md`.

## Not here

Runtime state — users, bindings, memory, threads — lives on each instance's own data volume.
Secrets live under `~/.agency/`. This directory is the source of what an agent *is*; the volumes
are where instances run. Where each component sits in the system:
`docs/ARCHITECTURE.md` § Components.
