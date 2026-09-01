"""The bridge's delivery state machine — the part that has to be right across a crash.

`telegram_bridge.py` keeps the channel vocabulary (what counts as a summon, how markdown is
stripped, how a reply is chunked). This module owns the part that decides, for one Telegram
update, whether a model turn runs, whether an answer is delivered, and what the durable record
says afterwards. It is here rather than inside a `while True` because none of that was testable
before: the only test that reached `main()` asserted the startup guard and returned before the
loop, so cursor handling, delivery and duplicate suppression had no coverage at all.

Everything it touches is injected — Telegram, IronClaw, the durable store, the clock, the
notifier. A crash is then just "stop calling methods", which is what makes the boundary tests
in the behavior-focused bridge tests possible.

## The semantics, stated exactly (and no more than is true)

  MODEL EXECUTION — at-most-once, except across one window that the pinned runtime cannot
  close. Once a response id is durable, the answer is fetched, never regenerated. Before it is
  durable, the bridge does not know whether a turn ran, and cannot find out: replaying the
  idempotency key needs the exact original request body, the body carries `retrieved_at` and
  the account records, and a key replayed against a different body is refused `409` without
  naming the response it conflicts with (all measured — multi/verify/test_responses_recovery.py).
  There is no lookup-by-key route. So that window ends in RECOVERY_BLOCKED: the client is told
  once, the operator sees it, and no second turn is run. Never a silent regeneration.

  TELEGRAM DELIVERY — acknowledged-complete, retryable-known-unsent, or reconciliation-needed.
  Each acknowledged chunk is evidence, but an exception may leave a chunk's acceptance unknown.
  A known-unsent first chunk retains the response under DELIVERY_RETRY. Any partial or ambiguous
  attempt retains it under DELIVERY_RECONCILE. Both require explicit, model-free redelivery.

  PROCESSING — serial within a group and bounded-concurrent across groups. Every fetched batch
  is journaled before workers start, so an out-of-order completion cannot advance Telegram's
  global cursor past an earlier unfinished update.

Nothing here is exactly-once, and nothing here should ever be described as exactly-once.
"""
import os
import uuid
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed

try:
    from . import bridge_state as bs
except ImportError:  # direct-script compatibility during service-unit rollout
    import bridge_state as bs

# Every outcome `handle_update` can return, and its RETURN CONTRACT — not decoration. `poll_once`
# records the latest one as `last_outcome` in the progress snapshot, so `ironworks bridge status`
# can say what the bridge last DID with an update. That distinguishes the two shapes a stale
# heartbeat cannot: polling happily while every turn fails, versus genuinely idle.
#
# It was write-only before, with a comment claiming tests asserted on it and a watchdog counted
# it. Neither was true — `poll_once` discarded the value — so the one place that wanted an
# outcome (the former monolithic delivery suite) asserted on LOG TEXT instead, exactly the coupling
# these constants exist to avoid.
OUT_IGNORED = "ignored"
OUT_ANSWERED = "answered"
OUT_REDELIVERED = "redelivered-same-answer"
OUT_ALREADY_DELIVERED = "already-delivered"
OUT_ALREADY_TERMINAL = "already-terminal"
OUT_RECOVERY_BLOCKED = "recovery-blocked"
OUT_DELIVERY_RETRY = "delivery-known-unsent-retry"
OUT_DELIVERY_RECONCILE = "delivery-reconciliation-needed"
OUT_FAILED = "failed"

# What a client is told. One sentence, no internals, and the SAME sentence whatever broke —
# a client-facing error that varies with the cause is a channel for internal detail.
CLIENT_FAILURE = ("Sorry — I hit a technical problem with that request. The operator has been "
                  "notified; please try again in a minute.")
CLIENT_BLOCKED = ("Sorry — I lost track of that request while restarting and I'm not able to "
                  "answer it safely. The operator has been notified; please wait for them to "
                  "reconcile it before sending that request again.")


class DeliveryAttemptError(RuntimeError):
    """Evidence carried by one chunked Telegram delivery attempt."""

    def __init__(self, message, acknowledged_chunks=0, known_not_sent=False):
        super().__init__(message)
        self.acknowledged_chunks = int(acknowledged_chunks)
        self.known_not_sent = bool(known_not_sent)


