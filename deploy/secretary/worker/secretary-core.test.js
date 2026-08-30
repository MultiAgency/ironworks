import assert from "node:assert/strict";
import test from "node:test";

import {createSecretary, createTelegramWebhook, createVisitorSessionBase, mdToHtml, stripMd,
  textOf, visitorId}
  from "./secretary-core.js";

const update = (id, text = "hello") => ({update_id: id, message: {
  text, chat: {id: 99, type: "private"}, from: {id: 7, first_name: "Ada"},
}});

class Storage {
  constructor() { this.values = new Map(); this.alarmAt = null; }
  async get(key) { return this.values.get(key); }
  async put(key, value) { this.values.set(key, value); }
  async delete(key) { this.values.delete(key); }
  async list({prefix, limit}) {
    return new Map([...this.values.entries()].filter(([key]) => key.startsWith(prefix))
      .sort(([a], [b]) => a.localeCompare(b)).slice(0, limit));
  }
  async getAlarm() { return this.alarmAt; }
  async setAlarm(value) { this.alarmAt = value; }
  async transaction(callback) {
    const pending = new Map(this.values);
    const txn = {
      get: async (key) => pending.get(key),
      put: async (key, value) => pending.set(key, structuredClone(value)),
      delete: async (key) => pending.delete(key),
    };
    const result = await callback(txn);
    this.values = pending;
    return result;
  }
}

class State {
  constructor() { this.storage = new Storage(); this.tail = Promise.resolve(); }
  blockConcurrencyWhile(callback) {
    const result = this.tail.then(callback);
    this.tail = result.catch(() => {});
    return result;
  }
}

const env = (extra = {}) => ({SECRETARY_RATE_LIMIT: "6", THREADS: null, ...extra});

const webhookRequest = (item) => new Request("https://secretary.example/telegram", {
  method: "POST", body: JSON.stringify(item),
  headers: {"Content-Type": "application/json", "X-Telegram-Bot-Api-Secret-Token": "secret"},
});

function webhookEnv(stub) {
  return {WEBHOOK_SECRET: "secret", VISITOR_SESSIONS: {
    idFromName: (uid) => `visitor:${uid}`,
    get: (id) => { assert.equal(id, "visitor:7"); return stub; },
  }};
}

test("outer webhook returns 200 only after durable acceptance", async () => {
  let accepted = false;
  const response = await createTelegramWebhook().fetch(webhookRequest(update(1)), webhookEnv({
    fetch: async () => { await Promise.resolve(); accepted = true; return new Response("queued", {status: 202}); },
  }));
  assert.equal(response.status, 200);
  assert.equal(accepted, true);
});

test("outer webhook returns retryable failure when the durable object request fails", async () => {
  const response = await createTelegramWebhook().fetch(webhookRequest(update(2)), webhookEnv({
    fetch: async () => { throw new Error("durable object unavailable"); },
  }));
  assert.equal(response.status, 503);
});

test("outer webhook returns retryable failure when enqueue rejects before commit", async () => {
  const Session = createVisitorSessionBase({executeUpdate: async () => {},
    deliverUpdate: async () => {}, notifyFailure: async () => {}});
  const state = new State();
  state.storage.transaction = async (callback) => {
    const pending = new Storage();
    pending.values = new Map(state.storage.values);
    await callback(pending);
    throw new Error("transaction aborted before commit");
  };
  const session = new Session(state, env());
  const response = await createTelegramWebhook().fetch(webhookRequest(update(3)), webhookEnv({
    fetch: (url, init) => session.fetch(new Request(url, init)),
  }));
  assert.equal(response.status, 503);
  assert.equal(await state.storage.get("last_update_id"), undefined);
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 0);
});

test("outer webhook retry accepts one durable copy after a failed attempt", async () => {
  const accepted = [];
  let attempt = 0;
  const stub = {fetch: async (_url, init) => {
    attempt += 1;
    if (attempt === 1) throw new Error("failed before commit");
    const item = JSON.parse(init.body);
    if (!accepted.includes(item.update_id)) accepted.push(item.update_id);
    return new Response(attempt === 2 ? "queued" : "duplicate", {status: attempt === 2 ? 202 : 200});
  }};
  const webhook = createTelegramWebhook();
  assert.equal((await webhook.fetch(webhookRequest(update(4)), webhookEnv(stub))).status, 503);
  assert.equal((await webhook.fetch(webhookRequest(update(4)), webhookEnv(stub))).status, 200);
  assert.equal((await webhook.fetch(webhookRequest(update(4)), webhookEnv(stub))).status, 200);
  assert.deepEqual(accepted, [4]);
});

