# The experiments

Every measurement this repository has made, in the order it made them. Three kinds of file: a `.md`
that reads a run, the `.json` transcripts it reads, and the occasional `.log` a probe's console
left behind.

**A transcript is never rewritten.** It holds what the grader produced at the time, under the
instrument named in it. When a later reading changes a verdict, the change goes in
`regrades.json` — prose as data, so something can read it — and when a grader is *corrected* the
old transcript keeps its old verdict and `crates/eval/tests/stored_answers.rs` carries the
declared difference. That is why the numbers below are not all comparable to each other, and why
each one is quoted against the file that produced it.

Reading these in order is not required. Reading the one that measured a claim **is**, before
repeating the claim.

---

## How to read a run

- **A grade** scores the seven-case suite once. One sample per case.
- **A probe** scores one case many times at a fixed composition. It measures variance, never
  quality, and it is never a column in the arc's matrix — a one-case tally would read as a suite
  score (`crates/site` keeps them apart by shape).
- **A regrade** re-scores answers already recorded, under a different instrument. No new turns.
- **A grader correction has a write-up and no transcript.** It changed an instrument rather than
  measuring a composition, so what it produced is a declared verdict difference in
  `crates/eval/tests/stored_answers.rs`. No turns are spent on one. The arc section below names
  which runs these were.
- **BLOCKED** means the measurement did not happen. It is not a pass and not a fail.
- **REVIEW** means the grader could not tell. Read the answer; do not count it.

---

## The record

Most write-ups are pruned. They were readings of transcripts that still exist, written while the
work was in progress, and the project is early enough that carrying three thousand lines of them
costs more than it returns. What survives: one line per measurement here, **every** transcript, and
the write-ups that *are* the measurement because no transcript backs them. The control fails if a
numbered run is missing from the two tables below, so an omission here is a defect, not an
oversight.

### Kept in full — no transcript, so the write-up is the evidence

