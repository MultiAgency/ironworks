# IronClaw runtime constraints

Constraints of the pinned IronClaw runtime that IronWorks cannot fix in IronWorks, because the
fix belongs in [nearai/ironclaw](https://github.com/nearai/ironclaw). Each entry states what is
true at the rev in `IRONCLAW_PIN`, what it costs, what IronWorks does about it, the probe that
re-establishes it, and the upstream change that would close it.

**The one rule still holds.** Nothing here is patched locally and no IronClaw source is vendored.
A constraint here is something we design around, not a reason to fork.

**Read the severity lines.** Only `ironclaw-1` carries a live cost.
`ironclaw-2` and `ironclaw-3` are upstream asks whose cost IronWorks has already absorbed, and
`ironclaw-4` and `ironclaw-5` are recorded so that nobody re-discovers the constraint and
proposes a fork to remove it.

**Re-establish every entry after a pin bump.** A bump can change any of them; the probes below
are how you find out.

---

## `ironclaw-1` — a sealed member's session cannot be revoked

**Severity: high, and the only entry here with a live cost.** It is the difference between "we
deprovisioned them" and "they can no longer reach anything."

**What is true.** Session lookup validates an HMAC signature, an expiry, a process-local revoked
set, and the tenant — and never reads the user directory
(`crates/product/ironclaw_webui/src/signed_session_login.rs`). So no change to a user record
affects an already-issued token: deleting a user does not revoke it, suspending a user does not
cut it off the product surface, and no route revokes another user's session. `POST /auth/logout`
is the only path to `revoke()` and is not mounted in env-bearer mode. The session lifetime is
`ADMIN_API_TOKEN_LIFETIME_DAYS = 365`, a constant with no config path
(`crates/app/ironclaw_cli/src/commands/serve.rs`). The denylist behind `revoke()` is
process-local and bounded, and upstream's own tests name both consequences —
`revoked_token_survives_a_simulated_process_restart` and
`denylist_eviction_can_resurrect_a_revoked_token`. Upstream documents the gap directly in
`crates/app/ironclaw_composition/tests/admin_api_e2e.rs`.

**What it costs.** A deprovisioned tenant's member token keeps authenticating the product surface
for up to a year. It cannot reach another tenant's data — the sealed-account boundary is
independent and still holds — but it can run turns as that deleted member.

**What IronWorks does.** Custody is the containment: the token is minted by the operator, stored
`0600`, used by the seam on the tenant's behalf, and never issued to a client or partner.
Deprovisioning probes the former token and exits **3** rather than 0 while it still
authenticates. The expiry is recorded in a ledger holding no token material, and
`./deploy/ironworks doctor` fails while any entry is outstanding. The break-glass is rotating the
operator token, which is the session signing key and logs out every tenant at once.

**Probe.** `python3 multi/verify/test_session_revocation.py` — mints a throwaway member, probes
with a negative control, deletes it, and reports whether the bearer still authenticates. A
runtime that gains revocation is a documentation change, not a test failure.

**Upstream change that would close it**, in preference order: consult user status in session
lookup so an absent or non-Active user fails closed, turning the existing delete and status
routes into real revocation; or a durable tenant-scoped revocation record consulted at lookup,
with an admin route that writes to it by user id; or, failing both, make the token lifetime
configurable so an operator can shorten the residual window.

---

## `ironclaw-2` — no config knob narrows built-in HTTP egress

**Severity: high as an upstream default; the cost is already absorbed here.**

**What is true.** A freshly minted member ships `builtin.http` at `always_allow` with a
compiled-in wildcard public-egress policy, and no runtime profile maps it to a deny mode. Only
private ranges, loopback, link-local, and cloud metadata are blocked — which is why the private
Account Service is safe from the model and the public internet is not.

**What IronWorks does.** Two independent controls, and neither substitutes for the other:
`multi/provision/confine-member.sh` disables every non-allowlisted tool per bearer and probes
fail-closed, and the network boundary in [`EGRESS_CONTAINMENT.md`](EGRESS_CONTAINMENT.md)
restricts destinations regardless of tool state. Per-bearer state is reversible by the bearer, so
the first rests on token custody; the second does not.

**Probe.** `python3 multi/verify/test_egress_closed.py` proves the outcome rather than the
setting, and `./deploy/ironworks egress status` reports the network boundary per host. Both are
required after a pin bump, and neither implies the other.

**Upstream change that would help.** A configurable egress allowlist for built-in capabilities —
the extension host already computes a private-IP deny per surface and built-ins have no
equivalent — or a production profile that applies default-deny to built-ins.

---

## `ironclaw-3` — the tool catalog is not the model's whole surface

**Severity: medium — an advertisement defect, not a containment escape.**

**What is true.** The settings catalog that `confine-member.sh` reads and certifies against is
not the set the model is offered. Measured on a confined member at the pinned rev, several
offered tools were in no catalog at all, and confinement has no lever over what nothing lists:
setting a tool state by id answers `400 unknown_key`.

**Why it is bounded.** `disabled` is enforced at dispatch, not merely advertised — a catalogued,
disabled tool is still offered but returns `policy_denied` when called. So this wastes context
and invites attempts; it is not an escape. The egress guarantee does not depend on catalog
completeness, and the one write-shaped uncatalogued tool is per-user isolated.

**Probe.** `python3 multi/verify/test_surface_drift.py` compares the live surface to a committed
expectation and fails loudly when a new tool is egress- or write-shaped;
`python3 multi/verify/test_member_admin_negative.py` covers the write and admin surfaces.

**Upstream change that would help.** Make the settings catalog exhaustive over host-authored
tools, or return a per-caller effective-surface endpoint an operator can confine against.

---

## `ironclaw-4` — no per-account persona on the hosted multi-tenant profile

**Severity: low — IronWorks does not need it, and the workaround is better than the feature.**

**What is true.** The auth middleware stamps the host-configured default agent onto every caller;
the admin user API has no agent field; there is no agents endpoint; and on the hosted-Postgres
path the identity source degrades to empty, so the instance bakes no persona at all.

**What IronWorks does.** The persona lives in the seam and is injected via `instructions` every
turn (`multi/seam/persona.py`, `multi/seam/context_ingress.py`), which is also what makes
per-tenant guidance and per-tenant service definitions possible without touching core. **This is
recorded as a constraint only so nobody re-discovers it and proposes the fork.**

**Probe.** `(cd multi/seam && python3 test_services.py && python3 test_client_guidance.py)` —
composition order and guidance binding are asserted structurally.

**Upstream change that would help.** None is wanted. Do not fork the runtime to add it.

---

## `ironclaw-5` — no extension or skill lifecycle on the production profile

**Severity: low.**

**What is true.** Extension-lifecycle commands fail closed on the hosted-multi-tenant and
production profiles ("extension lifecycle is available only for standalone Reborn services"), and
there is no multi-tenant profile upstream that has them.

**What IronWorks does.** The seam owns Telegram rather than an IronClaw extension, so **this
costs IronWorks nothing today** — but any future "install an extension on the MT instance" plan
is blocked by profile and would require patching core.

**Probe.** Re-check the production profile when a proposed feature depends on runtime-managed
extensions.

**Upstream change that would help.** Production-profile extension lifecycle with tenant-safe
semantics, if a current product requirement ever needs it.
