# The relationship book (synthetic)

The demonstration record for `relationship-intelligence@1`
(`multi/services/relationship-intelligence.json`): eight invented counterparties with dated
activities, plus the filled guidance that goes with them.

Everything here is **synthetic**. Every organization, person, date, agreement and incident is
invented; every domain is a reserved `.example` name. The book is a **snapshot taken at the start
of business on 2026-09-01**, and `GUIDANCE.demo.md` tells the analyst to assess dates, due items
and staleness as of that point. Without that policy the book changes meaning as wall-clock time
advances — a review scheduled for 2026-09-15 silently becomes overdue, and every quiet
relationship eventually reads as stale.

**The time of day is deliberate**, because three things land on the boundary itself. Larkspur's
operating report is due by close of business on the first day of each month, so at the start of
2026-09-01 the July and August reports are missed and September's is due today but not yet late.
Vireo's signed restoration is effective 2026-09-01, so at that same instant it is in effect. Read
the snapshot as a date alone and each of those three has two defensible answers.

## Where this sits

This is MultiAgency's **own side** of the table, in MultiAgency's voice, exactly like
`deploy/account-intel/data/candidates/` — which is why it lives beside it rather than under
`deploy/account-intel/fixtures/`. Two consequences:

- `deploy/account-intel/data/seed.py` globs `fixtures/accounts/*.json` only, so nothing here is
  picked up by `dev-up.sh`'s regression seed. It is seeded only when an explicit `REAL_DATA_DIR`
  names it, which is what provisioning does.
- **Never copy this book into `multi/verify/fixtures/clients/`.** Those are synthetic *external*
  proof clients, and `multi/verify/test_fixtures_offline.py` forbids MultiAgency-internal framing
  in them precisely because a book written in our voice, seeded for a client-shaped proof, once
  made a live proof fail about one run in three.

## What each file is

One JSON file per counterparty, in the candidate shape the seeder reads
(`deploy/account-intel/data/seed.py::upsert_account`, reached through `seed_real.py`):

```json
{
  "_synthetic": "provenance for a reader of this repository; not persisted, never model-visible",
  "record_id": "LARK-001",
  "account": { "name": "...", "domain": "...example", "industry": "...",
               "owner": "...", "facts": { "relationship_type": "..." } },
  "contacts":   [ { "name": "...", "title": "...", "engaged": true, "notes": "..." } ],
  "activities": [ { "activity_id": "LARK-A01", "occurred_at": "2026-02-10",
                    "kind": "agreement", "body": "..." } ]
}
```

Four things about that shape are worth knowing before editing a file:

1. **Keys inside `account{}` outside the seeder's column list are silently dropped.** Anything
   this book needs that is not a column goes in `facts{}`. Today that is one key,
   `relationship_type`.
2. **`contact_id` is generated** by the seeder as `<record_id>-C<n>`. Do not author one; an
   authored value is ignored.
3. **Re-seeding updates an activity's `body` only.** The upsert is
   `ON CONFLICT (org_id, activity_id) DO UPDATE SET body = EXCLUDED.body`, so correcting an
   `occurred_at` or a `kind` and re-seeding **does not change the stored row** — while the
   account's `updated_at` bumps and the seam dutifully re-injects the record as fresh. To change
   a date or a kind, change the `activity_id` with it, or delete the row.
4. **Nothing deletes.** Removing a file from the data directory leaves its rows in the store.

## Bring-up

The guidance must be in place **before** provisioning: `multi/provision/provision.sh` preflight
refuses without a validating, mode-600 guidance file at the canonical path, and it composes the
guidance against the service before it creates any authority.

1. Copy the eight book files to `~/.agency/account-data/<slug>/`.
2. Copy `GUIDANCE.demo.md` in this directory to `~/.agency/clients/<slug>.guidance.md`.
3. Edit its first line so the marker names the slug you are provisioning, keeping the service:
   `<!-- client-guidance v1 slug: <slug> service: relationship-intelligence -->`.