class Bridge:
    """One update at a time per tenant, durably; multiple tenants may run concurrently.

    transports (all injected):
      telegram.get_updates(offset, timeout) -> {"result": [...]}
      telegram.send_message(chat_id, text)
      turns.run(thread, text, speaker, idempotency_key, budget) -> reply text
      turns.fetch(client, response_id) -> reply text (raises if unavailable)
      state:  bridge_state.BridgeState
      clock.monotonic() / clock.now_iso()
      log(str)
    """

    def __init__(self, groups, threads, telegram, turns, state, clock, log,
                 budget_seconds, redact=lambda s: s, max_workers=None):
        self.groups = groups
        self.threads = threads
        self.tg = telegram
        self.turns = turns
        self.state = state
        self.clock = clock
        self.log = log
        self.budget = budget_seconds
        self.redact = redact
        self.stopping = False
        configured = max_workers if max_workers is not None else os.environ.get(
            "BRIDGE_MAX_WORKERS", "4")
        try:
            self.max_workers = max(1, min(16, int(configured)))
        except (TypeError, ValueError) as e:
            raise ValueError("BRIDGE_MAX_WORKERS must be an integer from 1 to 16") from e

    # ── one update ────────────────────────────────────────────────────────────────────
    def handle_update(self, upd):
        uid = int(upd["update_id"])
        row = self.state.update_row(uid)

        if row is not None and row["state"] in (bs.DELIVERY_RETRY, bs.DELIVERY_RECONCILE):
            self._advance()
            return (OUT_DELIVERY_RETRY if row["state"] == bs.DELIVERY_RETRY
                    else OUT_DELIVERY_RECONCILE)

        # A finished update is finished. No model call, no send, whatever Telegram thinks.
        if row is not None and row["state"] in bs.TERMINAL:
            self._advance()
            return OUT_ALREADY_TERMINAL

        # Delivered, but Telegram had not been acknowledged before the crash. Do not resend and
        # do not rerun; just advance toward acknowledgement. Tested HERE, before routing, because
        # it does not need a group: hoisting it keeps a delivered row whose tenant has left the
        # registry from being re-recorded as IGNORED, which would overwrite `delivered_at` and
        # the response id — the evidence a duplicate-delivery report from a client is diagnosed
        # from days later, and the reason compaction retains terminal rows at all.
        if row is not None and row["state"] == bs.DELIVERED:
            self._advance()
            return OUT_ALREADY_DELIVERED

        msg = upd.get("message") or upd.get("edited_message")
        summon = None
        if msg is not None:
            summon = self.summoned(msg)

        # AN IN-FLIGHT ROW IS NOT AN UNADDRESSED MESSAGE. The two branches above cover terminal
        # and delivery-recovery states, so TURN_STARTED / TURN_COMPLETED / DELIVERY_STARTED fell
        # through to the ROUTING test below — and `summoned()` returns None for any chat absent
        # from the current registry snapshot. A crashed bridge restarted while a client env is
        # moved aside (the operator move `_save_threads`' own docstring describes), a
        # deprovision-while-in-flight, or one malformed env that `load_clients` drops, all put a
        # durable and possibly already-BILLED answer through `note_terminal(IGNORED)`: no
        # error_code, cursor advanced past it, uncountable by `health()`, and unrecoverable
        # afterwards because the row is terminal. Recorded as "not addressed to us" — the one
        # thing it demonstrably was.
        #
        # Routing decides whether to ANSWER. It cannot decide what already happened.
        if summon is None and row is not None and row["state"] in bs.IN_FLIGHT:
            self.state.note_terminal(uid, bs.RECOVERY_BLOCKED, "route_lost_in_flight")
            self.log(f"[recovery-blocked] update {uid} was {row['state']} but its group is no "
                     "longer in the registry — a turn may have run and cannot be delivered; "
                     "operator reconciliation required (SECURITY.md)")
            return OUT_RECOVERY_BLOCKED

        if summon is None:
            gid = None if msg is None else str((msg.get("chat") or {}).get("id"))
            # Only when the row is genuinely absent. `poll_once` registers the whole batch
            # before any worker starts, so on the loop's own path this row already exists and
            # `note_received` is an INSERT … ON CONFLICT DO NOTHING that writes nothing — but
            # still costs a BEGIN IMMEDIATE, an fsync under synchronous=FULL, and the
            # process-wide lock that every other tenant's worker is waiting on. It stays as a
            # fallback because `handle_update` is also called directly, and `note_terminal`
            # below is an UPDATE that would match no row.
            if row is None:
                self.state.note_received(uid, gid, (msg or {}).get("message_id"))
            self.state.note_terminal(uid, bs.IGNORED)
            self._log_unregistered(msg)
            return OUT_IGNORED
        gid, req = summon
        hit = (msg or {}).get("message_id")

        # Same barrier, same reason: `note_received` returns `update_row(uid)` and does not
        # update gid on conflict, so when the batch pre-registered this update the row already
        # in hand is byte-identical to what a second call would return.
        row = row if row is not None else self.state.note_received(uid, gid, hit)
        st = row["state"]
        speaker = ((msg or {}).get("from") or {}).get("first_name") or "Someone"

        if st == bs.RECEIVED:
            return self._run_turn(uid, gid, req, speaker)

        if st == bs.TURN_STARTED:
            # A turn MAY have run. See the module docstring: this is not recoverable on the
            # pinned runtime, and running another one would bill a second turn AND produce a
            # different answer chained on a pointer that may already have moved.
            self.state.note_terminal(uid, bs.RECOVERY_BLOCKED, "turn_outcome_unknown")
            self.log(f"[recovery-blocked] update {uid} for {self.groups[gid].slug}: a model turn "
                     f"may have run before the crash and cannot be recovered (key was recorded, "
                     f"response id was not). No second turn was run. See "
                     f"SECURITY.md")
            self._notify(gid, CLIENT_BLOCKED)
            return OUT_RECOVERY_BLOCKED

        if st in (bs.TURN_COMPLETED, bs.DELIVERY_STARTED):
            # The answer exists and is retrievable. Deliver THAT one.
            return self._deliver_existing(uid, gid, row)

        # DELIVERED is handled above, before routing — it is unreachable from here, because a
        # row that existed was returned already and a row that did not is RECEIVED.
        # Unknown state: fail closed and say so, without guessing what it meant.
        self.state.note_terminal(uid, bs.FAILED_TERMINAL, "unknown_state")
        self.log(f"[state error] update {uid} carries unknown state {st!r} — refusing to "
                 "interpret it; the update is terminal and was not re-run")
        return OUT_FAILED

    # ── the two paths ─────────────────────────────────────────────────────────────────
    def _run_turn(self, uid, gid, req, speaker):
        thread = self.threads[gid]
        client = self.groups[gid]
        # Chosen HERE and recorded BEFORE the request, because it is the only handle that
        # exists before a reply that may never arrive. A key minted inside the HTTP call would
        # die with the process that minted it.
        key = uuid.uuid4().hex
        self.state.note_turn_started(uid, gid, key, thread.prev)
        self.state.note_worker(
            gid, uid, "turn", started_at=self.clock.now_iso(),
            deadline_at=self.clock.now_iso(offset=self.budget),
            heartbeat_at=self.clock.now_iso())
        self.state.note_progress(
            inflight_update_id=uid, inflight_gid=gid, inflight_stage="turn",
            inflight_started_at=self.clock.now_iso(),
            inflight_deadline_at=self.clock.now_iso(offset=self.budget))
        try:
            text = self.turns.run(thread, req, speaker=speaker, idempotency_key=key,
                                  budget=self.budget)
        except Exception as e:
            if getattr(e, "request_sent", False):
                # The model MAY have run. Treating this as an ordinary failure and asking the
                # client to retry would invite a second execution of an unknowable first turn.
                self.state.note_terminal(uid, bs.RECOVERY_BLOCKED,
                                         "turn_outcome_unknown")
                self.log(f"[recovery-blocked] {client.slug}@{gid} update {uid}: a model turn "
                         f"may have run ({type(e).__name__}) and no durable response id is "
                         "available. No second turn was run.")
                self._notify(gid, CLIENT_BLOCKED)
                self._clear_inflight(gid)
                return OUT_RECOVERY_BLOCKED
            # Contained: this update fails, the client is told once, and the loop goes on to
            # the next update. An ordinary turn exception has never stopped the loop and must
            # not start now.
            code = type(e).__name__
            self.state.note_terminal(uid, bs.FAILED_TERMINAL, code)
            self.log(f"[turn error] {client.slug}@{gid} update {uid}: {self.redact(e)}")
            self._notify(gid, CLIENT_FAILURE)
            self._clear_inflight(gid)
            return OUT_FAILED

        # THE CRITICAL TRANSACTION: the response id and the thread pointer it produced become
        # durable together, or neither does.
        self.state.commit_turn(uid, gid, thread)
        return self._deliver(uid, gid, text)

    def _deliver_existing(self, uid, gid, row):
        rid = row["response_id"]
        if not rid:
            self.state.note_terminal(uid, bs.RECOVERY_BLOCKED, "no_response_id")
            self.log(f"[recovery-blocked] update {uid}: recorded complete with no response id")
            self._notify(gid, CLIENT_BLOCKED)
            return OUT_RECOVERY_BLOCKED
        try:
            text = self.turns.fetch(self.groups[gid], rid)
        except Exception as e:
            # Retrieval is FALLIBLE — the retention window is unmeasured (Q5), so this is a
            # real path, not a defensive one. Never fall back to running a new turn.
            self.state.note_terminal(uid, bs.RECOVERY_BLOCKED, "response_unavailable")
            self.log(f"[recovery-blocked] update {uid}: response {rid} could not be retrieved "
                     f"({self.redact(e)}). No new turn was run.")
            self._notify(gid, CLIENT_BLOCKED)
            return OUT_RECOVERY_BLOCKED
        outcome = self._deliver(uid, gid, text)
        return OUT_REDELIVERED if outcome == OUT_ANSWERED else outcome

    def _deliver(self, uid, gid, text):
        self.state.note_state(uid, bs.DELIVERY_STARTED)
        self.state.note_worker(gid, uid, "deliver", heartbeat_at=self.clock.now_iso())
        self.state.note_progress(inflight_stage="deliver")
        try:
            self.tg.send_message(gid, text)
        except Exception as e:
            acked = int(getattr(e, "acknowledged_chunks", 0))
            known_unsent = bool(getattr(e, "known_not_sent", False)) and acked == 0
            if known_unsent:
                self.state.note_terminal(uid, bs.DELIVERY_RETRY,
                                         "delivery_known_not_sent")
                self.log(f"[delivery retry] update {uid}: Telegram rejected the first chunk "
                         f"before accepting it ({self.redact(e)}); stored answer retained for "
                         "explicit redelivery")
                outcome = OUT_DELIVERY_RETRY
            else:
                code = "delivery_partial" if acked else "delivery_uncertain"
                self.state.note_terminal(uid, bs.DELIVERY_RECONCILE, code)
                self.log(f"[delivery reconcile] update {uid}: {acked} chunk(s) acknowledged; "
                         f"complete delivery is not proved ({self.redact(e)}). Stored answer "
                         "retained; no model turn will be run.")
                outcome = OUT_DELIVERY_RECONCILE
            self._clear_inflight(gid)
            return outcome
        # Delivery and the next safe offset, together: a crash between them is precisely how an
        # already-answered update gets replayed.
        self.state.note_delivered(uid)
        self._clear_inflight(gid, last_delivered_at=self.clock.now_iso())
        return OUT_ANSWERED

    def redeliver_reconciled(self, uid):
        """Explicitly redeliver one retained answer; never execute a model turn.

        The operator accepts that already-acknowledged chunks may be duplicated. Failure leaves
        DELIVERY_RECONCILE and its response id intact so the command can be tried again.
        """
        row = self.state.update_row(uid)
        if (row is None or row["state"] not in (bs.DELIVERY_RETRY, bs.DELIVERY_RECONCILE)
                or not row["response_id"]):
            return OUT_FAILED
        gid, rid = str(row["gid"]), row["response_id"]
        try:
            text = self.turns.fetch(self.groups[gid], rid)
            self.tg.send_message(gid, text)
        except Exception as e:
            # error_code is LEFT ALONE. It holds `delivery_partial` or `delivery_uncertain` —
            # the only record of whether Telegram had already acknowledged chunks, i.e. whether
            # the operator's next attempt will duplicate content in a client group. This wrote
            # "redelivery_failed" over it (note_state's COALESCE takes the NEW value when one is
            # given), so the first failed attempt destroyed the evidence the second one needs —
            # on a command explicitly designed to be retried. The failure is on the log line
            # below and in the unchanged state; the delivery fact is not recoverable elsewhere.
            self.log(f"[delivery reconcile] explicit redelivery of update {uid} failed "
                     f"({self.redact(e)}); stored answer remains available, and the recorded "
                     f"delivery evidence ({row['error_code']}) is preserved for the next attempt")
            return OUT_DELIVERY_RECONCILE
        self.state.note_reconciled_delivered(uid)
        self.log(f"[delivery reconcile] update {uid} stored answer explicitly redelivered; "
                 "no model turn ran")
        return OUT_REDELIVERED

    # ── helpers ───────────────────────────────────────────────────────────────────────
    def _advance(self):
        self.state.advance_safe_cursor()

    def _clear_inflight(self, gid, **progress):
        """Retire the in-flight record, optionally carrying one more progress fact with it.

        `**progress` exists so the delivery path does not write meta twice in a row: these are
        pure `meta` writes, so folding them together costs nothing and saves a lock-held,
        fsync'd transaction per answered message. They stay OUT of `note_delivered`'s
        transaction deliberately — see `note_progress`: a progress write must never be able to
        fail a delivery."""
        self.state.clear_worker(gid)
        self.state.note_progress(inflight_update_id=None, inflight_gid=None,
                                 inflight_stage=None, inflight_started_at=None,
                                 inflight_deadline_at=None, **progress)

    def _notify(self, gid, text):
        """Tell the client once. Never allowed to raise: a failed apology must not turn one
        tenant's bad turn into every tenant's outage."""
        try:
            self.tg.send_message(gid, text)
        except Exception as e:
            self.log(f"[notify failed] {gid}: {self.redact(e)}")

    def _log_unregistered(self, msg):
        if msg is None:
            return
        gid = str((msg.get("chat") or {}).get("id"))
        if gid not in self.groups and self.addressed(msg) is not None:
            self.log(f"[ignored] summon from unregistered chat {gid}")

    # Injected by telegram_bridge so the channel vocabulary stays in one place.
    def summoned(self, msg):
        raise NotImplementedError

    def addressed(self, msg):
        raise NotImplementedError

    # ── the loop ──────────────────────────────────────────────────────────────────────
    def poll_once(self, timeout=25):
        """One fetch-and-process cycle. Returns the number of updates handled.

        The offset sent here is the DURABLE cursor, and sending it is what acknowledges
        everything below it to Telegram — which is what turns DELIVERED into ACKED."""
        offset = self.state.cursor
        try:
            r = self.tg.get_updates(offset=offset, timeout=timeout)
        except Exception as e:
            self.log("poll error: " + str(self.redact(e)))
            self.clock.sleep(3)
            return 0
        self.state.note_progress(last_poll_ok_at=self.clock.now_iso(),
                                 heartbeat_at=self.clock.now_iso())
        if offset is not None:
            self.state.mark_cursor_acked()
            self.state.compact()
        updates = r.get("result", []) or []
        self.state.note_progress(last_batch_size=len(updates))
        if not updates or self.stopping:
            return 0

        # Register the entire batch before work begins. Telegram has one global offset: without
        # these durable barriers, a fast later tenant could acknowledge a slow earlier tenant.
        groups = defaultdict(list)
        for upd in updates:
            msg = upd.get("message") or upd.get("edited_message") or {}
            gid = str((msg.get("chat") or {}).get("id")) if msg else None
            self.state.note_received(upd["update_id"], gid, msg.get("message_id"))
            groups[gid if gid is not None else f"__update_{upd['update_id']}"].append(upd)

        def process_group(group_updates):
            outcomes = []
            for upd in group_updates:
                if self.stopping:
                    break
                outcome = self.handle_update(upd)
                self.state.note_progress(last_update_at=self.clock.now_iso(),
                                         heartbeat_at=self.clock.now_iso(),
                                         last_outcome=outcome)
                outcomes.append(outcome)
            return outcomes

        handled = 0
        workers = min(self.max_workers, len(groups))
        with ThreadPoolExecutor(max_workers=workers, thread_name_prefix="bridge-tenant") as pool:
            futures = [pool.submit(process_group, group_updates)
                       for group_updates in groups.values()]
            for future in as_completed(futures):
                try:
                    handled += len(future.result())
                except Exception as e:
                    # The preregistered RECEIVED row remains an acknowledgement barrier. A
                    # worker defect can therefore be retried after restart without data loss.
                    self.log("[worker error] " + str(self.redact(e)))
        if handled < len(updates):
            self.log(f"[shutdown] stopping mid-batch; {len(updates) - handled} update(s) "
                     "left unacknowledged for the next process")
        return handled

    def run(self):
        # PID is recorded so a reader can tell "a bridge is running HERE" from "a bridge was
        # running here recently". The heartbeat cannot answer that: its grace window exists so a
        # busy turn is not a false alarm, and for the length of that window a STOPPED bridge is
        # indistinguishable from a working one. On a supervised host that gap is harmless because
        # something restarts the process; where nothing does, it is the difference between a
        # signal and a reassurance. Cleared on a clean stop so the absence is positive evidence
        # rather than a stale number.
        self.state.note_progress(started_at=self.clock.now_iso(),
                                 heartbeat_at=self.clock.now_iso(),
                                 pid=os.getpid())
        self.state.clear_workers()
        try:
            while not self.stopping:
                self.poll_once()
        finally:
            # COMPARE-AND-CLEAR, not a blind clear. A restart overlaps: the outgoing process may
            # still be unwinding a 25s long poll while its replacement has already recorded its
            # own pid, and a blind clear then wipes the LIVE process's pid. Observed exactly
            # that way -- the store read `pid=None` while a healthy bridge was polling, which
            # silently degrades the liveness check back to the blind behaviour it exists to fix.
            # Only the process that owns the slot may release it.
            if str(self.state.meta_get("pid")) == str(os.getpid()):
                self.state.note_progress(pid=None)
        self.log("[shutdown] clean stop; every completed turn is durable")

