#!/usr/bin/env python3
"""MultiAgency structured handoff (Session 2B) — reliable state transfer across a system boundary.

STATUS: NOT WIRED INTO THE PRODUCT — prerequisites cleared 2026-08-20, one step left.
Nothing on the client path calls this module yet (the bridge imports only `context_ingress`), so
today an "ADVANCE" decision in a client group produces ordinary prose and **no doc may promise
this object** until the caller exists. The two blockers that made wiring unsafe are now
fixed:
  1. `owner` / `value_band` had no source in the account schema, so the model would have invented
     exactly the fields where discipline matters. Now real nullable columns (`owner`, `stage`,
     `value_band` — schema.sql + migrate-002-handoff-fields.sql); NULL means the team hasn't
     recorded it and the brief must report UNKNOWN, never a guess.
  2. Generation used to advance the caller's `thread.prev` past a JSON-only turn (incl. failed
     attempts), contaminating the group's conversation. It now chains on a local `gen_prev` and
     never writes back to the caller's thread.
REMAINING TO WIRE — ONE STEP, NOT THREE (re-checked 2026-08-26; the list below had gone stale in
the direction that invents work):

  * OUTSTANDING, and the only real blocker: a product-path caller — an "ADVANCE" decision that
    generates, validates and delivers. It has no owner. Nothing else here can move until it does.
  * SATISFIED: "the migration applied to each live DB." `schema.sql` has declared `owner`,
    `stage` and `value_band` since 2026-08-20, so a DB built from it already has them;
    `migrate-002-handoff-fields.sql` is `ADD COLUMN IF NOT EXISTS`, i.e. idempotent and a no-op
    there; and `migrate.sh apply` runs from BOTH bring-up paths (`dev-up.sh`, `prod-up.sh`), so
    it is not a manual step anyone can forget. Verified on the laptop account DB: all three
    columns present. NOTE THE LIMIT — that is one box. A remote deployment is not measured from
    here, and "applied everywhere" is not a claim this file can make.
  * NOT YET, and correctly so: the README promise. No tracked doc promises THIS module's object
    — the 17-field account handoff — and none should until the caller exists. Stated precisely,
    because a loose version of this sentence is falsifiable by grep: `MULTI.template.md:55` and
    `ANALYST.md:34` do contain the words "structured brief"/"structured briefings", and neither
    is this (see below).

THE WORD "HANDOFF" MEANS THREE UNRELATED THINGS IN THIS REPO, and this module owns the newest
and least established of them. The other two:

  1. the **fleet-agent handoff** — `deploy/README.md` § Fleet-agent handoff, the human request
     format for standing up a NEW fleet agent (persona draft + provisioning notes);
  2. **`HANDOFF: ready`** — the Secretary persona's end-of-conversation signal that a visitor
     conversation is wrapped and the team should be given a lead brief.

...and "brief" collides twice more, which is why renaming this module to `brief.py` would trade
one collision for another: the Secretary **lead brief** (`deploy/secretary/brief-fields.json`)
and the **WORK ORDER** that `MULTI.template.md` calls "the structured brief the MultiAgency team
acts on" (goal / current situation / timeline — not an account object).

None of those four is a 17-field account object and none touches this file. A rename would remove
the collision, and the window is open BECAUSE nothing calls this module — the cost is this file, its
test, and no callers at all. That cost rises the moment the step above lands. Deliberately not
renamed here: the replacement noun is product vocabulary, "brief" is already taken by the
Secretary lead brief (`deploy/secretary/brief-fields.json`), and coining a term unilaterally is
how this repo has acquired names it later rejected. Decide it with the caller, while it is cheap.

Until then: exercised only by test_handoff_2b.py.

SHOULD IT SHIP AT ALL? THE REPO ALREADY HAS A WRITTEN TEST FOR THIS, and nobody had applied it
here. `docs/PRODUCT_DIRECTION.md` § Design test: a capability belongs in IronWorks when it
"serves a current organization-scoped use, keeps facts in durable records, keeps judgment in the
model turn, preserves human authority, and works around the unmodified pinned runtime.
Otherwise, defer it until a concrete requirement justifies expanding the product."

Applied honestly, 2026-08-26:

  current organization-scoped use  NO. Zero callers on the client path; no tenant env and no
                                   service definition in multi/services/ references it. It was
                                   built against a hypothesised need, not a request.
  facts in durable records         NO, BY ITS OWN DESIGN — see Scope above: in-memory only, no
                                   persistence, no write endpoint. The object is transient
                                   model output. That choice is right (a persisted one turns a
                                   prompt injection into a stored one) and it is also the
                                   reason this fails the test.
  judgment in the model turn       yes.
  preserves human authority        yes — the object is labelled DERIVED / verify before acting.
  works around the pinned runtime  yes.

Two of five, so the doc's own instruction applies: DEFER. Not delete, not wire — defer, which is
exactly what this file now IS: tracked, linted, gated by test_suite_contract.py, and honest in
its header about having no caller. That is the deferred state, and no further action is the
correct action until a concrete requirement arrives.

ONE TENSION TO SETTLE WITH THAT REQUIREMENT, not before it: `## Product boundaries` says
IronWorks does not "operate as a sales pipeline". Six of the thirteen model-authored fields
(`opportunity_hypothesis`, `commercial_timing`, `value_band`, `recommended_next_action`,
`follow_up_timing`, `relationship_path`) are pipeline-shaped. Reading recorded fields and
summarising them is not operating a pipeline, so this is a question rather than a violation —
but it is the question a caller has to answer, and it should be answered before the caller is
written rather than after.

Scope (decided 2026-08-18): IN-MEMORY transfer only. generate -> validate -> initialize a fresh
receiving context from the object. NO persistence, NO write endpoint, NO durable model-generated
state (which is exactly what would turn a transient prompt-injection into a stored one). The
persisted variant waits for (a) a real async reader and (b) native structured output + evidence_refs
that point at real store rows. (Background: the internal revenue-intelligence-vision and
ironclaw-upstream-direction notes — operator records, outside this repo.)

Security stance of THIS module:
  - Identity + provenance (account_id, account_name, source_thread_id, generated_at) are stamped by
    the adapter, NOT the model — the object cannot spoof which account/org it belongs to.
  - The object is model-generated content. The receiving side treats it as ATTRIBUTED / DERIVED
    ("agent-generated summary — verify before acting"), NEVER as trusted store facts. No
    trust-laundering: the "ACCOUNT RECORDS" envelope label is reserved for real store reads.
  - The model never gets a credential, a write tool, or a network reach here.

NOT an inter-agent platform: no agent-to-agent messaging, no shared memory, no autonomous room
creation, no tool continuation, no new account resolution, no coordination protocol.
"""
import re, json, datetime
try:
    from . import context_ingress as ing