| | |
|---|---|
| [`0001`](0001-alloy-drift.md) | Alloy drift over a long chained thread. **~3,100 tokens/turn** — the first cost bound under chain-breaking. **Superseded as a current figure:** that rate was `account-analysis@1`'s 14,337-byte alloy; the shipped service measures ~5,700–5,910 (§I of `docs/INVARIANTS.md`). The measurement stands; what it is the bound *for* moved |
| [`0011`](0011-continuation-costs-more-than-it-saves.md) | Continuation defeats prompt caching, and that outweighs the supplied set |
| [`0012`](0012-what-confinement-reached.md) | What confinement reached, and what it did not |
| [`0013`](0013-the-pin-fails-silently-not-closed.md) | A runtime log named the provider's configured model, not the turn's. **The pin fails silently rather than closed** |
| [`0025`](0025-lead-vocabulary-was-a-list-not-a-rule.md) | **Lead vocabulary was a list, not a rule.** Four literal spellings failed three leads that said exactly what the rule asks; replaced by a predicate |
| [`0026`](0026-reading-every-failure.md) | **Seven of the twenty-two failures are the model**, fourteen the instrument and one ambiguous, out of fifty-nine recorded answers. The only systematic read of every recorded failure, and the basis of every model-versus-instrument claim here |
| [`0027`](0027-the-rule-that-fired-most-was-wrong-five-ways.md) | **The rule that fired most was wrong five ways.** Four defects in `absence_as_negative` plus a fifth found while fixing them; scoping moved from presence to position. Also `ownership_default`'s adjacency gate and `dangerous` reading verification advice as closure — using a guard that already existed one rule over |
| [`0029`](0029-a-review-was-standing-in-front-of-a-verdict.md) | **A REVIEW was standing in front of a decided verdict.** The prefilter returned on its undecidable rules, so three answers a case grader could fail were filed as three nothing could judge. The one correction in the arc that moved verdicts *downward* |
| [`0030`](0030-the-frame-the-held-out-run-found.md) | The negated-inference frame — *"X is not proof that no Y exists"* — read as an assertion. **Corrected**, with the two-sided controls `0028` specified |
| [`0031`](0031-the-external-tool-lane-is-open.md) | `/v1/responses` accepts `tools` and `tool_choice`; the host parks rather than executes them. **Sending none is a choice, not a constraint** |
| [`0032`](0032-a-parked-run-says-completed.md) | One tool round trip. **A parked run reports `completed`** — so `completed` stops meaning *answered* — and resolving it calls `builtin__result_read`, outside the catalogue confinement governs |
| [`0033`](0033-the-loop-ran-and-a-chained-turn-could-not-recover.md) | **The serving loop's first continuous run.** 2 of 3 answered, one recovered from a real 503 by byte-identical replay. **ADR 0005 recovery was structurally impossible for a chained turn**, and the unrecoverable turn held the cursor forever. Both repaired; `dev/prove-continuation.sh` reproduces the defect and now passes 5/5 |
| [`0034`](0034-reading-the-answers-that-passed.md) | **Reading the fifty-six answers that passed.** The upward-only asymmetry was a one-directional search. `ownership-default` led *"UNKNOWN — the record does not assign ownership"* — what `:259` forbids — and passed. **Corrected**, and it is the first correction in the arc to move a verdict *downward* |
| [`0035`](0035-the-tool-arm-read-the-records-out-of-memory.md) | A cost measurement overtaken by its own result. **Declaring `tools` hands the runtime an agent loop**; unconfined it read this tenant's records out of `ironclaw.memory.*` and never called the declared tool. **Confined it works and costs the baseline** — 20,910 vs 20,899 tokens. The cost objection to tools is dead; confinement is a precondition, measured |
| [`0036`](0036-the-demo-question-lands-on-the-thirty-second-cliff.md) | **The shipped service's flagship question sits on the 30s wait timeout.** 11 of 17 turns returned 503; every completing sample 26.5-30.0s, every refused one 30.06-30.65s. **The lever is the scope of the question, not the length of the answer** — narrowing it to one item cleared 9/10, while instructing three bullets, or one sentence, cleared 1/3 each. **No answer was lost**; recovery is 0.04-16.4s and automatic under `relay-loop.sh`. Also: an unconditional "run it again" mints a *new* key and bills a second turn |
| [`0037`](0037-what-the-seam-never-asked-for.md) | **The seam's six upstream path shapes against a surface an order of magnitude wider**, asking whether the omissions were decisions anyone took. Eight checks in one sitting; four recorded-open questions answered, and a disagreement about catalogue size resolved as two correct readings of two different callers. **Read `0040` and `0039` with it:** `0040` retracts its thread-titles finding, and `0039` repeats it on the served profile |
| [`0038`](0038-the-rule-that-never-produced-a-verdict.md) | **A rule that had never graded anything.** Found by measuring every rule's firing rate rather than by reading answers: `inconsistent_finding_count` returned from its own loop on the first non-number word, and under that its nouns named nothing this service counts. **Corrected; no verdict moved.** Widening it the obvious way was measured first and would have produced seventeen false failures |
| [`0039`](0039-the-catalogue-has-two-axes.md) | **The settings catalogue varies by caller *and* by booted profile**, so a bare row count is under-specified. Repeats `0037` on the served sandboxed profile — everything else reproduced unchanged — and moved `dev/prove-confinement.sh` to mint its own member rather than confine the WebUI bearer |
| [`0040`](0040-the-cost-field-misses-the-pin-twice.md) | **`usage.cost` misses the pinned model in both directions** — `"0"` on create, GPT-4o's prices on fetch a second later — from two unrelated causes. Also retracts one of `0037`'s findings about thread titles |
| [`0041`](0041-an-unrooted-exclude-removed-a-workspace-member.md) | **An unrooted `rsync` exclusion removed `crates/site`.** The destination manifest still named the missing workspace member, so Cargo refused to load it; rooting the generated-directory exclusions produced a complete build tree |
| [`0042`](0042-protecthome-hid-the-rootless-daemon.md) | **`ProtectHome=true` hid the rootless Docker socket under `/run/user`.** `read-only` preserved the unit's filesystem confinement while allowing its required per-account daemon transport |
| [`0043`](0043-user-creation-can-outlive-token-minting.md) | **User creation precedes its one-time bearer mint at the IronClaw pin.** A mint failure can leave a tokenless user, and the pin has no reissue endpoint; reconcile and delete the orphan before retrying |
| [`0044`](0044-a-refused-turn-recovers-on-the-next-serving-cycle.md) | **A 503'd serving cycle recovers on the next one — fifteen for fifteen**, across the operator bearer and a confined member, always two `create` calls under **one** idempotency key. So `create=1` is the wrong assertion for a proof that allows a recovery cycle; one *distinct key* is. The barrier holding the cursor at the unresolved update is what makes the second cycle possible, measured deterministically. **Recovery reached 86 s**, outside `0036`'s 0.04-16.4 s. Also: a bare `429` in `degraded()` matched inside hex digests and produced **false BLOCKEDs in 2 of 8 sittings** |
| [`0045`](0045-the-first-proof-of-the-serving-host-found-its-own-defect.md) | **The first live host proof converted seven operator reports into measurements and found its own expected member was truncated.** The host was right; the instrument invented a difference and scored 7 PASS / 1 FAIL / 0 BLOCKED. The direct-address run did not establish the `multi-serve` alias, and correcting the proof does not retroactively make it green |
| [`0046`](0046-one-failing-update-took-its-whole-batch-with-it.md) | **One unreachable record store stopped a whole relay cycle at its first update.** An audit of a code path rather than an answer: `?` inside the per-update loop. Measured with no runtime and no model key, so the failure is deterministic and free to reproduce |
| [`0047`](0047-the-guidance-warning-never-runs-on-the-path-that-changes-what-is-served.md) | **The guidance warning never runs on the path that changes what is served.** The first run of `docs/AUDIT-BRIEF.md`: all nine dimensions examined against source at `4cb47cfaf3`, none by measurement — so it establishes what the code refuses, never that a deployment refuses it, and finding 1 is where those two came apart |
| [`0048`](0048-thirteen-fail-open-defects-and-three-that-were-not.md) | **Thirteen fail-open defects, and three findings that did not survive verification.** A code-quality sweep of all 20 crates by three read-only agents, every finding re-read against source before it was acted on. The three withdrawals are the reason the record is worth keeping |
| [`0049`](0049-guidance-publication-became-the-serving-change-path.md) | **Guidance publication became the serving-change path.** Remediates `0047`: the registry now supplies service and canonical destination, both known hazards refuse publication, the replace is durable and reversible, and the host-proof scorer has clean and deliberately hazardous controls. No live-host measurement was taken |
| [`0050`](0050-blocked-was-being-read-as-fail.md) | **BLOCKED was being read as FAIL, and a test stub was wedging the repository.** `degraded()` in `dev/lib.sh` scored seven of eleven blocking `Cause` variants as product failures — `0044` in the other direction, on the same hand-written pattern — and is now machine-checked against the type. A single-shot test stub with no timeouts passed 12/12 serially and failed 5/12 in parallel, and wedged seven processes holding cargo's build lock, the oldest for over a day |
| [`0051`](0051-the-probe-and-the-served-host-resolve-authority-differently.md) | **The probe and the served host resolve authority through different constructors.** A source reading at the pin that measures nothing: `ironclaw serve` has four boot arms and only `production` reads `[policy]` from configuration, while the served profile hardcodes its deployment mode, runtime profile and org constraints — so §I's profile caveat and `0039`'s catalogue axis are two effects of one cause. Also: the org ceiling §L-6 records as absent exists in the kernel and nothing at the pin can set it (`UPSTREAM WORK`); the operator inspector exposes the composed prompt but retains four runs, so it is an instrument and never a record; and upstream ships the flagship service as a first-party skill |
| [`0052`](0052-the-inspector-flattens-the-prompt-it-shows-you.md) | **The operator inspector flattens the prompt it shows you.** Every `U+000A` becomes `U+0020`, one for one — so the captured `instructions` is the alloy's exact 26,850 bytes and a different digest, and byte identity is **BLOCKED** at this pin for any correct seam. What held instead: the melted length, and all three composed parts in order with the safety tail last and frontmatter stripped. **Its own finding 3 is WITHDRAWN the same day**: it 'corrected' `0051` finding 4 by reporting the run id reachable only from the event stream, and that was the harness searching for `run_id` against a type declaring `turn_run_id` — the timeline returns 200 with it populated, so `0051` stands and the cancel route is a patch. **Four instrument defects found by running it**: one control that could never fire, and one that produced a PASS and a false correction because a fallback absorbed the failure of the path it backed up. Re-run on the **served profile**, where all of it reproduced and the alloy hashed identically — but the prompt around it did not: 60 components and 54 capabilities against 58 and 53, the extra one `builtin.shell`, and **1,893 more estimated tokens per turn**. The first measured instance of `0051` finding 1 |
| [`0053`](0053-the-review-found-four-ways-a-green-suite-overstated-the-product.md) | **The review found four ways a green suite overstated the product.** Hazardous guidance is invalid at the shared parser; export retry revalidates current scope and the embedded digest contract; the host proof names the templated cell; and snapshot manifest v2 preserves restore destinations without an unverifiable local receipt. The adapter also bounds unauthenticated input before allocation. All free controls passed; operational backup and live-host gates remain unmeasured |
| [`0054`](0054-a-cutover-orphans-every-conversation-and-the-order-of-the-bump-decides-what-it-costs.md) | **A profile cutover orphans every conversation's thread, and upstream answers `404 not_found` on the dead `previous_response_id`** — settling a case `DESIGN.md` called undefined. The seam scores that as *may have run*, though `interpret_error` already reasons the opposite way for 400/401/403, so the turn holds its room's cursor and recovery replays the same dead pointer. **The composition bump is both prevention and escape and the order decides which**: before the first turn it is invisible, after a wedge it releases the cursor at the cost of one reconciliation, never and the room stays stuck. Drain to zero first — an unresolved turn has no response id, so its replay is a new billed turn |
| [`0055`](0055-the-served-host-predates-the-registry-it-is-read-by.md) | **The served host predates the registry schema it is read by.** Its live registry carries no `org` field, which the current parser refuses as a whole (§B-4), so the distance from this tree is a schema generation rather than a set of names. Read-only over SSH: single-tenant units, one service account, flat state, no attestation, and a 35-byte legacy rollback marker. `publish-one` already migrates this exact case at no cost in members, and replaces that marker while doing it. The path rename this inspection was planning is recorded as abandoned |
| [`0056`](0056-the-served-host-was-never-running-the-pinned-model.md) | **The served host was never running the pinned model.** `model-policy check` reported `selection_enabled=false, default_matches=false, allowed_count=0` — the runtime answered from its own default and the pin, which `MODEL_PIN` calls a behavioural claim about privacy rather than a version string, had never been applied. Corrected and read back exactly. The redeploy that found it also migrated the registry onto the `org` schema with no member spent, and found two defects in units nobody had run: a `ProtectSystem=strict` sandbox that made the lifecycle store unopenable, and a `RuntimeDirectory` systemd deleted the instant the oneshot exited, so the unit reported `Finished` with its output already gone |
| [`0057`](0057-no-tool-call-completes-because-the-name-comes-back-dotted.md) | **No tool call completes, because the name comes back dotted.** The first run of the sandbox block, still **BLOCKED** — but §L-10's "unexercised" now has a cause. IronClaw advertises `builtin__shell`, the model returns `builtin.shell`, the provider-tool-name validator refuses the dot, and the loop burns four model calls before failing `model_invalid_output`. The API shows a client only `internal_error`, so finding it needed the runtime's log level raised. **Not our prompt** — every dotted name was removed and the alloy digest moved to prove it, and the rejection is unchanged. Three defects cleared first, all ours: `start_store` waited 6 s for a store that takes 0.12 s idle and up to 30 s under bring-up contention; the proof composed the shipped **read-only** persona and every turn correctly declined to act; and it had no recovery cycle, which `0044`'s mechanism now supplies and which carried the turn past the 30 s wait into the real failure. One earlier fix was **reverted** for outrunning its measurement |
| [`0058`](0058-the-serving-host-ran-its-own-preflight-from-a-directory-the-login-account-could-write.md) | **The serving host ran its own preflight from a directory the login account could write.** Two active units carried `ExecStartPre=+/opt/ironworks/deploy/check-serving-cell.sh` — systemd's `+` runs it **as root** — from a tree whose 11,142 objects were owned by the unprivileged login account, with `Restart=always` re-executing it unattended. A fail-closed check was the thing running attacker-controlled bytes as root; the same ownership also blocked the artifact preflight, so the vulnerability and the refusal had one cause. Found while preparing an install, not by making it. Also: **a worker image's ID is not a function of the image** — three builds agreed on all eight layer digests, `created`, labels and config, and carried three different IDs, refuting the hypothesis the test was built to confirm; two units were **active but disabled** and two superseded ones still **enabled**, a pair a reboot would have found first; and the guidance path is fixed at publication, so `/opt/rustworks` is pruned to the one file the live registry names. The host went 7/3/0 → **11/0/0**, `model.policy_matches_pin` held across a runtime replacement — the first test of the drift `MODEL_PIN` warns about — and the finding was closed in the same sitting by a new check 11, `privilege.root_execution_paths`, which walks every `ExecStart*=+` path *and its ancestors*, since the exposure lived in the directory rather than the file |
| [`0059`](0059-the-sandbox-works-once-the-model-is-told-the-tools-other-name.md) | **The sandbox works, once the model is told the tool's other name.** `dev/prove-workspace.sh` scores **3 PASS** — the first exercise of IronClaw's persistent per-user workspace, and the end of §L-10's "unexercised". **It passes on a workaround**: name `builtin__shell` in the prompt and the call dispatches; omit it and the runtime refuses its own model's output. Sharpens `0057` D10 in both directions — not a total block, but **not shell-specific either**, since `builtin.time` failed the same way in a turn where the shell had just succeeded, so the fix is per-tool and does not scale. Two probe defects stood behind it, both ours: `docker:27-dind` cannot create the egress network (`gateway_mode_ipv4=isolated`: 27.5.1 refuses it, 28.5.2 and 29.8.0 accept it), and `/data` was mounted into `ic` but not the daemon, so the proxy's config bind resolved to an empty directory. Per-user workspaces live at `sandbox-workspaces/users/<id>/` under the data directory, not under the configured workspace root |
| [`0060`](0060-a-second-quality-pass-and-the-three-recommendations-it-withdrew.md) | **A second quality pass, and the one recommendation it withdrew.** Eight live defects, the worst of which let a turn be filed under the wrong tenant: `row_to_record` decoded nineteen columns by position against a list written twice, with `instance` and `org` adjacent and both `TEXT`. Also `--store --dry-run` applying a confinement, a damaged `schema_version` reading as a fresh store in two of three stores, and `eval` still recovering the published page's figures from terminal output by field position — the technique its reader was rewritten to remove. Six drifted duplications collapsed into three new crates; the workspace's only `#[allow]` is gone. **Two withdrawals did not survive being questioned twice**: the rustdoc lint was reinstated *and made enforced*, finding four broken doc links including one `0048` had recorded as uncatchable, and the escaping property test was extended from one generator to all three. The third withdrawal — a gate table the graders' actual shape does not fit — stands. Two tests were found passing for the wrong reason |
| [`0061`](0061-the-freeze-holds-because-the-other-role-cannot-reach-the-bytes.md) | **The freeze holds, because the other role cannot reach the bytes.** 5 PASS: a Developer member writes a candidate, a QA member **cannot see it**, the seam copies it at a recorded digest, QA reads exactly that digest back, and a QA write never reaches the Developer. The pre-copy read is the check that makes the rest mean anything and is scored three-valued — the sentinel is the evidence, its absence is not, because a `|| echo` fallback cannot tell an absent file from a command that never ran. **This closes §L-7 rather than restating it**: per-member `sandbox-workspaces/users/<id>/` is not custody-backed, so unlike a settings-based tool floor no member can undo it by writing its own settings. Neither member confined and neither spent; no `D_t` or `E_t` exists, so this is the freeze in isolation, not a loop |
| [`0062`](0062-the-live-proofs-found-two-defects-the-whole-suite-could-not.md) | **The first live run after `crates/cli` became a library found two defects the whole suite could not.** An accepted `--call-log` was not created until after the record credential had been checked, so the refusals that matter most left no evidence — and `dev/lib.sh`'s old `creates` answered `0` for a missing file, making the vacuous case indistinguishable from the real one. `dev/prove-cutover.sh` could never pass against a probe: publication requires the runtime's requested-model policy to be the singleton `MODEL_PIN` and a fresh probe is `selection_enabled=false`, which the script neither applied nor declared — so it spent a member, was refused, and scored it FAIL without saying why. Both fixed; 9/9, 6/6 and 4/4. `dev/prove-delivery.sh` never failed across four runs but never came back clean in one: the provider returned `service_unavailable`, which is BLOCKED and is not a pass |
| [`0063`](0063-a-migration-and-its-version-were-two-commits.md) | **A migration and its version were two commits**, so a crash between them locked the journal or the proposal store out for good — and the comments beside both said it rolled back. A source review of the staged tree, no turn spent: `publish-one` re-run after a rollback undid it and reported nothing changed, both backup scripts scored a check that never ran as a corrupt repository, and **the control had seven ways to stay green**, one of which turned a BLOCKED into a FAIL and one of which called twelve rustdoc warnings `clean`. Check 11 widened to every command systemd runs as root and the tree they run from, and the host scored **13 PASS, 0 FAIL, 0 BLOCKED** with all of it |

