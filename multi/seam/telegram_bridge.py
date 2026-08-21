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
import os, re, json, time, pathlib, tempfile, urllib.request, urllib.parse
import context_ingress as ing

BOT = os.environ["TELEGRAM_BOT_TOKEN"]
BOT_USERNAME = os.environ.get("TELEGRAM_BOT_USERNAME", "")
API = f"https://api.telegram.org/bot{BOT}"
STATE_PATH = pathlib.Path(os.environ.get("BRIDGE_STATE")
                          or os.path.expanduser("~/.agency/bridge-threads.json"))


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
def _secrets(groups):
    s = {BOT}
    for c in groups.values():
        s.update((c.ironclaw_token, c.account_token))
    return {x for x in s if x}


def _redact(s, secrets):
    s = str(s)
    for sec in secrets:
        s = s.replace(sec, "<redacted>")
    return s


# --- persisted thread state: a restart resumes conversations instead of resetting them -------
def _load_threads(groups):
    threads = {}
    try:
        saved = json.loads(STATE_PATH.read_text())
    except (FileNotFoundError, ValueError):
        saved = {}
    for gid, client in groups.items():
        th = ing.Thread(client)
        st = saved.get(gid)
        if st:
            th.prev = st.get("prev")
            # `supplied` is {record_id: updated_at-as-supplied}. There is deliberately no
            # reader for the older list-of-ids shape: REFUSE it rather than coerce it. Coercing
            # to {} would derive ever_supplied=False for a thread that HAS had context, and that
            # is precisely the condition that trips the starvation recovery and nulls
            # thread.prev — silently discarding a live client conversation to save one migration.
            sup = st.get("supplied", {})
            if not isinstance(sup, dict):
                raise ValueError(
                    f"{STATE_PATH}: group {gid} has a pre-versioning 'supplied' list. Migrate once:\n"
                    "  python3 - <<'EOF'\n"
                    "  import json, pathlib\n"
                    f"  p = pathlib.Path({str(STATE_PATH)!r}); d = json.loads(p.read_text())\n"
                    "  for st in d.values():\n"
                    "      if isinstance(st.get('supplied'), list):\n"
                    "          st['ever_supplied'] = st.get('ever_supplied', bool(st['supplied']))\n"
                    "          st['supplied'] = {a: None for a in st['supplied']}\n"
                    "  p.write_text(json.dumps(d, indent=1))\n"
                    "  EOF")
            th.supplied = dict(sup)
            # ever_supplied MUST survive a restart, else the first post-restart turn that injects a
            # NEW account trips the data-starvation recovery (context_ingress.turn) and nulls
            # thread.prev — silently discarding the live conversation. Old state files predate the
            # field; derive it from supplied (a thread with supplied accounts has had context).
            th.ever_supplied = st.get("ever_supplied", bool(th.supplied))
        threads[gid] = th
    return threads


