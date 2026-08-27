#!/usr/bin/env python3
"""MultiAgency trusted context ingress (application adapter) — thin, boring.

BOUNDARY: IronClaw orchestrates the reasoning; this supplies trusted business context BEFORE
the turn. It is NOT a second agent runtime. The whole job is:
  authenticate -> resolve org/account -> fetch private Account Service context ->
  construct the trusted context envelope -> invoke IronClaw -> return the response.

It MUST NOT plan / reason / score / qualify / decide next actions / execute model-generated
fetch commands / duplicate IronClaw's loop. Context selection is DETERMINISTIC prefetch only.

TWO SEPARATE AUTHORITIES, both load-bearing. The model holds no CREDENTIAL authority — the
account token and private-network reach live in `account_service.py`, on the tenant's own
requests, and are never put on the IronClaw request this file builds. NETWORK authority is
removed elsewhere and does not come for free: a fresh member ships builtin.http
with a compiled-in wildcard egress policy, so the sealed member is confined at provisioning
(multi/provision/confine-member.sh) or a prompt-injected turn could POST this client's private
context to an arbitrary host.

Env:
  IRONCLAW_API (the one instance every client's sealed account lives on), ACCOUNT_BASE
  (default http://127.0.0.1:8443). Canonical serving reads the repo-root MODEL_PIN and rejects
  an off-pin process MODEL rather than treating it as an override.
  Per-client credentials come from `registry.load_clients()` (CLIENTS_DIR, default
  ~/.agency/clients/*.env) — each Thread carries its ClientConfig. There is deliberately
  NO ambient single-client fallback: a client that wasn't composed explicitly (registry
  guidance, or the internal composition requested by name) must not be servable.
"""
import contextvars, errno, os, json, datetime, hashlib, socket, time
import urllib.parse, urllib.request, urllib.error
# Re-exported deliberately: the proof and verify suites reach for these through
# `context_ingress`, because what the PRODUCT calls is the thing they mean to assert on. Each
# name has exactly one implementation, in the module named here. registry.py owns who may be
# served (tenant configuration, validated before any turn runs); envelope.py owns which records
# a turn is given and how they are rendered, with no I/O; this file owns everything that talks
# to something. responses.py holds the /v1/responses wire details this file and the proofs must
# share — the one output extractor and the one browser User-Agent; see its header for the copies
# that used to disagree, and for what a hosted instance's edge does to a python-urllib agent.
# `_svc` is deliberately NOT among these: it is the seam the suites fake, and a re-exported
# name patched here would leave account_service's own callers on the real implementation.
# Patch `account_service._svc`. `_catalog` and `_get_context` are called through THIS module's
# globals below, so patching them here does work.
try:
    from .persona import compose_persona
    from .registry import ClientConfig, _client
    from .registry import ACCOUNT_BASE, MODEL, load_clients  # noqa: F401 — re-export only
    from .envelope import build_envelope, resolve_targets
    from .account_service import (AccountScopeChanged, AccountScopeError,  # noqa: F401
                                  _catalog, _get_context, resolve_account_scopes)
    from .responses import BROWSER_UA, output_text
except ImportError:  # direct-script compatibility during service-unit rollout
    from persona import compose_persona
    from registry import ClientConfig, _client
    from registry import ACCOUNT_BASE, MODEL, load_clients  # noqa: F401 — re-export only
    from envelope import build_envelope, resolve_targets
    from account_service import (AccountScopeChanged, AccountScopeError,  # noqa: F401
                                 _catalog, _get_context, resolve_account_scopes)
    from responses import BROWSER_UA, output_text

# ── WHEN CONFIGURATION RESOLVES, and the rule that decides ───────────────────────────
# Two mechanisms live in this package and the boundary between them was never written down, so
# each new setting picked one by whichever neighbour it was pasted near.
#
#   AT USE — anything naming an EXTERNAL SYSTEM or a CREDENTIAL: IRONCLAW_API (`_api`),
#   TELEGRAM_BOT_TOKEN (`telegram_bridge._bot`), the bridge state paths
#   (`state_json_path`/`state_db_path`). Reading these at import makes *importing* the act of
#   configuring, which is what forced twenty-two files to carry `os.environ.setdefault(...)`
#   whose only job was to let an import succeed, and made test ORDER load-bearing. Resolving
#   per-call also lets two suites hold different configurations in one process.
#
#   AT IMPORT — process-lifetime TUNING that names nothing outside this box:
#   TURN_BUDGET_SECONDS, ORPHAN_MAX_UNVERSIONED_ATTEMPTS, bridge_state.RETAIN_TERMINAL. These
#   are read once because a turn's budget changing underneath a running turn is worse than a
#   restart, and PERSONA_ROOT, which selects the tree this process serves from.
#
#   MODEL is import-time ON PURPOSE (in registry.py, with the rest of tenant configuration) and
#   is not an exception to either rule: canonical serving is pinned, and `load_clients`
#   separately rejects an off-pin process MODEL, so it is not a knob.
#
# A setting that is neither — external, but wanted early — is the shape to argue about before
# adding. `CATALOG_TTL_SECONDS` was one: nine files set it before their imports, one with the
# comment "must precede the import", after the cache it configured had already been removed.
# Nothing read it. Ceremony outlives the thing it was for unless the rule is written down.


