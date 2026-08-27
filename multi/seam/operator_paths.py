"""Operator-state paths for the serving package; AGENCY_DIR is the single root override.

A DELIBERATE SECOND COPY of `deploy/lib/agency_paths.agency_dir`, and it must stay one.
CLAUDE.md: the serving path in `multi/seam/` may not import `deploy/` — operator tooling imports
product modules, never the reverse — so the seam cannot share the operator-side helper, and this
is the whole of what would be shared. Consolidating the two is the obvious cleanup and it is the
one that inverts the dependency direction; both sides must keep the same behaviour instead
(`AGENCY_DIR`, else `~/.agency`), which `deploy/lib/test_identities.py` and the seam suites each
exercise from their own side.
"""
import os
from pathlib import Path


def agency_dir(*parts):
    root = Path(os.environ.get("AGENCY_DIR") or Path.home() / ".agency")
    return root.joinpath(*parts) if parts else root
