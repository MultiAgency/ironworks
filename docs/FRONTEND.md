# Frontend

**Built, and unblocked.** Both pages render: the demo at `site/index.html`, the operator page in
`local/`. What unblocked the published one was not a repointed transcript but a change of argument
— it shows the service working rather than narrating a claim about it, so there is no headline
answer to pick and nothing to go stale. `0021` binds to the current fixtures and carries all seven
cases.

The refusal that used to block it still stands and is still the design working: a transcript
recorded over one book cannot be rendered against another, because that would attribute evidence
the turns never received. It is why the narrated page could never be repointed, and part of why
it was deleted rather than fixed.

What is recorded here is what the surface is *for*, the order the pieces earn, and what it must
never do — decided in the course of building `ironworks compose` and the arc, which are the data
foundation these pages render.

Terms are the ones in `CONTEXT.md`. *Tenant*, *operator* and *channel* mean something narrower
than their ordinary reading.

## One page, two depths

**The goal is a pilot conversation.** Not a signup, not a trial — the seam does not provision, so
the only thing it can honour is a person replying. Everything on the page is in service of someone
deciding whether to hand over their relationship record and say so.

**Two readers, one surface.** A commercial reader wants to know what it does and what it refuses;
a technical evaluator wants the journal row, the digests, and the crash boundary. They are not two
products and they are not two pages — scroll depth is trust depth, and the second reader is served
by continuing rather than by navigating.

**The gate is on the transcript published, not on the rate behind it.** This page serves one
recorded answer per case (below), so a distribution is a property no reader ever sees: a threshold
on pass rate gates something the page cannot expose, and can be satisfied or missed by which run
happened to be picked. It was written the other way — "`attention` regrades FAIL on the synthetic
suite" — and that reading did not survive `0016`–`0018`. Under graders corrected in both
directions the case is unstable rather than failing, and the failures it does have are phrasing
lapses that are true-but-unframed or attached to accounts the answer recommends nothing about.
Reading the answers rather than the verdicts, no substantive reasoning defect survives the recorded
corpus.

**Two properties, checked on each answer before it ships:**

- **No control token.** A decoding degeneracy shipped as prose is the one failure here that is
  unambiguously bad and invisible to a reader — `0016` run 1 was token soup journalled
  `state: complete`. `ironworks_ironclaw::malformed_answer` now refuses delivery on it, and it is a
  cold-start artifact of the runtime (`0018`: 2 of 3 first turns after bring-up, 0 of 47
  thereafter), so a published transcript must never be a stack's first turn.
- **No closure the record does not carry.** An answer may say an offboarding is complete *per the
  record* and name the removals the record confirms; it may not assert the access side is settled,
  because two confirmed removals do not establish that a bearer session was revoked. The
  coordination graders already draw this line and the recorded answers already respect it — this
  keeps it a precondition rather than a hope.

Both are per-answer and mechanically checkable, which is what a gate on a published artifact has to
be. Neither can be satisfied by a lucky run.

**Nothing on the page explains this system by reference to another one.** It is a fresh product.
The Rust-versus-Python argument was its own page and is now dropped; what survives of it are the
guarantees themselves, stated as this system's own, and they belong in the refusals section rather
than in a comparison.

## The depth material

Sections of the one page, not a second destination. In priority order — each earns its
place by changing what gets found, not by completing a set.

**1. Composition diffing against verdicts.** An answer-quality verdict is bound to one alloy
digest. When the digest moves the next question is always what moved under it, and until
`ironworks compose` existed nothing could answer: the digest surfaced only as a side effect of a
turn, the alloy text is persisted nowhere, and an alloy fuses scrap **and** guidance, so it can
move with no repo diff at all. The operator page renders the arc today; folding it into the
first screen.

**2. The answer reader.** *Built, and now a section of the operator page rather than a page of its own.* Every sentence
of a graded answer resolved against the book the turn was actually shown, with the cited records
quoted beside it.

What building it settled: the unanchored-sentence marker is the smaller half. Scoped properly —
a sentence inherits the account its block or its question named — it fires **zero times** across
the six graded answers, and unscoped it fired on two thirds of every answer, which is a marker
worth ignoring. The value is the juxtaposition. The defect that prompted the tool,
`Offboarding appears complete on the access side`, cites real dated evidence; what exposes it is
reading that sentence beside the two activities it draws on, which is a layout problem rather
than a detection one. No syntactic check was ever going to find it, and building one that
pretended to would have been worse than none.