### Pruned to their findings — transcripts retained

| | measured |
|---|---|
| `0002` | Eval-corpus parity — the first graded Rust output |
| `0003`, `0004`, `0005`, `0006`, `0007`, `0009` | The POC arc, runs 1–8; the cross-account brief is not reproducible |
| `0008`, `0010` | The first stability probes. `attention` 0/5 then 2/5 — a case can carry a PASS in a grade and not be reproducible |
| `0014`, `0015`, `0017` | Lever experiments on the cross-account brief. Each lever moved less than reported, or not at all |
| `0016` | **Half the improvement was the instrument.** Graders frozen before the answers exist, and the two-instrument comparison — the method everything later uses |
| `0018`, `0019` | The lever worked, the book did not; the refusal never moved |
| `0020` | One rule carried every failure. First seven-case grade over the ten-record book |
| `0021` | **Three runs, three different answers** at a composition that never moved — 6/7, 4/7, 3/7 |
| `0022` | `onboarding`'s unbroken record ends. Three answers discharge gate 3 in spellings it cannot reach; *"a stated prerequisite"* read as the STATED tier |
| `0023` | The headline case is the one that fails: `contradiction` asserts a winner on contradictory evidence **2 times in 10**; a REVIEW masked one; verification advice read as closure |
| `0024` | The Python→Rust port was **not verdict-preserving** — 5 of 40 moved, four taking the headline case 6/10 → 2/10 on markdown spacing. Introduced the corpus test |
| `0028` | A held-out test: **9/10 on unseen answers against 10/10 in training** |

