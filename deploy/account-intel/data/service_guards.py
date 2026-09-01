"""Pure guards for the Account Service — no Flask, no psycopg, no I/O.

They live here rather than inside `service.py` for one reason: `service.py` imports flask and
psycopg at module level, so nothing that does not have the service's dependencies installed can
import it — which means the two rules most worth testing were the two nothing could test.
Both are small, both are security-relevant, and both used to be one line each inside a handler:

  * `safe_error` — what a client is allowed to learn when the backend fails. `str(e)` on a
    psycopg error carries the connection string; `/health` is reachable with no credential.
  * `validate_identity_map` — what shape the hot-reloaded identity file must have before it is
    allowed to replace the live one.

Import from `service.py`; test with `python3 test_service_guards.py`, which needs nothing.
"""
import datetime
import uuid


class IdentityMapError(ValueError):
    """The identity file is present and parses as JSON, but is not a usable identity map."""


def new_ref():
    """A short, non-secret correlation id. Ties a client-visible error to a log line."""
    return uuid.uuid4().hex[:12]


def safe_error(code="backend_unavailable", ref=None):
    """The (body, status) a caller gets when something inside fails.

    STABLE CODE, NO DETAIL. `code` is a fixed vocabulary a caller can branch on; `ref` is the
    only varying field and it carries no information about the failure — it is a lookup key
    for a log line the operator can read and the caller cannot."""
    return {"ok": False, "error": code, "ref": ref or new_ref()}, 500


def validate_identity_map(doc, path="identities"):
    """Return `doc` if it is a usable {token: org_id} map; raise IdentityMapError if not.

    Type-checked on BOTH sides. A nested object as a value used to flow straight through to a
    bound SQL parameter, so the failure surfaced per request, far from the write that caused
    it, for every client at once. Refusing the load instead is what lets the caller keep the
    last known-good map — which is only worth keeping if something is willing to say no."""
    if not isinstance(doc, dict):
        raise IdentityMapError(
            f"{path}: top level is {type(doc).__name__}, expected an object mapping "
            "token -> org_id")
    for token, org in doc.items():
        if not isinstance(token, str) or not token.strip():
            raise IdentityMapError(f"{path}: a key is not a non-empty string")
        if not isinstance(org, str) or not org.strip():
            raise IdentityMapError(
                f"{path}: the value for one token is {type(org).__name__}, expected a "
                "non-empty org id string")
    return doc


LIKE_ESCAPE = "\\"


def like_contains(term):
    """A LIKE pattern matching `term` as a LITERAL substring, wildcards and all.

    `f"%{q.lower()}%"` passed the caller's `%` and `_` straight into the pattern, so the search
    language was the client's to choose: `?query=%` matched every row and turned `/find_account`
    — a lookup that answers with a named account — into `/list_accounts` without the bound that
    endpoint exists to impose. `_` was subtler and worse to reason about, since a single
    underscore matches any one character and reads like an ordinary part of a name.

    Not a cross-organization leak: the `org_id = %s` clause is separate and bound, and it is
    what keeps one org's book out of another's. This is about the endpoint meaning what it says.

    The escape character itself goes first, or escaping `%` would introduce a backslash that the
    next replacement could not tell from one the caller typed. The SQL must name the same
    character with `ESCAPE`, and `service.py` does — Postgres defaults to backslash, but a
    default is not the place to leave a rule that decides what a query means.
    """
    escaped = (term.replace(LIKE_ESCAPE, LIKE_ESCAPE * 2)
                   .replace("%", LIKE_ESCAPE + "%")
                   .replace("_", LIKE_ESCAPE + "_"))
    return f"%{escaped}%"


def duplicate_orgs(doc):
    """Orgs holding more than one live token, sorted.

    A DELIBERATE SECOND COPY of the counting in `deploy/lib/identities.org_token_counts`, and it
    must stay one. That module is OPERATOR-side; this one is imported by `service.py` and runs
    INSIDE the Account Service container, which mounts this directory and nothing else. There is
    no import path between them at runtime, so the choice is a duplicated four-line count or a
    shared module that cannot be shared. Same reasoning as `multi/seam/operator_paths.py`, which
    says so in its own header, and as this file's neighbour `service._now`.

    The two answer different questions from the same count — `org_token_counts` returns the whole
    map for the console, this returns only the offenders for a startup guard — so a future change
    to one is unlikely to silently need the other.

    NOT an error: this is what a credential rotation looks like mid-flight, and what a re-run
    of provisioning leaves behind after a failed first pass. It IS authority nobody is
    tracking, so it is reported rather than tolerated silently."""
    counts = {}
    for org in doc.values():
        counts[org] = counts.get(org, 0) + 1
    return sorted(o for o, n in counts.items() if n > 1)


def insecure_mode(st_mode):
    """The octal mode if the file is readable or writable by group/other, else None.

    Advisory, deliberately. This file holds every client's org token, so a loose mode is a real
    finding — but refusing to load over a mode bit trades a confidentiality risk for a certain
    outage of every tenant at once."""
    bad = st_mode & 0o077
    return st_mode & 0o777 if bad else None


