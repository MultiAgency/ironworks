# UPGRADE.md — the ironclaw pin-bump runbook

The deployed ironclaw rev is recorded in `IRONCLAW_PIN` at this repo's root — one line:
the full 40-char commit SHA, then a `#` comment naming it. That file is the version of
record; scripts and operators consume it with `cut -d' ' -f1 IRONCLAW_PIN`. Bumping it is
this procedure, start to finish. Nothing else changes what the fleet runs.

The pin of record is whatever `IRONCLAW_PIN` currently holds; this file does not restate it.
**Prefer a release tag over a bare main SHA** — upgrade-boot fixes ship on release branches
only, and pinning to main silently forgoes them, which is invisible from the SHA and shows up
later as a boot failure.

## Image properties the steps below assume

Re-check both if the base image changes:

- **`USER root` with a gosu entrypoint.** A bare `cap_drop: [ALL]` fails boot, and step 5's
  recreate is where you find out. `multi/instance/docker-compose.yml` and
  `deploy/secretary/instance/docker-compose.yml` both grant
  `cap_add: [CHOWN, SETUID, SETGID]` alongside `cap_drop: [ALL]`; keep them.
- **`sshd` starts only when `IRONCLAW_REBORN_SSH_PUBLIC_KEY` is set.** No compose in this
  repo sets it. Leave it unset — the MT container holds every client's data.

## Three that bite every time

- **The pin can move under you.** A release can be promoted between the moment you pick a
  rev and the moment you build it. Re-run step 2 against the rev you are ACTUALLY building;
  never reuse a delta computed for an earlier target. If the two differ by only a
  release-promotion commit then earlier measurements carry over — but that is a fact you
  confirm, not one you assume.
- **Name the from-rev and the to-rev before calling it a switch.** "Fast-forward" and
  "switch" are claims about a *pair*, and reading two different pairs as one claim is how
  this goes wrong. Run step 2 against your own pair and decide the trade explicitly before
  rebuilding; `git cherry <to-rev> <from-rev>` tells you whether what you are giving up was
  forward-ported.
- **Derived images need their own rebuild.** `ironclaw-multimediator` runs `ironclaw:vidgen`
  (`deploy/vidgen/Dockerfile` = base + node/git), so it cannot be migrated to the bare rev
  tag. Rebuild it with `--build-arg BASE=ironclaw:<9-char rev>`; `ironclaw.rev` is inherited
  from the base, so `verify-pin.sh` passes on the derived tag without stamping it again.

## Invariant: never override the entrypoint

Every run path — `deploy/provision-agent.sh`, `multi/instance/docker-compose.yml`,
`deploy/migrate-image.sh` — runs the image's stock `ironclaw-reborn-entrypoint` with no
`command:`/`--entrypoint` override (verified at the pinned rev). The entrypoint self-applies
ironclaw's config migrations on every boot (in-place `config.toml` rewrites, each with a
backup file) before `exec ironclaw serve`. Adding a command or entrypoint override to any
of these silently skips those migrations. Don't.

What the entrypoint does NOT handle: database/extension-state schema changes. Upgrades
handle those in-binary; **downgrades** can hit stored state the older binary cannot
deserialize — `migrate-image.sh` detects the `unknown variant` failure and prints the
surgery recipe.

**Measure that risk for your own pair; never infer it from a changelog.** The method, which is
what carries across bumps:

1. Clone the volumes. Never probe a live one.
2. Boot each binary against state written by the other — all four combinations of
   (old binary, new binary) × (old state, new state), on both the single-tenant volume profile
   and the multi-tenant Postgres profile.
3. Read the logs for `unknown variant` and `activation_state` hits. Zero on all four is the
   pass.
4. **Confirm non-vacuity before believing a pass.** A clone carrying no extension-install rows
   proves nothing about extension state; check the row counts you are relying on.

One class stays unknown under this method: a volume holding a *telegram* extension install,
which probe volumes generally lack. That is precisely the class `migrate-image.sh`'s surgery
recipe exists for, so keep using it for fleet agents rather than trusting a clean four-way
result to cover it.