## What the arc currently says

Stated here so a reader does not have to reconstruct it from the tables above. Every line is
measured, and names the file that measured it.

- **The suite is unstable at a fixed composition.** Three seven-case runs scored 6/7, 4/7 and 3/7
  with nothing changed between them (`0021`). Every case has both passed and failed.
- **Most recorded failures were the instrument, not the model.** Reading all twenty-two remaining
  failures found seven substantive lapses in fifty-nine answers; the rest were grader defects
  (`0026`).
- **Every grader defect found so far has been corrected**, across `0024`–`0027`, `0029`, `0030`,
  `0034` and `0038`. For a long time **every correction moved a verdict upward**, which `0034` explains
  without appealing to bias: every search read only the answers that *failed*, and a search like
  that can return nothing else. The first read in the other direction found a leniency defect and
  moved a verdict **down**, so the graders are wrong in both directions and the asymmetry is not
  evidence of anything. **`absence_as_negative` alone accounted for six**, the sixth found by ten
  answers nobody had read; the next one turned up in `ownership_default` the moment anyone read the
  answers that passed. That is an argument that another exists, in whichever direction nobody has
  searched yet — and `0038` is that argument collecting: it searched a direction none of the others
  had, **which rules have never graded anything**, and found one that had never been able to.
- **A REVIEW was outranking a decided verdict.** The service-wide prefilter returned on its REVIEW
  rules, so a case grader with an answer — in either direction — never ran (`0029`).