def _save_threads(threads):
    """Persist live thread state WITHOUT destroying the state of groups that aren't loaded.

    Two properties, both learned the hard way:

    1. UNROUTED ENTRIES SURVIVE. This used to serialize only `threads`, so any gid absent
       from the registry at startup was erased on the very next save — an operator moving a
       client env aside, restarting, and putting it back lost that group's prev/supplied/
       ever_supplied for good. Losing `ever_supplied` is precisely the condition that nulls a
       live conversation (see _load_threads). Removal must stay EXPLICIT: deprovision.sh
       deletes a client's entry deliberately; absence is not a deletion signal.
    2. THE FILE IS NEVER WORLD-READABLE, not even briefly. The old order was write → replace
       → chmod, which published every group's response ids at the process umask for the
       window in between. mkstemp+fchmod sets the mode before any content exists — the same
       pattern multi/provision/deprovision.sh already uses for the identities file.
    """
    STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    try:
        saved = json.loads(STATE_PATH.read_text())
    except (FileNotFoundError, ValueError):
        saved = {}
    data = {gid: st for gid, st in saved.items() if gid not in threads}
    data.update({gid: {"prev": th.prev,
                       "supplied": {aid: th.supplied[aid] for aid in sorted(th.supplied)},
                       "ever_supplied": th.ever_supplied}
                 for gid, th in threads.items()})
    fd, tmp = tempfile.mkstemp(dir=str(STATE_PATH.parent), prefix=".bridge-threads-", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(data, f)
        os.replace(tmp, STATE_PATH)
    except BaseException:
        os.unlink(tmp)
        raise


def tg(method, **params):
    data = urllib.parse.urlencode(params).encode()
    with urllib.request.urlopen(f"{API}/{method}", data=data, timeout=60) as x:
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
    for chunk in _chunks(to_plain(text)):
        tg("sendMessage", chat_id=chat_id, text=chunk)


def addressed(msg, bot_username=BOT_USERNAME):
    """Return the request text if this message is addressed to the bot — @mention (stripped),
    or a reply to one of the bot's own messages — else None. Telegram usernames are
    case-insensitive, so everything matches casefolded. THE one addressed-test, shared by
    routing and the observability log."""
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


def summoned(msg, groups, bot_username=BOT_USERNAME):
    """Return (group_id, request_text) if this message is addressed to us inside a registered
    client group, else None. Pure function — the routing decision in one testable place."""
    gid = str(msg.get("chat", {}).get("id"))
    if gid not in groups:
        return None
    req = addressed(msg, bot_username)
    return (gid, req) if req is not None else None


def main():
    if not BOT_USERNAME:
        # Without the username, addressed() matches NOTHING: the bot would run "healthy"
        # while deaf in every client group. Fail loudly at startup instead.
        raise RuntimeError("TELEGRAM_BOT_USERNAME is not set — the bot cannot detect @mentions "
                           "and would silently ignore every group message")
    groups = load_groups()
    secrets = _secrets(groups)
    threads = _load_threads(groups)
    offset = None
    print("trusted context ingress (telegram) serving "
          + ", ".join(f"{c.slug}@{gid}" for gid, c in sorted(groups.items())))
    while True:
        try:
            r = tg("getUpdates", timeout=25, **({"offset": offset} if offset else {}))
        except Exception as e:
            print("poll error:", _redact(e, secrets)); time.sleep(3); continue
        for upd in r.get("result", []):
            offset = upd["update_id"] + 1
            msg = upd.get("message") or upd.get("edited_message")
            if not msg:
                continue
            hit = summoned(msg, groups)
            if hit is None:
                # observability: a summon attempt from an UNREGISTERED group is the one silent
                # failure worth logging (e.g. Telegram changed the group id on supergroup upgrade)
                gid = msg.get("chat", {}).get("id")
                if str(gid) not in groups and addressed(msg) is not None:
                    print(f"[ignored] summon from unregistered chat {gid} "
                          f"({msg.get('chat', {}).get('title', '?')})")
                continue
            gid, req = hit
            speaker = msg.get("from", {}).get("first_name") or "Someone"
            try:
                # speaker is attribution metadata, NOT a message prefix, so a person's name
                # can never be parsed as an account (Session-1 fix).
                text, _ = ing.turn(threads[gid], req, speaker=speaker)
                _save_threads(threads)
                send(gid, text)
            except Exception as e:
                # The client gets a plain-language apology; the DETAIL goes to the operator
                # log only (a raw exception in a client group leaks internals and reads as
                # broken). The notify itself must never crash the shared loop.
                print(f"[turn error] {groups[gid].slug}@{gid}: {_redact(e, secrets)}", flush=True)
                try:
                    send(gid, "Sorry — I hit a technical problem with that request. "
                              "The operator has been notified; please try again in a minute.")
                except Exception as e2:
                    print(f"[error-notify failed] {gid}: {_redact(e2, secrets)}", flush=True)


if __name__ == "__main__":
    main()