# ── health: forward progress, not liveness ────────────────────────────────────────────
# The watchdog it replaces counted `poll error:` lines per systemd invocation, which was a
# genuine improvement over `is-active` — but a bridge wedged inside a turn is not polling, so
# it logs no poll errors at all. `is-active` green, zero errors, deaf to every tenant for as
# long as the turn ran. This decision compares the heartbeat against PROGRESS and against the
# in-flight deadline, so "alive" can never stand in for "working".
POLL_GRACE_SECONDS = 90        # long-poll is 25s; three missed cycles is a real stall
HEARTBEAT_GRACE_SECONDS = 120


def _age(iso, now_epoch):
    if not iso:
        return None
    import datetime
    try:
        return now_epoch - datetime.datetime.fromisoformat(iso).timestamp()
    except (ValueError, TypeError):
        return None


def health(snapshot, now_epoch, budget_seconds, deadline_grace=30, pid_alive=None):
    """(ok, reasons) from a progress snapshot. Pure, so it is testable without a bridge.

    Unhealthy when: the recorded process is gone; the in-flight turn has passed its deadline;
    the loop has not polled within the grace window AND is not legitimately busy inside a turn;
    the heartbeat is stale; or the store itself could not be read. A missing snapshot is
    unhealthy, not unknown — a bridge that cannot write its own progress cannot be asserted to
    be making any.

    `pid_alive` is the caller's answer to "is the recorded process still running on THIS host?"
    — the one question a snapshot cannot answer about itself. Three values, and the middle one
    matters most:
      True  -> the process is there; judge it on progress, exactly as before.
      False -> it is gone. FAIL regardless of how fresh the heartbeat is, because the freshness
               is a fossil: the grace window means a bridge stopped seconds ago still looks
               perfect, and on an unsupervised host nothing is coming to fix it.
      None  -> not determined (no pid recorded, or the reader is not on that host). Behave
               exactly as before this argument existed, so an older store and a remote reader
               are unaffected.
    """
    reasons = []
    if not snapshot:
        return False, ["no progress state — the bridge has never written a heartbeat"]
    if snapshot.get("unreadable"):
        return False, ["the state store could not be read: " + str(snapshot["unreadable"])]
    if pid_alive is False:
        pid = snapshot.get("pid")
        return False, [f"no bridge process is running here (recorded pid {pid} is gone) — the "
                       "heartbeat below is the last one it wrote before stopping, not evidence "
                       "of a live loop"]

    workers = snapshot.get("workers") or []
    inflight = snapshot.get("inflight_update_id")
    if workers:
        inflight = workers[0].get("update_id")
        for worker in workers:
            dl_age = _age(worker.get("deadline_at"), now_epoch)
            if dl_age is not None and dl_age > deadline_grace:
                reasons.append(
                    f"update {worker.get('update_id')} for group {worker.get('gid')} has been "
                    f"in flight past its deadline by {dl_age:.0f}s "
                    f"(stage={worker.get('stage')}) — that tenant worker is wedged")
    else:
        dl_age = _age(snapshot.get("inflight_deadline_at"), now_epoch)
        if inflight and dl_age is not None and dl_age > deadline_grace:
            reasons.append(
                f"update {inflight} has been in flight past its deadline by {dl_age:.0f}s "
                f"(stage={snapshot.get('inflight_stage')}) — the loop is wedged, not merely busy")

    poll_age = _age(snapshot.get("last_poll_ok_at"), now_epoch)
    start_age = _age(snapshot.get("started_at"), now_epoch)
    if poll_age is None:
        # STARTUP GRACE. `last_poll_ok_at` is written when getUpdates RETURNS, and the first
        # long poll takes up to 25s (60s if the socket stalls) — so a correctly running bridge
        # has no poll recorded for its first minute. Without this the watchdog fires a false
        # alarm on every restart, which is worse than no alarm: it trains the operator to
        # ignore the one signal that says the bridge has stopped receiving. Found on the
        # production rollout, where a healthy bridge reported FAIL for ~50 seconds.
        if start_age is not None and start_age <= POLL_GRACE_SECONDS:
            pass                      # starting up; nothing is wrong yet
        else:
            reasons.append(
                "no successful Telegram poll has ever been recorded"
                + (f" and the process started {start_age:.0f}s ago" if start_age is not None
                   else " and no start time was recorded"))
    elif poll_age > POLL_GRACE_SECONDS:
        # Being inside a turn EXPLAINS a gap in polling, but only up to the budget. Past that
        # the explanation is the symptom.
        allowed = POLL_GRACE_SECONDS + (budget_seconds if inflight else 0)
        if poll_age > allowed:
            reasons.append(f"no successful Telegram poll for {poll_age:.0f}s "
                           f"(allowed {allowed:.0f}s) — the bridge is not receiving")

    hb_age = _age(snapshot.get("heartbeat_at"), now_epoch)
    if hb_age is None:
        reasons.append("no heartbeat recorded")
    elif hb_age > HEARTBEAT_GRACE_SECONDS + (budget_seconds if inflight else 0):
        reasons.append(f"heartbeat is {hb_age:.0f}s old — the process is not running its loop")

    blocked = (snapshot.get("counts") or {}).get(bs.RECOVERY_BLOCKED, 0)
    if blocked:
        reasons.append(f"{blocked} update(s) are RECOVERY_BLOCKED and need operator "
                       "reconciliation (SECURITY.md)")
    delivery = (snapshot.get("counts") or {}).get(bs.DELIVERY_RECONCILE, 0)
    if delivery:
        reasons.append(f"{delivery} update(s) have uncertain/partial Telegram delivery and "
                       "need explicit stored-answer redelivery reconciliation")
    retry = (snapshot.get("counts") or {}).get(bs.DELIVERY_RETRY, 0)
    if retry:
        reasons.append(f"{retry} update(s) have a retained Telegram answer that was not sent "
                       "and need explicit redelivery")
    return (not reasons), reasons