- **The corrections generalise, at least once.** A held-out probe scored 9/10 against 10/10 in
  training, and its one failure was a frame no recorded answer had used (`0028`) — since corrected
  in its own change, with the two-sided controls that run specified (`0030`). That failure is the
  one piece of evidence here that a correction holds on answers it was not written against; every
  other correction has been validated only on the answers that produced it.
- **Where the model does fail it fails in two ways**, both named explicitly in the persona: leading
  with a winner on contradictory evidence, and stating an absence as a bare negative (`0026`).
- **No number here is a pass rate.** One composition, one synthetic book, and probes never designed
  as a sample.

## What the arc cannot say

- **Whether the corrections were directionally right.** Every one was found, judged and validated
  by the same reader. A held-out sample bounds overfitting; it does not bound bias. This wants a
  second reader, and nothing in the repository can supply one.
- **Anything about a real tenant.** The book is ten invented accounts. `docs/ACCOUNT-COORDINATION.md`
  said before any of this began that the next evidence which can move the service is supervised
  dogfooding against real coordination work, not further tuning against an invented book.

---

## Recorded tallies

Every transcript's own verdict count, kept here because the write-ups that used to carry them are
pruned. `crates/claims` reads each `- Recorded:` line against the transcript named above it, and
its non-vacuity guard fails if this section ever empties — a reader that reads nothing passes
everything.

