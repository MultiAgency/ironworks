---
status: accepted
---

# Execute turns by create-then-poll, not SSE streaming

IronClaw's own documentation says "Turn timeout (non-streaming): 30 seconds — use `stream: true`
for long-running work", which reads as a instruction to stream. Reading the pinned source shows
the 30 seconds is `DEFAULT_RESPONSES_WAIT_TIMEOUT`, a *wait* on the create call rather than a cap
on the turn. We create and poll, because a durable `response_id` makes an answer fetchable
forever and the entire recovery model rests on holding one.

> **Correction, 2026-08-31 — the decision stands, one premise did not.** This section originally
> continued: *"`POST /v1/responses` blocks up to 30s, then returns whatever status it has, and the
> client polls `GET /v1/responses/{id}` while status is `queued` or `in_progress`… that hands us a
> durable `response_id` within 30 seconds."* **The pinned source contradicts that.** `create`
> waits for the turn to *complete* under
> `tokio::time::timeout(wait_timeout, wait_for_response_completion(…))` and maps elapse to
> **`503 service_unavailable`**, returning **no id at all**
> (`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:327-347`; byte-identical at
> 1.3.0, so this was a misreading rather than drift). Measured at the pin: 503 at 30.05s.
>
> So for a turn that outruns 30s there is nothing to poll, and the sentence "hands us a
> `response_id` within 30 seconds" is false in exactly the case it was written to cover — which
> for this product is the flagship question, *"which three accounts deserve attention this week"*,
> over a 40-record book.
>
> **Why the decision is nevertheless unchanged.** The recovery model does not actually depend on
> receiving the id *during* create. It depends on the request being byte-reproducible, so the same
> idempotency key can be replayed and the runtime's idempotency index returns the original
> response — ADR 0005, measured at 0.085s against a turn whose create had 503'd. The 503 is
> classified `Unknown` (the turn may have run), never `NotSent`, so no second execution is
> licensed. Create-then-poll versus SSE is untouched by this: polling still follows a turn that
> answers inside the window, and a dropped stream would reopen the ambiguity this design closes.
>
> **What it costs, stated plainly:** a long turn is answered on the *next* poll cycle rather than
> the current one, and every such turn exercises the ambiguous path routinely rather than
> exceptionally. Upstream's own answer to this is `stream: true`, which this ADR declines for the
> reasons below; that trade is now an informed one rather than an unnoticed one.

## Considered options

**SSE streaming** would give lower latency to first token and one connection instead of a poll
loop. It was rejected because a dropped stream reopens exactly the ambiguity the design exists to
close — "did a turn run?" — whereas a persisted response id makes the answer fetchable forever
and means a crash mid-turn never costs a second model execution. Streaming also cannot be
combined cheaply with at-most-once semantics: we would have to persist the id from the first
frame and keep reading, which is the polling complexity plus a long-lived socket.

## Consequences

Turn latency is bounded below by the poll interval, so a fast turn can look slower than it is.
That is an acceptable price on a channel where answers arrive as chat messages.

The poll deadline and the overall turn budget are two different clocks and must not be conflated:
in the Python, which of two error messages an operator saw was a race between a 150s poll deadline
and a 180s turn budget. One wall-clock budget covers connect, create, poll and backoff; the poll
loop is bounded by it rather than by a second independent number.

## Pointer, 2026-09-02: a cursored projection is not the stream this declined

`docs/experiments/0037-what-the-seam-never-asked-for.md` check 6 measured something this ADR's
"Considered options" does not cover, so the decision is unchanged and this is a pointer rather than
a correction.

Every response id is also a thread id, and `GET /api/webchat/v2/threads/{response_id}/events` is a
Server-Sent Events stream **whose frame carries its own resume cursor**, replayable with
`Last-Event-ID` or `?after_cursor=`. The rejection above rests on "a dropped stream reopens exactly
the ambiguity the design exists to close". That is true of `stream: true` on the create call, whose
frames are the only delivery of the result. It is not true of a projection read on a separate route
after create has returned, where a drop is resumed from stored state and the response object remains
the record either way.

Nothing here licenses adopting it. Create-then-poll stands, ADR 0005 still carries recovery, and the
projection is not a delivery path — it is an observation one. It is written down because the reason
recorded for declining SSE would, read quickly, appear to decline this too, and it does not reach it.
