#!/usr/bin/env python3
"""Telegram channel side of the trusted context ingress — thin, boring.

Long-polls ONE bot and routes each private group to ITS client: chat.id -> ClientConfig from
the registry (~/.agency/clients/*.env, see multi/clients/README.md), one Thread per group,
each turn made with that client's own tokens (`context_ingress.turn`). NOT an agent loop —
one message in, one reply out; deterministic prefetch only. It adds no reasoning.

Only responds inside a registered group, and only when the bot is @mentioned or the message is
a reply to one of the bot's own messages. All members of a group share that group's IronClaw
thread (the group conversation); the speaker's name is passed as attribution metadata.
(These are exactly the messages a bot receives under Telegram privacy mode, so the shared bot
need not read other groups' ordinary chatter.)

DELIBERATE: one bot + one process serves every client group. A bot token allows only one
getUpdates poller, so per-client processes would force per-client bots; isolation lives in the
seam (per-thread credentials) and the sealed accounts, not in process count. The trade-off is
honest — this process is a single point of failure for all clients, and the bot token reads
every client group. Thread state ({prev, supplied} per group) persists to BRIDGE_STATE so a
restart resumes conversations instead of silently resetting every client.

Env:
  TELEGRAM_BOT_TOKEN    — the bot (BotFather)
  TELEGRAM_BOT_USERNAME — for @mention detection (no leading @)
  CLIENTS_DIR           — client registry (default ~/.agency/clients)
  BRIDGE_STATE          — thread-state file (default ~/.agency/bridge-threads.json)
  plus context_ingress env: IRONCLAW_API
"""
import datetime, os, re, json, signal, time, pathlib, urllib.request, urllib.parse
try:
    from . import bridge_core
    from . import bridge_state as bs
    from . import context_ingress as ing
    from . import redact as redact_mod
    from .operator_paths import agency_dir
except ImportError:  # direct-script compatibility during service-unit rollout
    import bridge_core
    import bridge_state as bs
    import context_ingress as ing
    import redact as redact_mod
    from operator_paths import agency_dir

def configured_username():
    """The @mention name this bridge answers to, RESOLVED ON USE — never bound at import.

    It was `BOT_USERNAME = os.environ.get(...)` at module scope, and
    `_bridge_delivery_support.py` sets `TELEGRAM_BOT_USERNAME` when IT is imported. Whichever
    happened first won for the whole process: collect `test_thread_compatibility.py` (which
    imports this module) before `test_bridge_delivery.py` and the name was `""`, `addressed()`
    returned None for every fixture, and thirteen delivery/recovery/operations tests failed
    together. Nothing was wrong with either file; the suite passed only because the alphabet put
    them in a working order. Same trap as `state_json_path()` below, and as `_bot()` above.

    An explicit `tb.BOT_USERNAME = …` still wins, because `test_telegram_bridge` uses that to
    drive the deaf-bot guard. There is no module-level binding of that name, so `globals()` holds
    it only when a caller has deliberately assigned it — and `__getattr__` serves the read when
    nobody has.
    """
    override = globals().get("BOT_USERNAME")
    if override is not None:
        return override
    return os.environ.get("TELEGRAM_BOT_USERNAME", "")


def _bot():
    """This bridge's Telegram bot token. Resolved on first use, not at import — see
    `context_ingress._api` for why, and for what fail-fast still guarantees. The deaf-bot guard
    that checks TELEGRAM_BOT_USERNAME at startup is unaffected: it runs in `main()`, which is a
    use, not an import."""
    v = os.environ.get("TELEGRAM_BOT_TOKEN")
    if not v:
        raise ing.SeamNotConfigured(
            "TELEGRAM_BOT_TOKEN is unset — this bridge has no bot to poll as.")
    return v


def _api_base():
    return f"https://api.telegram.org/bot{_bot()}"


