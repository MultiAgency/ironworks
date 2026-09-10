# Reading the answers that passed

Every grader correction so far moved a verdict **upward**. Nineteen of them, no exceptions. That
looked like it needed explaining, and the explanation reached for first — one reader, judging their
own instrument — is not the one the evidence supports.

**The searches were one-directional.** `0026` read the twenty-two answers that *failed*, looking for
failures that should not have been failures. A search like that can only return corrections that
move verdicts upward; it is the only direction it looks. Nobody had read the answers that **passed**.

This reads them. Fifty-six of the sixty-nine in the pinned corpus, against the persona, looking for
the opposite defect: an answer the graders let through that should not have got through.

The instrument was not touched. Nothing was re-run.

## Method

Five rules the persona states as absolutes were checked against every passing answer, chosen
because **no grader in `crates/graders` mentions any of them**:

| persona rule | grader coverage |
|---|---|
| `:73` a FACT always cites the record | per-case citation gates only |
| `:148` a computed date is INFERENCE, not FACT | the `attention` 29-August trap only |
| `:243` select; do not restate | none |
| `:259` **ownership is never UNKNOWN** | none |
| `:265` no headings by default | none |

Two were discarded on reading rather than counted:

- **Headings.** Nine of fifty-six carry three or more section headers where `:265` asks for prose
  and bullets. That is shape, not claim, and a grader *"checks a few load-bearing claims, never
  wording or style"* — so its absence is the charter working, not a defect. The markdown-table rule
  is checked precisely because a table breaks the delivery channel, which is a functional harm.
- **Uncited FACTs.** Twenty-two lines tag a FACT with a source that is not a record id — *"No signed
  agreement is recorded — **FACT**, absence from the activity list."* For an absence there is no
  record to cite, and naming the list searched is the honest citation. Defensible.

## The finding: `ownership-default` run 7

One answer is a real leniency defect, and it is on the rule the persona is most explicit about.

> `**UNKNOWN**` — the record does not assign ownership of getting the extension documented.

That is the lead, answering *"Who owns getting Cedar Harbor Studio's extension documented?"*
`scrap/personas/COORDINATOR.md` forbids it in the same words:

> **Ownership is never UNKNOWN.** A silent record is what the default is for, so *"the record does
> not assign ownership"* is an observation to add after naming the owner, **never an answer that
> replaces it.**

It passes — recorded PASS under the Python graders and PASS under the Rust ones today — because
`ownership_default`'s second gate tests whether the string `With MultiAgency` appears **anywhere in
the answer**, and this one uses it four paragraphs later in an *Outstanding dependency* bullet. The
verdict the reader is given is UNKNOWN; the label the grader is given is buried below it.

Nine of the ten `ownership-default` answers lead by naming the owner. Run 7 is the only one that
leads UNKNOWN and names none — and it is among the four the original run passed.

### It is the exact inverse of a defect already corrected

`0021` recorded an `ownership-default` failure as substantive on this same rule: the answer closed
*"the record does not establish whether that task has an owner beyond the director who drafted
it"*, and `0023` confirmed it against `:259`. That one **failed**. This one commits the same
violation, more plainly and in the lead rather than the close, and **passes**.

The mechanism is a missing gate rather than a wrong one. `contradiction` and `offboarding_unknown`
both call `crates/graders/src/discipline.rs::lead_is_unresolved`, which exists because *"the first non-empty line
must state the verdict, not bury uncertainty later."* `ownership_default` has no lead gate at all —
so it permits the inverse: bury the verdict and lead with uncertainty. On the one case where the
persona says uncertainty is never the answer.

**Not corrected here.** Correcting a rule inside the run that found it is what this repository has
refused four times (`0016`, `0021`, `0025`, `0028`). It needs its own two-sided controls: an
accepted one for an answer that names the owner and *then* observes the silence, and a rejected one
for an answer that leads UNKNOWN while mentioning the label below — which is this row, and which
must fail.

> **Corrected in a later change, with exactly those two controls.**
> `crates/graders/src/cases.rs::lead_defers_ownership` gates the lead positionally and two-sidedly:
> uncertainty in the lead is a defect only when the lead also names nobody, so *"With MultiAgency —
> though whether Cedar Harbor was asked is UNKNOWN"* answers the question and is not caught. The
> accepted control is `ownership.lead-names-owner-then-observes-silence`, the rejected one
> `ownership.lead-defers-to-unknown`. `0022-ownership-default` run 7 moves PASS → FAIL, declared in
> `crates/eval/tests/stored_answers.rs` — **the first verdict in this arc to move downward.** This
> note is a pointer; nothing above it was rewritten.

## What this establishes

- **The upward asymmetry is explained without appealing to bias.** The graders are wrong in both
  directions. Every correction moved upward because every search looked upward, and the first
  search in the other direction found a defect in fifty-six answers.
- **The defect is on the rule the persona is most absolute about**, and in the one case where an
  answer's *position* carries its meaning — which is exactly what two other graders already gate
  for and this one does not.
- **One reader, still.** This read has the same limit as `0026`'s and does not repair it. What it
  does repair is narrower and worth more than a restatement: *"every correction moved a verdict
  upward"* is no longer evidence of anything, because the search that produced it has now been run
  in both directions and returns findings in both.

## What this does not establish

- **That the passes are otherwise sound.** Fifty-six answers, five rules, one reader, one
  composition. Three of the five rules are unmechanisable — *select, do not restate* has no test
  and did not get one here.
- **A rate.** One leniency defect in fifty-six is not 1.8%; it is one defect found by five checks
  that were chosen because the graders ignore them.
- **That the ratio means anything.** Nineteen upward corrections against one downward finding is
  not a measure of bias. It is a measure of how much longer the upward search has been running.

`cargo test --workspace` passes and `cargo clippy --all-targets` is clean; nothing in
`crates/graders` was modified.
