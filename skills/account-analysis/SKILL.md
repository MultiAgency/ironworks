---
name: account-analysis
version: 0.1.0
description: Organization-generic account qualification and discovery — collect and classify evidence, assess an account against the ORGANIZATION GUIDANCE's own criteria, rank what's unknown by decision value, and ask the single highest-value discovery question. Use whenever the team asks to qualify, assess, research, or run discovery on an account.
activation:
  keywords:
    - qualify
    - qualification
    - discovery
    - assess account
    - research account
    - account intelligence
---

# Account Analysis & Discovery

Follow this loop whenever the team asks you to qualify or discover an account.

## 1. Collect evidence
Assess against the company's own guidance, then read the account's evidence.
- **Company guidance** — the **ORGANIZATION GUIDANCE section of your instructions**: the
  offer, target customer, qualification and disqualification criteria, account stages,
  and desired decisions. Assess against *those* criteria — the organization's bar, not a
  generic framework.
- **The account** — use the account context supplied to you: firmographics, contacts,
  prior interactions/activities, and the fields explicitly marked missing. Treat
  provenance (`source`, record id, `retrieved_at`) as part of the evidence. If the
  team names an account you have no context for, say so plainly rather than guessing.
  If the records status says the book is **empty**, do not run this loop at all — follow
  your empty-book instructions instead (say so, and help the team decide what to load).

## 2. Classify every piece of evidence
Tag every claim per the evidence discipline in your instructions (FACT · STATED · INFERENCE ·
HYPOTHESIS · UNKNOWN). If two sources disagree, record it as **conflicting** — do not
silently choose one.

## 3. Assess suitability against the organization's guidance
Work through the qualification criteria the organization's guidance defines. For each one, give a
short labeled block — **plain lines, no markdown tables** (the chat renders them as raw
pipes):

```
<criterion> — Strong | Partial | Weak | Unknown
  evidence: <tagged, with sources>
  uncertainty: <what would change this>
  implication: <what this means for pursuing the account>
```

Check the guidance's **disqualification criteria** separately and state each one you can
evaluate as MET or NOT MET. A met disqualifier is a hard stop — say so outright; never
soften it into a merely "Weak" criterion.

Do not invent a numeric score. Use the criteria the guidance actually defines — do not
import BANT, MEDDIC, MEDDPICC, or any named framework it does not name.

## 4. Identify material uncertainty and rank the gaps
List the unknowns that could change the next decision. Rank each by **decision value** —
how much the answer would move whether/how to pursue. For each: *Unknown → why it matters
→ which decision it affects → priority.*

## 5. Ask the single highest-value question
Formulate one specific, decision-relevant discovery question for the top gap. Never ask
something the evidence or the team has already answered.

## 6. Incorporate the answer and reassess
When the team answers, treat it as new evidence tagged **STATED** — cite who said it and
when. A team assertion is not a FACT: it becomes one only when a record confirms it. Fold
it into the assessment and **revise the affected criteria and the gap ranking**. Do not
re-ask an answered question. Then either ask the next-highest question or, if the picture
is clear enough, stop and recommend the next step. Where a STATED item is load-bearing for
the decision, note that it is worth capturing into the records.

## 7. Recommend the next step
State what the team should do next, using **only the decisions and stages the ORGANIZATION
GUIDANCE defines** (e.g. advance to the next stage it names / continue discovery /
deprioritize). If the guidance maps needs to offerings, point at the fit from *its* list;
say "unclear — discovery target" when the fit isn't yet evidenced. Never recommend a
vendor, service, or process the guidance does not name.

## Output shape (team-facing)
Lead with the conclusion. Plain chat text: no markdown at all — no tables, no `#` headings,
no `**bold**`. This chat renders none of it, so asterisks arrive as asterisks. Use short
labeled lines; the labels below are literally how they should appear.

Structure: Account · Suitability (the blocks above) · Evidence (Known / Stated / Inferred /
Conflicting) · Top unknowns (ranked, with why), then close with:

    FIT: the guidance-defined offering or stage that applies, or "unclear — discovery target"
    WHY: the evidence, tagged
    KEY UNKNOWN: the one that most changes the decision
    NEXT BEST QUESTION: one
    RECOMMENDED NEXT STEP: one of the guidance-defined decisions

Keep it tight and readable — a briefing, not a data dump. Show sources for facts; don't
expose retrieval mechanics.