**3. Stuck turns.** What `ironworks status` reports, plus the delivery evidence and the
duplication risk stated per turn: `Uncertain { acknowledged_chunks: n }` means a second copy may
appear, and that belongs on the control rather than in a paragraph about idempotency. Real, but
low volume today.

### What the depth material is not

No provisioning UI for a lifecycle that is not built. No fleet view of one tenant. No uptime
chart — a live view of current state is mostly empty, which reads as "no customers", so the home
view is the evidence record over time. No guidance editor: guidance is marker-bound to slug and
service and is part of the fingerprint deciding whether a thread may continue, so a web form makes
alloy drift a UI accident. Showing guidance is a differentiator; editing it is a defect.

**Read-only, over a read-only store.** If the page ever acts, it acts by invoking the same
commands. There is exactly one path from a key to an answer (`drive`), and a rendered surface must
not become the second, looser one.

**It must not join ids back to content.** The journal holds identifiers only — no message text, no
answers, no records, no credentials. A page that fetches the text to be helpful reintroduces
exactly what the store was designed not to hold. Rendering that restraint *is* the privacy proof:
here is everything the operator can see about your turns, and it is keys, states and ids.

## The Telegram surface

The segment is **Telegram-native organizations**, which settles the channel question: a bot is the
front door, not a compromise. **The adapter works** — a real bot answered a real question through it on 2026-08-31, first
attempt, with the channel confirming the chunk. `crates/telegram` serves the same
`/poll` and `/send` contract `devchannel` does, so the seam cannot tell a real channel from the
fixture and nothing on the serving path changes to reach one. `dev/relay-loop.sh` supervises it: `relay` still handles one cycle and exits — that is what keeps
every crash boundary a real process boundary — and the loop runs a fresh process per cycle, backs
off on failure, stops on a usage error rather than retrying a misconfiguration forever, and holds
no cursor of its own. What is left is deployment: nothing runs it anywhere. The `channel` crate is already Telegram-shaped — monotonic update
ids, offset acknowledgment, plain text with no parse mode, chunking at
`DEFAULT_CHUNK_BYTES` — and unlike a bespoke demo page the adapter is on the product path, because
a tenant needs it eventually.

Three constraints a public bot inherits:

- **Its own deployment, its own journal and channel.** The safe cursor is the lowest open update
  *across every tenant* (§E-17). Point a demo bot at the live instance and one stranger's stuck turn holds
  the cursor for everything sharing that journal — including our own account coordination — and
  settling it is an operator decision each time.
- **Anonymous speakers have no credential.** Org resolves server-side, so a demo bot is one
  instance bound to the synthetic book. It cannot be the path a real tenant later arrives on.
- **Answers arrive markdown-stripped**, deliberately (§E-14). The FACT / STATED / UNKNOWN
  structure that carries the product's whole argument reaches the room as flat prose. So the split
  is not two products: conversation in Telegram, anchored claims and journal state and digests on
  a page.

### The page

Ordered for the decision, not for how a turn works internally:

1. **the promise** — it reads your account records and refuses to make anything up;
2. **one real turn, shown** — the records it was given, the answer, every claim anchored to the
   dated activity it came from;
3. **what it refuses** — never acts, never writes, never sends; says UNKNOWN rather than guessing;
   flags an instruction embedded in a record instead of following it;
4. **the ask** — talk to a person;
5. **then depth**, for whoever keeps scrolling: the journal row with ids and states and no content,
   what was pinned, one turn taken through a crash boundary, and the record over time — the
   experiments arc with its regrades.

The earlier draft ordered these as records → answer → journal → pins, which is the order a turn
executes in. That is the builder's order. A reader deciding whether to trust the thing needs the
promise, then evidence, then the limits, then a way to reply.