4. `chmod 600 ~/.agency/clients/<slug>.guidance.md`.
5. Run preflight, then provisioning:

   ```sh
   multi/provision/provision.sh <slug> "<Display Name>" <group-id> \
     --service relationship-intelligence --dry-run
   multi/provision/provision.sh <slug> "<Display Name>" <group-id> \
     --service relationship-intelligence
   ```

6. Ensure the resulting `~/.agency/clients/<slug>.env` carries **`FACT_FIELDS=`** — declared
   empty. This book has no gap shape, and the tri-state in `multi/clients/README.md` is easy to
   get wrong: an *absent* key falls back to the legacy sales-column gap list, so the analyst
   would be told on every turn that each relationship is missing `budget` and `economic_buyer`.
7. **Restart the bridge only for registry, guidance, service or compatibility-identity changes.**
   Re-seeding records is not one of those: re-run `deploy/account-intel/data/seed-real.sh <slug>`,
   the account's `updated_at` moves, and the seam re-injects it on the next turn.

## What guards this

- `deploy/account-intel/data/test_relationship_book.py` — the fixture shape, offline.
- `deploy/lib/test_relationship_envelope.py` — that this book's evidence reaches the real
  envelope, in date order, with `FACT_FIELDS=` behaving as declared.

Both establish what reaches the model. Neither establishes that the model reasons over it
correctly: `relationship-intelligence@1` declares `"evaluation": null` on purpose, and nothing
here changes that. The reasoning rules live in `agent/identity/RELATIONSHIP_INTELLIGENCE.md` and
`skills/relationship-record/SKILL.md`.

## Measured limits

Observations from running the book, not a score. They are recorded because the alternative is
that whoever demos this next rediscovers them live. **Re-measure on any `MODEL_PIN` change** — a
pin is a behavioural claim, and everything below is a statement about one.

**2026-09-01, `MODEL_PIN qwen/qwen3.5-397b-a17b`, `relationship-intelligence@1`.**

*LARK-001's recurring obligation is found only when asked for directly.* The book plants two
missed occurrences of a monthly reporting obligation (July and August; four earlier reports
delivered). Three probes, whole book supplied each time (`accounts_supplied 8` of 8):

| question | result |
|---|---|
| "What needs attention?" | not surfaced |
| "What do we owe Larkspur, and what are they waiting on us for?" | not surfaced |
| "Has every monthly operating report to Larkspur been delivered?" | surfaced, completely and correctly |

So the limit is **salience, not reasoning**: asked the specific question it enumerates every due
date, names both gaps, and gets the epistemics right without prompting — *"no delivery record
exists"*, and a follow-up to establish whether the reports were undelivered or merely
unrecorded. The failure is that it does not reach for that enumeration unprompted, including at
"what do we owe X?", which is one of the six questions this desk exists to answer.

Worth stating plainly: the second reply was not merely incomplete. By presenting September's
report as the next thing due, it implied the cadence was in good standing when it was two months
behind. An omission that shifts the reader's conclusion is worse than silence.

**Do not answer this by adding guidance.** The instruction is already present in both layers —
`GUIDANCE.demo.md` § Cadences we operate to ("a missed occurrence of an agreed cadence is a
finding and should be reported as one") and `SKILL.md` step 5 ("check each occurrence against the
calendar rather than checking whether it ever happened") — and neither fired. A fourth restatement is the shape
there is already evidence against.

*The close is rendered as a task list.* All three replies ended with action items carrying
`(owner: MultiAgency)`. `SKILL.md` step 7 forbids exactly this: "Never present this as a task
list, a workflow, or a set of next actions to be executed. You are naming judgement calls, not
assigning work." Three for three is a behaviour, not a slip.

*What did hold, in the same runs:* the same-day contradiction reported with both sides and no
winner picked; staleness measured against the record's own 23-day precedent rather than a
threshold; the closed engagement correctly left out of "needs attention"; the not-yet-due
acceptance review not called overdue; supersession read forward to the amendment effective on
the snapshot date; and both directions of the start-of-business boundary — "due today, not yet
late" and "effective today".