class SeamNotConfigured(RuntimeError):
    """A required piece of seam configuration is absent from the environment."""


def _api():
    """The one instance every tenant's sealed account lives on.

    Resolved ON FIRST USE, not at import. Reading it at import made *importing* this module the
    act of configuring it, so 22 files carried `os.environ.setdefault(...)` lines whose only job
    was to let an import succeed — one of them setting a token it named "unused-by-this-probe".
    Fail-fast is still the point and is unchanged: a turn with no IRONCLAW_API raises here,
    loudly, before any request leaves. What moved is WHERE it fails — at a use the caller chose,
    rather than at whichever import happened to run first.

    Deliberately NOT cached: an uncached lookup is what lets two suites hold different
    configurations in one process. `setdefault` gave the first import the last word, which made
    test ORDER load-bearing and silent."""
    v = os.environ.get("IRONCLAW_API")
    if not v:
        raise SeamNotConfigured(
            "IRONCLAW_API is unset — the seam has no instance to talk to. Set it to the "
            "instance this bridge serves (e.g. http://127.0.0.1:3020).")
    return v.rstrip("/")


# ── the per-turn budget and recovery handle ──────────────────────────────────────────
# ONE wall-clock budget for everything a single turn does — connect, create, poll, back off,
# and the whole-dispatch retry. It replaces a nested stack of independent constants whose
# product was an accidental ceiling nobody chose: 4 x 180s + 18s backoff + 150s polling, doubled
# by the continuity self-heal, is 29.6 minutes during which the shared loop serves nobody.
#
# 180s is chosen from behaviour, not from arithmetic. MODEL_PIN records the pinned model
# answering a real partner question in 7.7s, and names 13-14s as the reason two other models
# were rejected as unusable in chat. A budget an order of magnitude above the measured p99
# absorbs a genuinely slow turn while bounding what one tenant can cost every other tenant.
TURN_BUDGET_SECONDS = float(os.environ.get("TURN_BUDGET_SECONDS", "180"))

# The idempotency key and deadline for the turn currently executing. A ContextVar rather than a
# parameter so no call site or test double changes its signature, and rather than a module
# global so it stays correct while tenant workers run concurrently.
_TURN_CTX = contextvars.ContextVar("ironworks_turn_ctx", default=None)


class TurnBudgetExceeded(RuntimeError):
    """The turn ran past its wall-clock budget. Carries whether the model request had already
    been ACCEPTED, because that decides whether a retry would be a second billed turn."""

    def __init__(self, message, request_sent=False):
        super().__init__(message)
        self.request_sent = request_sent


class TurnOutcomeUnknown(RuntimeError):
    """The request left the process, but no terminal IronClaw outcome was established."""

    request_sent = True


def _ctx():
    return _TURN_CTX.get() or {}


def _remaining():
    """Seconds left in this turn's budget, or None when no budget is in force (the dev oracle
    and the proof scripts call turn() without one)."""
    deadline = _ctx().get("deadline")
    return None if deadline is None else deadline - time.monotonic()


def _sleep_within_budget(seconds, request_sent):
    """Sleep, but never past the deadline. A backoff that outlives the budget turns a bounded
    turn back into an unbounded one, which is the whole defect being removed."""
    left = _remaining()
    if left is None:
        time.sleep(seconds)
        return
    if left <= 0:
        raise TurnBudgetExceeded(
            f"turn exceeded its {TURN_BUDGET_SECONDS:.0f}s budget", request_sent=request_sent)
    time.sleep(min(seconds, left))


def _check_budget(request_sent):
    left = _remaining()
    if left is not None and left <= 0:
        raise TurnBudgetExceeded(
            f"turn exceeded its {TURN_BUDGET_SECONDS:.0f}s budget", request_sent=request_sent)
    return left


# Errors that PROVE nothing reached IronClaw. urllib wraps a connect-phase OSError in a
# URLError whose `.reason` is the original; a refused port, an unreachable host or network, and
# a name that does not resolve all fail before a single byte of the request is written. Anything
# else — a timeout, a reset, a truncated read — is genuinely ambiguous and must be treated as
# sent, because the cost of guessing wrong in that direction is a second billed turn.
_NEVER_CONNECTED_ERRNOS = frozenset(
    e for e in (getattr(errno, n, None) for n in
                ("ECONNREFUSED", "EHOSTUNREACH", "ENETUNREACH", "ENETDOWN", "EADDRNOTAVAIL",
                 "EAFNOSUPPORT", "EHOSTDOWN"))
    if e is not None)


def _proved_unsent(exc):
    """True only when `exc` establishes that the request never left this process."""
    reason = getattr(exc, "reason", None)
    for e in (reason if isinstance(reason, BaseException) else None, exc):
        if isinstance(e, socket.gaierror):
            return True
        if isinstance(e, ConnectionRefusedError):
            return True
        if isinstance(e, OSError) and e.errno in _NEVER_CONNECTED_ERRNOS:
            return True
    return False