test("outer acceptance leaves model and delivery recovery to the visitor state machine", async () => {
  let executions = 0;
  let deliveries = 0;
  const runtime = {
    executeUpdate: async () => {
      executions += 1;
      return {next: "saved-response", reply: "hello", delivered: {visitor: false}};
    },
    deliverUpdate: async () => {
      deliveries += 1;
      if (deliveries === 1) throw new Error("telegram unavailable after acceptance");
    },
    notifyFailure: async () => {},
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  const webhook = createTelegramWebhook();
  const stub = {fetch: (url, init) => session.fetch(new Request(url, init))};

  assert.equal((await webhook.fetch(webhookRequest(update(5)), webhookEnv(stub))).status, 200);
  assert.equal(executions, 0, "outer acceptance started model work");
  await session.alarm();
  assert.equal(await state.storage.get("last_outcome"), "delivery-pending");
  await session.alarm();
  assert.equal(executions, 1);
  assert.equal(deliveries, 2);
  assert.equal(await state.storage.get("last_outcome"), "completed");
});

test("formatters escape Telegram HTML attributes and preserve plain fallback", () => {
  const html = mdToHtml('[click](https://example.com/"bad) **now**');
  assert.match(html, /&quot;/);
  assert.doesNotMatch(html, /href="[^"]*"bad/);
  assert.equal(stripMd("# Hi\n- **there**"), "Hi\n• there");
});

test("response reader exposes message output only", () => {
  assert.equal(textOf({output: [{type: "reasoning", content: [{type: "text", text: "secret"}]},
    {type: "message", content: [{type: "output_text", text: "visible"}]}]}), "visible");
});

test("only text private messages resolve to a visitor", () => {
  assert.equal(visitorId(update(1)), "7");
  assert.equal(visitorId({update_id: 2, message: {text: "x", chat: {type: "group"}, from: {id: 7}}}), null);
  assert.equal(visitorId({update_id: 3, message: {chat: {type: "private"}, from: {id: 7}}}), null);
});

test("simultaneous and duplicate updates are queued once and processed in order", async () => {
  const calls = [];
  const runtime = {executeUpdate: async (_env, item, prev) => {
    calls.push([item.update_id, prev]);
    return {next: `resp-${item.update_id}`, delivered: {}};
  }, deliverUpdate: async () => {}, notifyFailure: async () => {}};
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  const request = (item) => new Request("https://do/update", {method: "POST",
    body: JSON.stringify(item), headers: {"Content-Type": "application/json"}});
  const responses = await Promise.all([
    session.fetch(request(update(10))), session.fetch(request(update(10))),
    session.fetch(request(update(11))),
  ]);
  assert.deepEqual(responses.map((r) => r.status), [202, 200, 202]);
  await session.alarm();
  await session.alarm();
  assert.deepEqual(calls, [[10, undefined], [11, "resp-10"]]);
});

test("legacy KV continuity migrates once into the visitor object", async () => {
  const seen = [];
  const kv = {get: async (key) => key === "t:7" ? "legacy-response" : null,
    delete: async (key) => seen.push(key)};
  const runtime = {executeUpdate: async (_env, _item, prev) => {
    assert.equal(prev, "legacy-response"); return {next: "new-response", delivered: {}};
  }, deliverUpdate: async () => {}, notifyFailure: async () => {}};
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env({THREADS: kv}));
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(20))}));
  await session.alarm();
  assert.deepEqual(seen, ["t:7"]);
  assert.equal(await state.storage.get("previous_response_id"), "new-response");
});

test("rate limit bounds accepted work per visitor", async () => {
  const Session = createVisitorSessionBase({executeUpdate: async () => ({next: "x", delivered: {}}),
    deliverUpdate: async () => {}, notifyFailure: async () => {}});
  const session = new Session(new State(), env({SECRETARY_RATE_LIMIT: "2"}));
  const send = (id) => session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(id))}));
  assert.equal((await send(30)).status, 202);
  assert.equal((await send(31)).status, 202);
  assert.equal((await send(32)).status, 429);
  assert.equal((await send(32)).status, 429,
    "a rejected update retry was misclassified as an accepted duplicate");
});