**One transcript is deliberately absent, and it is not an omission.**
`0024-python-to-rust-grader-regrade.json` is a *regrade*: each case carries a `recorded` and a
`regraded` verdict rather than one verdict, so it has no tally of the shape this section holds.
Writing one would mean choosing which of the two readings to call the transcript's own, which is
exactly the disagreement the regrade exists to record.

`0002-eval-parity-run.json`
- Recorded: 10 PASS

`0002-eval-parity-stability.json`
- Recorded: 19 PASS, 1 REVIEW

`0003-account-coordination-poc.json`
- Recorded: 3 PASS, 2 FAIL, 1 BLOCKED

`0004-account-coordination-poc.json`
- Recorded: 3 PASS, 2 FAIL, 1 BLOCKED

`0005-account-coordination-poc.json`
- Recorded: 5 PASS, 1 FAIL

`0006-account-coordination-poc.json`
- Recorded: 4 PASS, 2 FAIL, 1 REVIEW

`0007-account-coordination-poc.json`
- Recorded: 5 PASS, 2 FAIL

`0008-account-coordination-attention-stability.json`
- Recorded: 2 FAIL, 3 REVIEW

`0009-account-coordination-poc.json`
- Recorded: 6 PASS, 1 FAIL

`0010-account-coordination-attention-stability.json`
- Recorded: 2 PASS, 3 FAIL

