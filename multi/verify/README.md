# multi/verify — the evidence

Reproducible proofs behind the verified architecture (see `../README.md`). Preserved out
of the session scratchpad so the evidence survives. Most hit an IronClaw instance +
(for the loop proofs) the Account Service; the **clean clone?** column says exactly what
each needs beyond this repo:

- **offline** — runs from a clean clone, no services, no credentials.
- **instance** — needs a local multi-tenant instance (recreate from `../instance/`,
  secrets minted per its `.env.example`) and your own LLM gateway key.
- **fixtures** — additionally needs provisioned proof clients (`proof-a`/`proof-b`
  registry entries + guidance + seeded Account Service); reproducible from a clean
  clone via the committed `fixtures/` kit (see "Reproduce from a clean clone" below).
- **operator-only** — live-deployment monitoring, not architecture evidence: touches
  the operator's untracked env or live-run transcripts and is not meant to run from
  a clone.

| script | proves | clean clone? |
|---|---|---|
| `test_injection.py` | channel-injected persona **governs** on hosted-MT (turn 1 PASS, required). Turn 2 tests whether injecting ONCE and leaning on `previous_response_id` drifts — historically it did; **at the pinned rev it does not** (see note below), so turn 2 is recorded as an observation, not a gate | instance (`WEBUI_TOKEN`) |
| `test_injection2.py` | sending the persona via the `instructions` field **every turn** holds it (turn 1 + 2 PASS) — the design rule | instance (`WEBUI_TOKEN`) |
| `test_product_loop.py` | **the moat**: sealed account + the CLIENT composition via `instructions` (`compose_client_persona` — what every registry client actually gets) + the committed `proof-a` book via `input` → grounded, evidence-tagged account intelligence in that client's own guidance vocabulary (7/7 context tells). Reads committed fixtures, so it needs no provisioned registry | instance (`WEBUI_TOKEN`) |
| `test_two_clients.py` | two provisioned clients, disjoint orgs, served end-to-end through sealed accounts: each sees only its own book, persona governs, no token in any request body (11/11) | fixtures |
| `test_adversarial_cross_org.py` | the hostile complement: an injected exfiltration turn as client A gets zero of client B's data and no token materializes (5/5) | fixtures |
| `test_client_guidance_live.py` | two registry clients with their OWN synthetic guidance get materially different, guidance-governed answers — and neither is steered toward MultiAgency's internal composition | fixtures |
| `test_adversarial_routing.py` | the bridge fails closed on malformed/spoofed chats, routes by `chat.id` never content, and catches duplicate-group collisions (pure-function; no live Telegram) | **offline** |
| `test_fixtures_offline.py` | the committed `fixtures/clients/` templates + synthetic guidance load through the real `load_clients()` via `CLIENTS_DIR`, each persona carries only its own guidance, and the templates hold placeholders — never secrets (12 checks) | **offline** |
| `test_freshness_lifecycle.py` | one account's `updated_at` through the REAL bridge: a pre-versioning state file is refused, the documented migration leaves versions unknown, the next turn HEALS in exactly one fetch, and a genuine edit is re-read unprompted (9/9). Pins the null-guard that once made "version unknown" mean "never re-fetch again" — which silently pinned every migrated thread to its first copy for the life of the thread | instance + account DB (leg D skips if the store is unreachable) |
| `test_instr_live.py` | the live single-tenant instance the bot talks to honors `instructions` (layered over its baked persona) | operator-only (live env) |
| `verify_live_isolation.py` | post-hoc verifier for a live two-group Telegram run: neither group's saved replies contain the other client's private markers | operator-only (live transcripts) |
| (deleted) the end-to-end adapter drive lives in `context_ingress.py`'s own `__main__` hero-flow oracle | — | — |

