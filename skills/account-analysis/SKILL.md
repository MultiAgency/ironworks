---
name: account-analysis
version: 0.1.0
description: Organization-generic account qualification and discovery — collect and classify evidence, assess an account against the ORGANIZATION GUIDANCE's own criteria, separate the decisive gaps from the merely unknown, and ask the most decisive question. Use whenever the team asks to qualify, assess, research, or run discovery on an account.
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
Apply the evidence discipline your instructions define — every claim carries its tag. Where two
sources disagree, that pair is **conflicting**: name both, and carry the label through to the
Evidence section of the answer.

## 3. Assess suitability against the organization's guidance
Work through the qualification criteria the organization's guidance defines. For each one, give a
short labeled block in this shape:

```
<criterion> — Strong | Partial | Weak | Unknown
  evidence: <tagged, with sources>
  uncertainty: <what would change this>
  implication: <what this means for pursuing the account>
```

Check the guidance's **disqualification criteria** separately and state each one you can
evaluate as MET or NOT MET. A met disqualifier is a hard stop — say so outright; never
soften it into a merely "Weak" criterion.

Work in the guidance's own vocabulary, using the criteria it actually defines, and score only
on a scale it itself sets out.

## 4. Rank the gaps by what they would decide
A gap is **decisive** when learning it would change the next decision — a different stage, a
different offering, a different answer to whether to pursue at all. Everything else is merely
unknown, however interesting. Put the decisive ones first, each as: *Unknown → the decision it
would change → what the answer would have to be to change it.*

Done when every criterion you marked Unknown or Weak in step 3 has been placed: decisive, or set
aside with one clause saying why not.

## 5. Ask the most decisive question
Formulate one specific discovery question for the top gap. Ask only what the evidence and the
team have left open.

## 6. Incorporate the answer and reassess
When the team answers, treat it as new evidence tagged **STATED** — cite who said it and
when. A team assertion is not a FACT: it becomes one only when a record confirms it. Fold
it into the assessment and **revise the affected criteria and the gap ranking** — an answered
gap is no longer decisive, and something else now is. Then either ask the next question or, if
the picture is clear enough, stop and recommend the next step. Where a STATED item is
load-bearing for the decision, note that it is worth capturing into the records.

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

Steps 1, 2, 4 and 6 are how you *work*; only step 3's blocks and the closing labels are how you
*write*. An answer carrying a section per step is this skill leaking into the reply.

Structure: Account · Suitability (the blocks above) · Evidence (Known / Stated / Inferred /
Conflicting) · Top unknowns (ranked, with why), then close with:

    FIT: the guidance-defined offering or stage that applies, or "unclear — discovery target"
    WHY: the evidence, tagged
    KEY UNKNOWN: the most decisive one
    NEXT BEST QUESTION: one
    RECOMMENDED NEXT STEP: one of the guidance-defined decisions

Keep it tight and readable — a briefing, not a data dump. Show sources for facts; don't
expose retrieval mechanics.