`0014-account-coordination-poc.json`
- Recorded: 4 PASS, 3 FAIL

`0015-account-coordination-poc.json`
- Recorded: 7 PASS

`0015-attention-stability-2.json`
- Recorded: 3 PASS, 2 FAIL

`0015-attention-stability.json`
- Recorded: 5 PASS

`0016-attention-frozen-graders.json`
- Recorded: 5 PASS, 5 FAIL

`0016-injection-frozen-graders.json`
- Recorded: 5 PASS, 5 FAIL

`0017-attention-absence-as-form.json`
- Recorded: 7 PASS, 3 FAIL

`0017-injection-absence-as-form.json`
- Recorded: 6 PASS, 2 FAIL, 2 BLOCKED

`0018-attention-ten-account-book.json`
- Recorded: 5 PASS, 4 FAIL, 1 BLOCKED

`0019-injection-ten-account-book.json`
- Recorded: 4 PASS, 6 FAIL

`0020-account-coordination-ten-account-book.json`
- Recorded: 4 PASS, 3 FAIL

`0021-account-coordination-corrected-absence-rule.json`
- Recorded: 3 PASS, 4 FAIL

`0022-account-coordination-onboarding-stability.json`
- Recorded: 9 PASS, 1 FAIL

`0022-account-coordination-ownership-default-stability.json`
- Recorded: 4 PASS, 4 FAIL, 2 REVIEW

`0023-account-coordination-contradiction-stability.json`
- Recorded: 6 PASS, 3 FAIL, 1 REVIEW

`0023-account-coordination-offboarding-unknown-stability.json`
- Recorded: 5 PASS, 5 FAIL

`0028-account-coordination-offboarding-held-out.json`
- Recorded: 9 PASS, 1 FAIL

---

## The fixtures under all of it

`fixtures/account-coordination/` — ten synthetic accounts, every domain RFC 2606 `.example`, every
record carrying `_synthetic`. They are invented analogues of coordination failures and never
redactions of a real book. `crates/graders` holds the graders; `crates/eval` runs them and
writes these transcripts, embedding the grader source it used.