**A bundled extension's manifest changing is a different class, and it migrates in-binary.**
The four-way probe is about DESERIALIZATION — whether the new binary can read the old row. It
says nothing about a delta that leaves the row's shape alone and changes what the binary
ships. Read for that separately: different tell, different fix.

The install row is short. `crates/extensions/ironclaw_extension_registry/src/installations.rs`,
`ExtensionInstallation`: `installation_id`, `extension_id`, `manifest_ref { extension_id,
manifest_hash }`, `incarnation_id`, `credential_bindings`, `updated_at`, `owner`. Not the
manifest body, not the activation-credential requirements, not the channel-connection
strategy — those are computed from the package at runtime. So a delta that only edits a
package's `manifest.toml`, or changes requirements the binary derives, cannot produce an
`unknown variant` on the install row. It produces a manifest-hash mismatch, and that has a
designed path.

Per stored installation at boot, in
`crates/extensions/ironclaw_extension_host/src/lifecycle_restore.rs`:
`validate_restored_manifest_hash` compares the stored hash against the manifest the new binary
bundles; on mismatch `migrate_host_bundled_manifest_hash` warns `bundled extension manifest
hash changed; migrating stored installation to new manifest hash` and upserts.
`prepare_manifest_migration` carries `credential_bindings` and `owner` across, so a fleet
agent's bot-token binding survives and nothing needs reconfiguring per agent. It mints a fresh
`incarnation_id`, which nothing in the channel identity or pairing path reads. Rollback runs
the same path in reverse. Re-read those three functions at your own pair — they are upstream
code and can move exactly like the persona surface can.

Two conditions make it a BOOT FAILURE instead, both returning the hash error up through the
restore loop: no stored manifest row for the extension, and a stored manifest whose source is
not `HostBundled`. Neither is reachable for an extension `provision-agent.sh` installed, which
is why they are worth naming — boot dying on a hash mismatch means the volume is not the shape
this path assumes.

**When the four-way probe IS the answer.** That row's deserializer is hand-written with
`deny_unknown_fields`, so a delta touching `ironclaw_extension_registry`, `ironclaw_host_api`
or `ironclaw_extension_contracts` can move the row shape under you. Check those three crates in
the compare first: zero files there means the shape did not move and the risk is the migration
path above; files there means probe it.

One thing neither the reading nor the four-way probe settles. A manifest that changes a
channel's `[channel.connection] strategy` changes how users CONNECT, and existing bindings were
minted under a ceremony the new manifest no longer declares; whether they still count as
connected is a live question. One throwaway container on a CLONED volume answers it, and that
is a far smaller run than the four-way matrix. Read this way for #7766 (1.3.0 -> 1.3.1-rc.1),
where telegram returns from `device_link` to `web_generated_code` — the reading found no shape
change; the binding question stayed open.

## Single-writer rule

libSQL admits one writer per database file (`ironclaw` `crates/substrates/
ironclaw_libsql_runtime/README.md`; upstream's own `railway.toml` sets
`overlapSeconds = 0` for the same reason). Never two containers on one volume; no
overlapping redeploys. The MT instance additionally runs with the Postgres resource
governor as a singleton — never a second MT container against the same DB or volume.

## 1.4.0 persistent-workspace initialization

IronClaw 1.4.0 adds a root entrypoint pass that resolves the default workspace to
`/data/ironclaw-reborn/workspace`, creates it, repairs its ownership, and then drops to uid 1000
with `gosu`. The official image already seeds `/data/ironclaw-reborn` as `1000:1000` mode `0755`.
That ordering matters under IronWorks' `cap_drop: [ALL]`: root carrying only
`CHOWN`, `SETUID`, and `SETGID` cannot create a new child in a directory owned by uid 1000,
because it deliberately has no `DAC_OVERRIDE`.