test("outer webhook never acknowledges a rate-limited retry as durably accepted", async () => {
  const Session = createVisitorSessionBase({executeUpdate: async () => ({next: "x", delivered: {}}),
    deliverUpdate: async () => {}, notifyFailure: async () => {}});
  const session = new Session(new State(), env({SECRETARY_RATE_LIMIT: "1"}));
  const webhook = createTelegramWebhook();
  const stub = {fetch: (url, init) => session.fetch(new Request(url, init))};

  assert.equal((await webhook.fetch(webhookRequest(update(40)), webhookEnv(stub))).status, 200);
  assert.equal((await webhook.fetch(webhookRequest(update(41)), webhookEnv(stub))).status, 503);
  assert.equal((await webhook.fetch(webhookRequest(update(41)), webhookEnv(stub))).status, 503);
});

test("malformed rate limits fail closed instead of accepting unlimited work", async () => {
  const runtime = {executeUpdate: async () => ({next: "x", delivered: {}}),
    deliverUpdate: async () => {}, notifyFailure: async () => {}};
  for (const configured of ["not-a-number", "0", "2.5"]) {
    const Session = createVisitorSessionBase(runtime);
    const session = new Session(new State(), env({SECRETARY_RATE_LIMIT: configured}));
    const send = (id) => session.fetch(new Request("https://do/update", {method: "POST",
      body: JSON.stringify(update(id))}));
    assert.equal((await send(1)).status, 202);
    assert.equal((await send(2)).status, 429);
  }
});

test("enqueue commits deduplication, rate accounting, and queue atomically", async () => {
  const Session = createVisitorSessionBase({executeUpdate: async () => ({next: "x"}),
    deliverUpdate: async () => {}, notifyFailure: async () => {}});
  const state = new State();
  const original = state.storage.transaction.bind(state.storage);
  let fail = true;
  state.storage.transaction = async (callback) => {
    if (fail) {
      fail = false;
      await original(async (txn) => { await callback(txn); throw new Error("crash before commit"); });
    }
    return original(callback);
  };
  const session = new Session(state, env());
  const request = () => new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(10))});
  await assert.rejects(session.fetch(request()), /crash before commit/);
  assert.equal(await state.storage.get("last_update_id"), undefined);
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 0);
  assert.equal((await session.fetch(request())).status, 202);
});

test("delivery retry preserves the response pointer and does not rerun the model", async () => {
  let executions = 0;
  let deliveries = 0;
  const runtime = {
    executeUpdate: async (_env, item, prev) => {
      executions += 1;
      assert.equal(item.update_id, 60);
      assert.equal(prev, undefined);
      return {next: "durable-response", reply: "hello", delivered: {visitor: false}};
    },
    deliverUpdate: async () => {
      deliveries += 1;
      if (deliveries === 1) throw new Error("telegram unavailable");
    },
    notifyFailure: async () => { throw new Error("model completed; do not send failure reply"); },
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(60))}));
  await session.alarm();
  assert.equal(await state.storage.get("previous_response_id"), "durable-response");
  assert.equal(await state.storage.get("last_outcome"), "delivery-pending");
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 1);
  await session.alarm();
  assert.equal(executions, 1);
  assert.equal(deliveries, 2);
  assert.equal(await state.storage.get("last_outcome"), "completed");
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 0);
});

test("transient model failure remains queued and recovers with the same update", async () => {
  let executions = 0;
  let notifications = 0;
  const runtime = {
    executeUpdate: async () => {
      executions += 1;
      if (executions === 1) throw new Error("transient");
      return {next: "recovered", delivered: {}};
    },
    deliverUpdate: async () => {},
    notifyFailure: async () => { notifications += 1; },
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(61))}));
  await session.alarm();
  assert.equal(await state.storage.get("last_outcome"), "model-retry");
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 1);
  await session.alarm();
  assert.equal(executions, 2);
  assert.equal(notifications, 0);
  assert.equal(await state.storage.get("previous_response_id"), "recovered");
  assert.equal(await state.storage.get("last_outcome"), "completed");
});

test("upstream HTTP and malformed JSON failures are contained and client-notified", async () => {
  for (const response of [new Response("no", {status: 503}), new Response("not-json")]) {
    let notified = 0;
    const secretary = createSecretary({persona: "p", modelPin: "m",
      brief: {fields: ["summary"], ask_template: "{FIELDS}"},
      fetchImpl: async (url) => url.includes("ironclaw") ? response.clone()
        : Response.json({ok: true, result: {message_id: 1}})});
    const runtime = {...secretary, notifyFailure: async () => { notified += 1; }};
    const Session = createVisitorSessionBase(runtime);
    const state = new State();
    const session = new Session(state, env({IRONCLAW_API: "https://ironclaw.invalid",
      IRONCLAW_TOKEN: "token", SECRETARY_BOT_TOKEN: "bot"}));
    await session.fetch(new Request("https://do/update", {method: "POST",
      body: JSON.stringify(update(40))}));
    await session.alarm();
    await session.alarm();
    await session.alarm();
    assert.equal(notified, 1);
    assert.equal(await state.storage.get("last_outcome"), "failed");
  }
});

