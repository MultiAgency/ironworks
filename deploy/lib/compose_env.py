#!/usr/bin/env python3
"""The environment `docker compose config` needs to interpolate this repository's stacks.

DERIVED FROM THE FILES, NOT LISTED. Two places needed this set and both hand-maintained it:
`deploy/run-quality.py` named nine variables, `deploy/lib/egress_status.overlay_configured`
named five, and they had ALREADY diverged — the four `run-quality` has that the other does not
include `ACCOUNT_DB_PASSWORD`, so adding a `${ACCOUNT_DB_PASSWORD:?}` to the egress overlay would
have broken `overlay_configured` (reporting the boundary unconfigured) while the quality gate
stayed green.

A list that has to match a set of files is a list that will stop matching them. Scanning for
`${NAME:?...}` — the shape that makes compose REFUSE to interpolate — cannot drift, because the
thing it reads is the thing it has to satisfy.

`${NAME:-default}` is deliberately NOT collected: compose supplies the default itself, and
setting those would validate a configuration nobody runs.
"""
import pathlib
import re

# `${NAME:?...}` and `${NAME?...}` — the two spellings compose treats as "required".
_REQUIRED = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*):?\?")

PLACEHOLDER = "compose-config-placeholder"


def required_vars(*paths):
    """Every variable the given compose files refuse to interpolate without. Sorted."""
    names = set()
    for path in paths:
        try:
            text = pathlib.Path(path).read_text()
        except OSError:
            continue          # a missing file is the caller's finding to report, not ours
        names.update(_REQUIRED.findall(text))
    return sorted(names)


def placeholder_env(*paths, base=None):
    """`base` (default: nothing) plus a placeholder for every required variable.

    The values are meaningless on purpose: `config` only has to INTERPOLATE. Anything that needs
    a real secret is not a config check.
    """
    env = dict(base or {})
    env.update({name: PLACEHOLDER for name in required_vars(*paths)})
    return env
