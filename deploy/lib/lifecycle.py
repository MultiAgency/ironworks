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
    python3 deploy/lib/lifecycle.py journal reached <slug> <stage>    # exit 0 = yes, 1 = no
    python3 deploy/lib/lifecycle.py journal clear <slug>
    python3 deploy/lib/lifecycle.py teardown set <slug> <state> [k=v ...]
    python3 deploy/lib/lifecycle.py teardown get <slug> [--json]
    python3 deploy/lib/lifecycle.py residual add <slug> uid=<id> lifetime_days=<n> [k=v ...]
    python3 deploy/lib/lifecycle.py residual has <slug>               # exit 0 = yes, 1 = no
    python3 deploy/lib/lifecycle.py residual classify <slug> TEST_RESIDUAL <reason...>
    python3 deploy/lib/lifecycle.py residual drop <slug>
    python3 deploy/lib/lifecycle.py residual list [--json]

`journal reached` and `residual has` were the two commands this list omitted, and they are the
two the shell actually branches on — `provision.sh` calls the first three times to decide whether
to skip creating authority, `deprovision.sh` calls the second. Both were documented only in the
code that implements them.

EXIT CODES, because two callers read them as booleans:

    0   success, or the query is TRUE
    1   the query is FALSE — `journal reached` / `residual has`, and nothing else
    2   `residual list` found outstanding ACTIVE_RISK authority
   64   usage: an unknown group, action, argument or field