**`test_injection.py` turn 2 — the documented drift no longer reproduces.** This
table used to state the expected result as "turn 1 PASS, turn 2 FAIL", and that contrast was the
empirical basis for the product rule that the seam re-sends the persona via `instructions` on
EVERY turn instead of relying on `previous_response_id`. Re-measured on the current model/rev:
**6 consecutive runs, turn 2 held the marker every time** (1 run by one lane, 5 by another, same
instance, same day). Turn 1 passed in all 6.
What that does and does not mean. It does NOT mean stop re-sending the persona — keep the rule.
Re-sending is cheap (automatic prompt caching on the TEE-hosted models we pin), it is strictly
safer, and `test_injection2.py` remains a hard gate on it. What changed is the EVIDENCE: the rule
is now justified by defence-in-depth rather than by an observed failure, and anyone citing "turn 2
drifts" as a measured fact is citing something that has not reproduced in 6 attempts. Do not
re-tighten this into a gate expecting FAIL — that would make a healthy system fail the build,
which is the trap this file's exit-code design already avoids.


**Tenant-wide surface probes — the surfaces the cross-org proofs above do NOT cover.**
Every sealed client shares one instance, so these ask whether the *tenant-wide* control-plane
surfaces (shared credential scope, shared mounts, shared catalog, the member/admin split) leak
across that boundary:

