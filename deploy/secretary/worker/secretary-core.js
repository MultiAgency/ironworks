export class UpstreamError extends Error {
  constructor(service, code) {
    super(`${service} request failed (${code})`);
    this.name = "UpstreamError";
    this.service = service;
    this.code = String(code);
  }
}

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;")
  .replace(/>/g, "&gt;").replace(/"/g, "&quot;");

export function mdToHtml(text) {
  let t = esc(text);
  t = t.replace(/`([^`]+)`/g, "<code>$1</code>");
  t = t.replace(/\*\*([^*\n]+)\*\*/g, "<b>$1</b>");
  t = t.replace(/__([^_\n]+)__/g, "<b>$1</b>");
  t = t.replace(/(^|[^*\w])\*([^*\n]+)\*(?![*\w])/g, "$1<i>$2</i>");
  t = t.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2">$1</a>');
  t = t.replace(/^\s{0,3}#{1,6}\s+(.*)$/gm, "<b>$1</b>");
  return t.replace(/^\s*[-*]\s+/gm, "• ");
}

export function stripMd(text) {
  return String(text).replace(/`([^`]+)`/g, "$1").replace(/\*\*([^*\n]+)\*\*/g, "$1")
    .replace(/__([^_\n]+)__/g, "$1").replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*[-*]\s+/gm, "• ")
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, "$1 ($2)");
}

export function textOf(doc) {
  let out = "";
  for (const item of doc?.output || []) {
    if (item?.type !== "message") continue;
    for (const content of item.content || []) {
      if ((content?.type === "output_text" || content?.type === "text") &&
          typeof content.text === "string") out += content.text;
    }
  }
  return out.trim();
}

export function visitorId(update) {
  const msg = update?.message || update?.edited_message;
  if (!msg || msg.chat?.type !== "private" || !msg.text || msg.from?.id == null) return null;
  return String(msg.from.id);
}

export function createTelegramWebhook() {
  return {
    async fetch(request, env) {
      if (request.method === "GET") return new Response("Aide (MultiAgency secretary) — ok");
      if (request.method !== "POST") return new Response("method not allowed", {status: 405});
      if (!env.WEBHOOK_SECRET ||
          request.headers.get("X-Telegram-Bot-Api-Secret-Token") !== env.WEBHOOK_SECRET) {
        return new Response("forbidden", {status: 403});
      }
      let update;
      try { update = await request.json(); }
      catch { return new Response("bad request", {status: 400}); }
      const uid = visitorId(update);
      if (!uid) return new Response("ok");

      const stub = env.VISITOR_SESSIONS.get(env.VISITOR_SESSIONS.idFromName(uid));
      let accepted;
      try {
        accepted = await stub.fetch("https://visitor.internal/update", {
          method: "POST", headers: {"Content-Type": "application/json"},
          body: JSON.stringify(update),
        });
      } catch (error) {
        console.log("visitor session unavailable", error?.name || "error");
        return new Response("durable acceptance unavailable", {status: 503});
      }
      if (!accepted.ok) {
        console.log("visitor session rejected", accepted.status);
        return new Response("durable acceptance unavailable", {status: 503});
      }
      // This is the acceptance boundary: the VisitorSession returns success only after its
      // enqueue transaction has committed and its alarm has been established (or an existing
      // durable queue entry has been found). Model execution and delivery happen later there.
      return new Response("ok");
    },
  };
}

async function jsonRequest(fetchImpl, url, init, service, timeoutMs) {
  let response;
  try {
    response = await fetchImpl(url, {...init, signal: AbortSignal.timeout(timeoutMs)});
  } catch (error) {
    throw new UpstreamError(service, error?.name === "TimeoutError" ? "timeout" : "network");
  }
  let body;
  try { body = await response.json(); } catch { throw new UpstreamError(service, "invalid-json"); }
  if (!response.ok || !body || typeof body !== "object") {
    throw new UpstreamError(service, `http-${response.status}`);
  }
  return body;
}

