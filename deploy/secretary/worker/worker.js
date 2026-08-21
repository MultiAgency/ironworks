// The MultiAgency secretary, as a Cloudflare Worker (Telegram webhook).
// Serverless + always-on. Per-visitor thread state lives in KV (a Worker is stateless per request).
// Calls the hosted IronClaw at env.IRONCLAW_API with a scoped token. Visitor-facing, tool-less,
// human-gated. THE deployment (the former Python poller is retired — one impl, no drift).
//
// Bindings (wrangler.jsonc): KV "THREADS"; vars IRONCLAW_API, TEAM_CHAT_ID, MODEL (optional
// override; unset by default so the bundled repo-root MODEL_PIN governs).
// Secrets (wrangler secret put): SECRETARY_BOT_TOKEN, IRONCLAW_TOKEN, WEBHOOK_SECRET.

// The canonical persona file — bundled as a text module
// (wrangler.jsonc rules); PERSONA.md one level up is the single source of truth.
import PERSONA_RAW from "../PERSONA.md";
import MODEL_PIN_RAW from "../../../MODEL_PIN";
import BRIEF from "../brief-fields.json";
const PERSONA = PERSONA_RAW.trim();

// The model of record, bundled from the repo-root MODEL_PIN by the same text-module rule.
// A Worker has no filesystem at runtime, so "falls back to MODEL_PIN" can only mean "at BUILD
// time" — this is that binding. It replaces a hardcoded "Qwen/..." literal which, because
// vars.MODEL is deliberately unset, WAS the live model selection for the visitor-facing front
// desk while wrangler.jsonc claimed the pin governed it.
//
// FAIL CLOSED, mirroring multi/verify/common.py's model_pin(): no fallback literal, throw at
// module load. MODEL_PIN is tracked, so an unparseable pin means a broken build, and a default
// here would be the one value that can SILENTLY outrank the pin — the pin's first stated reason
// is TEE-hosted privacy, so a stale literal quietly moves visitor conversations onto a model
// with weaker guarantees and nothing in the reply would say so. Refusing to boot is the
// cheaper failure.
const MODEL_PIN = MODEL_PIN_RAW.split("#", 1)[0].trim();
if (!MODEL_PIN) {
  throw new Error("MODEL_PIN names no model on its first line — refusing to serve on an unpinned model");
}

// Hosted IronClaw may sit behind Cloudflare bot-protection that 1010-blocks default agents.
const UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 " +
          "(KHTML, like Gecko) Chrome/125.0 Safari/537.36";

const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function mdToHtml(text) {
  let t = esc(text);
  t = t.replace(/`([^`]+)`/g, "<code>$1</code>");
  t = t.replace(/\*\*([^*\n]+)\*\*/g, "<b>$1</b>");
  t = t.replace(/__([^_\n]+)__/g, "<b>$1</b>");
  t = t.replace(/(^|[^*\w])\*([^*\n]+)\*(?![*\w])/g, "$1<i>$2</i>");
  t = t.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2">$1</a>');
  t = t.replace(/^\s{0,3}#{1,6}\s+(.*)$/gm, "<b>$1</b>");
  t = t.replace(/^\s*[-*]\s+/gm, "• ");
  return t;
}
function stripMd(text) {
  return text
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\*\*([^*\n]+)\*\*/g, "$1")
    .replace(/__([^_\n]+)__/g, "$1")
    .replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*[-*]\s+/gm, "• ")
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, "$1 ($2)");
}

async function tg(env, method, params) {
  const r = await fetch(`https://api.telegram.org/bot${env.SECRETARY_BOT_TOKEN}/${method}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(params),
  });
  return r.json();
}

// Send with Telegram HTML; fall back to plain text if the entities won't parse.
async function sendMsg(env, chatId, text) {
  const html = mdToHtml(text);
  if (html.length <= 3800) {
    const res = await tg(env, "sendMessage", { chat_id: chatId, text: html, parse_mode: "HTML" });
    if (res.ok) return res.result;
  }
  const plain = stripMd(text).slice(0, 3800) || "…";
  return (await tg(env, "sendMessage", { chat_id: chatId, text: plain })).result;
}