# ── the shared-note append boundary (#promotion) ────────────────────────────────────────────
#
# Every rule below is here rather than inside the Flask handler for the reason this module
# exists: a rule that cannot be tested without flask, psycopg and a database is a rule nothing
# tests. The route is wiring; these are the decisions.

#: Promoted activities live in their own id namespace so an append can never land on, or collide
#: with, an id the client's team authored (`NW-INT-01`, `HF-A1`, `FL-107-A1`). The append route is
#: the only writer of activities and it may only write here, which means a bug in the caller's id
#: derivation cannot reach a team-recorded row.
SHARED_ACTIVITY_PREFIX = "share-"

#: The one `kind` this route writes. Fixed server-side, never taken from the request: a caller
#: choosing the kind could disguise a promoted note as a call the team logged.
SHARED_ACTIVITY_KIND = "shared-note"

#: A transport bound, not a content rule. The seam caps a note far shorter (2000 bytes); this
#: exists only so an unbounded body cannot be pushed into a column, and it deliberately says
#: nothing about what the note may contain.
MAX_SHARED_BODY_BYTES = 8000
MAX_ID_BYTES = 128


class AppendRequestError(ValueError):
    """The request cannot be turned into an append. Carries the caller-facing reason code."""

    def __init__(self, code):
        super().__init__(code)
        self.code = code


def valid_contributor(value):
    """The same controlled-principal grammar Rust's `ContributorId` enforces.

    Lowercase ASCII, digits and `-`. Deliberately narrow and deliberately NOT applied to the
    body: a contributor is an identifier this system mints and renders as attribution, while the
    body is a person's own words and is not ours to constrain.
    """
    return (isinstance(value, str) and value != ""
            and all(c.islower() and c.isascii() or c.isdigit() or c == "-" for c in value))


def valid_date(value):
    """`YYYY-MM-DD`, and a real calendar date. Rejects `2026-02-31` as well as `not-a-date`."""
    if not isinstance(value, str) or len(value) != 10:
        return False
    try:
        datetime.date.fromisoformat(value)
    except ValueError:
        return False
    return True


def _bounded_text(value, limit):
    return isinstance(value, str) and value != "" and len(value.encode("utf-8")) <= limit


def parse_append_request(payload):
    """Validate one append request and return the fields the route will bind.

    Raises [`AppendRequestError`] with a stable code. `expected_org` is validated for SHAPE here
    and compared against the credential-resolved org by the caller — this function never sees a
    token and so can never be the thing that decides an organization.
    """
    if not isinstance(payload, dict):
        raise AppendRequestError("invalid_body")
    activity_id = payload.get("activity_id")
    account_id = payload.get("account_id")
    occurred_at = payload.get("occurred_at")
    body = payload.get("body")
    contributor = payload.get("contributor")
    expected_org = payload.get("expected_org")

    if not _bounded_text(activity_id, MAX_ID_BYTES):
        raise AppendRequestError("invalid_activity_id")
    if not activity_id.startswith(SHARED_ACTIVITY_PREFIX):
        # The namespace guard. This route is the only writer of activities, and confining it to
        # ids it minted is what keeps a caller-side derivation bug away from a team-authored row.
        raise AppendRequestError("activity_id_outside_shared_namespace")
    if not _bounded_text(account_id, MAX_ID_BYTES):
        raise AppendRequestError("invalid_account_id")
    if not valid_date(occurred_at):
        raise AppendRequestError("invalid_occurred_at")
    if not _bounded_text(body, MAX_SHARED_BODY_BYTES):
        raise AppendRequestError("invalid_body")
    if not valid_contributor(contributor):
        raise AppendRequestError("invalid_contributor")
    if not _bounded_text(expected_org, MAX_ID_BYTES):
        raise AppendRequestError("invalid_expected_org")
    return {"activity_id": activity_id, "account_id": account_id, "occurred_at": occurred_at,
            "body": body, "contributor": contributor, "expected_org": expected_org}


def append_conflict(existing, wanted, resolved_org):
    """The immutable-field comparison, as the name of the first field that differs or `None`.

    Order is deliberate and stable, so a conflict names one reason rather than whichever the
    dict happened to yield first. `kind` is included even though the route fixes it: a row whose
    kind differs was written by something other than this route, and silently agreeing with it
    would be the one case where a replay is not a replay.
    """
    for field, want in (
        ("org_id", resolved_org),
        ("account_id", wanted["account_id"]),
        ("occurred_at", wanted["occurred_at"]),
        ("kind", SHARED_ACTIVITY_KIND),
        ("body", wanted["body"]),
        ("contributor", wanted["contributor"]),
    ):
        got = existing.get(field)
        # `occurred_at` arrives as a `datetime.date` from psycopg and as a string from a fake.
        if isinstance(got, datetime.date):
            got = got.isoformat()
        if got != want:
            return field
    return None
