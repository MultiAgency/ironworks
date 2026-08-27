#!/usr/bin/env python3
"""Authoritative Account-Service identity state — the one reader AND the one writer.

WHY THIS EXISTS. Provisioning has to answer one question before it creates authority: *does
this org already have a usable Account-Service credential?* It used to answer a different
question — "did a previous run of THIS TOOL register one?" — by consulting the provisioning
journal. Those come apart the moment an org is created any other supported way (`seed-real.sh`
does exactly that), and when they do, provisioning mints a SECOND live token, overwrites the
local credential file, and leaves the first token authenticating. That is credential
accumulation manufactured by the tool meant to prevent it.

So: the journal records what the lifecycle DID; this module reports what the Account Service
HAS. Only the second may decide whether to register.

The identity map is `{token: org_id}` — the same file `service.py` hot-reloads via
ACCOUNT_IDENTITIES_FILE and `register-identity.sh` writes. It is the authoritative operator-side
representation of who holds authority for which org.

NOTHING HERE PRINTS A TOKEN except `resolve` and `other`, whose entire purpose is to hand
exactly one back to a caller that is about to use it. `count` and every diagnostic are
token-free by construction, so this module is safe to call from anything that logs.

WHY THE WRITES LIVE HERE TOO. They did not, and the reads and writes had drifted apart in the
way that matters. Four sites edited or counted this file with inline Python: register-identity.sh
added a token, provision.sh's compensator and deprovision.sh each removed an org's tokens, and
deprovision.sh counted them by hand. Only ONE of the four carried the corrupt-file refusal that
`load` and register-identity.sh both argue for at length — the two removal paths did a bare
`json.load(open(path))`, so a corrupt map produced a traceback in one and a rewrite in the
other. They did not even agree on the bytes: one wrote `indent=1`, the other did not.

Every one of those is the same invariant — a map that is authoritative for who holds live
authority must never be rewritten from a reading nobody could parse. It is stated once, in
`load`, and every mutation below goes through it.
"""
import json
import os
import pathlib
import sys
import tempfile

from agency_paths import agency_dir

OK, AMBIGUOUS, ABSENT, USAGE = 0, 2, 3, 64


class IdentityStateError(RuntimeError):
    """The identity map cannot be read as authoritative state."""


def identities_path(path=None):
    """The identity map of record. ACCOUNT_IDENTITIES_FILE wins, matching service.py."""
    return pathlib.Path(path or os.environ.get("ACCOUNT_IDENTITIES_FILE")
                        or agency_dir("account-identities/identities.json"))


def load(path=None):
    """The live {token: org} map, or {} when the file is genuinely absent.

    A CORRUPT file is NOT an empty one, and the distinction is load-bearing: reading it as empty
    would report every org as having no identity, and a caller acting on that would register a
    duplicate for every one of them. Absent means absent; unreadable means stop.
    """
    p = identities_path(path)
    try:
        doc = json.loads(p.read_text())
    except FileNotFoundError:
        return {}
    except (ValueError, OSError) as e:
        raise IdentityStateError(
            f"identity map at {p} is unreadable ({e}). Refusing to report identity state from a "
            "file that cannot be parsed — an empty reading here would mint duplicate credentials "
            "for every org.") from e
    if not isinstance(doc, dict) or not all(
            isinstance(k, str) and isinstance(v, str) for k, v in doc.items()):
        raise IdentityStateError(
            f"identity map at {p} is not a flat {{token: org}} object of strings.")
    return doc


def tokens_for_org(doc, org):
    """Every credential currently mapping to `org`, sorted for determinism."""
    return sorted(t for t, o in doc.items() if o == org)


def org_token_counts(doc):
    """{org: live credential count}, sorted by org. Counts only — no token value.

    An org holding more than one is NOT an error (it is what a rotation looks like mid-flight,
    and what a re-run of provisioning leaves behind), but it is authority nobody is tracking,
    so every deploy-side reader asks the same question the same way rather than re-counting."""
    counts = {}
    for org in doc.values():
        counts[org] = counts.get(org, 0) + 1
    return dict(sorted(counts.items()))