def _now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


ADMIN_PROBE_PATH = "/api/webchat/v2/admin/users"


class OperatorTokenInRegistry(RuntimeError):
    """A tenant's member token resolves to an operator identity, or could not be shown not to."""


def assert_no_member_is_the_operator(clients, timeout=10):
    """Refuse to serve unless every tenant's token is a SEALED MEMBER. Fails closed.

    WHY THIS EXISTS. `load_clients` already refuses a registry entry whose IRONCLAW_TOKEN is the
    operator token — but it recognises one by comparing against THIS PROCESS's environment
    (IRONCLAW_OPERATOR_TOKEN / IRONCLAW_REBORN_WEBUI_TOKEN / WEBUI_TOKEN). The bridge carries
    none of those, so that set is empty and the check has never been able to fire in the one
    process that serves tenants. It fires in `deploy/ironworks`, run on the operator's own box,
    where holding the operator token is normal and the check is therefore harmless. Alive where
    it does not matter, inert where it does — which is why nothing noticed.

    THE FIX IS TO STOP ASKING THE ENVIRONMENT. Whether a bearer is an operator is the runtime's
    fact, not the process's: an operator identity is accepted on the admin surface and a sealed
    member is refused there (401 or 403 — 403 means the identity resolved and the route said no,
    which for a member is still a closed door). So probe, rather than compare.

    PLACEMENT, per D-077: at startup, never at registry load. `load_clients` must stay usable on
    a clean clone with no instance — `multi/verify/test_fixtures_offline.py` pins that — so a
    network probe there would break the contract that lets fixtures validate offline.

    FAIL CLOSED, deliberately: an unreachable instance means UNKNOWN, and UNKNOWN raises. A
    control that cannot verify identity does not serve. The cost is real and accepted — an
    instance outage becomes a bridge outage — and it follows `load_groups()`, which raises on an
    empty registry, rather than `_catalog`, which degrades. A security check is the first kind,
    not the second: degrading here would restore exactly the silence this replaces."""
    base = _api()
    for slug, c in sorted(clients.items()):
        # BROWSER_UA for the reason responses.py:66-71 gives: a hosted IronClaw may sit behind
        # Cloudflare bot-protection that 1010-blocks the default python-urllib agent. That block
        # is served as HTTP 403 — which the `code not in (401, 403)` test below reads as proof of
        # a SEALED member. Without this header an edge that never saw the admin route could
        # certify every tenant, including one whose token really is the operator's.
        req = urllib.request.Request(base + ADMIN_PROBE_PATH,
                                     headers={"Authorization": f"Bearer {c.ironclaw_token}",
                                              "User-Agent": BROWSER_UA})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as x:
                code = x.status
        except urllib.error.HTTPError as e:
            code = e.code
        except OSError as e:
            raise OperatorTokenInRegistry(
                f"tenant {slug!r}: cannot reach {base} to verify the member token is not an "
                f"operator token ({e}). Refusing to serve — an unverified identity is not a "
                "verified one. Start the instance, or fix IRONCLAW_API.") from e
        if code not in (401, 403):
            raise OperatorTokenInRegistry(
                f"tenant {slug!r}: its IRONCLAW_TOKEN is ACCEPTED on the admin surface "
                f"(HTTP {code}) — that is an OPERATOR identity, not a sealed member. It could "
                "read across accounts and re-enable its own egress tools. Re-provision the "
                "tenant with multi/provision/provision-client.sh.")