"""
import argparse
import datetime
import json
import os
import pathlib
import sys

from agency_paths import agency_dir
from private_state import write_private

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
    # A MALFORMED `minted_at` USED TO BE SWALLOWED, and the swallow contradicted the paragraph
    # below it. `except ValueError: pass` left `base` at "now", so the entry recorded an expiry
    # computed from the clock — a fabricated fact, in the field the whole ledger exists to state,
    # with no diagnostic anywhere. The `prior.get("expires_at")` guard below does not catch it:
    # that protects RE-records, and this is how the FIRST one goes wrong.
    #
    # Naive is refused for the same reason rather than assumed to be UTC. `expires.timestamp()`
    # reads a naive value in LOCAL time, so the same input would yield a different
    # `expires_at_epoch` on an operator laptop than on the host, while `expires_at` recorded no
    # offset at all next to entries that all carry `+00:00`.
    base = datetime.datetime.now(datetime.timezone.utc)
    if minted:
        try:
            base = datetime.datetime.fromisoformat(minted)
        except ValueError as e:
            raise ValueError(
                f"minted_at={minted!r} is not an ISO-8601 timestamp ({e}). It is the only input "
                "to a token's recorded expiry, so a value this file cannot read must stop the "
                "record rather than silently date it from the clock.") from e
        if base.tzinfo is None:
            raise ValueError(
                f"minted_at={minted!r} carries no UTC offset. Every other timestamp in this "
                "ledger does, and a naive value is read in local time when the epoch is "
                "computed — give it an offset (…Z or +00:00).")
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
            # ValueError, not SystemExit: `main` maps ValueError to 64, where a bare SystemExit
            # with a string exits 1 — the code a caller reads as "no, that query is false".
            raise ValueError(f"expected key=value, got {a!r}")
        k, v = a.split("=", 1)
        fields[k] = v
    return fields


# ── CLI dispatch ──────────────────────────────────────────────────────────────────────
#
# THE EXIT CODES ARE THE INTERFACE. `deprovision.sh` runs `residual has <slug>` and reads the
# code as a boolean; `provision.sh` runs `journal reached`. The console and the release gate read
# `residual list`. So:
#
#     0   the thing succeeded, or the query is TRUE
#     1   the query is FALSE — and NOTHING ELSE. `residual has` on an absent slug, and only that
#     2   `residual list` found outstanding ACTIVE_RISK authority
#    64   usage: a group, action, argument or field this tool cannot accept
#
# EXIT 1 USED TO MEAN THREE DIFFERENT THINGS, and that is what this rewrite is for. It meant the
# boolean above; it meant "unknown group"/"unknown action"/"expected key=value"; and it meant an
# uncaught `IndexError` traceback from `rest[0]` on six paths where an argument was simply
# missing. A caller writing the obvious `if lifecycle.py residual has "$SLUG"; then` could not
# tell "no residual authority" from "you typo'd the subcommand" — the typo took the false branch
# silently. Usage errors are 64 now, and 64 alone.
#
# argparse REPLACES a hand-rolled three-level if/elif chain that unpacked positionally
# (`slug, stage, *kv = rest`), so a missing argument surfaced as `!! not enough values to unpack
# (expected at least 2, got 0)` — a Python internal, presented to an operator as usage text.
# `deploy/ironworks:1719` already dispatches this way; this is that shape.


class _Usage(argparse.ArgumentParser):
    """argparse exits 2 on a parse error, and 2 is taken — it means outstanding authority.

    `raise SystemExit(f"!! {msg}")` is the obvious override and is WRONG for exactly the reason
    this rewrite exists: SystemExit with a STRING argument prints it and exits **1**, the code a
    caller reads as "that query is false". Print, then exit with the number."""

    def error(self, message):
        self.print_usage(sys.stderr)
        print(f"!! {message}", file=sys.stderr)
        raise SystemExit(64)


def _parser():
    ap = _Usage(prog="lifecycle.py", description=__doc__.strip().splitlines()[0],
                formatter_class=argparse.RawDescriptionHelpFormatter)
    # `--json` is accepted before OR after the subcommand, as it always was: the callers in
    # multi/provision/ write it in both positions.
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--json", action="store_true", default=argparse.SUPPRESS)
    groups = ap.add_subparsers(dest="group")

    def sub(parent, name, *positionals, **kw):
        p = parent.add_parser(name, parents=[common], **kw)
        for spec in positionals:
            p.add_argument(spec) if isinstance(spec, str) else p.add_argument(*spec[0], **spec[1])
        return p

    journal = groups.add_parser("journal", help="provisioning progress").add_subparsers(dest="act")
    sub(journal, "set", "slug", "stage", (["kv"], {"nargs": "*", "metavar": "key=value"}))
    sub(journal, "get", "slug")
    sub(journal, "stage", "slug")
    # `choices` HERE AND NOWHERE ELSE IN THIS GROUP. `journal_reached` answers False for a stage
    # it does not know, which is right for the library (an empty journal has no stage) and wrong
    # for the CLI: `provision.sh` asks `journal reached <slug> org_registered` to decide whether
    # to SKIP creating an org token, so a mistyped stage name reads as "not reached" and mints a
    # second live credential — the credential accumulation this journal exists to prevent. The
    # three live call sites pass hardcoded valid stages, so this can only ever catch a typo, and
    # a typo is exactly what it needs to catch.
    #
    # `journal set` needs no `choices`: `journal_set` already raises ValueError on an unknown
    # stage, and `main` maps that to 64.
    sub(journal, "reached", "slug", (["stage"], {"choices": STAGES}))
    sub(journal, "clear", "slug")

    teardown = groups.add_parser("teardown", help="teardown receipts").add_subparsers(dest="act")
    sub(teardown, "set", "slug", "state", (["kv"], {"nargs": "*", "metavar": "key=value"}))
    sub(teardown, "get", "slug")

    residual = groups.add_parser("residual",
                                 help="residual-authority ledger").add_subparsers(dest="act")
    sub(residual, "add", "slug", (["kv"], {"nargs": "*", "metavar": "key=value"}))
    sub(residual, "has", "slug")
    sub(residual, "classify", "slug", "classification",
        (["reason"], {"nargs": "*", "metavar": "word"}))
    sub(residual, "drop", "slug")
    sub(residual, "list")
    return ap


def _cmd_journal(a, out):
    if a.act == "set":
        doc = journal_set(a.slug, a.stage, _kv(a.kv))
        out(json.dumps(doc) if a.json else f"journal {a.slug}: {a.stage}")
    elif a.act == "get":
        doc = journal_get(a.slug)
        out(json.dumps(doc, indent=1) if a.json else
            "\n".join(f"{k}: {v}" for k, v in sorted(doc.items()) if k != "history"))
    elif a.act == "stage":
        out(journal_stage(a.slug))
    elif a.act == "reached":
        return 0 if journal_reached(a.slug, a.stage) else 1
    elif a.act == "clear":
        out("cleared" if journal_clear(a.slug) else "no journal")
    return 0


def _cmd_teardown(a, out):
    if a.act == "set":
        doc = teardown_set(a.slug, a.state, _kv(a.kv))
        out(json.dumps(doc) if a.json else f"teardown {a.slug}: {a.state}")
    elif a.act == "get":
        doc = teardown_get(a.slug)
        out(json.dumps(doc) if a.json else
            "\n".join(f"{k}: {v}" for k, v in sorted(doc.items())))
    return 0


def _cmd_residual(a, out):
    if a.act == "add":
        e = residual_add(a.slug, _kv(a.kv))
        out(json.dumps(e) if a.json else
            f"residual authority recorded for {a.slug}: expires {e['expires_at']}")
    elif a.act == "has":
        # Exit-code query, deliberately NOT `residual list | grep`. That form is a trap under
        # `set -o pipefail`: `list` exits 2 while authority is outstanding, so the pipeline
        # reports 2 whatever grep found, and a caller reading it as a boolean concludes the
        # opposite of the truth. This one has no output to parse.
        out_, _ = residual_list()
        return 0 if a.slug in out_ else 1
    elif a.act == "classify":
        # `reason` arrives as words. The `reason ` prefix and a leading `-` are tolerated because
        # deprovision.sh's operator instructions have spelled it both ways.
        reason = " ".join(a.reason).lstrip("-").lstrip() if a.reason else ""
        if reason.startswith("reason "):
            reason = reason[len("reason "):]
        e = residual_classify(a.slug, a.classification, reason)
        out(f"{a.slug}: classified {e['classification']} — the token still "
            f"authenticates until {e['expires_at']}" if not a.json else json.dumps(e))
    elif a.act == "drop":
        out("dropped" if residual_drop(a.slug) else "not present")
    elif a.act == "list":
        outstanding, done = residual_list()
        if a.json:
            out(json.dumps({"outstanding": outstanding, "expired": done}, indent=1))
        else:
            blocking, waived = residual_split(outstanding)
            for slug, e in sorted(blocking.items()):
                out(f"OUTSTANDING {slug}  expires {e['expires_at']}  "
                    f"user {e.get('uid', '?')}  ACTIVE_RISK")
            for slug, e in sorted(waived.items()):
                # Still printed, still with its real expiry. A waiver changes what the release
                # gate does, never what the ledger says.
                out(f"OUTSTANDING {slug}  expires {e['expires_at']}  "
                    f"user {e.get('uid', '?')}  {e.get('classification')} "
                    f"(waived: {e.get('waiver_reason')})")
            for slug in sorted(done):
                out(f"expired     {slug}")
        return 2 if residual_split(outstanding)[0] else 0
    return 0


_GROUPS = {"journal": _cmd_journal, "teardown": _cmd_teardown, "residual": _cmd_residual}


def main(argv):
    if not argv:
        print(__doc__)
        return 64
    args = _parser().parse_args(argv)
    if args.group is None or getattr(args, "act", None) is None:
        # A group with no action. argparse cannot require this on its own without making
        # `--help` unreachable, so it is checked here — as usage, not as a traceback.
        print(f"!! {args.group or 'lifecycle.py'}: which action? "
              f"see `{sys.argv[0]} --help`", file=sys.stderr)
        return 64
    try:
        return _GROUPS[args.group](args, print)
    except ValueError as e:
        print(f"!! {e}", file=sys.stderr)
        return 64


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