def _write(doc, path=None):
    """Replace the identity map atomically, private before it has content.

    mkstemp + fchmod + os.replace, in that order: a write-then-chmod publishes every client's
    org token at the process umask for the window in between, and this file IS the authority
    map. `indent=1` because both writers used to disagree about it and a diffable file is worth
    more than the bytes."""
    p = identities_path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(p.parent))
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(doc, f, indent=1)
        os.replace(tmp, p)
    except BaseException:
        pathlib.Path(tmp).unlink(missing_ok=True)
        raise
    return p


def add(token, org, path=None):
    """Register `token` as authority for `org`. Refuses a corrupt map — see `load`.

    Rewriting an unparseable map with only the new token would REVOKE every other client's org
    token, and the map is hot-reloaded, so all of them would 401 immediately."""
    doc = load(path)
    doc[token] = org
    return _write(doc, path)


def remove_org(org, path=None):
    """Drop every credential mapping to `org`. Returns how many were removed.

    Zero is a normal answer, not a failure: already-absent is the desired end state, and both
    the provisioning compensator and deprovisioning may run against a map that never had this
    org. An absent FILE is likewise zero — there is nothing to deregister."""
    doc = load(path)
    kept = {t: o for t, o in doc.items() if o != org}
    removed = len(doc) - len(kept)
    if removed:
        _write(kept, path)
    return removed


def other_org_token(org, path=None, doc=None):
    """Some credential belonging to an org that is NOT `org`, or None.

    For the provisioning isolation smoke, which must make a real cross-org request to prove the
    store refuses it. Which one is immaterial — any other org's token either leaks or does not.
    Takes an already-loaded map so a caller that has one does not read the file twice."""
    doc = load(path) if doc is None else doc
    return next((t for t, o in sorted(doc.items()) if o != org), None)


_ACTIONS = ("count", "resolve", "add", "remove", "other")


def main(argv):
    if len(argv) != 3 or argv[1] not in _ACTIONS:
        print(f"usage: identities.py {{{'|'.join(_ACTIONS)}}} <org_id>\n"
              "       add reads the token from ORG_TOKEN in the environment, never argv —\n"
              "       an argument is visible in `ps` to every user on the box.", file=sys.stderr)
        return USAGE
    action, org = argv[1], argv[2]

    # The mutations report what they did and nothing else; neither echoes a token.
    try:
        if action == "add":
            token = os.environ.get("ORG_TOKEN")
            if not token:
                print("!! set ORG_TOKEN=<token> in the environment (not on argv — it is a secret)",
                      file=sys.stderr)
                return USAGE
            add(token, org)
            print(f"   registered 1 org token for {org} (hot-reloaded, effective now)")
            return OK
        if action == "remove":
            print(remove_org(org))
            return OK
        doc = load()
        toks = tokens_for_org(doc, org)
        if action == "other":
            print(other_org_token(org, doc=doc) or "")
            return OK
    except IdentityStateError as e:
        print(f"!! {e}", file=sys.stderr)
        return AMBIGUOUS
    if action == "count":
        print(len(toks))
        return OK
    # resolve: exactly one, or refuse. Never pick one of several — an arbitrary choice here
    # silently blesses one credential and leaves the others live and unaccounted for.
    if not toks:
        print(f"!! no Account-Service identity for org {org!r}", file=sys.stderr)
        return ABSENT
    if len(toks) > 1:
        print(f"!! org {org!r} has {len(toks)} live Account-Service credentials. Provisioning "
              "will not choose between them: deregister the stale ones from the identity map "
              "first, then re-run. (Counts only — no token is printed.)", file=sys.stderr)
        return AMBIGUOUS
    print(toks[0])
    return OK


if __name__ == "__main__":
    sys.exit(main(sys.argv))
