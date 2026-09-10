# Helping IronClaw

**This repository runs official, unmodified IronClaw, intends to keep doing so permanently, and will
never contain a patch to it.** The rule itself is owned by `README.md`, `CLAUDE.md` and
`docs/AUDIT-BRIEF.md`; what matters here is the corollary they do not spell out: **nothing on this
page is a step toward divergence.** We are not building leverage, not staging a fork, and not
seeking special-case behaviour for our shape. If a thing we want would only ever help us, the answer
is to change our side or accept the constraint — not to ask.

`docs/AUDIT-BRIEF.md` already defines `UPSTREAM WORK` as one of five work-owner classifications.
**Nothing registered what had been classified that way.** This is that register, and it answers the
question the rule leaves open: *a fix belongs upstream — so what have we actually got, and what
would a maintainer need to act on it?*

## The bar for proposing anything

**Only things a maintainer would want for their own reasons, and that could realistically merge.**
Being useful to us is not a reason; it is barely even evidence. Each entry has to clear all four:

1. **It serves IronClaw's own goals**, stated in their README, docs or architecture — not ours.
2. **It is in the grain of the existing design.** A change that fits a mechanism they already built
   is a review; one that asks them to adopt a new concept is a negotiation, and we are not the
   party who should be opening one.
3. **It is small enough to review, and reproducible without us.** A maintainer must be able to
   confirm it from their own tree and a pinned revision, with no access to our deployment, our
   tenants or our records.
4. **It leaves them owing us nothing.** No support obligation, no compatibility promise to a
   downstream they did not sign up for, no maintenance burden transferred.

**Bug reports clear this trivially** and that is why they lead. A proposal that changes behaviour is
held to all four and is written as a question, not a patch waiting to be accepted. **Anything that
fails keeps its entry and carries the bar it failed** — beside the assessment in §3 where the
reasoning is worth seeing applied, or in §4 where the answer is a standing one. A decline recorded
is a decision; a decline omitted is just a silence somebody re-derives.

## How to read an entry

**Every entry cites its evidence and does not restate it.** Where that evidence is an experiment,
the experiment is the thing that was checked and repeating its figures here would be one fact in two
documents. What is written here instead is the part the evidence has no reason to carry: how much it
matters to somebody who maintains the runtime, what they would need in order to reproduce it, what
this seam does about it meanwhile, and whether it has been sent.

**The evidence is not all of one kind, and the page should not pretend otherwise.** §1 is measured —
runs against a disposable instance at the pin. D9 is a **source reading**: two of their own documents
disagreeing, confirmable without running anything. D7 cites a doc-comment on the code that carries
the measurement rather than an experiment. §3's declined items are **judgements against the bar**,
not findings at all. Everything that *is* measured was measured at `IRONCLAW_PIN` (`4cb47cfaf3`,
v1.4.0), and **a finding goes stale silently** — `docs/UPGRADE.md` is explicit that a source reading
is not a compatibility claim — so re-measure before filing, not merely before believing.

**Nothing here has been filed.** §1 carries a `Filed` column so that stays visible rather than
assumed; D9 and §3 have no column and the same is true of them. When the first thing is sent, give
it a column rather than a sentence somebody has to keep in sync.

---

## 1. Measured defects

Ordered by what a maintainer can act on, not by what inconveniences us.