async function ironclaw(env, input, prev) {
  // Persona via top-level `instructions` EVERY turn — a once-only input-prepend drifts
  // (proven: multi/verify/test_injection.py vs test_injection2.py).
  // MODEL_PIN (repo root): TEE-hosted; proxied models get no TEE privacy and no prompt caching.
  // env.MODEL still wins, as the documented one-off override.
  const body = { model: env.MODEL || MODEL_PIN, instructions: PERSONA, input };
  if (prev) body.previous_response_id = prev;
  const r = await fetch(`${env.IRONCLAW_API}/v1/responses`, {
    method: "POST",
    headers: {
      "Authorization": "Bearer " + env.IRONCLAW_TOKEN,
      "Content-Type": "application/json",
      "User-Agent": UA,
      "Idempotency-Key": crypto.randomUUID().replace(/-/g, ""),
    },
    body: JSON.stringify(body),
  });
  return r.json();
}
function textOf(d) {
  let out = "";
  for (const it of d.output || [])
    if (it.type === "message")
      for (const c of it.content || [])
        if (c.type === "output_text" || c.type === "text") out += c.text;
  return out.trim();
}

// The internal/visitor boundary is STRUCTURAL, not prompt-enforced: the persona's wrap-up
// marker only *signals*; the brief itself comes from this dedicated, schema-validated turn
// whose output goes ONLY to the team chat. Nothing the
// visitor (or the model's visitor-facing text) writes can land in the team chat verbatim.
// The brief schema, from the one file both this Worker and test_aide_discovery.py read.
// It used to be a literal here AND a literal there, and they diverged in both directions while
// each side validated against its own copy — so both stayed green and no gate saw the gap.
const BRIEF_FIELDS = BRIEF.fields;
// Fail closed at module load, same rule as MODEL_PIN above: a brief schema that arrived empty or
// without its {FIELDS} placeholder would send the model an unusable prompt every wrap-up, and the
// only symptom would be briefs that never validate — silently, on the visitor-facing path.
if (!Array.isArray(BRIEF_FIELDS) || BRIEF_FIELDS.length === 0 ||
    typeof BRIEF.ask_template !== "string" || !BRIEF.ask_template.includes("{FIELDS}")) {
  throw new Error("brief-fields.json is malformed (need a non-empty fields[] and an ask_template containing {FIELDS})");
}

async function generateBrief(env, prev) {
  const ask = BRIEF.ask_template.replace("{FIELDS}", BRIEF_FIELDS.join(", "));
  let p = prev;
  for (let attempt = 0; attempt < 2; attempt++) {
    const d = await ironclaw(env, ask, p);
    p = d.id || p;
    try {
      let t = textOf(d).replace(/^```[a-zA-Z]*\n?/, "").replace(/\n?```$/, "").trim();
      const j = JSON.parse(t.slice(t.indexOf("{"), t.lastIndexOf("}") + 1));
      if (BRIEF_FIELDS.every((k) => typeof j[k] === "string" && j[k].trim()) &&
          Object.keys(j).length === BRIEF_FIELDS.length) {
        return { brief: BRIEF_FIELDS.map((k) => `${k}: ${j[k].trim()}`).join("\n"), prev: p };
      }
    } catch (e) { /* retry once, then give up cleanly */ }
  }
  return { brief: "", prev: p };
}