Do not add `DAC_OVERRIDE` to the long-lived runtime for this one mkdir. Every canonical path first
runs a one-shot initializer as `1000:1000` with no network, no capabilities, no privilege
escalation, and a read-only image filesystem; its only writable mount is the existing `/data`
volume. Compose expresses that as `workspace-init` plus
`condition: service_completed_successfully`; fleet provisioning and `migrate-image.sh` call
`fleet_prepare_workspace` for the same operation. The normal upstream entrypoint still starts as
root afterward, retains its ownership-repair behavior, and drops to the unprivileged runtime uid.

The workspace is persistent state inside the existing `/data` volume. No new backup target is
introduced: volume snapshots already include it. Restoring 1.3.0 after a 1.4.0 attempt still means
restoring the matching pre-upgrade `/data` snapshot (plus the matching database for Postgres) and
the 1.3.0 runtime; the presence of an empty workspace directory is not a substitute for that
paired rollback.

Before any live mutation, run `./deploy/workspace-boot-proof.sh` on the target host with
`IRONCLAW_BOOT_IMAGE` set to the exact rev-labelled image. It creates and removes only
disposable Docker objects while proving cold boot, authenticated assembly, restart persistence,
uid/capability state, SSH absence, and the durable workspace for both canonical profiles.

## 1.3.0 -> 1.4.0 trigger-history migration

IronClaw 1.4.0 performs a durable migration even though its changelog requires no manual
migration step. In both libSQL and Postgres, `trigger_run_history` gains a non-null `source`
defaulting to `schedule`; its primary key changes from `(tenant_id, trigger_id, fire_slot)` to
`(tenant_id, trigger_id, fire_slot, source)`; and `thread_id` becomes nullable. libSQL rebuilds
the table. Postgres replaces the primary-key constraint and changes the column constraint.

**The old runtime is not a rollback for the new schema.** IronClaw 1.3.0 writes with
`ON CONFLICT (tenant_id, trigger_id, fire_slot)`, which is no longer backed by a unique constraint
after the 1.4.0 migration. Starting the old container against migrated storage can therefore boot
yet fail trigger writes. A rollback must restore the old runtime **and** the matching pre-upgrade
database or volume snapshot.

Before step 5 is allowed for this pair, rehearse each deployed backend on a disposable restore:

1. Stop or snapshot under the backend's consistency rules and create a pre-upgrade backup.
2. Restore it under a new disposable database/volume and confirm the expected row counts.
3. Boot the exact rev-labelled 1.4.0 image as the only writer.
4. Verify the four-column primary key, `source='schedule'` on legacy rows, nullable `thread_id`,
   unchanged legacy row contents, and successful new scheduled and manual trigger writes.
5. Restart 1.4.0 against that migrated copy and repeat the reads and writes.
6. Demonstrate that rollback restores both the 1.3.0 image and the untouched pre-upgrade snapshot;
   then verify old reads and writes. Never point 1.3.0 at the migrated copy as the rollback test.

Run this once for the MT Postgres database and once for a fleet libSQL volume that contains real
trigger history. A schema-only empty fixture is vacuous. Record backup identifiers, row counts,
image labels, migration logs, restart result, write results, and restore result with the candidate's
live certification evidence. Do not perform this rehearsal on live storage.

## The bump

1. **Pick the rev.** Prefer **release tags** over arbitrary main SHAs: upgrade-boot
   fixes ship on release branches only (e.g. #7721, "accept the 1.2 activation_state row
   field so 1.3 boots after upgrade", is on the `ironclaw-v1.3.0-rc.2` branch and absent
   from main at the same date).
2. **Read the delta.** In the ironclaw checkout: `git fetch origin --tags &&
   git log <old-pin>..<new-pin>` — look for storage/extension-schema changes and diffs to
   `docker/reborn/entrypoint.sh` and `Dockerfile`. **Release tags diverge from main** —
   check with `git merge-base --is-ancestor <old-pin> <new-pin>`; if it fails this is a
   *switch*, not a fast-forward, and `git log <new-pin>..<old-pin>` lists what you are
   giving up. Read both directions before deciding. Confirm that list with
   `git cherry <new-pin> <old-pin>`: a commit forward-ported onto the release branch under
   a different SHA still shows in `git log` but is prefixed `-` by `git cherry` (already
   present); only `+` lines are genuinely dropped.
