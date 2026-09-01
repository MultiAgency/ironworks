#!/usr/bin/env python3
"""Would the bridge load this registry, with the staged tenant in it?

WHY THIS EXISTS. `multi/provision/provision.sh`'s last smoke leg is the final gate before a
tenant becomes servable, and it asked a question one tenant cannot answer. It copied the staged
`.env` alone into a temp directory and called `load_clients` on it, under a comment claiming to
exercise "credential uniqueness". `registry.load_clients` detects a reused `ACCOUNT_TOKEN`,
`IRONCLAW_TOKEN`, `TELEGRAM_GROUP_ID` or slug by comparing entries against EACH OTHER —
`seen_accounts`, `seen_tokens`, `seen_groups` — so a directory holding one entry passes every one
of them vacuously.

The reachable consequence, measured against the shipped scripts: `provision.sh` step 1 reuses an
organization's existing Account-Service credential when the identity map holds exactly one
(the supported path for a canonical dataset), and nothing before this leg compares that
credential against the tenants already in the registry. So provisioning a second slug onto an
org that already has one passed all five legs, activated by the atomic `mv`, and then
`registry.py`'s D-091 rule refused THE WHOLE REGISTRY at the next bridge start — every tenant,
not just the new one. Provisioning exited 0.

WHAT THIS ASKS INSTEAD. The real question is not "does this file parse" but "does the registry
the bridge will actually read still load once this tenant is in it". That needs the live entries
present, so the cross-entry rules have something to compare against.

TWO PHASES, BECAUSE THE ANSWER "NO" HAS TWO MEANINGS. A registry that was already broken before
this run must never be reported as a conflict this tenant introduced — the operator would tear
down a perfectly good tenant chasing someone else's defect. So:

    phase A   live entries alone      -> raises: REGISTRY_INVALID. Not this tenant's doing.
    phase B   live entries + staged   -> raises: STAGED_CONFLICT. This tenant is the conflict.

Phase A over an empty directory returns `{}` without raising, which is the correct reading for
the first tenant on a host: there is nothing to conflict with yet.

SYMLINKS, NOT COPIES. The previous inline version used `shutil.copy`, which does not preserve
mode — every tenant's `IRONCLAW_TOKEN` and `ACCOUNT_TOKEN` landed in a temp file at the process
umask. Symlinks avoid materialising a second copy of a credential at all: `read_text()` follows
them, and `mkdtemp` is 0700. They are also the only form that keeps a live `GUIDANCE_FILE=` key
validating, because `registry._canonical_guidance_path` resolves the sibling path and a symlink
resolves back to the real file where a copy would not.

This module is operator tooling and imports the product's loader deliberately: the whole point is
to ask the question with the SAME code the bridge runs, never a second implementation of it.
`CLAUDE.md` permits that direction (`deploy/` may import `multi/`, never the reverse).
"""
import os
import pathlib
import sys
import tempfile

OK, STAGED_CONFLICT, REGISTRY_INVALID, USAGE = "OK", "STAGED_CONFLICT", "REGISTRY_INVALID", "USAGE"
EXIT = {OK: 0, STAGED_CONFLICT: 2, REGISTRY_INVALID: 3, USAGE: 64}


def _load_clients():
    """The bridge's own loader, imported late so importing this module costs nothing.

    `context_ingress`, not `registry`, because that is the module the bridge calls and it reads
    `ACCOUNT_BASE`/`TURN_BUDGET_SECONDS` at import — asking the question through a different
    door would be asking a different question.
    """
    seam = pathlib.Path(__file__).resolve().parents[2] / "multi" / "seam"
    if str(seam) not in sys.path:
        sys.path.insert(0, str(seam))
    import context_ingress
    return context_ingress.load_clients


def _mirror(dest, *sources):
    """Symlink every `*.env` and `*.guidance.md` in `sources` into `dest`, first name wins.

    First-name-wins matters for the staged entry: it lives under `.staging/<slug>.env` while its
    guidance is already at the live `<slug>.guidance.md`, so the live directory legitimately
    supplies half of the staged tenant's pair. A second link at the same name would raise.
    """
    dest = pathlib.Path(dest)
    for source in sources:
        source = pathlib.Path(source)
        if not source.is_dir():
            continue
        for entry in sorted(source.iterdir()):
            if entry.suffix != ".env" and not entry.name.endswith(".guidance.md"):
                continue
            link = dest / entry.name
            if not link.exists():
                os.symlink(entry.resolve(), link)
    return dest


def _readable(message, mirror):
    """Rewrite mirror paths in a loader message back to the files the operator can open.

    `load_clients` names the offending file, which is the whole value of its message — and every
    path in it points into a temp directory that is gone by the time anyone reads it. Without
    this the operator is told the rule and then handed a path they cannot act on.
    """
    mirror = pathlib.Path(mirror)
    for link in sorted(mirror.iterdir()):
        if link.is_symlink():
            message = message.replace(str(link), os.readlink(link))
    return message.replace(f"{mirror}/", "").replace(str(mirror), "the registry")


def validate(live_dir, staged_env=None, guidance=None):
    """Return `(verdict, detail)` for the registry the bridge would read.

    `live_dir` is the real `CLIENTS_DIR`. `staged_env` is the not-yet-servable entry under
    `.staging/`; omit it to ask only whether the live registry loads today.
    """
    load_clients = _load_clients()
    live_dir = pathlib.Path(live_dir)

    with tempfile.TemporaryDirectory() as d:
        _mirror(d, live_dir)
        try:
            live = load_clients(d)
        except Exception as e:
            return REGISTRY_INVALID, _readable(f"{type(e).__name__}: {e}", d)

    if staged_env is None:
        return OK, f"{len(live)} live tenant(s) load"

    staged_env = pathlib.Path(staged_env)
    if not staged_env.is_file():
        return USAGE, f"no staged registry entry at {staged_env}"
    slug = staged_env.name[:-len(".env")]

    with tempfile.TemporaryDirectory() as d:
        # The staged entry FIRST, so its link wins its own name if a live file somehow shares it
        # — and so the loader's alphabetical walk cannot decide which of a colliding pair gets
        # named as the offender purely by where the mirror happened to put it.
        os.symlink(staged_env.resolve(), pathlib.Path(d) / staged_env.name)
        if guidance is not None:
            g = pathlib.Path(guidance)
            link = pathlib.Path(d) / f"{slug}.guidance.md"
            if g.is_file() and not link.exists():
                os.symlink(g.resolve(), link)
        _mirror(d, live_dir)
        try:
            clients = load_clients(d)
        except Exception as e:
            return STAGED_CONFLICT, _readable(f"{type(e).__name__}: {e}", d)

    c = clients.get(slug)
    if c is None:
        # Not reachable through provisioning (the file is named for the slug), but a loader that
        # returned no entry for the tenant this gate exists to admit must never read as a pass.
        return STAGED_CONFLICT, (
            f"the registry loaded but holds no tenant {slug!r} — "
            f"loaded: {', '.join(sorted(clients)) or '(none)'}")
    return OK, f"{c.slug} service={c.service_id} persona={c.persona_sha} (with {len(live)} live tenant(s))"


def main(argv):
    if len(argv) < 2:
        print("usage: registry_validation.py <live-clients-dir> [staged.env] [guidance.md]",
              file=sys.stderr)
        return EXIT[USAGE]
    verdict, detail = validate(*argv[1:4])
    stream = sys.stdout if verdict == OK else sys.stderr
    print(detail if verdict == OK else f"{verdict}: {detail}", file=stream)
    return EXIT[verdict]


if __name__ == "__main__":
    sys.exit(main(sys.argv))
