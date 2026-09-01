#!/usr/bin/env python3
"""Step 8: the whole IronWorks service path, through the contained runtime.

    fake Telegram -> bridge -> trusted context ingress -> IronClaw -> gateway -> provider

The raw-model proof (proof_checks.py) shows the RUNTIME works behind the boundary. This shows
the PRODUCT does: a real account-analysis turn on a real book, thread chaining, response
retrieval, bridge crash-recovery, and the architectural split that the containment must not
quietly break —

    trusted seam  ->  private business data      (must still work)
    IronClaw      ->  model provider only        (must be all it can reach)

Run by run-proof.sh --service-path, which owns the stack. It uses the eval org's synthetic
book: invented accounts on `.example` domains carrying a `_synthetic` banner, and no client
records of any kind.
"""
import atexit
import os
import pathlib
import shutil
import sys
import tempfile

REPO = pathlib.Path(os.environ.get("REPO") or pathlib.Path(__file__).resolve().parents[3])
sys.path.insert(0, str(REPO / "multi" / "seam"))
# The verify library, exactly as proof_checks.py next door reaches it: an operator-side proof
# may import the product and the proof helpers, and this file had grown its own copies of the
# scoreboard, the member mint and the member cleanup. The copies had already drifted — no
# browser User-Agent on the requests, and a minted member outside `common`'s at-exit leak
# backstop, which is the one thing that makes forgetting cleanup impossible.
sys.path.insert(0, str(REPO / "multi" / "verify"))
import common                                                            # noqa: E402

CLIENTS = pathlib.Path(os.environ.get("CLIENTS_DIR_SRC")
                       or pathlib.Path(os.environ.get("AGENCY_DIR")
                                       or pathlib.Path.home() / ".agency") / "clients")
SLUG = os.environ.get("PROOF_TENANT_SLUG", "eval")

checks = common.Checks()
check = checks.check


src_env = CLIENTS / f"{SLUG}.env"
src_guide = CLIENTS / f"{SLUG}.guidance.md"
if not src_env.exists() or not src_guide.exists():
    checks.block(f"the whole service path (tenant '{SLUG}')",
                 f"no '{SLUG}' tenant to borrow a synthetic book from ({CLIENTS})")
    checks.finish()

API = os.environ["PROOF_API"].rstrip("/")
OPERATOR = os.environ["PROOF_OPERATOR"]

account_token = ""
for line in src_env.read_text().splitlines():
    if line.strip().startswith("ACCOUNT_TOKEN="):
        account_token = line.split("=", 1)[1].strip().strip('"').strip("'")

# Registered for deletion by `mint_member` itself; the explicit hook below deletes it at exit
# rather than leaving it to common's leak sweep, which is worded as a leak because it is one.
member, uid = common.mint_member("egress service-path proof", OPERATOR, api=API)
atexit.register(common.delete_user, uid, OPERATOR, api=API)

