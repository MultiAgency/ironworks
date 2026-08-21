# UPGRADE.md — the ironclaw pin-bump runbook

The deployed ironclaw rev is recorded in `IRONCLAW_PIN` at this repo's root — one line:
the full 40-char commit SHA, then a `#` comment naming it. That file is the version of
record; scripts and operators consume it with `cut -d' ' -f1 IRONCLAW_PIN`. Bumping it is
this procedure, start to finish. Nothing else changes what the fleet runs.

## The current pin

`ironclaw-v1.3.0` (`70795c16e`) — a release **tag**, not a bare main SHA, because
upgrade-boot fixes ship on release branches only. How this pin was arrived at, and what
choosing it cost, is in **Pin history** at the foot of this file. You do not need that to
run the procedure; you need it when you are deciding the next pin.

Two properties of this image that the steps below assume:

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

**For the `b6c33d33d` ↔ `8483596bf` pair specifically, that risk was MEASURED, not
assumed (on clones — live volumes never touched):** all four combinations
boot healthy, with zero `unknown variant` / `activation_state` log hits.

| profile | binary | state written by | result |
|---|---|---|---|
| `hosted-single-tenant-volume` | main `b6c33d33d` | rc.2 | healthy |
| `hosted-single-tenant-volume` | rc.2 `8483596bf` | main | healthy |
| `production` (MT + Postgres) | main `b6c33d33d` | rc.2 | healthy, served real users |
| `production` (MT + Postgres) | rc.2 `8483596bf` | main-touched | healthy, 52 tool entries |

Non-vacuity was checked both times: the volume clones carried 4 extension-install rows
each, and the MT clone carried 3,955 `root_filesystem_entries` and served real sealed
member accounts over the admin API. **Untested and still unknown:** a volume holding a
*telegram* extension install (both probe volumes had none) — that is the exact class
`migrate-image.sh`'s surgery recipe exists for, so keep using it for fleet agents.
Re-measure with this method when either side of the pair changes; do not infer
compatibility from a changelog.

## Single-writer rule

libSQL admits one writer per database file (`ironclaw` `crates/substrates/
ironclaw_libsql_runtime/README.md`; upstream's own `railway.toml` sets
`overlapSeconds = 0` for the same reason). Never two containers on one volume; no
overlapping redeploys. The MT instance additionally runs with the Postgres resource
governor as a singleton — never a second MT container against the same DB or volume.

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
          python3 -c "import context_ingress as ing; c=ing.load_clients()['<slug>']; \
            print(sorted(ing._svc('/list_accounts', c)['accounts'][0]))"
          # -> must include updated_at

      General rule for this service: after ANY change under `deploy/account-intel/data/`,
      assert the new field/behaviour through a real call before believing the deploy.

   3. **Bridge** — `sudo systemctl restart bridge` after the MT instance answers
      `/api/health`. Expect one watchdog blip + recovery notice in the team chat — that
      is the alerting path working, not a failure.

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

> **The bump is not complete until step 6 is green — the egress items especially.**
> Treat a bump with unverified confinement as an *unfinished* bump, not a finished one with
> a loose end: between the build and a passing `test_egress_closed.py`, every member on the
> instance may hold network authority it did not have yesterday, and nothing else in the
> system would tell you. If you must stop mid-bump, stop *before* step 5 (restart), not
> after — an un-restarted old binary is safe; a restarted new one with stale confinement is
> not. Rolling back is cheaper than leaving step 6 half-done.

## Rollback

Retag `ironclaw:main` to the previous rev-named tag and repeat step 5 in the same order.
Expect `migrate-image.sh`'s unknown-variant surgery path on any agent whose extension
state was written by the newer binary.

## Pin history

The record of how each pin was chosen and what the bump actually cost. It is here, at the
end, rather than above the procedure: every entry is true of one pair of revs and goes stale
the moment the pin moves, whereas the steps above are true of every bump. Read this when you
are choosing the next pin, not when you are running one.

### `70795c16e` — `ironclaw-v1.3.0`

The FINAL release, one commit past `1.3.0-rc.2` (`chore(release): promote 1.3.0-rc.2 to
1.3.0`, #7754). Two different pairs are in play here and conflating them is how this gets
read wrong:

- **`rc.2 → v1.3.0` is a fast-forward** — one release-promotion commit, no code change. So
  everything measured for rc.2 carried over verbatim: the same `USER root` + gosu entrypoint,
  the same opt-in sshd, and the persona-surface gate passing (re-verified at this rev). The
  target moved from `8483596bf` to `70795c16e` *mid-flight*; the carry-over was confirmed,
  not assumed.
- **`main b6c33d33d → v1.3.0` is a real switch**, not a catch-up. It gains upgrade-boot fix
  #7721 (`505cf0a15`), which exists only on the release branch, and gives up 15 main commits
  — notably a libSQL write-lane starvation fix and native structured-output finalization.
  `git cherry 70795c16 b6c33d33d` confirmed none of the 15 was forward-ported. Main and the
  release branch diverged at `18ab836f2`. Do not "catch the pin up to main" without redoing
  step 2: the fleet was already running `1.3.0-rc.2` images, so rebuilding the older main pin
  would have *removed* #7721 from the agents — the exact regression step 1 warns about.

**Tool surface:** v1.3.0 widened nothing — still 50 tools, so the confinement allowlist
needed no change. Re-check this every bump; a new rev can widen what the allowlist was
written against.

**`pairing/mint` returns 404 for telegram on this rev.** v1.3.0 is a device-link build; rc.2
was a deep-link mint build. The generic pairing routes
(`.../{extension_id}/pairing/{mint,status,unpair}`) still exist — telegram simply registers no
pairing service now (it declares `method="device_link"`), so they 404 for
`extension_id=telegram` specifically. That is registry-driven and per-extension, not a removed
route. `migrate-image.sh` treats both builds as healthy and existing pairings survive in the
volume, but any NEW telegram personal-connection flow is device-link.

**How the bump actually ran, on each box.** Both follow the steps above; recorded here because
the container names and the rollback-tag discipline are easy to get wrong.

- *Serve VM:* built on the box with `--label ironclaw.rev`, preserving the outgoing image
  under its own rev tag (`ironclaw:b6c33d33d`, so rollback is a retag rather than a rebuild),
  retagged `ironclaw:main`, recreated `multi-ironclaw-1` (project `multi`) and
  `secretary-ironclaw-1` (project `secretary`), restarted the bridge, re-applied member
  confinement against the new surface, and finished by asserting `verify-pin.sh` exits 0 on
  both containers.
- *Laptop fleet:* rebuilt from a clean checkout at the pin **with** `--label ironclaw.rev` —
  an earlier laptop build of the same rev carried no label at all and would have migrated
  containers that still failed `verify-pin.sh`. Then `migrate-image.sh` per single-tenant
  container, compose recreate for the MT instance, `verify-pin.sh` across every container,
  and a `doctor.sh` fleet sweep. Outgoing images — base and derived — kept as rollback tags.

**A proof that reads differently per box, and is not a regression.**
`test_client_guidance_live` returns 10/13 on the VM and 13/13 on the laptop. All three
failures are marker checks: the VM's live proof-client guidance files contain neither
`Alpine` nor `Harbor`, while the committed fixtures carry 5 and 4 of those markers. Guidance
is a file on disk that a binary swap cannot touch — the VM's proof clients were simply
provisioned outside the fixture kit. Align them before reading that proof as a gate there.