async function handleUpdate(env, update) {
  const msg = update.message || update.edited_message;
  if (!msg) return;
  const chat = msg.chat || {};
  if (chat.type !== "private") return;           // the front desk is 1:1 DMs only
  let text = msg.text || "";
  if (!text) return;
  const uid = String((msg.from || {}).id);
  const cid = chat.id;
  const who = (((msg.from || {}).first_name || "") +
               ((msg.from || {}).username ? " @" + msg.from.username : "")).trim() || uid;

  let payload = null;
  if (text.startsWith("/start")) {
    const parts = text.split(/\s+/);
    payload = parts[1] || null;                   // e.g. "web" from the site deep-link
    text = "";
  }

  const key = "t:" + uid;
  const prev = await env.THREADS.get(key);
  const first = !prev;

  const input = first
    ? `A new person just messaged you${payload ? ` (they came from: ${payload})` : ""}. ` +
      `Greet them warmly and start.\n\nTHEM: ` + (text || "(they just opened the chat)")
    : "THEM: " + text;

  // Start the model call FIRST; the Telegram niceties (team ping, typing, placeholder) run
  // while it thinks instead of serially delaying it.
  const ironclawPromise = ironclaw(env, input, prev);
  if (first && env.TEAM_CHAT_ID) {
    await tg(env, "sendMessage", {
      chat_id: env.TEAM_CHAT_ID,
      text: `🟡 Front desk: new visitor ${who}` + (payload ? ` (via ${payload})` : ""),
    });
  }
  await tg(env, "sendChatAction", { chat_id: cid, action: "typing" });
  const ph = (await tg(env, "sendMessage", { chat_id: cid, text: "💭 Thinking..." })).result;

  const d = await ironclawPromise;
  let newId = d.id || prev;

  let reply = textOf(d) || "…";
  let wrapUp = false;
  const m = reply.match(/^[ \t]*HANDOFF:[ \t]*/m);
  if (m) {
    // m.index, NOT indexOf: a mid-sentence 'HANDOFF:' must not truncate the visitor reply.
    // Everything from the marker on is DISCARDED from the visitor stream and never sent to
    // the team either — the brief comes from the dedicated validated turn below.
    reply = reply.slice(0, m.index).trim() || "…";
    wrapUp = true;
  }

  // replace the placeholder with the reply; fall back if the edit can't hold it
  const html = mdToHtml(reply);
  let done = false;
  if (ph && html.length <= 3800) {
    let res = await tg(env, "editMessageText",
      { chat_id: cid, message_id: ph.message_id, text: html, parse_mode: "HTML" });
    if (!res.ok)
      res = await tg(env, "editMessageText",
        { chat_id: cid, message_id: ph.message_id, text: stripMd(reply).slice(0, 3800) || "…" });
    done = res.ok;
  }
  if (!done) {
    if (ph) await tg(env, "deleteMessage", { chat_id: cid, message_id: ph.message_id });
    await sendMsg(env, cid, reply);
  }

  if (wrapUp) {
    let g = { brief: "", prev: newId };
    try { g = await generateBrief(env, newId); } catch (e) { console.log("brief error:", String(e)); }
    newId = g.prev;
    if (env.TEAM_CHAT_ID) {
      // provenance label: model-generated from a visitor-influenced conversation — derived,
      // never verified fact. Only schema-validated fields reach this chat.
      await sendMsg(env, env.TEAM_CHAT_ID, g.brief
        ? `📋 Lead brief — ${who} (agent-generated from the visitor chat; verify before acting)\n\n${g.brief}`
        : `📋 Lead wrapped up — ${who} (brief generation failed twice; read the thread directly)`);
    }
  }
  if (newId) await env.THREADS.put(key, newId, { expirationTtl: 60 * 60 * 24 * 30 }); // 30d
}

export default {
  async fetch(request, env, ctx) {
    if (request.method === "GET") return new Response("Aide (MultiAgency secretary) — ok");
    if (request.method !== "POST") return new Response("method not allowed", { status: 405 });
    // Only Telegram (with the shared secret) may post updates. FAIL CLOSED: an unset
    // WEBHOOK_SECRET (e.g. secrets not re-put after a worker rename) must refuse everything,
    // not degrade to an unauthenticated public endpoint.
    if (!env.WEBHOOK_SECRET ||
        request.headers.get("X-Telegram-Bot-Api-Secret-Token") !== env.WEBHOOK_SECRET) {
      return new Response("forbidden", { status: 403 });
    }
    let update;
    try { update = await request.json(); } catch { return new Response("bad request", { status: 400 }); }
    // Answer Telegram immediately; do the model call + reply in the background.
    ctx.waitUntil(handleUpdate(env, update).catch((e) => console.log("handle error:", String(e))));
    return new Response("ok");
  },
};
