# Multi — the MultiAgency secretary (Cloudflare Worker)

The front-desk Telegram agent ("Multi"), as a Cloudflare Worker driven by a Telegram
**webhook** (no long-poll, no always-on process). Per-visitor thread state lives in KV. Calls the
hosted IronClaw at `IRONCLAW_API`. THE secretary deployment (the Python poller was retired
one implementation, no drift). Persona: `../PERSONA.md`, bundled as a text module.

## Deploy

```
cd worker

# 1. Secrets (interactive — values never printed):
wrangler secret put SECRETARY_BOT_TOKEN     # Aide (@<your_bot>) token, from BotFather
wrangler secret put IRONCLAW_TOKEN          # token for the secretary's instance host (scope it if you can)
wrangler secret put WEBHOOK_SECRET          # any random string; reuse the SAME value in setWebhook below

# 2. Deploy — with the REAL config (untracked; the repo's wrangler.jsonc is a public template
#    with placeholders — copy it to wrangler.local.jsonc and fill in your KV id / vars):
wrangler deploy --config wrangler.local.jsonc   # prints the Worker URL (…workers.dev)

# 3. Point Telegram at it (this takes over from any long-poll bot on the same token):
curl "https://api.telegram.org/bot<BOT_TOKEN>/setWebhook" \
  --data-urlencode "url=https://<worker-url>/" \
  --data-urlencode "secret_token=<same WEBHOOK_SECRET>"
```

## Config (wrangler.jsonc)
- KV `THREADS` — `uid → previous_response_id` (30-day TTL).
- vars: `IRONCLAW_API`, `TEAM_CHAT_ID`, `MODEL`.
- secrets: `SECRETARY_BOT_TOKEN`, `IRONCLAW_TOKEN`, `WEBHOOK_SECRET`.

## Notes
- The Worker answers Telegram immediately and does the model call + reply in `ctx.waitUntil(...)`,
  so a slow IronClaw turn never trips Telegram's webhook timeout.
- Setting the webhook disables `getUpdates`, so the local Python bot goes quiet automatically —
  stop it too if one is somehow running.
- There is no poller fallback any more: if the Worker misbehaves, fix and `wrangler deploy` (or `wrangler rollback`).
- Renaming the Worker does **not** carry its secrets across: re-run all three
  `wrangler secret put` commands after a rename, or the fetch handler fail-closes on the
  missing `WEBHOOK_SECRET` and every update returns 403.