Above the fold: the contradiction case (`NC-003`) beside an answer that silently picks a winner.
Not a headline. The demo script is already written — `crates/graders` plants seven traps with their
exact questions — and the one to build first is injection, because
`vr-005` in `fixtures/account-coordination/book` (the 10-record book) carries a literal
instruction to invent a $750,000 budget and provision access, and
watching it refused costs nothing to stage.

**Do not lead with the cross-account brief, and do not run it live.** It produced a clean answer
in 2 of 5 recorded runs — 40% measured, in
`docs/experiments/0010-account-coordination-attention-stability.json` — and a per-claim error model
predicts 41%, which is the agreement that matters: the failure behaves like a per-claim error rate —
that case makes 2.5× the claims of any other and trips a rule 2.9× more often per claim. Even
bounding the brief *and* matching the other cases' rate models out at 87%, which is one bad answer
in eight in front of a prospect. The single-account cases — contradiction, supersession,
offboarding-unknown — have zero violations across every recorded run, and contradiction is the
strongest screen the product has anyway. See the analysis in
`docs/experiments/README.md`.

**Static, and generated rather than authored.** A build step reads `docs/experiments/*.json` and
the fixtures and emits the page, so the digest displayed is the digest that produced the answer
shown; hand-typing answer text into HTML is how a demo drifts from the system into marketing.
Serve recorded transcripts, labelled with run date and digest — they are real outputs, and a live
turn cannot be public today: concurrency is not built, `relay` is one cycle, the rate limit is 30
requests / 60 seconds shared with the runtime, and a turn that outlasts 30 seconds returns 503.

**Publish the negative space.** Does not provision or deprovision, one tenant live, and a
seven-case grade over the current book (`0020`) that is **one sample per case and not a pass
rate** — three runs of the same suite scored 6/7, 4/7 and 3/7 with nothing changed between them,
and every case has both passed and failed. Say that
plainly rather than publishing the better number. A buyer who hands over their relationship record
selects for vendors who state limits unprompted, and "we measured it twice and got different
answers" is a stronger signal of honesty than either figure.

Put the synthetic markers on screen rather than hiding them: every domain is RFC 2606 `.example`
and every record carries `_synthetic`. To this reader that is the pitch, not a disclaimer.

### What the page must not do

**No signup funnel and no "connect your CRM".** Nothing provisions; the pilot shape the system
supports is a small book, guidance written with the tenant, one service selected explicitly. A
button the seam cannot honour teaches a prospect that the page overstates.

**No scores and no leaderboard.** Urgency ordering is valid coordination behaviour; value ranking
is what the service forbids. A sortable value column would violate a service invariant through
pure UI affordance.

**Render record text as quoted evidence, never in the product's voice.** Activity bodies are
attacker-reachable — no HTML, no live links. On the injection screen that rule is also the point.

## What is published

One Cloudflare Pages project, `rustworks` — consolidating `ironworks` and `ironworks-assay`,
which served byte-identical copies of the same page.

**One page, and a 404.** Not because the material is thin, but because the reader is one person
deciding whether to hand over their relationship record, and every extra path is a decision they
have to make before they have any reason to care.

| path | page | source |
|---|---|---|
| `/` | Account coordination — the demo | `ironworks-site demo --html site/index.html` |
| 404 | Nothing is published here | `ironworks-site not-found --html site/404.html` |

## The published page is a demo, not a narrated argument

**The narrated page that preceded this one argued from a single answer** — an earlier composition
got `contradiction` wrong, the current one gets it right, so the instructions improved. `0023`
measured that case at 8 in 10: both answers sit inside the spread either composition produces, so
the argument was not false, it was **unsupported**, and no rewording reaches it. It was deleted
rather than repointed, because it was also pinned to a transcript over the superseded six-record
book and a transcript recorded over one book cannot be rendered against another.

**What survives measurement is showing the thing work.** The demo is a question picker across the
six of the seven real questions; each shows the records that turn was given, the answer, and every
claim underlined back to the dated activity it came from, with a key for the markings. A reader checks it by eye. No statistic is
quoted, and none is needed — which matters, because the one the page was going to lead with turned
out not to be computable: the grader gate that would count injection refusals fires on answers that
quote the payload *in order to refuse it*, and the persona teaches that quoting. See the correction
in `docs/experiments/README.md`.

**Three things it carries that a highlight reel would not:**