3. **Edit `IRONCLAW_PIN`.** Full SHA, `#` comment with the tag name and date.
4. **Build on the VM.** Build a rev-named tag, then retag `ironclaw:main` to it —
   rollback becomes a retag, and `ironclaw:main` is never of unknown provenance:
   ```bash
   REV="$(cut -d' ' -f1 /opt/ironworks/IRONCLAW_PIN)"
   cd /opt/ironclaw-src && git fetch origin --tags && git checkout "$REV"
   docker build -f Dockerfile --label "ironclaw.rev=$REV" -t "ironclaw:${REV:0:9}" .
   docker tag "ironclaw:${REV:0:9}" ironclaw:main
   ```
   The `ironclaw.rev` label makes provenance checkable instead of inferred. Don't eyeball
   it — after each container comes up in step 5, assert it mechanically:
   `./deploy/verify-pin.sh <container>` compares the running image's `ironclaw.rev` label to
   `IRONCLAW_PIN` and exits non-zero on a MISMATCH or an UNLABELED image (unknown provenance).
4b. **Probe a clone with the image you just built — before step 5 touches anything live.**
   The image exists now, so this costs minutes, and it measures YOUR pair instead of inheriting
   a result from a neighbouring rev. Clone the volume of an agent that actually CARRIES the
   extension install in question — the non-vacuity rule above is the whole point, a clone with
   no install rows proves nothing — run the new image against the CLONE on an unused port under
   the single-writer rule, and read what the new binary does with state the old one wrote.

   Ordering is the load-bearing part. `migrate-image.sh` recovers a non-deserializing install
   per agent, but it finds out during the maintenance window, one agent at a time. The clone
   answers the same question before any client is unserved, which is the difference between a
   rolling restart and a surprise.

   **An upstream prebuilt image is not a substitute.** `nearaidev/ironclaw:sha-<7>` on Docker
   Hub is published per MAIN-branch SHA, so a pin on a RELEASE branch has none — measured:
   `sha-ba6f0d3` (1.3.1-rc.1) 404s while main SHAs from the same week resolve. A main image
   carrying the same PR is a different pair; treat it as an early indicator and never as the
   measurement. It also ships no `ironclaw.rev` label, so it must never be tagged
   `ironclaw:main` — `verify-pin.sh` would reject it, which is the label doing its job.