export function createSecretary({persona, modelPin, brief, fetchImpl = fetch}) {
  const fields = brief?.fields;
  if (!Array.isArray(fields) || fields.length === 0 ||
      typeof brief.ask_template !== "string" || !brief.ask_template.includes("{FIELDS}")) {
    throw new Error("brief schema is malformed");
  }

  async function tg(env, method, params) {
    const body = await jsonRequest(fetchImpl,
      `https://api.telegram.org/bot${env.SECRETARY_BOT_TOKEN}/${method}`,
      {method: "POST", headers: {"Content-Type": "application/json"},
       body: JSON.stringify(params)}, "telegram", 15000);
    if (body.ok !== true) throw new UpstreamError("telegram", "rejected");
    return body;
  }

  async function sendMsg(env, chatId, text) {
    const html = mdToHtml(text);
    if (html.length <= 3800) {
      try {
        return (await tg(env, "sendMessage",
          {chat_id: chatId, text: html, parse_mode: "HTML"})).result;
      } catch (error) {
        // "http-400" BELONGS HERE, and was the case this fallback exists for. Telegram reports
        // `Bad Request: can't parse entities` as HTTP 400, which `jsonRequest` turns into
        // UpstreamError("telegram", "http-400") — never the "rejected" code, which only happens
        // on a 200 carrying ok:false. So the one failure this retry was written to absorb was
        // the one it rethrew, and the visitor got no reply at all. `mdToHtml("# **Hi**")`
        // returns `<b><b>Hi</b></b>`, which Telegram 400s on, so it is reachable from ordinary
        // model output. Any 4xx from a parse_mode send is a reason to try it as plain text;
        // a 5xx, a timeout or a network error is not, and still propagates.
        const retryable = error instanceof UpstreamError &&
          (error.code === "rejected" || /^http-4\d\d$/.test(error.code));
        if (!retryable) throw error;
      }
    }
    return (await tg(env, "sendMessage",
      {chat_id: chatId, text: stripMd(text).slice(0, 3800) || "…"})).result;
  }

  async function ironclaw(env, input, prev, idempotencyKey) {
    const body = {model: env.MODEL || modelPin, instructions: persona, input};
    if (prev) body.previous_response_id = prev;
    const doc = await jsonRequest(fetchImpl, `${env.IRONCLAW_API}/v1/responses`, {
      method: "POST",
      headers: {"Authorization": `Bearer ${env.IRONCLAW_TOKEN}`,
        "Content-Type": "application/json", "User-Agent": "ironworks-secretary/1",
        "Idempotency-Key": idempotencyKey},
      body: JSON.stringify(body),
    }, "ironclaw", 120000);
    if (typeof doc.id !== "string" || !doc.id) throw new UpstreamError("ironclaw", "missing-id");
    if (doc.status && !["completed", "incomplete"].includes(doc.status)) {
      throw new UpstreamError("ironclaw", `status-${doc.status}`);
    }
    return doc;
  }

  async function generateBrief(env, prev, key) {
    const ask = brief.ask_template.replace("{FIELDS}", fields.join(", "));
    let pointer = prev;
    for (let attempt = 0; attempt < 2; attempt++) {
      const doc = await ironclaw(env, ask, pointer, `${key}-brief-${attempt}`);
      pointer = doc.id;
      try {
        const text = textOf(doc).replace(/^```[a-zA-Z]*\n?/, "").replace(/\n?```$/, "").trim();
        const parsed = JSON.parse(text.slice(text.indexOf("{"), text.lastIndexOf("}") + 1));
        if (fields.every((field) => typeof parsed[field] === "string" && parsed[field].trim()) &&
            Object.keys(parsed).length === fields.length) {
          return {brief: fields.map((field) => `${field}: ${parsed[field].trim()}`).join("\n"),
                  prev: pointer};
        }
      } catch { /* retry once */ }
    }
    return {brief: "", prev: pointer};
  }

  async function executeUpdate(env, update, previousResponseId) {
    const msg = update.message || update.edited_message;
    let text = msg.text;
    const uid = String(msg.from.id);
    const cid = msg.chat.id;
    const who = (`${msg.from?.first_name || ""}` +
      (msg.from?.username ? ` @${msg.from.username}` : "")).trim() || uid;
    let payload = null;
    if (text.startsWith("/start")) {
      payload = text.split(/\s+/)[1] || null;
      text = "";
    }
    const first = !previousResponseId;
    const input = first
      ? `A new person just messaged you${payload ? ` (they came from: ${payload})` : ""}. ` +
        `Greet them warmly and start.\n\nTHEM: ${text || "(they just opened the chat)"}`
      : `THEM: ${text}`;
    const key = `secretary-update-${update.update_id}`;
    const doc = await ironclaw(env, input, previousResponseId, key);
    let next = doc.id;
    let reply = textOf(doc) || "…";
    const marker = reply.match(/^[ \t]*HANDOFF:[ \t]*/m);
    const wrapUp = Boolean(marker);
    if (marker) reply = reply.slice(0, marker.index).trim() || "…";
    let teamBrief = null;
    if (wrapUp) {
      let generated = {brief: "", prev: next};
      try { generated = await generateBrief(env, next, key); }
      catch (error) { console.log("secretary brief failed", error?.code || error?.name || "error"); }
      next = generated.prev;
      if (env.TEAM_CHAT_ID) teamBrief = generated.brief
        ? `📋 Lead brief — ${who} (agent-generated from the visitor chat; verify before acting)\n\n${generated.brief}`
        : `📋 Lead wrapped up — ${who} (brief generation failed twice; read the thread directly)`;
    }
    return {next, reply, cid, who, payload, first, teamBrief,
      delivered: {teamNotice: false, visitor: false, teamBrief: false}};
  }

  async function deliverUpdate(env, _update, completed, checkpoint = async () => {}) {
    if (completed.first && env.TEAM_CHAT_ID && !completed.delivered.teamNotice) {
      await tg(env, "sendMessage", {chat_id: env.TEAM_CHAT_ID,
        text: `🟡 Front desk: new visitor ${completed.who}` +
          (completed.payload ? ` (via ${completed.payload})` : "")});
      completed.delivered.teamNotice = true;
      await checkpoint(completed);
    }
    if (!completed.delivered.visitor) {
      try { await tg(env, "sendChatAction", {chat_id: completed.cid, action: "typing"}); }
      catch (error) { console.log("secretary typing failed", error?.code || error?.name || "error"); }
      await sendMsg(env, completed.cid, completed.reply);
      completed.delivered.visitor = true;
      await checkpoint(completed);
    }
    if (completed.teamBrief && env.TEAM_CHAT_ID && !completed.delivered.teamBrief) {
      await sendMsg(env, env.TEAM_CHAT_ID, completed.teamBrief);
      completed.delivered.teamBrief = true;
      await checkpoint(completed);
    }
  }

  async function handleUpdate(env, update, previousResponseId) {
    const completed = await executeUpdate(env, update, previousResponseId);
    await deliverUpdate(env, update, completed);
    return completed.next;
  }

  async function notifyFailure(env, update) {
    const msg = update?.message || update?.edited_message;
    if (!msg?.chat?.id) return;
    try { await sendMsg(env, msg.chat.id, "Sorry — I hit a technical problem. Please try again shortly."); }
    catch { /* the channel itself is unavailable */ }
  }

  return {executeUpdate, deliverUpdate, handleUpdate, notifyFailure};
}

