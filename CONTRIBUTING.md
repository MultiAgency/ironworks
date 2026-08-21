# Contributing to ironworks

Thanks for your interest. ironworks is the config + tooling for running isolated AI
agents on the **official, unmodified [ironclaw](https://github.com/nearai/ironclaw)
binary**. Contributions that keep it that way are very welcome.

## The one rule

**Never fork or patch ironclaw.** Everything here is config, data (personas), and
scripts that run *around* the stock binary. A change that requires editing ironclaw's
source belongs upstream in ironclaw, not here. If you hit a wall that seems to need a
core change, open an issue describing it — often there's a seam that avoids the fork.

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

## Running the CI gates locally

Run the gates the way CI runs them, not a variant. Copy these verbatim — the two commands
below have traps that make a laxer local check pass while CI fails, and both have caught
real breakage in this repo:

```
# The file set CI uses: tracked PLUS untracked-not-ignored. `git ls-files` alone is
# index-only, so a subtree nobody has `git add`ed yet is invisible to every check.
git ls-files --cached --others --exclude-standard -z -- '*.sh' > /tmp/sh-files.z

xargs -0 -I{} bash -n {} < /tmp/sh-files.z     # syntax
xargs -0 shellcheck < /tmp/sh-files.z          # NO -S flag. See below.

ruff check . deploy/lib/compose-persona --select F,E9

# JS: --input-type=module is load-bearing. See below.
node --input-type=module --check < deploy/secretary/worker/worker.js
```

**Do not add `-S warning` (or any `-S`) to shellcheck.** CI runs it bare, and shellcheck exits
non-zero on *info*-level findings. A local `-S warning` hides those, so your file reads clean
every time you check it and fails the gate anyway. Your local check and the gate must ask the
same question, and the gate's is the stricter one.

**Do not trust a bare `node --check FILE.js`.** It exits 0 on a syntactically broken file when
that file contains an `import` — the ESM-detection path skips the check. Reading the file on
stdin with `--input-type=module` fails correctly on both. This is not hypothetical; see the
comment in `.github/workflows/seam-ci.yml`.

**Tests need no venv and no pytest.** Every test file is stdlib-only with a self-discovering
`__main__` runner, so `python3 <file>` works. CI installs pytest only as a convenient runner.
The `multi/seam` tests import sibling modules by bare name, so run them from inside that
directory:

```
(cd multi/seam && python3 test_ingress_fixes.py)
python3 multi/verify/test_fixtures_offline.py
python3 deploy/lib/test_compose_persona.py
python3 multi/eval/test_graders.py
```

The proofs under `multi/verify/` that need a live instance are listed in that directory's
README; CI deliberately excludes them.

## How to propose a change

1. Open an issue first for anything non-trivial, so we can agree on the approach.
2. Fork, branch, and keep changes contained — one concern per PR.
3. Match the surrounding style; don't bundle unrelated refactors into a feature.
4. If you change a script or the provisioning flow, note how you tested it.

## What lives where

- `agent/identity/` — personas (system prompts); `MULTI.template.md` is the client-agent template.
- `deploy/` — provisioning + health tooling for the fleet, the Account Service, and the secretary funnel.
- `multi/` — the multi-tenant client product: the seam, the instance, per-client provisioning, isolation proofs.
- `skills/` — agent skills.
- `docs/` — architecture, the agent spec, and design notes.

## Secrets

Never commit real credentials, tokens, IPs, or private chat/group IDs. Secrets live in
gitignored `.env` files and `~/.agency/`; the repo ships only `.env.example` templates
and placeholders. If you add config, add a placeholder to the example, not a real value.

## License

By contributing, you agree that your contributions are dual licensed under
[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), the same terms as the project.
