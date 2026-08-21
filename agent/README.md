# agent/ — the fleet's agent data (what the harness loads)

Pure agent data, no code — the personas the fleet's ironclaw instances run on.

- `identity/` — the persona files: control-plane (`SOUL.md`), product
  (`ACCOUNT_INTELLIGENCE.md`, `ANALYST.md` — channel-injected, never baked),
  frozen-experiment (`MULTIMEDIATOR.md`), the `MULTI.template.md` default, and the shared
  `_operational-tail.md` / `_safety-tail.md`. The exhaustive inventory of record, and the
  only place their count is stated, is `docs/ARCHITECTURE.md` § Personas — which also lists
  the personas that exist on disk but are deliberately not published.

  **Upstream attribution:** `_operational-tail.md` is preserved verbatim from the seeded
  default prompt of [nearai/ironclaw](https://github.com/nearai/ironclaw), and
  `_safety-tail.md` is derived from that prompt's Safety section. Both are upstream's work
  under its MIT OR Apache-2.0 license, kept intact so the harness guardrails ride along
  with whatever persona sits above. (The attribution lives here rather than inside those
  files because they are composed verbatim into live system prompts.)
  `provision-agent.sh` **composes** a chosen `PERSONA_SOURCE` + the tail and installs the
  result as that instance's global system prompt
  (`…/system/prompts/default-system.md`) — the repo file is the source of truth, the
  running instance holds a composed copy. See `identity/` and `deploy/provision-agent.sh`.
- `config/` — agent-level config notes. Channel setup and secrets are configured through
  ironclaw's **Admin → Configuration** / deployment env vars, not files here. See
  `config/README.md`.

Runtime **state** (users, bindings, memory, threads) is NOT here — each instance keeps
its own on its `ironclaw-<slug>-data` volume (`/data/ironclaw-reborn`). This repo is the
*source of what the fleet is*; the volumes are where instances run.
