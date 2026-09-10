# 0051 — the probe and the served host resolve authority through different constructors

**2026-09-06.** A source reading of the pinned runtime against this tree. **No measurement was
taken.** No runtime, model key, member, channel, record service or live host was contacted, and
nothing below is a number a run produced. `docs/AUDIT-BRIEF.md` ranks pinned source first among
evidence and `docs/UPGRADE.md` still holds a source reading to *"diff the contracts to know where to
look, then measure"* — so what this record establishes is **where to look**, and every finding names
the measurement that would close it.

Everything upstream was read with `git show 4cb47cfaf3:<path>`; a local checkout six commits behind
the pin was never the source. Upstream anchors carry the `ironclaw@4cb47cfaf3/` prefix and keep
their line numbers, because that tree cannot drift under a citation.

The question was the one `0037` asked — what does the seam never ask for — put to the parts of the
runtime `0037` did not reach: the composition layer, the policy kernel, the operator inspector, and
what upstream ships in `skills/`.

---

## 1. Four boot paths, and only one reads the policy configuration

`SOURCE READING` · generalises `0039` and the profile caveat in §I

`ironclaw serve` resolves its `EffectiveRuntimePolicy` through one of four arms
(`ironclaw@4cb47cfaf3/crates/app/ironclaw_cli/src/runtime/mod.rs:720`):

| booted profile | arm | where the policy comes from |
|---|---|---|
| `production`, `migration-dry-run` | `build_production_services_input` | **`[policy].deployment_mode` + `[policy].default_profile`, from the config file** |
| `hosted-single-tenant` | `build_hosted_single_tenant_services_input` | `hosted_single_tenant_runtime_policy()` — a constant |
| `standalone`, `standalone-unrestricted`, `hosted-single-tenant-volume` | `build_standalone_local_runtime_services_input` | a constant per composition profile |
| **`hosted-single-tenant-volume-sandboxed`**, `…-railway` | `build_sandboxed_local_runtime_services_input` | a constant per composition profile |

Only the first arm reads configuration.
`ironclaw@4cb47cfaf3/crates/app/ironclaw_composition/src/input.rs:1113` fails the build if either
key is absent — *"profile={profile} requires [policy].deployment_mode and [policy].default_profile"* —
and it is reached from exactly one public constructor.

The served arm resolves through
`build_sandboxed_local_runtime_services_input` → `build_standalone_local_runtime_services_input`
→ `local_runtime_build_input` → `hosted_single_tenant_volume_sandboxed_build_input`
(`ironclaw@4cb47cfaf3/crates/app/ironclaw_composition/src/deployment.rs:725`)
→ `hosted_single_tenant_volume_sandboxed_runtime_policy`
(`ironclaw@4cb47cfaf3/crates/app/ironclaw_composition/src/deployment.rs:817`)
→ `DeploymentConfig::hosted_single_tenant_volume_sandboxed`
(`ironclaw@4cb47cfaf3/crates/app/ironclaw_composition/src/deployment.rs:455`), which is literally

```
deployment:         DeploymentMode::HostedMultiTenant
requested_profile:  RuntimeProfile::HostedSafe
org_policy:         OrgPolicyConstraints::default()
```

**Which arm each stack takes.** `dev/docker-compose.probe.yml` sets
`IRONCLAW_REBORN_PROFILE=production`. `deploy/ironworks-ironclaw@.service` sets
`IRONCLAW_REBORN_PROFILE=hosted-single-tenant-volume-sandboxed`. So the stack every `dev/prove-*.sh`
measures and the stack that serves a tenant **do not differ in configuration on this axis — they
run different constructors**, and the served one takes no configuration on it at all.

This is a cause, not a new symptom. Two of its effects are already recorded and neither names it:

- §I of `docs/INVARIANTS.md` closes response isolation by member with *"it was measured on the
  `production` profile rather than the served one."*
- [`0039`](0039-the-catalogue-has-two-axes.md) measured the catalogue's profile axis — 53/52 against
  54/53 — and attributed it to the booted profile without asking what the booted profile selects.

The general statement is stronger than either: **a proof taken on the default probe overlay does not
transfer to the served host on any axis `EffectiveRuntimePolicy` reaches.** `probe-up.sh --sandboxed`
narrows this — it selects the served composition profile, so it runs the served constructor — and
`dev/prove.sh` and the rest do not use it.

**Narrowed, 2026-09-07, and it cuts against this record's own framing.** The transfer bound above is
about *measurements*, and it is correct. But "the confinement baseline is profile-relative" — which
this section leans on and which §L and `0039` also say — is **enforced by nothing**: the policy
fingerprint hashes the *service's* declared ceiling, not the deployment profile, and the
attestation never validates the observed catalogue. What blocks a stale observation crossing a
profile change is member identity, incidentally. `docs/INVARIANTS.md` §I carries the gap. Nothing
here is retracted; what is added is that the property this section treats as load-bearing is a
convention rather than a control.

