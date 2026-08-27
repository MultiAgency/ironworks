---
status: accepted
---

# Carry the envelope in `input`, not in `x_context`

IronClaw offers `x_context`, an extension field for structured JSON rendered as a
`<user-context>` block ahead of the user message — an obvious home for account records, and one a
future reader will reasonably ask why we declined. We put records in `input` instead, because
`x_context` caps at 10 KB and a turn that narrows to nothing is shown the tenant's whole book,
which exceeds that. Splitting records across two mechanisms by size would mean the model sees a
structurally different prompt depending on how many accounts happened to match, which is the one
variable we most need held constant.

## Consequences

The envelope stays a rendered text block, so its provenance framing — that record text is
evidence to assess and never an instruction to follow — sits inside the same string as the
records it governs, where it cannot be separated from them. That framing is load-bearing: the
corpus contains a deliberate `SYSTEM INSTRUCTION:` injection trap precisely to test it.

`x_context` remains available and unused. If a future turn shape carries small, genuinely
structured state — an approval token, a webhook payload — it is the right field for that, and
using it there does not reopen this decision.