except ImportError:
    import context_ingress as ing

# --- the canonical 17-field handoff contract ------------------------------------------------
# The model produces the 13 intelligence fields; the adapter authoritatively stamps the 4
# identity/provenance fields so they can't be model-spoofed.
_MODEL_STR = ["relationship_path", "opportunity_hypothesis", "commercial_timing", "value_band",
              "recommended_next_action", "owner", "follow_up_timing"]
_MODEL_LIST = ["key_people", "confirmed_facts", "assumptions", "unknowns", "evidence_refs", "risks"]
_PROV_STR = ["account_id", "account_name", "source_thread_id", "generated_at"]
STR_FIELDS = _MODEL_STR + _PROV_STR
LIST_FIELDS = _MODEL_LIST
REQUIRED = STR_FIELDS + LIST_FIELDS      # all 17

def validate(obj):
    """Deterministic schema check (no external dependency). Returns [] if valid, else the errors."""
    if not isinstance(obj, dict):
        return ["handoff must be a JSON object"]
    errs = []
    extra = set(obj) - set(REQUIRED)
    if extra:
        errs.append(f"unexpected keys: {sorted(extra)}")
    for k in STR_FIELDS:
        v = obj.get(k)
        if not isinstance(v, str) or not v.strip():
            errs.append(f"{k} must be a non-empty string")
    for k in LIST_FIELDS:
        v = obj.get(k)
        if not isinstance(v, list) or not all(isinstance(x, str) and x.strip() for x in v):
            errs.append(f"{k} must be a list of non-empty strings")
    return errs


_GEN_PROMPT = (
    "Produce the canonical handoff object for this prospect as ONE JSON object in your reply — no "
    "prose, no markdown, no code fence. Use EXACTLY these keys and no others:\n"
    + ", ".join(_MODEL_STR + _MODEL_LIST) + ".\n"
    "The string fields (" + ", ".join(_MODEL_STR) + ") are short strings. The list fields ("
    + ", ".join(_MODEL_LIST) + ") are arrays of strings.\n"
    "Rules: draw ONLY from this conversation and its trusted business context — do not invent facts. "
    "confirmed_facts = established facts; assumptions = your inferences; unknowns = genuine gaps. "
    "evidence_refs = for each key fact, cite its source (account-store activity date, or who said it "
    "and when). key_people = 'Name — Title (role/engagement)'. value_band = a coarse commercial size "
    "band. commercial_timing = the timing driver. Reply with ONLY the JSON object."
)


def _extract_json(text):
    """Tolerate a stray code fence or surrounding prose; parse the outermost {...}."""
    text = text.strip()
    if text.startswith("```"):
        text = re.sub(r"^```[a-zA-Z]*\n?", "", text)
        text = re.sub(r"\n?```$", "", text).strip()
    i, j = text.find("{"), text.rfind("}")
    if i != -1 and j > i:
        text = text[i:j + 1]
    return json.loads(text)