test("handoff marker is removed from visitor output and brief is schema validated", async () => {
  const telegramBodies = [];
  let modelCall = 0;
  const fetchImpl = async (url, init) => {
    if (url.includes("api.telegram.org")) {
      telegramBodies.push(JSON.parse(init.body));
      return Response.json({ok: true, result: {message_id: 1}});
    }
    modelCall += 1;
    const text = modelCall === 1 ? "Thanks\nHANDOFF: hidden" : '{"summary":"Qualified"}';
    return Response.json({id: `r${modelCall}`, status: "completed", output: [
      {type: "message", content: [{type: "output_text", text}]},
    ]});
  };
  const secretary = createSecretary({persona: "p", modelPin: "m",
    brief: {fields: ["summary"], ask_template: "Return {FIELDS}"}, fetchImpl});
  const next = await secretary.handleUpdate(env({IRONCLAW_API: "https://ic",
    IRONCLAW_TOKEN: "token", SECRETARY_BOT_TOKEN: "bot", TEAM_CHAT_ID: "team"}), update(50), "r0");
  assert.equal(next, "r2");
  assert.ok(telegramBodies.some((body) => body.text?.includes("Lead brief")));
  assert.ok(telegramBodies.every((body) => !body.text?.includes("hidden")));
});

test("a permanently failing delivery is capped instead of wedging the visitor's queue", async () => {
  // `attempts` was incremented only when the model had NOT run, so once an answer existed any
  // delivery error re-armed the alarm 30s later forever. `alarm()` only ever reads the FIRST
  // `queue:` key, so a visitor who blocked the bot (Telegram 403s every send) had every later
  // message go unprocessed while the object burned a model-free alarm every 30 seconds.
  let executions = 0;
  let deliveries = 0;
  let notified = 0;
  const runtime = {
    executeUpdate: async () => {
      executions += 1;
      return {next: "durable-response", reply: "hello", delivered: {visitor: false}};
    },
    deliverUpdate: async () => { deliveries += 1; throw new Error("Forbidden: bot was blocked"); },
    notifyFailure: async () => { notified += 1; },
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(70))}));
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(71))}));

  for (let i = 0; i < 10; i++) await session.alarm();

  // Three attempts EACH: message 70 is given up on, and then 71 is picked up and given its own
  // three. Unbounded, 70 alone would have consumed every alarm forever.
  assert.equal(deliveries, 6, "delivery was retried without a cap");
  assert.equal(notified, 2, "the failure was never reported to the visitor");
  assert.equal(await state.storage.get("last_outcome"), "delivery-failed");
  // AND THE SECOND MESSAGE GOT THROUGH: the head of the queue no longer blocks it forever.
  assert.equal(executions, 2, "the next queued message was never processed");
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 0);
});

test("every persisted queue record carries the counters the caps are read from", async () => {
  // THE MID-DELIVERY CHECKPOINT DROPPED THEM. `deliverUpdate` takes a snapshot callback so a
  // partially-delivered answer is not re-sent, and that checkpoint persisted `{update,
  // completed}` — no `attempts`, no `deliveryAttempts`. `alarm()` reads them back as
  // `stored?.deliveryAttempts || 0`, so that record restores with the cap reset.
  //
  // ASSERTED ON THE PERSISTED RECORD, not on a retry count, and that distinction is the whole
  // test. A retry count cannot see this: within one alarm the catch block increments its own
  // in-memory copy and overwrites the checkpoint, so the cap still lands on 3 with the bug
  // present. Only an isolate evicted between the checkpoint and the catch loses it — and a
  // durable object cannot be evicted on demand from a test. What CAN be checked, and is the
  // invariant that makes eviction survivable, is that no write ever leaves the record unable
  // to answer "how many attempts have there been".
  const seen = [];
  const runtime = {
    executeUpdate: async () => ({next: "r1", reply: "hello", delivered: {visitor: false}}),
    deliverUpdate: async (_env, _update, completed, snapshot) => {
      await snapshot({...completed, delivered: {visitor: false, partial: true}});
      // Read back what the checkpoint just committed — the state an evicted isolate would wake to.
      const entries = await state.storage.list({prefix: "queue:", limit: 10});
      for (const [, record] of entries) seen.push(record);
      throw new Error("Forbidden: bot was blocked");
    },
    notifyFailure: async () => {},
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(80))}));
  for (let i = 0; i < 10; i++) await session.alarm();

  assert.equal(seen.length, 3, "the checkpoint was not exercised on every delivery attempt");
  seen.forEach((record, i) => {
    assert.equal(typeof record.deliveryAttempts, "number",
      `checkpoint ${i} persisted no deliveryAttempts — an evicted isolate resets the cap`);
    assert.equal(typeof record.attempts, "number",
      `checkpoint ${i} persisted no attempts`);
  });
  // The cap itself still holds, so the record shape above is not bought at its expense.
  assert.equal(await state.storage.get("last_outcome"), "delivery-failed");
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 0);
});