| | What | Evidence | Filed |
|---|---|---|---|
| **D1** | **A settings write returns `200` without applying, and the member becomes permanently un-confinable.** Approval-off is one-way: the second `false` reuses a completed activity id and silently no-ops, so a member whose approval was ever re-enabled cannot be confined again at all. A success code for a no-op is the sharpest kind of API defect — every caller that reads the status is wrong. | §L-9 | no |
| **D2** | **`POST /api/v1/responses/{id}/cancel` is mounted and returns `400 invalid_request` with `param: null` in every state measured**, while the webchat run-cancel on the same run in the same state works and is idempotent. A dead route on the public API surface. | `0037` check 5 | no |
| **D3** | **A run waiting on the caller reports `status: "completed"`** with a trailing `function_call` item and no message. OpenAI's Responses API has `requires_action` for this; any client written to the spec mis-reads it as a finished turn with empty output. | `0032` | no |
| **D4** | **`model` is echoed on create and returned as the literal `"reborn"` on retrieval**, so which model produced a stored answer is unverifiable through the API that produced it. *Meanwhile:* the seam reads the field on acceptance — the only moment it is a name — and journals it as `ironworks_domain::ReportedModel`, which stores the placeholder as itself rather than as a model. That makes our own record answerable and changes nothing about theirs, so the defect stands as written. | `0013`, the pin-honouring half of `0040`, and `0059` for the placeholder observed live | no |
| **D5** | **`usage.cost` is wrong in both directions from two unrelated causes** — a name heuristic prices one family at zero on create, and retrieval prices the placeholder model at another vendor's rates. Token counts are correct throughout; only cost is affected. | `0040` | no |
| **D6** | **User creation precedes its one-time bearer mint and there is no reissue endpoint**, so a failed mint leaves a permanently tokenless user that must be deleted and re-created. | `0043` | no |
| **D7** | **`?user_id=` is silently ignored on the settings routes, which resolve the caller's own scope — and the write succeeds against the wrong principal.** An operator token used to confine a member "would confine the operator and report success". Same family as D1: a success code for something other than what was asked. | `crates/reconcile/src/ironclaw.rs::post_as_member`, whose doc records the measurement | no |
| ~~**D10**~~ | ~~The runtime rejects its own model's tool calls unless the prompt supplies the wire spelling, per tool.~~ **Withdrawn 2026-09-10, before filing: the measurement stands and the classification does not.** IronClaw advertises names in its provider-tool-name grammar (`builtin__shell`), the model answers with the capability-id spelling (`builtin.shell`), and the validator refuses it. Reading the pin, that is the contract working: `ToolCallContent.capability` is documented as a *normalized capability identity, never a raw provider tool name*, so the two forms are deliberately distinct types and checking a returned string against the grammar it was offered in is correct. `model_invalid_output` is literally accurate. The loop also re-selects its route and rebuilds the message list between attempts rather than replaying one request, so the retries are a fair second chance at a nondeterministic model. **Whose fault the spelling is could not be established from outside** — provider normalisation and the model itself are both live possibilities. What survives is the diagnosability ask in §3, which is where it belongs. The measured behaviour is `0057` and `0059`; they are dated records and stand unedited. | `0057`, `0059` | withdrawn |
| **D8** | **Deleting or suspending a member removes no authority from an already-issued session.** Described here as a class, deliberately: the session lifetime is long, compiled in, and has no configuration path, and the seam models no suspension state because doing so would be a lie in the type system. `docs/AUDIT-BRIEF.md` names this as the original `UPSTREAM WORK` item and states the containment that stays until an official release changes it. | §F-4, §F-5, `docs/AUDIT-BRIEF.md` | no |

**D8 is the one to handle carefully.** It is a security property, and the useful report describes
the shape and the containment — not a route to exploiting it.

## 2. A documentation defect, and the easiest thing on this page to fix

