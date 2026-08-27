#!/usr/bin/env python3
"""Where operator state lives — one answer for the Python side of the operator tooling.

The shell side answers this in `deploy/lib/fleet.sh` (`FLEET_AGENCY_DIR`), and the comment
there explains why a half-honoured `AGENCY_DIR` is worse than no knob at all: setting it moved
the journal and the residual-authority ledger and left the registry, the identity map and the
staging tree behind, while provisioning printed the variable to the operator as though it
governed all of them.

The Python side had the same split with a sharper edge. `lifecycle.py` honoured `AGENCY_DIR`;
`identities.py` and `egress_status.py` expanded `~/.agency` directly. But `egress-control.sh`
WRITES the egress degraded mark under `FLEET_AGENCY_DIR`, and `egress_status.evaluate()` is the
only thing that reads it — so on a host with `AGENCY_DIR` set, a deliberate egress rollback was
written to one path and looked for at another, and `ironworks doctor` reported the boundary
still VERIFIED. A degraded security state nobody is tracking is the thing the mark exists to
prevent.

No third-party imports, and no state of its own: this resolves a path and nothing else.
"""
import os
import pathlib


def agency_dir(*parts):
    """The operator state directory, or a path inside it.

    Resolved at CALL time, never captured at import, so a test (or a script) that redirects
    `AGENCY_DIR` gets the redirect rather than whatever was set when the module first loaded.
    Matches `FLEET_AGENCY_DIR` in fleet.sh exactly — if one of them changes, both do."""
    root = pathlib.Path(os.environ.get("AGENCY_DIR") or os.path.expanduser("~/.agency"))
    return root.joinpath(*parts) if parts else root