test("a queued message is drained immediately, not on the 30s retry timer", async () => {
  // The retry backoff and the drain schedule were one constant. The rate limit allows 6
  // messages/minute/visitor, so three messages in a row meant waiting 30s and 60s for replies
  // 2 and 3, and six queued meant ~2.5 minutes before the last reached the model at all.
  const runtime = {
    executeUpdate: async () => ({next: "r", reply: "hi", delivered: {visitor: true}}),
    deliverUpdate: async () => {},
    notifyFailure: async () => {},
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  for (const id of [80, 81, 82]) {
    await session.fetch(new Request("https://do/update", {method: "POST",
      body: JSON.stringify(update(id))}));
  }
  const before = Date.now();
  await session.alarm();
  assert.equal((await state.storage.list({prefix: "queue:", limit: 10})).size, 2);
  assert.ok(state.storage.alarmAt - before < 1000,
    `queue drain was scheduled ${state.storage.alarmAt - before}ms out, not immediately`);
});

test("a parse-mode rejection retries as plain text instead of dropping the reply", async () => {
  // Telegram reports `Bad Request: can't parse entities` as HTTP 400, which jsonRequest turns
  // into UpstreamError("telegram", "http-400") — never "rejected", the only code the fallback
  // used to accept. So the one failure this retry exists for was the one it rethrew, and the
  // visitor got no reply at all. mdToHtml("# **Hi**") produces `<b><b>Hi</b></b>`, which
  // Telegram 400s on, so ordinary model output reaches it.
  assert.equal(mdToHtml("# **Hi**"), "<b><b>Hi</b></b>");
  const sends = [];
  const fetchImpl = async (url, init) => {
    const params = JSON.parse(init.body);
    sends.push(params);
    if (params.parse_mode === "HTML") {
      return new Response(JSON.stringify({ok: false, description: "can't parse entities"}),
        {status: 400, headers: {"Content-Type": "application/json"}});
    }
    return new Response(JSON.stringify({ok: true, result: {message_id: 1}}),
      {status: 200, headers: {"Content-Type": "application/json"}});
  };
  const secretary = createSecretary({
    persona: "p", modelPin: "m", fetchImpl,
    brief: {fields: ["a"], ask_template: "give {FIELDS}"},
  });
  const completed = {cid: 99, reply: "# **Hi**", first: false, who: "Ada", payload: "",
                     teamBrief: "", delivered: {teamNotice: false, visitor: false,
                                                teamBrief: false}};
  await secretary.deliverUpdate({SECRETARY_BOT_TOKEN: "t"}, update(90), completed);
  assert.equal(completed.delivered.visitor, true,
    "the reply was dropped rather than retried as plain text");
  const messages = sends.filter((p) => p.text !== undefined);
  assert.equal(messages.length, 2, JSON.stringify(messages));
  assert.equal(messages[1].parse_mode, undefined);
  assert.equal(messages[1].text, "Hi");
});

test("a team-brief failure does not apologise to an already-answered visitor", async () => {
  // deliverUpdate sends the visitor reply first, then the team brief. Notifying on any delivery
  // failure told a visitor who HAD been answered that something broke — for them, nothing did.
  let notified = 0;
  const runtime = {
    executeUpdate: async () => ({next: "r", reply: "hi", delivered: {visitor: false}}),
    deliverUpdate: async (_env, _update, completed) => {
      completed.delivered.visitor = true;      // the visitor is answered...
      throw new Error("team chat unavailable"); // ...and only the team brief fails
    },
    notifyFailure: async () => { notified += 1; },
  };
  const Session = createVisitorSessionBase(runtime);
  const state = new State();
  const session = new Session(state, env());
  await session.fetch(new Request("https://do/update", {method: "POST",
    body: JSON.stringify(update(95))}));
  for (let i = 0; i < 5; i++) await session.alarm();
  assert.equal(await state.storage.get("last_outcome"), "delivery-failed");
  assert.equal(notified, 0, "an answered visitor was told their message failed");
});