**Measured, 2026-09-06 —
[`0052`](0052-the-inspector-flattens-the-prompt-it-shows-you.md) finding 3c.** The same harness on
both profiles at the same pin: the composed prompt carries **60 components and 54 capabilities on
the served profile against 58 and 53 on `production`**, the extra one being `builtin.shell`, and
the prompt is **1,893 estimated tokens larger** — about 13% more context per turn than the profile
every other proof measures. So this bullet is no longer only a code path. What did *not* differ:
the alloy, and everything else that harness measures.

**What would close it.** Not a measurement: an assertion. The composition profile is the field that
selects the constructor, and `deploy/check-ironclaw-instance.sh` already reconciles tenant,
`default_owner` and `WEBUI_USER_ID` fail-closed without reconciling it.

## 2. The ceiling §L-6 says does not exist, exists — and nothing can set it

`SOURCE READING` · `UPSTREAM WORK`

`OrgPolicyConstraints`
(`ironclaw@4cb47cfaf3/crates/kernel/ironclaw_runtime_policy/src/resolver.rs:16`) carries
`max_profile`: an org-imposed ceiling the resolver narrows a *requested* profile down to. And
`ironclaw_runtime_policy::resolve`
(`ironclaw@4cb47cfaf3/crates/kernel/ironclaw_runtime_policy/src/resolver.rs:129`) is the **only
sanctioned producer** of an `EffectiveRuntimePolicy` — a value constructed any other way is untrusted
by contract.

That is precisely the shape §L-6 records as absent: an authority ceiling the member cannot undo,
because the member has no path to the resolver at all. `POST /settings/tools` resolves the caller's
own scope; nothing a member can call reaches policy resolution.

**And at the pin nothing sets it.** Outside `ironclaw_runtime_policy/` itself, every occurrence of
`max_profile` and `admin_approves_dedicated_yolo` at `4cb47cfaf3` is in test code —
`ironclaw@4cb47cfaf3/crates/kernel/ironclaw_approvals/src/profile_gate_policy.rs:187` and `:260` sit
inside the `#[cfg(test)] mod tests` that opens at `:150`, and the other two are under `tests/`. Every
production construction is `OrgPolicyConstraints::default()`. There is no config key, no environment
variable and no HTTP route that populates it.

So the mechanism that would convert custody-backed confinement into a ceiling is real, is the right
one, and **its configuration edge is missing upstream.** Under `docs/AUDIT-BRIEF.md`'s
classifications this is `UPSTREAM WORK` — the second such item after session revocation, and a much
smaller one, because the kernel already decides on the value and only the edge that supplies it is
absent.

**The seam-side containment that stays until an official release carries that edge is the one that
is already there:** custody-backed confinement as §L-6 describes it, with §L-9's one-way approval
step and the read-back confirmation in `reconcile::ironclaw::BoundMember::apply`. Nothing about this
finding licenses relaxing any of it, and nothing about it belongs in this repository as code.

## 3. The operator inspector retains four runs

`SOURCE READING`

Four operator-scoped routes exist at the pin
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_webui/src/webui_v2/descriptors.rs:268`, `:270`, `:272`,
`:274`): a run snapshot, **the exact composed prompt**, per-tool activity, and a run event stream —
readable with the operator token rather than the member's.

The prompt route is interesting because of what the seam cannot currently say.
`ironworks_domain::instance::ServiceInstance::alloy_digest` is a claim about what the seam **sent**.
`instructions` is sticky at the pin and IronClaw prepends its own material, so nothing here measures
what the runtime **assembled** from it. A composed-prompt read is the only instrument that could.

**It does not close §I's attribution gap, and the reason is a constant.**
`ironclaw@4cb47cfaf3/crates/contracts/ironclaw_product_contracts/src/inspector.rs:61` sets
`DEFAULT_MAX_RETAINED_RUNS_PER_SESSION = 4`. The host keeps bounded diagnostics for the four most
recent runs of a `(tenant, user, thread)` session and no more; upstream holds a whole architecture
test to that number because a browser window wider than it advertised turns whose snapshot came back
empty. So this is a **probe instrument, never a record**, and §I's *"'did this turn respect its
capability set?' is not a question this architecture can answer"* stands exactly as written.

**What would close it.** One turn against a disposable probe, the composed prompt read back and
compared against the alloy the same turn already reports on its telemetry line — the line
`ironworks_eval::outcome::telemetry` parses. A negative result would matter: it would mean
`alloy_digest` attests to something other than what the model saw. The control has to cut both ways
(§K-8, §F-14) — a comparison that can only report *equal* is the same defect as one that can only
report *differs*, so the run must include a perturbed composition that is required to fail.

## 4. `0037`'s uncalled cancel route is the one that bills

`SOURCE READING` · joins `0036`, `0037` and `DESIGN.md`

Three facts already in this tree, never put together:

- [`0036`](0036-the-demo-question-lands-on-the-thirty-second-cliff.md): the shipped service's
  flagship question sat on the 30s cliff — **eleven of seventeen turns returned 503**.
- `DESIGN.md`: every turn abandoned at the cliff *"currently runs to completion and bills because
  nothing calls it."*
- [`0037`](0037-what-the-seam-never-asked-for.md) check 5:
  `POST /api/webchat/v2/threads/{thread_id}/runs/{run_id}/cancel` **works**, wants a
  `client_action_id`, and is idempotent — while `POST /api/v1/responses/{id}/cancel` returns
  `400 invalid_request` in every state measured.

The seam calls neither, and `ironworks_ironclaw::model::Outcome::Cancelled` is already modelled.
What is missing is the `run_id`, which the seam has no path to: the projection carrying it is the
cursored events endpoint that [`0037`](0037-what-the-seam-never-asked-for.md) check 6 measured and
that `docs/adr/0002-create-then-poll-not-streaming.md`'s pointer explicitly declines to license.

**And the coupling does not hold.** The timeline carries the run id too.
`RebornTimelineResponse.messages` is a `Vec<ThreadMessageRecord>`
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/reborn_services/types.rs:45`), and
`ThreadMessageRecord` has `pub turn_run_id: Option<String>`
(`ironclaw@4cb47cfaf3/crates/domains/ironclaw_threads/src/contract.rs:253`) — serialized, unlike
the neighbouring `tool_result_provider_call`, which is explicitly `skip_serializing` and is what a
field held back from the wire looks like in this struct.