def __getattr__(name):
    """`tb.BOT` stays available and lazy (PEP 562) for the redaction test, which asserts the bot
    token is among the secrets scrubbed from logs and wants the resolved value, not a literal.

    `API` was here too and had no reader at all; `context_ingress` carried the same shim for
    `IRONCLAW_API`, whose last reader was this module's own `_Turns.fetch` before that call moved
    to `ing.fetch_response`. Module-level `__getattr__` is a lot of mechanism to leave standing
    for nobody.

    Consulted only for names that are NOT already module globals, so a test assigning
    `tb.BOT_USERNAME` shadows normally — and `addressed`/`summoned` now read that global at call
    time rather than binding it as a default, so the assignment reaches them too."""
    if name == "BOT":
        return _bot()
    if name == "BOT_USERNAME":
        # Reached only while nobody has assigned it — an assignment creates the global and
        # shadows this, which is exactly what the deaf-bot test relies on.
        return configured_username()
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

# The legacy JSON thread file. It is now a MIGRATION SOURCE, not the live store — but the env
# var keeps its name and its meaning as "where this bridge's state lives", so every existing
# redirect (tests, the freshness proof's safety assertion, an operator's muscle memory) still
# isolates the store. The database is derived from it unless named explicitly, so redirecting
# one redirects both; a test that isolated the JSON path can never accidentally open the
# operator's real database.
# RESOLVED AT CALL TIME, not at import. Capturing these at import made a test that redirected
# BRIDGE_STATE *after* importing this module silently open the DEFAULT store — which, because
# opening it MIGRATES the operator's real thread file, is a side effect on a live host rather
# than a stale read. That is the same import-time-capture trap that already produced one
# order-dependent failure in this suite; a function cannot be captured early.
def state_json_path():
    """The legacy JSON thread file — now a MIGRATION SOURCE, not the live store. The env var
    keeps its name and meaning, so every existing redirect still isolates the state."""
    return pathlib.Path(os.environ.get("BRIDGE_STATE") or agency_dir("bridge-threads.json"))


def state_db_path():
    """The store. Derived from the JSON path unless named explicitly, so redirecting one
    redirects both and a test can never reach the operator's real database."""
    return pathlib.Path(os.environ.get("BRIDGE_STATE_DB")
                        or state_json_path().with_suffix(".db"))


def open_state(db_path=None, json_path=None):
    """The durable store, migrating the legacy JSON thread file in on first use.

    One store, because the conversation pointer and the update journal must not be able to
    disagree after a crash — see bridge_state.py. The migration is explicit and refuses the
    pre-versioning shape rather than coercing it, exactly as the JSON loader did."""
    src, dst = json_path or state_json_path(), db_path or state_db_path()
    st, n = bs.migrate_from_json(src, dst)
    if n:
        print(f"migrated {n} group thread(s) from {src} into {dst}; "
              "the JSON file is KEPT as a backup", flush=True)
    return st


def load_groups():
    """group_id -> ClientConfig, registry clients with a TELEGRAM_GROUP_ID. FAIL CLOSED on an
    empty registry — the SALES_GROUP_ID env-pair fallback was removed: it hand-built
    a client with no guidance-validated persona, so a misconfigured registry silently served a
    client group with the wrong (internal) composition."""
    groups = {str(c.telegram_group_id): c
              for c in ing.load_clients().values() if c.telegram_group_id}
    if not groups:
        raise RuntimeError("no client groups: populate CLIENTS_DIR (TELEGRAM_GROUP_ID per client)")
    return groups


# Never let a credential reach stdout or a group (e.g. a token-bearing URL inside an error).
# The redactor itself lives in redact.py so every printing path in the tree can reach the SAME
# one: this used to be a local four-liner that replaced exact substrings only, which meant a
# percent-encoded token in a urllib error, or an `Authorization:` line in a formatted
# exception, passed straight through — and nothing outside this file could redact at all.
def _secrets(groups):
    return redact_mod.secrets_of(groups, _bot())


def _redact(s, secrets):
    return redact_mod.redact(s, secrets)


