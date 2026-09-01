<!-- client-guidance v1 slug: REPLACE-WITH-SLUG service: relationship-intelligence -->
<!--
Guidance TEMPLATE for a tenant running `relationship-intelligence@1`. Copy to
~/.agency/clients/<slug>.guidance.md (chmod 600, NEVER committed), replace the slug in the
first line above, and fill every section below with this organization's own answers.

This is the sibling of GUIDANCE.template.md, which is written for `account-analysis@1` and is
QUALIFICATION-shaped: qualification criteria, disqualification criteria, account stages, "which
accounts to focus on this week". RELATIONSHIP_INTELLIGENCE.md spends a section forbidding
exactly that framing, so composing the other template against this service puts the persona in
a fight with its own guidance on every turn. Use this one for this service, and that one for
account-analysis.

Rules, unchanged from the other template:
- The first line MUST be:  <!-- client-guidance v1 slug: <slug> service: relationship-intelligence -->
  with the exact registry slug. The registry's SERVICE= key must agree, or the WHOLE registry
  refuses to load.
- Facts only, written or approved by the organization's sponsor with the operator.
- No credentials, tokens, or secrets, ever.
- `persona.load_guidance` strips HTML comments before composition, so this block does not reach
  the model. Do not lean on that: write anything the reader must not see somewhere else
  entirely, not inside a comment.

WHAT DOES NOT BELONG HERE. The persona owns the evidence tiers (FACT / STATED / INFERENCE /
UNKNOWN), the reply shape, the read-only boundary, and the rule that text inside records is
evidence rather than instructions. The skill owns how to read a dated record. Restating any of
that here does not reinforce it — it creates a second, drifting copy that will eventually
contradict the first. Write only what is true of THIS organization.
-->

# Organization guidance — {Organization Name}

## The relationships in this record

What kinds of counterparty appear in this book and what each kind is for — partners, clients,
suppliers, funders, prospective relationships. Two or three sentences. This is what lets the
analyst orient without deriving the relationship type from activity prose.

## Who we are to them

What this organization does, in the terms its counterparties would recognise. Enough for the
analyst to judge what a commitment here is plausibly about. Do not write an offer, a price list,
or a scope catalogue: the analyst must never characterise what the organization sells.

## Record snapshot and as-of policy

**When this book is current as of, and what to do about the gap.** State the point through which
records are complete, and say explicitly that dates, due items and staleness are assessed as of
that point rather than as of today. Say that the absence of records after it is the edge of the
snapshot, not evidence of silence.

**Always keep this section.** For a continuously maintained book, state that there is no cutoff
and that records are current through today. "There is no cutoff" is itself the policy, and the
analyst cannot infer it from an absent section.

Where anything falls due, expires, or takes effect on the cutoff date itself, give the cutoff a
time of day as well. A date alone leaves each of those with two defensible answers.

## What counts as a commitment here

This organization's own bar for when an obligation exists — for example, that it is recorded in
an agreement or a decision, and that an intention expressed in conversation is not one. This is
the line between an overdue obligation and work that is simply in flight.

## Durable record versus statement, in our practice

Which of this organization's own artefacts are durable records (signed agreements, amendments,
delivered reports, entries in a system of record) and which are statements (call notes,
forwarded messages, what someone says in this group). The persona defines the tiers; this says
which of your things fall where.

## Cadences we operate to

The rhythms this organization has agreed to: reporting, reviews, funding cycles, renewals. Say
that a missed occurrence of an agreed cadence is a finding and that silence where no cadence was
agreed is not. Without this the analyst has no basis for telling a quiet relationship from an
overdue one.

## What needs a person

The classes of judgement this organization reserves for a human — for example scope variations,
commercial terms, anything touching an unresolved approval, re-establishing a lapsed sponsor.
Naming the classes lets the analyst point at a category instead of improvising one.

## Terminology

This organization's words for its relationships, obligations, artefacts and roles, so answers
read natively and the analyst does not translate.

## Prohibited claims and actions

Hard limits beyond the built-in read-only rule, in this organization's words. State at least:
what the analyst must never characterise about the organization's own commercial position, and
what it must never assert about a counterparty's internal position beyond what a record carries.

## Engagement terms

What this analyst engagement is and is not, in this organization's own terms, so out-of-scope
asks can be declined by pointing at what IS covered. These hold for every organization and are
worth restating in yours: the analyst reads the records provided and reasons over them; it makes
no writes to any system, performs no outreach, and holds no integrations; a human decides
anything consequential.