So the run id is reachable from `GET /api/webchat/v2/threads/{thread_id}/timeline`, a route
[`0037`](0037-what-the-seam-never-asked-for.md) already measured at 200 to the owner and 404
cross-member, and **calling the working cancel route does not require adopting the cursored
projection ADR 0002's pointer declines.** That makes it a patch rather than an ADR — which is a
change in what the decision *is*, not permission to make it.

**Confirmed by measurement, 2026-09-06 —
[`0052`](0052-the-inspector-flattens-the-prompt-it-shows-you.md) finding 3, which is WITHDRAWN.**
This paragraph was briefly marked corrected: the first run of `dev/prove-composition.sh` reported
the run id reachable only from the event stream, and that was recorded here as a source reading
that had not survived measurement. **It was the harness searching for `run_id` instead of the
`turn_run_id` this paragraph names correctly one line above.** A later `GET` on the same thread
returned 200 with `turn_run_id` populated on both messages. The reading above stands, it is now
measured rather than inferred, and **the cancel route is a patch and not an ADR.** Both the
retraction and this restoration are dated in place, because a record edited to have been right is
not a record.

**What this does not say.** The field is an `Option`, and nothing here has seen one populated. Which
message kinds carry it, and in particular whether the message a turn refused at the cliff leaves
behind carries one at all, is unmeasured — this is the shape of the response type and not an
observation of a response. `dev/prove-composition.sh` check 2 asks the timeline first for exactly
this reason and reports which projection answered.

No saving is stated as a figure; `0036`'s ratio is the shape of one, and a ratio from seventeen
turns of one question is not a rate.

## 5. The flagship service ships upstream as a first-party skill

`SOURCE READING`

`ironclaw@4cb47cfaf3/skills/` contains `commitment-digest`, `commitment-triage`,
`delegation-tracker` and `decision-capture`. `commitment-digest`'s own frontmatter describes it as
*"Compose and deliver summaries of open commitments, deadlines, pending signals, and resolution
suggestions"*, and its body gathers through `memory_tree` and `memory_read`.

That is `relationship-intelligence@2`'s territory — surfacing commitments and dependencies from
dated records — shipped, first-party, free, and installable. It runs on the `ironclaw.memory.*`
surface that confinement disables and that
[`0035`](0035-the-tool-arm-read-the-records-out-of-memory.md) measured recovering 63 KB of this
tenant's records unprompted.

**This is a correction to a description, not a finding about the code.** `docs/PRD.md` §13 already
concedes *"No prospect has asked for it."* What this adds is narrower and worth stating before a
buyer finds it: **the capability is not the differentiator and cannot be sold as one.** What upstream
does not ship with it is the tenancy boundary, the tool floor, and the evidence trail — and those are
measured here. A pitch that leads with the capability is one search away from being answered by the
runtime it is built on.

---

## The smaller things, recorded and not scheduled

- `GET /api/webchat/v2/admin/users/{user_id}/thread-scrape/threads`, with per-thread and per-run
  artifact routes beneath it — operator-token thread reads that need no member credential. Relevant
  because confinement verification currently requires member custody by construction (§L-6). Worth a
  measurement; not a change.
- `/api/webchat/v2/automations`, with `pause`, `resume` and `run` — the native mechanism if a
  bespoke client asks for scheduled delivery, which is ADR 0010's fourth axis. Out of scope under
  §22, where nothing provisions.
- IronHub's provenance tiers include **Private** — a signed package scoped to one organization, and
  therefore a real distribution channel. Declined here under *pure seam*; recorded so that the
  decline is a decision rather than an omission, in the shape `0037` established.

## What this record does not establish

Anything about behaviour. Every finding above is a reading of source at one revision, and a source
reading goes stale silently — `docs/UPGRADE.md` says so and forbids inferring compatibility from
it. Finding 1 predicts a divergence it did not observe; finding 3 names an instrument it did not
run; finding 4 joins three measurements without taking a fourth. Finding 2 is the only one whose
form is complete, and it is complete because an absence in source *is* the whole claim.