# --- persisted thread state: a restart resumes conversations instead of resetting them -------
# Backed by the store rather than a JSON file, so a thread pointer and the update that produced
# it are written in ONE transaction. `state` is injectable for the same reason `path` used to
# be: process-global mutable state that one test sets and every later test inherits is its own
# defect class, and it had already produced one order-dependent failure here.
def _load_threads(groups, state=None):
    st = state or open_state()
    threads = {}
    for gid, client in groups.items():
        if not client.organization_verified:
            raise ing.AccountScopeError(
                f"tenant {client.slug!r}: organization identity was not verified by the Account "
                "Service before thread loading. Refusing continuation.")
        th = ing.Thread(client)
        row = st.thread_row(gid)
        if row:
            mismatches = st.identity_mismatches(row, client)
            if mismatches:
                # A v1/JSON row has no identity. Binding is safe only when there is literally
                # no conversation or context state to preserve; active legacy state cannot be
                # attributed after the fact, so continuation would be a guess.
                if (mismatches == ["legacy compatibility identity missing"]
                        and not st.thread_has_state(row)):
                    st.bind_thread_identity(gid, client)
                    row = st.thread_row(gid)
                else:
                    raise bs.ThreadCompatibilityError(client.slug, gid, mismatches)
            th.prev = row["prev"]
            # `supplied` is {record_id: updated_at-as-supplied}. The store's column is typed,
            # so the pre-versioning list shape cannot arrive here any more — it is refused at
            # MIGRATION instead (bridge_state.migrate_from_json), which is the same refusal in
            # the one place that can still see the old shape.
            th.supplied = dict(json.loads(row["supplied"] or "{}"))
            # ever_supplied MUST survive a restart, else the first post-restart turn that injects
            # a NEW account trips the data-starvation recovery (context_ingress.turn) and nulls
            # thread.prev — silently discarding the live conversation.
            th.ever_supplied = bool(row["ever_supplied"])
            th.last_turn_at = row["last_turn_at"]
            th.orphans = {aid: (v[0], v[1]) for aid, v in
                          json.loads(row["orphans"] or "{}").items()
                          if isinstance(v, (list, tuple)) and len(v) == 2}
        threads[gid] = th
    return threads


def _save_threads(threads, state=None):
    """Persist thread state for the given groups.

    UNROUTED ENTRIES SURVIVE, as before: this writes only the groups it was handed, and a row
    for a group absent from the registry is left alone. Removal stays EXPLICIT — deprovision.sh
    calls drop_thread; absence from the registry is not a deletion signal.

    Kept as a named function because the proofs and the freshness lifecycle test drive thread
    state through it. The serving path does NOT use it: a turn's pointer is committed inside
    `state.commit_turn`, together with the update that produced it.
    """
    st = state or open_state()
    with st._tx():
        for gid, th in threads.items():
            st._write_thread(gid, th)


def tg(method, **params):
    data = urllib.parse.urlencode(params).encode()
    with urllib.request.urlopen(f"{_api_base()}/{method}", data=data, timeout=60) as x:
        return json.loads(x.read())


_MD_FENCE = re.compile(r"^\s*```.*$")
_MD_HEADING = re.compile(r"^(\s*)#{1,6}\s+")
_MD_BOLD = re.compile(r"\*\*(?=\S)(.+?)(?<=\S)\*\*", re.S)
_MD_UNDERBOLD = re.compile(r"__(?=\S)(.+?)(?<=\S)__", re.S)
_MD_CODE = re.compile(r"`(?=\S)([^`\n]+?)(?<=\S)`")


def to_plain(text):
    """Strip the markdown this channel cannot render, deterministically.

    We send with NO `parse_mode`: Telegram then shows every marker literally, so a
    briefing arrives as `**FIT:**` and `# Summary`. Prompt guidance alone does NOT fix this —
    measured: telling the analyst "no markdown at all" cut bold spans from ~6 to 3
    per reply but never to 0; models reach for emphasis. Compliance is probabilistic, so the
    guarantee has to be deterministic and live here.

    Stripping, not converting: setting `parse_mode` would make Telegram 400 the whole message
    on any unescaped entity — a formatting nicety that can silently stop client delivery is a
    bad trade on the one channel clients actually use.

    Conservative by construction: each pattern needs a non-space character adjacent to the
    marker, so `2 * 3 * 4`, a bare `**`, and `a_b_c` are untouched. Fenced blocks lose only
    their fences; the code inside is kept verbatim (it is often the evidence)."""
    out = []
    for line in text.split("\n"):
        if _MD_FENCE.match(line):
            continue                                    # drop the fence, keep the contents
        line = _MD_HEADING.sub(r"\1", line)             # '## Summary' -> 'Summary'
        out.append(line)
    text = "\n".join(out)
    text = _MD_BOLD.sub(r"\1", text)
    text = _MD_UNDERBOLD.sub(r"\1", text)
    text = _MD_CODE.sub(r"\1", text)
    return text