5. **Restart, in order, under the single-writer rule:**
   1. **Fleet agents** (libSQL on per-agent volumes) — one at a time:
      `./deploy/migrate-image.sh <container> ironclaw:<9-char rev>`. It serializes
      stop → volume DB backup → rm → recreate, then asserts API-up, no deserialization
      errors, and pairing mint 200/404. Before touching anything it runs the
      **persona-surface gate**: the fleet writes personas to an upstream-internal
      path, so the script greps the pinned source (checkout at
      `$IRONCLAW_SRC`, default `/opt/ironclaw-src`) for all three path segments and
      refuses to migrate if any moved. After the fleet restarts, `doctor.sh`'s
      stray-stock-prompt sweep is the runtime backstop for the same failure.
   2. **MT instance** — `cd /opt/ironworks/multi/instance && docker compose up -d`.
      Compose stops the old container before starting the new one for the same service;
      never bring up a second MT container on the same DB/volume.
   2b. **Account Service** — `cd /opt/ironworks/deploy/account-intel/data && set -a &&
      . ~/.agency/account-db.env && set +a && docker compose up -d --build`, but **verify it
      actually restarted**. The compose file mounts the repo read-only, so a `service.py` edit
      is VISIBLE on the box the moment sync lands — while gunicorn keeps serving the module it
      imported at boot. `up -d --build` then rebuilds the image, reports every line as success,
      prints `Container … Running`, and changes NOTHING: `docker ps` still shows the old
      container's uptime. Add `--force-recreate account-service` when the container is already
      up, and read the uptime, not the exit code.

      This happens for real and it fails SILENTLY, which is what makes it worth a
      step of its own. The seam had shipped the staleness feature, which re-reads an account
      when the catalog's `updated_at` moves past the version a thread was given; the service
      change that puts `updated_at` INTO the catalog was on disk but not running. Nothing
      errored — the seam simply saw `updated_at: None` for every account, `_moved()` could
      never fire, and records would have gone stale forever with no log line. Verify with the
      cheapest possible assertion, which is the one that would have caught it:

          # from /opt/ironworks/multi/seam, with the client env sourced
          python3 -c "import account_service as asvc, registry as reg; \
            c=asvc.resolve_account_scopes(reg.load_clients())['<slug>']; \
            print(sorted(asvc._catalog(c)['accounts'][0]))"
          # -> must include updated_at

      General rule for this service: after ANY change under `deploy/account-intel/data/`,
      assert the new field/behaviour through a real call before believing the deploy.

   3. **Bridge** — `sudo systemctl restart bridge` after the MT instance answers
      `/api/health`. Expect one watchdog blip + recovery notice in the team chat — that
      is the alerting path working, not a failure.

      **The bridge's state is one transactional store** (`~/.agency/bridge-threads.db`), holding
      each group's conversation pointer and the per-update delivery journal together so a crash
      cannot leave them disagreeing. A restart never re-runs a completed turn; acknowledged,
      retryable, and reconciliation-required delivery outcomes are in `docs/BRIDGE_DELIVERY.md`.

      **Schema v1 -> v2 adds conversation compatibility identity.** Opening a v1 store first
      creates a mode-`0600` SQLite backup beside it (`bridge-threads.db.v1.bak-<UTC timestamp>`),
      then additively records service, version, full composed-instructions SHA-256, model, and a
      `FACT_FIELDS` policy SHA-256, plus the authenticated Account Service organization id and
      normalized Account Service base URL. It does not rewrite `prev`, supplied-context state,
      or the delivery journal. Active v1 rows have no identity that can be recovered honestly, so the
      bridge then refuses startup and prints the exact per-tenant reset command. With the bridge
      stopped, inspect and confirm each reset:

          ./deploy/ironworks tenant reset-thread <slug>
          ./deploy/ironworks tenant reset-thread <slug> --confirm <slug>

      The reset preserves update rows and Telegram cursors. Do not start old code on schema v2;
      rollback requires restoring the recorded v1 backup. There is no reverse migration.
      After the restart, confirm forward progress with `./deploy/ironworks bridge status`
      (exit 0 healthy, 2 unhealthy, 3 could-not-evaluate — the last is not a pass).

      **A restart onto a newer seam can require a one-time state migration, and the bridge
      will NOT start until it is done.** This is by design: `_load_threads` REFUSES a
      `~/.agency/bridge-threads.json` written before per-account versioning (`"supplied"` as a
      list of ids) rather than coercing it, because coercing derives `ever_supplied=false` for
      a thread that HAS had context — the exact condition that trips starvation recovery and
      nulls `thread.prev`, silently discarding a live client conversation to save one
      migration. Failing loudly costs one restart; failing quietly costs a group's history.

      Seen for real on the serve VM. The tell:

          systemctl is-active bridge     # -> activating, never active
          journalctl -u bridge -n 20     # -> ValueError: ... pre-versioning 'supplied' list

      The error message prints the exact migration to run. Back the file up first, keeping its
      mode — `cp -p ~/.agency/bridge-threads.json{,.bak-$(date -u +%Y%m%dT%H%M%SZ)}` — then run
      it and restart again. Do not hand-edit the JSON, and do not delete the file to "fix" the
      boot: deleting it resets every group's `prev`, which is the outcome the refusal exists to
      prevent. Verify with `systemctl is-active bridge` -> `active`, then confirm each group
      kept its `prev` and `ever_supplied`.