def _post_ironclaw(body, client=None, attempts=4):
    """POST a turn with an official idempotency key so a retry can NEVER create a second
    accepted turn. IronClaw's /v1/responses honors the `idempotency-key` header and replays the
    prior result for a repeat (ProductInboundAck::Duplicate; handlers.rs:186). The key is stable
    across retries of THIS turn, so even an ambiguous post-send timeout is safe to retry — the
    server dedups. The caller updates thread.prev ONLY after a confirmed success.
    NOTE: body carries only {model, instructions, input, previous_response_id} — no token, no
    account host.

    THE `sent` FLAG IS THE RECOVERY DECISION, not bookkeeping: it becomes
    `TurnOutcomeUnknown.request_sent`, and `bridge_core._run_turn` turns that into
    RECOVERY_BLOCKED — terminal, operator-visible, never replayed. It must therefore mean "a
    turn MAY have executed", so it is set from evidence: a status line proves the request
    arrived; `_proved_unsent` names the failures that prove it did not; everything else is
    ambiguous and counts as sent. An instance that is merely DOWN is an ordinary terminal
    failure the tenant can simply retry, and must not consume an operator's reconciliation."""
    import uuid
    c = _client(client)
    # The key comes from the CALLER when there is one, because the bridge must record it
    # durably BEFORE this request leaves the process — it is the only handle that exists
    # before a reply we might never receive. A locally-minted key would be lost with the
    # process that minted it. (Measured at the pinned rev: replaying a key returns the same
    # response id and identical text; a key replayed against a different body is refused 409
    # and never serves the earlier answer. multi/verify/test_responses_recovery.py.)
    key = _ctx().get("key") or uuid.uuid4().hex
    data = json.dumps(body).encode()
    headers = {"Authorization": "Bearer " + c.ironclaw_token, "Content-Type": "application/json",
               "Idempotency-Key": key, "User-Agent": BROWSER_UA}
    sent = False
    # No `last`/`raise last` tail: every path through this loop returns, continues (only while
    # i < attempts-1), or re-raises, so the loop cannot fall through and the tail was unreachable.
    for i in range(attempts):
        left = _check_budget(request_sent=sent)
        # Never let a single attempt outlive the whole turn's budget: the old 180s per-attempt
        # timeout was independent of everything above it, which is how four attempts became a
        # twelve-minute ceiling nobody chose.
        per_try = 180 if left is None else max(1.0, min(180.0, left))
        req = urllib.request.Request(_api() + "/v1/responses", data=data, headers=headers)
        try:
            # `sent` is set from EVIDENCE below, never from having built a request object:
            # constructing one opens no socket, so flagging it here made an instance that is
            # simply down indistinguishable from a turn that may have run.
            return json.loads(urllib.request.urlopen(req, timeout=per_try).read())
        except urllib.error.HTTPError as e:
            # A status line is proof the request arrived: from here a retry could be a SECOND
            # accepted turn if the key were not honoured; it is, and that is what makes it safe.
            sent = True
            if e.code in (429, 500, 502, 503, 504) and i < attempts - 1:
                _sleep_within_budget(3 * (i + 1), sent); continue
            if e.code in (500, 502, 503, 504):
                raise TurnOutcomeUnknown(
                    f"IronClaw returned HTTP {e.code} after the request was sent; execution "
                    "outcome is unknown") from e
            raise
        except TurnBudgetExceeded:
            raise
        except Exception as e:   # timeouts / connection errors — safe to retry under the same key
            sent = sent or not _proved_unsent(e)
            if i < attempts - 1:
                _sleep_within_budget(3, sent); continue
            if sent:
                raise TurnOutcomeUnknown(
                    f"IronClaw request outcome is unknown after {attempts} attempt(s): "
                    f"{type(e).__name__}") from e
            raise


def _await_completion(d, client=None, deadline=150, interval=2):
    """A turn that reaches for a tool returns `in_progress` before the final message lands
    (handlers return early; the run continues server-side). Poll GET /v1/responses/{id} until
    terminal so the caller never relays an empty reply. Returns the last snapshot on timeout."""
    c = _client(client)
    rid = d.get("id")
    waited = 0
    while rid and d.get("status") in ("queued", "in_progress") and waited < deadline:
        # The turn budget outranks this loop's own deadline. The request HAS been accepted by
        # now, so running out of budget here is recoverable — the response id is already known
        # and the caller can fetch it — which is why request_sent is True.
        _check_budget(request_sent=True)
        _sleep_within_budget(interval, True); waited += interval
        interval = min(interval * 1.5, 10)      # mild backoff: long tool runs poll gently
        req = urllib.request.Request(_api() + "/v1/responses/" + rid,
                                     headers={"Authorization": "Bearer " + c.ironclaw_token,
                                              "User-Agent": BROWSER_UA})
        left = _remaining()
        try:
            with urllib.request.urlopen(
                    req, timeout=30 if left is None else max(1.0, min(30.0, left))) as x:
                d = json.loads(x.read())
        except TurnBudgetExceeded:
            raise
        except Exception as e:
            raise TurnOutcomeUnknown(
                f"IronClaw accepted response {rid!r}, but its terminal outcome could not be "
                f"retrieved ({type(e).__name__})") from e
    return d


def fetch_response(client, response_id, timeout=30):
    """GET one already-completed response, as this tenant's own sealed member.

    PUBLIC because the bridge needs it: recovering an answer that already ran is a delivery
    concern, but the CREDENTIAL and the endpoint are this module's. `telegram_bridge` used to
    build the identical request itself, reaching through three names from here —
    `ing.IRONCLAW_API`, `ing.BROWSER_UA`, `ing.output_text` — which put the one call that must
    be scoped to a tenant's own token in the module that does not own tokens.

    Returns the parsed document. Raises like any other urllib call; the caller decides what a
    failure means, because for the bridge it is a recoverable redelivery and here it is not.
    """
    c = _client(client)
    req = urllib.request.Request(_api() + "/v1/responses/" + response_id,
                                 headers={"Authorization": "Bearer " + c.ironclaw_token,
                                          "User-Agent": BROWSER_UA})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def _completed(d):
    """Fail closed on a turn that did not complete. A failed/cancelled response — or one
    still running when the poll deadline expired — must not advance thread.prev or mark
    context as supplied: the reply never happened, so thread state must look untouched
    (the retry then re-delivers the context). Missing status = a terminal reply from a
    server that doesn't stamp one."""
    status = d.get("status")
    if status in (None, "completed"):
        return d
    rid = d.get("id")
    if status in ("queued", "in_progress") and rid:
        # POLL-DEADLINE EXHAUSTION IS NOT "NO REQUEST WAS SENT". The turn was accepted, it is
        # still running server-side, it is billed, and its response id is KNOWN — every
        # condition that makes an outcome recoverable. Raised as a bare RuntimeError, it reached
        # `bridge_core._run_turn`'s `getattr(e, "request_sent", False)` as False and became
        # FAILED_TERMINAL + CLIENT_FAILURE: terminal, response id discarded, client told the
        # turn failed. One second of wall clock later the identical fact raises
        # TurnBudgetExceeded(request_sent=True) from `_check_budget` and is RECOVERY_BLOCKED
        # instead — so which of the two an operator saw was a race between the 150s poll
        # deadline and the 180s turn budget. The new request_sent machinery covered every other
        # exit from `_await_completion` except its normal loop exit, which is this one.
        raise TurnOutcomeUnknown(
            f"IronClaw accepted response {rid!r} and it was still {status} when the poll "
            "deadline expired; its terminal outcome is unknown, not failed")
    raise RuntimeError(f"ironclaw turn did not complete: status={status!r} id={rid!r}")