| script | proves | clean clone? |
|---|---|---|
| `test_tenant_shared_secret_probe.py` | the tenant-shared `DEV_SECRET` surface is dormant: the MT container env carries no `IRONCLAW_REBORN_DEV_SECRET__*` (load-bearing allowlist) and a member turn surfaces no credential; a guarded `--staging` half pins the surface is real + leak-redacted | instance (staging leg guarded) |
| `test_tenant_shared_mount_probe.py` | a hostile member turn cannot read/write `/tenant-shared/{channel-pairing,reborn-identity,reborn-projects}` via alias, traversal, or absolute path; DB row-count leg (for the operator) pins zero bytes landed | fixtures (DB leg reports BLOCKED locally) |
| `test_catalog_parity.py` | the tool/skill catalog is byte-identical across two clients and carries no client-name marker (guards a future operator seeding shared skills/tools with one client's material) | fixtures |
| `test_registry_reconciliation.py` | the registry and the instance agree on exactly who exists: no member the registry has never heard of (the blind spot every registry-enumerating tool shares — `confine-existing.sh` and `test_egress_closed.py` both derive their member set from `CLIENTS_DIR`), no registry entry pointing at a deleted account, and no entry missing `IRONCLAW_USER_ID` (without it `deprovision.sh` silently skips deleting the sealed account) | instance (`WEBUI_TOKEN`) |
| `test_surface_drift.py` | the catalog is not the surface, so this watches the gap: leg A (structural) asserts every dangerous CATALOGUED tool is still disabled; leg B asks the model to enumerate what it is actually offered and fails on any drift from the committed expectation — loudly if a new tool is egress/write-shaped, since confinement may have no lever over it (`unknown_key`). Also watches for the `tool_search`/`describe`/`call` trio arriving. Run it after every pin bump | instance + one client (makes ONE model call) |
| `test_member_admin_negative.py` | a sealed member bearer is denied (401/403) on admin-users, settings/tools mutation, and operator routes; **project** routes are member-REACHABLE but per-user isolated (A creates → B's list omits it, B GET/DELETE by exact id → 404) — correcting an earlier INFERRED claim that the project surface was unreachable via product routes: it holds via caller-scoped ACLs, not route inaccessibility, and projects are a member-writable surface the seam never uses | fixtures (+ optional operator token) |

`common.py` holds the shared `/v1/responses` helpers. Each tenant-wide probe reports `BLOCKED`
per leg when the VM/instance is unreachable rather than skipping silently, and exits
`2` if no assertion ran. This index covers every shipped script.

## Reproduce from a clean clone

> **Status — what is actually verified, and what is not.**
>
> **Verified (2026-08-21).** The offline leg passes from a genuinely clean tree, checked by
> materialising the index into an empty directory (`git checkout-index -a -f --prefix=…`,
> which is exactly what a fresh clone receives) and running it there with no operator state
> on the box: `test_adversarial_routing.py`, `test_fixtures_offline.py`, both `multi/seam`
> suites, `test_compose_persona.py`, `test_tail_parity.py`, `test_graders.py`,
> `test_doc_refs.py`, `bash -n` + `shellcheck` over every tracked script, and `node --check`
> on the Worker. Every file the live bring-up below cites is present in that tree, and
> `test_product_loop.py` composes its persona and loads its book from clone-only files.
>
> **Inputs verified, execution not.** The bring-up's *ingredients* are checked and complete:
> `../instance/.env.example` covers every variable `../instance/docker-compose.yml` requires (the
> only other one, `IRONCLAW_IMAGE`, has a default); the account stack's two variables are both
> minted or set by `prod-up.sh`; and every path steps 1–4 name is tracked. So a reader will not
> hit a missing file or an undocumented variable.
>
> **What remains unproven is the sequence executing start to finish** — ordering, health-wait
> timing, and `provision.sh` against a genuinely empty registry. It cannot be walked on a host
> that already serves clients: the steps claim project names `multi` and `multiagency-data`,
> container `multiclaw`, ports 3020 and 8443, the external `multiagency-data` network, and
> `~/.agency/clients/` — all of which a live operator box is already using. Overriding six
> hardcoded values would test a different sequence than the one written here, and tearing the
> live stack down to free them is not an option. So this needs a virgin host, which is exactly
> the audience the steps are written for. Walk it there, then fix this paragraph.

0. **Offline sanity (no services)**: `python3 test_adversarial_routing.py` and
   `python3 test_fixtures_offline.py` — both must pass from a bare clone.
1. **Instance**: `cp ../instance/.env.example ../instance/.env`, mint fresh values,
   bring up `../instance/` (MT instance on `127.0.0.1:3020`).
2. **Account Service**: bring up `deploy/account-intel/data/` (service on
   `127.0.0.1:8443`).
3. **Proof clients**: copy the synthetic guidance into your registry dir first
   (`cp fixtures/clients/*.guidance.md "$CLIENTS_DIR"` — guidance is mandatory and
   fail-closed, so it must exist before provisioning), and put each client's committed
   book where `provision.sh` will seed it from:
   ```
   mkdir -p ~/.agency/account-data/proof-a ~/.agency/account-data/proof-b
   cp fixtures/clients/proof-a.account.json ~/.agency/account-data/proof-a/
   cp fixtures/clients/proof-b.account.json ~/.agency/account-data/proof-b/
   ```
   **Use these, not `deploy/account-intel/data/candidates/*.json`.** Both sets are invented
   (`.example` domains, `_synthetic` banners), so this is a FRAMING rule, not a realness
   one: the candidates are the demo book for MultiAgency's OWN side of the table, and their
   notes are written in the vendor's voice
   ("reached out after seeing MultiAgency builds custom AI agents"), which is correct
   there and wrong in a synthetic *external* client — and it puts a term from
   `test_client_guidance_live.py`'s `FORBIDDEN` list inside the very record that
   proof's `:42` tick requires the model to ground in. Seeding the proof orgs from the
   internal candidates made that proof fail roughly one run in three on a healthy,
   correctly-isolated system. `test_fixtures_offline.py` now pins the committed books
   clean, offline, so CI catches a regression instead of a live proof flaking.

   Then provision with
   `../provision/provision.sh proof-a "Proof Client A" -100900011` /
   `… proof-b "Proof Client B" -100900012` (seeds Northwind vs Studio Vireo and
   writes the registry `.env`s; the committed `fixtures/clients/*.env.template`
   show their shape).
4. **Run**: `cd ../instance && set -a; . ./.env; set +a; python3 ../verify/<script>`
   — the instance-level proofs first, then the fixtures-level suite.

The operator-only pair (`test_instr_live.py`, `verify_live_isolation.py`) is
deliberately excluded: both monitor the live deployment and prove nothing about the
architecture that the suite above doesn't.