def _chunks(text, limit=3800):
    """Split a reply on LINE boundaries under Telegram's 4096-char limit — a mid-line hard
    slice cuts sentences (and briefing lines) in half in front of the client. Only a single
    line longer than the limit is ever hard-split."""
    if not text:
        return ["(no response)"]
    out, cur = [], ""
    for line in text.split("\n"):
        while len(line) > limit:                    # pathological single line: hard-split
            if cur:
                out.append(cur); cur = ""
            out.append(line[:limit]); line = line[limit:]
        if not cur:
            cur = line
        elif len(cur) + 1 + len(line) <= limit:
            cur += "\n" + line
        else:
            out.append(cur); cur = line
    if cur:
        out.append(cur)
    return out or ["(no response)"]


def send(chat_id, text):
    # to_plain BEFORE _chunks: stripping shortens lines, so chunking must measure what is
    # actually sent, and a marker must never be split across two messages.
    acknowledged = 0
    for chunk in _chunks(to_plain(text)):
        try:
            result = tg("sendMessage", chat_id=chat_id, text=chunk)
            if not isinstance(result, dict) or result.get("ok") is not True:
                raise bridge_core.DeliveryAttemptError(
                    "Telegram explicitly rejected sendMessage",
                    acknowledged_chunks=acknowledged, known_not_sent=(acknowledged == 0))
            acknowledged += 1
        except bridge_core.DeliveryAttemptError:
            raise
        except urllib.error.HTTPError as e:
            raise bridge_core.DeliveryAttemptError(
                f"Telegram sendMessage returned HTTP {e.code}",
                acknowledged_chunks=acknowledged,
                known_not_sent=(acknowledged == 0 and 400 <= e.code < 500)) from e
        except Exception as e:
            raise bridge_core.DeliveryAttemptError(
                f"Telegram sendMessage outcome unknown ({type(e).__name__})",
                acknowledged_chunks=acknowledged, known_not_sent=False) from e


def addressed(msg, bot_username=None):
    """Return the request text if this message is addressed to the bot — @mention (stripped),
    or a reply to one of the bot's own messages — else None. Telegram usernames are
    case-insensitive, so everything matches casefolded. THE one addressed-test, shared by
    routing and the observability log.

    `None` means "use the configured name", read HERE rather than bound as a default. A default
    of `BOT_USERNAME` is evaluated once, when this `def` executes at import — so reassigning
    `tb.BOT_USERNAME` (which `__getattr__`'s docstring says shadows normally, and which
    test_telegram_bridge does) changed the `main()` guard and NOT this function. Two mechanisms
    for one setting, and only one of them visible. An explicit `""` still means "no name".
    """
    if bot_username is None:
        bot_username = configured_username()
    t = msg.get("text", "")
    if not t or not bot_username:
        return None
    mention = re.search(rf"@{re.escape(bot_username)}\b", t, re.IGNORECASE)
    if mention:
        return (t[:mention.start()] + t[mention.end():]).strip()
    reply_from = (msg.get("reply_to_message") or {}).get("from") or {}
    if (reply_from.get("username") or "").casefold() == bot_username.casefold():
        return t.strip()
    return None


def summoned(msg, groups, bot_username=None):
    """Return (group_id, request_text) if this message is addressed to us inside a registered
    client group, else None. Pure function — the routing decision in one testable place.

    `bot_username=None` means "the configured name" — see `addressed` for why it is not a
    default-argument binding."""
    gid = str(msg.get("chat", {}).get("id"))
    if gid not in groups:
        return None
    req = addressed(msg, bot_username)
    return (gid, req) if req is not None else None


class _Clock:
    """Real time, injected so the crash-boundary tests can be deterministic."""

    @staticmethod
    def monotonic():
        return time.monotonic()

    @staticmethod
    def sleep(seconds):
        time.sleep(seconds)

    @staticmethod
    def now_iso(offset=0):
        return (datetime.datetime.now(datetime.timezone.utc)
                + datetime.timedelta(seconds=offset)).replace(microsecond=0).isoformat()