class Thread:
    """One conversation FOR one client. Tracks the client's config (which credentials this
    thread's requests use), the IronClaw response id (continuity), and which accounts we've
    already supplied, so context is injected once, not re-blobbed every turn."""
    def __init__(self, client=None):
        self.client = _client(client)
        if not self.client.persona:
            raise RuntimeError(f"client {self.client.slug!r} has no persona — refusing to serve. "
                               "Compose it explicitly: registry tenants via compose_service_persona "
                               "(load_clients does this), the dev oracle via compose_persona().")
        self.prev = None
        # record_id -> the account's `updated_at` AS SUPPLIED. Not a set: "have we sent this?"
        # and "is what we sent still current?" are the same question, and keeping the version
        # beside the id is what lets staleness be measured instead of guessed.
        self.supplied = {}
        self.ever_supplied = False   # has any turn injected account context? (data-starvation recovery)
        # When this thread last COMPLETED a turn (UTC ISO-8601), or None if it never has.
        # Operator-facing, not behavioural: "when was this tenant last successfully exercised?"
        # was previously unanswerable without reading a chat room. Nothing in the seam branches
        # on it — a timestamp that steered behaviour would be a clock dependency in the turn
        # path, which is the shape that makes tests flaky.
        self.last_turn_at = None
        # account_id -> (catalog version when its context 404'd, attempts). See _is_known_orphan.
        self.orphans = {}


# WHAT A ROW MUST CARRY TO BE USABLE, and why this is checked rather than trusted.
#
# The seam read Account Service payloads with `[]` — `c["account_id"]`, `c["name"]`,
# `ctx["record_id"]`, `a["name"]`. One row missing one key raised KeyError out of `turn()`,
# which `bridge_core._run_turn` classifies as an ordinary failure (no request was sent), so the
# tenant got FAILED_TERMINAL on EVERY message until someone repaired the store. Reproduced: a
# two-row catalog with `name` absent from the second row killed a turn about the first.
#
# That is the wrong shape of failure. Degraded mode already exists for a store that is
# UNAVAILABLE — serve the turn, tell the model records are missing — and a store that is
# MALFORMED is the same fact to a client and a different one to an operator. So: an unusable
# row is dropped rather than trusted, the model is told the book it sees is short, and the
# operator is told which rows and why. One bad row must not blind the analyst to the other
# ninety-nine, and must not silently shrink the book either.
#
# `updated_at` is deliberately NOT required: it is read with `.get()` everywhere, and its
# absence has a defined meaning already (an UNKNOWN version re-fetches once — see
# `_stale_or_new`). Requiring it here would turn a supported state into a dropped record.
_CATALOG_ROW_REQUIRED = ("account_id", "name")


def _nonempty_str(v):
    return isinstance(v, str) and v.strip() != ""


def _usable_catalog_rows(rows):
    """Split a catalog's rows into the ones the envelope can render and the ones it cannot."""
    good, bad = [], []
    for r in rows if isinstance(rows, list) else []:
        ok = isinstance(r, dict) and all(_nonempty_str(r.get(k)) for k in _CATALOG_ROW_REQUIRED)
        (good if ok else bad).append(r)
    return good, bad


def _usable_context(ctx):
    """Whether `envelope._render_account` can render this account context at all.

    Mirrors exactly what that function subscripts — `ctx["record_id"]` and `ctx["account"]["name"]`
    — so this predicate and the renderer cannot disagree about what "usable" means."""
    if not isinstance(ctx, dict) or not _nonempty_str(ctx.get("record_id")):
        return False
    account = ctx.get("account")
    return isinstance(account, dict) and _nonempty_str(account.get("name"))


