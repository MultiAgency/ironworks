# Security

## Reporting a vulnerability

Report privately through this repository's **Security → Report a vulnerability**
tab (GitHub private vulnerability reporting). Please do not open a public issue
for a suspected vulnerability.

We aim to acknowledge a report within a few business days. This is a small
project with no bug bounty; we will tell you honestly what we can and cannot fix,
and we will credit you if you want the credit.

## What this repo is, and what it is not

This repo is configuration, personas, and tooling. It runs the **unmodified
upstream [ironclaw](https://github.com/nearai/ironclaw) binary** at the rev
pinned in `IRONCLAW_PIN` — no fork, no patches, and no ironclaw source is
included here.

So please route reports accordingly:

- **Report here** — anything in this repo's own code: the context seam
  (`multi/seam/`), provisioning and deprovisioning scripts (`multi/provision/`),
  the account data layer (`deploy/account-intel/data/`), fleet tooling
  (`deploy/`), the deployment units (`multi/serve/`), or a persona or skill file
  that weakens a boundary it is supposed to hold.
- **Report upstream** — anything in the agent runtime itself belongs with
  [nearai/ironclaw](https://github.com/nearai/ironclaw). If you are unsure which
  side a bug falls on, report it here and we will help route it.

## What we consider security-relevant

The boundaries this project exists to hold, roughly in order of severity:

- **Cross-tenant data access.** One client's turn reaching another client's
  accounts, memory, threads, or credentials.
- **Credential exposure.** Any path that puts a client's org token or sealed
  account token where the model, another client, or a log can see it. The seam
  is the only component that should ever hold them.
- **Confinement escape.** A member turn regaining network egress, write, or
  execution capability that provisioning is supposed to have removed — including
  via prompt injection in account data or a chat message.
- **Fail-open behavior.** Anything that serves a client when it should refuse:
  an unregistered group, a missing or mismatched guidance file, an unknown token,
  a duplicate group id.

## Known and accepted limitations

These are deliberate trade-offs, documented rather than hidden. Reports that
restate them are welcome but already known:

- **Deleting a member does not revoke its token.** A sealed account token is a
  signed session; the server's revoked-set is in-memory only and account deletion
  never adds to it, and upstream exposes no API to revoke another user's session.
  A leaked token therefore keeps authenticating until it expires — the lifetime is
  hardcoded at 365 days. The only containment is rotating the instance's session
  signing key, which logs out every client at once. Deprovisioning says this
  plainly rather than claiming access is cut; the mitigation that makes it
  tolerable is custody: the token is held by the seam and never reaches a client.
- **One bot, one bridge process.** All client groups share a single Telegram bot
  token and one bridge process — a shared single point of failure, and the token
  can read every group it is in. Turns are also served serially, so one long turn
  delays every other client's.
- **Tool disabling is per-bearer.** Upstream scopes tool permissions per
  principal with no tenant-global scope, so confinement is applied per member at
  provisioning time and must be re-applied after a runtime version bump.
- **Persona governance is empirical.** Injecting the persona every turn is
  behavior we re-prove against the runtime on each version bump, not a guarantee
  the runtime contracts.
- **The tool catalog confinement reads is not the whole model-visible surface.**
  `multi/provision/confine-member.sh` disables every tool outside its allowlist by
  reading `/api/webchat/v2/settings/tools`, and re-reads that catalog to certify the
  result. But the catalog and the surface the model is actually offered are two
  different sets. Measured on a confined client member at the pinned rev: the catalog
  held 50 entries (37 disabled) while the model was offered 17 tools, and **five of
  those were absent from the catalog entirely** — `result_read`,
  `outbound_delivery_targets_list`, `project_create`, `skill_activate` and
  `capability_info`. Confinement never considered them, because nothing listed them.
  Two things bound this, and neither is luck:
  - **`disabled` is enforced at dispatch, not merely advertised.** A catalogued,
    disabled tool is still offered to the model, but calling it is refused —
    `policy_denied`, "the capability is disabled by tool approval settings"
    (reproduced twice on a live member). So this is an *advertisement* defect: it
    wastes context and invites attempts, but it is not a containment escape.
  - **The egress guarantee does not rest on the catalog being complete.** The egress
    tools are catalogued and disabled, and `multi/verify/test_egress_closed.py` checks
    the *outcome* — a real member turn ordered to fetch a URL must call no network tool
    and return none of that URL's content. That check is what `deploy/UPGRADE.md`
    step 6 makes mandatory after every pin bump, and it holds regardless of what the
    catalog omits.

  Of the five uncatalogued tools, none grants egress; `project_create` is a write, and
  is per-user isolated (proven in `multi/verify/test_member_admin_negative.py`).
  Reports that a *catalogued* tool state is not honoured, or that any uncatalogued tool
  reaches another tenant or the network, are in scope and we want them.

## Supported versions

The pinned rev in `IRONCLAW_PIN` is the only supported runtime version. There are
no released versions of this repo yet, and no backports.