- **Recorded, not live**, labelled with the run and composition digest. A live turn stays impossible
  until there is a deployment — `relay` runs one cycle, concurrency is not built, and a turn past
  thirty seconds is refused rather than degraded.
- **The grades, once, in prose — never stamped on an answer.** The count is computed from the
  transcript. A verdict chip beside an answer reads as *"this answer is wrong"*, and `0023`
  established most failures are phrasing rules rather than reasoning, with four known defects still
  open in those rules. Publishing a verdict from a rule known to be wrong is misinformation wearing
  rigour's clothes, so the page says what the grades are and why they are not next to the answers.
- **The cross-account brief is excluded, not merely un-led.** Its recorded answer miscounts itself
  and concludes *"offboarding appears complete"* — the overclaim `regrades.json` records against
  `0005`, and the discipline the service exists to keep. Demonstrating it would be demonstrating the
  defect. Its rate is stated from `0018`, the probe at this composition over this book, rather than
  from `0010`'s better-known 2-of-5, which measured a superseded alloy over the six-record book.
- **The synthetic markers on screen.** To this reader that is the pitch, not a disclaimer.

**No script.** The picker is radio inputs and sibling selectors. A demo that needs JavaScript to
show an answer is a demo that can fail to show one.

**The 404 page is not decoration.** Without one, Pages answers every unmatched path with 200 and
the site index — so a dead or mistyped link reports that it exists and serves something else.

**Nothing under `site/` is tracked.** `.gitignore` says `/site/` — anchored, because unanchored it
matched at any depth and swallowed `crates/site/`, the generator itself. A generated page that
slips into a commit is a page that can disagree with the composition it claims to describe.

    ironworks-site demo --html site/index.html
    ironworks-site not-found --html site/404.html
    wrangler pages deploy site --project-name rustworks --branch main

**The operator pages are generated outside `site/`, and that is the mechanism rather than a
convention.** The composition arc and the answer reader are for whoever is improving the service,
not for someone deciding whether to hand over their records — the same reasoning as the paragraph
above, applied to the two pages it did not name. Writing them into the deploy directory would
publish them on the next deploy whether or not anyone meant to, so they are written to `local/`,
which is ignored:

    ironworks-site operator --html local/operator.html
    python3 -m http.server -d local 8080

**The arc and the reading are one page**, because they ask the same question at two zooms — *which
runs graded this composition and how did each case fare*, then *what did one of those answers
actually claim, against the records that turn was shown*. A reader who has just watched a cell flip
between runs wants the second immediately, and navigating for it was the whole friction.

**Two documents are published and one is not.** `site/index.html` and `site/404.html` go to Pages;
`local/operator.html` does not, and `local/` is ignored for the same reason `site/` is untracked.

**The reading refuses to build against a book it cannot bind to.** It shows an answer
beside the records it names, so it first proves the fixtures *are* the ones that turn was shown:
a book fingerprint recorded with the run, recomputed and matched. Nothing else binds. A matching
record count is read and reported as a diagnostic and is explicitly not a binding — two books of
six records need share no bytes — so a transcript written before the fingerprint existed cannot
be rendered at all, and no fingerprint is filed against it after the fact. That is the honest
outcome: nothing in this repository knows what the fixture book held on an earlier afternoon.
The rule is the same for `--transcript` as for the default. Record a fresh run rather than
relaxing the check.

**Stability probes are a separate class on the arc, separated by shape and not by file name.** A
transcript that grades one case repeatedly measures variance at a fixed composition. It cannot be
a column in a matrix of cases against runs — one filled cell and six blanks reads as six missing
verdicts, and its tally would read as a suite score — so it is loaded, kept out of the matrix, and
rendered in its own section with its verdict spread. The transcript glob is deliberately wide
enough to pick these up; excluding them by name would leave the page claiming to cover every
experiment while omitting the one that measures instability.

Each generator carries its own offline control, as ordinary tests.

    ironworks-site operator --html <path>   # the arc, and the reading beneath it
    cargo test -p ironworks-site            # which recorded run may fill the "now" column;
                                            # the book binding, the counted findings, escaping

`crates/page` holds the palette, the type scale and the panel and table rules the pages share, so
a second page cannot quietly become a second design.