with tempfile.TemporaryDirectory() as d:
    reg = pathlib.Path(d)
    shutil.copy(src_guide, reg / f"{SLUG}.guidance.md")
    (reg / f"{SLUG}.env").write_text(
        f"CLIENT_SLUG={SLUG}\nCLIENT_NAME=Egress Proof Tenant\n"
        f"ACCOUNT_TOKEN={account_token}\nIRONCLAW_TOKEN={member}\n"
        f"IRONCLAW_USER_ID={uid}\nTELEGRAM_GROUP_ID=-100999123\n")

    os.environ["CLIENTS_DIR"] = str(reg)
    os.environ["IRONCLAW_API"] = API
    os.environ.setdefault("TELEGRAM_BOT_USERNAME", "proof_bot")
    os.environ["BRIDGE_STATE"] = str(reg / "bridge-threads.json")

    import account_service as asvc
    import bridge_state as bs
    import context_ingress as ing
    import registry
    from responses import output_text
    import telegram_bridge as tb

    print("== the trusted seam still reaches the private business data ==")
    clients = asvc.resolve_account_scopes(registry.load_clients())
    cl = clients[SLUG]
    try:
        catalog = asvc._catalog(cl)
        check("the SEAM (on the host) reaches the private Account Service",
              catalog.get("org") is not None,
              f"got {str(catalog)[:80]}")
        common.note("book", f"org={catalog.get('org')} accounts={len(catalog.get('accounts', []))}")
    except Exception as e:
        check("the SEAM (on the host) reaches the private Account Service", False,
              f"{type(e).__name__}: {e}")

    print()
    print("== a real account-analysis turn, end to end, through the boundary ==")
    thread = ing.Thread(cl)
    text, supplied = ing.turn(thread, "Which accounts need attention? Name one and say why.",
                              speaker="Proof", budget=180)
    check("a normal account-analysis turn completes", bool(text) and bool(thread.prev),
          f"text={text[:90]!r}")
    check("the turn was GROUNDED in the tenant's own book (records were supplied)",
          bool(supplied), f"supplied={supplied}")
    common.note("answer", " ".join(text.split())[:160])
    first_prev = thread.prev

    # Guidance governs: the eval guidance names its own vocabulary, and the analyst is told to
    # use it. Evidence tagging is the discipline the persona mandates on every service.
    check("the analyst answered with the evidence discipline its guidance mandates",
          any(tag in text.upper() for tag in ("FACT", "INFERENCE", "UNKNOWN", "STATED")),
          "no evidence tags in the answer — guidance may not be reaching the model")

    print()
    print("== continuity, retrieval, and recovery ==")
    text2, _ = ing.turn(thread, "What would you need to know next?", speaker="Proof", budget=180)
    check("previous-response chaining works through the boundary",
          bool(text2) and thread.prev not in (None, first_prev),
          f"prev {first_prev} -> {thread.prev}")

    fetched = output_text(common.get("/v1/responses/" + first_prev, member, api=API))
    check("response retrieval by id still succeeds (bridge recovery depends on it)",
          fetched == text, "the retrieved text differs from what the turn returned")

    # The bridge's own recovery path, against the REAL contained runtime: crash after the turn
    # completes but before delivery, restart, and require that the stored answer is FETCHED and
    # sent rather than a second turn being run.
    class FakeTG:
        def __init__(self, batches):
            self.batches, self.sent = list(batches), []

        def get_updates(self, offset=None, timeout=25):
            return {"result": self.batches.pop(0) if self.batches else []}

        def send_message(self, chat_id, text):
            self.sent.append((str(chat_id), text))

    class Turns:
        runs = 0

        @staticmethod
        def run(thread, text, speaker=None, idempotency_key=None, budget=None):
            Turns.runs += 1
            reply, _ = ing.turn(thread, text, speaker=speaker,
                                idempotency_key=idempotency_key, budget=budget)
            return reply

        @staticmethod
        def fetch(client, response_id):
            return output_text(
                common.get("/v1/responses/" + response_id, client.ironclaw_token, api=API))

    class Clock:
        def monotonic(self):
            import time
            return time.monotonic()

        def sleep(self, s):
            pass

        def now_iso(self, offset=0):
            return "2026-08-24T00:00:00+00:00"

    gid = cl.telegram_group_id
    upd = {"update_id": 1, "message": {"message_id": 1, "chat": {"id": int(gid)},
                                       "from": {"first_name": "Proof"},
                                       "text": "@proof_bot summarise the book in one line"}}
    db = reg / "state.db"
    st = bs.BridgeState(db)
    groups = {gid: cl}
    threads = tb._load_threads(groups, st)

    class Boom(BaseException):
        pass

    class CrashTG(FakeTG):
        def send_message(self, chat_id, text):
            raise Boom("crash after the turn, before delivery")

    b = tb.TelegramBridge(groups=groups, threads=threads, telegram=CrashTG([[upd]]),
                          turns=Turns(), state=st, clock=Clock(), log=lambda *_: None,
                          budget_seconds=180)
    try:
        b.poll_once()
    except Boom:
        pass
    row = st.update_row(1)
    check("after a crash before delivery, the answer is durably recorded",
          row and row["response_id"], f"row={dict(row) if row else None}")
    st.close()

    runs_before = Turns.runs
    st2 = bs.BridgeState(db)
    tg2 = FakeTG([[upd]])
    b2 = tb.TelegramBridge(groups=groups, threads=tb._load_threads(groups, st2), telegram=tg2,
                           turns=Turns(), state=st2, clock=Clock(), log=lambda *_: None,
                           budget_seconds=180)
    b2.poll_once()
    check("on restart the STORED answer is fetched and delivered — no second model turn",
          Turns.runs == runs_before and len(tg2.sent) == 1,
          f"model runs {runs_before}->{Turns.runs}, sends={len(tg2.sent)}")
    st2.close()

    print()
    print("== the architectural split survives containment ==")
    # A NOTE, not a check: this leg has no assertion behind it — the evidence is the gateway's
    # own decision log, read by proof_checks.py. Scored as a check it added one to both sides
    # of the score line and could never fail, which is an unfalsifiable claim dressed as proof.
    common.note("IronClaw reached ONLY the model provider", "see the gateway decision log")
    common.note("split", "trusted seam -> private business data; IronClaw -> model provider only")

print()
checks.finish("SERVICE PATH PASSES UNDER CONTAINMENT — the product works, not just the runtime.")
