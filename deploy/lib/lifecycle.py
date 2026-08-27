#!/usr/bin/env python3
"""Tenant-lifecycle state: provisioning, deprovision scope, and residual authority.

Two small 0600 JSON files, one CLI, no third-party imports — so the shell scripts that drive
provisioning and deprovisioning share one implementation and one test suite instead of each
growing its own `python3 - <<PY` heredoc.

WHAT THE JOURNAL IS FOR. Provisioning creates authority in four places (an Account-Service org
token, a sealed IronClaw member, that member's confinement, the registry entry that makes the
tenant servable). Before this file, a failure halfway through left the operator reading a
half-finished terminal to work out what existed — and the script's own error text admitted it:
"Re-running mints a NEW org token and leaves the old entry registered." A journal makes the
partial state a fact on disk instead of a memory, which is what `--resume`, `--status` and a
truthful residual-authority report all need.

WHAT IT MUST NEVER HOLD. No token, ever. It records STAGE NAMES and OPAQUE IDENTIFIERS — the
slug, the org id, the sealed user id — every one of which is already non-secret and already
printed. A journal that stored the tokens would be a second copy of every client credential in
a file whose whole purpose is to survive a crash.

WHAT THE LEDGER IS FOR. Deleting a sealed member does NOT revoke its token on the pinned
runtime (measured: multi/verify/test_session_revocation.py). So a deprovisioned tenant leaves
an authenticating session behind for up to the session lifetime. That is a fact with an expiry
date, and something has to hold it between the deprovision run and the expiry, or "are any
credentials awaiting revocation?" is unanswerable. The ledger holds exactly that, with no token
material: slug, user id, when it was deleted, when the session can no longer authenticate.

    python3 deploy/lib/lifecycle.py journal set <slug> <stage> [k=v ...]
    python3 deploy/lib/lifecycle.py journal get <slug> [--json]
    python3 deploy/lib/lifecycle.py journal stage <slug>          # prints the stage, or ''
    python3 deploy/lib/lifecycle.py journal clear <slug>
    python3 deploy/lib/lifecycle.py teardown set <slug> <state> [k=v ...]
    python3 deploy/lib/lifecycle.py teardown get <slug> [--json]
    python3 deploy/lib/lifecycle.py residual add <slug> uid=<id> lifetime_days=<n> [k=v ...]
    python3 deploy/lib/lifecycle.py residual classify <slug> TEST_RESIDUAL <reason...>
    python3 deploy/lib/lifecycle.py residual drop <slug>
    python3 deploy/lib/lifecycle.py residual list [--json]
"""
import datetime
import json
import os
import pathlib
import sys
import tempfile

from agency_paths import agency_dir

# The stages, in order. `resume` restarts at the first one NOT reached; the order is the
# contract between provision.sh and this file, so it lives in one place.
STAGES = [
    "preflight_passed",     # every check that can be made before creating authority has passed
    "org_registered",       # Account-Service org token exists and resolves
    "data_seeded",          # the tenant's book is loaded (or there was none to load)
    "member_minted",        # a sealed IronClaw member exists
    "member_confined",      # that member's tool surface is confined and certified
    "staged",               # the registry entry is written to the staging dir, NOT yet live
    "smoke_passed",         # isolation + reachability gates passed against the real credentials
    "activated",            # the registry entry is live; the tenant is servable
]

# Words that must never appear as a journal field name. Belt to the braces of "we only pass ids":
# the CLI takes k=v pairs from shell, and a future caller passing `token=$TOK` would otherwise
# write a credential into a file that exists to survive crashes.
_FORBIDDEN_KEYS = ("token", "secret", "password", "key", "credential", "bearer")


def journal_path(slug):
    return agency_dir() / "provision-journal" / f"{slug}.json"


def ledger_path():
    return pathlib.Path(os.environ.get("RESIDUAL_LEDGER")
                        or agency_dir() / "residual-authority.json")


def teardown_path(slug):
    return agency_dir() / "deprovision" / f"{slug}.json"


def now_iso():
    return datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()