**D9 — the public API documentation contradicts the code at the pin.**
`ironclaw@4cb47cfaf3/docs/api/responses.mdx:438` tells readers that a `POST` carrying a foreign
`previous_response_id` does **not** return `404`, and that "the handler only decodes the UUID and
dispatches into the agent." At this revision it does return `404`, before any dispatch:
`lookup_authorized` returns `None` for a mapping that fails its ownership check
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/refs_storage.rs:352`), which becomes
a not-found at
`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:588`.

Their own internal contract document has it right
(`ironclaw@4cb47cfaf3/docs/internal/reborn/contracts/openai-compatible-api.md:148`), so this is
drift between two documents they maintain, with a one-line fix and no judgement call in it. It is
the obvious first filing: small, verifiable from source alone, and it costs a maintainer nothing to
accept.

Found while resolving a question this seam had about its own error handling
(`docs/INVARIANTS.md` §I), which is worth saying plainly — **the most useful thing we have to offer
upstream is a by-product of using the runtime carefully, not of auditing it.**

## 3. Opportunities that are not defects

**Two clear the bar.**

- **A configuration edge for the org policy ceiling.** *Clears all four.* The kernel already has the
  mechanism — the runtime-policy resolver is the only sanctioned producer of an effective policy,
  and its org constraints already carry a profile ceiling with a setter. Outside tests, **nothing
  constructs one**: no config key, no environment variable, no route. So a capability their own
  architecture defines is unreachable by any operator, which is a gap in *their* enterprise and
  org-admin story before it is anything to us. In the grain (it wires an existing type to existing
  config parsing), small, reproducible from a pinned checkout, and it obliges them to nothing.
  Written as a question, because they may have a reason it is unwired that the source does not show.
  The measured downstream consequence is in `0051` finding 2; the caveat that it reaches only one
  composition profile as scoped is in `docs/adr/0012-defer-the-move-to-the-production-composition-profile.md`.

- **Surfacing the failure category on a failed response.** *Clears all four, narrowly, and is a
  question rather than a request.* A turn that fails on the model's own output ends with a
  `SanitizedFailure` carrying both a `category` and a `detail`; the Responses API shows a client
  only `internal_error` / "An internal error occurred." Finding D10 therefore required raising the
  runtime's log level, which a hosted caller cannot do. The ask is the **category** alone — a small,
  closed set they already compute — never the detail, because the sanitisation is deliberate and
  the detail is where provider text would leak. In the grain (the value exists at the boundary and
  is discarded), small, reproducible from a pinned checkout, and it obliges them to nothing. Asked
  as a question because a stable category is a compatibility surface, and that is their call to
  make, not ours to assume. Evidence: `0057`.

**Two do not, and are recorded here so the decline is a decision.**

- **A multi-tenant conformance suite.** Fails bar 4. We could offer `dev/prove-tenancy.sh` and the
  isolation checks, and the *observation* behind it is sound — they ship a multi-tenant deployment
  profile, a hosted multi-tenant mode and an admin-user API while the README describes a personal
  assistant. But a downstream's suite living in their repository is a maintenance burden they did
  not ask for, and it quietly makes our deployment shape something they must not break. **If it is
  ever offered it is as something to read, never as a PR** — and the honest version is to publish it
  here and tell them where it is.
- **Our evidence discipline.** Not a contribution at all. `docs/UPGRADE.md`'s per-fact
  `[measured @ rev]` markers are a practice, and a practice is not a change anyone can merge.
  Recorded only because it is the thing most likely to be mistaken for one.

## 4. What we deliberately do not ask for

Recorded so that a decline reads as a decision rather than an oversight — the shape
`0037` established for this seam's own omissions.

- **Anything that would make us a fork.** The rule at the top is not reluctance; a boundary is only
  meaningful while the other side is somebody else's code.
- **Model-directed tool execution.** `0031` and §L-2 measured that declaring tools hands the runtime
  its own agent loop, which routes around this seam's envelope. That is correct behaviour for the
  runtime and wrong for this product. It is `DIVERGENT BY DESIGN`, not a defect.
- **Changes to the consumer privacy posture.** `docs/AUDIT-BRIEF.md` is explicit that IronClaw's
  promise about data staying on a user's machine does not describe a vendor-operated seam, and that
  we must not carry it across the boundary. We are not asking them to make it fit us.
- **A longer, configurable, or pollable create wait.** §G-5's thirty-second cliff is the single
  most expensive upstream behaviour this seam meets: `DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30s, a
  turn that outlasts it is refused `503 service_unavailable`, and
  [`0036`](experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md) measured the
  shipped service's own flagship question landing on it eleven times in seventeen. Measured again
  2026-09-10 at this pin and it is now worse: a one-account ask and the forty-record ask both
  refused, **4 of 4, every one at 30-31s**, so `0036`'s lever — narrowing the question — no longer
  moves it.
  **It is still not something to ask them for.** The design already answers it: the submission
  happened, the turn runs to completion server-side, and replaying the idempotency key retrieves the
  answer (ADR 0005, measured at 0.085s). This seam implements exactly that and it works — the
  redelivery check in `dev/prove-delivery.sh` passes on the runs where everything else blocks. What
  makes the wait too short is **our** shape: an alloy plus a forty-record envelope handed to a
  reasoning model. Asking for a longer wait would serve one workload — ours — which the bar above
  rejects outright; asking for a pollable response id instead of a refusal would ask them to adopt a
  different interaction model, which is a negotiation and not a review. The lever that remains is
  envelope size, and that is ours (`docs/INVARIANTS.md` §I).
  The cost of accepting it is stated rather than hidden: three of `dev/prove-delivery.sh`'s four
  checks rest on a create count that a cliff invalidates, so on a slow day they report BLOCKED and
  the delivery boundaries go unmeasured. `dev/lib.sh` carries the evidence for the two repairs that
  do **not** work — a harness retry contradicts the property under test, and narrowing the question
  no longer clears the wait.

- **Distribution through their catalogue**, for now. The catalogue has a private provenance tier
  that would scope a signed package to one organization, which is a real channel. It is declined
  under the current "pure seam" direction rather than unavailable.