class _Telegram:
    """The channel. `send_message` applies the deterministic plain-text + chunking rules, so
    everything that reaches a client goes through exactly one formatter."""

    @staticmethod
    def get_updates(offset=None, timeout=25):
        return tg("getUpdates", timeout=timeout, **({"offset": offset} if offset else {}))

    @staticmethod
    def send_message(chat_id, text):
        send(chat_id, text)


class _Turns:
    """IronClaw: run a turn, or fetch one that already ran."""

    @staticmethod
    def run(thread, text, speaker=None, idempotency_key=None, budget=None):
        reply, _ = ing.turn(thread, text, speaker=speaker,
                            idempotency_key=idempotency_key, budget=budget)
        return reply

    @staticmethod
    def fetch(client, response_id):
        """The exact answer that already ran. Scoped to the tenant's own sealed member — a
        response id is not a capability (measured: another tenant asking for it gets 404,
        multi/verify/test_responses_recovery.py)."""
        doc = ing.fetch_response(client, response_id)
        text = ing.output_text(doc)
        if not text:
            raise RuntimeError(f"response {response_id} carries no output text "
                               f"(status={doc.get('status')!r})")
        return text


class TelegramBridge(bridge_core.Bridge):
    """The channel vocabulary, kept where the channel lives."""

    def summoned(self, msg):
        return summoned(msg, self.groups)

    def addressed(self, msg):
        return addressed(msg)


def main():
    if not configured_username():
        # Without the username, addressed() matches NOTHING: the bot would run "healthy"
        # while deaf in every client group. Fail loudly at startup instead.
        # A CALL, not a bare `BOT_USERNAME`: module-level `__getattr__` (PEP 562) serves
        # attribute access from OUTSIDE the module and is not consulted for a global lookup
        # inside it, so the bare name would be a NameError here rather than a read.
        raise RuntimeError("TELEGRAM_BOT_USERNAME is not set — the bot cannot detect @mentions "
                           "and would silently ignore every group message")
    groups = load_groups()
    # Every tenant must be a SEALED MEMBER, verified against the runtime rather than against this
    # process's environment — see ing.assert_no_member_is_the_operator for why the environment
    # comparison in load_clients cannot fire here. At startup, never at registry load (D-077):
    # load_clients has to stay usable on a clean clone with no instance. Fails closed.
    ing.assert_no_member_is_the_operator({c.slug: c for c in groups.values()})
    groups = ing.resolve_account_scopes(groups)
    secrets = _secrets(groups)
    state = open_state()
    threads = _load_threads(groups, state)

    def log(line):
        print(_redact(line, secrets), flush=True)

    bridge = TelegramBridge(
        groups=groups, threads=threads, telegram=_Telegram(), turns=_Turns(), state=state,
        clock=_Clock(), log=log, budget_seconds=ing.TURN_BUDGET_SECONDS,
        redact=lambda e: _redact(e, secrets))

    # GRACEFUL STOP. `systemctl restart` sends SIGTERM, and a restart is ROUTINE — the registry
    # is read once at startup, so adding a tenant means restarting. Killed mid-turn, that lands
    # in the one window the runtime cannot recover (see bridge_core), and the client gets an
    # error instead of an answer. Each active tenant worker finishes its current update; queued
    # updates remain RECEIVED and keep the global cursor behind them for the replacement process.
    def _stop(signum, _frame):
        if bridge.stopping:
            return                       # a second signal: let the default disposition apply
        bridge.stopping = True
        log(f"[shutdown] signal {signum} received — finishing active tenant updates "
            f"(at most {ing.TURN_BUDGET_SECONDS:.0f}s), then stopping")
    for _sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(_sig, _stop)

    log("trusted context ingress (telegram) serving "
        + ", ".join(f"{c.slug}@{gid}" for gid, c in sorted(groups.items()))
        + f" | state={state_db_path()} cursor={state.cursor} budget={ing.TURN_BUDGET_SECONDS:.0f}s")
    try:
        bridge.run()
    finally:
        state.close()


if __name__ == "__main__":
    main()