6. **Proofs — all must go green:**
   - **The console first, because it is one command and it names what is broken:**
     `./deploy/ironworks doctor` (exit 0 clear · 2 FAILED · 3 BLOCKED · 64 usage). It checks the
     pins, the service definitions, the registry, every tenant's live confinement, the
     residual-authority ledger, and whether a session-revocation route has appeared upstream
     (which would be good news and a docs bug). It is not a replacement for the proofs below —
     it is the thing that tells you which of them to read first.
   - **Session revocation — re-measure it, do not inherit it:**
     `WEBUI_TOKEN=... python3 multi/verify/test_session_revocation.py`. It mints a throwaway
     member, probes with a negative control, deletes it, and reports whether a deleted member's
     token still authenticates. The answer at this pin is **RESIDUAL AUTHORITY** (exit 3), and
     `deprovision.sh`'s exit contract and `docs/IRONCLAW_RUNTIME_CONSTRAINTS.md` both depend on it
     staying the answer. A different result is a documentation change, not a test failure.
   - **Egress containment (network layer) — `./deploy/egress/egress-control.sh verify`.**
     Distinct from the tool-surface probe below and neither implies the other: this one asks
     what the CONTAINER can reach, which survives a tool being re-enabled or a taxonomy rename.
     **A pin bump invalidates the verification stamp automatically** — it is bound to the image
     id — so `./deploy/ironworks egress status` drops from VERIFIED to RUNNING until you re-run
     it. That is deliberate: a new rev can change the HTTP client and the tool taxonomy, and an
     inherited VERIFIED is the most dangerous kind of stale. On a host where the boundary is
     not applied this still FAILS by design; record the result rather than skipping it.
   - **Pin provenance:** `./deploy/verify-pin.sh <MT container> <each fleet container you
     rebuilt>` — asserts the running image's `ironclaw.rev` label equals `IRONCLAW_PIN`.
     The MT container is `multiclaw` on the laptop (the name `multi/instance/docker-compose.yml`
     pins) and still `multi-ironclaw-1` on the serve VM until its next deploy+recreate picks
     that compose change up. Both names are currently correct, on different boxes — pass the
     container explicitly, or set `MT_CONTAINER`, rather than trusting the bare default.
     A MISMATCH or UNLABELED result means the deployed binary is not the pinned one; stop and
     rebuild. This is the mechanical replacement for "trust that `ironclaw:main` is the pin."
   - `multi/verify/` against the live MT instance: `test_two_clients.py` (11/11),
     `test_adversarial_cross_org.py` (5/5), `verify_live_isolation.py`,
     `test_adversarial_routing.py`, `test_injection.py`, `test_injection2.py`,
     `test_instr_live.py`, `test_client_guidance_live.py`, `test_product_loop.py`.
   - `multi/verify/test_member_admin_negative.py` — **now load-bearing for BRIDGE
     STARTUP**, not only for the confinement story. The bridge refuses to serve a tenant
     whose token is not a sealed member, and decides that by probing
     `GET /api/webchat/v2/admin/users` and reading 401/403 as "member"
     (`context_ingress.assert_no_member_is_the_operator`). That expectation is THIS
     proof's, measured live. If a rev changes the denial code — 404, say — every bridge
     refuses to start, fail-closed, and no other check would have warned. Run it on the
     bump, not after the first outage.
   - **Answer quality — `multi/eval/run_eval.py --runs 2`** against the eval org. Everything
     above proves the plumbing still holds; this is the only check that the analyst is still
     *good*. A rev bump changes the model's behaviour, not just the harness, and the failure
     mode is silent: isolation stays green while answers get worse. The recorded baseline is
     20/20 with 10/10 verdicts stable across runs — a drop is a real regression. Read the
     `--json` transcript, never just the score, and remember a grader failing an answer you
     judge correct is the grader's bug until proven otherwise.
   - Fleet: `./deploy/doctor.sh` (whole fleet), plus `--deep` on one agent.
     Doctor is always strict: an unstamped persona is a FAIL, not a warning (the
     `--strict` flag and its WARN path were scaffolding for the re-stamp window and
     were deleted once every deployment carried a sentinel). Env files are also
     what drives coverage, so doctor ends a whole-fleet run with a **coverage check**:
     any running `ironclaw-*` container without one is reported, because nothing else
     in the run would have looked at it (`ironclaw-hq` was in exactly that state —
     a custom, unstamped persona behind an all-clear). Close a coverage FAIL by
     provisioning that agent's env file, or by stopping the container if it is a
     leftover — there is no suppression list, deliberately.
   - Live seam check: the bot reply in each client group answers with that client's
     data only.
   - **Surface drift — `multi/verify/test_surface_drift.py`.** The catalog `confine-member.sh`
     confines against is curated, not enumerated (extension registry + a hand-written host-tool
     list), so a bump that adds host-authored capabilities widens the model's surface with
     nothing to announce it. This probe compares the live surface to the committed expectation
     and fails loudly if a NEW tool is egress- or write-shaped — the case confinement may have
     no lever over at all (`POST /settings/tools/<id>` answers `400 unknown_key` for anything
     the catalog does not carry). A drift here is not automatically a hole; read it, then
     re-measure and update the EXPECTED sets in that file with the reason.
   - **Egress confinement — REQUIRED after every bump.** The member no-egress guarantee
     is enforced per-bearer against ironclaw's tool taxonomy, which can change across
     revs (new/renamed tools, a new tool NOT in the settings catalog). So a pin bump can
     silently re-open egress. After the build: (a) re-run `multi/provision/confine-existing.sh`
     to re-confine every registry client against the NEW surface (fail-closed), and (b) run
     `multi/verify/test_egress_closed.py` — the live acceptance probe: a confined member turn
     ordered to fetch a URL must call no network tool and bring back none of that URL's
     content. Both are needed and neither implies the other — (a) proves the settings state,
     (b) proves the outcome — so do NOT trust the confinement across a bump without both.
     The model's own prose about which tools it has is not evidence (it has been observed
     naming `outbound_deliver` as available on a bearer whose catalog had it disabled); only
     the called-tool list and the catalog are read as truth.
