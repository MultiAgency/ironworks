# Tenant guidance

`guidance.template.md` is the model-visible template for
`relationship-intelligence`. Copy that file to the tenant's own gitignored guidance file; do not
copy this README into it. The separation is load-bearing: operator instructions do not belong in
the file whose remaining bytes are composed into a model prompt.

## Binding the copy

The template's first line is its binding marker. Replace `your-slug` with the slug passed to
`registry publish-one`; leave the service as `relationship-intelligence`. A mismatch is refused by
`crates/domain/src/composition.rs::Guidance` before publication.

If the tenant reads named keys from the record `facts` object, append a `facts:` field after the
service value on that same first line. Its value is a JSON array of key names. Omit the field unless
the record store actually carries those keys. Edit the marker already present in the template;
do not paste an example marker into a comment in the guidance file.

Fill every `TO BE WRITTEN` section with this organisation's own prose, or delete a section the
organisation genuinely does not need. An empty heading is worse than an absent one. `## Scope` is
shared method prose and may remain as written.

Check the copy before publishing; this needs no runtime or model key:

```console
cargo run -q -p ironworks-cli --bin ironworks -- compose \
  --slug your-slug --service relationship-intelligence --guidance GUIDANCE.md
```

The tenant's copy is business material and is synced separately to the serving host. Keep anything
that should not reach a model out of it.

## What belongs in this guidance

This service coordinates accounts across onboarding, delivery, renewal and offboarding. It derives
current state; it does not qualify a sales pipeline or advance a stored verdict. Do not add an offer,
qualification criteria, disqualification-from-pursuit rules, or a stage ladder. Those instructions
would contradict the coordinator persona composed beside the guidance.

Write only what the composition does not already carry. `scrap/personas/COORDINATOR.md` already
defines the evidence tiers, read-only boundary, general never-equate discipline, and refusal to
invent offers, prices, or commitments. Repeating those rules here creates two copies that can drift.
The guidance earns its place by carrying this organisation's vocabulary, lifecycle language,
priorities, known conflations, and prohibited claims.

Do not derive durable guidance from current live records. Guidance is recomposed every turn, so a
fact copied from today's book becomes stale prompt material when the book changes.

The coordination shape was settled on 2026-09-05. The earlier sales-shaped template and its
`## How to answer` section were removed because they either contradicted or duplicated the persona.
`fixtures/account-coordination/guidance.md` is the existing coordination-shaped example;
`fixtures/eval.guidance.md` is bound to a different slug and is not a model for this service.

## Why operator notes are separate

HTML comments do not nest. The parser closes a comment at its first closing delimiter. A served
guidance file once put operator notes in a comment and illustrated the binding marker inside those
notes; the illustration ended the comment early and made 1,270 bytes of operator instructions
model-visible. The incident and served measurements are recorded in `deploy/README.md`.

The legacy leaked-note shape and an unwritten template are invalid `Guidance`; the shared parser
therefore refuses them during compose, publication, rollback and serving. The template no longer
relies on a correctly maintained comment block for confidentiality either. Keep operator procedure
here and tenant model-visible material in the template.