def write_private(path, doc):
    """Atomic write at 0600, mode set BEFORE any content exists.

    Same pattern as the bridge state file and the identities file: write-then-chmod publishes
    the content at the process umask for the window in between, and these files name every
    tenant that exists."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=f".{path.name}.", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(doc, f, indent=1, sort_keys=True)
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise


def read_json(path, default):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return default


def check_fields(fields):
    """Refuse anything that looks like a credential, by name. Raises ValueError."""
    for k, v in fields.items():
        low = k.lower()
        if any(w in low for w in _FORBIDDEN_KEYS):
            raise ValueError(
                f"refusing to journal field {k!r}: this file records stages and OPAQUE IDS "
                "only. A token here would be a second copy of a client credential, in a file "
                "whose whole purpose is to outlive a crash.")
        if not isinstance(v, (str, int)) or (isinstance(v, str) and len(v) > 200):
            raise ValueError(f"journal field {k!r} must be a short string or int")
    return fields


# ── journal ───────────────────────────────────────────────────────────────────────────

def journal_set(slug, stage, fields=None):
    if stage not in STAGES:
        raise ValueError(f"unknown stage {stage!r}; known: {', '.join(STAGES)}")
    fields = check_fields(dict(fields or {}))
    p = journal_path(slug)
    doc = read_json(p, {})
    doc.setdefault("slug", slug)
    doc.setdefault("started_at", now_iso())
    doc.setdefault("history", [])
    doc["stage"] = stage
    doc["updated_at"] = now_iso()
    doc.update(fields)
    doc["history"].append({"stage": stage, "at": doc["updated_at"]})
    write_private(p, doc)
    return doc


def journal_get(slug):
    return read_json(journal_path(slug), {})


def journal_stage(slug):
    return journal_get(slug).get("stage", "")


def journal_reached(slug, stage):
    """Has this provisioning run already got past `stage`?"""
    cur = journal_stage(slug)
    if cur not in STAGES or stage not in STAGES:
        return False
    return STAGES.index(cur) >= STAGES.index(stage)


def journal_clear(slug):
    p = journal_path(slug)
    try:
        p.unlink()
        return True
    except FileNotFoundError:
        return False


# ── deprovision scope receipt ───────────────────────────────────────────────────────────────────────────────

TEARDOWN_STATES = ("authenticated", "account_revoked", "complete")


def teardown_set(slug, state, fields=None):
    """Persist only non-secret evidence needed to resume a destructive teardown."""
    if state not in TEARDOWN_STATES:
        raise ValueError(f"unknown teardown state {state!r}")
    fields = check_fields(dict(fields or {}))
    p = teardown_path(slug)
    doc = read_json(p, {})
    doc.update(fields)
    doc.update({"slug": slug, "state": state, "updated_at": now_iso()})
    doc.setdefault("started_at", doc["updated_at"])
    write_private(p, doc)
    return doc


def teardown_get(slug):
    return read_json(teardown_path(slug), {})


# ── residual-authority classification ─────────────────────────────────────────────────
# A residual session is a FACT: a token that still authenticates. What varies is what that
# fact MEANS, and conflating the two is how a security gate becomes noise. A credential minted
# by a self-test on an operator laptop and one belonging to a departed client are both "still
# valid"; only one of them should stop a release.
#
# So the validity is measured and the MEANING is declared — never inferred, never silent:
#
#   ACTIVE_RISK             the default. Authority that matters. Blocks promotion.
#   TEST_RESIDUAL           a synthetic credential from a proof or self-test, explicitly waived
#                           by an operator WITH A REASON. Stays visible, stays in the ledger,
#                           keeps its real expiry — but does not block promotion.
#   EXPIRED                 derived from the clock. Never declared.
#   REVOKED                 only ever set by a probe that MEASURED a rejection. A human cannot
#                           assert this; that is the whole point of the deprovisioning gate.
#
# What this deliberately does NOT do: make the gate green by forgetting. `residual drop` still
# exists for a genuinely mistaken entry, but the honest move for a real-but-immaterial
# credential is to classify it, so the ledger keeps saying the token authenticates until 2027
# while the release gate stops treating a laptop self-test as client authority.
ACTIVE_RISK, TEST_RESIDUAL, EXPIRED, REVOKED = (
    "ACTIVE_RISK", "TEST_RESIDUAL", "EXPIRED", "REVOKED")
# REVOKED is absent on purpose: it is not operator-settable.
OPERATOR_SETTABLE = (ACTIVE_RISK, TEST_RESIDUAL)


# ── residual-authority ledger ─────────────────────────────────────────────────────────

def residual_add(slug, fields):
    """Record that a deleted tenant's member session can still authenticate until its expiry.

    `lifetime_days` is what the runtime mints (365 at the pinned rev, a Rust constant with no
    config path — see SECURITY.md). It is recorded per entry rather than assumed
    globally, so an entry stays readable after the runtime changes."""
    fields = check_fields(dict(fields))
    days = int(fields.pop("lifetime_days", 0) or 0)
    minted = fields.pop("minted_at", "") or ""
    base = datetime.datetime.now(datetime.timezone.utc)
    if minted:
        try:
            base = datetime.datetime.fromisoformat(minted)
        except ValueError:
            pass
    expires = base + datetime.timedelta(days=days)
    doc = read_json(ledger_path(), {})
    prior = doc.get(slug) or {}
    # THE EXPIRY IS A FACT ABOUT A TOKEN, NOT ABOUT THIS CALL. Recomputing it made the recorded
    # window move forward every time the entry was re-recorded, which turns an audit record into
    # a moving target — the exact wording deprovision.sh:311-320 uses for why it must not
    # happen. That script enforces it with a shell-side ALREADY_ABSENT flag, so any
    # partial-failure re-run that still finds the member present, or any second caller of this
    # module, extended the recorded expiry of a token whose real expiry did not move. The token
    # was minted once; only the first record can describe when it dies.
    doc[slug] = {**fields, "slug": slug,
                 "deleted_at": prior.get("deleted_at") or now_iso(),
                 "session_lifetime_days": prior.get("session_lifetime_days", days),
                 "expires_at": prior.get("expires_at")
                 or expires.replace(microsecond=0).isoformat(),
                 "expires_at_epoch": prior.get("expires_at_epoch", int(expires.timestamp())),
                 # A re-recorded entry keeps any classification an operator already made, so a
                 # repeated deprovision does not silently re-arm a considered waiver.
                 "classification": prior.get("classification", ACTIVE_RISK),
                 "waiver_reason": prior.get("waiver_reason"),
                 "classified_at": prior.get("classified_at")}
    write_private(ledger_path(), doc)
    return doc[slug]


def residual_classify(slug, classification, reason):
    """Declare what a residual session MEANS. Never what it is.

    A reason is mandatory and recorded: a waiver nobody can account for later is the same as no
    waiver. `REVOKED` is rejected outright — only a probe that measured a rejection may set
    that, or the deprovisioning gate would become an honour system."""
    if classification not in OPERATOR_SETTABLE:
        raise ValueError(
            f"{classification!r} is not operator-settable (allowed: {', '.join(OPERATOR_SETTABLE)}). "
            "EXPIRED is derived from the clock, and REVOKED may only be set by a probe that "
            "measured the token being refused — asserting it by hand would make the "
            "deprovisioning gate an honour system.")
    if not (reason or "").strip():
        raise ValueError("a classification needs a REASON; an unaccountable waiver is no waiver")
    doc = read_json(ledger_path(), {})
    if slug not in doc:
        raise ValueError(f"no residual-authority entry for {slug!r}")
    doc[slug]["classification"] = classification
    doc[slug]["waiver_reason"] = reason.strip()[:300]
    doc[slug]["classified_at"] = now_iso()
    write_private(ledger_path(), doc)
    return doc[slug]


def residual_drop(slug):
    doc = read_json(ledger_path(), {})
    if slug in doc:
        del doc[slug]
        write_private(ledger_path(), doc)
        return True
    return False


def residual_list():
    """(outstanding, expired) — split on the wall clock. `outstanding` is every session that
    STILL AUTHENTICATES, whatever it has been classified as: the ledger never stops reporting a
    live token. Callers that care about promotion filter on classification themselves."""
    doc = read_json(ledger_path(), {})
    now = datetime.datetime.now(datetime.timezone.utc).timestamp()
    out, done = {}, {}
    for slug, e in doc.items():
        (out if (e.get("expires_at_epoch") or 0) > now else done)[slug] = e
    return out, done


def residual_split(outstanding=None):
    """(blocking, waived) among still-valid sessions. Only ACTIVE_RISK blocks."""
    out = outstanding if outstanding is not None else residual_list()[0]
    blocking = {s: e for s, e in out.items()
                if e.get("classification", ACTIVE_RISK) == ACTIVE_RISK}
    waived = {s: e for s, e in out.items() if s not in blocking}
    return blocking, waived


# ── CLI ───────────────────────────────────────────────────────────────────────────────

def _kv(args):
    fields = {}
    for a in args:
        if "=" not in a:
            raise SystemExit(f"!! expected key=value, got {a!r}")
        k, v = a.split("=", 1)
        fields[k] = v
    return fields


def main(argv):
    if not argv:
        print(__doc__)
        return 64
    group, *rest = argv
    as_json = "--json" in rest
    rest = [a for a in rest if a != "--json"]
    try:
        if group == "journal":
            action, *rest = rest
            if action == "set":
                slug, stage, *kv = rest
                doc = journal_set(slug, stage, _kv(kv))
                print(json.dumps(doc) if as_json else f"journal {slug}: {stage}")
            elif action == "get":
                doc = journal_get(rest[0])
                print(json.dumps(doc, indent=1) if as_json else
                      "\n".join(f"{k}: {v}" for k, v in sorted(doc.items()) if k != "history"))
            elif action == "stage":
                print(journal_stage(rest[0]))
            elif action == "reached":
                return 0 if journal_reached(rest[0], rest[1]) else 1
            elif action == "clear":
                print("cleared" if journal_clear(rest[0]) else "no journal")
            else:
                raise SystemExit(f"!! unknown journal action {action!r}")
        elif group == "teardown":
            action, *rest = rest
            if action == "set":
                slug, state, *kv = rest
                doc = teardown_set(slug, state, _kv(kv))
                print(json.dumps(doc) if as_json else f"teardown {slug}: {state}")
            elif action == "get":
                doc = teardown_get(rest[0])
                print(json.dumps(doc) if as_json else
                      "\n".join(f"{k}: {v}" for k, v in sorted(doc.items())))
            else:
                raise SystemExit(f"!! unknown teardown action {action!r}")
        elif group == "residual":
            action, *rest = rest
            if action == "add":
                slug, *kv = rest
                e = residual_add(slug, _kv(kv))
                print(json.dumps(e) if as_json else
                      f"residual authority recorded for {slug}: expires {e['expires_at']}")
            elif action == "has":
                # Exit-code query, deliberately NOT `residual list | grep`. That form is a trap
                # under `set -o pipefail`: `list` exits 2 while authority is outstanding, so the
                # pipeline reports 2 whatever grep found, and a caller reading it as a boolean
                # concludes the opposite of the truth. This one has no output to parse.
                out, _ = residual_list()
                return 0 if rest[0] in out else 1
            elif action == "classify":
                slug, classification, *rest = rest
                reason = " ".join(rest).lstrip("-").lstrip() if rest else ""
                if reason.startswith("reason "):
                    reason = reason[len("reason "):]
                e = residual_classify(slug, classification, reason)
                print(json.dumps(e) if as_json else
                      f"{slug}: classified {e['classification']} — the token still "
                      f"authenticates until {e['expires_at']}")
            elif action == "drop":
                print("dropped" if residual_drop(rest[0]) else "not present")
            elif action == "list":
                out, done = residual_list()
                if as_json:
                    print(json.dumps({"outstanding": out, "expired": done}, indent=1))
                else:
                    blocking, waived = residual_split(out)
                    for slug, e in sorted(blocking.items()):
                        print(f"OUTSTANDING {slug}  expires {e['expires_at']}  "
                              f"user {e.get('uid', '?')}  ACTIVE_RISK")
                    for slug, e in sorted(waived.items()):
                        # Still printed, still with its real expiry. A waiver changes what the
                        # release gate does, never what the ledger says.
                        print(f"OUTSTANDING {slug}  expires {e['expires_at']}  "
                              f"user {e.get('uid', '?')}  {e.get('classification')} "
                              f"(waived: {e.get('waiver_reason')})")
                    for slug in sorted(done):
                        print(f"expired     {slug}")
                return 2 if residual_split(out)[0] else 0
            else:
                raise SystemExit(f"!! unknown residual action {action!r}")
        else:
            raise SystemExit(f"!! unknown group {group!r}")
    except ValueError as e:
        print(f"!! {e}", file=sys.stderr)
        return 64
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
