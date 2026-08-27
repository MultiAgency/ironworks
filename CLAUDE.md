# Claude Code repository instructions

## Core rule

Run official, unmodified IronClaw at `IRONCLAW_PIN`. IronWorks is configuration, data, and
tooling around it. A runtime change belongs upstream; current constraints are in `SECURITY.md`.

## Runtime Markdown

`skills/*/SKILL.md`, personas, tenant/eval guidance, and fixture interaction notes are product
inputs. Editing their body changes model behavior. Skill frontmatter is stripped during
composition, but everything below it is supplied to the model. Preserve composition ordering and
behavior unless the task explicitly changes it.

## Dependency direction

The serving path in `multi/seam/` must not import `deploy/`. Operator tooling may import product
modules, and operator-side proofs may use `deploy/lib`. Put serving-path code in `multi/seam/`.

Use `multi/seam/pins.py` for Python pin parsing and `deploy/lib/fleet.sh` for shell provisioning.
Never add a fallback literal for `MODEL_PIN` or `IRONCLAW_PIN`.

`IRONCLAW_API` and `TELEGRAM_BOT_TOKEN` resolve when used, not at import. Tests that drive the seam
should assign the environment they need directly.

## Working tree

The parent repository and `admin/` are separate repositories and may both contain unrelated
uncommitted work. Inspect status, preserve changes you did not create, and never reset, restore,
stash, or overwrite them.

## Gates and authorities

Use the exact local gate commands in `CONTRIBUTING.md`; the tests are stdlib-only and seam tests
must run from `multi/seam/`. `./deploy/ironworks --offline doctor` is the fastest repository-wide
signal. `./deploy/gate-coverage.sh` answers whether every path is tracked or ignored — never
whether the index matches the worktree. Those are independent, and only the first has a gate.

`README.md` owns the documentation map. Current code, service JSON, tests, and live operator
commands outrank prose. Public files must not contain client identities, private records, live
host details, or credentials.
