<!-- tenant-guidance v1 slug: your-slug service: relationship-intelligence -->
<!--
  OPERATOR NOTES — everything in an HTML comment is stripped before composition and never
  reaches a model. It also does not count toward the 400-character minimum, so a copy of this
  template padded with comments still refuses to parse. Write real prose below, not more comments.

  THIS IS THE TEMPLATE, not a served file. Copy it to the tenant's own guidance file, replace the
  marker's `slug` and `service`, and fill in the sections marked TO BE WRITTEN. A tenant's served
  guidance is their own business material and is gitignored — this template is the only versioned
  copy of the shape and the reasoning, so amend it here when either changes.

  The first line is the marker and is load-bearing. `slug` must equal the slug you publish
  (`registry publish-one <slug>`), and `service` must equal the recorded service. A mismatch is
  refused by `crates/domain/src/composition.rs::Guidance` before anything is written.

  An optional third field declares which record `facts` keys this tenant reads: append a
  `facts:` field to the marker on line 1, after the service, whose value is a JSON array of key
  names. Omit it unless the record store actually carries those keys.

  NEVER WRITE THE COMMENT-CLOSING SEQUENCE INSIDE THESE NOTES.
  `crates/domain/src/composition.rs::strip_html_comments` ends this block at the first one it
  finds, and everything after it is composed into the model prompt under the organization-guidance
  heading — the governing channel, not the data channel. The file this template was extracted from
  did exactly that until 2026-09-05: the third field was illustrated with a marker written out in
  full, which closed the block early and made the 1,270 bytes below it model-visible. `compose`
  prints the model-visible count now and warns on the class; measured on the copy that actually
  served it, 2,676 model-visible bytes against 1,401 once the block was closed correctly. That is
  why the paragraph above describes the `facts:` field rather than showing one. The same trap is
  documented for fact keys on `crates/domain/src/composition.rs::parse_fact_fields`, and the
  incident is recorded in `deploy/README.md`.

  SHAPE: COORDINATION, NOT QUALIFICATION — settled 2026-09-05, and reversible by the operator.

  This template used to ask for five sales-qualification sections. They were wrong for this
  service, and the evidence is one-sided. `services/relationship-intelligence.json` gives the
  responsibility as coordinating accounts across onboarding, delivery, renewal and offboarding,
  deriving current state rather than reading or advancing a stored verdict, with `writes`, `egress`
  and `outreach` all none. `scrap/personas/COORDINATOR.md` carries a section headed "You are not
  qualifying a sales pipeline" — those questions "belong to a sales composition, and this is not
  one" — says never to infer a pipeline stage from a legacy sales column, and names no offer
  deliberately, because an offer written into it once outlived the offer in a prompt recomposed
  every turn. Guidance is recomposed every turn too. The only guidance ever written for this
  service, `fixtures/account-coordination/guidance.md`, is coordination-shaped: no ladder, no
  offer. `fixtures/eval.guidance.md`, the five-section sales example, is bound to slug `eval` and
  is not this service's model to copy.

  So an offer paragraph, qualification criteria, disqualification-from-pursuit and a stage ladder
  are all struck. Writing them would put instructions into the governing channel that the persona
  composed beside them is explicitly told to refuse.

  WHAT YOU MUST WRITE. `## Scope` below is method: measured against
  `scrap/personas/COORDINATOR.md` and genuinely absent from it, so it is safe to copy as it stands.
  The five sections after it are this organisation's own material and nobody but the operator can
  supply them. Each is stubbed with what it is for; replace the stub with prose, or delete the
  section if this organisation genuinely has nothing to say under it. An empty heading is worse
  than an absent one.

  A `## How to answer` section shipped here until 2026-09-05 and was struck, because it was this
  template's own rule breaking itself. Two of its paragraphs restated the persona — the
  absence-is-not-a-negative rule and the legacy-sales-field exception — and a third **contradicted**
  it: it asked for a two-way evidence/inference split where the persona mandates four tiers, FACT,
  STATED, INFERENCE and UNKNOWN, and says "never invent a fifth". That is not duplication, it is a
  competing instruction in the same channel, and it was live in a served prompt. Measure any method
  prose against the persona before adding it back; there is no tenant of this service composed
  without one.

  WRITE ONLY WHAT THE COMPOSITION DOES NOT ALREADY CARRY. `scrap/personas/COORDINATOR.md` is
  composed into every turn beside the guidance, and it already holds the general never-equate
  discipline — a deleted member is not a revoked token, an invoice sent is not an invoice paid, a
  request to remove access is not access removed — along with the read-only boundary and the
  refusal to state what MultiAgency offers, charges or will commit to. Restating any of it here
  makes one rule into two that must be kept in agreement, and the two will drift. Guidance earns
  its place by carrying what the persona cannot know: this organisation's own words, its own
  conflations, its own prohibitions.

  DO NOT DERIVE ANY OF IT FROM THE LIVE RECORDS. Guidance is recomposed every turn, so anything
  read off today's records goes stale inside the prompt — the failure `scrap/personas/COORDINATOR.md`
  documents about the offer that outlived itself.

  The tenant's copy of this file is rsynced to the serving host and is the tenant's own material:
  keep anything you would not want in a model prompt out of it.

  Check the copy parses before you publish, which needs no runtime and no model key:

      cargo run -q -p ironworks-cli -- compose \
        --slug your-slug --service relationship-intelligence --guidance GUIDANCE.md
-->

# Client guidance

## Scope

Speak only about the accounts in the supplied records. Do not speculate about organisations,
contacts or commitments that do not appear in them, and do not carry an assumption from one
account to another because they look similar.

Recommend only actions this organisation can actually take. Where the next step depends on
information nobody has yet gathered, the recommendation is to gather it, stated as such.

## The lifecycle in your words

TO BE WRITTEN — what onboarding, delivery, renewal and offboarding are called here, and what marks
a move between them. Describe them; do not build a ladder the model is meant to advance.

## What to surface first

TO BE WRITTEN — which commitments and dependencies most deserve a person's attention, and what
"blocked" means for this organisation.

## What must never be equated

TO BE WRITTEN — this organisation's own versions of "a request is not a completion". Only the
pairs a reader here has actually conflated; the general ones are already in the persona.

## Vocabulary

TO BE WRITTEN — what this organisation calls an account, a piece of work, a person, and the
counterparty.

## Prohibited claims & actions

TO BE WRITTEN — claims this organisation in particular must never make.