7. **Record.** The commit touching `IRONCLAW_PIN` carries the bump: old → new rev, the
   build tag, and the proof results (including the confine-existing re-run + egress-closed
   check). No other file is the system of record.

   `./deploy/ironworks release verify --json` produces the machine-readable half: every gate it
   could run here, with its result, and — named explicitly rather than omitted — every gate it
   could not, with the reason. Attach it to the bump commit. An artifact whose blocked items
   are invisible is a green artifact, which is the failure this command exists to prevent.

> **The bump is not complete until step 6 is green — the egress items especially.**
> Treat a bump with unverified confinement as an *unfinished* bump, not a finished one with
> a loose end: between the build and a passing `test_egress_closed.py`, every member on the
> instance may hold network authority it did not have yesterday, and nothing else in the
> system would tell you. If you must stop mid-bump, stop *before* step 5 (restart), not
> after — an un-restarted old binary is safe; a restarted new one with stale confinement is
> not. Rolling back is cheaper than leaving step 6 half-done.

## Rollback

For a migration-free pair, retag `ironclaw:main` to the previous rev-named tag and repeat step 5
in the same order. For 1.4.0 after trigger-history migration, that is insufficient: stop the new
runtime, restore the pre-upgrade Postgres database or fleet volume, retag the old runtime, and
only then start it. Runtime and storage are one rollback unit. If the matching snapshot is absent
or its restore was not rehearsed, rollback is unavailable and the only safe path is fix-forward.

Expect `migrate-image.sh`'s unknown-variant surgery path on any agent whose extension state was
written by a newer binary, independently of the trigger-history rule above.
