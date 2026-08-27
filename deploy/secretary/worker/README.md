# Multi — the MultiAgency secretary (Cloudflare Worker)

**Not part of the IronWorks product path.** The Secretary is MultiAgency's own public front desk:
a separate application built on IronClaw, with its own instance, volume, and trust domain. It
shares this repository's pin, image, and secret conventions and nothing else — no tenant, no
organization scope, no guidance binding, no service definition, no Account Service. The boundaries
in [`../../../SECURITY.md`](../../../SECURITY.md) describe the product path and do not describe
this. It runs the pinned runtime, so it is in scope for
[`../../UPGRADE.md`](../../UPGRADE.md).

The front-desk Telegram agent ("Multi"), as a Cloudflare Worker driven by a Telegram
**webhook** (no long-poll, no always-on process). Per-visitor ordered work, deduplication, rate
state, and thread continuity live in one SQLite-backed Durable Object per visitor. The former KV
binding is retained only as a read-once migration source. Calls the
hosted IronClaw at `IRONCLAW_API`. This is THE secretary deployment — the Python poller was
retired, so there is one implementation and no drift. Persona: `../PERSONA.md`, bundled as a
text module.

## Layout

Three files, and the split is what makes the logic testable off Cloudflare:

- `worker.js` — the Workers entry point and nothing else: bundle-time imports (persona,
  `MODEL_PIN`, brief fields), the webhook `fetch` handler with its secret-token check, and the
  `VisitorSession` Durable Object export. It holds no secretary behavior of its own.
- `secretary-core.js` — every decision the secretary makes: `createSecretary` (model call,
  prompt assembly, timeouts, error classes), `createVisitorSessionBase` (queue, dedup, rate
  state, thread continuity, alarms), and `visitorId`. Plain ESM with no Workers globals at module
  scope, which is what lets it be unit-tested under `node --test`.
- `secretary-core.test.js` — those unit tests. `./deploy/ironworks test` runs them, so a change
  to secretary behavior is gated like the rest of the repo rather than only at deploy time.

## Deploy

```
cd worker

# 1. Secrets (interactive — values never printed):
wrangler secret put SECRETARY_BOT_TOKEN     # Aide (@<your_bot>) token, from BotFather
wrangler secret put IRONCLAW_TOKEN          # token for the secretary's instance host (scope it if you can)
wrangler secret put WEBHOOK_SECRET          # any random string; reuse the SAME value in setWebhook below

# 2. Deploy — with the REAL config (untracked; the repo's wrangler.jsonc is a public template
#    with placeholders — copy it to wrangler.local.jsonc and fill in your KV id / vars.
#    Keep the VISITOR_SESSIONS binding and exports declaration unchanged).
#    wrangler bundles from worker.js, so secretary-core.js must be present beside it —
#    a deploy from a tree missing it fails at bundle time, not at runtime:
wrangler deploy --config wrangler.local.jsonc   # prints the Worker URL (…workers.dev)

# 3. Point Telegram at it (this takes over from any long-poll bot on the same token):
curl "https://api.telegram.org/bot<BOT_TOKEN>/setWebhook" \
  --data-urlencode "url=https://<worker-url>/" \
  --data-urlencode "secret_token=<same WEBHOOK_SECRET>"
```

## Config (wrangler.jsonc)
- Durable Object `VISITOR_SESSIONS` — one ordered queue and response pointer per visitor.
- KV `THREADS` — legacy `uid → previous_response_id`; delete the binding only after every
  active visitor has migrated or the old 30-day retention window has elapsed.
- vars: `IRONCLAW_API`, `TEAM_CHAT_ID`, `MODEL`, `SECRETARY_RATE_LIMIT` (default 6/minute/visitor).
- secrets: `SECRETARY_BOT_TOKEN`, `IRONCLAW_TOKEN`, `WEBHOOK_SECRET`.

## Notes
- The Worker answers Telegram immediately and does the model call + reply in `ctx.waitUntil(...)`,
  so a slow IronClaw turn never trips Telegram's webhook timeout.
- Telegram update ids are deduplicated before enqueue. Durable Object alarms perform model calls
  serially for one visitor; different visitors use different objects and remain concurrent.
- Upstream calls have bounded timeouts and validate both HTTP and JSON application status. Logs
  carry stable error classes, never response bodies or credentials.
- Setting the webhook disables `getUpdates`, so the local Python bot goes quiet automatically —
  stop it too if one is somehow running.
- There is no poller fallback any more: if the Worker misbehaves, fix and `wrangler deploy` (or `wrangler rollback`).
- Renaming the Worker does **not** carry its secrets across: re-run all three
  `wrangler secret put` commands after a rename, or the fetch handler fail-closes on the
  missing `WEBHOOK_SECRET` and every update returns 403.
