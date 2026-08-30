#!/usr/bin/env python3
"""How operator state files are written — one answer for the Python side of the operator tooling.

`agency_paths.py` owns WHERE this state lives; this owns HOW it is published. They are separate
modules because that one is deliberately copied into the serving path
(`multi/seam/operator_paths.py` says so in its header) and stays tiny to keep the copy safe,
while nothing in the seam writes operator state at all.

WHY THIS EXISTS. Three files in this directory wrote a private JSON document atomically, and all
three docstrings made the same argument for the same ordering:

    identities.py  — the identity map: every client's Account-Service org token
    lifecycle.py   — the provisioning journal and the residual-authority ledger
    egress_status.py — the egress verification stamp and the degraded mark

The argument was right and two of the three implemented it. `egress_status._record` did
`write_text` then `os.chmod` — the exact write-then-chmod its own docstring named as the defect —
so the boundary state file existed at the process umask for the window in between, on a host
where that umask is whatever the operator's shell set. Its test asserted the FINAL mode, which a
write-then-chmod also satisfies, so the gate could not see it. It also used a fixed
`.tmp` suffix where the other two used `mkstemp`, so two concurrent writers collided there and
nowhere else.

The two correct copies had themselves drifted: `lifecycle`'s cleanup handler used a bare
`os.unlink`, which raises `FileNotFoundError` FROM INSIDE the `except BaseException` and replaces
the original exception with a misleading one. `identities`' used `missing_ok=True`. This keeps
the hardened form.

No state of its own, and no third-party imports.
"""
import json
import os
import pathlib
import tempfile


def write_private(path, doc):
    """Publish `doc` as JSON at `path`, atomically and never readable by anyone else.

    mkstemp + fchmod + replace, IN THAT ORDER. The order is the whole point: a write-then-chmod
    publishes the content at the process umask for the window in between, and every caller of
    this function is writing either a credential map or the state an operator reads to decide
    whether a security boundary still holds. `os.replace` is atomic within a filesystem, so a
    concurrent reader sees the old document or the new one, never half of one.

    `mkstemp` in the DESTINATION directory, not a fixed name: same filesystem (so the replace is
    a rename rather than a copy), unique (so two writers cannot collide), and created 0600 by
    mkstemp itself before `fchmod` narrows it further.

    `indent=1, sort_keys=True` for every caller. These files are read by operators and diffed
    across runs; insertion order is not information any of them carry.
    """
    p = pathlib.Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(p.parent), prefix=f".{p.name}.", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(doc, f, indent=1, sort_keys=True)
        os.replace(tmp, p)
    except BaseException:
        # `missing_ok`, not a bare unlink: if the failure came after `os.replace` the temp file
        # is already gone, and a FileNotFoundError raised here would REPLACE the exception that
        # actually stopped the write.
        pathlib.Path(tmp).unlink(missing_ok=True)
        raise
    return p