def _catalog_or_degraded(cl):
    """This client's accounts and org id, or an empty book plus a note if the store is down.

    DEGRADED MODE: the account store being down must not kill conversation itself. Proceed with
    no context and tell the MODEL records are briefly unavailable — the client gets a working
    chat plus an honest caveat, not a stack trace.

    The org falls back to the id, NOT the display name: `org` is model-visible (build_envelope
    emits `organization: {org}`) and the healthy path sends catalog["org"], the id. Sending
    cl.name here made the org identify itself one way normally and another way in the one
    situation where the model is ALSO being told records are unavailable. Do not "improve" this
    to `cl.name or cl.slug`.

    Returns (candidates, org, note).
    """
    try:
        catalog = _catalog(cl)
        accounts, unusable = _usable_catalog_rows(catalog["accounts"])
        if unusable:
            # Operator-facing: which rows, and enough to find them. Client-facing: only that the
            # book is short, because "your store returned a row without a name" is a fact about
            # our plumbing, not about their business (same rule as `_record_orphan`).
            ids = ", ".join(sorted(
                repr(r.get("account_id")) for r in unusable if isinstance(r, dict))) or "unidentifiable"
            print(f"[malformed] {cl.slug}: {len(unusable)} catalog row(s) lack "
                  f"{' or '.join(_CATALOG_ROW_REQUIRED)} and were dropped ({ids}) — the turn "
                  "was served without them", flush=True)
            return accounts, catalog["org"], (
                f"partial — {len(unusable)} record(s) in your book could not be read just now "
                "and are not included below; answer from what is here and say the book is "
                "incomplete if the question needs them")
        return accounts, catalog["org"], None
    except AccountScopeChanged:
        raise
    except Exception as e:
        print(f"[degraded] {cl.slug}: account store unreachable ({type(e).__name__}) — "
              "serving turn without records", flush=True)
        return [], cl.slug, (
            "temporarily unavailable — the records store could not be reached just now; "
            "answer from conversation history and say records are briefly unavailable "
            "if the question needs them")


# How many times one thread will re-ask for an account the catalog lists but whose context
# 404s, when the catalog carries NO version to key a negative result on. With a version the
# retry is event-driven (re-ask when the catalog row moves) and this bound never applies; the
# bound exists only for the version-less case, where the alternative is either "ask forever"
# (the defect this replaced) or "never ask again" (which cannot self-heal).
ORPHAN_MAX_UNVERSIONED_ATTEMPTS = int(os.environ.get("ORPHAN_MAX_ATTEMPTS", "3"))


def _record_orphan(thread, client, aid, version, reason="/get_account_context returns 404"):
    """Record a catalogued-but-unfetchable account, and tell the OPERATOR once per transition.

    Logged, never enveloped: the client asked a business question, and "your store lists a row
    my read of it cannot resolve" is an operator fact about our own plumbing. It goes to the
    process log with the tenant slug and the account id — both non-secret — and to the bridge
    state file, which is what `ironworks tenant inspect` reads.

    `reason` is a parameter because there are now two ways to be unfetchable and they need
    DIFFERENT repairs: a 404 means the catalog and the record store disagree about what exists,
    while an unrenderable payload means the row exists and is malformed. Same bounded retry, same
    envelope silence — but an operator sent to prune a catalog when the real fix is a null column
    has been told the wrong thing, and the log is the only place they will read it.
    """
    prev = thread.orphans.get(aid)
    attempts = (prev[1] if prev else 0) + 1
    thread.orphans[aid] = (version, attempts)
    if not prev:
        # The retry rule differs by whether the catalog carries a version, and the log has to
        # say which one is in force — an operator who reads "until that row changes" about an
        # account with no version would wait for an event that cannot arrive.
        rule = (f"not re-asked until its catalog row moves past {version!r}" if version is not None
                else f"catalog carries no version, so it is retried at most "
                     f"{ORPHAN_MAX_UNVERSIONED_ATTEMPTS} times per thread")
        print(f"[catalog-orphan] {client.slug}: account {aid!r} is listed by /list_accounts but "
              f"{reason} — {rule}. Reconcile the store or prune the "
              "catalog; `ironworks tenant inspect` reports this.", flush=True)


def _is_known_orphan(thread, aid, current):
    """True when this thread has already established that `aid` is catalogued-but-unfetchable
    AND nothing has changed that would make asking again worthwhile.

    THE DEFECT THIS CLOSES. An account the catalog LISTS but whose `get_account_context` 404s
    used to be re-fetched on every single turn, forever: the caller dropped the `None`, so the
    id never entered `thread.supplied`, so it re-targeted next turn. That cost one wasted round
    trip per turn per orphan and quietly broke the "the book costs one fetch per thread" bound
    that `_targets_for`'s widening fallback depends on — the more orphans a book accumulated,
    the more every single turn cost, with nothing reporting it.

    THE FIX IS A VERSIONED NEGATIVE RESULT, not a blanket suppression. A 404 is recorded
    against the catalog version that produced it. When that version moves, the negative result
    is stale and the account is asked for again — so an account that is repaired in the store
    (any repair changes `updated_at`) heals on the next turn without an operator doing
    anything. Only when the catalog carries no version at all does this fall back to a bounded
    attempt count, because there is then no event to key the retry on.
    """
    seen = thread.orphans.get(aid)
    if not seen:
        return False
    version_at_404, attempts = seen
    if version_at_404 is not None:
        return version_at_404 == current.get(aid)     # unchanged row -> still an orphan
    return attempts >= ORPHAN_MAX_UNVERSIONED_ATTEMPTS


