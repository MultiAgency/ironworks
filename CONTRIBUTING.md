# Contributing to IronWorks

Thanks for your interest. IronWorks is the operator-run application layer that serves
organization-scoped services on the **official, unmodified
[IronClaw](https://github.com/nearai/ironclaw) binary** — configuration, data, and tooling around
a runtime it never forks. `README.md` owns what the product is and what else this repository
operates. Contributions that keep the one rule below true are very welcome.

## The one rule

**Never fork or patch IronClaw.** Everything here is config, data (personas), and
scripts that run *around* the stock binary. A change that requires editing IronClaw's
source belongs upstream in IronClaw, not here. If you hit a wall that seems to need a
core change, open an issue describing it — often there's a seam that avoids the fork.
Constraints we have already measured and designed around are in
`docs/IRONCLAW_RUNTIME_CONSTRAINTS.md`.

## Verifying against upstream

The one rule says what not to do. This says what to do instead.

Before you write anything that depends on how IronClaw *behaves* — an endpoint shape, a session
semantic, a tool name, a config key, an error string — read it in an IronClaw checkout, at the
rev named by `IRONCLAW_PIN`, and check that rev out explicitly. A checkout you already have is
probably sitting somewhere else. And do not read `main`: the pin is a release tag, *not* an
ancestor of main, and the boot fix it carries is absent from main. Main is a different version,
not a newer one, so it misleads in both directions — a pattern you read there may not exist in
the binary we run, and a fix the pin already has may look unfixed.

Authority order, highest first: the pinned source, then upstream releases and issues, then
nothing. A blog post, a Stack Overflow answer, or a model's recollection is not a source for
runtime behaviour. If the behaviour you need does not exist at the pin, that is an upstream
blocker, not a local patch — open the issue rather than reaching for a fork.

Cite what you read, in the shape the tree already uses: prose names the upstream repo-relative
path (`multi/README.md` names `crates/substrates/ironclaw_network/src/policy.rs`), a code
comment names file and line (`multi/seam/context_ingress.py` names `handlers.rs:186`). That is
why `deploy/lib/test_doc_refs.py` excludes `.rs` and `.toml` — an upstream path is a citation,
not a promise about this tree. A line number only means something against a rev, so name the rev
when the surrounding text does not. The model to copy is
`multi/verify/test_session_revocation.py`, whose header states what it expects at the pinned rev
and says how it knows: source-traced first, then measured.

Two corollaries that are easy to get wrong:

- **Never add a fallback literal for `MODEL_PIN` or `IRONCLAW_PIN`** — a default is the one value
  that can silently outrank the pin. Read them through `multi/seam/pins.py` in new Python and
  `deploy/lib/fleet.sh` in new shell, and let a broken checkout fail loudly. Those two are not
  the tree's only readers — `multi/verify/common.py` wraps `pins`, and
  `deploy/secretary/test_aide_discovery.py` re-implements the parse to avoid a `sys.path` hack
  and says so — but every reader holds the
  no-literal rule, and `multi/seam/test_pins.py` runs the shell adapter against the Python one so
  the pair cannot silently diverge.
- **The model pin is a behavioural claim, not a version string.** Its comment records what was
  tested, against a real book, on a stated date. A model swap re-earns that evidence — re-run the
  proofs named there rather than inheriting their result.

## Before your first commit

Install the local hooks. They are not optional and CI cannot replace them:

```
pipx install pre-commit    # or: brew install pre-commit
pre-commit install
```

Two gates run on every commit. `gitleaks` scans staged content for secrets. `gate-coverage`
refuses any path that is neither tracked nor ignored — because every other check in this repo
derives its file set from the index or from history, so untracked-and-unignored content passes
them all by being *invisible* rather than by being clean. That gate is local-only by nature: a
CI checkout has nothing untracked, so a CI job would be a no-op. If you skip `pre-commit
install`, nothing anywhere enforces it.

### A file has three states, and `git ls-files` reports only one

Worktree, index, and running instance can each hold a different version, and most tooling here
reads exactly one of them. **A path appearing in `git ls-files` proves nothing about its
content.** `git show :<path>` is the only thing that separates the states; `git status`'s
second column (`AM`, ` M`) is the warning that they differ.

This is not a theoretical distinction — it decides whether a `git add` is safe. `deploy/lib/`
carried the shape at its worst: `identities.py`, `lifecycle.py` and `egress_status.py` imported
a helper in the **worktree** that their **staged** versions did not import, while the helper
itself was untracked. Read one way the tree was fine, read another it was broken, and each of
the three readers below was correct about a different thing:

- a **clone** gets the index, so it worked;
- `deploy/sync-vm.sh` takes its manifest from `git ls-files` but transfers **worktree** bytes,
  so a VM would have received importers without the module they import;
- and staging those three files *without* the helper would have broken the clone too.

So when you stage a file, stage what it needs in the same change, and check the direction you
are not looking at. `deploy/gate-coverage.sh` names the untracked third state but cannot see
this one: every path involved was either tracked or reported, and the defect lived in the
disagreement between two versions of a tracked path. `multi/seam/test_bridge_delivery.py` and
its support module had the identical split at the same time, so it is a pattern, not an
accident.

What it looks like when it happens to you: `git checkout -- multi/`, run in a scratch copy to
undo an experiment, replaced those files with their staged versions and pytest went from clean
to **13 collection errors** — no edit, no bad command, just an ordinary git operation swapping
one valid state for another valid state that the rest of the tree no longer matched. That is the
whole failure mode in one line. Nothing warns you, because nothing is wrong with either version.

## Running the CI gates locally

Install the declared development environment once, then run the same aggregate command as CI:

```sh
python3 -m pip install --require-hashes -r requirements-dev.lock
./deploy/ironworks test
```

**It exits 0, 2 or 3, and 3 is not a pass.** The gate holds the same verdict contract the console
does (§ "Instrumenting a new subsystem"): `0` every evaluated check passed, `2` something FAILED,
`3` nothing failed but something could not be **evaluated** — and the summary names which. A tool
that is not installed is blocked, never failed: without docker the compose stacks are unchecked,
without `shellcheck` or `node` those linters are unchecked, and skipping the install line above
leaves `coverage` and `pytest` missing, which blocks the entire test run. None of those mean the
repository is broken, and reporting them as failures is what made a laptop without docker unable
to see this gate green. Treat a `3` as work still to do, not as noise to route around.

One check has no separate command because it is not a tool: the **pipefail substitution guard**.
`$(... grep ...)` and `$(... | grep ...)` under `set -euo pipefail` die *silently* when the
pattern does not match — the substitution exits non-zero, `set -e` aborts, and the caller sees a
clean exit with nothing done. `bash -n` parses it happily and shellcheck does not flag it, so
this guard is the only thing that ever has. Only scripts that opt into `pipefail` are at risk, and
the exemption is per-line: append `|| true` and test emptiness explicitly on the line that needs
it, rather than exempting a whole file.

The individual commands below remain useful when iterating on one subsystem.

Run the gates the way CI runs them, not a variant. Copy these verbatim — the two commands
below have traps that make a laxer local check pass while CI fails, and both have caught
real breakage in this repo:

```
# The file set CI uses: tracked PLUS untracked-not-ignored. `git ls-files` alone is
# index-only, so a subtree nobody has `git add`ed yet is invisible to every check.
git ls-files --cached --others --exclude-standard -z -- '*.sh' > /tmp/sh-files.z

xargs -0 -I{} bash -n {} < /tmp/sh-files.z     # syntax
xargs -0 shellcheck < /tmp/sh-files.z          # NO -S flag. See below.

# Both extensionless Python entry points must be named: ruff discovers by extension.
ruff check . deploy/lib/compose-persona deploy/ironworks

# JS: --input-type=module is load-bearing. See below.
node --input-type=module --check < deploy/secretary/worker/worker.js
```

**Do not add `-S warning` (or any `-S`) to shellcheck.** CI runs it bare, and shellcheck exits
non-zero on *info*-level findings. A local `-S warning` hides those, so your file reads clean
every time you check it and fails the gate anyway. Your local check and the gate must ask the
same question, and the gate's is the stricter one.

**Match the pinned version, too — `shellcheck 0.11.0`.** The flag is not the only way the two
questions come apart. CI used to `apt-get install -y shellcheck`, which takes whatever the runner
image carries, so the gate drifted with Ubuntu and could not be reproduced anywhere: on
2026-08-27 the runner's copy raised three SC2015 findings that 0.11.0 does not, seam-ci had been
red on every run because of it, and five Dependabot PRs were blocked behind a failure none of
them had caused. `.github/workflows/seam-ci.yml` now installs a checksummed release, so
`shellcheck --version` matching that pin is what makes your local run mean anything. Bumping the
pin means re-reading whatever it newly reports rather than assuming the tree moved.

**Do not trust a bare `node --check FILE.js`.** It exits 0 on a syntactically broken file when
that file contains an `import` — the ESM-detection path skips the check. Reading the file on
stdin with `--input-type=module` fails correctly on both. This is not hypothetical; see the
comment in `.github/workflows/seam-ci.yml`.

Most focused suites remain stdlib-only and directly runnable. The aggregate gate uses pytest for
recursive discovery—particularly the behavior-split bridge suites—and imports `multi.seam` as an
installed-style package from the repository root:

```
python3 -m pytest multi/seam
python3 multi/verify/test_fixtures_offline.py
python3 deploy/lib/test_compose_persona.py
python3 multi/eval/test_graders.py

# lifecycle state, the operator console, and the Account Service guards
python3 deploy/lib/test_lifecycle.py
python3 deploy/lib/test_ironworks_cli.py
python3 deploy/account-intel/data/test_service_guards.py
```

The `__init__.py` package markers under `multi/` and `multi/seam/` and the `try: from . import
… / except ImportError: import …` shims in the seam modules are a **matched pair**: the markers
are what make pytest resolve seam tests from the repository root, and the shims are what keep them
runnable as bare scripts from inside `multi/seam/`. Remove a shim while the markers exist and
collection dies with `ModuleNotFoundError: No module named 'context_ingress'` — loud, but with
nothing to say why. Remove the markers instead and the tests still pass, from the root and from
the directory both, while every shim quietly becomes dead code that only bites whoever re-adds a
package marker. Change either half only with the other in view.

The console's own gate is worth running by hand when you touch anything it reads:

```
./deploy/ironworks --offline doctor        # exit 0 clear · 2 FAILED · 3 BLOCKED · 64 usage
./deploy/ironworks service validate
```

The proofs under `multi/verify/` that need a live instance are listed in that directory's
README; CI deliberately excludes them.

## How to propose a change

1. Open an issue first for anything non-trivial, so we can agree on the approach.
2. Fork, branch, and keep changes contained — one concern per PR.
3. Match the surrounding style; don't bundle unrelated refactors into a feature.
4. If you change a script or the provisioning flow, note how you tested it.

## Retiring something

Removal is a change like any other, and it has a known failure mode here: prose outlives code.
The doc-refs gate keeps catching the fallout after the fact, which means it is being run after
the removal instead of as part of it.

1. Remove the code, its tests, its config, its compose entries, and its provisioning steps in
   one change. A retired subtree that leaves its test file behind still reads as maintained.
2. Grep the whole tree for the path, not just `docs/`. Scripts cite paths too.
   `deploy/lib/test_doc_refs.py` gates markdown, service JSON, compose files, wrangler config,
   systemd units, and the libraries shell scripts `source` — the last by basename, so it sees a
   rename or a deletion but not a move. Everything else is still yours to find with `grep`: a
   path in a shell COMMENT, a Dockerfile `COPY`, an nginx config. Build-context and runtime
   paths are deliberately out of scope, because guessing at them is how a gate gets noisy
   enough that someone switches it off.
3. Update the documentation map in `README.md`. It owns the map; nothing else does.
4. Run `python3 deploy/lib/test_doc_refs.py`.
5. If a doc must keep naming a path this repo does not ship, add it to that file's
   `ALLOWED_MISSING` with a reason and the right kind — `"absent"` for retired or
   living-in-another-repo, `"untracked"` for deliberately gitignored. The entry is the record of
   the decision, which is what lets an *unexplained* dangling path stay a failure.

**The gate's blind spot bites hardest during exactly this work.** It resolves citations against
the index, because the index is what a clone gets. A file you deleted from your worktree but
have not unstaged still resolves clean while nobody on your machine can open it. `git status` is
what surfaces that; read it before concluding the removal landed.

Retiring something from the running fleet is not a repo edit. A deleted member's session can
still authenticate until it expires, so `multi/provision/deprovision.sh` records the tenant in
the residual-authority ledger — which `deploy/lib/lifecycle.py` owns, and which is where that
behaviour changes. Removal is done when the session cannot authenticate, not when the row
disappears. The console's `revocation.residual` check reports every outstanding session and
fails on the ones classified ACTIVE_RISK, naming the earliest expiry; a ledger entry that is
merely outstanding is reported, not failed, so the gate does not stay red until it expires.

Do not leave the old path running behind a flag "for now." Two implementations of one thing is
double the surface for every proof in `multi/verify/`.

## Instrumenting a new subsystem

If an operator cannot see it from the console, it is not operable. When you add a subsystem — a
service, a proxy, a background loop, a store — add its check to `deploy/ironworks` in the same
change, not after the first incident.

The output contract is fixed, and a new check does not get to reinterpret it:

- Every check is **CONFIG** (the configuration says the right thing) or **LIVE** (we asked the
  running system). Never merge the two into one word: configured is not running.
- Verdicts are PASS, FAIL, BLOCKED, SKIPPED. BLOCKED means *could not evaluate* and exits 3. A
  check that could not run must never report PASS — that is the whole reason the verdict exists.
- Ids are `<subsystem>.<claim>`, and the title states the claim, so a FAIL line reads as the
  guarantee that is broken rather than the thing that was looked at.
- Exit codes 0 / 2 / 3 / 64 are stable and scripts depend on them.
- Output passes through `multi/seam/redact.py`. Bind any new secret material with
  `bind_secrets` before anything derived from it can reach a title or a detail, and keep
  `--json` alone on stdout.
- **Delegate the judgement.** The console re-implements no rule: image provenance is
  `deploy/verify-pin.sh`, registry validity is the seam's loader, service definitions are
  `multi/services/`. A check carrying its own copy of a security rule is a second, quieter
  answer to that question.

Cover the new check in `deploy/lib/test_ironworks_cli.py` — that suite pins the properties that
make the console safe to paste into a ticket, and a check added outside it is untested exactly
where it is most dangerous. What happens when a check you added fires is the operator's
problem next: recovery procedures are in `deploy/README.md`, and the incident path is
`docs/INCIDENT_RESPONSE.md`.

## What lives where

`README.md` owns the documentation map and the component list, including what in this repository
is the product path and what is adjacent to it. Read it there; a second copy here would be the
one that goes stale.

## Secrets

Never commit real credentials, tokens, IPs, or private chat/group IDs. Secrets live in
gitignored `.env` files and `~/.agency/`; the repo ships only `.env.example` templates
and placeholders. If you add config, add a placeholder to the example, not a real value.

### Sourcing an env file: `set -a` or plain `.`

Both appear in this tree, and two scripts argue in comments for opposite conventions. They are
not a split to be unified — they answer different questions, and the discriminator is:

**Export (`set -a; . file; set +a`) only when a child process reads the value out of its own
environment. Otherwise plain `.`, and hand the values to children explicitly.**

Required, because inheritance is the transport:

- `deploy/account-intel/data/{dev-up,prod-up,migrate}.sh` — `docker compose` interpolates
  `${ACCOUNT_DB_PASSWORD:?}` from `docker-compose.yml` and reads it from its environment.
- `multi/serve/multi-backup.sh` (and `deploy/backup-laptop-agency.sh`, gitignored) — `restic`
  takes `RESTIC_REPOSITORY` / `RESTIC_PASSWORD` from the environment, nowhere else.
  `multi/serve/multi-watchdog.sh` needs it for the same reason, but only around one command.

Not required, so plain `.`: a script that copies each value into a shell variable and passes it
on via argv, `docker compose exec -e`, or an explicit `VAR=… child` prefix. Exporting there puts
every secret in the file into every child's environment for nothing — which is what
`deploy/account-intel/data/seed-real.sh` and `deploy/repoint-hostname.sh` (gitignored) say in
their comments.

**THE TRAP, and the reason this is written down rather than left to reading the script: a
variable consumed by inheritance is never named in the script that needs it.** `multi-backup.sh`
mentions `BACKUP_ALERT_*` and nothing else, so it reads like a plain-source candidate — while
the `RESTIC_*` pair its `restic backup` depends on appears nowhere in the file. Grepping the
script cannot tell you; check what the env file defines and what each child actually reads.
Unify in this direction and the failure is silent and total.

When only part of a script needs the exported form, scope it to a subshell rather than the whole
run — `multi/serve/multi-watchdog.sh` does this so `backup.env`'s `RESTIC_*` never reach the
alerting path.

## License

By contributing, you agree that your contributions are dual licensed under
[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), the same terms as the project.