function rateLimit(value) {
  const parsed = Number(value ?? 6);
  return Number.isSafeInteger(parsed) && parsed >= 1 ? parsed : 1;
}

const MAX_MODEL_ATTEMPTS = 3;
// Delivery gets its own cap, and a smaller one: a model retry may succeed where the last
// failed, but a visitor who blocked the bot will 403 every send forever.
const MAX_DELIVERY_ATTEMPTS = 3;

export function createVisitorSessionBase(runtime) {
  return class VisitorSessionBase {
    constructor(state, env) { this.state = state; this.env = env; }

    async fetch(request) {
      let update;
      try { update = await request.json(); } catch { return new Response("bad request", {status: 400}); }
      // Only the short, storage-only enqueue is blocked. Model calls run from alarm(), outside
      // blockConcurrencyWhile's 30-second ceiling; one alarm drains one item at a time.
      return this.state.blockConcurrencyWhile(() => this.enqueue(update));
    }

    async enqueue(update) {
      const uid = visitorId(update);
      if (!uid || !Number.isSafeInteger(update.update_id)) return new Response("ignored");
      const now = Date.now();
      const queueKey = `queue:${String(update.update_id).padStart(20, "0")}`;
      const retryKey = `retry:${String(update.update_id).padStart(20, "0")}`;
      const decision = await this.state.storage.transaction(async (txn) => {
        const last = Number(await txn.get("last_update_id") ?? -1);
        const retryPending = Boolean(await txn.get(retryKey));
        if (update.update_id <= last && !retryPending) {
          return {kind: "duplicate", queued: Boolean(await txn.get(queueKey))};
        }
        const recent = ((await txn.get("rate")) || []).filter((t) => now - t < 60000);
        if (recent.length >= rateLimit(this.env.SECRETARY_RATE_LIMIT)) {
          // The outer Worker must return non-2xx here, so remember that this ID was rejected.
          // Without this marker its retry would hit last_update_id and be acknowledged as a
          // duplicate even though it never entered the queue.
          await txn.put(retryKey, true);
          await txn.put("last_update_id", Math.max(last, update.update_id));
          await txn.put("last_outcome", "rate-limited");
          return {kind: "rate-limited"};
        }
        recent.push(now);
        await txn.put("rate", recent);
        await txn.put("last_update_id", Math.max(last, update.update_id));
        await txn.delete(retryKey);
        await txn.put(queueKey, {update, completed: null, attempts: 0});
        return {kind: "queued"};
      });
      if (decision.kind === "queued" || decision.queued) {
        const alarm = await this.state.storage.getAlarm();
        if (alarm == null) await this.state.storage.setAlarm(now);
      }
      if (decision.kind === "rate-limited") {
        return new Response("rate limited", {status: 429});
      }
      return new Response(decision.kind, {status: decision.kind === "queued" ? 202 : 200});
    }

    async alarm() {
      const queued = await this.state.storage.list({prefix: "queue:", limit: 1});
      const first = queued.entries().next();
      if (first.done) return;
      const [queueKey, stored] = first.value;
      const update = stored?.update || stored;
      let completed = stored?.completed || null;
      let attempts = Number(stored?.attempts || 0);
      let deliveryAttempts = Number(stored?.deliveryAttempts || 0);
      const uid = visitorId(update);

      let prev = await this.state.storage.get("previous_response_id");
      if (!prev && this.env.THREADS) {
        prev = await this.env.THREADS.get(`t:${uid}`);
        if (prev) {
          await this.state.storage.put("previous_response_id", prev);
          await this.env.THREADS.delete(`t:${uid}`);
        }
      }
      try {
        if (!completed) {
          completed = await runtime.executeUpdate(this.env, update, prev);
          await this.state.storage.transaction(async (txn) => {
            await txn.put(queueKey, {update, completed});
            if (completed.next) await txn.put("previous_response_id", completed.next);
            await txn.put("last_outcome", "model-completed");
          });
        }
        await runtime.deliverUpdate(this.env, update, completed, async (snapshot) => {
          await this.state.storage.put(queueKey, {update, completed: snapshot});
        });
        await this.state.storage.transaction(async (txn) => {
          await txn.put("last_outcome", "completed");
          await txn.delete(queueKey);
        });
      } catch (error) {
        console.log("secretary update failed", error?.code || error?.name || "error");
        if (completed) {
          // DELIVERY NEEDS A CAP TOO. `attempts` was incremented only in the else branch, so
          // once the model had run, ANY delivery error re-armed the alarm 30s later forever —
          // and since `alarm()` only ever reads the FIRST `queue:` key, one permanently failing
          // delivery (the visitor blocked the bot, so Telegram 403s every send) meant every
          // later message from that visitor was never processed, while the object burned a
          // model-free alarm every 30 seconds indefinitely. The answer is kept and the failure
          // reported, exactly as a model failure is; what stops is the retrying.
          deliveryAttempts += 1;
          if (deliveryAttempts < MAX_DELIVERY_ATTEMPTS) {
            await this.state.storage.transaction(async (txn) => {
              await txn.put(queueKey, {update, completed, deliveryAttempts});
              await txn.put("last_outcome", "delivery-pending");
            });
          } else {
            await this.state.storage.transaction(async (txn) => {
              await txn.put("last_outcome", "delivery-failed");
              await txn.delete(queueKey);
            });
            // Only if the VISITOR never got their answer. `deliverUpdate` sends the visitor
            // reply and then the team brief, so a team-brief failure would otherwise apologise
            // to a visitor who was already answered — telling them something broke when, for
            // them, nothing did.
            if (!completed.delivered?.visitor) await runtime.notifyFailure(this.env, update);
          }
        } else {
          attempts += 1;
          if (attempts < MAX_MODEL_ATTEMPTS) {
            await this.state.storage.transaction(async (txn) => {
              await txn.put(queueKey, {update, completed: null, attempts});
              await txn.put("last_outcome", "model-retry");
            });
          } else {
            await this.state.storage.transaction(async (txn) => {
              await txn.put("last_outcome", "failed");
              await txn.delete(queueKey);
            });
            await runtime.notifyFailure(this.env, update);
          }
        }
      }
      // DRAINING IS NOT RETRYING, and one constant was serving as both. A queued message that
      // nothing has failed on should be picked up now, not in 30 seconds: the rate limit allows
      // 6 messages/minute/visitor, so someone who sent three in a row waited 30s and 60s for
      // replies 2 and 3, and six queued messages put the last one ~2.5 minutes from even
      // reaching the model. The backoff belongs to the failure paths above, which set their own
      // outcome; a clean pass through schedules the next item immediately.
      const more = await this.state.storage.list({prefix: "queue:", limit: 1});
      if (more.size) {
        const failed = await this.state.storage.get("last_outcome");
        const backoff = failed === "model-retry" || failed === "delivery-pending";
        await this.state.storage.setAlarm(Date.now() + (backoff ? 30000 : 0));
      }
    }
  };
}