def _targets_for(thread, user_text, candidates, current, stopwords):
    """The account ids this turn should fetch: never sent before, or moved since we sent them.

    NO-TARGET FALLBACK — the WIDENING half of the resolution contract, not a safety net under a
    broken resolver. `resolve_targets` narrows ONLY on a deliberate mention and returns [] for
    everything else; [] means "widen", not "supply nothing", and this is the half that honours
    it. Without it, natural phrasings ("is anything time-sensitive", "what's slipping") walk
    past the resolver and the model answers a book-wide question with zero records: honest and
    useless. Cost is bounded by the book ONCE per thread — the inject-once filter below does the
    rest — and once everything has been supplied this is a no-op. Deliberate naming still wins;
    this only fires when the resolver found no target at all.

    FRESHNESS IS MEASURED, NOT ASKED FOR. An account is re-sent when the catalog's `updated_at`
    has moved past the version this thread was given. This replaced a keyword list ("what
    changed", "refresh", "latest on") that guessed intent from prose and failed in both
    directions: widen the resolved set and it re-fetched the whole book; tighten name matching
    and "refresh <multi-word account>" matched nothing, so an explicit refresh silently no-oped.
    Data goes stale whether or not anyone thinks to ask. The catalog is already fetched every
    turn and cached per client, so this costs one column, not a round trip.
    """
    named = resolve_targets(user_text, candidates, stopwords)   # content ONLY — never the speaker
    if not named and candidates:
        named = [c["account_id"] for c in candidates]
    named = [aid for aid in named if not _is_known_orphan(thread, aid, current)]

    def _moved(aid):
        # An UNKNOWN sent version (None) means "re-fetch once", never "never again". A None gets
        # written two ways: a turn served before the Account Service emitted `updated_at`, and
        # the pre-versioning state migration in telegram_bridge.py, which sets every id to None
        # by design. Requiring `sent_v is not None` here pinned each of those accounts to its
        # first copy for the LIFE of the thread — bridge-threads.json persists, so no restart
        # cleared it — the exact failure this design replaced. Treating None as unknown
        # self-heals in one fetch. When neither side has a version this is still False, so there
        # is no re-fetch storm.
        now_v, sent_v = current.get(aid), thread.supplied.get(aid)
        return now_v is not None and now_v != sent_v

    return [aid for aid in named if aid not in thread.supplied or _moved(aid)]


def _dispatch(body, cl, thread):
    """POST the turn, retrying once on a fresh thread if the continuity pointer is rejected.

    SELF-HEAL a poisoned continuity pointer: if the server no longer knows our
    previous_response_id (expired/lost), every future turn would 404 forever. Retry once on a
    fresh thread instead of bricking the group.
    """
    # The recovery retry removes previous_response_id, so it is a DIFFERENT request body. The
    # pinned runtime correctly rejects a changed body under the same idempotency key. Derive a
    # stable second key from the durable first one: reproducible within this logical attempt,
    # distinct without inventing another random recovery handle.
    import uuid
    base_key = _ctx().get("key") or uuid.uuid4().hex

    def post_with_key(request_body, key):
        turn_ctx = dict(_ctx())
        turn_ctx["key"] = key
        token = _TURN_CTX.set(turn_ctx)
        try:
            return _post_ironclaw(request_body, cl)
        finally:
            _TURN_CTX.reset(token)

    try:
        return _completed(_await_completion(
            post_with_key(body, base_key), cl))
    except urllib.error.HTTPError as e:
        if e.code == 404 and body.get("previous_response_id"):
            print(f"[recover] {cl.slug}: previous_response_id rejected (404) — "
                  "continuing on a fresh thread", flush=True)
            thread.prev = None
            body.pop("previous_response_id")
            fresh_key = hashlib.sha256((base_key + "\0fresh-thread").encode()).hexdigest()
            return _completed(_await_completion(
                post_with_key(body, fresh_key), cl))
        raise




def turn(thread, user_text, speaker=None, idempotency_key=None, budget=None):
    """Run one ingress turn: resolve -> fetch -> package -> call IronClaw -> return.

    `idempotency_key` is the caller's durable recovery handle, recorded before this is called.
    `budget` is the whole turn's wall-clock allowance in seconds (None = unbounded, which is
    what the dev oracle and the proof scripts want; the bridge always passes one).

    `speaker` (the human who sent the message) is attribution ONLY — it is deliberately excluded
    from account resolution (resolve_targets sees only the message content, never the speaker),
    so a person's name can never be read as an account. Inject-once by default; an account is
    re-sent only when the catalog's `updated_at` has moved past the version this thread was
    given — never because of how the question was worded. Nothing the user types re-fetches
    anything. Returns (agent_text, supplied_account_ids).
    """
    token = _TURN_CTX.set({"key": idempotency_key,
                           "deadline": None if budget is None else time.monotonic() + budget})
    try:
        return _turn_inner(thread, user_text, speaker)
    finally:
        _TURN_CTX.reset(token)


