#!/usr/bin/env python3
"""The forbidden-destination policy: one loader, one schema, one fail-closed parse.

`deploy/egress/forbidden-destinations.json` names the addresses a contained runtime must not be
able to reach. The LIST was already shared — `probe-egress.sh` and the proof stack both read the
same file, which was the point of writing it down. The SCHEMA was not: three places each restated
what a valid entry looks like.

    deploy/egress/proof/proof_checks.py        the runtime loader — the only one that fail-closed
    deploy/lib/test_egress_destinations.py     a gate test, restating the rules
    deploy/egress/test_probe_attempts.py       a second gate test, restating a subset of them

Only the first was enforced anywhere a probe would notice, and the two tests asserted a schema by
describing it rather than by exercising the reader the probes actually use — so a loader that
grew stricter than the tests, or looser, would not have been caught by either.

WHY THE PARSE IS FAIL-CLOSED. An empty or malformed list makes every egress leg assert nothing
and pass: the probe iterates the destinations, finds none, and reports that none were reachable.
That is indistinguishable from containment. `set(...) != {...}` rather than a subset check for
the same reason — an entry carrying an extra key is an entry someone meant to mean something,
and silently ignoring it is how a destination stops being probed without anyone deciding it.

No third-party imports, and no state: this reads a file and validates it.
"""
import json
import pathlib

REQUIRED_KEYS = {"label", "host", "port"}


def destinations_path(repo_root=None):
    """The committed policy file. `repo_root` for callers that already resolved one."""
    root = pathlib.Path(repo_root) if repo_root else pathlib.Path(__file__).resolve().parents[2]
    return root / "deploy" / "egress" / "forbidden-destinations.json"


def load_forbidden_destinations(repo_root=None):
    """The validated destination list, or ValueError. Never an empty list.

    Raises rather than returning a default, because every caller is about to decide whether a
    security boundary holds and there is no safe default for "the policy is unreadable".
    """
    path = destinations_path(repo_root)
    doc = json.loads(path.read_text())
    destinations = doc.get("destinations") if isinstance(doc, dict) else None
    if not isinstance(destinations, list) or not destinations:
        raise ValueError(f"{path}: destinations must be a non-empty list")
    for destination in destinations:
        if (not isinstance(destination, dict)
                or set(destination) != REQUIRED_KEYS
                or not isinstance(destination["label"], str)
                or not isinstance(destination["host"], str)
                or not isinstance(destination["port"], int)
                or isinstance(destination["port"], bool)
                or not destination["label"] or not destination["host"]):
            raise ValueError(f"{path}: invalid destination {destination!r}")
    return destinations
