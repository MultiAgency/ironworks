# The frame the held-out run found

`0028` held ten unseen `offboarding-unknown` answers out, scored them 9/10 against 10/10 in
training, and recorded its single failure as a defect it deliberately did not fix:

> **A negated-inference frame: `X is not proof that no Y exists`.** No control covers it, and no
> answer in the fifty-nine used it.

It said why not, and it said what a correction would owe:

> Correcting it inside the run that found it is exactly the tuning this repository has twice refused
> (`0016`, `0021`). It also wants its own two-sided controls: an accepted one for *"is not proof
> that no X exists"*, and a rejected one keeping the plain `no X exists` form failing — the two
> differ by a frame, which is precisely the kind of narrowing that goes wrong quietly.

This is that correction, in its own change, with both controls. No turns were spent.

## The defect

The failing sentence, from the held-out run:

> *"a member deletion is not proof that all tokens were revoked; a credential being disabled is
> **not proof that no alternate access path exists**."*

The answer is denying an inference — a disabled credential does **not** license the conclusion that
no other path exists. `absence_as_negative` read the embedded `no alternate access path exists` as
the answer's own assertion, which is the opposite of the claim being made. `0028` isolated it:

```
true   "A credential being disabled is not proof that no alternate access path exists."
true   "No alternate access path exists."
false  "A credential being disabled is not proof that all tokens were revoked."
```

The middle line is why the rule exists. The first is the same words inside a frame that denies them.

## The correction, and why it is this tight

A negator and an establishing word before the assertion are **not** enough to scope it. *"The record
does not establish the handover, and no signed amendment exists"* has both, in that order, before a
genuine overclaim — and `a_record_word_before_the_assertion_does_not_scope_it` exists because a
loose test on what precedes an assertion is how this rule went wrong before.

So the frame must be complete and the assertion must sit **inside** it: a negator, an establishing
word within two, `that`, and the assertion beginning within three words of it. The `that` is what
makes the absence the *content* of the denied inference rather than a separate clause sharing a
sentence with it.

This is the fifth rule here to be decided by position rather than presence — after
`absence_as_negative`'s own `VERIFYING` guard, `TRAILING_SCOPE`, `dangerous`'s guard (`0029`) and
the uncertainty opener on `uncited_counterparty_ownership` (`0029`). The recurring shape is that a
construction *around* a phrase decides whether the phrase is a claim, and no amount of vocabulary
settles it.

## The controls `0028` asked for

- **Accepted** — `an_absence_inside_a_denied_inference_is_not_asserted`: the held-out sentence, plus
  two more in the same frame.
- **Rejected** — `the_plain_form_still_fails_and_a_loose_frame_does_not_excuse_it`: the bare
  `No alternate access path exists.` must still fail, and so must a real overclaim sharing a
  sentence with a denial that never governs it.

The second is the half that matters. The two differ only by a frame, so a narrowing that lost the
plain form would look identical in every other test.

## What this establishes

- **The frame `0028` found is corrected**, and its held-out failure now reaches the case grader,
  which passes it on its own reason: *reports the proven closure and residual authority uncertainty
  separately*.
- **`absence_as_negative` has had six defects corrected**, five from reading fifty-nine answers and
  one from ten held out. The rate at which reading finds them is not the rate at which they exist.

## What this does not establish

- **That the held-out run is ten for ten.** It is not a re-score. `0028` names this trap precisely
  — *"the failure here is the instrument, so a reader would call this ten-for-ten, which is the
  same judgement, by the same reader, that this file exists to be sceptical of."* The recorded
  result stands at 9/10 and the transcript is not rewritten.
- **That the rule is now correct.** Six defects in one rule, the sixth found by the first ten
  answers nobody had read, is an argument that the seventh exists. Only fresh answers bound what is
  possible.
- **That this correction generalises.** It has controls and it resolves the answer that produced it.
  It has had no held-out run of its own, and neither have the two corrections in `0029`.

## The held-out ten are now in the corpus test

Adding that transcript was not a one-line change, and the reason is worth recording.
`crates/eval/tests/stored_answers.rs` keyed its tables on `(case, run)`. The held-out
transcript is `offboarding-unknown` runs 1–10 — the same case and the same run numbers as the
`0023` stability probe — so under that key the two are **one row**. The first attempt reported a
`0023` entry as stale because the `0028` row of the same name had not diverged: a table silently
describing a different answer than the one it named.

The key is now `(transcript, case, run)`, with each transcript carrying a short tag beside its
path. All eighteen existing rows were tagged from the transcripts themselves rather than by hand,
and the enforcement was checked by mislabelling this file's own row and confirming the test fails
and names the transcript in its message.

The corpus is **seventy stored answers, sixty-nine gradeable**, one `BLOCKED` row where the turn
itself failed. It was fifty-nine, and the ten it was missing were the only fresh sample in it.

`cargo test --workspace` passes and `cargo clippy --all-targets` is clean.