def _turn_inner(thread, user_text, speaker=None):
    cl = thread.client
    candidates, org, note = _catalog_or_degraded(cl)
    current = {c["account_id"]: c.get("updated_at") for c in candidates}
    targets = _targets_for(thread, user_text, candidates, current, cl.name_stopwords)

    # An account the catalog LISTS but whose get_account_context 404s is a CATALOG/STORE
    # INCONSISTENCY, not a client-visible event: a 404 contributes no context either way, so
    # the answer is unaffected and nothing about it belongs in the envelope. What it must not
    # do is cost a round trip every turn forever (see `_is_known_orphan`), and it must not be
    # invisible to the operator — an inconsistency nobody is told about is one nobody fixes.
    contexts = []
    for aid in targets:
        ctx = _get_context(aid, cl)
        # A context that came back but cannot be RENDERED is the same fact as one that did not
        # come back: "your store lists a row my read of it cannot resolve" — `_record_orphan`'s
        # own words. So it takes the same path, which is already bounded, self-healing when the
        # row is repaired, and visible to `ironworks tenant inspect`. Trusting it instead raised
        # KeyError out of the turn and failed every subsequent message for the tenant.
        malformed = ctx is not None and not _usable_context(ctx)
        if ctx and not malformed:
            contexts.append(ctx)
            thread.orphans.pop(aid, None)      # it answers now: the negative result is spent
            continue
        _record_orphan(thread, cl, aid, current.get(aid),
                       reason=("/get_account_context returned a record with no usable "
                               "record_id/name" if malformed else "/get_account_context returns 404"))

    # EMPTY BOOK (declared, never implied): with zero accounts loaded, the model must be told —
    # a bare message + a persona that says "work from the records supplied to you" leaves
    # confabulate-or-stall to chance. The persona's empty-book section defines the behavior;
    # this line supplies the fact.
    if note is None and not candidates:
        note = ("empty — no account records have been loaded for this organization yet "
                "(see your empty-book instructions)")

    # Data-starvation recovery: if this thread has prior history but was NEVER given account
    # context (e.g. the org was empty and the model told the user "I have no records"), and
    # context is now available, do NOT chain to that stale thread — its history anchors the
    # model to the data-starved stance even once context is injected. First-contact only: once a
    # thread has had context, later new/updated accounts inject into the conversation normally.
    if contexts and thread.prev and not thread.ever_supplied:
        thread.prev = None
        # The dropped thread may hold facts the team supplied conversationally during the
        # empty-book period — surface the loss instead of silently discarding it.
        note = ("first records just loaded; this group's earlier conversation (from before any "
                "records existed) is not attached to this thread — ask the team to restate "
                "anything important from it")

    body = {"model": cl.model, "instructions": cl.persona,
            "input": build_envelope(user_text, contexts, org, speaker, note=note,
                                    fact_fields=cl.fact_fields)}
    if thread.prev:
        body["previous_response_id"] = thread.prev

    d = _dispatch(body, cl, thread)

    # Bookkeeping ONLY after a confirmed-complete turn (same rule as thread.prev): if the post
    # raised OR the response came back failed/still-running, the context was never delivered and
    # must not be marked as supplied.
    for c in contexts:
        # record the VERSION we sent, so the next turn can tell whether it is still current
        thread.supplied[c["record_id"]] = current.get(c["record_id"])
    if contexts:
        thread.ever_supplied = True
    thread.prev = d.get("id")
    thread.last_turn_at = _now()      # same rule as thread.prev: only after a confirmed turn
    return output_text(d), [c["record_id"] for c in contexts]


# --- Verification oracle: run the frozen hero flow through backend-supplied context ----------
HERO = [
    "Which of these prospects should we focus on?",
    "Why Northwind?",
    "Prepare me for the conversation with Northwind.",
    "They told me: budget's approved, and they need it live before their Q1 renewal surge — "
    "support triples then. They're on Zendesk.",
    "What changed?",
    "What should we do next?",
    "And should we bother with Apex Financial?",
]

if __name__ == "__main__":
    # Internal dev/demo flow: env-pair credentials + the INTERNAL composition, requested
    # explicitly by name — there is deliberately no ambient default persona.
    th = Thread(ClientConfig(slug="internal-dev",
                             ironclaw_token=os.environ["IRONCLAW_TOKEN"],
                             account_token=os.environ["ACCOUNT_TOKEN"],
                             persona=compose_persona()))
    for i, prompt in enumerate(HERO, 1):
        text, supplied = turn(th, prompt)
        print(f"\n===== TURN {i} =====")
        print(f"USER: {prompt}")
        print(f"[trusted context supplied for: {supplied or 'none (thread history)'}]")
        print(f"AGENT:\n{text[:1400]}")
