"""The `/v1/responses` wire details the product and the proofs must share, so they cannot drift.

TWO THINGS LIVE HERE, and the bar for a third is the same one both cleared: it is sent on or
read from a `/v1/responses` call, the product and a proof must do it IDENTICALLY or the proof
measures a system nobody runs, and it can be stated with no import weight. Anything that fails
the last test belongs in `context_ingress`; anything that fails the second belongs to whichever
side owns it. This is not the module for "small things about responses".

  1. `output_text` — what the model actually said. The reason this module exists; the story is
     below and it is the longer half of the file on purpose.
  2. `BROWSER_UA` — the User-Agent the call carries. Added because it had exactly the same
     defect with exactly the same cause: three copies, no owner, and a comment for a guard.

WHAT THIS IS FOR. "What did the model actually say?" was answered by three separate walks of the
same document — `context_ingress._output_text`, `multi/verify/common.text_of`, and
`deploy/egress/proof/proof_checks.text_of` — and they did not agree. The product filtered on
item and content TYPE; both proof copies walked every content entry that carried a `text` key.

That is not a cosmetic difference. Measured against a document with a reasoning item beside the
message:

    {"output": [{"type": "reasoning", "content": [{"type": "reasoning_text",
                                                   "text": "INTERNAL SCRATCHPAD"}]},
                {"type": "message",   "content": [{"type": "output_text",
                                                   "text": "the real answer"}]}]}

    product -> "the real answer"
    proofs  -> "INTERNAL SCRATCHPAD\nthe real answer"

So the proofs could assert on text the product would never deliver, and specifically on the
model's own reasoning. The two injection proofs (`test_injection.py`, `test_injection2.py`) are
the sharp case: they decide whether the model refused by looking for markers in the reply, and
reasoning about an attempted injection is exactly the text most likely to carry those markers.
A refusal could be credited to a model that complied, or denied to one that refused.

WHY A MODULE OF ITS OWN, rather than exporting it from `context_ingress`. Importing
`context_ingress` costs its import-time configuration (the model pin, the account base). The
verify suite's `common.py` is imported by thirteen proofs including an offline one that CI runs,
and `proof_checks.py` runs inside a disposable stack. A reader with no import weight can be
shared by all of them; `pins.py` next door exists for the same reason.

WHY THE SEAM OWNS IT. The product defines what a client receives. A proof asserting on anything
else is asserting about a system nobody runs. `multi/` importing `deploy/` is forbidden and this
direction is the permitted one: operator tooling reads product modules.
"""

# ── the User-Agent every /v1/responses call carries ──────────────────────────────────
# THE SAME DEFECT AS THE READER, one line instead of a walk. It existed as a bare "Mozilla/5.0"
# in `multi/verify/common.py`, as the full string below in `context_ingress`, and as a third
# variant in `multi/provision/confine-member.sh`. `common.py` guarded its copy with a comment —
# "the browser User-Agent is LOAD-BEARING, not decoration: the instance shapes some responses by
# it, so a proof that sends a different one is measuring a different system" — and then set it to
# the one string the product does not send. By its own argument every proof was measuring a
# different system. A comment cannot hold two constants together; one definition can, which is
# why the value moved here rather than being duplicated more carefully.
#
# AND THE CLAIM WAS MEASURED rather than inherited. Against the pinned rev on a live MT instance
# (2026-08-26): three agents — the default `Python-urllib/3.x`, bare "Mozilla/5.0", and the
# string below — over `POST /v1/responses` and `GET /v1/responses/{id}`, authenticated as a
# sealed member and unauthenticated.
#
#     access        : identical (200 / 200 / 200 with a token; 401 / 401 / 401 without)
#     turn          : identical (`completed`, same answer to the same prompt)
#     unknown id    : identical (404)
#
# So the instance does NOT shape responses by this header, and that justification is gone. What
# the header is actually for is the EDGE, which is what `context_ingress` said all along: a
# hosted IronClaw may sit behind Cloudflare bot-protection that 1010-blocks the default
# python-urllib agent, and `common.post(api=...)` can target one. Edge-load-bearing, not
# instance-load-bearing. Sending exactly what production sends costs nothing and leaves no way
# for a proof and the product to be told apart by a request header.
BROWSER_UA = ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
              "(KHTML, like Gecko) Chrome/125.0 Safari/537.36")


# Item and content types that carry client-visible assistant text. The filter is the whole
# point: everything NOT named here — reasoning, tool calls, annotations — is model-internal and
# must never reach a client or an assertion about one.
_MESSAGE_ITEM = "message"
_TEXT_CONTENT = ("output_text", "text")


def output_text(doc):
    """The assistant's client-visible text from a response document, concatenated.

    Never raises: a malformed or empty document yields "". Callers that treat empty as an error
    (`telegram_bridge._Turns.fetch`) decide that for themselves, because "the model said nothing"
    and "this document is not a response" are the same string but not the same event.
    """
    text = []
    for item in (doc or {}).get("output") or []:
        if not isinstance(item, dict) or item.get("type") != _MESSAGE_ITEM:
            continue
        for content in item.get("content") or []:
            if isinstance(content, dict) and content.get("type") in _TEXT_CONTENT:
                text.append(content.get("text") or "")
    return "\n".join(text).strip()


# NO top-level `output_text` FALLBACK, deliberately. The old `verify/common.text_of` had one and
# the product's `_output_text` did not, which was the second of the three disagreements. An
# earlier draft of this module kept it, on the reasoning that it fires only when the structured
# walk found nothing and so cannot leak an internal item.
#
# That reasoning was about robustness, and it is a CHANGE TO WHAT A CLIENT RECEIVES: a document
# with no message item yields "", and `telegram_bridge._Turns.fetch` reports that as a failed
# fetch. This module therefore matches the product extractor it replaced, exactly — which is what
# `multi/verify/test_output_text_visibility.py` asserts, over a corpus, against the proof readers.
#
# AND THE SHAPE IS MEASURED, not assumed. Against the pinned rev + MODEL_PIN on a live MT
# instance (2026-08-26), across four responses — three creates including one prompt written to
# invite reasoning, plus a GET of a stored response, which is the path `_Turns.fetch` takes:
#
#     top-level `output_text` present : never (0/4)
#     a `message` item present        : always (4/4), content type `output_text`
#     create text == fetched text     : yes
#
# So the fallback answered a shape this runtime does not emit, on either path. It is not a
# robustness measure being traded away for purity — it is dead defensive code for a case that
# does not occur here. If a future rev starts flattening, the bridge will say so loudly (a failed
# fetch, not a silent empty delivery), and THAT is when to reopen this with new measurements.