def generate_handoff(thread, account_id):
    """Generate ONE schema-constrained handoff object from a validated SI thread.

    READS the SI thread lineage (so it draws on the whole conversation) but NEVER writes back to
    it: generation chains on a LOCAL prev, so the caller's `thread.prev` still points at the last
    real conversational turn when this returns. Advancing the group's thread past a JSON-only turn
    (or a failed attempt) is what would contaminate the conversation — the client's next message
    would follow a machine-readable brief instead of the discussion.

    Tool-free (inline JSON — never a 'written'/'document' request, which makes the agent reach for
    a file tool and return in_progress). Identity + provenance are adapter-stamped. Retries once on
    invalid output. Returns the validated 17-field object (does NOT persist it)."""
    cl = thread.client
    ctx = ing._get_context(account_id, cl)
    if not ctx:
        raise ValueError(f"unknown account {account_id}")
    account_name = ctx["account"]["name"]
    source_thread_id = thread.prev or ""          # the validated SI thread state, before generation
    # Generation's OWN lineage: seeded from the conversation, advanced only locally. The caller's
    # thread.prev is never assigned here, so neither a JSON-only turn nor a failed attempt can end
    # up as the group conversation's anchor.
    gen_prev = thread.prev
    prompt, obj = _GEN_PROMPT, None
    for _ in range(2):
        # persona via `instructions` EVERY turn (once-only drifts — the repo's proven rule):
        # the evidence discipline the 17 fields depend on must govern THIS turn too.
        body = {"model": cl.model, "instructions": cl.persona, "input": prompt}
        if gen_prev:
            body["previous_response_id"] = gen_prev
        # Same completion semantics as ing.turn(): poll a tool-using turn to terminal. A
        # failed/still-running response raises here, and because only gen_prev advances, the SI
        # thread lineage is untouched either way.
        d = ing._completed(ing._await_completion(ing._post_ironclaw(body, cl), cl))
        gen_prev = d.get("id") or gen_prev
        try:
            model_obj = _extract_json(ing.output_text(d))
        except Exception:
            prompt = "Your last reply was not a single valid JSON object. " + _GEN_PROMPT
            continue
        obj = {k: model_obj.get(k) for k in (_MODEL_STR + _MODEL_LIST)}
        obj.update({
            "account_id": account_id,             # adapter-owned identity (not model-spoofable)
            "account_name": account_name,
            "source_thread_id": source_thread_id,
            "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        })
        errs = validate(obj)
        if not errs:
            return obj
        prompt = "Your last object had these problems: " + "; ".join(errs) + ". " + _GEN_PROMPT
    raise ValueError(f"handoff did not validate after retries: {validate(obj) if obj else 'no JSON'}")


def render_handoff(obj):
    """Human/agent-readable rendering that PRESERVES the confirmed/assumption/unknown separation."""
    def block(label, items):
        return [f"{label}:"] + [f"  - {x}" for x in items]
    L = [f"account: {obj['account_name']} ({obj['account_id']})",
         f"relationship_path: {obj['relationship_path']}",
         f"opportunity_hypothesis: {obj['opportunity_hypothesis']}"]
    L += block("key_people", obj["key_people"])
    L += block("confirmed_facts", obj["confirmed_facts"])
    L += block("assumptions", obj["assumptions"])
    L += block("unknowns", obj["unknowns"])
    L += block("evidence_refs", obj["evidence_refs"])
    L += [f"commercial_timing: {obj['commercial_timing']}",
          f"value_band: {obj['value_band']}",
          f"recommended_next_action: {obj['recommended_next_action']}",
          f"owner: {obj['owner']}",
          f"follow_up_timing: {obj['follow_up_timing']}"]
    L += block("risks", obj["risks"])
    L += [f"source_thread_id: {obj['source_thread_id']}", f"generated_at: {obj['generated_at']}"]
    return "\n".join(L)


def receiving_turn(handoff_obj, question, client=None):
    """Minimal reader: initialize a FRESH operator context from the handoff object ALONE (no original
    thread, no store fetch) and answer the operator's question.

    The object is labeled ATTRIBUTED / DERIVED — an agent-generated summary to verify, NOT trusted
    store facts — so the receiving side does not launder model output into ground truth."""
    cl = ing._client(client)
    if not cl.persona:
        raise RuntimeError(f"client {cl.slug!r} has no persona — refusing to serve (see Thread)")
    errs = validate(handoff_obj)
    if errs:
        raise ValueError(f"refusing to initialize from an invalid handoff: {errs}")
    envelope = (
        "RECEIVED HANDOFF — this is an AGENT-GENERATED summary handed to you across a system "
        "boundary. It is the ONLY context you have (you did NOT see the original conversation). "
        "Treat its claims and recommended action as DERIVED and to be VERIFIED, not as trusted "
        "store facts; honor its confirmed/assumption/unknown split.\n\n"
        + render_handoff(handoff_obj)
        + "\n\nOPERATOR QUESTION\n" + question)
    d = ing._completed(ing._await_completion(
        ing._post_ironclaw({"model": cl.model, "instructions": cl.persona,
                            "input": envelope}, cl), cl))   # fresh: no previous_response_id
    return ing.output_text(d)
